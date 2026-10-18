//! Shared controls: fields, icon buttons, checkboxes, toasts.
//!
//! Password fields always keep a Show/Hide label. Icon buttons are drawn
//! shapes with a tooltip, never a character the font does not contain.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Label, RichText, Sense, Stroke, StrokeKind};

use super::icons::{self, Icon};
use super::theme;

/// Host permanent password: 8 characters. Account passwords are a separate,
/// stricter rule enforced by the server.
pub const PERMANENT_PASSWORD_HINT: &str = "At least 8 characters";

/// A brief message that fades after a few seconds.
#[derive(Default)]
pub struct Toast {
    text: String,
    until: Option<Instant>,
}

impl Toast {
    pub fn show(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.until = Some(Instant::now() + Duration::from_secs(3));
    }

    pub fn render(&mut self, ctx: &egui::Context) {
        let Some(until) = self.until else { return };
        if Instant::now() >= until {
            self.until = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        let p = theme::paint_ctx(ctx);
        egui::Area::new(egui::Id::new("toast"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(p.elev)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .stroke(Stroke::new(1.0, p.line))
                    .show(ui, |ui| {
                        ui.colored_label(p.text, &self.text);
                    });
            });
    }
}

pub fn copy_to_clipboard(text: &str) -> bool {
    match arboard::Clipboard::new() {
        Ok(mut cb) => cb.set_text(text.to_string()).is_ok(),
        Err(_) => false,
    }
}

pub fn heading(ui: &mut egui::Ui, text: &str) {
    let p = theme::paint(ui);
    ui.label(RichText::new(text).color(p.muted).size(11.5).strong());
    ui.add_space(4.0);
}

pub fn field_label(ui: &mut egui::Ui, text: &str) {
    let p = theme::paint(ui);
    ui.label(RichText::new(text).color(p.muted).size(12.5));
    ui.add_space(4.0);
}

pub fn text_field(ui: &mut egui::Ui, value: &mut String, hint: &str, monospace: bool) -> egui::Response {
    let font = if monospace { egui::TextStyle::Monospace } else { egui::TextStyle::Body };
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .font(font)
            .margin(egui::Margin::symmetric(12, 10))
            .desired_width(f32::INFINITY),
    )
}

/// Password field with a Show/Hide toggle. The toggle is a word, not an icon,
/// so it stays readable in every font.
pub fn password_field(ui: &mut egui::Ui, value: &mut String, hint: &str, shown: &mut bool) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let show_w = 64.0;
        let width = (ui.available_width() - show_w - 8.0).max(48.0);
        let resp = ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .password(!*shown)
                .desired_width(width)
                .margin(egui::Margin::symmetric(12, 10)),
        );
        let label = if *shown { "Hide" } else { "Show" };
        if ui.add_sized([show_w, 36.0], egui::Button::new(label)).on_hover_text(label).clicked() {
            *shown = !*shown;
        }
        resp
    })
    .inner
}

pub fn accent_button(ui: &mut egui::Ui, label: &str, size: egui::Vec2) -> bool {
    let p = theme::paint(ui);
    ui.add_sized(size, egui::Button::new(RichText::new(label).strong().color(p.on_accent)).fill(p.accent)).clicked()
}

/// Square icon button. The tooltip is required so the shape is never the only label.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, tip: &str) -> egui::Response {
    icon_button_active(ui, icon, tip, false)
}

/// Like [`icon_button`], but drawn in the accent color when `active` (a filled star, for example).
pub fn icon_button_active(ui: &mut egui::Ui, icon: Icon, tip: &str, active: bool) -> egui::Response {
    let p = theme::paint(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), Sense::click());
    let bg = if resp.is_pointer_button_down_on() {
        p.accent.linear_multiply(0.35)
    } else if resp.hovered() {
        p.elev
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
    }
    let color = if active {
        p.accent
    } else if resp.hovered() {
        p.text
    } else {
        p.muted
    };
    icons::paint(ui.painter(), rect.shrink(7.0), icon, color);
    resp.on_hover_text(tip)
}

/// Checkbox drawn by us, so the check mark is not a missing font glyph.
pub fn checkbox(ui: &mut egui::Ui, on: &mut bool, label: &str) -> bool {
    let p = theme::paint(ui);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, box_resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::click());
        let fill = if *on {
            p.accent
        } else if box_resp.hovered() {
            p.elev
        } else {
            p.card
        };
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(4),
            Stroke::new(1.0, if *on { p.accent } else { p.line }),
            StrokeKind::Inside,
        );
        if *on {
            icons::paint(ui.painter(), rect.shrink(3.0), Icon::Check, Color32::WHITE);
        }
        let lab = ui.add(Label::new(label).sense(Sense::click()));
        if box_resp.clicked() || lab.clicked() {
            *on = !*on;
            changed = true;
        }
    });
    changed
}

pub fn radio(ui: &mut egui::Ui, current: &mut String, value: &str, label: &str) -> bool {
    let p = theme::paint(ui);
    let selected = current == value;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, mark) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::click());
        ui.painter().circle_stroke(rect.center(), 8.0, Stroke::new(1.4, if selected { p.accent } else { p.line }));
        if selected {
            ui.painter().circle_filled(rect.center(), 4.5, p.accent);
        }
        let lab = ui.add(Label::new(label).sense(Sense::click()));
        if (mark.clicked() || lab.clicked()) && !selected {
            *current = value.to_string();
            changed = true;
        }
    });
    changed
}

/// A small spinner. Requests another frame so it keeps turning.
pub fn spinner(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let c = rect.center();
    let r = 6.0;
    let stroke = Stroke::new(1.6, color);
    let a0 = t * 6.0;
    let mut prev = c + egui::vec2(a0.cos(), a0.sin()) * r;
    for i in 1..=10 {
        let a = a0 + i as f32 * 0.22;
        let next = c + egui::vec2(a.cos(), a.sin()) * r;
        ui.painter().line_segment([prev, next], stroke);
        prev = next;
    }
    ui.ctx().request_repaint();
}

pub fn muted(ui: &egui::Ui, text: &str) -> RichText {
    RichText::new(text).color(theme::paint(ui).muted).size(12.5)
}

pub fn status_dot(ui: &mut egui::Ui, color: Color32, label: &str) {
    theme::pill(ui, color, label);
}
