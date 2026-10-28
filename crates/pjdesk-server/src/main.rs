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

mod access;
mod account_mirror;
mod access_keys;
mod accounts;
mod audit;
mod cloud;
mod control;
mod failover;
mod gui;
mod handoff;
mod password_policy;
mod public_addr;
mod registry;
mod relay;
mod requests;
mod role_handoff;
mod self_update;
mod server;
#[cfg(windows)]
mod service;
#[cfg(not(windows))]
#[path = "service_stub.rs"]
mod service;
mod status_pipe;
mod status_view;
#[cfg(windows)]
mod tray;
#[cfg(not(windows))]
#[path = "tray_stub.rs"]
mod tray;
mod tray_tip;
mod ui_icons;
mod ui_theme;
mod ui_widgets;
mod traywatch;
mod window_lock;
mod winshow;

use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pjdesk-server", version, about = "PJDesk signaling, rendezvous and relay server")]
struct Cli {
    #[command(flatten)]
    opts: ServerOpts,
    /// Tray icon only. This process never opens a window. Windows starts this at sign-in.
    #[arg(long, hide = true)]
    tray: bool,
    /// Window only. This process never owns the tray icon. The tray starts it.
    #[arg(long, hide = true)]
    window: bool,
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
    /// After a silent install: restart the service and the signed-in user's tray.
    #[command(name = "update-handoff", hide = true)]
    UpdateHandoff,
    /// Check for a newer PJDesk Server and install it.
    Update {
        /// Only report whether an update is available; don't install.
        #[arg(long)]
        check: bool,
        /// Used by the service and the scheduled task. Installs only when
        /// automatic updates are on, and waits for a standby when this server
        /// is the primary.
        #[arg(long, hide = true)]
        auto: bool,
    },
    /// Turn automatic server updates on or off. Off until you run this.
    #[command(name = "update-task")]
    UpdateTask {
        /// Turn automatic updates off and remove the scheduled task.
        #[arg(long)]
        disable: bool,
    },
    /// Leave an update request for the running server (used by the window when
    /// it isn't elevated). TARGET is a device ID, or "*" for every online one.
    #[command(hide = true)]
    RequestUpdate { target: String },
    /// Forget a device (used by the window when it isn't elevated).
    #[command(hide = true)]
    ForgetDevice { id: String },
    /// Remove a computer and block it from registering again.
    #[command(hide = true)]
    BlockDevice { id: String },
    /// Let a blocked computer register again.
    #[command(hide = true)]
    UnblockDevice { id: String },
    /// Replace the account-mirror signing key. The new private key is not printed.
    #[command(name = "rotate-sign-key")]
    RotateSignKey,
    /// Manage user accounts (sign-in for the address book and saved logins).
    #[command(subcommand)]
    Account(AccountCommand),
    /// Cloud sync to a Cloudflare R2 bucket, for a backup server.
    #[command(subcommand)]
    Cloud(CloudCommand),
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
    /// Set an account's role: user, operator, or admin.
    ///
    /// Operators can group computers and read the audit log. Admins can also
    /// change roles and the server policy. Users sign in and sync an address book.
    Role { username: String, role: String },
}

