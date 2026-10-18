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
use crate::registry::{format_local, DeviceRow, Registry};

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
    show_new_pass: bool,
    update_start: String,
    update_end: String,

    /// Cloud sync status written by the running server.
    cloud: Option<crate::cloud::CloudStatus>,
    cloud_on: bool,
    cloud_form: CloudForm,
    /// A setup/join running on a worker thread (network calls).
    cloud_job: Option<std::sync::mpsc::Receiver<CloudJobResult>>,
}

/// How a cloud job (network calls on a worker thread) ended.
enum JobDone {
    Message(String),
    /// Own bucket: it already holds a copy; ask before replacing it.
    BucketOccupied,
    /// PJDesk account: it holds another server's copy; ask which way to go.
    Ask { cfg: crate::cloud::CloudConfig, from: String, written_at: u64, readable: bool },
}

type CloudJobResult = Result<JobDone, String>;

#[derive(Default)]
struct CloudForm {
    /// "Your own bucket" instead of a PJDesk account.
    own_bucket: bool,
    acct_email: String,
    acct_password: String,
    show_acct_password: bool,
    /// Signed in, waiting for the user to choose backup vs. replace.
    pending: Option<crate::cloud::CloudConfig>,
    ask: Option<(String, u64, bool)>,
    joining: bool,
    endpoint: String,
    bucket: String,
    key_id: String,
    secret: String,
    show_secret: bool,
    code: String,
    show_code: bool,
    confirm_replace: bool,
    confirm_join: bool,
    confirm_promote: bool,
    confirm_demote: bool,
}

