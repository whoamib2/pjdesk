#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! PJDesk client.
//!
//! One executable plays both roles:
//! * **Host** – shares this computer: stays online with the server, shows an
//!   ID and a one-time password, and streams the screen to an authenticated
//!   controller while replaying its mouse and keyboard input.
//! * **Controller** – connects to a host by ID + password and shows its
//!   screen in a window, forwarding local input.
//!
//! Double-clicking `pjdesk.exe` does both: it hosts in the background and
//! lets you type a partner ID to control another machine.

#[cfg(test)]
mod icon_asset;

mod account;
mod access_keys;
mod audio;
mod client_update;
mod chatpop;
mod clip;
mod color;
mod config;
mod gui;
mod handoff;
mod host;
mod hostfacts;
mod identity;
mod incoming;
mod idfmt;
mod network;
mod p2p;
mod routes_store;
mod platform;
mod secure;
mod service;
mod session;
mod session_window;
mod support_keys;
mod signaling;
mod syspaths;
mod temp_sign;
mod update_pipe;
mod vault;
mod viewer;
mod traywatch;
mod winshow;

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use pjdesk_proto::{format_id, is_valid_id, normalize_id, tls};

use crate::config::Config;
use crate::signaling::ServerAddr;

/// Set once the host has registered and printed its ID (interactive mode
/// waits for it so the prompt doesn't interleave with the banner).
pub static HOST_ONLINE: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(name = "pjdesk", version, about = "PJDesk remote desktop: share this computer or control another one")]
struct Cli {
    /// Server address as host[:port] (overrides the saved setting).
    #[arg(long, global = true)]
    server: Option<String>,
    /// Apply an encrypted network key (`pjdesk-net1:…`). The same blob is read
    /// from a renamed installer, `PJDesk-Setup-<version>--<blob>.exe`.
    #[arg(long = "config", global = true, value_name = "BLOB")]
    import_config: Option<String>,
    /// Tray icon only. This process never opens a window.
    #[arg(long, hide = true)]
    tray: bool,
    /// Window only. This process never owns the tray icon. The tray starts it.
    #[arg(long, hide = true)]
    window: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Share this computer: go online and wait for a controller.
    Host(HostArgs),
    /// Control another computer by its ID.
    Connect(ConnectArgs),
    /// Show or change saved settings.
    Config(ConfigArgs),
    /// Sign in to your PJDesk server account (saves your login).
    Login(LoginArgs),
    /// Sign out and forget the saved login.
    Logout,
    /// After a silent install: restart the service and the signed-in user's window.
    #[command(name = "update-handoff", hide = true)]
    UpdateHandoff,
    /// Check for a newer PJDesk and install it. The service does this; there is no scheduled task.
    Update {
        /// Only report whether an update is available; don't install.
        #[arg(long)]
        check: bool,
    },
    /// Internal: turn automatic updates on or off. The service does the check.
    #[command(name = "update-task", hide = true)]
    UpdateTask {
        #[arg(long)]
        disable: bool,
    },
    /// Run the text-mode host/controller in this console (no window).
    #[command(hide = true)]
    Console,
    /// Manage the background service (reach this computer at the sign-in screen).
    #[command(subcommand)]
    Service(ServiceCommand),
    /// Internal: the hosting helper the service launches into a user session.
    #[command(name = "__session-host", hide = true)]
    SessionHost {
        /// Path to the service config to host from.
        #[arg(long)]
        config: Option<String>,
    },
    /// Internal: the compact chat window for a session.
    #[command(name = "__chat-window", hide = true)]
    ChatWindow {
        #[arg(long)]
        dir: String,
    },
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Install and start the background service (run from an Administrator prompt).
    Install {
        /// Server address to use (defaults to your current server).
        #[arg(long)]
        server: Option<String>,
        /// Unattended password (prefer a prompt; this shows up in the process list).
        #[arg(long)]
        password: Option<String>,
        /// Read the unattended password from a file, then delete the file.
        /// The window uses this so the password is not on the command line.
        #[arg(long)]
        password_file: Option<String>,
        /// Also add a Windows Firewall rule.
        #[arg(long)]
        firewall: bool,
    },
    /// Stop and remove the background service.
    Uninstall,
    /// Start the installed service.
    Start,
    /// Stop the running service.
    Stop,
    /// Change the service's server or unattended password.
    Configure {
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        password: Option<String>,
        /// Read the unattended password from a file, then delete the file.
        #[arg(long)]
        password_file: Option<String>,
        /// Clear the permanent password so the service uses a one-time password.
        #[arg(long)]
        clear_password: bool,
    },
    /// Entry point used by the Windows Service Control Manager.
    #[command(hide = true)]
    Run,
}

