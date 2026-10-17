//! The PJDesk Server management window (what you get by double-clicking
//! `pjdesk-server.exe`): service control, the server key fingerprint, user
//! accounts and registered devices, and a tail of the log.
//!
//! Account and service changes need Administrator rights (the data folder is
//! locked to SYSTEM and Administrators), so when the window can't write there
//! it offers to relaunch elevated.

use std::path::PathBuf;

use anyhow::Result;
use eframe::egui::{self, Color32, RichText};

use std::collections::HashMap;

use crate::accounts::{self, Accounts};
use crate::registry::{DeviceRow, Registry};

const ACCENT: Color32 = Color32::from_rgb(0x4d, 0x9b, 0xff);
const OK: Color32 = Color32::from_rgb(0x3f, 0xcf, 0x6b);
const WARN: Color32 = Color32::from_rgb(0xf2, 0xb0, 0x3a);
const MUTED: Color32 = Color32::from_rgb(0x9a, 0xa2, 0xb1);
const CARD: Color32 = Color32::from_rgb(0x1c, 0x20, 0x28);

pub fn run(data_dir: PathBuf, port: u16) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 720.0])
            .with_min_inner_size([620.0, 520.0])
            .with_title("PJDesk Server")
            .with_app_id("pjdesk-server")
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: include_bytes!("../assets/icon_server_256.rgba").to_vec(),
                width: 256,
                height: 256,
            })),
        renderer: match std::env::var("PJDESK_RENDERER").as_deref() {
            Ok("glow") => eframe::Renderer::Glow,
            _ => eframe::Renderer::Wgpu,
        },
        ..Default::default()
    };
    eframe::run_native("PJDesk Server", options, Box::new(move |cc| Ok(Box::new(App::new(cc, data_dir, port)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

struct App {
    data_dir: PathBuf,
    port: u16,
    fingerprint: String,
    can_write: bool,
    accounts: Vec<accounts::AccountInfo>,
    devices: Vec<DeviceRow>,
    /// Live online devices (id → reported version), from the server's online.json.
    online: HashMap<String, String>,
    log_tail: String,
    status: String,

    new_user: String,
    new_pass: String,
    new_admin: bool,
    last_refresh: std::time::Instant,
    tray: Option<crate::tray::Tray>,
    really_quit: bool,
    told_tray: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, data_dir: PathBuf, port: u16) -> Self {
        apply_theme(&cc.egui_ctx);
        let mut app = Self {
            data_dir,
            port,
            fingerprint: String::new(),
            can_write: false,
            accounts: Vec::new(),
            devices: Vec::new(),
            online: HashMap::new(),
            log_tail: String::new(),
            status: String::new(),
            new_user: String::new(),
            new_pass: String::new(),
            new_admin: false,
            last_refresh: std::time::Instant::now(),
            tray: crate::tray::Tray::new(&cc.egui_ctx),
            really_quit: false,
            told_tray: false,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        // Writable if we can create/touch the data dir.
        self.can_write = std::fs::create_dir_all(&self.data_dir).is_ok()
            && std::fs::write(self.data_dir.join(".wtest"), b"1").is_ok();
        let _ = std::fs::remove_file(self.data_dir.join(".wtest"));

        // Display the fingerprint read-only. This NEVER creates, regenerates or
        // re-permissions anything — doing that on every refresh rotated the key
        // and flashed console windows. The server's run path owns the identity;
        // the window just shows it.
        self.fingerprint = match crate::server::read_identity(&self.data_dir) {
            Some(id) => pjdesk_proto::tls::format_fingerprint(&id.fingerprint()),
            None => "(unknown — start the server to create it)".into(),
        };
        self.accounts = Accounts::load(&self.data_dir).map(|a| a.list()).unwrap_or_default();
        self.devices = Registry::load(self.data_dir.join("devices.json")).map(|r| r.list()).unwrap_or_default();
        self.online = read_online(&self.data_dir);
        self.log_tail = tail(&self.data_dir.join("server.log"), 40);
    }

    fn accounts_mut(&mut self) -> Option<Accounts> {
        match Accounts::load(&self.data_dir) {
            Ok(a) => Some(a),
            Err(e) => {
                self.status = format!("Can't open accounts: {e}");
                None
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.last_refresh.elapsed() > std::time::Duration::from_secs(3) {
            self.last_refresh = std::time::Instant::now();
            self.refresh();
        }
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(std::time::Duration::from_secs(1));

        // System tray: Show/Quit, and hide-to-tray when the window is closed.
        let tray_actions = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        let has_tray = self.tray.is_some();
        for action in tray_actions {
            match action {
                crate::tray::Action::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                crate::tray::Action::Quit => {
                    self.really_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if has_tray && !self.really_quit && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            if !self.told_tray {
                self.told_tray = true;
                self.status = "PJDesk Server is still running in the tray — right-click it to Quit.".into();
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("PJDesk Server").size(20.0).strong().color(Color32::WHITE));
                    ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).color(MUTED));
                });
                ui.add_space(8.0);

                if !self.can_write {
                    card(ui, |ui| {
                        ui.colored_label(WARN, "⚠ Running without Administrator rights — accounts and the service can't be changed.");
                        if ui.add(egui::Button::new("Restart as administrator").fill(ACCENT)).clicked() {
                            if run_elevated(&[]) {
                                std::process::exit(0);
                            }
                        }
                    });
                    ui.add_space(8.0);
                }

                self.service_card(ui);
                ui.add_space(8.0);
                self.accounts_card(ui);
                ui.add_space(8.0);
                self.devices_card(ui);
                ui.add_space(8.0);
                self.log_card(ui);

                if !self.status.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(WARN, "•");
                        ui.label(&self.status);
                        if ui.small_button("✕").clicked() {
                            self.status.clear();
                        }
                    });
                }
            });
        });
    }
}

