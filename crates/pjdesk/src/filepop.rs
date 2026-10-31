//! The compact file-transfer window, one per session.
//!
//! Like the chat window it runs as its own small process
//! (`pjdesk __files-window --dir <dir>`). The viewer writes `state.json` (the
//! queue and the other side's receive folder) and touches it at least every two
//! seconds. The window appends the person's requests (send these paths,
//! cancel, list or fetch their files) to `out.json`.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub bytes: u64,
    pub incoming: bool,
    /// "offering", "offered", "sending", "downloading", "done", "cancelled" or an error sentence.
    pub state: String,
}

impl Item {
    pub fn finished(&self) -> bool {
        self.state == "done"
    }
    pub fn cancelled(&self) -> bool {
        self.state == "cancelled"
    }
    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), "offering" | "offered" | "sending" | "downloading")
    }
    pub fn failed(&self) -> bool {
        !self.active() && !self.finished() && !self.cancelled()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub peer: String,
    /// File transfer is allowed in this session.
    pub allowed: bool,
    pub note: String,
    pub items: Vec<Item>,
    /// Names in the other computer's receive folder, after List.
    pub remote: Vec<String>,
    /// Where files from the other computer are saved here.
    pub save_dir: String,
    /// Bumped when the window should come forward.
    pub raise: u64,
    #[serde(default)]
    pub beat: u64,
    #[serde(default)]
    pub close: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Request {
    Send(Vec<PathBuf>),
    Cancel(u64),
    List,
    Pull(String),
    ClearFinished,
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

/// Appends requests to `out.json`. The owner takes them all at once.
fn push_requests(dir: &Path, more: Vec<Request>) {
    if more.is_empty() {
        return;
    }
    let path = dir.join("out.json");
    let mut all: Vec<Request> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    all.extend(more);
    if let Ok(body) = serde_json::to_vec(&all) {
        write_atomic(&path, &body);
    }
}

/// The viewer's handle on one session's files window.
pub struct FilePop {
    dir: PathBuf,
    child: Option<Child>,
    state: State,
    written: Option<State>,
    last_beat: Instant,
}

impl FilePop {
    pub fn new(tag: &str) -> FilePop {
        let safe: String = tag.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
        let dir = std::env::temp_dir().join(format!("pjdesk-files-{}-{}", std::process::id(), safe));
        FilePop { dir, child: None, state: State::default(), written: None, last_beat: Instant::now() }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

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

    /// Opens the window, or brings it forward when it is already up.
    pub fn show(&mut self) {
        self.state.raise = self.state.raise.wrapping_add(1);
        self.flush(true);
        if self.is_open() || cfg!(test) {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("__files-window").arg("--dir").arg(&self.dir);
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Ok(child) => self.child = Some(child),
            Err(e) => tracing::warn!("Could not open the files window: {e}"),
        }
    }

    pub fn update(&mut self, state: State) {
        let raise = self.state.raise;
        self.state = State { raise, beat: self.state.beat, close: false, ..state };
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

    pub fn take_requests(&mut self) -> Vec<Request> {
        let path = self.dir.join("out.json");
        let Ok(bytes) = std::fs::read(&path) else { return Vec::new() };
        let _ = std::fs::remove_file(&path);
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

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

impl Drop for FilePop {
    fn drop(&mut self) {
        self.close();
    }
}

/// "1.4 MB", "820 KB".
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Bytes per second over the recent samples of one transfer.
#[derive(Default)]
pub struct Speed {
    samples: std::collections::VecDeque<(Instant, u64)>,
}

impl Speed {
    pub fn note(&mut self, at: Instant, bytes: u64) -> f64 {
        if self.samples.back().map(|s| s.1) != Some(bytes) || self.samples.is_empty() {
            self.samples.push_back((at, bytes));
        }
        while self.samples.len() > 2 && at.duration_since(self.samples[0].0) > Duration::from_secs(3) {
            self.samples.pop_front();
        }
        let (t0, b0) = self.samples[0];
        let secs = at.duration_since(t0).as_secs_f64();
        if secs < 0.3 {
            return 0.0;
        }
        bytes.saturating_sub(b0) as f64 / secs
    }
}

/// Native "Add files" / "Add folder" picker. Multi-select for files.
#[cfg(windows)]
fn pick(folders: bool) -> Vec<PathBuf> {
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH};
    let mut out = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(dialog) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return out };
        let mut opts = dialog.GetOptions().unwrap_or_default();
        opts |= FOS_ALLOWMULTISELECT;
        opts |= if folders { FOS_PICKFOLDERS } else { FOS_FILEMUSTEXIST };
        let _ = dialog.SetOptions(opts);
        let title: Vec<u16> = (if folders { "Choose folders to send" } else { "Choose files to send" }).encode_utf16().chain(std::iter::once(0)).collect();
        let _ = dialog.SetTitle(windows::core::PCWSTR(title.as_ptr()));
        if dialog.Show(None).is_err() {
            return out;
        }
        let Ok(items) = dialog.GetResults() else { return out };
        let n = items.GetCount().unwrap_or(0);
        for i in 0..n {
            let Ok(item) = items.GetItemAt(i) else { continue };
            if let Ok(name) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                if let Ok(text) = name.to_string() {
                    out.push(PathBuf::from(text));
                }
                CoTaskMemFree(Some(name.0 as *const _));
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn pick(_folders: bool) -> Vec<PathBuf> {
    Vec::new()
}

/// Paths dropped on the window through the Windows drop target.
static DROPPED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

pub fn run_window(dir: PathBuf) -> anyhow::Result<()> {
    use eframe::egui;
    let viewport = egui::ViewportBuilder::default()
        .with_title("PJDesk Files")
        .with_inner_size([380.0, 470.0])
        .with_min_inner_size([300.0, 320.0])
        .with_drag_and_drop(true)
        .with_app_id("pjdesk-files");
    let options = eframe::NativeOptions { viewport, persist_window: false, ..Default::default() };
    eframe::run_native(
        "PJDesk Files",
        options,
        Box::new(move |cc| Ok(Box::new(window::FilesApp::new(&cc.egui_ctx, Some(dir))))),
    )
    .map_err(|e| anyhow::anyhow!("files window: {e}"))
}

pub mod window {
    use super::*;
    use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};

    const ACCENT: Color32 = Color32::from_rgb(0x0A, 0x84, 0xFF);
    const BG: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1E);
    const CARD: Color32 = Color32::from_rgb(0x2C, 0x2C, 0x2E);
    const HEADER_A: Color32 = Color32::from_rgb(0x5E, 0x5C, 0xE6);
    const OK: Color32 = Color32::from_rgb(0x30, 0xD1, 0x58);
    const BAD: Color32 = Color32::from_rgb(0xFF, 0x45, 0x3A);
    const MUTED: Color32 = Color32::from_rgb(0x98, 0x98, 0x9F);

    pub struct FilesApp {
        dir: Option<PathBuf>,
        pub state: State,
        last_poll: Instant,
        last_raise: u64,
        started: Instant,
        speeds: std::collections::HashMap<u64, Speed>,
        hooked: bool,
        pub show_remote: bool,
    }

    impl FilesApp {
        pub fn new(ctx: &egui::Context, dir: Option<PathBuf>) -> FilesApp {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = BG;
            visuals.window_fill = BG;
            ctx.set_visuals(visuals);
            let mut app = FilesApp {
                dir,
                state: State { allowed: true, ..State::default() },
                last_poll: Instant::now() - Duration::from_secs(5),
                last_raise: 0,
                started: Instant::now(),
                speeds: Default::default(),
                hooked: false,
                show_remote: false,
            };
            app.poll(ctx);
            app.last_raise = app.state.raise;
            app
        }

        fn poll(&mut self, ctx: &egui::Context) {
            let Some(dir) = self.dir.clone() else { return };
            if self.last_poll.elapsed() < Duration::from_millis(150) {
                return;
            }
            self.last_poll = Instant::now();
            if let Some(state) = std::fs::read(dir.join("state.json")).ok().and_then(|b| serde_json::from_slice::<State>(&b).ok()) {
                let stale = now_ms().saturating_sub(state.beat) > 8_000;
                if state.close || (stale && self.started.elapsed() > Duration::from_secs(8)) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if state.raise != self.last_raise {
                    self.last_raise = state.raise;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                self.state = state;
            } else if self.started.elapsed() > Duration::from_secs(8) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        fn request(&self, reqs: Vec<Request>) {
            if let Some(dir) = &self.dir {
                push_requests(dir, reqs);
            }
        }

        fn send_paths(&self, paths: Vec<PathBuf>) {
            if !paths.is_empty() {
                self.request(vec![Request::Send(paths)]);
            }
        }

        pub fn draw(&mut self, ui: &mut egui::Ui) {
            let ctx = ui.ctx().clone();
            // Drops: egui's own and the Windows drop target.
            let mut dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
            dropped.extend(std::mem::take(&mut *DROPPED.lock().unwrap()));
            if !dropped.is_empty() {
                self.send_paths(dropped);
            }
            let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());

            egui::Panel::top("files-head")
                .frame(egui::Frame::NONE.fill(HEADER_A).inner_margin(Margin::symmetric(14, 10)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⇅").size(20.0).color(Color32::WHITE));
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Files").strong().size(16.0).color(Color32::WHITE));
                            let peer = if self.state.peer.is_empty() { "this session".to_string() } else { self.state.peer.clone() };
                            ui.label(RichText::new(peer).size(12.0).color(Color32::from_white_alpha(210)));
                        });
                    });
                });

            egui::Panel::bottom("files-foot")
                .frame(egui::Frame::NONE.fill(BG).inner_margin(Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Saved here to {}", short_dir(&self.state.save_dir))).size(11.0).color(MUTED));
                    });
                });

            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG).inner_margin(Margin::same(12))).show(ui, |ui| {
                if !self.state.allowed {
                    ui.label(RichText::new("File transfer is not allowed in this session.").color(BAD));
                    return;
                }
                // Drop zone with the two pickers.
                let zone_fill = if hovering { Color32::from_rgb(0x1F, 0x3A, 0x5F) } else { CARD };
                let stroke = Stroke::new(if hovering { 2.0 } else { 1.0 }, if hovering { ACCENT } else { Color32::from_gray(80) });
                egui::Frame::new().fill(zone_fill).stroke(stroke).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(14)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(if hovering { "Drop to send" } else { "Drag files or folders here" }).size(15.0).strong());
                        ui.label(RichText::new("They go to the other computer's Downloads\\PJDesk folder").size(11.0).color(MUTED));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let w = (ui.available_width() - 8.0) / 2.0;
                            let add = egui::Button::new(RichText::new("Add files…").color(Color32::WHITE)).fill(ACCENT).corner_radius(CornerRadius::same(8)).min_size(egui::vec2(w, 30.0));
                            if ui.add(add).clicked() {
                                let paths = pick(false);
                                self.send_paths(paths);
                            }
                            let folder = egui::Button::new("Add folder…").corner_radius(CornerRadius::same(8)).min_size(egui::vec2(w, 30.0));
                            if ui.add(folder).clicked() {
                                let paths = pick(true);
                                self.send_paths(paths);
                            }
                        });
                    });
                });
                if !self.state.note.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&self.state.note).size(11.0).color(Color32::from_rgb(0xFF, 0x9F, 0x0A)));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let active = self.state.items.iter().filter(|i| i.active()).count();
                    let head = if self.state.items.is_empty() { "Queue".to_string() } else if active > 0 { format!("Queue · {active} in progress") } else { "Queue".to_string() };
                    ui.label(RichText::new(head).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.state.items.iter().any(|i| !i.active()) && ui.small_button("Clear finished").clicked() {
                            self.request(vec![Request::ClearFinished]);
                        }
                    });
                });
                let now = Instant::now();
                let remote_h = if self.show_remote { 150.0 } else { 34.0 };
                egui::ScrollArea::vertical().max_height((ui.available_height() - remote_h).max(60.0)).auto_shrink([false, false]).show(ui, |ui| {
                    if self.state.items.is_empty() {
                        ui.add_space(10.0);
                        ui.label(RichText::new("Nothing sent or received yet.").color(MUTED));
                    }
                    let items = self.state.items.clone();
                    for item in items.iter().rev() {
                        let speed = self.speeds.entry(item.id).or_default().note(now, item.bytes);
                        self.row(ui, item, speed);
                        ui.add_space(6.0);
                    }
                });
                ui.add_space(4.0);
                egui::CollapsingHeader::new(RichText::new("Get files from the other computer").strong())
                    .default_open(false)
                    .show(ui, |ui| {
                        self.show_remote = true;
                        if ui.button("Show their PJDesk folder").clicked() {
                            self.request(vec![Request::List]);
                        }
                        egui::ScrollArea::vertical().id_salt("remote").max_height(100.0).show(ui, |ui| {
                            if self.state.remote.is_empty() {
                                ui.label(RichText::new("Nothing listed yet.").size(11.0).color(MUTED));
                            }
                            for name in self.state.remote.clone() {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&name).size(12.0));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.small_button("Get").clicked() {
                                            self.request(vec![Request::Pull(name.clone())]);
                                        }
                                    });
                                });
                            }
                        });
                    });
            });
        }

        fn row(&self, ui: &mut egui::Ui, item: &Item, speed: f64) {
            egui::Frame::new().fill(CARD).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (arrow, color) = if item.incoming { ("↓", OK) } else { ("↑", ACCENT) };
                    ui.label(RichText::new(arrow).size(16.0).color(color).strong());
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(RichText::new(&item.name).size(13.0).strong()).truncate());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if item.active() {
                                    if ui.small_button("✕").on_hover_text("Cancel").clicked() {
                                        self.request(vec![Request::Cancel(item.id)]);
                                    }
                                }
                            });
                        });
                        let frac = if item.size == 0 { 0.0 } else { (item.bytes as f32 / item.size as f32).clamp(0.0, 1.0) };
                        let bar_color = if item.finished() { OK } else if item.failed() || item.cancelled() { BAD } else { ACCENT };
                        ui.add(egui::ProgressBar::new(if item.finished() { 1.0 } else { frac }).desired_height(6.0).fill(bar_color).corner_radius(CornerRadius::same(3)));
                        let status = status_line(item, speed);
                        let tint = if item.finished() { OK } else if item.failed() { BAD } else { MUTED };
                        ui.label(RichText::new(status).size(11.0).color(tint));
                    });
                });
            });
        }
    }

    pub fn status_line(item: &Item, speed: f64) -> String {
        let size = human_bytes(item.size);
        match item.state.as_str() {
            "done" => format!("{} · {size}", if item.incoming { "Received" } else { "Sent" }),
            "cancelled" => "Cancelled".into(),
            "offering" | "offered" => format!("Waiting · {size}"),
            "sending" | "downloading" => {
                let mut s = format!("{} of {size}", human_bytes(item.bytes));
                if speed > 1.0 {
                    s.push_str(&format!(" · {}/s", human_bytes(speed as u64)));
                    let left = item.size.saturating_sub(item.bytes) as f64 / speed;
                    if left >= 1.0 {
                        s.push_str(&format!(" · {} left", if left < 60.0 { format!("{left:.0} s") } else { format!("{:.0} min", left / 60.0) }));
                    }
                }
                s
            }
            other => format!("Failed: {other}"),
        }
    }

    fn short_dir(dir: &str) -> String {
        if dir.is_empty() {
            return "Downloads\\PJDesk".into();
        }
        let p = std::path::Path::new(dir);
        let parts: Vec<_> = p.components().rev().take(2).collect();
        parts.iter().rev().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("\\")
    }

    impl eframe::App for FilesApp {
        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            let ctx = ui.ctx().clone();
            if !self.hooked {
                self.hooked = true;
                hook_drops(frame, &ctx);
            }
            self.poll(&ctx);
            self.show_remote = false;
            self.draw(ui);
            let busy = self.state.items.iter().any(|i| i.active());
            ctx.request_repaint_after(Duration::from_millis(if busy { 150 } else { 300 }));
        }
    }

    #[cfg(windows)]
    fn hook_drops(frame: &eframe::Frame, ctx: &egui::Context) {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = frame.window_handle() else { return };
        let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
        let ctx = ctx.clone();
        crate::viewer::attach_drop_hwnd(win.hwnd.get(), move |paths| {
            DROPPED.lock().unwrap().extend(paths);
            ctx.request_repaint();
        });
    }

    #[cfg(not(windows))]
    fn hook_drops(_frame: &eframe::Frame, _ctx: &egui::Context) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_and_window_trade_state_and_requests() {
        let mut pop = FilePop::new("test-files");
        pop.update(State { peer: "OMAS-DESKTOP".into(), allowed: true, items: vec![Item { id: 1, name: "a.txt".into(), size: 10, bytes: 5, incoming: false, state: "sending".into() }], ..State::default() });
        pop.show();
        let state: State = serde_json::from_slice(&std::fs::read(pop.dir().join("state.json")).unwrap()).unwrap();
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.raise, 1);
        push_requests(pop.dir(), vec![Request::Send(vec![PathBuf::from("C:/x/a.txt")])]);
        push_requests(pop.dir(), vec![Request::Cancel(1)]);
        assert_eq!(pop.take_requests(), vec![Request::Send(vec![PathBuf::from("C:/x/a.txt")]), Request::Cancel(1)]);
        assert!(pop.take_requests().is_empty());
        let dir = pop.dir().to_path_buf();
        pop.close();
        assert!(!dir.exists());
    }

    #[test]
    fn sizes_speeds_and_status() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(250 * 1024 * 1024), "250 MB");
        let t = Instant::now();
        let mut s = Speed::default();
        s.note(t, 0);
        let v = s.note(t + Duration::from_secs(1), 1_000_000);
        assert!((v - 1_000_000.0).abs() < 1.0);
        let item = Item { id: 1, name: "a".into(), size: 2_000_000, bytes: 1_000_000, incoming: true, state: "downloading".into() };
        let line = window::status_line(&item, v);
        assert!(line.contains("/s") && line.contains("left"), "{line}");
        let bad = Item { state: "disk full".into(), ..item };
        assert!(bad.failed());
        assert_eq!(window::status_line(&bad, 0.0), "Failed: disk full");
    }
}

