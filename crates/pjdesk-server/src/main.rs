#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! PJDesk server.
//!
//! One small executable that provides:
//! * **Signaling** on TCP (TLS 1.3): device registration, ID management and
//!   session setup between controllers and hosts.
//! * **Rendezvous** on UDP: tells each peer its public address so they can
//!   punch through NATs and connect directly.
//! * **Relay** on the same UDP port: forwards the peers' (end-to-end encrypted)
//!   QUIC packets when a direct connection is impossible.
//!
//! Runs in a console (`pjdesk-server`) or as a Windows service
//! (`pjdesk-server service install`).

mod accounts;
#[cfg(windows)]
mod gui;
mod registry;
#[cfg(windows)]
mod tray;
mod relay;
mod server;
#[cfg(windows)]
mod service;

use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pjdesk-server", version, about = "PJDesk signaling, rendezvous and relay server")]
struct Cli {
    #[command(flatten)]
    opts: ServerOpts,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Options shared by every mode.
#[derive(Args, Clone, Debug)]
pub struct ServerOpts {
    /// IP address to listen on.
    #[arg(long, global = true, default_value = "0.0.0.0")]
    pub bind: IpAddr,
    /// Port used for both TCP (signaling) and UDP (rendezvous + relay).
    #[arg(long, global = true, default_value_t = pjdesk_proto::DEFAULT_PORT)]
    pub port: u16,
    /// Folder for the server key/certificate, device registry and logs.
    /// Default: %ProgramData%\PJDesk\server on Windows.
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server in this console (the default).
    Run,
    /// Print the server key fingerprint, so clients can pin it.
    Fingerprint,
    /// Check for a newer PJDesk Server and install it (the daily task runs this).
    Update {
        /// Only report whether an update is available; don't install.
        #[arg(long)]
        check: bool,
    },
    /// Leave an update request for the running server (used by the window when
    /// it isn't elevated). TARGET is a device ID, or "*" for every online one.
    #[command(hide = true)]
    RequestUpdate { target: String },
    /// Manage user accounts (sign-in for the address book and saved logins).
    #[command(subcommand)]
    Account(AccountCommand),
    /// Install, remove, start or stop the Windows service.
    #[command(subcommand)]
    Service(ServiceCommand),
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Add an account, or change its password (prompts if --password is omitted).
    Add {
        username: String,
        #[arg(long)]
        password: Option<String>,
        /// Grant administrator rights.
        #[arg(long)]
        admin: bool,
    },
    /// List accounts.
    List,
    /// Remove an account and its address book.
    Remove { username: String },
    /// Enable or disable an account without deleting it.
    Disable {
        username: String,
        /// Re-enable instead of disabling.
        #[arg(long)]
        undo: bool,
    },
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Install as an auto-start Windows service (run from an elevated prompt).
    Install {
        /// Also add Windows Defender Firewall rules for the server.
        #[arg(long)]
        firewall: bool,
    },
    /// Stop and remove the Windows service (and its firewall rules).
    Uninstall,
    /// Start the installed service.
    Start,
    /// Stop the running service.
    Stop,
    /// Entry point used by the Service Control Manager.
    #[command(hide = true)]
    Run,
}

fn main() -> Result<()> {
    // The SCM-launched `service run` has no console and sets up its own logging.
    let raw: Vec<String> = std::env::args().collect();
    let is_background = raw.get(1).map(|s| s == "service").unwrap_or(false) && raw.iter().any(|a| a == "run");
    if raw.len() > 1 && !is_background {
        ensure_console();
    }
    let cli = Cli::parse();
    let data_dir = server::resolve_data_dir(cli.opts.data_dir.clone());

    match cli.command {
        None => {
            // Default: the management window on Windows; the console server
            // elsewhere (the GUI is Windows-only).
            #[cfg(windows)]
            {
                if let Err(e) = gui::run(data_dir, cli.opts.port) {
                    fatal(&format!("PJDesk Server could not start: {e:#}"));
                }
                Ok(())
            }
            #[cfg(not(windows))]
            {
                init_console_logging();
                let runtime = tokio::runtime::Runtime::new()?;
                runtime.block_on(server::run(cli.opts, data_dir, async {
                    let _ = tokio::signal::ctrl_c().await;
                }))
            }
        }
        Some(Command::Run) => {
            init_console_logging();
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(server::run(cli.opts, data_dir, async {
                let _ = tokio::signal::ctrl_c().await;
            }))
        }
        Some(Command::Fingerprint) => {
            std::fs::create_dir_all(&data_dir)?;
            let identity = server::load_identity(&data_dir)?;
            println!("{}", pjdesk_proto::tls::format_fingerprint(&identity.fingerprint()));
            Ok(())
        }
        Some(Command::Update { check }) => {
            use pjdesk_proto::update::Outcome;
            match pjdesk_proto::update::perform(
                pjdesk_proto::UPDATE_REPO,
                env!("CARGO_PKG_VERSION"),
                "PJDesk-Server-Setup",
                check,
            )? {
                Outcome::UpToDate(v) => println!("PJDesk Server {v} is up to date."),
                Outcome::Found(v) => println!("PJDesk Server {v} is available."),
                Outcome::Installing(v) => println!("Installing PJDesk Server {v}..."),
            }
            Ok(())
        }
        Some(Command::RequestUpdate { target }) => {
            std::fs::create_dir_all(&data_dir)?;
            let targets = vec![target];
            std::fs::write(data_dir.join("update-requests.json"), serde_json::to_vec(&targets)?)?;
            Ok(())
        }
        Some(Command::Account(cmd)) => account_command(cmd, &data_dir),
        Some(Command::Service(cmd)) => service_command(cmd, cli.opts, data_dir),
    }
}

/// Shows a fatal error: a message box on Windows (no console), stderr elsewhere.
fn fatal(message: &str) -> ! {
    #[cfg(windows)]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        MessageBoxW(None, &HSTRING::from(message), &HSTRING::from("PJDesk Server"), MB_OK | MB_ICONERROR);
    }
    eprintln!("{message}");
    std::process::exit(1);
}