#[derive(Clone, Copy)]
enum SvcAction {
    Install,
    Uninstall,
    Start,
    Stop,
}

impl App {
    fn service_card(&mut self, ui: &mut egui::Ui) {
        let mut action: Option<SvcAction> = None;
        card(ui, |ui| {
            heading(ui, "SERVICE");
            let (installed, running) = svc_status();
            ui.horizontal(|ui| {
                let (c, t) = if !installed {
                    (MUTED, "Not installed")
                } else if running {
                    (OK, "Installed and running")
                } else {
                    (WARN, "Installed, not running")
                };
                ui.colored_label(c, "●");
                ui.label(t);
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if !installed {
                    if ui.add(egui::Button::new("Install service").fill(ACCENT)).clicked() {
                        action = Some(SvcAction::Install);
                    }
                } else {
                    if ui.button("Uninstall").clicked() {
                        action = Some(SvcAction::Uninstall);
                    }
                    if running {
                        if ui.button("Stop").clicked() {
                            action = Some(SvcAction::Stop);
                        }
                    } else if ui.button("Start").clicked() {
                        action = Some(SvcAction::Start);
                    }
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new(format!("Listening port: {} (TCP + UDP)", self.port)).color(MUTED).size(12.0));
            ui.label(RichText::new(format!("Data folder: {}", self.data_dir.display())).color(MUTED).size(12.0));
            ui.add_space(4.0);
            ui.label(RichText::new("Server key fingerprint").color(MUTED).size(12.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.fingerprint).monospace().size(11.0));
                if ui.small_button("⧉").clicked() {
                    copy(&self.fingerprint);
                    self.status = "Fingerprint copied".into();
                }
            });
            ui.label(RichText::new("Clients pin this on first connect; compare it when setting up a new computer.").color(MUTED).size(11.0));
        });

        if let Some(action) = action {
            self.run_service_action(action);
        }
    }

    /// Runs a service action in-process (the server always runs elevated).
    fn run_service_action(&mut self, action: SvcAction) {
        let result = match action {
            SvcAction::Install => {
                let opts = crate::ServerOpts {
                    bind: std::net::IpAddr::from([0, 0, 0, 0]),
                    port: self.port,
                    data_dir: Some(self.data_dir.clone()),
                };
                crate::service::install(&opts, &self.data_dir, true).map(|_| "Service installed and started.")
            }
            SvcAction::Uninstall => crate::service::uninstall().map(|_| "Service removed."),
            SvcAction::Start => crate::service::start().map(|_| "Service started."),
            SvcAction::Stop => crate::service::stop().map(|_| "Service stopped."),
        };
        self.status = match result {
            Ok(msg) => msg.to_string(),
            Err(e) => format!("{e:#}"),
        };
        self.refresh();
    }

