//! Look and feel: a dark theme with rounded cards, larger text and a few
//! reusable widgets (cards, chips, big action buttons).

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, TextStyle, Vec2};

pub const BG: Color32 = Color32::from_rgb(0x12, 0x14, 0x18);
pub const SURFACE: Color32 = Color32::from_rgb(0x1b, 0x1e, 0x24);
pub const SURFACE_2: Color32 = Color32::from_rgb(0x24, 0x28, 0x30);
pub const BORDER: Color32 = Color32::from_rgb(0x2e, 0x33, 0x3d);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe9, 0xef);
pub const MUTED: Color32 = Color32::from_rgb(0x8d, 0x95, 0xa5);
pub const ACCENT: Color32 = Color32::from_rgb(0x3b, 0x82, 0xf6);
pub const OK: Color32 = Color32::from_rgb(0x22, 0xc5, 0x5e);
pub const DANGER: Color32 = Color32::from_rgb(0xef, 0x44, 0x44);
pub const WARN: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);

const RADIUS: u8 = 10;

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(14.0, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(10.0, 10.0);
        s.button_padding = Vec2::new(14.0, 8.0);
        s.interact_size.y = 34.0;
        s.window_margin = Margin::same(16);

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(TEXT);
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(14);
        v.extreme_bg_color = Color32::from_rgb(0x0d, 0x0f, 0x12);
        v.faint_bg_color = SURFACE;
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.hyperlink_color = ACCENT;
        v.error_fg_color = DANGER;
        v.warn_fg_color = WARN;

        let w = &mut v.widgets;
        for (state, fill) in [
            (&mut w.inactive, SURFACE_2),
            (&mut w.hovered, Color32::from_rgb(0x2f, 0x35, 0x40)),
            (&mut w.active, Color32::from_rgb(0x38, 0x3f, 0x4c)),
            (&mut w.open, SURFACE_2),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.corner_radius = CornerRadius::same(RADIUS);
            state.bg_stroke = Stroke::new(1.0, BORDER);
            state.fg_stroke = Stroke::new(1.0, TEXT);
        }
        w.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        w.noninteractive.bg_fill = SURFACE;
        w.noninteractive.weak_bg_fill = SURFACE;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.corner_radius = CornerRadius::same(RADIUS);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    });
}

/// A rounded panel with an optional title.
pub fn card<R>(ui: &mut egui::Ui, title: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(t) = title {
                ui.label(RichText::new(t.to_uppercase()).size(12.0).color(MUTED).strong());
                ui.add_space(2.0);
            }
            add(ui)
        })
        .inner
}

/// Small coloured pill, e.g. "● Online".
pub fn chip(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| ui.label(RichText::new(text.into()).size(13.0).color(color).strong()))
        .response
}

/// Prominent action button.
pub fn big_button(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text.into()).size(16.0).color(Color32::WHITE).strong())
            .fill(color)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(RADIUS))
            .min_size(Vec2::new(0.0, 42.0)),
    )
}

/// Secondary text.
pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(MUTED)
}
