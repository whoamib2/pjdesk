//! DeskLink server.
//!
//! One small executable that provides:
//! * **Signaling** on TCP (TLS 1.3): device registration, ID management and
//!   session setup between controllers and hosts.
//! * **Rendezvous** on UDP: tells each peer its public address so they can
//!   punch through NATs and connect directly.
//! * **Relay** on the same UDP port: forwards the peers' (end-to-end encrypted)
//!   QUIC packets when a direct connection is impossible.
//!
//! Runs in a console (`desklink-server`) or as a Windows service
//! (`desklink-server service install`).

mod registry;
mod relay;
mod server;
#[cfg(windows)]
mod service;

use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "desklink-server", version, about = "DeskLink signaling, rendezvous and relay server")]
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
    #[arg(long, global = true, default_value_t = desklink_proto::DEFAULT_PORT)]
    pub port: u16,
    /// Folder for the server key/certificate, device registry and logs.
    /// Default: %ProgramData%\DeskLink\server on Windows.
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server in this console (the default).
    Run,
    /// Print the server key fingerprint, so clients can pin it.
    Fingerprint,
    /// Install, remove, start or stop the Windows service.
    #[command(subcommand)]
    Service(ServiceCommand),
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
    let cli = Cli::parse();
    let data_dir = server::resolve_data_dir(cli.opts.data_dir.clone());

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => {
            init_console_logging();
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(server::run(cli.opts, data_dir, async {
                let _ = tokio::signal::ctrl_c().await;
            }))
        }
        Command::Fingerprint => {
            std::fs::create_dir_all(&data_dir)?;
            let identity = server::load_identity(&data_dir)?;
            println!("{}", desklink_proto::tls::format_fingerprint(&identity.fingerprint()));
            Ok(())
        }
        Command::Service(cmd) => service_command(cmd, cli.opts, data_dir),
    }
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
    anyhow::bail!("service management is only available on Windows; run `desklink-server` directly or use systemd")
}

fn init_console_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).init();
}
