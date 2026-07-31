//! NVST input-channel binary protocol - the wire format for controller state sent over the
//! `input_channel_v1` WebRTC data channel.

const INPUT_HEARTBEAT: u32 = 2;
const INPUT_GAMEPAD: u32 = 12;
/// Like the mouse, press and release are separate packet types.
const INPUT_KEY_DOWN: u32 = 3;
const INPUT_KEY_UP: u32 = 4;
/// Press and release are separate packet types rather than one type with an action field.
const INPUT_MOUSE_BUTTON_DOWN: u32 = 8;
const INPUT_MOUSE_BUTTON_UP: u32 = 9;
/// Relative movement. The protocol has no absolute-position packet, so a touchscreen has to be
/// driven as a trackpad - see `input::stream_pointer_events`.
const INPUT_MOUSE_MOVE_REL: u32 = 7;

/// Beyond this the server treats the delta as a glitch; the reference client clamps rather than
/// letting a bad reading fling the cursor across the screen.
pub const MAX_MOUSE_DELTA: i16 = 4096;

const WRAPPER_LEGACY_INPUT: u8 = 0x21;
/// Marks a single unframed event - unlike `0x21` it carries no length prefix.
const WRAPPER_SINGLE_INPUT: u8 = 0x22;
const WRAPPER_VERSION_MARKER: u8 = 0x23;
// wrapper byte for partial-reliable gamepad state
const WRAPPER_PARTIAL_GAMEPAD: u8 = 0x26;
const GAMEPAD_PAYLOAD_SIZE: u16 = 26;
const GAMEPAD_INNER_SIZE: u16 = 20;
const GAMEPAD_RESERVED_MARKER: u16 = 85;

/// Bitmap of connected controllers - the Vita is always exactly one, in slot 0.
pub const GAMEPAD_BITMAP_PRIMARY: u16 = 1;

/// `MOD_SHIFT` in the host's modifier bitmap. Anything the Vita's keyboard produces that needs a
/// shifted key on a US layout (capitals, `!`, `?`, ...) is sent with this set.
pub const KEY_MODIFIER_SHIFT: u16 = 0x01;

/// One key press, as the host wants it: a Windows virtual-key code plus the PS/2 set 1 scancode.
///
/// Both are required. Games reading through DirectInput or raw input see the scancode, while
/// anything going through the Windows message queue sees the virtual key, and sending only one of
/// them leaves half of them deaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub keycode: u16,
    pub scancode: u16,
    pub modifiers: u16,
}

impl KeyStroke {
    pub const fn new(keycode: u16, scancode: u16) -> Self {
        Self {
            keycode,
            scancode,
            modifiers: 0,
        }
    }

    pub const fn shifted(self) -> Self {
        Self {
            modifiers: self.modifiers | KEY_MODIFIER_SHIFT,
            ..self
        }
    }
}

/// Keys a game needs that the Vita's text keyboard cannot produce.
pub const KEY_ESCAPE: KeyStroke = KeyStroke::new(0x1B, 0x01);
pub const KEY_ENTER: KeyStroke = KeyStroke::new(0x0D, 0x1C);
pub const KEY_TAB: KeyStroke = KeyStroke::new(0x09, 0x0F);
pub const KEY_BACKSPACE: KeyStroke = KeyStroke::new(0x08, 0x0E);
pub const KEY_SPACE: KeyStroke = KeyStroke::new(0x20, 0x39);
pub const KEY_LEFT_SHIFT: KeyStroke = KeyStroke::new(0xA0, 0x2A);
pub const KEY_LEFT_CTRL: KeyStroke = KeyStroke::new(0xA2, 0x1D);
pub const KEY_LEFT_ALT: KeyStroke = KeyStroke::new(0xA4, 0x38);
pub const KEY_F1: KeyStroke = KeyStroke::new(0x70, 0x3B);
pub const KEY_F2: KeyStroke = KeyStroke::new(0x71, 0x3C);
pub const KEY_F3: KeyStroke = KeyStroke::new(0x72, 0x3D);
pub const KEY_F4: KeyStroke = KeyStroke::new(0x73, 0x3E);

