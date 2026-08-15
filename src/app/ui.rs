use super::{App, AppState, CatalogFilter, CatalogSort};
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

fn vita_front(ctx: &egui::Context) -> Option<Arc<egui::TextureHandle>> {
    const FRONT_PNG: &[u8] = include_bytes!("../../assets/front.png");
    embedded_texture(ctx, "vita_front", FRONT_PNG, 480)
}

fn vita_back(ctx: &egui::Context) -> Option<Arc<egui::TextureHandle>> {
    const BACK_PNG: &[u8] = include_bytes!("../../assets/back.png");
    embedded_texture(ctx, "vita_back", BACK_PNG, 480)
}

const CART_ASPECT: f32 = 447.0 / 558.0;
const CART_WINDOW_X: (f32, f32) = (0.1611, 0.8479);
const CART_WINDOW_Y: (f32, f32) = (0.0376, 0.8513);

const REAR_PAD_X: (f32, f32) = (0.20, 0.80);
const REAR_PAD_Y: (f32, f32) = (0.18, 0.82);

const FRONT_SCREEN_X: (f32, f32) = (0.20, 0.79);
const FRONT_SCREEN_Y: (f32, f32) = (0.12, 0.83);

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
    Clock,
    Globe,
    Monitor,
    Person,
    Signal,
    Check,
    ChevronDown,
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
        StreamIcon::Clock => {
            let stroke = egui::Stroke::new(1.2_f32, tint);
            let center = rect.center();
            let radius = rect.width().min(rect.height()) * 0.45;
            painter.circle_stroke(center, radius, stroke);

            let cx = center.x;
            let cy = center.y;
            painter.line_segment([center, egui::pos2(cx + radius * 0.4, cy - radius * 0.5)], stroke);
            painter.line_segment([center, egui::pos2(cx - radius * 0.5, cy)], stroke);
        }
        StreamIcon::Globe => {
            let stroke = egui::Stroke::new(1.2_f32, tint);
            let center = rect.center();
            let radius = rect.width().min(rect.height()) * 0.42;
            painter.circle_stroke(center, radius, stroke);
            let meridian: Vec<egui::Pos2> = (0..=8)
                .map(|step| {
                    let t = step as f32 / 8.0 * std::f32::consts::PI - std::f32::consts::FRAC_PI_2;
                    egui::pos2(center.x + radius * 0.42 * t.sin(), center.y - radius * t.cos())
                })
                .collect();
            painter.line(meridian, stroke);
            painter.line_segment(
                [egui::pos2(center.x - radius, center.y), egui::pos2(center.x + radius, center.y)],
                stroke,
            );
        }
        StreamIcon::Monitor => {
            let stroke = egui::Stroke::new(1.2_f32, tint);
            let inset = rect.shrink2(egui::vec2(1.0, 3.0));
            let screen = egui::Rect::from_min_size(
                inset.min,
                egui::vec2(inset.width(), inset.height() * 0.75),
            );
            painter.rect_stroke(screen, 1.5, stroke, egui::StrokeKind::Inside);
            let stand_top = screen.max.y;
            let cx = inset.center().x;
            painter.line_segment(
                [egui::pos2(cx, stand_top), egui::pos2(cx, inset.max.y)],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(cx - inset.width() * 0.22, inset.max.y),
                    egui::pos2(cx + inset.width() * 0.22, inset.max.y),
                ],
                stroke,
            );
        }
        StreamIcon::Person => {
            let stroke = egui::Stroke::new(1.2_f32, tint);
            let center = rect.center();
            let head_r = rect.height() * 0.16;
            let head_c = egui::pos2(center.x, rect.min.y + rect.height() * 0.32);
            painter.circle_stroke(head_c, head_r, stroke);
            let shoulders = egui::Rect::from_center_size(
                egui::pos2(center.x, rect.max.y - rect.height() * 0.10),
                egui::vec2(rect.width() * 0.62, rect.height() * 0.38),
            );
            painter.rect_stroke(
                shoulders,
                egui::CornerRadius {
                    nw: (shoulders.width() * 0.5) as u8,
                    ne: (shoulders.width() * 0.5) as u8,
                    sw: 0,
                    se: 0,
                },
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        StreamIcon::Signal => {
            let inset = rect.shrink(2.0);
            let bar_width = inset.width() / 5.0;
            for (index, height_fraction) in [0.35_f32, 0.62, 0.85, 1.0].into_iter().enumerate() {
                let height = inset.height() * height_fraction;
                let x = inset.min.x + index as f32 * bar_width * 1.3;
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(x, inset.max.y - height),
                        egui::vec2(bar_width * 0.7, height),
                    ),
                    0.5,
                    tint,
                );
            }
        }
        StreamIcon::Check => {
            let stroke = egui::Stroke::new(1.8_f32, tint);
            let c = rect.center();
            let dx = rect.width() * 0.22;
            let dy = rect.height() * 0.22;
            painter.line_segment(
                [egui::pos2(c.x - dx, c.y), egui::pos2(c.x - dx * 0.15, c.y + dy)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x - dx * 0.15, c.y + dy), egui::pos2(c.x + dx, c.y - dy)],
                stroke,
            );
        }
        StreamIcon::ChevronDown => {
            let stroke = egui::Stroke::new(1.6_f32, tint);
            let c = rect.center();
            let dx = rect.width() * 0.24;
            let dy = rect.height() * 0.16;
            painter.line_segment([egui::pos2(c.x - dx, c.y - dy), egui::pos2(c.x, c.y + dy)], stroke);
            painter.line_segment([egui::pos2(c.x, c.y + dy), egui::pos2(c.x + dx, c.y - dy)], stroke);
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

const KEYBOARD_CAP_SIZE: egui::Vec2 = egui::vec2(38.0, 26.0);
const KEYBOARD_CAP_SPACING: f32 = 2.0;
const KEYBOARD_COLUMNS: f32 = 15.0;
const KEYBOARD_ROWS: f32 = 6.0;
const KEYBOARD_PADDING: f32 = 8.0;

pub(crate) fn keyboard_panel_rect(screen: egui::Rect) -> egui::Rect {
    let width = KEYBOARD_COLUMNS * KEYBOARD_CAP_SIZE.x
        + (KEYBOARD_COLUMNS - 1.0) * KEYBOARD_CAP_SPACING
        + KEYBOARD_PADDING * 2.0;
    let height = KEYBOARD_ROWS * KEYBOARD_CAP_SIZE.y
        + (KEYBOARD_ROWS - 1.0) * KEYBOARD_CAP_SPACING
        + KEYBOARD_PADDING * 2.0;
    let min = egui::pos2(screen.center().x - width / 2.0, screen.max.y - height);
    egui::Rect::from_min_size(min, egui::vec2(width, height))
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
fn text1(i18n: &I18n, id: &'static str, key: &'static str, value: impl ToString) -> std::rc::Rc<str> {
    let mut args = FluentArgs::new();
    args.set(key, arg_string(value.to_string()));
    i18n.text_with(id, args)
}

fn text2(
    i18n: &I18n,
    id: &'static str,
    first: (&'static str, impl ToString),
    second: (&'static str, impl ToString),
) -> std::rc::Rc<str> {
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
    sort: CatalogSort,
    filter: CatalogFilter,
    /// `pageInfo.totalCount` from the server - generally far more than we page in, so the header
    /// shows "N of M" to explain why the list stops where it does.
    total_count: Option<usize>,
    /// A background page fetch is in flight, i.e.
    loading_more: bool,
    /// Starred app ids. Held by the app rather than re-read here, because this is rebuilt on every
    /// repaint and the list lives on the memory card.
    favorites: &'a std::collections::BTreeSet<String>,
    regions: RegionsView<'a>,
    settings: SettingsView,
}

#[derive(Clone, Copy)]
struct SettingsView {
    open: bool,
    tab: crate::app::settings_menu::SettingsTab,
    focus: usize,
    expanded: Option<usize>,
    option_focus: usize,
}

impl SettingsView {
    fn from_app(app: &App) -> Self {
        Self {
            open: app.settings_open,
            tab: app.settings_tab,
            focus: app.settings_focus,
            expanded: app.settings_expanded,
            option_focus: app.settings_option_focus,
        }
    }
}

struct RegionsView<'a> {
    list: &'a [crate::gfn::regions::StreamRegion],
    busy: bool,
    measuring: bool,
    error: Option<&'a str>,
}

impl<'a> RegionsView<'a> {
    fn from_app(app: &'a App) -> Self {
        Self {
            list: &app.regions,
            busy: app.is_loading_regions(),
            measuring: app.regions_measuring,
            error: app.regions_error.as_deref(),
        }
    }
}

const SPLASH_FADE_IN: f64 = 0.55;
const SPLASH_HOLD: f64 = 1.05;
const SPLASH_FADE_OUT: f64 = 0.60;
const SPLASH_TOTAL: f64 = SPLASH_FADE_IN + SPLASH_HOLD + SPLASH_FADE_OUT;
const SPLASH_OPAQUE_UNTIL: f64 = SPLASH_FADE_IN + SPLASH_HOLD;

pub fn build_ui(ctx: &egui::Context, app: &App) -> Vec<AppCommand> {
    let splash_elapsed = ctx.input(|input| input.time);
    if splash_elapsed < SPLASH_TOTAL {
        ctx.request_repaint();
    }
    if splash_elapsed < SPLASH_OPAQUE_UNTIL {
        splash_overlay(ctx);
        return Vec::new();
    }

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
                    sort: app.catalog_sort,
                    filter: app.catalog_filter,
                    total_count: app.catalog_total_count(),
                    favorites: &app.favorites,
                    regions: RegionsView::from_app(app),
                    settings: SettingsView::from_app(app),
                    loading_more: app.is_loading_more_catalog(),
                },
            ));
            if app.server_picker_open {
                commands.extend(server_picker_modal(
                    ctx,
                    &i18n,
                    app,
                    selected_game(games, filtered_indices, *selected),
                ));
            }
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
                sort: app.catalog_sort,
                filter: app.catalog_filter,
                total_count: app.catalog_total_count(),
                loading_more: app.is_loading_more_catalog(),
                favorites: &app.favorites,
                regions: RegionsView::from_app(app),
                settings: SettingsView::from_app(app),
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
                sort: app.catalog_sort,
                filter: app.catalog_filter,
                total_count: app.catalog_total_count(),
                loading_more: app.is_loading_more_catalog(),
                favorites: &app.favorites,
                regions: RegionsView::from_app(app),
                settings: SettingsView::from_app(app),
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
                sort: app.catalog_sort,
                filter: app.catalog_filter,
                total_count: app.catalog_total_count(),
                loading_more: app.is_loading_more_catalog(),
                favorites: &app.favorites,
                regions: RegionsView::from_app(app),
                settings: SettingsView::from_app(app),
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
                app.keyboard_open,
                app.show_stream_stats,
                app.toolbar_expanded,
                app.mouse_trackpad_enabled,
            ) {
                commands.push(cmd);
            }
        }
        AppState::Error { message, code, .. } => error_screen(ctx, &i18n, message, *code),
    }

    if app.show_controls_modal && matches!(app.state, AppState::Streaming { .. })
        && let Some(cmd) = stream_controls_modal(ctx, &i18n)
    {
        commands.push(cmd);
    }

    if app.keyboard_open && matches!(app.state, AppState::Streaming { .. }) {
        commands.extend(on_screen_keyboard(ctx, app.key_shift, app.key_ctrl, app.key_alt));
    }

    if crate::gfn::stream_prefs::session_timer_enabled() {
        if let AppState::Streaming { session_start, .. } = &app.state {
            session_timer_overlay(
                ctx,
                *session_start,
                app.membership_tier.as_deref(),
                app.battery,
            );
        }
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

    if alpha_u8 >= 255 {
        painter.rect_filled(screen, 0.0, BG_DEEP);
    } else {
        painter.rect_filled(
            screen,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0x0e, 0x0e, 0x0e, alpha_u8),
        );
    }

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
            ui.label(i18n.text("login-subtitle").as_ref());
            ui.add_space(24.0);
            button_hint(ui, &i18n.text("login-hint"), 13.0, TEXT_DIM, true);
            ui.add_space(24.0);
            if let Some(last_input) = app.last_input {
                ui.weak(text1(i18n, "login-last-input", "input", format!("{last_input:?}")).as_ref());
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
            ui.label(i18n.text("login-requesting-code").as_ref());
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
            ui.heading(i18n.text("device-title").as_ref());
        });
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 220.0);
                ui.label(i18n.text("device-step-open").as_ref());
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&challenge.verification_uri_complete)
                        .monospace()
                        .strong(),
                );
                ui.add_space(20.0);
                ui.label(i18n.text("device-step-scan").as_ref());
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
                ui.label(i18n.text("device-waiting").as_ref());
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
            ui.heading(text1(i18n, "catalog-welcome", "name", &user.display_name).as_ref());
            ui.add_space(20.0);
            ui.spinner();
            ui.add_space(12.0);
            ui.label(i18n.text("catalog-loading").as_ref());
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
                            egui::RichText::new(i18n.text("catalog-library-title").as_ref())
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
                        egui::RichText::new(
                            text2(
                                i18n,
                                key,
                                ("shown", view.filtered_indices.len()),
                                ("total", total),
                            )
                            .as_ref(),
                        )
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
                    commands.extend(settings_modal(
                        ui,
                        i18n,
                        view.user,
                        &view.regions,
                        view.settings,
                    ));
                    ui.add_space(6.0);
                    if let Some(cmd) = sort_picker(ui, i18n, view.sort, view.games) {
                        commands.push(cmd);
                    }
                    ui.add_space(6.0);
                    if let Some(cmd) = filter_picker(ui, i18n, view.filter) {
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
fn controls_hint_overlay(ctx: &egui::Context, i18n: &I18n) -> Option<AppCommand> {
    let mut command = None;
    const HINT_ANIMATION: f64 = 0.9;
    let started_id = egui::Id::new("controls_hint_started_at");
    let now = ctx.input(|input| input.time);
    let started_at = ctx
        .data_mut(|data| *data.get_temp_mut_or_insert_with(started_id, || now));
    let progress = ((now - started_at) / HINT_ANIMATION).clamp(0.0, 1.0) as f32;
    if progress < 1.0 {
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
            ui.set_width(320.0);
            ui.heading(egui::RichText::new(i18n.text("controls-hint-heading").as_ref()).size(15.0));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(i18n.text("controls-hint-rear").as_ref())
                    .size(10.0)
                    .color(TEXT_DIM),
            );
            ui.add_space(8.0);
            rear_touch_diagram(ui, 112.0, Some(progress));
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(i18n.text("controls-hint-sticks").as_ref())
                    .size(10.0)
                    .color(TEXT_DIM),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(i18n.text("controls-hint-touch").as_ref())
                    .size(10.0)
                    .color(TEXT_DIM),
            );
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                if ui
                    .add_sized(
                        [130.0, 28.0],
                        egui::Button::new(i18n.text("controls-hint-dismiss").as_ref()).fill(BG_RAISED),
                    )
                    .clicked()
                {
                    command = Some(AppCommand::DismissControlsHint);
                }
            });
        });
    command
}

