//! The compact chat window used on both sides of a session.
//!
//! It runs as its own small process (`pjdesk __chat-window --dir <dir>`), so
//! the viewer's winit loop and the host's helper threads never have to share
//! an event loop with it. The owner writes `state.json` into the directory and
//! touches it at least every two seconds. The window writes the person's
//! replies to `out.json` and how many lines they have seen to `seen.json`. The
//! window closes itself when the owner stops touching the state.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use pjdesk_proto::peer::ChatEntry;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct State {
    /// The other side's name, shown in the header.
    pub peer: String,
    /// The chat is usable. When false, `note` explains why.
    pub ready: bool,
    pub note: String,
    pub lines: Vec<ChatEntry>,
    /// Bumped when the window should come forward, beep and flash.
    pub alert: u64,
    /// Milliseconds since the Unix epoch of the owner's last touch.
    #[serde(default)]
    pub beat: u64,
    /// The owner asked the window to close.
    #[serde(default)]
    pub close: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Seen {
    pub lines: usize,
    pub focused: bool,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn write_atomic(path: &Path, body: &[u8]) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::write(path, body);
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The owner's handle on one chat window.
pub struct ChatPop {
    dir: PathBuf,
    child: Option<Child>,
    state: State,
    written: Option<State>,
    last_beat: Instant,
    /// The person closed the window since the last `take_closed`.
    closed: bool,
}

impl ChatPop {
    pub fn new(tag: &str) -> ChatPop {
        let safe: String = tag.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
        let dir = std::env::temp_dir().join(format!("pjdesk-chat-{}-{}", std::process::id(), safe));
        ChatPop { dir, child: None, state: State::default(), written: None, last_beat: Instant::now(), closed: false }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The window process is still up.
    pub fn is_open(&mut self) -> bool {
        match self.child.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                self.child = None;
                self.closed = true;
                false
            }
            None => false,
        }
    }

    /// True once after the person closed the window.
    pub fn take_closed(&mut self) -> bool {
        let _ = self.is_open();
        std::mem::take(&mut self.closed)
    }

    /// Opens the window if it is not up. `alert` brings it forward with a sound.
    pub fn show(&mut self, alert: bool) {
        if alert {
            self.state.alert = self.state.alert.wrapping_add(1);
        }
        self.flush(true);
        if self.is_open() {
            return;
        }
        if cfg!(test) {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("__chat-window").arg("--dir").arg(&self.dir);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Ok(child) => self.child = Some(child),
            Err(e) => tracing::warn!("Could not open the chat window: {e}"),
        }
    }

    /// Updates what the window shows. Cheap when nothing changed.
    pub fn update(&mut self, peer: &str, ready: bool, note: &str, lines: &[ChatEntry]) {
        if self.state.peer != peer {
            self.state.peer = peer.to_string();
        }
        self.state.ready = ready;
        if self.state.note != note {
            self.state.note = note.to_string();
        }
        if self.state.lines.len() != lines.len() || self.state.lines.last() != lines.last() {
            self.state.lines = lines.to_vec();
        }
        self.flush(false);
    }

    fn flush(&mut self, force: bool) {
        let mut probe = self.state.clone();
        probe.beat = 0;
        let changed = self.written.as_ref() != Some(&probe);
        if !force && !changed && self.last_beat.elapsed() < Duration::from_secs(2) {
            return;
        }
        let _ = std::fs::create_dir_all(&self.dir);
        self.state.beat = now_ms();
        if let Ok(body) = serde_json::to_vec(&self.state) {
            write_atomic(&self.dir.join("state.json"), &body);
        }
        self.written = Some(probe);
        self.last_beat = Instant::now();
    }

    /// Replies typed in the window since the last call.
    pub fn take_replies(&mut self) -> Vec<String> {
        let path = self.dir.join("out.json");
        let Ok(bytes) = std::fs::read(&path) else { return Vec::new() };
        let _ = std::fs::remove_file(&path);
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    /// How many lines the person has seen in the focused window.
    pub fn seen(&self) -> Seen {
        std::fs::read(self.dir.join("seen.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Incoming lines the person has not looked at yet.
    pub fn unread(&self, lines: &[ChatEntry]) -> u32 {
        let seen = self.seen().lines.min(lines.len());
        lines[seen..].iter().filter(|l| !l.mine).count() as u32
    }

    /// Closes the window and forgets the files.
    pub fn close(&mut self) {
        self.state.close = true;
        self.flush(true);
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
        self.state = State::default();
        self.written = None;
    }
}

impl Drop for ChatPop {
    fn drop(&mut self) {
        self.close();
    }
}

/// Window placement, remembered across sessions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub on_top: bool,
}

impl Default for Place {
    fn default() -> Self {
        Place { x: f32::NAN, y: f32::NAN, w: 360.0, h: 460.0, on_top: true }
    }
}

fn place_path() -> PathBuf {
    crate::config::Config::path().with_file_name("chat-window.json")
}

pub fn load_place() -> Place {
    let mut p: Place = std::fs::read(place_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    p.w = p.w.clamp(280.0, 900.0);
    p.h = p.h.clamp(300.0, 1200.0);
    p
}

pub fn save_place(p: &Place) {
    if let Ok(body) = serde_json::to_vec(p) {
        if let Some(dir) = place_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        write_atomic(&place_path(), &body);
    }
}

/// "3:07 PM" in this computer's time zone.
pub fn local_clock(unix: u64) -> String {
    let (h, m) = local_hm(unix);
    let (h12, ampm) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    format!("{h12}:{m:02} {ampm}")
}

#[cfg(windows)]
fn local_hm(unix: u64) -> (u64, u64) {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = (unix + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_ok() && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok() {
            return (local.wHour as u64, local.wMinute as u64);
        }
    }
    ((unix % 86_400) / 3600, (unix % 3600) / 60)
}

#[cfg(not(windows))]
fn local_hm(unix: u64) -> (u64, u64) {
    ((unix % 86_400) / 3600, (unix % 3600) / 60)
}

/// What the window does with the keyboard: Enter sends, Shift+Enter is a new line.
pub fn enter_sends(enter: bool, shift: bool) -> bool {
    enter && !shift
}

/// Runs the window. Returns when the person closes it or the owner goes away.
pub fn run_window(dir: PathBuf) -> anyhow::Result<()> {
    use eframe::egui;
    let place = load_place();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("PJDesk Chat")
        .with_inner_size([place.w, place.h])
        .with_min_inner_size([280.0, 300.0])
        .with_app_id("pjdesk-chat");
    if place.x.is_finite() && place.y.is_finite() {
        viewport = viewport.with_position([place.x, place.y]);
    }
    if place.on_top {
        viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnTop);
    }
    let options = eframe::NativeOptions { viewport, persist_window: false, ..Default::default() };
    eframe::run_native(
        "PJDesk Chat",
        options,
        Box::new(move |cc| Ok(Box::new(window::ChatApp::new(&cc.egui_ctx, Some(dir), place)))),
    )
    .map_err(|e| anyhow::anyhow!("chat window: {e}"))
}

pub mod window {
    use super::*;
    use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};

    pub const ACCENT: Color32 = Color32::from_rgb(0x0A, 0x84, 0xFF);
    const MINE_BG: Color32 = Color32::from_rgb(0x0A, 0x84, 0xFF);
    const THEIRS_BG: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3C);
    const BG: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1E);
    const HEADER_A: Color32 = Color32::from_rgb(0x5E, 0x5C, 0xE6);
    const PEER_NAME: Color32 = Color32::from_rgb(0xFF, 0x9F, 0x0A);

    pub struct ChatApp {
        dir: Option<PathBuf>,
        pub state: State,
        pub draft: String,
        place: Place,
        last_poll: Instant,
        last_alert: u64,
        seen: usize,
        flash_until: Option<Instant>,
        stick_bottom: bool,
        started: Instant,
    }

    impl ChatApp {
        pub fn new(ctx: &egui::Context, dir: Option<PathBuf>, place: Place) -> ChatApp {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = BG;
            visuals.window_fill = BG;
            ctx.set_visuals(visuals);
            let mut app = ChatApp {
                dir,
                state: State { ready: true, ..State::default() },
                draft: String::new(),
                place,
                last_poll: Instant::now() - Duration::from_secs(5),
                last_alert: 0,
                seen: 0,
                flash_until: None,
                stick_bottom: true,
                started: Instant::now(),
            };
            app.poll(ctx);
            app.last_alert = app.state.alert;
            if app.state.lines.iter().any(|l| !l.mine) {
                app.flash_until = Some(Instant::now() + Duration::from_millis(1500));
                alert_sound();
            }
            app
        }

        fn poll(&mut self, ctx: &egui::Context) {
            let Some(dir) = self.dir.clone() else { return };
            if self.last_poll.elapsed() < Duration::from_millis(150) {
                return;
            }
            self.last_poll = Instant::now();
            match std::fs::read(dir.join("state.json")).ok().and_then(|b| serde_json::from_slice::<State>(&b).ok()) {
                Some(state) => {
                    let stale = now_ms().saturating_sub(state.beat) > 8_000;
                    if state.close || (stale && self.started.elapsed() > Duration::from_secs(8)) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if state.alert != self.last_alert {
                        self.last_alert = state.alert;
                        self.flash_until = Some(Instant::now() + Duration::from_millis(1500));
                        alert_sound();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                            egui::UserAttentionType::Informational,
                        ));
                    }
                    if state.lines.len() != self.state.lines.len() {
                        self.stick_bottom = true;
                    }
                    self.state = state;
                }
                None if self.started.elapsed() > Duration::from_secs(10) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                None => {}
            }
        }

        fn send(&mut self) {
            let Some(text) = pjdesk_proto::peer::chat_line(self.draft.trim_end_matches('\n')) else {
                self.draft.clear();
                return;
            };
            if !self.state.ready {
                return;
            }
            self.draft.clear();
            self.stick_bottom = true;
            let Some(dir) = &self.dir else {
                self.state.lines.push(ChatEntry { mine: true, name: "Me".into(), text, unix: pjdesk_proto::peer::chat_now() });
                return;
            };
            let path = dir.join("out.json");
            let mut out: Vec<String> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            out.push(text);
            if let Ok(body) = serde_json::to_vec(&out) {
                write_atomic(&path, &body);
            }
        }

        fn note_seen(&mut self, focused: bool) {
            let Some(dir) = &self.dir else { return };
            let lines = if focused { self.state.lines.len() } else { self.seen };
            if lines == self.seen && !focused {
                return;
            }
            if lines != self.seen || focused {
                self.seen = lines;
                let body = serde_json::to_vec(&Seen { lines, focused }).unwrap_or_default();
                let path = dir.join("seen.json");
                if std::fs::read(&path).ok().as_deref() != Some(body.as_slice()) {
                    write_atomic(&path, &body);
                }
            }
        }

        fn remember(&mut self, ctx: &egui::Context) {
            ctx.input(|i| {
                let vp = i.viewport();
                if let Some(r) = vp.outer_rect {
                    self.place.x = r.min.x;
                    self.place.y = r.min.y;
                }
                if let Some(r) = vp.inner_rect {
                    self.place.w = r.width();
                    self.place.h = r.height();
                }
            });
        }

        pub fn draw(&mut self, ui: &mut egui::Ui) {
            let ctx = &ui.ctx().clone();
            let flashing = self.flash_until.map(|t| Instant::now() < t).unwrap_or(false);
            egui::Panel::top("chat-head")
                .frame(egui::Frame::new().fill(if flashing { PEER_NAME } else { HEADER_A }).inner_margin(Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let dot = if self.state.ready { Color32::from_rgb(0x30, 0xD1, 0x58) } else { Color32::from_rgb(0x8E, 0x8E, 0x93) };
                        let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 5.0, dot);
                        let peer = if self.state.peer.is_empty() { "Chat".to_string() } else { self.state.peer.clone() };
                        ui.label(RichText::new(peer).strong().size(15.0).color(Color32::WHITE));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let pin = if self.place.on_top { "📌 On top" } else { "📌 Pin" };
                            let btn = egui::Button::new(RichText::new(pin).size(12.0).color(Color32::WHITE))
                                .fill(if self.place.on_top { Color32::from_white_alpha(50) } else { Color32::TRANSPARENT })
                                .corner_radius(CornerRadius::same(10));
                            if ui.add(btn).on_hover_text("Keep this chat window above other windows").clicked() {
                                self.place.on_top = !self.place.on_top;
                                let level = if self.place.on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
                                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
                                save_place(&self.place);
                            }
                        });
                    });
                });

            egui::Panel::bottom("chat-input")
                .frame(egui::Frame::new().fill(Color32::from_rgb(0x2C, 0x2C, 0x2E)).inner_margin(Margin::same(8)))
                .show(ui, |ui| {
                    if !self.state.ready && !self.state.note.is_empty() {
                        ui.label(RichText::new(&self.state.note).size(12.0).color(Color32::from_rgb(0xF2, 0xB0, 0x3A)));
                    }
                    ui.horizontal(|ui| {
                        let send_w = 64.0;
                        let id = egui::Id::new("chat-draft");
                        let had_focus = ctx.memory(|m| m.has_focus(id));
                        // Enter sends; Shift+Enter falls through to the editor as a new line.
                        let enter = had_focus && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                        let edit = egui::TextEdit::multiline(&mut self.draft)
                            .id(id)
                            .hint_text("Message  (Enter to send, Shift+Enter for a new line)")
                            .desired_rows(2)
                            .desired_width(ui.available_width() - send_w - 8.0)
                            .char_limit(pjdesk_proto::peer::MAX_CHAT_TEXT);
                        let resp = ui.add_enabled(self.state.ready, edit);
                        if self.started.elapsed() < Duration::from_millis(400) {
                            resp.request_focus();
                        }
                        let mut go = super::enter_sends(enter, false);
                        let send = egui::Button::new(RichText::new("Send").strong().color(Color32::WHITE))
                            .fill(ACCENT)
                            .corner_radius(CornerRadius::same(16))
                            .min_size(egui::vec2(send_w, 34.0));
                        if ui.add_enabled(self.state.ready && !self.draft.trim().is_empty(), send).clicked() {
                            go = true;
                        }
                        if go {
                            self.send();
                            resp.request_focus();
                        }
                    });
                });

            egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(8, 6))).show(ui, |ui| {
                let stick = std::mem::take(&mut self.stick_bottom);
                egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    if self.state.lines.is_empty() {
                        ui.add_space(24.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("No messages yet. Say hello!").color(Color32::from_gray(150)));
                        });
                    }
                    let max_w = (ui.available_width() * 0.78).max(140.0);
                    let mut prev: Option<(bool, String)> = None;
                    for line in &self.state.lines {
                        let same = prev.as_ref().map(|(m, n)| *m == line.mine && *n == line.name).unwrap_or(false);
                        prev = Some((line.mine, line.name.clone()));
                        if !same {
                            ui.add_space(6.0);
                            let layout = if line.mine { egui::Layout::right_to_left(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Min) };
                            ui.with_layout(layout, |ui| {
                                let who = if line.mine { "You".to_string() } else { line.name.clone() };
                                ui.label(RichText::new(who).size(11.0).strong().color(if line.mine { Color32::from_rgb(0x64, 0xD2, 0xFF) } else { PEER_NAME }));
                                ui.label(RichText::new(local_clock(line.unix)).size(10.5).color(Color32::from_gray(140)));
                            });
                        }
                        ui.add_space(2.0);
                        bubble(ui, line, max_w);
                    }
                    ui.add_space(4.0);
                    if stick {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                });
            });
        }
    }

    fn bubble(ui: &mut egui::Ui, line: &ChatEntry, max_w: f32) {
        let (fill, fg) = if line.mine { (MINE_BG, Color32::WHITE) } else { (THEIRS_BG, Color32::from_rgb(0xF2, 0xF2, 0xF7)) };
        let layout = if line.mine { egui::Layout::right_to_left(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Min) };
        ui.with_layout(layout, |ui| {
            let radius = if line.mine {
                CornerRadius { nw: 16, ne: 16, sw: 16, se: 4 }
            } else {
                CornerRadius { nw: 16, ne: 16, sw: 4, se: 16 }
            };
            egui::Frame::new().fill(fill).corner_radius(radius).inner_margin(Margin::symmetric(12, 7)).stroke(Stroke::NONE).show(ui, |ui| {
                ui.set_max_width(max_w);
                ui.add(egui::Label::new(RichText::new(&line.text).size(14.0).color(fg)).wrap());
            });
        });
    }

    impl eframe::App for ChatApp {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            let ctx = &ui.ctx().clone();
            self.poll(ctx);
            let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
            self.note_seen(focused);
            self.remember(ctx);
            self.draw(ui);
            if ctx.input(|i| i.viewport().close_requested()) {
                save_place(&self.place);
            }
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
            save_place(&self.place);
        }
    }

    fn alert_sound() {
        #[cfg(windows)]
        unsafe {
            let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_sends_and_shift_enter_does_not() {
        assert!(enter_sends(true, false));
        assert!(!enter_sends(true, true));
        assert!(!enter_sends(false, false));
    }

    #[test]
    fn the_owner_and_the_window_trade_files() {
        let mut pop = ChatPop::new("test-trade");
        let lines = vec![
            ChatEntry { mine: false, name: "Ben".into(), text: "hi".into(), unix: 1_700_000_000 },
            ChatEntry { mine: true, name: "Me".into(), text: "hello".into(), unix: 1_700_000_001 },
            ChatEntry { mine: false, name: "Ben".into(), text: "ok".into(), unix: 1_700_000_002 },
        ];
        pop.update("Ben", true, "", &lines);
        pop.show(true);
        let state: State = serde_json::from_slice(&std::fs::read(pop.dir().join("state.json")).unwrap()).unwrap();
        assert_eq!(state.lines.len(), 3);
        assert_eq!(state.alert, 1);
        assert_eq!(pop.unread(&lines), 2);
        std::fs::write(pop.dir().join("seen.json"), serde_json::to_vec(&Seen { lines: 2, focused: true }).unwrap()).unwrap();
        assert_eq!(pop.unread(&lines), 1);
        std::fs::write(pop.dir().join("out.json"), b"[\"on my way\"]").unwrap();
        assert_eq!(pop.take_replies(), vec!["on my way".to_string()]);
        assert!(pop.take_replies().is_empty());
        let dir = pop.dir().to_path_buf();
        pop.close();
        assert!(!dir.exists());
    }

    #[test]
    fn the_clock_reads_hours_and_minutes() {
        let s = local_clock(1_700_000_000);
        assert!(s.ends_with("AM") || s.ends_with("PM"));
        assert!(s.contains(':'));
    }
}

#[cfg(all(test, feature = "ui-shots"))]
mod shots {
    use super::*;
    use eframe::egui;

    #[test]
    fn chat_window_shot() {
        let dir = std::path::PathBuf::from(std::env::var("PJDESK_SHOT_DIR").unwrap_or_else(|_| "/workspace".into()));
        let ctx = egui::Context::default();
        let mut app = window::ChatApp::new(&ctx, None, Place::default());
        app.state.peer = "Ben (OFFICE-PC)".into();
        let t = 1_793_460_000;
        let line = |mine: bool, name: &str, text: &str, d: u64| ChatEntry { mine, name: name.into(), text: text.into(), unix: t + d };
        app.state.lines = vec![
            line(false, "Ben", "Hi! I'm connected now. Can you see my screen?", 0),
            line(true, "Support", "Yes, I can see it. Give me a second to open Settings.", 25),
            line(true, "Support", "Okay, found the printer page.", 31),
            line(false, "Ben", "Great, thanks.\nThe printer is the HP in the hallway.", 70),
            line(true, "Support", "Got it. Installing the driver now — should take a minute.", 95),
        ];
        app.draft = "Almost done".into();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(360.0, 460.0))
            .with_pixels_per_point(2.0)
            .wgpu()
            .build_ui_state(|ui, app: &mut window::ChatApp| app.draw(ui), app);
        harness.run_steps(4);
        let image = harness.render().expect("render");
        let path = dir.join("pjdesk-0.6.76-chat-window.png");
        image.save(&path).expect("save");
        eprintln!("wrote {}", path.display());
    }
}
