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
use crate::registry::{format_local, BlockedRow, DeviceRow, Registry};
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

/// Show the window from the tray. This runs on the window thread. It answers
/// the tray watch first, then asks for the window to be visible and focused,
/// then restores it. The PIN lock, when there is one, is drawn inside it.
fn reveal_window(ctx: &egui::Context, reason: &str) {
    crate::traywatch::ack();
    crate::winshow::note_user_open();
    // Put the taskbar button back before the restore looks the window up.
    crate::winshow::reveal_on_taskbar("PJDesk Server");
    ctx.request_repaint();
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    // The tray thread already posted the restore. This thread focuses the
    // viewport and draws the PIN lock. It does not clear the service account.
    crate::winshow::activate(reason, "PJDesk Server", crate::tray::holds_window());
}

fn server_window_options() -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 820.0])
            .with_min_inner_size([760.0, 640.0])
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
        // Closing the window returns here. This process then exits.
        run_and_return: true,
        ..Default::default()
    };
    // Presenting into a minimized window waits on vsync, and that wait is
    // what makes the process stop answering Windows. The close button
    // destroys the window instead. The minimize button still has a surface,
    // so that present does not wait.
    options.wgpu_options.surface.present_mode = eframe::wgpu::PresentMode::AutoNoVsync;
    options.wgpu_options.surface.desired_maximum_frame_latency = Some(1);
    options
}

