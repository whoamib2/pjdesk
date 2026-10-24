//! The unelevated window asks the PJDesk service to install an update.
//!
//! The service listens on `\\.\pipe\PJDeskClientUpdate`. It runs as SYSTEM, so
//! it can write `C:\ProgramData\PJDesk\updates` without the window touching
//! that folder's permissions. The pipe is local. Remote clients are rejected.
//! One JSON request, then progress lines, then one finished line.

use serde::{Deserialize, Serialize};

pub const PIPE_NAME: &str = r"\\.\pipe\PJDeskClientUpdate";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateReply {
    pub ok: bool,
    pub message: String,
}

#[derive(Deserialize)]
struct Incoming {
    cmd: String,
}

#[derive(Serialize)]
struct Outgoing<'a> {
    progress: &'a str,
    done: bool,
    ok: bool,
    message: &'a str,
}

#[derive(Debug)]
enum Step {
    Progress(String),
    Finished(UpdateReply),
    Ignore,
}

fn classify(line: &str) -> Step {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        progress: String,
        #[serde(default)]
        done: bool,
        #[serde(default)]
        ok: bool,
        #[serde(default)]
        message: String,
    }
    let Ok(row) = serde_json::from_str::<Row>(line.trim()) else {
        return Step::Ignore;
    };
    if row.done {
        Step::Finished(UpdateReply { ok: row.ok, message: row.message })
    } else if !row.progress.is_empty() {
        Step::Progress(row.progress)
    } else {
        Step::Ignore
    }
}

/// The service closed the pipe after it had started the installer.
fn finish_from_disconnect(last_progress: &str) -> UpdateReply {
    if last_progress.starts_with("Installing") {
        UpdateReply { ok: true, message: last_progress.to_string() }
    } else if last_progress.is_empty() {
        UpdateReply {
            ok: false,
            message: "The PJDesk service closed the update before it finished.".into(),
        }
    } else {
        UpdateReply {
            ok: false,
            message: format!("The PJDesk service stopped during the update ({last_progress})."),
        }
    }
}

#[cfg(windows)]
pub fn serve() {
    loop {
        if !serve_once() {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
}

#[cfg(not(windows))]
pub fn serve() {}

#[cfg(windows)]
fn serve_once() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::core::w;
    use windows::Win32::Foundation::{LocalFree, HLOCAL, INVALID_HANDLE_VALUE};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, ImpersonateNamedPipeClient, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows::Win32::Security::RevertToSelf;

    static BUSY: AtomicBool = AtomicBool::new(false);

    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SYSTEM and Administrators full. Interactive users can connect.
        let sddl = w!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;IU)");
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, SDDL_REVISION_1, &mut sd, None).is_err() {
            return false;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let pipe_name = windows::core::HSTRING::from(PIPE_NAME);
        let pipe = CreateNamedPipeW(
            &pipe_name,
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            4,
            64 * 1024,
            4096,
            0,
            Some(&sa),
        );
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        if pipe == INVALID_HANDLE_VALUE {
            return false;
        }
        let connected = match ConnectNamedPipe(pipe, None) {
            Ok(()) => true,
            Err(e) => (e.code().0 as u32 & 0xFFFF) == 535,
        };
        if !connected {
            let _ = windows::Win32::Foundation::CloseHandle(pipe);
            return false;
        }
        // Read first. Impersonating before any bytes arrive fails for a normal
        // window (often Win32 1368) and used to reject the request with no log.
        let mut buf = vec![0u8; 4096];
        let mut n = 0u32;
        let read = ReadFile(pipe, Some(&mut buf), Some(&mut n), None);
        let caller = match ImpersonateNamedPipeClient(pipe) {
            Ok(()) => {
                let _ = RevertToSelf();
                "local user".to_string()
            }
            Err(e) => {
                let _ = RevertToSelf();
                let note = format!("could not impersonate the caller ({e})");
                tracing::warn!("{note}");
                note
            }
        };
        let write_line = |body: Outgoing| {
            if let Ok(mut bytes) = serde_json::to_vec(&body) {
                bytes.push(b'\n');
                let _ = WriteFile(pipe, Some(&bytes), None, None);
            }
        };
        let request = if let Err(e) = read {
            let reason = format!("Could not read the update request ({e}).");
            tracing::warn!("{reason} caller={caller}");
            Err(reason)
        } else if n == 0 {
            let reason = "The update request was empty.".to_string();
            tracing::warn!("{reason} caller={caller}");
            Err(reason)
        } else if n as usize >= buf.len() {
            let reason = "The update request was too large.".to_string();
            tracing::warn!("{reason} caller={caller}");
            Err(reason)
        } else {
            match serde_json::from_slice::<Incoming>(&buf[..n as usize]) {
                Ok(req) => {
                    tracing::info!("update request {:?} from {caller}", req.cmd);
                    Ok(req)
                }
                Err(e) => {
                    let reason = format!("The update request was not understood ({e}).");
                    tracing::warn!("{reason} caller={caller}");
                    Err(reason)
                }
            }
        };
        let cmd = request.as_ref().map(|r| r.cmd.trim().to_string()).unwrap_or_default();
        if let Err(reason) = request {
            write_line(Outgoing { progress: "", done: true, ok: false, message: &reason });
        } else if cmd == "ping" {
            write_line(Outgoing { progress: "", done: true, ok: true, message: "ready" });
        } else if cmd == "update" {
            if BUSY.swap(true, Ordering::SeqCst) {
                write_line(Outgoing {
                    progress: "",
                    done: true,
                    ok: false,
                    message: "An update is already running.",
                });
            } else {
                let last_line = std::sync::Mutex::new(String::new());
                let outcome = pjdesk_proto::update::install_client_as_service(env!("CARGO_PKG_VERSION"), &|line| {
                    tracing::info!("{line}");
                    *last_line.lock().unwrap() = line.to_string();
                    write_line(Outgoing { progress: line, done: false, ok: false, message: "" });
                });
                BUSY.store(false, Ordering::SeqCst);
                match outcome {
                    Ok(pjdesk_proto::update::Outcome::UpToDate(version)) => {
                        let saved = last_line.lock().unwrap().clone();
                        let message = if saved.contains("GitHub latest") {
                            saved
                        } else {
                            format!("PJDesk {version} is up to date.")
                        };
                        write_line(Outgoing {
                            progress: "",
                            done: true,
                            ok: true,
                            message: &message,
                        });
                    }
                    Ok(pjdesk_proto::update::Outcome::Installing(version)) => write_line(Outgoing {
                        progress: "",
                        done: true,
                        ok: true,
                        message: &format!("Installing PJDesk {version}. PJDesk will close and open again."),
                    }),
                    Ok(pjdesk_proto::update::Outcome::Found(version)) => write_line(Outgoing {
                        progress: "",
                        done: true,
                        ok: true,
                        message: &format!("PJDesk {version} is available."),
                    }),
                    Err(e) => {
                        tracing::warn!("update failed: {e:#}");
                        write_line(Outgoing {
                            progress: "",
                            done: true,
                            ok: false,
                            message: &format!("{e:#}"),
                        });
                    }
                }
            }
        } else {
            let reason = format!("That command was not understood ({cmd}).");
            tracing::warn!("{reason} caller={caller}");
            write_line(Outgoing { progress: "", done: true, ok: false, message: &reason });
        }
        let _ = DisconnectNamedPipe(pipe);
        let _ = windows::Win32::Foundation::CloseHandle(pipe);
        true
    }
}

