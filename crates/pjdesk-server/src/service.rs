//! Windows service integration (install / uninstall / start / stop / run).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo,
    ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::ServerOpts;

pub const SERVICE_NAME: &str = "PJDeskServer";
const DISPLAY_NAME: &str = "PJDesk Server";
const DESCRIPTION: &str = "PJDesk remote desktop signaling, rendezvous and relay server.";
const FIREWALL_RULE: &str = "PJDesk Server";

/// The SCM calls `service_main` on its own thread; options are passed here.
static SERVICE_ARGS: OnceLock<(ServerOpts, PathBuf)> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

/// Called as `pjdesk-server service run` by the Service Control Manager.
pub fn run_dispatcher(opts: ServerOpts, data_dir: PathBuf) -> Result<()> {
    let _ = SERVICE_ARGS.set((opts, data_dir));
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("this command is only run by Windows; use `pjdesk-server service start` instead")?;
    Ok(())
}

fn service_main(_arguments: Vec<OsString>) {
    let Some((opts, data_dir)) = SERVICE_ARGS.get().cloned() else { return };
    init_file_logging(&data_dir);
    if let Err(e) = run_service(opts, data_dir) {
        tracing::error!("service failed: {e:#}");
    }
}

fn run_service(opts: ServerOpts, data_dir: PathBuf) -> Result<()> {
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status = service_control_handler::register(SERVICE_NAME, handler)?;
    let report =
        |state: ServiceState, accept: ServiceControlAccept, exit_code: ServiceExitCode, wait_hint: Duration| {
            status.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accept,
                exit_code,
                checkpoint: 0,
                wait_hint,
                process_id: None,
            })
        };

    report(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ServiceExitCode::Win32(0),
        Duration::default(),
    )?;
    // server::run protects the data folder itself, just before it reads the
    // identity (see crate::server::run).
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(crate::server::run(opts, data_dir, async move {
        let _ = stop_rx.wait_for(|stopped| *stopped).await;
        let _ = report(
            ServiceState::StopPending,
            ServiceControlAccept::empty(),
            ServiceExitCode::Win32(0),
            Duration::from_secs(10),
        );
    }));
    if let Err(e) = &result {
        tracing::error!("server stopped with an error: {e:#}");
    }
    // Close the sockets before reporting "stopped", so a restart can bind at once.
    runtime.shutdown_timeout(Duration::from_secs(3));
    let exit_code = if result.is_ok() { ServiceExitCode::Win32(0) } else { ServiceExitCode::ServiceSpecific(1) };
    report(ServiceState::Stopped, ServiceControlAccept::empty(), exit_code, Duration::default())?;
    result
}

