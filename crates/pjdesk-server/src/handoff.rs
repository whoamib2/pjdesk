//! After a silent update: start the PJDesk Server service again, then the
//! signed-in user's tray.
//!
//! The installer used to `Exec` the tray from its own process. An update runs
//! as SYSTEM, so that tray came up in session 0 and the user's window, started
//! later from the Run key, could not read the data folder. This command starts
//! the service (with retries) and the tray inside the interactive session, on
//! that user's token.

use anyhow::Result;

use pjdesk_proto::handoff::{self, ServiceOutcome};

/// `--tray` keeps the icon and does not require the window to be elevated.
pub fn relaunch_args() -> &'static [&'static str] {
    &["--tray"]
}

pub fn run() -> Result<()> {
    pjdesk_proto::update::record_handoff("server handoff start");
    let mut outcome = ensure_service();
    let mut launched = relaunch_users();
    if matches!(outcome, ServiceOutcome::Running { .. }) {
        std::thread::sleep(handoff::HEALTH_RECHECK);
        let (installed, running) = service_status();
        if installed && !running {
            pjdesk_proto::update::record_handoff("server health check: service stopped, starting again");
            let _ = crate::service::start();
            std::thread::sleep(handoff::start_retry_delay(1));
            let (installed, running) = service_status();
            outcome = handoff::service_outcome(installed, running, handoff::SERVICE_START_ATTEMPTS);
            if launched == 0 {
                launched = relaunch_users();
            }
        }
    }
    // The tray does not listen. The computer is back only when the service is.
    let back = matches!(outcome, ServiceOutcome::Running { .. });
    let line = format!("server handoff done service={outcome:?} user_processes={launched} back={back}");
    pjdesk_proto::update::record_handoff(&line);
    println!("{line}");
    if back {
        Ok(())
    } else {
        anyhow::bail!("PJDesk Server is not running after the update")
    }
}

fn ensure_service() -> ServiceOutcome {
    for attempt in 1..=handoff::SERVICE_START_ATTEMPTS {
        let (installed, running) = service_status();
        if !installed {
            pjdesk_proto::update::record_handoff("server service is not installed");
            return ServiceOutcome::NotInstalled;
        }
        if running {
            pjdesk_proto::update::record_handoff(&format!("server service is running (attempt {attempt})"));
            return ServiceOutcome::Running { attempts: attempt };
        }
        pjdesk_proto::update::record_handoff(&format!("server service start attempt {attempt}"));
        if let Err(e) = crate::service::start() {
            pjdesk_proto::update::record_handoff(&format!("server service start failed: {e:#}"));
        }
        std::thread::sleep(handoff::start_retry_delay(attempt));
    }
    let (installed, running) = service_status();
    let outcome = handoff::service_outcome(installed, running, handoff::SERVICE_START_ATTEMPTS);
    pjdesk_proto::update::record_handoff(&format!("server service result {outcome:?}"));
    outcome
}

#[cfg(windows)]
fn service_status() -> (bool, bool) {
    crate::service::status()
}

#[cfg(not(windows))]
fn service_status() -> (bool, bool) {
    (false, false)
}

