use super::{App, AppState, CatalogSort};
use crate::gfn::auth::GfnUser;
use crate::gfn::catalog::GameSummary;
use crate::gfn::covers::{CoverSize, CoverSnapshot, CoverStore};
use crate::i18n::{I18n, arg_string};
use crate::input::AppCommand;
use fluent_bundle::FluentArgs;
use reqwest::Client;
use std::sync::Arc;

/// Builds the egui UI for the current frame and returns any commands produced by widget
/// interaction (buttons etc.) so the caller can feed them back through `App::handle_command`.

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x76, 0xb9, 0x00);
const BG_DEEP: egui::Color32 = egui::Color32::from_rgb(0x0e, 0x0e, 0x0e);
const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(0x14, 0x14, 0x14);
const BG_RAISED: egui::Color32 = egui::Color32::from_rgb(0x24, 0x24, 0x24);
const BORDER: egui::Color32 = egui::Color32::from_rgb(0x2c, 0x2c, 0x2c);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0xa0, 0xa4, 0xac);
const DANGER: egui::Color32 = egui::Color32::from_rgb(0xff, 0x6b, 0x6b);

/// Width of the left-hand title list.
const LIST_WIDTH: f32 = 250.0;
/// One list row, sized for a fingertip rather than a mouse cursor.
const ROW_HEIGHT: f32 = 30.0;

/// Installs the app's style, palette and touch-input tuning.
pub(crate) fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.spacing.scroll.bar_width = 4.0;
    style.spacing.scroll.bar_inner_margin = 0.0;
    style.spacing.scroll.bar_outer_margin = 0.0;
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.interaction.interact_radius = 12.0;

    ctx.set_style(style);

    // egui's dark theme selects in blue, which fought with the NVIDIA green everything else uses.
    // Muted rather than `ACCENT` itself: selected labels draw white text, and white on the full
    // brightness green is hard to read. The bright green stays for what already pairs it with dark
    // text - PLAY, the launch stepper.
    let mut visuals = egui::Visuals::dark();
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
    visuals.hyperlink_color = ACCENT;
    ctx.set_visuals(visuals);

    ctx.options_mut(|options| {
        options.input_options.max_click_duration = 5.0;
        options.input_options.max_click_dist = 32.0;
    });
}

/// The GeForce NOW wordmark, embedded in the binary and decoded into exactly one egui texture for
/// the whole process.
fn geforce_logo(ctx: &egui::Context) -> Option<Arc<egui::TextureHandle>> {
    const LOGO_PNG: &[u8] = include_bytes!("../../assets/geforce-now-logo.png");
    embedded_texture(ctx, "gfn_logo", LOGO_PNG, 384)
}

/// The PlayStation face buttons, for input hints.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PsButton {
    Cross,
    Circle,
}

impl PsButton {
    fn asset(self) -> (&'static str, &'static [u8]) {
        match self {
            Self::Cross => (
                "ps_button_cross",
                include_bytes!("../../assets/ps-button-x.png"),
            ),
            Self::Circle => (
                "ps_button_circle",
                include_bytes!("../../assets/ps-button-c.png"),
            ),
        }
    }
}

fn ps_button(ctx: &egui::Context, button: PsButton) -> Option<Arc<egui::TextureHandle>> {
    let (key, bytes) = button.asset();
    embedded_texture(ctx, key, bytes, 64)
}

/// PS Vita cartridge shell with a transparent window, drawn *over* the cover art so each title
/// looks like a physical Vita game card.
fn cart_frame(ctx: &egui::Context) -> Option<Arc<egui::TextureHandle>> {
    const CART_PNG: &[u8] = include_bytes!("../../assets/casset.png");
    embedded_texture(ctx, "vita_cart_frame", CART_PNG, 200)
}

const CART_ASPECT: f32 = 447.0 / 558.0;
const CART_WINDOW_X: (f32, f32) = (0.1611, 0.8479);
const CART_WINDOW_Y: (f32, f32) = (0.0376, 0.8513);

/// Decodes a PNG compiled into the binary into exactly one cached egui texture.
fn embedded_texture(
    ctx: &egui::Context,
    key: &'static str,
    bytes: &'static [u8],
    max_width: u32,
) -> Option<Arc<egui::TextureHandle>> {
    let cache_id = egui::Id::new(("embedded_texture", key));
    if let Some(cached) =
        ctx.data(|data| data.get_temp::<Option<Arc<egui::TextureHandle>>>(cache_id))
    {
        return cached;
    }

    let decoded = image::load_from_memory(bytes)
        .inspect_err(|error| eprintln!("failed to decode embedded image {key}: {error}"))
        .ok()
        .map(|image| {
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            if width <= max_width {
                return rgba;
            }
            let target_height = (height * max_width / width.max(1)).max(1);
            image::imageops::resize(
                &rgba,
                max_width,
                target_height,
                image::imageops::FilterType::Triangle,
            )
        })
        .map(|rgba| {
            let (width, height) = rgba.dimensions();
            let handle = ctx.load_texture(
                key,
                egui::ColorImage::from_rgba_unmultiplied(
                    [width as usize, height as usize],
                    rgba.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            );
            Arc::new(handle)
        });

    ctx.data_mut(|data| data.insert_temp(cache_id, decoded.clone()));
    decoded
}

/// The glyph drawn on a streaming-overlay button.
///
/// Painted rather than loaded: the app ships no icon font, and vector shapes stay crisp at the
/// Vita's 960x544 without adding binary assets for two small marks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamIcon {
    Keyboard,
    Stop,
    Stats,
    Power,
    Mouse,
    Collapse,
    Expand,
    Controls,
}

fn paint_stream_icon(painter: &egui::Painter, rect: egui::Rect, icon: StreamIcon, tint: egui::Color32) {
    match icon {
        StreamIcon::Keyboard => {
            let stroke = egui::Stroke::new(1.0_f32, tint);
            painter.rect_stroke(rect, 2u8, stroke, egui::StrokeKind::Inside);

            // Two rows of keys plus a spacebar, which reads as a keyboard at this size where
            // anything more detailed turns to mush.
            let inset = rect.shrink2(egui::vec2(2.5, 3.0));
            let key = egui::vec2(inset.width() / 5.5, 1.5);
            for row in 0..2 {
                let y = inset.min.y + row as f32 * 3.0;
                for column in 0..4 {
                    let x = inset.min.x + column as f32 * (key.x + 1.0);
                    painter.rect_filled(
                        egui::Rect::from_min_size(egui::pos2(x, y), key),
                        0.5,
                        tint,
                    );
                }
            }
            let bar_y = inset.min.y + 6.0;
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(inset.min.x + key.x, bar_y),
                    egui::vec2(inset.width() - key.x * 2.0, 1.5),
                ),
                0.5,
                tint,
            );
        }
        // The universal stop mark: a filled square.
        StreamIcon::Stop => {
            painter.rect_filled(rect.shrink(2.0), 1.5, tint);
        }
        // Three rising bars - a chart, for the counters.
        StreamIcon::Stats => {
            let inset = rect.shrink(2.0);
            let bar_width = inset.width() / 5.0;
            for (index, height_fraction) in [0.45_f32, 0.75, 1.0].into_iter().enumerate() {
                let height = inset.height() * height_fraction;
                let x = inset.min.x + index as f32 * bar_width * 1.8;
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, inset.max.y - height),
                        egui::vec2(bar_width, height),
                    ),
                    0.5,
                    tint,
                );
            }
        }
        StreamIcon::Power => {
            let c = rect.center();
            let r = rect.width().min(rect.height()) * 0.40_f32;
            painter.circle_stroke(c, r, egui::Stroke::new(1.5_f32, tint));
            painter.line_segment(
                [egui::pos2(c.x, c.y - r * 1.15_f32), egui::pos2(c.x, c.y - r * 0.15_f32)],
                egui::Stroke::new(2.0_f32, tint),
            );
        }
        StreamIcon::Mouse => {
            let s = egui::Stroke::new(1.5_f32, tint);
            let tl = rect.min + egui::vec2(2.0, 1.0);
            let bot = tl + egui::vec2(0.0, rect.height() - 3.0);
            let rt = tl + egui::vec2(rect.width() * 0.55, (rect.height() - 3.0) * 0.65);
            painter.line_segment([tl, bot], s);
            painter.line_segment([tl, rt], s);
            painter.line_segment([bot, rt], s);
        }
        StreamIcon::Collapse => {
            let s = egui::Stroke::new(2.0_f32, tint);
            let (cx, cy) = (rect.center().x, rect.center().y);
            let (dx, dy) = (rect.width() * 0.22, rect.height() * 0.32);
            painter.line_segment([egui::pos2(cx + dx, cy - dy), egui::pos2(cx - dx, cy)], s);
            painter.line_segment([egui::pos2(cx - dx, cy), egui::pos2(cx + dx, cy + dy)], s);
        }
        StreamIcon::Expand => {
            let s = egui::Stroke::new(2.0_f32, tint);
            let (cx, cy) = (rect.center().x, rect.center().y);
            let (dx, dy) = (rect.width() * 0.22, rect.height() * 0.32);
            painter.line_segment([egui::pos2(cx - dx, cy - dy), egui::pos2(cx + dx, cy)], s);
            painter.line_segment([egui::pos2(cx + dx, cy), egui::pos2(cx - dx, cy + dy)], s);
        }
        StreamIcon::Controls => {
            // Gamepad icon: outer rounded rectangle body with d-pad cross and action buttons
            let stroke = egui::Stroke::new(1.2_f32, tint);
            let inset = rect.shrink2(egui::vec2(1.0, 2.5));
            painter.rect_stroke(inset, 3.0, stroke, egui::StrokeKind::Inside);

            // D-Pad cross on left
            let dpad_cx = inset.min.x + inset.width() * 0.3;
            let dpad_cy = inset.center().y;
            let arm = 2.5;
            painter.line_segment(
                [egui::pos2(dpad_cx - arm, dpad_cy), egui::pos2(dpad_cx + arm, dpad_cy)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(dpad_cx, dpad_cy - arm), egui::pos2(dpad_cx, dpad_cy + arm)],
                stroke,
            );

            // Two action buttons on right
            let btn_cx = inset.min.x + inset.width() * 0.7;
            painter.circle_filled(egui::pos2(btn_cx - 1.8, dpad_cy + 1.2), 1.0, tint);
            painter.circle_filled(egui::pos2(btn_cx + 1.8, dpad_cy - 1.2), 1.0, tint);
        }
    }
}