    fn accounts_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            heading(ui, "USER ACCOUNTS");
            ui.label(RichText::new("Accounts let people sign in to sync their address book and saved logins.").color(MUTED).size(12.0));
            ui.add_space(6.0);

            if self.accounts.is_empty() {
                ui.label(RichText::new("No accounts yet.").color(MUTED));
            }
            let mut remove: Option<String> = None;
            let mut toggle_disabled: Option<(String, bool)> = None;
            for a in &self.accounts {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&a.username).strong().color(Color32::WHITE));
                    if a.is_admin {
                        ui.label(RichText::new("admin").color(ACCENT).size(11.0));
                    }
                    if a.disabled {
                        ui.label(RichText::new("disabled").color(WARN).size(11.0));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(self.can_write, |ui| {
                            if ui.small_button("Remove").clicked() {
                                remove = Some(a.username.clone());
                            }
                            if ui.small_button(if a.disabled { "Enable" } else { "Disable" }).clicked() {
                                toggle_disabled = Some((a.username.clone(), !a.disabled));
                            }
                        });
                    });
                });
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(RichText::new("Add an account").color(MUTED).size(12.0));
            ui.add_enabled_ui(self.can_write, |ui| {
                egui::Grid::new("newacct").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Username");
                    ui.text_edit_singleline(&mut self.new_user);
                    ui.end_row();
                    ui.label("Password");
                    ui.add(egui::TextEdit::singleline(&mut self.new_pass).password(true));
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.new_admin, "Administrator");
                    if ui.add(egui::Button::new("Add").fill(ACCENT)).clicked() {
                        let (u, p, admin) = (self.new_user.trim().to_string(), self.new_pass.clone(), self.new_admin);
                        if let Some(mut accts) = self.accounts_mut() {
                            match accts.create(&u, &p, admin, false) {
                                Ok(()) => {
                                    self.status = format!("Added account \"{}\"", accounts::normalize_username(&u));
                                    self.new_user.clear();
                                    self.new_pass.clear();
                                    self.new_admin = false;
                                    self.refresh();
                                }
                                Err(e) => self.status = format!("{e}"),
                            }
                        }
                    }
                });
            });

            if let Some(u) = remove {
                if let Some(mut accts) = self.accounts_mut() {
                    let _ = accts.delete(&u);
                    self.refresh();
                }
            }
            if let Some((u, disabled)) = toggle_disabled {
                if let Some(mut accts) = self.accounts_mut() {
                    let _ = accts.set_disabled(&u, disabled);
                    self.refresh();
                }
            }
        });
    }

    fn devices_card(&mut self, ui: &mut egui::Ui) {
        let mut remove: Option<String> = None;
        let mut update: Option<String> = None;
        let mut update_all = false;
        card(ui, |ui| {
            let online = self.online.len();
            ui.horizontal(|ui| {
                heading(ui, &format!("REGISTERED COMPUTERS ({}, {online} online)", self.devices.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if online > 0
                        && ui.small_button("Update all online").on_hover_text("Tell every online computer to update now").clicked()
                    {
                        update_all = true;
                    }
                });
            });
            if self.devices.is_empty() {
                ui.label(RichText::new("No computers have registered yet.").color(MUTED));
            }
            egui::ScrollArea::vertical().max_height(200.0).id_salt("devices").show(ui, |ui| {
                for d in &self.devices {
                    let live = self.online.get(&d.id);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(pjdesk_proto::format_id(&d.id)).monospace().color(Color32::WHITE));
                        ui.label(RichText::new(&d.hostname).color(MUTED));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_enabled_ui(self.can_write, |ui| {
                                if ui.small_button("Forget").clicked() {
                                    remove = Some(d.id.clone());
                                }
                            });
                            if live.is_some() && ui.small_button("Update").on_hover_text("Update this computer now").clicked() {
                                update = Some(d.id.clone());
                            }
                            let version = live.cloned().unwrap_or_else(|| d.version.clone());
                            let vtext = if version.is_empty() { "unknown".into() } else { format!("v{version}") };
                            if live.is_some() {
                                ui.label(RichText::new(format!("● online · {vtext}")).color(OK).size(11.0));
                            } else {
                                ui.label(RichText::new(format!("{vtext} · {}", ago(d.last_seen))).color(MUTED).size(11.0));
                            }
                        });
                    });
                }
            });
        });
        if let Some(id) = remove {
            if let Ok(mut reg) = Registry::load(self.data_dir.join("devices.json")) {
                let _ = reg.remove(&id);
                self.refresh();
            }
        }
        if update_all {
            self.request_update("*");
        }
        if let Some(id) = update {
            self.request_update(&id);
        }
    }

    /// Asks the running server to update one device (`"*"` = every online one)
    /// by leaving a request file it picks up. The data folder is admin-only, so
    /// this relaunches elevated when the window itself can't write there.
    fn request_update(&mut self, target: &str) {
        if self.can_write {
            match write_update_request(&self.data_dir, target) {
                Ok(()) => {
                    self.status = if target == "*" {
                        "Asked all online computers to update.".into()
                    } else {
                        "Asked that computer to update.".into()
                    }
                }
                Err(e) => self.status = format!("Could not write the update request: {e}"),
            }
        } else if run_elevated(&["request-update", target]) {
            self.status = "Sent the update request.".into();
        }
    }

    fn log_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            heading(ui, "LOG");
            if self.log_tail.is_empty() {
                ui.label(RichText::new("No log yet (the server writes server.log in its data folder when it runs).").color(MUTED).size(12.0));
            } else {
                egui::ScrollArea::vertical().max_height(160.0).id_salt("log").stick_to_bottom(true).show(ui, |ui| {
                    ui.label(RichText::new(&self.log_tail).monospace().size(11.0).color(MUTED));
                });
            }
        });
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(MUTED).size(12.0).strong());
    ui.add_space(2.0);
}