const SETTINGS_MODAL_W: f32 = 520.0;
const SETTINGS_MODAL_H: f32 = 360.0;
const SETTINGS_BODY_H: f32 = 268.0;

///
fn settings_modal(
    ui: &mut egui::Ui,
    i18n: &I18n,
    user: &GfnUser,
    regions: &RegionsView<'_>,
    settings: SettingsView,
) -> Vec<AppCommand> {
    use crate::app::settings_menu::SettingsTab;

    let mut commands = Vec::new();
    let gear = ui.add_sized(
        [34.0, 30.0],
        egui::Button::new(egui::RichText::new("\u{2699}").size(15.0)).fill(BG_RAISED),
    );
    if gear.clicked() {
        commands.push(AppCommand::OpenSettings);
    }
    if !settings.open {
        return commands;
    }

    let modal = egui::Modal::new(egui::Id::new("settings_modal"))
        .backdrop_color(egui::Color32::from_black_alpha(180))
        .frame(
            egui::Frame::default()
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0_f32, BORDER))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(14, 12)),
        )
        .show(ui.ctx(), |ui| {
            let mut close_requested = false;
            ui.set_width(SETTINGS_MODAL_W);
            ui.set_min_height(SETTINGS_MODAL_H);
            ui.set_max_height(SETTINGS_MODAL_H);

            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new(i18n.text("settings-heading").as_ref()).size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for tab in SettingsTab::ALL {
                    if let Some(cmd) = settings_tab_button(ui, i18n, tab, settings.tab) {
                        commands.push(cmd);
                    }
                }
            });
            ui.add_space(4.0);
            ui.separator();

            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), SETTINGS_BODY_H),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("settings_content")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            if let Some(email) = &user.email {
                                if settings.tab == SettingsTab::Account {
                                    ui.label(
                                        egui::RichText::new(email).size(12.0).color(egui::Color32::WHITE),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "OpenNOW-Vita {}",
                                            env!("CARGO_PKG_VERSION")
                                        ))
                                        .size(10.0)
                                        .color(TEXT_DIM),
                                    );
                                    ui.add_space(6.0);
                                    ui.separator();
                                }
                            }

                            if settings.tab == SettingsTab::Controls {
                                for cmd in controls_settings_panel(ui, i18n, settings, regions) {
                                    commands.push(cmd);
                                }
                            } else {
                                let row_count = settings.tab.row_count();
                                for row in 0..row_count {
                                    let Some(info) =
                                        crate::app::settings_menu::row_info(settings.tab, row)
                                    else {
                                        continue;
                                    };
                                    let focused = settings.focus == row;
                                    let expanded = settings.expanded == Some(row);
                                    if let Some(cmd) = settings_item(
                                        ui,
                                        i18n,
                                        settings.tab,
                                        row,
                                        &info,
                                        focused,
                                        expanded,
                                        settings.option_focus,
                                        regions,
                                        false,
                                    ) {
                                        commands.push(cmd);
                                    }
                                    ui.separator();
                                }
                            }
                        });
                },
            );

            close_requested
        });

    if modal.inner || modal.should_close() {
        commands.push(AppCommand::CloseSettings);
    }
    commands
}