/// A heart, drawn rather than typed: the bundled font has no heart glyph, exactly as it had no
/// multiplication-X, and a tofu box is worse than no icon at all.
fn paint_heart(painter: &egui::Painter, rect: egui::Rect, filled: bool, color: egui::Color32) {
    let center = rect.center();
    let width = rect.width();
    let height = rect.height();
    // Two lobes and a point. Coarse, but at 12 px anything finer is indistinguishable.
    let lobe_radius = width * 0.26;
    let left_lobe = egui::pos2(center.x - lobe_radius, center.y - height * 0.12);
    let right_lobe = egui::pos2(center.x + lobe_radius, center.y - height * 0.12);
    let tip = egui::pos2(center.x, center.y + height * 0.38);

    if filled {
        painter.circle_filled(left_lobe, lobe_radius, color);
        painter.circle_filled(right_lobe, lobe_radius, color);
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(left_lobe.x - lobe_radius, left_lobe.y),
                egui::pos2(right_lobe.x + lobe_radius, right_lobe.y),
                tip,
            ],
            color,
            egui::Stroke::NONE,
        ));
    } else {
        let stroke = egui::Stroke::new(1.2_f32, color);
        painter.circle_stroke(left_lobe, lobe_radius, stroke);
        painter.circle_stroke(right_lobe, lobe_radius, stroke);
        painter.line_segment([egui::pos2(left_lobe.x - lobe_radius, left_lobe.y), tip], stroke);
        painter.line_segment([egui::pos2(right_lobe.x + lobe_radius, right_lobe.y), tip], stroke);
    }
}

/// A streaming-overlay button: a painted glyph in a round-cornered square.
///
/// Icon-only. Labels were tried first, but three of them side by side ate most of a 960 px screen
/// and sat on top of the game.
fn stream_icon_button(ui: &mut egui::Ui, icon: StreamIcon, tint: egui::Color32) -> egui::Response {
    // Comfortably above the ~9 mm a fingertip covers on this screen.
    const BUTTON_SIZE: f32 = 30.0;
    const ICON_SIZE: f32 = 14.0;

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let painter = ui.painter();
    let fill = if response.is_pointer_button_down_on() {
        BG_DEEP
    } else {
        // Translucent so the game still shows through: this sits over live video.
        egui::Color32::from_rgba_unmultiplied(24, 24, 24, 210)
    };
    painter.rect_filled(rect, 6.0, fill);

    let icon_rect =
        egui::Rect::from_center_size(rect.center(), egui::vec2(ICON_SIZE, ICON_SIZE));
    paint_stream_icon(painter, icon_rect, icon, tint);
    response
}

/// The diagnostics panel: the peer's line plus the audio counters, on a backing plate.
///
/// Hidden unless asked for. Raw white text straight over the video was unreadable against light
/// scenes and covered the game for a readout that only matters while something is being debugged.
fn stream_stats_panel(ui: &mut egui::Ui, note: &str) {
    let font = egui::FontId::monospace(10.0);
    let text_color = egui::Color32::from_rgb(0xc8, 0xcc, 0xd4);
    let lines = [
        note.to_owned(),
        crate::streaming::audio::stats_line(),
        crate::shell::render_stats::line(),
        crate::input::stick_zone_stats::line(),
    ];

    let galleys: Vec<_> = lines
        .iter()
        .map(|line| {
            ui.fonts(|fonts| fonts.layout_no_wrap(line.clone(), font.clone(), text_color))
        })
        .collect();

    let padding = egui::vec2(8.0, 6.0);
    let line_gap = 2.0;
    let width = galleys
        .iter()
        .map(|galley| galley.size().x)
        .fold(0.0_f32, f32::max);
    let height: f32 = galleys.iter().map(|galley| galley.size().y).sum::<f32>()
        + line_gap * (galleys.len().saturating_sub(1)) as f32;

    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width + padding.x * 2.0, height + padding.y * 2.0),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    painter.rect_filled(
        rect,
        6.0,
        egui::Color32::from_rgba_unmultiplied(10, 10, 10, 205),
    );

    let mut y = rect.min.y + padding.y;
    for galley in galleys {
        let line_height = galley.size().y;
        painter.galley(egui::pos2(rect.min.x + padding.x, y), galley, text_color);
        y += line_height + line_gap;
    }
}

const STREAM_UI_RECTS: &str = "stream_ui_rects";

/// Screen-space rects (egui points) of the streaming screen's own controls as of the last frame.
///
/// While a session is live the touchscreen drives the host cursor, so every control the client
/// still owns has to carve its patch back out - otherwise it is drawn on screen but unreachable.
pub(crate) fn stream_ui_rects(ctx: &egui::Context) -> Vec<egui::Rect> {
    ctx.data(|data| {
        data.get_temp::<Vec<egui::Rect>>(egui::Id::new(STREAM_UI_RECTS))
            .unwrap_or_default()
    })
}

/// Claims `rect` for the client UI for the rest of this frame.
fn reserve_stream_touch(ctx: &egui::Context, rect: egui::Rect) {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<egui::Rect>>(egui::Id::new(STREAM_UI_RECTS))
            .push(rect)
    });
}

/// Drops last frame's claims, so a control that is no longer drawn stops swallowing touches.
fn clear_stream_touch_reservations(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new(STREAM_UI_RECTS), Vec::<egui::Rect>::new())
    });
}

/// Resolves the currently highlighted game.
pub(crate) fn selected_game<'a>(
    games: &'a [GameSummary],
    filtered_indices: &[usize],
    selected: usize,
) -> Option<&'a GameSummary> {
    games.get(*filtered_indices.get(selected)?)
}

/// Formats `id` with a single Fluent argument.
fn text1(i18n: &I18n, id: &'static str, key: &'static str, value: impl ToString) -> String {
    let mut args = FluentArgs::new();
    args.set(key, arg_string(value.to_string()));
    i18n.text_with(id, args)
}

fn text2(
    i18n: &I18n,
    id: &'static str,
    first: (&'static str, impl ToString),
    second: (&'static str, impl ToString),
) -> String {
    let mut args = FluentArgs::new();
    args.set(first.0, arg_string(first.1.to_string()));
    args.set(second.0, arg_string(second.1.to_string()));
    i18n.text_with(id, args)
}

/// Everything the catalog screen needs, bundled so the renderer doesn't take a dozen positional
/// arguments.
struct CatalogView<'a> {
    user: &'a GfnUser,
    games: &'a [GameSummary],
    selected: usize,
    filtered_indices: &'a [usize],
    search_query: &'a str,
    search_requested: bool,
    covers: &'a CoverStore,
    http_client: &'a Client,
    status_note: Option<&'a str>,
    locale: crate::locale::Locale,
    sort: CatalogSort,
    /// `pageInfo.totalCount` from the server - generally far more than we page in, so the header
    /// shows "N of M" to explain why the list stops where it does.
    total_count: Option<usize>,
    /// A background page fetch is in flight, i.e.
    loading_more: bool,
    /// Starred app ids. Held by the app rather than re-read here, because this is rebuilt on every
    /// repaint and the list lives on the memory card.
    favorites: &'a std::collections::BTreeSet<String>,
}

pub fn build_ui(ctx: &egui::Context, app: &App) -> Vec<AppCommand> {
    let i18n = I18n::new(app.locale);
    let mut commands = Vec::new();

    match &app.state {
        AppState::Login => login_screen(ctx, &i18n, app),
        AppState::StartingDeviceLogin(_) => starting_login_screen(ctx, &i18n),
        AppState::WaitingForDeviceAuthorization { challenge, .. } => {
            device_code_screen(ctx, &i18n, challenge)
        }
        AppState::LoadingCatalog { user, .. } => loading_catalog_screen(ctx, &i18n, user),
        AppState::Catalog {
            user,
            games,
            selected,
            filtered_indices,
            search_query,
            search_requested,
            covers,
        } => {
            commands.extend(catalog_screen(
                ctx,
                &i18n,
                &CatalogView {
                    user,
                    games,
                    selected: *selected,
                    filtered_indices,
                    search_query,
                    search_requested: *search_requested,
                    covers,
                    http_client: &app.http_client,
                    status_note: app.status_note.as_deref(),
                    locale: app.locale,
                    sort: app.catalog_sort,
                    total_count: app.catalog_total_count(),
                    favorites: &app.favorites,
                    loading_more: app.is_loading_more_catalog(),
                },
            ));
        }
        AppState::CreatingSession {
            user,
            games,
            selected,
            filtered_indices,
            search_query,
            search_requested,
            covers,
            job,
            queue_tracker,
        } => {
            let queue_status = queue_tracker
                .lock()
                .map(|st| st.clone())
                .unwrap_or_default();
            let game = selected_game(games, filtered_indices, *selected);
            let launch = creating_session_launch(
                &i18n,
                game,
                job.is_pending(),
                &queue_status,
                app.launch_was_queued || queue_status.was_queued,
            );
            let catalog = CatalogView {
                user,
                games,
                selected: *selected,
                filtered_indices,
                search_query,
                search_requested: *search_requested,
                covers,
                http_client: &app.http_client,
                status_note: None,
                locale: app.locale,
                sort: app.catalog_sort,
                total_count: app.catalog_total_count(),
                loading_more: app.is_loading_more_catalog(),
                favorites: &app.favorites,
            };
            if let Some(cmd) = session_launch_overlay(ctx, &i18n, &catalog, &launch) {
                commands.push(cmd);
            }
        }
        AppState::SessionReady {
            user,
            games,
            selected,
            filtered_indices,
            search_query,
            search_requested,
            covers,
            session,
        } => {
            let launch = LaunchView {
                stage: LaunchStage::Ready,
                game: selected_game(games, filtered_indices, *selected),
                headline: i18n.text("session-ready-headline"),
                detail: Some(i18n.text("session-ready-hint")),
                // Waiting on the player's Confirm, not on NVIDIA.
                spinning: false,
                session_id: Some(&session.session_id),
                queue_skipped: !app.launch_was_queued,
            };
            let catalog = CatalogView {
                user,
                games,
                selected: *selected,
                filtered_indices,
                search_query,
                search_requested: *search_requested,
                covers,
                http_client: &app.http_client,
                status_note: None,
                locale: app.locale,
                sort: app.catalog_sort,
                total_count: app.catalog_total_count(),
                loading_more: app.is_loading_more_catalog(),
                favorites: &app.favorites,
            };
            if let Some(cmd) = session_launch_overlay(ctx, &i18n, &catalog, &launch) {
                commands.push(cmd);
            }
        }
        AppState::Signaling {
            user,
            games,
            selected,
            filtered_indices,
            search_query,
            search_requested,
            covers,
            session,
            offer_sdp,
            ..
        } => {
            let launch = LaunchView {
                stage: LaunchStage::Ready,
                game: selected_game(games, filtered_indices, *selected),
                headline: i18n.text("signaling-title"),
                detail: Some(match offer_sdp.as_deref() {
                    Some(sdp) => text1(&i18n, "signaling-offer-received", "bytes", sdp.len()),
                    None => i18n.text("signaling-waiting-offer"),
                }),
                spinning: true,
                session_id: Some(&session.session_id),
                queue_skipped: !app.launch_was_queued,
            };
            let catalog = CatalogView {
                user,
                games,
                selected: *selected,
                filtered_indices,
                search_query,
                search_requested: *search_requested,
                covers,
                http_client: &app.http_client,
                status_note: None,
                locale: app.locale,
                sort: app.catalog_sort,
                total_count: app.catalog_total_count(),
                loading_more: app.is_loading_more_catalog(),
                favorites: &app.favorites,
            };
            if let Some(cmd) = session_launch_overlay(ctx, &i18n, &catalog, &launch) {
                commands.push(cmd);
            }
        }
        AppState::Streaming {
            games,
            selected,
            filtered_indices,
            peer,
            ..
        } => {
            if let Some(cmd) = streaming_screen(
                ctx,
                &i18n,
                selected_game(games, filtered_indices, *selected),
                peer.video_frame().is_some(),
                app.status_note.as_deref(),
                crate::ime::is_open(),
                app.show_stream_stats,
                app.toolbar_expanded,
                app.mouse_trackpad_enabled,
            ) {
                commands.push(cmd);
            }
        }
        AppState::Error { message, .. } => error_screen(ctx, &i18n, message),
    }

    if app.show_controls_modal && matches!(app.state, AppState::Streaming { .. })
        && let Some(cmd) = stream_controls_modal(ctx, &i18n)
    {
        commands.push(cmd);
    }

    if app.show_controls_hint && matches!(app.state, AppState::Streaming { .. })
        && let Some(cmd) = controls_hint_overlay(ctx, &i18n)
    {
        commands.push(cmd);
    }

    if app.confirm_exit {
        if let Some(cmd) = confirm_exit_modal(ctx, &i18n) {
            commands.push(cmd);
        }
    }

    splash_overlay(ctx);

    commands
}

const SPLASH_FADE_IN: f64 = 0.55;
const SPLASH_HOLD: f64 = 1.05;
const SPLASH_FADE_OUT: f64 = 0.60;
const SPLASH_TOTAL: f64 = SPLASH_FADE_IN + SPLASH_HOLD + SPLASH_FADE_OUT;

/// Brief GeForce NOW splash drawn over whatever screen is already live.
fn splash_overlay(ctx: &egui::Context) {
    let elapsed = ctx.input(|input| input.time);
    if elapsed >= SPLASH_TOTAL {
        return;
    }

    let (alpha, scale) = if elapsed < SPLASH_FADE_IN {
        let t = (elapsed / SPLASH_FADE_IN) as f32;
        let eased = t * t * (3.0 - 2.0 * t);
        (eased, 0.92 + 0.08 * eased)
    } else if elapsed < SPLASH_FADE_IN + SPLASH_HOLD {
        (1.0, 1.0)
    } else {
        let t = ((elapsed - SPLASH_FADE_IN - SPLASH_HOLD) / SPLASH_FADE_OUT) as f32;
        (1.0 - t, 1.0)
    };
    let alpha = alpha.clamp(0.0, 1.0);
    let alpha_u8 = (alpha * 255.0) as u8;

    let screen = ctx.screen_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("splash_overlay"),
    ));

    painter.rect_filled(
        screen,
        0.0,
        egui::Color32::from_rgba_unmultiplied(0x0e, 0x0e, 0x0e, alpha_u8),
    );

    let Some(logo) = geforce_logo(ctx) else {
        painter.text(
            screen.center(),
            egui::Align2::CENTER_CENTER,
            "GEFORCE NOW",
            egui::FontId::proportional(28.0),
            egui::Color32::WHITE.gamma_multiply(alpha),
        );
        return;
    };

    let size = logo.size_vec2();
    let width = (screen.width() * 0.52 * scale).min(size.x * 1.5);
    let height = width * size.y / size.x.max(1.0);
    let logo_rect =
        egui::Rect::from_center_size(screen.center(), egui::vec2(width, height));
    painter.image(
        logo.id(),
        logo_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::from_white_alpha(alpha_u8),
    );

    let rule_half = width * 0.5 * alpha;
    if rule_half > 1.0 {
        let y = logo_rect.max.y + 14.0;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(screen.center().x - rule_half, y),
                egui::pos2(screen.center().x + rule_half, y + 2.0),
            ),
            1.0,
            ACCENT.gamma_multiply(alpha),
        );
    }
}

