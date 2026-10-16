//! Small shared UI helpers.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32};

use super::theme;

/// A brief message that fades after a few seconds (copied, opened, signed in…).
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
        egui::Area::new(egui::Id::new("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(theme::ELEV)
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .stroke(egui::Stroke::new(1.0, theme::LINE))
                    .show(ui, |ui| {
                        ui.colored_label(Color32::WHITE, &self.text);
                    });
            });
    }
}

/// Copies text to the clipboard, returning whether it worked.
pub fn copy_to_clipboard(text: &str) -> bool {
    match arboard::Clipboard::new() {
        Ok(mut cb) => cb.set_text(text.to_string()).is_ok(),
        Err(_) => false,
    }
}

/// A status indicator rendered as a tinted pill.
pub fn status_dot(ui: &mut egui::Ui, color: Color32, label: &str) {
    theme::pill(ui, color, label);
}

/// A section heading in muted small-caps styling.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::MUTED).size(11.5).strong());
    ui.add_space(4.0);
}

/// A small caption label placed above an input field.
pub fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::MUTED).size(12.0));
    ui.add_space(5.0);
}

/// A padded, full-width single-line text field with consistent styling.
/// Returns the response so callers can detect Enter / focus loss.
pub fn text_field(
    ui: &mut egui::Ui,
    value: &mut String,
    hint: &str,
    password: bool,
    monospace: bool,
) -> egui::Response {
    let font = if monospace { egui::TextStyle::Monospace } else { egui::TextStyle::Body };
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .password(password)
            .font(font)
            .margin(egui::Margin::symmetric(12, 10))
            .desired_width(f32::INFINITY),
    )
}

/// A full-width primary (accent) button of a comfortable height.
pub fn primary_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add_sized(
        [ui.available_width(), 40.0],
        egui::Button::new(egui::RichText::new(label).strong().size(15.0).color(Color32::WHITE)).fill(theme::ACCENT),
    )
    .clicked()
}
