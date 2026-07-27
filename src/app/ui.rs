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
/// interaction (buttons etc.) so the caller can feed them back through
/// `App::handle_command`.

// Shared palette. NVIDIA green is the single accent; everything else is a dark neutral so the
// covers themselves carry the color.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x76, 0xb9, 0x00);
const BG_DEEP: egui::Color32 = egui::Color32::from_rgb(0x0e, 0x0e, 0x0e);
const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(0x14, 0x14, 0x14);
const BG_RAISED: egui::Color32 = egui::Color32::from_rgb(0x24, 0x24, 0x24);
const BORDER: egui::Color32 = egui::Color32::from_rgb(0x2c, 0x2c, 0x2c);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0xa0, 0xa4, 0xac);
const DANGER: egui::Color32 = egui::Color32::from_rgb(0xff, 0x6b, 0x6b);

/// Width of the left-hand title list. The Vita gives us ~738x418 points at `UI_SCALE` 1.3, so
/// this leaves ~470pt for the detail panel - enough for a 150pt cover plus readable metadata.
const LIST_WIDTH: f32 = 250.0;
/// One list row, sized for a fingertip rather than a mouse cursor.
const ROW_HEIGHT: f32 = 30.0;

/// Installs the app's style, palette and touch-input tuning. Call **once** at startup, from
/// `shell::run` - egui persists style/visuals/options across frames, and doing this per frame
/// meant cloning the whole `Style` (nested `Visuals`, the `TextStyles` map, `Spacing`) plus
/// taking the options lock 60 times a second for values that never change.
pub(crate) fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    // Thin, unobtrusive scrollbar - the default one eats too much of a 250pt-wide list.
    style.spacing.scroll.bar_width = 4.0;
    style.spacing.scroll.bar_inner_margin = 0.0;
    style.spacing.scroll.bar_outer_margin = 0.0;
    // Touch targets: the Vita is driven by a fingertip on a 5" panel, so give every widget a
    // little more slop than egui's mouse-oriented defaults.
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.interaction.interact_radius = 12.0;

    ctx.set_style(style);
    ctx.set_visuals(egui::Visuals::dark());

    // egui's defaults assume a mouse: a press is only a *click* if it lasts under 0.8s and the
    // pointer stays within 6pt. A thumb tap on the Vita's resistive touchscreen routinely
    // breaks both - it lingers well past a second and wanders several points while lifting -
    // so taps were silently being discarded as aborted drags. That was the "some buttons just
    // don't work" report: the widgets were fine, the click never qualified as one.
    ctx.options_mut(|options| {
        options.input_options.max_click_duration = 5.0;
        options.input_options.max_click_dist = 32.0;
    });
}

/// The GeForce NOW wordmark, embedded in the binary and decoded into exactly one egui texture
/// for the whole process.
///
/// Embedded with `include_bytes!` rather than shipped as a VPK asset: only `static/` is bundled
/// into the VPK (`package.metadata.vita`), and at 10 KB the PNG is cheaper to carry in the
/// executable than to plumb through the asset directory.
///
/// Decoded down to `LOGO_MAX_WIDTH` first. The SDL painter rounds every texture up to a power of
/// two, so the source 866x230 would land in a 1024x256 texture (1 MB of VRAM) to draw a 90pt
/// header logo; 384 wide lands in 512x128 (256 KB) and is still sharp at the splash size.
/// The result is cached - including the failure case, so a bad decode can't retry every frame.
fn geforce_logo(ctx: &egui::Context) -> Option<Arc<egui::TextureHandle>> {
    const LOGO_PNG: &[u8] = include_bytes!("../../assets/geforce-now-logo.png");
    embedded_texture(ctx, "gfn_logo", LOGO_PNG, 384)
}

/// The PlayStation face buttons, for input hints. Drawn as artwork rather than text because the
/// bundled egui font has no glyphs for them and unsupported code points render as tofu boxes.
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
///
/// Decoded at 200px wide, which is both roughly 1:1 with how large it is drawn (~184pt at
/// `UI_SCALE` 1.3) and the largest size that still lands inside a 256x256 power-of-two texture -
/// 200 wide implies 250 tall, and one more pixel of height would double the texture to 256x512.
fn cart_frame(ctx: &egui::Context) -> Option<Arc<egui::TextureHandle>> {
    const CART_PNG: &[u8] = include_bytes!("../../assets/casset.png");
    embedded_texture(ctx, "vita_cart_frame", CART_PNG, 200)
}

