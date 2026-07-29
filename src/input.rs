//! Input mapping: SDL2 events (keyboard, Vita controller, touch) -> app-level commands.
//!
//! The Vita controller mapping GUID string and the front/rear touch device ids below are
//! adapted from `green-vita` (MPL-2.0, https://github.com/Day-OS/green-vita) - see
//! `THIRD_PARTY_NOTICES.md`. They encode non-obvious platform quirks (SDL's built-in Vita
//! controller driver does not ship a default `SDL_GameControllerDB` mapping, and VitaSDK's
//! `SDL_vitatouch.c` registers the front/back touch panels as SDL touch devices 1 and 2, in
//! that order) that are not documented anywhere else and are cheaper to keep than to
//! rediscover.

use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton;

/// Directional/confirm navigation, independent of what screen is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputCommand {
    Back,
    Confirm,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
}

/// Top-level command enum the shell feeds into `App::handle_command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    Input(InputCommand),
    SetSearchQuery(String),
    /// Ask the shell to open the platform text-input method (SDL IME / on-screen keyboard).
    RequestSearch,
    /// Close/submit search input and stop the platform text-input method.
    CloseSearch,
    ToggleConfirmExit,
    CancelConfirmExit,
    ConfirmExitSession,
    /// Emitted by the catalog screen's language picker.
    SetLocale(crate::locale::Locale),
    /// Emitted by the catalog screen's sort picker.
    SetSort(crate::app::CatalogSort),
    /// Emitted by the stream-quality section of the account popup.
    SetStreamFps(crate::gfn::stream_prefs::StreamFps),
    /// Emitted by the rear-trigger section of the account popup.
    SetTriggerIntensity(crate::gfn::stream_prefs::TriggerIntensity),
    /// Closes the first-run controls explainer for good.
    DismissControlsHint,
    /// Stars or unstars a game from its row in the library.
    ToggleFavorite(String),
    /// Shows or hides the streaming diagnostics panel.
    ToggleStreamStats,
    /// Shows or hides the in-game keyboard.
    ToggleKeyboard,
    /// A key tapped on the streaming overlay's special-key row.
    SendKey(crate::gfn::input_protocol::KeyStroke),
    /// Emitted by the stick-zone section of the settings modal.
    SetStickZones(crate::gfn::stream_prefs::StickZones),
    /// Emitted by the volume-boost section of the account popup.
    SetAudioBoost(crate::gfn::stream_prefs::AudioBoost),
    /// Emitted when a row in the catalog list is tapped/clicked.
    SelectGame(usize),
    /// Toggles the streaming toolbar between expanded and collapsed.
    ToggleToolbar,
    /// Emits a momentary right-click to the host.
    RightClick,
    /// Toggles the in-stream controls configuration modal (L2/R2 and L3/R3).
    ToggleControlsModal,
    /// Toggles front touch trackpad host mouse input on and off.
    ToggleMouseTrackpad,
}

impl From<InputCommand> for AppCommand {
    fn from(command: InputCommand) -> Self {
        AppCommand::Input(command)
    }
}

/// The front touchscreen's own SDL touch device id (registered second on the Vita's touch
/// backend, `SDL_vitatouch.c`: `SDL_AddTouch(1, ..., "Front")`, `SDL_AddTouch(2, ..., "Back")`).
pub const FRONT_TOUCH_DEVICE_ID: i64 = 1;

/// The rear touch panel, registered right after the front one by `SDL_vitatouch.c`.
const REAR_TOUCH_DEVICE_ID: i64 = 2;



pub fn map_keyboard_event(event: &Event) -> Option<AppCommand> {
    let Event::KeyDown {
        keycode: Some(key),
        repeat: false,
        ..
    } = event
    else {
        return None;
    };
    let command = match *key {
        Keycode::Escape => InputCommand::Back,
        Keycode::Return => InputCommand::Confirm,
        Keycode::Up => InputCommand::MoveUp,
        Keycode::Down => InputCommand::MoveDown,
        Keycode::Left => InputCommand::MoveLeft,
        Keycode::Right => InputCommand::MoveRight,
        _ => return None,
    };
    Some(command.into())
}

