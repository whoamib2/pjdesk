//! After a silent update: start the background service again, then the
//! signed-in user's PJDesk window.
//!
//! The installer stops the service and kills every `pjdesk.exe` so it can
//! replace the file. `net start` on its own is one attempt, and it does
//! nothing when the service was never installed — the window was the host.
//! This command retries the service and starts the window in the interactive
//! session, as that user, so the computer registers again.

use anyhow::Result;

use pjdesk_proto::handoff::{self, ServiceOutcome};

/// After an install or update PJDesk comes back silently: the tray icon only,
/// with no main window, PIN screen or notice. The service hosts; the window
/// (and any "Updated to X" note) appears only when the user opens it.
pub fn relaunch_args() -> &'static [&'static str] {
    &["--minimized"]
}

pub fn run() -> Result<()> {
    pjdesk_proto::update::record_handoff("client handoff start");
    let mut outcome = ensure_service();
    let mut launched = relaunch_users();
    if matches!(outcome, ServiceOutcome::Running { .. }) {
        std::thread::sleep(handoff::HEALTH_RECHECK);
        let (installed, running) = service_status();
        if installed && !running {
            pjdesk_proto::update::record_handoff("client health check: service stopped, starting again");
            let _ = crate::service::start();
            std::thread::sleep(handoff::start_retry_delay(1));
            let (installed, running) = service_status();
            outcome = handoff::service_outcome(installed, running, handoff::SERVICE_START_ATTEMPTS);
            if launched == 0 {
                launched = relaunch_users();
            }
        }
    }
    let back = handoff::computer_is_back(outcome, launched);
    let line = format!("client handoff done service={outcome:?} user_processes={launched} back={back}");
    pjdesk_proto::update::record_handoff(&line);
    println!("{line}");
    if back {
        Ok(())
    } else {
        anyhow::bail!("PJDesk is not running after the update")
    }
}

fn ensure_service() -> ServiceOutcome {
    for attempt in 1..=handoff::SERVICE_START_ATTEMPTS {
        let (installed, running) = service_status();
        if !installed {
            pjdesk_proto::update::record_handoff("client service is not installed");
            return ServiceOutcome::NotInstalled;
        }
        if running {
            pjdesk_proto::update::record_handoff(&format!("client service is running (attempt {attempt})"));
            return ServiceOutcome::Running { attempts: attempt };
        }
        pjdesk_proto::update::record_handoff(&format!("client service start attempt {attempt}"));
        if let Err(e) = crate::service::start() {
            pjdesk_proto::update::record_handoff(&format!("client service start failed: {e:#}"));
        }
        std::thread::sleep(handoff::start_retry_delay(attempt));
    }
    let (installed, running) = service_status();
    let outcome = handoff::service_outcome(installed, running, handoff::SERVICE_START_ATTEMPTS);
    pjdesk_proto::update::record_handoff(&format!("client service result {outcome:?}"));
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
        CreateProcessAsUserW, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTF_USESHOWWINDOW,
        STARTUPINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let Ok(exe) = std::env::current_exe() else {
        pjdesk_proto::update::record_handoff("client relaunch: cannot find pjdesk.exe");
        return 0;
    };
    let mut info: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    let listed = unsafe { WTSEnumerateSessionsW(None, 0, 1, &mut info, &mut count).is_ok() };
    if !listed || info.is_null() {
        pjdesk_proto::update::record_handoff("client relaunch: no interactive sessions");
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
                    "client relaunch: session {} has no user token",
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
                pjdesk_proto::update::record_handoff(&format!(
                    "client relaunch: session {} has no environment",
                    session.SessionId
                ));
                let _ = CloseHandle(token);
                continue;
            }
            // Do not inherit the service's std handles. They are not valid in
            // the user session, and the window then cannot start a viewer
            // (os error 6). CREATE_NO_WINDOW keeps a console-subsystem build
            // from attaching a console the service does not have.
            let mut si = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                dwFlags: STARTF_USESHOWWINDOW,
                wShowWindow: SW_HIDE.0 as u16,
                ..Default::default()
            };
            let mut pi = PROCESS_INFORMATION::default();
            let created = CreateProcessAsUserW(
                Some(token),
                PCWSTR::null(),
                Some(PWSTR(cmdline.as_mut_ptr())),
                None,
                None,
                relaunch_inherits_handles(),
                CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
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
                    pjdesk_proto::update::record_handoff(&format!("client relaunch: session {} started", session.SessionId));
                    let _ = CloseHandle(pi.hProcess);
                    let _ = CloseHandle(pi.hThread);
                }
                Err(e) => {
                    pjdesk_proto::update::record_handoff(&format!(
                        "client relaunch: session {} failed: {e}",
                        session.SessionId
                    ));
                }
            }
        }
        WTSFreeMemory(info.cast());
    }
    launched
}

#[cfg(not(windows))]
fn relaunch_users() -> u32 {
    pjdesk_proto::update::record_handoff("client relaunch: not Windows");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_process_comes_back_minimized_to_the_tray() {
        assert_eq!(relaunch_args(), &["--minimized"]);
    }

    #[test]
    fn the_service_does_not_hand_its_handles_to_the_window() {
        assert!(!relaunch_inherits_handles());
    }
}

/// `CreateProcessAsUser` from the service must not inherit handles. The
/// service has no console, and those handles are invalid in the user session.
pub(crate) fn relaunch_inherits_handles() -> bool {
    false
}
