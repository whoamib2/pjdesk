//! Icons drawn with the egui painter.
//!
//! The default egui font has no emoji and no icon glyphs, so a button labeled
//! "⧉" or "👁" paints as an empty box. These shapes do not depend on a font.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Copy,
    Eye,
    EyeOff,
    Refresh,
    Gear,
    User,
    Trash,
    Lock,
    Monitor,
    Star,
    Close,
    Check,
    Alert,
    /// Four panes of a window. Not a colored logo.
    OsWindows,
    /// A laptop. Not an apple.
    OsMac,
    /// A terminal window. Not a penguin.
    OsLinux,
    /// A phone. Not a robot.
    OsAndroid,
    /// A monitor with a question mark.
    OsUnknown,
}

pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height());
    let stroke = Stroke::new((s * 0.09).clamp(1.2, 1.8), color);
    match icon {
        Icon::Copy => {
            let box_size = Vec2::splat(s * 0.52);
            let back = Rect::from_center_size(c + Vec2::new(-s * 0.08, s * 0.08), box_size);
            let front = Rect::from_center_size(c + Vec2::new(s * 0.08, -s * 0.08), box_size);
            painter.rect_stroke(back, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.rect_filled(front, CornerRadius::same(2), Color32::TRANSPARENT);
            painter.rect_stroke(front, CornerRadius::same(2), stroke, StrokeKind::Inside);
        }
        Icon::Eye => eye(painter, c, s, color, stroke, false),
        Icon::EyeOff => eye(painter, c, s, color, stroke, true),
        Icon::Refresh => {
            let r = s * 0.32;
            arc(painter, c, r, 0.6, 5.4, stroke);
            let tip = c + Vec2::new(r * 0.6_f32.cos(), r * 0.6_f32.sin());
            painter.arrow(tip, Vec2::new(s * 0.16, -s * 0.02), stroke);
        }
        Icon::Gear => {
            let r = s * 0.18;
            painter.circle_stroke(c, r, stroke);
            painter.circle_filled(c, r * 0.38, color);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let dir = Vec2::new(a.cos(), a.sin());
                painter.line_segment([c + dir * (r * 1.15), c + dir * (r * 2.05)], stroke);
            }
        }
        Icon::User => {
            painter.circle_stroke(c + Vec2::new(0.0, -s * 0.16), s * 0.16, stroke);
            arc(painter, c + Vec2::new(0.0, s * 0.18), s * 0.28, std::f32::consts::PI * 1.15, std::f32::consts::PI * 1.85, stroke);
        }
        Icon::Trash => {
            let body = Rect::from_center_size(c + Vec2::new(0.0, s * 0.06), Vec2::new(s * 0.52, s * 0.48));
            painter.rect_stroke(body, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment(
                [Pos2::new(body.left() - s * 0.06, body.top()), Pos2::new(body.right() + s * 0.06, body.top())],
                stroke,
            );
            painter.line_segment(
                [c + Vec2::new(-s * 0.12, -s * 0.34), c + Vec2::new(s * 0.12, -s * 0.34)],
                stroke,
            );
            painter.line_segment([c + Vec2::new(0.0, -s * 0.18), c + Vec2::new(0.0, s * 0.22)], stroke);
        }
        Icon::Lock => {
            let body = Rect::from_center_size(c + Vec2::new(0.0, s * 0.12), Vec2::new(s * 0.56, s * 0.42));
            painter.rect_stroke(body, CornerRadius::same(2), stroke, StrokeKind::Inside);
            arc(painter, c + Vec2::new(0.0, -s * 0.02), s * 0.18, std::f32::consts::PI, std::f32::consts::TAU, stroke);
            painter.circle_filled(c + Vec2::new(0.0, s * 0.12), s * 0.05, color);
        }
        Icon::Monitor => {
            let screen = Rect::from_center_size(c + Vec2::new(0.0, -s * 0.06), Vec2::new(s * 0.72, s * 0.48));
            painter.rect_stroke(screen, CornerRadius::same(2), stroke, StrokeKind::Inside);
            painter.line_segment([c + Vec2::new(0.0, s * 0.18), c + Vec2::new(0.0, s * 0.32)], stroke);
            painter.line_segment([c + Vec2::new(-s * 0.18, s * 0.32), c + Vec2::new(s * 0.18, s * 0.32)], stroke);
        }
        Icon::Star => {
            painter.add(egui::Shape::convex_polygon(star_points(c, s * 0.40, s * 0.17), color, Stroke::NONE));
        }
        Icon::Close => {
            let d = s * 0.26;
            painter.line_segment([c - Vec2::splat(d), c + Vec2::splat(d)], stroke);
            painter.line_segment([c + Vec2::new(-d, d), c + Vec2::new(d, -d)], stroke);
        }
        Icon::Check => {
            let a = c + Vec2::new(-s * 0.28, 0.0);
            let b = c + Vec2::new(-s * 0.06, s * 0.22);
            let d = c + Vec2::new(s * 0.32, -s * 0.24);
            painter.line_segment([a, b], stroke);
            painter.line_segment([b, d], stroke);
        }
        Icon::Alert => {
            let top = c + Vec2::new(0.0, -s * 0.34);
            let left = c + Vec2::new(-s * 0.34, s * 0.28);
            let right = c + Vec2::new(s * 0.34, s * 0.28);
            painter.add(egui::Shape::convex_polygon(vec![top, right, left], Color32::TRANSPARENT, stroke));
            painter.line_segment([c + Vec2::new(0.0, -s * 0.08), c + Vec2::new(0.0, s * 0.08)], stroke);
            painter.circle_filled(c + Vec2::new(0.0, s * 0.18), s * 0.045, color);
        }
        Icon::OsWindows => four_panes(painter, c, s, color),
        Icon::OsMac => laptop(painter, c, s, stroke, color),
        Icon::OsLinux => terminal(painter, c, s, stroke, color),
        Icon::OsAndroid => phone(painter, c, s, stroke, color),
        Icon::OsUnknown => unknown_monitor(painter, c, s, stroke, color),
    }
}

