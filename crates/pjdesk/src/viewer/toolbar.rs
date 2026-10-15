//! A small control bar drawn over the top of the viewer, like RustDesk's.
//!
//! It auto-hides and appears when the pointer is near the top of the window.
//! Text is rasterized with `ab_glyph` straight into the softbuffer, so the
//! viewer stays a lightweight winit + softbuffer window.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, Glyph, ScaleFont};

/// Actions a toolbar button can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Fullscreen,
    ActualSize,
    Refresh,
    CtrlAltDel,
    ToggleBlockInput,
    Disconnect,
}

struct Button {
    label: &'static str,
    action: Action,
    x: f32,
    w: f32,
}

const BAR_H: f32 = 34.0;
const PAD: f32 = 12.0;
const FONT_PX: f32 = 15.0;

fn font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("valid bundled font"))
}

/// Measures a label at the toolbar font size.
fn text_width(text: &str) -> f32 {
    let f = font().as_scaled(FONT_PX);
    let mut w = 0.0;
    let mut prev: Option<char> = None;
    for c in text.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern(f.glyph_id(p), id);
        }
        w += f.h_advance(id);
        prev = Some(c);
    }
    w
}

pub struct Toolbar {
    buttons: Vec<Button>,
    width: f32,
    /// Shown now (pointer near the top, or pinned open briefly after a move).
    visible: bool,
    hovered: Option<usize>,
    /// True while input forwarding is suppressed ("view only" from this side).
    pub block_input: bool,
}

impl Default for Toolbar {
    fn default() -> Self {
        let labels = [
            ("Full screen", Action::Fullscreen),
            ("Actual size", Action::ActualSize),
            ("Refresh", Action::Refresh),
            ("Ctrl+Alt+Del", Action::CtrlAltDel),
            ("Block input", Action::ToggleBlockInput),
            ("Disconnect", Action::Disconnect),
        ];
        let mut buttons = Vec::new();
        let mut x = PAD;
        for (label, action) in labels {
            let w = text_width(label) + PAD * 2.0;
            buttons.push(Button { label, action, x, w });
            x += w;
        }
        Toolbar { buttons, width: x + PAD, visible: false, hovered: None, block_input: false }
    }
}

impl Toolbar {
    /// Left edge of the bar, centered in a window `win_w` pixels wide.
    fn bar_x(&self, win_w: usize) -> f32 {
        ((win_w as f32 - self.width) / 2.0).max(0.0)
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Updates hover/visibility for a pointer at `(px, py)` in window pixels.
    /// Returns true if the pointer is over the bar (so the caller shouldn't
    /// forward that movement to the host).
    pub fn on_pointer(&mut self, px: f64, py: f64, win_w: usize) -> bool {
        // Reveal when near the top; hide once the pointer drops below the bar.
        if py <= BAR_H as f64 + 4.0 {
            self.visible = true;
        } else if py > BAR_H as f64 + 48.0 {
            self.visible = false;
        }
        self.hovered = None;
        if !self.visible {
            return false;
        }
        let bx = self.bar_x(win_w) as f64;
        if py <= BAR_H as f64 && px >= bx && px <= bx + self.width as f64 {
            let local = px - bx;
            self.hovered = self.buttons.iter().position(|b| local >= b.x as f64 && local < (b.x + b.w) as f64);
            return true;
        }
        false
    }

    /// Returns the action if `(px, py)` falls on a button (for a click).
    pub fn hit(&self, px: f64, py: f64, win_w: usize) -> Option<Action> {
        if !self.visible || py > BAR_H as f64 {
            return None;
        }
        let bx = self.bar_x(win_w) as f64;
        let local = px - bx;
        self.buttons.iter().find(|b| local >= b.x as f64 && local < (b.x + b.w) as f64).map(|b| b.action)
    }

    /// Draws the bar into the ARGB `buf` (if visible).
    pub fn draw(&self, buf: &mut [u32], win_w: usize, win_h: usize) {
        if !self.visible || win_h < BAR_H as usize {
            return;
        }
        let bx = self.bar_x(win_w);
        // Bar background.
        fill_rect(buf, win_w, win_h, bx, 0.0, self.width, BAR_H, 0xE6_1b_20_28);
        line(buf, win_w, win_h, bx, BAR_H, self.width, 0xFF_2a_30_3b);

        for (i, b) in self.buttons.iter().enumerate() {
            let on = matches!(b.action, Action::ToggleBlockInput) && self.block_input;
            if self.hovered == Some(i) {
                fill_rect(buf, win_w, win_h, bx + b.x, 3.0, b.w, BAR_H - 6.0, 0xFF_30_38_46);
            } else if on {
                fill_rect(buf, win_w, win_h, bx + b.x, 3.0, b.w, BAR_H - 6.0, 0xFF_3a_2a_2a);
            }
            let color = if on { 0xFF_f2_b0_3a } else { 0xFF_e8_ec_f2 };
            let tw = text_width(b.label);
            draw_text(buf, win_w, win_h, bx + b.x + (b.w - tw) / 2.0, 22.0, b.label, color);
        }
    }
}

/// Alpha-blends a solid rectangle (ARGB, alpha in the high byte) into `buf`.
fn fill_rect(buf: &mut [u32], w: usize, h: usize, x: f32, y: f32, rw: f32, rh: f32, argb: u32) {
    let (x0, y0) = (x.max(0.0) as usize, y.max(0.0) as usize);
    let x1 = ((x + rw) as usize).min(w);
    let y1 = ((y + rh) as usize).min(h);
    let a = ((argb >> 24) & 0xff) as f32 / 255.0;
    for yy in y0..y1 {
        for xx in x0..x1 {
            blend(&mut buf[yy * w + xx], argb, a);
        }
    }
}

fn line(buf: &mut [u32], w: usize, h: usize, x: f32, y: f32, len: f32, argb: u32) {
    let yy = y as usize;
    if yy >= h {
        return;
    }
    let x0 = x.max(0.0) as usize;
    let x1 = ((x + len) as usize).min(w);
    for xx in x0..x1 {
        buf[yy * w + xx] = argb & 0x00_ff_ff_ff | (argb & 0xff_00_00_00);
    }
}

/// Rasterizes `text` with its baseline at `y`, left edge at `x`.
fn draw_text(buf: &mut [u32], w: usize, h: usize, x: f32, y: f32, text: &str, argb: u32) {
    let scaled = font().as_scaled(FONT_PX);
    let mut caret = x;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            caret += scaled.kern(p, id);
        }
        let glyph: Glyph = id.with_scale_and_position(FONT_PX, ab_glyph::point(caret, y));
        if let Some(outline) = scaled.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    blend(&mut buf[py as usize * w + px as usize], argb, cov);
                }
            });
        }
        caret += scaled.h_advance(id);
        prev = Some(id);
    }
}

/// Blends `argb` over `dst` with coverage/alpha `a` (0..1).
fn blend(dst: &mut u32, argb: u32, a: f32) {
    let a = a.clamp(0.0, 1.0);
    let (sr, sg, sb) = ((argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff);
    let (dr, dg, db) = ((*dst >> 16) & 0xff, (*dst >> 8) & 0xff, *dst & 0xff);
    let mix = |s: u32, d: u32| ((s as f32 * a + d as f32 * (1.0 - a)) as u32) & 0xff;
    *dst = 0xff00_0000 | (mix(sr, dr) << 16) | (mix(sg, dg) << 8) | mix(sb, db);
}