pub fn map_controller_button_event(event: &Event) -> Option<AppCommand> {
    match event {
        Event::ControllerButtonDown {
            button: Button::B, ..
        } => Some(InputCommand::Back.into()),
        Event::ControllerButtonDown {
            button: Button::A, ..
        } => Some(InputCommand::Confirm.into()),
        _ => None,
    }
}

const MENU_STICK_DEADZONE: f32 = 0.5;

/// Polled every frame (rather than event-driven) so holding a direction auto-repeats; see the
/// repeat-delay/interval constants in `shell::run`.
pub fn held_menu_direction(controller: Option<&GameController>) -> Option<InputCommand> {
    let controller = controller?;
    if controller.button(Button::DPadUp) {
        return Some(InputCommand::MoveUp);
    }
    if controller.button(Button::DPadDown) {
        return Some(InputCommand::MoveDown);
    }
    if controller.button(Button::DPadLeft) {
        return Some(InputCommand::MoveLeft);
    }
    if controller.button(Button::DPadRight) {
        return Some(InputCommand::MoveRight);
    }
    let x = axis_to_f32(controller.axis(Axis::LeftX));
    let y = axis_to_f32(controller.axis(Axis::LeftY));
    if y.abs() >= x.abs() {
        match y {
            y if y <= -MENU_STICK_DEADZONE => Some(InputCommand::MoveUp),
            y if y >= MENU_STICK_DEADZONE => Some(InputCommand::MoveDown),
            _ => None,
        }
    } else {
        match x {
            x if x <= -MENU_STICK_DEADZONE => Some(InputCommand::MoveLeft),
            x if x >= MENU_STICK_DEADZONE => Some(InputCommand::MoveRight),
            _ => None,
        }
    }
}

fn axis_to_f32(raw: i16) -> f32 {
    raw as f32 / i16::MAX as f32
}

