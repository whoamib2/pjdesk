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
    /// Switch to one remote display. The number is the host's display index.
    SelectDisplay(u8),
    /// One display at a time.
    ViewSingle,
    /// Every display in one picture.
    ViewAll,
    /// One local window per remote display.
    ViewSeparate,
    /// Close the screen window this bar belongs to.
    CloseScreen,
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
const MAP_W: f32 = 280.0;
const MAP_H: f32 = 148.0;
const MODE_H: f32 = 28.0;
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
    Screens,
}

enum Kind {
    Act(Action),
    Menu(usize),
    Collapse,
    Screens,
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
    displays: Vec<pjdesk_proto::layout::Screen>,
    current: u8,
    view_mode: u8,
    screens_open: bool,
}

impl Default for Toolbar {
    fn default() -> Self {
        let menus = vec![owned(ZOOM_ITEMS), owned(KEY_ITEMS), owned(CONTROL_ITEMS)];
        let spec: [(&'static str, &'static str, Kind, Mark); 8] = [
            ("View", "Zoom", Kind::Menu(0), Mark::View),
            ("Screens", "Switch display", Kind::Screens, Mark::Screens),
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
            displays: Vec::new(),
            current: 0,
            view_mode: 0,
            screens_open: false,
        }
    }
}

fn owned(items: &[(&str, Action)]) -> Menu {
    let items: Vec<(String, Action)> = items.iter().map(|(l, a)| ((*l).to_string(), *a)).collect();
    let width = items.iter().map(|(l, _)| text_width(l)).fold(160.0_f32, f32::max) + PAD * 3.0;
    Menu { items, width }
}

impl Toolbar {
    /// The displays the host last published, which one is current, and the view mode.
    pub fn sync(&mut self, displays: Vec<pjdesk_proto::layout::Screen>, current: u8, view_mode: u8) {
        self.displays = displays;
        self.current = current;
        self.view_mode = view_mode;
    }

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

    fn screens_popup_rect(&self, win_w: usize) -> Option<(f32, f32, f32, f32)> {
        if self.collapsed || !self.visible || !self.screens_open {
            return None;
        }
        let bi = self.buttons.iter().position(|b| matches!(b.kind, Kind::Screens))?;
        let (bx, by, _, bh) = self.bar_rect(win_w);
        let left = bx + self.buttons[bi].x;
        let height = 8.0 + MAP_H + 8.0 + MODE_H * 3.0 + 8.0;
        Some((left, by + bh + 4.0, MAP_W, height))
    }

