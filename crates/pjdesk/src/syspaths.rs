//! Machine-wide paths and the small status file the background service shares
//! with the GUI. These are cross-platform so the GUI can read the status file
//! on any build; only the service (Windows) writes them.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// `%ProgramData%\PJDesk` on Windows, else a local fallback (dev/testing).
pub fn program_data_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
            .join("PJDesk")
    } else {
        std::env::var_os("PJDESK_PROGRAMDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp/pjdesk"))
    }
}

/// Folder the background service keeps its config, identity and status in.
pub fn service_dir() -> PathBuf {
    program_data_dir().join("service")
}

/// The service's client config (same format as the per-user config).
pub fn service_config_path() -> PathBuf {
    service_dir().join("client.json")
}

/// The service's live status, including the current one-time password.
/// The service folder's ACL is what keeps other local users from reading it.
pub fn service_status_path() -> PathBuf {
    service_dir().join("status.json")
}

/// What the GUI shows when the background service is hosting this computer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub id: Option<String>,
    pub online: bool,
    pub connected: Option<String>,
    /// The current rotating one-time password to share. `None` means none is
    /// known yet (the service is still starting).
    #[serde(default)]
    pub password: Option<String>,
    /// Whether a permanent password is also set (it works in addition).
    #[serde(default)]
    pub permanent_set: bool,
    /// Last measured round-trip to the server, in milliseconds. 0 means unknown.
    #[serde(default)]
    pub rtt_ms: u64,
    /// `LAN` or `Internet` once the service has reached the server.
    #[serde(default)]
    pub reach: String,
    #[serde(default)]
    pub updated_unix: u64,
    /// Temporary 8-digit support code. Empty until the service has made one.
    #[serde(default)]
    pub support_code: String,
    /// Temporary password that matches the support code. Not the permanent one.
    #[serde(default)]
    pub support_password: String,
    /// Unix time the support pair was created. 0 until the first pair.
    #[serde(default)]
    pub support_born_unix: u64,
    /// Unix time of the last Register or Pong from the server. 0 if this file
    /// was written by an older service.
    #[serde(default)]
    pub acked_unix: u64,
    /// Server address the service registered with. Empty on an older file.
    #[serde(default)]
    pub server: String,
}

impl ServiceStatus {
    pub fn now(
        id: Option<String>,
        online: bool,
        connected: Option<String>,
        password: Option<String>,
        permanent_set: bool,
    ) -> Self {
        Self {
            id,
            online,
            connected,
            password,
            permanent_set,
            rtt_ms: 0,
            reach: String::new(),
            updated_unix: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            support_code: String::new(),
            support_password: String::new(),
            support_born_unix: 0,
            acked_unix: 0,
            server: String::new(),
        }
    }

    /// The ack time to show. An older service file has no ack stamp; its
    /// rewrite time is used only while it still says online and names no server.
    pub fn ack_stamp(&self) -> u64 {
        if self.acked_unix > 0 {
            self.acked_unix
        } else if self.online && self.server.trim().is_empty() {
            self.updated_unix
        } else {
            0
        }
    }

    /// The service has a recent server ack and the status file is still fresh.
    pub fn registration_fresh(&self, now: u64) -> bool {
        self.is_fresh() && pjdesk_proto::registration_acked(self.online, self.ack_stamp(), now)
    }

    pub fn load() -> Option<Self> {
        let bytes = std::fs::read(service_status_path()).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn write(&self) {
        if let Some(dir) = service_status_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let tmp = service_status_path().with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, service_status_path());
            }
        }
    }

    /// Fresh if written in the last ~10 seconds (so a dead service looks offline).
    pub fn is_fresh(&self) -> bool {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        now.saturating_sub(self.updated_unix) < 10
    }

    /// The service process is up and this computer is registered through it.
    /// Service Control Manager "Running" is not enough: the helper can be
    /// alive while its host thread has stopped.
    pub fn is_hosting(&self) -> bool {
        self.is_fresh() && self.online
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosting_requires_a_fresh_online_status() {
        let mut status = ServiceStatus::now(Some("692996394".into()), true, None, None, false);
        assert!(status.is_hosting());
        status.online = false;
        assert!(!status.is_hosting(), "a fresh file that is not online is not hosting");
        status.online = true;
        status.updated_unix = 0;
        assert!(!status.is_hosting(), "a stale file is not hosting");
    }

    #[test]
    fn an_old_online_file_counts_and_a_named_server_needs_an_ack() {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut old = ServiceStatus::now(Some("902488771".into()), true, None, None, false);
        assert!(old.registration_fresh(now), "a fresh online file from an older service still counts");
        old.server = "pjdesk.spader.us:21200".into();
        old.acked_unix = 0;
        assert!(!old.registration_fresh(now), "a named server with no ack is not online");
        old.acked_unix = now;
        assert!(old.registration_fresh(now));
        old.acked_unix = now.saturating_sub(120);
        assert!(!old.registration_fresh(now));
    }
}
