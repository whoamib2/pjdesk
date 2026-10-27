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
/// Device IDs to disconnect, delete, and block.
pub const BLOCK_FILE: &str = "block-requests.json";
/// Device IDs to allow to register again.
pub const UNBLOCK_FILE: &str = "unblock-requests.json";
/// Access key ids to revoke. The running server disconnects those computers.
pub const REVOKE_KEY_FILE: &str = "revoke-key-requests.json";
/// Drop live connections that have neither an account nor a server access key.
/// Written when Require signed-in accounts is turned on. Offline computers are
/// not waited on.
pub const REQUIRE_DISCONNECT_FILE: &str = "require-disconnect.json";
/// Run a cloud sync pass now.
pub const SYNC_FILE: &str = "sync-requests.json";
/// Make this server the cloud primary.
pub const PROMOTE_FILE: &str = "promote-requests.json";
/// Stop this server uploading to the cloud.
pub const DEMOTE_FILE: &str = "demote-requests.json";
/// Reload the key, accounts and computer list from disk (after joining).
pub const RELOAD_FILE: &str = "reload-requests.json";
/// Assign a group: `deviceId=Group name` (an empty name clears it).
pub const GROUP_FILE: &str = "group-requests.json";
/// Turn incoming-only on or off: `deviceId=on` or `deviceId=off`.
/// The password is checked before this file is written. It is not stored here.
pub const INCOMING_FILE: &str = "incoming-only-requests.json";

const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// A target the service is willing to act on. Anything else is ignored, so a
/// crafted file cannot smuggle a path or a wildcard the caller did not mean.
pub fn acceptable_target(file: &str, target: &str) -> bool {
    match file {
        UPDATE_FILE => target == "*" || pjdesk_proto::is_valid_id(target),
        FORGET_FILE | BLOCK_FILE | UNBLOCK_FILE => pjdesk_proto::is_valid_id(target),
        REVOKE_KEY_FILE => crate::access_keys::is_key_id(target),
        SYNC_FILE | RELOAD_FILE | PROMOTE_FILE | DEMOTE_FILE | REQUIRE_DISCONNECT_FILE => target == "now",
        GROUP_FILE => parse_group_target(target).is_some(),
        INCOMING_FILE => parse_incoming_target(target).is_some(),
        _ => false,
    }
}

/// Adds `target` to a request file, keeping any not yet picked up.
pub fn push(data_dir: &Path, file: &str, target: &str) -> std::io::Result<()> {
    if !acceptable_target(file, target) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "rejected request target"));
    }
    let path = data_dir.join(file);
    let mut list: Vec<String> = read_list(&path);
    if !list.iter().any(|t| t == target) {
        list.push(target.to_string());
    }
    list.truncate(64);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&list).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &path)?;
    crate::cloud::protect(&path);
    Ok(())
}

fn read_list(path: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    if bytes.len() > MAX_REQUEST_BYTES {
        return Vec::new();
    }
    serde_json::from_slice::<Vec<String>>(&bytes)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.len() <= 64)
        .take(64)
        .collect()
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
    let list = read_list(&taken);
    let _ = std::fs::remove_file(&taken);
    list.into_iter().filter(|t| acceptable_target(file, t)).collect()
}

/// `123456789=on` or `123456789=off`. Anything else is ignored.
pub fn parse_incoming_target(target: &str) -> Option<(String, bool)> {
    let (id, flag) = target.split_once('=')?;
    if !pjdesk_proto::is_valid_id(id) {
        return None;
    }
    match flag {
        "on" => Some((id.to_string(), true)),
        "off" => Some((id.to_string(), false)),
        _ => None,
    }
}

/// Checks the account password when turning incoming-only off, saves the
/// registry, and asks the running server to tell that computer. The password
/// is not written to the request file or the log.
pub fn apply_incoming(data_dir: &Path, id: &str, on: bool, password: &str) -> Result<String, String> {
    let id = id.trim();
    if !pjdesk_proto::is_valid_id(id) {
        return Err("That is not a PJDesk computer ID.".into());
    }
    if !on {
        let email = crate::cloud::saved_account_email(data_dir);
        if crate::account_mirror::confirm_owner(data_dir, &email, password).is_err() {
            return Err(
                "Enter the account owner's password to allow this computer to connect to others.".into(),
            );
        }
    }
    let mut reg = crate::registry::Registry::load(data_dir.join("devices.json")).map_err(|e| e.to_string())?;
    reg.set_incoming_only(id, on).map_err(|e| e.to_string())?;
    let target = format!("{id}={}", if on { "on" } else { "off" });
    push(data_dir, INCOMING_FILE, &target).map_err(|e| e.to_string())?;
    let shown = pjdesk_proto::format_id(id);
    Ok(if on {
        format!("Incoming only is on for {shown}.")
    } else {
        format!("{shown} can connect to other computers.")
    })
}

/// `123456789=Office` or `123456789=` to clear. The group is a label, not a path.
pub fn parse_group_target(target: &str) -> Option<(String, String)> {
    let (id, group) = target.split_once('=')?;
    if !pjdesk_proto::is_valid_id(id) || group.chars().count() > 40 {
        return None;
    }
    if group.chars().any(|c| c.is_control() || c == '/' || c == '\\') {
        return None;
    }
    Some((id.to_string(), group.to_string()))
}

/// Forgets a device: removes it from `devices.json` now (enough when the
/// server isn't running) and asks a running server to drop it too.
pub fn forget_device(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    let mut reg = crate::registry::Registry::load(data_dir.join("devices.json"))?;
    reg.remove(id)?;
    push(data_dir, FORGET_FILE, id)?;
    Ok(())
}

/// Removes a computer and blocks that device from registering again.
/// A running server applies the same request to its live table and disconnects it.
pub fn block_device(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    let mut reg = crate::registry::Registry::load(data_dir.join("devices.json"))?;
    if !reg.block(id)? {
        anyhow::bail!("that computer is not registered");
    }
    push(data_dir, BLOCK_FILE, id)?;
    Ok(())
}

/// Revokes an access key and removes every computer that used it from the file.
/// A running server applies the same request to its live table and disconnects them.
pub fn revoke_access_key(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    crate::access_keys::Store::revoke(data_dir, id.trim()).map_err(|e| anyhow::anyhow!(e))?;
    let mut reg = crate::registry::Registry::load(data_dir.join("devices.json"))?;
    let _ = reg.take_access_key(id.trim());
    push(data_dir, REVOKE_KEY_FILE, id.trim())?;
    Ok(())
}

/// Lets a blocked computer register again.
pub fn unblock_device(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    let mut reg = crate::registry::Registry::load(data_dir.join("devices.json"))?;
    if !reg.unblock(id)? {
        anyhow::bail!("that computer is not blocked");
    }
    push(data_dir, UNBLOCK_FILE, id)?;
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
        // A wildcard is an update-all, not a forget. A path is never a target.
        assert!(push(&dir, FORGET_FILE, "*").is_err());
        assert!(push(&dir, FORGET_FILE, "../accounts.json").is_err());
        std::fs::write(dir.join(UPDATE_FILE), br#"["*","not-an-id","123456789"]"#).unwrap();
        assert_eq!(take(&dir, UPDATE_FILE), vec!["*".to_string(), "123456789".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
