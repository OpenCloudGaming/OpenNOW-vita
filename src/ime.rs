//! In-game keyboard, built on the Vita's inline IME (`sceImeOpen`).
//!
//! The inline IME does not report key presses - it reports *edits to a text buffer*. So key
//! presses have to be inferred, using the technique vita-moonlight arrived at
//! (`src/keyboardsystem.c`): keep a buffer of filler characters with the caret parked in the
//! middle, and read each edit's shape to work out which key caused it.
//!
//! Parking the caret at index 1 of a 3-cell filler buffer makes every possible edit distinguishable:
//!
//! | IME event        | caret | key it must have been |
//! |------------------|-------|-----------------------|
//! | `UPDATE_TEXT`    | 1     | the character now in the buffer |
//! | `UPDATE_TEXT`    | 0     | Backspace (it ate the filler to the left) |
//! | `UPDATE_CARET`   | 0     | Left arrow |
//! | `UPDATE_CARET`   | 2     | Right arrow |
//! | `PRESS_ENTER`    | -     | Enter |
//!
//! After every key the buffer and caret are reset, so the next keystroke starts from the same
//! known state. Without that reset the second keypress would be read against a buffer that the
//! first one had already moved, and the inference falls apart.
//!
//! Two libime rules shape the structure here, and breaking either one takes the firmware down with
//! C2-12828-1 rather than returning an error:
//!
//! - `sceImeOpen`, `sceImeUpdate`, `sceImeSetText`, `sceImeSetCaret` and `sceImeClose` all belong
//!   to one thread. Everything below runs on the shell loop, which is where `open` and `update` are
//!   called from.
//! - The event handler must not call back into libime. So the reset is not done in the handler; it
//!   raises [`RECENTER_PENDING`] and [`update`] performs it, which is the same split moonlight uses
//!   with its `forzar_centro` flag.

use crate::gfn::input_protocol::{
    KEY_BACKSPACE, KEY_ENTER, KeyStroke, key_for_char,
};
use std::sync::Mutex;
use vitasdk_sys::*;

/// Virtual-key codes for the arrows, which `key_for_char` has no character to map from.
const KEY_LEFT: KeyStroke = KeyStroke::new(0x25, 0x4B);
const KEY_RIGHT: KeyStroke = KeyStroke::new(0x27, 0x4D);

/// What `sceImeParamInit` would stamp into `sdkVersion`. It is a `static inline` in
/// `psp2/libime.h`, so there is no symbol to link against and the value is inlined here instead.
/// libime rejects a version it does not recognise.
const PSP2_SDK_VERSION: SceUInt32 = 0x0357_0011;

/// Filler cell. Not a printable character, so a real keystroke is always distinguishable from the
/// padding around it.
const FILLER: SceWChar16 = 1;
/// Three filler cells plus a terminator. Three is the minimum that leaves a cell on each side of
/// the parked caret, which is what makes left and right movement tell themselves apart.
const BUFFER_LEN: usize = 4;
/// The buffers are over-allocated past `BUFFER_LEN`: libime writes up to `maxTextLength` cells and
/// is not documented as counting the terminator, so the slack keeps a miscount off the next static.
const BUFFER_CAPACITY: usize = 8;
/// Where the caret is re-parked after every keystroke.
const CARET_HOME: u32 = 1;

/// Keystrokes detected since the last drain. The IME hands us events on its own callback, so this
/// is the only way across to the shell loop.
static PENDING_KEYS: Mutex<Vec<KeyStroke>> = Mutex::new(Vec::new());

#[repr(align(64))]
struct TextBuf([SceWChar16; BUFFER_CAPACITY]);

/// What the IME is seeded with. Kept separate from [`INPUT_BUFFER`] because libime holds both
/// pointers for the lifetime of the session, and pointing them at one buffer has it reading the
/// text it is concurrently writing.
static mut INITIAL_TEXT: TextBuf = TextBuf([FILLER, FILLER, FILLER, 0, 0, 0, 0, 0]);
/// Where the IME writes the edited text, and what the handler reads back.
static mut INPUT_BUFFER: TextBuf = TextBuf([FILLER, FILLER, FILLER, 0, 0, 0, 0, 0]);

/// Scratch space the IME requires us to own for as long as it is open.
#[repr(align(64))]
struct WorkBuf([u8; SCE_IME_WORK_BUFFER_SIZE as usize]);
static mut WORK_BUFFER: WorkBuf = WorkBuf([0; SCE_IME_WORK_BUFFER_SIZE as usize]);

