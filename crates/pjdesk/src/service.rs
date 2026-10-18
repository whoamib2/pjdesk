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

use anyhow::{Context, Result};

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

    // Server: explicit flag, else keep existing, else copy the installing user's,
    // else the baked-in default — so the service can always register, even when
    // it's installed as SYSTEM and the user hasn't signed in yet.
    if let Some(s) = server {
        cfg.server = Some(s.to_string());
    } else if cfg.server.is_none() {
        if let Ok(user) = Config::load() {
            cfg.server = user.server;
            cfg.server_fingerprint = user.server_fingerprint;
        }
    }
    if cfg.server.as_deref().map(str::trim).unwrap_or("").is_empty() {
        cfg.server = Some(pjdesk_proto::DEFAULT_SERVER.to_string());
    }

    // Unattended password: set a *permanent* one only when explicitly provided
    // (or keep an existing one). Otherwise the host uses a rotating one-time
    // password shown in the PJDesk window, so letting someone in needs no
    // permanent password at all. An empty return means "using a one-time password".
    let effective = match password {
        Some(p) if !p.is_empty() => {
            // Machine scope so the sign-in-screen helper (SYSTEM) can read it.
            // The file's ACL, not DPAPI alone, keeps other users out.
            cfg.settings.set_permanent_password_machine(Some(p));
            p.to_string()
        }
        _ => cfg.settings.permanent_password().unwrap_or_default(),
    };
    cfg.save_to(&path)?;
    Ok(effective)
}

/// Clears the service's permanent password, so it falls back to a rotating
/// one-time password shown in the app.
pub fn clear_service_password() -> Result<()> {
    use crate::config::Config;
    let path = crate::syspaths::service_config_path();
    let mut cfg = Config::load_from(&path).unwrap_or_default();
    cfg.settings.set_permanent_password(None);
    cfg.save_to(&path)?;
    Ok(())
}

