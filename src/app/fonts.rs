
use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

const JP_FONT: &[u8] = include_bytes!("../../assets/fonts/NotoSansJP-Subset.otf");

pub(crate) fn configure(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "noto-jp".to_owned(),
        Arc::new(FontData::from_static(JP_FONT)),
    );

    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .get_mut(&family)
            .expect("default font family")
            .push("noto-jp".to_owned());
    }

    ctx.set_fonts(fonts);
}