// Where the transparent window sits inside `casset.png`, as fractions of the image. Measured off
// the alpha channel rather than eyeballed, by walking out from the centre to the first opaque
// pixel - so the artwork stays aligned if the asset is ever re-exported at another size.
const CART_ASPECT: f32 = 447.0 / 558.0;
const CART_WINDOW_X: (f32, f32) = (0.1611, 0.8479);
const CART_WINDOW_Y: (f32, f32) = (0.0376, 0.8513);

/// Decodes a PNG compiled into the binary into exactly one cached egui texture.
///
/// Assets are embedded with `include_bytes!` rather than shipped in the VPK because only
/// `static/` is bundled (`package.metadata.vita`), and these are a few KB each.
///
/// `max_width` matters more than it looks: the SDL painter rounds every texture up to a power of
/// two, so a 866px-wide source becomes a 1024-wide texture whatever size it is drawn at. Decoding
/// the logo down to 384 puts it in a 512x128 texture (256 KiB) instead of 1024x256 (1 MiB).
/// The result is cached including the failure case, so a bad decode cannot retry every frame.
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

    // Deliberately outside any `data_mut` closure: `load_texture` takes the texture-manager lock,
    // and taking it while holding the data lock risks a re-entrant deadlock.
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

/// Resolves the currently highlighted game. `selected` indexes into `filtered_indices`, *not*
/// into `games` - indexing `games` directly (as an earlier version of every screen below did)
/// shows a different title than the one Confirm actually launches whenever a search or a
/// non-default sort is active.
pub(crate) fn selected_game<'a>(
    games: &'a [GameSummary],
    filtered_indices: &[usize],
    selected: usize,
) -> Option<&'a GameSummary> {
    games.get(*filtered_indices.get(selected)?)
}

/// Formats `id` with a single Fluent argument. Values are always passed as strings so Fluent's
/// number formatting can't inject locale-specific group separators into things like session ids
/// or byte counts.
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

/// Everything the catalog screen needs, bundled so the renderer doesn't take a dozen
/// positional arguments.
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
    /// A background page fetch is in flight, i.e. the list is about to grow.
    loading_more: bool,
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
                    loading_more: app.is_loading_more_catalog(),
                },
            ));
        }
        AppState::CreatingSession {
            games,
            selected,
            filtered_indices,
            job,
            queue_tracker,
            ..
        } => {
            let queue_status = queue_tracker
                .lock()
                .map(|st| st.clone())
                .unwrap_or_default();
            if let Some(cmd) = creating_session_screen(
                ctx,
                &i18n,
                selected_game(games, filtered_indices, *selected),
                job.is_pending(),
                &queue_status,
                app.status_note.as_deref(),
            ) {
                commands.push(cmd);
            }
        }
        AppState::SessionReady {
            games,
            selected,
            filtered_indices,
            session,
            ..
        } => {
            if let Some(cmd) = session_ready_screen(
                ctx,
                &i18n,
                selected_game(games, filtered_indices, *selected),
                session,
                app.status_note.as_deref(),
            ) {
                commands.push(cmd);
            }
        }
        AppState::Signaling {
            games,
            selected,
            filtered_indices,
            session,
            offer_sdp,
            ..
        } => {
            if let Some(cmd) = signaling_screen(
                ctx,
                &i18n,
                selected_game(games, filtered_indices, *selected),
                session,
                offer_sdp.as_deref(),
                app.status_note.as_deref(),
            ) {
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
            ) {
                commands.push(cmd);
            }
        }
        AppState::Error { message, .. } => error_screen(ctx, &i18n, message),
    }

    if app.confirm_exit {
        if let Some(cmd) = confirm_exit_modal(ctx, &i18n) {
            commands.push(cmd);
        }
    }

    splash_overlay(ctx);

    commands
}

// Startup animation timings, in seconds since the shell's first frame.
const SPLASH_FADE_IN: f64 = 0.55;
const SPLASH_HOLD: f64 = 1.05;
const SPLASH_FADE_OUT: f64 = 0.60;
const SPLASH_TOTAL: f64 = SPLASH_FADE_IN + SPLASH_HOLD + SPLASH_FADE_OUT;

