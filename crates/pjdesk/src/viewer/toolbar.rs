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
    /// Show another computer already open in this window.
    SelectSession(usize),
    /// End that computer's session. The window stays if another tab is open.
    CloseSession(usize),
    /// Android host: 0 Back, 1 Home, 2 Recents, 3 Volume up, 4 Volume down, 5 Power, 6 Notifications.
    Android(u8),
    /// Show or hide the Android navigation bar under the phone picture.
    ToggleAndroidBar,
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
/// Persistent screen tabs. Hidden when the remote has a single display.
pub const TAB_H: f32 = 40.0;
/// Computer tabs. Hidden until the window is given a session list.
pub const SESSION_H: f32 = 36.0;
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

const ANDROID_ITEMS: &[(&str, Action)] = &[
    ("Back", Action::Android(0)),
    ("Home", Action::Android(1)),
    ("Recents", Action::Android(2)),
    ("Volume up", Action::Android(3)),
    ("Volume down", Action::Android(4)),
    ("Power", Action::Android(5)),
    ("Notifications", Action::Android(6)),
];

/// Height of the navigation bar docked under an Android picture.
pub const ANDROID_BAR_H: f32 = 96.0;

/// Shown on the bar when the phone's accessibility service is not running.
pub const ANDROID_CONTROL_HINT: &str = "Turn on control on the phone";

/// One control on the Android navigation bar. `which` is the viewer index
/// [`crate::viewer::window`] sends: 0 Back, 1 Home, 2 Recents, 3 Vol+, 4 Vol−, 5 Power, 6 Notify.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AndroidButton {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub which: u8,
}

/// Where the bar sits in the window. It matches the phone picture when there is one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AndroidBarBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A click on the bar. `Chrome` is the padding, so it is not a phone gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AndroidHit {
    Button(u8),
    Chrome,
}

/// The bar under `picture`, or a centered bar when the picture has not arrived.
pub fn android_bar_box(pic_x: i32, pic_y: i32, pic_w: usize, pic_h: usize, win_w: usize, win_h: usize) -> AndroidBarBox {
    let h = ANDROID_BAR_H;
    if win_w == 0 || win_h == 0 {
        return AndroidBarBox { x: 0.0, y: 0.0, w: 0.0, h };
    }
    if pic_w < 8 || pic_h < 8 {
        let max_w = win_w as f32;
        let w = (max_w * 0.72).clamp(320.0_f32.min(max_w), 520.0_f32.min(max_w));
        let x = ((max_w - w) / 2.0).max(0.0);
        return AndroidBarBox { x, y: (win_h as f32 - h).max(0.0), w, h };
    }
    let left = pic_x.max(0) as f32;
    let right = (pic_x.saturating_add(pic_w as i32)).clamp(0, win_w as i32) as f32;
    let bottom = pic_y.saturating_add(pic_h as i32).clamp(0, win_h as i32);
    let y = if bottom as f32 + h <= win_h as f32 + 0.5 {
        bottom as f32
    } else {
        (win_h as f32 - h).max(0.0)
    };
    AndroidBarBox { x: left, y, w: (right - left).max(1.0), h }
}

/// Back, Home, and Recents centered and large. Notify and Vol− on the left, Vol+ and Power on the right.
pub fn android_bar_buttons(bar: AndroidBarBox, disabled: bool) -> Vec<AndroidButton> {
    if bar.w < 8.0 || bar.h < 8.0 {
        return Vec::new();
    }
    let hint_band = if disabled { 20.0 } else { 0.0 };
    let region_top = bar.y + 8.0;
    let region_h = (bar.h - 16.0 - hint_band).max(20.0);
    let mut nav = 52.0_f32.min(region_h);
    let mut end_h = 32.0_f32.min(nav);
    let mut nav_gap = 14.0_f32;
    let mut end_gap = 6.0_f32;
    let mut pad = 8.0_f32;
    let side = 10.0_f32;
    let labels = ["Notify", "Vol-", "Vol+", "Power"];
    let mut end_ws: Vec<f32> = labels.iter().map(|label| text_width(label) + 18.0).collect();
    let left_w = end_ws[0] + end_gap + end_ws[1];
    let right_w = end_ws[2] + end_gap + end_ws[3];
    let cluster = nav * 3.0 + nav_gap * 2.0;
    let flank = (pad + left_w + side).max(pad + right_w + side);
    let needed = cluster + flank * 2.0;
    if needed > bar.w && needed > 0.0 {
        let scale = bar.w / needed;
        nav *= scale;
        end_h *= scale;
        nav_gap *= scale;
        end_gap *= scale;
        pad *= scale;
        for w in &mut end_ws {
            *w *= scale;
        }
    }
    let cluster = nav * 3.0 + nav_gap * 2.0;
    let cluster_x = bar.x + (bar.w - cluster) / 2.0;
    let nav_y = region_top + (region_h - nav) / 2.0;
    let end_y = nav_y + (nav - end_h) / 2.0;
    let mut buttons = Vec::with_capacity(7);
    let mut x = bar.x + pad;
    let left_which = [6u8, 4];
    for i in 0..2 {
        buttons.push(AndroidButton { x, y: end_y, w: end_ws[i], h: end_h, which: left_which[i] });
        x += end_ws[i] + end_gap;
    }
    for i in 0..3 {
        buttons.push(AndroidButton {
            x: cluster_x + i as f32 * (nav + nav_gap),
            y: nav_y,
            w: nav,
            h: nav,
            which: i as u8,
        });
    }
    x = bar.x + bar.w - pad - end_ws[2] - end_gap - end_ws[3];
    buttons.push(AndroidButton { x, y: end_y, w: end_ws[2], h: end_h, which: 3 });
    x += end_ws[2] + end_gap;
    buttons.push(AndroidButton { x, y: end_y, w: end_ws[3], h: end_h, which: 5 });
    buttons
}

/// Which part of the navigation bar the point is on.
pub fn android_bar_hit(
    x: f64,
    y: f64,
    pic_x: i32,
    pic_y: i32,
    pic_w: usize,
    pic_h: usize,
    win_w: usize,
    win_h: usize,
    disabled: bool,
) -> Option<AndroidHit> {
    let bar = android_bar_box(pic_x, pic_y, pic_w, pic_h, win_w, win_h);
    if bar.w < 1.0 || x < bar.x as f64 || y < bar.y as f64 || x > (bar.x + bar.w) as f64 || y > (bar.y + bar.h) as f64 {
        return None;
    }
    let buttons = android_bar_buttons(bar, disabled);
    if let Some(button) = buttons.iter().find(|b| b.which <= 2 && inside_button(b, x, y)) {
        return Some(AndroidHit::Button(button.which));
    }
    if let Some(button) = buttons.iter().find(|b| inside_button(b, x, y)) {
        return Some(AndroidHit::Button(button.which));
    }
    Some(AndroidHit::Chrome)
}

fn inside_button(button: &AndroidButton, x: f64, y: f64) -> bool {
    x >= button.x as f64 && y >= button.y as f64 && x <= (button.x + button.w) as f64 && y <= (button.y + button.h) as f64
}

