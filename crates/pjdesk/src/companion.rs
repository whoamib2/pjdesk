//! The small session companion on the controlled computer.
//!
//! `pjdesk __session-companion --dir <dir>` starts minimized on the taskbar
//! when someone connects and closes on its own when the session ends. It has
//! two buttons for the person at this computer: Chat and Send files (to the
//! person who is connected). The host writes `state.json` (the peer, a beat,
//! and `close`); the window appends "chat" / "files" to `out.json`.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::filepop::{now_ms, write_atomic};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub peer: String,
    pub chat: bool,
    pub files: bool,
    #[serde(default)]
    pub beat: u64,
    #[serde(default)]
    pub close: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Ask {
    Chat,
    Files,
}

fn push(dir: &Path, ask: Ask) {
    let path = dir.join("out.json");
    let mut all: Vec<Ask> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    all.push(ask);
    if let Ok(body) = serde_json::to_vec(&all) {
        write_atomic(&path, &body);
    }
}

/// The host's handle on the companion process.
pub struct Companion {
    dir: PathBuf,
    child: Option<Child>,
    state: State,
    last_beat: Instant,
}

impl Companion {
    pub fn new(tag: &str) -> Companion {
        let safe: String = tag.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
        let dir = std::env::temp_dir().join(format!("pjdesk-companion-{}-{}", std::process::id(), safe));
        Companion { dir, child: None, state: State::default(), last_beat: Instant::now() - Duration::from_secs(10) }
    }

    #[allow(dead_code)]
    pub fn is_open(&mut self) -> bool {
        match self.child.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                self.child = None;
                false
            }
            None => false,
        }
    }

    /// Starts the window (minimized) once per session and keeps its state fresh.
    pub fn keep(&mut self, state: State, start: bool) {
        let changed = State { beat: 0, ..state.clone() } != State { beat: 0, ..self.state.clone() };
        self.state = state;
        if changed || self.last_beat.elapsed() > Duration::from_secs(2) {
            let _ = std::fs::create_dir_all(&self.dir);
            self.state.beat = now_ms();
            if let Ok(body) = serde_json::to_vec(&self.state) {
                write_atomic(&self.dir.join("state.json"), &body);
            }
            self.last_beat = Instant::now();
        }
        if start && self.child.is_none() && !cfg!(test) {
            let Ok(exe) = std::env::current_exe() else { return };
            let mut cmd = std::process::Command::new(exe);
            cmd.arg("__session-companion").arg("--dir").arg(&self.dir);
            cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            match cmd.spawn() {
                Ok(child) => self.child = Some(child),
                Err(e) => tracing::warn!("Could not open the session window: {e}"),
            }
        }
    }

    pub fn take(&mut self) -> Vec<Ask> {
        let path = self.dir.join("out.json");
        let Ok(bytes) = std::fs::read(&path) else { return Vec::new() };
        let _ = std::fs::remove_file(&path);
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub fn close(&mut self) {
        self.state.close = true;
        self.state.beat = now_ms();
        if let Ok(body) = serde_json::to_vec(&self.state) {
            write_atomic(&self.dir.join("state.json"), &body);
        }
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
    }
}

impl Drop for Companion {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn run_window(dir: PathBuf) -> anyhow::Result<()> {
    use eframe::egui;
    let viewport = egui::ViewportBuilder::default()
        .with_title("PJDesk session")
        .with_inner_size([300.0, 112.0])
        .with_resizable(false)
        .with_active(false)
        .with_app_id("pjdesk-session");
    let options = eframe::NativeOptions { viewport, persist_window: false, ..Default::default() };
    eframe::run_native(
        "PJDesk session",
        options,
        Box::new(move |cc| Ok(Box::new(window::CompanionApp::new(&cc.egui_ctx, dir)))),
    )
    .map_err(|e| anyhow::anyhow!("session window: {e}"))
}

pub mod window {
    use super::*;
    use eframe::egui::{self, Color32, CornerRadius, Margin, RichText};

