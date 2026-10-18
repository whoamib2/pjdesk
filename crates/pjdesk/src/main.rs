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

mod account;
mod audio;
mod clip;
mod color;
mod config;
mod gui;
mod handoff;
mod host;
mod identity;
mod p2p;
mod platform;
mod secure;
mod service;
mod session;
mod signaling;
mod syspaths;
mod viewer;

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
    /// Check for a newer PJDesk and install it (the daily task runs this).
    Update {
        /// Only report whether an update is available; don't install.
        #[arg(long)]
        check: bool,
    },
    /// Internal: turn the daily auto-update task on or off.
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
    /// Ports a tunnel may open on this computer (loopback).
    #[arg(skip)]
    pub tunnel_ports: Vec<u16>,
    /// True when this process is the background service helper (runs as SYSTEM).
    #[arg(skip)]
    pub service_host: bool,
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
            tunnel_ports: Vec::new(),
            service_host: false,
        }
    }
}

impl HostArgs {
    /// Builds host arguments from saved settings, using the permanent password
    /// if one is set (otherwise a fresh random password each start).
    pub fn from_settings(settings: &config::Settings) -> Self {
        Self {
            permanent_password: settings.permanent_password(),
            display: 0,
            fps: settings.fps,
            bitrate: settings.bitrate_kbps,
            max_width: settings.max_width,
            gdi: settings.force_gdi,
            test_pattern: false,
            view_only: settings.view_only,
            follow_desktop: false,
            approval: if settings.approval == "unattended" { "unattended".into() } else { "prompt".into() },
            receive_dir: settings.receive_dir.clone(),
            allow_direct: settings.allow_direct,
            tunnel_ports: settings.tunnel_ports.clone(),
            service_host: false,
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
    // The service and its helper run with no console and set up their own
    // (file) logging, so route them before touching the console.
    let raw: Vec<String> = std::env::args().collect();
    let is_background = raw.iter().any(|a| a == "__session-host" || a == "update-handoff")
        || (raw.get(1).map(|s| s == "service").unwrap_or(false) && raw.iter().any(|a| a == "run"));

    // With any other argument, attach a console so output and clap errors show.
    if raw.len() > 1 && !is_background {
        ensure_console();
    }
    let cli = Cli::parse();

    match &cli.command {
        None => {
            // Default: the graphical app.
            init_gui_logging();
            if let Err(e) = gui::run(cli.server.clone()) {
                fatal(&format!("PJDesk could not start: {e:#}"));
            }
        }
        Some(Cmd::SessionHost { config }) => {
            if let Err(e) = service::session_host(config.clone()) {
                tracing::error!("session host: {e:#}");
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
            if let Err(e) = run(cli) {
                eprintln!("\nError: {e:#}");
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
        Some(Cmd::SessionHost { .. }) | None => {
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

/// Attaches to the parent console if there is one, otherwise allocates a new
/// console, so a `windows_subsystem = "windows"` build still works as a CLI.
#[cfg(windows)]
fn ensure_console() {
    unsafe {
        use windows::Win32::System::Console::{AttachConsole, GetConsoleWindow, ATTACH_PARENT_PROCESS};
        if GetConsoleWindow().0.is_null() {
            // Attach to the launching terminal if there is one, but never
            // AllocConsole: the app spawns the viewer (`connect`) as a child
            // process, and allocating a console there popped up a stray command
            // window every time you started a remote session.
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
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
    match pjdesk_proto::update::perform(pjdesk_proto::UPDATE_REPO, env!("CARGO_PKG_VERSION"), "PJDesk-Setup", check)? {
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
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,quinn=warn,quinn_proto=warn,rustls=warn"));
    let _ =
        tracing_subscriber::fmt().with_env_filter(filter).with_target(false).with_writer(std::io::stderr).try_init();
}

/// GUI logging goes to a rolling file under the config folder, since there may
/// be no console attached.
fn init_gui_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "info,quinn=warn,quinn_proto=warn,rustls=warn,wgpu=warn,naga=warn,eframe=warn,egui_wgpu=warn",
        )
    });
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false);
    if let Some(dir) = Config::path().parent() {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("pjdesk.log")) {
            builder.with_ansi(false).with_writer(std::sync::Mutex::new(file)).try_init().ok();
            return;
        }
    }
    let _ = builder.with_writer(std::io::stderr).try_init();
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
