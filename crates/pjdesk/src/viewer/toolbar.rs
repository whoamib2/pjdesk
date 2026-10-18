//! Floating session toolbar drawn over the viewer.
//!
//! It sits in a rounded bar near the top, collapses to a small pill, and shows
//! a tooltip for the button under the pointer. Text is rasterized with
//! `ab_glyph` into the softbuffer, and the icons are filled shapes so they do
//! not depend on a font having a particular glyph.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, Glyph, ScaleFont};

/// Actions a toolbar button or menu item can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ZoomFit,
    ZoomTo(u16),
    ZoomIn,
    ZoomOut,
    Keys(Combo),
    Fullscreen,
    Refresh,
    ToggleBlockHost,
    TogglePrivacy,
    ToggleAudio,
    ToggleRecord,
    SendClipboard,
    Restart,
    Quality(&'static str),
    /// Open the side panel on this tab: 0 files, 1 chat, 2 more.
    OpenPanel(u8),
    Disconnect,
}

/// Key combinations the "Send keys" menu can send to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combo {
    CtrlAltDel,
    Win,
    AltTab,
    CtrlEsc,
    WinD,
    WinE,
    WinL,
    TaskMgr,
    AltF4,
    PrintScreen,
}

const TOP: f32 = 10.0;
const BAR_H: f32 = 40.0;
const ITEM_H: f32 = 30.0;
const PAD: f32 = 12.0;
const FONT_PX: f32 = 14.0;
const RADIUS: f32 = 12.0;

#[derive(Clone, Copy)]
enum Mark {
    View,
    Keys,
    Session,
    Files,
    Chat,
    End,
    Collapse,
}

enum Kind {
    Act(Action),
    Menu(usize),
    Collapse,
}

struct Button {
    label: &'static str,
    tip: &'static str,
    kind: Kind,
    mark: Mark,
    x: f32,
    w: f32,
}

struct Menu {
    items: Vec<(String, Action)>,
    width: f32,
}

fn font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("valid bundled font"))
}