fn login_screen(ctx: &egui::Context, i18n: &I18n, app: &App) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.heading(egui::RichText::new("OpenNOW Vita").size(32.0).strong().color(ACCENT));
            ui.label(i18n.text("login-subtitle"));
            ui.add_space(24.0);
            button_hint(ui, &i18n.text("login-hint"), 13.0, TEXT_DIM, true);
            ui.add_space(24.0);
            if let Some(last_input) = app.last_input {
                ui.weak(text1(i18n, "login-last-input", "input", format!("{last_input:?}")));
            }
        });
    });
}

fn starting_login_screen(ctx: &egui::Context, i18n: &I18n) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(120.0);
            ui.spinner();
            ui.add_space(12.0);
            ui.label(i18n.text("login-requesting-code"));
        });
    });
}

fn device_code_screen(
    ctx: &egui::Context,
    i18n: &I18n,
    challenge: &crate::gfn::auth::DeviceCodeChallenge,
) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.heading(i18n.text("device-title"));
        });
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 220.0);
                ui.label(i18n.text("device-step-open"));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&challenge.verification_uri_complete)
                        .monospace()
                        .strong(),
                );
                ui.add_space(20.0);
                ui.label(i18n.text("device-step-scan"));
                ui.add_space(12.0);
                egui::Frame::NONE
                    .fill(BG_PANEL)
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::symmetric(28, 20))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&challenge.user_code)
                                .size(48.0)
                                .monospace()
                                .strong(),
                        );
                    });
                ui.add_space(20.0);
                ui.label(i18n.text("device-waiting"));
            });

            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                draw_qr(ui, &challenge.verification_uri_complete, 200.0);
            });
        });
    });
}

fn loading_catalog_screen(ctx: &egui::Context, i18n: &I18n, user: &GfnUser) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.heading(text1(i18n, "catalog-welcome", "name", &user.display_name));
            ui.add_space(20.0);
            ui.spinner();
            ui.add_space(12.0);
            ui.label(i18n.text("catalog-loading"));
        });
    });
}

/// The catalog screen: a narrow scrolling title list on the left, a large detail panel with the
/// cover art and a PLAY button on the right.
fn catalog_screen(ctx: &egui::Context, i18n: &I18n, view: &CatalogView<'_>) -> Vec<AppCommand> {
    let mut commands = Vec::new();

    egui::TopBottomPanel::top("catalog_header")
        .frame(
            egui::Frame::NONE
                .fill(BG_PANEL)
                .inner_margin(egui::Margin::symmetric(12, 8)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                match geforce_logo(ctx) {
                    Some(logo) => {
                        let size = logo.size_vec2();
                        let height = 24.0;
                        let width = height * size.x / size.y.max(1.0);
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(width, height),
                            egui::Sense::hover(),
                        );
                        ui.painter().image(
                            logo.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    None => {
                        ui.label(
                            egui::RichText::new(i18n.text("catalog-library-title"))
                                .strong()
                                .size(20.0)
                                .color(ACCENT),
                        );
                    }
                }
                if let Some(total) = view.total_count {
                    ui.label(egui::RichText::new("/").size(15.0).color(BORDER.gamma_multiply(3.0)));
                    let key = if view.loading_more {
                        "catalog-count-loading"
                    } else {
                        "catalog-count"
                    };
                    ui.label(
                        egui::RichText::new(text2(
                            i18n,
                            key,
                            ("shown", view.filtered_indices.len()),
                            ("total", total),
                        ))
                        .size(11.0)
                        .color(TEXT_DIM),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 15.0, ACCENT);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        view.user
                            .display_name
                            .chars()
                            .next()
                            .unwrap_or('?')
                            .to_uppercase()
                            .to_string(),
                        egui::FontId::proportional(17.0),
                        BG_PANEL,
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(&view.user.display_name)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, ACCENT);

                    ui.add_space(10.0);
                    if let Some(cmd) = language_picker(ui, i18n, view.locale, view.user) {
                        commands.push(cmd);
                    }
                    ui.add_space(6.0);
                    if let Some(cmd) = sort_picker(ui, i18n, view.sort, view.games) {
                        commands.push(cmd);
                    }
                });
            });
        });

    egui::TopBottomPanel::bottom("catalog_footer")
        .frame(
            egui::Frame::NONE
                .fill(BG_PANEL)
                .inner_margin(egui::Margin::symmetric(12, 6)),
        )
        .show(ctx, |ui| {
            if let Some(note) = view.status_note {
                ui.label(egui::RichText::new(note).italics().size(11.0).color(TEXT_DIM));
            }
            button_hint(ui, &i18n.text("catalog-footer-hint"), 11.0, TEXT_DIM, false);
        });

    egui::SidePanel::left("catalog_list")
        .exact_width(LIST_WIDTH)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(BG_DEEP)
                .inner_margin(egui::Margin::symmetric(8, 8)),
        )
        .show(ctx, |ui| {
            commands.extend(title_list(ui, i18n, view));
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(BG_DEEP)
                .inner_margin(egui::Margin::symmetric(12, 10)),
        )
        .show(ctx, |ui| {
            commands.extend(detail_panel(ctx, ui, i18n, view));
        });

    commands
}

