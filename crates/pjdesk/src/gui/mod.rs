//! The PJDesk desktop app: the window you get by running `pjdesk` with no
//! arguments (or double-clicking it).
//!
//! Left half shares this computer (your ID + password, served by a host thread
//! running in the background); right half connects to another computer. A
//! settings window and an optional sign-in to your server round it out. Each
//! outgoing session opens in its own viewer process, so the window stays
//! responsive.

mod panels;
mod settings;
mod theme;
mod tray;
mod widgets;

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use pjdesk_proto::{format_id, is_valid_id, normalize_id};
use eframe::egui;

use crate::account;
use crate::config::Config;
use crate::host::{self, HostShared};
use crate::signaling::ServerAddr;
use crate::HostArgs;

/// Launches the app and blocks until the window is closed.
pub fn run(server_flag: Option<String>) -> Result<()> {
    let mut config = Config::load().unwrap_or_default();
    // A --server flag updates the saved server.
    if let Some(flag) = &server_flag {
        if let Ok(addr) = ServerAddr::parse(flag) {
            config.server = Some(addr.to_string());
            let _ = config.save();
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([940.0, 620.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("PJDesk")
            .with_app_id("pjdesk")
            .with_icon(std::sync::Arc::new(app_icon())),
        renderer: preferred_renderer(),
        ..Default::default()
    };

    eframe::run_native("PJDesk", options, Box::new(|cc| Ok(Box::new(App::new(cc, config)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// The window/taskbar icon, decoded from a raw 256×256 RGBA blob so no image
/// decoder is needed at runtime.
fn app_icon() -> egui::IconData {
    egui::IconData { rgba: include_bytes!("../../assets/icon_256.rgba").to_vec(), width: 256, height: 256 }
}

/// wgpu renders correctly on the broadest range of Windows GPUs here; glow is a
/// fallback via `PJDESK_RENDERER=glow`.
fn preferred_renderer() -> eframe::Renderer {
    match std::env::var("PJDESK_RENDERER").as_deref() {
        Ok("glow") => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    }
}

/// Supervises the background host thread (the "share this computer" side).
///
/// When the background service is running it hosts this computer, so the GUI
/// stays out of the way (it would otherwise register the same device twice) and
/// just shows the service's status.
struct Engine {
    shared: Arc<HostShared>,
    server: Option<ServerAddr>,
    /// True when the background service is running and doing the hosting.
    service_running: bool,
}

impl Engine {
    /// Starts a host thread if a server is configured and the service isn't
    /// already hosting; otherwise stays idle.
    fn start(config: &Config) -> Self {
        let service_running = crate::service::status().1;
        let shared = Arc::new(HostShared::default());
        let server = ServerAddr::parse(&config.effective_server()).ok();
        if !service_running {
            if let Some(server) = &server {
                spawn_host(server.clone(), config.clone(), shared.clone());
            }
        }
        Self { shared, server, service_running }
    }

    /// Restarts hosting with the current config (e.g. after the server or a
    /// quality setting changed).
    fn restart(&mut self, config: &Config) {
        self.shared.request_stop();
        self.service_running = crate::service::status().1;
        let shared = Arc::new(HostShared::default());
        self.shared = shared.clone();
        self.server = ServerAddr::parse(&config.effective_server()).ok();
        if !self.service_running {
            if let Some(server) = &self.server {
                spawn_host(server.clone(), config.clone(), shared);
            }
        }
    }

    /// Re-checks whether the service is running; restarts the engine if that
    /// changed (so the GUI starts or stops its own hosting to match).
    fn refresh_service(&mut self, config: &Config) {
        let running = crate::service::status().1;
        if running != self.service_running {
            self.restart(config);
        }
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

/// Outcome of a background sign-in attempt.
enum LoginMsg {
    Ok { display_name: String },
    Err(String),
}

pub struct App {
    cfg: Config,
    engine: Engine,

    // "Control remote" panel.
    connect_id: String,
    connect_password: String,
    remember: bool,

    /// Reveal this computer's host password in the UI (masked by default).
    show_password: bool,
    /// Input for setting a permanent password from the share panel.
    perm_input: String,

    // Account sign-in.
    show_login: bool,
    login_user: String,
    login_pass: String,
    login_busy: bool,
    login_rx: Option<Receiver<LoginMsg>>,

    // Settings window.
    show_settings: bool,
    settings: settings::SettingsDraft,

    // Add-to-address-book dialog.
    book_alias: String,
    book_target: Option<String>,

    status: String,
    toast: widgets::Toast,
    /// System-tray icon (minimize-to-tray); `None` if the system has no tray.
    tray: Option<tray::Tray>,
    /// The app logo texture, shown in the header.
    logo: Option<egui::TextureHandle>,
    /// Set when the user picks Quit from the tray, so the close isn't trapped.
    really_quit: bool,
    /// Whether we've shown the "still running in the tray" hint once.
    told_tray: bool,
    last_service_check: std::time::Instant,
    /// Cached service status, refreshed on a timer so we don't read the status
    /// file from disk on every repaint (which makes typing feel laggy).
    svc_status: crate::syspaths::ServiceStatus,
    svc_status_at: std::time::Instant,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, cfg: Config) -> Self {
        theme::apply(&cc.egui_ctx);
        let engine = Engine::start(&cfg);
        let settings = settings::SettingsDraft::from_config(&cfg);
        let mut app = Self {
            cfg,
            engine,
            connect_id: String::new(),
            connect_password: String::new(),
            remember: false,
            show_password: false,
            perm_input: String::new(),
            show_login: false,
            login_user: String::new(),
            login_pass: String::new(),
            login_busy: false,
            login_rx: None,
            show_settings: false,
            settings,
            book_alias: String::new(),
            book_target: None,
            status: String::new(),
            toast: widgets::Toast::default(),
            tray: tray::Tray::new(&cc.egui_ctx),
            logo: Some(cc.egui_ctx.load_texture(
                "pjdesk-logo",
                egui::ColorImage::from_rgba_unmultiplied([256, 256], include_bytes!("../../assets/icon_256.rgba")),
                egui::TextureOptions::LINEAR,
            )),
            really_quit: false,
            told_tray: false,
            last_service_check: std::time::Instant::now(),
            svc_status: crate::syspaths::ServiceStatus::load().unwrap_or_default(),
            svc_status_at: std::time::Instant::now(),
        };
        // Refresh a saved sign-in in the background.
        if app.cfg.account.is_some() {
            app.start_token_refresh();
        }
        app
    }

    fn server_addr(&self) -> Option<ServerAddr> {
        ServerAddr::parse(&self.cfg.effective_server()).ok()
    }

    /// Saves config without clobbering the identity the host thread owns.
    fn save_cfg(&mut self) {
        self.cfg.reload_identity_from_disk();
        if let Err(e) = self.cfg.save() {
            self.status = format!("Could not save settings: {e}");
        }
    }

    /// Sets a permanent password (kept in addition to the one-time password —
    /// both let a controller in). Applies to the service too when it's hosting.
    fn set_permanent_password(&mut self, pw: &str) {
        let pw = pw.trim();
        if pw.is_empty() {
            return;
        }
        self.cfg.settings.set_permanent_password(Some(pw));
        self.save_cfg();
        if crate::service::status().0 {
            crate::platform::run_elevated(&["service", "configure", "--password", pw]);
        }
        let cfg = self.cfg.clone();
        self.engine.restart(&cfg);
        self.perm_input.clear();
        self.toast.show("Permanent password set");
    }

    /// Removes the permanent password, leaving only the rotating one-time
    /// password. Clears it everywhere it's stored and restarts hosting.
    fn remove_permanent_password(&mut self) {
        self.cfg.settings.set_permanent_password(None);
        self.save_cfg();
        // If the background service is the one hosting, clear its password too.
        if crate::service::status().0 {
            crate::platform::run_elevated(&["service", "configure", "--clear-password"]);
        }
        let cfg = self.cfg.clone();
        self.engine.restart(&cfg);
        self.toast.show("Permanent password removed");
    }

    // --- Account ----------------------------------------------------------

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
            // account::refresh persists the token + address book itself (with
            // the identity guard), so we don't save the stale clone here.
            match rt.block_on(account::refresh(&server, &mut cfg)) {
                Ok(Some(info)) => {
                    let _ = tx.send(LoginMsg::Ok { display_name: info.display_name });
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
                    // account::login already saved the token + address book.
                    let _ = tx.send(LoginMsg::Ok { display_name: info.display_name });
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
            Ok(LoginMsg::Ok { display_name, .. }) => {
                self.login_busy = false;
                self.login_rx = None;
                self.show_login = false;
                self.login_pass.clear();
                // The worker saved the token + address book to disk; reload it.
                let keep_id = (self.cfg.device_id.clone(), self.cfg.device_secret.clone());
                if let Ok(fresh) = Config::load() {
                    self.cfg = fresh;
                    self.cfg.device_id = self.cfg.device_id.clone().or(keep_id.0);
                    self.cfg.device_secret = self.cfg.device_secret.clone().or(keep_id.1);
                }
                self.toast.show(format!("Signed in as {display_name}"));
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

    /// Pushes the address book to the server after a local change.
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

    // --- Connecting -------------------------------------------------------

    /// Launches a viewer process for `id`, using `password` (or a saved one).
    fn connect_to(&mut self, id: &str, password: Option<String>) {
        let id = normalize_id(id);
        if !is_valid_id(&id) {
            self.status = "That isn't a 9-digit PJDesk ID.".into();
            return;
        }
        let server = self.cfg.effective_server();
        let password = password
            .filter(|p| !p.is_empty())
            .or_else(|| self.cfg.saved_password(&id))
            .unwrap_or_default();

        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                self.status = format!("Can't find the PJDesk program: {e}");
                return;
            }
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--server").arg(server).arg("connect").arg(&id);
        if !password.is_empty() {
            cmd.env("PJDESK_PASSWORD", &password);
        }
        // Don't let the spawned viewer flash a console window.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        match cmd.spawn() {
            Ok(_) => {
                self.toast.show(format!("Opening {}", format_id(&id)));
                if self.remember && !password.is_empty() {
                    self.remember_password(&id, &password);
                }
            }
            Err(e) => self.status = format!("Could not open the viewer: {e}"),
        }
    }

    fn remember_password(&mut self, id: &str, password: &str) {
        // Prefer the synced address book when signed in; else the local store.
        if self.cfg.account.is_some() {
            if let Some(entry) = self.cfg.address_book.iter_mut().find(|e| e.id == id) {
                entry.password = Some(password.to_string());
            } else {
                self.cfg.address_book.push(pjdesk_proto::signal::AddressEntry {
                    id: id.to_string(),
                    password: Some(password.to_string()),
                    ..Default::default()
                });
            }
            self.sync_address_book();
        } else {
            self.cfg.remember_password(id, Some(password));
            self.save_cfg();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_login();
        if self.last_service_check.elapsed() > Duration::from_secs(3) {
            self.last_service_check = std::time::Instant::now();
            let cfg = self.cfg.clone();
            self.engine.refresh_service(&cfg);
        }
        if self.svc_status_at.elapsed() > Duration::from_millis(900) {
            self.svc_status_at = std::time::Instant::now();
            self.svc_status = crate::syspaths::ServiceStatus::load().unwrap_or_default();
        }
        let ctx = ui.ctx().clone();

        // System tray: Show/Quit, and hide-to-tray when the window is closed.
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

        // Keep status (online/connected) fresh.
        ctx.request_repaint_after(Duration::from_millis(400));

        egui::Panel::top("topbar").show(ui, |ui| self.top_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(6.0);
            ui.columns(2, |cols| {
                self.host_panel(&mut cols[0]);
                self.control_panel(&mut cols[1]);
            });
            ui.add_space(8.0);
            ui.separator();
            self.address_book(ui);
        });

        self.settings_window(&ctx);
        self.login_window(&ctx);
        self.add_book_window(&ctx);
        self.toast.render(&ctx);

        if !self.status.is_empty() {
            egui::Panel::bottom("status").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(theme::WARN, "⚠");
                    ui.label(&self.status);
                    if ui.small_button("✕").clicked() {
                        self.status.clear();
                    }
                });
            });
        }
    }
}