    /// `Some` when the pointer is inside the display map. The inner value is
    /// the action, or `None` for a click that only dismisses the map.
    fn screens_click(&self, px: f64, py: f64, win_w: usize) -> Option<Option<Action>> {
        let (mx, my, mw, mh) = self.screens_popup_rect(win_w)?;
        if px < mx as f64 || px > (mx + mw) as f64 || py < my as f64 || py > (my + mh) as f64 {
            return None;
        }
        let local_x = px as f32 - mx;
        let local_y = py as f32 - my;
        if local_y >= 8.0 && local_y < 8.0 + MAP_H {
            let rects = pjdesk_proto::layout::map_rects(&self.displays, MAP_W, MAP_H, 8.0);
            for r in rects {
                if local_x >= r.x && local_x < r.x + r.w && local_y >= 8.0 + r.y && local_y < 8.0 + r.y + r.h {
                    return Some(Some(Action::SelectDisplay(r.index)));
                }
            }
            return Some(None);
        }
        let mode_top = 8.0 + MAP_H + 8.0;
        if local_y >= mode_top && local_y < mode_top + MODE_H * 3.0 {
            let row = ((local_y - mode_top) / MODE_H) as usize;
            let action = [Action::ViewSingle, Action::ViewAll, Action::ViewSeparate][row.min(2)];
            return Some(Some(action));
        }
        Some(None)
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
        if let Some((mx, my, mw, mh)) = self.screens_popup_rect(win_w) {
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
        let menu_bottom = self
            .open_menu_rect(win_w)
            .map(|(_, my, _, mh)| my + mh)
            .into_iter()
            .chain(self.screens_popup_rect(win_w).map(|(_, my, _, mh)| my + mh))
            .fold(TOP + BAR_H, f32::max);
        let reveal = TOP + BAR_H + if self.hovered_button.is_some() { 32.0 } else { 8.0 };
        if py <= reveal as f64 {
            self.visible = true;
        } else if py > menu_bottom as f64 + 16.0 {
            self.visible = false;
            self.open = None;
            self.screens_open = false;
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
        if self.screens_popup_rect(win_w).is_some_and(|(mx, my, mw, mh)| {
            px >= mx as f64 && px <= (mx + mw) as f64 && py >= my as f64 && py <= (my + mh) as f64
        }) {
            return true;
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
        if let Some(hit) = self.screens_click(px, py, win_w) {
            self.screens_open = false;
            return hit;
        }
        if let Some(i) = self.hit_button(px, py, win_w) {
            match self.buttons[i].kind {
                Kind::Menu(_) => {
                    self.screens_open = false;
                    self.open = if self.open == Some(i) { None } else { Some(i) };
                    return None;
                }
                Kind::Screens => {
                    self.open = None;
                    self.screens_open = !self.screens_open;
                    return None;
                }
                Kind::Collapse => {
                    self.collapsed = true;
                    self.open = None;
                    self.screens_open = false;
                    self.visible = false;
                    return None;
                }
                Kind::Act(a) => {
                    self.open = None;
                    self.screens_open = false;
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
        self.screens_open = false;
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
            let menu_open = self.open == Some(i) || (self.screens_open && matches!(b.kind, Kind::Screens));
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
            if self.open.is_none() && !self.screens_open {
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
        self.draw_screens(buf, win_w, win_h);
    }

    fn draw_screens(&self, buf: &mut [u32], win_w: usize, win_h: usize) {
        let Some((mx, my, mw, mh)) = self.screens_popup_rect(win_w) else {
            return;
        };
        fill_round(buf, win_w, win_h, mx, my, mw, mh, 10.0, 0xF7_22_27_30);
        let rects = pjdesk_proto::layout::map_rects(&self.displays, MAP_W, MAP_H, 8.0);
        if rects.is_empty() {
            draw_text(buf, win_w, win_h, mx + PAD, my + 28.0, "One screen", 0xFF_9a_a3_b5);
        }
        let separate = self.view_mode == pjdesk_proto::peer::VIEW_SEPARATE;
        let all = self.view_mode == pjdesk_proto::peer::VIEW_ALL;
        for r in &rects {
            let on = self.view_mode == pjdesk_proto::peer::VIEW_SINGLE && r.index == self.current;
            if on || all || separate {
                fill_round(buf, win_w, win_h, mx + r.x - 2.0, my + 8.0 + r.y - 2.0, r.w + 4.0, r.h + 4.0, 4.0, 0xFF_4d_9b_ff);
            }
            fill_round(buf, win_w, win_h, mx + r.x, my + 8.0 + r.y, r.w, r.h, 3.0, if on { 0xFF_2a_3c_52 } else { 0xFF_32_3b_4a });
            let label = (r.index + 1).to_string();
            let tw = text_width(&label);
            draw_text(
                buf,
                win_w,
                win_h,
                mx + r.x + (r.w - tw) / 2.0,
                my + 8.0 + r.y + r.h / 2.0 + 5.0,
                &label,
                0xFF_e8_ec_f2,
            );
        }
        let mode_y = my + 8.0 + MAP_H + 8.0;
        let modes = [("One screen", 0u8), ("All screens", 1), ("Separate windows", 2)];
        for (i, (label, mode)) in modes.iter().enumerate() {
            let y = mode_y + i as f32 * MODE_H;
            let on = self.view_mode == *mode;
            fill_round(
                buf,
                win_w,
                win_h,
                mx + 8.0,
                y,
                MAP_W - 16.0,
                MODE_H - 4.0,
                6.0,
                if on { 0xFF_2a_3c_52 } else { 0xFF_2c_36_46 },
            );
            draw_text(buf, win_w, win_h, mx + 16.0, y + 16.0, label, 0xFF_e8_ec_f2);
        }
    }
}

const COMPACT_H: f32 = 36.0;

/// Bottom bar on a separate screen window: the screen name, fullscreen, and close.
pub fn compact_rect(win_w: usize, win_h: usize) -> (f32, f32, f32, f32) {
    let w = 280.0_f32.min((win_w as f32 - 16.0).max(120.0));
    let x = ((win_w as f32 - w) / 2.0).max(8.0);
    let y = (win_h as f32 - COMPACT_H - 12.0).max(8.0);
    (x, y, w, COMPACT_H)
}

pub fn compact_covers(px: f64, py: f64, win_w: usize, win_h: usize) -> bool {
    let (x, y, w, h) = compact_rect(win_w, win_h);
    px >= x as f64 && py >= y as f64 && px <= (x + w) as f64 && py <= (y + h) as f64
}

pub fn compact_click(px: f64, py: f64, win_w: usize, win_h: usize) -> Option<Action> {
    let (x, y, w, h) = compact_rect(win_w, win_h);
    if px < x as f64 || py < y as f64 || px > (x + w) as f64 || py > (y + h) as f64 {
        return None;
    }
    let local = px as f32 - x;
    if local >= w - 36.0 {
        Some(Action::CloseScreen)
    } else if local >= w - 118.0 {
        Some(Action::Fullscreen)
    } else {
        None
    }
}

pub fn draw_compact(buf: &mut [u32], win_w: usize, win_h: usize, label: &str) {
    let (x, y, w, h) = compact_rect(win_w, win_h);
    fill_round(buf, win_w, win_h, x, y, w, h, 10.0, 0xF2_14_18_22);
    draw_text(buf, win_w, win_h, x + 12.0, y + 23.0, label, 0xFF_e8_ec_f2);
    draw_text(buf, win_w, win_h, x + w - 110.0, y + 23.0, "Full", 0xFF_c5_d0_de);
    draw_text(buf, win_w, win_h, x + w - 28.0, y + 23.0, "x", 0xFF_f0_8a_86);
}

pub fn draw_toast(buf: &mut [u32], win_w: usize, win_h: usize, text: &str) {
    let tw = text_width(text) + 24.0;
    let x = ((win_w as f32 - tw) / 2.0).max(8.0);
    let y = (win_h as f32 * 0.72).min(win_h as f32 - 80.0).max(48.0);
    fill_round(buf, win_w, win_h, x, y, tw, 32.0, 8.0, 0xF2_2a_24_18);
    draw_text(buf, win_w, win_h, x + 12.0, y + 21.0, text, 0xFF_f2_b0_3a);
}

/// Icons share a 14×14 box and a 2px stroke so the bar reads as one set.
fn draw_mark(buf: &mut [u32], w: usize, h: usize, x: f32, y: f32, mark: Mark, argb: u32) {
    let hole = 0xFF_14_18_22;
    match mark {
        Mark::View => {
            fill_rect(buf, w, h, x, y + 4.0, 14.0, 6.0, argb);
            fill_rect(buf, w, h, x + 2.0, y + 6.0, 10.0, 2.0, hole);
            fill_rect(buf, w, h, x + 5.0, y + 5.0, 4.0, 4.0, argb);
        }
        Mark::Keys => {
            fill_rect(buf, w, h, x, y + 2.0, 14.0, 10.0, argb);
            fill_rect(buf, w, h, x + 2.0, y + 4.0, 10.0, 6.0, hole);
            fill_rect(buf, w, h, x + 3.0, y + 5.0, 3.0, 2.0, argb);
            fill_rect(buf, w, h, x + 8.0, y + 5.0, 3.0, 2.0, argb);
        }
        Mark::Session => {
            fill_circle(buf, w, h, x + 7.0, y + 7.0, 6.0, argb);
            fill_circle(buf, w, h, x + 7.0, y + 7.0, 3.0, hole);
        }
        Mark::Files => {
            fill_rect(buf, w, h, x + 1.0, y + 1.0, 12.0, 12.0, argb);
            fill_rect(buf, w, h, x + 3.0, y + 4.0, 8.0, 7.0, hole);
            fill_rect(buf, w, h, x + 3.0, y + 1.0, 5.0, 3.0, argb);
        }
        Mark::Chat => {
            fill_rect(buf, w, h, x, y + 1.0, 14.0, 9.0, argb);
            fill_rect(buf, w, h, x + 2.0, y + 3.0, 10.0, 5.0, hole);
            fill_rect(buf, w, h, x + 3.0, y + 9.0, 3.0, 4.0, argb);
        }
        Mark::End => {
            fill_rect(buf, w, h, x + 2.0, y + 1.0, 10.0, 2.0, argb);
            fill_rect(buf, w, h, x + 2.0, y + 11.0, 10.0, 2.0, argb);
            fill_rect(buf, w, h, x + 2.0, y + 1.0, 2.0, 12.0, argb);
            fill_rect(buf, w, h, x + 10.0, y + 1.0, 2.0, 12.0, argb);
            fill_rect(buf, w, h, x + 5.0, y + 4.0, 4.0, 2.0, argb);
            fill_rect(buf, w, h, x + 6.0, y + 4.0, 2.0, 6.0, argb);
        }
        Mark::Collapse => fill_rect(buf, w, h, x + 1.0, y + 6.0, 12.0, 2.0, argb),
        Mark::Screens => {
            fill_rect(buf, w, h, x, y + 1.0, 6.0, 5.0, argb);
            fill_rect(buf, w, h, x + 8.0, y + 1.0, 6.0, 5.0, argb);
            fill_rect(buf, w, h, x + 2.0, y + 8.0, 10.0, 5.0, argb);
            fill_rect(buf, w, h, x + 1.0, y + 2.0, 4.0, 3.0, hole);
            fill_rect(buf, w, h, x + 9.0, y + 2.0, 4.0, 3.0, hole);
            fill_rect(buf, w, h, x + 3.0, y + 9.0, 8.0, 3.0, hole);
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

#[cfg(feature = "ui-shots")]
pub(crate) fn render_monitor_shots(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    write_png(&dir.join("monitors-switcher.png"), 1100, 720, paint_switcher);
    write_png(&dir.join("monitors-all.png"), 1100, 640, paint_all_screens);
    write_png(&dir.join("monitors-separate.png"), 1100, 640, paint_separate);
}

#[cfg(feature = "ui-shots")]
fn sample_bar() -> Toolbar {
    let mut bar = Toolbar::default();
    bar.sync(pjdesk_proto::layout::sample_layout(), 0, 0);
    bar.visible = true;
    bar
}

#[cfg(feature = "ui-shots")]
fn paint_switcher(buf: &mut [u32], w: usize, h: usize) {
    buf.fill(0xFF_14_18_22);
    let mut bar = sample_bar();
    bar.screens_open = true;
    let (bx, _, _, _) = bar.bar_rect(w);
    let _ = bx;
    bar.draw(buf, w, h);
}

#[cfg(feature = "ui-shots")]
fn paint_all_screens(buf: &mut [u32], w: usize, h: usize) {
    buf.fill(0xFF_10_13_18);
    let screens = pjdesk_proto::layout::sample_layout();
    let rects = pjdesk_proto::layout::map_rects(&screens, w as f32, h as f32, 36.0);
    let colors = [0xFF_2f_4a_68, 0xFF_2c_45_38, 0xFF_4a_3c_2a];
    for (i, r) in rects.iter().enumerate() {
        fill_rect(buf, w, h, r.x, r.y, r.w, r.h, colors[i % colors.len()]);
        let label = format!("{}", r.index + 1);
        draw_text(buf, w, h, r.x + 12.0, r.y + 28.0, &label, 0xFF_e8_ec_f2);
    }
    let mut bar = sample_bar();
    bar.view_mode = pjdesk_proto::peer::VIEW_ALL;
    bar.draw(buf, w, h);
}

#[cfg(feature = "ui-shots")]
fn paint_separate(buf: &mut [u32], w: usize, h: usize) {
    buf.fill(0xFF_10_13_18);
    let frames = [
        (36.0_f32, 36.0, 500.0, 560.0, "Office PC — Screen 1", "Screen 1", 0xFF_2f_4a_68),
        (564.0, 78.0, 500.0, 480.0, "Office PC — Screen 2", "Screen 2", 0xFF_2c_45_38),
    ];
    for (x, y, ww, hh, title, label, color) in frames {
        fill_round(buf, w, h, x, y, ww, 32.0, 8.0, 0xFF_1c_22_2c);
        draw_text(buf, w, h, x + 12.0, y + 21.0, title, 0xFF_e8_ec_f2);
        fill_rect(buf, w, h, x, y + 32.0, ww, hh - 32.0, color);
        let bar_w = 260.0;
        let bx = x + (ww - bar_w) / 2.0;
        let by = y + hh - 48.0;
        fill_round(buf, w, h, bx, by, bar_w, 36.0, 10.0, 0xF2_14_18_22);
        draw_text(buf, w, h, bx + 12.0, by + 23.0, label, 0xFF_e8_ec_f2);
        draw_text(buf, w, h, bx + bar_w - 110.0, by + 23.0, "Full", 0xFF_c5_d0_de);
        draw_text(buf, w, h, bx + bar_w - 28.0, by + 23.0, "x", 0xFF_f0_8a_86);
    }
}

#[cfg(feature = "ui-shots")]
fn write_png(path: &std::path::Path, w: u32, h: u32, paint: fn(&mut [u32], usize, usize)) {
    let mut pixels = vec![0u32; w as usize * h as usize];
    paint(&mut pixels, w as usize, h as usize);
    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for p in pixels {
        rgba.push(((p >> 16) & 0xff) as u8);
        rgba.push(((p >> 8) & 0xff) as u8);
        rgba.push((p & 0xff) as u8);
        rgba.push(255);
    }
    let file = std::fs::File::create(path).unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut wr| wr.write_image_data(&rgba)).unwrap_or_else(|e| panic!("png {}: {e}", path.display()));
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

    #[test]
    fn screen_map_click_selects_the_display_under_the_pointer() {
        let mut bar = Toolbar::default();
        bar.sync(pjdesk_proto::layout::sample_layout(), 0, 0);
        let w = 1100;
        let screens = bar.buttons.iter().find(|b| matches!(b.kind, Kind::Screens)).unwrap();
        let (bx, by, _, _) = bar.bar_rect(w);
        assert_eq!(bar.click((bx + screens.x + 8.0) as f64, (by + 12.0) as f64, w), None);
        assert!(bar.screens_open);
        let (mx, my, _, _) = bar.screens_popup_rect(w).unwrap();
        let rects = pjdesk_proto::layout::map_rects(&bar.displays, MAP_W, MAP_H, 8.0);
        let left = rects.iter().find(|r| r.index == 1).unwrap();
        let x = mx + left.x + left.w / 2.0;
        let y = my + 8.0 + left.y + left.h / 2.0;
        assert_eq!(bar.click(x as f64, y as f64, w), Some(Action::SelectDisplay(1)));
    }

    #[test]
    fn screens_menu_offers_separate_windows() {
        let mut bar = Toolbar::default();
        bar.sync(pjdesk_proto::layout::sample_layout(), 0, 0);
        let w = 1100usize;
        let screens = bar.buttons.iter().find(|b| matches!(b.kind, Kind::Screens)).unwrap();
        let (bx, by, _, _) = bar.bar_rect(w);
        assert_eq!(bar.click((bx + screens.x + 8.0) as f64, (by + 12.0) as f64, w), None);
        let (mx, my, _, _) = bar.screens_popup_rect(w).unwrap();
        let mode_top = my + 8.0 + MAP_H + 8.0;
        let y = mode_top + MODE_H * 2.0 + 8.0;
        assert_eq!(bar.click((mx + 24.0) as f64, y as f64, w), Some(Action::ViewSeparate));
    }
}
