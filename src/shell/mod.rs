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
use surface::{FramePaintStats, HEIGHT, VitaSurface, WIDTH};

/// Scales `pixels_per_point` up so the UI reads legibly on the Vita's small screen.
const UI_SCALE: f32 = 1.3;
const DIRECTION_REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(200);
const DIRECTION_REPEAT_INTERVAL: Duration = Duration::from_millis(70);

pub(crate) const TARGET_FRAME_TIME: Duration = Duration::from_millis(16);

const FRAME_STATS_INTERVAL: Duration = Duration::from_secs(2);
const SLOW_FRAME_THRESHOLD: Duration = Duration::from_millis(20);
const LONG_GAP_THRESHOLD: Duration = Duration::from_millis(25);

pub(crate) mod render_stats {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    pub(crate) static FRAME_US: AtomicU32 = AtomicU32::new(0);
    pub(crate) static UI_US: AtomicU32 = AtomicU32::new(0);
    pub(crate) static PAINT_US: AtomicU32 = AtomicU32::new(0);
    pub(crate) static PRESENT_US: AtomicU32 = AtomicU32::new(0);
    pub(crate) static DRAW_CALLS: AtomicU32 = AtomicU32::new(0);
    pub(crate) static OVER_BUDGET: AtomicU64 = AtomicU64::new(0);

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
            "cpu frame:{:.1}ms ui:{:.1}ms paint:{:.1}ms present:{:.1}ms draws:{} over:{}",
            FRAME_US.load(Ordering::Relaxed) as f32 / 1000.0,
            UI_US.load(Ordering::Relaxed) as f32 / 1000.0,
            PAINT_US.load(Ordering::Relaxed) as f32 / 1000.0,
            PRESENT_US.load(Ordering::Relaxed) as f32 / 1000.0,
            DRAW_CALLS.load(Ordering::Relaxed),
            OVER_BUDGET.load(Ordering::Relaxed),
        )
    }
}

#[derive(Default)]
struct FrameStats {
    window_started_at: Option<Instant>,
    frames: u32,
    tick: Duration,
    build_ui: Duration,
    tessellate: Duration,
    texture_apply: Duration,
    geometry: Duration,
    present: Duration,
    draw_calls: u64,
    textures_uploaded: u64,
    vertices_drawn: u64,
    iterations: u32,
    last_painted_at: Option<Instant>,
    max_gap: Duration,
    long_gaps: u32,
    pending_log: Vec<String>,
}

impl FrameStats {
    fn note_iteration(&mut self) {
        self.iterations += 1;
        self.window_started_at.get_or_insert_with(Instant::now);
    }