/// Scancodes for the digits `1`..`9`, `0`, in that order - the top number row.
const DIGIT_SCANCODES: [u16; 10] = [
    0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B,
];

/// Scancodes for `a`..`z`, which are famously not in alphabetical order on a PC keyboard.
const LETTER_SCANCODES: [u16; 26] = [
    0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
    0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
];

/// The characters a US layout reaches with Shift, and the unshifted key they sit on.
const SHIFTED_SYMBOLS: [(char, char); 21] = [
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    ('~', '`'),
];

/// Punctuation reachable without Shift, as `(char, virtual key, scancode)`.
const PUNCTUATION: [(char, u16, u16); 11] = [
    ('-', 0xBD, 0x0C),
    ('=', 0xBB, 0x0D),
    ('[', 0xDB, 0x1A),
    (']', 0xDD, 0x1B),
    ('\\', 0xDC, 0x2B),
    (';', 0xBA, 0x27),
    ('\'', 0xDE, 0x28),
    (',', 0xBC, 0x33),
    ('.', 0xBE, 0x34),
    ('/', 0xBF, 0x35),
    ('`', 0xC0, 0x29),
];

/// Maps one character typed on the Vita's keyboard to the key press that produces it on a US
/// layout host.
///
/// Returns `None` for anything outside that layout - accented characters and non-Latin scripts
/// have no single-key equivalent, so they are dropped rather than sent as some wrong key.
pub fn key_for_char(character: char) -> Option<KeyStroke> {
    if character == ' ' {
        return Some(KEY_SPACE);
    }
    if let Some(digit) = character.to_digit(10) {
        let index = digit as usize;
        // '0' sits at the right-hand end of the number row, not the start.
        let scancode = DIGIT_SCANCODES[if index == 0 { 9 } else { index - 1 }];
        return Some(KeyStroke::new(b'0' as u16 + digit as u16, scancode));
    }
    if character.is_ascii_alphabetic() {
        let lower = character.to_ascii_lowercase();
        let index = (lower as u8 - b'a') as usize;
        // The virtual key is always the uppercase letter; case is carried by the Shift modifier.
        let stroke = KeyStroke::new(
            character.to_ascii_uppercase() as u16,
            LETTER_SCANCODES[index],
        );
        return Some(if character.is_ascii_uppercase() {
            stroke.shifted()
        } else {
            stroke
        });
    }
    if let Some((_, base)) = SHIFTED_SYMBOLS
        .iter()
        .find(|(shifted, _)| *shifted == character)
    {
        return key_for_char(*base).map(KeyStroke::shifted);
    }
    PUNCTUATION
        .iter()
        .find(|(punctuation, _, _)| *punctuation == character)
        .map(|(_, keycode, scancode)| KeyStroke::new(*keycode, *scancode))
}

/// One controller snapshot in XInput conventions: bitmask per `XINPUT_GAMEPAD_*`, stick axes
/// -32768..32767 with +Y up, triggers 0-255.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GamepadInput {
    pub controller_id: u8,
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub left_stick_x: i16,
    pub left_stick_y: i16,
    pub right_stick_x: i16,
    pub right_stick_y: i16,
    /// Microseconds on the session clock (time since the peer connected).
    pub timestamp_us: u64,
}

/// The five buttons the legacy input channel can address. The Vita only ever raises `Left` and
/// `Right`, but the wire ids are fixed by the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left = 1,
    Middle = 2,
    Right = 3,
    X1 = 4,
    X2 = 5,
}

/// A pointer event bound for the host desktop, as opposed to the client's own UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    /// Relative movement in host pixels. There is no absolute-position packet in this protocol.
    MoveBy { dx: i16, dy: i16 },
    Button { button: MouseButton, pressed: bool },
}

pub struct InputEncoder {
    protocol_version: u8,
}