fn controls_settings_panel(
    ui: &mut egui::Ui,
    i18n: &I18n,
    settings: SettingsView,
    regions: &RegionsView<'_>,
) -> Vec<AppCommand> {
    use crate::app::settings_menu::SettingsTab;

    let mut commands = Vec::new();
    let tab = SettingsTab::Controls;
    let gap = 10.0;
    let half = ((ui.available_width() - gap) / 2.0).max(120.0);

    ui.add_space(2.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(half);
            if let Some(info) = crate::app::settings_menu::row_info(tab, 0) {
                rear_touch_diagram(ui, 92.0, None);
                ui.add_space(4.0);
                if let Some(cmd) =
                    settings_chip_choice(ui, i18n, tab, 0, &info, settings.focus == 0)
                {
                    commands.push(cmd);
                }
            }
        });
        ui.add_space(gap);
        ui.vertical(|ui| {
            ui.set_width(half);
            if let Some(info) = crate::app::settings_menu::row_info(tab, 1) {
                front_stick_zones_diagram(ui, 92.0);
                ui.add_space(4.0);
                if let Some(cmd) =
                    settings_chip_choice(ui, i18n, tab, 1, &info, settings.focus == 1)
                {
                    commands.push(cmd);
                }
            }
        });
    });

    ui.add_space(6.0);
    ui.separator();

    for row in 2..tab.row_count() {
        let Some(info) = crate::app::settings_menu::row_info(tab, row) else {
            continue;
        };
        if let Some(cmd) = settings_item(
            ui,
            i18n,
            tab,
            row,
            &info,
            settings.focus == row,
            settings.expanded == Some(row),
            settings.option_focus,
            regions,
            false,
        ) {
            commands.push(cmd);
        }
        ui.separator();
    }

    commands
}

fn settings_chip_choice(
    ui: &mut egui::Ui,
    i18n: &I18n,
    tab: crate::app::settings_menu::SettingsTab,
    row: usize,
    info: &crate::app::settings_menu::RowInfo,
    focused: bool,
) -> Option<AppCommand> {
    let mut command = None;
    ui.add_space(4.0);
    let current = crate::app::settings_menu::current_option_index(tab, row, &[], i18n.locale());
    let count = crate::app::settings_menu::option_count(tab, row, 0);

    let block = ui.vertical(|ui| {
        ui.label(egui::RichText::new(i18n.text(info.label_key).as_ref()).size(12.5).strong());
        if let Some(desc_key) = info.desc_key {
            ui.label(egui::RichText::new(i18n.text(desc_key).as_ref()).size(9.5).color(TEXT_DIM));
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for option in 0..count {
                let label = crate::app::settings_menu::option_label(tab, row, option, i18n, &[]);
                let selected = option == current;
                let fill = if selected {
                    ACCENT.gamma_multiply(0.35)
                } else {
                    BG_RAISED
                };
                let text = egui::RichText::new(label)
                    .size(11.0)
                    .color(if selected {
                        egui::Color32::WHITE
                    } else {
                        TEXT_DIM
                    });
                if ui
                    .add(egui::Button::new(text).fill(fill).min_size(egui::vec2(0.0, 28.0)))
                    .clicked()
                {
                    command = Some(AppCommand::ChooseSettingsOption(row, option));
                }
            }
        });
    });
    if focused {
        ui.painter().rect_stroke(
            block.response.rect.expand(3.0),
            4.0,
            egui::Stroke::new(1.5_f32, ACCENT),
            egui::StrokeKind::Outside,
        );
    }

    command
}

fn battery_color(battery: crate::power::BatteryStatus) -> egui::Color32 {
    if battery.charging {
        ACCENT
    } else if battery.is_critical() {
        DANGER
    } else if battery.should_warn() {
        egui::Color32::from_rgb(0xe0, 0xa8, 0x30)
    } else {
        egui::Color32::WHITE
    }
}

fn paint_battery(painter: &egui::Painter, rect: egui::Rect, battery: crate::power::BatteryStatus) {
    let color = battery_color(battery);
    let body = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.max.x - 3.0, rect.max.y),
    )
    .shrink2(egui::vec2(0.0, 3.0));
    painter.rect_stroke(
        body,
        2.0,
        egui::Stroke::new(1.2_f32, color),
        egui::StrokeKind::Inside,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(body.max.x + 1.0, body.center().y - 3.0),
            egui::vec2(2.0, 6.0),
        ),
        1.0,
        color,
    );
    let inner = body.shrink(2.5);
    let filled = inner.width() * (f32::from(battery.percent) / 100.0);
    if filled > 0.5 {
        painter.rect_filled(
            egui::Rect::from_min_size(inner.min, egui::vec2(filled, inner.height())),
            1.0,
            color,
        );
    }
    if battery.charging {
        let c = body.center();
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(c.x + 1.0, c.y - 5.0),
                egui::pos2(c.x - 2.5, c.y + 0.5),
                egui::pos2(c.x - 0.2, c.y + 0.5),
                egui::pos2(c.x - 1.0, c.y + 5.0),
                egui::pos2(c.x + 2.5, c.y - 0.5),
                egui::pos2(c.x + 0.2, c.y - 0.5),
            ],
            BG_DEEP,
            egui::Stroke::new(1.0_f32, BG_DEEP),
        ));
    }
}

fn uv_subrect(image: egui::Rect, x: (f32, f32), y: (f32, f32)) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(
            image.min.x + image.width() * x.0,
            image.min.y + image.height() * y.0,
        ),
        egui::pos2(
            image.min.x + image.width() * x.1,
            image.min.y + image.height() * y.1,
        ),
    )
}

fn allocate_device_image(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    max_height: f32,
) -> Option<egui::Rect> {
    let size = texture.size_vec2();
    let width = ui.available_width().max(1.0);
    let height = (width * size.y / size.x.max(1.0)).min(max_height).max(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return None;
    }

    let painter = ui.painter();
    painter.rect_filled(rect, 6.0, BG_DEEP);
    painter.rect_stroke(
        rect,
        6u8,
        egui::Stroke::new(1.0_f32, BORDER),
        egui::StrokeKind::Inside,
    );

    let pad = 4.0;
    let inner = rect.shrink(pad);
    let scale = (inner.width() / size.x.max(1.0)).min(inner.height() / size.y.max(1.0));
    let draw = egui::vec2(size.x * scale, size.y * scale);
    let image = egui::Rect::from_center_size(inner.center(), draw);
    painter.image(
        texture.id(),
        image,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    Some(image)
}

fn paint_pulsing_zone(
    painter: &egui::Painter,
    cell: egui::Rect,
    label: &str,
    time: f64,
    phase: f64,
    font_size: f32,
) {
    let pulse = 0.5 + 0.5 * ((time * 3.0 + phase * std::f64::consts::TAU).sin() as f32);
    let cell = cell.shrink(2.0);
    painter.rect_filled(cell, 4.0, ACCENT.gamma_multiply(0.12 + pulse * 0.25));
    painter.rect_stroke(
        cell,
        4u8,
        egui::Stroke::new(1.5_f32, ACCENT.gamma_multiply(0.4 + pulse * 0.6)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        cell.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(font_size),
        egui::Color32::WHITE,
    );
}

fn paint_intro_zone(
    painter: &egui::Painter,
    cell: egui::Rect,
    label: &str,
    local: f32,
    font_size: f32,
) {
    if local <= 0.0 {
        return;
    }
    let cell = cell.shrink(2.0);
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
        egui::FontId::proportional(font_size),
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha),
    );
}