/// Draws the window once. Closing it returns, and this process exits.
/// The tray is a different process and is not started here.
pub fn run(data_dir: PathBuf, port: u16) -> Result<()> {
    // A replacement waits for the frozen window to leave before taking the lock.
    crate::traywatch::wait_for_replaced_process();
    if !crate::tray::acquire_window() {
        return Ok(());
    }
    crate::tray::ensure_login_default();
    tracing::info!("Opening the PJDesk Server window. This process draws it once. The account is read from the service.");
    let dir = data_dir.clone();
    let built = eframe::run_native(
        "PJDesk Server",
        server_window_options(),
        Box::new(move |cc| Ok(Box::new(App::new(cc, dir, port)))),
    );
    if let Err(e) = built {
        return Err(anyhow::anyhow!("{e}"));
    }
    tracing::info!("The PJDesk Server window process is exiting. The tray is a different process and keeps running. The service account is left as it is.");
    Ok(())
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
    blocked: Vec<BlockedRow>,
    /// Live online devices (id → reported version), from the server's online.json.
    online: HashMap<String, String>,
    log_tail: String,
    status: String,

    last_refresh: std::time::Instant,
    tray: Option<crate::tray::Tray>,
    told_tray: bool,
    /// Last time a minimized frame said the loop was alive.
    hidden_log: Option<Instant>,
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
    gate: crate::window_lock::Gate,
    /// Folder reads. The window paints the last result.
    disk_rx: Option<std::sync::mpsc::Receiver<FolderView>>,
    /// Pipe ping and service-manager query. Separate from the folder read so a
    /// stuck pipe cannot stop the window from painting.
    svc_rx: Option<std::sync::mpsc::Receiver<ServiceProbe>>,
    bg_tx: std::sync::mpsc::Sender<String>,
    bg_rx: std::sync::mpsc::Receiver<String>,
    svc_installed: bool,
    svc_running: bool,
    policy: crate::access::Policy,
    watch: crate::public_addr::IpWatch,
    saved_address: String,
    update_auto: bool,
    update_auto_dirty: bool,
    update_status: crate::self_update::UpdateStatus,
    cloud_email: String,
    cloud_token: String,
    cloud_api: String,
    /// Email the service reported over the control pipe. Empty when it has no account.
    /// This window does not read that from a file it cannot open, and startup does not clear it.
    service_email: String,
    /// The service has answered `session` at least once.
    service_session_known: bool,
    /// The last `session` ask got no answer. The saved account is left as it is.
    service_session_failed: bool,
    session_rx: Option<std::sync::mpsc::Receiver<Option<crate::control::Response>>>,
    session_at: Option<Instant>,
    own_bucket: bool,
    window_log: String,
    /// Device id waiting for Remove to be confirmed.
    confirm_remove: Option<String>,
    /// Require signed-in accounts is waiting for Turn on. The checkbox stays on the saved value.
    confirm_require: bool,
    /// The service has not answered the signed-in-accounts switch yet.
    require_busy: bool,
    /// Value the service just saved. The checkbox shows this until the folder or the snapshot agrees.
    require_hold: Option<bool>,
    require_rx: Option<std::sync::mpsc::Receiver<RequireResult>>,
    ak_label: String,
    ak_secs: u64,
    ak_limit: String,
    /// Shown once after Create. Not written to the status line.
    ak_secret: String,
    ak_show: bool,
    ak_keys: Vec<pjdesk_proto::signal::AccessKeyBrief>,
    /// The list returned with the last create or revoke. Shown until a snapshot has the same keys.
    ak_hold: Option<Vec<pjdesk_proto::signal::AccessKeyBrief>>,
    ak_confirm: Option<String>,
    ak_rx: Option<std::sync::mpsc::Receiver<AkResult>>,
    ak_busy: bool,
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
    /// Last sign-in failure, shown on the account card. No password or token.
    sign_error: String,
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
        let (bg_tx, bg_rx) = std::sync::mpsc::channel();
        let mut app = Self {
            data_dir,
            port,
            fingerprint: String::new(),
            can_write: false,
            control_ready: false,
            accounts: Vec::new(),
            devices: Vec::new(),
            blocked: Vec::new(),
            online: HashMap::new(),
            log_tail: String::new(),
            status: String::new(),
            last_refresh: std::time::Instant::now(),
            tray: None,
            told_tray: false,
            hidden_log: None,
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
            gate: crate::window_lock::Gate::new(),
            disk_rx: None,
            svc_rx: None,
            bg_tx,
            bg_rx,
            svc_installed: false,
            svc_running: false,
            policy: crate::access::Policy::default(),
            watch: crate::public_addr::IpWatch::default(),
            saved_address: String::new(),
            update_auto: false,
            update_auto_dirty: false,
            update_status: crate::self_update::UpdateStatus::default(),
            cloud_email: String::new(),
            cloud_token: String::new(),
            cloud_api: crate::cloud::default_api(),
            service_email: String::new(),
            service_session_known: false,
            service_session_failed: false,
            session_rx: None,
            session_at: None,
            own_bucket: false,
            window_log: String::new(),
            confirm_remove: None,
            confirm_require: false,
            require_busy: false,
            require_hold: None,
            require_rx: None,
            ak_label: String::new(),
            ak_secs: 0,
            ak_limit: "1".into(),
            ak_secret: String::new(),
            ak_show: false,
            ak_keys: Vec::new(),
            ak_hold: None,
            ak_confirm: None,
            ak_rx: None,
            ak_busy: false,
        };
        crate::public_addr::remember_port(&app.data_dir, port);
        if spawn_probe {
            // The first paint must not wait on the data folder, the control pipe,
            // or the service manager. Headless shots still read inline.
            app.request_refresh();
            app.request_service_probe();
        } else {
            app.refresh();
        }
        let email = app.signed_in_email();
        let (token, api) = app.cloud_session();
        app.gate.arm(&email, &token, &api);
        if spawn_probe {
            app.probe = spawn_status_probe(&cc.egui_ctx);
        }
        app
    }

    fn start_address_guess(&mut self) {
        if self.pub_guess_rx.is_some() || !self.saved_address.trim().is_empty() {
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
    }

    fn service_state(&self) -> (bool, bool) {
        if let Some(state) = self.svc_override {
            return state;
        }
        (self.svc_installed, self.svc_running)
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
        self.apply_folder(read_folder(&self.data_dir));
    }

    /// Reads the data folder on a worker. Painting never waits on a pipe, the
    /// service manager, a public-address file, or the log.
    fn request_refresh(&mut self) {
        if self.disk_rx.is_some()
            || self.preview_locked
            || self.preview_devices
            || self.preview_network
            || self.preview_netkey
            || self.preview_account
        {
            return;
        }
        let dir = self.data_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.disk_rx = Some(rx);
        std::thread::Builder::new()
            .name("pjdesk-server-window".into())
            .spawn(move || {
                let _ = tx.send(read_folder(&dir));
            })
            .ok();
    }

    /// Pipe ping and Service Control Manager. A hung pipe stays on this worker.
    fn request_service_probe(&mut self) {
        if self.svc_rx.is_some() || self.preview_locked || self.svc_override.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.svc_rx = Some(rx);
        let token = self.cloud_token.clone();
        std::thread::Builder::new()
            .name("pjdesk-server-service".into())
            .spawn(move || {
                let (installed, running) = svc_status();
                let _ = tx.send(ServiceProbe { control_ready: crate::control::ready_with(&token), installed, running });
            })
            .ok();
    }

    fn poll_disk(&mut self) {
        if let Some(rx) = &self.disk_rx {
            match rx.try_recv() {
                Ok(view) => {
                    self.disk_rx = None;
                    if !self.preview_locked {
                        self.apply_folder(view);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.disk_rx = None,
            }
        }
        if let Some(rx) = &self.svc_rx {
            match rx.try_recv() {
                Ok(probe) => {
                    self.svc_rx = None;
                    if self.svc_override.is_none() && !self.preview_locked {
                        self.control_ready = probe.control_ready;
                        self.svc_installed = probe.installed;
                        self.svc_running = probe.running;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.svc_rx = None,
            }
        }
    }

    fn apply_folder(&mut self, view: FolderView) {
        self.access = view.access;
        self.window_log = view.window_log;
        let incoming_require = view.policy.require_accounts;
        if self.require_hold == Some(incoming_require) {
            self.require_hold = None;
        }
        let held = self.require_hold;
        self.policy = view.policy;
        if let Some(held) = held {
            self.policy.require_accounts = held;
            self.policy.require_chosen = true;
        }
        self.watch = view.watch;
        self.saved_address = view.public_address.as_ref().map(|saved| saved.address.clone()).unwrap_or_default();
        if !self.pub_addr_ready {
            if let Some(saved) = &view.public_address {
                self.pub_addr = saved.address.clone();
                self.pub_addr_note = saved.note.clone();
                self.pub_addr_needs_confirm = saved.needs_confirm;
                self.pub_addr_confirmed = saved.confirmed;
            }
            if !view.update_start.is_empty() || !view.update_end.is_empty() {
                self.update_start = view.update_start.clone();
                self.update_end = view.update_end.clone();
            }
            self.pub_addr_ready = true;
        } else if let Some(saved) = &view.public_address {
            if saved.address.trim() == self.pub_addr.trim() {
                self.pub_addr_note = saved.note.clone();
                self.pub_addr_needs_confirm = saved.needs_confirm;
                self.pub_addr_confirmed = saved.confirmed;
            }
        }
        self.update_status = view.update_status;
        if self.update_auto == view.update_auto {
            self.update_auto_dirty = false;
        } else if !self.update_auto_dirty {
            self.update_auto = view.update_auto;
        }
        self.cloud_email = view.cloud_email;
        self.cloud_token = view.cloud_token;
        self.cloud_api = view.cloud_api;
        self.own_bucket = view.own_bucket;
        if view.access != Access::Readable {
            self.can_write = false;
            return;
        }
        self.can_write = view.can_write;
        self.fingerprint = view.fingerprint;
        self.accounts = view.accounts;
        self.devices = view.devices;
        self.blocked = view.blocked;
        log_online_refresh(&self.online, &view.online);
        self.online = view.online;
        self.log_tail = view.log_tail;
        self.ak_keys = view.access_keys;
        self.cloud_on = view.cloud_on;
        self.cloud = view.cloud;
        if self.check_release {
            self.start_address_guess();
        }
    }

    fn spawn_note(&self, job: impl FnOnce() -> String + Send + 'static) {
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
    }

    fn poll_notes(&mut self) {
        let mut refresh = false;
        while let Ok(msg) = self.bg_rx.try_recv() {
            if msg.contains("Signed out") {
                self.service_email.clear();
                self.service_session_known = true;
                self.service_session_failed = false;
                self.session_at = Some(Instant::now());
            }
            self.status = msg;
            refresh = true;
        }
        if refresh {
            self.request_refresh();
        }
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

    /// Tray clicks arrive from the other process. Close exits this process.
    /// The tray is not in this process, so nothing here calls `run_native` again.
    fn on_tray(&mut self, ctx: &egui::Context) {
        let tray_actions = crate::tray::gui_actions();
        for action in tray_actions {
            match action {
                crate::tray::Action::Show => {
                    tracing::info!("Tray click reached PJDesk Server. Showing the window. The PIN lock is shown when one is set.");
                    reveal_window(ctx, "Open");
                    self.gate.engage();
                }
                crate::tray::Action::CheckUpdates => {
                    tracing::info!("Tray click: Check for updates. Showing the window.");
                    reveal_window(ctx, "Check for updates");
                    self.gate.engage();
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
                    tracing::info!("Tray click: Make this the primary. Showing the window.");
                    reveal_window(ctx, "Make this the primary");
                    self.gate.engage();
                    self.cloud_form.confirm_promote = true;
                    self.cloud_form.confirm_demote = false;
                }
                crate::tray::Action::Demote => {
                    tracing::info!("Tray click: Step down to standby. Showing the window.");
                    reveal_window(ctx, "Step down to standby");
                    self.gate.engage();
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
                    tracing::info!("The tray asked this window to exit. The service account is left as it is.");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let closing = ctx.input(|i| i.viewport().close_requested());
        if closing && !self.told_tray {
            self.told_tray = true;
            tracing::info!("The window was closed. This process is exiting. The tray is a different process and keeps running.");
            self.status = "PJDesk Server stays in the tray.".into();
        }
        self.note_if_minimized(ctx);
    }

    /// A minimized window can still be asked to present. Say so, and do not
    /// schedule another present from here. The close button never gets here
    /// because that window has already been destroyed.
    fn note_if_minimized(&mut self, ctx: &egui::Context) {
        let hidden = ctx.input(|i| {
            let view = i.viewport();
            view.minimized == Some(true) || view.visible() == Some(false)
        });
        if !hidden {
            self.hidden_log = None;
            return;
        }
        let due = self
            .hidden_log
            .map(|at| pjdesk_proto::trayshow::parked_alive_due(at.elapsed().as_millis() as u64))
            .unwrap_or(true);
        if due {
            self.hidden_log = Some(Instant::now());
            tracing::info!("The window is minimized. This frame ran, so the loop is alive. It is not being drawn again until it is restored.");
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.on_tray(ctx);
        if self.want_session() {
            self.request_session(ctx);
        }
        self.poll_session();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.last_refresh.elapsed() > Duration::from_secs(3) {
            self.last_refresh = Instant::now();
            self.request_refresh();
            self.request_service_probe();
        }
        self.poll_disk();
        self.poll_session();
        self.poll_notes();
        self.poll_access_keys();
        self.poll_require();
        let snaps: Vec<Snapshot> = self
            .probe
            .as_ref()
            .map(|rx| rx.try_iter().filter_map(|snap| snap).collect())
            .unwrap_or_default();
        for snap in snaps {
            self.accept_snapshot(snap);
        }
        let ctx = ui.ctx().clone();
        let hidden = ctx.input(|i| i.viewport().minimized == Some(true) || i.viewport().visible() == Some(false));
        // A minimized window is not asked to present again. That present is
        // what stopped the message loop. A restore from the taskbar wakes it.
        if !hidden {
            ctx.request_repaint_after(if self.waiting_for_service() || self.disk_rx.is_some() || self.svc_rx.is_some() || self.ak_rx.is_some() || self.require_rx.is_some() {
                Duration::from_millis(50)
            } else {
                Duration::from_secs(1)
            });
        }
        self.poll_cloud_job();
        self.poll_netkey();
        self.poll_release();

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
                (
                    self.cloud.as_ref().map(|s| s.role.clone()).unwrap_or_default(),
                    self.cloud.as_ref().map(|s| s.epoch).unwrap_or(0),
                    self.cloud.as_ref().map(|s| s.active_name.clone()).unwrap_or_default(),
                    self.update_status.available.clone(),
                    self.update_status.last_result.clone(),
                    self.online.len(),
                )
            };
            let reported = shown_update(0, &last_result, &available, "");
            tray.set_tooltip(&crate::tray_tip::format(&crate::tray_tip::Tip {
                installed,
                running,
                hosts,
                role: &role,
                epoch,
                active: &who,
                available: &reported.available,
                last_result: &reported.last_result,
            }));
        }

        let email = self.signed_in_email();
        let (token, api) = self.cloud_session();
        self.gate.poll(&ctx, &email, &token, &api);
        let hidden = ctx.input(|i| i.viewport().minimized == Some(true) || i.viewport().visible() == Some(false));
        self.gate.note_visibility(&ctx, hidden);
        if self.gate.blocking() {
            ctx.request_repaint_after(Duration::from_millis(200));
            self.gate.draw(ui);
            return;
        }
        if self.check_release && !self.pub_addr_ready {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui_widgets::spinner(ui, p(ui).accent);
                    ui.label("Opening PJDesk Server…");
                });
            });
            return;
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
                self.gate.security_card(ui);
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
        use crate::self_update::{self};
        let reported = shown_update(
            self.update_status.last_check_at,
            &self.update_status.last_result,
            &self.update_status.available,
            &self.update_status.error,
        );
        let may_edit = self.may_edit_settings();
        card(ui, |ui| {
            heading(ui, "UPDATES");
            let mut auto = self.update_auto;
            let auto_changed = ui
                .add_enabled_ui(may_edit, |ui| {
                    ui_widgets::checkbox(ui, &mut auto, "Install server updates automatically")
                })
                .inner;
            if auto_changed {
                self.update_auto = auto;
                self.update_auto_dirty = true;
                let dir = self.data_dir.clone();
                self.status = if auto {
                    "Turning automatic server updates on…".into()
                } else {
                    "Turning automatic server updates off…".into()
                };
                self.spawn_note(move || {
                    let mut settings = self_update::Settings::load(&dir);
                    settings.auto = auto;
                    settings.save(&dir);
                    match self_update::set_task(&dir, auto) {
                        Ok(()) => {
                            if auto {
                                "Automatic server updates are on. The service checks about once an hour.".into()
                            } else {
                                "Automatic server updates are off.".into()
                            }
                        }
                        Err(e) => format!("{e:#}"),
                    }
                });
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
                        let dir = self.data_dir.clone();
                        let start = self.update_start.clone();
                        let end = self.update_end.clone();
                        self.status = "Saving the maintenance window…".into();
                        self.spawn_note(move || match self_update::set_window(&dir, &start, &end) {
                            Ok(()) => "Saved the maintenance window. Leave both blank for any time.".into(),
                            Err(e) => format!("{e}"),
                        });
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
        self.ask_service_or(req, "Asking the PJDesk service…", "The PJDesk service did not answer.".into());
    }

    /// Sends one command to the service. The account token is attached when this window has it,
    /// so the signed-in owner does not need elevation. A timeout replaces the pending line.
    fn ask_service_or(&mut self, mut req: crate::control::Request, pending: &str, unanswered: String) {
        if req.account_token.is_empty() {
            req.account_token = self.cloud_token.clone();
        }
        self.status = pending.to_string();
        self.spawn_note(move || match crate::control::call(&req) {
            Some(response) if response.code == "timeout" => unanswered,
            Some(response) if response.message.is_empty() && !response.ok => unanswered,
            Some(response) => response.message,
            None => unanswered,
        });
    }

    fn owner_signed_in(&self) -> bool {
        !self.signed_in_email().is_empty()
    }

    /// The service is the writer. A signed-in owner uses the pipe even when this window cannot
    /// write ProgramData and even when the last ping did not carry the account token yet.
    fn owner_uses_pipe(&self) -> bool {
        self.owner_signed_in() || self.svc_running || self.control_ready
    }

    fn require_accounts_saved(&self) -> bool {
        if let Some(held) = self.require_hold {
            return held;
        }
        if self.access == Access::Readable {
            self.policy.require_accounts
        } else {
            self.live.as_ref().map(|snap| snap.require_accounts).unwrap_or(false)
        }
    }

    fn accept_snapshot(&mut self, mut snap: Snapshot) {
        if self.access != Access::Readable && self.require_hold == Some(snap.require_accounts) {
            self.require_hold = None;
        }
        if let Some(held) = self.require_hold {
            snap.require_accounts = held;
        }
        let covered = self.ak_hold.as_ref().is_some_and(|hold| hold_covered(&snap.access_keys, hold));
        if covered {
            self.ak_hold = None;
        }
        self.live = Some(snap);
    }

    fn poll_require(&mut self) {
        let Some(rx) = &self.require_rx else { return };
        match rx.try_recv() {
            Ok(result) => {
                self.require_rx = None;
                self.require_busy = false;
                self.status = result.message;
                if result.ok {
                    self.require_hold = Some(result.require);
                    self.policy.require_accounts = result.require;
                    self.policy.require_chosen = true;
                    if let Some(live) = &mut self.live {
                        live.require_accounts = result.require;
                    }
                }
                self.request_refresh();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.require_rx = None;
                self.require_busy = false;
                self.status = "The PJDesk service did not answer. Require signed-in accounts was not changed.".into();
            }
        }
    }

    fn commit_require(&mut self, require: bool) {
        if self.require_busy {
            return;
        }
        self.require_busy = true;
        self.confirm_require = false;
        self.status = "Saving the signed-in accounts setting…".into();
        let (tx, rx) = std::sync::mpsc::channel();
        self.require_rx = Some(rx);
        let dir = self.data_dir.clone();
        let token = self.cloud_token.clone();
        let service = self.owner_uses_pipe();
        let can_write = self.can_write;
        std::thread::spawn(move || {
            let result = if service {
                match crate::control::call(&crate::control::Request {
                    cmd: "set_require".into(),
                    require_accounts: Some(require),
                    account_token: token,
                    ..Default::default()
                }) {
                    Some(response) if response.ok => RequireResult { ok: true, require, message: response.message },
                    Some(response) if response.code == "timeout" => RequireResult {
                        ok: false,
                        require,
                        message: "The PJDesk service did not answer. Require signed-in accounts was not changed.".into(),
                    },
                    None if can_write => save_require_local(&dir, require),
                    Some(response) => RequireResult {
                        ok: false,
                        require,
                        message: if response.message.is_empty() {
                            "Require signed-in accounts was not changed.".into()
                        } else {
                            response.message
                        },
                    },
                    None => RequireResult {
                        ok: false,
                        require,
                        message: "The PJDesk service did not answer. Require signed-in accounts was not changed.".into(),
                    },
                }
            } else if can_write {
                save_require_local(&dir, require)
            } else {
                RequireResult {
                    ok: false,
                    require,
                    message: "The PJDesk service did not answer. Require signed-in accounts was not changed.".into(),
                }
            };
            let _ = tx.send(result);
        });
    }

    /// The checkbox shows the saved value. Turning it on while unsigned computers are listed
    /// asks first. It does not stay ticked when the save fails.
    fn require_checkbox(&mut self, ui: &mut egui::Ui, unsigned: &[UnknownComputer]) {
        let saved = self.require_accounts_saved();
        let mut shown = saved;
        let enabled = self.owner_signed_in() || self.control_ready || (self.can_write && self.using_own_bucket());
        let names = unsigned_labels(unsigned);
        ui.add_enabled_ui(enabled, |ui| {
            if ui.checkbox(&mut shown, "Require signed-in accounts").changed() {
                if shown && !names.is_empty() {
                    self.confirm_require = true;
                } else {
                    self.confirm_require = false;
                    self.commit_require(shown);
                }
            }
        });
        if !enabled {
            ui.label(
                RichText::new("Sign in with your PJDesk account above to change this.")
                    .color(p(ui).muted)
                    .size(12.0),
            );
        }
        if self.confirm_require {
            if names.is_empty() {
                self.confirm_require = false;
            } else {
                ui.label(
                    RichText::new(format!("These computers will be disconnected: {}.", names.join(", ")))
                        .color(p(ui).warn),
                );
                ui.horizontal(|ui| {
                    if ui.small_button("Turn on").clicked() {
                        self.commit_require(true);
                    }
                    if ui.small_button("Cancel").clicked() {
                        self.confirm_require = false;
                    }
                });
            }
        }
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
        let data_dir = self.data_dir.clone();
        let port = self.port;
        self.status = "Working on the PJDesk service…".into();
        self.spawn_note(move || {
            let result = match action {
                SvcAction::Install => {
                    let opts = crate::ServerOpts {
                        bind: std::net::IpAddr::from([0, 0, 0, 0]),
                        port,
                        data_dir: Some(data_dir.clone()),
                    };
                    crate::service::install(&opts, &data_dir, true).map(|_| "Service installed and started.")
                }
                SvcAction::Uninstall => crate::service::uninstall().map(|_| "Service removed."),
                SvcAction::Start => crate::service::start().map(|_| "Service started."),
                SvcAction::Stop => crate::service::stop().map(|_| "Service stopped."),
                SvcAction::Restart => crate::service::restart().map(|_| "Service restarted."),
            };
            match result {
                Ok(msg) => msg.to_string(),
                Err(e) => format!("{e:#}"),
            }
        });
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
                .add_enabled_ui(managed, |ui| {
                    ui_widgets::checkbox(ui, &mut auto, "Install server updates automatically")
                })
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
            let reported = shown_update(snap.last_check_at, &snap.last_result, &snap.update_available, &snap.update_error);
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
                ui.colored_label(p(ui).warn, &reported.error);
            }
            if managed {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Check for updates now").clicked() {
                        check_now = true;
                    }
                    if !reported.available.is_empty() && ui.button("Install this update").clicked() {
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
                    ui.add(
                        egui::TextEdit::singleline(&mut self.update_start).hint_text("any time").desired_width(70.0),
                    );
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
                    (p(ui).ok, primary_role_text(snap.epoch))
                } else {
                    (p(ui).accent, standby_role_text(&snap.active_name, snap.epoch))
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

    /// Bearer token for the signed-in PJDesk account, when this window can read it.
    fn cloud_session(&self) -> (String, String) {
        if self.access == Access::Readable && !self.cloud_token.is_empty() {
            return (self.cloud_token.clone(), self.cloud_api.clone());
        }
        (String::new(), crate::cloud::default_api())
    }

    /// Email of the PJDesk account this server is signed in as.
    /// A locked data folder is not "signed out". The service's `session`
    /// answer is the account. Until that answer arrives, the status pipe's
    /// email is kept so the form does not flash.
    fn signed_in_email(&self) -> String {
        let local = if let Some(cloud) = &self.cloud {
            if !cloud.account.is_empty() {
                cloud.account.clone()
            } else if self.access == Access::Readable {
                self.cloud_email.clone()
            } else {
                String::new()
            }
        } else if self.access == Access::Readable {
            self.cloud_email.clone()
        } else {
            String::new()
        };
        let live = self.live.as_ref().map(|s| s.account.clone()).unwrap_or_default();
        crate::control::shown_account(
            self.access == Access::Readable,
            &local,
            self.service_session_known,
            &self.service_email,
            &live,
        )
    }

    fn want_session(&self) -> bool {
        if !self.check_release || self.preview_locked || self.access == Access::Readable || self.session_rx.is_some() {
            return false;
        }
        let wait = if self.service_session_known { Duration::from_secs(5) } else { Duration::from_secs(2) };
        self.session_at.is_none_or(|at| at.elapsed() >= wait)
    }

    /// Asks the service who is signed in. Does not write `cloud-sync.json`.
    fn request_session(&mut self, ctx: &egui::Context) {
        if self.session_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.session_rx = Some(rx);
        self.session_at = Some(Instant::now());
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("pjdesk-server-session".into())
            .spawn(move || {
                let report = crate::control::call(&crate::control::Request { cmd: "session".into(), ..Default::default() });
                let _ = tx.send(report);
                ctx.request_repaint();
            })
            .ok();
    }

    fn poll_session(&mut self) {
        let Some(rx) = &self.session_rx else { return };
        let report = match rx.try_recv() {
            Ok(report) => report,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
        };
        self.session_rx = None;
        self.session_at = Some(Instant::now());
        match report {
            Some(response) if response.ok => {
                let email = response.email.trim().to_string();
                if email != self.service_email || !self.service_session_known {
                    tracing::info!(
                        "session: the service account is {}",
                        if email.is_empty() { "signed out".to_string() } else { email.clone() }
                    );
                }
                self.service_email = email;
                self.service_session_known = true;
                self.service_session_failed = false;
            }
            _ => {
                self.service_session_failed = true;
                tracing::info!("session: the PJDesk service did not answer. The saved sign-in was left on the service.");
            }
        }
    }

    /// True when Cloud Sync is the advanced "your own bucket" path.
    fn using_own_bucket(&self) -> bool {
        self.access == Access::Readable && self.cloud_on && self.own_bucket
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
            let who = other_server_name(&active_name);
            return format!("Standby of this account. Primary: {who}. Same account, no join code.");
        }
        let peer = if peer_name.is_empty() || same_as_this_computer(&peer_name) { String::new() } else { peer_name };
        if peer.is_empty() {
            "Primary: this server. A second server that signs in with this account becomes the standby. No join code."
                .to_string()
        } else {
            format!("Primary: this server. Standby: {peer}. Same account, no join code.")
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
        let cloud_on =
            if readable { self.cloud_on } else { self.live.as_ref().is_some_and(|s| s.cloud_on) || !email.is_empty() };
        let choosing_bucket = self.cloud_form.own_bucket;
        card(ui, |ui| {
            heading(ui, "PJDESK ACCOUNT");
            if !readable && !crate::control::show_sign_in_form(false, &email, self.service_session_known) && email.is_empty() {
                ui.horizontal(|ui| {
                    ui_widgets::spinner(ui, p(ui).accent);
                    ui.label("Reading the saved sign-in from the service…");
                });
                if self.service_session_failed {
                    ui.label("The PJDesk service did not answer. The saved sign-in was left on the service. Trying again.");
                }
                return;
            }
            if !readable && !self.control_ready && !email.is_empty() {
                ui.label(RichText::new(format!("Signed in as {email}")).strong().size(18.0).color(p(ui).text));
                if !role_line.is_empty() {
                    ui.label(&role_line);
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
            let sign_error = self.cloud_form.sign_error.clone();
            ui.add_enabled_ui(!busy, |ui| {
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
            if !sign_error.is_empty() {
                ui.add_space(4.0);
                ui.colored_label(p(ui).warn, &sign_error);
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
                    self.request_refresh();
                }
            }
            if let Some((u, disabled)) = toggle_disabled {
                if let Some(mut accts) = self.accounts_mut() {
                    let _ = accts.set_disabled(&u, disabled);
                    self.request_refresh();
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
                        ui.label(primary_role_text(s.epoch));
                    } else {
                        ui_widgets::dot(ui, p(ui).accent);
                        ui.label(standby_role_text(&s.active_name, s.epoch));
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
                self.cloud_form.sign_error.clear();
                if email.is_empty() || password.is_empty() {
                    let who = if email.is_empty() { "(no email)".to_string() } else { email.clone() };
                    tracing::info!("sign-in: {who}: not sent (email or password empty)");
                    let msg = "Enter your PJDesk account email and password.".to_string();
                    self.cloud_form.sign_error = msg.clone();
                    self.status = msg;
                    if email.is_empty() {
                        self.cloud_form.acct_password = password;
                    }
                    return;
                }
                tracing::info!("sign-in: {email}: starting");
                self.status = format!("Signing in as {email}…");
                if !self.can_write {
                    self.spawn_cloud_job(move || sign_in_through_service(email, password));
                    return;
                }
                self.spawn_cloud_job(move || {
                    let outcome = (|| {
                        let cfg = cloud::sign_in(&cloud::default_api(), &email, &password)?;
                        finish_account(&dir, cfg, online, None)
                    })();
                    log_sign_in_outcome(&email, &outcome);
                    outcome
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
                        let outcome = match crate::control::call_for(
                            &crate::control::Request {
                                cmd: "sign_in_choose".into(),
                                pending,
                                choice: choice.into(),
                                ..Default::default()
                            },
                            crate::control::SIGN_IN_WAIT,
                        ) {
                            Some(response) if response.ok => Ok(JobDone::Message(response.message)),
                            Some(response) if response.code == "timeout" || response.message.trim().is_empty() => {
                                Err(anyhow::anyhow!("The PJDesk service did not answer. Sign-in was not finished."))
                            }
                            Some(response) => Err(anyhow::anyhow!(response.message)),
                            None => Err(anyhow::anyhow!("The PJDesk service did not answer. Sign-in was not finished.")),
                        };
                        match &outcome {
                            Ok(JobDone::Message(msg)) => tracing::info!("sign-in: choice: {msg}"),
                            Err(err) => tracing::info!("sign-in: choice failed: {err:#}"),
                            _ => {}
                        }
                        outcome
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
                self.request_refresh();
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
                self.request_refresh();
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
                if let Some(email) = crate::control::email_from_sign_in_message(&msg) {
                    self.service_email = email;
                    self.service_session_known = true;
                    self.service_session_failed = false;
                    self.session_at = Some(Instant::now());
                    tracing::info!("sign-in: the window is showing {}", self.service_email);
                }
                self.status = msg;
                let f = &mut self.cloud_form;
                f.sign_error.clear();
                f.secret.clear();
                f.code.clear();
                f.acct_password.clear();
                f.show_secret = false;
                f.show_code = false;
                f.show_acct_password = false;
                self.request_refresh();
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
            Err(e) => {
                self.cloud_form.sign_error = e.clone();
                self.status = e;
            }
        }
    }

    fn network_card(&mut self, ui: &mut egui::Ui) {
        if self.access != Access::Readable && (self.control_ready || self.owner_signed_in()) {
            self.network_managed(ui);
            return;
        }
        if self.access != Access::Readable {
            let snap = self.live.clone();
            let mut ak_act = None;
            card(ui, |ui| {
                heading(ui, "SIGNED-IN ACCOUNTS");
                if let Some(live) = &snap {
                    let on = live.require_accounts;
                    ui.label(if on {
                        "Require signed-in accounts is on."
                    } else {
                        "Require signed-in accounts is off. Already-registered computers keep connecting."
                    });
                    let waiting = unknown_from_listed(live.devices.iter().filter(|d| !d.signed_in));
                    let blocked = blocks_from_listed(&live.blocked);
                    ui.add_space(6.0);
                    self.not_signed_in(ui, &waiting, &blocked, false);
                }
                ak_act = self.server_access_keys(ui, false);
                self.locked_line(ui);
            });
            if let Some(act) = ak_act {
                self.start_access_key(act);
            }
            return;
        }
        let policy = self.policy.clone();
        let unsigned = unknown_from_devices(self.devices.iter().filter(|d| !d.signed_in));
        let blocked = blocks_from_registry(&self.blocked);
        let mut block_act = None;
        let mut ak_act = None;
        self.poll_public_address();
        let saved_address = self.saved_address.clone();
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
            ui.label(RichText::new(self.watch.line()).color(p(ui).muted).size(12.0));
            let hostname = crate::public_addr::is_hostname(self.pub_addr.trim());
            let mut auto = self.watch.enabled(self.pub_addr.trim());
            if hostname {
                ui.label(
                    RichText::new("This address is a hostname, so a public IP change does not replace it.")
                        .color(p(ui).muted)
                        .size(12.0),
                );
            }
            ui.add_enabled_ui(may_edit && !hostname, |ui| {
                if ui.checkbox(&mut auto, "Update the address when my public IP changes").changed() {
                    self.watch.auto = Some(auto);
                    let watch = self.watch.clone();
                    let dir = self.data_dir.clone();
                    std::thread::spawn(move || watch.save(&dir));
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
                "Off by default, so an update does not take a computer offline. Turning this on disconnects computers that have not signed in. This follows the PJDesk account signed in above.",
            ).color(p(ui).muted));
            ui.add_space(6.0);
            self.require_checkbox(ui, &unsigned);
            ui.add_space(8.0);
            let can_act = self.owner_uses_pipe() || self.can_write;
            block_act = self.not_signed_in(ui, &unsigned, &blocked, can_act);
            ak_act = self.server_access_keys(ui, can_act);
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
        if let Some(act) = block_act {
            self.apply_block(act);
        }
        if publish {
            self.publish_public_address(replace);
        }
        if let Some(act) = ak_act {
            self.start_access_key(act);
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
        let watch_known = live.as_ref().is_some_and(|s| s.public_watch_known);
        let mut watch = live.as_ref().is_some_and(|s| s.public_watch);
        let ip_line = live.as_ref().map(|s| s.public_ip_line.clone()).unwrap_or_default();
        let hostname = crate::public_addr::is_hostname(self.pub_addr.trim());
        let mut watch_changed = false;
        let mut publish = false;
        let mut replace = false;
        let mut block_act = None;
        let mut ak_act = None;
        let address = self.pub_addr.clone();
        card(ui, |ui| {
            heading(ui, "PUBLIC ADDRESS");
            ui.label(
                RichText::new("Saved by the PJDesk service. The address computers use to reach this server.")
                    .color(p(ui).muted),
            );
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
            if let Some(live) = &live {
                if !live.address_warn.is_empty() {
                    ui.label(
                        RichText::new(&live.address_warn)
                            .color(egui::Color32::from_rgb(0xb5, 0x86, 0x0b))
                            .size(13.0),
                    );
                }
                if !live.routes_line.is_empty() {
                    ui.label(RichText::new(&live.routes_line).color(p(ui).muted).size(12.0));
                }
            }
            if hostname {
                ui.label(
                    RichText::new("This address is a hostname, so a public IP change does not replace it.")
                        .color(p(ui).muted)
                        .size(12.0),
                );
            } else if watch_known && ui.checkbox(&mut watch, "Update the address when my public IP changes").changed() {
                watch_changed = true;
            }
            if !self.pub_addr_note.is_empty() {
                ui.label(RichText::new(&self.pub_addr_note).color(p(ui).muted).size(12.0));
            }
            if self.pub_addr_needs_confirm && ui.button("Replace the server on My network").clicked() {
                publish = true;
                replace = true;
            } else if !self.pub_addr.trim().is_empty()
                && self.pub_addr.trim() != address.trim()
                && ui.button("Update My network").clicked()
            {
                publish = true;
            }
            ui.add_space(12.0);
            heading(ui, "SIGNED-IN ACCOUNTS");
            ui.add_space(6.0);
            let (waiting, blocked_rows) = match &live {
                Some(live) => (
                    unknown_from_listed(live.devices.iter().filter(|d| !d.signed_in)),
                    blocks_from_listed(&live.blocked),
                ),
                None => (Vec::new(), Vec::new()),
            };
            self.require_checkbox(ui, &waiting);
            ui.label(RichText::new(
                "Off by default, so an update does not take a computer offline. Turning this on disconnects computers that have not signed in.",
            ).color(p(ui).muted).size(12.0));
            if live.is_some() {
                ui.add_space(6.0);
                let can_act = self.owner_uses_pipe() || self.can_write;
                block_act = self.not_signed_in(ui, &waiting, &blocked_rows, can_act);
            }
            ak_act = self.server_access_keys(ui, self.owner_uses_pipe() || self.can_write);
        });
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
        if let Some(act) = block_act {
            self.apply_block(act);
        }
        if let Some(act) = ak_act {
            self.start_access_key(act);
        }
    }

    fn publish_public_address(&mut self, replace: bool) {
        let address = self.pub_addr.trim().to_string();
        if address.is_empty() {
            self.pub_addr_note = "Type the public address computers use to reach this server.".into();
            return;
        }
        self.pub_addr_confirmed = true;
        self.pub_addr_needs_confirm = false;
        self.pub_addr_note = "Publishing this address…".into();
        self.saved_address = address.clone();
        let dir = self.data_dir.clone();
        let online = self.online.len();
        self.status = "Publishing this address…".into();
        self.spawn_note(move || {
            let mut record = crate::public_addr::load(&dir).unwrap_or(crate::public_addr::PublicAddress {
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
            let signed_in = crate::cloud::CloudConfig::load(&dir)
                .is_some_and(|cfg| cfg.is_account() && !cfg.token.is_empty());
            record.note = if signed_in {
                "Publishing this address…".into()
            } else {
                "Sign in with your PJDesk account above to publish this address.".into()
            };
            crate::public_addr::save(&dir, &record);
            if !signed_in {
                return record.note;
            }
            crate::cloud::publish_network(&dir, online, replace);
            "Published this address.".into()
        });
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

    fn shown_keys(&self) -> Vec<pjdesk_proto::signal::AccessKeyBrief> {
        let local = if self.access == Access::Readable { self.ak_keys.clone() } else { Vec::new() };
        let live = self.live.as_ref().map(|snap| snap.access_keys.clone()).unwrap_or_default();
        let hold = self.ak_hold.clone().unwrap_or_default();
        keys_to_show(&local, &live, &hold)
    }

    fn key_label(&self, key_id: &str) -> String {
        if key_id.is_empty() {
            return String::new();
        }
        self.shown_keys()
            .into_iter()
            .find(|key| key.id == key_id)
            .map(|key| key.label)
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| "Access key".into())
    }

    fn poll_access_keys(&mut self) {
        let Some(rx) = &self.ak_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.ak_rx = None;
        self.ak_busy = false;
        match result {
            AkResult::Secret { secret, keys } => {
                self.ak_secret = secret;
                self.ak_show = true;
                self.ak_label.clear();
                self.status = "Copy this key now. It is shown once.".into();
                self.hold_keys(keys);
            }
            AkResult::Note { message, keys } => {
                self.status = message;
                self.hold_keys(keys);
            }
        }
    }

    fn hold_keys(&mut self, keys: Vec<pjdesk_proto::signal::AccessKeyBrief>) {
        if !keys.is_empty() {
            self.ak_hold = Some(keys);
        }
        self.request_refresh();
    }

    fn start_access_key(&mut self, act: AkAct) {
        if let AkAct::Dismiss = act {
            self.ak_secret.clear();
            self.ak_show = false;
            return;
        }
        if self.ak_busy {
            return;
        }
        let create = matches!(act, AkAct::Create);
        let key_id = match act {
            AkAct::Revoke(id) => id,
            _ => String::new(),
        };
        if create && self.ak_label.trim().is_empty() {
            self.status = "Give the key a label, such as the person's name.".into();
            return;
        }
        let limit = if create {
            match self.ak_limit.trim().parse::<u32>() {
                Ok(n) if (1..=100).contains(&n) => n,
                _ => {
                    self.status = "The computer limit is a number from 1 to 100.".into();
                    return;
                }
            }
        } else {
            1
        };
        let label = self.ak_label.clone();
        let secs = self.ak_secs;
        let dir = self.data_dir.clone();
        let can_write = self.can_write;
        let via_pipe = self.owner_uses_pipe() || !can_write;
        let token = self.cloud_token.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.ak_busy = true;
        self.ak_confirm = None;
        self.status = if create { "Creating an access key…".into() } else { "Revoking that access key…".into() };
        std::thread::spawn(move || {
            let unanswered = if create {
                "The PJDesk service did not answer. The key was not created."
            } else {
                "The PJDesk service did not answer. The key was not revoked."
            };
            let result = if create {
                let pipe = || {
                    crate::control::call(&crate::control::Request {
                        cmd: "create_access_key".into(),
                        account_token: token.clone(),
                        label: label.clone(),
                        expires_secs: secs,
                        computer_limit: limit,
                        ..Default::default()
                    })
                };
                let local = || match crate::access_keys::Store::create(&dir, &label, secs, limit) {
                    Ok(secret) => AkResult::Secret {
                        secret,
                        keys: crate::access_keys::Store::public_list(&dir),
                    },
                    Err(e) => AkResult::Note { message: e, keys: Vec::new() },
                };
                if via_pipe {
                    match pipe() {
                        Some(response) if response.ok && !response.secret.is_empty() => AkResult::Secret {
                            secret: response.secret,
                            keys: response.keys,
                        },
                        Some(response) if response.code == "timeout" => {
                            AkResult::Note { message: unanswered.into(), keys: Vec::new() }
                        }
                        None if can_write => local(),
                        Some(response) => AkResult::Note {
                            message: if response.message.is_empty() {
                                "Could not create an access key.".into()
                            } else {
                                response.message
                            },
                            keys: response.keys,
                        },
                        None => AkResult::Note { message: unanswered.into(), keys: Vec::new() },
                    }
                } else {
                    local()
                }
            } else {
                let pipe = || {
                    crate::control::call(&crate::control::Request {
                        cmd: "revoke_access_key".into(),
                        account_token: token,
                        key_id: key_id.clone(),
                        ..Default::default()
                    })
                };
                let local = || match crate::requests::revoke_access_key(&dir, &key_id) {
                    Ok(()) => AkResult::Note {
                        message: "Revoked. Every computer that used it will be disconnected and removed.".into(),
                        keys: crate::access_keys::Store::public_list(&dir),
                    },
                    Err(e) => AkResult::Note { message: format!("Could not revoke that key: {e}"), keys: Vec::new() },
                };
                if via_pipe {
                    match pipe() {
                        Some(response) if response.ok => {
                            AkResult::Note { message: response.message, keys: response.keys }
                        }
                        Some(response) if response.code == "timeout" => {
                            AkResult::Note { message: unanswered.into(), keys: Vec::new() }
                        }
                        None if can_write => local(),
                        Some(response) => AkResult::Note {
                            message: if response.message.is_empty() {
                                "Could not revoke that key.".into()
                            } else {
                                response.message
                            },
                            keys: response.keys,
                        },
                        None => AkResult::Note { message: unanswered.into(), keys: Vec::new() },
                    }
                } else {
                    local()
                }
            };
            let _ = tx.send(result);
        });
        self.ak_rx = Some(rx);
    }

    /// Create, copy, and revoke server access keys. The secret stays in this window.
    fn server_access_keys(&mut self, ui: &mut egui::Ui, can_act: bool) -> Option<AkAct> {
        let mut act = None;
        ui.add_space(10.0);
        heading(ui, "SERVER ACCESS KEYS");
        ui.label(
            RichText::new(
                "A key lets someone use this server without signing in to your account. It is shown once. This server stores only a hash. Network keys are separate and are not changed.",
            )
            .color(p(ui).muted),
        );
        ui.add_space(6.0);
        if !self.ak_secret.is_empty() {
            ui.label(RichText::new("Copy this key now. It is shown once and is not saved here.").color(p(ui).warn));
            let mut shown = self.ak_secret.clone();
            ui.add(
                egui::TextEdit::multiline(&mut shown)
                    .desired_rows(2)
                    .desired_width(420.0)
                    .font(egui::TextStyle::Monospace)
                    .password(!self.ak_show)
                    .interactive(false),
            );
            ui.horizontal(|ui| {
                if ui.button("Copy").clicked() {
                    copy(&self.ak_secret);
                    self.status = "Access key copied.".into();
                }
                let toggle = if self.ak_show { "Hide" } else { "Show" };
                if ui.button(toggle).clicked() {
                    self.ak_show = !self.ak_show;
                }
                if ui.button("Done").clicked() {
                    act = Some(AkAct::Dismiss);
                }
            });
            ui.add_space(6.0);
        }
        if can_act {
            ui.label("Label");
            ui.add(egui::TextEdit::singleline(&mut self.ak_label).hint_text("Ada").desired_width(280.0));
            ui.label("Expires");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.ak_secs, 0, "Never");
                ui.selectable_value(&mut self.ak_secs, 86_400, "1 day");
                ui.selectable_value(&mut self.ak_secs, 7 * 86_400, "7 days");
                ui.selectable_value(&mut self.ak_secs, 30 * 86_400, "30 days");
            });
            ui.label("Computer limit");
            ui.add(egui::TextEdit::singleline(&mut self.ak_limit).hint_text("1").desired_width(80.0));
            let label = if self.ak_busy { "Creating…" } else { "Create key" };
            if ui.add_enabled(!self.ak_busy, egui::Button::new(label)).clicked() {
                act = Some(AkAct::Create);
            }
        } else if self.access != Access::Readable && !self.control_ready {
            ui.label(
                RichText::new("Restart as administrator, or start the PJDesk service, to create a key.")
                    .color(p(ui).muted)
                    .size(12.0),
            );
        }
        let keys = self.shown_keys();
        let now = crate::registry::unix_now();
        if keys.is_empty() {
            ui.label(RichText::new("No access keys yet.").color(p(ui).muted));
        }
        for key in &keys {
            let expired = key.revoked_at == 0 && key.expires_at != 0 && now >= key.expires_at;
            let state = if key.revoked_at != 0 {
                "revoked"
            } else if expired {
                "expired"
            } else {
                "active"
            };
            let until = if key.expires_at == 0 {
                "does not expire".to_string()
            } else {
                format!("expires {}", format_local(key.expires_at))
            };
            let count = if key.computers == 1 { "1 computer".to_string() } else { format!("{} computers", key.computers) };
            ui.add_space(4.0);
            ui.label(RichText::new(&key.label).strong());
            ui.label(RichText::new(format!("{state} · {until} · limit {} · {count}", key.computer_limit.max(1))).color(p(ui).muted).size(12.0));
            if !key.names.is_empty() {
                ui.label(RichText::new(key.names.join(", ")).color(p(ui).text).size(12.0));
            }
            if can_act && key.revoked_at == 0 && !expired {
                let confirming = self.ak_confirm.as_deref() == Some(key.id.as_str());
                ui.horizontal(|ui| {
                    if confirming {
                        ui.label(RichText::new("Revoke this key? Every computer that used it will be disconnected and removed.").color(p(ui).warn));
                        if ui.small_button("Revoke").clicked() {
                            act = Some(AkAct::Revoke(key.id.clone()));
                        }
                        if ui.small_button("Cancel").clicked() {
                            self.ak_confirm = None;
                        }
                    } else if ui.small_button("Revoke").clicked() {
                        self.ak_confirm = Some(key.id.clone());
                    }
                });
            }
        }
        act
    }

    /// Computers that registered without a PJDesk account, and computers Ben removed.
    fn not_signed_in(
        &mut self,
        ui: &mut egui::Ui,
        waiting: &[UnknownComputer],
        blocked: &[BlockedComputer],
        can_act: bool,
    ) -> Option<BlockAct> {
        let mut act = None;
        heading(ui, "NOT SIGNED IN YET");
        ui.label(
            RichText::new(
                "These computers registered without a signed-in PJDesk account. Remove one that is not yours. It is disconnected, its registration is deleted, and it cannot register again until you unblock it.",
            )
            .color(p(ui).muted),
        );
        ui.add_space(6.0);
        if waiting.is_empty() {
            ui.label(RichText::new("No computers are waiting to sign in.").color(p(ui).muted));
        }
        if self.confirm_remove.as_ref().is_some_and(|id| waiting.iter().all(|row| row.id != *id)) {
            self.confirm_remove = None;
        }
        for row in waiting {
            let shown = pjdesk_proto::format_id(&row.id);
            let name = if row.hostname.trim().is_empty() { "Unknown computer".to_string() } else { row.hostname.clone() };
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&shown).monospace().strong());
                ui.label(RichText::new(&name).strong());
                if !row.last_ip.trim().is_empty() {
                    ui.label(RichText::new(row.last_ip.trim()).monospace().color(p(ui).muted));
                }
            });
            let access_label = if row.access_label.is_empty() {
                self.key_label(&row.access_key_id)
            } else {
                row.access_label.clone()
            };
            if !access_label.is_empty() {
                ui.label(RichText::new(format!("Access key: {access_label}")).color(p(ui).text).size(12.0));
            }
            ui.label(RichText::new(computer_facts(&row.last_ip, row.first_seen, row.last_seen, 0)).color(p(ui).muted).size(12.0));
            if can_act {
                let confirming = self.confirm_remove.as_deref() == Some(row.id.as_str());
                ui.horizontal(|ui| {
                    if confirming {
                        ui.label(RichText::new("Remove this computer?").color(p(ui).warn));
                        if ui.small_button("Remove").clicked() {
                            act = Some(BlockAct::Remove(row.id.clone()));
                        }
                        if ui.small_button("Cancel").clicked() {
                            self.confirm_remove = None;
                        }
                    } else if ui.small_button("Remove").clicked() {
                        self.confirm_remove = Some(row.id.clone());
                    }
                });
            }
        }
        if !blocked.is_empty() {
            ui.add_space(10.0);
            heading(ui, "BLOCKED COMPUTERS");
            ui.label(
                RichText::new("These computers cannot register. Unblock lets that computer register again.")
                    .color(p(ui).muted),
            );
            for row in blocked {
                let shown = pjdesk_proto::format_id(&row.id);
                let name = if row.hostname.trim().is_empty() { "Unknown computer".to_string() } else { row.hostname.clone() };
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&shown).monospace());
                    ui.label(&name);
                    if !row.last_ip.trim().is_empty() {
                        ui.label(RichText::new(row.last_ip.trim()).monospace().color(p(ui).muted));
                    }
                    if can_act && ui.small_button("Unblock").clicked() {
                        act = Some(BlockAct::Unblock(row.id.clone()));
                    }
                });
                ui.label(
                    RichText::new(computer_facts(&row.last_ip, row.first_seen, row.last_seen, row.blocked_at))
                        .color(p(ui).muted)
                        .size(12.0),
                );
            }
        }
        act
    }

    fn apply_block(&mut self, act: BlockAct) {
        let (id, removing) = match act {
            BlockAct::Remove(id) => (id, true),
            BlockAct::Unblock(id) => (id, false),
        };
        self.confirm_remove = None;
        let shown = pjdesk_proto::format_id(&id);
        let unanswered = if removing {
            format!("The PJDesk service did not answer. {shown} was not removed.")
        } else {
            format!("The PJDesk service did not answer. {shown} was not unblocked.")
        };
        if !self.owner_uses_pipe() && !self.can_write {
            let cmd = if removing { "block-device" } else { "unblock-device" };
            if run_elevated(&[cmd, &id]) {
                self.status = format!("Asked Windows to approve that for {shown}.");
            } else {
                self.status = unanswered;
            }
            return;
        }
        let dir = self.data_dir.clone();
        let token = self.cloud_token.clone();
        let via_pipe = self.owner_uses_pipe();
        let can_write = self.can_write;
        let cmd = if removing { "block" } else { "unblock" };
        self.status = if removing { format!("Removing {shown}…") } else { format!("Unblocking {shown}…") };
        self.spawn_note(move || {
            if via_pipe {
                match crate::control::call(&crate::control::Request {
                    cmd: cmd.into(),
                    account_token: token,
                    device_id: id.clone(),
                    ..Default::default()
                }) {
                    Some(response) if response.code == "timeout" => return unanswered,
                    Some(response) if response.ok => return response.message,
                    Some(response) => {
                        return if response.message.is_empty() { unanswered } else { response.message };
                    }
                    None if can_write => {}
                    None => return unanswered,
                }
            }
            let result = if removing {
                crate::requests::block_device(&dir, &id)
            } else {
                crate::requests::unblock_device(&dir, &id)
            };
            match result {
                Ok(()) if removing => format!(
                    "Removed {shown}. It is disconnected and cannot register again until it is unblocked."
                ),
                Ok(()) => format!("{shown} can register again."),
                Err(e) if removing => format!("Could not remove {shown}: {e}"),
                Err(e) => format!("Could not unblock {shown}: {e}"),
            }
        });
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
                            .small_button("Update all online computers")
                            .on_hover_text(
                                "Tell every online PJDesk client to download and install the latest release now",
                            )
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
                        let mut seen = if d.last_seen == 0 {
                            String::new()
                        } else if d.first_seen > 0 {
                            format!("Last seen {} ({})\nFirst registered {}", format_local(d.last_seen), ago(d.last_seen), format_local(d.first_seen))
                        } else {
                            format!("Last seen {}", format_local(d.last_seen))
                        };
                        if !d.last_ip.trim().is_empty() {
                            if !seen.is_empty() {
                                seen.push('\n');
                            }
                            seen.push_str(&format!("Last IP {}", d.last_ip.trim()));
                        }
                        if !d.access_label.is_empty() {
                            ui.label(RichText::new(&d.access_label).color(p(ui).text).size(11.0));
                        } else if !d.signed_in {
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
                            last_seen: d.last_seen,
                            first_seen: d.first_seen,
                            signed_in: d.signed_in,
                            access_label: d.access_label.clone(),
                            last_ip: d.last_ip.clone(),
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
                    access_label: self.key_label(&d.access_key_id),
                    last_ip: d.last_ip.clone(),
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
        if version.is_empty() || same_as_this_computer(&name) {
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
        self.request_refresh();
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
        let from_service = self.access != Access::Readable;
        let service_log = if from_service {
            self.live.as_ref().map(|s| s.log_tail.clone()).unwrap_or_default()
        } else {
            self.log_tail.clone()
        };
        card(ui, |ui| {
            heading(ui, "LOG");
            if from_service && service_log.is_empty() {
                self.locked_line(ui);
            } else if service_log.is_empty() {
                ui.label(
                    RichText::new("No log yet (the server writes server.log in its data folder when it runs).")
                        .color(p(ui).muted)
                        .size(12.0),
                );
            } else {
                egui::ScrollArea::vertical().max_height(160.0).id_salt("log").stick_to_bottom(true).show(ui, |ui| {
                    ui.label(RichText::new(&service_log).monospace().size(11.0).color(p(ui).muted));
                });
            }
            ui.add_space(8.0);
            if ui.button("Copy diagnostics").clicked() {
                copy(&diagnostics_text(&service_log, &self.window_log));
                self.status = "Copied diagnostics. Passwords and tokens are left out.".into();
            }
            ui.label(
                RichText::new("This works without administrator rights. The copy leaves out passwords and tokens.")
                    .color(p(ui).muted)
                    .size(12.0),
            );
        });
    }
}

fn log_sign_in_outcome(email: &str, outcome: &anyhow::Result<JobDone>) {
    match outcome {
        Ok(JobDone::Message(msg)) => tracing::info!("sign-in: {email}: {msg}"),
        Ok(JobDone::Ask { .. } | JobDone::ServiceAsk { .. }) => {
            tracing::info!("sign-in: {email}: choose which copy to keep")
        }
        Ok(_) => tracing::info!("sign-in: {email}: finished"),
        Err(err) => tracing::info!("sign-in: {email}: {err:#}"),
    }
}

/// Asks the service to sign in. The password goes to the service and is not logged.
/// The window log records the email and the result text only.
fn sign_in_through_service(email: String, password: String) -> anyhow::Result<JobDone> {
    let outcome = match crate::control::call_for(
        &crate::control::Request {
            cmd: "sign_in".into(),
            email: email.clone(),
            password,
            ..Default::default()
        },
        crate::control::SIGN_IN_WAIT,
    ) {
        Some(response) if response.code == "ask" => Ok(JobDone::ServiceAsk {
            pending: response.pending,
            from: response.from,
            written_at: response.written_at,
            readable: response.readable,
        }),
        Some(response) if response.ok => Ok(JobDone::Message(response.message)),
        Some(response) if response.code == "timeout" || response.message.trim().is_empty() => {
            Err(anyhow::anyhow!("The PJDesk service did not answer. Sign-in was not finished."))
        }
        Some(response) => Err(anyhow::anyhow!(response.message)),
        None => Err(anyhow::anyhow!("The PJDesk service did not answer. Sign-in was not finished.")),
    };
    log_sign_in_outcome(&email, &outcome);
    outcome
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

struct UnknownComputer {
    id: String,
    hostname: String,
    last_ip: String,
    first_seen: u64,
    last_seen: u64,
    access_key_id: String,
    access_label: String,
}

struct BlockedComputer {
    id: String,
    hostname: String,
    last_ip: String,
    first_seen: u64,
    last_seen: u64,
    blocked_at: u64,
}

enum BlockAct {
    Remove(String),
    Unblock(String),
}

enum AkAct {
    Create,
    Revoke(String),
    Dismiss,
}

enum AkResult {
    Secret { secret: String, keys: Vec<pjdesk_proto::signal::AccessKeyBrief> },
    Note { message: String, keys: Vec<pjdesk_proto::signal::AccessKeyBrief> },
}

struct ServiceProbe {
    control_ready: bool,
    installed: bool,
    running: bool,
}

struct FolderView {
    access: Access,
    window_log: String,
    policy: crate::access::Policy,
    watch: crate::public_addr::IpWatch,
    public_address: Option<crate::public_addr::PublicAddress>,
    update_status: crate::self_update::UpdateStatus,
    update_auto: bool,
    update_start: String,
    update_end: String,
    cloud_email: String,
    cloud_token: String,
    cloud_api: String,
    own_bucket: bool,
    can_write: bool,
    fingerprint: String,
    accounts: Vec<accounts::AccountInfo>,
    devices: Vec<DeviceRow>,
    blocked: Vec<BlockedRow>,
    online: HashMap<String, String>,
    log_tail: String,
    cloud_on: bool,
    cloud: Option<crate::cloud::CloudStatus>,
    access_keys: Vec<pjdesk_proto::signal::AccessKeyBrief>,
}

/// Local files only. The control pipe and the service manager are [`request_service_probe`].
fn read_folder(dir: &std::path::Path) -> FolderView {
    let window_log = status_view::redacted_log_tail(&window_log_path());
    let mut access = status_view::access(dir);
    if access == Access::Missing && std::fs::create_dir_all(dir).is_ok() {
        access = status_view::access(dir);
    }
    let policy = crate::access::Policy::load(dir);
    let watch = crate::public_addr::IpWatch::load(dir);
    let public_address = crate::public_addr::load(dir);
    crate::self_update::UpdateStatus::reconcile(dir);
    let update_status = crate::self_update::UpdateStatus::load(dir);
    let (update_auto, update_start, update_end) = match crate::self_update::Settings::read(dir) {
        crate::self_update::Loaded::Present(settings) => (settings.auto, settings.window_start, settings.window_end),
        _ => (false, String::new(), String::new()),
    };
    let (cloud_email, cloud_token, cloud_api, own_bucket) = match crate::cloud::CloudConfig::load(dir) {
        Some(cfg) if cfg.is_account() => {
            let api = if cfg.api.trim().is_empty() {
                crate::cloud::default_api()
            } else {
                cfg.api.trim_end_matches('/').to_string()
            };
            let token = if cfg.token.is_empty() { String::new() } else { cfg.token };
            (cfg.email, token, api, false)
        }
        Some(_) => (String::new(), String::new(), crate::cloud::default_api(), true),
        None => (String::new(), String::new(), crate::cloud::default_api(), false),
    };
    let mut view = FolderView {
        access,
        window_log,
        policy,
        watch,
        public_address,
        update_status,
        update_auto,
        update_start,
        update_end,
        cloud_email,
        cloud_token,
        cloud_api,
        own_bucket,
        can_write: false,
        fingerprint: String::new(),
        accounts: Vec::new(),
        devices: Vec::new(),
        blocked: Vec::new(),
        online: HashMap::new(),
        log_tail: String::new(),
        cloud_on: false,
        cloud: None,
        access_keys: Vec::new(),
    };
    if access != Access::Readable {
        return view;
    }
    view.can_write = std::fs::write(dir.join(".wtest"), b"1").is_ok();
    let _ = std::fs::remove_file(dir.join(".wtest"));
    view.fingerprint = match crate::server::read_identity(dir) {
        Some(id) => pjdesk_proto::tls::format_fingerprint(&id.fingerprint()),
        None => "(unknown — start the server to create it)".into(),
    };
    view.accounts = Accounts::load(dir).map(|a| a.list()).unwrap_or_default();
    if let Ok(reg) = Registry::load(dir.join("devices.json")) {
        view.blocked = reg.blocked();
        view.devices = reg.list();
    }
    view.online = read_online(dir);
    view.log_tail = status_view::redacted_log_tail(&dir.join("server.log"));
    view.access_keys = crate::access_keys::Store::public_list(dir);
    view.cloud_on = dir.join(crate::cloud::CONFIG_FILE).exists();
    view.cloud = crate::cloud::CloudStatus::load(dir);
    view
}

fn unknown_from_devices<'a>(rows: impl Iterator<Item = &'a DeviceRow>) -> Vec<UnknownComputer> {
    rows.map(|d| UnknownComputer {
        id: d.id.clone(),
        hostname: d.hostname.clone(),
        last_ip: d.last_ip.clone(),
        first_seen: d.first_seen,
        last_seen: d.last_seen,
        access_key_id: d.access_key_id.clone(),
        access_label: String::new(),
    })
    .collect()
}

fn unknown_from_listed<'a>(rows: impl Iterator<Item = &'a status_view::ListedDevice>) -> Vec<UnknownComputer> {
    rows.map(|d| UnknownComputer {
        id: d.id.clone(),
        hostname: d.hostname.clone(),
        last_ip: d.last_ip.clone(),
        first_seen: d.first_seen,
        last_seen: d.last_seen,
        access_key_id: String::new(),
        access_label: d.access_label.clone(),
    })
    .collect()
}

fn blocks_from_registry(rows: &[BlockedRow]) -> Vec<BlockedComputer> {
    rows.iter()
        .map(|d| BlockedComputer {
            id: d.id.clone(),
            hostname: d.hostname.clone(),
            last_ip: d.last_ip.clone(),
            first_seen: d.first_seen,
            last_seen: d.last_seen,
            blocked_at: d.blocked_at,
        })
        .collect()
}

fn blocks_from_listed(rows: &[status_view::ListedBlock]) -> Vec<BlockedComputer> {
    rows.iter()
        .map(|d| BlockedComputer {
            id: d.id.clone(),
            hostname: d.hostname.clone(),
            last_ip: d.last_ip.clone(),
            first_seen: d.first_seen,
            last_seen: d.last_seen,
            blocked_at: d.blocked_at,
        })
        .collect()
}

struct RequireResult {
    ok: bool,
    require: bool,
    message: String,
}

/// Names for the confirm line. A computer on a server access key is not listed:
/// turning the setting on leaves that key connected.
fn unsigned_labels(rows: &[UnknownComputer]) -> Vec<String> {
    rows.iter()
        .filter(|row| row.access_key_id.is_empty() && row.access_label.is_empty())
        .map(|row| {
            let id = pjdesk_proto::format_id(&row.id);
            let host = row.hostname.trim();
            if host.is_empty() {
                id
            } else {
                format!("{id} {host}")
            }
        })
        .collect()
}

fn save_require_local(dir: &std::path::Path, require: bool) -> RequireResult {
    let mut policy = crate::access::Policy::load(dir);
    policy.require_accounts = require;
    policy.require_chosen = true;
    if let Err(e) = policy.save(dir) {
        return RequireResult {
            ok: false,
            require,
            message: format!("Could not save the signed-in accounts setting: {e}"),
        };
    }
    if require {
        if let Err(e) = crate::requests::push(dir, crate::requests::REQUIRE_DISCONNECT_FILE, "now") {
            return RequireResult {
                ok: false,
                require,
                message: format!("Saved the setting, but could not ask the server to disconnect unsigned computers: {e}"),
            };
        }
    }
    crate::cloud::push_require_accounts(dir, require);
    RequireResult {
        ok: true,
        require,
        message: if require {
            "Require signed-in accounts is on.".into()
        } else {
            "Require signed-in accounts is off. Already-registered computers keep connecting.".into()
        },
    }
}

fn computer_facts(ip: &str, first_seen: u64, last_seen: u64, blocked_at: u64) -> String {
    let ip = if ip.trim().is_empty() { "unknown" } else { ip.trim() };
    let first = if first_seen == 0 { "unknown".into() } else { format_local(first_seen) };
    let last = if last_seen == 0 { "unknown".into() } else { format!("{} ({})", format_local(last_seen), ago(last_seen)) };
    let mut line = format!("Last IP {ip}   ·   First seen {first}   ·   Last seen {last}");
    if blocked_at > 0 {
        line.push_str(&format!("   ·   Blocked {}", format_local(blocked_at)));
    }
    line
}

struct ShownDevice {
    id: String,
    hostname: String,
    version: String,
    online: bool,
    last_seen: u64,
    first_seen: u64,
    signed_in: bool,
    access_label: String,
    last_ip: String,
}

/// Installed version against the saved update line. Local only: a stale
/// `Installing 0.6.32` does not wait on the service pipe or a new check.
fn shown_update(last_check_at: u64, last_result: &str, available: &str, error: &str) -> crate::self_update::UpdateStatus {
    crate::self_update::UpdateStatus {
        last_check_at,
        last_result: last_result.to_string(),
        available: available.to_string(),
        error: error.to_string(),
    }
    .settled(env!("CARGO_PKG_VERSION"))
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
/// stale file, which means the server isn't running.
fn log_online_refresh(previous: &HashMap<String, String>, next: &HashMap<String, String>) {
    let mut before: Vec<String> = previous.keys().cloned().collect();
    let mut after: Vec<String> = next.keys().cloned().collect();
    before.sort();
    after.sort();
    if before == after {
        return;
    }
    let listed = if after.is_empty() {
        "none".to_string()
    } else {
        after.iter().map(|id| pjdesk_proto::format_id(id)).collect::<Vec<_>>().join(", ")
    };
    tracing::info!("Status refresh from online.json: online {listed}");
}

fn read_online(data_dir: &std::path::Path) -> HashMap<String, String> {
    #[derive(serde::Deserialize)]
    struct OnlineInfo {
        id: String,
        #[serde(default)]
        version: String,
    }
    let path = data_dir.join("online.json");
    if !crate::cloud::online_file_is_fresh(&path) {
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

fn diagnostics_text(service_log: &str, window: &str) -> String {
    let mut out = String::from("PJDesk Server diagnostics\n\n--- server.log ---\n");
    if service_log.is_empty() {
        out.push_str("(not available)\n");
    } else {
        out.push_str(service_log);
        out.push('\n');
    }
    if !window.is_empty() {
        out.push_str("\n--- pjdesk-server.log ---\n");
        out.push_str(&window);
        out.push('\n');
    }
    out
}

fn window_log_path() -> std::path::PathBuf {
    std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("PJDesk")
        .join("pjdesk-server.log")
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

fn this_computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|text| text.trim().to_string()))
        .unwrap_or_default()
}

fn same_as_this_computer(name: &str) -> bool {
    pjdesk_proto::failover::same_computer_name(name, &this_computer_name())
}

fn other_server_name(active_name: &str) -> String {
    if active_name.trim().is_empty() || same_as_this_computer(active_name) {
        "the other server".into()
    } else {
        active_name.trim().to_string()
    }
}

fn brief(id: &str, label: &str) -> pjdesk_proto::signal::AccessKeyBrief {
    pjdesk_proto::signal::AccessKeyBrief { id: id.into(), label: label.into(), computer_limit: 1, ..Default::default() }
}

/// Empty local reads (the window cannot open a SYSTEM-locked file) must not hide
/// keys the service already has. A list that contains a new id wins until the
/// other list catches up. When both have the same ids, the live snapshot wins
/// so computer counts and a revoke show up.
fn keys_to_show(
    local: &[pjdesk_proto::signal::AccessKeyBrief],
    live: &[pjdesk_proto::signal::AccessKeyBrief],
    hold: &[pjdesk_proto::signal::AccessKeyBrief],
) -> Vec<pjdesk_proto::signal::AccessKeyBrief> {
    if !hold.is_empty() && !key_ids_cover(live, hold) && !key_ids_cover(local, hold) {
        return hold.to_vec();
    }
    if local.is_empty() {
        return if live.is_empty() { hold.to_vec() } else { live.to_vec() };
    }
    if live.is_empty() {
        return local.to_vec();
    }
    if key_ids_cover(local, live) && !key_ids_cover(live, local) {
        return local.to_vec();
    }
    live.to_vec()
}

fn key_ids_cover(hay: &[pjdesk_proto::signal::AccessKeyBrief], needle: &[pjdesk_proto::signal::AccessKeyBrief]) -> bool {
    needle.iter().all(|row| hay.iter().any(|have| have.id == row.id))
}

fn hold_covered(live: &[pjdesk_proto::signal::AccessKeyBrief], hold: &[pjdesk_proto::signal::AccessKeyBrief]) -> bool {
    hold.iter().all(|held| {
        live.iter().any(|row| row.id == held.id && row.label == held.label && row.revoked_at == held.revoked_at)
    })
}

fn primary_role_text(epoch: u64) -> String {
    format!("Role: Primary. This server uploads to the cloud (generation {epoch}).")
}

fn standby_role_text(active_name: &str, epoch: u64) -> String {
    format!("Role: Standby. Primary: {} (generation {epoch}).", other_server_name(active_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_access_key_shows_before_the_folder_read_catches_up() {
        let created = brief("aa", "Ada");
        let listed = keys_to_show(&[], &[], &[created.clone()]);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].label, "Ada");
        let stale_local = Vec::new();
        let live_without = Vec::new();
        assert_eq!(keys_to_show(&stale_local, &live_without, &[created.clone()])[0].id, "aa");
        let mut live = created.clone();
        live.computers = 1;
        assert_eq!(keys_to_show(&[], &[live.clone()], &[created.clone()])[0].computers, 1);
        assert!(hold_covered(&[live.clone()], &[created]));
        assert!(keys_to_show(&[brief("bb", "Local")], &[], &[]).iter().any(|row| row.id == "bb"));
        assert_eq!(keys_to_show(&[], &[brief("cc", "Live")], &[])[0].id, "cc");
    }

    #[test]
    fn the_confirm_names_unsigned_computers_and_skips_access_keys() {
        let rows = vec![
            UnknownComputer {
                id: "833201199".into(),
                hostname: "DESKTOP-D7E4AOR".into(),
                last_ip: "203.0.113.8".into(),
                first_seen: 1,
                last_seen: 2,
                access_key_id: String::new(),
                access_label: String::new(),
            },
            UnknownComputer {
                id: "111222333".into(),
                hostname: "Ada-PC".into(),
                last_ip: String::new(),
                first_seen: 1,
                last_seen: 2,
                access_key_id: "key".into(),
                access_label: "Ada".into(),
            },
        ];
        assert_eq!(
            unsigned_labels(&rows),
            vec!["833 201 199 DESKTOP-D7E4AOR".to_string()]
        );
        let facts = computer_facts("203.0.113.8", 0, 0, 0);
        assert!(facts.contains("Last IP 203.0.113.8"));
        assert!(computer_facts("", 0, 0, 0).contains("Last IP unknown"));
    }
}
