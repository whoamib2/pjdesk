//! A status snapshot the running server can show to a window that is not
//! allowed to read the data folder.
//!
//! The folder is locked to SYSTEM and Administrators. An unelevated window
//! that treats a failed read as "no key", "updates off", or "not signed in"
//! looks like the update wiped the server. This snapshot is the allow-list of
//! facts that are safe to show: no key material, no cloud token, no passwords.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cloud::{self, CloudStatus};
use crate::self_update::{self, Loaded};

/// Files an update must leave in place. The installer copies the exe only.
/// The installer test is what reads this list.
#[cfg_attr(not(test), allow(dead_code))]
pub const PRESERVED: &[&str] = &[
    "server-cert.pem",
    "server-key.pem",
    "accounts.json",
    "devices.json",
    "server-update.json",
    "server-update-status.json",
    "cloud-sync.json",
    "cloud-status.json",
    "cloud-state.json",
    "cloud-instance.txt",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Readable,
    /// The folder exists and this process cannot list it.
    Locked,
    Missing,
}

pub fn access(dir: &Path) -> Access {
    match std::fs::read_dir(dir) {
        Ok(_) => Access::Readable,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Access::Missing,
        Err(_) => Access::Locked,
    }
}

/// Facts the service publishes. Field names are the whole contract: a secret
/// must never be added here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: String,
    pub port: u16,
    /// The process that published this is the running server.
    pub service_running: bool,
    pub fingerprint: String,
    pub fingerprint_known: bool,
    pub update_auto: bool,
    pub update_known: bool,
    pub window_start: String,
    pub window_end: String,
    pub last_check_at: u64,
    pub last_result: String,
    pub update_available: String,
    pub update_error: String,
    pub cloud_on: bool,
    pub cloud_known: bool,
    pub account: String,
    pub role: String,
    pub epoch: u64,
    pub active_name: String,
    pub bucket: String,
    pub last_upload_at: u64,
    pub cloud_updated_at: u64,
    pub cloud_error: String,
    pub notice: String,
    /// The other Cloud Sync server, from that server's health report.
    #[serde(default)]
    pub peer_name: String,
    #[serde(default)]
    pub peer_version: String,
    #[serde(default)]
    pub peer_role: String,
    /// Registered computers, without secrets. Filled when the data folder is readable.
    #[serde(default)]
    pub devices: Vec<ListedDevice>,
    /// True when register, lookup, and relay require a network access token.
    #[serde(default)]
    pub require_accounts: bool,
    /// Address computers use to reach this server. Not a secret.
    #[serde(default)]
    pub public_address: String,
    #[serde(default)]
    pub public_note: String,
    #[serde(default)]
    pub public_needs_confirm: bool,
    #[serde(default)]
    pub public_confirmed: bool,
    /// Whether the public-IP watch setting was read.
    #[serde(default)]
    pub public_watch_known: bool,
    /// True when this server updates a raw IP address on its own.
    #[serde(default)]
    pub public_watch: bool,
    #[serde(default)]
    pub public_ip_line: String,
    /// Last lines of server.log, with passwords and tokens removed.
    /// An unelevated window can copy this. The data folder stays locked.
    #[serde(default)]
    pub log_tail: String,
}

/// One computer, safe to put on the status pipe. No password, secret, or key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedDevice {
    pub id: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub online: bool,
    /// False when this computer has not registered with a network access token.
    #[serde(default)]
    pub signed_in: bool,
}

