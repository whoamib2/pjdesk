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
                    .fill(Color32::from_rgb(0x26, 0x2c, 0x37))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(14, 9))
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

/// A colored status dot followed by a label.
pub fn status_dot(ui: &mut egui::Ui, color: Color32, label: &str) {
    ui.horizontal(|ui| {
        ui.colored_label(color, "●");
        ui.label(label);
    });
}

/// A section heading in muted small caps-ish styling.
pub fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::MUTED).size(12.0).strong());
    ui.add_space(2.0);
}