/// SDL's built-in Vita controller driver reports raw joystick buttons/axes but does not ship a
/// `SDL_GameControllerDB` mapping for the Vita's own front controls, so `GameControllerSubsystem`
/// would otherwise never recognize it as a "game controller".
pub fn register_vita_controller_mapping(sdl: &sdl2::Sdl) -> Result<(), String> {
    let joystick_subsystem = sdl.joystick()?;
    if joystick_subsystem
        .num_joysticks()
        .map_err(|e| e.to_string())?
        == 0
    {
        return Ok(());
    }
    let guid = joystick_subsystem
        .device_guid(0)
        .map_err(|e| e.to_string())?;
    let mapping = format!(
        "{guid},PSVita Controller,\
         a:b2,b:b1,x:b3,y:b0,\
         back:b10,start:b11,\
         leftshoulder:b4,rightshoulder:b5,\
         leftstick:b14,rightstick:b15,\
         dpup:b8,dpdown:b6,dpleft:b7,dpright:b9,\
         leftx:a0,lefty:a1,rightx:a2,righty:a3,\
         lefttrigger:b12,righttrigger:b13,platform:PS Vita,"
    );
    sdl.game_controller()?
        .add_mapping(&mapping)
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn open_first_controller(subsystem: &sdl2::GameControllerSubsystem) -> Option<GameController> {
    let available = subsystem.num_joysticks().ok()?;
    (0..available).find_map(|id| {
        if !subsystem.is_game_controller(id) {
            return None;
        }
        subsystem.open(id).ok()
    })
}

/// Forwards mouse/front-touch input to egui.
pub fn map_pointer_event(
    event: &Event,
    screen_size: (f32, f32),
    pixels_per_point: f32,
    pointer_pos: &mut egui::Pos2,
) -> Option<egui::Event> {
    match *event {
        Event::MouseMotion { x, y, .. } => {
            *pointer_pos = mouse_to_screen_pos(x, y, pixels_per_point);
            Some(egui::Event::PointerMoved(*pointer_pos))
        }
        Event::MouseButtonDown {
            mouse_btn, x, y, ..
        } => Some(pointer_button_at(
            pointer_pos,
            mouse_to_screen_pos(x, y, pixels_per_point),
            map_mouse_button(mouse_btn),
            true,
        )),
        Event::MouseButtonUp {
            mouse_btn, x, y, ..
        } => Some(pointer_button_at(
            pointer_pos,
            mouse_to_screen_pos(x, y, pixels_per_point),
            map_mouse_button(mouse_btn),
            false,
        )),
        Event::FingerDown { touch_id, x, y, .. } if touch_id == FRONT_TOUCH_DEVICE_ID => {
            Some(pointer_button_at(
                pointer_pos,
                touch_to_screen_pos(x, y, screen_size),
                egui::PointerButton::Primary,
                true,
            ))
        }
        Event::FingerMotion { touch_id, x, y, .. } if touch_id == FRONT_TOUCH_DEVICE_ID => {
            *pointer_pos = touch_to_screen_pos(x, y, screen_size);
            Some(egui::Event::PointerMoved(*pointer_pos))
        }
        Event::FingerUp { touch_id, x, y, .. } if touch_id == FRONT_TOUCH_DEVICE_ID => {
            Some(pointer_button_at(
                pointer_pos,
                touch_to_screen_pos(x, y, screen_size),
                egui::PointerButton::Primary,
                false,
            ))
        }
        _ => None,
    }
}

/// Front-touch -> host mouse, for the streaming screen.
///
/// The NVST input protocol has no absolute-position packet, only relative deltas, so the
/// touchscreen is driven as a trackpad rather than as a direct-touch pointer: dragging a finger
/// moves the cursor by the distance dragged, and a short tap that barely moves is a left click.
/// Rear touch is deliberately left alone - it sits under the player's fingers during normal play.
#[derive(Default)]
pub struct StreamTouchState {
    /// Where the finger was on the previous event, in normalized 0..1 touch coordinates.
    last: Option<(f32, f32)>,
    /// Accumulated travel since finger-down, to tell a tap from a drag.
    travel: f32,
    down_at: Option<std::time::Instant>,
}

/// How far (in normalized touch units) a finger may travel and still count as a tap.
const TAP_MAX_TRAVEL: f32 = 0.03;
/// How long a contact may last and still count as a tap.
const TAP_MAX_DURATION: std::time::Duration = std::time::Duration::from_millis(300);

impl StreamTouchState {
    /// Translates one SDL event into the host-mouse events it implies, if any.
    pub fn map(
        &mut self,
        event: &Event,
        stream_size: (f32, f32),
    ) -> Vec<crate::gfn::input_protocol::MouseEvent> {
        use crate::gfn::input_protocol::{MouseButton as StreamMouseButton, MouseEvent};

        let mut out = Vec::new();
        match *event {
            Event::FingerDown { touch_id, x, y, .. } if touch_id == FRONT_TOUCH_DEVICE_ID => {
                self.last = Some((x, y));
                self.travel = 0.0;
                self.down_at = Some(std::time::Instant::now());
            }
            Event::FingerMotion { touch_id, x, y, .. } if touch_id == FRONT_TOUCH_DEVICE_ID => {
                if let Some((prev_x, prev_y)) = self.last {
                    let (dx, dy) = (x - prev_x, y - prev_y);
                    self.travel += (dx * dx + dy * dy).sqrt();
                    // Normalized touch delta scaled into host pixels.
                    let move_x = (dx * stream_size.0).round() as i32;
                    let move_y = (dy * stream_size.1).round() as i32;
                    if move_x != 0 || move_y != 0 {
                        out.push(MouseEvent::MoveBy {
                            dx: move_x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                            dy: move_y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                        });
                    }
                }
                self.last = Some((x, y));
            }
            Event::FingerUp { touch_id, .. } if touch_id == FRONT_TOUCH_DEVICE_ID => {
                let tapped = self.travel <= TAP_MAX_TRAVEL
                    && self
                        .down_at
                        .is_some_and(|at| at.elapsed() <= TAP_MAX_DURATION);
                if tapped {
                    out.push(MouseEvent::Button {
                        button: StreamMouseButton::Left,
                        pressed: true,
                    });
                    out.push(MouseEvent::Button {
                        button: StreamMouseButton::Left,
                        pressed: false,
                    });
                }
                self.last = None;
                self.travel = 0.0;
                self.down_at = None;
            }
            _ => {}
        }
        out
    }
}

/// Where a front-touch event landed, in egui points - used to decide whether a touch belongs to
/// the client's own on-screen controls or to the game behind them.
pub fn front_touch_position(event: &Event, screen_size: (f32, f32)) -> Option<egui::Pos2> {
    match *event {
        Event::FingerDown { touch_id, x, y, .. }
        | Event::FingerMotion { touch_id, x, y, .. }
        | Event::FingerUp { touch_id, x, y, .. }
            if touch_id == FRONT_TOUCH_DEVICE_ID =>
        {
            Some(touch_to_screen_pos(x, y, screen_size))
        }
        _ => None,
    }
}

fn pointer_button_at(
    pointer_pos: &mut egui::Pos2,
    pos: egui::Pos2,
    button: egui::PointerButton,
    pressed: bool,
) -> egui::Event {
    *pointer_pos = pos;
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::default(),
    }
}