enum CloudAct {
    SignIn,
    Choose(crate::cloud::Choice),
    CancelAsk,
    CreateAccount,
    Setup { replace: bool },
    Join,
    SyncNow,
    Promote,
    Demote,
    CopyCode,
    Off,
    SignInAgain,
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
            show_new_pass: false,
            update_start: String::new(),
            update_end: String::new(),
            cloud: None,
            cloud_on: false,
            cloud_form: CloudForm { bucket: "pjdesk-sync".into(), ..Default::default() },
            cloud_job: None,
        };
        let update = crate::self_update::Settings::load(&app.data_dir);
        app.update_start = update.window_start;
        app.update_end = update.window_end;
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
        self.cloud_on = self.data_dir.join(crate::cloud::CONFIG_FILE).exists();
        self.cloud = crate::cloud::CloudStatus::load(&self.data_dir);
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
        self.poll_cloud_job();

        // System tray: Show/Quit, and hide-to-tray when the window is closed.
        let tray_actions = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        let has_tray = self.tray.is_some();
        for action in tray_actions {
            match action {
                crate::tray::Action::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                crate::tray::Action::CheckUpdates => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    let dir = self.data_dir.clone();
                    self.status = "Checking for updates…".into();
                    std::thread::spawn(move || {
                        let _ = crate::self_update::check_now(&dir);
                    });
                }
                crate::tray::Action::Promote => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    self.cloud_form.confirm_promote = true;
                    self.cloud_form.confirm_demote = false;
                }
                crate::tray::Action::Demote => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    self.cloud_form.confirm_demote = true;
                    self.cloud_form.confirm_promote = false;
                }
                crate::tray::Action::SyncNow => {
                    self.status = match crate::requests::push(&self.data_dir, crate::requests::SYNC_FILE, "now") {
                        Ok(()) => "Syncing now.".into(),
                        Err(e) => format!("Could not ask the server to sync: {e}"),
                    };
                }
                crate::tray::Action::Quit => {
                    self.really_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if has_tray && ctx.input(|i| i.viewport().minimized == Some(true)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if has_tray && !self.really_quit && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            if !self.told_tray {
                self.told_tray = true;
                self.status = "PJDesk Server is still running in the tray — right-click it to Quit.".into();
            }
        }

        if let Some(tray) = &self.tray {
            let role = self.cloud.as_ref().map(|s| s.role.as_str()).filter(|s| !s.is_empty()).unwrap_or("off");
            let epoch = self.cloud.as_ref().map(|s| s.epoch).unwrap_or(0);
            let who = self.cloud.as_ref().map(|s| s.active_name.as_str()).filter(|s| !s.is_empty()).unwrap_or("none");
            let upd = crate::self_update::UpdateStatus::load(&self.data_dir);
            let available = if upd.available.is_empty() {
                String::new()
            } else {
                format!("\nUpdate available: {}", upd.available)
            };
            tray.set_tooltip(&format!("PJDesk Server\nCloud sync: {role} · generation {epoch} · active: {who}{available}"));
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
                self.updates_card(ui);
                ui.add_space(8.0);
                self.cloud_card(ui);
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
    fn updates_card(&mut self, ui: &mut egui::Ui) {
        use crate::self_update::{self, Settings};
        let mut settings = Settings::load(&self.data_dir);
        let reported = self_update::UpdateStatus::load(&self.data_dir);
        card(ui, |ui| {
            heading(ui, "UPDATES");
            let mut auto = settings.auto;
            if ui.checkbox(&mut auto, "Install server updates automatically").changed() {
                settings.auto = auto;
                settings.save(&self.data_dir);
                match self_update::set_task(&self.data_dir, auto) {
                    Ok(()) => {
                        self.status = if auto {
                            "Automatic server updates are on. The service checks about once an hour.".into()
                        } else {
                            "Automatic server updates are off.".into()
                        };
                    }
                    Err(e) => {
                        self.status = format!("{e:#}");
                    }
                }
            }
            ui.label(
                RichText::new(
                    "Off until you turn it on. A standby updates first. A primary waits until the standby is on the new version, or 24 hours, so both servers do not restart together.",
                )
                .color(MUTED)
                .size(11.0),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Maintenance window");
                ui.add(egui::TextEdit::singleline(&mut self.update_start).hint_text("any time").desired_width(70.0));
                ui.label("to");
                ui.add(egui::TextEdit::singleline(&mut self.update_end).hint_text("HH:MM").desired_width(70.0));
                if ui.button("Save").clicked() {
                    self.status = match self_update::set_window(&self.data_dir, &self.update_start, &self.update_end) {
                        Ok(()) => "Saved the maintenance window. Leave both blank for any time.".into(),
                        Err(e) => format!("{e}"),
                    };
                }
            });
            ui.label(RichText::new("Local time. Leave both blank to install at any time.").color(MUTED).size(11.0));
            ui.add_space(4.0);
            let checked = if reported.last_check_at == 0 {
                "Last check: never".to_string()
            } else {
                format!("Last check: {}", format_local(reported.last_check_at))
            };
            let result = if reported.last_result.is_empty() { "no result yet" } else { reported.last_result.as_str() };
            ui.label(RichText::new(format!("{checked} · {result}")).color(MUTED).size(12.0));
            if !reported.available.is_empty() {
                ui.label(format!("Available version: {}", reported.available));
            }
            if !reported.error.is_empty() {
                ui.colored_label(WARN, format!("⚠ {}", reported.error));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Check for updates now").clicked() {
                    let dir = self.data_dir.clone();
                    self.status = "Checking for updates…".into();
                    std::thread::spawn(move || {
                        let _ = self_update::check_now(&dir);
                    });
                }
                if !reported.available.is_empty() && ui.add(egui::Button::new("Install this update").fill(ACCENT)).clicked() {
                    let dir = self.data_dir.clone();
                    self.status = "Installing the update…".into();
                    std::thread::spawn(move || {
                        let _ = self_update::install_if_planned(&dir, true);
                    });
                }
            });
        });
    }

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
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.new_pass).password(!self.show_new_pass));
                        reveal_toggle(ui, &mut self.show_new_pass);
                    });
                    ui.end_row();
                });
                if !self.new_pass.is_empty() {
                    let report = crate::password_policy::evaluate(&self.new_pass, &self.new_user);
                    let word = match report.score {
                        0 => "empty",
                        1 => "too weak",
                        2 => "acceptable",
                        3 => "strong",
                        _ => "very strong",
                    };
                    ui.label(RichText::new(format!("Strength: {word}")).color(if report.ok { OK } else { WARN }).size(12.0));
                    for check in &report.checks {
                        let mark = if check.met { "✓" } else { "○" };
                        let color = if check.met { OK } else { MUTED };
                        ui.label(RichText::new(format!("{mark} {}", check.label)).color(color).size(11.0));
                    }
                }
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

    fn cloud_card(&mut self, ui: &mut egui::Ui) {
        let mut act: Option<CloudAct> = None;
        let busy = self.cloud_job.is_some();
        let dns = pjdesk_proto::DEFAULT_SERVER.split(':').next().unwrap_or("your server's name").to_string();
        let port = self.port;
        let online = self.online.len();
        let can_write = self.can_write;
        card(ui, |ui| {
            heading(ui, "CLOUD SYNC & BACKUP SERVER");
            if self.cloud_on {
                let s = self.cloud.clone().unwrap_or_default();
                let is_account = !s.account.is_empty();
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let fresh = s.updated_at + 90 >= now;
                ui.horizontal(|ui| {
                    if !fresh {
                        ui.colored_label(WARN, "●");
                        ui.label("Sync is on, but the server isn't running, so nothing is syncing.");
                    } else if s.role == "active" {
                        ui.colored_label(OK, "●");
                        let who = if s.active_name.is_empty() { "this server".to_string() } else { s.active_name.clone() };
                        ui.label(format!("Primary: {who} uploads to the cloud (generation {}).", s.epoch));
                    } else {
                        ui.colored_label(ACCENT, "●");
                        let who = if s.active_name.is_empty() { "the other server".to_string() } else { s.active_name.clone() };
                        ui.label(format!("Standby. Primary: {who} (generation {}).", s.epoch));
                    }
                });
                if !s.notice.is_empty() {
                    ui.colored_label(WARN, s.notice.clone());
                }
                let detail = if s.role == "active" || s.applied_from.is_empty() {
                    if s.last_upload_at > 0 {
                        format!("Last uploaded {}", format_local(s.last_upload_at))
                    } else if s.role == "active" {
                        "Not uploaded yet".to_string()
                    } else {
                        "Waiting for the main server's first upload".to_string()
                    }
                } else {
                    format!("Synced from {} (its data from {})", s.applied_from, format_local(s.applied_written_at))
                };
                ui.label(RichText::new(format!("{detail} · {}", s.bucket)).color(MUTED).size(12.0));
                let signed_out = s.error.contains("sign in again");
                if !s.error.is_empty() {
                    ui.colored_label(WARN, format!("⚠ {}", s.error));
                }
                ui.add_space(6.0);
                ui.add_enabled_ui(can_write && !busy, |ui| {
                    ui.horizontal(|ui| {
                        if signed_out && ui.add(egui::Button::new("Sign in again").fill(ACCENT)).clicked() {
                            act = Some(CloudAct::SignInAgain);
                        }
                        if ui.button("Sync now").clicked() {
                            act = Some(CloudAct::SyncNow);
                        }
                        if s.role == "standby" {
                            if ui.button("Make this the primary").clicked() {
                                self.cloud_form.confirm_promote = true;
                            }
                        } else if s.role == "active" && ui.button("Step down to standby").clicked() {
                            self.cloud_form.confirm_demote = true;
                        }
                        if !is_account
                            && ui
                                .button("Copy join code")
                                .on_hover_text("Everything a backup server needs to join. Keep it like a password.")
                                .clicked()
                        {
                            act = Some(CloudAct::CopyCode);
                        }
                        let (label, tip) = if is_account {
                            ("Sign out", "Sign this server out of PJDesk Cloud (the cloud copy is kept)")
                        } else {
                            ("Turn off", "Stop syncing on this server (the cloud copy is kept)")
                        };
                        if ui.button(label).on_hover_text(tip).clicked() {
                            act = Some(CloudAct::Off);
                        }
                    });
                });
                if self.cloud_form.confirm_promote {
                    ui.add_space(4.0);
                    ui.colored_label(
                        WARN,
                        "Make this server the primary? It downloads the latest copy first. The other server becomes the standby the next time it checks, even if that server is offline right now.",
                    );
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Make this the primary").fill(ACCENT)).clicked() {
                            self.cloud_form.confirm_promote = false;
                            act = Some(CloudAct::Promote);
                        }
                        if ui.button("Cancel").clicked() {
                            self.cloud_form.confirm_promote = false;
                        }
                    });
                }
                if self.cloud_form.confirm_demote {
                    ui.add_space(4.0);
                    ui.colored_label(
                        WARN,
                        "Step down to standby? This server will stop uploading. The other server stays the primary until you make it the primary there.",
                    );
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Step down").fill(ACCENT)).clicked() {
                            self.cloud_form.confirm_demote = false;
                            act = Some(CloudAct::Demote);
                        }
                        if ui.button("Cancel").clicked() {
                            self.cloud_form.confirm_demote = false;
                        }
                    });
                }
                ui.add_space(4.0);
                let how_to_add = if is_account {
                    format!("Backup server: install PJDesk Server on another computer and sign in there with {}.", s.account)
                } else {
                    "Backup server: install PJDesk Server on another computer, choose Your own bucket → This is a backup server, and paste the join code.".to_string()
                };
                ui.label(
                    RichText::new(format!(
                        "{how_to_add} To switch over: forward TCP + UDP {port} to that computer, point {dns} at it, and stop this server. Computers move over within a couple of minutes."
                    ))
                    .color(MUTED)
                    .size(11.0),
                );
            } else {
                ui.label(
                    RichText::new(format!(
                        "Keep an encrypted copy of this server (its key, user accounts, address books and computer list) in the cloud. \
                         A backup server stays in sync and takes over when you point {dns} at it."
                    ))
                    .color(MUTED)
                    .size(12.0),
                );
                ui.add_space(6.0);
                let f = &mut self.cloud_form;
                ui.horizontal(|ui| {
                    if ui.selectable_label(!f.own_bucket, "PJDesk account").clicked() {
                        f.own_bucket = false;
                    }
                    if ui.selectable_label(f.own_bucket, "Your own bucket (advanced)").clicked() {
                        f.own_bucket = true;
                    }
                });
                ui.add_space(4.0);
                ui.add_enabled_ui(can_write && !busy, |ui| {
                    if !f.own_bucket {
                        if let Some((from, written_at, readable)) = f.ask.clone() {
                            if readable {
                                ui.colored_label(
                                    WARN,
                                    format!(
                                        "Your account already holds {from}'s data (from {}), and {online} computer(s) are connected to this server.",
                                        format_local(written_at)
                                    ),
                                );
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(egui::Button::new("Make this server a backup").fill(ACCENT))
                                        .on_hover_text(format!("Replace this server's key, accounts and computer list with {from}'s"))
                                        .clicked()
                                    {
                                        act = Some(CloudAct::Choose(crate::cloud::Choice::Join));
                                    }
                                    if ui
                                        .button("Use this server's data")
                                        .on_hover_text("Replace the copy in the cloud with this server's data")
                                        .clicked()
                                    {
                                        act = Some(CloudAct::Choose(crate::cloud::Choice::Replace));
                                    }
                                    if ui.button("Cancel").clicked() {
                                        act = Some(CloudAct::CancelAsk);
                                    }
                                });
                            } else {
                                ui.colored_label(WARN, "Your account's cloud copy was made with an older password, so it can't be opened.");
                                ui.horizontal(|ui| {
                                    if ui.add(egui::Button::new("Upload this server's data").fill(ACCENT)).clicked() {
                                        act = Some(CloudAct::Choose(crate::cloud::Choice::Replace));
                                    }
                                    if ui.button("Cancel").clicked() {
                                        act = Some(CloudAct::CancelAsk);
                                    }
                                });
                            }
                        } else {
                            egui::Grid::new("cloudacct").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                                ui.label("Email");
                                ui.add(egui::TextEdit::singleline(&mut f.acct_email).desired_width(300.0));
                                ui.end_row();
                                ui.label("Password");
                                ui.horizontal(|ui| {
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut f.acct_password)
                                            .password(!f.show_acct_password)
                                            .desired_width(270.0),
                                    );
                                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                        act = Some(CloudAct::SignIn);
                                    }
                                    reveal_toggle(ui, &mut f.show_acct_password);
                                });
                                ui.end_row();
                            });
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if ui.add(egui::Button::new("Sign in").fill(ACCENT)).clicked() {
                                    act = Some(CloudAct::SignIn);
                                }
                                ui.label(RichText::new("No account yet?").color(MUTED).size(12.0));
                                if ui.link("Create one at pjdesk.com").clicked() {
                                    act = Some(CloudAct::CreateAccount);
                                }
                            });
                            ui.label(
                                RichText::new(
                                    "The first server you sign in is the main one; sign in on another PJDesk Server with the same account to make it the backup. \
                                     Your password never leaves this computer, and the copy is encrypted so only your servers can read it.",
                                )
                                .color(MUTED)
                                .size(11.0),
                            );
                        }
                    } else {
                        ui.horizontal(|ui| {
                            if ui.selectable_label(!f.joining, "This is the main server").clicked() {
                                f.joining = false;
                            }
                            if ui.selectable_label(f.joining, "This is a backup server").clicked() {
                                f.joining = true;
                            }
                        });
                        ui.add_space(4.0);
                        if !f.joining {
                            egui::Grid::new("cloudsetup").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                                ui.label("S3 endpoint");
                                ui.add(
                                    egui::TextEdit::singleline(&mut f.endpoint)
                                        .hint_text("https://<account-id>.r2.cloudflarestorage.com")
                                        .desired_width(330.0),
                                );
                                ui.end_row();
                                ui.label("Bucket");
                                ui.add(egui::TextEdit::singleline(&mut f.bucket).desired_width(330.0));
                                ui.end_row();
                                ui.label("Access key ID");
                                ui.add(egui::TextEdit::singleline(&mut f.key_id).desired_width(330.0));
                                ui.end_row();
                                ui.label("Secret access key");
                                ui.horizontal(|ui| {
                                    ui.add(egui::TextEdit::singleline(&mut f.secret).password(!f.show_secret).desired_width(300.0));
                                    reveal_toggle(ui, &mut f.show_secret);
                                });
                                ui.end_row();
                            });
                            ui.add_space(4.0);
                            if f.confirm_replace {
                                ui.colored_label(WARN, "This bucket already holds a PJDesk server copy. Replace it with this server's?");
                                ui.horizontal(|ui| {
                                    if ui.add(egui::Button::new("Replace it").fill(ACCENT)).clicked() {
                                        act = Some(CloudAct::Setup { replace: true });
                                    }
                                    if ui.button("Cancel").clicked() {
                                        f.confirm_replace = false;
                                    }
                                });
                            } else if ui.add(egui::Button::new("Start syncing").fill(ACCENT)).clicked() {
                                act = Some(CloudAct::Setup { replace: false });
                            }
                        } else {
                            ui.horizontal(|ui| {
                                ui.label("Join code");
                                ui.add(
                                    egui::TextEdit::singleline(&mut f.code)
                                        .password(!f.show_code)
                                        .hint_text("pjdesk-sync-1:…")
                                        .desired_width(330.0),
                                );
                                reveal_toggle(ui, &mut f.show_code);
                            });
                            ui.label(
                                RichText::new("Copy it from the main server (Copy join code). Joining replaces this server's key, accounts and computer list with the main server's.")
                                    .color(MUTED)
                                    .size(11.0),
                            );
                            ui.add_space(4.0);
                            if f.confirm_join {
                                ui.colored_label(WARN, format!("{online} computer(s) are connected to this server right now. Join anyway?"));
                                ui.horizontal(|ui| {
                                    if ui.add(egui::Button::new("Join anyway").fill(ACCENT)).clicked() {
                                        act = Some(CloudAct::Join);
                                    }
                                    if ui.button("Cancel").clicked() {
                                        f.confirm_join = false;
                                    }
                                });
                            } else if ui.add(egui::Button::new("Join").fill(ACCENT)).clicked() {
                                if online > 0 {
                                    f.confirm_join = true;
                                } else {
                                    act = Some(CloudAct::Join);
                                }
                            }
                        }
                    }
                });
            }
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Talking to the cloud…").color(MUTED).size(12.0));
                });
            }
        });
        if let Some(act) = act {
            self.run_cloud_action(act);
        }
    }

    fn run_cloud_action(&mut self, act: CloudAct) {
        use crate::cloud::{self, CloudConfig};
        let dir = self.data_dir.clone();
        let online = self.online.len();
        match act {
            CloudAct::SignIn => {
                let email = self.cloud_form.acct_email.trim().to_string();
                let password = std::mem::take(&mut self.cloud_form.acct_password);
                if email.is_empty() || password.is_empty() {
                    self.status = "Enter your PJDesk account email and password.".into();
                    return;
                }
                self.spawn_cloud_job(move || {
                    let cfg = cloud::sign_in(&cloud::default_api(), &email, &password)?;
                    finish_account(&dir, cfg, online, None)
                });
            }
            CloudAct::Choose(choice) => {
                self.cloud_form.ask = None;
                let Some(cfg) = self.cloud_form.pending.take() else { return };
                self.spawn_cloud_job(move || finish_account(&dir, cfg, online, Some(choice)));
            }
            CloudAct::CancelAsk => {
                self.cloud_form.ask = None;
                if let Some(cfg) = self.cloud_form.pending.take() {
                    std::thread::spawn(move || cloud::discard_sign_in(&cfg));
                }
            }
            CloudAct::CreateAccount => open_url(&format!("{}/account/#create", cloud::default_api())),
            CloudAct::SignInAgain => {
                if let Some(cfg) = CloudConfig::load(&dir) {
                    self.cloud_form.acct_email = cfg.email.clone();
                }
                self.cloud_form.own_bucket = false;
                cloud::turn_off(&dir);
                self.status = "Enter your PJDesk account password to sign in again.".into();
                self.refresh();
            }
            CloudAct::SyncNow => {
                self.status = match crate::requests::push(&dir, crate::requests::SYNC_FILE, "now") {
                    Ok(()) => "Syncing now.".into(),
                    Err(e) => format!("Could not ask the server to sync: {e}"),
                };
            }
            CloudAct::Promote => {
                if crate::cloud::status_is_fresh(&dir) {
                    self.status = match crate::requests::push(&dir, crate::requests::PROMOTE_FILE, "now") {
                        Ok(()) => "Asking the running server to become the primary.".into(),
                        Err(e) => format!("Could not ask the server to become the primary: {e}"),
                    };
                } else {
                    self.spawn_cloud_job(move || {
                        crate::cloud::promote(&dir)?;
                        Ok(JobDone::Message(
                            "This server is now the primary. The other server will switch to standby the next time it checks.".into(),
                        ))
                    });
                }
            }
            CloudAct::Demote => {
                if crate::cloud::status_is_fresh(&dir) {
                    self.status = match crate::requests::push(&dir, crate::requests::DEMOTE_FILE, "now") {
                        Ok(()) => "Asking the running server to step down to standby.".into(),
                        Err(e) => format!("Could not ask the server to step down: {e}"),
                    };
                } else {
                    self.spawn_cloud_job(move || {
                        crate::cloud::demote(&dir)?;
                        Ok(JobDone::Message(
                            "This server stepped down to standby and will not upload.".into(),
                        ))
                    });
                }
            }
            CloudAct::CopyCode => match CloudConfig::load(&dir) {
                Some(cfg) => {
                    copy(&cfg.join_code());
                    self.status = "Join code copied. Save it somewhere safe, like your password manager; it's what adds a backup server.".into();
                }
                None => self.status = "Can't read the sync settings (restart as administrator).".into(),
            },
            CloudAct::Off => {
                let was_account = self.cloud.as_ref().is_some_and(|s| !s.account.is_empty());
                std::thread::spawn({
                    let dir = dir.clone();
                    move || cloud::sign_out(&dir)
                });
                // Remove the settings right away so the window updates.
                cloud::turn_off(&dir);
                self.status = if was_account {
                    "Signed out of PJDesk Cloud; cloud sync is off on this server. The cloud copy is kept.".into()
                } else {
                    "Cloud sync is off on this server. The copy in the bucket is kept.".into()
                };
                self.refresh();
            }
            CloudAct::Setup { replace } => {
                let f = &self.cloud_form;
                let cfg = match CloudConfig::new(&f.endpoint, &f.bucket, &f.key_id, &f.secret) {
                    Ok(c) => c,
                    Err(e) => {
                        self.status = format!("{e}");
                        return;
                    }
                };
                self.cloud_form.confirm_replace = false;
                self.spawn_cloud_job(move || {
                    if !replace && matches!(cloud::check_bucket(&cfg)?, cloud::SetupCheck::Occupied) {
                        return Ok(JobDone::BucketOccupied);
                    }
                    cloud::setup(&dir, &cfg)?;
                    let _ = crate::requests::push(&dir, crate::requests::SYNC_FILE, "now");
                    Ok(JobDone::Message(
                        "Cloud sync is on and this server's data is uploaded. Next: Copy join code, then use it on your backup server."
                            .into(),
                    ))
                });
            }
            CloudAct::Join => {
                self.cloud_form.confirm_join = false;
                let cfg = match CloudConfig::from_join_code(&self.cloud_form.code) {
                    Ok(c) => c,
                    Err(e) => {
                        self.status = format!("{e}");
                        return;
                    }
                };
                self.spawn_cloud_job(move || {
                    let from = cloud::join(&dir, &cfg)?;
                    let _ = crate::requests::push(&dir, crate::requests::RELOAD_FILE, "now");
                    Ok(JobDone::Message(format!(
                        "Joined. This server now has the key, accounts and computer list from {from}, and stays in sync on standby."
                    )))
                });
            }
        }
    }

    fn spawn_cloud_job(&mut self, job: impl FnOnce() -> anyhow::Result<JobDone> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(job().map_err(|e| format!("{e:#}")));
        });
        self.cloud_job = Some(rx);
    }

    fn poll_cloud_job(&mut self) {
        let Some(rx) = &self.cloud_job else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.cloud_job = None;
        match result {
            Ok(JobDone::Message(msg)) => {
                self.status = msg;
                let f = &mut self.cloud_form;
                f.secret.clear();
                f.code.clear();
                f.acct_password.clear();
                f.show_secret = false;
                f.show_code = false;
                f.show_acct_password = false;
                self.refresh();
            }
            Ok(JobDone::BucketOccupied) => self.cloud_form.confirm_replace = true,
            Ok(JobDone::Ask { cfg, from, written_at, readable }) => {
                self.cloud_form.pending = Some(cfg);
                self.cloud_form.ask = Some((from, written_at, readable));
            }
            Err(e) => self.status = e,
        }
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
                            if ui
                                .small_button("Forget")
                                .on_hover_text("Remove from this list. If this computer connects again, it comes back with the same ID.")
                                .clicked()
                            {
                                remove = Some(d.id.clone());
                            }
                            if live.is_some() && ui.small_button("Update").on_hover_text("Update this computer now").clicked() {
                                update = Some(d.id.clone());
                            }
                            let version = live.cloned().unwrap_or_else(|| d.version.clone());
                            let vtext = if version.is_empty() { String::new() } else { format!(" · v{version}") };
                            let details = format!(
                                "Last seen {} ({})\nFirst registered {}",
                                format_local(d.last_seen),
                                ago(d.last_seen),
                                format_local(d.first_seen)
                            );
                            if live.is_some() {
                                ui.label(RichText::new(format!("● online now{vtext}")).color(OK).size(11.0))
                                    .on_hover_text(details);
                            } else {
                                ui.label(
                                    RichText::new(format!("last seen {}{vtext}", format_local(d.last_seen)))
                                        .color(MUTED)
                                        .size(11.0),
                                )
                                .on_hover_text(details);
                            }
                        });
                    });
                }
            });
        });
        if let Some(id) = remove {
            self.forget(&id);
        }
        if update_all {
            self.request_update("*");
        }
        if let Some(id) = update {
            self.request_update(&id);
        }
    }

    /// Forgets a device. Goes through the running server (which keeps the
    /// device list in memory) so it doesn't write the record back; relaunches
    /// elevated when the window can't write the admin-only data folder.
    fn forget(&mut self, id: &str) {
        let shown = pjdesk_proto::format_id(id);
        if self.can_write {
            match crate::requests::forget_device(&self.data_dir, id) {
                Ok(()) => self.status = format!("Forgot {shown}."),
                Err(e) => self.status = format!("Could not forget {shown}: {e}"),
            }
        } else if run_elevated(&["forget-device", id]) {
            self.status = format!("Forgot {shown}.");
        }
        self.refresh();
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

/// Finishes signing in to a PJDesk account: uploads, joins, or asks.
fn finish_account(
    dir: &std::path::Path,
    cfg: crate::cloud::CloudConfig,
    online: usize,
    choice: Option<crate::cloud::Choice>,
) -> anyhow::Result<JobDone> {
    use crate::cloud::Connected;
    let email = cfg.email.clone();
    Ok(match crate::cloud::connect_account(dir, &cfg, online, choice)? {
        Connected::Uploaded => {
            let _ = crate::requests::push(dir, crate::requests::SYNC_FILE, "now");
            JobDone::Message(format!(
                "Signed in as {email}. This is the main server, and its data is backed up to PJDesk Cloud.                  To add a backup server, sign in on another PJDesk Server with the same account."
            ))
        }
        Connected::Resumed => JobDone::Message(format!("Signed in as {email}. Syncing as before.")),
        Connected::Joined { from } => {
            let _ = crate::requests::push(dir, crate::requests::RELOAD_FILE, "now");
            JobDone::Message(format!(
                "Signed in as {email}. This server now has {from}'s key, accounts and computer list, and stays on standby to take over."
            ))
        }
        Connected::Replaced => {
            let _ = crate::requests::push(dir, crate::requests::SYNC_FILE, "now");
            JobDone::Message(format!("Signed in as {email}. This server's data replaced the copy in PJDesk Cloud."))
        }
        Connected::Ask { from, written_at, readable } => JobDone::Ask { cfg, from, written_at, readable },
    })
}

/// Opens a web page in the default browser.
#[cfg(windows)]
fn open_url(url: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe {
        ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(url),
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// A show/hide toggle for a password field.
fn reveal_toggle(ui: &mut egui::Ui, shown: &mut bool) {
    if ui.small_button(if *shown { "Hide" } else { "Show" }).clicked() {
        *shown = !*shown;
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
    crate::requests::push(data_dir, crate::requests::UPDATE_FILE, target)
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
