//! The background service: runs PJDesk from boot so this computer is reachable
//! before anyone signs in, during UAC prompts and at the lock screen.
//!
//! Windows runs a service in "session 0", which has no visible desktop, so the
//! service itself can't capture the screen. Instead it launches a helper
//! (`pjdesk __session-host`) into whichever session is on screen, as
//! LocalSystem, and relaunches it when the active session changes (sign-in,
//! sign-out, fast user switch). The helper follows the input desktop so it can
//! see the secure desktop (UAC / sign-in). This mirrors how RustDesk and
//! TeamViewer reach the sign-in screen.
//!
//! The service keeps its own config and identity under
//! `%ProgramData%\PJDesk\service`, locked to SYSTEM and Administrators.

use std::path::PathBuf;

use anyhow::Result;

pub const SERVICE_NAME: &str = "PJDesk";
#[cfg(windows)]
const DISPLAY_NAME: &str = "PJDesk";
#[cfg(windows)]
const DESCRIPTION: &str =
    "PJDesk remote desktop: lets you reach this computer at the sign-in screen and during UAC prompts.";
#[cfg(windows)]
const FIREWALL_RULE: &str = "PJDesk";

/// Seeds the service config (server + unattended password) and reports what it
/// used, so install and `configure` share one path.
pub fn write_service_config(server: Option<&str>, password: Option<&str>) -> Result<String> {
    use crate::config::Config;
    let path = crate::syspaths::service_config_path();
    let mut cfg = Config::load_from(&path).unwrap_or_default();

    // Server: explicit flag, else keep existing, else copy the installing user's.
    if let Some(s) = server {
        cfg.server = Some(s.to_string());
    } else if cfg.server.is_none() {
        if let Ok(user) = Config::load() {
            cfg.server = user.server;
            cfg.server_fingerprint = user.server_fingerprint;
        }
    }

    // Unattended password: explicit flag, else keep existing, else generate.
    let generated;
    let effective = match password {
        Some(p) if !p.is_empty() => {
            cfg.settings.set_permanent_password(Some(p));
            p.to_string()
        }
        _ => match cfg.settings.permanent_password() {
            Some(existing) => existing,
            None => {
                generated = pjdesk_proto::auth::generate_password();
                cfg.settings.set_permanent_password(Some(&generated));
                generated
            }
        },
    };
    cfg.save_to(&path)?;
    Ok(effective)
}