fn mouse_to_screen_pos(x: i32, y: i32, pixels_per_point: f32) -> egui::Pos2 {
    egui::pos2(x as f32 / pixels_per_point, y as f32 / pixels_per_point)
}

fn touch_to_screen_pos(x: f32, y: f32, (width, height): (f32, f32)) -> egui::Pos2 {
    egui::pos2(x * width, y * height)
}

fn map_mouse_button(button: MouseButton) -> egui::PointerButton {
    match button {
        MouseButton::Right => egui::PointerButton::Secondary,
        MouseButton::Middle => egui::PointerButton::Middle,
        _ => egui::PointerButton::Primary,
    }
}

/// The Vita has no L2/R2. Its shoulder buttons are L1/R1, and the controller mapping's
/// `lefttrigger:b12,righttrigger:b13` only resolves on a Vita TV with a DualShock attached - on a
/// handheld those buttons do not exist, so the triggers were simply unreachable and any game
/// bound to them was unplayable.
///


/// Live diagnostics for the front stick zones, so a "L3/R3 doesn't work" report can be checked
/// against what the touch router actually decided instead of guessed at from source.
pub mod stick_zone_stats {
    use std::sync::atomic::{AtomicBool, Ordering};

    static TOUCH_OWNED_BY_ZONE: AtomicBool = AtomicBool::new(false);
    static LEFT_ACTIVE: AtomicBool = AtomicBool::new(false);
    static RIGHT_ACTIVE: AtomicBool = AtomicBool::new(false);

    pub(crate) fn record_touch_owned(owned: bool) {
        TOUCH_OWNED_BY_ZONE.store(owned, Ordering::Relaxed);
    }

    pub(crate) fn record_clicks(left: bool, right: bool) {
        LEFT_ACTIVE.store(left, Ordering::Relaxed);
        RIGHT_ACTIVE.store(right, Ordering::Relaxed);
    }

    pub fn line() -> String {
        format!(
            "inp zones:{} touch_owned:{} l3:{} r3:{}",
            crate::gfn::stream_prefs::stick_zones().debug_label(),
            u8::from(TOUCH_OWNED_BY_ZONE.load(Ordering::Relaxed)),
            u8::from(LEFT_ACTIVE.load(Ordering::Relaxed)),
            u8::from(RIGHT_ACTIVE.load(Ordering::Relaxed)),
        )
    }
}

/// The front screen's bottom corners, standing in for the stick clicks the Vita has no buttons for.
///
/// The rear panel was tried first and gave the whole panel away to four ~1.2 cm zones with nothing
/// to tell them apart by feel. The front screen has room: two corners the thumb already rests near,
/// with the middle of the screen - where the game is - left alone.
#[derive(Default)]
pub struct FrontStickZones {
    /// Touch positions by SDL finger id, normalized 0..1. Multi-touch, so both clicks can be held
    /// at once and releasing one must not cancel the other.
    fingers: Vec<(i64, f32, f32)>,
    enabled: bool,
}

/// Where the zones sit, normalized 0..1. Bottom third, outer quarter of each side.
pub const STICK_ZONE_TOP: f32 = 0.66;
pub const STICK_ZONE_WIDTH: f32 = 0.25;

/// Whether a normalized position falls in one of the zones.
fn in_stick_zone(x: f32, y: f32, left: bool) -> bool {
    y >= STICK_ZONE_TOP
        && if left {
            x < STICK_ZONE_WIDTH
        } else {
            x >= 1.0 - STICK_ZONE_WIDTH
        }
}

