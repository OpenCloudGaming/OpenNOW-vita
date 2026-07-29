#[link(name = "SDL2", kind = "static")]
unsafe extern "C" {}

mod egui_painter;
mod surface;

use crate::app::ui::build_ui;
use crate::app::{App, AppState};
use crate::input::{
    front_touch_position, gamepad_snapshot, held_menu_direction, map_controller_button_event,
    map_keyboard_event, map_pointer_event, open_first_controller,
    register_vita_controller_mapping, RearTouchTriggers, StreamTouchState,
};
use crate::streaming::audio::AudioRenderer;
use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use surface::{HEIGHT, VitaSurface, WIDTH};
use tokio::time::sleep;

/// Scales `pixels_per_point` up so the UI reads legibly on the Vita's small screen.
const UI_SCALE: f32 = 1.3;
const DIRECTION_REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(350);
const DIRECTION_REPEAT_INTERVAL: Duration = Duration::from_millis(90);

pub(crate) const TARGET_FRAME_TIME: Duration = Duration::from_millis(16);
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(4);


/// Where the render loop's time goes, for the on-screen readout.
///
/// The loop only sleeps when it comes in under `TARGET_FRAME_TIME`; when it overruns it runs flat
/// out, which pegs a core. Knowing *which* phase overran is the difference between fixing it and
/// guessing at it.
pub(crate) mod render_stats {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    pub(crate) static FRAME_US: AtomicU32 = AtomicU32::new(0);
    pub(crate) static UI_US: AtomicU32 = AtomicU32::new(0);
    pub(crate) static PAINT_US: AtomicU32 = AtomicU32::new(0);
    /// Frames that missed the deadline, so the loop never got to sleep.
    pub(crate) static OVER_BUDGET: AtomicU64 = AtomicU64::new(0);

    /// Exponential smoothing, so the readout is legible instead of flickering every frame.
    pub(crate) fn record(slot: &AtomicU32, sample_us: u32) {
        let previous = slot.load(Ordering::Relaxed);
        let smoothed = if previous == 0 {
            sample_us
        } else {
            (previous * 7 + sample_us) / 8
        };
        slot.store(smoothed, Ordering::Relaxed);
    }

    pub(crate) fn line() -> String {
        format!(
            "cpu frame:{:.1}ms ui:{:.1}ms paint:{:.1}ms over:{}",
            FRAME_US.load(Ordering::Relaxed) as f32 / 1000.0,
            UI_US.load(Ordering::Relaxed) as f32 / 1000.0,
            PAINT_US.load(Ordering::Relaxed) as f32 / 1000.0,
            OVER_BUDGET.load(Ordering::Relaxed),
        )
    }
}

