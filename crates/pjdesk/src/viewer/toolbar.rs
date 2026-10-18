//! A small control bar drawn over the top of the viewer, like RustDesk's.
//!
//! It auto-hides and appears when the pointer is near the top of the window.
//! Two of its buttons ("Zoom" and "Send keys") open drop-down menus. Text is
//! rasterized with `ab_glyph` straight into the softbuffer, so the viewer stays
//! a lightweight winit + softbuffer window.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, Glyph, ScaleFont};

/// Actions a toolbar button or menu item can trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Fit the picture to the window (adaptive).
    ZoomFit,
    /// Set an explicit zoom percentage (100 = pixel-for-pixel).
    ZoomTo(u16),
    ZoomIn,
    ZoomOut,
    /// Send a key combination to the remote computer.
    Keys(Combo),
    Fullscreen,
    Refresh,
    /// Ask the host to ignore its own keyboard and mouse.
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

const BAR_H: f32 = 36.0;
const ITEM_H: f32 = 28.0;
const PAD: f32 = 12.0;
const FONT_PX: f32 = 15.0;

enum Kind {
    Act(Action),
    /// Opens the menu at this index in `menus`.
    Menu(usize),
}

struct Button {
    label: &'static str,
    kind: Kind,
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

/// Measures a label at the toolbar font size.
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
    /// Shown now (pointer near the top, or a menu is open).
    visible: bool,
    hovered_button: Option<usize>,
    /// Index (into `buttons`) of the button whose menu is open.
    open: Option<usize>,
    hovered_item: Option<usize>,
    /// True while input forwarding is suppressed ("view only" from this side).
    pub block_input: bool,
}

