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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Which list is showing under the connect form.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeskTab {
    Recent,
    Favorites,
    Address,
    Lan,
}

/// Launches the app and blocks until the window is closed.
pub fn run(server_flag: Option<String>) -> Result<()> {
    let mut config = Config::load().unwrap_or_default();
    if let Some(flag) = &server_flag {
        if let Ok(addr) = ServerAddr::parse(flag) {
            config.server = Some(addr.to_string());
            let _ = config.save();
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([1040.0, 680.0])
            .with_title("PJDesk")
            .with_app_id("pjdesk")
            .with_icon(std::sync::Arc::new(app_icon())),
        renderer: preferred_renderer(),
        ..Default::default()
    };

    eframe::run_native("PJDesk", options, Box::new(|cc| Ok(Box::new(App::new(cc, config)))))
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
    net_use_code: bool,
    net_busy: bool,
    net_error: String,
    show_net_sign_in: bool,
    net_rx: Option<Receiver<Result<crate::network::Session, String>>>,
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
            desk_tab: DeskTab::Recent,
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
            net_use_code: false,
            net_busy: false,
            net_error: String::new(),
            show_net_sign_in: false,
            net_rx: None,
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
                os_family: "windows".into(),
                os_version: "10.0.26100".into(),
                ..Default::default()
            },
        );
        self.cfg.settings.peers.insert(
            "555666777".into(),
            crate::config::PeerNote {
                favorite: false,
                last_connected: now.saturating_sub(86_400),
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
        self.cfg.address_book.push(pjdesk_proto::signal::AddressEntry {
            id: "444555666".into(),
            alias: "Studio".into(),
            ..Default::default()
        });
        self.cfg.address_book.push(pjdesk_proto::signal::AddressEntry {
            id: "777888999".into(),
            alias: "Phone".into(),
            ..Default::default()
        });
        self.cfg.settings.peers.insert(
            "444555666".into(),
            crate::config::PeerNote {
                favorite: true,
                last_connected: unix_now().saturating_sub(7200),
                os_family: "macos".into(),
                os_version: "15.1".into(),
                ..Default::default()
            },
        );
        self.cfg.settings.peers.insert(
            "777888999".into(),
            crate::config::PeerNote {
                os_family: "android".into(),
                os_version: "14".into(),
                last_connected: unix_now().saturating_sub(400_000),
                ..Default::default()
            },
        );
        self.cfg.remember_password("123456789", Some("saved-secret"));
        self.lan = vec![("Office PC".into(), "123456789".into(), "192.168.1.20:21201".into())];
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
        self.show_net_sign_in = true;
    }

    #[cfg(feature = "ui-shots")]
    fn load_password_preview(&mut self) {
        self.load_preview(false);
        self.prompt_open = true;
        self.prompt_id = "555666777".into();
        self.prompt_password = "correct horse".into();
        self.prompt_show = false;
        self.prompt_remember = true;
        self.prompt_rejected = false;
    }

    fn server_addr(&self) -> Option<ServerAddr> {
        ServerAddr::parse(&self.cfg.effective_server()).ok()
    }

    fn save_cfg(&mut self) {
        self.cfg.reload_identity_from_disk();
        if let Err(e) = self.cfg.save() {
            self.status = format!("Could not save settings: {e}");
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
        self.save_cfg();
    }

    fn toggle_favorite(&mut self, id: &str) {
        let note = self.cfg.settings.peers.entry(id.to_string()).or_default();
        note.favorite = !note.favorite;
        let on = note.favorite;
        if !on && note.last_connected == 0 {
            self.cfg.settings.peers.remove(id);
        }
        self.save_cfg();
        self.toast.show(if on { "Added to favorites" } else { "Removed from favorites" });
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
            Ok(Ok(session)) => {
                self.net_busy = false;
                self.net_rx = None;
                self.net_pass.clear();
                self.net_code.clear();
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
                self.cfg.apply_network_session(&session);
                self.net_email = self.cfg.network_email.clone().unwrap_or_default();
                self.save_cfg();
                crate::network::copy_to_service(&self.cfg);
                self.engine.retarget(&self.cfg);
                self.toast.show(format!("Signed in as {}", self.net_email));
            }
            Ok(Err(e)) => {
                self.net_busy = false;
                self.net_rx = None;
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
        let device = self.cfg.network_device();
        let name = crate::hostname();
        let (tx, rx) = std::sync::mpsc::channel();
        self.net_rx = Some(rx);
        self.net_busy = true;
        self.net_error.clear();
        if self.net_use_code {
            let code = self.net_code.clone();
            std::thread::Builder::new()
                .name("network-sign-in".into())
                .spawn(move || {
                    let result = crate::network::enroll(&code, &device, &name).map_err(|e| e.to_string());
                    let _ = tx.send(result);
                })
                .ok();
        } else {
            let email = self.net_email.clone();
            let password = self.net_pass.clone();
            std::thread::Builder::new()
                .name("network-sign-in".into())
                .spawn(move || {
                    let result = crate::network::sign_in(&email, &password, &device, &name).map_err(|e| e.to_string());
                    let _ = tx.send(result);
                })
                .ok();
        }
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
            self.open_prompt(&id, files, false);
            return;
        }
        let remember = !from_store && self.remember;
        self.spawn_viewer(&id, &password, files, remember, from_store);
    }

    fn open_prompt(&mut self, id: &str, files: bool, rejected: bool) {
        self.prompt_open = true;
        self.prompt_id = id.to_string();
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
        if files {
            cmd.env("PJDESK_OPEN_PANEL", "files");
        }
        if from_store {
            cmd.env("PJDESK_SAVED", "1");
        }
        if remember {
            cmd.env("PJDESK_REMEMBER", "1");
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

    fn poll_viewers(&mut self) {
        let pending = std::mem::take(&mut self.viewer_rx);
        let mut reopen = None;
        for rx in pending {
            match rx.try_recv() {
                Ok((id, code, files)) => {
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
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => self.viewer_rx.push(rx),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if let Some((id, files)) = reopen {
            self.open_prompt(&id, files, true);
        }
    }

    fn connect_direct(&mut self, addr: &str) {
        let addr = addr.trim().to_string();
        if addr.is_empty() {
            self.status = "Enter an address, such as 192.168.1.20.".into();
            return;
        }
        let server = self.cfg.effective_server();
        let password = self.connect_password.clone();
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.status = format!("Can't find the PJDesk program: {e}");
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--server").arg(server).arg("connect").arg("--direct").arg(&addr);
        if !password.is_empty() {
            cmd.env("PJDESK_PASSWORD", &password);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Ok(_) => self.toast.show(format!("Opening {addr}")),
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
            if !self.lan.is_empty() {
                self.desk_tab = DeskTab::Lan;
            }
        }
    }
}

fn last_session_reason() -> Option<String> {
    let path = crate::config::Config::path().parent()?.join("last-session.txt");
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.ctx = ui.ctx().clone();
        let ctx = self.ctx.clone();
        self.poll_login();
        self.poll_network();
        self.poll_password();
        self.poll_probe();
        self.poll_viewers();

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
                tray::Action::Quit => {
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
                self.toast.show("PJDesk is still running — click the tray icon, or right-click it to Quit");
            }
        }

        let pending = self.login_rx.is_some()
            || self.change_rx.is_some()
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
        self.password_prompt(&ctx);
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