/// Whether a normalized position falls in *either* zone - what the touch router asks before
/// handing a gesture to the host's mouse.
pub fn is_in_stick_zone(x: f32, y: f32) -> bool {
    in_stick_zone(x, y, true) || in_stick_zone(x, y, false)
}

impl FrontStickZones {
    pub fn handle(&mut self, event: &Event) {
        match *event {
            Event::FingerDown {
                touch_id,
                finger_id,
                x,
                y,
                ..
            }
            | Event::FingerMotion {
                touch_id,
                finger_id,
                x,
                y,
                ..
            } if touch_id == FRONT_TOUCH_DEVICE_ID => {
                match self.fingers.iter_mut().find(|(id, _, _)| *id == finger_id) {
                    Some(slot) => {
                        slot.1 = x;
                        slot.2 = y;
                    }
                    None => self.fingers.push((finger_id, x, y)),
                }
            }
            Event::FingerUp {
                touch_id,
                finger_id,
                ..
            } if touch_id == FRONT_TOUCH_DEVICE_ID => {
                self.fingers.retain(|(id, _, _)| *id != finger_id);
            }
            _ => {}
        }
    }

    /// Picks up a changed setting; call when a session starts, alongside `reload_intensity`.
    pub fn reload_enabled(&mut self) {
        self.enabled = crate::gfn::stream_prefs::stick_zones().is_active();
        if !self.enabled {
            self.fingers.clear();
        }
    }

    fn held(&self, left: bool) -> bool {
        self.enabled
            && self
                .fingers
                .iter()
                .any(|(_, x, y)| in_stick_zone(*x, *y, left))
    }

    /// L3, from the bottom-left corner of the screen.
    pub fn left_stick_click(&self) -> bool {
        self.held(true)
    }

    /// R3, from the bottom-right corner.
    pub fn right_stick_click(&self) -> bool {
        self.held(false)
    }
}

/// The rear touch panel stands in, split down the middle: left half is L2, right half is R2.
///
/// It was briefly split into quadrants to fit L3/R3 in as well, but the panel is only ~2.5 cm tall
/// and has no landmark to tell top from bottom by feel, so a quadrant was ~1.2 cm of guesswork and
/// reaching for L2 could give L3. The stick clicks moved to the front screen instead.
#[derive(Default)]
pub struct RearTouchTriggers {
    /// Touch x positions by SDL finger id. The panel is multi-touch, so both triggers can be held
    /// at once - a vector rather than a single slot.
    fingers: Vec<(i64, f32)>,
    /// How hard a touch presses, read from the player's setting. Cached rather than read per
    /// frame because this is polled 60 times a second and the setting lives on the memory card.
    intensity: u8,
}

impl RearTouchTriggers {
    pub fn handle(&mut self, event: &Event) {
        match *event {
            Event::FingerDown {
                touch_id,
                finger_id,
                x,
                ..
            }
            | Event::FingerMotion {
                touch_id,
                finger_id,
                x,
                ..
            } if touch_id == REAR_TOUCH_DEVICE_ID => {
                match self.fingers.iter_mut().find(|(id, _)| *id == finger_id) {
                    Some(slot) => slot.1 = x,
                    None => self.fingers.push((finger_id, x)),
                }
            }
            Event::FingerUp {
                touch_id,
                finger_id,
                ..
            } if touch_id == REAR_TOUCH_DEVICE_ID => {
                self.fingers.retain(|(id, _)| *id != finger_id);
            }
            _ => {}
        }
    }

    fn held(&self, left_half: bool) -> bool {
        self.fingers
            .iter()
            .any(|(_, x)| if left_half { *x < 0.5 } else { *x >= 0.5 })
    }

    /// Picks up a changed intensity setting; call when a session starts.
    pub fn reload_intensity(&mut self) {
        self.intensity = crate::gfn::stream_prefs::trigger_intensity().value();
    }

    fn pressure(&self) -> u8 {
        if self.intensity == 0 {
            crate::gfn::stream_prefs::TriggerIntensity::default().value()
        } else {
            self.intensity
        }
    }

    fn left_trigger(&self) -> u8 {
        if self.held(true) { self.pressure() } else { 0 }
    }

    fn right_trigger(&self) -> u8 {
        if self.held(false) { self.pressure() } else { 0 }
    }
}

