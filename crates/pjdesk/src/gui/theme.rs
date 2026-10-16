//! A refined, cohesive dark theme — a calm "control room" look, so the app
//! doesn't read as the default egui demo.

use eframe::egui::{self, Color32};

// Brand + semantic colors.
pub const ACCENT: Color32 = Color32::from_rgb(0x4d, 0x9b, 0xff);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x6b, 0xad, 0xff);
pub const ACCENT_INK: Color32 = Color32::from_rgb(0x07, 0x11, 0x1f);
pub const OK: Color32 = Color32::from_rgb(0x46, 0xd1, 0x7d);
pub const WARN: Color32 = Color32::from_rgb(0xf2, 0xb0, 0x3a);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);

// Text + surfaces.
pub const TEXT: Color32 = Color32::from_rgb(0xe9, 0xed, 0xf3);
pub const MUTED: Color32 = Color32::from_rgb(0x8c, 0x95, 0xa6);
pub const PANEL: Color32 = Color32::from_rgb(0x0d, 0x0f, 0x14);
pub const CARD: Color32 = Color32::from_rgb(0x17, 0x1b, 0x23);
pub const ELEV: Color32 = Color32::from_rgb(0x20, 0x25, 0x30);
pub const LINE: Color32 = Color32::from_rgb(0x29, 0x30, 0x3c);

pub fn apply(ctx: &egui::Context) {
    // Force dark, so the app looks the same whatever the OS theme is.
    ctx.set_theme(egui::ThemePreference::Dark);

    let mut v = egui::Visuals::dark();
    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.window_stroke = egui::Stroke::new(1.0, LINE);
    v.extreme_bg_color = Color32::from_rgb(0x0a, 0x0c, 0x11);
    v.faint_bg_color = ELEV;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.linear_multiply(0.30);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);

    v.widgets.noninteractive.bg_stroke.color = LINE;
    v.widgets.noninteractive.fg_stroke.color = TEXT;

    // Inputs / resting controls.
    v.widgets.inactive.bg_fill = ELEV;
    v.widgets.inactive.weak_bg_fill = ELEV;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
    v.widgets.inactive.fg_stroke.color = TEXT;

    v.widgets.hovered.bg_fill = Color32::from_rgb(0x2a, 0x31, 0x3e);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x3b, 0x45, 0x56));
    v.widgets.hovered.fg_stroke.color = Color32::WHITE;

    v.widgets.active.bg_fill = Color32::from_rgb(0x32, 0x3b, 0x4a);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke.color = Color32::WHITE;

    let r = egui::CornerRadius::same(9);
    v.window_corner_radius = egui::CornerRadius::same(14);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;

    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals(v);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(14.0, 9.0);
        style.spacing.window_margin = egui::Margin::same(16);
        style.spacing.menu_margin = egui::Margin::same(8);
        style.spacing.interact_size.y = 32.0;

        use egui::{FontFamily::Proportional, FontId, TextStyle};
        style.text_styles.insert(TextStyle::Heading, FontId::new(22.0, Proportional));
        style.text_styles.insert(TextStyle::Body, FontId::new(14.5, Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(14.5, Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(12.0, Proportional));
    });
}

/// A rounded card container with a subtle fill + border, for a section.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(CARD)
        .corner_radius(egui::CornerRadius::same(14))
        .inner_margin(egui::Margin::same(18))
        .stroke(egui::Stroke::new(1.0, LINE))
        .show(ui, add)
        .inner
}

/// A compact rounded "pill" with a tinted background, for status and tags.
pub fn pill(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::NONE
        .fill(color.linear_multiply(0.14))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.colored_label(color, egui::RichText::new("●").size(10.0));
                ui.label(egui::RichText::new(text).color(TEXT).size(12.5));
            });
        });
}