/// Reads the server's live online snapshot (id → reported version). Ignores a
/// stale file (older than ~15 s), which means the server isn't running.
fn read_online(data_dir: &std::path::Path) -> HashMap<String, String> {
    #[derive(serde::Deserialize)]
    struct OnlineInfo {
        id: String,
        #[serde(default)]
        version: String,
    }
    let path = data_dir.join("online.json");
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|e| e.as_secs() <= 15).unwrap_or(false))
        .unwrap_or(false);
    if !fresh {
        return HashMap::new();
    }
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<OnlineInfo>>(&b).ok())
        .map(|v| v.into_iter().map(|o| (o.id, o.version)).collect())
        .unwrap_or_default()
}

/// Leaves an update request the running server picks up (`"*"` = all online).
fn write_update_request(data_dir: &std::path::Path, target: &str) -> std::io::Result<()> {
    let targets = vec![target.to_string()];
    let bytes = serde_json::to_vec(&targets).unwrap_or_else(|_| b"[]".to_vec());
    std::fs::write(data_dir.join("update-requests.json"), bytes)
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(CARD)
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0x2a, 0x30, 0x3b)))
        .show(ui, add)
        .inner
}

fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(0x14, 0x17, 0x1d);
    v.window_fill = CARD;
    v.hyperlink_color = ACCENT;
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x26, 0x2c, 0x37);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x30, 0x38, 0x46);
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals(v);
}

fn copy(text: &str) {
    if let Ok(mut cb) = arboard::Clipboard::new() {
        let _ = cb.set_text(text.to_string());
    }
}

fn tail(path: &std::path::Path, lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else { return String::new() };
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

fn ago(unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let d = now.saturating_sub(unix);
    if d < 60 {
        "just now".into()
    } else if d < 3600 {
        format!("{} min ago", d / 60)
    } else if d < 86400 {
        format!("{} h ago", d / 3600)
    } else {
        format!("{} d ago", d / 86400)
    }
}

/// Relaunches this program elevated with the given arguments.
#[cfg(windows)]
fn run_elevated(args: &[&str]) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let Ok(exe) = std::env::current_exe() else { return false };
    let params = args.join(" ");
    unsafe {
        let h = ShellExecuteW(
            None,
            &HSTRING::from("runas"),
            &HSTRING::from(exe.as_os_str()),
            &HSTRING::from(params),
            windows::core::PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        h.0 as isize > 32
    }
}

#[cfg(not(windows))]
fn run_elevated(_args: &[&str]) -> bool {
    false
}

#[cfg(windows)]
fn svc_status() -> (bool, bool) {
    crate::service::status()
}
#[cfg(not(windows))]
fn svc_status() -> (bool, bool) {
    (false, false)
}
