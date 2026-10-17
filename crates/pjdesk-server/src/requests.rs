//! Request files the management window leaves for the running server.
//!
//! The window and the server (normally the Windows service) are separate
//! processes. The server owns `devices.json` in memory, so the window can't
//! just edit the file — the server would write its own copy back on the next
//! connection. Instead the window drops a small JSON list of device IDs here
//! and the server applies it within a couple of seconds.

use std::path::Path;

/// Device IDs to update now (`"*"` = every online device).
pub const UPDATE_FILE: &str = "update-requests.json";
/// Device IDs to forget.
pub const FORGET_FILE: &str = "forget-requests.json";
/// Run a cloud sync pass now.
pub const SYNC_FILE: &str = "sync-requests.json";
/// Reload the key, accounts and computer list from disk (after joining).
pub const RELOAD_FILE: &str = "reload-requests.json";

/// Adds `target` to a request file, keeping any not yet picked up.
pub fn push(data_dir: &Path, file: &str, target: &str) -> std::io::Result<()> {
    let path = data_dir.join(file);
    let mut list: Vec<String> =
        std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if !list.iter().any(|t| t == target) {
        list.push(target.to_string());
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&list).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &path)
}

/// Takes the pending requests (reads and removes the file). Moves the file
/// aside first, so a request written meanwhile lands in a new file instead of
/// being lost.
pub fn take(data_dir: &Path, file: &str) -> Vec<String> {
    let path = data_dir.join(file);
    let taken = path.with_extension("json.taken");
    if std::fs::rename(&path, &taken).is_err() {
        return Vec::new();
    }
    let list = std::fs::read(&taken).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let _ = std::fs::remove_file(&taken);
    list
}

/// Forgets a device: removes it from `devices.json` now (enough when the
/// server isn't running) and asks a running server to drop it too.
pub fn forget_device(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    let mut reg = crate::registry::Registry::load(data_dir.join("devices.json"))?;
    reg.remove(id)?;
    push(data_dir, FORGET_FILE, id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_accumulates_and_take_drains() {
        let dir = std::env::temp_dir().join(format!("pjdesk-requests-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        push(&dir, FORGET_FILE, "111111111").unwrap();
        push(&dir, FORGET_FILE, "222222222").unwrap();
        push(&dir, FORGET_FILE, "111111111").unwrap();
        assert_eq!(take(&dir, FORGET_FILE), vec!["111111111".to_string(), "222222222".to_string()]);
        assert!(take(&dir, FORGET_FILE).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