/// Brief GeForce NOW splash drawn over whatever screen is already live.
///
/// Deliberately an *overlay* rather than an `AppState`: login restore and the catalog fetch both
/// start on the first frame, so gating them behind a splash state would trade real startup time
/// for the animation. This way the work happens underneath and the splash simply fades off it.
/// It also never swallows input - by the time a user could react to anything it is gone.
fn splash_overlay(ctx: &egui::Context) {
    let elapsed = ctx.input(|input| input.time);
    if elapsed >= SPLASH_TOTAL {
        return;
    }

    // Smoothstep in, linear out. `progress` also drives a slight scale-up so the logo settles
    // into place instead of just appearing.
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

    // Opaque backdrop that fades with the logo, so the screen underneath is revealed rather than
    // cut to.
    painter.rect_filled(
        screen,
        0.0,
        egui::Color32::from_rgba_unmultiplied(0x0e, 0x0e, 0x0e, alpha_u8),
    );

    let Some(logo) = geforce_logo(ctx) else {
        // No logo texture (decode failed): fall back to the wordmark as text so the startup
        // still reads as deliberate rather than as a black flash.
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

    // Accent rule that draws itself out from the centre under the logo.
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
            ui.heading(egui::RichText::new("Jade Vita").size(32.0).strong().color(ACCENT));
            ui.label(i18n.text("login-subtitle"));
            ui.add_space(24.0);
            ui.label(i18n.text("login-hint"));
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
/// cover art and a PLAY button on the right. Both halves stay on screen at once, so moving the
/// selection updates the detail live instead of pushing a separate screen.
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
                // The GeForce NOW wordmark carries the branding here; the translated
                // "catalog-library-title" string stays as the fallback if the logo can't decode.
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
                // "N of M", so a list that stops at MAX_CATALOG_PAGES doesn't look like the whole
                // catalog - and so a list that visibly grows is explained rather than startling.
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
                    // Avatar: the account's initial in an accent-filled circle.
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
                    // Signed-in indicator. We only ever render this header once the account's
                    // catalog has loaded, so reaching here *is* the "connected" condition.
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, ACCENT);

                    ui.add_space(10.0);
                    if let Some(cmd) = language_picker(ui, view.locale, view.user) {
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
            ui.label(
                egui::RichText::new(i18n.text("catalog-footer-hint"))
                    .size(11.0)
                    .color(TEXT_DIM),
            );
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

/// Language button + popup listing the available UI languages, with the signed-in account shown
/// above them - the header itself only has room for the display name, so this is where you
/// confirm *which* NVIDIA account you're on.
///
/// Labelled "Aa" rather than a gear or globe glyph: egui's bundled proportional font has a
/// limited glyph set and unsupported code points render as a tofu box instead of being skipped.
fn language_picker(
    ui: &mut egui::Ui,
    current: crate::locale::Locale,
    user: &GfnUser,
) -> Option<AppCommand> {
    let mut command = None;
    let response = ui.add_sized(
        [34.0, 30.0],
        egui::Button::new(egui::RichText::new("Aa").size(13.0)).fill(BG_RAISED),
    );
    let popup_id = ui.make_persistent_id("language_picker_popup");
    if response.clicked() {
        ui.memory_mut(|mem| mem.toggle_popup(popup_id));
    }
    egui::popup_below_widget(
        ui,
        popup_id,
        &response,
        egui::PopupCloseBehavior::CloseOnClick,
        |ui| {
            ui.set_min_width(190.0);
            if let Some(email) = &user.email {
                ui.label(egui::RichText::new(email).size(10.0).color(TEXT_DIM));
                ui.separator();
            }
            for candidate in crate::locale::Locale::ALL {
                if ui
                    .selectable_label(candidate == current, candidate.label())
                    .clicked()
                {
                    command = Some(AppCommand::SetLocale(candidate));
                }
            }
        },
    );
    command
}

/// Sort button + popup. Uses a plain "Sort: " text prefix rather than an arrow glyph: egui's
/// bundled proportional font has a limited glyph set and unsupported code points render as a
/// tofu box rather than simply being skipped.
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
                // GFN only reports `lastPlayedDate` for titles this account has actually
                // launched through GFN before - most accounts will see 0 here. Showing the
                // count makes that visible instead of the option silently doing nothing.
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
    // Result count lives inside the field's placeholder, as in the reference client - it keeps
    // the count next to what it describes without spending a separate row of a 418pt-tall screen.
    let hint = if view.search_query.is_empty() {
        format!(
            "{}  ({})",
            i18n.text("catalog-search-hint"),
            view.filtered_indices.len()
        )
    } else {
        i18n.text("catalog-search-hint")
    };
    let response = ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text(hint)
            .desired_width(ui.available_width())
            .margin(egui::vec2(8.0, 6.0)),
    );
    if view.search_requested && !response.has_focus() {
        response.request_focus();
    }
    if response.gained_focus() && !view.search_requested {
        commands.push(AppCommand::RequestSearch);
    }
    if response.changed() {
        commands.push(AppCommand::SetSearchQuery(query));
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
    // Row text is laid out once per row into a truncating single-line galley and painted through
    // the scroll area's *shared* painter (`ui.painter()`). Both details matter on Vita:
    //
    // - A per-row `painter_at(rect)` would give every row its own clip rect, and egui emits a
    //   separate `ClippedPrimitive` per distinct clip rect - i.e. one `set_clip_rect` +
    //   `render_geometry` SDL draw call *per row* instead of one batched mesh for the whole list.
    // - `TextWrapping::truncate_at_width` keeps layout to a single row and elides the overflow,
    //   so no clip rect is needed to contain long titles, and it skips the multi-row wrapping
    //   work a plain `layout(.., wrap_width, ..)` call does.
    let font_id = egui::FontId::proportional(12.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        // egui's kinetic drag-scroll keeps gliding after the finger lifts, which reads as the
        // list "floating" away from the d-pad selection, so it is turned off here.
        .drag_to_scroll(false)
        .show_rows(ui, ROW_HEIGHT, total, |ui, row_range| {
            let mut selected_response: Option<egui::Response> = None;
            let painter = ui.painter().clone();
            for row in row_range {
                let Some(&game_index) = view.filtered_indices.get(row) else {
                    continue;
                };
                let game = &view.games[game_index];
                let is_selected = row == view.selected;

                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ROW_HEIGHT - 3.0),
                    egui::Sense::click(),
                );
                if !ui.is_rect_visible(rect) {
                    if is_selected {
                        selected_response = Some(response);
                    }
                    continue;
                }

                // Every row gets a rounded plate; the selected one is lighter and carries a
                // full accent outline, which reads far better than a bare highlight on a 5"
                // panel viewed at arm's length.
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

                // Row thumbnail. Requested here (only for rows actually on screen) at the small
                // `Icon` size, so a screenful costs ~16 KiB each rather than a full cover apiece.
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
                        // Cover-fit: box art is portrait, the slot is square, so crop rather
                        // than squash.
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

                if response.clicked() {
                    commands.push(AppCommand::SelectGame(row));
                }
                if is_selected {
                    selected_response = Some(response);
                }
            }

            // D-pad navigation only moves `selected`; keep the active row on screen without
            // making the user drag the list. Only scroll when the selection actually changed -
            // `scroll_to_me` recomputes its delta from the live layout every call, and with the
            // renderer's nearest-neighbour sampling even a sub-pixel delta shows up as jitter.
            if let Some(response) = selected_response {
                let scroll_state_id = egui::Id::new("catalog_list_last_scrolled_selected");
                let already_scrolled =
                    ui.ctx().data(|d| d.get_temp::<usize>(scroll_state_id)) == Some(view.selected);
                if !already_scrolled {
                    response.scroll_to_me(None);
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(scroll_state_id, view.selected));
                }
            }
        });

    commands
}