pub(crate) fn draw_android_bar(
    buf: &mut [u32],
    win_w: usize,
    win_h: usize,
    pic_x: i32,
    pic_y: i32,
    pic_w: usize,
    pic_h: usize,
    disabled: bool,
) {
    let bar = android_bar_box(pic_x, pic_y, pic_w, pic_h, win_w, win_h);
    if bar.w < 1.0 || win_w == 0 || win_h == 0 {
        return;
    }
    let bg = if disabled { 0xF0_18_1a_20 } else { 0xF0_10_13_18 };
    let glyph = if disabled { 0xFF_6e_76_84 } else { 0xFF_f2_f4_f8 };
    let well = if disabled { 0xFF_28_2c_34 } else { 0xFF_2a_33_42 };
    let end_well = if disabled { 0xFF_24_28_30 } else { 0xFF_22_28_33 };
    fill_rect(buf, win_w, win_h, bar.x, bar.y, bar.w, bar.h, bg);
    fill_rect(buf, win_w, win_h, bar.x, bar.y, bar.w, 1.0, 0xFF_2e_36_44);
    for button in android_bar_buttons(bar, disabled) {
        if button.which <= 2 {
            draw_nav_glyph(buf, win_w, win_h, &button, glyph, well);
        } else {
            fill_round(buf, win_w, win_h, button.x, button.y, button.w, button.h, 8.0, end_well);
            let label = match button.which {
                6 => "Notify",
                4 => "Vol-",
                3 => "Vol+",
                _ => "Power",
            };
            let tw = text_width(label);
            if tw <= button.w - 4.0 {
                draw_text(buf, win_w, win_h, button.x + (button.w - tw) / 2.0, button.y + button.h * 0.68, label, glyph);
            }
        }
    }
    if disabled {
        let tw = text_width(ANDROID_CONTROL_HINT);
        draw_text(
            buf,
            win_w,
            win_h,
            bar.x + (bar.w - tw) / 2.0,
            bar.y + bar.h - 8.0,
            ANDROID_CONTROL_HINT,
            0xFF_e7_c2_b6,
        );
    }
}

fn draw_nav_glyph(buf: &mut [u32], w: usize, h: usize, button: &AndroidButton, color: u32, well: u32) {
    let cx = button.x + button.w / 2.0;
    let cy = button.y + button.h / 2.0;
    let r = button.w.min(button.h) / 2.0 - 1.0;
    if r < 2.0 {
        return;
    }
    fill_circle(buf, w, h, cx, cy, r, well);
    let s = r * 0.62;
    match button.which {
        0 => fill_triangle(buf, w, h, cx - s * 0.95, cy, cx + s * 0.8, cy - s, cx + s * 0.8, cy + s, color),
        1 => fill_circle(buf, w, h, cx, cy, s * 0.72, color),
        2 => {
            let side = s * 1.35;
            fill_round(buf, w, h, cx - side / 2.0, cy - side / 2.0, side, side, (side * 0.18).max(2.0), color);
        }
        _ => {}
    }
}

pub(crate) fn draw_banner(buf: &mut [u32], win_w: usize, win_h: usize, y: f32, text: &str) {
    fill_rect(buf, win_w, win_h, 0.0, y, win_w as f32, 36.0, 0xF0_3a_18_18);
    draw_text(buf, win_w, win_h, 16.0, y + 10.0, text, 0xFF_ff_d0_c8);
}

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
    /// Light client theme. The tab strip follows this; the floating bar stays dark glass.
    light: bool,
    hovered_slot: Option<usize>,
    /// Computer names in this window. Empty keeps the older single-session strip.
    sessions: Vec<String>,
    unread: Vec<u32>,
    active_session: usize,
    hovered_session: Option<usize>,
    /// The other computer sent ChatReady. Until then Chat stays visible and does nothing.
    chat_ready: bool,
    android: bool,
    /// The navigation bar under an Android picture. On until the Phone menu hides it.
    android_nav: bool,
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
            ("Chat", pjdesk_proto::peer::CHAT_NEEDS_TIP, Kind::Act(Action::OpenPanel(1)), Mark::Chat),
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
            light: false,
            hovered_slot: None,
            sessions: Vec::new(),
            unread: Vec::new(),
            active_session: 0,
            hovered_session: None,
            chat_ready: false,
            android: false,
            android_nav: true,
        }
    }
}

struct Chrome {
    panel: u32,
    card: u32,
    elev: u32,
    line: u32,
    text: u32,
    muted: u32,
    accent: u32,
}