// ----------------------------------------------------------------------------
// Windows implementation
// ----------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::sync::OnceLock;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        DuplicateTokenEx, SecurityIdentification, SetTokenInformation, TokenPrimary, TokenSessionId, TOKEN_ACCESS_MASK,
        TOKEN_ALL_ACCESS,
    };
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, TerminateProcess, CREATE_NO_WINDOW,
        CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
    };
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceInfo,
        ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    use super::{DESCRIPTION, DISPLAY_NAME, SERVICE_NAME};

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
        if effective.is_empty() {
            println!("This computer uses a one-time password, shown in the PJDesk window.");
            println!("Open PJDesk to see its ID and the current password to share.");
        } else {
            println!("An unattended password is set. It was not printed here.");
            println!("Anyone with your ID and that password can control this computer.");
        }
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

    /// Stops (waiting for it to settle) and starts the service, so a changed
    /// config is picked up. A no-op if the service isn't installed.
    pub fn restart() -> Result<()> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let Ok(service) = manager
            .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START)
        else {
            return Ok(());
        };
        if service.query_status().map(|s| s.current_state != ServiceState::Stopped).unwrap_or(false) {
            let _ = service.stop();
            for _ in 0..50 {
                if service.query_status().map(|s| s.current_state == ServiceState::Stopped).unwrap_or(true) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        service.start::<&str>(&[])?;
        println!("Restarted the PJDesk service.");
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
        use std::time::Instant;
        let mut current_session = u32::MAX;
        let mut child: Option<Child> = None;
        let mut spawned_at = Instant::now();
        let mut quick_failures: u32 = 0;

        loop {
            if stop_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
                break;
            }
            let active = unsafe { WTSGetActiveConsoleSessionId() };
            if active == u32::MAX {
                continue; // No console session right now (fast user switching, etc.).
            }
            let session_changed = active != current_session;
            let child_dead = child.as_ref().map(|c| !c.is_alive()).unwrap_or(true);
            if !session_changed && !child_dead {
                continue; // Healthy and on the same session — nothing to do.
            }

            // If the helper keeps dying almost immediately, slow the relaunch
            // rate (up to ~30 s) so a persistent failure can't spin once a
            // second, and record the exit code so the cause is visible.
            if child_dead && !session_changed {
                if let Some(c) = &child {
                    tracing::warn!("session-host (session {current_session}) exited with code {}", c.exit_code());
                }
                if spawned_at.elapsed() < Duration::from_secs(8) {
                    quick_failures = (quick_failures + 1).min(10);
                    let wait = Duration::from_secs(u64::from(quick_failures) * 3);
                    if spawned_at.elapsed() < wait {
                        continue;
                    }
                } else {
                    quick_failures = 0;
                }
            }

            if let Some(mut c) = child.take() {
                c.kill();
            }
            // The helper runs as the signed-in user and must read the service
            // config. Grant that user, and nobody else: a blanket Users
            // Modify let every account read the one-time password and rewrite
            // which server this computer trusts.
            super::secure_service_dir(Some(active));
            match spawn_session_host(active) {
                Ok(c) => {
                    tracing::info!("Launched session-host in session {active}");
                    child = Some(c);
                    current_session = active;
                    spawned_at = Instant::now();
                }
                Err(e) => tracing::warn!("could not launch session-host in session {active}: {e:#}"),
            }
        }
        if let Some(mut c) = child.take() {
            c.kill();
        }
        // Mark offline for the GUI.
        crate::syspaths::ServiceStatus::now(None, false, None, None, false).write();
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
        fn exit_code(&self) -> u32 {
            use windows::Win32::System::Threading::GetExitCodeProcess;
            let mut code = 0u32;
            unsafe {
                let _ = GetExitCodeProcess(self.process, &mut code);
            }
            code
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
            // Prefer the interactive user's own token for this session: the
            // helper then runs exactly like the user's own app — stable, and
            // able to read secrets DPAPI-encrypted for that user — instead of
            // as LocalSystem. Only when nobody is signed in (the lock / sign-in
            // screen) do we fall back to a SYSTEM token so we stay reachable.
            let mut token = HANDLE::default();
            let as_user = WTSQueryUserToken(session_id, &mut token).is_ok();
            if !as_user {
                let mut proc_token = HANDLE::default();
                OpenProcessToken(GetCurrentProcess(), TOKEN_ACCESS_MASK(TOKEN_ALL_ACCESS.0), &mut proc_token)
                    .context("OpenProcessToken")?;
                let dup_result = DuplicateTokenEx(
                    proc_token,
                    TOKEN_ALL_ACCESS,
                    None,
                    SecurityIdentification,
                    TokenPrimary,
                    &mut token,
                );
                let _ = CloseHandle(proc_token);
                dup_result.context("DuplicateTokenEx")?;
                // Move the SYSTEM token into the target session.
                let sid = session_id;
                if let Err(e) =
                    SetTokenInformation(token, TokenSessionId, &sid as *const u32 as *const core::ffi::c_void, 4)
                {
                    let _ = CloseHandle(token);
                    return Err(e).context("SetTokenInformation(session)");
                }
            }

            // Build the command line and desktop. The helper gets its config
            // path on the command line — the token's environment block does not
            // carry our PJDESK_CONFIG, so passing it that way would silently fail.
            let exe = match std::env::current_exe() {
                Ok(exe) => exe,
                Err(e) => {
                    let _ = CloseHandle(token);
                    return Err(e.into());
                }
            };
            let cfg_path = crate::syspaths::service_config_path();
            let mut cmdline: Vec<u16> =
                format!("\"{}\" __session-host --config \"{}\"", exe.display(), cfg_path.display())
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
            let mut desktop: Vec<u16> = "winsta0\\default".encode_utf16().chain(std::iter::once(0)).collect();

            // Environment block for the chosen token.
            let mut env_block: *mut core::ffi::c_void = std::ptr::null_mut();
            let have_env = CreateEnvironmentBlock(&mut env_block, Some(token), false).is_ok();

            let mut si = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                ..Default::default()
            };
            let mut pi = PROCESS_INFORMATION::default();

            let flags = CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW;
            let result = CreateProcessAsUserW(
                Some(token),
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

            if have_env {
                let _ = DestroyEnvironmentBlock(env_block);
            }
            let _ = CloseHandle(token);
            let _ = &mut si;

            result.context("CreateProcessAsUser")?;
            Ok(Child { process: pi.hProcess, thread: pi.hThread })
        }
    }

}

#[cfg(windows)]
pub use imp::{install, restart, run_dispatcher, start, status, stop, uninstall};