/// Right-hand detail panel: big cover, title, metadata and the PLAY button for whichever game
/// the list has highlighted.
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

    // Only the highlighted game's cover is downloaded, rather than one per visible row. Covers
    // are cached forever (`CoverStore` has no eviction) and each decodes to a 256px RGBA
    // texture, so fetching a screenful at a time would grow VRAM use fast on a console with
    // very little of it to spare.
    if let Some(url) = game.cover_url.clone() {
        view.covers
            .request(view.http_client, ctx, game.app_id.clone(), url);
    }

    // The selected game's own art, dimmed, filling the panel behind everything - the look the
    // reference client uses. Painted before the content so the text and PLAY button land on top.
    draw_panel_backdrop(ui, ctx, view.covers, game);

    // Sized so the cartridge fits between the header and footer with the info column still wide
    // enough for a two-line title.
    let cart_height = 226.0;

    ui.horizontal(|ui| {
        draw_cover(ui, ctx, view.covers, game, cart_height);

        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(&game.title)
                    .size(19.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            );
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
            // "Press (X) to Start", with the real face-button artwork inline.
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
/// only does flat fills. Returns whether it was activated this frame.
///
/// The gradient is a single `Mesh` with per-vertex colours (two triangles), so it costs the same
/// as the flat rectangle it replaces.
fn play_button(ui: &mut egui::Ui, i18n: &I18n) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(200.0, 44.0), egui::Sense::click());
    let painter = ui.painter();

    // Lift the whole ramp slightly while pressed/hovered rather than swapping to a flat colour,
    // so the button keeps its shape as feedback.
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
    // Rounded ends are drawn as flat circles in the mid tone; the gradient covers the straight
    // middle. At this size the seam is invisible and it avoids tessellating a rounded gradient.
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

/// How strongly the backdrop art shows through. The Vita's panel is 5", so this is deliberately
/// dimmer than the reference client's - its text carries a drop shadow that ours doesn't, and
/// legibility of the title and metadata wins over the artwork.
const BACKDROP_ALPHA: u8 = 58;

/// Paints the selected game's cover across the whole detail panel as a dimmed backdrop.
///
/// Costs no extra memory: this is the *same* cached texture the cover thumbnail already uses, so
/// it is one more quad per frame with no allocation and nothing new for the LRU to hold. Being
/// only 256px (see `covers::MAX_COVER_DIM`) stretched over the full panel, it comes out soft -
/// which is the blurred look the reference client has anyway.
///
/// Uses cover-fit (crop to fill) rather than aspect-fit: letterbox bars behind the content would
/// look like a rendering mistake rather than a backdrop.
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
    // Narrow the UV window on whichever axis overflows, keeping the crop centred.
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

/// Trims an ISO-8601 timestamp down to its `YYYY-MM-DD` date part. GFN sends full timestamps and
/// the panel only has room for the date.
fn short_date(iso: &str) -> &str {
    iso.split('T').next().unwrap_or(iso)
}

/// Draws the cover art seated inside a PS Vita cartridge shell.
///
/// `cart_height` sizes the whole card; the artwork slot is derived from the measured window
/// fractions so it lines up with the shell's opening. Order matters: art first, shell on top -
/// the shell's alpha is what frames it, and its opaque material hides the square corners of the
/// artwork behind the window's rounded ones.
///
/// Falls back to a plain framed slot if the shell asset can't be decoded, so a bad asset costs
/// the decoration and nothing else.
fn draw_cover(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    covers: &CoverStore,
    game: &GameSummary,
    cart_height: f32,
) {
    let cart_width = cart_height * CART_ASPECT;
    let (cart, _) =
        ui.allocate_exact_size(egui::vec2(cart_width, cart_height), egui::Sense::hover());
    let shell = cart_frame(ctx);

    // Shared painter, not `painter_at`: a distinct clip rect costs an extra `ClippedPrimitive`
    // (and so an extra SDL draw call) for no benefit here - everything is fitted inside `rect`.
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

    match covers.get(&game.app_id) {
        Some(CoverSnapshot::Ready(image)) => {
            let tex = image.texture(
                ctx,
                &CoverStore::texture_key(&game.app_id, CoverSize::Cover),
            );
            let tex_size = tex.size_vec2();
            let src_aspect = tex_size.x / tex_size.y.max(1.0);
            let slot_aspect = rect.width() / rect.height();
            // Cover-fit (crop) rather than aspect-fit: the shell's window is a fixed shape, and
            // letterbox bars inside a cartridge would read as a bug rather than as framing.
            let uv = if src_aspect > slot_aspect {
                let inset = (1.0 - slot_aspect / src_aspect) / 2.0;
                egui::Rect::from_min_max(egui::pos2(inset, 0.0), egui::pos2(1.0 - inset, 1.0))
            } else {
                let inset = (1.0 - src_aspect / slot_aspect) / 2.0;
                egui::Rect::from_min_max(egui::pos2(0.0, inset), egui::pos2(1.0, 1.0 - inset))
            };
            painter.image(tex.id(), rect, uv, egui::Color32::WHITE);
        }
        Some(CoverSnapshot::Loading) => {
            ui.put(rect, egui::Spinner::new());
        }
        Some(CoverSnapshot::Failed) | None => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                game.title.chars().next().unwrap_or('?').to_string(),
                egui::FontId::proportional(48.0),
                TEXT_DIM,
            );
        }
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
/// Unrecognized stores fall back to a neutral "Game" pill rather than hiding the badge.
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
fn session_header(ui: &mut egui::Ui, i18n: &I18n, title: String) -> Option<AppCommand> {
    let mut command = None;
    ui.horizontal(|ui| {
        ui.heading(egui::RichText::new(title).size(20.0).color(egui::Color32::WHITE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(i18n.text("session-stop-button")).color(DANGER),
                    )
                    .fill(BG_RAISED),
                )
                .clicked()
            {
                command = Some(AppCommand::ToggleConfirmExit);
            }
        });
    });
    ui.separator();
    command
}

