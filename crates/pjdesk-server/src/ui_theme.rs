//! Same light and dark visuals as the client window.

#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle};

#[derive(Clone, Copy)]
pub struct Paint {
    pub accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub card: Color32,
    pub elev: Color32,
    pub line: Color32,
    pub on_accent: Color32,
    pub shadow: Color32,
    pub panel: Color32,
}

pub fn paint_dark() -> Paint {
    Paint {
        accent: Color32::from_rgb(0x4d, 0x9b, 0xff),
        ok: Color32::from_rgb(0x46, 0xd1, 0x7d),
        warn: Color32::from_rgb(0xf2, 0xb0, 0x3a),
        text: Color32::from_rgb(0xe9, 0xed, 0xf3),
        muted: Color32::from_rgb(0x9a, 0xa3, 0xb5),
        panel: Color32::from_rgb(0x10, 0x13, 0x18),
        card: Color32::from_rgb(0x18, 0x1c, 0x24),
        elev: Color32::from_rgb(0x22, 0x28, 0x33),
        line: Color32::from_rgb(0x2e, 0x36, 0x44),
        on_accent: Color32::WHITE,
        shadow: Color32::from_black_alpha(48),
    }
}

pub fn paint_light() -> Paint {
    Paint {
        accent: Color32::from_rgb(0x2f, 0x6f, 0xed),
        ok: Color32::from_rgb(0x1c, 0x8a, 0x4c),
        warn: Color32::from_rgb(0xb4, 0x6b, 0x00),
        text: Color32::from_rgb(0x1c, 0x23, 0x30),
        muted: Color32::from_rgb(0x5c, 0x67, 0x78),
        panel: Color32::from_rgb(0xf3, 0xf5, 0xf8),
        card: Color32::WHITE,
        elev: Color32::from_rgb(0xe8, 0xec, 0xf2),
        line: Color32::from_rgb(0xd5, 0xdb, 0xe3),
        on_accent: Color32::WHITE,
        shadow: Color32::from_black_alpha(22),
    }
}

pub fn paint(ui: &egui::Ui) -> Paint {
    if ui.visuals().dark_mode {
        paint_dark()
    } else {
        paint_light()
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".to_owned(),
        Arc::new(FontData::from_static(include_bytes!("../../pjdesk/assets/Inter-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "InterSemi".to_owned(),
        Arc::new(FontData::from_static(include_bytes!("../../pjdesk/assets/Inter-SemiBold.ttf"))),
    );
    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        prop.insert(0, "Inter".to_owned());
    }
    fonts.families.insert(FontFamily::Name("InterSemi".into()), vec!["InterSemi".to_owned()]);
    ctx.set_fonts(fonts);
}

/// `theme` is "dark", "light", or anything else (follow Windows).
pub fn apply(ctx: &egui::Context, theme: &str) {
    install_fonts(ctx);
    let pref = match theme {
        "dark" => egui::ThemePreference::Dark,
        "light" => egui::ThemePreference::Light,
        _ => egui::ThemePreference::System,
    };
    ctx.set_theme(pref);
    ctx.set_visuals_of(egui::Theme::Dark, visuals(paint_dark(), true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(paint_light(), false));
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);
        style.spacing.window_margin = Margin::same(16);
        style.spacing.interact_size.y = 36.0;
        style.text_styles.insert(TextStyle::Heading, FontId::new(22.0, FontFamily::Name("InterSemi".into())));
        style.text_styles.insert(TextStyle::Body, FontId::new(14.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(14.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(12.5, FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace));
    });
}

fn visuals(p: Paint, dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.dark_mode = dark;
    v.override_text_color = Some(p.text);
    v.panel_fill = p.panel;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.extreme_bg_color = if dark { Color32::from_rgb(0x0b, 0x0d, 0x12) } else { Color32::from_rgb(0xe7, 0xeb, 0xf0) };
    v.faint_bg_color = p.elev;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.linear_multiply(0.28);
    v.warn_fg_color = p.warn;
    v.error_fg_color = Color32::from_rgb(0xe0, 0x5a, 0x4f);
    v.widgets.noninteractive.fg_stroke.color = p.text;
    v.widgets.inactive.bg_fill = p.elev;
    v.widgets.inactive.weak_bg_fill = p.elev;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.inactive.fg_stroke.color = p.text;
    let hover = if dark { Color32::from_rgb(0x2c, 0x34, 0x42) } else { Color32::from_rgb(0xdc, 0xe3, 0xee) };
    v.widgets.hovered.bg_fill = hover;
    v.widgets.hovered.weak_bg_fill = hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent.linear_multiply(0.8));
    v.widgets.hovered.fg_stroke.color = p.text;
    let active = if dark { Color32::from_rgb(0x34, 0x3e, 0x4e) } else { Color32::from_rgb(0xd0, 0xd8, 0xe6) };
    v.widgets.active.bg_fill = active;
    v.widgets.active.weak_bg_fill = active;
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.active.fg_stroke.color = p.text;
    v.widgets.open.bg_fill = p.card;
    let r = CornerRadius::same(10);
    v.window_corner_radius = CornerRadius::same(12);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    v.widgets.open.corner_radius = r;
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: p.shadow };
    v
}

pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let p = paint(ui);
    egui::Frame::NONE
        .fill(p.card)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .stroke(Stroke::new(1.0, p.line))
        .shadow(egui::Shadow { offset: [0, 1], blur: 8, spread: 0, color: p.shadow })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}