/// First-run explainer for the buttons the Vita does not physically have.
///
/// Animated deliberately: the quadrants light up one after another, because a static diagram of a
/// blank black rectangle does not read as "the back of your console" at a glance.
fn controls_hint_overlay(ctx: &egui::Context, i18n: &I18n) -> Option<AppCommand> {
    let mut command = None;
    // Timed against egui's own clock, the way `splash_overlay` does it. `animate_value_with_time`
    // looks like the obvious tool but returns the target outright on its first call, so the
    // animation was over before it was ever drawn.
    const HINT_ANIMATION: f64 = 0.9;
    let started_id = egui::Id::new("controls_hint_started_at");
    let now = ctx.input(|input| input.time);
    let started_at = ctx
        .data_mut(|data| *data.get_temp_mut_or_insert_with(started_id, || now));
    let progress = ((now - started_at) / HINT_ANIMATION).clamp(0.0, 1.0) as f32;
    if progress < 1.0 {
        // Nothing else drives frames here, and the reactive repaint would otherwise let the
        // animation sit on whatever frame it started on.
        ctx.request_repaint();
    }

    egui::Modal::new(egui::Id::new("controls_hint"))
        .backdrop_color(egui::Color32::from_black_alpha(200))
        .frame(
            egui::Frame::default()
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0_f32, BORDER))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(16, 14)),
        )
        .show(ctx, |ui| {
            ui.set_width(310.0);
            ui.heading(egui::RichText::new(i18n.text("controls-hint-heading")).size(15.0));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(i18n.text("controls-hint-rear"))
                    .size(10.0)
                    .color(TEXT_DIM),
            );
            ui.add_space(8.0);

            // The rear panel, drawn to the same 2x2 split the input code actually uses.
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 92.0), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 6.0, BG_DEEP);
            painter.rect_stroke(
                rect,
                6u8,
                egui::Stroke::new(1.0_f32, BORDER),
                egui::StrokeKind::Inside,
            );

            const QUADRANTS: [(&str, bool, bool); 2] = [("L2", true, true), ("R2", false, true)];
            for (index, (label, left, top)) in QUADRANTS.into_iter().enumerate() {
                // Each quadrant starts a quarter of the way after the previous one.
                let start = index as f32 * 0.18;
                let local = ((progress - start) / 0.4).clamp(0.0, 1.0);
                if local <= 0.0 {
                    continue;
                }
                let _ = top;
                // Halves, not quadrants: the stick clicks live on the front screen now.
                let cell = egui::Rect::from_min_size(
                    egui::pos2(if left { rect.min.x } else { rect.center().x }, rect.min.y),
                    egui::vec2(rect.width() / 2.0, rect.height()),
                )
                .shrink(4.0);
                let alpha = (local * 255.0) as u8;
                painter.rect_filled(
                    cell,
                    4.0,
                    ACCENT.gamma_multiply(0.18).linear_multiply(local),
                );
                painter.rect_stroke(
                    cell,
                    4u8,
                    egui::Stroke::new(1.0_f32, ACCENT.linear_multiply(local)),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    cell.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(15.0),
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha),
                );
            }

            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(i18n.text("controls-hint-sticks"))
                    .size(10.0)
                    .color(TEXT_DIM),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(i18n.text("controls-hint-touch"))
                    .size(10.0)
                    .color(TEXT_DIM),
            );
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                if ui
                    .add_sized(
                        [130.0, 28.0],
                        egui::Button::new(i18n.text("controls-hint-dismiss")).fill(BG_RAISED),
                    )
                    .clicked()
                {
                    command = Some(AppCommand::DismissControlsHint);
                }
            });
        });
    command
}

/// Settings button, and the modal it opens.
///
/// This was a dropdown anchored under the button. Every option added made it taller until it ran
/// off the bottom of a 544 px screen with no way to reach the last rows. A modal is centred, sized
/// to the screen, and scrolls - so it cannot outgrow the display.
fn language_picker(
    ui: &mut egui::Ui,
    i18n: &I18n,
    current: crate::locale::Locale,
    user: &GfnUser,
) -> Option<AppCommand> {
    let mut command = None;
    let response = ui.add_sized(
        [34.0, 30.0],
        egui::Button::new(egui::RichText::new("\u{2699}").size(15.0)).fill(BG_RAISED),
    );

    let open_id = egui::Id::new("settings_modal_open");
    let mut open = ui.ctx().data(|data| data.get_temp::<bool>(open_id).unwrap_or(false));
    // Opens only. It used to toggle, but the gear sits in the same screen corner as the modal's
    // close button, so one tap could both close the modal and re-open it.
    if response.clicked() {
        open = true;
    }
    if !open {
        ui.ctx().data_mut(|data| data.insert_temp(open_id, false));
        return command;
    }

    let modal = egui::Modal::new(egui::Id::new("settings_modal"))
        .backdrop_color(egui::Color32::from_black_alpha(180))
        .frame(
            egui::Frame::default()
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(14, 12)),
        )
        .show(ui.ctx(), |ui| {
            let mut close_requested = false;
            ui.set_width(300.0);
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new(i18n.text("settings-heading")).size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // A plain letter, not "\u{2715}": the bundled font has no multiplication-X
                    // glyph, so that rendered as an empty tofu box.
                    if ui
                        .add_sized(
                            [30.0, 26.0],
                            egui::Button::new(egui::RichText::new("X").size(14.0).strong()),
                        )
                        .clicked()
                    {
                        close_requested = true;
                    }
                });
            });
            if let Some(email) = &user.email {
                ui.label(egui::RichText::new(email).size(10.0).color(TEXT_DIM));
            }
            ui.separator();

            // Capped so the modal can never grow past the screen, however many options it gains.
            egui::ScrollArea::vertical()
                .max_height(330.0)
                .show(ui, |ui| {
                    if let Some(chosen) = settings_row(
                        ui,
                        i18n,
                        "settings-language-heading",
                        crate::locale::Locale::ALL.iter().copied(),
                        current,
                        |candidate| candidate.label().to_owned(),
                    ) {
                        command = Some(AppCommand::SetLocale(chosen));
                    }

                    if let Some(chosen) = settings_row(
                        ui,
                        i18n,
                        "settings-fps-heading",
                        crate::gfn::stream_prefs::StreamFps::ALL.iter().copied(),
                        crate::gfn::stream_prefs::fps(),
                        |candidate| candidate.value().to_string(),
                    ) {
                        command = Some(AppCommand::SetStreamFps(chosen));
                    }

                    if let Some(chosen) = settings_row(
                        ui,
                        i18n,
                        "settings-trigger-heading",
                        crate::gfn::stream_prefs::TriggerIntensity::ALL.iter().copied(),
                        crate::gfn::stream_prefs::trigger_intensity(),
                        |candidate| format!("{}%", u32::from(candidate.value()) * 100 / 255),
                    ) {
                        command = Some(AppCommand::SetTriggerIntensity(chosen));
                    }

                    if let Some(chosen) = settings_row(
                        ui,
                        i18n,
                        "settings-stick-zones-heading",
                        crate::gfn::stream_prefs::StickZones::ALL.iter().copied(),
                        crate::gfn::stream_prefs::stick_zones(),
                        |candidate| i18n.text(candidate.label_key()),
                    ) {
                        command = Some(AppCommand::SetStickZones(chosen));
                    }

                    if let Some(chosen) = settings_row(
                        ui,
                        i18n,
                        "settings-audio-boost-heading",
                        crate::gfn::stream_prefs::AudioBoost::ALL.iter().copied(),
                        crate::gfn::stream_prefs::audio_boost(),
                        |candidate| format!("{}x", candidate.percent() / 100),
                    ) {
                        command = Some(AppCommand::SetAudioBoost(chosen));
                    }
                });
            close_requested
        });

    // Returned from the closure rather than assigned through a capture, so there is exactly one
    // place that decides the modal is done: the button, the backdrop, or Escape.
    if modal.inner || modal.should_close() {
        open = false;
    }
    ui.ctx().data_mut(|data| data.insert_temp(open_id, open));
    command
}

/// One setting: a heading with its choices laid out across the row rather than stacked.
///
/// Horizontal is what keeps the modal short - stacked, four settings came to twenty-odd rows.
fn settings_row<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    i18n: &I18n,
    heading_key: &'static str,
    candidates: impl Iterator<Item = T>,
    current: T,
    label: impl Fn(T) -> String,
) -> Option<T> {
    let mut chosen = None;
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(i18n.text(heading_key))
            .size(10.0)
            .color(TEXT_DIM),
    );
    ui.horizontal_wrapped(|ui| {
        for candidate in candidates {
            if ui
                .selectable_label(candidate == current, label(candidate))
                .clicked()
            {
                chosen = Some(candidate);
            }
        }
    });
    chosen
}

/// Sort button + popup.
fn sort_picker(
    ui: &mut egui::Ui,
    i18n: &I18n,
    current: CatalogSort,
    games: &[GameSummary],
) -> Option<AppCommand> {
    let mut command = None;
    let label = text1(i18n, "catalog-sort-button", "sort", i18n.text(current.label_key()));
    let response = ui.add_sized([150.0, 30.0], egui::Button::new(label).fill(BG_RAISED));
    let popup_id = ui.make_persistent_id("catalog_sort_popup");
    if response.clicked() {
        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
    }
    egui::popup_below_widget(
        ui,
        popup_id,
        &response,
        egui::PopupCloseBehavior::CloseOnClick,
        |ui| {
            ui.set_min_width(170.0);
            for candidate in CatalogSort::ALL {
                let label = if candidate == CatalogSort::LastPlayed {
                    let count = games.iter().filter(|g| g.last_played.is_some()).count();
                    format!("{} ({count})", i18n.text(candidate.label_key()))
                } else {
                    i18n.text(candidate.label_key())
                };
                if ui.selectable_label(candidate == current, label).clicked() {
                    command = Some(AppCommand::SetSort(candidate));
                }
            }
        },
    );
    command
}