fn rear_touch_diagram(ui: &mut egui::Ui, max_height: f32, intro: Option<f32>) {
    let Some(texture) = vita_back(ui.ctx()) else {
        return;
    };
    let Some(image) = allocate_device_image(ui, &texture, max_height) else {
        return;
    };

    let mode = crate::gfn::stream_prefs::rear_touch_mode();
    let trigger_swap = crate::gfn::stream_prefs::trigger_swap_enabled();
    let (tl, tr) = if trigger_swap {
        ("L1", "R1")
    } else {
        ("L2", "R2")
    };
    let (bl, br) = ("L3", "R3");

    let pad = uv_subrect(image, REAR_PAD_X, REAR_PAD_Y);
    let painter = ui.painter();

    if let Some(progress) = intro {
        let halves = [("L2", 0.0_f32, 0.0), ("R2", 1.0, 0.5)];
        let cell_size = egui::vec2(pad.width() / 2.0, pad.height());
        for (index, (label, column, phase)) in halves.iter().enumerate() {
            let start = index as f32 * 0.18;
            let local = ((progress - start) / 0.4).clamp(0.0, 1.0);
            let cell = egui::Rect::from_min_size(
                egui::pos2(pad.min.x + column * cell_size.x, pad.min.y),
                cell_size,
            );
            let _ = phase;
            paint_intro_zone(painter, cell, label, local, 12.0);
        }
        return;
    }

    let time = ui.ctx().input(|input| input.time);
    ui.ctx().request_repaint();

    match mode {
        crate::gfn::stream_prefs::RearTouchMode::Halves => {
            let cell_size = egui::vec2(pad.width() / 2.0, pad.height());
            for (label, column, phase) in [(tl, 0.0_f32, 0.0_f64), (tr, 1.0, 0.5)] {
                let cell = egui::Rect::from_min_size(
                    egui::pos2(pad.min.x + column * cell_size.x, pad.min.y),
                    cell_size,
                );
                paint_pulsing_zone(painter, cell, label, time, phase, 12.0);
            }
        }
        crate::gfn::stream_prefs::RearTouchMode::Quadrant => {
            let cell_size = egui::vec2(pad.width() / 2.0, pad.height() / 2.0);
            for (label, column, row, phase) in [
                (tl, 0.0_f32, 0.0_f32, 0.00_f64),
                (tr, 1.0, 0.0, 0.25),
                (bl, 0.0, 1.0, 0.50),
                (br, 1.0, 1.0, 0.75),
            ] {
                let cell = egui::Rect::from_min_size(
                    egui::pos2(
                        pad.min.x + column * cell_size.x,
                        pad.min.y + row * cell_size.y,
                    ),
                    cell_size,
                );
                paint_pulsing_zone(painter, cell, label, time, phase, 10.0);
            }
        }
    }
}

fn front_stick_zones_diagram(ui: &mut egui::Ui, max_height: f32) {
    let Some(texture) = vita_front(ui.ctx()) else {
        return;
    };
    let Some(image) = allocate_device_image(ui, &texture, max_height) else {
        return;
    };

    let zones = crate::gfn::stream_prefs::stick_zones();
    if !zones.is_active() {
        return;
    }

    let screen = uv_subrect(image, FRONT_SCREEN_X, FRONT_SCREEN_Y);
    let time = ui.ctx().input(|input| input.time);
    ui.ctx().request_repaint();

    let painter = ui.painter();
    let top = crate::input::STICK_ZONE_TOP;
    let width = crate::input::STICK_ZONE_WIDTH;
    let left = egui::Rect::from_min_max(
        egui::pos2(screen.min.x, screen.min.y + screen.height() * top),
        egui::pos2(screen.min.x + screen.width() * width, screen.max.y),
    );
    let right = egui::Rect::from_min_max(
        egui::pos2(
            screen.max.x - screen.width() * width,
            screen.min.y + screen.height() * top,
        ),
        egui::pos2(screen.max.x, screen.max.y),
    );
    paint_pulsing_zone(painter, left, "L3", time, 0.0, 10.0);
    paint_pulsing_zone(painter, right, "R3", time, 0.5, 10.0);
}

fn ping_color(ms: u32) -> egui::Color32 {
    match ms {
        0..=40 => ACCENT,
        41..=80 => egui::Color32::from_rgb(0xe0, 0xa8, 0x30),
        _ => DANGER,
    }
}

fn queue_color(position: u32) -> egui::Color32 {
    match position {
        0..=9 => ACCENT,
        10..=24 => egui::Color32::from_rgb(0xe0, 0xa8, 0x30),
        _ => DANGER,
    }
}

fn format_wait(seconds: u64) -> String {
    if seconds >= 60 {
        format!("~{}m", seconds / 60)
    } else {
        format!("~{seconds}s")
    }
}

fn server_picker_modal(
    ctx: &egui::Context,
    i18n: &I18n,
    app: &App,
    game: Option<&GameSummary>,
) -> Vec<AppCommand> {
    let mut commands = Vec::new();
    let regions = &app.regions;
    let queue = &app.queue_stats;
    let focus = app.server_picker_focus;

    let queue_for = |url: &str| {
        crate::gfn::queue_stats::server_code_from_url(url).and_then(|code| queue.get(&code).copied())
    };
    let best_index = regions
        .iter()
        .enumerate()
        .filter_map(|(index, region)| region.ping_ms.map(|ping| (index, region, ping)))
        .min_by_key(|(_, region, ping)| {
            let depth = queue_for(&region.url).map_or(u32::MAX, |r| r.queue_position);
            (*ping, depth)
        })
        .map(|(index, _, _)| index);
    let closest_index = regions
        .iter()
        .enumerate()
        .filter_map(|(index, region)| region.ping_ms.map(|ping| (index, ping)))
        .min_by_key(|(_, ping)| *ping)
        .map(|(index, _)| index);

    egui::Modal::new(egui::Id::new("server_picker_modal"))
        .backdrop_color(egui::Color32::from_black_alpha(190))
        .frame(
            egui::Frame::default()
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0_f32, BORDER))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(14, 12)),
        )
        .show(ctx, |ui| {
            ui.set_width(520.0);

            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new(i18n.text("server-picker-heading").as_ref()).size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(
                            [30.0, 26.0],
                            egui::Button::new(egui::RichText::new("X").size(14.0).strong()),
                        )
                        .clicked()
                    {
                        commands.push(AppCommand::CloseServerPicker);
                    }
                });
            });
            if let Some(game) = game {
                ui.label(
                    egui::RichText::new(&game.title)
                        .size(10.5)
                        .color(TEXT_DIM),
                );
            }
            ui.separator();

            if app.is_loading_regions() {
                ui.label(
                    egui::RichText::new(i18n.text(if app.regions_measuring {
                        "settings-region-measuring"
                    } else {
                        "settings-region-loading"
                    }).as_ref())
                    .size(10.0)
                    .color(TEXT_DIM),
                );
            }
            if app.is_loading_queue_stats() {
                ui.label(
                    egui::RichText::new(i18n.text("server-picker-queue-loading").as_ref())
                        .size(10.0)
                        .color(TEXT_DIM),
                );
            }

            egui::ScrollArea::vertical()
                .id_salt("server_picker_list")
                .max_height(200.0)
                .show(ui, |ui| {
                    let auto_detail = best_index
                        .and_then(|index| regions.get(index))
                        .map(|region| match region.ping_ms {
                            Some(ms) => format!("{} · {ms} ms", region.name),
                            None => region.name.clone(),
                        });
                    if server_picker_row(
                        ui,
                        &i18n.text("settings-region-auto"),
                        auto_detail.as_deref(),
                        None,
                        None,
                        focus == 0,
                        None,
                    ) {
                        commands.push(AppCommand::FocusServerPicker(0));
                        commands.push(AppCommand::LaunchOnServer(String::new()));
                    }

                    for (index, region) in regions.iter().enumerate() {
                        let row = index + 1;
                        let badge = if Some(index) == best_index {
                            Some(i18n.text("server-picker-auto-badge"))
                        } else if Some(index) == closest_index {
                            Some(i18n.text("server-picker-closest-badge"))
                        } else {
                            None
                        };
                        if server_picker_row(
                            ui,
                            &region.name,
                            None,
                            region.ping_ms,
                            queue_for(&region.url),
                            focus == row,
                            badge.as_deref(),
                        ) {
                            commands.push(AppCommand::FocusServerPicker(row));
                            commands.push(AppCommand::LaunchOnServer(region.url.clone()));
                        }
                    }
                });

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(i18n.text("server-picker-hint").as_ref())
                    .size(9.5)
                    .color(TEXT_DIM),
            );
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let launch = egui::Button::new(
                        egui::RichText::new(i18n.text("server-picker-launch").as_ref())
                            .size(12.0)
                            .strong()
                            .color(BG_DEEP),
                    )
                    .fill(ACCENT)
                    .min_size(egui::vec2(96.0, 28.0));
                    if ui.add(launch).clicked() {
                        commands.push(AppCommand::LaunchOnServer(
                            match focus.checked_sub(1) {
                                None => String::new(),
                                Some(index) => regions
                                    .get(index)
                                    .map(|region| region.url.clone())
                                    .unwrap_or_default(),
                            },
                        ));
                    }
                    ui.add_space(6.0);
                    let cancel = egui::Button::new(
                        egui::RichText::new(i18n.text("server-picker-cancel").as_ref()).size(12.0),
                    )
                    .fill(BG_RAISED)
                    .min_size(egui::vec2(84.0, 28.0));
                    if ui.add(cancel).clicked() {
                        commands.push(AppCommand::CloseServerPicker);
                    }
                    ui.add_space(6.0);
                    let refresh = ui.add_sized(
                        [28.0, 28.0],
                        egui::Button::new("").fill(BG_RAISED),
                    );
                    if refresh.clicked() {
                        commands.push(AppCommand::LoadQueueStats);
                        commands.push(AppCommand::TestRegionLatency);
                    }
                    paint_stream_icon(
                        ui.painter(),
                        refresh.rect.shrink(7.0),
                        StreamIcon::Signal,
                        ACCENT,
                    );
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(i18n.text("server-picker-powered-by").as_ref())
                                .size(9.5)
                                .color(TEXT_DIM),
                        )
                        .truncate(),
                    );
                });
            });
        });

    commands
}