#[derive(Subcommand)]
enum CloudCommand {
    /// Sign in to PJDesk Cloud with your PJDesk account. The first server
    /// signed in to an account is the main one; another server signed in to
    /// the same account becomes its backup.
    Signin {
        #[arg(long)]
        email: String,
        /// Prompted for if omitted.
        #[arg(long)]
        password: Option<String>,
        /// PJDesk Cloud address (default https://pjdesk.com).
        #[arg(long)]
        api: Option<String>,
        /// Take the account's copy even if computers are connected here.
        #[arg(long, conflicts_with = "replace")]
        backup: bool,
        /// Replace the account's copy with this server's data.
        #[arg(long)]
        replace: bool,
    },
    /// Sign out of PJDesk Cloud and stop syncing.
    Signout,
    /// Use your own Cloudflare R2 / S3 bucket instead: turn on sync as the
    /// main server and upload now. Prints the join code for backup servers.
    Setup {
        /// S3 endpoint from Cloudflare (https://<account-id>.r2.cloudflarestorage.com) or the account ID.
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        bucket: String,
        #[arg(long)]
        access_key_id: String,
        #[arg(long)]
        secret_access_key: String,
        /// Replace a copy already in the bucket.
        #[arg(long)]
        replace: bool,
    },
    /// Join as a backup server: download the main server's key, accounts and
    /// computer list (replacing this server's) and keep them in sync.
    Join { code: String },
    /// Print the join code for adding a backup server.
    Code,
    /// Show the sync status.
    Status,
    /// Sync now.
    Sync,
    /// Make this server the primary. It pulls the latest copy, then writes it
    /// back with a higher generation so the other server becomes the standby.
    Promote {
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Stop uploading. This server becomes a standby; another server stays
    /// primary until it is promoted.
    Demote {
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Stop syncing on this server (the copy in the bucket is kept).
    Off,
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
    /// Stop the service if it is running, then start it.
    Restart,
    /// Entry point used by the Service Control Manager.
    #[command(hide = true)]
    Run,
}

/// The tray process never draws the window. A click starts `--window`
/// or brings the existing window forward. A double-click with nothing
/// running starts the tray and draws the window in this process, once.
#[cfg(windows)]
fn launch_windows(data_dir: std::path::PathBuf, port: u16, tray: bool, window: bool) -> Result<()> {
    if tray {
        return tray::run_tray(data_dir, port);
    }
    if window || tray::gui_is_running() {
        if !window && tray::gui_is_running() {
            tray::signal_gui();
            tracing::info!("PJDesk Server is already open. Asked that window to come forward.");
            return Ok(());
        }
        return gui::run(data_dir, port);
    }
    if tray::tray_is_running() {
        tray::signal_tray();
        tracing::info!("Asked the PJDesk Server tray to open the window.");
        return Ok(());
    }
    if !tray::spawn_tray(port, &data_dir) {
        tracing::info!("Could not start the PJDesk Server tray. The window will still open.");
    }
    gui::run(data_dir, port)
}

fn main() -> Result<()> {
    // The SCM-launched `service run` has no console and sets up its own logging.
    let raw: Vec<String> = std::env::args().collect();
    let is_background = (raw.get(1).map(|s| s == "service").unwrap_or(false) && raw.iter().any(|a| a == "run"))
        || raw.iter().any(|a| a == "--tray" || a == "--window" || a == "update-handoff");
    if raw.len() > 1 && !is_background {
        ensure_console();
    }
    let cli = Cli::parse();
    let data_dir = server::resolve_data_dir(cli.opts.data_dir.clone());

    match cli.command {
        None => {
            // Default: the management window on Windows, plus a tray process
            // that never draws it. `--tray` is the icon only. `--window` is
            // the window only. The console server is the non-Windows default.
            #[cfg(windows)]
            {
                init_window_logging();
                if let Err(e) = launch_windows(data_dir, cli.opts.port, cli.tray, cli.window) {
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
        Some(Command::UpdateHandoff) => handoff::run(),
        Some(Command::Update { check, auto }) => {
            std::fs::create_dir_all(&data_dir)?;
            if check {
                println!("{}", self_update::check_now(&data_dir)?);
                return Ok(());
            }
            if auto && !self_update::Settings::load(&data_dir).auto {
                println!("Automatic server updates are off. Run `pjdesk-server update-task` to turn them on.");
                return Ok(());
            }
            println!("{}", self_update::install_if_planned(&data_dir, !auto)?);
            Ok(())
        }
        Some(Command::UpdateTask { disable }) => {
            std::fs::create_dir_all(&data_dir)?;
            self_update::set_task(&data_dir, !disable)?;
            if disable {
                println!("Automatic server updates are off.");
            } else {
                println!("Automatic server updates are on. The service checks about once an hour.");
            }
            Ok(())
        }
        Some(Command::RequestUpdate { target }) => {
            std::fs::create_dir_all(&data_dir)?;
            requests::push(&data_dir, requests::UPDATE_FILE, &target)?;
            Ok(())
        }
        Some(Command::ForgetDevice { id }) => {
            std::fs::create_dir_all(&data_dir)?;
            requests::forget_device(&data_dir, &id)
        }
        Some(Command::BlockDevice { id }) => {
            std::fs::create_dir_all(&data_dir)?;
            requests::block_device(&data_dir, &id)
        }
        Some(Command::UnblockDevice { id }) => {
            std::fs::create_dir_all(&data_dir)?;
            requests::unblock_device(&data_dir, &id)
        }
        Some(Command::RotateSignKey) => {
            std::fs::create_dir_all(&data_dir)?;
            match account_mirror::rotate_sign_key(&data_dir) {
                Ok(sentence) => {
                    println!("{sentence}");
                    Ok(())
                }
                Err(err) => anyhow::bail!("{err}"),
            }
        }
        Some(Command::Account(cmd)) => account_command(cmd, &data_dir),
        Some(Command::Cloud(cmd)) => cloud_command(cmd, &data_dir),
        Some(Command::Service(cmd)) => service_command(cmd, cli.opts, data_dir),
    }
}

/// Shows a fatal error: a message box on Windows (no console), stderr elsewhere.
#[cfg_attr(not(windows), allow(dead_code))]
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
            println!(
                "{} account \"{}\"{}.",
                if existed { "Updated" } else { "Created" },
                accounts::normalize_username(&username),
                if admin { " (administrator)" } else { "" }
            );
        }
        AccountCommand::List => {
            let list = accounts.list();
            if list.is_empty() {
                println!("No accounts yet. Add one with: pjdesk-server account add <username>");
            }
            for a in list {
                let name = if a.display_name.is_empty() || a.display_name == a.username {
                    String::new()
                } else {
                    format!(" ({})", a.display_name)
                };
                println!(
                    "{:20} {:10}{}{}",
                    a.username,
                    a.role.as_str(),
                    name,
                    if a.disabled { "  (disabled)" } else { "" }
                );
            }
        }
        AccountCommand::Role { username, role } => {
            let parsed = match role.to_ascii_lowercase().as_str() {
                "user" => pjdesk_proto::signal::Role::User,
                "operator" => pjdesk_proto::signal::Role::Operator,
                "admin" | "administrator" => pjdesk_proto::signal::Role::Admin,
                other => anyhow::bail!("role must be user, operator, or admin (got {other})"),
            };
            accounts.set_role(&username, parsed)?;
            let _ = crate::requests::push(data_dir, crate::requests::RELOAD_FILE, "now");
            println!("{} is now {}.", accounts::normalize_username(&username), parsed.as_str());
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

fn cloud_command(cmd: CloudCommand, data_dir: &std::path::Path) -> Result<()> {
    use cloud::{CloudConfig, CloudStatus};
    std::fs::create_dir_all(data_dir)?;
    match cmd {
        CloudCommand::Setup { endpoint, bucket, access_key_id, secret_access_key, replace } => {
            let cfg = CloudConfig::new(&endpoint, &bucket, &access_key_id, &secret_access_key)?;
            if matches!(cloud::check_bucket(&cfg)?, cloud::SetupCheck::Occupied) && !replace {
                anyhow::bail!(
                    "bucket \"{}\" already holds a PJDesk server copy. To make this server the main one anyway, \
                     add --replace; to make it a backup, use `cloud join` with the main server's join code.",
                    cfg.bucket
                );
            }
            cloud::setup(data_dir, &cfg)?;
            let _ = requests::push(data_dir, requests::RELOAD_FILE, "now");
            println!("Cloud sync is on; this server's state is uploaded to \"{}\".", cfg.bucket);
            println!("\nJoin code for a backup server (keep it like a password):\n\n{}\n", cfg.join_code());
        }
        CloudCommand::Signin { email, password, api, backup, replace } => {
            let password = match password {
                Some(p) => p,
                None => rpassword::prompt_password(format!("Password for {email}: "))?,
            };
            let api = api.unwrap_or_else(cloud::default_api);
            let cfg = cloud::sign_in(&api, &email, &password)?;
            let choice = if replace {
                Some(cloud::Choice::Replace)
            } else if backup {
                Some(cloud::Choice::Join)
            } else {
                None
            };
            let online = cloud::computers_online(data_dir);
            match cloud::connect_account(data_dir, &cfg, online, choice)? {
                cloud::Connected::Uploaded => {
                    let _ = requests::push(data_dir, requests::SYNC_FILE, "now");
                    println!(
                        "Signed in as {}. This is the main server; its data is uploaded to PJDesk Cloud.",
                        cfg.email
                    );
                }
                cloud::Connected::Resumed => println!("Signed in as {}; syncing as before.", cfg.email),
                cloud::Connected::Joined { from } => {
                    let _ = requests::push(data_dir, requests::RELOAD_FILE, "now");
                    println!("Signed in as {}. This server now has {from}'s key, accounts and computer list, and is on standby.", cfg.email);
                }
                cloud::Connected::Replaced => {
                    let _ = requests::push(data_dir, requests::SYNC_FILE, "now");
                    println!("Signed in as {}. This server's data replaced the copy in PJDesk Cloud.", cfg.email);
                }
                cloud::Connected::Ask { from, written_at, readable } => {
                    cloud::discard_sign_in(&cfg);
                    if readable {
                        anyhow::bail!(
                            "{} already holds {from}'s data (from {}) and {online} computer(s) are connected here. \
                             Run again with --backup to make this server a backup of it, or --replace to upload this server's data instead.",
                            cfg.email,
                            registry::format_local(written_at)
                        );
                    }
                    anyhow::bail!("the account's cloud copy was made with an older password; run again with --replace to upload this server's data");
                }
            }
        }
        CloudCommand::Signout => {
            cloud::sign_out(data_dir);
            println!("Signed out; cloud sync is off on this server.");
        }
        CloudCommand::Join { code } => {
            let cfg = CloudConfig::from_join_code(&code)?;
            let from = cloud::join(data_dir, &cfg)?;
            let _ = requests::push(data_dir, requests::RELOAD_FILE, "now");
            println!("Joined: this server now has the key, accounts and computer list from {from}, and stays in sync.");
            if let Some(id) = server::read_identity(data_dir) {
                println!("Server key fingerprint: {}", pjdesk_proto::tls::format_fingerprint(&id.fingerprint()));
            }
        }
        CloudCommand::Code => match CloudConfig::load(data_dir) {
            Some(cfg) => println!("{}", cfg.join_code()),
            None => anyhow::bail!("cloud sync isn't set up on this server"),
        },
        CloudCommand::Status => match (CloudConfig::load(data_dir), CloudStatus::load(data_dir)) {
            (None, _) => println!("Cloud sync is off."),
            (Some(cfg), status) => {
                let s = status.unwrap_or_default();
                if cfg.is_account() {
                    println!("Signed in to PJDesk Cloud as {} ({})", cfg.email, cfg.api);
                } else {
                    println!("Bucket: {} ({})", cfg.bucket, cfg.endpoint);
                }
                println!("Role: {}", if s.role.is_empty() { "unknown (is the server running?)" } else { &s.role });
                if s.epoch > 0 || !s.active_name.is_empty() {
                    let who = if s.active_name.is_empty() { "unknown" } else { &s.active_name };
                    println!("Generation: {} (active server: {who})", s.epoch);
                }
                if !s.notice.is_empty() {
                    println!("Notice: {}", s.notice);
                }
                if s.last_upload_at > 0 {
                    println!("Last upload: {}", registry::format_local(s.last_upload_at));
                }
                if !s.applied_from.is_empty() {
                    println!("Last synced from: {} ({})", s.applied_from, registry::format_local(s.applied_written_at));
                }
                if !s.error.is_empty() {
                    println!("Error: {}", s.error);
                }
            }
        },
        CloudCommand::Sync => {
            requests::push(data_dir, requests::SYNC_FILE, "now")?;
            println!("Asked the running server to sync now.");
        }
        CloudCommand::Promote { yes } => {
            if !confirm(
                yes,
                "Make this server the primary? It will download the latest cloud copy, then the other server becomes the standby the next time it checks.",
            )? {
                println!("Cancelled.");
                return Ok(());
            }
            println!("{}", role_handoff::become_primary(data_dir)?);
        }
        CloudCommand::Demote { yes } => {
            if !confirm(
                yes,
                "Step this server down? It hands the primary to the other server and stays primary if that server does not take it.",
            )? {
                println!("Cancelled.");
                return Ok(());
            }
            println!("{}", role_handoff::step_down(data_dir)?);
        }
        CloudCommand::Off => {
            cloud::turn_off(data_dir);
            println!("Cloud sync is off on this server (the copy in the bucket is kept).");
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
        ServiceCommand::Restart => service::restart(),
        ServiceCommand::Run => service::run_dispatcher(opts, data_dir),
    }
}

#[cfg(not(windows))]
fn service_command(_cmd: ServiceCommand, _opts: ServerOpts, _data_dir: PathBuf) -> Result<()> {
    anyhow::bail!("service management is only available on Windows; run `pjdesk-server` directly or use systemd")
}

fn confirm(yes: bool, prompt: &str) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    eprint!("{prompt} [y/N] ");
    use std::io::Write;
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
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

/// The tray has no console. Activation is written next to the client log.
#[cfg(windows)]
fn init_window_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,wgpu=warn,naga=warn,eframe=warn,egui_wgpu=warn"));
    let dir = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let dir = dir.join("PJDesk");
    let _ = std::fs::create_dir_all(&dir);
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false).with_ansi(false);
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("pjdesk-server.log")) {
        let _ = builder.with_writer(std::sync::Mutex::new(file)).try_init();
        return;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .try_init();
}

fn init_console_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init();
}