fn four_panes(painter: &egui::Painter, c: Pos2, s: f32, color: Color32) {
    let gap = (s * 0.08).max(1.5);
    let pane = ((s * 0.82) - gap) / 2.0;
    let origin = c - Vec2::splat(pane + gap / 2.0);
    for row in 0..2 {
        for col in 0..2 {
            let r = Rect::from_min_size(
                origin + Vec2::new(col as f32 * (pane + gap), row as f32 * (pane + gap)),
                Vec2::splat(pane),
            );
            painter.rect_filled(r, CornerRadius::same(2), color);
        }
    }
}

fn laptop(painter: &egui::Painter, c: Pos2, s: f32, stroke: Stroke, color: Color32) {
    let screen = Rect::from_center_size(c + Vec2::new(0.0, -s * 0.08), Vec2::new(s * 0.62, s * 0.42));
    painter.rect_stroke(screen, CornerRadius::same(2), stroke, StrokeKind::Inside);
    painter.rect_stroke(screen.shrink(s * 0.1), CornerRadius::same(1), Stroke::new(stroke.width * 0.6, color), StrokeKind::Inside);
    painter.line_segment(
        [c + Vec2::new(-s * 0.42, s * 0.24), c + Vec2::new(s * 0.42, s * 0.24)],
        Stroke::new(stroke.width * 1.4, color),
    );
    painter.line_segment(
        [c + Vec2::new(-s * 0.16, s * 0.24), c + Vec2::new(-s * 0.28, s * 0.34)],
        stroke,
    );
    painter.line_segment(
        [c + Vec2::new(s * 0.16, s * 0.24), c + Vec2::new(s * 0.28, s * 0.34)],
        stroke,
    );
}