fn creating_session_screen(
    ctx: &egui::Context,
    i18n: &I18n,
    game: Option<&GameSummary>,
    is_polling: bool,
    queue_status: &crate::gfn::cloudmatch::QueueStatus,
    status_note: Option<&str>,
) -> Option<AppCommand> {
    let mut command = None;
    egui::CentralPanel::default().show(ctx, |ui| {
        command = session_header(ui, i18n, i18n.text("session-creating-title"));

        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.spinner();
            ui.add_space(16.0);
            match game {
                Some(game) => ui.heading(
                    egui::RichText::new(text1(
                        i18n,
                        "session-preparing-game",
                        "game",
                        &game.title,
                    ))
                    .size(18.0),
                ),
                None => ui.heading(egui::RichText::new(i18n.text("session-preparing")).size(18.0)),
            };
            if is_polling {
                ui.add_space(16.0);
                if queue_status.queue_position > 0 {
                    ui.label(
                        egui::RichText::new(text1(
                            i18n,
                            "session-queue-position",
                            "position",
                            queue_status.queue_position,
                        ))
                        .color(ACCENT)
                        .strong()
                        .size(17.0),
                    );
                    ui.add_space(8.0);
                    if queue_status.eta_ms > 0 {
                        let secs = (queue_status.eta_ms / 1000) % 60;
                        let mins = queue_status.eta_ms / 60000;
                        let eta = if mins > 0 {
                            text2(
                                i18n,
                                "session-eta-minutes",
                                ("minutes", mins),
                                ("seconds", secs),
                            )
                        } else {
                            text1(i18n, "session-eta-seconds", "seconds", secs)
                        };
                        ui.label(egui::RichText::new(eta).size(14.0));
                    }
                    ui.add_space(6.0);
                    ui.weak(text1(
                        i18n,
                        "session-queue-live",
                        "attempt",
                        queue_status.attempt,
                    ));
                } else if queue_status.attempt > 0 {
                    ui.label(
                        egui::RichText::new(text1(
                            i18n,
                            "session-connecting-attempt",
                            "attempt",
                            queue_status.attempt,
                        ))
                        .size(14.0),
                    );
                } else {
                    ui.label(i18n.text("session-waiting-ready"));
                }
            }
        });

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            if let Some(note) = status_note {
                ui.label(egui::RichText::new(note).italics().size(11.0).color(TEXT_DIM));
            }
            ui.label(
                egui::RichText::new(i18n.text("session-exit-hint"))
                    .size(11.0)
                    .color(TEXT_DIM),
            );
        });
    });
    command
}