/// Asks the service to check, download, verify, and install. `None` when the
/// service is not listening.
#[cfg(windows)]
pub fn request_update(progress: &(dyn Fn(&str) + Sync)) -> Option<UpdateReply> {
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        SECURITY_IMPERSONATION, SECURITY_SQOS_PRESENT,
    };
    use windows::Win32::System::Pipes::WaitNamedPipeW;

    let bytes = serde_json::to_vec(&serde_json::json!({"cmd": "update"})).ok()?;
    unsafe {
        let pipe_name = windows::core::HSTRING::from(PIPE_NAME);
        if !WaitNamedPipeW(&pipe_name, 1500).as_bool() {
            return None;
        }
        let handle = CreateFileW(
            &pipe_name,
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | SECURITY_SQOS_PRESENT | SECURITY_IMPERSONATION,
            None,
        )
        .ok()?;
        if WriteFile(handle, Some(&bytes), None, None).is_err() {
            let _ = CloseHandle(handle);
            return None;
        }
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        let mut last_progress = String::new();
        let mut finished = None;
        loop {
            let mut n = 0u32;
            if ReadFile(handle, Some(&mut chunk), Some(&mut n), None).is_err() || n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n as usize]);
            while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line);
                match classify(&text) {
                    Step::Progress(line) => {
                        last_progress = line.clone();
                        progress(&line);
                    }
                    Step::Finished(reply) => {
                        finished = Some(reply);
                    }
                    Step::Ignore => {}
                }
            }
            if finished.is_some() || buf.len() > 256 * 1024 {
                break;
            }
        }
        let _ = CloseHandle(handle);
        Some(finished.unwrap_or_else(|| finish_from_disconnect(&last_progress)))
    }
}

#[cfg(not(windows))]
pub fn request_update(_progress: &(dyn Fn(&str) + Sync)) -> Option<UpdateReply> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_and_a_finished_line_are_told_apart() {
        match classify(r#"{"progress":"Downloading PJDesk 0.6.6… 40%"}"#) {
            Step::Progress(text) => assert!(text.contains("40%")),
            other => panic!("expected progress, got {other:?}"),
        }
        match classify(r#"{"done":true,"ok":true,"message":"PJDesk 0.6.6 is up to date."}"#) {
            Step::Finished(reply) => {
                assert!(reply.ok);
                assert!(reply.message.contains("up to date"));
            }
            other => panic!("expected finished, got {other:?}"),
        }
        assert!(matches!(classify("not json"), Step::Ignore));
    }

    #[test]
    fn a_dropped_pipe_after_install_started_is_still_success() {
        let reply = finish_from_disconnect("Installing PJDesk 0.6.6. PJDesk will close and open again.");
        assert!(reply.ok);
        let failed = finish_from_disconnect("Downloading PJDesk 0.6.6… 10%");
        assert!(!failed.ok);
    }
}