pub(crate) fn text_width(text: &str) -> f32 {
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

const ZOOM_ITEMS: &[(&str, Action)] = &[
    ("Fit to window", Action::ZoomFit),
    ("100%  (actual size)", Action::ZoomTo(100)),
    ("125%", Action::ZoomTo(125)),
    ("150%", Action::ZoomTo(150)),
    ("200%", Action::ZoomTo(200)),
    ("300%", Action::ZoomTo(300)),
    ("400%", Action::ZoomTo(400)),
    ("75%", Action::ZoomTo(75)),
    ("50%", Action::ZoomTo(50)),
];

const CONTROL_ITEMS: &[(&str, Action)] = &[
    ("Full screen", Action::Fullscreen),
    ("Refresh picture", Action::Refresh),
    ("Block remote input", Action::ToggleBlockHost),
    ("Privacy screen", Action::TogglePrivacy),
    ("Audio", Action::ToggleAudio),
    ("Send clipboard", Action::SendClipboard),
    ("Record", Action::ToggleRecord),
    ("Restart computer", Action::Restart),
    ("Quality: smooth", Action::Quality("smooth")),
    ("Quality: balanced", Action::Quality("balanced")),
    ("Quality: sharp", Action::Quality("sharp")),
    ("Quality: high", Action::Quality("high")),
    ("Port forward, monitors…", Action::OpenPanel(2)),
];

const KEY_ITEMS: &[(&str, Action)] = &[
    ("Ctrl + Alt + Del", Action::Keys(Combo::CtrlAltDel)),
    ("Windows key", Action::Keys(Combo::Win)),
    ("Alt + Tab", Action::Keys(Combo::AltTab)),
    ("Start menu  (Ctrl+Esc)", Action::Keys(Combo::CtrlEsc)),
    ("Show desktop  (Win+D)", Action::Keys(Combo::WinD)),
    ("File Explorer  (Win+E)", Action::Keys(Combo::WinE)),
    ("Lock screen  (Win+L)", Action::Keys(Combo::WinL)),
    ("Task Manager", Action::Keys(Combo::TaskMgr)),
    ("Close window  (Alt+F4)", Action::Keys(Combo::AltF4)),
    ("Print Screen", Action::Keys(Combo::PrintScreen)),
];

pub struct Toolbar {
    buttons: Vec<Button>,
    menus: Vec<Menu>,
    width: f32,
    collapsed_w: f32,
    /// Expanded bar is on screen (pointer near the top, or a menu is open).
    visible: bool,
    /// User hid the bar. A pill stays so it can be opened again.
    collapsed: bool,
    hovered_button: Option<usize>,
    open: Option<usize>,
    hovered_item: Option<usize>,
    /// True while input forwarding is suppressed ("view only" from this side).
    pub block_input: bool,
}

impl Default for Toolbar {
    fn default() -> Self {
        let menus = vec![owned(ZOOM_ITEMS), owned(KEY_ITEMS), owned(CONTROL_ITEMS)];
        let spec: [(&'static str, &'static str, Kind, Mark); 7] = [
            ("View", "Zoom", Kind::Menu(0), Mark::View),
            ("Keys", "Send special keys", Kind::Menu(1), Mark::Keys),
            ("Session", "Audio, privacy, record, restart", Kind::Menu(2), Mark::Session),
            ("Files", "Send and receive files", Kind::Act(Action::OpenPanel(0)), Mark::Files),
            ("Chat", "Messages", Kind::Act(Action::OpenPanel(1)), Mark::Chat),
            ("End", "Disconnect", Kind::Act(Action::Disconnect), Mark::End),
            ("Hide", "Hide the toolbar", Kind::Collapse, Mark::Collapse),
        ];
        let mut buttons = Vec::new();
        let mut x = PAD;
        for (label, tip, kind, mark) in spec {
            let w = 16.0 + 6.0 + text_width(label) + PAD * 2.0;
            buttons.push(Button { label, tip, kind, mark, x, w });
            x += w;
        }
        let collapsed_w = text_width("PJDesk") + PAD * 2.0 + 8.0;
        Toolbar {
            buttons,
            menus,
            width: x + PAD,
            collapsed_w,
            visible: true,
            collapsed: false,
            hovered_button: None,
            open: None,
            hovered_item: None,
            block_input: false,
        }
    }
}

fn owned(items: &[(&str, Action)]) -> Menu {
    let items: Vec<(String, Action)> = items.iter().map(|(l, a)| ((*l).to_string(), *a)).collect();
    let width = items.iter().map(|(l, _)| text_width(l)).fold(160.0_f32, f32::max) + PAD * 3.0;
    Menu { items, width }
}

impl Toolbar {
    fn bar_rect(&self, win_w: usize) -> (f32, f32, f32, f32) {
        let w = if self.collapsed { self.collapsed_w } else { self.width };
        let x = ((win_w as f32 - w) / 2.0).max(8.0);
        (x, TOP, w, BAR_H)
    }

    pub fn is_visible(&self) -> bool {
        self.visible || self.collapsed
    }

    fn menu_index(&self, button: usize) -> Option<usize> {
        match self.buttons.get(button).map(|b| &b.kind) {
            Some(Kind::Menu(m)) => Some(*m),
            _ => None,
        }
    }

    fn open_menu_rect(&self, win_w: usize) -> Option<(f32, f32, f32, f32)> {
        if self.collapsed {
            return None;
        }
        let bi = self.open?;
        let m = self.menu_index(bi)?;
        let menu = &self.menus[m];
        let (bx, by, _, bh) = self.bar_rect(win_w);
        let left = bx + self.buttons[bi].x;
        let height = menu.items.len() as f32 * ITEM_H + 8.0;
        Some((left, by + bh + 4.0, menu.width, height))
    }

    fn tooltip_rect(&self, win_w: usize) -> Option<(f32, f32, f32, f32)> {
        if self.collapsed || !self.visible {
            return None;
        }
        let i = self.hovered_button?;
        let tip = self.buttons.get(i)?.tip;
        let (bx, by, _, bh) = self.bar_rect(win_w);
        let tw = text_width(tip) + 16.0;
        let x = bx + self.buttons[i].x;
        Some((x, by + bh + 6.0, tw, 22.0))
    }

    fn hit_button(&self, px: f64, py: f64, win_w: usize) -> Option<usize> {
        let (bx, by, bw, bh) = self.bar_rect(win_w);
        if px < bx as f64 || px > (bx + bw) as f64 || py < by as f64 || py > (by + bh) as f64 {
            return None;
        }
        if self.collapsed {
            return Some(0);
        }
        let local = px - bx as f64;
        self.buttons.iter().position(|b| local >= b.x as f64 && local < (b.x + b.w) as f64)
    }

    pub fn covers(&self, px: f64, py: f64, win_w: usize) -> bool {
        if self.collapsed {
            return self.hit_button(px, py, win_w).is_some();
        }
        if !self.visible {
            return false;
        }
        if self.hit_button(px, py, win_w).is_some() {
            return true;
        }
        if let Some((mx, my, mw, mh)) = self.open_menu_rect(win_w) {
            if px >= mx as f64 && px <= (mx + mw) as f64 && py >= my as f64 && py <= (my + mh) as f64 {
                return true;
            }
        }
        if let Some((tx, ty, tw, th)) = self.tooltip_rect(win_w) {
            if px >= tx as f64 && px <= (tx + tw) as f64 && py >= ty as f64 && py <= (ty + th) as f64 {
                return true;
            }
        }
        false
    }

    pub fn on_pointer(&mut self, px: f64, py: f64, win_w: usize) -> bool {
        if self.collapsed {
            self.hovered_button = self.hit_button(px, py, win_w);
            return self.hovered_button.is_some();
        }
        let menu_bottom = self.open_menu_rect(win_w).map(|(_, my, _, mh)| my + mh).unwrap_or(TOP + BAR_H);
        let reveal = TOP + BAR_H + if self.hovered_button.is_some() { 32.0 } else { 8.0 };
        if py <= reveal as f64 {
            self.visible = true;
        } else if py > menu_bottom as f64 + 16.0 {
            self.visible = false;
            self.open = None;
            self.hovered_button = None;
            self.hovered_item = None;
            return false;
        }
        self.hovered_button = None;
        self.hovered_item = None;
        if !self.visible {
            return false;
        }
        if let Some(i) = self.hit_button(px, py, win_w) {
            self.hovered_button = Some(i);
            return true;
        }
        if let Some((mx, my, mw, mh)) = self.open_menu_rect(win_w) {
            if px >= mx as f64 && px <= (mx + mw) as f64 && py >= my as f64 && py <= (my + mh) as f64 {
                let idx = ((py - my as f64 - 4.0) / ITEM_H as f64) as usize;
                self.hovered_item = Some(idx);
                return true;
            }
        }
        false
    }

    pub fn click(&mut self, px: f64, py: f64, win_w: usize) -> Option<Action> {
        if self.collapsed {
            if self.hit_button(px, py, win_w).is_some() {
                self.collapsed = false;
                self.visible = true;
            }
            return None;
        }
        if !self.visible {
            return None;
        }
        if let Some(i) = self.hit_button(px, py, win_w) {
            match self.buttons[i].kind {
                Kind::Menu(_) => {
                    self.open = if self.open == Some(i) { None } else { Some(i) };
                    return None;
                }
                Kind::Collapse => {
                    self.collapsed = true;
                    self.open = None;
                    self.visible = false;
                    return None;
                }
                Kind::Act(a) => {
                    self.open = None;
                    return Some(a);
                }
            }
        }
        if let Some((mx, my, mw, mh)) = self.open_menu_rect(win_w) {
            if px >= mx as f64 && px <= (mx + mw) as f64 && py >= my as f64 && py <= (my + mh) as f64 {
                let idx = ((py - my as f64 - 4.0) / ITEM_H as f64) as usize;
                let m = self.menu_index(self.open.unwrap()).unwrap();
                if let Some((_, action)) = self.menus[m].items.get(idx) {
                    let action = *action;
                    self.open = None;
                    return Some(action);
                }
            }
        }
        self.open = None;
        None
    }

    pub fn draw(&self, buf: &mut [u32], win_w: usize, win_h: usize) {
        if win_h < (TOP + BAR_H) as usize {
            return;
        }
        let (bx, by, bw, bh) = self.bar_rect(win_w);
        if self.collapsed {
            fill_round(buf, win_w, win_h, bx, by, bw, bh, RADIUS, 0xF2_1c_22_2c);
            let label = "PJDesk";
            let tw = text_width(label);
            draw_text(buf, win_w, win_h, bx + (bw - tw) / 2.0, by + 26.0, label, 0xFF_e8_ec_f2);
            return;
        }
        if !self.visible {
            return;
        }
        fill_round(buf, win_w, win_h, bx - 1.0, by - 1.0, bw + 2.0, bh + 2.0, RADIUS, 0xFF_2a_33_42);
        fill_round(buf, win_w, win_h, bx, by, bw, bh, RADIUS, 0xF2_14_18_22);

        for (i, b) in self.buttons.iter().enumerate() {
            let end = matches!(b.kind, Kind::Act(Action::Disconnect));
            let menu_open = self.open == Some(i);
            let session = matches!(b.mark, Mark::Session);
            if self.hovered_button == Some(i) || menu_open {
                let hover = if end { 0xFF_5a_30_34 } else { 0xFF_2c_36_46 };
                fill_round(buf, win_w, win_h, bx + b.x + 4.0, by + 6.0, b.w - 8.0, bh - 12.0, 8.0, hover);
            } else if session && self.block_input {
                fill_round(buf, win_w, win_h, bx + b.x + 4.0, by + 6.0, b.w - 8.0, bh - 12.0, 8.0, 0xFF_3a_32_22);
            }
            let color = if end {
                0xFF_f0_8a_86
            } else if (session && self.block_input) || menu_open {
                0xFF_f2_b0_3a
            } else {
                0xFF_e8_ec_f2
            };
            draw_mark(buf, win_w, win_h, bx + b.x + 10.0, by + 12.0, b.mark, color);
            draw_text(buf, win_w, win_h, bx + b.x + 28.0, by + 26.0, b.label, color);
        }

        if let Some((tx, ty, tw, th)) = self.tooltip_rect(win_w) {
            if self.open.is_none() {
                if let Some(i) = self.hovered_button {
                    fill_round(buf, win_w, win_h, tx, ty, tw, th, 6.0, 0xF2_22_27_30);
                    draw_text(buf, win_w, win_h, tx + 8.0, ty + 15.0, self.buttons[i].tip, 0xFF_e8_ec_f2);
                }
            }
        }

        if let (Some((mx, my, mw, mh)), Some(bi)) = (self.open_menu_rect(win_w), self.open) {
            let m = self.menu_index(bi).unwrap();
            fill_round(buf, win_w, win_h, mx, my, mw, mh, 10.0, 0xF7_22_27_30);
            for (idx, (label, _)) in self.menus[m].items.iter().enumerate() {
                let iy = my + 4.0 + idx as f32 * ITEM_H;
                if self.hovered_item == Some(idx) {
                    fill_round(buf, win_w, win_h, mx + 4.0, iy + 2.0, mw - 8.0, ITEM_H - 4.0, 6.0, 0xFF_32_3b_4a);
                }
                draw_text(buf, win_w, win_h, mx + PAD, iy + ITEM_H * 0.68, label, 0xFF_e8_ec_f2);
            }
        }
    }
}

fn draw_mark(buf: &mut [u32], w: usize, h: usize, x: f32, y: f32, mark: Mark, argb: u32) {
    match mark {
        Mark::View => {
            fill_rect(buf, w, h, x, y, 12.0, 9.0, argb);
            fill_rect(buf, w, h, x + 2.0, y + 2.0, 8.0, 5.0, 0xFF_14_18_22);
        }
        Mark::Keys => {
            fill_rect(buf, w, h, x, y + 2.0, 12.0, 8.0, argb);
            fill_rect(buf, w, h, x + 1.0, y + 3.0, 4.0, 2.0, 0xFF_14_18_22);
            fill_rect(buf, w, h, x + 7.0, y + 3.0, 4.0, 2.0, 0xFF_14_18_22);
        }
        Mark::Session => fill_circle(buf, w, h, x + 6.0, y + 5.0, 5.0, argb),
        Mark::Files => {
            fill_rect(buf, w, h, x + 2.0, y, 8.0, 11.0, argb);
            fill_rect(buf, w, h, x + 4.0, y + 2.0, 4.0, 7.0, 0xFF_14_18_22);
        }
        Mark::Chat => {
            fill_rect(buf, w, h, x, y + 1.0, 12.0, 8.0, argb);
            fill_rect(buf, w, h, x + 2.0, y + 8.0, 3.0, 3.0, argb);
        }
        Mark::End => fill_rect(buf, w, h, x + 2.0, y + 1.0, 8.0, 8.0, argb),
        Mark::Collapse => {
            fill_rect(buf, w, h, x, y + 5.0, 12.0, 2.0, argb);
        }
    }
}

pub(crate) fn fill_rect(buf: &mut [u32], w: usize, h: usize, x: f32, y: f32, rw: f32, rh: f32, argb: u32) {
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

fn fill_round(buf: &mut [u32], w: usize, h: usize, x: f32, y: f32, rw: f32, rh: f32, rad: f32, argb: u32) {
    let rad = rad.min(rw / 2.0).min(rh / 2.0).max(0.0);
    let (x0, y0) = (x.max(0.0) as usize, y.max(0.0) as usize);
    let x1 = ((x + rw) as usize).min(w);
    let y1 = ((y + rh) as usize).min(h);
    let a = ((argb >> 24) & 0xff) as f32 / 255.0;
    let right = x + rw;
    let bottom = y + rh;
    for yy in y0..y1 {
        for xx in x0..x1 {
            let px = xx as f32 + 0.5;
            let py = yy as f32 + 0.5;
            if inside_round(px, py, x, y, right, bottom, rad) {
                blend(&mut buf[yy * w + xx], argb, a);
            }
        }
    }
}

fn inside_round(px: f32, py: f32, left: f32, top: f32, right: f32, bottom: f32, rad: f32) -> bool {
    if px < left || px > right || py < top || py > bottom {
        return false;
    }
    let cx = if px < left + rad {
        left + rad
    } else if px > right - rad {
        right - rad
    } else {
        px
    };
    let cy = if py < top + rad {
        top + rad
    } else if py > bottom - rad {
        bottom - rad
    } else {
        py
    };
    let dx = px - cx;
    let dy = py - cy;
    dx * dx + dy * dy <= rad * rad
}

fn fill_circle(buf: &mut [u32], w: usize, h: usize, cx: f32, cy: f32, r: f32, argb: u32) {
    let r2 = r * r;
    let x0 = (cx - r).max(0.0) as usize;
    let y0 = (cy - r).max(0.0) as usize;
    let x1 = ((cx + r) as usize + 1).min(w);
    let y1 = ((cy + r) as usize + 1).min(h);
    for yy in y0..y1 {
        for xx in x0..x1 {
            let dx = xx as f32 + 0.5 - cx;
            let dy = yy as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r2 {
                blend(&mut buf[yy * w + xx], argb, 1.0);
            }
        }
    }
}

pub(crate) fn draw_text(buf: &mut [u32], w: usize, h: usize, x: f32, y: f32, text: &str, argb: u32) {
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

fn blend(dst: &mut u32, argb: u32, a: f32) {
    let a = a.clamp(0.0, 1.0);
    let (sr, sg, sb) = ((argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff);
    let (dr, dg, db) = ((*dst >> 16) & 0xff, (*dst >> 8) & 0xff, *dst & 0xff);
    let mix = |s: u32, d: u32| ((s as f32 * a + d as f32 * (1.0 - a)) as u32) & 0xff;
    *dst = 0xff00_0000 | (mix(sr, dr) << 16) | (mix(sg, dg) << 8) | mix(sb, db);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_draws_icons_and_end_disconnects() {
        let mut bar = Toolbar::default();
        let w = 900;
        let h = 200;
        let mut buf = vec![0u32; w * h];
        let (bx, by, _, _) = bar.bar_rect(w);
        bar.on_pointer((bx + 20.0) as f64, (by + 10.0) as f64, w);
        assert!(bar.is_visible());
        bar.draw(&mut buf, w, h);
        assert!(buf.iter().any(|p| *p != 0), "the floating bar should paint pixels");

        let end = bar.buttons.iter().find(|b| matches!(b.kind, Kind::Act(Action::Disconnect))).unwrap();
        let x = bx + end.x + 8.0;
        let action = bar.click(x as f64, (by + 12.0) as f64, w);
        assert_eq!(action, Some(Action::Disconnect));
    }

    #[test]
    fn toolbar_collapse_keeps_a_pill() {
        let mut bar = Toolbar::default();
        let w = 900;
        let (bx, by, bw, _) = bar.bar_rect(w);
        let hide = bar.buttons.iter().find(|b| matches!(b.kind, Kind::Collapse)).unwrap();
        let x = bx + hide.x + 8.0;
        assert_eq!(bar.click(x as f64, (by + 12.0) as f64, w), None);
        assert!(bar.collapsed);
        assert!(bar.covers((bx + bw / 2.0) as f64, (by + 8.0) as f64, w) || bar.covers(w as f64 / 2.0, (by + 8.0) as f64, w));
        let (cx, cy, _, _) = bar.bar_rect(w);
        bar.click((cx + 8.0) as f64, (cy + 8.0) as f64, w);
        assert!(!bar.collapsed);
    }
}