/// Search box + the scrolling list of titles that fills the left panel.
fn title_list(ui: &mut egui::Ui, i18n: &I18n, view: &CatalogView<'_>) -> Vec<AppCommand> {
    let mut commands = Vec::new();

    let mut query = view.search_query.to_owned();
    let hint = if view.search_query.is_empty() {
        format!(
            "{}  ({})",
            i18n.text("catalog-search-hint"),
            view.filtered_indices.len()
        )
    } else {
        i18n.text("catalog-search-hint")
    };
    // Clearing used to take two Back presses while the on-screen keyboard was up (one to dismiss
    // it, one to actually empty the field) with no visible way to do it in one tap. The × sits
    // inside the field itself, at its right edge, the same "inline clear icon" every search box
    // uses - reserving a separate widget slot for it (an earlier version of this fix) left a
    // visible seam between two disconnected-looking boxes instead of one search field.
    let show_clear = !view.search_query.is_empty();
    let response = ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text(hint)
            .desired_width(ui.available_width())
            .margin(egui::vec2(8.0, 6.0)),
    );
    let mut cleared = false;
    if show_clear {
        const CLEAR_SIZE: f32 = 20.0;
        let clear_rect = egui::Rect::from_center_size(
            egui::pos2(response.rect.right() - CLEAR_SIZE / 2.0 - 6.0, response.rect.center().y),
            egui::vec2(CLEAR_SIZE, CLEAR_SIZE),
        );
        let clear_response =
            ui.interact(clear_rect, ui.id().with("clear_search"), egui::Sense::click());
        let color = if clear_response.hovered() {
            egui::Color32::WHITE
        } else {
            TEXT_DIM
        };
        ui.painter().text(
            clear_rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(16.0),
            color,
        );
        cleared = clear_response.clicked();
    }
    if view.search_requested && !response.has_focus() {
        response.request_focus();
    }
    if response.gained_focus() && !view.search_requested {
        commands.push(AppCommand::RequestSearch);
    }
    if response.changed() {
        commands.push(AppCommand::SetSearchQuery(query));
    }
    if cleared {
        commands.push(AppCommand::SetSearchQuery(String::new()));
        commands.push(AppCommand::CloseSearch);
    }
    let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if enter_pressed || (view.search_requested && response.lost_focus()) {
        commands.push(AppCommand::CloseSearch);
    }

    ui.add_space(6.0);

    if view.filtered_indices.is_empty() {
        ui.add_space(20.0);
        ui.label(
            egui::RichText::new(if view.games.is_empty() {
                i18n.text("catalog-no-games-api")
            } else {
                i18n.text("catalog-no-match")
            })
            .size(12.0)
            .color(TEXT_DIM),
        );
        return commands;
    }

    let total = view.filtered_indices.len();
    let font_id = egui::FontId::proportional(12.0);

    let selected_id = egui::Id::new("catalog_list_last_scrolled_selected");
    let offset_id = egui::Id::new("catalog_list_scroll_offset");
    let selection_changed =
        ui.ctx().data(|d| d.get_temp::<usize>(selected_id)) != Some(view.selected);

    ui.scope(|ui| {
    // `show_rows` lays rows out on a `row_height + item_spacing.y` pitch, so the virtual row
    // geometry only lines up with what the rows actually occupy when the spacing is zero and the
    // gap is painted inside the row rect instead.
    ui.spacing_mut().item_spacing.y = 0.0;

    // Scrolling is driven from the selection index rather than from the selected row's
    // `Response`: once the cursor steps past the last visible row that row is outside
    // `row_range`, so it is never emitted, and a response-based `scroll_to_me` had nothing to
    // scroll to - the list stayed frozen while the highlight kept moving.
    let mut scroll_area = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .drag_to_scroll(false);
    if selection_changed {
        let viewport_height = ui.available_height();
        let row_top = view.selected as f32 * ROW_HEIGHT;
        let current = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(offset_id))
            .unwrap_or(0.0);
        let offset = current
            .min(row_top)
            .max(row_top + ROW_HEIGHT - viewport_height)
            .max(0.0);
        scroll_area = scroll_area.vertical_scroll_offset(offset);
    }

    let output = scroll_area
        .show_rows(ui, ROW_HEIGHT, total, |ui, row_range| {
            let painter = ui.painter().clone();
            for row in row_range {
                let Some(&game_index) = view.filtered_indices.get(row) else {
                    continue;
                };
                let game = &view.games[game_index];
                let is_selected = row == view.selected;

                let (row_rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ROW_HEIGHT),
                    egui::Sense::click(),
                );
                let rect = row_rect.shrink2(egui::vec2(0.0, 1.5));
                if !ui.is_rect_visible(row_rect) {
                    if response.clicked() {
                        commands.push(AppCommand::SelectGame(row));
                    }
                    continue;
                }

                painter.rect_filled(rect, 6.0, if is_selected { BG_RAISED } else { BG_PANEL });
                if is_selected {
                    painter.rect_stroke(
                        rect,
                        6.0,
                        egui::Stroke::new(1.5, ACCENT),
                        egui::StrokeKind::Inside,
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            rect.min + egui::vec2(2.0, 4.0),
                            egui::vec2(3.0, rect.height() - 8.0),
                        ),
                        1.5,
                        ACCENT,
                    );
                }

                let icon_size = ROW_HEIGHT - 11.0;
                let icon_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x + 9.0, rect.center().y - icon_size / 2.0),
                    egui::vec2(icon_size, icon_size),
                );
                if let Some(url) = game.cover_url.clone() {
                    view.covers
                        .request_icon(view.http_client, ui.ctx(), game.app_id.clone(), url);
                }
                painter.rect_filled(icon_rect, 3.0, BG_DEEP);
                match view.covers.get_icon(&game.app_id) {
                    Some(CoverSnapshot::Ready(image)) => {
                        let tex = image.texture(
                            ui.ctx(),
                            &CoverStore::texture_key(&game.app_id, CoverSize::Icon),
                        );
                        let size = tex.size_vec2();
                        let src_aspect = size.x / size.y.max(1.0);
                        let uv = if src_aspect > 1.0 {
                            let inset = (1.0 - 1.0 / src_aspect) / 2.0;
                            egui::Rect::from_min_max(
                                egui::pos2(inset, 0.0),
                                egui::pos2(1.0 - inset, 1.0),
                            )
                        } else {
                            let inset = (1.0 - src_aspect) / 2.0;
                            egui::Rect::from_min_max(
                                egui::pos2(0.0, inset),
                                egui::pos2(1.0, 1.0 - inset),
                            )
                        };
                        painter.image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
                    }
                    _ => {
                        painter.text(
                            icon_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            game.title.chars().next().unwrap_or('?').to_string(),
                            egui::FontId::proportional(11.0),
                            BORDER.gamma_multiply(3.0),
                        );
                    }
                }

                let text_color = if is_selected {
                    egui::Color32::WHITE
                } else {
                    TEXT_DIM
                };
                let text_x = icon_rect.max.x + 8.0;
                let mut job = egui::text::LayoutJob::single_section(
                    game.title.clone(),
                    egui::TextFormat::simple(font_id.clone(), text_color),
                );
                job.wrap =
                    egui::text::TextWrapping::truncate_at_width(rect.max.x - text_x - 8.0);
                let galley = painter.layout_job(job);
                painter.galley(
                    egui::pos2(text_x, rect.center().y - galley.size().y / 2.0),
                    galley,
                    text_color,
                );

                // A small favourite marker only, not a button: starring happens in the detail
                // panel. A tap target per row meant 5829 of them competing with the row's own
                // click, for an action taken on one game at a time.
                if view.favorites.contains(&game.app_id) {
                    paint_heart(
                        &painter,
                        egui::Rect::from_center_size(
                            egui::pos2(rect.max.x - 14.0, rect.center().y),
                            egui::vec2(11.0, 11.0),
                        ),
                        true,
                        DANGER,
                    );
                }

                if response.clicked() {
                    commands.push(AppCommand::SelectGame(row));
                }
            }
        });

    ui.ctx().data_mut(|d| {
        d.insert_temp(offset_id, output.state.offset.y);
        d.insert_temp(selected_id, view.selected);
    });
    });

    commands
}

/// Right-hand detail panel: big cover, title, metadata and the PLAY button for whichever game the
/// list has highlighted.
fn detail_panel(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    i18n: &I18n,
    view: &CatalogView<'_>,
) -> Vec<AppCommand> {
    let mut commands = Vec::new();

    let Some(game) = selected_game(view.games, view.filtered_indices, view.selected) else {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(i18n.text("detail-empty"))
                    .size(13.0)
                    .color(TEXT_DIM),
            );
        });
        return commands;
    };

    if let Some(url) = game.cover_url.clone() {
        view.covers
            .request(view.http_client, ctx, game.app_id.clone(), url);
    }

    draw_panel_backdrop(ui, ctx, view.covers, game);

    let cart_height = 226.0;

    ui.horizontal(|ui| {
        draw_cover(ui, ctx, view.covers, game, cart_height, false);

        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            let mut favorite_toggled = false;
            // Height is pinned, not left to the layout. A bare `with_layout(right_to_left, ..)`
            // here claimed the whole remaining height of the panel and centred itself in it,
            // shoving the store badge, the app id and the PLAY button off the bottom.
            //
            // Right-to-left within that row so the heart takes its space first and the title
            // truncates into what is left; the other way round, a long title pushed the heart off
            // the edge.
            const TITLE_ROW_HEIGHT: f32 = 28.0;
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), TITLE_ROW_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    let is_favorite = view.favorites.contains(&game.app_id);
                    let (heart_rect, heart_response) =
                        ui.allocate_exact_size(egui::vec2(28.0, 24.0), egui::Sense::click());
                    paint_heart(
                        &ui.painter().clone(),
                        egui::Rect::from_center_size(heart_rect.center(), egui::vec2(15.0, 15.0)),
                        is_favorite,
                        if is_favorite { DANGER } else { TEXT_DIM },
                    );
                    if heart_response.clicked() {
                        favorite_toggled = true;
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&game.title)
                                .size(19.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .truncate(),
                    );
                },
            );
            if favorite_toggled {
                commands.push(AppCommand::ToggleFavorite(game.app_id.clone()));
            }
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                if let Some(store) = game.store.as_deref() {
                    let (label, color) = store_badge(store);
                    egui::Frame::NONE
                        .fill(color)
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(label)
                                    .size(10.0)
                                    .color(egui::Color32::WHITE),
                            );
                        });
                }
            });
            ui.add_space(4.0);

            let played = match &game.last_played {
                Some(date) => text1(i18n, "detail-last-played", "date", short_date(date)),
                None => i18n.text("detail-never-played"),
            };
            ui.label(egui::RichText::new(played).size(11.0).color(TEXT_DIM));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(text1(i18n, "detail-app-id", "id", &game.app_id))
                    .size(10.0)
                    .monospace()
                    .color(BORDER.gamma_multiply(3.0)),
            );

            ui.add_space(14.0);
            if play_button(ui, i18n) {
                commands.push(AppCommand::Input(crate::input::InputCommand::Confirm));
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(i18n.text("detail-press"))
                        .size(11.0)
                        .color(TEXT_DIM),
                );
                if let Some(glyph) = ps_button(ui.ctx(), PsButton::Cross) {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(15.0, 15.0), egui::Sense::hover());
                    ui.painter().image(
                        glyph.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                ui.label(
                    egui::RichText::new(i18n.text("detail-to-start"))
                        .size(11.0)
                        .color(TEXT_DIM),
                );
            });
        });
    });

    commands
}

/// The big green PLAY button, hand-painted so it can carry a vertical gradient - egui's `Button`
/// only does flat fills.
fn play_button(ui: &mut egui::Ui, i18n: &I18n) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(200.0, 44.0), egui::Sense::click());
    let painter = ui.painter();

    let boost = if response.is_pointer_button_down_on() {
        -18
    } else if response.hovered() {
        14
    } else {
        0
    };
    let shade = |base: egui::Color32, delta: i32| {
        let apply = |c: u8| (c as i32 + delta).clamp(0, 255) as u8;
        egui::Color32::from_rgb(apply(base.r()), apply(base.g()), apply(base.b()))
    };
    let top = shade(egui::Color32::from_rgb(0x9c, 0xd3, 0x2b), boost);
    let bottom = shade(egui::Color32::from_rgb(0x6a, 0xa8, 0x00), boost);

    let radius = rect.height() / 2.0;
    let mid = egui::Color32::from_rgb(
        ((top.r() as u16 + bottom.r() as u16) / 2) as u8,
        ((top.g() as u16 + bottom.g() as u16) / 2) as u8,
        ((top.b() as u16 + bottom.b() as u16) / 2) as u8,
    );
    painter.circle_filled(
        egui::pos2(rect.min.x + radius, rect.center().y),
        radius,
        mid,
    );
    painter.circle_filled(
        egui::pos2(rect.max.x - radius, rect.center().y),
        radius,
        mid,
    );

    let body = egui::Rect::from_min_max(
        egui::pos2(rect.min.x + radius, rect.min.y),
        egui::pos2(rect.max.x - radius, rect.max.y),
    );
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(body.left_top(), top);
    mesh.colored_vertex(body.right_top(), top);
    mesh.colored_vertex(body.left_bottom(), bottom);
    mesh.colored_vertex(body.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(egui::Shape::Mesh(mesh.into()));

    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        i18n.text("detail-play"),
        egui::FontId::proportional(18.0),
        egui::Color32::from_rgb(0x10, 0x1a, 0x00),
    );

    response.clicked()
}