fn chrome(light: bool) -> Chrome {
    if light {
        Chrome {
            panel: 0xFF_f3_f5_f8,
            card: 0xFF_ff_ff_ff,
            elev: 0xFF_e8_ec_f2,
            line: 0xFF_d5_db_e3,
            text: 0xFF_1c_23_30,
            muted: 0xFF_5c_67_78,
            accent: 0xFF_2f_6f_ed,
        }
    } else {
        Chrome {
            panel: 0xFF_10_13_18,
            card: 0xFF_18_1c_24,
            elev: 0xFF_22_28_33,
            line: 0xFF_2e_36_44,
            text: 0xFF_e9_ed_f3,
            muted: 0xFF_9a_a3_b5,
            accent: 0xFF_4d_9b_ff,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SlotKind {
    Display(u8),
    All,
    Separate,
}

struct Slot {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    kind: SlotKind,
    label: String,
    tip: String,
    active: bool,
}

struct SessionSlot {
    index: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    close: (f32, f32, f32, f32),
    label: String,
    badge: String,
    active: bool,
}

fn badge_text(count: u32) -> String {
    if count == 0 {
        String::new()
    } else if count > 9 {
        "9+".into()
    } else {
        count.to_string()
    }
}

fn fit_label(label: &str, max_w: f32) -> String {
    if text_width(label) <= max_w {
        return label.to_string();
    }
    let mut out = String::new();
    for c in label.chars() {
        let next = format!("{out}{c}…");
        if text_width(&next) > max_w {
            break;
        }
        out.push(c);
    }
    if out.is_empty() { "…".into() } else { format!("{out}…") }
}

fn tooltip_rect_of(slot: &Slot, below: f32) -> Option<(f32, f32, f32, f32)> {
    if slot.tip.is_empty() {
        return None;
    }
    let tw = text_width(&slot.tip) + 16.0;
    Some((slot.x, below + 6.0, tw, 22.0))
}

fn draw_windows_icon(buf: &mut [u32], w: usize, h: usize, x: f32, y: f32, color: u32, hole: u32) {
    fill_rect(buf, w, h, x + 4.0, y, 10.0, 8.0, color);
    fill_rect(buf, w, h, x + 5.0, y + 1.0, 8.0, 6.0, hole);
    fill_rect(buf, w, h, x, y + 5.0, 10.0, 8.0, color);
    fill_rect(buf, w, h, x + 1.0, y + 6.0, 8.0, 6.0, hole);
}

fn owned(items: &[(&str, Action)]) -> Menu {
    let items: Vec<(String, Action)> = items.iter().map(|(l, a)| ((*l).to_string(), *a)).collect();
    let width = items.iter().map(|(l, _)| text_width(l)).fold(160.0_f32, f32::max) + PAD * 3.0;
    Menu { items, width }
}

fn nav_toggle_label(on: bool) -> &'static str {
    if on { "Hide navigation bar" } else { "Show navigation bar" }
}

impl Toolbar {
    /// The displays the host last published, which one is current, and the view mode.
    pub fn sync(&mut self, displays: Vec<pjdesk_proto::layout::Screen>, current: u8, view_mode: u8) {
        self.displays = displays;
        self.current = current;
        self.view_mode = view_mode;
    }

    pub fn set_light(&mut self, light: bool) {
        self.light = light;
    }

    pub fn letterbox(&self) -> u32 {
        chrome(self.light).panel
    }

    /// Names of the computers open in this window, and which one is showing.
    ///
    /// An empty list leaves the strip out, so a remote with one display still
    /// has no tab row.
    pub fn set_sessions(&mut self, names: &[String], active: usize) {
        if self.sessions != names {
            self.unread.resize(names.len(), 0);
        }
        self.sessions = names.to_vec();
        self.active_session = active;
        if self.hovered_session.is_some_and(|i| i >= self.sessions.len()) {
            self.hovered_session = None;
        }
    }

    /// Unread chat counts, one per session tab. A closed chat panel keeps the badge.
    pub fn set_unread(&mut self, counts: &[u32]) {
        self.unread = counts.to_vec();
        self.unread.resize(self.sessions.len(), 0);
    }

    pub fn set_chat_ready(&mut self, ready: bool) {
        self.chat_ready = ready;
        let tip = if ready { "Messages" } else { pjdesk_proto::peer::CHAT_NEEDS_TIP };
        for button in &mut self.buttons {
            if button.label == "Chat" {
                button.tip = tip;
            }
        }
    }

    /// Height reserved at the top for computer tabs and, under them, screen tabs.
    pub fn tab_height(&self) -> f32 {
        self.session_height() + if self.shows_tabs() { TAB_H } else { 0.0 }
    }

    fn session_height(&self) -> f32 {
        if self.shows_sessions() { SESSION_H } else { 0.0 }
    }

    fn shows_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    fn shows_tabs(&self) -> bool {
        self.displays.len() >= 2
    }

    /// Screen tabs start under the computer tabs when those are showing.
    fn monitor_top(&self) -> f32 {
        self.session_height()
    }

    /// Shows or hides the Android section (Back, Home, Recents, volume, power).
    pub fn set_android(&mut self, on: bool) {
        if self.android == on {
            return;
        }
        self.android = on;
        self.open = None;
        let mut menus = vec![owned(ZOOM_ITEMS), owned(KEY_ITEMS), owned(CONTROL_ITEMS)];
        let mut spec: Vec<(&str, &str, Kind, Mark)> = vec![
            ("View", "Zoom", Kind::Menu(0), Mark::View),
            ("Screens", "Switch display", Kind::Screens, Mark::Screens),
            ("Keys", "Send special keys", Kind::Menu(1), Mark::Keys),
            ("Session", "Audio, privacy, record, restart", Kind::Menu(2), Mark::Session),
        ];
        if on {
            menus.push(owned(ANDROID_ITEMS));
            let label = nav_toggle_label(self.android_nav);
            let item_w = text_width(label) + PAD * 3.0;
            let menu = menus.last_mut().unwrap();
            if item_w > menu.width {
                menu.width = item_w;
            }
            menu.items.push((label.to_string(), Action::ToggleAndroidBar));
            spec.push(("Phone", "Navigation bar, Back, Home, volume, power", Kind::Menu(3), Mark::Session));
        }
        spec.extend([
            ("Files", "Send and receive files", Kind::Act(Action::OpenPanel(0)), Mark::Files),
            ("Chat", pjdesk_proto::peer::CHAT_NEEDS_TIP, Kind::Act(Action::OpenPanel(1)), Mark::Chat),
            ("End", "Disconnect", Kind::Act(Action::Disconnect), Mark::End),
            ("Hide", "Hide the toolbar", Kind::Collapse, Mark::Collapse),
        ]);
        let mut buttons = Vec::new();
        let mut x = PAD;
        for (label, tip, kind, mark) in spec {
            let w = 16.0 + 6.0 + text_width(label) + PAD * 2.0;
            buttons.push(Button { label, tip, kind, mark, x, w });
            x += w;
        }
        self.buttons = buttons;
        self.menus = menus;
        self.width = x + PAD;
        if self.chat_ready {
            for button in &mut self.buttons {
                if button.label == "Chat" {
                    button.tip = "Messages";
                }
            }
        }
    }

    /// The navigation bar is drawn for an Android peer until the viewer hides it.
    pub fn shows_android_nav(&self) -> bool {
        self.android && self.android_nav
    }

    pub fn android_nav_on(&self) -> bool {
        self.android_nav
    }

    /// Phone menu: Hide navigation bar / Show navigation bar.
    pub fn toggle_android_nav(&mut self) {
        if !self.android {
            return;
        }
        self.android_nav = !self.android_nav;
        let label = nav_toggle_label(self.android_nav);
        if let Some(menu) = self.menus.get_mut(3) {
            if let Some(item) = menu.items.iter_mut().find(|(_, action)| *action == Action::ToggleAndroidBar) {
                item.0 = label.to_string();
            }
            let w = text_width(label) + PAD * 3.0;
            if w > menu.width {
                menu.width = w;
            }
        }
    }

    fn bar_rect(&self, win_w: usize) -> (f32, f32, f32, f32) {
        let w = if self.collapsed { self.collapsed_w } else { self.width };
        let x = ((win_w as f32 - w) / 2.0).max(8.0);
        // Leave room under the tabs for a resolution tooltip.
        let y = if self.shows_tabs() {
            self.tab_height() + 36.0
        } else if self.shows_sessions() {
            self.tab_height() + 8.0
        } else {
            TOP
        };
        (x, y, w, BAR_H)
    }

    /// One tab per display, then All screens, then the separate-windows button.
    fn tab_slots(&self, win_w: usize) -> Vec<Slot> {
        if !self.shows_tabs() || win_w < 120 {
            return Vec::new();
        }
        let y = self.monitor_top() + 6.0;
        let h = 28.0;
        let right = 28.0;
        let limit = win_w as f32 - right - 16.0;
        let mut order: Vec<usize> = (0..self.displays.len()).collect();
        order.sort_by_key(|&i| self.displays[i].index);
        let mut slots = Vec::new();
        let mut x = 8.0;
        for i in order {
            let screen = &self.displays[i];
            let label = format!("Screen {}", screen.index + 1);
            let w = (text_width(&label) + 28.0).max(88.0);
            if x + w > limit {
                break;
            }
            let tip = if screen.width > 0 && screen.height > 0 {
                format!("{} × {}", screen.width, screen.height)
            } else {
                String::new()
            };
            slots.push(Slot {
                x,
                y,
                w,
                h,
                kind: SlotKind::Display(screen.index),
                label,
                tip,
                active: self.view_mode == pjdesk_proto::peer::VIEW_SINGLE && screen.index == self.current,
            });
            x += w + 4.0;
        }
        let label = "All screens".to_string();
        let w = text_width(&label) + 28.0;
        if x + w <= limit {
            slots.push(Slot {
                x,
                y,
                w,
                h,
                kind: SlotKind::All,
                label,
                tip: "Every display in one picture".into(),
                active: self.view_mode == pjdesk_proto::peer::VIEW_ALL,
            });
        }
        slots.push(Slot {
            x: win_w as f32 - right - 8.0,
            y,
            w: right,
            h,
            kind: SlotKind::Separate,
            label: String::new(),
            tip: "Separate windows".into(),
            active: self.view_mode == pjdesk_proto::peer::VIEW_SEPARATE,
        });
        slots
    }

    fn hit(px: f64, py: f64, x: f32, y: f32, w: f32, h: f32) -> bool {
        px >= x as f64 && py >= y as f64 && px <= (x + w) as f64 && py <= (y + h) as f64
    }

    fn point_in(px: f64, py: f64, s: &Slot) -> bool {
        Self::hit(px, py, s.x, s.y, s.w, s.h)
    }

    fn slot_at(&self, px: f64, py: f64, win_w: usize) -> Option<usize> {
        let slots = self.tab_slots(win_w);
        if let Some(i) = self.hovered_slot {
            if let Some(tip) = tooltip_rect_of(slots.get(i)?, self.monitor_top() + TAB_H) {
                if px >= tip.0 as f64
                    && py >= tip.1 as f64
                    && px <= (tip.0 + tip.2) as f64
                    && py <= (tip.1 + tip.3) as f64
                {
                    return Some(i);
                }
            }
        }
        slots.iter().position(|s| Self::point_in(px, py, s))
    }

    fn tab_action(&self, px: f64, py: f64, win_w: usize) -> Option<Action> {
        let slots = self.tab_slots(win_w);
        let s = slots.iter().find(|s| Self::point_in(px, py, s))?;
        Some(match s.kind {
            SlotKind::Display(index) => Action::SelectDisplay(index),
            SlotKind::All => Action::ViewAll,
            SlotKind::Separate => Action::ViewSeparate,
        })
    }

    fn session_slots(&self, win_w: usize) -> Vec<SessionSlot> {
        if !self.shows_sessions() || win_w < 80 {
            return Vec::new();
        }
        let n = self.sessions.len();
        let gap = 4.0;
        let pad = 8.0;
        let avail = (win_w as f32 - pad * 2.0 - gap * (n.saturating_sub(1) as f32)).max(72.0);
        let w = (avail / n as f32).clamp(72.0, 220.0);
        let h = 28.0;
        let y = 4.0;
        let mut x = pad;
        let mut slots = Vec::with_capacity(n);
        for (index, label) in self.sessions.iter().enumerate() {
            let close_w = 22.0;
            let badge = badge_text(self.unread.get(index).copied().unwrap_or(0));
            let badge_w = if badge.is_empty() { 0.0 } else { text_width(&badge) + 14.0 };
            let close = (x + w - close_w - 4.0, y + 4.0, close_w, h - 8.0);
            slots.push(SessionSlot {
                index,
                x,
                y,
                w,
                h,
                close,
                label: fit_label(label, (w - close_w - 20.0 - badge_w).max(24.0)),
                badge,
                active: index == self.active_session,
            });
            x += w + gap;
        }
        slots
    }

    fn session_at(&self, px: f64, py: f64, win_w: usize) -> Option<usize> {
        self.session_slots(win_w).iter().position(|s| Self::hit(px, py, s.x, s.y, s.w, s.h))
    }

    fn session_action(&self, px: f64, py: f64, win_w: usize) -> Option<Action> {
        let slots = self.session_slots(win_w);
        let slot = slots.iter().find(|s| Self::hit(px, py, s.x, s.y, s.w, s.h))?;
        let (cx, cy, cw, ch) = slot.close;
        let on_close = Self::hit(px, py, cx, cy, cw, ch);
        Some(if on_close { Action::CloseSession(slot.index) } else { Action::SelectSession(slot.index) })
    }

    fn draw_sessions(&self, buf: &mut [u32], win_w: usize, win_h: usize) {
        let slots = self.session_slots(win_w);
        if slots.is_empty() || win_h < SESSION_H as usize {
            return;
        }
        let c = chrome(self.light);
        fill_rect(buf, win_w, win_h, 0.0, 0.0, win_w as f32, SESSION_H, c.panel);
        fill_rect(buf, win_w, win_h, 0.0, SESSION_H - 1.0, win_w as f32, 1.0, c.line);
        for slot in &slots {
            let hover = self.hovered_session == Some(slot.index);
            let fill = if slot.active {
                Some(c.card)
            } else if hover {
                Some(c.elev)
            } else {
                None
            };
            if let Some(color) = fill {
                fill_round(buf, win_w, win_h, slot.x, slot.y, slot.w, slot.h, 8.0, color);
            }
            if slot.active {
                fill_rect(buf, win_w, win_h, slot.x + 12.0, slot.y + slot.h - 2.0, slot.w - 24.0, 2.0, c.accent);
            }
            let color = if slot.active || hover { c.text } else { c.muted };
            draw_text(buf, win_w, win_h, slot.x + 10.0, slot.y + 19.0, &slot.label, color);
            if !slot.badge.is_empty() {
                let bw = text_width(&slot.badge) + 10.0;
                let (cx, _, _, _) = slot.close;
                let bx = (cx - bw - 4.0).max(slot.x + 4.0);
                fill_round(buf, win_w, win_h, bx, slot.y + 5.0, bw, slot.h - 10.0, 8.0, c.accent);
                draw_text(buf, win_w, win_h, bx + 5.0, slot.y + 19.0, &slot.badge, 0xFF_ff_ff_ff);
            }
            let (cx, cy, cw, ch) = slot.close;
            if hover {
                fill_round(buf, win_w, win_h, cx, cy, cw, ch, 4.0, c.elev);
            }
            let mark = "×";
            let mw = text_width(mark);
            draw_text(buf, win_w, win_h, cx + (cw - mw) / 2.0, cy + ch - 4.0, mark, color);
        }
    }

    pub fn draw_tabs(&self, buf: &mut [u32], win_w: usize, win_h: usize) {
        self.draw_sessions(buf, win_w, win_h);
        let slots = self.tab_slots(win_w);
        let top = self.monitor_top();
        if slots.is_empty() || win_h < (top + TAB_H) as usize {
            return;
        }
        let c = chrome(self.light);
        fill_rect(buf, win_w, win_h, 0.0, top, win_w as f32, TAB_H, c.panel);
        fill_rect(buf, win_w, win_h, 0.0, top + TAB_H - 1.0, win_w as f32, 1.0, c.line);
        for (i, slot) in slots.iter().enumerate() {
            let hover = self.hovered_slot == Some(i);
            let fill = if slot.active {
                Some(c.card)
            } else if hover {
                Some(c.elev)
            } else {
                None
            };
            if let Some(color) = fill {
                fill_round(buf, win_w, win_h, slot.x, slot.y, slot.w, slot.h, 8.0, color);
            }
            if slot.active {
                fill_rect(buf, win_w, win_h, slot.x + 12.0, slot.y + slot.h - 2.0, slot.w - 24.0, 2.0, c.accent);
            }
            let color = if slot.active {
                c.text
            } else if hover {
                c.text
            } else {
                c.muted
            };
            if slot.kind == SlotKind::Separate {
                let hole = fill.unwrap_or(c.panel);
                draw_windows_icon(buf, win_w, win_h, slot.x + (slot.w - 14.0) / 2.0, slot.y + 7.0, color, hole);
            } else {
                let tw = text_width(&slot.label);
                draw_text(buf, win_w, win_h, slot.x + (slot.w - tw) / 2.0, slot.y + 19.0, &slot.label, color);
            }
        }
        if let Some(i) = self.hovered_slot {
            if let Some(slot) = slots.get(i) {
                if let Some((x, y, tw, th)) = tooltip_rect_of(slot, self.monitor_top() + TAB_H) {
                    let x = x.min((win_w as f32 - tw - 8.0).max(8.0));
                    fill_round(buf, win_w, win_h, x, y, tw, th, 6.0, c.card);
                    fill_rect(buf, win_w, win_h, x, y, tw, 1.0, c.line);
                    draw_text(buf, win_w, win_h, x + 8.0, y + 15.0, &slot.tip, c.text);
                }
            }
        }
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
        let strip = self.tab_height();
        if strip > 0.0 && py >= 0.0 && py <= strip as f64 && px >= 0.0 && px <= win_w as f64 {
            return true;
        }
        if self.slot_at(px, py, win_w).is_some() {
            return true;
        }
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
        if let Some(i) = self.session_at(px, py, win_w) {
            self.hovered_session = Some(i);
            self.hovered_slot = None;
            self.hovered_button = None;
            return true;
        }
        self.hovered_session = None;
        if let Some(i) = self.slot_at(px, py, win_w) {
            self.hovered_slot = Some(i);
            self.hovered_button = None;
            return true;
        }
        self.hovered_slot = None;
        let strip = self.tab_height();
        if strip > 0.0 && py >= 0.0 && py <= strip as f64 && px >= 0.0 && px <= win_w as f64 {
            self.hovered_button = None;
            return true;
        }
        if self.collapsed {
            self.hovered_button = self.hit_button(px, py, win_w);
            return self.hovered_button.is_some();
        }
        let (_, by, _, bh) = self.bar_rect(win_w);
        let menu_bottom = self
            .open_menu_rect(win_w)
            .map(|(_, my, _, mh)| my + mh)
            .into_iter()
            .chain(self.screens_popup_rect(win_w).map(|(_, my, _, mh)| my + mh))
            .fold(by + bh, f32::max);
        let reveal = by + bh + if self.hovered_button.is_some() { 32.0 } else { 8.0 };
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
        if let Some(action) = self.session_action(px, py, win_w) {
            self.open = None;
            self.screens_open = false;
            return Some(action);
        }
        if let Some(action) = self.tab_action(px, py, win_w) {
            self.open = None;
            self.screens_open = false;
            return Some(action);
        }
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
                    if matches!(a, Action::OpenPanel(1)) && !self.chat_ready {
                        return None;
                    }
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
            let chat_off = b.label == "Chat" && !self.chat_ready;
            let color = if end {
                0xFF_f0_8a_86
            } else if chat_off {
                0xFF_6a_73_82
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
                fill_round(
                    buf,
                    win_w,
                    win_h,
                    mx + r.x - 2.0,
                    my + 8.0 + r.y - 2.0,
                    r.w + 4.0,
                    r.h + 4.0,
                    4.0,
                    0xFF_4d_9b_ff,
                );
            }
            fill_round(
                buf,
                win_w,
                win_h,
                mx + r.x,
                my + 8.0 + r.y,
                r.w,
                r.h,
                3.0,
                if on { 0xFF_2a_3c_52 } else { 0xFF_32_3b_4a },
            );
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

/// Close-all prompt drawn over the session window when more than one computer is open.
pub fn confirm_close_boxes(win_w: usize, win_h: usize) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
    let card_w = 440.0_f32.min((win_w as f32 - 32.0).max(280.0));
    let card_h = 168.0;
    let x = ((win_w as f32 - card_w) / 2.0).max(8.0);
    let y = ((win_h as f32 - card_h) / 2.0).max(8.0);
    let btn_w = 140.0;
    let btn_h = 36.0;
    let by = y + card_h - btn_h - 20.0;
    let close = (x + 24.0, by, btn_w, btn_h);
    let cancel = (x + card_w - 24.0 - btn_w, by, btn_w, btn_h);
    (close, cancel)
}

/// `Some(true)` is Close all. `Some(false)` is Cancel.
pub fn confirm_close_hit(px: f64, py: f64, win_w: usize, win_h: usize) -> Option<bool> {
    let (close, cancel) = confirm_close_boxes(win_w, win_h);
    if Toolbar::hit(px, py, close.0, close.1, close.2, close.3) {
        return Some(true);
    }
    if Toolbar::hit(px, py, cancel.0, cancel.1, cancel.2, cancel.3) {
        return Some(false);
    }
    None
}

pub fn draw_confirm_close(buf: &mut [u32], win_w: usize, win_h: usize, sessions: usize, light: bool) {
    let c = chrome(light);
    fill_rect(buf, win_w, win_h, 0.0, 0.0, win_w as f32, win_h as f32, 0x88_10_13_18);
    let (close, cancel) = confirm_close_boxes(win_w, win_h);
    let card_w = cancel.0 + cancel.2 + 24.0 - (close.0 - 24.0);
    let card_h = 168.0;
    let x = close.0 - 24.0;
    let y = close.1 + close.3 + 20.0 - card_h;
    fill_round(buf, win_w, win_h, x, y, card_w, card_h, 12.0, c.card);
    let title = format!("Close all {sessions} sessions?");
    draw_text(buf, win_w, win_h, x + 24.0, y + 40.0, &title, c.text);
    draw_text(
        buf,
        win_w,
        win_h,
        x + 24.0,
        y + 68.0,
        "This ends every computer open in this window.",
        c.muted,
    );
    fill_round(buf, win_w, win_h, close.0, close.1, close.2, close.3, 8.0, 0xFF_8a_30_34);
    fill_round(buf, win_w, win_h, cancel.0, cancel.1, cancel.2, cancel.3, 8.0, c.elev);
    let close_label = "Close all";
    let cancel_label = "Cancel";
    draw_text(
        buf,
        win_w,
        win_h,
        close.0 + (close.2 - text_width(close_label)) / 2.0,
        close.1 + 24.0,
        close_label,
        0xFF_ff_ff_ff,
    );
    draw_text(
        buf,
        win_w,
        win_h,
        cancel.0 + (cancel.2 - text_width(cancel_label)) / 2.0,
        cancel.1 + 24.0,
        cancel_label,
        c.text,
    );
}

/// What the session window draws over a picture that is no longer live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCover {
    Reconnecting,
    Lost,
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCoverAction {
    Cancel,
    Reconnect,
    Close,
}

fn cover_buttons(kind: SessionCover) -> &'static [(&'static str, SessionCoverAction)] {
    match kind {
        SessionCover::Reconnecting => {
            &[("Reconnect now", SessionCoverAction::Reconnect), ("Close", SessionCoverAction::Close)]
        }
        SessionCover::Lost => &[("Reconnect now", SessionCoverAction::Reconnect), ("Close", SessionCoverAction::Close)],
        SessionCover::Ended => &[("Close", SessionCoverAction::Close)],
    }
}

fn cover_boxes(win_w: usize, win_h: usize, buttons: usize) -> (f32, f32, f32, f32, Vec<(f32, f32, f32, f32)>) {
    let card_w = 440.0_f32.min((win_w as f32 - 32.0).max(280.0));
    let card_h = 156.0;
    let x = ((win_w as f32 - card_w) / 2.0).max(8.0);
    let y = ((win_h as f32 - card_h) / 2.0).max(8.0);
    let btn_h = 36.0;
    let by = y + card_h - btn_h - 22.0;
    let mut boxes = Vec::new();
    if buttons <= 1 {
        let btn_w = 160.0;
        boxes.push((x + (card_w - btn_w) / 2.0, by, btn_w, btn_h));
    } else {
        let btn_w = 150.0;
        let gap = 16.0;
        let start = x + (card_w - (btn_w * 2.0 + gap)) / 2.0;
        boxes.push((start, by, btn_w, btn_h));
        boxes.push((start + btn_w + gap, by, btn_w, btn_h));
    }
    (x, y, card_w, card_h, boxes)
}

pub fn session_cover_hit(kind: SessionCover, px: f64, py: f64, win_w: usize, win_h: usize) -> Option<SessionCoverAction> {
    let buttons = cover_buttons(kind);
    let (_, _, _, _, boxes) = cover_boxes(win_w, win_h, buttons.len());
    for (button, (x, y, w, h)) in buttons.iter().zip(boxes) {
        if Toolbar::hit(px, py, x, y, w, h) {
            return Some(button.1);
        }
    }
    None
}

pub fn draw_session_cover(buf: &mut [u32], win_w: usize, win_h: usize, kind: SessionCover, detail: &str, light: bool) {
    let c = chrome(light);
    let ended = detail.lines().next().unwrap_or(detail).trim();
    let ended = if ended.is_empty() { "The session ended" } else { ended };
    let title = match kind {
        SessionCover::Reconnecting => "Connection lost, reconnecting…",
        SessionCover::Lost => "Connection lost",
        SessionCover::Ended => ended,
    };
    let buttons = cover_buttons(kind);
    fill_rect(buf, win_w, win_h, 0.0, 0.0, win_w as f32, win_h as f32, 0x88_10_13_18);
    let (x, y, card_w, card_h, boxes) = cover_boxes(win_w, win_h, buttons.len());
    fill_round(buf, win_w, win_h, x, y, card_w, card_h, 12.0, c.card);
    let shown = if title.chars().count() > 48 {
        let end = title.char_indices().nth(48).map(|(i, _)| i).unwrap_or(title.len());
        &title[..end]
    } else {
        title
    };
    draw_text(buf, win_w, win_h, x + 24.0, y + 42.0, shown, c.text);
    let subtitle = match kind {
        SessionCover::Reconnecting => {
            let line = detail.trim();
            if line.is_empty() { "Trying again now…" } else { line }
        }
        SessionCover::Lost => "Every route failed.",
        SessionCover::Ended => "",
    };
    if !subtitle.is_empty() {
        draw_text(buf, win_w, win_h, x + 24.0, y + 70.0, subtitle, c.muted);
    }
    for (button, (bx, by, bw, bh)) in buttons.iter().zip(boxes) {
        let fill = if button.1 == SessionCoverAction::Reconnect { c.accent } else { c.elev };
        let ink = if button.1 == SessionCoverAction::Reconnect { 0xFF_ff_ff_ff } else { c.text };
        fill_round(buf, win_w, win_h, bx, by, bw, bh, 8.0, fill);
        draw_text(buf, win_w, win_h, bx + (bw - text_width(button.0)) / 2.0, by + 24.0, button.0, ink);
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

fn fill_triangle(buf: &mut [u32], w: usize, h: usize, ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32, argb: u32) {
    let area = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
    if area.abs() < 0.5 {
        return;
    }
    let min_x = ax.min(bx).min(cx).max(0.0) as usize;
    let max_x = ((ax.max(bx).max(cx) as usize) + 1).min(w);
    let min_y = ay.min(by).min(cy).max(0.0) as usize;
    let max_y = ((ay.max(by).max(cy) as usize) + 1).min(h);
    for yy in min_y..max_y {
        for xx in min_x..max_x {
            let px = xx as f32 + 0.5;
            let py = yy as f32 + 0.5;
            let w0 = (bx - px) * (cy - py) - (cx - px) * (by - py);
            let w1 = (cx - px) * (ay - py) - (ax - px) * (cy - py);
            let w2 = (ax - px) * (by - py) - (bx - px) * (ay - py);
            if w0 * area >= 0.0 && w1 * area >= 0.0 && w2 * area >= 0.0 {
                blend(&mut buf[yy * w + xx], argb, 1.0);
            }
        }
    }
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
    write_png(&dir.join("tabs-2-dark.png"), 1100, 640, paint_tabs_2_dark);
    write_png(&dir.join("tabs-2-light.png"), 1100, 640, paint_tabs_2_light);
    write_png(&dir.join("tabs-3-dark.png"), 1100, 640, paint_tabs_3_dark);
    write_png(&dir.join("tabs-3-light.png"), 1100, 640, paint_tabs_3_light);
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
fn paint_tabs_2_dark(buf: &mut [u32], w: usize, h: usize) {
    paint_tabs(buf, w, h, 2, false);
}

#[cfg(feature = "ui-shots")]
fn paint_tabs_2_light(buf: &mut [u32], w: usize, h: usize) {
    paint_tabs(buf, w, h, 2, true);
}

#[cfg(feature = "ui-shots")]
fn paint_tabs_3_dark(buf: &mut [u32], w: usize, h: usize) {
    paint_tabs(buf, w, h, 3, false);
}

#[cfg(feature = "ui-shots")]
fn paint_tabs_3_light(buf: &mut [u32], w: usize, h: usize) {
    paint_tabs(buf, w, h, 3, true);
}

#[cfg(feature = "ui-shots")]
fn paint_tabs(buf: &mut [u32], w: usize, h: usize, count: usize, light: bool) {
    let page = chrome(light);
    buf.fill(page.panel);
    let wall = if light { 0xFF_d5_e3_f4 } else { 0xFF_1a_28_38 };
    fill_rect(buf, w, h, 0.0, TAB_H, w as f32, (h as f32 - TAB_H).max(0.0), wall);
    let desk = if light { 0xFF_ff_ff_ff } else { 0xFF_24_30_3c };
    fill_round(buf, w, h, 36.0, TAB_H + 28.0, w as f32 - 72.0, (h as f32 - TAB_H - 72.0).max(40.0), 10.0, desk);
    fill_rect(buf, w, h, 0.0, h as f32 - 28.0, w as f32, 28.0, if light { 0xFF_ff_ff_ff } else { 0xFF_14_18_22 });
    let mut bar = Toolbar::default();
    bar.set_light(light);
    let displays = if count >= 3 {
        pjdesk_proto::layout::sample_layout()
    } else {
        vec![
            pjdesk_proto::layout::Screen { index: 0, x: 0, y: 0, width: 1920, height: 1080, dpi: 96 },
            pjdesk_proto::layout::Screen { index: 1, x: 1920, y: 0, width: 2560, height: 1440, dpi: 96 },
        ]
    };
    let current = if count >= 3 { 1 } else { 0 };
    bar.sync(displays, current, pjdesk_proto::peer::VIEW_SINGLE);
    if let Some(slot) = bar.tab_slots(w).into_iter().rev().find(|s| matches!(s.kind, SlotKind::Display(_))) {
        bar.on_pointer((slot.x + slot.w / 2.0) as f64, (slot.y + 8.0) as f64, w);
    }
    bar.draw_tabs(buf, w, h);
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
    encoder
        .write_header()
        .and_then(|mut wr| wr.write_image_data(&rgba))
        .unwrap_or_else(|e| panic!("png {}: {e}", path.display()));
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
        assert!(
            bar.covers((bx + bw / 2.0) as f64, (by + 8.0) as f64, w)
                || bar.covers(w as f64 / 2.0, (by + 8.0) as f64, w)
        );
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

    fn two_screens() -> Vec<pjdesk_proto::layout::Screen> {
        vec![
            pjdesk_proto::layout::Screen { index: 0, x: 0, y: 0, width: 1920, height: 1080, dpi: 96 },
            pjdesk_proto::layout::Screen { index: 1, x: 1920, y: 0, width: 2560, height: 1440, dpi: 144 },
        ]
    }

    #[test]
    fn tab_strip_is_hidden_when_the_remote_has_one_display() {
        let mut bar = Toolbar::default();
        bar.sync(vec![two_screens().remove(0)], 0, 0);
        assert!(bar.tab_slots(1100).is_empty());
        assert!(!bar.covers(40.0, 12.0, 1100));
        assert_eq!(bar.click(40.0, 12.0, 1100), None);
    }

    #[test]
    fn tab_strip_switches_display_all_screens_and_separate_windows() {
        let mut bar = Toolbar::default();
        bar.sync(two_screens(), 0, 0);
        let w = 1100usize;
        let slots = bar.tab_slots(w);
        assert_eq!(slots.iter().filter(|s| matches!(s.kind, SlotKind::Display(_))).count(), 2);
        let screen2 = slots.iter().find(|s| matches!(s.kind, SlotKind::Display(1))).unwrap();
        assert_eq!(bar.click((screen2.x + 10.0) as f64, (screen2.y + 8.0) as f64, w), Some(Action::SelectDisplay(1)));
        let all = slots.iter().find(|s| matches!(s.kind, SlotKind::All)).unwrap();
        assert_eq!(bar.click((all.x + 10.0) as f64, (all.y + 8.0) as f64, w), Some(Action::ViewAll));
        let separate = slots.iter().find(|s| matches!(s.kind, SlotKind::Separate)).unwrap();
        assert_eq!(bar.click((separate.x + 8.0) as f64, (separate.y + 8.0) as f64, w), Some(Action::ViewSeparate));
    }

    #[test]
    fn chat_stays_disabled_until_the_other_computer_can_chat() {
        let mut bar = Toolbar::default();
        let w = 1100usize;
        let chat = bar.buttons.iter().find(|b| b.label == "Chat").unwrap();
        assert_eq!(chat.tip, pjdesk_proto::peer::CHAT_NEEDS_TIP);
        let (bx, by, _, _) = bar.bar_rect(w);
        let x = (bx + chat.x + 8.0) as f64;
        let y = (by + 12.0) as f64;
        assert_eq!(bar.click(x, y, w), None);
        bar.set_chat_ready(true);
        let chat = bar.buttons.iter().find(|b| b.label == "Chat").unwrap();
        assert_eq!(chat.tip, "Messages");
        assert_eq!(bar.click(x, y, w), Some(Action::OpenPanel(1)));
    }

    #[test]
    fn unread_chat_shows_on_the_session_tab() {
        let mut bar = Toolbar::default();
        bar.set_sessions(&["Cabin Desk".into(), "CHIP20".into()], 0);
        bar.set_unread(&[3, 0]);
        let slots = bar.session_slots(1100);
        assert_eq!(slots[0].badge, "3");
        assert!(slots[1].badge.is_empty());
        bar.set_unread(&[12, 1]);
        let slots = bar.session_slots(1100);
        assert_eq!(slots[0].badge, "9+");
        assert_eq!(slots[1].badge, "1");
    }

    #[test]
    fn session_tabs_name_each_computer_and_keep_screen_tabs() {
        let mut bar = Toolbar::default();
        assert_eq!(bar.tab_height(), 0.0);
        bar.set_sessions(&["Cabin Desk".into(), "CHIP20".into()], 1);
        assert_eq!(bar.tab_height(), SESSION_H);
        bar.sync(two_screens(), 0, 0);
        assert_eq!(bar.tab_height(), SESSION_H + TAB_H);
        let w = 1100usize;
        let sessions = bar.session_slots(w);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].label, "Cabin Desk");
        assert!(sessions[1].active);
        assert!(!sessions[0].active);
        let (cx, cy, cw, ch) = sessions[0].close;
        assert_eq!(
            bar.click((cx + cw / 2.0) as f64, (cy + ch / 2.0) as f64, w),
            Some(Action::CloseSession(0))
        );
        assert_eq!(
            bar.click((sessions[1].x + 12.0) as f64, (sessions[1].y + 8.0) as f64, w),
            Some(Action::SelectSession(1))
        );
        let screens = bar.tab_slots(w);
        assert!(screens.iter().any(|s| matches!(s.kind, SlotKind::Display(0))));
        assert!(screens.iter().all(|s| s.y >= SESSION_H));
        let screen = screens.iter().find(|s| matches!(s.kind, SlotKind::Display(1))).unwrap();
        assert_eq!(
            bar.click((screen.x + 10.0) as f64, (screen.y + 8.0) as f64, w),
            Some(Action::SelectDisplay(1))
        );
        let separate = screens.iter().find(|s| matches!(s.kind, SlotKind::Separate)).unwrap();
        assert_eq!(
            bar.click((separate.x + 8.0) as f64, (separate.y + 8.0) as f64, w),
            Some(Action::ViewSeparate)
        );
        let (close, cancel) = confirm_close_boxes(w, 700);
        assert!(close.0 + close.2 < cancel.0);
        assert_eq!(confirm_close_hit((close.0 + 8.0) as f64, (close.1 + 8.0) as f64, w, 700), Some(true));
        assert_eq!(confirm_close_hit((cancel.0 + 8.0) as f64, (cancel.1 + 8.0) as f64, w, 700), Some(false));
        assert_eq!(confirm_close_hit(4.0, 4.0, w, 700), None);
    }

    #[test]
    fn tab_strip_highlights_the_active_tab() {
        let mut bar = Toolbar::default();
        bar.sync(pjdesk_proto::layout::sample_layout(), 2, pjdesk_proto::peer::VIEW_SINGLE);
        let slots = bar.tab_slots(1100);
        assert_eq!(slots.iter().filter(|s| matches!(s.kind, SlotKind::Display(_))).count(), 3);
        assert!(slots.iter().any(|s| matches!(s.kind, SlotKind::Display(2)) && s.active));
        assert_eq!(slots.iter().filter(|s| s.active).count(), 1);

        bar.sync(pjdesk_proto::layout::sample_layout(), 0, pjdesk_proto::peer::VIEW_ALL);
        let slots = bar.tab_slots(1100);
        assert!(slots.iter().any(|s| matches!(s.kind, SlotKind::All) && s.active));
        assert!(slots.iter().all(|s| !matches!(s.kind, SlotKind::Display(_)) || !s.active));

        bar.sync(pjdesk_proto::layout::sample_layout(), 0, pjdesk_proto::peer::VIEW_SEPARATE);
        let slots = bar.tab_slots(1100);
        assert!(slots.iter().any(|s| matches!(s.kind, SlotKind::Separate) && s.active));
    }

    #[test]
    fn active_tab_uses_the_theme_accent() {
        let mut bar = Toolbar::default();
        bar.sync(two_screens(), 0, 0);
        let w = 1100;
        let h = 80;
        let mut buf = vec![0u32; w * h];
        bar.draw_tabs(&mut buf, w, h);
        assert!(buf.contains(&0xFF_4d_9b_ff), "dark accent underline");
        bar.set_light(true);
        buf.fill(0);
        bar.draw_tabs(&mut buf, w, h);
        assert!(buf.contains(&0xFF_2f_6f_ed), "light accent underline");
        assert!(buf.contains(&0xFF_f3_f5_f8), "light strip");
    }

    #[test]
    fn android_nav_bar_is_docked_under_the_phone() {
        let win_w = 800usize;
        let win_h = 700usize;
        let pic_w = 480usize;
        let pic_h = 500usize;
        let pic_x = 160i32;
        let pic_y = win_h as i32 - ANDROID_BAR_H as i32 - pic_h as i32;
        let bar = android_bar_box(pic_x, pic_y, pic_w, pic_h, win_w, win_h);
        assert_eq!(bar.x, pic_x as f32);
        assert_eq!(bar.w, pic_w as f32);
        assert_eq!(bar.y, (pic_y + pic_h as i32) as f32);
        assert_eq!(bar.h, ANDROID_BAR_H);
        let buttons = android_bar_buttons(bar, false);
        let back = button(&buttons, 0);
        let home = button(&buttons, 1);
        let recents = button(&buttons, 2);
        let notify = button(&buttons, 6);
        let vol_down = button(&buttons, 4);
        let vol_up = button(&buttons, 3);
        let power = button(&buttons, 5);
        assert!(back.x < home.x && home.x < recents.x);
        assert!(notify.x < back.x && vol_down.x < back.x);
        assert!(vol_up.x > recents.x && power.x > recents.x);
        assert!(back.h >= 48.0 && home.w >= 48.0 && recents.h >= 48.0);
        assert!(notify.h < back.h && power.h < home.h);
        let cluster_mid = (back.x + recents.x + recents.w) / 2.0;
        assert!((cluster_mid - (bar.x + bar.w / 2.0)).abs() < 1.0);
        assert!(!overlaps(&back, &home) && !overlaps(&home, &recents));
        assert!(!overlaps(&notify, &back) && !overlaps(&power, &recents));
        let hx = (home.x + home.w / 2.0) as f64;
        let hy = (home.y + home.h / 2.0) as f64;
        assert_eq!(
            android_bar_hit(hx, hy, pic_x, pic_y, pic_w, pic_h, win_w, win_h, false),
            Some(AndroidHit::Button(1))
        );
        assert_eq!(
            android_bar_hit((back.x + 4.0) as f64, (back.y + 4.0) as f64, pic_x, pic_y, pic_w, pic_h, win_w, win_h, false),
            Some(AndroidHit::Button(0))
        );
        assert_eq!(
            android_bar_hit(hx, (bar.y - 2.0) as f64, pic_x, pic_y, pic_w, pic_h, win_w, win_h, false),
            None
        );
        assert_eq!(
            android_bar_hit((bar.x + 1.0) as f64, (bar.y + 2.0) as f64, pic_x, pic_y, pic_w, pic_h, win_w, win_h, false),
            Some(AndroidHit::Chrome)
        );
        let narrow = android_bar_box(20, 40, 360, 400, 800, 600);
        let fitted = android_bar_buttons(narrow, false);
        assert!(fitted.iter().all(|b| b.x >= narrow.x - 0.5 && b.x + b.w <= narrow.x + narrow.w + 0.5));
        for (i, a) in fitted.iter().enumerate() {
            for b in fitted.iter().skip(i + 1) {
                assert!(!overlaps(a, b), "{:?} overlaps {:?}", a.which, b.which);
            }
        }
    }

    #[test]
    fn android_nav_bar_waits_centered_and_draws_grey_without_control() {
        let waiting = android_bar_box(0, 0, 0, 0, 1000, 700);
        assert!((320.0..=520.0).contains(&waiting.w));
        assert!((waiting.x - (1000.0 - waiting.w) / 2.0).abs() < 1.0);
        assert_eq!(waiting.y, 700.0 - ANDROID_BAR_H);
        let slim = android_bar_box(0, 0, 0, 0, 200, 400);
        assert!(slim.w <= 200.0 && slim.x >= 0.0);

        let win_w = 800;
        let win_h = 700;
        let pic_w = 480usize;
        let pic_h = 500i32;
        let pic_x = 160;
        let pic_y = win_h as i32 - ANDROID_BAR_H as i32 - pic_h;
        let mut buf = vec![0u32; win_w * win_h];
        draw_android_bar(&mut buf, win_w, win_h, pic_x, pic_y, pic_w, pic_h as usize, false);
        let home = button(&android_bar_buttons(android_bar_box(pic_x, pic_y, pic_w, pic_h as usize, win_w, win_h), false), 1);
        let px = (home.x + home.w / 2.0) as usize;
        let py = (home.y + home.h / 2.0) as usize;
        assert_eq!(buf[py * win_w + px], 0xFF_f2_f4_f8);
        buf.fill(0);
        draw_android_bar(&mut buf, win_w, win_h, pic_x, pic_y, pic_w, pic_h as usize, true);
        let home = button(&android_bar_buttons(android_bar_box(pic_x, pic_y, pic_w, pic_h as usize, win_w, win_h), true), 1);
        let px = (home.x + home.w / 2.0) as usize;
        let py = (home.y + home.h / 2.0) as usize;
        assert_eq!(buf[py * win_w + px], 0xFF_6e_76_84, "control off greys the Home button");
    }

    #[test]
    fn phone_menu_hides_the_navigation_bar() {
        let mut bar = Toolbar::default();
        assert!(bar.android_nav_on());
        assert!(!bar.shows_android_nav());
        bar.set_android(true);
        assert!(bar.shows_android_nav());
        let phone = bar.buttons.iter().find(|b| b.label == "Phone").unwrap();
        let (bx, by, _, _) = bar.bar_rect(1400);
        assert_eq!(bar.click((bx + phone.x + 10.0) as f64, (by + 12.0) as f64, 1400), None);
        let (mx, my, _, _) = bar.open_menu_rect(1400).unwrap();
        let idx = bar.menus[3].items.iter().position(|(_, action)| *action == Action::ToggleAndroidBar).unwrap();
        assert_eq!(bar.menus[3].items[idx].0, "Hide navigation bar");
        let y = my + 4.0 + idx as f32 * ITEM_H + 8.0;
        assert_eq!(bar.click((mx + 16.0) as f64, y as f64, 1400), Some(Action::ToggleAndroidBar));
        bar.toggle_android_nav();
        assert!(!bar.shows_android_nav());
        assert!(bar.menus[3].items.iter().any(|(label, action)| label == "Show navigation bar" && *action == Action::ToggleAndroidBar));
        bar.toggle_android_nav();
        assert!(bar.shows_android_nav());
    }

    #[test]
    fn reconnect_overlay_has_reconnect_now_and_close() {
        let w = 800;
        let h = 600;
        let mut buf = vec![0u32; w * h];
        draw_session_cover(&mut buf, w, h, SessionCover::Reconnecting, "Trying again in 90 s.", false);
        assert!(buf.iter().any(|px| *px != 0));
        assert_eq!(session_cover_hit(SessionCover::Reconnecting, 317.0, 338.0, w, h), Some(SessionCoverAction::Reconnect));
        assert_eq!(session_cover_hit(SessionCover::Reconnecting, 483.0, 338.0, w, h), Some(SessionCoverAction::Close));
        assert_eq!(session_cover_hit(SessionCover::Lost, 400.0, 20.0, w, h), None);
        assert_eq!(session_cover_hit(SessionCover::Lost, 317.0, 338.0, w, h), Some(SessionCoverAction::Reconnect));
        assert_eq!(session_cover_hit(SessionCover::Lost, 483.0, 338.0, w, h), Some(SessionCoverAction::Close));
        assert_eq!(session_cover_hit(SessionCover::Ended, 400.0, 338.0, w, h), Some(SessionCoverAction::Close));
    }

    fn button(buttons: &[AndroidButton], which: u8) -> AndroidButton {
        *buttons.iter().find(|b| b.which == which).unwrap()
    }

    fn overlaps(a: &AndroidButton, b: &AndroidButton) -> bool {
        a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
    }
}
