//! Local named pipe the unelevated window reads.
//!
//! The server process (LocalSystem) creates `\\.\pipe\PJDeskServerStatus`.
//! Its ACL lets the interactive user read and nobody remote connect. Each
//! connection receives one JSON [`crate::status_view::Snapshot`] and then
//! the server closes the pipe.

use std::path::PathBuf;

use crate::status_view::Snapshot;

#[cfg_attr(not(windows), allow(dead_code))]
pub const PIPE_NAME: &str = r"\\.\pipe\PJDeskServerStatus";

#[cfg(windows)]
pub fn serve(data_dir: PathBuf, port: u16) {
    let _pipe = PIPE_NAME;
    loop {
        if !serve_once(&data_dir, port) {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}

#[cfg(not(windows))]
pub fn serve(_data_dir: PathBuf, _port: u16) {}

#[cfg(windows)]
fn serve_once(data_dir: &std::path::Path, port: u16) -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{LocalFree, HLOCAL, INVALID_HANDLE_VALUE};
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::{WriteFile, PIPE_ACCESS_OUTBOUND};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SY and BA full control. IU (interactive users) can read. Remote clients are rejected below.
        let sddl = w!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GR;;;IU)");
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, SDDL_REVISION_1, &mut sd, None).is_err() {
            return false;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let pipe = CreateNamedPipeW(
            w!(r"\\.\pipe\PJDeskServerStatus"),
            PIPE_ACCESS_OUTBOUND,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64 * 1024,
            0,
            0,
            Some(&sa),
        );
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        if pipe == INVALID_HANDLE_VALUE {
            return false;
        }
        // 535 is ERROR_PIPE_CONNECTED: the client arrived before ConnectNamedPipe.
        let connected = match ConnectNamedPipe(pipe, None) {
            Ok(()) => true,
            Err(e) => (e.code().0 as u32 & 0xFFFF) == 535,
        };
        if !connected {
            let _ = windows::Win32::Foundation::CloseHandle(pipe);
            return false;
        }
        let snap = Snapshot::from_disk(data_dir, port, true);
        if let Ok(bytes) = serde_json::to_vec(&snap) {
            let _ = WriteFile(pipe, Some(&bytes), None, None);
        }
        let _ = DisconnectNamedPipe(pipe);
        let _ = windows::Win32::Foundation::CloseHandle(pipe);
        true
    }
}

/// Reads one snapshot. Returns `None` within a quarter second when the
/// service is not publishing (stopped, or this is not Windows).
#[cfg(windows)]
pub fn read_snapshot() -> Option<Snapshot> {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, OPEN_EXISTING,
    };
    use windows::Win32::System::Pipes::WaitNamedPipeW;

    unsafe {
        if !WaitNamedPipeW(w!(r"\\.\pipe\PJDeskServerStatus"), 250).as_bool() {
            return None;
        }
        let handle = CreateFileW(
            w!(r"\\.\pipe\PJDeskServerStatus"),
            GENERIC_READ.0,
            FILE_SHARE_READ,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let mut n = 0u32;
            if ReadFile(handle, Some(&mut chunk), Some(&mut n), None).is_err() || n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n as usize]);
            if buf.len() > 256 * 1024 {
                break;
            }
        }
        let _ = CloseHandle(handle);
        serde_json::from_slice(&buf).ok()
    }
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn read_snapshot() -> Option<Snapshot> {
    None
}