fn server_picker_row(
    ui: &mut egui::Ui,
    name: &str,
    detail: Option<&str>,
    ping_ms: Option<u32>,
    queue: Option<crate::gfn::queue_stats::QueueReading>,
    focused: bool,
    badge: Option<&str>,
) -> bool {
    let height = if detail.is_some() { 34.0 } else { 26.0 };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return response.clicked();
    }

    let painter = ui.painter();
    if focused {
        painter.rect_filled(rect, 5.0, ACCENT.gamma_multiply(0.16));
        painter.rect_stroke(
            rect,
            5.0,
            egui::Stroke::new(1.0_f32, ACCENT),
            egui::StrokeKind::Inside,
        );
    }

    let text_x = rect.min.x + 8.0;
    let name_y = if detail.is_some() {
        rect.min.y + 11.0
    } else {
        rect.center().y
    };
    let name_end = painter.text(
        egui::pos2(text_x, name_y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(11.5),
        egui::Color32::WHITE,
    );
    if let Some(detail) = detail {
        painter.text(
            egui::pos2(text_x, rect.min.y + 24.0),
            egui::Align2::LEFT_CENTER,
            detail,
            egui::FontId::proportional(9.5),
            TEXT_DIM,
        );
    }
    if let Some(badge) = badge {
        painter.text(
            egui::pos2(name_end.max.x + 8.0, name_y),
            egui::Align2::LEFT_CENTER,
            badge,
            egui::FontId::proportional(8.5),
            ACCENT,
        );
    }

    let mut x = rect.max.x - 8.0;
    if let Some(seconds) = queue.and_then(|reading| reading.eta_seconds) {
        let drawn = painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            format_wait(seconds),
            egui::FontId::proportional(10.0),
            TEXT_DIM,
        );
        x = drawn.min.x - 10.0;
    }
    if let Some(reading) = queue {
        let drawn = painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            format!("Q:{}", reading.queue_position),
            egui::FontId::proportional(10.5),
            queue_color(reading.queue_position),
        );
        x = drawn.min.x - 10.0;
    }
    if let Some(ms) = ping_ms {
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            format!("{ms} ms"),
            egui::FontId::proportional(10.5),
            ping_color(ms),
        );
    }

    response.clicked()
}

fn settings_tab_button(
    ui: &mut egui::Ui,
    i18n: &I18n,
    tab: crate::app::settings_menu::SettingsTab,
    current: crate::app::settings_menu::SettingsTab,
) -> Option<AppCommand> {
    use crate::app::settings_menu::SettingsTab;

    let icon = match tab {
        SettingsTab::Stream => StreamIcon::Globe,
        SettingsTab::Controls => StreamIcon::Controls,
        SettingsTab::App => StreamIcon::Monitor,
        SettingsTab::Account => StreamIcon::Person,
    };
    let active = tab == current;
    let label = i18n.text(tab.label_key());
    let (rect, response) = ui.allocate_exact_size(egui::vec2(122.0, 28.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if active {
            painter.rect_filled(rect, 5.0, ACCENT.gamma_multiply(0.14));
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.min.x + 8.0, rect.max.y - 3.0),
                    egui::vec2(rect.width() - 16.0, 2.0),
                ),
                1.0,
                ACCENT,
            );
        }
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.min.x + 16.0, rect.center().y),
            egui::vec2(13.0, 13.0),
        );
        paint_stream_icon(painter, icon_rect, icon, if active { ACCENT } else { TEXT_DIM });
        painter.text(
            egui::pos2(rect.min.x + 30.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label.as_ref(),
            egui::FontId::proportional(12.0),
            if active {
                egui::Color32::WHITE
            } else {
                TEXT_DIM
            },
        );
    }
    response.clicked().then_some(AppCommand::SetSettingsTab(tab))
}

fn settings_item(
    ui: &mut egui::Ui,
    i18n: &I18n,
    tab: crate::app::settings_menu::SettingsTab,
    row: usize,
    info: &crate::app::settings_menu::RowInfo,
    focused: bool,
    expanded: bool,
    option_focus: usize,
    regions: &RegionsView<'_>,
    show_touch_diagrams: bool,
) -> Option<AppCommand> {
    use crate::app::settings_menu::RowKind;

    let mut command = None;
    ui.add_space(4.0);

    if matches!(info.kind, RowKind::Region)
        && regions.list.is_empty()
        && !regions.busy
        && regions.error.is_none()
    {
        command = Some(AppCommand::LoadRegions);
    }

    let control_width = if matches!(info.kind, RowKind::Region) {
        200.0
    } else {
        160.0
    };
    let header_response = ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - control_width).max(80.0));
            ui.label(egui::RichText::new(i18n.text(info.label_key).as_ref()).size(12.5).strong());
            if let Some(desc_key) = info.desc_key {
                ui.label(egui::RichText::new(i18n.text(desc_key).as_ref()).size(9.5).color(TEXT_DIM));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match info.kind {
                RowKind::Toggle(on) => {
                    let game_only = info.label_key == "settings-game-profile-heading";
                    let can_toggle = !game_only || crate::gfn::stream_prefs::active_game().is_some();
                    let mut value = on;
                    let response = ui.add_enabled(
                        can_toggle,
                        egui::Checkbox::without_text(&mut value),
                    );
                    if response.changed() && can_toggle {
                        command = Some(AppCommand::ChooseSettingsOption(row, 0));
                    }
                }
                RowKind::Choice | RowKind::Region => {
                    let summary = if info.kind == RowKind::Region && regions.busy {
                        i18n.text(if regions.measuring {
                            "settings-region-measuring"
                        } else {
                            "settings-region-loading"
                        })
                        .to_string()
                    } else {
                        crate::app::settings_menu::current_summary(tab, row, i18n, regions.list, i18n.locale())
                    };
                    let button = egui::Button::new(egui::RichText::new(format!("{summary}   ")).size(11.0))
                        .fill(BG_RAISED)
                        .min_size(egui::vec2(150.0, 28.0));
                    let button_response = ui.add(button);
                    if button_response.clicked() {
                        command = Some(AppCommand::ExpandSettingsRow(if expanded { None } else { Some(row) }));
                    }
                    let chevron_rect = egui::Rect::from_center_size(
                        egui::pos2(button_response.rect.max.x - 14.0, button_response.rect.center().y),
                        egui::vec2(12.0, 12.0),
                    );
                    paint_stream_icon(ui.painter(), chevron_rect, StreamIcon::ChevronDown, TEXT_DIM);
                    if info.kind == RowKind::Region {
                        let test_btn = ui.add_sized([28.0, 28.0], egui::Button::new("").fill(BG_RAISED));
                        if test_btn.clicked() {
                            command = Some(if regions.list.is_empty() {
                                AppCommand::LoadRegions
                            } else {
                                AppCommand::TestRegionLatency
                            });
                        }
                        paint_stream_icon(ui.painter(), test_btn.rect.shrink(7.0), StreamIcon::Signal, ACCENT);
                    }
                }
            }
        });
    });

    if focused {
        ui.painter().rect_stroke(
            header_response.response.rect.expand(3.0),
            4.0,
            egui::Stroke::new(1.5_f32, ACCENT),
            egui::StrokeKind::Outside,
        );
    }

    if let RowKind::Region = info.kind {
        if let Some(error) = regions.error {
            ui.label(egui::RichText::new(error).size(10.0).color(DANGER));
        }
    }

    if expanded {
        let option_count = crate::app::settings_menu::option_count(tab, row, regions.list.len());
        egui::Frame::default()
            .fill(BG_DEEP)
            .stroke(egui::Stroke::new(1.0_f32, BORDER))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| {
                let current_index = crate::app::settings_menu::current_option_index(
                    tab,
                    row,
                    regions.list,
                    i18n.locale(),
                );
                for option in 0..option_count {
                    let label = crate::app::settings_menu::option_label(tab, row, option, i18n, regions.list);
                    let is_current = option == current_index;
                    let is_focused = option == option_focus;
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
                    if ui.is_rect_visible(rect) {
                        if is_focused {
                            ui.painter().rect_filled(rect, 4.0, ACCENT.gamma_multiply(0.18));
                        }
                        ui.painter().text(
                            egui::pos2(rect.min.x + 8.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &label,
                            egui::FontId::proportional(11.5),
                            if is_current { ACCENT } else { egui::Color32::WHITE },
                        );
                        if is_current {
                            let check_rect = egui::Rect::from_center_size(
                                egui::pos2(rect.max.x - 14.0, rect.center().y),
                                egui::vec2(12.0, 12.0),
                            );
                            paint_stream_icon(ui.painter(), check_rect, StreamIcon::Check, ACCENT);
                        }
                        if info.kind == RowKind::Region && option > 0 {
                            if let Some(best) = regions
                                .list
                                .iter()
                                .filter_map(|r| r.ping_ms)
                                .min()
                            {
                                if regions.list.get(option - 1).and_then(|r| r.ping_ms) == Some(best) {
                                    ui.painter().text(
                                        egui::pos2(rect.max.x - 46.0, rect.center().y),
                                        egui::Align2::RIGHT_CENTER,
                                        i18n.text("settings-region-best"),
                                        egui::FontId::proportional(8.5),
                                        ACCENT,
                                    );
                                }
                            }
                        }
                    }
                    if response.clicked() {
                        command = Some(AppCommand::ChooseSettingsOption(row, option));
                    }
                }
            });
    }

    if show_touch_diagrams {
        if info.label_key == "settings-rear-touch-mode-heading" {
            ui.add_space(8.0);
            rear_touch_diagram(ui, 120.0, None);
            ui.add_space(6.0);
        }
        if info.label_key == "settings-stick-zones-heading" {
            ui.add_space(8.0);
            front_stick_zones_diagram(ui, 120.0);
            ui.add_space(6.0);
        }
    }

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
        egui::RichText::new(i18n.text(heading_key).as_ref())
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
    let label = text1(i18n, "catalog-sort-button", "sort", i18n.text(current.label_key()).as_ref());
    let response = ui.add_sized([150.0, 30.0], egui::Button::new(label.as_ref()).fill(BG_RAISED));
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
                    i18n.text(candidate.label_key()).to_string()
                };
                if ui.selectable_label(candidate == current, label).clicked() {
                    command = Some(AppCommand::SetSort(candidate));
                }
            }
        },
    );
    command
}

