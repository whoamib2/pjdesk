//! The PJDesk desktop app: the window you get by running `pjdesk` with no
//! arguments (or double-clicking it).
//!
//! The left rail is this computer. The main column connects to another one.
//! The first outgoing session opens a viewer process. Another computer, while
//! that window is still open, is added as a tab in it. Anything that can
//! block (service control, scheduled tasks, the status file) runs on a
//! background thread; the window only reads the latest snapshot.

mod applock;
mod icons;
mod instance;
mod panels;
mod probe;
mod saved;
mod settings;
mod theme;
mod tray;
mod widgets;

#[cfg(feature = "ui-shots")]
mod shots;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use eframe::egui;
use pjdesk_proto::{format_id, is_valid_id, normalize_id};

use crate::account;
use crate::config::Config;
use crate::host::{self, HostShared};
use crate::signaling::ServerAddr;
use crate::HostArgs;

/// Connect form, or the saved-computers page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MainPage {
    Connect,
    Saved,
}

/// Which computer list is showing beside Connect.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeskTab {
    Saved,
    Recent,
    Lan,
}

fn client_window_options() -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 840.0])
            .with_min_inner_size([saved::MIN_INNER_WIDTH, 680.0])
            .with_title("PJDesk")
            .with_app_id("pjdesk")
            .with_icon(std::sync::Arc::new(app_icon())),
        renderer: preferred_renderer(),
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

/// Tray-only process. Never calls eframe.
pub fn run_tray() {
    tray::run_tray();
}

/// Starts the tray when one is not running, then draws the window in this
/// process. A later click starts `--window` from the tray.
pub fn open_or_forward(server_flag: Option<String>, import_config: Option<String>) -> Result<()> {
    #[cfg(windows)]
    {
        if instance::gui_is_running() {
            instance::signal_show();
            tracing::info!("PJDesk is already open. Asked that window to come forward.");
            return Ok(());
        }
        if tray::tray_is_running() {
            tray::signal_tray();
            tracing::info!("Asked the PJDesk tray to open the window.");
            return Ok(());
        }
        if !tray::spawn_tray() {
            tracing::info!("Could not start the PJDesk tray. The window will still open.");
        }
    }
    run(server_flag, import_config)
}

/// Draws the window once. Closing it returns, and this process exits.
/// The tray is a different process and is not started here.
pub fn run(server_flag: Option<String>, import_config: Option<String>) -> Result<()> {
    // A replacement waits for the frozen window to leave before taking the lock.
    crate::traywatch::wait_for_replaced_process();
    // No flag skips the lock. A second launch wakes the window that is already open.
    if pjdesk_proto::applock::second_window_should_exit(instance::acquire_or_signal()) {
        return Ok(());
    }
    crate::client_update::adopt_completed_update();
    crate::temp_sign::scrub_abandoned();
    let mut config = Config::load().unwrap_or_default();
    if config.decide_saved_sync_on_load() {
        let _ = config.save();
    }
    if let Some(flag) = &server_flag {
        if let Ok(addr) = ServerAddr::parse(flag) {
            config.server = Some(addr.to_string());
            let _ = config.save();
        }
    }
    if config.settings.start_with_windows {
        let _ = settings::set_run_at_login(true);
    }

    let cfg = config.clone();
    let import = import_config;
    let built = eframe::run_native(
        "PJDesk",
        client_window_options(),
        Box::new(move |cc| {
            let mut app = App::new(cc, cfg);
            app.take_installer_key(import);
            Ok(Box::new(app))
        }),
    );
    if let Err(e) = built {
        return Err(anyhow::anyhow!("{e}"));
    }
    tracing::info!("The PJDesk window process is exiting. The tray is a different process and keeps running.");
    Ok(())
}

fn app_icon() -> egui::IconData {
    egui::IconData { rgba: include_bytes!("../../assets/icon_256.rgba").to_vec(), width: 256, height: 256 }
}

fn preferred_renderer() -> eframe::Renderer {
    match std::env::var("PJDESK_RENDERER").as_deref() {
        Ok("glow") => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    }
}

/// Supervises the background host thread (the "share this computer" side).
///
/// When the background service is running it hosts this computer, so the GUI
/// stays out of the way. The service check itself happens on the probe thread;
/// this struct only reacts to the snapshot.
struct Engine {
    shared: Arc<HostShared>,
    server: Option<ServerAddr>,
    service_running: bool,
    /// False until the first probe snapshot. We do not start a host before that,
    /// or we would register the same computer twice when the service is up.
    ready: bool,
    /// Consecutive "service not running" samples after it has been seen running.
    service_down_polls: u32,
}

impl Engine {
    fn idle(config: &Config) -> Self {
        Self {
            shared: Arc::new(HostShared::default()),
            server: ServerAddr::parse(&config.effective_server()).ok(),
            service_running: false,
            ready: false,
            service_down_polls: 0,
        }
    }

    fn on_service(&mut self, config: &Config, hosting: bool) {
        let was_ready = self.ready;
        let window_was_hosting = self.ready && !self.service_running;
        let duty = probe::host_duty(self.ready, self.service_running, self.service_down_polls, hosting);
        self.ready = true;
        self.service_down_polls = duty.down_polls;
        if !was_ready {
            self.service_running = duty.service_running;
            if duty.window_hosts {
                tracing::info!(
                    "The background service is not registered; this window will register this computer with {}",
                    self.server.as_ref().map(|s| s.to_string()).unwrap_or_else(|| "no server".into())
                );
                self.spawn(config);
            }
            return;
        }
        if duty.window_hosts == window_was_hosting && duty.service_running == self.service_running {
            if !hosting && self.service_running {
                tracing::info!(
                    "The background service missed a registration check ({}/{})",
                    duty.down_polls,
                    probe::SERVICE_DOWN_POLLS
                );
            }
            return;
        }
        if duty.window_hosts && !window_was_hosting {
            tracing::info!(
                "The background service missed {} registration checks; this window will register this computer",
                probe::SERVICE_DOWN_POLLS
            );
        } else if !duty.window_hosts && window_was_hosting {
            tracing::info!("The background service is registered; this window stopped registering this computer");
        }
        self.restart(config, duty.service_running);
    }

    fn restart(&mut self, config: &Config, service_running: bool) {
        self.shared.request_stop();
        self.ready = true;
        self.service_running = service_running;
        let shared = Arc::new(HostShared::default());
        self.shared = shared.clone();
        self.server = ServerAddr::parse(&config.effective_server()).ok();
        if !service_running {
            if let Some(server) = self.server.clone() {
                spawn_host(server, config.clone(), shared);
            }
        }
    }

    fn spawn(&mut self, config: &Config) {
        if let Some(server) = self.server.clone() {
            tracing::info!("Registration target (window): {server}");
            spawn_host(server, config.clone(), self.shared.clone());
        } else {
            tracing::warn!("This window did not register; no server address is set");
        }
    }

    fn retarget(&mut self, config: &Config) {
        let running = self.service_running;
        self.restart(config, running);
    }
}

fn spawn_host(server: ServerAddr, config: Config, shared: Arc<HostShared>) {
    let mut args = HostArgs::from_settings(&config.settings);
    args.support_unattended = config.support_unattended();
    std::thread::Builder::new()
        .name("host".into())
        .spawn(move || {
            if let Err(e) = host::run_with(server, config, args, Some(shared.clone())) {
                *shared.last_error.lock().unwrap() = Some(format!("{e:#}"));
            }
        })
        .expect("spawn host thread");
}

enum LoginMsg {
    Ok { display_name: String, notice: String },
    Err(String),
}

/// In-memory sign-in. `snapshot` is this computer before the visit.
struct TempHold {
    snapshot: Config,
    last_activity: Instant,
    warning: bool,
}

pub struct App {
    cfg: Config,
    engine: Engine,
    ctx: egui::Context,

    connect_id: String,
    connect_password: String,
    remember: bool,
    show_password: bool,
    perm_input: String,
    show_perm_input: bool,
    show_connect_password: bool,
    show_login_pass: bool,
    direct_addr: String,
    lan: Vec<(String, String, String)>,
    lan_rx: Option<Receiver<Vec<(String, String, String)>>>,
    /// Stops the background LAN listener. None when discovery is off.
    lan_stop: Option<Arc<AtomicBool>>,
    password_notice: String,
    show_change: bool,
    change_pass: String,
    change_pass2: String,
    show_change_pass: bool,
    change_rx: Option<Receiver<Result<(), String>>>,
    chat_draft: String,

    show_login: bool,
    login_user: String,
    login_pass: String,
    login_busy: bool,
    login_rx: Option<Receiver<LoginMsg>>,

    show_settings: bool,
    settings: settings::SettingsDraft,
    access_panel: crate::access_keys::Panel,
    support_keys: crate::support_keys::Panel,
    show_support_keys: bool,
    support_tip: String,
    book_alias: String,
    book_target: Option<String>,
    desk_tab: DeskTab,
    page: MainPage,
    saved_query: String,
    saved_sort: saved::SavedSort,
    saved_favorites: bool,
    renaming: Option<String>,
    rename_text: String,
    confirm_remove: Option<String>,
    confirm_delete: Option<String>,
    confirm_clear_saved: bool,
    confirm_sign_out_clear: bool,
    delete_rx: Option<Receiver<Result<String, String>>>,
    next_account_pull: u64,
    /// Last self-online value written to the log, so a refresh does not repeat it.
    logged_self_online: Option<bool>,
    prompt_open: bool,
    prompt_id: String,
    prompt_password: String,
    prompt_show: bool,
    prompt_remember: bool,
    prompt_files: bool,
    prompt_rejected: bool,
    prompt_checking: bool,
    prompt_direct: Option<String>,
    /// Viewer is waiting for a password after the other computer answered.
    ask_path: Option<std::path::PathBuf>,
    ask_alive: bool,
    ask_person_next: bool,
    ask_view_next: bool,
    ask_view_label: String,
    viewer_rx: Vec<Receiver<(String, i32, bool, u32)>>,
    /// Process ids of viewer windows this window started. A session-tab offer
    /// is sent only to one of these, so a stale listener file is ignored.
    viewer_pids: Vec<u32>,
    /// TCP to the open session window. The UI thread only polls this.
    admit_rx: Option<Receiver<bool>>,
    admit_job: Option<AdmitJob>,

    /// Background service / scheduled-task snapshot. Never queried on this thread.
    probe: Option<probe::Probe>,
    scm_installed: bool,
    scm_running: bool,
    scm_known: bool,
    svc_status: crate::syspaths::ServiceStatus,

    status: String,
    net_email: String,
    net_pass: String,
    net_show: bool,
    net_code: String,
    net_consent: bool,
    /// 0 account, 1 enrollment code, 2 network key, 3 server access key.
    net_mode: u8,
    net_blob: String,
    /// Server address typed with a server access key. The key itself has no address.
    net_server: String,
    net_key_pass: String,
    net_key_show: bool,
    /// The key is limited to one account, so email and password are shown too.
    net_key_account: bool,
    net_busy: bool,
    net_error: String,
    /// When pjdesk.com paused email sign-in. Sign in stays off until this instant.
    net_pause_until: Option<Instant>,
    show_net_sign_in: bool,
    /// The signed-in account owns an empty network and this computer already has a server.
    net_offer: bool,
    /// Login token for that offer. Not saved and not logged.
    net_login: String,
    net_rx: Option<Receiver<Result<crate::network::SignIn, String>>>,
    /// Fills Saved computers from the account without opening a connection.
    saved_rx: Option<Receiver<Result<crate::network::Session, String>>>,
    /// Network key waiting in the import dialog. Not logged.
    import_blob: Option<String>,
    import_pass: String,
    import_show: bool,
    /// True when a paste offered the import. False when Import was already clicked
    /// and the dialog is only collecting a passphrase.
    import_confirm: bool,
    /// Checked each time the sign-in form opens. Session-only when true.
    net_temporary: bool,
    net_enroll_only: bool,
    pending_temporary: bool,
    pending_enroll_only: bool,
    /// The join in flight used a code or a key, not an email and password.
    pending_without_password: bool,
    /// Set while a temporary sign-in is in this process. The refresh token is not on disk.
    temp: Option<TempHold>,
    session_end: Option<Receiver<()>>,
    temp_left: u64,
    /// Nine-digit id already published for this sign-in. Empty until that succeeds.
    enrolled_for: String,
    enroll_after: Instant,
    enroll_rx: Option<Receiver<bool>>,
    /// Account rows from the last sign-in. A temporary visit uploads these, not the list that was already on this computer.
    account_computers: Vec<crate::network::SavedComputer>,
    import_hint: String,
    /// Blob in flight for the current enroll, stored only after the server accepts it.
    pending_blob: Option<String>,
    net_importing: bool,
    /// A paste the user dismissed, so the same clipboard event is not offered again.
    dismissed_paste: Option<String>,
    toast: widgets::Toast,
    /// A session that ended before the viewer window opened. Stays up until dismissed.
    session_error: Option<String>,
    tray: Option<tray::Tray>,
    logo: Option<egui::TextureHandle>,
    told_tray: bool,
    /// Last time a minimized frame said the loop was alive.
    hidden_log: Option<Instant>,
    /// Show toggle for the support password. Separate from the one-time password.
    show_support_password: bool,
    support_code_in: String,
    support_pass_in: String,
    support_pass_show: bool,
    support_busy: bool,
    support_error: String,
    support_rx: Option<Receiver<Result<(String, String), String>>>,
    /// Last support code pjdesk.com accepted. Empty until that succeeds.
    published_code: String,
    published_id: String,
    published_at: Instant,
    offer_retry: Instant,
    offer_rx: Option<Receiver<Result<(String, String), String>>>,
    show_invite: bool,
    join_id: String,
    join_code: String,
    join_expires: u64,
    join_busy: bool,
    join_error: String,
    join_rx: Option<Receiver<InviteMsg>>,
    /// True after the first lock of this sign-in, so a later frame does not lock again.
    lock_armed: bool,
    lock_ui: bool,
    lock_pin: String,
    lock_show: bool,
    lock_error: String,
    lock_busy: bool,
    lock_forgot: bool,
    lock_password: String,
    lock_pass_show: bool,
    lock_rx: Option<Receiver<applock::LockMsg>>,
    /// Last PIN already sent to a silent hash check. A mismatch is not a failure.
    lock_peeked: String,
    lock_peek_tx: Option<Sender<bool>>,
    lock_peek_rx: Option<Receiver<bool>>,
    lock_pull_rx: Option<Receiver<Result<applock::LockRemote, String>>>,
    next_lock_pull: Instant,
    last_input: Instant,
    minimized_at: Option<Instant>,
    reauth_until: Instant,
    pin_current: String,
    pin_new: String,
    pin_confirm: String,
    pin_password: String,
    /// Typed on Set a PIN when this computer has no account email yet.
    pin_email: String,
    pin_show_current: bool,
    pin_show_new: bool,
    pin_show_confirm: bool,
    pin_show_password: bool,
    pin_note: String,
    pin_idle_minutes: u32,
    /// Account password was accepted. The window stays locked until a new PIN is saved.
    lock_resetting: bool,
    /// The unlock PIN box has been focused for this appearance.
    lock_pin_armed: bool,
    /// The Set-a-PIN field has been focused for this appearance.
    setup_pin_focused: bool,
}