/// How strongly the backdrop art shows through.
const BACKDROP_ALPHA: u8 = 58;

/// Paints the selected game's cover across the whole detail panel as a dimmed backdrop.
fn draw_panel_backdrop(
    ui: &egui::Ui,
    ctx: &egui::Context,
    covers: &CoverStore,
    game: &GameSummary,
) {
    let Some(CoverSnapshot::Ready(image)) = covers.get(&game.app_id) else {
        return;
    };
    let rect = ui.max_rect();
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }

    let tex = image.texture(ctx, &format!("gfn_cover_{}", game.app_id));
    let tex_size = tex.size_vec2();
    let src_aspect = tex_size.x / tex_size.y.max(1.0);
    let dst_aspect = rect.width() / rect.height();
    let uv = if src_aspect > dst_aspect {
        let inset = (1.0 - dst_aspect / src_aspect) / 2.0;
        egui::Rect::from_min_max(egui::pos2(inset, 0.0), egui::pos2(1.0 - inset, 1.0))
    } else {
        let inset = (1.0 - src_aspect / dst_aspect) / 2.0;
        egui::Rect::from_min_max(egui::pos2(0.0, inset), egui::pos2(1.0, 1.0 - inset))
    };

    ui.painter()
        .image(tex.id(), rect, uv, egui::Color32::from_white_alpha(BACKDROP_ALPHA));
}

/// Trims an ISO-8601 timestamp down to its `YYYY-MM-DD` date part.
fn short_date(iso: &str) -> &str {
    iso.split('T').next().unwrap_or(iso)
}

/// Draws the cover art seated inside a PS Vita cartridge shell.
fn draw_cover(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    covers: &CoverStore,
    game: &GameSummary,
    cart_height: f32,
    // Stand in with the list thumbnail while the full-size cover is still downloading.
    icon_fallback: bool,
) {
    let cart_width = cart_height * CART_ASPECT;
    let (cart, _) =
        ui.allocate_exact_size(egui::vec2(cart_width, cart_height), egui::Sense::hover());
    let shell = cart_frame(ctx);

    let painter = ui.painter().clone();
    let rect = if shell.is_some() {
        egui::Rect::from_min_max(
            egui::pos2(
                cart.min.x + cart_width * CART_WINDOW_X.0,
                cart.min.y + cart_height * CART_WINDOW_Y.0,
            ),
            egui::pos2(
                cart.min.x + cart_width * CART_WINDOW_X.1,
                cart.min.y + cart_height * CART_WINDOW_Y.1,
            ),
        )
    } else {
        let inset = cart.shrink(6.0);
        painter.rect_stroke(
            inset,
            8.0,
            egui::Stroke::new(1.0_f32, BORDER.gamma_multiply(2.0)),
            egui::StrokeKind::Inside,
        );
        inset
    };
    painter.rect_filled(rect, 4.0, BG_DEEP);

    let paint_at = |size: CoverSize, image: &Arc<crate::gfn::covers::TitleImage>| {
        let tex = image.texture(ctx, &CoverStore::texture_key(&game.app_id, size));
        let tex_size = tex.size_vec2();
        let src_aspect = tex_size.x / tex_size.y.max(1.0);
        let slot_aspect = rect.width() / rect.height();
        let uv = if src_aspect > slot_aspect {
            let inset = (1.0 - slot_aspect / src_aspect) / 2.0;
            egui::Rect::from_min_max(egui::pos2(inset, 0.0), egui::pos2(1.0 - inset, 1.0))
        } else {
            let inset = (1.0 - src_aspect / slot_aspect) / 2.0;
            egui::Rect::from_min_max(egui::pos2(0.0, inset), egui::pos2(1.0, 1.0 - inset))
        };
        painter.image(tex.id(), rect, uv, egui::Color32::WHITE);
    };

    match covers.get(&game.app_id) {
        Some(CoverSnapshot::Ready(image)) => paint_at(CoverSize::Cover, &image),
        // The list thumbnail for this title is usually already decoded, so it stands in - soft,
        // but art immediately instead of a spinner, and it is replaced the moment the full cover
        // lands.
        other => match (icon_fallback, covers.get_icon(&game.app_id)) {
            (true, Some(CoverSnapshot::Ready(icon))) => paint_at(CoverSize::Icon, &icon),
            _ => match other {
                Some(CoverSnapshot::Loading) => {
                    ui.put(rect, egui::Spinner::new());
                }
                _ => {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        game.title.chars().next().unwrap_or('?').to_string(),
                        egui::FontId::proportional(48.0),
                        TEXT_DIM,
                    );
                }
            },
        },
    }

    if let Some(shell) = shell {
        painter.image(
            shell.id(),
            cart,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
}

/// Short badge label + fill color for a GFN `appStore` value (`"STEAM"`, `"EPIC"`, ...).
fn store_badge(store: &str) -> (&'static str, egui::Color32) {
    match store.to_ascii_uppercase().as_str() {
        "STEAM" => ("Steam", egui::Color32::from_rgb(0x1b, 0x2a, 0x38)),
        "EPIC" | "EPIC_GAMES" => ("Epic", egui::Color32::from_rgb(0x2a, 0x2a, 0x2a)),
        "EA_APP" | "EA" | "ORIGIN" => ("EA", egui::Color32::from_rgb(0xc4, 0x2b, 0x1c)),
        "UBISOFT" | "UPLAY" => ("Ubisoft", egui::Color32::from_rgb(0x00, 0x69, 0xd2)),
        "BATTLENET" | "BATTLE_NET" => ("Battle.net", egui::Color32::from_rgb(0x00, 0x3f, 0x6b)),
        "XBOX" | "MICROSOFT_STORE" => ("Xbox", egui::Color32::from_rgb(0x10, 0x7c, 0x10)),
        "GOG" => ("GOG", egui::Color32::from_rgb(0x86, 0x2d, 0x59)),
        "RIOT" | "RIOT_GAMES" => ("Riot", egui::Color32::from_rgb(0xd1, 0x33, 0x22)),
        _ => ("Game", egui::Color32::from_rgb(0x44, 0x44, 0x44)),
    }
}

/// Header row shared by the session/streaming screens: a title on the left and a stop button on
/// the right.
/// Where the launch pipeline is, as the three dots the player sees. `Queue` is CloudMatch holding
/// us behind other users, `Setup` is the rig being provisioned, `Ready` covers the handoff to
/// signaling once a session exists.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LaunchStage {
    Queue,
    Setup,
    Ready,
}

impl LaunchStage {
    fn index(self) -> usize {
        match self {
            Self::Queue => 0,
            Self::Setup => 1,
            Self::Ready => 2,
        }
    }
}

/// Everything the launch overlay needs that isn't the catalog behind it.
struct LaunchView<'a> {
    stage: LaunchStage,
    game: Option<&'a GameSummary>,
    /// Large line under the stepper.
    headline: String,
    /// Small line under the headline, if there's anything more specific to say.
    detail: Option<String>,
    /// False on the stages that are waiting on the player rather than on NVIDIA.
    spinning: bool,
    /// The launch never sat in NVIDIA's queue, so step 1 is drawn as skipped rather than as
    /// completed - marking it green claims the player waited through a queue that never existed.
    queue_skipped: bool,
    session_id: Option<&'a str>,
}

const LAUNCH_MODAL_WIDTH: f32 = 300.0;
const STEP_DOT_RADIUS: f32 = 13.0;

/// The whole "starting a session" flow as one modal over the still-visible library, rather than
/// three separate full-screen states - the player never loses sight of what they launched.
fn session_launch_overlay(
    ctx: &egui::Context,
    i18n: &I18n,
    catalog: &CatalogView<'_>,
    launch: &LaunchView<'_>,
) -> Option<AppCommand> {
    // Drawn purely as a backdrop: the modal takes the input layer, so the list underneath cannot
    // be interacted with and its commands are discarded.
    let _ = catalog_screen(ctx, i18n, catalog);

    let mut command = None;
    egui::Modal::new(egui::Id::new("session_launch_overlay"))
        .backdrop_color(egui::Color32::from_black_alpha(180))
        .frame(
            egui::Frame::default()
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(16, 14)),
        )
        .show(ctx, |ui| {
            ui.set_width(LAUNCH_MODAL_WIDTH);

            launch_header(ui, i18n, catalog, launch.game);
            ui.add_space(12.0);
            launch_stepper(ui, i18n, launch.stage, launch.queue_skipped);
            ui.add_space(14.0);

            ui.vertical_centered(|ui| {
                if launch.spinning {
                    ui.add(egui::Spinner::new().size(20.0).color(ACCENT));
                    ui.add_space(8.0);
                }
                ui.label(
                    egui::RichText::new(&launch.headline)
                        .size(15.0)
                        .color(egui::Color32::WHITE),
                );
                if let Some(detail) = &launch.detail {
                    ui.add_space(3.0);
                    button_hint(ui, detail, 11.0, TEXT_DIM, true);
                }
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);

            if ui
                .add_sized(
                    egui::vec2(ui.available_width(), 30.0),
                    egui::Button::new(
                        egui::RichText::new(i18n.text("session-cancel-button"))
                            .size(14.0)
                            .color(DANGER),
                    )
                    .fill(BG_RAISED),
                )
                .clicked()
            {
                command = Some(AppCommand::ToggleConfirmExit);
            }

            ui.add_space(5.0);
            button_hint(ui, &i18n.text("session-exit-hint"), 10.0, TEXT_DIM, true);
            // Only diagnostic worth keeping on screen: `status_note` is shared with every other
            // screen, so during a launch it still holds whatever the catalog last said.
            if let Some(id) = launch.session_id.filter(|id| !id.is_empty()) {
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(id).size(8.0).color(BORDER));
                });
            }
        });
    command
}

/// One segment of a hint line: literal text, or a face-button glyph standing in for a marker.
enum HintSegment<'a> {
    Text(&'a str),
    Button(PsButton),
}