/// Repairs the permissions on the data folder (server key/cert, device
/// registry, accounts, log) so the server can always read its own files, and
/// so other local users cannot.
///
/// The folder is owned by SYSTEM and granted only to SYSTEM and Administrators,
/// with inheritance set on the directory itself. Children are then reset so
/// they inherit that ACL. Inheritance flags must not be applied to files with
/// `/T`: an earlier version did that, `icacls` stripped each file's inherited
/// permissions, and the empty DACL denied access to everyone, SYSTEM included.
/// Resetting the directory itself would put ProgramData's ACL back, and that
/// ACL lets Users read files and create new ones. Every step is best-effort —
/// the server must still come up if it fails.
pub(crate) fn repair_data_dir(dir: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::fs::create_dir_all(dir);
    // Reclaim ownership first. An earlier version left some files with an
    // empty DACL that even SYSTEM could not read.
    let _ = std::process::Command::new("takeown")
        .arg("/f")
        .arg(dir)
        .args(["/r", "/d", "y"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    // The folder must NOT inherit ProgramData's ACL. That ACL lets Users read
    // files and create new ones, which exposed the TLS private key, account
    // tokens and address-book passwords, and let any local user drop a request
    // file the service would obey (update every host, forget a computer,
    // reload accounts). Inheritance flags are set on the directory only —
    // applying (OI)(CI) to files with /T is what emptied DACLs before.
    let status = std::process::Command::new("icacls")
        .arg(dir)
        .args(["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/Q"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    // Children pick up that ACL by inheritance. /reset on the directory itself
    // would undo the line above, so it is applied to the children only.
    let _ = std::process::Command::new("icacls")
        .arg(format!("{}\\*", dir.display()))
        .args(["/reset", "/T", "/C", "/Q"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !matches!(status, Ok(s) if s.success()) {
        tracing::warn!("could not repair permissions on {} ({status:?})", dir.display());
    }
}

fn init_file_logging(data_dir: &Path) {
    let _ = std::fs::create_dir_all(data_dir);
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir.join("server.log")) {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
            .with_ansi(false)
            .with_target(false)
            .with_writer(std::sync::Mutex::new(file))
            .try_init();
    }
}

fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
    ServiceManager::local_computer(None::<&str>, access)
        .context("cannot open the Service Control Manager; run this from an Administrator command prompt")
}

/// Installs the service (auto-start, LocalSystem) and starts it.
pub fn install(opts: &ServerOpts, data_dir: &Path, firewall: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let manager = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;

    let mut args: Vec<OsString> = vec![
        "service".into(),
        "run".into(),
        "--bind".into(),
        opts.bind.to_string().into(),
        "--port".into(),
        opts.port.to_string().into(),
    ];
    if let Some(dir) = &opts.data_dir {
        // The service starts in System32, so relative paths must be resolved now.
        args.push("--data-dir".into());
        args.push(std::path::absolute(dir)?.into_os_string());
    }

    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.clone(),
        launch_arguments: args,
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let (service, created) = open_or_create(&manager, &info)?;
    let _ = service.set_description(DESCRIPTION);

    if firewall {
        add_firewall_rule(&exe)?;
    }
    repair_data_dir(data_dir);

    start_if_needed(&service)?;
    if created {
        println!("Installed and started the \"{DISPLAY_NAME}\" service ({SERVICE_NAME}).");
    } else {
        println!("The \"{DISPLAY_NAME}\" service is already installed. It is updated and running.");
    }
    println!("It starts automatically with Windows and logs to server.log in its data folder.");
    if !firewall {
        println!("Tip: re-install with --firewall, or allow TCP+UDP port {} in Windows Firewall.", opts.port);
    }
    Ok(())
}

/// Stops (if needed) and deletes the service, and removes the firewall rule.
pub fn uninstall() -> Result<()> {
    let manager = manager(ServiceManagerAccess::CONNECT)?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)
        .context("the service is not installed")?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        let _ = service.stop();
        for _ in 0..50 {
            if service.query_status()?.current_state == ServiceState::Stopped {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    service.delete()?;
    remove_firewall_rule();
    println!("Removed the \"{DISPLAY_NAME}\" service.");
    Ok(())
}

pub fn start() -> Result<()> {
    let manager = manager(ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(SERVICE_NAME, ServiceAccess::START).context("the service is not installed")?;
    service.start::<&str>(&[])?;
    println!("Started {SERVICE_NAME}.");
    Ok(())
}

/// Whether the service exists, and whether it is running (for the GUI).
pub fn status() -> (bool, bool) {
    let Ok(manager) = manager(ServiceManagerAccess::CONNECT) else { return (false, false) };
    match manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(service) => match service.query_status() {
            Ok(s) => (true, s.current_state == ServiceState::Running),
            Err(_) => (true, false),
        },
        Err(_) => (false, false),
    }
}

pub fn stop() -> Result<()> {
    let manager = manager(ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS).context("the service is not installed")?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        service.stop()?;
        for _ in 0..50 {
            if service.query_status()?.current_state == ServiceState::Stopped {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    println!("Stopped {SERVICE_NAME}.");
    Ok(())
}

/// Stops the service if it is running, then starts it. One elevation covers both.
pub fn restart() -> Result<()> {
    let dir = crate::server::resolve_data_dir(None);
    crate::cloud::announce_maintenance(&dir);
    let _ = stop();
    start()
}

/// One program-based inbound rule, plus explicit TCP and UDP 21200 rules.
/// The program path is quoted the way netsh parses it, so a path with spaces applies.
fn add_firewall_rule(exe: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    remove_firewall_rule();
    let program = format!("program=\"{}\"", exe.display());
    let status = std::process::Command::new("netsh")
        .raw_arg(format!(
            "advfirewall firewall add rule name=\"{FIREWALL_RULE}\" dir=in action=allow {program} enable=yes profile=any"
        ))
        .status()
        .context("running netsh")?;
    if !status.success() {
        anyhow::bail!("netsh could not add the firewall rule");
    }
    for (name, proto) in [("PJDesk Server TCP", "TCP"), ("PJDesk Server UDP", "UDP")] {
        let _ = std::process::Command::new("netsh")
            .raw_arg(format!(
                "advfirewall firewall add rule name=\"{name}\" dir=in action=allow protocol={proto} localport=21200 profile=any"
            ))
            .status();
    }
    println!("Added Windows Firewall rule \"{FIREWALL_RULE}\".");
    Ok(())
}

fn remove_firewall_rule() {
    use std::os::windows::process::CommandExt;
    for name in [FIREWALL_RULE, "PJDesk Server TCP", "PJDesk Server UDP"] {
        let _ = std::process::Command::new("netsh")
            .raw_arg(format!("advfirewall firewall delete rule name=\"{name}\""))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn open_or_create(
    manager: &ServiceManager,
    info: &ServiceInfo,
) -> Result<(windows_service::service::Service, bool)> {
    let rights = ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS;
    match manager.create_service(info, rights) {
        Ok(service) => Ok((service, true)),
        Err(e) if win_code(&e) == Some(1073) => {
            let service = manager
                .open_service(SERVICE_NAME, rights)
                .context("PJDesk Server is already installed, but the existing service could not be opened")?;
            if let Err(err) = service.change_config(info) {
                tracing::warn!("could not update the existing PJDesk Server service: {err}");
            }
            Ok((service, false))
        }
        Err(e) => Err(e).context("could not create the PJDesk Server service"),
    }
}

fn start_if_needed(service: &windows_service::service::Service) -> Result<()> {
    match service.start::<&str>(&[]) {
        Ok(()) => Ok(()),
        Err(e) if win_code(&e) == Some(1056) => Ok(()),
        Err(e) => {
            let running = service.query_status().ok().is_some_and(|status| {
                matches!(
                    status.current_state,
                    ServiceState::Running | ServiceState::StartPending | ServiceState::ContinuePending
                )
            });
            if running {
                Ok(())
            } else {
                Err(e).context("the PJDesk Server service is installed but did not start")
            }
        }
    }
}

fn win_code(err: &windows_service::Error) -> Option<i32> {
    match err {
        windows_service::Error::Winapi(io) => io.raw_os_error(),
        _ => None,
    }
}