impl Snapshot {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn from_disk(dir: &Path, port: u16, service_running: bool) -> Self {
        let mut snap = Self { version: env!("CARGO_PKG_VERSION").into(), port, service_running, ..Self::default() };
        if access(dir) != Access::Readable {
            return snap;
        }
        if let Some(id) = crate::server::read_identity(dir) {
            snap.fingerprint = pjdesk_proto::tls::format_fingerprint(&id.fingerprint());
            snap.fingerprint_known = true;
        } else if !dir.join("server-key.pem").exists() {
            snap.fingerprint_known = true;
        }
        match self_update::Settings::read(dir) {
            Loaded::Present(s) => {
                snap.update_auto = s.auto;
                snap.update_known = true;
                snap.window_start = s.window_start;
                snap.window_end = s.window_end;
            }
            Loaded::Absent => snap.update_known = true,
            Loaded::Unreadable => snap.update_known = false,
        }
        match self_update::UpdateStatus::read(dir) {
            Loaded::Present(s) => {
                snap.last_check_at = s.last_check_at;
                snap.last_result = s.last_result;
                snap.update_available = s.available;
                snap.update_error = s.error;
            }
            Loaded::Absent => {}
            Loaded::Unreadable => snap.update_known = false,
        }
        let status = CloudStatus::load(dir);
        let config_exists = dir.join(cloud::CONFIG_FILE).exists();
        if let Some(s) = status {
            snap.cloud_known = true;
            snap.cloud_on = s.enabled || config_exists;
            snap.account = s.account;
            snap.role = s.role;
            snap.epoch = s.epoch;
            snap.active_name = s.active_name;
            snap.bucket = s.bucket;
            snap.last_upload_at = s.last_upload_at;
            snap.cloud_updated_at = s.updated_at;
            snap.cloud_error = s.error;
            snap.notice = s.notice;
            snap.peer_name = s.peer_name;
            snap.peer_version = s.peer_version;
            snap.peer_role = s.peer_role;
        } else if !dir.join(cloud::STATUS_FILE).exists() {
            snap.cloud_known = true;
            snap.cloud_on = config_exists;
        }
        snap.devices = listed_devices(dir);
        let policy = crate::access::Policy::load(dir);
        snap.require_accounts = policy.require_accounts;
        if let Some(address) = crate::public_addr::load(dir) {
            snap.public_address = address.address;
            snap.public_note = address.note;
            snap.public_needs_confirm = address.needs_confirm;
            snap.public_confirmed = address.confirmed;
        }
        let watch = crate::public_addr::IpWatch::load(dir);
        snap.public_watch_known = true;
        snap.public_watch = watch.enabled(&snap.public_address);
        snap.public_ip_line = watch.line();
        snap.log_tail = redacted_log_tail(&dir.join("server.log"));
        snap
    }
}

/// Last lines of a log, safe to put on the clipboard or the status pipe.
pub(crate) fn redacted_log_tail(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(80);
    let joined = lines[start..].join("\n");
    let redacted = redact_secrets(&joined);
    let max = 16_000;
    if redacted.chars().count() <= max {
        redacted
    } else {
        let skip = redacted.chars().count() - max;
        redacted.chars().skip(skip).collect()
    }
}

/// Drops passwords, tokens, and key material. Host names, IDs, and addresses stay.
pub(crate) fn redact_secrets(text: &str) -> String {
    text.lines().map(redact_line).collect::<Vec<_>>().join("\n")
}

fn redact_line(line: &str) -> String {
    let upper = line.to_ascii_uppercase();
    if upper.contains("PRIVATE KEY") || upper.contains("BEGIN CERTIFICATE") || upper.contains("BEGIN RSA") {
        return "[redacted]".into();
    }
    line.split_inclusive(char::is_whitespace).map(redact_word).collect()
}