pub async fn run(mut app: App) -> Result<()> {
    let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
    let video = sdl.video().map_err(anyhow::Error::msg)?;
    let audio = sdl.audio().map_err(anyhow::Error::msg)?;
    register_vita_controller_mapping(&sdl).map_err(anyhow::Error::msg)?;
    let game_controller_subsystem = sdl.game_controller().map_err(anyhow::Error::msg)?;
    let mut controller = open_first_controller(&game_controller_subsystem);
    let mut event_pump = sdl.event_pump().map_err(anyhow::Error::msg)?;
    let mut surface = VitaSurface::new(&video)?;
    let _audio_renderer =
        AudioRenderer::new(&audio).context("failed to set up audio renderer")?;
    let egui_ctx = egui::Context::default();
    crate::app::ui::apply_theme(&egui_ctx);
    let start_time = Instant::now();
    let mut pointer_pos = egui::Pos2::ZERO;
    let mut held_direction = None;
    let mut held_direction_since = Instant::now();
    let mut last_direction_repeat_at = Instant::now();
    let mut text_input_active = false;
    let mut touch_owned_by_ui = false;
    let mut touch_owned_by_stick_zone = false;
    let mut stream_touch = StreamTouchState::default();
    let mut rear_touch = RearTouchTriggers::default();
    let mut stick_zones = crate::input::FrontStickZones::default();
    let mut was_streaming = false;
    // Reactive repainting outside a session: the catalog is static between interactions, so
    // re-running and re-tessellating egui 60 times a second burns a whole core to redraw an
    // identical screen. Streaming always repaints - the video changes every frame.
    let mut last_painted_at = Instant::now();
    let mut needs_repaint = true;

    loop {
        let loop_started_at = Instant::now();
        let mut egui_events = Vec::new();
        let mut direct_commands = Vec::new();
        let mut stream_mouse_events = Vec::new();
        // While a session is live the touchscreen belongs to the game rather than to the client
        // UI - except for the "Stop session" button, whose rect the UI publishes each frame.
        // Without that carve-out the button is on screen but unreachable, leaving no way out of a
        // running game.
        // Any client-owned modal has to take the touchscreen back, or its buttons are drawn over
        // the game but every tap goes to the game's mouse instead - which is exactly how the
        // controls hint ended up with a "Got it" that did nothing.
        let touch_drives_stream = matches!(app.state, AppState::Streaming { .. })
            && !app.confirm_exit
            && !app.show_controls_hint
            && !app.show_controls_modal;
        let stream_ui_rects: Vec<egui::Rect> = crate::app::ui::stream_ui_rects(&egui_ctx)
            .into_iter()
            // Fingertips are wider than a button's hit box.
            .map(|rect| rect.expand(8.0))
            .collect();
        let screen_points = (WIDTH as f32 / UI_SCALE, HEIGHT as f32 / UI_SCALE);
        // Touch deltas are normalized 0..1, so they scale by the streamed frame's own size to
        // land as host pixels: a drag across the whole panel moves the cursor across the whole
        // remote screen.
        let stream_size = (WIDTH as f32, HEIGHT as f32);

        for event in event_pump.poll_iter() {
            if let Some(command) = map_keyboard_event(&event)
                && !direct_commands.contains(&command)
            {
                direct_commands.push(command);
            }
            if let Some(command) = map_controller_button_event(&event)
                && !direct_commands.contains(&command)
            {
                direct_commands.push(command);
            }
            rear_touch.handle(&event);
            stick_zones.handle(&event);
            // Ownership of a touch is decided once, on finger-down, and the rest of the gesture
            // follows it. Deciding per-event would let a drag that starts on the game and ends on
            // the button swallow the mouse-up, leaving the host holding the button down.
            if let Some(pos) = front_touch_position(&event, screen_points)
                && matches!(event, sdl2::event::Event::FingerDown { .. })
            {
                touch_owned_by_ui = stream_ui_rects.iter().any(|rect| rect.contains(pos));
            }
            // Decided on finger-down like the rest, and checked *after* the client's own UI: the
            // overlay buttons sit at the top and the zones at the bottom, but the order makes the
            // precedence explicit rather than accidental.
            if let sdl2::event::Event::FingerDown { touch_id, x, y, .. } = event
                && touch_id == crate::input::FRONT_TOUCH_DEVICE_ID
            {
                touch_owned_by_stick_zone = !touch_owned_by_ui
                    && crate::gfn::stream_prefs::stick_zones().is_active()
                    && crate::input::is_in_stick_zone(x, y);
                crate::input::stick_zone_stats::record_touch_owned(touch_owned_by_stick_zone);
            }

            if touch_owned_by_stick_zone {
                // Already fed to `stick_zones` above; it must not also drive the host cursor.
            } else if touch_drives_stream && !touch_owned_by_ui && app.mouse_trackpad_enabled {
                stream_mouse_events.extend(stream_touch.map(&event, stream_size));
            } else if let Some(egui_event) =
                map_pointer_event(&event, screen_points, UI_SCALE, &mut pointer_pos)
            {
                egui_events.push(egui_event);
            }
            match event {
                sdl2::event::Event::TextInput { ref text, .. } => {
                    egui_events.push(egui::Event::Text(text.clone()));
                }
                sdl2::event::Event::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Backspace),
                    repeat,
                    ..
                } => {
                    egui_events.push(egui::Event::Key {
                        key: egui::Key::Backspace,
                        physical_key: None,
                        pressed: true,
                        repeat,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                sdl2::event::Event::ControllerDeviceAdded { .. } if controller.is_none() => {
                    controller = open_first_controller(&game_controller_subsystem);
                }
                sdl2::event::Event::ControllerDeviceRemoved { .. } => {
                    controller = None;
                }
                _ => {}
            }
        }

        match held_menu_direction(controller.as_ref()) {
            Some(direction) if held_direction == Some(direction) => {
                if held_direction_since.elapsed() >= DIRECTION_REPEAT_INITIAL_DELAY
                    && last_direction_repeat_at.elapsed() >= DIRECTION_REPEAT_INTERVAL
                {
                    direct_commands.push(direction.into());
                    last_direction_repeat_at = Instant::now();
                }
            }
            Some(direction) => {
                direct_commands.push(direction.into());
                held_direction = Some(direction);
                held_direction_since = Instant::now();
                last_direction_repeat_at = Instant::now();
            }
            None => held_direction = None,
        }

        let had_direct_commands = !direct_commands.is_empty();
        for command in direct_commands {
            app.handle_command(command).await?;
        }
        app.tick().await?;

        let show_video = {
            let streaming_peer = match &app.state {
                AppState::Streaming { peer, .. } => Some(peer),
                _ => None,
            };
            // Picked up on entry to a session rather than per frame: the setting lives on the
            // memory card and this runs 60 times a second.
            if streaming_peer.is_some() != was_streaming {
                was_streaming = streaming_peer.is_some();
            }
            if was_streaming {
                rear_touch.reload_intensity();
                stick_zones.reload_enabled();
            }
            surface.sync_video_frame(streaming_peer)?;
            if let (Some(peer), Some(active_controller)) = (streaming_peer, controller.as_ref()) {
                peer.send_gamepad(gamepad_snapshot(active_controller, &rear_touch, &stick_zones));
                crate::input::stick_zone_stats::record_clicks(
                    stick_zones.left_stick_click(),
                    stick_zones.right_stick_click(),
                );
            }
            if let Some(peer) = streaming_peer {
                for mouse_event in stream_mouse_events.drain(..) {
                    peer.send_mouse(mouse_event);
                }
            }
            // Audio no longer passes through here at all: the peer thread hands packets to the
            // decode worker as they arrive, so playback is not paced by the video frame rate.
            streaming_peer.is_some_and(|peer| peer.video_frame().is_some())
        };

        let search_requested = matches!(
            &app.state,
            AppState::Catalog {
                search_requested: true,
                ..
            }
        );
        // Deliberately *not* keyed on the in-game keyboard: SDL's text input on Vita is itself an
        // IME dialog (`sceImeDialogInit`), and libime refuses to have that up at the same time as
        // the inline `sceImeOpen` the in-game keyboard uses. Starting both is what crashes the
        // firmware with C2-12828-1.
        if search_requested && !text_input_active {
            video.text_input().start();
            text_input_active = true;
        } else if !search_requested && text_input_active {
            video.text_input().stop();
            text_input_active = false;
        }

        let had_egui_events = !egui_events.is_empty();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(WIDTH as f32 / UI_SCALE, HEIGHT as f32 / UI_SCALE),
            )),
            viewport_id: egui::ViewportId::ROOT,
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(UI_SCALE),
                    ..Default::default()
                },
            ))
            .collect(),
            time: Some(start_time.elapsed().as_secs_f64()),
            predicted_dt: TARGET_FRAME_TIME.as_secs_f32(),
            events: egui_events,
            ..Default::default()
        };

        // App state changes behind egui's back (a finished job, a new status line), so a floor
        // keeps the screen honest without polling at frame rate. 100 ms is imperceptible on a menu
        // and still ~6x less work than repainting every frame.
        const IDLE_REPAINT_FLOOR: Duration = Duration::from_millis(100);
        let repaint_now = show_video
            || needs_repaint
            || had_egui_events
            || had_direct_commands
            || last_painted_at.elapsed() >= IDLE_REPAINT_FLOOR;
        if !repaint_now {
            let frame_deadline = loop_started_at + TARGET_FRAME_TIME;
            while Instant::now() < frame_deadline {
                let remaining = frame_deadline.saturating_duration_since(Instant::now());
                sleep(remaining.min(INPUT_POLL_INTERVAL)).await;
            }
            continue;
        }

        let ui_started_at = Instant::now();
        let mut ui_commands = Vec::new();
        let full_output = egui_ctx.run(raw_input, |ctx| {
            ui_commands = build_ui(ctx, &app);
        });
        // egui asks for another frame while anything is animating - a spinner, a fade, a hover.
        needs_repaint = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|viewport| viewport.repaint_delay.is_zero());
        last_painted_at = Instant::now();

        for command in ui_commands {
            if command == crate::input::AppCommand::RightClick {
                use crate::gfn::input_protocol::{MouseButton as StreamMouseButton, MouseEvent};
                stream_mouse_events.push(MouseEvent::Button {
                    button: StreamMouseButton::Right,
                    pressed: true,
                });
                stream_mouse_events.push(MouseEvent::Button {
                    button: StreamMouseButton::Right,
                    pressed: false,
                });
            }
            app.handle_command(command).await?;
        }

        let clipped_primitives =
            egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        render_stats::record(
            &render_stats::UI_US,
            ui_started_at.elapsed().as_micros() as u32,
        );

        let paint_started_at = Instant::now();
        surface.draw_scene(show_video)?;
        surface.paint_egui(
            full_output.pixels_per_point,
            &clipped_primitives,
            &full_output.textures_delta,
        )?;
        render_stats::record(
            &render_stats::PAINT_US,
            paint_started_at.elapsed().as_micros() as u32,
        );
        render_stats::record(
            &render_stats::FRAME_US,
            loop_started_at.elapsed().as_micros() as u32,
        );

        let frame_deadline = loop_started_at + TARGET_FRAME_TIME;
        if Instant::now() >= frame_deadline {
            render_stats::OVER_BUDGET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if Instant::now() < frame_deadline {
            while Instant::now() < frame_deadline {
                let remaining = frame_deadline.saturating_duration_since(Instant::now());
                sleep(remaining.min(INPUT_POLL_INTERVAL)).await;
                if Instant::now() >= frame_deadline {
                    break;
                }
                event_pump.pump_events();
                if let (AppState::Streaming { peer, .. }, Some(active_controller)) =
                    (&app.state, controller.as_ref())
                {
                    peer.send_gamepad(gamepad_snapshot(active_controller, &rear_touch, &stick_zones));
                crate::input::stick_zone_stats::record_clicks(
                    stick_zones.left_stick_click(),
                    stick_zones.right_stick_click(),
                );
                }
            }
        } else {
            tokio::task::yield_now().await;
        }
    }
}
