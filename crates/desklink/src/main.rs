//! DeskLink client.
//!
//! One executable plays both roles:
//! * **Host** – shares this computer: stays online with the server, shows an
//!   ID and a one-time password, and streams the screen to an authenticated
//!   controller while replaying its mouse and keyboard input.
//! * **Controller** – connects to a host by ID + password and shows its
//!   screen in a window, forwarding local input.
//!
//! Double-clicking `desklink.exe` does both: it hosts in the background and
//! lets you type a partner ID to control another machine.

mod account;
mod color;
mod config;
mod host;
mod p2p;
mod secure;
mod session;
mod signaling;
mod udp_fallback;
mod viewer;

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use desklink_proto::{format_id, is_valid_id, normalize_id, tls};

use crate::config::Config;
use crate::signaling::ServerAddr;

/// Set once the host has registered and printed its ID (interactive mode
/// waits for it so the prompt doesn't interleave with the banner).
pub static HOST_ONLINE: AtomicBool = AtomicBool::new(false);

#[derive(Parser)]
#[command(name = "desklink", version, about = "DeskLink remote desktop: share this computer or control another one")]
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
    /// Sign in to your DeskLink server account (saves your login).
    Login(LoginArgs),
    /// Sign out and forget the saved login.
    Logout,
}

#[derive(Args, Clone, Debug)]
struct LoginArgs {
    /// Account username (asked interactively if omitted).
    username: Option<String>,
    /// Account password (asked interactively if omitted).
    #[arg(long, env = "DESKLINK_ACCOUNT_PASSWORD", hide_env_values = true)]
    password: Option<String>,
}

#[derive(Args, Clone, Debug)]
pub struct HostArgs {
    /// Use this fixed password instead of a new random one each start
    /// (for unattended access).
    #[arg(long, env = "DESKLINK_HOST_PASSWORD")]
    pub password: Option<String>,
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
}

impl Default for HostArgs {
    fn default() -> Self {
        Self {
            password: std::env::var("DESKLINK_HOST_PASSWORD").ok(),
            display: 0,
            fps: 30,
            bitrate: 6000,
            max_width: 2560,
            gdi: false,
            test_pattern: false,
            view_only: false,
        }
    }
}

#[derive(Args, Clone, Debug)]
pub struct ConnectArgs {
    /// ID of the computer to control, with or without spaces ("123 456 789").
    #[arg(required = true, num_args = 1..=9, value_name = "ID")]
    pub id: Vec<String>,
    /// Its password (asked interactively if omitted).
    #[arg(long, env = "DESKLINK_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,
    /// Skip the direct (P2P) attempt and go straight through the server's relay.
    #[arg(long)]
    pub force_relay: bool,
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
    let cli = Cli::parse();
    init_logging();
    let interactive = cli.command.is_none();
    if let Err(e) = run(cli) {
        eprintln!("\nError: {e:#}");
        if interactive {
            // Double-clicked: keep the window open so the message can be read.
            eprintln!("Press Enter to close.");
            let _ = std::io::stdin().lock().read_line(&mut String::new());
        }
        std::process::exit(1);
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
        None => interactive(config, cli.server),
    }
}

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
        bail!("no server configured. Run: desklink config --set-server <address of your DeskLink server>");
    }
    print!("Address of your DeskLink server (e.g. 192.168.1.10 or desk.example.com): ");
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
    println!("DeskLink {} — remote desktop", env!("CARGO_PKG_VERSION"));
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
            println!("\"{}\" isn't a DeskLink ID (IDs have 9 digits).", line.trim());
            continue;
        }
        let password = rpassword::prompt_password(format!("Password for {}: ", format_id(&id)))?;
        // Each viewer runs in its own process with its own window.
        let exe = std::env::current_exe().context("locating desklink.exe")?;
        let spawned = std::process::Command::new(exe)
            .arg("--server")
            .arg(server.to_string())
            .arg("connect")
            .arg(&id)
            .env("DESKLINK_PASSWORD", password.trim())
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
        .unwrap_or_else(|| "desklink".into())
}