fn session_ready_screen(
    ctx: &egui::Context,
    i18n: &I18n,
    game: Option<&GameSummary>,
    session: &crate::gfn::cloudmatch::SessionInfo,
    status_note: Option<&str>,
) -> Option<AppCommand> {
    let mut command = None;
    egui::CentralPanel::default().show(ctx, |ui| {
        command = session_header(ui, i18n, i18n.text("session-ready-title"));

        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            ui.add_space(12.0);
            if let Some(game) = game {
                ui.heading(
                    egui::RichText::new(text1(i18n, "session-game", "game", &game.title))
                        .size(17.0),
                );
                ui.add_space(8.0);
            }
            ui.label(text1(i18n, "session-id", "id", &session.session_id));
            ui.label(text1(i18n, "session-server-ip", "ip", &session.server_ip));
            ui.label(text1(
                i18n,
                "session-signaling",
                "server",
                &session.signaling_server,
            ));
            ui.label(text1(
                i18n,
                "session-signaling-url",
                "url",
                &session.signaling_url,
            ));
            if let Some(profile) = &session.negotiated_stream_profile {
                if let Some(res) = &profile.resolution {
                    ui.label(text1(i18n, "session-resolution", "value", res));
                }
                if let Some(fps) = profile.fps {
                    ui.label(text1(i18n, "session-fps", "value", fps));
                }
                if let Some(codec) = &profile.codec {
                    ui.label(text1(i18n, "session-codec", "value", codec));
                }
            }
            ui.add_space(16.0);
            ui.label(i18n.text("session-ready-hint"));
        });

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            if let Some(note) = status_note {
                ui.label(egui::RichText::new(note).italics().size(11.0).color(TEXT_DIM));
            }
            ui.label(
                egui::RichText::new(i18n.text("session-ready-footer"))
                    .size(11.0)
                    .color(TEXT_DIM),
            );
        });
    });
    command
}