impl Default for InputEncoder {
    fn default() -> Self {
        Self {
            protocol_version: 2,
        }
    }
}

impl InputEncoder {
    pub fn set_protocol_version(&mut self, protocol_version: u8) {
        self.protocol_version = protocol_version;
    }

    pub fn protocol_version(&self) -> u8 {
        self.protocol_version
    }

    /// Keepalive the server expects every ~2 seconds once the channel is up.
    pub fn encode_heartbeat(&self) -> Vec<u8> {
        INPUT_HEARTBEAT.to_le_bytes().to_vec()
    }

    pub fn encode_gamepad_state(&self, bitmap: u16, input: GamepadInput) -> Vec<u8> {
        let mut payload = Vec::with_capacity(38);
        payload.extend_from_slice(&INPUT_GAMEPAD.to_le_bytes());
        payload.extend_from_slice(&GAMEPAD_PAYLOAD_SIZE.to_le_bytes());
        payload.extend_from_slice(&(input.controller_id as u16).to_le_bytes());
        payload.extend_from_slice(&bitmap.to_le_bytes());
        payload.extend_from_slice(&GAMEPAD_INNER_SIZE.to_le_bytes());
        payload.extend_from_slice(&input.buttons.to_le_bytes());
        payload.extend_from_slice(
            &(input.left_trigger as u16 | ((input.right_trigger as u16) << 8)).to_le_bytes(),
        );
        payload.extend_from_slice(&input.left_stick_x.to_le_bytes());
        payload.extend_from_slice(&input.left_stick_y.to_le_bytes());
        payload.extend_from_slice(&input.right_stick_x.to_le_bytes());
        payload.extend_from_slice(&input.right_stick_y.to_le_bytes());
        payload.extend_from_slice(&0u16.to_le_bytes());
        payload.extend_from_slice(&GAMEPAD_RESERVED_MARKER.to_le_bytes());
        payload.extend_from_slice(&0u16.to_le_bytes());
        payload.extend_from_slice(&input.timestamp_us.to_le_bytes());
        self.wrap_legacy_input(input.timestamp_us, &payload)
    }

    // partial reliable gamepad frame, 54 bytes total, format copied byte for byte from
    // OpenNOW-Switch: [0x23][u64 BE ts][0x26][u8 ctrl_id][u16 BE seq][0x21][u16 BE len=26][26b payload]
    pub fn encode_gamepad_state_partially_reliable(
        &self,
        bitmap: u16,
        input: GamepadInput,
        sequence: u16,
    ) -> Vec<u8> {
        let mut inner_payload = Vec::with_capacity(26);
        inner_payload.extend_from_slice(&(input.controller_id as u16).to_le_bytes());
        inner_payload.extend_from_slice(&bitmap.to_le_bytes());
        inner_payload.extend_from_slice(&GAMEPAD_INNER_SIZE.to_le_bytes());
        inner_payload.extend_from_slice(&input.buttons.to_le_bytes());
        inner_payload.extend_from_slice(
            &(input.left_trigger as u16 | ((input.right_trigger as u16) << 8)).to_le_bytes(),
        );
        inner_payload.extend_from_slice(&input.left_stick_x.to_le_bytes());
        inner_payload.extend_from_slice(&input.left_stick_y.to_le_bytes());
        inner_payload.extend_from_slice(&input.right_stick_x.to_le_bytes());
        inner_payload.extend_from_slice(&input.right_stick_y.to_le_bytes());
        inner_payload.extend_from_slice(&0u16.to_le_bytes());
        inner_payload.extend_from_slice(&GAMEPAD_RESERVED_MARKER.to_le_bytes());
        inner_payload.extend_from_slice(&0u16.to_le_bytes());

        let mut bytes = Vec::with_capacity(54);
        bytes.push(WRAPPER_VERSION_MARKER); // 0x23
        bytes.extend_from_slice(&input.timestamp_us.to_be_bytes()); // 8 bytes BE
        bytes.push(WRAPPER_PARTIAL_GAMEPAD); // 0x26
        bytes.push(input.controller_id); // 1 byte controller_id
        bytes.extend_from_slice(&sequence.to_be_bytes()); // 2 bytes BE sequence
        bytes.push(WRAPPER_LEGACY_INPUT); // 0x21
        bytes.extend_from_slice(&(inner_payload.len() as u16).to_be_bytes()); // 2 bytes BE len (26)
        bytes.extend_from_slice(&inner_payload); // 26 bytes payload
        bytes
    }