// same as sort_picker but for my games / all games
fn filter_picker(ui: &mut egui::Ui, i18n: &I18n, current: CatalogFilter) -> Option<AppCommand> {
    let mut command = None;
    let label = text1(i18n, "catalog-filter-button", "filter", i18n.text(current.label_key()).as_ref());
    let response = ui.add_sized([150.0, 30.0], egui::Button::new(label.as_ref()).fill(BG_RAISED));
    let popup_id = ui.make_persistent_id("catalog_filter_popup");
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
            for candidate in CatalogFilter::ALL {
                let label = i18n.text(candidate.label_key());
                if ui.selectable_label(candidate == current, label.as_ref()).clicked() {
                    command = Some(AppCommand::SetFilter(candidate));
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
        i18n.text("catalog-search-hint").to_string()
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
            egui::RichText::new(
                if view.games.is_empty() {
                    i18n.text("catalog-no-games-api")
                } else {
                    i18n.text("catalog-no-match")
                }
                .as_ref(),
            )
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
                        egui::Stroke::new(1.5_f32, ACCENT),
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
                if !view.covers.is_requested(&game.app_id, CoverSize::Icon)
                    && let Some(url) = game.cover_url.clone()
                {
                    view.covers
                        .request_icon(view.http_client, ui.ctx(), game.app_id.clone(), url);
                }
                painter.rect_filled(icon_rect, 3.0, BG_DEEP);
                match view.covers.get_icon(&game.app_id) {
                    Some(CoverSnapshot::Ready(image)) => {
                        let tex = image.texture(ui.ctx(), || {
                            CoverStore::texture_key(&game.app_id, CoverSize::Icon)
                        });
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
                egui::RichText::new(i18n.text("detail-empty").as_ref())
                    .size(13.0)
                    .color(TEXT_DIM),
            );
        });
        return commands;
    };

    if !view.covers.is_requested(&game.app_id, CoverSize::Cover)
        && let Some(url) = game.cover_url.clone()
    {
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
            ui.label(egui::RichText::new(played.as_ref()).size(11.0).color(TEXT_DIM));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(text1(i18n, "detail-app-id", "id", &game.app_id).as_ref())
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
                    egui::RichText::new(i18n.text("detail-press").as_ref())
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
                    egui::RichText::new(i18n.text("detail-to-start").as_ref())
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

    let tex = image.texture(ctx, || CoverStore::texture_key(&game.app_id, CoverSize::Cover));
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
        let tex = image.texture(ctx, || CoverStore::texture_key(&game.app_id, size));
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
    headline: std::rc::Rc<str>,
    /// Small line under the headline, if there's anything more specific to say.
    detail: Option<std::rc::Rc<str>>,
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
                .stroke(egui::Stroke::new(1.0_f32, BORDER))
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
                    egui::RichText::new(launch.headline.as_ref())
                        .size(15.0)
                        .color(egui::Color32::WHITE),
                );
                if let Some(detail) = &launch.detail {
                    ui.add_space(3.0);
                    button_hint(ui, detail.as_ref(), 11.0, TEXT_DIM, true);
                }
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);

            if ui
                .add_sized(
                    egui::vec2(ui.available_width(), 30.0),
                    egui::Button::new(
                        egui::RichText::new(i18n.text("session-cancel-button").as_ref())
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
                if !catalog.covers.is_requested(&game.app_id, CoverSize::Cover)
                    && let Some(url) = game.cover_url.clone()
                {
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
                egui::RichText::new(i18n.text("session-now-loading").as_ref())
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
                egui::Stroke::new(2.0_f32, if reached { ACCENT } else { BORDER }),
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
            painter.circle_stroke(center, STEP_DOT_RADIUS, egui::Stroke::new(1.5_f32, ACCENT));
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
    // Checked before the server-error case: a patch is reported as a 5xx but is not a failure, and
    // it can hold the launch for many minutes - long enough that silence reads as a hang.
    if queue_status.app_patching {
        return LaunchView {
            stage: LaunchStage::Setup,
            game,
            headline: i18n.text("session-app-patching"),
            detail: Some(i18n.text("session-app-patching-detail")),
            spinning: true,
            session_id: None,
            queue_skipped: !was_queued,
        };
    }

    if queue_status.has_video_ad {
        let percent = (queue_status.ad_progress_pct.clamp(0.0, 1.0) * 100.0).round() as u32;
        return LaunchView {
            stage: LaunchStage::Queue,
            game,
            headline: i18n.text("session-ad-playing"),
            detail: Some(text1(i18n, "session-ad-progress", "percent", percent)),
            spinning: true,
            session_id: None,
            queue_skipped: !was_queued,
        };
    }

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
                .stroke(egui::Stroke::new(1.0_f32, BORDER))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(16, 14)),
        )
        .show(ctx, |ui| {
            ui.set_width(LAUNCH_MODAL_WIDTH);
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.heading(egui::RichText::new(i18n.text("exit-heading").as_ref()).size(17.0));
                ui.add_space(10.0);
                ui.label(i18n.text("exit-body").as_ref());
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(i18n.text("exit-cancel").as_ref()).fill(BG_RAISED))
                        .clicked()
                    {
                        command = Some(AppCommand::CancelConfirmExit);
                    }
                    ui.add_space(16.0);
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(i18n.text("exit-confirm").as_ref()).color(DANGER),
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
                        egui::RichText::new(text1(i18n, "streaming-game", "game", &game.title).as_ref())
                            .size(18.0),
                    ),
                    None => {
                        ui.heading(egui::RichText::new(i18n.text("streaming-generic").as_ref()).size(18.0))
                    }
                };
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(i18n.text("streaming-signaling-done").as_ref())
                        .color(ACCENT)
                        .strong(),
                );
                ui.add_space(8.0);
                ui.label(
                    status_note
                        .map(str::to_owned)
                        .unwrap_or_else(|| i18n.text("streaming-waiting-negotiation").to_string()),
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

                    let timer_active = crate::gfn::stream_prefs::session_timer_enabled();
                    let timer = stream_icon_button(
                        ui,
                        StreamIcon::Clock,
                        if timer_active { ACCENT } else { TEXT_DIM },
                    );
                    reserve_stream_touch(ui.ctx(), timer.rect);
                    if timer.clicked() {
                        command = Some(AppCommand::ToggleSessionTimer);
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
        .backdrop_color(egui::Color32::from_black_alpha(160))
        .frame(
            egui::Frame::default()
                .fill(BG_PANEL)
                .stroke(egui::Stroke::new(1.0_f32, BORDER))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(12, 10)),
        )
        .show(ctx, |ui| {
            ui.set_width(280.0);

            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new(i18n.text("controls-hint-heading").as_ref())
                        .size(14.0)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(
                            [26.0, 22.0],
                            egui::Button::new(egui::RichText::new("X").size(12.0).strong()),
                        )
                        .clicked()
                    {
                        command = Some(AppCommand::ToggleControlsModal);
                    }
                });
            });
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);

            rear_touch_diagram(ui, 72.0, None);
            ui.add_space(6.0);

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
                "settings-rear-touch-mode-heading",
                crate::gfn::stream_prefs::RearTouchMode::ALL.iter().copied(),
                crate::gfn::stream_prefs::rear_touch_mode(),
                |candidate| i18n.text(candidate.label_key()).to_string(),
            ) {
                command = Some(AppCommand::SetRearTouchMode(chosen));
            }

            ui.add_space(2.0);

            if let Some(chosen) = settings_row(
                ui,
                i18n,
                "settings-stick-zones-heading",
                crate::gfn::stream_prefs::StickZones::ALL.iter().copied(),
                crate::gfn::stream_prefs::stick_zones(),
                |candidate| i18n.text(candidate.label_key()).to_string(),
            ) {
                command = Some(AppCommand::SetStickZones(chosen));
            }

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(2.0);

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(i18n.text("settings-trigger-swap-heading").as_ref())
                        .size(11.0)
                        .color(egui::Color32::WHITE),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut value = crate::gfn::stream_prefs::trigger_swap_enabled();
                    if ui
                        .add(egui::Checkbox::without_text(&mut value))
                        .changed()
                    {
                        command = Some(AppCommand::ToggleTriggerSwap);
                    }
                });
            });

            ui.add_space(8.0);

            if ui
                .add_sized(
                    [ui.available_width(), 24.0],
                    egui::Button::new(
                        egui::RichText::new(i18n.text("account-close").as_ref()).size(11.0),
                    )
                    .fill(BG_RAISED),
                )
                .clicked()
            {
                command = Some(AppCommand::ToggleControlsModal);
            }
        });

    command
}