// ----------------------------------------------------------------------------
// Windows implementation
// ----------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        DuplicateTokenEx, SetTokenInformation, SecurityIdentification, TokenPrimary, TokenSessionId,
        TOKEN_ACCESS_MASK, TOKEN_ALL_ACCESS,
    };
    use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, TerminateProcess, CREATE_NO_WINDOW,
        CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
    };
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo,
        ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    use super::{DESCRIPTION, DISPLAY_NAME, FIREWALL_RULE, SERVICE_NAME};

    fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
        ServiceManager::local_computer(None::<&str>, access)
            .context("cannot open the Service Control Manager; run this from an Administrator prompt")
    }

    pub fn install(server: Option<&str>, password: Option<&str>, firewall: bool) -> Result<()> {
        let effective = super::write_service_config(server, password)?;
        super::protect_service_dir();

        let exe = std::env::current_exe()?;
        let manager = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
        let info = ServiceInfo {
            name: SERVICE_NAME.into(),
            display_name: DISPLAY_NAME.into(),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe.clone(),
            launch_arguments: vec!["service".into(), "run".into()],
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        };
        let service = manager
            .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
            .context("could not create the service (is it already installed?)")?;
        service.set_description(DESCRIPTION)?;
        if firewall {
            super::add_firewall_rule(&exe);
        }
        service.start::<&str>(&[]).context("service installed but failed to start")?;
        println!("Installed and started the PJDesk background service.");
        println!("Unattended password for this computer: {effective}");
        println!("Keep it safe — anyone with your ID and this password can control this computer.");
        Ok(())
    }

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
        super::remove_firewall_rule();
        println!("Removed the PJDesk background service.");
        Ok(())
    }

    pub fn start() -> Result<()> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service =
            manager.open_service(SERVICE_NAME, ServiceAccess::START).context("the service is not installed")?;
        service.start::<&str>(&[])?;
        println!("Started the PJDesk service.");
        Ok(())
    }

    pub fn stop() -> Result<()> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service =
            manager.open_service(SERVICE_NAME, ServiceAccess::STOP).context("the service is not installed")?;
        service.stop()?;
        println!("Stopped the PJDesk service.");
        Ok(())
    }

    /// Whether the service exists, and whether it is running right now.
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

    // --- SCM dispatcher + supervisor -------------------------------------

    define_windows_service!(ffi_service_main, service_main);

    static STARTED: OnceLock<()> = OnceLock::new();

    pub fn run_dispatcher() -> Result<()> {
        let _ = STARTED.set(());
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .context("this command is only run by Windows; use `pjdesk service start` instead")?;
        Ok(())
    }

    fn service_main(_args: Vec<OsString>) {
        super::init_service_logging();
        if let Err(e) = run_service() {
            tracing::error!("service failed: {e:#}");
        }
    }

    fn run_service() -> Result<()> {
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let handler = move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let status_handle = service_control_handler::register(SERVICE_NAME, handler)?;
        let set = |state: ServiceState, controls: ServiceControlAccept, wait: Duration| {
            status_handle.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: controls,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: wait,
                process_id: None,
            })
        };

        set(ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN, Duration::default())?;
        super::protect_service_dir();
        supervise(&stop_rx);
        set(ServiceState::StopPending, ServiceControlAccept::empty(), Duration::from_secs(3))?;
        set(ServiceState::Stopped, ServiceControlAccept::empty(), Duration::default())?;
        Ok(())
    }

    /// Keeps a session-host helper running in whichever session is on screen.
    fn supervise(stop_rx: &std::sync::mpsc::Receiver<()>) {
        let mut current_session = u32::MAX;
        let mut child: Option<Child> = None;

        loop {
            if stop_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
                break;
            }
            let active = unsafe { WTSGetActiveConsoleSessionId() };
            let child_dead = child.as_ref().map(|c| !c.is_alive()).unwrap_or(true);

            if active != u32::MAX && (active != current_session || child_dead) {
                if let Some(mut c) = child.take() {
                    c.kill();
                }
                match spawn_session_host(active) {
                    Ok(c) => {
                        tracing::info!("Launched session-host in session {active}");
                        child = Some(c);
                        current_session = active;
                    }
                    Err(e) => tracing::warn!("could not launch session-host in session {active}: {e:#}"),
                }
            }
        }
        if let Some(mut c) = child.take() {
            c.kill();
        }
        // Mark offline for the GUI.
        crate::syspaths::ServiceStatus::now(None, false, None).write();
    }

    /// A launched helper process we can poll and kill.
    struct Child {
        process: HANDLE,
        thread: HANDLE,
    }

    impl Child {
        fn is_alive(&self) -> bool {
            use windows::Win32::System::Threading::GetExitCodeProcess;
            const STILL_ACTIVE: u32 = 259;
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(self.process, &mut code).is_ok() && code == STILL_ACTIVE }
        }
        fn kill(&mut self) {
            unsafe {
                let _ = TerminateProcess(self.process, 1);
                let _ = CloseHandle(self.process);
                let _ = CloseHandle(self.thread);
            }
        }
    }

    /// Launches `pjdesk __session-host` as LocalSystem inside `session_id`.
    ///
    /// We duplicate the service's own (SYSTEM) token, move it to the target
    /// session, and start the helper on that session's default desktop. Running
    /// as SYSTEM is what lets the helper reach the secure desktop.
    fn spawn_session_host(session_id: u32) -> Result<Child> {
        unsafe {
            // Our own process token.
            let mut proc_token = HANDLE::default();
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ACCESS_MASK(TOKEN_ALL_ACCESS.0),
                &mut proc_token,
            )
            .context("OpenProcessToken")?;

            // A primary token we can hand to CreateProcessAsUser.
            let mut dup = HANDLE::default();
            let dup_result = DuplicateTokenEx(
                proc_token,
                TOKEN_ALL_ACCESS,
                None,
                SecurityIdentification,
                TokenPrimary,
                &mut dup,
            );
            let _ = CloseHandle(proc_token);
            dup_result.context("DuplicateTokenEx")?;

            // Move the token into the target session.
            let sid = session_id;
            SetTokenInformation(dup, TokenSessionId, &sid as *const u32 as *const core::ffi::c_void, 4)
                .context("SetTokenInformation(session)")?;

            // Environment block for the token.
            let mut env_block: *mut core::ffi::c_void = std::ptr::null_mut();
            let have_env = CreateEnvironmentBlock(&mut env_block, Some(dup), false).is_ok();

            // Command line and desktop.
            let exe = std::env::current_exe()?;
            let mut cmdline: Vec<u16> =
                format!("\"{}\" __session-host", exe.display()).encode_utf16().chain(std::iter::once(0)).collect();
            let mut desktop: Vec<u16> = "winsta0\\default".encode_utf16().chain(std::iter::once(0)).collect();

            // Pass the service config path to the helper via the config env var.
            let cfg_path = crate::syspaths::service_config_path();
            let prev = std::env::var_os("PJDESK_CONFIG");
            std::env::set_var("PJDESK_CONFIG", &cfg_path);

            let mut si = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                ..Default::default()
            };
            let mut pi = PROCESS_INFORMATION::default();

            let flags = CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW;
            let result = CreateProcessAsUserW(
                Some(dup),
                windows::core::PCWSTR::null(),
                Some(PWSTR(cmdline.as_mut_ptr())),
                None,
                None,
                false,
                flags,
                if have_env { Some(env_block) } else { None },
                windows::core::PCWSTR::null(),
                &si,
                &mut pi,
            );

            // Restore our env var.
            match prev {
                Some(v) => std::env::set_var("PJDESK_CONFIG", v),
                None => std::env::remove_var("PJDESK_CONFIG"),
            }
            if have_env {
                let _ = DestroyEnvironmentBlock(env_block);
            }
            let _ = CloseHandle(dup);
            let _ = &mut si;

            result.context("CreateProcessAsUser")?;
            Ok(Child { process: pi.hProcess, thread: pi.hThread })
        }
    }

    pub fn is_windows() -> bool {
        true
    }

    pub fn _unused(_: &Path, _: PathBuf) {}
}