impl Default for Toolbar {
    fn default() -> Self {
        let menus = vec![owned(ZOOM_ITEMS), owned(KEY_ITEMS), owned(CONTROL_ITEMS)];
        let spec: [(&'static str, Kind); 6] = [
            ("View \u{25be}", Kind::Menu(0)),
            ("Keys \u{25be}", Kind::Menu(1)),
            ("Session \u{25be}", Kind::Menu(2)),
            ("Files", Kind::Act(Action::OpenPanel(0))),
            ("Chat", Kind::Act(Action::OpenPanel(1))),
            ("End", Kind::Act(Action::Disconnect)),
        ];
        let mut buttons = Vec::new();
        let mut x = PAD;
        for (label, kind) in spec {
            let w = text_width(label) + PAD * 2.0;
            buttons.push(Button { label, kind, x, w });
            x += w;
        }
        Toolbar { buttons, menus, width: x + PAD, visible: true, hovered_button: None, open: None, hovered_item: None, block_input: false }
    }
}

fn owned(items: &[(&str, Action)]) -> Menu {
    let items: Vec<(String, Action)> = items.iter().map(|(l, a)| ((*l).to_string(), *a)).collect();
    let width = items.iter().map(|(l, _)| text_width(l)).fold(0.0_f32, f32::max) + PAD * 3.0;
    Menu { items, width }
}

impl Toolbar {
    /// Left edge of the bar, centered in a window `win_w` pixels wide.
    fn bar_x(&self, win_w: usize) -> f32 {
        ((win_w as f32 - self.width) / 2.0).max(0.0)
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    fn menu_index(&self, button: usize) -> Option<usize> {
        match self.buttons[button].kind {
            Kind::Menu(m) => Some(m),
            Kind::Act(_) => None,
        }
    }

    /// (left x, top y, width, height) of the currently open menu, if any.
    fn open_menu_rect(&self, win_w: usize) -> Option<(f32, f32, f32, f32)> {
        let bi = self.open?;
        let m = self.menu_index(bi)?;
        let menu = &self.menus[m];
        let left = self.bar_x(win_w) + self.buttons[bi].x;
        let height = menu.items.len() as f32 * ITEM_H;
        Some((left, BAR_H, menu.width, height))
    }

    /// Does `(px, py)` fall on the bar or the open menu? (Then the caller must
    /// not forward that pointer event to the host.)
    pub fn covers(&self, px: f64, py: f64, win_w: usize) -> bool {
        if !self.visible {
            return false;
        }
        let bx = self.bar_x(win_w) as f64;
        if py <= BAR_H as f64 && px >= bx && px <= bx + self.width as f64 {
            return true;
        }
        if let Some((mx, my, mw, mh)) = self.open_menu_rect(win_w) {
            if px >= mx as f64 && px <= (mx + mw) as f64 && py >= my as f64 && py <= (my + mh) as f64 {
                return true;
            }
        }
        false
    }

    /// Updates hover/visibility for a pointer at `(px, py)`. Returns true if the
    /// pointer is over the bar or open menu.
    pub fn on_pointer(&mut self, px: f64, py: f64, win_w: usize) -> bool {
        let menu_bottom = self.open_menu_rect(win_w).map(|(_, my, _, mh)| my + mh).unwrap_or(BAR_H);
        if py <= BAR_H as f64 + 4.0 {
            self.visible = true;
        } else if py > menu_bottom as f64 + 24.0 {
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
        let bx = self.bar_x(win_w) as f64;
        if py <= BAR_H as f64 {
            if px >= bx && px <= bx + self.width as f64 {
                let local = px - bx;
                self.hovered_button = self.buttons.iter().position(|b| local >= b.x as f64 && local < (b.x + b.w) as f64);
                return true;
            }
            return false;
        }
        if let Some((mx, my, mw, mh)) = self.open_menu_rect(win_w) {
            if px >= mx as f64 && px <= (mx + mw) as f64 && py >= my as f64 && py <= (my + mh) as f64 {
                self.hovered_item = Some(((py - my as f64) / ITEM_H as f64) as usize);
                return true;
            }
        }
        false
    }

    /// Handles a left-button press at `(px, py)`. Opens/closes a menu (returns
    /// `None`) or yields the action to perform.
    pub fn click(&mut self, px: f64, py: f64, win_w: usize) -> Option<Action> {
        if !self.visible {
            return None;
        }
        let bx = self.bar_x(win_w) as f64;
        if py <= BAR_H as f64 {
            if px >= bx && px <= bx + self.width as f64 {
                let local = px - bx;
                if let Some(i) = self.buttons.iter().position(|b| local >= b.x as f64 && local < (b.x + b.w) as f64) {
                    match self.buttons[i].kind {
                        Kind::Menu(_) => {
                            self.open = if self.open == Some(i) { None } else { Some(i) };
                            return None;
                        }
                        Kind::Act(a) => {
                            self.open = None;
                            return Some(a);
                        }
                    }
                }
            }
            self.open = None;
            return None;
        }
        if let Some((mx, my, mw, mh)) = self.open_menu_rect(win_w) {
            if px >= mx as f64 && px <= (mx + mw) as f64 && py >= my as f64 && py <= (my + mh) as f64 {
                let idx = ((py - my as f64) / ITEM_H as f64) as usize;
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

    /// Draws the bar (and the open menu) into the ARGB `buf`.
    pub fn draw(&self, buf: &mut [u32], win_w: usize, win_h: usize) {
        if !self.visible || win_h < BAR_H as usize {
            return;
        }
        let bx = self.bar_x(win_w);
        fill_rect(buf, win_w, win_h, bx, 0.0, self.width, BAR_H, 0xF2_14_18_22);
        fill_rect(buf, win_w, win_h, bx, 0.0, self.width, 2.0, 0xFF_4d_9b_ff);
        line(buf, win_w, win_h, bx, BAR_H - 1.0, self.width, 0xFF_2a_30_3b);

        for (i, b) in self.buttons.iter().enumerate() {
            let end = matches!(b.kind, Kind::Act(Action::Disconnect));
            let toggled = self.block_input;
            let menu_open = self.open == Some(i);
            if self.hovered_button == Some(i) || menu_open {
                let hover = if end { 0xFF_5a_30_34 } else { 0xFF_2c_36_46 };
                fill_rect(buf, win_w, win_h, bx + b.x + 2.0, 6.0, b.w - 4.0, BAR_H - 10.0, hover);
            } else if toggled {
                fill_rect(buf, win_w, win_h, bx + b.x + 2.0, 6.0, b.w - 4.0, BAR_H - 10.0, 0xFF_3a_32_22);
            }
            let color = if end {
                0xFF_f0_8a_86
            } else if toggled || menu_open {
                0xFF_f2_b0_3a
            } else {
                0xFF_e8_ec_f2
            };
            let tw = text_width(b.label);
            draw_text(buf, win_w, win_h, bx + b.x + (b.w - tw) / 2.0, 23.0, b.label, color);
        }

        if let (Some((mx, my, mw, mh)), Some(bi)) = (self.open_menu_rect(win_w), self.open) {
            let m = self.menu_index(bi).unwrap();
            fill_rect(buf, win_w, win_h, mx, my, mw, mh, 0xF7_22_27_30);
            line(buf, win_w, win_h, mx, my + mh, mw, 0xFF_2a_30_3b);
            for (idx, (label, _)) in self.menus[m].items.iter().enumerate() {
                let iy = my + idx as f32 * ITEM_H;
                if self.hovered_item == Some(idx) {
                    fill_rect(buf, win_w, win_h, mx + 2.0, iy + 2.0, mw - 4.0, ITEM_H - 4.0, 0xFF_32_3b_4a);
                }
                draw_text(buf, win_w, win_h, mx + PAD * 1.5, iy + ITEM_H * 0.66, label, 0xFF_e8_ec_f2);
            }
        }
    }
}

/// Alpha-blends a solid rectangle (ARGB, alpha in the high byte) into `buf`.
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

/// Blends `argb` over `dst` with coverage/alpha `a` (0..1).
fn blend(dst: &mut u32, argb: u32, a: f32) {
    let a = a.clamp(0.0, 1.0);
    let (sr, sg, sb) = ((argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff);
    let (dr, dg, db) = ((*dst >> 16) & 0xff, (*dst >> 8) & 0xff, *dst & 0xff);
    let mix = |s: u32, d: u32| ((s as f32 * a + d as f32 * (1.0 - a)) as u32) & 0xff;
    *dst = 0xff00_0000 | (mix(sr, dr) << 16) | (mix(sg, dg) << 8) | mix(sb, db);
}
