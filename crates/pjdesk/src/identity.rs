//! Device identity for one install.
//!
//! The legacy secret ([`machine_secret`]) is derived from the Windows
//! MachineGuid so the window and the SYSTEM service prove the same computer.
//! A cloned image copies that GUID, so it is not unique. [`install_key`] is a
//! random 32-byte value created once in ProgramData and read by both
//! processes. The server keeps the first install that claims an ID and gives
//! the next install key its own ID.

use std::path::Path;
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

/// Random key created once for this install. The window and the service read
/// the same file, so they stay one computer. A cloned image does not share it
/// unless the file itself was copied after it was created.
pub fn install_key() -> [u8; 32] {
    static CACHE: OnceLock<[u8; 32]> = OnceLock::new();
    *CACHE.get_or_init(|| ensure_install_key(&install_key_path()))
}

fn install_key_path() -> std::path::PathBuf {
    crate::syspaths::program_data_dir().join("install-key")
}

fn ensure_install_key(path: &Path) -> [u8; 32] {
    if let Some(key) = read_install_key(path) {
        return key;
    }
    let key = pjdesk_proto::random_bytes::<32>();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = format!("{}\n", hex::encode(key));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            use std::io::Write;
            if file.write_all(text.as_bytes()).is_err() {
                return read_install_key(path).unwrap_or(key);
            }
            share_install_key(path);
            read_install_key(path).unwrap_or(key)
        }
        Err(_) => read_install_key(path).unwrap_or(key),
    }
}

fn read_install_key(path: &Path) -> Option<[u8; 32]> {
    let text = std::fs::read_to_string(path).ok()?;
    let bytes = hex::decode(text.trim()).ok()?;
    let key = <[u8; 32]>::try_from(bytes.as_slice()).ok()?;
    if key == [0u8; 32] { None } else { Some(key) }
}

/// Users can read the key. SYSTEM and Administrators can change it. The
/// unelevated window and the service then share one install.
#[cfg(windows)]
fn share_install_key(path: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .args(["/grant", "*S-1-5-18:F", "*S-1-5-32-544:F", "*S-1-5-32-545:R", "/Q"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(windows))]
fn share_install_key(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_key_is_created_once_and_reread() {
        let dir = std::env::temp_dir().join(format!("pjdesk-install-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("install-key");
        let first = ensure_install_key(&path);
        let second = ensure_install_key(&path);
        assert_eq!(first, second);
        assert_ne!(first, [0u8; 32]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