fn terminal(painter: &egui::Painter, c: Pos2, s: f32, stroke: Stroke, color: Color32) {
    let frame = Rect::from_center_size(c, Vec2::new(s * 0.78, s * 0.62));
    painter.rect_stroke(frame, CornerRadius::same(3), stroke, StrokeKind::Inside);
    let left = Pos2::new(frame.left() + s * 0.14, c.y + s * 0.02);
    painter.line_segment([left + Vec2::new(0.0, -s * 0.12), left + Vec2::new(s * 0.12, 0.0)], stroke);
    painter.line_segment([left + Vec2::new(s * 0.12, 0.0), left + Vec2::new(0.0, s * 0.12)], stroke);
    painter.line_segment(
        [left + Vec2::new(s * 0.2, s * 0.12), left + Vec2::new(s * 0.42, s * 0.12)],
        Stroke::new(stroke.width, color),
    );
}

fn phone(painter: &egui::Painter, c: Pos2, s: f32, stroke: Stroke, color: Color32) {
    let body = Rect::from_center_size(c, Vec2::new(s * 0.46, s * 0.78));
    painter.rect_stroke(body, CornerRadius::same((s * 0.1) as u8), stroke, StrokeKind::Inside);
    painter.circle_stroke(c + Vec2::new(0.0, s * 0.24), s * 0.06, stroke);
    painter.line_segment(
        [Pos2::new(body.center().x - s * 0.08, body.top() + s * 0.08), Pos2::new(body.center().x + s * 0.08, body.top() + s * 0.08)],
        stroke,
    );
    let _ = color;
}

fn unknown_monitor(painter: &egui::Painter, c: Pos2, s: f32, stroke: Stroke, color: Color32) {
    let screen = Rect::from_center_size(c + Vec2::new(0.0, -s * 0.06), Vec2::new(s * 0.74, s * 0.5));
    painter.rect_stroke(screen, CornerRadius::same(2), stroke, StrokeKind::Inside);
    let q = screen.center();
    arc(painter, q + Vec2::new(0.0, -s * 0.04), s * 0.1, 0.4, 3.4, stroke);
    painter.line_segment([q + Vec2::new(s * 0.02, s * 0.02), q + Vec2::new(0.0, s * 0.08)], stroke);
    painter.circle_filled(q + Vec2::new(0.0, s * 0.16), s * 0.035, color);
    painter.line_segment([c + Vec2::new(0.0, s * 0.2), c + Vec2::new(0.0, s * 0.32)], stroke);
    painter.line_segment([c + Vec2::new(-s * 0.16, s * 0.32), c + Vec2::new(s * 0.16, s * 0.32)], stroke);
}

fn eye(painter: &egui::Painter, c: Pos2, s: f32, color: Color32, stroke: Stroke, slashed: bool) {
    painter.circle_stroke(c, s * 0.20, stroke);
    painter.circle_filled(c, s * 0.07, color);
    painter.line_segment([c + Vec2::new(-s * 0.36, s * 0.02), c + Vec2::new(-s * 0.20, -s * 0.08)], stroke);
    painter.line_segment([c + Vec2::new(s * 0.36, s * 0.02), c + Vec2::new(s * 0.20, -s * 0.08)], stroke);
    if slashed {
        painter.line_segment([c + Vec2::new(-s * 0.32, s * 0.28), c + Vec2::new(s * 0.32, -s * 0.28)], stroke);
    }
}

fn arc(painter: &egui::Painter, c: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let steps = 14;
    let mut prev = c + Vec2::new(a0.cos(), a0.sin()) * r;
    for i in 1..=steps {
        let t = a0 + (a1 - a0) * (i as f32 / steps as f32);
        let next = c + Vec2::new(t.cos(), t.sin()) * r;
        painter.line_segment([prev, next], stroke);
        prev = next;
    }
}

fn star_points(c: Pos2, outer: f32, inner: f32) -> Vec<Pos2> {
    let mut pts = Vec::with_capacity(10);
    for i in 0..10 {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        let r = if i % 2 == 0 { outer } else { inner };
        pts.push(c + Vec2::new(a.cos(), a.sin()) * r);
    }
    pts
}
