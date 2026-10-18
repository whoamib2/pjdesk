//! Buttons, checkboxes, and password toggles for the server window.

#![cfg_attr(not(windows), allow(dead_code))]

use eframe::egui::{self, Color32, CornerRadius, Label, RichText, Sense, Stroke, StrokeKind};

use crate::ui_icons::{self, Icon};
use crate::ui_theme;

pub fn checkbox(ui: &mut egui::Ui, on: &mut bool, label: &str) -> bool {
    let enabled = ui.is_enabled();
    let mut changed = false;
    let p = ui_theme::paint(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, box_resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::click());
        let fill = if *on {
            p.accent
        } else if box_resp.hovered() && enabled {
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
            ui_icons::paint(ui.painter(), rect.shrink(3.0), Icon::Check, Color32::WHITE);
        }
        let lab = ui.add(Label::new(label).sense(Sense::click()));
        if enabled && (box_resp.clicked() || lab.clicked()) {
            *on = !*on;
            changed = true;
        }
    });
    changed
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, tip: &str) -> bool {
    let p = ui_theme::paint(ui);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), Sense::click());
    if resp.hovered() {
        let bg = if resp.is_pointer_button_down_on() { p.accent.linear_multiply(0.35) } else { p.elev };
        ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
    }
    let color = if resp.hovered() { p.text } else { p.muted };
    ui_icons::paint(ui.painter(), rect.shrink(7.0), icon, color);
    resp.on_hover_text(tip).clicked()
}

pub fn accent_button(ui: &mut egui::Ui, label: &str) -> bool {
    let p = ui_theme::paint(ui);
    ui.add_sized(
        [260.0, 36.0],
        egui::Button::new(RichText::new(label).strong().color(p.on_accent)).fill(p.accent),
    )
    .clicked()
}

pub fn reveal(ui: &mut egui::Ui, shown: &mut bool) {
    let label = if *shown { "Hide" } else { "Show" };
    if ui.add_sized([64.0, 36.0], egui::Button::new(label)).on_hover_text(label).clicked() {
        *shown = !*shown;
    }
}

pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
}

pub fn spinner(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let c = rect.center();
    let stroke = Stroke::new(1.6, color);
    let a0 = t * 6.0;
    let mut prev = c + egui::vec2(a0.cos(), a0.sin()) * 6.0;
    for i in 1..=10 {
        let a = a0 + i as f32 * 0.22;
        let next = c + egui::vec2(a.cos(), a.sin()) * 6.0;
        ui.painter().line_segment([prev, next], stroke);
        prev = next;
    }
    ui.ctx().request_repaint();
}