/// The helper that actually hosts, launched by the service into a user session.
/// Follows the input desktop and publishes a status file for the GUI.
///
/// `config_path` is the service config the launcher passes on the command line;
/// if absent (e.g. run by hand) it falls back to the default service config.
pub fn session_host(config_path: Option<String>) -> Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::config::Config;
    use crate::host::{self, HostShared};
    use crate::signaling::ServerAddr;
    use crate::syspaths::ServiceStatus;

    // Point every config read/write at the service config, so the host thread
    // also saves its identity there.
    let path = config_path.map(std::path::PathBuf::from).unwrap_or_else(crate::syspaths::service_config_path);
    std::env::set_var("PJDESK_CONFIG", &path);

    init_service_logging();
    let mut config = Config::load().unwrap_or_default();
    // 0.3.6 sealed the unattended password to the user who set it, so SYSTEM
    // could not read it at the sign-in screen. Re-seal it for the machine the
    // first time a user who can still open the old blob runs the helper.
    if config.settings.permanent_password.as_deref().is_some_and(|s| s.starts_with("dpapi:")) {
        if let Some(pw) = config.settings.permanent_password() {
            config.settings.set_permanent_password_machine(Some(&pw));
            let _ = config.save();
        }
    }
    // Falls back to the baked-in default server, so the service registers and
    // gets an ID even if its config was never seeded with one.
    let server = ServerAddr::parse(&config.effective_server())
        .map_err(|e| anyhow::anyhow!("invalid server address \"{}\": {e}", config.effective_server()))?;

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

    // Publish status for the GUI until the process is killed. The one-time
    // password is always shown; a permanent password (if set) works too but is
    // never written here.
    loop {
        let password = Some(shared.password()).filter(|p| !p.is_empty());
        ServiceStatus::now(shared.id(), shared.is_online(), shared.connected_peer(), password, shared.permanent_set())
            .write();
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Name of the daily auto-update scheduled task (matches the installer).
pub const UPDATE_TASK: &str = "PJDesk Update";

/// Whether the daily auto-update task is registered.
#[cfg(windows)]
pub fn update_task_enabled() -> bool {
    std::process::Command::new("schtasks")
        .args(["/Query", "/TN", UPDATE_TASK])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Creates or removes the daily auto-update task (needs Administrator rights).
#[cfg(windows)]
pub fn set_update_task(enable: bool) -> Result<()> {
    if !enable {
        let _ = std::process::Command::new("schtasks").args(["/Delete", "/TN", UPDATE_TASK, "/F"]).status();
        println!("Automatic updates turned off.");
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    // A short (8.3) path has no spaces, so the task's /TR needs no inner quotes.
    let tr = format!("{} update", short_path(&exe).unwrap_or_else(|| exe.display().to_string()));
    let status = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            UPDATE_TASK,
            "/TR",
            &tr,
            "/SC",
            "MINUTE",
            "/MO",
            "30",
            "/RU",
            "SYSTEM",
            "/RL",
            "HIGHEST",
            "/F",
        ])
        .status()
        .context("running schtasks")?;
    if !status.success() {
        anyhow::bail!("schtasks could not create the update task (run as Administrator)");
    }
    println!("Automatic updates turned on (checks every 30 minutes).");
    Ok(())
}

#[cfg(windows)]
fn short_path(path: &std::path::Path) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide = HSTRING::from(path.as_os_str());
    unsafe {
        let len = GetShortPathNameW(PCWSTR(wide.as_ptr()), None);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        let written = GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buf));
        if written == 0 || written as usize >= buf.len() + 1 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..written as usize]))
    }
}

#[cfg(not(windows))]
pub fn update_task_enabled() -> bool {
    false
}
#[cfg(not(windows))]
pub fn set_update_task(_enable: bool) -> Result<()> {
    anyhow::bail!("scheduled updates are only available on Windows")
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

/// Restricts the service folder to SYSTEM and Administrators, then (when a
/// console session is given) the user who is signed in there.
///
/// The one-time password is published in `status.json` so the PJDesk window
/// can show it. That file used to be writable and readable by every local
/// user, which handed the password — and the ability to retarget the server
/// — to any account on the machine. The signed-in user still needs access,
/// because the session helper runs as them and the window runs as them.
#[cfg(windows)]
pub fn protect_service_dir() {
    secure_service_dir(None);
}

#[cfg(windows)]
pub fn secure_service_dir(session_id: Option<u32>) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let dir = crate::syspaths::service_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::process::Command::new("takeown")
        .arg("/f")
        .arg(&dir)
        .args(["/r", "/d", "y"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    // Directory only. (OI)(CI) on a file is an invalid ACE and is what left
    // empty DACLs when an earlier version passed /T.
    let mut grant = String::from("*S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F");
    if let Some(session_id) = session_id {
        if let Some(sid) = session_user_sid(session_id) {
            grant.push_str(&format!(" *{sid}:(OI)(CI)M"));
        }
    }
    let _ = std::process::Command::new("icacls")
        .arg(&dir)
        .args(["/inheritance:r", "/grant:r"])
        .args(grant.split_whitespace())
        .arg("/Q")
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = std::process::Command::new("icacls")
        .arg(format!("{}\\*", dir.display()))
        .args(["/reset", "/T", "/C", "/Q"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// SID of the user logged into `session_id`, as `S-1-...`.
#[cfg(windows)]
fn session_user_sid(session_id: u32) -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_USER};
    use windows::Win32::System::RemoteDesktop::WTSQueryUserToken;

    unsafe {
        let mut token = HANDLE::default();
        if WTSQueryUserToken(session_id, &mut token).is_err() {
            return None;
        }
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        if needed == 0 {
            let _ = CloseHandle(token);
            return None;
        }
        // The SID bytes follow the TOKEN_USER header in this buffer, so it
        // has to be one allocation and pointer-aligned.
        let mut buf = vec![0u64; needed.div_ceil(8) as usize];
        let ok = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), needed, &mut needed);
        let _ = CloseHandle(token);
        if ok.is_err() {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut wide = windows::core::PWSTR::null();
        if ConvertSidToStringSidW(user.User.Sid, &mut wide).is_err() || wide.is_null() {
            return None;
        }
        let sid = wide.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(wide.as_ptr() as *mut _)));
        sid.filter(|s| s.starts_with("S-1-") && s.len() < 200 && s.bytes().all(|b| b.is_ascii_graphic()))
    }
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
pub fn restart() -> Result<()> {
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