    const BG: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1E);
    const HEADER_A: Color32 = Color32::from_rgb(0x5E, 0x5C, 0xE6);
    const MUTED: Color32 = Color32::from_rgb(0x98, 0x98, 0x9F);

    pub struct CompanionApp {
        dir: PathBuf,
        pub state: State,
        last_poll: Instant,
        started: Instant,
        title: String,
        minimized: bool,
    }

    impl CompanionApp {
        pub fn new(ctx: &egui::Context, dir: PathBuf) -> CompanionApp {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = BG;
            visuals.window_fill = BG;
            ctx.set_visuals(visuals);
            CompanionApp {
                dir,
                state: State::default(),
                last_poll: Instant::now() - Duration::from_secs(1),
                started: Instant::now(),
                minimized: false,
                title: String::new(),
            }
        }

        fn poll(&mut self, ctx: &egui::Context) {
            if self.last_poll.elapsed() < Duration::from_millis(250) {
                return;
            }
            self.last_poll = Instant::now();
            match std::fs::read(self.dir.join("state.json")).ok().and_then(|b| serde_json::from_slice::<State>(&b).ok()) {
                Some(state) => {
                    let stale = now_ms().saturating_sub(state.beat) > 8_000;
                    if state.close || stale {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    self.state = state;
                }
                None if self.started.elapsed() > Duration::from_secs(8) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                None => {}
            }
            let title = title_for(&self.state.peer);
            if title != self.title {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
                self.title = title;
            }
        }

        pub fn draw(&mut self, ui: &mut egui::Ui) {
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG).inner_margin(Margin::same(12))).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("●").color(HEADER_A));
                    let who = if self.state.peer.is_empty() { "Someone".to_string() } else { self.state.peer.clone() };
                    ui.label(RichText::new(format!("{who} is connected")).strong());
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let size = egui::vec2(132.0, 32.0);
                    let chat = egui::Button::new(RichText::new("Chat").color(Color32::WHITE))
                        .fill(HEADER_A)
                        .corner_radius(CornerRadius::same(8))
                        .min_size(size);
                    if ui.add_enabled(self.state.chat, chat).clicked() {
                        push(&self.dir, Ask::Chat);
                    }
                    let files = egui::Button::new(RichText::new("Send files").color(Color32::WHITE))
                        .fill(HEADER_A)
                        .corner_radius(CornerRadius::same(8))
                        .min_size(size);
                    if ui.add_enabled(self.state.files, files).clicked() {
                        push(&self.dir, Ask::Files);
                    }
                });
                if !self.state.files {
                    ui.label(RichText::new("File transfer is off for this session.").size(11.0).color(MUTED));
                }
            });
        }
    }

    impl eframe::App for CompanionApp {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            let ctx = ui.ctx().clone();
            if !self.minimized {
                // Start on the taskbar, out of the way of the person being helped.
                self.minimized = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            self.poll(&ctx);
            self.draw(ui);
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    pub fn title_for(peer: &str) -> String {
        if peer.is_empty() {
            "PJDesk session".into()
        } else {
            format!("PJDesk – {peer} is connected")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_reach_the_host_and_close_ends_the_window() {
        let mut c = Companion::new("test-companion");
        c.keep(State { peer: "BENLENOVO".into(), chat: true, files: true, ..Default::default() }, true);
        push(&c.dir, Ask::Chat);
        push(&c.dir, Ask::Files);
        assert_eq!(c.take(), vec![Ask::Chat, Ask::Files]);
        assert!(c.take().is_empty());
        let state: State = serde_json::from_slice(&std::fs::read(c.dir.join("state.json")).unwrap()).unwrap();
        assert!(!state.close);
        assert_eq!(window::title_for("BENLENOVO"), "PJDesk – BENLENOVO is connected");
        let dir = c.dir.clone();
        c.close();
        assert!(!dir.exists());
    }
}