    /// Relative pointer movement, `dx`/`dy` in host pixels. Note the endianness split: the
    /// packet type is little-endian like every other payload here, but the movement fields that
    /// follow are big-endian.
    pub fn encode_mouse_move(&self, dx: i16, dy: i16, timestamp_us: u64) -> Vec<u8> {
        let mut payload = Vec::with_capacity(22);
        payload.extend_from_slice(&INPUT_MOUSE_MOVE_REL.to_le_bytes());
        payload.extend_from_slice(&dx.clamp(-MAX_MOUSE_DELTA, MAX_MOUSE_DELTA).to_be_bytes());
        payload.extend_from_slice(&dy.clamp(-MAX_MOUSE_DELTA, MAX_MOUSE_DELTA).to_be_bytes());
        payload.extend_from_slice(&0u16.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&timestamp_us.to_be_bytes());
        self.wrap_legacy_input(timestamp_us, &payload)
    }

    /// Mouse button press/release. Unlike movement this is framed with the single-input wrapper,
    /// which carries no length prefix - an asymmetry in the protocol, not an oversight here.
    pub fn encode_mouse_button(
        &self,
        button: MouseButton,
        pressed: bool,
        timestamp_us: u64,
    ) -> Vec<u8> {
        let mut payload = Vec::with_capacity(18);
        let packet_type = if pressed {
            INPUT_MOUSE_BUTTON_DOWN
        } else {
            INPUT_MOUSE_BUTTON_UP
        };
        payload.extend_from_slice(&packet_type.to_le_bytes());
        payload.push(button as u8);
        payload.push(0);
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&timestamp_us.to_be_bytes());
        self.wrap_single_input(timestamp_us, &payload)
    }

    /// Keyboard press/release. `keycode` is a Windows virtual-key code and `scancode` a PS/2 set 1
    /// scancode; the host wants both. Framed with the single-input wrapper, like mouse buttons.
    pub fn encode_key(&self, key: KeyStroke, pressed: bool, timestamp_us: u64) -> Vec<u8> {
        let mut payload = Vec::with_capacity(18);
        let packet_type = if pressed { INPUT_KEY_DOWN } else { INPUT_KEY_UP };
        payload.extend_from_slice(&packet_type.to_le_bytes());
        // Type is little-endian, the three fields after it are big-endian - the same split the
        // mouse-move packet has.
        payload.extend_from_slice(&key.keycode.to_be_bytes());
        payload.extend_from_slice(&key.modifiers.to_be_bytes());
        payload.extend_from_slice(&key.scancode.to_be_bytes());
        payload.extend_from_slice(&timestamp_us.to_be_bytes());
        self.wrap_single_input(timestamp_us, &payload)
    }

    /// `[0x23][u64 BE timestamp][0x22][payload]` - no length field, unlike `wrap_legacy_input`.
    fn wrap_single_input(&self, timestamp_us: u64, payload: &[u8]) -> Vec<u8> {
        if self.protocol_version < 3 {
            return payload.to_vec();
        }
        let mut bytes = Vec::with_capacity(10 + payload.len());
        bytes.push(WRAPPER_VERSION_MARKER);
        bytes.extend_from_slice(&timestamp_us.to_be_bytes());
        bytes.push(WRAPPER_SINGLE_INPUT);
        bytes.extend_from_slice(payload);
        bytes
    }

    /// Protocol v3 frames every event as `[0x23][u64 BE timestamp][0x21][u16 BE len][payload]`;
    /// v2 sends the bare payload.
    fn wrap_legacy_input(&self, timestamp_us: u64, payload: &[u8]) -> Vec<u8> {
        if self.protocol_version < 3 {
            return payload.to_vec();
        }

        let mut bytes = Vec::with_capacity(12 + payload.len());
        bytes.push(WRAPPER_VERSION_MARKER);
        bytes.extend_from_slice(&timestamp_us.to_be_bytes());
        bytes.push(WRAPPER_LEGACY_INPUT);
        bytes.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }
}

