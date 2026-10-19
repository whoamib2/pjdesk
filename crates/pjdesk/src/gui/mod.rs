//! The PJDesk desktop app: the window you get by running `pjdesk` with no
//! arguments (or double-clicking it).
//!
//! The left rail is this computer. The main column connects to another one.
//! Each outgoing session opens in its own viewer process. Anything that can
//! block (service control, scheduled tasks, the status file) runs on a
//! background thread; the window only reads the latest snapshot.

mod icons;
mod panels;
mod probe;
mod saved;
mod settings;
mod theme;
mod tray;
mod widgets;

#[cfg(feature = "ui-shots")]
mod shots;

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

/// Launches the app and blocks until the window is closed.
pub fn run(server_flag: Option<String>, import_config: Option<String>) -> Result<()> {
    crate::temp_sign::scrub_abandoned();
    let mut config = Config::load().unwrap_or_default();
    if let Some(flag) = &server_flag {
        if let Ok(addr) = ServerAddr::parse(flag) {
            config.server = Some(addr.to_string());
            let _ = config.save();
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 840.0])
            .with_min_inner_size([920.0, 680.0])
            .with_title("PJDesk")
            .with_app_id("pjdesk")
            .with_icon(std::sync::Arc::new(app_icon())),
        renderer: preferred_renderer(),
        ..Default::default()
    };

    eframe::run_native(
        "PJDesk",
        options,
        Box::new(move |cc| {
            let mut app = App::new(cc, config);
            app.take_installer_key(import_config);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
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
}

impl Engine {
    fn idle(config: &Config) -> Self {
        Self {
            shared: Arc::new(HostShared::default()),
            server: ServerAddr::parse(&config.effective_server()).ok(),
            service_running: false,
            ready: false,
        }
    }

    fn on_service(&mut self, config: &Config, running: bool) {
        if !self.ready {
            self.ready = true;
            self.service_running = running;
            if !running {
                self.spawn(config);
            }
            return;
        }
        if running != self.service_running {
            self.restart(config, running);
        }
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
            spawn_host(server, config.clone(), self.shared.clone());
        }
    }

    fn retarget(&mut self, config: &Config) {
        let running = self.service_running;
        self.restart(config, running);
    }
}

fn spawn_host(server: ServerAddr, config: Config, shared: Arc<HostShared>) {
    let args = HostArgs::from_settings(&config.settings);
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
    viewer_rx: Vec<Receiver<(String, i32, bool)>>,

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
    /// 0 account, 1 enrollment code, 2 network key.
    net_mode: u8,
    net_blob: String,
    net_key_pass: String,
    net_key_show: bool,
    /// The key is limited to one account, so email and password are shown too.
    net_key_account: bool,
    net_busy: bool,
    net_error: String,
    show_net_sign_in: bool,
    /// The signed-in account owns an empty network and this computer already has a server.
    net_offer: bool,
    /// Login token for that offer. Not saved and not logged.
    net_login: String,
    net_rx: Option<Receiver<Result<crate::network::SignIn, String>>>,
    /// Fills Saved computers from the account without opening a connection.
    saved_rx: Option<Receiver<()>>,
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
    tray: Option<tray::Tray>,
    logo: Option<egui::TextureHandle>,
    really_quit: bool,
    told_tray: bool,
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
            viewer_rx: Vec::new(),
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
            net_mode: 0,
            net_blob: String::new(),
            net_key_pass: String::new(),
            net_key_show: false,
            net_key_account: false,
            net_busy: false,
            net_error: String::new(),
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
            tray: tray::Tray::new(&cc.egui_ctx),
            logo: Some(cc.egui_ctx.load_texture(
                "pjdesk-logo",
                egui::ColorImage::from_rgba_unmultiplied([256, 256], include_bytes!("../../assets/icon_256.rgba")),
                egui::TextureOptions::LINEAR,
            )),
            really_quit: false,
            told_tray: false,
        };
        if app.cfg.account.is_some() {
            app.start_token_refresh();
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
        self.lan = vec![("OFFICE-PC".into(), "123456789".into(), "192.168.1.20:21201".into())];
        self.page = MainPage::Saved;
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
        // Show the account's computers for this visit. The list that was already
        // on this computer stays in the snapshot and is not uploaded.
        self.cfg.address_book.clear();
        self.cfg.saved_hidden.clear();
        self.cfg.settings.peers.clear();
        self.cfg.ephemeral_network = true;
        self.cfg.apply_network_session(&session);
        self.net_email = self.cfg.network_email.clone().unwrap_or_default();
        self.settings.set_server_text(&self.cfg.effective_server());
        self.temp = Some(TempHold { snapshot, last_activity: Instant::now(), warning: false });
        self.temp_left = 0;
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
        self.net_email = self.cfg.network_email.clone().unwrap_or_default();
        self.show_net_sign_in = false;
        self.enrolled_for.clear();
        self.account_computers.clear();
        self.temp_left = 0;
        crate::temp_sign::clear_marker();
        self.engine.retarget(&self.cfg);
        self.settings.set_server_text(&self.cfg.effective_server());
        self.toast.show("Signed out");
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
        let account = self.account_computers.clone();
        let local = if self.temp.is_some() { Vec::new() } else { crate::network::saved_from_config(&self.cfg) };
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
        if self.scm_known {
            self.scm_running
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
        self.engine.on_service(&cfg, snap.running);
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
                self.net_blob.clear();
                self.net_key_pass.clear();
                self.import_pass.clear();
                self.net_error.clear();
                self.show_net_sign_in = false;
                self.pending_temporary = false;
                self.pending_enroll_only = false;
                let who = if email.is_empty() { self.net_email.trim().to_string() } else { email };
                self.toast.show(format!("Added this computer to {who}. Nothing from that sign-in was kept on this computer."));
            }
            Ok(Ok(crate::network::SignIn::Ready(session))) => {
                self.net_busy = false;
                self.net_rx = None;
                self.net_offer = false;
                self.net_login.clear();
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
                self.net_blob.clear();
                self.net_key_pass.clear();
                self.import_pass.clear();
                self.net_error.clear();
                self.show_net_sign_in = false;
                if session.email.is_empty() {
                    if self.cfg.network_email.is_none() {
                        self.cfg.network_email = Some(self.net_email.trim().to_string());
                    }
                }
                let mut session = session;
                if session.email.is_empty() {
                    session.email = self.net_email.trim().to_lowercase();
                }
                let temporary = self.pending_temporary;
                self.pending_temporary = false;
                self.pending_enroll_only = false;
                if temporary {
                    self.begin_temporary(session);
                } else {
                    self.note_enrolled(&session.computers);
                    self.cfg.apply_network_session(&session);
                    self.net_email = self.cfg.network_email.clone().unwrap_or_default();
                    self.settings.set_server_text(&self.cfg.effective_server());
                    self.save_cfg();
                    crate::network::copy_to_service(&self.cfg);
                    self.engine.retarget(&self.cfg);
                    if imported {
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
                if e.contains("specific account") {
                    self.show_net_sign_in = true;
                    self.net_mode = 2;
                    self.net_key_account = true;
                }
                self.net_offer = false;
                self.net_login.clear();
                self.net_error = e;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.net_busy = false;
                self.net_rx = None;
            }
        }
    }

    fn start_network_sign_in(&mut self) {
        if self.net_busy {
            return;
        }
        let enroll_only = self.net_enroll_only;
        self.net_enroll_only = false;
        let pjdesk_id = self.visible_id();
        if enroll_only && !pjdesk_proto::is_valid_id(&pjdesk_id) {
            self.net_error = "This computer does not have an ID yet. Wait until Your ID appears, then try again.".into();
            return;
        }
        self.pending_temporary = self.net_temporary && !enroll_only;
        self.pending_enroll_only = enroll_only;
        let this = crate::network::ThisComputer::local(&pjdesk_id);
        let local = if self.pending_temporary || enroll_only {
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
            std::thread::Builder::new()
                .name("network-sign-in".into())
                .spawn(move || {
                    let result = match crate::network::enroll(&code, &device, &name) {
                        Ok(session) => crate::network::finish_account_sign_in(session, &this, &local, enroll_only)
                            .map_err(|e| e.to_string()),
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
                            crate::network::finish_account_sign_in(session, &this, &local, enroll_only).map_err(|e| e.to_string())
                        }
                        Ok(other) => Ok(other),
                        Err(e) => Err(e.to_string()),
                    };
                    let _ = tx.send(result);
                })
                .ok();
        }
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
        let local = if self.pending_temporary || enroll_only {
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
                let result = match crate::network::claim_current_server(&token, &address, &fingerprint, &device, &name) {
                    Ok(session) => {
                        crate::network::finish_account_sign_in(session, &this, &local, enroll_only).map_err(|e| e.to_string())
                    }
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
        let Some(blob) = pjdesk_proto::netkey::blob_from_text(&text) else {
            self.toast.show(if text.trim().is_empty() { "Clipboard is empty" } else { "Invalid server configuration" });
            return;
        };
        self.queue_import(blob, true);
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
        let found = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Paste(text) => pjdesk_proto::netkey::blob_from_text(text),
                _ => None,
            })
        });
        let Some(blob) = found else { return };
        if self.dismissed_paste.as_deref() == Some(blob.as_str()) {
            return;
        }
        self.queue_import(blob, false);
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

    /// Leaves the PJDesk account. The server address, saved computers, and
    /// per-computer passwords stay on this computer.
    /// A temporary sign-in instead restores this computer to how it was.
    fn sign_out_network(&mut self) {
        if self.temp.is_some() {
            self.end_temporary_sign_in();
            return;
        }
        self.cfg.reload_identity_from_disk();
        self.cfg.leave_network_sign_in();
        self.show_net_sign_in = false;
        if let Err(e) = self.cfg.save() {
            self.status = format!("Could not save settings: {e}");
        }
        self.toast.show("Signed out");
    }

    fn request_saved_refresh(&mut self) {
        if self.temp.is_some() || self.saved_rx.is_some() || self.cfg.network_refresh_token().is_empty() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.saved_rx = Some(rx);
        std::thread::spawn(move || {
            if let Ok(mut cfg) = crate::config::Config::load() {
                crate::network::refresh_config_now(&mut cfg);
            }
            let _ = tx.send(());
        });
    }

    fn poll_saved_refresh(&mut self) {
        let Some(rx) = &self.saved_rx else { return };
        match rx.try_recv() {
            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.saved_rx = None;
                if let Ok(disk) = crate::config::Config::load() {
                    self.cfg.take_peer_reports_from(&disk);
                }
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
        if let Some(note) = self.cfg.settings.peers.get(id) {
            cmd.env("PJDESK_VIEW", note.view_mode.to_string());
            cmd.env("PJDESK_DISPLAY", note.last_display.to_string());
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                self.mark_recent(id);
                let what = if files { "Opening file transfer for" } else { "Opening" };
                self.toast.show(format!("{what} {}", format_id(id)));
                let (tx, rx) = std::sync::mpsc::channel();
                let id = id.to_string();
                std::thread::spawn(move || {
                    let code = child.wait().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
                    let _ = tx.send((id, code, files));
                });
                self.viewer_rx.push(rx);
            }
            Err(e) => self.status = format!("Could not open the viewer: {e}"),
        }
    }

    fn poll_client_update(&mut self) {
        let note = crate::client_update::note();
        if note.is_empty() || note == self.status {
            return;
        }
        self.status = note;
    }

    fn poll_viewers(&mut self) {
        let pending = std::mem::take(&mut self.viewer_rx);
        let mut reopen = None;
        for rx in pending {
            match rx.try_recv() {
                Ok((id, code, files)) => {
                    self.reload_peer_reports();
                    if code != 0 {
                        let reason = last_session_reason().unwrap_or_else(|| {
                            if code == 3 {
                                "Wrong password".into()
                            } else {
                                "The session ended before it could start.".into()
                            }
                        });
                        self.status = reason;
                        // A saved password was rejected. Ask once, in the prompt.
                        // Any other failure stays on this page with the reason,
                        // and the password already typed is left in the field.
                        if code == 3 {
                            reopen = Some((id, files));
                        } else if code == 4 {
                            self.prompt_open = false;
                            self.prompt_checking = false;
                            self.ask_alive = false;
                        }
                    }
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

    fn connect_direct(&mut self, addr: &str) {
        let addr = addr.trim().to_string();
        if addr.is_empty() {
            self.status = "Enter an address, such as 192.168.1.20.".into();
            return;
        }
        self.spawn_ask(&addr, false, Some(addr.clone()));
    }

    fn spawn_direct_with_password(&mut self, addr: &str, password: &str) {
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
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                let (tx, rx) = std::sync::mpsc::channel();
                let id = addr.to_string();
                std::thread::spawn(move || {
                    let code = child.wait().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
                    let _ = tx.send((id, code, false));
                });
                self.viewer_rx.push(rx);
            }
            Err(e) => self.status = format!("Could not open the viewer: {e}"),
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
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                self.toast.show(format!("Opening {addr}"));
                let (tx, rx) = std::sync::mpsc::channel();
                let id = addr.to_string();
                std::thread::spawn(move || {
                    let code = child.wait().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
                    let _ = tx.send((id, code, false));
                });
                self.viewer_rx.push(rx);
            }
            Err(e) => self.status = format!("Could not open the viewer: {e}"),
        }
    }

    fn find_on_lan(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.lan_rx = Some(rx);
        self.status = "Looking for computers on this network…".into();
        std::thread::spawn(move || {
            let found = crate::host::direct::discover_for(std::time::Duration::from_secs(4));
            let rows = found
                .into_iter()
                .map(|(b, src)| {
                    let mut addr = src;
                    addr.set_port(b.port);
                    (if b.name.is_empty() { "Computer".into() } else { b.name }, b.id, addr.to_string())
                })
                .collect::<Vec<_>>();
            let _ = tx.send(rows);
        });
    }

    fn poll_lan(&mut self) {
        let Some(rx) = &self.lan_rx else { return };
        if let Ok(rows) = rx.try_recv() {
            self.lan_rx = None;
            self.status = if rows.is_empty() {
                "No PJDesk computers answered on this network.".into()
            } else {
                format!("Found {} computer(s) on this network.", rows.len())
            };
            self.lan = rows;
            self.note_lan_names();
            if !self.lan.is_empty() {
                self.desk_tab = DeskTab::Lan;
            }
        }
    }

    /// A computer already saved learns its name from a LAN announcement when it has none yet.
    fn note_lan_names(&mut self) {
        let mut changed = false;
        let found = self.lan.clone();
        for (name, id, _) in found {
            let id = normalize_id(&id);
            let name = name.trim();
            if name.is_empty() || name == "Computer" || !is_valid_id(&id) {
                continue;
            }
            let known = self.cfg.address_book.iter().any(|e| e.id == id) || self.cfg.settings.peers.contains_key(&id);
            if !known {
                continue;
            }
            let note = self.cfg.settings.peers.entry(id).or_default();
            if note.hostname.is_empty() {
                note.hostname = name.to_string();
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

fn last_session_reason() -> Option<String> {
    let path = crate::config::Config::path().parent()?.join("last-session.txt");
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&glow::Context>) {
        self.end_temporary_sign_in();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.ctx = ui.ctx().clone();
        let ctx = self.ctx.clone();
        self.poll_login();
        self.poll_network();
        self.poll_temporary(&ctx);
        self.poll_self_enroll();
        self.poll_saved_refresh();
        self.poll_password();
        self.poll_probe();
        self.poll_ask();
        self.poll_viewers();
        self.poll_client_update();
        self.notice_sign_in_paste(&ctx);

        if ctx
            .input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Comma)))
            && !self.show_settings
        {
            self.open_settings();
        }

        let tray_actions = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        let has_tray = self.tray.is_some();
        for action in tray_actions {
            match action {
                tray::Action::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                tray::Action::Update => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.show_settings = true;
                    self.settings.show_about();
                    crate::client_update::start();
                }
                tray::Action::Quit => {
                    self.really_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let closing = ctx.input(|i| i.viewport().close_requested());
        if closing && (self.really_quit || !has_tray) && self.temp.is_some() {
            self.end_temporary_sign_in();
        }
        if has_tray && !self.really_quit && closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            if !self.told_tray {
                self.told_tray = true;
                self.toast.show("PJDesk is still running — click the tray icon, or right-click it to Quit");
            }
        }

        let pending = crate::client_update::running()
            || self.login_rx.is_some()
            || self.change_rx.is_some()
            || self.net_rx.is_some()
            || self.saved_rx.is_some()
            || self.lan_rx.is_some()
            || !self.viewer_rx.is_empty()
            || !self.engine.ready;
        ctx.request_repaint_after(Duration::from_millis(if pending { 50 } else { 400 }));

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
        self.temporary_warning(&ctx);
        self.import_prompt(&ctx);
        self.password_prompt(&ctx);
        self.consent_prompt(&ctx);
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