#[derive(Args, Clone, Debug)]
struct LoginArgs {
    /// Account username (asked interactively if omitted).
    username: Option<String>,
    /// Account password (asked interactively if omitted).
    #[arg(long, env = "PJDESK_ACCOUNT_PASSWORD", hide_env_values = true)]
    password: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct HostArgs {
    /// A permanent password for unattended access. The host always also shows a
    /// rotating one-time password; both let a controller in.
    #[arg(long = "password", env = "PJDESK_HOST_PASSWORD")]
    pub permanent_password: Option<String>,
    /// Monitor to share (0 = primary).
    #[arg(long, default_value_t = 0)]
    pub display: usize,
    /// Maximum frames per second.
    #[arg(long, default_value_t = 30)]
    pub fps: u32,
    /// Target video bitrate in kbit/s.
    #[arg(long, default_value_t = 6000)]
    pub bitrate: u32,
    /// Screens wider than this are downscaled (by 2x, 3x...) before encoding.
    #[arg(long, default_value_t = 2560)]
    pub max_width: u32,
    /// Use GDI screen capture instead of DXGI Desktop Duplication.
    #[arg(long)]
    pub gdi: bool,
    /// Stream a synthetic test pattern instead of the screen.
    #[arg(long)]
    pub test_pattern: bool,
    /// Let controllers watch but not control.
    #[arg(long)]
    pub view_only: bool,
    /// Follow the active input desktop (service helper only), so capture sees
    /// UAC prompts and the sign-in screen. Not a user-facing flag.
    #[arg(skip)]
    pub follow_desktop: bool,
    /// "prompt" or "unattended". Not a command-line flag; copied from settings.
    #[arg(skip)]
    pub approval: String,
    /// Directory received files may be written into.
    #[arg(skip)]
    pub receive_dir: String,
    /// Listen for direct IP / LAN connections.
    #[arg(skip)]
    pub allow_direct: bool,
    /// Answer LAN discovery. Off when the person opts out in Settings.
    #[arg(skip)]
    pub discover_lan: bool,
    /// Ports a tunnel may open on this computer (loopback).
    #[arg(skip)]
    pub tunnel_ports: Vec<u16>,
    /// True when this process is the background service helper (runs as SYSTEM).
    #[arg(skip)]
    pub service_host: bool,
    /// This computer accepted an unattended support key. Signaling sessions open
    /// without Allow. Direct connections still ask.
    #[arg(skip)]
    pub support_unattended: bool,
    /// The permanent password came from the saved config, so the host re-reads
    /// it from that file on every connection instead of keeping a startup copy.
    #[arg(skip)]
    pub permanent_from_config: bool,
}

impl Default for HostArgs {
    fn default() -> Self {
        Self {
            permanent_password: std::env::var("PJDESK_HOST_PASSWORD").ok(),
            display: 0,
            fps: 30,
            bitrate: 6000,
            max_width: 2560,
            gdi: false,
            test_pattern: false,
            view_only: false,
            follow_desktop: false,
            approval: "prompt".into(),
            receive_dir: String::new(),
            allow_direct: true,
            discover_lan: true,
            tunnel_ports: Vec::new(),
            service_host: false,
            support_unattended: false,
            permanent_from_config: false,
        }
    }
}

/// Empty, or the word Desktop, is this user's Desktop. The word Downloads is that folder.
pub fn resolve_receive_dir(configured: &str) -> String {
    let text = configured.trim();
    if text.eq_ignore_ascii_case("downloads") {
        return home_folder("Downloads");
    }
    if !text.is_empty() && !text.eq_ignore_ascii_case("desktop") {
        return text.to_string();
    }
    home_folder("Desktop")
}

fn home_folder(name: &str) -> String {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    match home {
        Some(home) => std::path::PathBuf::from(home).join(name).to_string_lossy().into_owned(),
        None => String::new(),
    }
}

impl HostArgs {
    /// Builds host arguments from saved settings, using the permanent password
    /// if one is set (otherwise a fresh random password each start).
    pub fn from_settings(settings: &config::Settings) -> Self {
        Self {
            permanent_password: settings.permanent_password(),
            permanent_from_config: true,
            display: 0,
            fps: settings.fps,
            bitrate: settings.bitrate_kbps,
            max_width: settings.max_width,
            gdi: settings.force_gdi,
            test_pattern: false,
            view_only: settings.view_only,
            follow_desktop: false,
            approval: if settings.approval == "unattended" { "unattended".into() } else { "prompt".into() },
            receive_dir: resolve_receive_dir(&settings.receive_dir),
            allow_direct: settings.allow_direct,
            discover_lan: settings.discover_lan,
            tunnel_ports: settings.tunnel_ports.clone(),
            service_host: false,
            support_unattended: false,
        }
    }
}

#[derive(Args, Clone, Debug)]
pub struct ConnectArgs {
    /// ID of the computer to control, with or without spaces ("123 456 789").
    /// Omit this when using --direct.
    #[arg(required = false, num_args = 0..=9, value_name = "ID")]
    pub id: Vec<String>,
    /// Its password (asked interactively if omitted).
    #[arg(long, env = "PJDESK_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,
    /// Skip the direct (P2P) attempt and go straight through the server's relay.
    #[arg(long)]
    pub force_relay: bool,
    /// Connect to this address instead of an ID (`192.168.1.20` or `192.168.1.20:21201`).
    #[arg(long)]
    pub direct: Option<String>,
    /// Accept a new certificate for `--direct` and replace the saved pin.
    #[arg(long)]
    pub trust: bool,
    /// No window: receive and decode frames, print statistics and exit (testing).
    #[arg(long)]
    pub headless: bool,
    /// Number of frames to receive in --headless mode.
    #[arg(long, default_value_t = 150)]
    pub frames: u32,
}

#[derive(Args, Clone, Debug)]
struct ConfigArgs {
    /// Save the server address (host[:port]).
    #[arg(long, value_name = "ADDRESS")]
    set_server: Option<String>,
    /// Pin the server key fingerprint shown in the server console.
    #[arg(long)]
    fingerprint: Option<String>,
    /// Forget the saved server fingerprint (after reinstalling the server).
    #[arg(long)]
    reset_fingerprint: bool,
    /// Forget this device's ID; a new one is assigned on the next start.
    #[arg(long)]
    reset_id: bool,
}

fn main() {
    // The service relaunches this window with no console. Std handles are then
    // null, and the first read or write returns "The handle is invalid."
    #[cfg(windows)]
    repair_stdio_handles();
    // The service and its helper run with no console and set up their own
    // (file) logging, so route them before touching the console.
    let raw: Vec<String> = std::env::args().collect();
    let is_connect = raw.iter().any(|a| a == "connect");
    if is_connect {
        // An elevated viewer cannot take a file drop from Explorer. Open the
        // same command again at medium integrity, then leave this process.
        if platform::relaunch_medium_if_elevated() {
            return;
        }
        // The viewer window needs the main thread in the STA apartment before
        // DPAPI or QUIC touch COM. Doing that later made create_window fail
        // with no log line and no window after the password was accepted.
        init_viewer_com();
    }
    let is_background = raw.iter().any(|a| a == "__session-host" || a == "__chat-window" || a == "update-handoff" || a == "--tray" || a == "--window")
        || (raw.get(1).map(|s| s == "service").unwrap_or(false) && raw.iter().any(|a| a == "run"));

    // With any other argument, attach a console so output and clap errors show.
    if raw.len() > 1 && !is_background {
        ensure_console();
    }
    let cli = Cli::parse();

    match &cli.command {
        None => {
            // Default: the window, plus a tray process that never draws it.
            // `--tray` is the icon only. `--window` is the window only.
            init_gui_logging();
            let started = if cli.tray {
                gui::run_tray();
                Ok(())
            } else if cli.window {
                gui::run(cli.server.clone(), cli.import_config.clone())
            } else {
                gui::open_or_forward(cli.server.clone(), cli.import_config.clone())
            };
            if let Err(e) = started {
                fatal(&format!("PJDesk could not start: {e:#}"));
            }
        }
        Some(Cmd::SessionHost { config }) => {
            if let Err(e) = service::session_host(config.clone()) {
                tracing::error!("session host: {e:#}");
            }
        }
        Some(Cmd::ChatWindow { dir }) => {
            if let Err(e) = chatpop::run_window(std::path::PathBuf::from(dir)) {
                tracing::warn!("{e:#}");
            }
        }
        Some(Cmd::UpdateHandoff) => {
            // A missing client.json must not keep the computer offline.
            if let Err(e) = handoff::run() {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
        }
        Some(Cmd::Service(ServiceCommand::Run)) => {
            if let Err(e) = service::run_dispatcher() {
                tracing::error!("service dispatcher: {e:#}");
            }
        }
        Some(_) => {
            init_logging();
            if is_connect {
                install_session_panic_hook();
            }
            if let Err(e) = run(cli) {
                let text = format!("{e:#}");
                note_failure(&text);
                eprintln!("\nError: {text}");
                std::process::exit(1);
            }
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut config = Config::load()?;
    match cli.command {
        Some(Cmd::Config(args)) => config_command(&mut config, args, cli.server),
        Some(Cmd::Host(args)) => {
            let server = resolve_server(&mut config, cli.server, false)?;
            host::run(server, config, args)
        }
        Some(Cmd::Connect(args)) => {
            let server = resolve_server(&mut config, cli.server, false)?;
            viewer::run(server, config, args)
        }
        Some(Cmd::Login(args)) => login_command(&mut config, args, cli.server),
        Some(Cmd::Logout) => logout_command(&mut config, cli.server),
        Some(Cmd::Update { check }) => update_command(check),
        Some(Cmd::UpdateHandoff) => handoff::run(),
        Some(Cmd::UpdateTask { disable }) => service::set_update_task(!disable),
        Some(Cmd::Console) => interactive(config, cli.server),
        Some(Cmd::Service(cmd)) => service_command(cmd),
        Some(Cmd::SessionHost { .. }) | Some(Cmd::ChatWindow { .. }) | None => {
            unreachable!("handled in main")
        }
    }
}

fn password_from_file(password: Option<String>, password_file: Option<String>) -> Result<Option<String>> {
    match password_file {
        Some(path) => Ok(Some(crate::platform::read_password_file(std::path::Path::new(&path))?)),
        None => Ok(password),
    }
}

fn service_command(cmd: ServiceCommand) -> Result<()> {
    match cmd {
        ServiceCommand::Install { server, password, password_file, firewall } => {
            let password = password_from_file(password, password_file)?;
            service::install(server.as_deref(), password.as_deref(), firewall)
        }
        ServiceCommand::Uninstall => service::uninstall(),
        ServiceCommand::Start => service::start(),
        ServiceCommand::Stop => service::stop(),
        ServiceCommand::Configure { server, password, password_file, clear_password } => {
            let password = password_from_file(password, password_file)?;
            if clear_password {
                service::clear_service_password()?;
            }
            if server.is_some() || password.is_some() {
                service::write_service_config(server.as_deref(), password.as_deref())?;
            }
            // Pick up the new config if the service is running.
            if service::status().1 {
                service::restart()?;
            }
            println!("Service settings updated.");
            Ok(())
        }
        ServiceCommand::Run => service::run_dispatcher(),
    }
}

/// Shows a fatal error to the user: a message box on Windows (we may have no
/// console), stderr elsewhere.
fn fatal(message: &str) -> ! {
    #[cfg(windows)]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        MessageBoxW(None, &HSTRING::from(message), &HSTRING::from("PJDesk"), MB_OK | MB_ICONERROR);
    }
    eprintln!("{message}");
    std::process::exit(1);
}

/// Attaches to the parent console if there is one, so a
/// `windows_subsystem = "windows"` build still works as a CLI.
///
/// Never AllocConsole: the app spawns the viewer (`connect`) as a child, and
/// allocating a console there popped up a command window for every session.
/// A failed attach is normal when the service relaunched this process. It must
/// not leave the std handles invalid.
#[cfg(windows)]
fn ensure_console() {
    unsafe {
        use windows::Win32::System::Console::{AttachConsole, GetConsoleWindow, ATTACH_PARENT_PROCESS};
        if GetConsoleWindow().0.is_null() {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
    repair_stdio_handles();
}

/// Point a missing stdin, stdout, or stderr at NUL.
///
/// `GetStdHandle` is null or `INVALID_HANDLE_VALUE` when this process was
/// started by the service (`CreateProcessAsUser`, no inherited console). Rust
/// then reports os error 6 for a console read, a log line, or a child spawn
/// that tries to inherit the handle.
#[cfg(windows)]
fn repair_stdio_handles() {
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};

    unsafe {
        let nul = windows::core::HSTRING::from("NUL");
        for (id, access) in [
            (STD_INPUT_HANDLE, GENERIC_READ.0),
            (STD_OUTPUT_HANDLE, GENERIC_WRITE.0),
            (STD_ERROR_HANDLE, GENERIC_WRITE.0),
        ] {
            if stdio_handle_ok(id) {
                continue;
            }
            let Ok(handle) = CreateFileW(
                &nul,
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            ) else {
                continue;
            };
            if SetStdHandle(id, handle).is_err() {
                let _ = CloseHandle(handle);
            }
        }
    }
}

#[cfg(windows)]
fn stdio_handle_ok(id: windows::Win32::System::Console::STD_HANDLE) -> bool {
    unsafe {
        match windows::Win32::System::Console::GetStdHandle(id) {
            Ok(handle) => !handle.is_invalid() && handle != windows::Win32::Foundation::HANDLE::default(),
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
fn ensure_console() {}

fn login_command(config: &mut Config, args: LoginArgs, server_flag: Option<String>) -> Result<()> {
    let server = resolve_server(config, server_flag, true)?;
    let username = match args.username {
        Some(u) => u,
        None => {
            print!("Username: ");
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            line.trim().to_string()
        }
    };
    let password = match args.password {
        Some(p) => p,
        None => rpassword::prompt_password(format!("Password for {username}: "))?,
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let info = runtime.block_on(crate::account::login(&server, config, &username, &password))?;
    println!(
        "Signed in as {}{}. Address book: {} saved computer(s).",
        if info.display_name.is_empty() { username.as_str() } else { info.display_name.as_str() },
        if info.is_admin { " (administrator)" } else { "" },
        info.address_book.len()
    );
    Ok(())
}

fn update_command(check: bool) -> Result<()> {
    use pjdesk_proto::update::Outcome;
    if !check {
        // This process is the administrator prompt (or an elevated console).
        // It may lock down the update folder. The normal window does not.
        return match pjdesk_proto::update::install_client_as_service(env!("CARGO_PKG_VERSION"), &|line| {
            println!("{line}");
        })? {
            Outcome::UpToDate(v) => {
                println!("PJDesk {v} is up to date.");
                Ok(())
            }
            Outcome::Found(v) => {
                println!("PJDesk {v} is available.");
                Ok(())
            }
            Outcome::Installing(v) => {
                println!("Installing PJDesk {v}...");
                Ok(())
            }
        };
    }
    match pjdesk_proto::update::perform_client(pjdesk_proto::UPDATE_REPO, env!("CARGO_PKG_VERSION"), true)? {
        Outcome::UpToDate(v) => println!("PJDesk {v} is up to date."),
        Outcome::Found(v) => println!("PJDesk {v} is available (run `pjdesk update` to install it)."),
        Outcome::Installing(v) => println!("Installing PJDesk {v}..."),
    }
    Ok(())
}

fn logout_command(config: &mut Config, server_flag: Option<String>) -> Result<()> {
    if config.account.is_none() {
        println!("Not signed in.");
        return Ok(());
    }
    let server = resolve_server(config, server_flag, false)?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(crate::account::logout(&server, config))?;
    println!("Signed out.");
    Ok(())
}

/// Works out which server to use, prompting on first run if allowed.
fn resolve_server(config: &mut Config, flag: Option<String>, may_prompt: bool) -> Result<ServerAddr> {
    if let Some(text) = flag {
        let addr = ServerAddr::parse(&text)?;
        if config.server.is_none() {
            config.server = Some(addr.to_string());
            config.save()?;
        }
        return Ok(addr);
    }
    if let Some(text) = &config.server {
        return ServerAddr::parse(text);
    }
    if !may_prompt {
        bail!("no server configured. Run: pjdesk config --set-server <address of your PJDesk server>");
    }
    print!("Address of your PJDesk server (e.g. 192.168.1.10 or desk.example.com): ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let addr = ServerAddr::parse(&line)?;
    config.server = Some(addr.to_string());
    config.save()?;
    println!("Saved to {}", Config::path().display());
    Ok(addr)
}

fn config_command(config: &mut Config, args: ConfigArgs, server_flag: Option<String>) -> Result<()> {
    let mut changed = false;
    if let Some(text) = args.set_server.or(server_flag) {
        let addr = ServerAddr::parse(&text)?;
        if config.server.as_deref() != Some(&addr.to_string()) {
            // A different server has a different key.
            config.server_fingerprint = None;
        }
        config.server = Some(addr.to_string());
        changed = true;
    }
    if args.reset_fingerprint {
        config.server_fingerprint = None;
        changed = true;
    }
    if let Some(fp) = args.fingerprint {
        let fp = tls::parse_fingerprint(&fp)?;
        config.server_fingerprint = Some(tls::format_fingerprint(&fp));
        changed = true;
    }
    if args.reset_id {
        config.device_id = None;
        config.device_secret = None;
        changed = true;
    }
    if changed {
        config.save()?;
        println!("Saved.");
    }
    println!("Config file : {}", Config::path().display());
    println!("Server      : {}", config.server.as_deref().unwrap_or("(not set)"));
    println!(
        "Server key  : {}",
        config.server_fingerprint.as_deref().unwrap_or("(not pinned yet; trusted on first connection)")
    );
    println!(
        "Device ID   : {}",
        config.device_id.as_deref().map(format_id).unwrap_or_else(|| "(assigned when hosting starts)".into())
    );
    Ok(())
}

/// Double-click mode: host in the background, and offer to control others.
fn interactive(mut config: Config, server_flag: Option<String>) -> Result<()> {
    println!("PJDesk {} — remote desktop", env!("CARGO_PKG_VERSION"));
    let server = resolve_server(&mut config, server_flag, true)?;

    let host_server = server.clone();
    let host_config = config.clone();
    std::thread::Builder::new().name("host".into()).spawn(move || {
        if let Err(e) = host::run(host_server, host_config, HostArgs::default()) {
            eprintln!("\nSharing this computer stopped: {e:#}");
        }
    })?;

    // Let the host print its ID/password first.
    let start = Instant::now();
    while !HOST_ONLINE.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(50));
    }

    let stdin = std::io::stdin();
    loop {
        print!(
            "\nTo control another computer, type its ID and press Enter (keep this window open to stay reachable): "
        );
        std::io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            // No console input (e.g. started from a script): just keep hosting.
            loop {
                std::thread::park();
            }
        }
        let id = normalize_id(&line);
        if id.is_empty() {
            continue;
        }
        if !is_valid_id(&id) {
            println!("\"{}\" isn't a PJDesk ID (IDs have 9 digits).", line.trim());
            continue;
        }
        let password = rpassword::prompt_password(format!("Password for {}: ", format_id(&id)))?;
        // Each viewer runs in its own process with its own window.
        let exe = std::env::current_exe().context("locating pjdesk.exe")?;
        let spawned = std::process::Command::new(exe)
            .arg("--server")
            .arg(server.to_string())
            .arg("connect")
            .arg(&id)
            .env("PJDESK_PASSWORD", password.trim())
            .spawn();
        match spawned {
            Ok(_) => println!("Opening a viewer for {}...", format_id(&id)),
            Err(e) => println!("Could not start the viewer: {e}"),
        }
    }
}

fn init_logging() {
    init_log_file("info,quinn=warn,quinn_proto=warn,rustls=warn");
}

/// GUI logging goes to a rolling file under the config folder, since there may
/// be no console attached. The viewer process uses the same file, so a failed
/// connection shows up in `%APPDATA%\\PJDesk\\pjdesk.log`.
fn init_gui_logging() {
    init_log_file("info,quinn=warn,quinn_proto=warn,rustls=warn,wgpu=warn,naga=warn,eframe=warn,egui_wgpu=warn");
}

fn init_log_file(default_filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false);
    if let Some(dir) = Config::path().parent() {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("pjdesk.log")) {
            builder.with_ansi(false).with_writer(std::sync::Mutex::new(file)).try_init().ok();
            return;
        }
    }
    // A missing console makes stderr itself os error 6. Drop the line rather
    // than failing the session while trying to record why it failed.
    #[cfg(windows)]
    if !stdio_handle_ok(windows::Win32::System::Console::STD_ERROR_HANDLE) {
        let _ = builder.with_writer(std::sync::Mutex::new(std::io::sink())).try_init();
        return;
    }
    let _ = builder.with_writer(std::io::stderr).try_init();
}

/// Where to look on the computer that is sharing its screen.
pub fn host_log_hint() -> &'static str {
    "On that computer, check %APPDATA%\\PJDesk\\pjdesk.log. If the PJDesk service is hosting, also check C:\\ProgramData\\PJDesk\\service\\service.log."
}

/// The viewer thread has to be STA before anything else initializes COM.
#[cfg(windows)]
fn init_viewer_com() {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

#[cfg(not(windows))]
fn init_viewer_com() {}

fn install_session_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        note_failure(&format!("The viewer stopped unexpectedly: {info}"));
        default(info);
    }));
}