fn signaling_screen(
    ctx: &egui::Context,
    i18n: &I18n,
    game: Option<&GameSummary>,
    session: &crate::gfn::cloudmatch::SessionInfo,
    offer_sdp: Option<&str>,
    status_note: Option<&str>,
) -> Option<AppCommand> {
    let mut command = None;
    egui::CentralPanel::default().show(ctx, |ui| {
        command = session_header(ui, i18n, i18n.text("signaling-title"));

        ui.vertical(|ui| {
            ui.add_space(16.0);
            if let Some(game) = game {
                ui.heading(
                    egui::RichText::new(text1(i18n, "session-game", "game", &game.title))
                        .size(17.0),
                );
                ui.add_space(8.0);
            }
            ui.label(text1(i18n, "signaling-session", "id", &session.session_id));
            ui.add_space(12.0);
            ui.spinner();
            ui.add_space(12.0);
            match offer_sdp {
                Some(sdp) => {
                    ui.label(text1(i18n, "signaling-offer-received", "bytes", sdp.len()));
                }
                None => {
                    ui.label(i18n.text("signaling-waiting-offer"));
                }
            }
        });

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            if let Some(note) = status_note {
                ui.label(egui::RichText::new(note).italics().size(11.0).color(TEXT_DIM));
            }
            ui.label(
                egui::RichText::new(i18n.text("session-exit-hint"))
                    .size(11.0)
                    .color(TEXT_DIM),
            );
        });
    });
    command
}

fn confirm_exit_modal(ctx: &egui::Context, i18n: &I18n) -> Option<AppCommand> {
    let mut command = None;
    egui::Window::new(i18n.text("exit-window-title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
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
) -> Option<AppCommand> {
    let mut command = None;

    // The video itself is drawn by the shell (`surface::draw_scene`) straight from the SDL
    // textures the frame producer writes into - the direct-texture path. This panel
    // is transparent so that quad shows through; egui only overlays UI on top.
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
                // Live pipeline stage from the peer thread - the key diagnostic when the
                // stream stalls before the first decoded frame.
                ui.label(
                    status_note
                        .map(str::to_owned)
                        .unwrap_or_else(|| i18n.text("streaming-waiting-negotiation")),
                );
            });
        }

        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new(i18n.text("session-stop-button")).color(DANGER),
                    )
                    .fill(BG_RAISED),
                )
                .clicked()
            {
                command = Some(AppCommand::ToggleConfirmExit);
            }
        });

        // Small always-on pipeline readout - kept visible over the video so a black frame is
        // still diagnosable from a screenshot.
        if let Some(note) = status_note {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.add_space(6.0);
                ui.colored_label(
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 140),
                    egui::RichText::new(note).size(11.0),
                );
            });
        }
    });

    command
}

fn error_screen(ctx: &egui::Context, i18n: &I18n, message: &str) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(70.0);
            ui.heading(egui::RichText::new(i18n.text("error-title")).size(22.0).color(DANGER));
            ui.add_space(12.0);
            ui.label(message);
            ui.add_space(24.0);
            ui.label(
                egui::RichText::new(i18n.text("error-hint"))
                    .size(11.0)
                    .color(TEXT_DIM),
            );
        });
    });
}

/// Draws a QR code's module grid as plain filled rects (not an image/texture blit) - adapted
/// from green-vita (MPL-2.0), src/app/ui/screens/token_setup.rs. See THIRD_PARTY_NOTICES.md.
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