#[cfg(windows)]
fn relaunch_users() -> u32 {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows::Win32::System::RemoteDesktop::{
        WTSEnumerateSessionsW, WTSFreeMemory, WTSQueryUserToken, WTSActive, WTS_SESSION_INFOW,
    };
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let Ok(exe) = std::env::current_exe() else {
        pjdesk_proto::update::record_handoff("server relaunch: cannot find pjdesk-server.exe");
        return 0;
    };
    let mut info: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    let listed = unsafe { WTSEnumerateSessionsW(None, 0, 1, &mut info, &mut count).is_ok() };
    if !listed || info.is_null() {
        pjdesk_proto::update::record_handoff("server relaunch: no interactive sessions");
        return 0;
    }
    let mut launched = 0u32;
    unsafe {
        for i in 0..count {
            let session = &*info.add(i as usize);
            if session.State != WTSActive || session.SessionId == 0 {
                continue;
            }
            let mut token = windows::Win32::Foundation::HANDLE::default();
            if WTSQueryUserToken(session.SessionId, &mut token).is_err() {
                pjdesk_proto::update::record_handoff(&format!(
                    "server relaunch: session {} has no user token",
                    session.SessionId
                ));
                continue;
            }
            let extra = relaunch_args().join(" ");
            let line = if extra.is_empty() {
                format!("\"{}\"", exe.display())
            } else {
                format!("\"{}\" {extra}", exe.display())
            };
            let mut cmdline: Vec<u16> = line.encode_utf16().chain(std::iter::once(0)).collect();
            let mut desktop: Vec<u16> = "winsta0\\default".encode_utf16().chain(std::iter::once(0)).collect();
            let mut env = std::ptr::null_mut();
            let have_env = CreateEnvironmentBlock(&mut env, Some(token), false).is_ok();
            if !have_env {
                let _ = CloseHandle(token);
                pjdesk_proto::update::record_handoff(&format!(
                    "server relaunch: session {} has no environment",
                    session.SessionId
                ));
                continue;
            }
            let mut si = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                ..Default::default()
            };
            let mut pi = PROCESS_INFORMATION::default();
            let created = CreateProcessAsUserW(
                Some(token),
                PCWSTR::null(),
                Some(PWSTR(cmdline.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_UNICODE_ENVIRONMENT,
                Some(env),
                PCWSTR::null(),
                &si,
                &mut pi,
            );
            let _ = DestroyEnvironmentBlock(env);
            let _ = CloseHandle(token);
            let _ = &mut si;
            match created {
                Ok(()) => {
                    launched += 1;
                    pjdesk_proto::update::record_handoff(&format!("server relaunch: session {} tray started", session.SessionId));
                    let _ = CloseHandle(pi.hProcess);
                    let _ = CloseHandle(pi.hThread);
                }
                Err(e) => pjdesk_proto::update::record_handoff(&format!(
                    "server relaunch: session {} failed: {e}",
                    session.SessionId
                )),
            }
        }
        WTSFreeMemory(info.cast());
    }
    launched
}

#[cfg(not(windows))]
fn relaunch_users() -> u32 {
    pjdesk_proto::update::record_handoff("server relaunch: not Windows");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_process_is_the_tray() {
        assert_eq!(relaunch_args(), &["--tray"]);
    }

    #[test]
    fn installers_restart_after_the_file_swap_and_keep_server_data() {
        let server = include_str!("../../../installer/pjdesk-server.nsi");
        let client = include_str!("../../../installer/pjdesk-client.nsi");
        for (name, src) in [("server", server), ("client", client)] {
            let kill = src.find("taskkill").unwrap_or_else(|| panic!("{name} never stops the old process"));
            let handoff = src.rfind("update-handoff").unwrap_or_else(|| panic!("{name} never restarts"));
            assert!(kill < handoff, "{name} kills the process after the restart");
            assert!(src.contains("net start"), "{name} has no service start fallback");
            for line in src.lines() {
                let lower = line.to_ascii_lowercase();
                let deletes = lower.contains("rmdir") || lower.trim_start().to_ascii_lowercase().starts_with("delete ");
                if deletes {
                    assert!(!lower.contains("programdata"), "{name} removes ProgramData: {line}");
                    for file in crate::status_view::PRESERVED {
                        assert!(!line.contains(file), "{name} deletes {file}");
                    }
                }
            }
        }
        assert!(
            !server.contains("Exec '\"$INSTDIR\\${EXE}\" --tray'"),
            "the installer must not start the tray as itself"
        );
        assert!(!client.to_ascii_lowercase().contains("appdata"), "the client installer touches the saved sign-in");
    }
}