/// Whether the IME is currently open, so `update` knows to pump it and `open` stays idempotent.
static OPEN: Mutex<bool> = Mutex::new(false);
/// Raised by the handler once it has read a keystroke; [`update`] does the actual reset on the
/// owning thread. See the module docs for why the handler cannot do it itself.
static RECENTER_PENDING: Mutex<bool> = Mutex::new(false);
/// The IME emits a spurious delete event immediately after opening. Swallowing it stops the
/// keyboard from sending a phantom Backspace to the game every time it is summoned.
static IGNORE_FIRST_DELETE: Mutex<bool> = Mutex::new(false);

fn queue(key: KeyStroke) {
    if let Ok(mut pending) = PENDING_KEYS.lock() {
        pending.push(key);
    }
}

/// Asks [`update`] to restore the filler buffer and re-park the caret, so the next edit is measured
/// from a known starting point.
fn request_recenter() {
    if let Ok(mut pending) = RECENTER_PENDING.lock() {
        *pending = true;
    }
}

/// Restores the filler buffer and re-parks the caret.
///
/// # Safety
/// Must run on the thread that called `sceImeOpen`, while the IME is open.
unsafe fn recenter() {
    unsafe {
        for buffer in [&raw mut INITIAL_TEXT.0, &raw mut INPUT_BUFFER.0] {
            (*buffer)[0] = FILLER;
            (*buffer)[1] = FILLER;
            (*buffer)[2] = FILLER;
            (*buffer)[3] = 0;
        }

        // Text before caret: the caret index is only meaningful against the text libime has just
        // been handed.
        sceImeSetText((&raw const INITIAL_TEXT.0).cast(), BUFFER_LEN as u32);

        let mut caret: SceImeCaret = core::mem::zeroed();
        caret.index = CARET_HOME;
        sceImeSetCaret(&caret);
    }
}

/// The character the user just typed, or `None` if the buffer holds only filler.
///
/// # Safety
/// Only called from the IME handler, where `INPUT_BUFFER` is not being written by anyone else.
unsafe fn typed_character() -> Option<char> {
    unsafe {
        let buffer = &raw const INPUT_BUFFER.0;
        (0..BUFFER_LEN)
            .map(|index| (*buffer)[index])
            .find(|cell| *cell != 0 && *cell != FILLER)
            .and_then(|cell| char::from_u32(u32::from(cell)))
    }
}

/// Called by the IME for every edit the user makes.
///
/// Must not call into libime - see the module docs.
unsafe extern "C" fn on_ime_event(_arg: *mut core::ffi::c_void, event: *const SceImeEventData) {
    let Some(event) = (unsafe { event.as_ref() }) else {
        return;
    };
    // SAFETY: `caretIndex` is the active union member for the events this reads; `rect` and
    // `text` belong to CHANGE_SIZE and the preedit path, which are not handled here.
    let caret = unsafe { event.param.caretIndex };

    match event.id {
        SCE_IME_EVENT_UPDATE_TEXT => {
            let character = unsafe { typed_character() };

            // Opening the keyboard fires a delete with nothing in the buffer. Sending Backspace
            // for it would silently eat a character in whatever the game had focused.
            if character.is_none()
                && caret == 0
                && IGNORE_FIRST_DELETE
                    .lock()
                    .map(|mut flag| std::mem::replace(&mut *flag, false))
                    .unwrap_or(false)
            {
                request_recenter();
                return;
            }

            match character {
                // A real character landed where the caret was parked.
                Some(character) => {
                    if let Some(key) = key_for_char(character) {
                        queue(key);
                    }
                    // Anything `key_for_char` rejects has no key on a US layout, so it is dropped
                    // rather than sent as some arbitrary wrong key.
                }
                // Nothing new, and the caret moved left: the filler to the left was deleted.
                None if caret == 0 => queue(KEY_BACKSPACE),
                None => {}
            }
            request_recenter();
        }
        SCE_IME_EVENT_UPDATE_CARET => {
            // The caret started at CARET_HOME, so which side it landed on is the arrow pressed.
            match caret {
                0 => queue(KEY_LEFT),
                2 => queue(KEY_RIGHT),
                _ => {}
            }
            request_recenter();
        }
        SCE_IME_EVENT_PRESS_ENTER => {
            queue(KEY_ENTER);
            request_recenter();
        }
        SCE_IME_EVENT_PRESS_CLOSE => {
            // Closing is the one libime call the handler is allowed to make, and the next
            // `sceImeUpdate` reporting failure is how `update` learns the session is gone.
            unsafe { sceImeClose() };
        }
        _ => {}
    }
}

