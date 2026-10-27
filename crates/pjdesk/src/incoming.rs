//! Incoming-only: this computer can be controlled, and cannot start a session.
//!
//! The flag lives in ProgramData, not in the per-user config. The service
//! locks the file so a non-admin account cannot change it. The window, the
//! `connect` command, and a link all read it before dialing.

use std::path::{Path, PathBuf};

use pjdesk_proto::signal::{incoming_only_change, INCOMING_ONLY_REFUSAL};

pub use pjdesk_proto::signal::{INCOMING_ONLY_LABEL, INCOMING_ONLY_REFUSAL as REFUSAL};

/// True when the service file says this computer cannot connect out.
pub fn enabled() -> bool {
    read_flag(&flag_path())
}

/// Writes the flag and locks it to SYSTEM and Administrators for changes.
/// Users may read it. A non-admin write fails, which is the point.
pub fn apply(on: bool) -> std::io::Result<()> {
    let dir = policy_dir();
    std::fs::create_dir_all(&dir)?;
    lock_dir(&dir);
    write_flag(&flag_path(), on)?;
    lock_flag(&flag_path());
    Ok(())
}

/// `Err` with [`REFUSAL`] when outgoing sessions are not allowed.
pub fn refuse_if(on: bool) -> Result<(), &'static str> {
    if on { Err(INCOMING_ONLY_REFUSAL) } else { Ok(()) }
}

pub fn refuse_outgoing() -> Result<(), &'static str> {
    refuse_if(enabled())
}

fn policy_dir() -> PathBuf {
    crate::syspaths::program_data_dir().join("policy")
}

fn flag_path() -> PathBuf {
    policy_dir().join("incoming-only")
}

/// The Windows service is the only writer while it is up. A window on a
/// machine with no service, and every non-Windows build, writes the file itself.
pub fn owns_flag(service_host: bool) -> bool {
    if service_host {
        return true;
    }
    !crate::syspaths::ServiceStatus::load().is_some_and(|status| status.is_fresh())
}

fn ask_path() -> PathBuf {
    crate::syspaths::service_dir().join("incoming-ask.json")
}

fn result_path() -> PathBuf {
    crate::syspaths::service_dir().join("incoming-result.json")
}

fn read_flag(path: &Path) -> bool {
    std::fs::read_to_string(path).ok().map(|text| text.trim() == "1").unwrap_or(false)
}

fn write_flag(path: &Path, on: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = if on { "1\n" } else { "0\n" };
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// SYSTEM and Administrators can change the file. Users can read it.
/// A non-admin account cannot flip the flag by editing the file, and cannot
/// delete it, because the policy folder itself is not writable by Users.
#[cfg(windows)]
fn lock_flag(path: &Path) {
    lock_acl(path, false);
}

#[cfg(windows)]
fn lock_dir(path: &Path) {
    lock_acl(path, true);
}

#[cfg(windows)]
fn lock_acl(path: &Path, directory: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let full = if directory { "*S-1-5-18:(OI)(CI)F" } else { "*S-1-5-18:F" };
    let admins = if directory { "*S-1-5-32-544:(OI)(CI)F" } else { "*S-1-5-32-544:F" };
    let users = if directory { "*S-1-5-32-545:(OI)(CI)R" } else { "*S-1-5-32-545:R" };
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", full, admins, users, "/Q"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(windows))]
fn lock_flag(_path: &Path) {}

#[cfg(not(windows))]
fn lock_dir(_path: &Path) {}

/// The window asks the host (the service, when it is running) to change the flag.
/// `password` is required to turn it off. The file is removed as soon as it is read.
pub fn queue_ask(on: bool, password: &str) -> std::io::Result<()> {
    let path = ask_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::json!({ "on": on, "password": password });
    std::fs::write(path, body.to_string())?;
    Ok(())
}

pub fn take_ask() -> Option<(bool, String)> {
    let path = ask_path();
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(path);
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let on = value.get("on").and_then(|v| v.as_bool())?;
    let password = value.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some((on, password))
}

pub fn write_result(ok: bool, message: &str) {
    let path = result_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = serde_json::json!({ "ok": ok, "message": message });
    let _ = std::fs::write(path, body.to_string());
}

pub fn take_result() -> Option<(bool, String)> {
    let path = result_path();
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(path);
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let ok = value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let message = value.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some((ok, message))
}

/// Applies a local request after the password check. Returns the message for the window.
pub fn decide_ask(currently_on: bool, want_on: bool, password_ok: bool) -> Result<bool, &'static str> {
    incoming_only_change(want_on, currently_on, password_ok)?;
    Ok(want_on)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_is_refused_when_incoming_only_is_on() {
        assert!(refuse_if(false).is_ok());
        assert_eq!(refuse_if(true).unwrap_err(), INCOMING_ONLY_REFUSAL);
        assert!(decide_ask(false, true, false).is_ok());
        assert!(decide_ask(true, false, false).is_err());
        assert_eq!(decide_ask(true, false, true).unwrap(), false);
    }

    #[test]
    fn the_flag_file_round_trips_without_using_the_user_config() {
        let dir = std::env::temp_dir().join(format!("pjdesk-incoming-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("incoming-only");
        assert!(!read_flag(&path));
        write_flag(&path, true).unwrap();
        assert!(read_flag(&path));
        write_flag(&path, false).unwrap();
        assert!(!read_flag(&path));
        assert!(!path.ends_with("client.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
