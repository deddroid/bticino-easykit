//! Fallback font for symbols egui's bundled fonts lack (arrows, dots, ■ ▶).

use eframe::egui;

const CANDIDATES: &[&str] = &[
    // Linux
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSansSymbols2-Regular.ttf",
    // Windows
    "C:\\Windows\\Fonts\\seguisym.ttf",
    "C:\\Windows\\Fonts\\segoeui.ttf",
];

pub fn install(ctx: &egui::Context) {
    let Some(bytes) = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) else { return };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("fallback".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("fallback".into());
    }
    ctx.set_fonts(fonts);
}