struct AdmitJob {
    id: String,
    password: String,
    files: bool,
    remember: bool,
    from_store: bool,
    direct: String,
}

enum InviteMsg {
    Ready(crate::network::JoinCode),
    Active(crate::network::JoinCode),
    Clear,
    Failed(String),
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, cfg: Config) -> Self {
        Self::create(cc, cfg, false)
    }

    fn create(cc: &eframe::CreationContext<'_>, cfg: Config, preview: bool) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, &cfg.settings.theme);
        let engine = Engine::idle(&cfg);
        let settings = settings::SettingsDraft::from_config(&cfg, None);
        let remember = cfg.settings.remember_passwords;
        let net_email = cfg.network_email.clone().unwrap_or_default();
        let mut app = Self {
            cfg,
            engine,
            ctx: cc.egui_ctx.clone(),
            connect_id: String::new(),
            connect_password: String::new(),
            remember,
            show_password: false,
            perm_input: String::new(),
            show_perm_input: false,
            show_connect_password: false,
            show_login_pass: false,
            direct_addr: String::new(),
            lan: Vec::new(),
            lan_rx: None,
            lan_stop: None,
            password_notice: String::new(),
            show_change: false,
            change_pass: String::new(),
            change_pass2: String::new(),
            show_change_pass: false,
            change_rx: None,
            chat_draft: String::new(),
            show_login: false,
            login_user: String::new(),
            login_pass: String::new(),
            login_busy: false,
            login_rx: None,
            show_settings: false,
            settings,
            access_panel: crate::access_keys::Panel::default(),
            support_keys: crate::support_keys::Panel::default(),
            show_support_keys: false,
            support_tip: String::new(),
            book_alias: String::new(),
            book_target: None,
            desk_tab: DeskTab::Saved,
            page: MainPage::Connect,
            saved_query: String::new(),
            saved_sort: saved::SavedSort::Recent,
            saved_favorites: false,
            renaming: None,
            rename_text: String::new(),
            confirm_remove: None,
            confirm_delete: None,
            confirm_clear_saved: false,
            confirm_sign_out_clear: false,
            delete_rx: None,
            next_account_pull: 0,
            logged_self_online: None,
            prompt_open: false,
            prompt_id: String::new(),
            prompt_password: String::new(),
            prompt_show: false,
            prompt_remember: false,
            prompt_files: false,
            prompt_rejected: false,
            prompt_checking: false,
            prompt_direct: None,
            ask_path: None,
            ask_alive: false,
            ask_person_next: false,
            ask_view_next: false,
            ask_view_label: String::new(),
            viewer_rx: Vec::new(),
            viewer_pids: Vec::new(),
            admit_rx: None,
            admit_job: None,
            probe: if preview { None } else { Some(probe::Probe::start(cc.egui_ctx.clone())) },
            scm_installed: false,
            scm_running: false,
            scm_known: false,
            svc_status: crate::syspaths::ServiceStatus::default(),
            status: String::new(),
            net_email,
            net_pass: String::new(),
            net_show: false,
            net_code: String::new(),
            net_consent: false,
            net_mode: 0,
            net_blob: String::new(),
            net_server: String::new(),
            net_key_pass: String::new(),
            net_key_show: false,
            net_key_account: false,
            net_busy: false,
            net_error: String::new(),
            net_pause_until: None,
            show_net_sign_in: false,
            net_offer: false,
            net_login: String::new(),
            net_rx: None,
            saved_rx: None,
            import_blob: None,
            import_pass: String::new(),
            import_show: false,
            import_confirm: false,
            import_hint: String::new(),
            net_temporary: true,
            net_enroll_only: false,
            pending_temporary: false,
            pending_enroll_only: false,
            pending_without_password: false,
            temp: None,
            session_end: if preview { None } else { Some(crate::temp_sign::watch_session_end()) },
            temp_left: 0,
            enrolled_for: String::new(),
            enroll_after: Instant::now(),
            enroll_rx: None,
            account_computers: Vec::new(),
            pending_blob: None,
            net_importing: false,
            dismissed_paste: None,
            toast: widgets::Toast::default(),
            session_error: None,
            tray: None,
            logo: Some(cc.egui_ctx.load_texture(
                "pjdesk-logo",
                egui::ColorImage::from_rgba_unmultiplied([256, 256], include_bytes!("../../assets/icon_256.rgba")),
                egui::TextureOptions::LINEAR,
            )),
            told_tray: false,
            hidden_log: None,
            show_support_password: false,
            support_code_in: String::new(),
            support_pass_in: String::new(),
            support_pass_show: false,
            support_busy: false,
            support_error: String::new(),
            support_rx: None,
            published_code: String::new(),
            published_id: String::new(),
            published_at: Instant::now(),
            offer_retry: Instant::now(),
            offer_rx: None,
            show_invite: false,
            join_id: String::new(),
            join_code: String::new(),
            join_expires: 0,
            join_busy: false,
            join_error: String::new(),
            join_rx: None,
            lock_armed: false,
            lock_ui: false,
            lock_pin: String::new(),
            lock_show: false,
            lock_error: String::new(),
            lock_busy: false,
            lock_forgot: false,
            lock_password: String::new(),
            lock_pass_show: false,
            lock_rx: None,
            lock_peeked: String::new(),
            lock_peek_tx: None,
            lock_peek_rx: None,
            lock_pull_rx: None,
            next_lock_pull: Instant::now(),
            last_input: Instant::now(),
            minimized_at: None,
            reauth_until: Instant::now(),
            pin_current: String::new(),
            pin_new: String::new(),
            pin_confirm: String::new(),
            pin_password: String::new(),
            pin_email: String::new(),
            pin_show_current: false,
            pin_show_new: false,
            pin_show_confirm: false,
            pin_show_password: false,
            pin_note: String::new(),
            pin_idle_minutes: 5,
            lock_resetting: false,
            lock_pin_armed: false,
            setup_pin_focused: false,
        };
        let lock_email = app.account_email();
        if let Some(verifier) = app.cfg.app_lock_for(&lock_email) {
            app.pin_idle_minutes = (verifier.idle_sec / 60).clamp(1, 24 * 60) as u32;
        }
        if app.cfg.account.is_some() {
            app.start_token_refresh();
        }
        if !preview && app.cfg.release_stuck_pin_prompt() {
            if let Err(err) = app.cfg.save() {
                tracing::info!("Could not record the PIN prompt: {err}");
            }
        }
        if !preview {
            crate::host::direct::set_lan_discovery(app.cfg.settings.discover_lan);
            app.ensure_lan_watch();
        }
        app
    }

    #[cfg(feature = "ui-shots")]
    fn load_preview(&mut self, open_settings: bool) {
        self.engine.ready = true;
        self.engine.service_running = true;
        self.scm_known = true;
        self.scm_installed = true;
        self.scm_running = true;
        self.svc_status = crate::syspaths::ServiceStatus {
            id: Some("902488771".into()),
            online: true,
            connected: None,
            password: Some("a1b2c3d4".into()),
            permanent_set: false,
            rtt_ms: 18,
            reach: "Internet".into(),
            updated_unix: unix_now(),
            support_code: "12345678".into(),
            support_password: "temp-pass".into(),
            support_born_unix: unix_now(),
            acked_unix: unix_now(),
            server: "office.example:21200".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            error: String::new(),
        };
        self.connect_id = "123 456 789".into();
        self.cfg.server = Some("office.example:21200".into());
        self.cfg.network_email = Some("ben@spader.us".into());
        self.cfg.address_book = vec![
            pjdesk_proto::signal::AddressEntry {
                id: "123456789".into(),
                alias: "Office PC".into(),
                password: Some("saved-secret".into()),
                ..Default::default()
            },
            pjdesk_proto::signal::AddressEntry { id: "555666777".into(), alias: "Laptop".into(), ..Default::default() },
        ];
        let now = unix_now();
        self.cfg.settings.peers.insert(
            "123456789".into(),
            crate::config::PeerNote {
                favorite: true,
                last_connected: now.saturating_sub(3600),
                hostname: "OFFICE-PC".into(),
                os_family: "windows".into(),
                os_version: "11".into(),
                ..Default::default()
            },
        );
        self.cfg.settings.peers.insert(
            "555666777".into(),
            crate::config::PeerNote {
                favorite: false,
                last_connected: now.saturating_sub(86_400),
                hostname: "BEN-LAPTOP".into(),
                os_family: "linux".into(),
                os_version: "24.04".into(),
                ..Default::default()
            },
        );
        self.desk_tab = DeskTab::Recent;
        if open_settings {
            self.settings = settings::SettingsDraft::from_config(&self.cfg, Some(true));
            self.settings.preview_security();
            self.show_settings = true;
        }
    }

    #[cfg(feature = "ui-shots")]
    fn load_saved_preview(&mut self) {
        self.load_preview(false);
        if let Some(entry) = self.cfg.address_book.iter_mut().find(|e| e.id == "123456789") {
            entry.alias = "Front desk".into();
        }
        if let Some(entry) = self.cfg.address_book.iter_mut().find(|e| e.id == "555666777") {
            entry.alias.clear();
        }
        self.cfg.address_book.push(pjdesk_proto::signal::AddressEntry {
            id: "444555666".into(),
            alias: "Studio".into(),
            ..Default::default()
        });
        self.cfg.address_book.push(pjdesk_proto::signal::AddressEntry {
            id: "777888999".into(),
            alias: String::new(),
            ..Default::default()
        });
        self.cfg.address_book.push(pjdesk_proto::signal::AddressEntry {
            id: "386626882".into(),
            alias: "386 626 882".into(),
            ..Default::default()
        });
        self.cfg.settings.peers.insert(
            "444555666".into(),
            crate::config::PeerNote {
                favorite: true,
                last_connected: unix_now().saturating_sub(7200),
                hostname: "mac-studio".into(),
                os_family: "macos".into(),
                os_version: "15".into(),
                ..Default::default()
            },
        );
        self.cfg.settings.peers.insert(
            "777888999".into(),
            crate::config::PeerNote {
                hostname: "Pixel-8".into(),
                os_family: "android".into(),
                os_version: "14".into(),
                last_connected: unix_now().saturating_sub(400_000),
                ..Default::default()
            },
        );
        self.cfg.remember_password("123456789", Some("saved-secret"));
        if let Some(note) = self.cfg.settings.peers.get_mut("123456789") {
            note.console_user = "SPADER\\Ben".into();
        }
        self.lan = vec![("OFFICE-PC".into(), "123456789".into(), "192.168.1.20:21201".into())];
        self.page = MainPage::Saved;
        self.desk_tab = DeskTab::Saved;
        self.saved_query.clear();
    }

    #[cfg(feature = "ui-shots")]
    fn load_sign_in_preview(&mut self) {
        self.engine.ready = true;
        self.cfg.server = None;
        self.cfg.device_id = None;
        self.cfg.network_email = None;
        self.net_email = "ben@spader.us".into();
        self.net_pass.clear();
        self.net_show = false;
        self.net_mode = 0;
        self.show_net_sign_in = true;
    }

    #[cfg(feature = "ui-shots")]
    fn load_empty_network_preview(&mut self) {
        self.load_sign_in_preview();
        self.cfg.server = Some(pjdesk_proto::LEGACY_REGISTERED_SERVER.into());
        self.cfg.device_id = Some("123456789".into());
        self.cfg.server_fingerprint = Some("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff".into());
        self.net_error = crate::network::NO_SERVER.into();
        self.net_offer = true;
        self.net_login = "preview".into();
    }

    #[cfg(feature = "ui-shots")]
    fn load_network_key_preview(&mut self) {
        self.load_sign_in_preview();
        self.net_mode = 2;
        self.net_blob = "pjdesk-net1:paste-the-key-here".into();
        self.net_key_pass.clear();
        self.net_key_show = false;
        self.net_key_account = false;
    }

    #[cfg(feature = "ui-shots")]
    fn load_server_config_preview(&mut self) {
        self.cfg.server = Some("office.example:21200".into());
        self.cfg.server_fingerprint = Some("ab:cd:ef:01".into());
        self.cfg.network_key = Some("pjdesk-net1:paste-the-key-here".into());
        self.load_preview(true);
        self.settings.preview_network();
    }

    #[cfg(feature = "ui-shots")]
    fn load_import_offer_preview(&mut self) {
        self.load_sign_in_preview();
        self.import_blob = Some("pjdesk-net1:paste-the-key-here".into());
        self.import_confirm = true;
        self.import_hint = "A network key was pasted. Importing it sets this computer's server and enrolls it.".into();
    }

    #[cfg(feature = "ui-shots")]
    fn load_password_preview(&mut self) {
        self.load_preview(false);
        self.net_pass = "account-secret-not-for-connect".into();
        self.prompt_open = true;
        self.prompt_id = "123456789".into();
        self.prompt_password.clear();
        self.prompt_show = false;
        self.prompt_remember = true;
        self.prompt_rejected = false;
    }

    fn server_addr(&self) -> Option<ServerAddr> {
        ServerAddr::parse(&self.cfg.effective_server()).ok()
    }

    fn save_cfg(&mut self) {
        if self.cfg.ephemeral_network {
            if let Err(e) = self.cfg.save() {
                self.status = format!("Could not save settings: {e}");
            }
            if let Ok(disk) = Config::load() {
                if self.cfg.device_id.is_none() {
                    self.cfg.device_id = disk.device_id;
                }
                if self.cfg.device_secret.is_none() {
                    self.cfg.device_secret = disk.device_secret;
                }
            }
            return;
        }
        self.cfg.reload_identity_from_disk();
        if let Err(e) = self.cfg.save() {
            self.status = format!("Could not save settings: {e}");
        }
    }

    /// The 9-digit ID shown as Your ID, once the server has assigned it.
    fn visible_id(&self) -> String {
        if let Some(id) = &self.cfg.device_id {
            let id = normalize_id(id);
            if is_valid_id(&id) {
                return id;
            }
        }
        if self.engine.service_running {
            if let Some(id) = &self.svc_status.id {
                let id = normalize_id(id);
                if is_valid_id(&id) {
                    return id;
                }
            }
        }
        self.engine.shared.id().map(|id| normalize_id(&id)).filter(|id| is_valid_id(id)).unwrap_or_default()
    }

    fn note_enrolled(&mut self, computers: &[crate::network::SavedComputer]) {
        self.account_computers = computers.to_vec();
        let id = self.visible_id();
        if is_valid_id(&id) && computers.iter().any(|c| normalize_id(&c.id) == id) {
            self.enrolled_for = id;
        } else {
            self.enrolled_for.clear();
        }
    }

    /// Keeps the refresh token in this process. Disk stays as it was before sign-in.
    fn begin_temporary(&mut self, session: crate::network::Session) {
        let snapshot = self.cfg.clone();
        let had = !snapshot.network_refresh_token().is_empty();
        crate::temp_sign::begin_marker(had);
        self.note_enrolled(&session.computers);
        let syncing = self.cfg.syncs_saved_computers();
        self.cfg.ephemeral_network = true;
        if syncing {
            // Show the account's computers for this visit. The list that was
            // already on this computer stays in the snapshot and is not uploaded.
            self.cfg.address_book.clear();
            self.cfg.saved_hidden.clear();
            self.cfg.settings.peers.clear();
            self.cfg.allow_saved_merge();
        } else {
            self.cfg.saved_sync = Some(false);
        }
        self.cfg.apply_network_session(&session);
        if !syncing {
            // Remember "don't download" without writing this visit's tokens.
            let _ = self.cfg.save();
        }
        self.net_email = self.cfg.network_email.clone().unwrap_or_default();
        self.settings.set_server_text(&self.cfg.effective_server());
        self.temp = Some(TempHold { snapshot, last_activity: Instant::now(), warning: false });
        self.temp_left = 0;
        crate::network::copy_to_service(&self.cfg);
        self.engine.retarget(&self.cfg);
        self.toast.show("Signed in until you close PJDesk");
    }

    /// Drops a temporary sign-in. This computer stays in the account.
    fn end_temporary_sign_in(&mut self) {
        let Some(temp) = self.temp.take() else { return };
        self.cfg.ephemeral_network = false;
        let (passwords, recents) = crate::temp_sign::marker_ids();
        let mut disk = Config::load().unwrap_or_else(|_| temp.snapshot.clone());
        crate::temp_sign::restore_after_temporary(&mut disk, &temp.snapshot, &passwords, &recents);
        if disk.device_id.is_none() {
            disk.device_id = self.cfg.device_id.clone();
        }
        if disk.device_secret.is_none() {
            disk.device_secret = self.cfg.device_secret.clone();
        }
        disk.ephemeral_network = false;
        if let Err(e) = disk.save() {
            self.status = format!("Could not save settings: {e}");
        }
        self.cfg = disk;
        crate::network::copy_to_service(&self.cfg);
        self.net_email = self.cfg.network_email.clone().unwrap_or_default();
        self.show_net_sign_in = false;
        self.enrolled_for.clear();
        self.account_computers.clear();
        self.temp_left = 0;
        crate::temp_sign::clear_marker();
        crate::vault::clear_session_key();
        self.engine.retarget(&self.cfg);
        self.settings.set_server_text(&self.cfg.effective_server());
        self.toast.show("Signed out");
    }

    /// Writes the visit's session the same way a normal sign-in does.
    fn stay_signed_in(&mut self) {
        if self.temp.is_none() && !self.cfg.ephemeral_network {
            return;
        }
        self.cfg.ephemeral_network = false;
        crate::vault::store_session_key(&mut self.cfg);
        crate::temp_sign::clear_marker();
        self.temp = None;
        self.temp_left = 0;
        // Save this session directly. The usual save reloads the account that
        // was already on disk, which is empty for this visit.
        if let Err(e) = self.cfg.save() {
            self.status = format!("Could not save settings: {e}");
        }
        crate::network::copy_to_service(&self.cfg);
        self.sync_passwords(true);
        self.toast.show("Signed in on this computer");
    }

    /// Keeps this visit in memory and takes the account off disk.
    fn sign_out_when_closes(&mut self) {
        if self.temp.is_some() || self.cfg.ephemeral_network {
            return;
        }
        let mut snapshot = self.cfg.clone();
        snapshot.ephemeral_network = false;
        snapshot.leave_network_sign_in();
        if let Err(e) = snapshot.save() {
            self.status = format!("Could not save settings: {e}");
            return;
        }
        self.cfg.ephemeral_network = true;
        crate::temp_sign::begin_marker(false);
        self.temp = Some(TempHold { snapshot, last_activity: Instant::now(), warning: false });
        self.temp_left = 0;
        self.toast.show("Signed in until you close PJDesk");
    }

    fn sign_in_duration_menu(&mut self, ui: &mut egui::Ui) {
        let until_close = self.temp.is_some() || self.cfg.ephemeral_network;
        if ui.selectable_label(!until_close, "Stay signed in on this computer").clicked() {
            self.stay_signed_in();
            ui.close();
        }
        if ui.selectable_label(until_close, "Sign out when PJDesk closes").clicked() {
            self.sign_out_when_closes();
            ui.close();
        }
    }

    /// True when Windows locked, signed out, or is shutting down since the last check.
    fn session_ended(&mut self) -> bool {
        let Some(rx) = &self.session_end else { return false };
        let mut ended = false;
        loop {
            match rx.try_recv() {
                Ok(()) => ended = true,
                Err(std::sync::mpsc::TryRecvError::Empty) => return ended,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.session_end = None;
                    return ended;
                }
            }
        }
    }

    fn poll_temporary(&mut self, ctx: &egui::Context) {
        if self.temp.is_none() {
            let _ = self.session_ended();
            return;
        }
        if self.session_ended() {
            self.end_temporary_sign_in();
            return;
        }
        let busy = !self.viewer_rx.is_empty() || self.ask_alive;
        let input = ctx.input(|i| {
            i.pointer.any_pressed()
                || i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Key { pressed: true, .. }
                            | egui::Event::Text(_)
                            | egui::Event::MouseWheel { .. }
                            | egui::Event::Touch { .. }
                            | egui::Event::Paste(_)
                    )
                })
        });
        if busy || input {
            if let Some(temp) = &mut self.temp {
                temp.last_activity = Instant::now();
                temp.warning = false;
            }
            self.temp_left = 0;
        }
        let minutes = self.cfg.settings.temp_idle_minutes;
        let last = self.temp.as_ref().map(|t| t.last_activity).unwrap_or_else(Instant::now);
        match crate::temp_sign::idle_remaining(last, minutes, Instant::now()) {
            Some(0) => self.end_temporary_sign_in(),
            Some(left) => {
                self.temp_left = left;
                if let Some(temp) = &mut self.temp {
                    temp.warning = true;
                }
            }
            None => {
                self.temp_left = 0;
                if let Some(temp) = &mut self.temp {
                    temp.warning = false;
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn temporary_warning(&mut self, ctx: &egui::Context) {
        let show = self.temp.as_ref().is_some_and(|t| t.warning) && self.temp_left > 0;
        if !show {
            return;
        }
        let left = self.temp_left;
        let mut stay = false;
        egui::Window::new("Still there?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(format!("Signing out in {left} seconds because PJDesk is idle."));
                ui.add_space(8.0);
                if ui.button("Stay signed in").clicked() {
                    stay = true;
                }
            });
        if stay {
            if let Some(temp) = &mut self.temp {
                temp.last_activity = Instant::now();
                temp.warning = false;
            }
            self.temp_left = 0;
        }
    }

    /// Publishes this computer once its 9-digit ID exists. A temporary visit
    /// does not upload the saved list that was already on this computer.
    fn poll_self_enroll(&mut self) {
        if let Some(rx) = &self.enroll_rx {
            match rx.try_recv() {
                Ok(true) => self.enroll_rx = None,
                Ok(false) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.enroll_rx = None;
                    self.enrolled_for.clear();
                    self.enroll_after = Instant::now() + Duration::from_secs(30);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
            }
        }
        if Instant::now() < self.enroll_after || self.enroll_rx.is_some() {
            return;
        }
        let signed_in = self.temp.is_some() || !self.cfg.network_refresh_token().is_empty();
        if !signed_in {
            return;
        }
        let id = self.visible_id();
        if !is_valid_id(&id) || self.enrolled_for == id {
            return;
        }
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        if refresh.is_empty() || device.is_empty() {
            return;
        }
        self.enrolled_for = id.clone();
        let mut account = self.account_computers.clone();
        let support = self.cfg.support_grant.clone();
        if let Some(grant) = &support {
            crate::network::ensure_support_computer(&mut account, &id, &grant.label);
        }
        let local = if self.temp.is_some() || support.is_some() || !self.cfg.syncs_saved_computers() {
            Vec::new()
        } else {
            crate::network::saved_from_config(&self.cfg)
        };
        let this = crate::network::ThisComputer::local(&id);
        let (tx, rx) = std::sync::mpsc::channel();
        self.enroll_rx = Some(rx);
        std::thread::Builder::new()
            .name("enroll-self".into())
            .spawn(move || {
                let ok = crate::network::enroll_known(&refresh, &device, &account, &local, &this).is_ok();
                let _ = tx.send(ok);
            })
            .ok();
    }

    /// Re-reads hostname and OS the viewer stored, so the list matches the last connect.
    fn reload_peer_reports(&mut self) {
        if let Ok(disk) = crate::config::Config::load() {
            self.cfg.take_peer_reports_from(&disk);
        }
    }

    fn hosting_by_service(&self) -> bool {
        if self.svc_status.is_hosting() {
            true
        } else if self.scm_known {
            // The probe has reported. SCM Running without a fresh online
            // status means this window is the registrar.
            false
        } else {
            self.engine.service_running
        }
    }

    fn set_permanent_password(&mut self, pw: &str) {
        let pw = pw.trim();
        if pw.is_empty() {
            return;
        }
        if pw.chars().count() < 8 {
            self.status = "Use at least 8 characters for a permanent password.".into();
            return;
        }
        self.cfg.settings.set_permanent_password(Some(pw));
        self.save_cfg();
        if self.scm_known && self.scm_installed {
            crate::platform::run_elevated_with_password(&["service", "configure"], pw);
            self.toast.show("Approve the administrator prompt, then the password is set");
        }
        let cfg = self.cfg.clone();
        self.engine.restart(&cfg, self.hosting_by_service());
        self.perm_input.clear();
        if !(self.scm_known && self.scm_installed) {
            self.toast.show("Permanent password set");
        }
    }

    fn remove_permanent_password(&mut self) {
        self.cfg.settings.set_permanent_password(None);
        self.save_cfg();
        if self.scm_known && self.scm_installed {
            crate::platform::run_elevated(&["service", "configure", "--clear-password"]);
        }
        let cfg = self.cfg.clone();
        self.engine.restart(&cfg, self.hosting_by_service());
        self.toast.show("Permanent password removed");
    }

    fn poll_probe(&mut self) {
        let snap = {
            let Some(probe) = &mut self.probe else { return };
            if !probe.poll() {
                return;
            }
            probe.latest.clone()
        };
        self.scm_installed = snap.installed;
        self.scm_running = snap.running;
        self.scm_known = true;
        self.svc_status = snap.svc;
        let cfg = self.cfg.clone();
        // SCM "Running" only means the service process is up. The window keeps
        // this computer registered until status.json is fresh and online.
        let hosting = probe::service_holds_registration(
            self.svc_status.is_hosting(),
            &self.svc_status.server,
            &cfg.effective_server(),
        );
        self.engine.on_service(&cfg, hosting);
        self.note_self_status();
    }

    /// Online when either the service or this window has a recent server ack.
    /// A fresh service ack counts even when the service's saved address is not
    /// the account's current primary. The window still hosts only when that
    /// address matches.
    fn registrar_acked(&self) -> bool {
        let now = unix_now();
        self.svc_status.registration_fresh(now) || (self.engine.ready && self.engine.shared.acked_recently())
    }

    fn note_self_status(&mut self) {
        let acked = self.registrar_acked();
        if self.svc_status.registration_fresh(unix_now()) {
            let id = self
                .svc_status
                .id
                .clone()
                .or_else(|| self.cfg.device_id.clone())
                .unwrap_or_default();
            let id = pjdesk_proto::normalize_id(&id);
            let reported = self.cfg.settings.peers.get(&id).map(|n| n.app_version.clone()).unwrap_or_default();
            crate::service::ask_registrar_relaunch(&self.svc_status.app_version, &reported);
        }
        if self.logged_self_online == Some(acked) {
            return;
        }
        let id = self.cfg.device_id.clone().unwrap_or_default();
        let account = self.cfg.settings.peers.get(&id).is_some_and(|n| n.reported_online);
        let source = if self.engine.service_running { "the background service" } else { "this window" };
        tracing::info!(
            "This computer is {} according to {source}. The account list says it is {}.",
            if acked { "online" } else { "not registered" },
            if account { "online" } else { "offline" }
        );
        self.logged_self_online = Some(acked);
    }

    fn share_rtt(&self) -> u64 {
        if self.engine.service_running {
            self.svc_status.rtt_ms
        } else {
            self.engine.shared.rtt_ms()
        }
    }

    fn recent_ids(&self) -> Vec<String> {
        let mut ids: Vec<(String, u64)> = self
            .cfg
            .settings
            .peers
            .iter()
            .filter(|(_, note)| note.last_connected > 0)
            .map(|(id, note)| (id.clone(), note.last_connected))
            .collect();
        ids.sort_by(|a, b| b.1.cmp(&a.1));
        ids.into_iter().map(|(id, _)| id).collect()
    }

    fn saved_for(&self, id: &str) -> Option<String> {
        self.cfg
            .address_book
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.password.clone())
            .filter(|p| !p.is_empty())
            .or_else(|| self.cfg.saved_password(id))
    }

    fn mark_recent(&mut self, id: &str) {
        let note = self.cfg.settings.peers.entry(id.to_string()).or_default();
        note.last_connected = unix_now();
        if self.temp.is_some() {
            crate::temp_sign::note_recent(id);
        }
        self.save_cfg();
    }

    fn toggle_favorite(&mut self, id: &str) {
        let note = self.cfg.settings.peers.entry(id.to_string()).or_default();
        note.favorite = !note.favorite;
        let on = note.favorite;
        self.sync_address_book();
        self.toast.show(if on { "Starred" } else { "Star removed" });
    }

    fn start_token_refresh(&mut self) {
        let Some(server) = self.server_addr() else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        self.login_rx = Some(rx);
        self.login_busy = true;
        let mut cfg = self.cfg.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(LoginMsg::Err(e.to_string()));
                    return;
                }
            };
            match rt.block_on(account::refresh(&server, &mut cfg)) {
                Ok(Some(info)) => {
                    let _ = tx.send(LoginMsg::Ok { display_name: info.display_name, notice: info.notice });
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = tx.send(LoginMsg::Err(e.to_string()));
                }
            }
        });
    }

    fn start_login(&mut self, tx: Sender<LoginMsg>) {
        let Some(server) = self.server_addr() else {
            self.status = "Set your server in Settings first.".into();
            return;
        };
        let (user, pass) = (self.login_user.trim().to_string(), self.login_pass.clone());
        if user.is_empty() || pass.is_empty() {
            self.status = "Enter a username and password.".into();
            return;
        }
        self.login_busy = true;
        let mut cfg = self.cfg.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            match rt.block_on(account::login(&server, &mut cfg, &user, &pass)) {
                Ok(info) => {
                    let _ = tx.send(LoginMsg::Ok { display_name: info.display_name, notice: info.notice });
                }
                Err(e) => {
                    let _ = tx.send(LoginMsg::Err(format!("{e:#}")));
                }
            }
        });
    }

    fn poll_login(&mut self) {
        let Some(rx) = &self.login_rx else { return };
        match rx.try_recv() {
            Ok(LoginMsg::Ok { display_name, notice }) => {
                self.login_busy = false;
                self.login_rx = None;
                self.show_login = false;
                self.login_pass.clear();
                let keep_id = (self.cfg.device_id.clone(), self.cfg.device_secret.clone());
                if let Ok(fresh) = Config::load() {
                    self.cfg = fresh;
                    self.cfg.device_id = self.cfg.device_id.clone().or(keep_id.0);
                    self.cfg.device_secret = self.cfg.device_secret.clone().or(keep_id.1);
                }
                if notice.is_empty() {
                    self.toast.show(format!("Signed in as {display_name}"));
                } else {
                    self.password_notice = notice;
                    self.show_change = true;
                    self.toast.show(format!("Signed in as {display_name}"));
                }
            }
            Ok(LoginMsg::Err(e)) => {
                self.login_busy = false;
                self.login_rx = None;
                self.status = e;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.login_busy = false;
                self.login_rx = None;
            }
        }
    }

    fn poll_network(&mut self) {
        let Some(rx) = &self.net_rx else { return };
        match rx.try_recv() {
            Ok(Ok(crate::network::SignIn::NoServer { role, login_token })) => {
                self.net_busy = false;
                self.net_rx = None;
                self.net_error = crate::network::NO_SERVER.to_string();
                let server = self.cfg.effective_server();
                let fingerprint = self.cfg.server_fingerprint.clone().unwrap_or_default();
                self.net_offer = crate::network::offer_current_server(&role, &server, &fingerprint);
                self.net_login = if self.net_offer { login_token } else { String::new() };
            }
            Ok(Ok(crate::network::SignIn::Added { email })) => {
                self.net_busy = false;
                self.net_rx = None;
                self.net_offer = false;
                self.net_login.clear();
                self.pending_blob = None;
                self.net_importing = false;
                self.net_pass.clear();
                self.net_code.clear();
                self.net_consent = false;
                self.support_keys.need_consent = false;
                self.support_keys.enter_consent = false;
                self.support_keys.enter_code.clear();
                self.net_blob.clear();
                self.net_key_pass.clear();
                self.import_pass.clear();
                self.net_error.clear();
                self.net_pause_until = None;
                self.show_net_sign_in = false;
                self.pending_temporary = false;
                self.pending_enroll_only = false;
                self.pending_without_password = false;
                let who = if email.is_empty() { self.net_email.trim().to_string() } else { email };
                self.toast.show(format!(
                    "Added this computer to {who}. Nothing from that sign-in was kept on this computer."
                ));
            }
            Ok(Ok(crate::network::SignIn::Ready(session))) => {
                self.net_busy = false;
                self.net_rx = None;
                self.net_offer = false;
                self.net_login.clear();
                let without_password = self.pending_without_password;
                self.pending_without_password = false;
                if let Some(blob) =
                    self.pending_blob.take().or_else(|| pjdesk_proto::netkey::blob_from_text(&self.net_blob))
                {
                    if !self.pending_temporary {
                        self.cfg.network_key = Some(blob);
                        crate::network::clear_installer_network_key();
                    }
                }
                let imported = self.net_importing;
                self.net_importing = false;
                self.net_pass.clear();
                self.net_code.clear();
                self.net_consent = false;
                self.support_keys.need_consent = false;
                self.support_keys.enter_consent = false;
                self.support_keys.enter_code.clear();
                self.net_blob.clear();
                self.net_key_pass.clear();
                self.import_pass.clear();
                self.net_error.clear();
                self.net_pause_until = None;
                self.show_net_sign_in = false;
                let mut session = session;
                if !without_password {
                    if session.email.is_empty() && self.cfg.network_email.is_none() {
                        self.cfg.network_email = Some(self.net_email.trim().to_string());
                    }
                    if session.email.is_empty() {
                        session.email = self.net_email.trim().to_lowercase();
                    }
                }
                let temporary = self.pending_temporary;
                self.pending_temporary = false;
                self.pending_enroll_only = false;
                if let Some(key) = session.vault_key.take() {
                    crate::vault::set_session_key(key);
                    if !temporary {
                        self.cfg.password_vault_key = Some(crate::secure::protect(&hex::encode(key)));
                    }
                }
                if temporary {
                    self.begin_temporary(session);
                    self.sync_passwords(false);
                    if without_password {
                        self.cfg.joined_without_password = true;
                        self.arrive_without_password();
                    }
                } else {
                    self.cfg.joined_without_password = without_password;
                self.note_enrolled(&session.computers);
                self.cfg.prepare_sign_in_saved_sync();
                self.cfg.apply_network_session(&session);
                self.net_email = self.cfg.network_email.clone().unwrap_or_default();
                self.settings.set_server_text(&self.cfg.effective_server());
                self.save_cfg();
                crate::network::copy_to_service(&self.cfg);
                self.engine.retarget(&self.cfg);
                self.sync_passwords(true);
                if without_password {
                        self.arrive_without_password();
                    } else if imported {
                        self.toast.show("Import server configuration successfully");
                    } else {
                        self.toast.show(format!("Signed in as {}", self.net_email));
                    }
                }
            }
            Ok(Err(e)) => {
                self.net_busy = false;
                self.net_rx = None;
                self.net_importing = false;
                self.pending_blob = None;
                self.pending_without_password = false;
                if e.contains("specific account") {
                    self.show_net_sign_in = true;
                    self.net_mode = 2;
                    self.net_key_account = true;
                }
                self.net_offer = false;
                self.net_login.clear();
                if let Some((owner, label)) = crate::support_keys::parse_consent(&e) {
                    self.net_consent = false;
                    self.support_keys.need_consent = true;
                    self.support_keys.enter_owner = owner.clone();
                    self.support_keys.enter_label = label;
                    self.support_keys.enter_note = format!(
                        "{owner} needs your permission before this computer can be opened without asking."
                    );
                    self.note_sign_in_error(e);
                } else {
                    self.note_sign_in_error(e);
                    self.support_keys.enter_note = self.net_error.clone();
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.net_busy = false;
                self.net_rx = None;
            }
        }
    }

    fn sync_passwords(&mut self, upload: bool) {
        if !self.cfg.syncs_saved_computers() {
            return;
        }
        let Some(key) = crate::vault::key_from(&self.cfg) else { return };
        let refresh = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        let local = if upload { crate::vault::local_entries(&self.cfg) } else { Vec::new() };
        crate::vault::schedule(refresh, device, key, local, upload);
    }

    fn poll_vault(&mut self) {
        let Some(result) = crate::vault::take_result() else { return };
        match result {
            Ok(entries) => {
                let ephemeral = self.cfg.ephemeral_network;
                crate::vault::apply_entries(&mut self.cfg, &entries);
                if ephemeral {
                    for entry in &entries {
                        if !entry.password.is_empty() {
                            crate::temp_sign::note_password(&entry.id);
                        }
                    }
                } else if let Err(e) = self.cfg.save() {
                    self.status = format!("Could not save settings: {e}");
                }
            }
            Err(e) => {
                tracing::info!("password vault: {e}");
            }
        }
    }

    fn start_network_sign_in(&mut self) {
        if self.net_busy {
            return;
        }
        if self.net_mode == 0 && self.sign_in_paused().is_some() {
            return;
        }
        if (self.net_mode == 2 || self.net_mode == 3) && self.route_pasted_key() {
            return;
        }
        let enroll_only = self.net_enroll_only;
        self.net_enroll_only = false;
        let pjdesk_id = self.visible_id();
        if enroll_only && !pjdesk_proto::is_valid_id(&pjdesk_id) {
            self.net_error =
                "This computer does not have an ID yet. Wait until Your ID appears, then try again.".into();
            return;
        }
        self.pending_temporary = self.net_temporary && !enroll_only;
        self.pending_enroll_only = enroll_only;
        self.pending_without_password = self.net_mode == 1 || self.net_mode == 2;
        let this = crate::network::ThisComputer::local(&pjdesk_id);
        let local = if self.pending_temporary || enroll_only || !self.cfg.syncs_saved_computers() {
            Vec::new()
        } else {
            crate::network::saved_from_config(&self.cfg)
        };
        let device = self.cfg.network_device();
        let name = crate::hostname();
        let (tx, rx) = std::sync::mpsc::channel();
        self.net_rx = Some(rx);
        self.net_busy = true;
        self.net_error.clear();
        self.net_offer = false;
        self.net_login.clear();
        if self.net_mode == 1 {
            let code = self.net_code.clone();
            let consent = self.net_consent;
            let pjdesk_for_key = pjdesk_id.clone();
            std::thread::Builder::new()
                .name("network-sign-in".into())
                .spawn(move || {
                    let result = match crate::network::enroll(&code, &device, &name, &pjdesk_for_key, consent) {
                        Ok(session) => {
                            let upload = if session.support.is_some() { Vec::new() } else { local };
                            crate::network::finish_account_sign_in(session, &this, &upload, enroll_only)
                                .map_err(|e| e.to_string())
                        }
                        Err(e) => Err(e.to_string()),
                    };
                    let _ = tx.send(result);
                })
                .ok();
        } else if self.net_mode == 2 {
            let blob = self.net_blob.clone();
            if let Some(found) = pjdesk_proto::netkey::blob_from_text(&blob) {
                self.pending_blob = Some(found);
            }
            let passphrase = self.net_key_pass.clone();
            let email = self.net_email.clone();
            let password = self.net_pass.clone();
            let account = self.net_key_account;
            std::thread::Builder::new()
                .name("network-sign-in".into())
                .spawn(move || {
                    let creds = if account { Some((email.as_str(), password.as_str())) } else { None };
                    let result = match crate::network::redeem(&blob, &passphrase, &device, &name, creds) {
                        Ok(session) => crate::network::finish_account_sign_in(session, &this, &local, enroll_only)
                            .map_err(|e| e.to_string()),
                        Err(e) => Err(e.to_string()),
                    };
                    let _ = tx.send(result);
                })
                .ok();
        } else {
            let email = self.net_email.clone();
            let password = self.net_pass.clone();
            std::thread::Builder::new()
                .name("network-sign-in".into())
                .spawn(move || {
                    let result = match crate::network::sign_in(&email, &password, &device, &name) {
                        Ok(crate::network::SignIn::Ready(session)) => {
                            crate::network::finish_account_sign_in(session, &this, &local, enroll_only)
                                .map_err(|e| e.to_string())
                        }
                        Ok(other) => Ok(other),
                        Err(e) => Err(e.to_string()),
                    };
                    let _ = tx.send(result);
                })
                .ok();
        }
    }

    /// An access key enrolls here. A network key pasted on the access-key form is sent back.
    /// Returns true when redeem must not run.
    fn route_pasted_key(&mut self) -> bool {
        let kind = pjdesk_proto::netkey::classify_paste(&self.net_blob);
        match (self.net_mode, kind) {
            (_, pjdesk_proto::netkey::PasteKind::Access) => {
                let key = self.net_blob.clone();
                self.apply_server_access_key(&key);
                true
            }
            (_, pjdesk_proto::netkey::PasteKind::AccessIncomplete) => {
                self.net_mode = 3;
                self.show_net_sign_in = true;
                self.net_error = pjdesk_proto::netkey::ACCESS_KEY_INCOMPLETE.into();
                true
            }
            (2, pjdesk_proto::netkey::PasteKind::Neither) => {
                self.net_error = pjdesk_proto::netkey::KEY_NOT_EITHER.into();
                true
            }
            (3, pjdesk_proto::netkey::PasteKind::Network) => {
                self.net_mode = 2;
                self.net_error = pjdesk_proto::netkey::NETWORK_KEY_GOES.into();
                true
            }
            (3, pjdesk_proto::netkey::PasteKind::Neither) => {
                self.net_error = "This is not a server access key. A server access key starts with pjdesk-key1. A network key starts with pjdesk-net1 and goes in Use a network key.".into();
                true
            }
            _ => false,
        }
    }

    /// Saves a server access key and registers this computer without a PJDesk account.
    /// Expiry and the computer limit are enforced by the server when it registers.
    fn apply_server_access_key(&mut self, raw: &str) {
        let Some(key) = pjdesk_proto::netkey::access_key_from_text(raw) else {
            self.net_mode = 3;
            self.show_net_sign_in = true;
            self.net_error = pjdesk_proto::netkey::ACCESS_KEY_INCOMPLETE.into();
            return;
        };
        let typed = self.net_server.trim();
        let address = if !typed.is_empty() { typed.to_string() } else { self.cfg.effective_server() };
        if address.is_empty() {
            self.net_mode = 3;
            self.net_blob = key;
            self.show_net_sign_in = true;
            self.net_error = pjdesk_proto::netkey::ACCESS_KEY_NEEDS_SERVER.into();
            return;
        }
        let parsed = match ServerAddr::parse(&address) {
            Ok(addr) => addr.to_string(),
            Err(err) => {
                self.net_mode = 3;
                self.net_blob = key;
                self.show_net_sign_in = true;
                self.net_error = format!("Server address: {err}");
                return;
            }
        };
        if self.cfg.server.as_deref().map(str::trim) != Some(parsed.as_str()) {
            self.cfg.server_fingerprint = None;
            self.cfg.server = Some(parsed);
        }
        self.cfg.set_server_access_key(&key);
        if crate::vault::key_from(&self.cfg).is_none() || self.cfg.joined_without_password {
            self.cfg.joined_without_password = true;
        }
        if let Err(err) = self.cfg.save() {
            self.net_error = format!("Could not save the access key: {err}");
            return;
        }
        crate::network::copy_to_service(&self.cfg);
        self.engine.retarget(&self.cfg);
        self.net_busy = false;
        self.net_blob.clear();
        self.net_error.clear();
        self.show_net_sign_in = false;
        self.arrive_without_password();
    }

    /// A code or a key is connected. The main window follows. No account password and no PIN.
    fn arrive_without_password(&mut self) {
        self.lock_ui = false;
        self.lock_armed = true;
        self.lock_resetting = false;
        self.pin_note.clear();
        self.toast.show(pjdesk_proto::applock::connected_confirmation(
            &self.account_email(),
            &self.cfg.effective_server(),
        ));
    }

    fn claim_current_server(&mut self) {
        if self.net_busy || self.net_login.is_empty() {
            return;
        }
        let token = std::mem::take(&mut self.net_login);
        let address = self.cfg.effective_server();
        let fingerprint = self.cfg.server_fingerprint.clone().unwrap_or_default();
        let device = self.cfg.network_device();
        let name = crate::hostname();
        let enroll_only = self.pending_enroll_only;
        let this = crate::network::ThisComputer::local(&self.visible_id());
        let local = if self.pending_temporary || enroll_only || !self.cfg.syncs_saved_computers() {
            Vec::new()
        } else {
            crate::network::saved_from_config(&self.cfg)
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.net_rx = Some(rx);
        self.net_busy = true;
        self.net_error.clear();
        std::thread::Builder::new()
            .name("network-claim".into())
            .spawn(move || {
                let result = match crate::network::claim_current_server(&token, &address, &fingerprint, &device, &name)
                {
                    Ok(session) => crate::network::finish_account_sign_in(session, &this, &local, enroll_only)
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(result);
            })
            .ok();
    }

    /// Applies a network key carried by the installer file name or `--config`.
    /// An embedded key enrolls immediately. A passphrase key asks for the
    /// passphrase first. The blob stays encrypted on disk until then.
    fn take_installer_key(&mut self, explicit: Option<String>) {
        let Some(blob) = crate::network::installer_network_key(explicit.as_deref()) else { return };
        if self.cfg.network_key.as_deref() == Some(blob.as_str()) && !self.cfg.network_refresh_token().is_empty() {
            crate::network::clear_installer_network_key();
            return;
        }
        if pjdesk_proto::netkey::is_embedded(&blob) {
            self.begin_import(blob, String::new());
            return;
        }
        if pjdesk_proto::netkey::needs_passphrase(&blob) {
            self.import_blob = Some(blob);
            self.import_pass.clear();
            self.import_show = false;
            self.import_confirm = false;
            self.import_hint =
                "This copy of PJDesk includes a network key. Enter the passphrase that was shared separately.".into();
        }
    }

    fn begin_import(&mut self, blob: String, passphrase: String) {
        if self.net_busy {
            return;
        }
        self.import_blob = None;
        self.import_hint.clear();
        self.net_blob = blob;
        self.net_key_pass = passphrase;
        self.net_mode = 2;
        self.net_key_account = false;
        self.net_importing = true;
        self.net_error.clear();
        self.start_network_sign_in();
    }

    fn import_server_config(&mut self) {
        let text = crate::clip::get_text().unwrap_or_default();
        match pjdesk_proto::netkey::classify_paste(&text) {
            pjdesk_proto::netkey::PasteKind::Access => self.apply_server_access_key(&text),
            pjdesk_proto::netkey::PasteKind::AccessIncomplete => {
                self.toast.show(pjdesk_proto::netkey::ACCESS_KEY_INCOMPLETE);
            }
            pjdesk_proto::netkey::PasteKind::Network => {
                if let Some(blob) = pjdesk_proto::netkey::blob_from_text(&text) {
                    self.queue_import(blob, true);
                }
            }
            pjdesk_proto::netkey::PasteKind::Neither => {
                let message = if text.trim().is_empty() {
                    "Clipboard is empty"
                } else {
                    pjdesk_proto::netkey::KEY_NOT_EITHER
                };
                self.toast.show(message);
            }
        }
    }

    fn export_server_config(&mut self) {
        let Some(blob) = self.cfg.network_key.clone().filter(|s| pjdesk_proto::netkey::blob_from_text(s).is_some())
        else {
            self.toast.show("There is no server config to export.");
            return;
        };
        if widgets::copy_to_clipboard(&blob) {
            self.toast.show("Export server configuration successfully");
        } else {
            self.toast.show("Could not copy the server config.");
        }
    }

    /// `immediate` is the Settings button: apply a self-contained key at once.
    /// A paste on the sign-in screen always offers first.
    fn queue_import(&mut self, blob: String, immediate: bool) {
        if pjdesk_proto::netkey::needs_passphrase(&blob) {
            self.import_blob = Some(blob);
            self.import_pass.clear();
            self.import_show = false;
            self.import_confirm = !immediate;
            self.import_hint = if immediate {
                "Enter the passphrase for this server config. It was shared separately from the key.".into()
            } else {
                "A network key was pasted. Importing it sets this computer's server and enrolls it.".into()
            };
            return;
        }
        if !pjdesk_proto::netkey::is_embedded(&blob) {
            self.toast.show("Invalid server configuration");
            return;
        }
        if immediate {
            self.begin_import(blob, String::new());
        } else {
            self.import_blob = Some(blob);
            self.import_pass.clear();
            self.import_show = false;
            self.import_confirm = true;
            self.import_hint =
                "A network key was pasted. Importing it sets this computer's server and enrolls it.".into();
        }
    }

    fn showing_sign_in(&self) -> bool {
        self.cfg.effective_server().trim().is_empty() || self.show_net_sign_in
    }

    fn notice_sign_in_paste(&mut self, ctx: &egui::Context) {
        if !self.showing_sign_in() {
            return;
        }
        let pasted = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Paste(text) => Some(text.clone()),
                _ => None,
            })
        });
        let Some(text) = pasted else { return };
        match pjdesk_proto::netkey::classify_paste(&text) {
            pjdesk_proto::netkey::PasteKind::Access | pjdesk_proto::netkey::PasteKind::AccessIncomplete => {
                self.net_blob = text;
                self.net_mode = 3;
                self.show_net_sign_in = true;
                if self.net_server.trim().is_empty() {
                    self.net_server = self.cfg.effective_server();
                }
                let key = self.net_blob.clone();
                self.apply_server_access_key(&key);
            }
            pjdesk_proto::netkey::PasteKind::Network => {
                let Some(blob) = pjdesk_proto::netkey::blob_from_text(&text) else { return };
                if self.dismissed_paste.as_deref() == Some(blob.as_str()) {
                    return;
                }
                self.queue_import(blob, false);
            }
            pjdesk_proto::netkey::PasteKind::Neither => {}
        }
    }

    fn dismiss_import(&mut self) {
        self.dismissed_paste = self.import_blob.clone();
        self.import_blob = None;
        self.import_pass.clear();
        self.import_hint.clear();
        self.import_show = false;
    }

    fn poll_password(&mut self) {
        let Some(rx) = &self.change_rx else { return };
        match rx.try_recv() {
            Ok(Ok(())) => {
                self.change_rx = None;
                self.show_change = false;
                self.change_pass.clear();
                self.change_pass2.clear();
                self.status.clear();
                let keep_id = (self.cfg.device_id.clone(), self.cfg.device_secret.clone());
                if let Ok(fresh) = Config::load() {
                    self.cfg = fresh;
                    self.cfg.device_id = self.cfg.device_id.clone().or(keep_id.0);
                    self.cfg.device_secret = self.cfg.device_secret.clone().or(keep_id.1);
                }
                self.toast.show("Password updated");
            }
            Ok(Err(e)) => {
                self.change_rx = None;
                self.show_change = true;
                self.status = e;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.change_rx = None;
            }
        }
    }

    /// Leaves the PJDesk account. A normal sign-out keeps the saved list.
    /// A temporary sign-in clears the saved list, cached passwords, and recents.
    fn sign_out_network(&mut self) {
        if self.temp.is_some() {
            self.end_temporary_sign_in();
            return;
        }
        self.cfg.reload_identity_from_disk();
        self.cfg.leave_network_sign_in();
        crate::network::copy_to_service(&self.cfg);
        self.engine.retarget(&self.cfg);
        self.show_net_sign_in = false;
        if let Err(e) = self.cfg.save() {
            self.status = format!("Could not save settings: {e}");
        }
        self.toast.show("Signed out");
    }

    /// Clears the saved list on this computer and leaves the account signed in.
    /// The usual save reloads peer notes from disk, which would put the list back.
    fn clear_saved_on_this_computer(&mut self) {
        self.cfg.clear_local_saved();
        self.cfg.saved_sync = Some(false);
        if let Some(temp) = &mut self.temp {
            temp.snapshot.clear_local_saved();
            temp.snapshot.saved_sync = Some(false);
        }
        if let Err(e) = self.cfg.save() {
            self.status = format!("Could not save settings: {e}");
        }
        self.toast.show("Removed saved computers from this computer");
    }

    /// Signs out on this computer and clears the saved list, passwords, and recents.
    /// The pjdesk.com account and other devices are left as they are.
    fn sign_out_and_clear_local(&mut self) {
        if self.temp.is_some() {
            self.temp = None;
            self.temp_left = 0;
            crate::temp_sign::clear_marker();
        }
        self.cfg.ephemeral_network = false;
        self.cfg.clear_local_saved();
        self.cfg.saved_sync = Some(false);
        self.cfg.leave_network_sign_in();
        crate::vault::clear_session_key();
        if let Err(e) = self.cfg.save() {
            self.status = format!("Could not save settings: {e}");
        }
        crate::network::copy_to_service(&self.cfg);
        self.engine.retarget(&self.cfg);
        self.net_email.clear();
        self.show_net_sign_in = false;
        self.enrolled_for.clear();
        self.account_computers.clear();
        self.toast.show("Signed out. Saved computers were removed from this computer");
    }

    fn request_saved_refresh(&mut self) {
        self.next_account_pull = 0;
    }

    /// Downloads the account list once and keeps syncing it on this device.
    fn load_saved_computers(&mut self) {
        self.cfg.saved_sync = Some(true);
        self.cfg.local_saved_cleared = false;
        self.cfg.network_list_merged = false;
        if let Err(e) = self.cfg.save() {
            self.status = format!("Could not save settings: {e}");
        }
        self.request_saved_refresh();
        self.sync_passwords(false);
        self.toast.show("Loading saved computers");
    }

    fn maybe_pull_account(&mut self) {
        if (self.temp.is_some() && !self.cfg.syncs_saved_computers()) || self.saved_rx.is_some() || self.delete_rx.is_some() {
            return;
        }
        let refresh_token = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        if refresh_token.is_empty() || device.is_empty() {
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if now < self.next_account_pull {
            return;
        }
        self.next_account_pull = now.saturating_add(20);
        let pjdesk_id = self.cfg.device_id.clone().unwrap_or_default();
        let (tx, rx) = std::sync::mpsc::channel();
        self.saved_rx = Some(rx);
        std::thread::spawn(move || {
            let result = crate::network::pull_list(&refresh_token, &device, &pjdesk_id).map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }

    fn poll_saved_refresh(&mut self) {
        let Some(rx) = &self.saved_rx else { return };
        match rx.try_recv() {
            Ok(Ok(mut session)) => {
                self.saved_rx = None;
                if session.email.is_empty() {
                    session.email = self.cfg.network_email.clone().unwrap_or_default();
                }
                let revoked = session.access.is_empty() && session.refresh.is_empty();
                if revoked {
                    self.cfg.leave_network_sign_in();
                    let _ = self.cfg.save();
                    crate::network::copy_to_service(&self.cfg);
                    self.engine.retarget(&self.cfg);
                    return;
                }
                let before = self.cfg.effective_server();
                self.cfg.apply_network_session(&session);
                let after = self.cfg.effective_server();
                tracing::info!("{}", saved::presence_refresh_line(&session.presence));
                if before != after {
                    tracing::info!("The account server is {after}; registering with it again");
                    let cfg = self.cfg.clone();
                    self.engine.retarget(&cfg);
                }
                let _ = self.cfg.save();
                self.note_self_status();
            }
            Ok(Err(message)) => {
                self.saved_rx = None;
                tracing::info!("Status refresh from the account failed: {message}");
                if message.contains("Sign in required") || message.to_ascii_lowercase().contains("revoked") {
                    self.cfg.leave_network_sign_in();
                    let _ = self.cfg.save();
                    crate::network::copy_to_service(&self.cfg);
                    self.engine.retarget(&self.cfg);
                    self.toast.show("Signed out");
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.saved_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn start_account_delete(&mut self, id: String) {
        let refresh_token = self.cfg.network_refresh_token();
        let device = self.cfg.network_device_id.clone().unwrap_or_default();
        if refresh_token.is_empty() || device.is_empty() {
            self.status = "Sign in to delete a computer from your account.".into();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.delete_rx = Some(rx);
        std::thread::spawn(move || {
            let result = crate::network::delete_from_account(&refresh_token, &device, &id).map_err(|e| e.to_string());
            let _ = tx.send(result.map(|_| id));
        });
    }

    fn poll_account_delete(&mut self) {
        let Some(rx) = &self.delete_rx else { return };
        match rx.try_recv() {
            Ok(Ok(id)) => {
                self.delete_rx = None;
                self.cfg.forget_account_computer(&id);
                let _ = self.cfg.save();
                self.next_account_pull = 0;
                self.toast.show("Deleted from your account");
            }
            Ok(Err(message)) => {
                self.delete_rx = None;
                self.status = message;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.delete_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn sign_out(&mut self) {
        if let Some(server) = self.server_addr() {
            let mut cfg = self.cfg.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                let _ = rt.block_on(account::logout(&server, &mut cfg));
            });
        }
        self.cfg.account = None;
        self.cfg.address_book.clear();
        self.save_cfg();
        self.toast.show("Signed out");
    }

    fn sync_address_book(&mut self) {
        self.save_cfg();
        std::thread::spawn(|| {
            let _ = crate::network::push_saved_computers_from_disk();
        });
        if self.cfg.account.is_none() {
            return;
        }
        let Some(server) = self.server_addr() else { return };
        let mut cfg = self.cfg.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let _ = rt.block_on(account::push_address_book(&server, &mut cfg));
        });
    }

    /// Asks the other computer to show a view prompt. No password is sent.
    fn request_view(&mut self, id: &str) {
        let id = normalize_id(id);
        if !is_valid_id(&id) {
            self.status = "That isn't a 9-digit PJDesk ID.".into();
            return;
        }
        let label = self.row_title(&id);
        self.status = format!("Waiting for {label} to accept");
        self.ask_view_label = label;
        self.ask_view_next = true;
        self.spawn_viewer(&id, "", false, false, false);
    }

    fn connect_to(&mut self, id: &str, password: Option<String>, files: bool) {
        let id = normalize_id(id);
        if !is_valid_id(&id) {
            self.status = "That isn't a 9-digit PJDesk ID.".into();
            return;
        }
        let typed = password.unwrap_or_default();
        let from_store = typed.trim().is_empty();
        let password = if from_store { self.cfg.saved_password(&id).unwrap_or_default() } else { typed };
        if password.is_empty() {
            self.spawn_ask(&id, files, None);
            return;
        }
        let remember = !from_store && self.prompt_remember;
        self.spawn_viewer(&id, &password, files, remember, from_store);
    }

    fn open_prompt(&mut self, id: &str, files: bool, rejected: bool) {
        self.prompt_open = true;
        self.prompt_id = id.to_string();
        // The PJDesk account password is a different field and is never copied here.
        self.prompt_password.clear();
        self.prompt_show = false;
        self.prompt_remember = self.cfg.settings.remember_passwords;
        self.prompt_files = files;
        self.prompt_rejected = rejected;
    }

    /// Starts the viewer. A remembered password is written by the viewer only
    /// after the other computer accepts it (`PJDESK_REMEMBER`). A password that
    /// came from storage is marked `PJDESK_SAVED`; if it is rejected the viewer
    /// exits 3 and this window asks again.
    fn spawn_viewer(&mut self, id: &str, password: &str, files: bool, remember: bool, from_store: bool) {
        if !password.is_empty() && self.offer_session_tab(id, password, files, remember, from_store, "") {
            self.ask_person_next = false;
            self.ask_view_next = false;
            return;
        }
        self.launch_viewer_process(id, password, files, remember, from_store);
    }

    fn launch_viewer_process(&mut self, id: &str, password: &str, files: bool, remember: bool, from_store: bool) {
        let server = self.cfg.effective_server();
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.status = format!("Can't find the PJDesk program: {e}");
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--server").arg(server).arg("connect").arg(id);
        if !password.is_empty() {
            cmd.env("PJDESK_PASSWORD", password);
        }
        if let Some(path) = &self.ask_path {
            if password.is_empty() {
                cmd.env("PJDESK_ASK", path);
            }
        }
        if files {
            cmd.env("PJDESK_OPEN_PANEL", "files");
        }
        if from_store {
            cmd.env("PJDESK_SAVED", "1");
        }
        if self.ask_person_next {
            cmd.env("PJDESK_ASK_PERSON", "1");
            self.ask_person_next = false;
        }
        if self.ask_view_next {
            cmd.env("PJDESK_ASK_VIEW", "1");
            if !self.ask_view_label.trim().is_empty() {
                cmd.env("PJDESK_PEER_LABEL", self.ask_view_label.trim());
            }
            self.ask_view_next = false;
        }
        if remember {
            cmd.env("PJDESK_REMEMBER", "1");
            if self.temp.is_some() {
                crate::temp_sign::note_password(id);
            }
        }
        if self.temp.is_some() {
            let token = self.cfg.network_access_token();
            if !token.is_empty() {
                cmd.env("PJDESK_ACCESS", token);
            }
        }
        if !self.net_email.trim().is_empty() {
            cmd.env("PJDESK_ACCOUNT", self.net_email.trim());
        }
        if let Some(note) = self.cfg.settings.peers.get(id) {
            cmd.env("PJDESK_VIEW", note.view_mode.to_string());
            cmd.env("PJDESK_DISPLAY", note.last_display.to_string());
        }
        prepare_viewer(&mut cmd);
        match cmd.spawn() {
            Ok(child) => {
                clear_session_status();
                self.mark_recent(id);
                let what = if files { "Opening file transfer for" } else { "Opening" };
                self.toast.show(format!("{what} {}", format_id(id)));
                self.track_viewer(id.to_string(), child, files);
            }
            Err(e) => self.fail_session(format!("Could not open the viewer: {e}")),
        }
    }

    /// Hands this computer to the session window that is already open.
    fn offer_session_tab(
        &mut self,
        id: &str,
        password: &str,
        files: bool,
        remember: bool,
        from_store: bool,
        direct: &str,
    ) -> bool {
        if password.is_empty() || self.viewer_pids.is_empty() {
            return false;
        }
        let Some((port, pid)) = crate::session_window::read_listener() else { return false };
        if !self.viewer_pids.contains(&pid) {
            return false;
        }
        let (view, display) = self
            .cfg
            .settings
            .peers
            .get(id)
            .map(|note| (note.view_mode, note.last_display))
            .unwrap_or((0, 0));
        let label = if direct.is_empty() { self.row_title(id) } else { direct.to_string() };
        let admit = crate::session_window::Admit {
            id: id.to_string(),
            password: password.to_string(),
            server: self.cfg.effective_server(),
            direct: direct.to_string(),
            label,
            files,
            remember,
            from_store,
            force_relay: false,
            view,
            display,
        };
        if self.admit_rx.is_some() {
            return true;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let started = std::thread::Builder::new()
            .name("session-admit".into())
            .spawn(move || {
                let _ = tx.send(crate::session_window::send_admit(port, &admit));
            })
            .is_ok();
        if !started {
            return false;
        }
        self.admit_rx = Some(rx);
        self.admit_job = Some(AdmitJob {
            id: id.to_string(),
            password: password.to_string(),
            files,
            remember,
            from_store,
            direct: direct.to_string(),
        });
        let what = if files { "Opening file transfer for" } else { "Opening" };
        let who = if direct.is_empty() { format_id(id) } else { direct.to_string() };
        self.toast.show(format!("{what} {who}"));
        true
    }

    /// The session window's reply. A miss starts a new viewer process.
    fn poll_admit(&mut self) {
        let Some(rx) = &self.admit_rx else { return };
        let accepted = match rx.try_recv() {
            Ok(accepted) => accepted,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        };
        self.admit_rx = None;
        let Some(job) = self.admit_job.take() else { return };
        if !accepted {
            if job.direct.is_empty() {
                self.launch_viewer_process(&job.id, &job.password, job.files, job.remember, job.from_store);
            } else {
                self.launch_direct_process(&job.direct, &job.password);
            }
            return;
        }
        clear_session_status();
        if job.direct.is_empty() {
            self.mark_recent(&job.id);
        }
    }

    fn track_viewer(&mut self, id: String, mut child: std::process::Child, files: bool) {
        let pid = child.id();
        self.viewer_pids.push(pid);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let code = child.wait().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
            let _ = tx.send((id, code, files, pid));
        });
        self.viewer_rx.push(rx);
    }

    fn poll_client_update(&mut self) {
        let note = crate::client_update::note();
        if note.is_empty() || note == self.status {
            return;
        }
        self.status = note;
    }

    fn poll_session_status(&mut self) {
        if self.viewer_rx.is_empty() && !self.ask_alive {
            return;
        }
        let Some(text) = read_session_status() else { return };
        if text != self.status {
            self.status = text;
        }
    }

    fn note_viewer_exit(&mut self, id: &str, code: i32, files: bool, reopen: &mut Option<(String, bool)>) {
        self.reload_peer_reports();
        if code == 0 {
            return;
        }
        let reason = last_session_reason().unwrap_or_else(|| {
            if code == 3 { "Wrong password".into() } else { "The session ended before it could start.".into() }
        });
        // A wrong password comes back in the password dialog.
        // Any other failure is a dialog, not only the Opening toast.
        if code == 3 {
            self.status = reason;
            *reopen = Some((id.to_string(), files));
        } else {
            self.fail_session(reason);
            if code == 4 {
                self.prompt_open = false;
                self.prompt_checking = false;
                self.ask_alive = false;
            }
        }
    }

    fn poll_viewers(&mut self) {
        let pending = std::mem::take(&mut self.viewer_rx);
        let mut reopen = None;
        if let Some((id, code, files)) = crate::session_window::take_admit_result() {
            self.note_viewer_exit(&id, code, files, &mut reopen);
        }
        for rx in pending {
            match rx.try_recv() {
                Ok((id, code, files, pid)) => {
                    self.viewer_pids.retain(|existing| *existing != pid);
                    self.note_viewer_exit(&id, code, files, &mut reopen);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => self.viewer_rx.push(rx),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if let Some((id, files)) = reopen {
            self.ask_alive = false;
            self.prompt_checking = false;
            self.prompt_id = id;
            self.prompt_files = files;
            self.prompt_rejected = true;
            self.prompt_open = true;
            if self.prompt_password.is_empty() {
                self.prompt_show = false;
            }
        }
    }

    /// Starts a viewer that asks for the password only after the other computer answers.
    fn spawn_ask(&mut self, id: &str, files: bool, direct: Option<String>) {
        let path = std::env::temp_dir().join(format!("pjdesk-ask-{}", std::process::id()));
        let _ = std::fs::write(&path, "wait\n");
        let mut reply = path.as_os_str().to_os_string();
        reply.push(".reply");
        let _ = std::fs::remove_file(reply);
        self.ask_path = Some(path);
        self.ask_alive = true;
        self.prompt_open = false;
        self.prompt_checking = false;
        self.prompt_rejected = false;
        self.prompt_password.clear();
        self.prompt_id = id.to_string();
        self.prompt_files = files;
        self.prompt_direct = direct.clone();
        self.prompt_remember = self.cfg.settings.remember_passwords;
        if let Some(addr) = direct {
            self.spawn_direct_viewer(&addr);
        } else {
            self.spawn_viewer(id, "", files, false, false);
        }
    }

    fn poll_ask(&mut self) {
        let Some(path) = self.ask_path.clone() else { return };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.starts_with("accepted") {
            if self.prompt_remember && self.prompt_direct.is_none() {
                let id = normalize_id(&self.prompt_id);
                if is_valid_id(&id) && !self.prompt_password.is_empty() {
                    self.cfg.remember_password(&id, Some(&self.prompt_password));
                    let _ = self.cfg.save();
                    self.sync_passwords(true);
                }
            }
            self.prompt_open = false;
            self.prompt_checking = false;
            self.ask_path = None;
            self.ask_alive = false;
            return;
        }
        if text.starts_with("ready") || text.starts_with("rejected") {
            if !self.prompt_open {
                self.prompt_open = true;
                self.prompt_remember = self.cfg.settings.remember_passwords;
            }
            if text.starts_with("rejected") {
                self.prompt_rejected = true;
                self.prompt_checking = false;
            }
        }
    }

    fn write_ask_reply(&self, body: &str) {
        let Some(path) = &self.ask_path else { return };
        let mut reply = path.as_os_str().to_os_string();
        reply.push(".reply");
        let _ = std::fs::write(reply, body);
    }

    /// A computer found on the LAN. Always asks for its password and connects
    /// with `--direct`. No account, and the password is not in the beacon.
    fn connect_discovered(&mut self, id: &str, addr: &str) {
        let id = normalize_id(id);
        let addr = addr.trim();
        if addr.is_empty() {
            self.status = "That computer did not give a network address.".into();
            return;
        }
        self.spawn_ask(&id, false, Some(addr.to_string()));
    }

    fn connect_direct(&mut self, addr: &str) {
        let addr = addr.trim().to_string();
        if addr.is_empty() {
            self.status = "Enter an address, such as 192.168.1.20.".into();
            return;
        }
        self.spawn_ask(&addr, false, Some(addr.clone()));
    }

    fn spawn_direct_with_password(&mut self, addr: &str, password: &str) {
        if self.offer_session_tab(addr, password, false, false, false, addr) {
            return;
        }
        self.launch_direct_process(addr, password);
    }

    fn launch_direct_process(&mut self, addr: &str, password: &str) {
        let server = self.cfg.effective_server();
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.status = format!("Can't find the PJDesk program: {e}");
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--server").arg(server).arg("connect").arg("--direct").arg(addr);
        cmd.env("PJDESK_PASSWORD", password);
        if !self.net_email.trim().is_empty() {
            cmd.env("PJDESK_ACCOUNT", self.net_email.trim());
        }
        prepare_viewer(&mut cmd);
        match cmd.spawn() {
            Ok(child) => self.track_viewer(addr.to_string(), child, false),
            Err(e) => self.fail_session(format!("Could not open the viewer: {e}")),
        }
    }

    fn spawn_direct_viewer(&mut self, addr: &str) {
        let server = self.cfg.effective_server();
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.status = format!("Can't find the PJDesk program: {e}");
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--server").arg(server).arg("connect").arg("--direct").arg(addr);
        if let Some(path) = &self.ask_path {
            cmd.env("PJDESK_ASK", path);
        }
        if !self.net_email.trim().is_empty() {
            cmd.env("PJDESK_ACCOUNT", self.net_email.trim());
        }
        prepare_viewer(&mut cmd);
        match cmd.spawn() {
            Ok(child) => {
                self.toast.show(format!("Opening {addr}"));
                self.track_viewer(addr.to_string(), child, false);
            }
            Err(e) => self.fail_session(format!("Could not open the viewer: {e}")),
        }
    }

    fn find_on_lan(&mut self) {
        if !self.cfg.settings.discover_lan {
            self.status = "LAN discovery is turned off in Settings.".into();
            return;
        }
        self.ensure_lan_watch();
        self.desk_tab = DeskTab::Lan;
        if self.lan.is_empty() {
            self.status = "Looking for computers on this network…".into();
        } else {
            self.status = format!("Found {} computer(s) on this network.", self.lan.len());
        }
    }

    /// One listener while the window is open and discovery is on.
    /// A later result updates the list in place and does not change the tab.
    fn ensure_lan_watch(&mut self) {
        if !self.cfg.settings.discover_lan {
            self.stop_lan_watch();
            return;
        }
        if self.lan_stop.is_some() && self.lan_rx.is_some() {
            return;
        }
        self.stop_lan_watch();
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        if std::thread::Builder::new()
            .name("lan-discover".into())
            .spawn(move || crate::host::direct::listen_discovered(stop_thread, tx))
            .is_err()
        {
            return;
        }
        self.lan_stop = Some(stop);
        self.lan_rx = Some(rx);
    }

    fn stop_lan_watch(&mut self) {
        if let Some(stop) = self.lan_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.lan_rx = None;
        self.lan.clear();
    }

    fn poll_lan(&mut self) {
        enum Heard {
            None,
            Rows(Vec<(String, String, String)>),
            Dead,
        }
        let heard = {
            let Some(rx) = &self.lan_rx else { return };
            let mut latest = None;
            let mut dead = false;
            loop {
                match rx.try_recv() {
                    Ok(rows) => latest = Some(rows),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        dead = true;
                        break;
                    }
                }
            }
            if dead {
                Heard::Dead
            } else if let Some(rows) = latest {
                Heard::Rows(rows)
            } else {
                Heard::None
            }
        };
        match heard {
            Heard::None => {}
            Heard::Dead => {
                self.lan_rx = None;
                if let Some(stop) = self.lan_stop.take() {
                    stop.store(true, Ordering::Relaxed);
                }
                self.ensure_lan_watch();
            }
            Heard::Rows(rows) => {
                self.lan = rows;
                self.note_lan_names();
            }
        }
    }

    /// A computer already saved learns its name from a LAN announcement when it has none yet.
    fn note_lan_names(&mut self) {
        let mut changed = false;
        let found = self.lan.clone();
        for (name, id, addr) in found {
            let id = normalize_id(&id);
            let name = name.trim();
            let addr = addr.trim();
            if !is_valid_id(&id) {
                continue;
            }
            let known = self.cfg.address_book.iter().any(|e| e.id == id) || self.cfg.settings.peers.contains_key(&id);
            if !known {
                continue;
            }
            let note = self.cfg.settings.peers.entry(id).or_default();
            if !name.is_empty() && name != "Computer" && note.hostname.is_empty() {
                note.hostname = name.to_string();
                changed = true;
            }
            if !addr.is_empty() && !note.lan_addrs.iter().any(|have| pjdesk_proto::routes::same_endpoint(have, addr)) {
                note.lan_addrs.push(addr.to_string());
                if note.lan_addrs.len() > 8 {
                    let drop = note.lan_addrs.len() - 8;
                    note.lan_addrs.drain(0..drop);
                }
                changed = true;
            }
        }
        if changed {
            self.save_cfg();
        }
    }
}

impl App {
    /// Allow or Decline for a connection that still needs the person here.
    /// The in-process host posts it on the shared state. The service writes
    /// `consent.json` for this window.
    fn consent_prompt(&mut self, ctx: &egui::Context) {
        let shared = self.engine.shared.pending_consent();
        let file = read_service_consent();
        let (from, ip, id, from_file) = if let Some(ask) = shared {
            (ask.from, ask.ip, ask.id, false)
        } else if let Some(ask) = file {
            (ask.from, ask.ip, ask.id, true)
        } else {
            ctx.request_repaint_after(Duration::from_millis(400));
            return;
        };
        raise_consent(ctx, id);
        let mut allow = false;
        let mut decline = false;
        egui::Window::new("Allow this connection?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!("{from} ({ip}) wants to control this computer."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Decline").clicked() {
                        decline = true;
                    }
                    if widgets::accent_button(ui, "Allow", egui::Vec2::new(120.0, 36.0)) {
                        allow = true;
                    }
                });
            });
        if allow || decline {
            if from_file {
                write_service_consent(allow);
            } else {
                self.engine.shared.answer_consent(id, allow);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

#[derive(serde::Deserialize)]
struct ServiceConsent {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    from: String,
    #[serde(default)]
    ip: String,
}

fn raise_consent(ctx: &egui::Context, id: u64) {
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if LAST.swap(id, std::sync::atomic::Ordering::SeqCst) == id {
        return;
    }
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    #[cfg(windows)]
    unsafe {
        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(
            windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK,
        );
    }
}

fn poll_client_heartbeat() {
    let dir = crate::syspaths::service_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("client-alive"), b"1");
}

fn adopt_service_notice() {
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = crate::syspaths::service_dir().join("connect-notice.json");
    let Ok(bytes) = std::fs::read(&path) else {
        if LAST.swap(0, std::sync::atomic::Ordering::SeqCst) != 0 {
            crate::host::notice::end();
        }
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
    let seq = value.get("seq").and_then(|v| v.as_u64()).unwrap_or(1);
    let prev = LAST.swap(seq, std::sync::atomic::Ordering::SeqCst);
    if prev == seq {
        return;
    }
    let account = value.get("account").and_then(|v| v.as_str()).unwrap_or("");
    let computer = value.get("computer").and_then(|v| v.as_str()).unwrap_or("");
    let user = value.get("user").and_then(|v| v.as_str()).unwrap_or("");
    let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if prev == 0 {
        crate::host::notice::begin(computer);
    }
    crate::host::notice::update(account, computer, user, id);
}

fn read_service_consent() -> Option<ServiceConsent> {
    let path = crate::syspaths::service_dir().join("consent.json");
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_service_consent(accept: bool) {
    let path = crate::syspaths::service_dir().join("consent-reply.json");
    let body = serde_json::json!({
        "id": 1,
        "accept": accept,
        "permissions": if accept { pjdesk_proto::perms::KEYBOARD | pjdesk_proto::perms::MOUSE } else { 0 },
        "receive_dir": "",
    });
    let _ = std::fs::write(path, body.to_string());
}

/// Every viewer this window starts has null stdin, stdout, and stderr.
///
/// View phone is hidden and has no console. Inheriting this process's
/// handles would let the child treat a dead console as its terminal.
fn prepare_viewer(cmd: &mut std::process::Command) {
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        allow_viewer_foreground();
        cmd.creation_flags(0x0800_0000);
    }
}

impl App {
    fn fail_session(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = text.clone();
        self.session_error = Some(text);
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn session_error_dialog(&mut self, ctx: &egui::Context) {
        let Some(text) = self.session_error.clone() else { return };
        let mut close = false;
        egui::Window::new("Could not start the session")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(&text);
                ui.add_space(12.0);
                if widgets::accent_button(ui, "OK", egui::Vec2::new(120.0, 36.0)) {
                    close = true;
                }
            });
        if close {
            self.session_error = None;
        }
    }
}

/// Show the window from the tray. This runs on the window thread. It answers
/// the tray watch first, then asks for the window to be visible and focused,
/// then restores it. The PIN lock, when there is one, is drawn inside it.
fn reveal_window(ctx: &egui::Context, reason: &str) {
    crate::traywatch::ack();
    crate::winshow::note_user_open();
    // Put the taskbar button back before the restore looks the window up.
    crate::winshow::reveal_on_taskbar("PJDesk");
    ctx.request_repaint();
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    // The tray thread already posted the restore. This thread focuses the
    // viewport and draws the PIN lock. Opening does not sign this computer out.
    crate::winshow::activate(reason, "PJDesk", instance::holds_window());
}

/// Lets the viewer process bring its window in front of this one.
#[cfg(windows)]
fn allow_viewer_foreground() {
    use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    unsafe {
        let _ = AllowSetForegroundWindow(0xFFFF_FFFF);
    }
}

fn clear_session_status() {
    let path = crate::config::Config::path();
    let Some(dir) = path.parent() else { return };
    let _ = std::fs::remove_file(dir.join("session-status.txt"));
    let _ = std::fs::remove_file(dir.join("last-session.txt"));
}

fn read_session_status() -> Option<String> {
    let path = crate::config::Config::path().parent()?.join("session-status.txt");
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn last_session_reason() -> Option<String> {
    let path = crate::config::Config::path().parent()?.join("last-session.txt");
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

impl App {
    /// Code, temporary password, this computer's ID, and whether this process
    /// is hosting (the service hosts otherwise).
    fn support_view(&self) -> (Option<String>, Option<String>, String, bool) {
        let id = self.visible_id();
        if self.engine.service_running {
            let code = self.svc_status.support_code.clone();
            let password = self.svc_status.support_password.clone();
            ((!code.is_empty()).then_some(code), (!password.is_empty()).then_some(password), id, false)
        } else {
            let code = self.engine.shared.support_code();
            let password = self.engine.shared.support_password();
            ((!code.is_empty()).then_some(code), (!password.is_empty()).then_some(password), id, true)
        }
    }

    fn refresh_support_pair(&mut self) {
        if self.engine.service_running {
            let path = crate::syspaths::service_dir().join("support.refresh");
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, b"1\n");
        } else {
            self.engine.shared.request_support_refresh();
        }
        self.toast.show("New support code");
    }

    fn poll_support_offer(&mut self) {
        if let Some(rx) = &self.offer_rx {
            match rx.try_recv() {
                Ok(Ok((code, id))) => {
                    self.offer_rx = None;
                    self.published_code = code;
                    self.published_id = id;
                    self.published_at = Instant::now();
                }
                Ok(Err(_)) => {
                    self.offer_rx = None;
                    self.offer_retry = Instant::now() + Duration::from_secs(30);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.offer_rx = None;
                    self.offer_retry = Instant::now() + Duration::from_secs(30);
                }
            }
        }
        if Instant::now() < self.offer_retry || self.offer_rx.is_some() {
            return;
        }
        let (code, _, id, _) = self.support_view();
        let Some(code) = code.filter(|code| code.len() == 8) else { return };
        if !pjdesk_proto::is_valid_id(&id) {
            return;
        }
        let stale = self.published_at.elapsed() >= Duration::from_secs(4 * 60);
        if self.published_code == code && self.published_id == id && !stale {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.offer_rx = Some(rx);
        let send_code = code.clone();
        let send_id = id.clone();
        let spawned = std::thread::Builder::new().name("support-offer".into()).spawn(move || {
            let result = crate::network::offer_support(&send_code, &send_id)
                .map(|_| (send_code, send_id))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        if spawned.is_err() {
            self.offer_rx = None;
            self.offer_retry = Instant::now() + Duration::from_secs(30);
        }
    }

    fn revoke_published_support(&mut self) {
        let code = if self.published_code.len() == 8 {
            self.published_code.clone()
        } else {
            return;
        };
        let id = self.published_id.clone();
        self.published_code.clear();
        self.published_id.clear();
        let _ = crate::network::revoke_support(&code, &id);
    }

    fn start_support_connect(&mut self) {
        if self.support_busy {
            return;
        }
        let code = self.support_code_in.clone();
        let password = self.support_pass_in.trim().to_string();
        let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() != 8 || password.is_empty() {
            self.support_error = "Enter the 8-digit code and the temporary password.".into();
            return;
        }
        self.support_busy = true;
        self.support_error.clear();
        let (tx, rx) = std::sync::mpsc::channel();
        self.support_rx = Some(rx);
        std::thread::Builder::new()
            .name("support-lookup".into())
            .spawn(move || {
                let result =
                    crate::network::lookup_support(&digits).map(|id| (id, password)).map_err(|e| e.to_string());
                let _ = tx.send(result);
            })
            .ok();
    }

    fn poll_support_connect(&mut self) {
        let Some(rx) = &self.support_rx else { return };
        match rx.try_recv() {
            Ok(Ok((id, password))) => {
                self.support_rx = None;
                self.support_busy = false;
                self.connect_with_typed(&id, &password);
            }
            Ok(Err(message)) => {
                self.support_rx = None;
                self.support_busy = false;
                self.support_error = message;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.support_rx = None;
                self.support_busy = false;
                self.support_error = "Could not reach pjdesk.com.".into();
            }
        }
    }

    /// Connects with a password typed for this attempt. It is not saved.
    fn connect_with_typed(&mut self, id: &str, password: &str) {
        let id = normalize_id(id);
        if !pjdesk_proto::is_valid_id(&id) {
            self.status = "That isn't a 9-digit PJDesk ID.".into();
            return;
        }
        self.status = format!("Connecting to {}", format_id(&id));
        self.spawn_viewer(&id, password, false, false, false);
    }

    fn open_invite(&mut self) {
        self.show_invite = true;
        self.join_error.clear();
        if self.join_code.is_empty() && self.join_rx.is_none() {
            self.start_invite(InviteKind::List);
        }
    }

    fn start_invite(&mut self, kind: InviteKind) {
        if self.join_busy {
            return;
        }
        let id = self.join_id.clone();
        let mut cfg = self.cfg.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.join_rx = Some(rx);
        self.join_busy = true;
        self.join_error.clear();
        std::thread::Builder::new()
            .name("invite-code".into())
            .spawn(move || {
                crate::network::refresh_config(&mut cfg);
                let refresh = cfg.network_refresh_token();
                let device = cfg.network_device_id.clone().unwrap_or_default();
                let msg = match kind {
                    InviteKind::Create => match crate::network::create_join_code(&refresh, &device) {
                        Ok(code) => InviteMsg::Ready(code),
                        Err(e) => InviteMsg::Failed(e.to_string()),
                    },
                    InviteKind::List => match crate::network::list_join_codes(&refresh, &device) {
                        Ok(codes) => match codes.into_iter().next() {
                            Some(code) => InviteMsg::Active(code),
                            None => InviteMsg::Clear,
                        },
                        Err(e) => InviteMsg::Failed(e.to_string()),
                    },
                    InviteKind::Revoke => match crate::network::revoke_join_code(&refresh, &device, &id) {
                        Ok(()) => InviteMsg::Clear,
                        Err(e) => InviteMsg::Failed(e.to_string()),
                    },
                };
                let _ = tx.send(msg);
            })
            .ok();
    }

    fn poll_invite(&mut self) {
        let Some(rx) = &self.join_rx else { return };
        match rx.try_recv() {
            Ok(InviteMsg::Ready(code)) => {
                self.join_rx = None;
                self.join_busy = false;
                self.join_id = code.id;
                self.join_code = code.code;
                self.join_expires = code.expires_at;
            }
            Ok(InviteMsg::Active(code)) => {
                self.join_rx = None;
                self.join_busy = false;
                if self.join_code.is_empty() {
                    self.join_id = code.id;
                    self.join_expires = code.expires_at;
                }
            }
            Ok(InviteMsg::Clear) => {
                self.join_rx = None;
                self.join_busy = false;
                self.join_id.clear();
                self.join_code.clear();
                self.join_expires = 0;
            }
            Ok(InviteMsg::Failed(message)) => {
                tracing::warn!("Invite code failed: {message}");
                self.join_rx = None;
                self.join_busy = false;
                self.join_error = message;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.join_rx = None;
                self.join_busy = false;
            }
        }
    }

    /// Tray clicks arrive from the other process. Close exits this process.
    /// The tray is not in this process, so nothing here calls `run_native` again.
    fn on_tray(&mut self, ctx: &egui::Context) {
        let tray_actions = tray::gui_actions();
        for action in tray_actions {
            match action {
                tray::Action::Show => {
                    tracing::info!("Tray click reached PJDesk. Showing the window. The PIN lock is shown when one is set.");
                    reveal_window(ctx, "Open");
                    self.lock_from_tray();
                }
                tray::Action::Update => {
                    tracing::info!("Tray click: Check for updates. Showing the window.");
                    reveal_window(ctx, "Check for updates");
                    if self.signed_in() {
                        self.lock_from_tray();
                    } else {
                        self.show_settings = true;
                        self.settings.show_about();
                    }
                    crate::client_update::start();
                }
                tray::Action::Revoke => self.revoke_support_access(),
                tray::Action::Quit => {
                    tracing::info!("The tray asked this window to exit.");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let closing = ctx.input(|i| i.viewport().close_requested());
        if closing && self.temp.is_some() {
            self.end_temporary_sign_in();
        }
        if closing && !self.told_tray {
            self.told_tray = true;
            tracing::info!("The window was closed. This process is exiting. The tray is a different process and keeps running.");
            self.toast.show("PJDesk stays in the tray.");
        }
        self.note_if_minimized(ctx);
    }

    /// A minimized window can still be asked to present. Say so, and do not
    /// schedule another present from here.
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

enum InviteKind {
    Create,
    List,
    Revoke,
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        // X exits this process. A temporary sign-in ends. The saved account
        // in the config file stays. The tray is a different process.
        if self.engine.service_running {
            let path = crate::syspaths::service_dir().join("support.refresh");
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, b"1\n");
        } else {
            self.engine.shared.request_support_refresh();
        }
        self.revoke_published_support();
        self.end_temporary_sign_in();
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ctx = ctx.clone();
        self.on_tray(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.ctx = ui.ctx().clone();
        let ctx = self.ctx.clone();
        self.poll_login();
        self.poll_network();
        self.poll_vault();
        self.poll_temporary(&ctx);
        self.poll_self_enroll();
        self.maybe_pull_account();
        self.poll_saved_refresh();
        self.poll_account_delete();
        self.poll_password();
        self.poll_probe();
        self.poll_ask();
        self.poll_session_status();
        self.poll_viewers();
        self.poll_admit();
        self.poll_client_update();
        self.poll_support_offer();
        self.poll_support_connect();
        self.poll_invite();
        self.poll_support_keys();
        self.poll_support_leave();
        self.poll_app_lock();
        self.note_lock_visibility(&ctx);
        self.notice_sign_in_paste(&ctx);

        if ctx
            .input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Comma)))
            && !self.show_settings
            && !self.lock_ui
        {
            self.open_settings();
        }

        let banner = self.cfg.support_banner();
        let tip = if banner.is_empty() { "PJDesk".to_string() } else { banner };
        if tip != self.support_tip {
            self.support_tip = tip.clone();
            if let Some(tray) = &self.tray {
                tray.set_tooltip(&tip);
            }
        }
        let pending = crate::client_update::running()
            || self.login_rx.is_some()
            || self.change_rx.is_some()
            || self.net_rx.is_some()
            || self.saved_rx.is_some()
            || self.delete_rx.is_some()
            || self.lan_rx.is_some()
            || !self.viewer_rx.is_empty()
            || !self.engine.ready
            || self.lock_busy;
        let hidden = ctx.input(|i| i.viewport().minimized == Some(true) || i.viewport().visible() == Some(false));
        // A minimized window is not asked to present again. That present is
        // what stopped the message loop. A restore from the taskbar wakes it.
        if !hidden {
            ctx.request_repaint_after(Duration::from_millis(if pending { 50 } else { 400 }));
        }

        if self.lock_ui && self.cfg.joined_by_code_or_key() && self.saved_pin().is_none() && !self.lock_resetting {
            self.lock_ui = false;
        }
        if self.lock_ui && (self.signed_in() || self.pin_required()) {
            if self.lock_resetting || self.saved_pin().is_none() {
                self.setup_screen(ui);
            } else {
                self.lock_screen(ui);
            }
            self.consent_prompt(&ctx);
            poll_client_heartbeat();
            adopt_service_notice();
            self.poll_lan();
            return;
        }

        let p = theme::paint_ctx(&ctx);
        egui::Panel::top("topbar")
            .frame(
                egui::Frame::NONE
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .stroke(egui::Stroke::new(0.0, p.line)),
            )
            .show(ui, |ui| self.top_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.panel).inner_margin(egui::Margin::same(16)))
            .show(ui, |ui| self.body(ui));

        self.settings_window(&ctx);
        self.saved_clear_dialogs(&ctx);
        self.invite_window(&ctx);
        self.support_keys_window(&ctx);
        self.session_error_dialog(&ctx);
        self.temporary_warning(&ctx);
        self.import_prompt(&ctx);
        self.password_prompt(&ctx);
        self.consent_prompt(&ctx);
        poll_client_heartbeat();
        adopt_service_notice();
        self.login_window(&ctx);
        self.change_password_window(&ctx);
        self.poll_lan();
        self.add_book_window(&ctx);
        self.toast.render(&ctx);

        if !self.status.is_empty() {
            egui::Panel::bottom("status")
                .frame(egui::Frame::NONE.fill(p.elev).inner_margin(egui::Margin::symmetric(16, 8)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        theme::icon_in(ui, icons::Icon::Alert, 16.0, p.warn);
                        ui.label(&self.status);
                        if widgets::icon_button(ui, icons::Icon::Close, "Dismiss").clicked() {
                            self.status.clear();
                        }
                    });
                });
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
