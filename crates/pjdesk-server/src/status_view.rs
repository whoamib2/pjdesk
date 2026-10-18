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
}

impl Snapshot {
    pub fn from_disk(dir: &Path, port: u16, service_running: bool) -> Self {
        let mut snap = Self {
            version: env!("CARGO_PKG_VERSION").into(),
            port,
            service_running,
            ..Self::default()
        };
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
        } else if !dir.join(cloud::STATUS_FILE).exists() {
            snap.cloud_known = true;
            snap.cloud_on = config_exists;
        }
        snap
    }
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

        let json = serde_json::to_string(&snap).unwrap();
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