fn redact_word(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    for key in ["password=", "token=", "secret=", "refresh_token=", "authorization=", "pin="] {
        if let Some(at) = lower.find(key) {
            let end = at + key.len();
            let value = word[end..].trim_end_matches(|c: char| c.is_whitespace());
            let ws = &word[end + value.len()..];
            if value.is_empty() {
                return word.to_string();
            }
            return format!("{}[redacted]{ws}", &word[..end]);
        }
    }
    let trimmed = word.trim_end_matches(|c: char| c.is_whitespace());
    let ws = &word[trimmed.len()..];
    if trimmed.starts_with("eyJ") && trimmed.len() >= 20 {
        return format!("[redacted]{ws}");
    }
    let alnum = trimmed.chars().filter(|c| c.is_ascii_alphanumeric()).count();
    if alnum >= 40 && !trimmed.contains('.') && !trimmed.contains(':') {
        return format!("[redacted]{ws}");
    }
    word.to_string()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn listed_devices(dir: &Path) -> Vec<ListedDevice> {
    #[derive(Deserialize)]
    struct File {
        #[serde(default)]
        devices: std::collections::BTreeMap<String, Rec>,
    }
    #[derive(Deserialize)]
    struct Rec {
        #[serde(default)]
        hostname: String,
        #[serde(default)]
        app_version: String,
        #[serde(default)]
        signed_in: bool,
    }
    #[derive(Deserialize)]
    struct Online {
        id: String,
        #[serde(default)]
        version: String,
    }
    let Ok(bytes) = std::fs::read(dir.join("devices.json")) else {
        return Vec::new();
    };
    let Ok(file) = serde_json::from_slice::<File>(&bytes) else {
        return Vec::new();
    };
    let online_fresh = crate::cloud::online_file_is_fresh(&dir.join("online.json"));
    let online: std::collections::BTreeMap<String, String> = if online_fresh {
        std::fs::read(dir.join("online.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<Online>>(&b).ok())
            .map(|rows| rows.into_iter().map(|o| (o.id, o.version)).collect())
            .unwrap_or_default()
    } else {
        std::collections::BTreeMap::new()
    };
    let mut rows: Vec<ListedDevice> = file
        .devices
        .into_iter()
        .map(|(id, rec)| {
            let live = online.get(&id);
            ListedDevice {
                id,
                hostname: rec.hostname,
                version: live.cloned().unwrap_or(rec.app_version),
                online: live.is_some(),
                signed_in: rec.signed_in,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.hostname.cmp(&b.hostname).then(a.id.cmp(&b.id)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("pjdesk-status-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn snapshot_reports_real_settings_and_drops_secrets() {
        let dir = dir("snap");
        let _id = pjdesk_proto::tls::TlsIdentity::load_or_create(
            &dir.join("server-cert.pem"),
            &dir.join("server-key.pem"),
            "pjdesk-server",
        )
        .unwrap();
        let key_pem = std::fs::read_to_string(dir.join("server-key.pem")).unwrap();
        std::fs::write(
            dir.join("server-update.json"),
            r#"{"auto":true,"window_start":"01:00","window_end":"04:00","jitter_secs":12}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("server-update-status.json"),
            r#"{"last_check_at":1700000000,"last_result":"Up to date","available":"","error":""}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("cloud-sync.json"),
            r#"{"mode":"account","email":"ben@spader.us","token":"SUPERSECRETTOKEN","secret_access_key":"SUPERSECRETKEY","key":"abc"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("cloud-status.json"),
            r#"{"enabled":true,"role":"active","account":"ben@spader.us","epoch":4,"active_name":"office","bucket":"pjdesk","updated_at":1700000100}"#,
        )
        .unwrap();

        let snap = Snapshot::from_disk(&dir, 21200, true);
        assert!(snap.fingerprint_known);
        assert!(!snap.fingerprint.is_empty());
        assert!(snap.update_known && snap.update_auto);
        assert_eq!(snap.window_start, "01:00");
        assert_eq!(snap.last_result, "Up to date");
        assert!(snap.cloud_known && snap.cloud_on);
        assert_eq!(snap.account, "ben@spader.us");
        assert_eq!(snap.role, "active");
        assert_eq!(snap.epoch, 4);
        std::fs::write(
            dir.join("devices.json"),
            r#"{"devices":{"123456789":{"secret_sha256":"DEVICESECRETSHA","hostname":"office","app_version":"0.5.1","first_seen":1,"last_seen":2}}}"#,
        )
        .unwrap();
        let snap = Snapshot::from_disk(&dir, 21200, true);
        assert_eq!(snap.devices.len(), 1);
        assert_eq!(snap.devices[0].version, "0.5.1");
        assert_eq!(snap.devices[0].hostname, "office");
        std::fs::write(
            dir.join("server.log"),
            "host 902488771 registered via LAN (10.0.0.20:21200)\npassword=hunter2 token=SUPERSECRETTOKENVALUE1234567890ABCDEF\n-----BEGIN PRIVATE KEY-----\n",
        )
        .unwrap();
        let snap = Snapshot::from_disk(&dir, 21200, true);
        assert!(snap.log_tail.contains("902488771"), "{}", snap.log_tail);
        assert!(snap.log_tail.contains("10.0.0.20"), "{}", snap.log_tail);
        assert!(!snap.log_tail.contains("hunter2"), "{}", snap.log_tail);
        assert!(!snap.log_tail.contains("SUPERSECRETTOKENVALUE"), "{}", snap.log_tail);
        assert!(!snap.log_tail.contains("PRIVATE KEY"), "{}", snap.log_tail);

        let json = serde_json::to_string(&snap).unwrap();
        assert!(!json.contains("DEVICESECRETSHA"));
        assert!(!json.contains("SUPERSECRETTOKEN"));
        assert!(!json.contains("SUPERSECRETKEY"));
        assert!(!json.contains("BEGIN"));
        assert!(!json.contains(&key_pem));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_folder_is_not_a_blank_server() {
        let dir = dir("locked");
        std::fs::write(dir.join("server-update.json"), r#"{"auto":true,"jitter_secs":3}"#).unwrap();
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0);
            std::fs::set_permissions(&dir, perms.clone()).unwrap();
            let saw = access(&dir);
            let snap = Snapshot::from_disk(&dir, 21200, true);
            perms.set_mode(0o700);
            std::fs::set_permissions(&dir, perms).unwrap();
            if saw == Access::Locked {
                assert!(!snap.update_known);
                assert!(!snap.update_auto);
                assert!(!snap.fingerprint_known);
                assert!(!snap.cloud_known);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
