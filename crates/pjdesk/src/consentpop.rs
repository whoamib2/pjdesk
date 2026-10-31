//! The small Allow / Decline popup shown when someone asks to connect.
//!
//! It runs as its own process (`pjdesk __consent-window --dir <dir>`) so the
//! PJDesk main window stays hidden, minimized or locked while someone connects.
//! The owner writes `ask.json` and reads `answer.json` ("allow" or "decline").

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Ask {
    pub id: u64,
    pub from: String,
    pub ip: String,
}

/// The owner's handle on the popup for one request.
pub struct ConsentPop {
    dir: PathBuf,
    child: Option<Child>,
    id: u64,
}

impl ConsentPop {
    pub fn new() -> ConsentPop {
        let dir = std::env::temp_dir().join(format!("pjdesk-consent-{}", std::process::id()));
        ConsentPop { dir, child: None, id: 0 }
    }

    /// Shows the popup for this request. A new id replaces the old popup.
    pub fn show(&mut self, ask: &Ask) {
        if self.id == ask.id && self.child.is_some() {
            return;
        }
        self.close();
        self.id = ask.id;
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::remove_file(self.dir.join("answer.json"));
        if let Ok(body) = serde_json::to_vec(ask) {
            let _ = std::fs::write(self.dir.join("ask.json"), body);
        }
        if cfg!(test) {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("__consent-window").arg("--dir").arg(&self.dir);
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Ok(child) => self.child = Some(child),
            Err(e) => tracing::warn!("Could not open the connection prompt: {e}"),
        }
    }

    /// Some(true) for Allow, Some(false) for Decline, once.
    pub fn take_answer(&mut self) -> Option<bool> {
        let path = self.dir.join("answer.json");
        let text = std::fs::read_to_string(&path).ok()?;
        let _ = std::fs::remove_file(&path);
        let allow = text.trim().trim_matches('"') == "allow";
        self.close();
        Some(allow)
    }

    /// The request went away (answered elsewhere or timed out).
    pub fn close(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(self.dir.join("ask.json"));
        self.id = 0;
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for ConsentPop {
    fn drop(&mut self) {
        self.close();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn answer(dir: &Path, allow: bool) {
    let _ = std::fs::write(dir.join("answer.json"), if allow { "\"allow\"" } else { "\"decline\"" });
}

/// Bottom-right corner of the primary work area for a window of `size` points.
fn corner(size: [f32; 2]) -> Option<[f32; 2]> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
        let mut work = RECT::default();
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut work as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_ok() {
            let scale = GetDpiForSystem().max(96) as f32 / 96.0;
            let right = work.right as f32 / scale;
            let bottom = work.bottom as f32 / scale;
            return Some([right - size[0] - 12.0, bottom - size[1] - 12.0]);
        }
    }
    let _ = size;
    None
}

pub fn run_window(dir: PathBuf) -> anyhow::Result<()> {
    use eframe::egui;
    let ask: Ask = std::fs::read(dir.join("ask.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| anyhow::anyhow!("no connection request"))?;
    let size = [340.0, 118.0];
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("PJDesk: allow this connection?")
        .with_inner_size(size)
        .with_resizable(false)
        .with_decorations(false)
        .with_taskbar(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop)
        .with_app_id("pjdesk-consent");
    if let Some(pos) = corner(size) {
        viewport = viewport.with_position(pos);
    }
    let options = eframe::NativeOptions { viewport, persist_window: false, ..Default::default() };
    let started = Instant::now();
    eframe::run_native(
        "PJDesk Consent",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(Popup { dir, ask, started }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("connection prompt: {e}"))
}

struct Popup {
    dir: PathBuf,
    ask: Ask,
    started: Instant,
}

impl eframe::App for Popup {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        use eframe::egui;
        let ctx = ui.ctx().clone();
        // The owner removed the request: answered or timed out.
        if !self.dir.join("ask.json").is_file() || self.started.elapsed() > Duration::from_secs(70) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(0x1b, 0x22, 0x30)).inner_margin(egui::Margin::same(12)))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Allow this connection?").strong().size(15.0));
                let who = if self.ask.ip.is_empty() { self.ask.from.clone() } else { format!("{} ({})", self.ask.from, self.ask.ip) };
                ui.label(format!("{who} wants to control this computer."));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Decline").clicked() {
                        answer(&self.dir, false);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    let allow = egui::Button::new(egui::RichText::new("Allow").color(egui::Color32::WHITE))
                        .fill(egui::Color32::from_rgb(0x0A, 0x84, 0xFF))
                        .min_size(egui::vec2(110.0, 28.0));
                    if ui.add(allow).clicked() {
                        answer(&self.dir, true);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        ctx.request_repaint_after(Duration::from_millis(300));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn answers_are_read_once() {
        let mut pop = super::ConsentPop::new();
        pop.show(&super::Ask { id: 7, from: "OMAS-DESKTOP".into(), ip: "10.0.0.2".into() });
        assert_eq!(pop.id(), 7);
        assert_eq!(pop.take_answer(), None);
        super::answer(&pop.dir, true);
        assert_eq!(pop.take_answer(), Some(true));
        assert_eq!(pop.take_answer(), None);
    }
}
