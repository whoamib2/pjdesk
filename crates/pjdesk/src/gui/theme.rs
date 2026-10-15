//! A calm dark theme and a few shared colors, so the app doesn't read as the
//! default egui demo.

use eframe::egui::{self, Color32};

pub const ACCENT: Color32 = Color32::from_rgb(0x4d, 0x9b, 0xff);
pub const OK: Color32 = Color32::from_rgb(0x3f, 0xcf, 0x6b);
pub const WARN: Color32 = Color32::from_rgb(0xf2, 0xb0, 0x3a);
pub const MUTED: Color32 = Color32::from_rgb(0x9a, 0xa2, 0xb1);
pub const CARD: Color32 = Color32::from_rgb(0x1c, 0x20, 0x28);
pub const PANEL: Color32 = Color32::from_rgb(0x14, 0x17, 0x1d);

pub fn apply(ctx: &egui::Context) {
    // Force dark, so the app looks the same whatever the OS theme is.
    ctx.set_theme(egui::ThemePreference::Dark);

    let mut v = egui::Visuals::dark();
    v.dark_mode = true;
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::from_rgb(0x0e, 0x10, 0x15);
    v.faint_bg_color = Color32::from_rgb(0x20, 0x25, 0x2e);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(0x2a, 0x30, 0x3b);
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x26, 0x2c, 0x37);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x22, 0x27, 0x31);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x30, 0x38, 0x46);
    v.widgets.active.bg_fill = ACCENT.linear_multiply(0.5);
    v.window_corner_radius = egui::CornerRadius::same(10);
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(7);
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(7);
    v.widgets.active.corner_radius = egui::CornerRadius::same(7);

    // Apply to the dark theme slot and the current frame.
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals(v);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.window_margin = egui::Margin::same(14);
    });
}

/// A rounded card container with a subtle fill, for grouping a section.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(CARD)
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0x2a, 0x30, 0x3b)))
        .show(ui, add)
        .inner
}