#[cfg(all(test, feature = "ui-shots"))]
mod shots {
    use super::*;
    use eframe::egui;

    #[test]
    fn files_window_shot() {
        let dir = std::path::PathBuf::from(std::env::var("PJDESK_SHOT_DIR").unwrap_or_else(|_| "/workspace".into()));
        let ctx = egui::Context::default();
        let mut app = window::FilesApp::new(&ctx, None);
        let it = |id, name: &str, size, bytes, incoming, state: &str| Item { id, name: name.into(), size, bytes, incoming, state: state.into() };
        app.state = State {
            peer: "Ben (OMAS-DESKTOP)".into(),
            allowed: true,
            save_dir: "C:\\Users\\Ben\\Downloads\\PJDesk".into(),
            items: vec![
                it(1, "Quarterly report.xlsx", 2_400_000, 2_400_000, false, "done"),
                it(2, "Photos\\IMG_2041.jpg", 5_200_000, 5_200_000, false, "done"),
                it(3, "Install media.iso", 900_000_000, 312_000_000, false, "sending"),
                it(4, "notes.txt", 4_000, 0, false, "offering"),
                it(5, "driver.zip", 30_000_000, 0, true, "connection lost"),
            ],
            ..State::default()
        };
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(380.0, 520.0))
            .with_pixels_per_point(2.0)
            .wgpu()
            .build_ui_state(|ui, app: &mut window::FilesApp| app.draw(ui), app);
        harness.run_steps(4);
        let image = harness.render().expect("render");
        let path = dir.join("pjdesk-0.6.77-files-window.png");
        image.save(&path).expect("save");
        eprintln!("wrote {}", path.display());
    }
}
