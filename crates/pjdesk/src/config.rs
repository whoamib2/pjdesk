//! Client settings, stored as JSON in `%APPDATA%\PJDesk\client.json`
//! (or `~/.config/PJDesk/client.json`). Set `PJDESK_CONFIG` to use
//! another file (handy for running two instances on one machine).
//!
//! Secrets in this file — the account login token, a saved permanent password,
//! and any "remembered" passwords for remote computers — are encrypted with
//! [`crate::secure`] (DPAPI on Windows) before they are written.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use pjdesk_proto::signal::AddressEntry;
use pjdesk_proto::tls::{parse_fingerprint, Fingerprint};
use serde::{Deserialize, Serialize};

use crate::secure;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Server address, `host[:port]`.
    #[serde(default)]
    pub server: Option<String>,
    /// Pinned server key fingerprint (saved on first connection).
    #[serde(default)]
    pub server_fingerprint: Option<String>,
    /// This device's ID, assigned by the server.
    #[serde(default)]
    pub device_id: Option<String>,
    /// Secret proving ownership of `device_id` (hex, 32 bytes).
    #[serde(default)]
    pub device_secret: Option<String>,
    /// Quality, behaviour and startup settings.
    #[serde(default)]
    pub settings: Settings,
    /// The signed-in account, if any.
    #[serde(default)]
    pub account: Option<Account>,
    /// Cached copy of the account's address book (also usable offline).
    #[serde(default)]
    pub address_book: Vec<AddressEntry>,
    /// Remembered passwords for direct connections, by device ID (encrypted).
    /// The account address book is the synced store; this is the local one.
    #[serde(default)]
    saved_passwords: BTreeMap<String, String>,
}

/// Quality, behaviour and startup preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Maximum frames per second when sharing this screen.
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Target video bitrate in kbit/s.
    #[serde(default = "default_bitrate")]
    pub bitrate_kbps: u32,
    /// Screens wider than this are downscaled before encoding.
    #[serde(default = "default_max_width")]
    pub max_width: u32,
    /// Prefer GDI capture over DXGI (compatibility).
    #[serde(default)]
    pub force_gdi: bool,
    /// When sharing, let controllers watch but not control by default.
    #[serde(default)]
    pub view_only: bool,
    /// A fixed password for unattended access (encrypted). When set, the host
    /// uses it instead of a fresh random password each start.
    #[serde(default)]
    pub permanent_password: Option<String>,
    /// Start PJDesk automatically when this user signs in to Windows.
    #[serde(default)]
    pub start_with_windows: bool,
    /// "dark" or "light".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// "prompt" asks the person at this computer before a session starts.
    /// "unattended" accepts on the password alone, with a restricted permission set.
    #[serde(default = "default_approval")]
    pub approval: String,
    /// Folder received files are written into. Empty means file transfer is refused.
    #[serde(default)]
    pub receive_dir: String,
    /// Listen for direct IP connections and announce this computer on the LAN.
    #[serde(default = "yes")]
    pub allow_direct: bool,
    /// Host ports a remote session may forward to (loopback only unless the server policy says otherwise).
    #[serde(default)]
    pub tunnel_ports: Vec<u16>,
    /// Local notes for computers (favorites, last connection). Not synced.
    #[serde(default)]
    pub peers: BTreeMap<String, PeerNote>,
    /// Pinned QUIC fingerprints for direct IP connections, keyed by `host:port`.
    #[serde(default)]
    pub direct_pins: BTreeMap<String, String>,
}

fn yes() -> bool {
    true
}
fn default_theme() -> String {
    "dark".into()
}
fn default_approval() -> String {
    "prompt".into()
}

/// Remembered locally, not part of the synced address book.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerNote {
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub last_connected: u64,
}

fn default_fps() -> u32 {
    30
}
fn default_bitrate() -> u32 {
    6000
}
fn default_max_width() -> u32 {
    2560
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fps: default_fps(),
            bitrate_kbps: default_bitrate(),
            max_width: default_max_width(),
            force_gdi: false,
            view_only: false,
            permanent_password: None,
            start_with_windows: false,
            theme: default_theme(),
            approval: default_approval(),
            receive_dir: String::new(),
            allow_direct: true,
            tunnel_ports: Vec::new(),
            peers: BTreeMap::new(),
            direct_pins: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// The permanent password in the clear, if one is set and decodes.
    pub fn permanent_password(&self) -> Option<String> {
        self.permanent_password.as_deref().and_then(secure::reveal)
    }

    /// Sets (or clears, with `None`) the permanent password, encrypting it
    /// for the current Windows user.
    pub fn set_permanent_password(&mut self, password: Option<&str>) {
        self.permanent_password = password.filter(|p| !p.is_empty()).map(secure::protect);
    }

    /// Like [`set_permanent_password`], but the ciphertext can also be opened
    /// by the SYSTEM service at the sign-in screen. Used only for the service
    /// config file, which is locked down separately.
    pub fn set_permanent_password_machine(&mut self, password: Option<&str>) {
        self.permanent_password = password.filter(|p| !p.is_empty()).map(secure::protect_machine);
    }
}