/// The server's first message on the input channel announces the protocol version: either `[526
/// u16 LE][version u16 LE]` or a leading `0x0e` byte with the version word itself.
pub fn parse_input_handshake_version(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 2 {
        return None;
    }

    let first_word = u16::from_le_bytes([bytes[0], bytes[1]]);
    if first_word == 526 {
        return Some(if bytes.len() >= 4 {
            u16::from_le_bytes([bytes[2], bytes[3]])
        } else {
            2
        });
    }

    if bytes[0] == 0x0e {
        return Some(first_word);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v3() -> InputEncoder {
        let mut encoder = InputEncoder::default();
        encoder.set_protocol_version(3);
        encoder
    }

    /// Byte-for-byte against OpenNOW-Switch's `InputEncodingSelfTest`, which asserts
    /// `BuildKeyboardPayload(ts, 0x41, 0x1e, 0, true)` gives an 18-byte payload with
    /// `[0]==3, [4]==0, [5]==0x41, [8]==0, [9]==0x1e`.
    #[test]
    fn key_payload_matches_the_reference_encoding() {
        let encoder = InputEncoder::default(); // v2: no wrapper, bare payload
        let payload = encoder.encode_key(KeyStroke::new(0x41, 0x1E), true, 0x0102030405060708);
        assert_eq!(payload.len(), 18);
        assert_eq!(payload[0], 3, "key down is packet type 3");
        assert_eq!(&payload[0..4], &3u32.to_le_bytes(), "type is little-endian");
        assert_eq!((payload[4], payload[5]), (0, 0x41), "keycode is big-endian");
        assert_eq!((payload[6], payload[7]), (0, 0), "no modifiers");
        assert_eq!((payload[8], payload[9]), (0, 0x1E), "scancode is big-endian");
        assert_eq!(&payload[10..18], &0x0102030405060708u64.to_be_bytes());
    }

    #[test]
    fn key_release_is_packet_type_four() {
        let encoder = InputEncoder::default();
        let payload = encoder.encode_key(KeyStroke::new(0x41, 0x1E), false, 1);
        assert_eq!(&payload[0..4], &4u32.to_le_bytes());
    }

    /// The same single-input framing mouse buttons use: `[0x23][u64 BE ts][0x22][payload]`, with
    /// no length prefix.
    #[test]
    fn v3_wraps_keys_as_single_input() {
        let payload = v3().encode_key(KeyStroke::new(0x41, 0x1E), true, 0x0102030405060708);
        assert_eq!(payload.len(), 28);
        assert_eq!(payload[0], WRAPPER_VERSION_MARKER);
        assert_eq!(&payload[1..9], &0x0102030405060708u64.to_be_bytes());
        assert_eq!(payload[9], WRAPPER_SINGLE_INPUT);
        assert_eq!(payload[10], 3, "payload follows the wrapper immediately");
    }

    #[test]
    fn lowercase_letters_are_unshifted_and_uppercase_are_shifted() {
        let a = key_for_char('a').expect("'a' should map");
        assert_eq!((a.keycode, a.scancode, a.modifiers), (0x41, 0x1E, 0));
        let upper = key_for_char('A').expect("'A' should map");
        // Same key, distinguished only by the modifier - the virtual key is always uppercase.
        assert_eq!((upper.keycode, upper.scancode), (0x41, 0x1E));
        assert_eq!(upper.modifiers, KEY_MODIFIER_SHIFT);
    }

    /// Scancodes follow the physical PC layout, not the alphabet: 'z' is 0x2C and 'q' is 0x10.
    #[test]
    fn letter_scancodes_follow_the_physical_layout() {
        assert_eq!(key_for_char('q').unwrap().scancode, 0x10);
        assert_eq!(key_for_char('z').unwrap().scancode, 0x2C);
        assert_eq!(key_for_char('m').unwrap().scancode, 0x32);
    }

    /// '0' lives at the right-hand end of the number row, which an index-by-value would get wrong.
    #[test]
    fn digits_map_across_the_number_row() {
        let one = key_for_char('1').unwrap();
        assert_eq!((one.keycode, one.scancode), (0x31, 0x02));
        let nine = key_for_char('9').unwrap();
        assert_eq!((nine.keycode, nine.scancode), (0x39, 0x0A));
        let zero = key_for_char('0').unwrap();
        assert_eq!((zero.keycode, zero.scancode), (0x30, 0x0B));
    }

    #[test]
    fn shifted_symbols_resolve_to_their_base_key_plus_shift() {
        let exclamation = key_for_char('!').expect("'!' should map");
        let one = key_for_char('1').unwrap();
        assert_eq!(exclamation.keycode, one.keycode);
        assert_eq!(exclamation.scancode, one.scancode);
        assert_eq!(exclamation.modifiers, KEY_MODIFIER_SHIFT);

        let question = key_for_char('?').expect("'?' should map");
        let slash = key_for_char('/').unwrap();
        assert_eq!(question.scancode, slash.scancode);
        assert_eq!(question.modifiers, KEY_MODIFIER_SHIFT);
    }

    #[test]
    fn unshifted_punctuation_carries_no_modifier() {
        let period = key_for_char('.').expect("'.' should map");
        assert_eq!((period.keycode, period.scancode, period.modifiers), (0xBE, 0x34, 0));
    }

    #[test]
    fn space_maps_to_the_space_bar() {
        assert_eq!(key_for_char(' '), Some(KEY_SPACE));
    }

    /// Characters with no single-key equivalent on a US layout are dropped rather than sent as
    /// some arbitrary wrong key.
    #[test]
    fn characters_outside_the_layout_are_rejected() {
        assert_eq!(key_for_char('ñ'), None);
        assert_eq!(key_for_char('é'), None);
        assert_eq!(key_for_char('好'), None);
        assert_eq!(key_for_char('€'), None);
    }

    #[test]
    fn every_ascii_printable_character_maps() {
        for byte in 0x20u8..=0x7E {
            let character = byte as char;
            assert!(
                key_for_char(character).is_some(),
                "printable ASCII {character:?} (0x{byte:02X}) has no key mapping"
            );
        }
    }

    #[test]
    fn partially_reliable_gamepad_encoding_matches_expected_structure() {
        let encoder = v3();
        let input = GamepadInput {
            controller_id: 0,
            buttons: 0x0001,
            left_trigger: 100,
            right_trigger: 200,
            left_stick_x: 1000,
            left_stick_y: -1000,
            right_stick_x: 2000,
            right_stick_y: -2000,
            timestamp_us: 0x0102030405060708,
        };
        let payload = encoder.encode_gamepad_state_partially_reliable(GAMEPAD_BITMAP_PRIMARY, input, 42);
        assert_eq!(payload.len(), 54);
        assert_eq!(payload[0], WRAPPER_VERSION_MARKER); // 0x23
        assert_eq!(&payload[1..9], &0x0102030405060708u64.to_be_bytes());
        assert_eq!(payload[9], WRAPPER_PARTIAL_GAMEPAD); // 0x26
        assert_eq!(payload[10], 0); // controller_id
        assert_eq!(&payload[11..13], &42u16.to_be_bytes()); // sequence
        assert_eq!(payload[13], WRAPPER_LEGACY_INPUT); // 0x21
        assert_eq!(&payload[14..16], &26u16.to_be_bytes()); // inner len 26
    }
}