/// Attaches to the parent console, or allocates one, so a windowed build still
/// works as a CLI.
#[cfg(windows)]
fn ensure_console() {
    unsafe {
        use windows::Win32::System::Console::{AllocConsole, AttachConsole, GetConsoleWindow, ATTACH_PARENT_PROCESS};
        if GetConsoleWindow().0.is_null() {
            if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
                let _ = AllocConsole();
            }
        }
    }
}

#[cfg(not(windows))]
fn ensure_console() {}

fn account_command(cmd: AccountCommand, data_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let mut accounts = accounts::Accounts::load(data_dir)?;
    match cmd {
        AccountCommand::Add { username, password, admin } => {
            let password = match password {
                Some(p) => p,
                None => {
                    let p = rpassword::prompt_password(format!("New password for \"{username}\": "))?;
                    let again = rpassword::prompt_password("Repeat password: ")?;
                    if p != again {
                        anyhow::bail!("passwords did not match");
                    }
                    p
                }
            };
            let existed = accounts.list().iter().any(|a| a.username == accounts::normalize_username(&username));
            accounts.create(&username, &password, admin, true)?;
            println!("{} account \"{}\"{}.", if existed { "Updated" } else { "Created" }, accounts::normalize_username(&username), if admin { " (administrator)" } else { "" });
        }
        AccountCommand::List => {
            let list = accounts.list();
            if list.is_empty() {
                println!("No accounts yet. Add one with: pjdesk-server account add <username>");
            }
            for a in list {
                println!(
                    "{:20} {}{}",
                    a.username,
                    if a.is_admin { "admin" } else { "user " },
                    if a.disabled { "  (disabled)" } else { "" }
                );
            }
        }
        AccountCommand::Remove { username } => {
            accounts.delete(&username)?;
            println!("Removed \"{}\".", accounts::normalize_username(&username));
        }
        AccountCommand::Disable { username, undo } => {
            accounts.set_disabled(&username, !undo)?;
            println!("{} \"{}\".", if undo { "Enabled" } else { "Disabled" }, accounts::normalize_username(&username));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn service_command(cmd: ServiceCommand, opts: ServerOpts, data_dir: PathBuf) -> Result<()> {
    match cmd {
        ServiceCommand::Install { firewall } => service::install(&opts, &data_dir, firewall),
        ServiceCommand::Uninstall => service::uninstall(),
        ServiceCommand::Start => service::start(),
        ServiceCommand::Stop => service::stop(),
        ServiceCommand::Run => service::run_dispatcher(opts, data_dir),
    }
}

#[cfg(not(windows))]
fn service_command(_cmd: ServiceCommand, _opts: ServerOpts, _data_dir: PathBuf) -> Result<()> {
    anyhow::bail!("service management is only available on Windows; run `pjdesk-server` directly or use systemd")
}

fn init_console_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init();
}
