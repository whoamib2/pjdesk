//! The PJDesk Server management window (what you get by double-clicking
//! `pjdesk-server.exe`): one PJDesk account sign-in, service control, the
//! server key fingerprint, registered devices, and a tail of the log.
//!
//! That PJDesk account is Cloud Sync, the active or standby role, and My
//! network. Older local accounts in `accounts.json` still work, and a new
//! server does not need them. Install, stop, and uninstall need a Windows
//! administrator. Saving settings needs that same write access to the data
//! folder (SYSTEM and Administrators) and the signed-in PJDesk account. There
//! is no separate PJDesk password. When the window can't write the folder it
//! offers to relaunch elevated.

// The window is the Windows entry point. Linux compiles it for headless shots.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui::{self, Color32, RichText};

use std::collections::HashMap;

use crate::accounts::{self, Accounts};
use crate::registry::{format_local, DeviceRow, Registry};
use crate::status_view::{self, Access, Snapshot};
use crate::ui_icons::Icon;
use crate::ui_theme::{self, Paint};
use crate::ui_widgets;

#[cfg(feature = "ui-shots")]
#[path = "gui_shots.rs"]
mod shots;

fn p(ui: &egui::Ui) -> Paint {
    ui_theme::paint(ui)
}

pub fn run(data_dir: PathBuf, port: u16, start_hidden: bool) -> Result<()> {
    // One icon per user. A second launch asks the first window to show itself.
    if !crate::tray::acquire_or_signal() {
        return Ok(());
    }
    crate::tray::ensure_login_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 820.0])
            .with_min_inner_size([760.0, 640.0])
            .with_visible(!start_hidden)
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
    /// The running service accepted a management command from this user.
    control_ready: bool,
    accounts: Vec<accounts::AccountInfo>,
    devices: Vec<DeviceRow>,
    /// Live online devices (id → reported version), from the server's online.json.
    online: HashMap<String, String>,
    log_tail: String,
    status: String,

    last_refresh: std::time::Instant,
    tray: Option<crate::tray::Tray>,
    really_quit: bool,
    told_tray: bool,
    update_start: String,
    update_end: String,

    /// Cloud sync status written by the running server.
    cloud: Option<crate::cloud::CloudStatus>,
    cloud_on: bool,
    cloud_form: CloudForm,
    /// A setup/join running on a worker thread (network calls).
    cloud_job: Option<std::sync::mpsc::Receiver<CloudJobResult>>,
    /// Whether this process can list the data folder.
    access: Access,
    /// Latest snapshot from the service pipe. Used when the folder is locked.
    live: Option<Snapshot>,
    probe: Option<std::sync::mpsc::Receiver<Option<Snapshot>>>,
    opened_at: Instant,
    /// Screenshots inject the service state. The real window asks Windows.
    svc_override: Option<(bool, bool)>,
    preview_locked: bool,
    /// Headless shot of the computer list.
    preview_devices: bool,
    /// Headless shot of the signed-in-accounts card.
    preview_network: bool,
    /// Headless shot of the network-key dialog.
    preview_netkey: bool,
    /// Headless shot of the PJDesk account card.
    preview_account: bool,
    nk_open: bool,
    nk_passphrase_mode: bool,
    nk_pass: String,
    nk_show: bool,
    nk_days: String,
    nk_max: String,
    nk_bound: String,
    nk_blob: String,
    nk_embedded: bool,
    nk_error: String,
    nk_busy: bool,
    nk_rx: Option<std::sync::mpsc::Receiver<Result<crate::cloud::CreatedKey, String>>>,
    nk_invites: Vec<crate::cloud::NetworkInvite>,
    nk_loaded: bool,
    nk_list_rx: Option<std::sync::mpsc::Receiver<Result<Vec<crate::cloud::NetworkInvite>, String>>>,
    pub_addr: String,
    pub_addr_ready: bool,
    pub_addr_note: String,
    pub_addr_needs_confirm: bool,
    pub_addr_confirmed: bool,
    pub_guess_rx: Option<std::sync::mpsc::Receiver<crate::public_addr::PublicAddress>>,
    /// Latest client release from pjdesk.com, fetched off this thread.
    latest_release: Option<String>,
    release_at: Option<Instant>,
    release_rx: Option<std::sync::mpsc::Receiver<Option<String>>>,
    check_release: bool,
}

/// How a cloud job (network calls on a worker thread) ended.
enum JobDone {
    Message(String),
    /// Own bucket: it already holds a copy; ask before replacing it.
    BucketOccupied,
    /// PJDesk account: it holds another server's copy; ask which way to go.
    Ask {
        cfg: crate::cloud::CloudConfig,
        from: String,
        written_at: u64,
        readable: bool,
    },
    /// The service is holding the sign-in until this window chooses.
    ServiceAsk {
        pending: String,
        from: String,
        written_at: u64,
        readable: bool,
    },
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
    /// Sign-in held by the service until this window chooses join or replace.
    service_pending: String,
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
    Setup {
        replace: bool,
    },
    Join,
    SyncNow,
    Promote,
    Demote,
    CopyCode,
    Off,
    /// Sign out, then offer the sign-in form for a different PJDesk account.
    Switch,
    SignInAgain,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, data_dir: PathBuf, port: u16) -> Self {
        Self::build(cc, data_dir, port, true)
    }

    fn build(cc: &eframe::CreationContext<'_>, data_dir: PathBuf, port: u16, spawn_probe: bool) -> Self {
        ui_theme::apply(&cc.egui_ctx, "system");
        let mut app = Self {
            data_dir,
            port,
            fingerprint: String::new(),
            can_write: false,
            control_ready: false,
            accounts: Vec::new(),
            devices: Vec::new(),
            online: HashMap::new(),
            log_tail: String::new(),
            status: String::new(),
            last_refresh: std::time::Instant::now(),
            tray: crate::tray::Tray::new(&cc.egui_ctx),
            really_quit: false,
            told_tray: false,
            update_start: String::new(),
            update_end: String::new(),
            cloud: None,
            cloud_on: false,
            cloud_form: CloudForm { bucket: "pjdesk-sync".into(), ..Default::default() },
            cloud_job: None,
            access: Access::Missing,
            live: None,
            probe: None,
            opened_at: Instant::now(),
            svc_override: None,
            preview_locked: false,
            preview_devices: false,
            preview_network: false,
            preview_netkey: false,
            preview_account: false,
            nk_open: false,
            nk_passphrase_mode: true,
            nk_pass: String::new(),
            nk_show: false,
            nk_days: String::new(),
            nk_max: String::new(),
            nk_bound: String::new(),
            nk_blob: String::new(),
            nk_embedded: false,
            nk_error: String::new(),
            nk_busy: false,
            nk_rx: None,
            nk_invites: Vec::new(),
            nk_loaded: false,
            nk_list_rx: None,
            pub_addr: String::new(),
            pub_addr_ready: false,
            pub_addr_note: String::new(),
            pub_addr_needs_confirm: false,
            pub_addr_confirmed: false,
            pub_guess_rx: None,
            latest_release: None,
            release_at: None,
            release_rx: None,
            check_release: spawn_probe,
        };
        if let crate::self_update::Loaded::Present(update) = crate::self_update::Settings::read(&app.data_dir) {
            app.update_start = update.window_start;
            app.update_end = update.window_end;
        }
        crate::public_addr::remember_port(&app.data_dir, port);
        app.refresh();
        if spawn_probe {
            app.probe = spawn_status_probe(&cc.egui_ctx);
            app.start_address_guess();
        }
        app
    }

    fn start_address_guess(&mut self) {
        if self.pub_guess_rx.is_some()
            || crate::public_addr::load(&self.data_dir).is_some_and(|saved| !saved.address.trim().is_empty())
        {
            return;
        }
        let dir = self.data_dir.clone();
        let port = self.port;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::public_addr::ensure(&dir, port));
        });
        self.pub_guess_rx = Some(rx);
    }

    fn poll_public_address(&mut self) {
        if let Some(rx) = &self.pub_guess_rx {
            if let Ok(record) = rx.try_recv() {
                self.pub_guess_rx = None;
                if self.pub_addr.trim().is_empty() {
                    self.pub_addr = record.address;
                    self.pub_addr_note = record.note;
                    self.pub_addr_needs_confirm = record.needs_confirm;
                    self.pub_addr_confirmed = record.confirmed;
                }
            }
        }
        if !self.pub_addr_ready {
            if let Some(saved) = crate::public_addr::load(&self.data_dir) {
                self.pub_addr = saved.address;
                self.pub_addr_note = saved.note;
                self.pub_addr_needs_confirm = saved.needs_confirm;
                self.pub_addr_confirmed = saved.confirmed;
            }
            self.pub_addr_ready = true;
        } else if let Some(saved) = crate::public_addr::load(&self.data_dir) {
            if saved.address.trim() == self.pub_addr.trim() {
                self.pub_addr_note = saved.note;
                self.pub_addr_needs_confirm = saved.needs_confirm;
                self.pub_addr_confirmed = saved.confirmed;
            }
        }
    }

    fn service_state(&self) -> (bool, bool) {
        if let Some(state) = self.svc_override {
            return state;
        }
        svc_status()
    }

    fn waiting_for_service(&self) -> bool {
        self.access != Access::Readable && self.live.is_none() && self.opened_at.elapsed() < Duration::from_secs(2)
    }

    fn via_service(&self) -> bool {
        self.control_ready && !self.can_write
    }

    fn refresh(&mut self) {
        if self.preview_locked {
            self.access = Access::Locked;
            self.can_write = false;
            self.control_ready = false;
            return;
        }
        self.control_ready = crate::control::ready();
        self.access = status_view::access(&self.data_dir);
        if self.access == Access::Missing && std::fs::create_dir_all(&self.data_dir).is_ok() {
            self.access = status_view::access(&self.data_dir);
        }
        // A locked folder must not be filled in with defaults. The pipe, if
        // the service is running, is what this window shows instead.
        if self.access != Access::Readable {
            self.can_write = false;
            return;
        }
        self.can_write = std::fs::write(self.data_dir.join(".wtest"), b"1").is_ok();
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
        if self.last_refresh.elapsed() > Duration::from_secs(3) {
            self.last_refresh = Instant::now();
            self.refresh();
        }
        if let Some(rx) = &self.probe {
            while let Ok(msg) = rx.try_recv() {
                if let Some(snap) = msg {
                    self.live = Some(snap);
                }
            }
        }
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(if self.waiting_for_service() {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(1)
        });
        self.poll_cloud_job();
        self.poll_netkey();
        self.poll_release();

        // System tray: Show/Quit, and hide-to-tray when the window is closed.
        let tray_actions = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        let has_tray = self.tray.is_some();
        for action in tray_actions {
            match action {
                crate::tray::Action::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                crate::tray::Action::ServiceStart => self.elevate_service(&["service", "start"]),
                crate::tray::Action::ServiceStop => self.elevate_service(&["service", "stop"]),
                crate::tray::Action::ServiceRestart => {
                    if self.control_ready {
                        self.ask_service(crate::control::Request { cmd: "restart".into(), ..Default::default() });
                    } else {
                        self.elevate_service(&["service", "restart"]);
                    }
                }
                crate::tray::Action::OpenLog => crate::tray::open_log_folder(&self.data_dir),
                crate::tray::Action::CheckUpdates => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    if self.via_service() {
                        self.status = "Checking for updates…".into();
                        self.spawn_cloud_job(|| {
                            service_job(crate::control::Request { cmd: "check".into(), ..Default::default() })
                        });
                    } else {
                        let dir = self.data_dir.clone();
                        self.status = "Checking for updates…".into();
                        std::thread::spawn(move || {
                            let _ = crate::self_update::check_now(&dir);
                        });
                    }
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
                    if self.via_service() {
                        self.ask_service(crate::control::Request { cmd: "sync".into(), ..Default::default() });
                    } else {
                        self.status = match crate::requests::push(&self.data_dir, crate::requests::SYNC_FILE, "now") {
                            Ok(()) => "Syncing now.".into(),
                            Err(e) => format!("Could not ask the server to sync: {e}"),
                        };
                    }
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
            let (installed, running) = self.service_state();
            let (role, epoch, who, available, last_result, hosts) = if self.access != Access::Readable {
                if let Some(s) = &self.live {
                    (
                        s.role.clone(),
                        s.epoch,
                        s.active_name.clone(),
                        s.update_available.clone(),
                        s.last_result.clone(),
                        self.online.len(),
                    )
                } else {
                    (String::new(), 0, String::new(), String::new(), String::new(), 0)
                }
            } else {
                let upd = crate::self_update::UpdateStatus::load(&self.data_dir);
                (
                    self.cloud.as_ref().map(|s| s.role.clone()).unwrap_or_default(),
                    self.cloud.as_ref().map(|s| s.epoch).unwrap_or(0),
                    self.cloud.as_ref().map(|s| s.active_name.clone()).unwrap_or_default(),
                    upd.available,
                    upd.last_result,
                    self.online.len(),
                )
            };
            tray.set_tooltip(&crate::tray_tip::format(&crate::tray_tip::Tip {
                installed,
                running,
                hosts,
                role: &role,
                epoch,
                active: &who,
                available: &available,
                last_result: &last_result,
            }));
        }

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("PJDesk Server").size(22.0).strong().color(p(ui).text));
                    ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).color(p(ui).muted));
                });
                ui.add_space(8.0);

                if self.via_service() {
                    card(ui, |ui| {
                        ui.label("Managed through the PJDesk service.");
                        ui.label(
                            RichText::new("Install, uninstall, and stop still ask Windows to approve that one action.")
                                .color(p(ui).muted)
                                .size(12.0),
                        );
                        if ui.link("Restart as administrator").clicked() && run_elevated(&[]) {
                            std::process::exit(0);
                        }
                    });
                    ui.add_space(8.0);
                } else if !self.can_write {
                    card(ui, |ui| {
                        ui.horizontal(|ui| {
                            glyph(ui, Icon::Alert, p(ui).warn);
                            ui.label("Running without Administrator rights. You can look. Install, stop, and uninstall need a Windows administrator. PJDesk settings use the signed-in account once this window can write the data folder, or once the PJDesk service can take those actions. There is no separate PJDesk password.");
                        });
                        ui.add_space(8.0);
                        if ui_widgets::accent_button(ui, "Restart as administrator") && run_elevated(&[]) {
                            std::process::exit(0);
                        }
                    });
                    ui.add_space(8.0);
                }

                if self.preview_devices {
                    ui.label(RichText::new(format!("This server  v{}", env!("CARGO_PKG_VERSION"))).strong().size(18.0));
                    ui.add_space(8.0);
                    self.devices_card(ui);
                    return;
                }
                if self.preview_network || self.preview_netkey {
                    ui.label(RichText::new(format!("This server  v{}", env!("CARGO_PKG_VERSION"))).strong().size(18.0));
                    ui.add_space(8.0);
                    self.network_card(ui);
                    return;
                }
                self.account_card(ui);
                if self.preview_account {
                    return;
                }
                ui.add_space(8.0);
                self.service_card(ui);
                ui.add_space(8.0);
                self.network_card(ui);
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
                        glyph(ui, Icon::Alert, p(ui).warn);
                        ui.label(&self.status);
                        if ui_widgets::icon_button(ui, Icon::Close, "Dismiss") {
                            self.status.clear();
                        }
                    });
                }
            });
        });
        self.netkey_dialog(ui);
    }
}