enum KeyCap {
    Char(char, char),
    Key(&'static str, crate::gfn::input_protocol::KeyStroke),
    Backspace,
    Enter,
    Space,
    Shift,
    Ctrl,
    Alt,
}

fn keyboard_layout() -> [Vec<(KeyCap, f32)>; 6] {
    use crate::gfn::input_protocol::*;
    [
        vec![
            (KeyCap::Key("Esc", KEY_ESCAPE), 1.0),
            (KeyCap::Key("F1", KEY_F1), 1.0), (KeyCap::Key("F2", KEY_F2), 1.0),
            (KeyCap::Key("F3", KEY_F3), 1.0), (KeyCap::Key("F4", KEY_F4), 1.0),
            (KeyCap::Key("F5", KEY_F5), 1.0), (KeyCap::Key("F6", KEY_F6), 1.0),
            (KeyCap::Key("F7", KEY_F7), 1.0), (KeyCap::Key("F8", KEY_F8), 1.0),
            (KeyCap::Key("F9", KEY_F9), 1.0), (KeyCap::Key("F10", KEY_F10), 1.0),
            (KeyCap::Key("F11", KEY_F11), 1.0), (KeyCap::Key("F12", KEY_F12), 1.0),
            (KeyCap::Key("Home", KEY_HOME), 1.0),
            (KeyCap::Key("End", KEY_END), 1.0),
        ],
        vec![
            (KeyCap::Char('`', '~'), 1.0),
            (KeyCap::Char('1', '!'), 1.0), (KeyCap::Char('2', '@'), 1.0),
            (KeyCap::Char('3', '#'), 1.0), (KeyCap::Char('4', '$'), 1.0),
            (KeyCap::Char('5', '%'), 1.0), (KeyCap::Char('6', '^'), 1.0),
            (KeyCap::Char('7', '&'), 1.0), (KeyCap::Char('8', '*'), 1.0),
            (KeyCap::Char('9', '('), 1.0), (KeyCap::Char('0', ')'), 1.0),
            (KeyCap::Char('-', '_'), 1.0), (KeyCap::Char('=', '+'), 1.0),
            (KeyCap::Backspace, 2.0),
        ],
        vec![
            (KeyCap::Key("Tab", KEY_TAB), 1.5),
            (KeyCap::Char('q', 'Q'), 1.0), (KeyCap::Char('w', 'W'), 1.0),
            (KeyCap::Char('e', 'E'), 1.0), (KeyCap::Char('r', 'R'), 1.0),
            (KeyCap::Char('t', 'T'), 1.0), (KeyCap::Char('y', 'Y'), 1.0),
            (KeyCap::Char('u', 'U'), 1.0), (KeyCap::Char('i', 'I'), 1.0),
            (KeyCap::Char('o', 'O'), 1.0), (KeyCap::Char('p', 'P'), 1.0),
            (KeyCap::Char('[', '{'), 1.0), (KeyCap::Char(']', '}'), 1.0),
            (KeyCap::Char('\\', '|'), 1.5),
        ],
        vec![
            (KeyCap::Key("Caps", KEY_CAPS_LOCK), 1.75),
            (KeyCap::Char('a', 'A'), 1.0), (KeyCap::Char('s', 'S'), 1.0),
            (KeyCap::Char('d', 'D'), 1.0), (KeyCap::Char('f', 'F'), 1.0),
            (KeyCap::Char('g', 'G'), 1.0), (KeyCap::Char('h', 'H'), 1.0),
            (KeyCap::Char('j', 'J'), 1.0), (KeyCap::Char('k', 'K'), 1.0),
            (KeyCap::Char('l', 'L'), 1.0), (KeyCap::Char(';', ':'), 1.0),
            (KeyCap::Char('\'', '"'), 1.0),
            (KeyCap::Enter, 2.25),
        ],
        vec![
            (KeyCap::Shift, 2.25),
            (KeyCap::Char('z', 'Z'), 1.0), (KeyCap::Char('x', 'X'), 1.0),
            (KeyCap::Char('c', 'C'), 1.0), (KeyCap::Char('v', 'V'), 1.0),
            (KeyCap::Char('b', 'B'), 1.0), (KeyCap::Char('n', 'N'), 1.0),
            (KeyCap::Char('m', 'M'), 1.0), (KeyCap::Char(',', '<'), 1.0),
            (KeyCap::Char('.', '>'), 1.0), (KeyCap::Char('/', '?'), 1.0),
            (KeyCap::Shift, 2.75),
        ],
        vec![
            (KeyCap::Ctrl, 1.25),
            (KeyCap::Alt, 1.25),
            (KeyCap::Key("Win", KEY_LEFT_WIN), 1.25),
            (KeyCap::Space, 4.25),
            (KeyCap::Key("AltGr", KEY_RIGHT_ALT), 1.0),
            (KeyCap::Key("Menu", KEY_MENU), 1.0),
            (KeyCap::Key("Ctrl", KEY_RIGHT_CTRL), 1.0),
            (KeyCap::Key("<", KEY_LEFT), 1.0), (KeyCap::Key("^", KEY_UP), 1.0),
            (KeyCap::Key("v", KEY_DOWN), 1.0), (KeyCap::Key(">", KEY_RIGHT), 1.0),
        ],
    ]
}

fn on_screen_keyboard(ctx: &egui::Context, shift: bool, ctrl: bool, alt: bool) -> Vec<AppCommand> {
    use crate::gfn::input_protocol::key_for_char;

    let mut commands = Vec::new();
    let panel_rect = keyboard_panel_rect(ctx.screen_rect());
    reserve_stream_touch(ctx, panel_rect);

    egui::Area::new(egui::Id::new("on_screen_keyboard"))
        .fixed_pos(panel_rect.min)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::window(&ui.style())
                .fill(BG_PANEL.gamma_multiply(0.96))
                .inner_margin(egui::Margin::same(KEYBOARD_PADDING as i8))
                .outer_margin(egui::Margin::ZERO)
                .show(ui, |ui| {
                    ui.set_width(panel_rect.width() - KEYBOARD_PADDING * 2.0);
                    ui.spacing_mut().item_spacing = egui::vec2(KEYBOARD_CAP_SPACING, KEYBOARD_CAP_SPACING);

                    for row in keyboard_layout() {
                        ui.horizontal(|ui| {
                            for (cap, units) in row {
                                let (label, active) = match &cap {
                                    KeyCap::Char(lower, upper) => {
                                        (if shift { upper.to_string() } else { lower.to_string() }, false)
                                    }
                                    KeyCap::Key(label, _) => (label.to_string(), false),
                                    KeyCap::Backspace => ("Bksp".to_string(), false),
                                    KeyCap::Enter => ("Enter".to_string(), false),
                                    KeyCap::Space => (String::new(), false),
                                    KeyCap::Shift => ("Shift".to_string(), shift),
                                    KeyCap::Ctrl => ("Ctrl".to_string(), ctrl),
                                    KeyCap::Alt => ("Alt".to_string(), alt),
                                };
                                let width =
                                    KEYBOARD_CAP_SIZE.x * units + KEYBOARD_CAP_SPACING * (units - 1.0);
                                let mut button = egui::Button::new(
                                    egui::RichText::new(label).size(11.0),
                                );
                                button = if active {
                                    button.fill(ACCENT.gamma_multiply(0.35))
                                } else {
                                    button.fill(BG_RAISED)
                                };
                                let response =
                                    ui.add_sized([width, KEYBOARD_CAP_SIZE.y], button);
                                if !response.clicked() {
                                    continue;
                                }
                                match cap {
                                    KeyCap::Char(lower, upper) => {
                                        let ch = if shift { upper } else { lower };
                                        if let Some(key) = key_for_char(ch) {
                                            commands.push(if ctrl || alt {
                                                AppCommand::SendChord { ctrl, alt, key }
                                            } else {
                                                AppCommand::SendKey(key)
                                            });
                                        }
                                    }
                                    KeyCap::Key(_, key) => {
                                        commands.push(if ctrl || alt {
                                            AppCommand::SendChord { ctrl, alt, key }
                                        } else {
                                            AppCommand::SendKey(key)
                                        });
                                    }
                                    KeyCap::Backspace => {
                                        let key = crate::gfn::input_protocol::KEY_BACKSPACE;
                                        commands.push(if ctrl || alt {
                                            AppCommand::SendChord { ctrl, alt, key }
                                        } else {
                                            AppCommand::SendKey(key)
                                        });
                                    }
                                    KeyCap::Enter => {
                                        let key = crate::gfn::input_protocol::KEY_ENTER;
                                        commands.push(if ctrl || alt {
                                            AppCommand::SendChord { ctrl, alt, key }
                                        } else {
                                            AppCommand::SendKey(key)
                                        });
                                    }
                                    KeyCap::Space => {
                                        let key = crate::gfn::input_protocol::KEY_SPACE;
                                        commands.push(if ctrl || alt {
                                            AppCommand::SendChord { ctrl, alt, key }
                                        } else {
                                            AppCommand::SendKey(key)
                                        });
                                    }
                                    KeyCap::Shift => commands.push(AppCommand::ToggleKeyShift),
                                    KeyCap::Ctrl => commands.push(AppCommand::ToggleKeyCtrl),
                                    KeyCap::Alt => commands.push(AppCommand::ToggleKeyAlt),
                                }
                            }
                        });
                    }
                });
        });

    commands
}