/// A signed-in server account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub username: String,
    #[serde(default)]
    pub display_name: String,
    /// Login token (encrypted), presented instead of the password next time.
    token: String,
    #[serde(default)]
    pub is_admin: bool,
}

impl Account {
    pub fn new(username: String, display_name: String, token: &str, is_admin: bool) -> Self {
        Self { username, display_name, token: secure::protect(token), is_admin }
    }

    /// The login token in the clear, if it decodes.
    pub fn token(&self) -> Option<String> {
        secure::reveal(&self.token)
    }
}

impl Config {
    pub fn path() -> PathBuf {
        if let Some(path) = std::env::var_os("PJDESK_CONFIG") {
            return PathBuf::from(path);
        }
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        };
        base.unwrap_or_else(|| PathBuf::from(".")).join("PJDesk").join("client.json")
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    /// Loads a config from a specific file (missing file → defaults).
    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// Writes this config to a specific file (atomically).
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// The device secret, created (and saved) on first use.
    ///
    /// Prefers a machine-derived secret so the GUI and the background service
    /// share one identity (no duplicate device registrations) and the assigned
    /// ID survives restarts and reinstalls. Falls back to a stored random secret
    /// only where no stable machine id is available.
    pub fn device_secret(&mut self) -> Result<[u8; 32]> {
        if let Some(secret) = crate::identity::machine_secret() {
            let hexed = hex::encode(secret);
            if self.device_secret.as_deref() != Some(hexed.as_str()) {
                // Adopting the machine secret: any stored ID belonged to an older
                // secret, so drop it and let the server assign one for this one.
                self.device_secret = Some(hexed);
                self.device_id = None;
                self.save()?;
            }
            return Ok(secret);
        }
        if let Some(hex_secret) = &self.device_secret {
            if let Ok(bytes) = hex::decode(hex_secret) {
                if let Ok(secret) = <[u8; 32]>::try_from(bytes.as_slice()) {
                    return Ok(secret);
                }
            }
        }
        let secret = pjdesk_proto::random_bytes::<32>();
        self.device_secret = Some(hex::encode(secret));
        self.device_id = None; // a new secret can't claim the old ID
        self.save()?;
        Ok(secret)
    }

    pub fn pinned_fingerprint(&self) -> Option<Fingerprint> {
        self.server_fingerprint.as_deref().and_then(|s| parse_fingerprint(s).ok())
    }

    /// The server to use: the one the user set, else the baked-in default, so a
    /// fresh install is reachable without any configuration.
    pub fn effective_server(&self) -> String {
        self.server
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| pjdesk_proto::DEFAULT_SERVER.to_string())
    }

    /// Re-reads the device identity and pinned fingerprint from disk. The host
    /// thread assigns and saves these; calling this before [`Self::save`] keeps
    /// the GUI from clobbering them when it writes settings.
    pub fn reload_identity_from_disk(&mut self) {
        if let Ok(disk) = Self::load() {
            if disk.device_id.is_some() {
                self.device_id = disk.device_id;
            }
            if disk.device_secret.is_some() {
                self.device_secret = disk.device_secret;
            }
            if disk.server_fingerprint.is_some() {
                self.server_fingerprint = disk.server_fingerprint;
            }
        }
    }

    // --- Remembered passwords for direct connections -----------------------

    /// Remembers (encrypted) or, with `None`, forgets a device's password.
    pub fn remember_password(&mut self, id: &str, password: Option<&str>) {
        match password.filter(|p| !p.is_empty()) {
            Some(pw) => {
                self.saved_passwords.insert(id.to_string(), secure::protect(pw));
            }
            None => {
                self.saved_passwords.remove(id);
            }
        }
    }

    /// A remembered password for `id`, from the address book first, then local.
    pub fn saved_password(&self, id: &str) -> Option<String> {
        if let Some(entry) = self.address_book.iter().find(|e| e.id == id) {
            if let Some(pw) = &entry.password {
                return Some(pw.clone());
            }
        }
        self.saved_passwords.get(id).and_then(|s| secure::reveal(s))
    }

    // --- Account -----------------------------------------------------------

    pub fn account_token(&self) -> Option<String> {
        self.account.as_ref().and_then(|a| a.token())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembered_password_roundtrips() {
        let mut c = Config::default();
        c.remember_password("123456789", Some("s3cret"));
        assert_eq!(c.saved_password("123456789").as_deref(), Some("s3cret"));
        c.remember_password("123456789", None);
        assert_eq!(c.saved_password("123456789"), None);
    }

    #[test]
    fn address_book_password_takes_priority() {
        let mut c = Config::default();
        c.remember_password("111222333", Some("local"));
        c.address_book =
            vec![AddressEntry { id: "111222333".into(), password: Some("synced".into()), ..Default::default() }];
        assert_eq!(c.saved_password("111222333").as_deref(), Some("synced"));
    }

    #[test]
    fn permanent_password_encodes() {
        let mut s = Settings::default();
        s.set_permanent_password(Some("letmein"));
        assert_ne!(s.permanent_password.as_deref(), Some("letmein"));
        assert_eq!(s.permanent_password().as_deref(), Some("letmein"));
        s.set_permanent_password(None);
        assert_eq!(s.permanent_password(), None);
    }
}