#[cfg(windows)]
pub use imp::{install, is_windows, run_dispatcher, start, status, stop, uninstall};

/// The helper that actually hosts, launched by the service into a user session.
/// Follows the input desktop and publishes a status file for the GUI.
pub fn session_host() -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::config::Config;
    use crate::host::{self, HostShared};
    use crate::signaling::ServerAddr;
    use crate::syspaths::ServiceStatus;

    init_service_logging();
    let config = Config::load().unwrap_or_default();
    let server = match config.server.as_deref().and_then(|s| ServerAddr::parse(s).ok()) {
        Some(s) => s,
        None => {
            anyhow::bail!("the service has no server configured; run `pjdesk service install --server <address>`")
        }
    };

    let shared = Arc::new(HostShared::default());
    let mut args = crate::HostArgs::from_settings(&config.settings);
    args.follow_desktop = true;

    let host_shared = shared.clone();
    std::thread::Builder::new().name("host".into()).spawn(move || {
        if let Err(e) = host::run_with(server, config, args, Some(host_shared.clone())) {
            tracing::error!("session host stopped: {e:#}");
            *host_shared.last_error.lock().unwrap() = Some(format!("{e:#}"));
        }
    })?;

    // Publish status for the GUI until the process is killed.
    loop {
        ServiceStatus::now(shared.id(), shared.is_online(), shared.connected_peer()).write();
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Logs to a file in the service folder (the service has no console).
pub fn init_service_logging() {
    let dir = crate::syspaths::service_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("service.log")) {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("info,quinn=warn,rustls=warn"))
            .with_ansi(false)
            .with_target(false)
            .with_writer(std::sync::Mutex::new(file))
            .try_init();
    }
}

/// Restricts the service folder to SYSTEM and Administrators (it holds the
/// identity and the unattended password).
#[cfg(windows)]
pub fn protect_service_dir() {
    let dir = crate::syspaths::service_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::process::Command::new("icacls")
        .arg(&dir)
        .args(["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/T", "/Q"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    // The status file is read by the (non-admin) GUI, so allow users to read it.
    let _ = std::process::Command::new("icacls")
        .arg(crate::syspaths::service_status_path())
        .args(["/grant:r", "*S-1-5-32-545:(R)", "/Q"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(windows)]
fn add_firewall_rule(exe: &std::path::Path) {
    remove_firewall_rule();
    let _ = std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={FIREWALL_RULE}"),
            "dir=in",
            "action=allow",
            &format!("program={}", exe.display()),
            "enable=yes",
            "profile=any",
        ])
        .status();
}

#[cfg(windows)]
fn remove_firewall_rule() {
    let _ = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

// ----------------------------------------------------------------------------
// Non-Windows stubs (the service is Windows-only).
// ----------------------------------------------------------------------------

#[cfg(not(windows))]
pub fn install(_server: Option<&str>, _password: Option<&str>, _firewall: bool) -> Result<()> {
    anyhow::bail!("the background service is only available on Windows")
}
#[cfg(not(windows))]
pub fn uninstall() -> Result<()> {
    anyhow::bail!("the background service is only available on Windows")
}
#[cfg(not(windows))]
pub fn start() -> Result<()> {
    anyhow::bail!("the background service is only available on Windows")
}
#[cfg(not(windows))]
pub fn stop() -> Result<()> {
    anyhow::bail!("the background service is only available on Windows")
}
#[cfg(not(windows))]
pub fn run_dispatcher() -> Result<()> {
    anyhow::bail!("the background service is only available on Windows")
}
#[cfg(not(windows))]
pub fn status() -> (bool, bool) {
    (false, false)
}
#[cfg(not(windows))]
pub fn protect_service_dir() {}
#[cfg(not(windows))]
#[allow(dead_code)]
fn _paths() -> PathBuf {
    PathBuf::new()
}
