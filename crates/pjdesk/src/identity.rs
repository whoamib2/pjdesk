//! A stable, per-machine device identity.
//!
//! The device *secret* proves ownership of the server-assigned ID. Deriving it
//! from a machine-unique value (the Windows MachineGuid) means the GUI and the
//! background service compute the **same** secret, so they register as one
//! device — no duplicates — and the ID is the same across restarts and
//! reinstalls. If no machine id is available we fall back to a stored random
//! secret (see [`crate::config::Config::device_secret`]).

use std::sync::OnceLock;

/// The machine-derived device secret, or `None` on a platform without a stable
/// machine id. Computed once and cached.
pub fn machine_secret() -> Option<[u8; 32]> {
    static CACHE: OnceLock<Option<[u8; 32]>> = OnceLock::new();
    *CACHE.get_or_init(|| machine_id().map(|id| pjdesk_proto::derive_device_secret(&id)))
}

/// A stable identifier unique to this computer.
#[cfg(windows)]
fn machine_id() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // The MachineGuid is written once at OS install and is stable for the life
    // of the installation. Reading it via `reg` (with no console window) avoids
    // a fragile registry FFI dance and works the same from the GUI and SYSTEM.
    let out = std::process::Command::new("reg")
        .args(["query", r"HKLM\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if line.contains("MachineGuid") {
            if let Some(idx) = line.find("REG_SZ") {
                let val = line[idx + "REG_SZ".len()..].trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// Dev/testing fallback: an explicit id via the environment, else none (so the
/// stored random secret is used).
#[cfg(not(windows))]
fn machine_id() -> Option<String> {
    std::env::var("PJDESK_MACHINE_ID").ok().filter(|s| !s.trim().is_empty())
}
