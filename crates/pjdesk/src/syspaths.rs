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

/// The service's live status, readable by the GUI (no secrets).
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
    #[serde(default)]
    pub updated_unix: u64,
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
            updated_unix: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        }
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
}