/// How long a fallback error body may run before it is cut. Past this it wraps into a wall of text
/// that nobody reads and that pushes the hint off the screen.
const MAX_ERROR_BODY: usize = 220;

// old text-based classifier, only hit when we never got a real gfn code (sign-in, catalog
// graphql, signaling socket). has spanish words too bc the text mightve already been
// translated. dont add more to this list, thats what the code table is for now
fn legacy_error_keys(message: &str) -> Option<(&'static str, &'static str)> {
    let haystack = message.to_ascii_lowercase();

    // Checked before the session case: an expired login often mentions "session" too, and the
    // recovery is completely different.
    if haystack.contains("401")
        || haystack.contains("sign in again")
        || haystack.contains("expired")
        || haystack.contains("expirado")
        || haystack.contains("caduc")
    {
        return Some(("error-auth-title", "error-auth-body"));
    }

    if haystack.contains("session_limit") || haystack.contains("active session") {
        return Some(("error-session-busy-title", "error-session-busy-body"));
    }

    None
}

// title/body to show the player. code decides it when we have one, substring checks below
// are just the fallback for stuff that never carried a code (sign-in, catalog, signaling)
fn present_error(
    i18n: &I18n,
    message: &str,
    code: Option<crate::gfn::error_codes::GfnErrorCode>,
) -> (String, String) {
    if let Some(code) = code {
        if let Some((title, body)) = code.message_keys() {
            return (i18n.text(title).to_string(), i18n.text(body).to_string());
        }
        // A code NVIDIA has not given wording to. Naming it still beats the raw JSON this used to
        // print, and it is the string a player can search for or quote in a bug report.
        return (
            i18n.text("error-gfn-unknown-title").to_string(),
            text1(
                i18n,
                "error-gfn-unknown-body",
                "detail",
                match code.name() {
                    Some(name) => format!("{name} ({})", code.0),
                    None => code.0.to_string(),
                },
            )
            .to_string(),
        );
    }

    if let Some((title, body)) = legacy_error_keys(message) {
        return (i18n.text(title).to_string(), i18n.text(body).to_string());
    }

    let mut body = message.trim().to_owned();
    if body.chars().count() > MAX_ERROR_BODY {
        // By chars, not bytes: truncating mid-codepoint would panic on an accented message.
        body = body.chars().take(MAX_ERROR_BODY - 3).collect::<String>() + "...";
    }
    (i18n.text("error-title").to_string(), body)
}

fn error_screen(
    ctx: &egui::Context,
    i18n: &I18n,
    message: &str,
    code: Option<crate::gfn::error_codes::GfnErrorCode>,
) {
    let (title, body) = present_error(i18n, message, code);
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(70.0);
            ui.heading(egui::RichText::new(title).size(22.0).color(DANGER));
            ui.add_space(12.0);
            ui.label(egui::RichText::new(body).size(13.0));
            ui.add_space(24.0);
            ui.label(
                egui::RichText::new(i18n.text("error-hint").as_ref())
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

fn session_timer_overlay(
    ctx: &egui::Context,
    start_time: std::time::Instant,
    tier_str: Option<&str>,
    battery: Option<crate::power::BatteryStatus>,
) {
    let tier_val = tier_str.unwrap_or("Free");
    let max_duration: u32 = match tier_val {
        "Ultimate" | "RTX3080" => 8 * 60 * 60, // 8 hours
        "Premium" | "Priority" => 6 * 60 * 60, // 6 hours
        _ => 60 * 60,                          // 1 hour for Free
    };
    
    let elapsed = start_time.elapsed().as_secs() as u32;
    let _remaining = max_duration.saturating_sub(elapsed);
    let progress = (elapsed as f32 / max_duration as f32).clamp(0.0, 1.0);

    let format_time = |secs: u32| -> String {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let s = secs % 60;
        if hours > 0 {
            format!("{hours}:{mins:02}:{s:02}")
        } else {
            format!("{mins:02}:{s:02}")
        }
    };

    let elapsed_str = format_time(elapsed);
    let total_str = format_time(max_duration);
    let text = format!("{elapsed_str} / {total_str}");

    egui::Window::new("session_timer_overlay")
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -10.0))
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(egui::Color32::from_black_alpha(200))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .corner_radius(8.0),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                paint_stream_icon(ui.painter(), icon_rect, StreamIcon::Clock, egui::Color32::WHITE);

                let (arrow_rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
                let arrow_center = arrow_rect.center();
                let arrow_points = [
                    egui::pos2(arrow_center.x - 3.5, arrow_center.y - 2.0),
                    egui::pos2(arrow_center.x + 3.5, arrow_center.y - 2.0),
                    egui::pos2(arrow_center.x, arrow_center.y + 2.5),
                ];
                ui.painter().add(egui::Shape::convex_polygon(
                    arrow_points.to_vec(),
                    egui::Color32::WHITE,
                    egui::Stroke::NONE,
                ));

                let bar = egui::ProgressBar::new(progress)
                    .text(egui::RichText::new(text).color(egui::Color32::WHITE))
                    .desired_width(120.0);
                ui.add(bar);

                if let Some(battery) = battery {
                    ui.add_space(6.0);
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(22.0, 16.0), egui::Sense::hover());
                    paint_battery(ui.painter(), rect, battery);
                    ui.label(
                        egui::RichText::new(format!("{}%", battery.percent))
                            .size(10.5)
                            .color(battery_color(battery)),
                    );
                }
            });
        });
}

#[cfg(test)]
mod error_presentation_tests {
    use super::{
        keyboard_layout, keyboard_panel_rect, legacy_error_keys, KEYBOARD_CAP_SIZE,
        KEYBOARD_CAP_SPACING, KEYBOARD_COLUMNS, KEYBOARD_PADDING,
    };
    use crate::gfn::error_codes::GfnErrorCode;

    fn classify(message: &str) -> &'static str {
        match legacy_error_keys(message) {
            Some(("error-auth-title", _)) => "auth",
            Some(_) => "session",
            None => "generic",
        }
    }

    #[test]
    fn a_session_limit_is_not_shown_as_a_generic_failure() {
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

    // this one wouldve landed on the auth branch if we still matched by text
    #[test]
    fn a_code_decides_regardless_of_the_wording() {
        let (title, _) = GfnErrorCode::SESSION_LIMIT_PER_DEVICE_REACHED
            .message_keys()
            .expect("the per-device limit has wording");
        assert_eq!(title, "error-gfn-session-limit-per-device-reached-title");
        assert_eq!(
            classify("CloudMatch rejected the launch: token expired"),
            "auth",
            "without a code this is all the classifier has to go on"
        );
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

    #[test]
    fn keyboard_rows_all_span_full_width() {
        for (index, row) in keyboard_layout().iter().enumerate() {
            let units: f32 = row.iter().map(|(_, units)| units).sum();
            assert!(
                (units - KEYBOARD_COLUMNS).abs() < f32::EPSILON,
                "row {index} sums to {units} cap-units, but the panel is sized for \
                 {KEYBOARD_COLUMNS}; caps outside it cannot be touched",
            );
        }
    }

    #[test]
    fn a_full_width_row_exactly_fills_the_panel() {
        let inner = keyboard_panel_rect(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(960.0 / 1.3, 544.0 / 1.3),
        ))
        .width()
            - KEYBOARD_PADDING * 2.0;
        let row = KEYBOARD_COLUMNS * KEYBOARD_CAP_SIZE.x
            + (KEYBOARD_COLUMNS - 1.0) * KEYBOARD_CAP_SPACING;
        assert!((inner - row).abs() < f32::EPSILON, "{inner} != {row}");
    }
}