/// Full controller snapshot for the streaming session, in XInput conventions (the format the NVST
/// input channel speaks - see `gfn::input_protocol`).
pub fn gamepad_snapshot(
    controller: &GameController,
    rear_touch: &RearTouchTriggers,
    stick_zones: &FrontStickZones,
) -> crate::gfn::input_protocol::GamepadInput {
    const DPAD_UP: u16 = 0x0001;
    const DPAD_DOWN: u16 = 0x0002;
    const DPAD_LEFT: u16 = 0x0004;
    const DPAD_RIGHT: u16 = 0x0008;
    const START: u16 = 0x0010;
    const BACK: u16 = 0x0020;
    const LEFT_THUMB: u16 = 0x0040;
    const RIGHT_THUMB: u16 = 0x0080;
    const LEFT_SHOULDER: u16 = 0x0100;
    const RIGHT_SHOULDER: u16 = 0x0200;
    const A: u16 = 0x1000;
    const B: u16 = 0x2000;
    const X: u16 = 0x4000;
    const Y: u16 = 0x8000;

    let mut buttons = 0u16;
    let mut set = |button: Button, mask: u16| {
        if controller.button(button) {
            buttons |= mask;
        }
    };
    set(Button::DPadUp, DPAD_UP);
    set(Button::DPadDown, DPAD_DOWN);
    set(Button::DPadLeft, DPAD_LEFT);
    set(Button::DPadRight, DPAD_RIGHT);
    set(Button::Start, START);
    set(Button::Back, BACK);
    set(Button::LeftStick, LEFT_THUMB);
    set(Button::RightStick, RIGHT_THUMB);
    set(Button::LeftShoulder, LEFT_SHOULDER);
    set(Button::RightShoulder, RIGHT_SHOULDER);
    set(Button::A, A);
    set(Button::B, B);
    set(Button::X, X);
    set(Button::Y, Y);

    // SDL never raises the stick clicks on a handheld Vita - there are none - so the front
    // screen's bottom corners stand in.
    if stick_zones.left_stick_click() {
        buttons |= LEFT_THUMB;
    }
    if stick_zones.right_stick_click() {
        buttons |= RIGHT_THUMB;
    }

    let axis = |axis: Axis| controller.axis(axis);
    let trigger = |value: i16| (value.max(0) / 129).min(255) as u8;

    crate::gfn::input_protocol::GamepadInput {
        controller_id: 0,
        buttons,
        // Whichever source is pressing harder wins, so an attached DualShock on a Vita TV still
        // works while the rear panel covers the handheld.
        left_trigger: trigger(axis(Axis::TriggerLeft)).max(rear_touch.left_trigger()),
        right_trigger: trigger(axis(Axis::TriggerRight)).max(rear_touch.right_trigger()),
        left_stick_x: axis(Axis::LeftX),
        left_stick_y: axis(Axis::LeftY).saturating_neg(),
        right_stick_x: axis(Axis::RightX),
        right_stick_y: axis(Axis::RightY).saturating_neg(),
        timestamp_us: 0,
    }
}

#[cfg(test)]
mod stick_zone_tests {
    use super::*;

    #[test]
    fn the_bottom_corners_map_to_their_own_side() {
        // Bottom-left is L3 and nothing else; bottom-right is R3.
        assert!(in_stick_zone(0.05, 0.95, true));
        assert!(!in_stick_zone(0.05, 0.95, false));
        assert!(in_stick_zone(0.95, 0.95, false));
        assert!(!in_stick_zone(0.95, 0.95, true));
    }

    /// The middle of the screen is where the game is - it has to stay mouse.
    #[test]
    fn the_centre_of_the_screen_is_not_a_zone() {
        assert!(!is_in_stick_zone(0.5, 0.5));
        assert!(!is_in_stick_zone(0.5, 0.95), "bottom centre is still mouse");
        assert!(!is_in_stick_zone(0.05, 0.4), "left edge but too high");
    }

    #[test]
    fn the_zones_only_cover_the_bottom_third() {
        assert!(!is_in_stick_zone(0.05, STICK_ZONE_TOP - 0.01));
        assert!(is_in_stick_zone(0.05, STICK_ZONE_TOP + 0.01));
    }
}
