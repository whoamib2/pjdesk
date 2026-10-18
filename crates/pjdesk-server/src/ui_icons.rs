//! Icons drawn with the egui painter, same approach as the client window.
//! A character such as "●" or "⧉" is not in the default font and paints as an empty box.

#![cfg_attr(not(windows), allow(dead_code))]

use eframe::egui::{self, Color32, CornerRadius, Rect, Stroke, StrokeKind, Vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Copy,
    Check,
    Alert,
    Close,
}

pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let stroke = Stroke::new((s * 0.1).clamp(1.2, 1.8), color);
    match icon {
        Icon::Copy => {
            let box_size = Vec2::splat(s * 0.52);
            let back = Rect::from_center_size(c + Vec2::new(-s * 0.08, s * 0.08), box_size);
            let front = Rect::from_center_size(c + Vec2::new(s * 0.08, -s * 0.08), box_size);
            painter.rect_stroke(back, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.rect_filled(front, CornerRadius::same(2), Color32::TRANSPARENT);
            painter.rect_stroke(front, CornerRadius::same(2), stroke, StrokeKind::Inside);
        }
        Icon::Check => {
            painter.line_segment([c + Vec2::new(-s * 0.28, 0.0), c + Vec2::new(-s * 0.06, s * 0.24)], stroke);
            painter.line_segment([c + Vec2::new(-s * 0.06, s * 0.24), c + Vec2::new(s * 0.32, -s * 0.22)], stroke);
        }
        Icon::Alert => {
            let top = c + Vec2::new(0.0, -s * 0.34);
            let left = c + Vec2::new(-s * 0.34, s * 0.28);
            let right = c + Vec2::new(s * 0.34, s * 0.28);
            painter.add(egui::Shape::convex_polygon(vec![top, right, left], Color32::TRANSPARENT, stroke));
            painter.line_segment([c + Vec2::new(0.0, -s * 0.08), c + Vec2::new(0.0, s * 0.08)], stroke);
            painter.circle_filled(c + Vec2::new(0.0, s * 0.18), s * 0.045, color);
        }
        Icon::Close => {
            painter.line_segment([c + Vec2::new(-s * 0.24, -s * 0.24), c + Vec2::new(s * 0.24, s * 0.24)], stroke);
            painter.line_segment([c + Vec2::new(s * 0.24, -s * 0.24), c + Vec2::new(-s * 0.24, s * 0.24)], stroke);
        }
    }
}