#[derive(Clone, Copy)]
enum SvcAction {
    Install,
    Uninstall,
    Start,
    Stop,
    Restart,
}

impl App {
    fn updates_card(&mut self, ui: &mut egui::Ui) {
        if self.access != Access::Readable {
            self.updates_locked(ui);
            return;
        }
        use crate::self_update::{self, Settings};
        let mut settings = Settings::load(&self.data_dir);
        let reported = self_update::UpdateStatus::load(&self.data_dir);
        let may_edit = self.may_edit_settings();
        card(ui, |ui| {
            heading(ui, "UPDATES");
            let mut auto = settings.auto;
            let auto_changed = ui
                .add_enabled_ui(may_edit, |ui| {
                    ui_widgets::checkbox(ui, &mut auto, "Install server updates automatically")
                })
                .inner;
            if auto_changed {
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
                .color(p(ui).muted)
                .size(11.0),
            );
            ui.add_space(4.0);
            ui.add_enabled_ui(may_edit, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Maintenance window");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.update_start).hint_text("any time").desired_width(70.0),
                    );
                    ui.label("to");
                    ui.add(egui::TextEdit::singleline(&mut self.update_end).hint_text("HH:MM").desired_width(70.0));
                    if ui.button("Save").clicked() {
                        self.status =
                            match self_update::set_window(&self.data_dir, &self.update_start, &self.update_end) {
                                Ok(()) => "Saved the maintenance window. Leave both blank for any time.".into(),
                                Err(e) => format!("{e}"),
                            };
                    }
                });
            });
            if !may_edit {
                ui.label(
                    RichText::new(if !self.can_write {
                        "Restart as administrator to save this. Windows protects the data folder. There is no separate PJDesk password."
                    } else {
                        "Sign in with your PJDesk account above to change this. There is no separate server password."
                    })
                    .color(p(ui).muted)
                    .size(12.0),
                );
            }
            ui.label(
                RichText::new("Local time. Leave both blank to install at any time.").color(p(ui).muted).size(11.0),
            );
            ui.add_space(4.0);
            let checked = if reported.last_check_at == 0 {
                "Last check: never".to_string()
            } else {
                format!("Last check: {}", format_local(reported.last_check_at))
            };
            let result = if reported.last_result.is_empty() { "no result yet" } else { reported.last_result.as_str() };
            ui.label(RichText::new(format!("{checked} · {result}")).color(p(ui).muted).size(12.0));
            if !reported.available.is_empty() {
                ui.label(format!("Available version: {}", reported.available));
            }
            if !reported.error.is_empty() {
                ui.horizontal(|ui| {
                    glyph(ui, Icon::Alert, p(ui).warn);
                    ui.colored_label(p(ui).warn, &reported.error);
                });
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
                if !reported.available.is_empty()
                    && ui.add(egui::Button::new("Install this update").fill(p(ui).accent)).clicked()
                {
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
            let (installed, running) = self.service_state();
            ui.horizontal(|ui| {
                let (c, t) = if !installed {
                    (p(ui).muted, "Not installed")
                } else if running {
                    (p(ui).ok, "Installed and running")
                } else {
                    (p(ui).warn, "Installed, not running")
                };
                ui_widgets::dot(ui, c);
                ui.label(t);
            });
            let version =
                self.live.as_ref().map(|s| s.version.clone()).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
            ui.label(RichText::new(format!("This server  v{version}")).strong().size(18.0).color(p(ui).text));
            if let Some(peer) = self.peer_version_line() {
                ui.label(RichText::new(peer).color(p(ui).muted));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if !installed {
                    if ui.add(egui::Button::new("Install service").fill(p(ui).accent)).clicked() {
                        action = Some(SvcAction::Install);
                    }
                } else {
                    if ui.button("Uninstall").clicked() {
                        action = Some(SvcAction::Uninstall);
                    }
                    if running {
                        if ui.button("Restart").clicked() {
                            action = Some(SvcAction::Restart);
                        }
                        if ui.button("Stop").clicked() {
                            action = Some(SvcAction::Stop);
                        }
                    } else if ui.button("Start").clicked() {
                        action = Some(SvcAction::Start);
                    }
                }
            });
            ui.label(
                RichText::new("Install, stop, and uninstall ask Windows to approve that one action. Restart goes through the running service.")
                    .color(p(ui).muted)
                    .size(11.0),
            );
            ui.add_space(8.0);
            let mut tray_login = crate::tray::login_enabled();
            if ui_widgets::checkbox(ui, &mut tray_login, "Start tray at Windows sign-in") {
                crate::tray::set_login(tray_login);
                self.status = if tray_login {
                    "The tray icon will start when you sign in to Windows.".into()
                } else {
                    "The tray icon will no longer start at sign-in.".into()
                };
            }
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Listening port: {} (TCP + UDP)", self.port)).color(p(ui).muted).size(12.0));
            ui.label(RichText::new(format!("Data folder: {}", self.data_dir.display())).color(p(ui).muted).size(12.0));
            ui.add_space(4.0);
            ui.label(RichText::new("Server key fingerprint").color(p(ui).muted).size(12.0));
            ui.horizontal(|ui| {
                let (text, can_copy) = self.fingerprint_line();
                if text.is_empty() {
                    ui_widgets::spinner(ui, p(ui).accent);
                    ui.label("Reading status from the service…");
                } else {
                    ui.label(RichText::new(&text).monospace().size(13.0));
                    if can_copy && ui_widgets::icon_button(ui, Icon::Copy, "Copy fingerprint") {
                        copy(&text);
                        self.status = "Fingerprint copied".into();
                    }
                }
            });
            ui.label(
                RichText::new("Clients pin this on first connect; compare it when setting up a new computer.")
                    .color(p(ui).muted)
                    .size(11.0),
            );
        });

        if let Some(action) = action {
            self.run_service_action(action);
        }
    }

    /// Asks Windows to elevate one service command. This window stays as it is.
    fn elevate_service(&mut self, args: &[&str]) {
        if run_elevated(args) {
            self.status = "Asked Windows to approve that action. This window stays open.".into();
        } else {
            self.status = "Windows did not start the elevated action.".into();
        }
    }

    fn ask_service(&mut self, req: crate::control::Request) {
        self.status = match crate::control::call(&req) {
            Some(response) => response.message,
            None => "The PJDesk service did not answer.".into(),
        };
    }

    fn run_service_action(&mut self, action: SvcAction) {
        if matches!(action, SvcAction::Restart) && self.control_ready {
            self.ask_service(crate::control::Request { cmd: "restart".into(), ..Default::default() });
            return;
        }
        if !self.can_write {
            let args: &[&str] = match action {
                SvcAction::Install => &["service", "install", "--firewall"],
                SvcAction::Uninstall => &["service", "uninstall"],
                SvcAction::Start => &["service", "start"],
                SvcAction::Stop => &["service", "stop"],
                SvcAction::Restart => &["service", "restart"],
            };
            self.elevate_service(args);
            return;
        }
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
            SvcAction::Restart => crate::service::restart().map(|_| "Service restarted."),
        };
        self.status = match result {
            Ok(msg) => msg.to_string(),
            Err(e) => format!("{e:#}"),
        };
        self.refresh();
    }

    fn fingerprint_line(&self) -> (String, bool) {
        if self.access == Access::Readable {
            let known = !self.fingerprint.contains("unknown");
            return (self.fingerprint.clone(), known && !self.fingerprint.is_empty());
        }
        if let Some(snap) = &self.live {
            if snap.fingerprint_known && !snap.fingerprint.is_empty() {
                return (snap.fingerprint.clone(), true);
            }
            if snap.fingerprint_known {
                return ("(the server has not created a key yet)".into(), false);
            }
        }
        if self.waiting_for_service() {
            return (String::new(), false);
        }
        ("Locked — open as administrator to view".into(), false)
    }

    fn updates_locked(&mut self, ui: &mut egui::Ui) {
        let mut check_now = false;
        let mut install_now = false;
        card(ui, |ui| {
            heading(ui, "UPDATES");
            let Some(snap) = self.live.clone() else {
                self.locked_line(ui);
                return;
            };
            if !snap.update_known {
                self.locked_line(ui);
                return;
            }
            let managed = self.control_ready;
            let mut auto = snap.update_auto;
            let changed = ui
                .add_enabled_ui(managed, |ui| ui_widgets::checkbox(ui, &mut auto, "Install server updates automatically"))
                .inner;
            if changed && managed {
                self.ask_service(crate::control::Request {
                    cmd: "set_update".into(),
                    auto: Some(auto),
                    ..Default::default()
                });
            }
            if !managed {
                ui.label(
                    RichText::new("Shown from the running service. Restart as administrator to change it.")
                        .color(p(ui).muted)
                        .size(12.0),
                );
            }
            if !snap.window_start.is_empty() || !snap.window_end.is_empty() {
                ui.label(format!("Maintenance window {} to {}", snap.window_start, snap.window_end));
            }
            let checked = if snap.last_check_at == 0 {
                "Last check: never".to_string()
            } else {
                format!("Last check: {}", format_local(snap.last_check_at))
            };
            let result = if snap.last_result.is_empty() { "no result yet" } else { snap.last_result.as_str() };
            ui.label(RichText::new(format!("{checked} · {result}")).color(p(ui).muted).size(12.0));
            if !snap.update_available.is_empty() {
                ui.label(format!("Available version: {}", snap.update_available));
            }
            if !snap.update_error.is_empty() {
                ui.colored_label(p(ui).warn, &snap.update_error);
            }
            if managed {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Check for updates now").clicked() {
                        check_now = true;
                    }
                    if !snap.update_available.is_empty() && ui.button("Install this update").clicked() {
                        install_now = true;
                    }
                });
                if self.update_start.is_empty() && !snap.window_start.is_empty() {
                    self.update_start = snap.window_start.clone();
                }
                if self.update_end.is_empty() && !snap.window_end.is_empty() {
                    self.update_end = snap.window_end.clone();
                }
                ui.horizontal(|ui| {
                    ui.label("Maintenance window");
                    ui.add(egui::TextEdit::singleline(&mut self.update_start).hint_text("any time").desired_width(70.0));
                    ui.label("to");
                    ui.add(egui::TextEdit::singleline(&mut self.update_end).hint_text("HH:MM").desired_width(70.0));
                    if ui.button("Save").clicked() {
                        self.ask_service(crate::control::Request {
                            cmd: "set_window".into(),
                            window_start: self.update_start.clone(),
                            window_end: self.update_end.clone(),
                            ..Default::default()
                        });
                    }
                });
            }
        });
        if check_now {
            self.status = "Checking for updates…".into();
            self.spawn_cloud_job(|| service_job(crate::control::Request { cmd: "check".into(), ..Default::default() }));
        } else if install_now {
            self.status = "Installing the update…".into();
            self.spawn_cloud_job(|| {
                service_job(crate::control::Request { cmd: "update_now".into(), ..Default::default() })
            });
        }
    }

    fn cloud_locked(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            heading(ui, "CLOUD SYNC & BACKUP SERVER");
            let Some(snap) = self.live.clone() else {
                self.locked_line(ui);
                return;
            };
            if !snap.cloud_known {
                self.locked_line(ui);
                return;
            }
            if !snap.cloud_on {
                ui.label("Cloud sync is off.");
                if !self.control_ready {
                    ui.label(RichText::new("Restart as administrator to sign in.").color(p(ui).muted).size(12.0));
                }
                return;
            }
            ui.horizontal(|ui| {
                let (color, text) = if snap.role == "active" {
                    let who =
                        if snap.active_name.is_empty() { "this server".to_string() } else { snap.active_name.clone() };
                    (p(ui).ok, format!("Primary: {who} uploads to the cloud (generation {}).", snap.epoch))
                } else {
                    let who = if snap.active_name.is_empty() {
                        "the other server".to_string()
                    } else {
                        snap.active_name.clone()
                    };
                    (p(ui).accent, format!("Standby. Primary: {who} (generation {}).", snap.epoch))
                };
                ui_widgets::dot(ui, color);
                ui.label(text);
            });
            if !snap.notice.is_empty() {
                ui.colored_label(p(ui).warn, &snap.notice);
            }
            if !snap.cloud_error.is_empty() {
                ui.colored_label(p(ui).warn, &snap.cloud_error);
            }
            if self.control_ready {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Become the primary").clicked() {
                        self.ask_service(crate::control::Request { cmd: "promote".into(), ..Default::default() });
                    }
                    if ui.button("Step down to standby").clicked() {
                        self.ask_service(crate::control::Request { cmd: "demote".into(), ..Default::default() });
                    }
                    if ui.button("Sync now").clicked() {
                        self.ask_service(crate::control::Request { cmd: "sync".into(), ..Default::default() });
                    }
                });
            } else {
                ui.label(
                    RichText::new("Shown from the running service. Restart as administrator to change cloud sync.")
                        .color(p(ui).muted)
                        .size(12.0),
                );
            }
        });
    }

    fn locked_line(&self, ui: &mut egui::Ui) {
        if self.waiting_for_service() {
            ui.horizontal(|ui| {
                ui_widgets::spinner(ui, p(ui).accent);
                ui.label("Reading status from the service…");
            });
        } else {
            ui.label("Locked — open as administrator to view.");
        }
    }

    /// Email of the PJDesk account this server is signed in as.
    fn signed_in_email(&self) -> String {
        if let Some(cloud) = &self.cloud {
            if !cloud.account.is_empty() {
                return cloud.account.clone();
            }
        }
        if self.access == Access::Readable {
            if let Some(cfg) = crate::cloud::CloudConfig::load(&self.data_dir) {
                if cfg.is_account() && !cfg.email.is_empty() {
                    return cfg.email;
                }
            }
        }
        self.live.as_ref().map(|s| s.account.clone()).filter(|s| !s.is_empty()).unwrap_or_default()
    }

    /// True when Cloud Sync is the advanced "your own bucket" path.
    fn using_own_bucket(&self) -> bool {
        self.access == Access::Readable
            && self.cloud_on
            && crate::cloud::CloudConfig::load(&self.data_dir).is_some_and(|cfg| !cfg.is_account())
    }

    /// PJDesk settings save when this window can write the data folder and a
    /// PJDesk account (or an own-bucket sync) is signed in. No second password.
    fn may_edit_settings(&self) -> bool {
        self.can_write && (!self.signed_in_email().is_empty() || self.using_own_bucket())
    }

    fn account_role_line(&self) -> String {
        let (role, active_name, peer_name) = if let Some(s) = &self.cloud {
            if !s.role.is_empty() {
                (s.role.clone(), s.active_name.clone(), s.peer_name.clone())
            } else if let Some(live) = &self.live {
                (live.role.clone(), live.active_name.clone(), live.peer_name.clone())
            } else {
                return String::new();
            }
        } else if let Some(live) = &self.live {
            (live.role.clone(), live.active_name.clone(), live.peer_name.clone())
        } else {
            return String::new();
        };
        if role.is_empty() {
            return String::new();
        }
        if role == "standby" {
            let who = if active_name.is_empty() { "the other server".to_string() } else { active_name };
            return format!("Standby of this account. Primary: {who}. Same account, no join code.");
        }
        let who = if active_name.is_empty() { "this server".to_string() } else { active_name };
        if peer_name.is_empty() {
            format!(
                "Primary: {who}. A second server that signs in with this account becomes the standby. No join code."
            )
        } else {
            format!("Primary: {who}. Standby: {peer_name}. Same account, no join code.")
        }
    }

    fn account_card(&mut self, ui: &mut egui::Ui) {
        let email = self.signed_in_email();
        let own_bucket = self.using_own_bucket();
        let role_line = self.account_role_line();
        let mut act: Option<CloudAct> = None;
        let busy = self.cloud_job.is_some();
        let can_act = self.can_write || self.control_ready;
        let readable = self.access == Access::Readable;
        let cloud_on = if readable {
            self.cloud_on
        } else {
            self.live.as_ref().is_some_and(|s| s.cloud_on) || !email.is_empty()
        };
        let choosing_bucket = self.cloud_form.own_bucket;
        card(ui, |ui| {
            heading(ui, "PJDESK ACCOUNT");
            if !readable && !self.control_ready {
                if email.is_empty() {
                    ui.label("Not signed in.");
                } else {
                    ui.label(RichText::new(format!("Signed in as {email}")).strong().size(18.0).color(p(ui).text));
                    if !role_line.is_empty() {
                        ui.label(&role_line);
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "You can look. Signing out, and saving PJDesk settings, needs this window to write the data folder. Restart as administrator for that. There is no separate PJDesk password.",
                    )
                    .color(p(ui).muted)
                    .size(12.0),
                );
                return;
            }
            if own_bucket {
                ui.label("This server uses your own bucket. A join code is only for that path.");
                ui.label(
                    RichText::new(
                        "A PJDesk account is the sign-in for Cloud Sync, the active or standby role, and My network. The other server uses the same account and does not need a join code.",
                    )
                    .color(p(ui).muted)
                    .size(12.0),
                );
                return;
            }
            if choosing_bucket {
                ui.label("Join codes are only for your own bucket, under Cloud Sync below.");
                ui.label(
                    RichText::new("A PJDesk account does not need a join code. The other server signs in with the same account and becomes the standby.")
                        .color(p(ui).muted)
                        .size(12.0),
                );
                return;
            }
            if !email.is_empty() && cloud_on {
                ui.label(RichText::new(format!("Signed in as {email}")).strong().size(20.0).color(p(ui).text));
                if !role_line.is_empty() {
                    ui.add_space(2.0);
                    ui.label(&role_line);
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "This one login is Cloud Sync and backup, this server's active or standby role, and My network (the address, the key fingerprint, and the slot). Allowed accounts and Require signed-in accounts follow this account. There is no separate server password.",
                    )
                    .color(p(ui).muted)
                    .size(12.0),
                );
                ui.add_space(8.0);
                ui.add_enabled_ui(can_act && !busy, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .button("Sign out")
                            .on_hover_text("Sign this server out of PJDesk Cloud. The cloud copy is kept.")
                            .clicked()
                        {
                            act = Some(CloudAct::Off);
                        }
                        if ui
                            .button("Switch account")
                            .on_hover_text("Sign out, then sign in with a different PJDesk account.")
                            .clicked()
                        {
                            act = Some(CloudAct::Switch);
                        }
                    });
                });
                if !can_act {
                    ui.label(
                        RichText::new("Restart as administrator to sign out. Windows protects the saved sign-in.")
                            .color(p(ui).muted)
                            .size(12.0),
                    );
                }
                return;
            }
            ui.label(
                RichText::new(
                    "Sign in once with your PJDesk account (the same login as pjdesk.com). That is Cloud Sync and backup, this server's role, and My network. A second server with the same account becomes the standby. No join code, and no separate server password.",
                )
                .color(p(ui).muted)
                .size(12.0),
            );
            ui.add_space(6.0);
            let online = self.online.len();
            ui.add_enabled_ui(can_act && !busy, |ui| {
                let f = &mut self.cloud_form;
                if let Some((from, written_at, readable_copy)) = f.ask.clone() {
                    if readable_copy {
                        let when = format_local(written_at);
                        ui.colored_label(
                            p(ui).warn,
                            format!("This account already holds {from}'s data (from {when}), and {online} computer(s) are connected to this server."),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::new("Use this server as the standby").fill(p(ui).accent))
                                .on_hover_text(format!("This server takes {from}'s key, accounts and computer list, and stays on standby. No join code."))
                                .clicked()
                            {
                                act = Some(CloudAct::Choose(crate::cloud::Choice::Join));
                            }
                            if ui.button("Use this server's data").on_hover_text("Replace the copy in the cloud with this server's data").clicked() {
                                act = Some(CloudAct::Choose(crate::cloud::Choice::Replace));
                            }
                            if ui.button("Cancel").clicked() {
                                act = Some(CloudAct::CancelAsk);
                            }
                        });
                    } else {
                        ui.colored_label(p(ui).warn, "This account's cloud copy was made with an older password, so it can't be opened.");
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new("Upload this server's data").fill(p(ui).accent)).clicked() {
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
                                egui::TextEdit::singleline(&mut f.acct_password).password(!f.show_acct_password).desired_width(270.0),
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
                        if ui.add(egui::Button::new("Sign in").fill(p(ui).accent)).clicked() {
                            act = Some(CloudAct::SignIn);
                        }
                        ui.label(RichText::new("No account yet?").color(p(ui).muted).size(12.0));
                        if ui.link("Create one at pjdesk.com").clicked() {
                            act = Some(CloudAct::CreateAccount);
                        }
                    });
                    if ui.link("Use your own bucket instead").clicked() {
                        f.own_bucket = true;
                    }
                }
            });
            if !can_act {
                ui.label(
                    RichText::new("Restart as administrator to sign in. Windows protects the data folder. There is no separate PJDesk password.")
                        .color(p(ui).muted)
                        .size(12.0),
                );
            }
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Talking to the cloud…").color(p(ui).muted).size(12.0));
                });
            }
        });
        if let Some(act) = act {
            self.run_cloud_action(act);
        }
    }

    fn accounts_card(&mut self, ui: &mut egui::Ui) {
        if self.access != Access::Readable {
            card(ui, |ui| {
                heading(ui, "OLDER LOCAL ACCOUNTS");
                self.locked_line(ui);
            });
            return;
        }
        let may_edit = self.may_edit_settings();
        card(ui, |ui| {
            heading(ui, "PJDESK ACCOUNT");
            if self.signed_in_email().is_empty() {
                ui.label(RichText::new("Sign in with your pjdesk.com account.").strong().color(p(ui).text));
            } else {
                ui.label(RichText::new(format!("Signed in as {}", self.signed_in_email())).strong().color(p(ui).text));
            }
            ui.label(
                RichText::new(
                    "Older local accounts in accounts.json are left on this computer and are not used to sign in. Connections keep working.",
                )
                .color(p(ui).muted)
                .size(12.0),
            );
            ui.add_space(6.0);

            if self.accounts.is_empty() {
                ui.label(
                    RichText::new("None. A new server does not need these. Computers sign in with the PJDesk account.")
                        .color(p(ui).muted),
                );
            }
            let mut remove: Option<String> = None;
            let mut toggle_disabled: Option<(String, bool)> = None;
            for a in &self.accounts {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&a.username).strong().color(p(ui).text));
                    if a.is_admin {
                        ui.label(RichText::new("admin").color(p(ui).accent).size(11.0));
                    }
                    if a.disabled {
                        ui.label(RichText::new("disabled").color(p(ui).warn).size(11.0));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_enabled_ui(may_edit, |ui| {
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

            if !may_edit && !self.accounts.is_empty() {
                let why = if !self.can_write {
                    "Restart as administrator to change these. Windows protects the data folder. There is no separate PJDesk password."
                } else {
                    "Sign in with your PJDesk account above to change these. There is no separate server password."
                };
                ui.label(RichText::new(why).color(p(ui).muted).size(12.0));
            }

            ui.add_space(8.0);
            ui.label(RichText::new("Manage the account at pjdesk.com.").color(p(ui).muted).size(12.0));

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
        if self.access != Access::Readable {
            self.cloud_locked(ui);
            return;
        }
        let mut act: Option<CloudAct> = None;
        let busy = self.cloud_job.is_some();
        let dns = "your server's name".to_string();
        let online = self.online.len();
        let can_write = self.can_write;
        card(ui, |ui| {
            heading(ui, "CLOUD SYNC & BACKUP SERVER");
            if self.cloud_on {
                let s = self.cloud.clone().unwrap_or_default();
                let is_account = !s.account.is_empty();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let fresh = s.updated_at + 90 >= now;
                ui.horizontal(|ui| {
                    if !fresh {
                        ui_widgets::dot(ui, p(ui).warn);
                        ui.label("Sync is on, but the server isn't running, so nothing is syncing.");
                    } else if s.role == "active" {
                        ui_widgets::dot(ui, p(ui).ok);
                        let who =
                            if s.active_name.is_empty() { "this server".to_string() } else { s.active_name.clone() };
                        ui.label(format!("Primary: {who} uploads to the cloud (generation {}).", s.epoch));
                    } else {
                        ui_widgets::dot(ui, p(ui).accent);
                        let who = if s.active_name.is_empty() {
                            "the other server".to_string()
                        } else {
                            s.active_name.clone()
                        };
                        ui.label(format!("Standby. Primary: {who} (generation {}).", s.epoch));
                    }
                });
                if let Some(peer) = self.peer_version_line() {
                    ui.label(RichText::new(peer).color(p(ui).muted));
                }
                if !s.notice.is_empty() {
                    ui.colored_label(p(ui).warn, s.notice.clone());
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
                ui.label(RichText::new(format!("{detail} · {}", s.bucket)).color(p(ui).muted).size(12.0));
                let signed_out = s.error.contains("sign in again");
                if !s.error.is_empty() {
                    ui.horizontal(|ui| {
                        glyph(ui, Icon::Alert, p(ui).warn);
                        ui.colored_label(p(ui).warn, &s.error);
                    });
                }
                ui.add_space(6.0);
                ui.add_enabled_ui(can_write && !busy, |ui| {
                    ui.horizontal(|ui| {
                        if signed_out && ui.add(egui::Button::new("Sign in again").fill(p(ui).accent)).clicked() {
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
                                .on_hover_text("Only for your own bucket. A PJDesk account does not use a join code.")
                                .clicked()
                        {
                            act = Some(CloudAct::CopyCode);
                        }
                        if !is_account
                            && ui
                                .button("Turn off")
                                .on_hover_text("Stop syncing on this server (the cloud copy is kept)")
                                .clicked()
                        {
                            act = Some(CloudAct::Off);
                        }
                    });
                });
                if self.cloud_form.confirm_promote {
                    ui.add_space(4.0);
                    ui.colored_label(
                        p(ui).warn,
                        "Make this server the primary? It downloads the latest copy first. The other server becomes the standby the next time it checks, even if that server is offline right now.",
                    );
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Make this the primary").fill(p(ui).accent)).clicked() {
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
                        p(ui).warn,
                        "Step down to standby? This server will stop uploading. The other server stays the primary until you make it the primary there.",
                    );
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Step down").fill(p(ui).accent)).clicked() {
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
                    format!(
                        "The other server: install PJDesk Server and sign in with the same account ({}). It becomes the standby. No join code.",
                        s.account
                    )
                } else {
                    "The other server, for your own bucket: install PJDesk Server, choose Your own bucket, then This is a backup server, and paste the join code. It becomes the standby.".to_string()
                };
                ui.label(
                    RichText::new(format!(
                        "{how_to_add} To switch, choose Make this the primary on that server (or the tray, or pjdesk-server cloud promote). Computers connect to whichever server {dns} points to, so if the new primary is on a different network, point DNS and the port forward at it."
                    ))
                    .color(p(ui).muted)
                    .size(11.0),
                );
            } else if self.cloud_form.own_bucket {
                ui.label(
                    RichText::new(
                        "Your own bucket is the advanced path. A join code pairs the standby. A PJDesk account, above, does not need one.",
                    )
                    .color(p(ui).muted)
                    .size(12.0),
                );
                ui.add_space(6.0);
                if ui.button("Use a PJDesk account instead").clicked() {
                    self.cloud_form.own_bucket = false;
                }
                ui.add_space(4.0);
                let f = &mut self.cloud_form;
                ui.add_enabled_ui(can_write && !busy, |ui| {
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
                                ui.colored_label(p(ui).warn, "This bucket already holds a PJDesk server copy. Replace it with this server's?");
                                ui.horizontal(|ui| {
                                    if ui.add(egui::Button::new("Replace it").fill(p(ui).accent)).clicked() {
                                        act = Some(CloudAct::Setup { replace: true });
                                    }
                                    if ui.button("Cancel").clicked() {
                                        f.confirm_replace = false;
                                    }
                                });
                            } else if ui.add(egui::Button::new("Start syncing").fill(p(ui).accent)).clicked() {
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
                                    .color(p(ui).muted)
                                    .size(11.0),
                            );
                            ui.add_space(4.0);
                            if f.confirm_join {
                                ui.colored_label(p(ui).warn, format!("{online} computer(s) are connected to this server right now. Join anyway?"));
                                ui.horizontal(|ui| {
                                    if ui.add(egui::Button::new("Join anyway").fill(p(ui).accent)).clicked() {
                                        act = Some(CloudAct::Join);
                                    }
                                    if ui.button("Cancel").clicked() {
                                        f.confirm_join = false;
                                    }
                                });
                            } else if ui.add(egui::Button::new("Join").fill(p(ui).accent)).clicked() {
                                if online > 0 {
                                    f.confirm_join = true;
                                } else {
                                    act = Some(CloudAct::Join);
                                }
                            }
                        }
                });
            } else {
                ui.label(
                    RichText::new(
                        "Sign in with your PJDesk account at the top of this window. That one login turns on Cloud Sync, sets this server as active or standby, and registers it on My network.",
                    )
                    .color(p(ui).muted)
                    .size(12.0),
                );
            }
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Talking to the cloud…").color(p(ui).muted).size(12.0));
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
                if self.via_service() {
                    self.spawn_cloud_job(move || service_job(crate::control::Request {
                        cmd: "sign_in".into(),
                        email,
                        password,
                        ..Default::default()
                    }));
                    return;
                }
                self.spawn_cloud_job(move || {
                    let cfg = cloud::sign_in(&cloud::default_api(), &email, &password)?;
                    finish_account(&dir, cfg, online, None)
                });
            }
            CloudAct::Choose(choice) => {
                self.cloud_form.ask = None;
                let pending = std::mem::take(&mut self.cloud_form.service_pending);
                if !pending.is_empty() {
                    let choice = match choice {
                        crate::cloud::Choice::Join => "join",
                        crate::cloud::Choice::Replace => "replace",
                    };
                    self.spawn_cloud_job(move || {
                        service_job(crate::control::Request {
                            cmd: "sign_in_choose".into(),
                            pending,
                            choice: choice.into(),
                            ..Default::default()
                        })
                    });
                    return;
                }
                let Some(cfg) = self.cloud_form.pending.take() else { return };
                self.spawn_cloud_job(move || finish_account(&dir, cfg, online, Some(choice)));
            }
            CloudAct::CancelAsk => {
                self.cloud_form.ask = None;
                let pending = std::mem::take(&mut self.cloud_form.service_pending);
                if !pending.is_empty() {
                    self.ask_service(crate::control::Request {
                        cmd: "sign_in_cancel".into(),
                        pending,
                        ..Default::default()
                    });
                }
                if let Some(cfg) = self.cloud_form.pending.take() {
                    std::thread::spawn(move || cloud::discard_sign_in(&cfg));
                }
            }
            CloudAct::CreateAccount => open_url(&format!("{}/account/#create", cloud::default_api())),
            CloudAct::SignInAgain => {
                if !self.can_write && !self.control_ready {
                    self.status = "Restart as administrator to sign in again. Windows protects the saved sign-in. There is no separate PJDesk password.".into();
                    return;
                }
                if self.via_service() {
                    if self.cloud_form.acct_email.trim().is_empty() {
                        self.cloud_form.acct_email = self.signed_in_email();
                    }
                    self.ask_service(crate::control::Request { cmd: "sign_out".into(), ..Default::default() });
                    self.cloud_form.own_bucket = false;
                    self.status = "Enter your PJDesk account password to sign in again.".into();
                    return;
                }
                if let Some(cfg) = CloudConfig::load(&dir) {
                    self.cloud_form.acct_email = cfg.email.clone();
                }
                self.cloud_form.own_bucket = false;
                cloud::turn_off(&dir);
                self.status = "Enter your PJDesk account password to sign in again.".into();
                self.refresh();
            }
            CloudAct::SyncNow => {
                if self.via_service() {
                    self.ask_service(crate::control::Request { cmd: "sync".into(), ..Default::default() });
                    return;
                }
                self.status = match crate::requests::push(&dir, crate::requests::SYNC_FILE, "now") {
                    Ok(()) => "Syncing now.".into(),
                    Err(e) => format!("Could not ask the server to sync: {e}"),
                };
            }
            CloudAct::Promote => {
                if self.via_service() {
                    self.ask_service(crate::control::Request { cmd: "promote".into(), ..Default::default() });
                    return;
                }
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
                if self.via_service() {
                    self.ask_service(crate::control::Request { cmd: "demote".into(), ..Default::default() });
                    return;
                }
                if crate::cloud::status_is_fresh(&dir) {
                    self.status = match crate::requests::push(&dir, crate::requests::DEMOTE_FILE, "now") {
                        Ok(()) => "Asking the running server to step down to standby.".into(),
                        Err(e) => format!("Could not ask the server to step down: {e}"),
                    };
                } else {
                    self.spawn_cloud_job(move || {
                        crate::cloud::demote(&dir)?;
                        Ok(JobDone::Message("This server stepped down to standby and will not upload.".into()))
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
            CloudAct::Off | CloudAct::Switch => {
                if !self.can_write && !self.control_ready {
                    self.status = "Restart as administrator to sign out. Windows protects the saved sign-in. There is no separate PJDesk password.".into();
                    return;
                }
                if self.via_service() {
                    let switching = matches!(act, CloudAct::Switch);
                    self.ask_service(crate::control::Request { cmd: "sign_out".into(), ..Default::default() });
                    self.cloud_form.own_bucket = false;
                    self.cloud_form.acct_password.clear();
                    if switching {
                        self.status = "Signed out. Sign in with the PJDesk account this server should use.".into();
                    }
                    return;
                }
                let switching = matches!(act, CloudAct::Switch);
                let was_account =
                    self.cloud.as_ref().is_some_and(|s| !s.account.is_empty()) || !self.signed_in_email().is_empty();
                std::thread::spawn({
                    let dir = dir.clone();
                    move || cloud::sign_out(&dir)
                });
                // Remove the settings right away so the window updates.
                cloud::turn_off(&dir);
                self.cloud_form.own_bucket = false;
                self.cloud_form.acct_password.clear();
                self.status = if switching {
                    "Signed out. Sign in with the PJDesk account this server should use.".into()
                } else if was_account {
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
            Ok(JobDone::ServiceAsk { pending, from, written_at, readable }) => {
                self.cloud_form.service_pending = pending;
                self.cloud_form.ask = Some((from, written_at, readable));
                self.status = "Choose which copy to keep.".into();
            }
            Err(e) => self.status = e,
        }
    }

    fn network_card(&mut self, ui: &mut egui::Ui) {
        if self.access != Access::Readable && self.control_ready {
            self.network_managed(ui);
            return;
        }
        if self.access != Access::Readable {
            card(ui, |ui| {
                heading(ui, "SIGNED-IN ACCOUNTS");
                if let Some(live) = &self.live {
                    let on = live.require_accounts;
                    ui.label(if on {
                        "Require signed-in accounts is on."
                    } else {
                        "Require signed-in accounts is off. Already-registered computers keep connecting."
                    });
                    let waiting: Vec<_> = live.devices.iter().filter(|d| !d.signed_in).cloned().collect();
                    ui.add_space(6.0);
                    ui.label(RichText::new("Not signed in yet").strong());
                    if waiting.is_empty() {
                        ui.label(RichText::new("No computer is waiting to sign in.").color(p(ui).muted));
                    } else {
                        for d in waiting {
                            ui.label(format!("{}  {}", pjdesk_proto::format_id(&d.id), d.hostname));
                        }
                    }
                }
                self.locked_line(ui);
            });
            return;
        }
        let mut policy = crate::access::Policy::load(&self.data_dir);
        let unsigned: Vec<(String, String)> = self
            .devices
            .iter()
            .filter(|d| !d.signed_in)
            .map(|d| (pjdesk_proto::format_id(&d.id), d.hostname.clone()))
            .collect();
        let mut require = policy.require_accounts;
        let mut changed = false;
        self.poll_public_address();
        let saved_address = crate::public_addr::load(&self.data_dir).map(|saved| saved.address).unwrap_or_default();
        let dirty = self.pub_addr.trim() != saved_address.trim();
        let mut publish = false;
        let mut replace = false;
        let may_edit = self.may_edit_settings();
        let lock_reason = if !self.can_write {
            "Restart as administrator to save this. Windows protects the data folder. There is no separate PJDesk password."
        } else {
            "Sign in with your PJDesk account above to change this. There is no separate server password."
        };
        card(ui, |ui| {
            heading(ui, "PUBLIC ADDRESS");
            ui.label(RichText::new(
                "The address computers use to reach this server. A guess is filled in from the name they already use, or from this server's public address.",
            ).color(p(ui).muted));
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.pub_addr)
                    .hint_text("host or host:port")
                    .desired_width(360.0)
                    .font(egui::TextStyle::Monospace),
            );
            ui.label(RichText::new(crate::public_addr::IpWatch::load(&self.data_dir).line()).color(p(ui).muted).size(12.0));
            let hostname = crate::public_addr::is_hostname(self.pub_addr.trim());
            let mut watch = crate::public_addr::IpWatch::load(&self.data_dir);
            let mut auto = watch.enabled(self.pub_addr.trim());
            if hostname {
                ui.label(RichText::new(
                    "This address is a hostname, so a public IP change does not replace it.",
                ).color(p(ui).muted).size(12.0));
            }
            ui.add_enabled_ui(may_edit && !hostname, |ui| {
                if ui.checkbox(&mut auto, "Update the address when my public IP changes").changed() {
                    watch.auto = Some(auto);
                    watch.save(&self.data_dir);
                }
            });
            if !self.pub_addr_note.is_empty() {
                let color = if self.pub_addr_needs_confirm
                    || crate::public_addr::is_private_address(self.pub_addr.trim()) && !self.pub_addr_confirmed
                {
                    p(ui).warn
                } else {
                    p(ui).muted
                };
                ui.label(RichText::new(&self.pub_addr_note).color(color).size(12.0));
            }
            if may_edit && self.pub_addr_needs_confirm && ui.button("Replace the server on My network").clicked() {
                publish = true;
                replace = true;
            } else if may_edit
                && !self.pub_addr_confirmed
                && !self.pub_addr.trim().is_empty()
                && ui.button("Confirm this address").clicked()
            {
                publish = true;
            } else if may_edit && dirty && !self.pub_addr.trim().is_empty() && ui.button("Update My network").clicked()
            {
                publish = true;
            }
            if !may_edit {
                ui.label(RichText::new(lock_reason).color(p(ui).muted).size(12.0));
            }
            ui.add_space(12.0);
            heading(ui, "SIGNED-IN ACCOUNTS");
            ui.label(RichText::new(
                "Off by default, so an update does not take a computer offline. Turn this on only after every computer below has signed in. This follows the PJDesk account signed in above.",
            ).color(p(ui).muted));
            ui.add_space(6.0);
            ui.add_enabled_ui(may_edit, |ui| {
                if ui.checkbox(&mut require, "Require signed-in accounts").changed() {
                    changed = true;
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("Not signed in yet").strong());
            if unsigned.is_empty() {
                ui.label(
                    RichText::new("Every registered computer has signed in, or none have registered.")
                        .color(p(ui).muted),
                );
            } else {
                for (id, name) in &unsigned {
                    ui.horizontal(|ui| {
                        ui_widgets::dot(ui, p(ui).warn);
                        ui.label(format!("{id}  {name}"));
                    });
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Allowed accounts").strong());
            if policy.allowed_accounts.is_empty() {
                ui.label(RichText::new("Any account with a token for this server. Invite or remove people on pjdesk.com → Account → My network.").color(p(ui).muted));
            } else {
                for email in &policy.allowed_accounts {
                    ui.label(email);
                }
            }
            if policy.public_key.is_empty() {
                ui.label(
                    RichText::new("The cloud signing key is saved here the next time this server checks in.")
                        .color(p(ui).muted)
                        .size(12.0),
                );
            } else {
                ui.label(RichText::new("Cloud signing key is pinned.").color(p(ui).muted).size(12.0));
            }
            ui.add_space(10.0);
            ui.label(RichText::new("Network keys").strong());
            ui.label(
                RichText::new("A copy-paste key joins this network. It is shown once and is not saved here.")
                    .color(p(ui).muted),
            );
            if may_edit && ui.button("Create a network key").clicked() {
                self.nk_open = true;
                self.nk_blob.clear();
                self.nk_pass.clear();
            }
            if !may_edit {
                ui.label(RichText::new(lock_reason).color(p(ui).muted).size(12.0));
            }
            self.ensure_invites();
            if self.nk_invites.is_empty() {
                ui.label(RichText::new("No network keys yet.").color(p(ui).muted));
            }
            let mut revoke: Option<String> = None;
            for inv in &self.nk_invites {
                let state = if inv.revoked_at.is_some() {
                    "revoked"
                } else if inv.mode == "embedded" {
                    "self-contained"
                } else {
                    "passphrase"
                };
                let limit = match inv.max_uses {
                    Some(max) => format!("{} of {max} uses", inv.uses),
                    None => "no use limit".into(),
                };
                let exp = match inv.expires_at {
                    Some(t) => format!(" · expires {}", crate::registry::format_local(t)),
                    None => String::new(),
                };
                let bound =
                    if inv.bound_email.is_empty() { String::new() } else { format!(" · only {}", inv.bound_email) };
                ui.label(format!("{state} · {limit}{exp}{bound}"));
                for use_row in &inv.history {
                    let who = if use_row.device_name.is_empty() { "A device" } else { use_row.device_name.as_str() };
                    let when = if use_row.used_at == 0 {
                        String::new()
                    } else {
                        format!(" · {}", crate::registry::format_local(use_row.used_at))
                    };
                    ui.label(RichText::new(format!("  {who} · {}{when}", use_row.email)).color(p(ui).muted).size(12.0));
                }
                if inv.revoked_at.is_none() && may_edit && ui.small_button("Revoke").clicked() {
                    revoke = Some(inv.id.clone());
                }
            }
            if let Some(id) = revoke {
                self.spawn_revoke_key(id);
            }
        });
            if changed {
            policy.require_accounts = require;
            policy.require_chosen = true;
            if let Err(e) = policy.save(&self.data_dir) {
                self.status = format!("Could not save the signed-in accounts setting: {e}");
            } else {
                let dir = self.data_dir.clone();
                std::thread::spawn(move || crate::cloud::push_require_accounts(&dir, require));
            }
        }
        if publish {
            self.publish_public_address(replace);
        }
    }

    fn network_managed(&mut self, ui: &mut egui::Ui) {
        if self.pub_addr.is_empty() {
            if let Some(live) = &self.live {
                if !live.public_address.is_empty() {
                    self.pub_addr = live.public_address.clone();
                    self.pub_addr_note = live.public_note.clone();
                    self.pub_addr_needs_confirm = live.public_needs_confirm;
                    self.pub_addr_confirmed = live.public_confirmed;
                }
            }
        }
        let live = self.live.clone();
        let mut require = live.as_ref().map(|s| s.require_accounts).unwrap_or(false);
        let watch_known = live.as_ref().is_some_and(|s| s.public_watch_known);
        let mut watch = live.as_ref().is_some_and(|s| s.public_watch);
        let ip_line = live.as_ref().map(|s| s.public_ip_line.clone()).unwrap_or_default();
        let hostname = crate::public_addr::is_hostname(self.pub_addr.trim());
        let mut changed = false;
        let mut watch_changed = false;
        let mut publish = false;
        let mut replace = false;
        let address = self.pub_addr.clone();
        card(ui, |ui| {
            heading(ui, "PUBLIC ADDRESS");
            ui.label(RichText::new(
                "Saved by the PJDesk service. The address computers use to reach this server.",
            ).color(p(ui).muted));
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.pub_addr)
                    .hint_text("host or host:port")
                    .desired_width(360.0)
                    .font(egui::TextStyle::Monospace),
            );
            if !ip_line.is_empty() {
                ui.label(RichText::new(&ip_line).color(p(ui).muted).size(12.0));
            }
            if hostname {
                ui.label(RichText::new(
                    "This address is a hostname, so a public IP change does not replace it.",
                ).color(p(ui).muted).size(12.0));
            } else if watch_known && ui.checkbox(&mut watch, "Update the address when my public IP changes").changed() {
                watch_changed = true;
            }
            if !self.pub_addr_note.is_empty() {
                ui.label(RichText::new(&self.pub_addr_note).color(p(ui).muted).size(12.0));
            }
            if self.pub_addr_needs_confirm && ui.button("Replace the server on My network").clicked() {
                publish = true;
                replace = true;
            } else if !self.pub_addr.trim().is_empty() && self.pub_addr.trim() != address.trim() && ui.button("Update My network").clicked()
            {
                publish = true;
            }
            ui.add_space(12.0);
            heading(ui, "SIGNED-IN ACCOUNTS");
            ui.add_space(6.0);
            if ui.checkbox(&mut require, "Require signed-in accounts").changed() {
                changed = true;
            }
            ui.label(RichText::new(
                "Off by default, so an update does not take a computer offline. Turn this on only after every computer has signed in.",
            ).color(p(ui).muted).size(12.0));
            if let Some(live) = &live {
                let waiting: Vec<_> = live.devices.iter().filter(|d| !d.signed_in).cloned().collect();
                ui.add_space(6.0);
                ui.label(RichText::new("Not signed in yet").strong());
                if waiting.is_empty() {
                    ui.label(RichText::new("No computer is waiting to sign in.").color(p(ui).muted));
                } else {
                    for d in waiting {
                        ui.label(format!("{}  {}", pjdesk_proto::format_id(&d.id), d.hostname));
                    }
                }
            }
        });
        if changed {
            self.ask_service(crate::control::Request {
                cmd: "set_require".into(),
                require_accounts: Some(require),
                ..Default::default()
            });
        }
        if watch_changed {
            self.ask_service(crate::control::Request {
                cmd: "set_watch".into(),
                watch_auto: Some(watch),
                ..Default::default()
            });
        }
        if publish {
            self.ask_service(crate::control::Request {
                cmd: "set_address".into(),
                address: self.pub_addr.trim().to_string(),
                replace,
                ..Default::default()
            });
        }
    }

    fn publish_public_address(&mut self, replace: bool) {
        let address = self.pub_addr.trim().to_string();
        if address.is_empty() {
            self.pub_addr_note = "Type the public address computers use to reach this server.".into();
            return;
        }
        let mut record = crate::public_addr::load(&self.data_dir).unwrap_or(crate::public_addr::PublicAddress {
            address: String::new(),
            confirmed: false,
            source: String::new(),
            needs_confirm: false,
            note: String::new(),
        });
        if record.address.trim() != address {
            record.source = "manual".into();
        }
        record.address = address;
        record.confirmed = true;
        record.needs_confirm = false;
        let signed_in = crate::cloud::CloudConfig::load(&self.data_dir)
            .is_some_and(|cfg| cfg.is_account() && !cfg.token.is_empty());
        record.note = if signed_in {
            "Publishing this address…".into()
        } else {
            "Sign in with your PJDesk account above to publish this address.".into()
        };
        crate::public_addr::save(&self.data_dir, &record);
        self.pub_addr_confirmed = true;
        self.pub_addr_needs_confirm = false;
        self.pub_addr_note = record.note.clone();
        if !signed_in {
            return;
        }
        let dir = self.data_dir.clone();
        let online = self.online.len();
        std::thread::spawn(move || crate::cloud::publish_network(&dir, online, replace));
    }

    fn ensure_invites(&mut self) {
        if self.nk_loaded
            || self.nk_list_rx.is_some()
            || self.preview_network
            || self.preview_netkey
            || !self.may_edit_settings()
        {
            return;
        }
        self.nk_loaded = true;
        let dir = self.data_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::cloud::list_network_keys(&dir).map_err(|e| e.to_string()));
        });
        self.nk_list_rx = Some(rx);
    }

    fn spawn_revoke_key(&mut self, id: String) {
        let dir = self.data_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.nk_busy = true;
        std::thread::spawn(move || {
            let result = crate::cloud::revoke_network_key(&dir, &id)
                .map(|()| crate::cloud::CreatedKey { blob: String::new(), embedded: false })
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.nk_rx = Some(rx);
        self.nk_loaded = false;
    }

    fn spawn_create_key(&mut self) {
        if self.nk_busy {
            return;
        }
        let passphrase_mode = self.nk_passphrase_mode;
        let pass = std::mem::take(&mut self.nk_pass);
        if passphrase_mode && pass.chars().count() < 8 {
            self.nk_pass = pass;
            self.nk_error = "Use a passphrase of at least 8 characters.".into();
            return;
        }
        let days = match self.nk_days.trim() {
            "" => None,
            text => match text.parse::<u64>() {
                Ok(n) if n >= 1 => Some(n),
                _ => {
                    self.nk_pass = pass;
                    self.nk_error = "Expiry has to be a number of days, or empty.".into();
                    return;
                }
            },
        };
        let max_uses = match self.nk_max.trim() {
            "" => None,
            text => match text.parse::<u32>() {
                Ok(n) if n >= 1 => Some(n),
                _ => {
                    self.nk_pass = pass;
                    self.nk_error = "Maximum uses has to be at least 1, or leave it empty.".into();
                    return;
                }
            },
        };
        let bound = self.nk_bound.clone();
        let dir = self.data_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.nk_busy = true;
        self.nk_error.clear();
        std::thread::spawn(move || {
            let pass = pass;
            let result = crate::cloud::create_network_key(
                &dir,
                if passphrase_mode { Some(pass.as_str()) } else { None },
                days,
                max_uses,
                &bound,
            )
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.nk_rx = Some(rx);
    }

    fn poll_netkey(&mut self) {
        if let Some(rx) = &self.nk_list_rx {
            if let Ok(result) = rx.try_recv() {
                self.nk_list_rx = None;
                match result {
                    Ok(list) => self.nk_invites = list,
                    Err(e) => self.nk_error = e,
                }
            }
        }
        let Some(rx) = &self.nk_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.nk_rx = None;
        self.nk_busy = false;
        match result {
            Ok(created) if created.blob.is_empty() => {
                self.nk_loaded = false;
                self.nk_invites.clear();
            }
            Ok(created) => {
                self.nk_embedded = created.embedded;
                self.nk_blob = created.blob;
                self.nk_open = true;
                self.nk_loaded = false;
                self.nk_invites.clear();
            }
            Err(e) => self.nk_error = e,
        }
    }

    fn netkey_dialog(&mut self, ui: &mut egui::Ui) {
        if !self.nk_open {
            return;
        }
        let mut open = self.nk_open;
        let mut create = false;
        let mut close = false;
        let showing = !self.nk_blob.is_empty();
        egui::Window::new("Network key")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.set_min_width(420.0);
                if showing {
                    if self.nk_embedded {
                        ui.colored_label(
                            p(ui).warn,
                            "Anyone who has this network key can join. Share it like a password.",
                        );
                    } else {
                        ui.label("The passphrase is not part of this key. Share it separately.");
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Copy this now. It is shown once and is not saved here.").color(p(ui).muted),
                    );
                    ui.add(
                        egui::TextEdit::multiline(&mut self.nk_blob)
                            .desired_rows(5)
                            .desired_width(400.0)
                            .font(egui::TextStyle::Monospace)
                            .interactive(false),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Copy").clicked() {
                            copy(&self.nk_blob);
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                } else {
                    ui.label("This key contains the server address and a way to enroll. It is encrypted.");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.nk_passphrase_mode, true, "Passphrase");
                        ui.selectable_value(&mut self.nk_passphrase_mode, false, "Self-contained");
                    });
                    if self.nk_passphrase_mode {
                        ui.add_space(6.0);
                        ui.label("Passphrase");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.nk_pass)
                                    .password(!self.nk_show)
                                    .hint_text("At least 8 characters")
                                    .desired_width(280.0),
                            );
                            let label = if self.nk_show { "Hide" } else { "Show" };
                            if ui.button(label).clicked() {
                                self.nk_show = !self.nk_show;
                            }
                        });
                    } else {
                        ui.add_space(6.0);
                        ui.colored_label(
                            p(ui).warn,
                            "Anyone who has this network key can join. Share it like a password.",
                        );
                    }
                    ui.add_space(6.0);
                    ui.label("Expires in days (optional)");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.nk_days)
                            .hint_text("Leave empty for no expiry")
                            .desired_width(200.0),
                    );
                    ui.label("Maximum uses (optional)");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.nk_max)
                            .hint_text("Leave empty for no limit")
                            .desired_width(200.0),
                    );
                    ui.label("Limit to this account (optional)");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.nk_bound)
                            .hint_text("ada@example.com")
                            .desired_width(280.0),
                    );
                    if !self.nk_error.is_empty() {
                        ui.colored_label(p(ui).warn, &self.nk_error);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        let label = if self.nk_busy { "Creating…" } else { "Create network key" };
                        if ui.add_enabled(!self.nk_busy, egui::Button::new(label)).clicked() {
                            create = true;
                        }
                    });
                }
            });
        if create {
            self.spawn_create_key();
        }
        if !open || close {
            self.nk_open = false;
            self.nk_blob.clear();
            self.nk_pass.clear();
            self.nk_error.clear();
        } else {
            self.nk_open = open;
        }
    }

    fn devices_card(&mut self, ui: &mut egui::Ui) {
        let locked = self.access != Access::Readable;
        if locked && self.live.as_ref().map(|s| s.devices.is_empty()).unwrap_or(true) {
            card(ui, |ui| {
                heading(ui, "REGISTERED COMPUTERS");
                self.locked_line(ui);
            });
            return;
        }
        let rows = self.device_rows();
        let mut remove: Option<String> = None;
        let mut update: Option<String> = None;
        let mut update_all = false;
        let online = rows.iter().filter(|r| r.online).count();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                heading(ui, &format!("REGISTERED COMPUTERS ({}, {online} online)", rows.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !locked
                        && online > 0
                        && ui
                            .small_button("Update all online")
                            .on_hover_text("Tell every online computer to update now")
                            .clicked()
                    {
                        update_all = true;
                    }
                });
            });
            if locked {
                ui.label(
                    RichText::new("From the running service. Restart as administrator to change this list.")
                        .color(p(ui).muted)
                        .size(12.0),
                );
            }
            if rows.is_empty() {
                ui.label(RichText::new("No computers have registered yet.").color(p(ui).muted));
            }
            ui.horizontal(|ui| {
                ui.add_sized(
                    [130.0, 16.0],
                    egui::Label::new(RichText::new("ID").size(11.0).color(p(ui).muted).strong()),
                );
                ui.add_sized(
                    [140.0, 16.0],
                    egui::Label::new(RichText::new("Name").size(11.0).color(p(ui).muted).strong()),
                );
                ui.add_sized(
                    [150.0, 16.0],
                    egui::Label::new(RichText::new("Version").size(11.0).color(p(ui).muted).strong()),
                );
                ui.label(RichText::new("Status").size(11.0).color(p(ui).muted).strong());
            });
            egui::ScrollArea::vertical().max_height(220.0).id_salt("devices").show(ui, |ui| {
                for d in &rows {
                    let behind = update_available(self.latest_release.as_deref(), &d.version);
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [130.0, 18.0],
                            egui::Label::new(RichText::new(pjdesk_proto::format_id(&d.id)).monospace().color(p(ui).text)),
                        );
                        ui.add_sized([140.0, 18.0], egui::Label::new(RichText::new(&d.hostname).color(p(ui).text)));
                        ui.add_sized(
                            [150.0, 18.0],
                            egui::Label::new(RichText::new(version_text(&d.version)).color(p(ui).text)),
                        );
                        if behind {
                            ui.label(
                                RichText::new("update available")
                                    .size(11.0)
                                    .color(p(ui).warn)
                                    .background_color(p(ui).elev),
                            );
                        }
                        let seen = if d.last_seen == 0 {
                            String::new()
                        } else if d.first_seen > 0 {
                            format!("Last seen {} ({})\nFirst registered {}", format_local(d.last_seen), ago(d.last_seen), format_local(d.first_seen))
                        } else {
                            format!("Last seen {}", format_local(d.last_seen))
                        };
                        if !d.signed_in {
                            ui.label(RichText::new("not signed in").color(p(ui).warn).size(11.0));
                        }
                        if d.online {
                            ui.horizontal(|ui| {
                                ui_widgets::dot(ui, p(ui).ok);
                                ui.label(RichText::new("online").color(p(ui).ok).size(11.0));
                            })
                            .response
                            .on_hover_text(seen);
                        } else if d.last_seen > 0 {
                            ui.label(RichText::new(format!("last seen {}", format_local(d.last_seen))).color(p(ui).muted).size(11.0))
                                .on_hover_text(seen);
                        }
                        if !locked {
                            if d.online && ui.small_button("Update").on_hover_text("Update this computer now").clicked() {
                                update = Some(d.id.clone());
                            }
                            if ui
                                .small_button("Forget")
                                .on_hover_text("Remove from this list. If this computer connects again, it comes back with the same ID.")
                                .clicked()
                            {
                                remove = Some(d.id.clone());
                            }
                        }
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

    fn device_rows(&self) -> Vec<ShownDevice> {
        if self.access != Access::Readable {
            return self
                .live
                .as_ref()
                .map(|s| {
                    s.devices
                        .iter()
                        .map(|d| ShownDevice {
                            id: d.id.clone(),
                            hostname: d.hostname.clone(),
                            version: d.version.clone(),
                            online: d.online,
                            last_seen: 0,
                            first_seen: 0,
                            signed_in: d.signed_in,
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        self.devices
            .iter()
            .map(|d| {
                let live = self.online.get(&d.id);
                ShownDevice {
                    id: d.id.clone(),
                    hostname: d.hostname.clone(),
                    version: live.cloned().unwrap_or_else(|| d.version.clone()),
                    online: live.is_some(),
                    last_seen: d.last_seen,
                    first_seen: d.first_seen,
                    signed_in: d.signed_in,
                }
            })
            .collect()
    }

    fn peer_version_line(&self) -> Option<String> {
        let (name, version, role) = if let Some(s) = &self.cloud {
            (s.peer_name.clone(), s.peer_version.clone(), s.peer_role.clone())
        } else if let Some(s) = &self.live {
            (s.peer_name.clone(), s.peer_version.clone(), s.peer_role.clone())
        } else {
            return None;
        };
        if version.is_empty() {
            return None;
        }
        let who = if name.is_empty() { "Other server".to_string() } else { name };
        let role = if role.is_empty() { String::new() } else { format!(" ({role})") };
        Some(format!("{who}{role}  v{version}"))
    }

    fn poll_release(&mut self) {
        if let Some(rx) = &self.release_rx {
            if let Ok(v) = rx.try_recv() {
                self.release_rx = None;
                if v.is_some() {
                    self.latest_release = v;
                }
                self.release_at = Some(Instant::now());
            }
        }
        if !self.check_release || self.release_rx.is_some() {
            return;
        }
        let due = self.release_at.map(|t| t.elapsed() > Duration::from_secs(600)).unwrap_or(true);
        if !due {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.release_rx = Some(rx);
        self.release_at = Some(Instant::now());
        std::thread::spawn(move || {
            let _ = tx.send(pjdesk_proto::update::site_version().ok());
        });
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
        if self.access != Access::Readable {
            card(ui, |ui| {
                heading(ui, "LOG");
                self.locked_line(ui);
            });
            return;
        }
        card(ui, |ui| {
            heading(ui, "LOG");
            if self.log_tail.is_empty() {
                ui.label(
                    RichText::new("No log yet (the server writes server.log in its data folder when it runs).")
                        .color(p(ui).muted)
                        .size(12.0),
                );
            } else {
                egui::ScrollArea::vertical().max_height(160.0).id_salt("log").stick_to_bottom(true).show(ui, |ui| {
                    ui.label(RichText::new(&self.log_tail).monospace().size(11.0).color(p(ui).muted));
                });
            }
        });
    }
}

fn service_job(req: crate::control::Request) -> anyhow::Result<JobDone> {
    match crate::control::call(&req) {
        Some(response) if response.code == "ask" => Ok(JobDone::ServiceAsk {
            pending: response.pending,
            from: response.from,
            written_at: response.written_at,
            readable: response.readable,
        }),
        Some(response) if response.ok => Ok(JobDone::Message(response.message)),
        Some(response) => Err(anyhow::anyhow!(response.message)),
        None => Err(anyhow::anyhow!("The PJDesk service did not answer.")),
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
            crate::cloud::publish_network(dir, online, false);
            let _ = crate::requests::push(dir, crate::requests::SYNC_FILE, "now");
            JobDone::Message(format!(
                "Signed in as {email}. This is the main server, and its data is backed up to PJDesk Cloud. To add a standby, sign in on another PJDesk Server with the same account. No join code."
            ))
        }
        Connected::Resumed => {
            crate::cloud::publish_network(dir, online, false);
            JobDone::Message(format!("Signed in as {email}. Syncing as before."))
        }
        Connected::Joined { from } => {
            crate::cloud::publish_network(dir, online, false);
            let _ = crate::requests::push(dir, crate::requests::RELOAD_FILE, "now");
            JobDone::Message(format!(
                "Signed in as {email}. This server now has {from}'s key, accounts and computer list, and stays on standby. No join code."
            ))
        }
        Connected::Replaced => {
            crate::cloud::publish_network(dir, online, false);
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

#[cfg(not(windows))]
fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// A show/hide toggle for a password field. The label is a word, not an icon.
fn reveal_toggle(ui: &mut egui::Ui, shown: &mut bool) {
    ui_widgets::reveal(ui, shown);
}

struct ShownDevice {
    id: String,
    hostname: String,
    version: String,
    online: bool,
    last_seen: u64,
    first_seen: u64,
    signed_in: bool,
}

fn version_text(version: &str) -> String {
    if version.trim().is_empty() {
        "unknown (<0.5.2)".into()
    } else {
        version.to_string()
    }
}

/// Empty reported versions are behind whenever a release is known. `is_newer`
/// is false for a version it cannot parse, so an old client that sent nothing
/// still gets the badge.
fn update_available(latest: Option<&str>, reported: &str) -> bool {
    let Some(latest) = latest.filter(|s| !s.is_empty()) else {
        return false;
    };
    if reported.trim().is_empty() {
        return true;
    }
    pjdesk_proto::update::is_newer(reported, latest)
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(p(ui).muted).size(12.0).strong());
    ui.add_space(4.0);
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
    ui_theme::card(ui, add)
}

fn glyph(ui: &mut egui::Ui, icon: Icon, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    crate::ui_icons::paint(ui.painter(), rect, icon, color);
}

fn spawn_status_probe(ctx: &egui::Context) -> Option<std::sync::mpsc::Receiver<Option<Snapshot>>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("pjdesk-server-status".into())
        .spawn(move || loop {
            let snap = crate::status_pipe::read_snapshot();
            if snap.is_some() {
                if tx.send(snap).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
            std::thread::sleep(Duration::from_secs(1));
        })
        .ok()?;
    Some(rx)
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