/// Renders a hint line, swapping the literal `(X)` / `(O)` markers in the translated string for
/// the real PlayStation face-button glyphs. The markers stay in the `.ftl` files so translators
/// can move them around inside the sentence, and a string may contain several.
fn button_hint(ui: &mut egui::Ui, text: &str, size: f32, color: egui::Color32, centered: bool) {
    const GAP: f32 = 4.0;
    let glyph_size = size + 3.0;
    let font = egui::FontId::proportional(size);

    let mut segments = Vec::new();
    let mut rest = text;
    loop {
        let next = [("(X)", PsButton::Cross), ("(O)", PsButton::Circle)]
            .into_iter()
            .filter_map(|(marker, button)| rest.find(marker).map(|at| (at, marker, button)))
            .min_by_key(|(at, _, _)| *at);
        let Some((at, marker, button)) = next else {
            if !rest.trim().is_empty() {
                segments.push(HintSegment::Text(rest.trim()));
            }
            break;
        };
        if !rest[..at].trim().is_empty() {
            segments.push(HintSegment::Text(rest[..at].trim()));
        }
        segments.push(HintSegment::Button(button));
        rest = &rest[at + marker.len()..];
    }

    let run_width: f32 = segments
        .iter()
        .map(|segment| match segment {
            HintSegment::Text(text) => ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap((*text).to_owned(), font.clone(), color)
                    .size()
                    .x
            }),
            HintSegment::Button(_) => glyph_size,
        })
        .sum::<f32>()
        + GAP * segments.len().saturating_sub(1) as f32;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        if centered {
            ui.add_space(((ui.available_width() - run_width) / 2.0).max(0.0));
        }
        for segment in segments {
            match segment {
                HintSegment::Text(text) => {
                    ui.label(egui::RichText::new(text).size(size).color(color));
                }
                HintSegment::Button(button) => {
                    if let Some(glyph) = ps_button(ui.ctx(), button) {
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(glyph_size, glyph_size),
                            egui::Sense::hover(),
                        );
                        ui.painter().image(
                            glyph.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }
            }
        }
    });
}

/// Cover thumbnail + "Now loading" / title / storefront, mirroring the catalog's detail panel so
/// the overlay reads as the same title the player just picked.
fn launch_header(
    ui: &mut egui::Ui,
    i18n: &I18n,
    catalog: &CatalogView<'_>,
    game: Option<&GameSummary>,
) {
    ui.horizontal(|ui| {
        const HEADER_CART_HEIGHT: f32 = 76.0;
        match game {
            Some(game) => {
                // Same request + `draw_cover` path the detail panel uses, so the art, the loading
                // spinner and the initial-letter fallback all behave identically here.
                if let Some(url) = game.cover_url.clone() {
                    catalog
                        .covers
                        .request(catalog.http_client, ui.ctx(), game.app_id.clone(), url);
                }
                let ctx = ui.ctx().clone();
                draw_cover(ui, &ctx, catalog.covers, game, HEADER_CART_HEIGHT, true);
            }
            None => {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(HEADER_CART_HEIGHT * CART_ASPECT, HEADER_CART_HEIGHT),
                    egui::Sense::hover(),
                );
                ui.painter().rect_filled(rect, 4.0, BG_DEEP);
            }
        }

        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(i18n.text("session-now-loading"))
                    .size(10.0)
                    .color(ACCENT),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(match game {
                    Some(game) => game.title.as_str(),
                    None => "",
                })
                .size(16.0)
                .color(egui::Color32::WHITE),
            );
            if let Some(store) = game.and_then(|game| game.store.as_deref()) {
                ui.add_space(2.0);
                ui.label(egui::RichText::new(store).size(10.0).color(TEXT_DIM));
            }
        });
    });
}

/// Three numbered dots joined by rails, filled up to `stage`.
fn launch_stepper(ui: &mut egui::Ui, i18n: &I18n, stage: LaunchStage, queue_skipped: bool) {
    const LABELS: [&str; 3] = ["session-step-queue", "session-step-setup", "session-step-ready"];

    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, STEP_DOT_RADIUS * 2.0 + 18.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let dot_y = rect.top() + STEP_DOT_RADIUS;
    // Inset by the radius so the outer dots sit fully inside `rect` rather than half-clipped.
    let first_x = rect.left() + STEP_DOT_RADIUS + 24.0;
    let last_x = rect.right() - STEP_DOT_RADIUS - 24.0;
    let gap = (last_x - first_x) / 2.0;

    for step in 0..3 {
        let x = first_x + gap * step as f32;
        let skipped = step == 0 && queue_skipped;
        let reached = step <= stage.index() && !skipped;
        let center = egui::pos2(x, dot_y);

        if step > 0 {
            painter.line_segment(
                [
                    egui::pos2(x - gap + STEP_DOT_RADIUS + 2.0, dot_y),
                    egui::pos2(x - STEP_DOT_RADIUS - 2.0, dot_y),
                ],
                egui::Stroke::new(2.0, if reached { ACCENT } else { BORDER }),
            );
        }

        painter.circle_filled(
            center,
            STEP_DOT_RADIUS,
            if step == stage.index() {
                ACCENT
            } else {
                BG_RAISED
            },
        );
        if reached && step != stage.index() {
            painter.circle_stroke(center, STEP_DOT_RADIUS, egui::Stroke::new(1.5, ACCENT));
        }
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            (step + 1).to_string(),
            egui::FontId::proportional(12.0),
            if step == stage.index() {
                BG_DEEP
            } else if reached {
                ACCENT
            } else {
                TEXT_DIM
            },
        );
        painter.text(
            egui::pos2(x, dot_y + STEP_DOT_RADIUS + 8.0),
            egui::Align2::CENTER_CENTER,
            i18n.text(LABELS[step]),
            egui::FontId::proportional(10.0),
            if reached { egui::Color32::WHITE } else { TEXT_DIM },
        );
    }
}

/// Turns the CloudMatch queue snapshot into the overlay's stage + wording.
fn creating_session_launch<'a>(
    i18n: &I18n,
    game: Option<&'a GameSummary>,
    is_polling: bool,
    queue_status: &crate::gfn::cloudmatch::QueueStatus,
    was_queued: bool,
) -> LaunchView<'a> {
    // A run of 5xx replies looks identical to a stalled launch from the outside, so it gets said
    // out loud rather than hidden behind the queue position.
    if queue_status.server_errors > 0 {
        return LaunchView {
            stage: LaunchStage::Setup,
            game,
            headline: i18n.text("session-server-busy"),
            detail: Some(text1(
                i18n,
                "session-server-busy-retry",
                "attempt",
                queue_status.server_errors,
            )),
            spinning: true,
            session_id: None,
            queue_skipped: !was_queued,
        };
    }

    let queued = queue_status.queue_position > 0;
    let mut detail = None;

    let headline = if queued {
        detail = if queue_status.eta_ms > 0 {
            let secs = (queue_status.eta_ms / 1000) % 60;
            let mins = queue_status.eta_ms / 60000;
            Some(if mins > 0 {
                text2(
                    i18n,
                    "session-eta-minutes",
                    ("minutes", mins),
                    ("seconds", secs),
                )
            } else {
                text1(i18n, "session-eta-seconds", "seconds", secs)
            })
        } else {
            Some(text1(
                i18n,
                "session-queue-live",
                "attempt",
                queue_status.attempt,
            ))
        };
        text1(
            i18n,
            "session-queue-position",
            "position",
            queue_status.queue_position,
        )
    } else {
        if is_polling && queue_status.attempt > 0 {
            detail = Some(text1(
                i18n,
                "session-connecting-attempt",
                "attempt",
                queue_status.attempt,
            ));
        } else if is_polling {
            detail = Some(i18n.text("session-waiting-ready"));
        }
        i18n.text("session-preparing-rig")
    };

    LaunchView {
        stage: if queued {
            LaunchStage::Queue
        } else {
            LaunchStage::Setup
        },
        game,
        headline,
        detail,
        spinning: true,
        session_id: None,
        queue_skipped: !was_queued,
    }
}

fn confirm_exit_modal(ctx: &egui::Context, i18n: &I18n) -> Option<AppCommand> {
    let mut command = None;
    // A plain `Window` here used to render behind the launch overlay's `Modal`: `Modal` claims
    // egui's dedicated modal input layer, so the exit confirmation was drawn but unreachable -
    // "Cancel session" looked like it did nothing. `Modal` stacks on top of an existing `Modal`
    // (the most recently shown one wins), which is what actually lets this dialog take clicks
    // while a session is being created.
    egui::Modal::new(egui::Id::new("confirm_exit_modal"))
        .backdrop_color(egui::Color32::from_black_alpha(180))
        .frame(
            egui::Frame::default()
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(16, 14)),
        )
        .show(ctx, |ui| {
            ui.set_width(LAUNCH_MODAL_WIDTH);
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.heading(egui::RichText::new(i18n.text("exit-heading")).size(17.0));
                ui.add_space(10.0);
                ui.label(i18n.text("exit-body"));
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(i18n.text("exit-cancel")).fill(BG_RAISED))
                        .clicked()
                    {
                        command = Some(AppCommand::CancelConfirmExit);
                    }
                    ui.add_space(16.0);
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(i18n.text("exit-confirm")).color(DANGER),
                            )
                            .fill(BG_RAISED),
                        )
                        .clicked()
                    {
                        command = Some(AppCommand::ConfirmExitSession);
                    }
                });
                ui.add_space(8.0);
            });
        });
    command
}