    fn record(
        &mut self,
        tick: Duration,
        build_ui: Duration,
        tessellate: Duration,
        paint: FramePaintStats,
    ) {
        let now = Instant::now();
        let texture_apply = Duration::from_secs_f64(paint.texture_apply_secs);
        let geometry = Duration::from_secs_f64(paint.geometry_secs);
        let present = Duration::from_secs_f64(paint.present_secs);
        self.frames += 1;
        self.tick += tick;
        self.build_ui += build_ui;
        self.tessellate += tessellate;
        self.texture_apply += texture_apply;
        self.geometry += geometry;
        self.present += present;
        self.draw_calls += paint.draw_calls as u64;
        self.textures_uploaded += paint.textures_uploaded as u64;
        self.vertices_drawn += paint.vertices_drawn as u64;
        let paint_total = texture_apply + geometry + present;
        let total = tick + build_ui + tessellate + paint_total;
        if let Some(previous) = self.last_painted_at {
            let gap = now.duration_since(previous);
            self.max_gap = self.max_gap.max(gap);
            if gap > LONG_GAP_THRESHOLD {
                self.long_gaps += 1;
                self.pending_log.push(format!(
                    "long gap: {:.1}ms since last painted frame (work={:.1}ms elsewhere={:.1}ms)",
                    gap.as_secs_f64() * 1000.0,
                    total.as_secs_f64() * 1000.0,
                    gap.saturating_sub(total).as_secs_f64() * 1000.0,
                ));
            }
        }
        self.last_painted_at = Some(now);
        if total > SLOW_FRAME_THRESHOLD {
            self.pending_log.push(format!(
                "slow frame: tick={:.1}ms build_ui={:.1}ms tessellate={:.1}ms paint={:.1}ms \
                 (texture_apply={:.1}ms×{} geometry={:.1}ms×{}draws/{}verts present={:.1}ms) total={:.1}ms",
                tick.as_secs_f64() * 1000.0,
                build_ui.as_secs_f64() * 1000.0,
                tessellate.as_secs_f64() * 1000.0,
                paint_total.as_secs_f64() * 1000.0,
                texture_apply.as_secs_f64() * 1000.0,
                paint.textures_uploaded,
                geometry.as_secs_f64() * 1000.0,
                paint.draw_calls,
                paint.vertices_drawn,
                present.as_secs_f64() * 1000.0,
                total.as_secs_f64() * 1000.0,
            ));
        }

        let ui_us = (build_ui + tessellate).as_micros() as u32;
        let paint_us = (texture_apply + geometry).as_micros() as u32;
        let present_us = present.as_micros() as u32;
        let frame_us = total.as_micros() as u32;
        render_stats::record(&render_stats::UI_US, ui_us);
        render_stats::record(&render_stats::PAINT_US, paint_us);
        render_stats::record(&render_stats::PRESENT_US, present_us);
        render_stats::record(&render_stats::FRAME_US, frame_us);
        render_stats::DRAW_CALLS.store(paint.draw_calls, std::sync::atomic::Ordering::Relaxed);
        if tick + build_ui + tessellate + texture_apply + geometry >= TARGET_FRAME_TIME {
            render_stats::OVER_BUDGET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn maybe_flush(&mut self) {
        let Some(window_started_at) = self.window_started_at else {
            return;
        };
        let elapsed = window_started_at.elapsed();
        if elapsed < FRAME_STATS_INTERVAL {
            return;
        }
        let seconds = elapsed.as_secs_f64();
        let frames = self.frames.max(1) as f64;
        self.pending_log.push(format!(
            "frame stats ({:.1}s): {} painted ({:.1} fps) · {} iterations · \
             worst frame gap {:.0}ms, {} over {}ms",
            seconds,
            self.frames,
            self.frames as f64 / seconds,
            self.iterations,
            self.max_gap.as_secs_f64() * 1000.0,
            self.long_gaps,
            LONG_GAP_THRESHOLD.as_millis(),
        ));
        if self.frames > 0 {
            self.pending_log.push(format!(
                "  avg per painted frame: tick={:.2}ms build_ui={:.2}ms tessellate={:.2}ms \
                 texture_apply={:.2}ms ({:.1} uploads) geometry={:.2}ms ({:.1} draws, {:.0} verts) present={:.2}ms",
                self.tick.as_secs_f64() * 1000.0 / frames,
                self.build_ui.as_secs_f64() * 1000.0 / frames,
                self.tessellate.as_secs_f64() * 1000.0 / frames,
                self.texture_apply.as_secs_f64() * 1000.0 / frames,
                self.textures_uploaded as f64 / frames,
                self.geometry.as_secs_f64() * 1000.0 / frames,
                self.draw_calls as f64 / frames,
                self.vertices_drawn as f64 / frames,
                self.present.as_secs_f64() * 1000.0 / frames,
            ));
        }
        crate::logger::write_frame_stats(&self.pending_log.join("\n"));
        let last_painted_at = self.last_painted_at;
        *self = FrameStats {
            last_painted_at,
            ..FrameStats::default()
        };
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
    crate::app::fonts::configure(&egui_ctx);
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
    let mut frame_stats = FrameStats::default();
    crate::logger::reset_frame_stats_log();
    crate::logger::write_frame_stats("=== OpenNOW-vita frame stats — new session ===");

    loop {
        let loop_started_at = Instant::now();
        frame_stats.note_iteration();
        frame_stats.maybe_flush();
        let mut egui_events = Vec::new();
        let mut direct_commands = Vec::new();
        let mut stream_mouse_events = Vec::new();
        let touch_drives_stream =
            matches!(app.state, AppState::Streaming { .. }) && !app.ui_owns_touch();
        let screen_points = (WIDTH as f32 / UI_SCALE, HEIGHT as f32 / UI_SCALE);
        let mut stream_ui_rects: Vec<egui::Rect> = crate::app::ui::stream_ui_rects(&egui_ctx)
            .into_iter()
            // Fingertips are wider than a button's hit box.
            .map(|rect| rect.expand(8.0))
            .collect();
        if app.keyboard_open {
            let screen = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(screen_points.0, screen_points.1),
            );
            stream_ui_rects.push(crate::app::ui::keyboard_panel_rect(screen));
        }
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
                touch_owned_by_stick_zone = touch_drives_stream
                    && !touch_owned_by_ui
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

        for command in direct_commands {
            app.handle_command(command).await?;
        }
        let tick_started_at = Instant::now();
        app.tick().await?;
        let tick_elapsed = tick_started_at.elapsed();

        let show_video = {
            let streaming_peer = match &app.state {
                AppState::Streaming { peer, .. } => Some(peer),
                _ => None,
            };
            // Picked up on entry to a session rather than per frame: the setting lives on the
            // memory card and this runs 60 times a second.
            if streaming_peer.is_some() != was_streaming {
                was_streaming = streaming_peer.is_some();
                if was_streaming {
                    rear_touch.reload_intensity();
                    stick_zones.reload_enabled();
                }
            }
            let latest_video = streaming_peer.and_then(|peer| peer.video_frame());
            surface.sync_video_frame(streaming_peer, latest_video.as_ref())?;
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
            latest_video.is_some()
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

        let build_ui_started_at = Instant::now();
        let mut ui_commands = Vec::new();
        let full_output = egui_ctx.run(raw_input, |ctx| {
            ui_commands = build_ui(ctx, &app);
        });
        let build_ui_elapsed = build_ui_started_at.elapsed();

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

        let tessellate_started_at = Instant::now();
        let clipped_primitives =
            egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let tessellate_elapsed = tessellate_started_at.elapsed();

        surface.draw_scene(show_video)?;
        let paint_stats = surface.paint_egui(
            full_output.pixels_per_point,
            &clipped_primitives,
            &full_output.textures_delta,
        )?;
        frame_stats.record(tick_elapsed, build_ui_elapsed, tessellate_elapsed, paint_stats);
        let frame_deadline = loop_started_at + TARGET_FRAME_TIME;
        let remaining = frame_deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            tokio::time::sleep(remaining).await;
        } else {
            tokio::task::yield_now().await;
        }
    }
}