/// Writes the reason a viewer or connect command stopped, next to pjdesk.log.
/// The main window reads this when the viewer process exits.
fn note_failure(text: &str) {
    let text = phone_view_failure(text);
    let connect = std::env::args().any(|a| a == "connect");
    if connect {
        tracing::error!("session failed: {text}");
    } else {
        tracing::error!("{text}");
    }
    if let Some(dir) = Config::path().parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join("last-session.txt"), &text);
        let _ = std::fs::write(dir.join("session-status.txt"), &text);
    }
}

/// Clear sentences for a Windows-to-phone view. Other connections keep their own text.
fn phone_view_failure(text: &str) -> String {
    if std::env::var("PJDESK_ASK_VIEW").ok().as_deref() != Some("1") {
        return text.to_string();
    }
    let lower = text.to_ascii_lowercase();
    if lower.contains("declin") || lower.contains("not allowed to view") {
        "Phone declined".into()
    } else if lower.contains("screen sharing") || lower.contains("screen capture") || lower.contains("mediaprojection")
    {
        "Screen sharing not allowed on the phone".into()
    } else if lower.contains("timed out")
        || lower.contains("did not answer")
        || lower.contains("no answer")
        || lower.contains("offline")
        || lower.contains("closed")
        || lower.contains("asleep")
        || lower.contains("did not finish")
    {
        "Phone app is closed or asleep".into()
    } else {
        text.to_string()
    }
}

/// A step the main window can show while the viewer is still running.
pub fn note_status(text: &str) {
    tracing::info!("{text}");
    if let Some(dir) = Config::path().parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join("session-status.txt"), text);
    }
}

/// Best-effort name of this computer, shown to the other side.
pub fn hostname() -> String {
    for var in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(name) = std::env::var(var) {
            if !name.trim().is_empty() {
                return name.trim().to_string();
            }
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "pjdesk".into())
}

#[cfg(test)]
mod cli_lock {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn no_flag_skips_the_window_lock() {
        let help = Cli::command().render_long_help().to_string().to_lowercase();
        assert!(!help.contains("no-pin"), "{help}");
        assert!(!help.contains("skip-lock"), "{help}");
        assert!(!help.contains("--unlocked"), "{help}");
    }
}