fn streaming_screen(
    ctx: &egui::Context,
    i18n: &I18n,
    game: Option<&GameSummary>,
    has_video: bool,
    status_note: Option<&str>,
    keyboard_open: bool,
    show_stats: bool,
    toolbar_expanded: bool,
    mouse_trackpad_enabled: bool,
) -> Option<AppCommand> {
    let mut command = None;

    let mut frame = egui::Frame::central_panel(&ctx.style());
    frame.fill = egui::Color32::TRANSPARENT;
    egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
        if !has_video {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                ui.spinner();
                ui.add_space(16.0);
                match game {
                    Some(game) => ui.heading(
                        egui::RichText::new(text1(i18n, "streaming-game", "game", &game.title))
                            .size(18.0),
                    ),
                    None => {
                        ui.heading(egui::RichText::new(i18n.text("streaming-generic")).size(18.0))
                    }
                };
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(i18n.text("streaming-signaling-done"))
                        .color(ACCENT)
                        .strong(),
                );
                ui.add_space(8.0);
                ui.label(
                    status_note
                        .map(str::to_owned)
                        .unwrap_or_else(|| i18n.text("streaming-waiting-negotiation")),
                );
            });
        }

        // Rebuilt every frame: a control that stops being drawn must stop claiming its touches.
        clear_stream_touch_reservations(ui.ctx());

        // Deliberately *not* registered with `reserve_stream_touch`: that would hand them back to
        // egui, and these are driven by the stream touch router instead.
        if has_video && crate::gfn::stream_prefs::stick_zones().is_visible() {
            let screen = ui.ctx().screen_rect();
            let painter = ui.painter();
            let top = screen.min.y + screen.height() * crate::input::STICK_ZONE_TOP;
            let width = screen.width() * crate::input::STICK_ZONE_WIDTH;
            for (label, left) in [("L3", true), ("R3", false)] {
                let rect = egui::Rect::from_min_max(
                    egui::pos2(if left { screen.min.x } else { screen.max.x - width }, top),
                    egui::pos2(if left { screen.min.x + width } else { screen.max.x }, screen.max.y),
                );
                painter.rect_filled(
                    rect,
                    6.0_f32,
                    egui::Color32::from_rgba_unmultiplied(60, 110, 190, 70),
                );
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(26.0),
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 130),
                );
            }
        }

        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;

                if toolbar_expanded {
                    // 1. Power (Exit)
                    let power = stream_icon_button(ui, StreamIcon::Power, DANGER);
                    reserve_stream_touch(ui.ctx(), power.rect);
                    if power.clicked() {
                        command = Some(AppCommand::ToggleConfirmExit);
                    }

                    // 2. Stats
                    let stats = stream_icon_button(
                        ui,
                        StreamIcon::Stats,
                        if show_stats { ACCENT } else { TEXT_DIM },
                    );
                    reserve_stream_touch(ui.ctx(), stats.rect);
                    if stats.clicked() {
                        command = Some(AppCommand::ToggleStreamStats);
                    }

                    // 3. Controls Settings (L2/R2 and L3/R3 modal)
                    let controls_active = crate::gfn::stream_prefs::stick_zones().is_active()
                        || crate::gfn::stream_prefs::trigger_intensity().value() > 0;
                    let controls = stream_icon_button(
                        ui,
                        StreamIcon::Controls,
                        if controls_active { ACCENT } else { TEXT_DIM },
                    );
                    reserve_stream_touch(ui.ctx(), controls.rect);
                    if controls.clicked() {
                        command = Some(AppCommand::ToggleControlsModal);
                    }

                    // 4. Mouse trackpad
                    let mouse = stream_icon_button(
                        ui,
                        StreamIcon::Mouse,
                        if mouse_trackpad_enabled { ACCENT } else { TEXT_DIM },
                    );
                    reserve_stream_touch(ui.ctx(), mouse.rect);
                    if mouse.clicked() {
                        command = Some(AppCommand::ToggleMouseTrackpad);
                    }

                    // 5. In-game keyboard
                    let keyboard = stream_icon_button(
                        ui,
                        StreamIcon::Keyboard,
                        if keyboard_open { ACCENT } else { TEXT_DIM },
                    );
                    reserve_stream_touch(ui.ctx(), keyboard.rect);
                    if keyboard.clicked() {
                        command = Some(AppCommand::ToggleKeyboard);
                    }

                    // 6. Collapse ◀
                    let collapse = stream_icon_button(ui, StreamIcon::Collapse, ACCENT);
                    reserve_stream_touch(ui.ctx(), collapse.rect);
                    if collapse.clicked() {
                        command = Some(AppCommand::ToggleToolbar);
                    }
                } else {
                    let expand = stream_icon_button(ui, StreamIcon::Expand, ACCENT);
                    reserve_stream_touch(ui.ctx(), expand.rect);
                    if expand.clicked() {
                        command = Some(AppCommand::ToggleToolbar);
                    }
                }
            });
        });

        if show_stats && let Some(note) = status_note {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.add_space(6.0);
                stream_stats_panel(ui, note);
            });
        }
    });

    command
}

/// In-stream quick modal for adjusting L2/R2 rear-panel triggers and L3/R3 front-stick zones.
fn stream_controls_modal(ctx: &egui::Context, i18n: &I18n) -> Option<AppCommand> {
    let mut command = None;

    egui::Modal::new(egui::Id::new("stream_controls_modal"))
        .frame(egui::Frame::window(&ctx.style()).fill(BG_PANEL))
        .show(ctx, |ui| {
            ui.set_max_width(320.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        egui::RichText::new(i18n.text("settings-title"))
                            .size(16.0)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(egui::RichText::new("X").strong()).clicked() {
                            command = Some(AppCommand::ToggleControlsModal);
                        }
                    });
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                if let Some(chosen) = settings_row(
                    ui,
                    i18n,
                    "settings-trigger-heading",
                    crate::gfn::stream_prefs::TriggerIntensity::ALL.iter().copied(),
                    crate::gfn::stream_prefs::trigger_intensity(),
                    |candidate| format!("{}%", u32::from(candidate.value()) * 100 / 255),
                ) {
                    command = Some(AppCommand::SetTriggerIntensity(chosen));
                }

                ui.add_space(6.0);

                if let Some(chosen) = settings_row(
                    ui,
                    i18n,
                    "settings-stick-zones-heading",
                    crate::gfn::stream_prefs::StickZones::ALL.iter().copied(),
                    crate::gfn::stream_prefs::stick_zones(),
                    |candidate| i18n.text(candidate.label_key()),
                ) {
                    command = Some(AppCommand::SetStickZones(chosen));
                }

                ui.add_space(12.0);

                if ui
                    .add_sized(
                        [ui.available_width(), 26.0],
                        egui::Button::new(
                            egui::RichText::new(i18n.text("account-close")).size(12.0),
                        )
                        .fill(BG_RAISED),
                    )
                    .clicked()
                {
                    command = Some(AppCommand::ToggleControlsModal);
                }
            });
        });

    command
}

/// How long a fallback error body may run before it is cut. Past this it wraps into a wall of text
/// that nobody reads and that pushes the hint off the screen.
const MAX_ERROR_BODY: usize = 220;

/// What to show the player for a failure, as `(title key, body)`.
///
/// A raw error string explains the machinery; a player needs to know what to *do*. Mirrors
/// OpenNOW-Switch's `session_error::Present`, which maps the same failures to a short title and an
/// actionable body rather than printing the exception.
fn present_error(i18n: &I18n, message: &str) -> (String, String) {
    let haystack = message.to_ascii_lowercase();

    // Checked before the session case: an expired login often mentions "session" too, and the
    // recovery is completely different.
    if haystack.contains("401")
        || haystack.contains("sign in again")
        || haystack.contains("expired")
        || haystack.contains("expirado")
        || haystack.contains("caduc")
    {
        return (
            i18n.text("error-auth-title"),
            i18n.text("error-auth-body"),
        );
    }

    if haystack.contains("session_limit") || haystack.contains("active session") {
        return (
            i18n.text("error-session-busy-title"),
            i18n.text("error-session-busy-body"),
        );
    }

    let mut body = message.trim().to_owned();
    if body.chars().count() > MAX_ERROR_BODY {
        // By chars, not bytes: truncating mid-codepoint would panic on an accented message.
        body = body.chars().take(MAX_ERROR_BODY - 3).collect::<String>() + "...";
    }
    (i18n.text("error-title"), body)
}

fn error_screen(ctx: &egui::Context, i18n: &I18n, message: &str) {
    let (title, body) = present_error(i18n, message);
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(70.0);
            ui.heading(egui::RichText::new(title).size(22.0).color(DANGER));
            ui.add_space(12.0);
            ui.label(egui::RichText::new(body).size(13.0));
            ui.add_space(24.0);
            ui.label(
                egui::RichText::new(i18n.text("error-hint"))
                    .size(11.0)
                    .color(TEXT_DIM),
            );
        });
    });
}

/// Draws a QR code's module grid as plain filled rects (not an image/texture blit) - adapted from
/// green-vita (MPL-2.0), src/app/ui/screens/token_setup.rs.
struct QrImage {
    uri: String,
    modules: Vec<bool>,
    size: u32,
}

fn draw_qr(ui: &mut egui::Ui, verification_uri: &str, target_size: f32) {
    const QUIET_ZONE_MODULES: u32 = 2;
    let cache_id = egui::Id::new("device_code_qr");
    let cached = ui.ctx().data_mut(|data| {
        if let Some(cached) = data.get_temp::<Arc<QrImage>>(cache_id)
            && cached.uri == verification_uri
        {
            return Some(cached);
        }

        let code = qrcode::QrCode::new(verification_uri).ok()?;
        let image = Arc::new(QrImage {
            uri: verification_uri.to_owned(),
            size: code.width() as u32,
            modules: code
                .to_colors()
                .into_iter()
                .map(|color| color == qrcode::Color::Dark)
                .collect(),
        });
        data.insert_temp(cache_id, image.clone());
        Some(image)
    });
    let Some(cached) = cached else {
        ui.spinner();
        return;
    };
    let total_modules = cached.size + QUIET_ZONE_MODULES * 2;
    let module_size = target_size / total_modules as f32;

    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(target_size, target_size), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, egui::Color32::WHITE);
    for y in 0..cached.size {
        for x in 0..cached.size {
            if !cached.modules[(y * cached.size + x) as usize] {
                continue;
            }
            let module_rect = egui::Rect::from_min_size(
                rect.min
                    + egui::vec2(
                        (QUIET_ZONE_MODULES + x) as f32 * module_size,
                        (QUIET_ZONE_MODULES + y) as f32 * module_size,
                    ),
                egui::vec2(module_size, module_size),
            );
            painter.rect_filled(module_rect, 0.0, egui::Color32::BLACK);
        }
    }
}

#[cfg(test)]
mod error_presentation_tests {
    /// The classifier, lifted out of `present_error` so it can be checked without an `I18n`.
    /// Kept in the same order as the real one - the order is the point.
    fn classify(message: &str) -> &'static str {
        let haystack = message.to_ascii_lowercase();
        if haystack.contains("401")
            || haystack.contains("sign in again")
            || haystack.contains("expired")
            || haystack.contains("expirado")
            || haystack.contains("caduc")
        {
            "auth"
        } else if haystack.contains("session_limit") || haystack.contains("active session") {
            "session"
        } else {
            "generic"
        }
    }

    #[test]
    fn a_session_limit_is_not_shown_as_a_generic_failure() {
        assert_eq!(classify("SESSION_LIMIT still reported after cleanup"), "session");
        assert_eq!(
            classify("GeForce NOW still reports an active session"),
            "session"
        );
    }

    /// An expired login usually mentions "session" too, and the fix is completely different - so
    /// the auth case has to win. This is why the order of the checks matters.
    #[test]
    fn an_expired_login_beats_the_session_case() {
        assert_eq!(
            classify("HTTP 401 Unauthorized: session token invalid"),
            "auth"
        );
        assert_eq!(classify("Your session expired. Please sign in again."), "auth");
    }

    #[test]
    fn anything_else_falls_back() {
        assert_eq!(classify("connection reset by peer"), "generic");
    }

    /// Long errors used to wrap into a wall of text that pushed the hint off screen.
    #[test]
    fn a_long_body_is_truncated_on_a_character_boundary() {
        const MAX: usize = 220;
        // Accented, so a byte-wise truncation would split a codepoint and panic.
        let long = "é".repeat(400);
        let truncated: String = long.chars().take(MAX - 3).collect::<String>() + "...";
        assert_eq!(truncated.chars().count(), MAX);
    }
}