/// Whether the in-game keyboard is currently showing.
pub fn is_open() -> bool {
    OPEN.lock().map(|open| *open).unwrap_or(false)
}

/// Shows the in-game keyboard. Does nothing if it is already up.
///
/// Must be called from the same thread as [`update`] - see the module docs.
pub fn open() -> bool {
    match OPEN.lock() {
        Ok(open) if *open => return true,
        Ok(_) => {}
        Err(_) => return false,
    }

    if let Ok(mut flag) = IGNORE_FIRST_DELETE.lock() {
        *flag = true;
    }
    if let Ok(mut pending) = RECENTER_PENDING.lock() {
        *pending = false;
    }

    // SAFETY: the IME is not open, so nothing else is touching the buffers, and they are statics -
    // libime borrows them until `sceImeClose`.
    let result = unsafe {
        // libime lives in a loadable module. Calling into its stubs before this lands is a jump
        // through an unresolved import, which is the crash rather than an error code.
        sceSysmoduleLoadModule(SCE_SYSMODULE_IME);

        for buffer in [&raw mut INITIAL_TEXT.0, &raw mut INPUT_BUFFER.0] {
            (*buffer)[0] = FILLER;
            (*buffer)[1] = FILLER;
            (*buffer)[2] = FILLER;
            (*buffer)[3] = 0;
        }

        // Equivalent to `sceImeParamInit`, which is header-only.
        let mut param: SceImeParam = core::mem::zeroed();
        param.sdkVersion = PSP2_SDK_VERSION;
        param.supportedLanguages = SCE_IME_LANGUAGE_ENGLISH.into();
        param.languagesForced = SCE_TRUE as SceBool;
        param.type_ = SCE_IME_TYPE_DEFAULT;
        param.option = SCE_IME_OPTION_NO_ASSISTANCE;
        param.work = (&raw mut WORK_BUFFER.0).cast();
        param.arg = core::ptr::null_mut();
        param.handler = Some(on_ime_event);
        param.filter = None;
        param.initialText = (&raw mut INITIAL_TEXT.0).cast();
        param.maxTextLength = BUFFER_LEN as u32;
        param.inputTextBuffer = (&raw mut INPUT_BUFFER.0).cast();
        param.enterLabel = SCE_IME_ENTER_LABEL_DEFAULT as SceUChar8;

        let result = sceImeOpen(&param);
        if result >= 0 {
            // Only safe once the session exists, and it must happen before the first update so the
            // caret starts parked where the inference expects it.
            recenter();
        }
        result
    };

    if result < 0 {
        eprintln!("Could not open the in-game keyboard: 0x{result:08X}");
        return false;
    }
    if let Ok(mut open) = OPEN.lock() {
        *open = true;
    }
    true
}

/// Hides the in-game keyboard.
pub fn close() {
    let was_open = match OPEN.lock() {
        Ok(mut open) => std::mem::replace(&mut *open, false),
        Err(_) => return,
    };
    if was_open {
        unsafe { sceImeClose() };
    }
    if let Ok(mut pending) = PENDING_KEYS.lock() {
        pending.clear();
    }
}

/// Pumps the IME so it can deliver events. Must be called every frame while the keyboard is up,
/// from the thread that called [`open`].
pub fn update() {
    if !is_open() {
        return;
    }

    let recenter_now = RECENTER_PENDING
        .lock()
        .map(|mut pending| std::mem::replace(&mut *pending, false))
        .unwrap_or(false);

    // SAFETY: the session is open and this is its owning thread.
    let status = unsafe {
        if recenter_now {
            recenter();
        }
        sceImeUpdate()
    };

    // The user dismissed the keyboard, so the session is already gone. `close` would call
    // `sceImeClose` on it a second time.
    if status < 0 {
        if let Ok(mut open) = OPEN.lock() {
            *open = false;
        }
        if let Ok(mut pending) = PENDING_KEYS.lock() {
            pending.clear();
        }
    }
}

/// Takes the keystrokes detected since the last call.
pub fn take_keys() -> Vec<KeyStroke> {
    PENDING_KEYS
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default()
}
