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

/// A support key this computer accepted. The 8 digits are not kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportGrant {
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub key_id: String,
    #[serde(default)]
    pub unattended: bool,
    /// True after the client ticked the consent box for an unattended key.
    #[serde(default)]
    pub consented: bool,
}

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
    /// The account also keeps a sealed copy. This file never stores them in
    /// the clear, and the account service only stores ciphertext.
    #[serde(default)]
    pub(crate) saved_passwords: BTreeMap<String, String>,
    /// When each saved password was last changed here, including a deletion.
    /// An id with a time and no password was deleted.
    #[serde(default)]
    pub(crate) password_vault_times: BTreeMap<String, u64>,
    /// Hex vault key, sealed with DPAPI. Empty for a temporary sign-in.
    #[serde(default)]
    pub password_vault_key: Option<String>,
    /// Sealed PJLOCK01 blob for the account PIN. Empty when no PIN is set.
    #[serde(default)]
    pub app_lock_sealed: String,
    #[serde(default)]
    pub app_lock_updated: u64,
    /// Account the cached verifier belongs to.
    #[serde(default)]
    pub app_lock_email: String,
    /// Opened verifier (salt and slow hash), sealed with DPAPI. Not the PIN.
    #[serde(default)]
    pub app_lock_verifier: Option<String>,
    #[serde(default)]
    pub app_lock_fails: u32,
    /// Unix time until which more PIN tries wait. 0 when there is no wait.
    #[serde(default)]
    pub app_lock_until: u64,
    /// The PIN is saved on this computer and has not reached the account yet.
    #[serde(default)]
    pub app_lock_pending: bool,
    /// Skip for now on Set a PIN. The blocking screen stays off.
    #[serde(default)]
    pub pin_declined: bool,
    /// `0.6.47` after this build has decided whether an older session was stuck on Set a PIN.
    #[serde(default)]
    pub pin_prompt: String,
    /// This computer joined with a code or a key. The account email may still be known.
    /// The account password was not typed here, so a PIN stays optional.
    #[serde(default)]
    pub joined_without_password: bool,
    /// PJDesk account that owns the private server this client uses.
    #[serde(default)]
    pub network_email: Option<String>,
    /// `owner` or `member`, from the last sign-in. Empty until then.
    #[serde(default)]
    pub network_role: String,
    /// Set when this computer was enrolled with a support key. The digits are not stored.
    #[serde(default)]
    pub support_grant: Option<SupportGrant>,
    /// Server access key, sealed so the background service can present it.
    /// The key text is not stored in the clear.
    #[serde(default)]
    pub server_access_key: Option<String>,
    /// Refresh token for that account, sealed so the background service can read it.
    #[serde(default)]
    pub network_refresh: Option<String>,
    /// Short-lived access token for the private server.
    #[serde(default)]
    pub network_access: Option<String>,
    #[serde(default)]
    pub network_access_exp: u64,
    /// Stable id named by the access token. Not the nine-digit PJDesk ID.
    #[serde(default)]
    pub network_device_id: Option<String>,
    #[serde(default)]
    pub network_standby: Option<String>,
    #[serde(default)]
    pub network_standby_fingerprint: Option<String>,
    /// LAN addresses of the active server (`ip:port`), published with its public address.
    #[serde(default)]
    pub network_lan: Vec<String>,
    /// The active server's observed public IP, without a port. Empty until it publishes one.
    #[serde(default)]
    pub network_public_ip: String,
    /// Public and LAN addresses of both servers. Hostnames are listed separately.
    #[serde(default)]
    pub network_routes: pjdesk_proto::routes::ServerRoutes,
    /// LAN address that completed a handshake on this network, keyed by the local IPv4 (`ip:10.0.0.8`).
    #[serde(default)]
    pub lan_path_cache: BTreeMap<String, String>,
    /// The encrypted network key last applied on this computer. The passphrase
    /// is not stored. Export copies this string; nothing logs it.
    #[serde(default)]
    pub network_key: Option<String>,
    /// True after this computer has merged its local Saved list into the account once.
    /// Sign-out clears it so the next sign-in merges again. The list itself stays.
    #[serde(default)]
    pub network_list_merged: bool,
    /// Saved computers this person removed on this computer only.
    /// They stay off this list. The account still has them.
    #[serde(default)]
    pub saved_hidden: Vec<String>,
    /// Computers deleted from the pjdesk.com account. They stay off every
    /// signed-in client until that computer signs in again.
    #[serde(default)]
    pub account_removed: Vec<String>,
    /// The saved list on this computer was cleared. Account refreshes must
    /// not put it back. Load my saved computers clears this. A sign-in clears
    /// it only when this device already syncs the account list.
    #[serde(default)]
    pub local_saved_cleared: bool,
    /// Whether this device downloads the account's saved computers.
    /// `None` is a config from before the choice. Startup sets `Some(true)`
    /// when this computer already shows a list, so an update does not wipe it.
    #[serde(default)]
    pub saved_sync: Option<bool>,
    /// A sign-in that must not be written. Saves keep the account, the saved
    /// list, and passwords already on disk, and write this process's device id.
    #[serde(skip)]
    pub ephemeral_network: bool,
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
    /// The first-login "Remember password" box starts checked.
    #[serde(default)]
    pub remember_passwords: bool,
    /// Minutes with no PJDesk activity before a temporary sign-in ends.
    #[serde(default = "default_temp_idle")]
    pub temp_idle_minutes: u32,
    /// "system" follows Windows, or "dark" / "light".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// "prompt" asks the person at this computer before a session starts.
    /// "unattended" accepts on the password alone, with a restricted permission set.
    #[serde(default = "default_approval")]
    pub approval: String,
    /// Folder received files are written into. Empty means file transfer is refused.
    #[serde(default)]
    pub receive_dir: String,
    /// Listen for direct IP connections.
    #[serde(default = "yes")]
    pub allow_direct: bool,
    /// Look for PJDesk on the local network, and let others find this computer.
    /// On unless the person turns it off. The announcement has no password.
    #[serde(default = "yes")]
    pub discover_lan: bool,
    /// Saved-list layout on this computer: "list", "small", or "big".
    #[serde(default = "default_saved_view")]
    pub saved_view: String,
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

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(1)
}
fn default_theme() -> String {
    "system".into()
}
fn default_approval() -> String {
    "prompt".into()
}
fn default_saved_view() -> String {
    "list".into()
}

/// True when `text` is the same 9-digit ID, ignoring spaces.
pub fn same_id(text: &str, id: &str) -> bool {
    let a: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let b: String = id.chars().filter(|c| !c.is_whitespace()).collect();
    !a.is_empty() && a.eq_ignore_ascii_case(&b)
}

/// A name the user typed. The ID and the computer's own hostname are not nicknames.
pub fn nickname_text(alias: &str, id: &str, hostname: &str) -> String {
    let alias = alias.trim();
    if alias.is_empty() || same_id(alias, id) {
        return String::new();
    }
    if !hostname.trim().is_empty() && alias.eq_ignore_ascii_case(hostname.trim()) {
        return String::new();
    }
    alias.to_string()
}

/// Remembered locally, not part of the synced address book.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerNote {
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub last_connected: u64,
    /// Hostname the host reported. Empty until a connection, LAN presence, or the account list supplies one.
    #[serde(default)]
    pub hostname: String,
    /// "windows", "macos", "linux", "android", or empty when the host did not say.
    #[serde(default)]
    pub os_family: String,
    #[serde(default)]
    pub os_version: String,
    /// Last interactive user reported by that computer. Empty until one is known.
    #[serde(default)]
    pub console_user: String,
    /// User order in Saved. 0 means unordered. Not a password and not uploaded by itself.
    #[serde(default)]
    pub sort_order: u32,
    /// Friendly name reported by that computer. A nickname typed here still wins on this screen.
    #[serde(default)]
    pub display_name: String,
    /// When the account's server last saw this computer. 0 until then.
    #[serde(default)]
    pub last_seen: u64,
    /// True when the account's server says this computer is connected.
    #[serde(default)]
    pub reported_online: bool,
    /// PJDesk version reported for this computer. Empty until the account list has one.
    #[serde(default)]
    pub app_version: String,
    /// LAN addresses this computer advertised. A controller on the same subnet
    /// dials these on the direct port and does not need a relay.
    #[serde(default)]
    pub lan_addrs: Vec<String>,
    /// QUIC certificate from a previous session or a LAN beacon.
    #[serde(default)]
    pub host_fingerprint: String,
    /// 0 = one display, 1 = all displays, 2 = a window per display.
    #[serde(default)]
    pub view_mode: u8,
    #[serde(default)]
    pub last_display: u8,
    /// Where each remote display's window sat, restored when those local
    /// monitors are still attached.
    #[serde(default)]
    pub windows: Vec<SavedWindow>,
}

/// A separate-window placement for one remote display.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedWindow {
    pub display: u8,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// winit monitor name. Empty when the window had no monitor yet.
    #[serde(default)]
    pub monitor: String,
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
fn default_temp_idle() -> u32 {
    30
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
            remember_passwords: false,
            temp_idle_minutes: default_temp_idle(),
            theme: default_theme(),
            approval: default_approval(),
            receive_dir: String::new(),
            allow_direct: true,
            discover_lan: true,
            saved_view: default_saved_view(),
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

    /// Records the operating system a host reported, reloading first so a
    /// change the window saved while the viewer was open is not overwritten.
    pub fn remember_platform(id: &str, family: &str, version: &str) {
        if id.is_empty() || family.is_empty() {
            return;
        }
        let path = Self::path();
        let mut cfg = Self::load_from(&path).unwrap_or_default();
        let note = cfg.settings.peers.entry(id.to_string()).or_default();
        if note.os_family == family && note.os_version == version {
            return;
        }
        note.os_family = family.to_string();
        note.os_version = version.to_string();
        let _ = cfg.save_to(&path);
    }

    /// Caches the hostname a host reported, reloading first so a save from the
    /// window does not drop it. Refreshed on each connect. An ID is not a hostname.
    pub fn remember_hostname(id: &str, hostname: &str) {
        let hostname = hostname.trim();
        if hostname.is_empty() || !pjdesk_proto::is_valid_id(id) || same_id(hostname, id) {
            return;
        }
        let path = Self::path();
        let mut cfg = Self::load_from(&path).unwrap_or_default();
        if cfg.local_saved_cleared {
            return;
        }
        let note = cfg.settings.peers.entry(id.to_string()).or_default();
        if note.hostname == hostname {
            return;
        }
        note.hostname = hostname.to_string();
        let _ = cfg.save_to(&path);
        std::thread::spawn(|| {
            let _ = crate::network::push_saved_computers_from_disk();
        });
    }

    /// Fills hostname and OS this copy does not have yet. A value already in
    /// memory is kept. Used before a save so the window does not wipe what the
    /// viewer just wrote.
    pub fn fill_peer_facts_from(&mut self, disk: &Config) {
        if self.local_saved_cleared {
            return;
        }
        for (id, src) in &disk.settings.peers {
            let note = self.settings.peers.entry(id.clone()).or_default();
            if note.hostname.trim().is_empty() && !src.hostname.trim().is_empty() {
                note.hostname = src.hostname.clone();
            }
            if note.os_family.trim().is_empty() && !src.os_family.trim().is_empty() {
                note.os_family = src.os_family.clone();
                note.os_version = src.os_version.clone();
            } else if note.os_version.is_empty() && note.os_family == src.os_family && !src.os_version.is_empty() {
                note.os_version = src.os_version.clone();
            }
            if note.console_user.trim().is_empty() && !src.console_user.trim().is_empty() {
                note.console_user = src.console_user.clone();
            }
            if src.last_seen > note.last_seen {
                note.last_seen = src.last_seen;
                note.reported_online = src.reported_online;
            }
            if src.last_connected > note.last_connected {
                note.last_connected = src.last_connected;
            }
            if note.windows.is_empty() && !src.windows.is_empty() {
                note.windows = src.windows.clone();
            }
            if note.view_mode == 0 && note.last_display == 0 && (src.view_mode != 0 || src.last_display != 0) {
                note.view_mode = src.view_mode;
                note.last_display = src.last_display;
            }
            for addr in &src.lan_addrs {
                if !note.lan_addrs.iter().any(|have| pjdesk_proto::routes::same_endpoint(have, addr)) {
                    note.lan_addrs.push(addr.clone());
                }
            }
            if note.host_fingerprint.trim().is_empty() && !src.host_fingerprint.trim().is_empty() {
                note.host_fingerprint = src.host_fingerprint.clone();
            }
        }
    }

    /// Remembers LAN addresses and the host certificate so the next session
    /// can connect directly. Reloads first so a window save is not overwritten.
    pub fn remember_lan(id: &str, addrs: &[String], fingerprint: Option<&str>) {
        let id = pjdesk_proto::normalize_id(id);
        if !pjdesk_proto::is_valid_id(&id) {
            return;
        }
        let path = Self::path();
        let mut cfg = Self::load_from(&path).unwrap_or_default();
        if cfg.local_saved_cleared {
            return;
        }
        let note = cfg.settings.peers.entry(id).or_default();
        let mut changed = false;
        for addr in addrs {
            let addr = addr.trim();
            if addr.is_empty() {
                continue;
            }
            if note.lan_addrs.iter().any(|have| pjdesk_proto::routes::same_endpoint(have, addr)) {
                continue;
            }
            note.lan_addrs.push(addr.to_string());
            changed = true;
        }
        if note.lan_addrs.len() > 8 {
            let drop = note.lan_addrs.len() - 8;
            note.lan_addrs.drain(0..drop);
            changed = true;
        }
        if let Some(fp) = fingerprint.map(str::trim).filter(|fp| !fp.is_empty()) {
            if note.host_fingerprint != fp {
                note.host_fingerprint = fp.to_string();
                changed = true;
            }
        }
        if changed {
            let _ = cfg.save_to(&path);
        }
    }

    /// Copies host-reported hostname and OS from `disk` when that file has them.
    /// The viewer writes those after a connect; the window re-reads them when
    /// Saved computers opens and when the viewer exits.
    pub fn take_peer_reports_from(&mut self, disk: &Config) {
        if self.local_saved_cleared {
            return;
        }
        for (id, src) in &disk.settings.peers {
            let note = self.settings.peers.entry(id.clone()).or_default();
            if !src.hostname.trim().is_empty() {
                note.hostname = src.hostname.clone();
            }
            if !src.os_family.trim().is_empty() {
                note.os_family = src.os_family.clone();
                if !src.os_version.is_empty() {
                    note.os_version = src.os_version.clone();
                }
            }
            if !src.console_user.trim().is_empty() {
                note.console_user = src.console_user.clone();
            }
            if src.last_seen > note.last_seen {
                note.last_seen = src.last_seen;
            }
            note.reported_online = src.reported_online;
            if src.last_connected > note.last_connected {
                note.last_connected = src.last_connected;
            }
        }
        for entry in &disk.address_book {
            let id = pjdesk_proto::normalize_id(&entry.id);
            if !pjdesk_proto::is_valid_id(&id) {
                continue;
            }
            if self.address_book.iter().any(|e| e.id == id) {
                continue;
            }
            self.address_book.push(AddressEntry {
                id,
                alias: entry.alias.clone(),
                password: None,
                tags: entry.tags.clone(),
            });
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// Writes this config to a specific file (atomically).
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        let writing = self.for_disk(path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&writing)?)?;
        std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// A temporary sign-in keeps the account fields already on disk.
    fn for_disk(&self, path: &std::path::Path) -> Self {
        if !self.ephemeral_network {
            return self.clone();
        }
        let mut writing = self.clone();
        writing.ephemeral_network = false;
        let disk = Self::load_from(path).ok();
        if let Some(disk) = disk {
            writing.network_email = disk.network_email;
            writing.network_refresh = disk.network_refresh;
            writing.network_access = disk.network_access;
            writing.network_access_exp = disk.network_access_exp;
            writing.network_list_merged = disk.network_list_merged;
            writing.network_standby = disk.network_standby;
            writing.network_standby_fingerprint = disk.network_standby_fingerprint;
            writing.network_lan = disk.network_lan;
            writing.network_public_ip = disk.network_public_ip;
            writing.network_routes = disk.network_routes;
            writing.lan_path_cache = disk.lan_path_cache;
            writing.network_key = disk.network_key;
            writing.joined_without_password = disk.joined_without_password;
            writing.server = disk.server;
            writing.server_fingerprint = disk.server_fingerprint;
            writing.saved_hidden = disk.saved_hidden;
            writing.password_vault_key = disk.password_vault_key;
            if self.local_saved_cleared {
                writing.address_book.clear();
                writing.saved_passwords.clear();
                writing.password_vault_times.clear();
                writing.settings.peers.clear();
                writing.settings.direct_pins.clear();
                writing.local_saved_cleared = true;
            } else {
                writing.address_book = disk.address_book;
                writing.saved_passwords = disk.saved_passwords;
                writing.password_vault_times = disk.password_vault_times;
                writing.settings.peers = disk.settings.peers;
                writing.local_saved_cleared = disk.local_saved_cleared;
            }
            let mut removed = disk.account_removed;
            for id in &self.account_removed {
                if !removed.iter().any(|h| h == id) {
                    removed.push(id.clone());
                }
            }
            writing.account_removed = removed;
            let gone = writing.account_removed.clone();
            writing.address_book.retain(|e| !gone.iter().any(|h| h == &e.id));
            for id in &gone {
                writing.settings.peers.remove(id);
                writing.saved_passwords.remove(id);
            }
            if writing.device_id.is_none() {
                writing.device_id = disk.device_id;
            }
            if writing.device_secret.is_none() {
                writing.device_secret = disk.device_secret;
            }
            if writing.network_device_id.is_none() {
                writing.network_device_id = disk.network_device_id;
            }
        } else {
            writing.network_email = None;
            writing.network_refresh = None;
            writing.network_access = None;
            writing.network_access_exp = 0;
            writing.network_list_merged = false;
            writing.address_book.clear();
            writing.saved_hidden.clear();
            writing.account_removed.clear();
            writing.saved_passwords.clear();
            writing.password_vault_times.clear();
            writing.password_vault_key = None;
            writing.settings.peers.clear();
            writing.server = None;
            writing.server_fingerprint = None;
            writing.network_key = None;
            writing.joined_without_password = false;
        }
        writing
    }

    /// The legacy device secret, created (and saved) on first use.
    ///
    /// Prefers a machine-derived secret so the window and the background service
    /// share one identity. A cloned image copies that secret, so the ID itself
    /// comes from [`crate::identity::install_key`], not from this value. Falls
    /// back to a stored random secret only where no stable machine id is
    /// available. Adopting the machine secret still drops an ID that belonged
    /// to a different stored secret. Creating the install key does not.
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

    /// The server to use. An explicit address wins. An install that already has
    /// a device id keeps the server it registered with, so an update does not
    /// take it offline. A fresh install has neither, and waits for sign-in.
    pub fn effective_server(&self) -> String {
        if let Some(s) = self.server.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return s.to_string();
        }
        if self.device_id.is_some() {
            return pjdesk_proto::LEGACY_REGISTERED_SERVER.to_string();
        }
        String::new()
    }

    /// Advertised routes, or the older LAN and public fields when the catalog has no IP.
    /// A saved hostname stays last.
    pub fn dial_routes(&self) -> pjdesk_proto::routes::ServerRoutes {
        let standby = self.network_standby.clone().unwrap_or_default();
        let standby_public = self.network_routes.standby_public.first().cloned().unwrap_or_default();
        pjdesk_proto::routes::routes_with_legacy(
            &self.network_routes,
            &self.effective_server(),
            &self.network_lan,
            &self.network_public_ip,
            &standby,
            &self.network_routes.standby_lan,
            &standby_public,
        )
    }

    pub fn network_access_token(&self) -> String {
        self.network_access.as_deref().and_then(secure::reveal).filter(|s| !s.is_empty()).unwrap_or_default()
    }

    pub fn server_access_key(&self) -> String {
        self.server_access_key.as_deref().and_then(secure::reveal).filter(|s| !s.is_empty()).unwrap_or_default()
    }

    pub fn set_server_access_key(&mut self, key: &str) {
        let key = key.trim();
        self.server_access_key = if key.is_empty() { None } else { Some(secure::protect_machine(key)) };
    }

    /// What this computer sends before Register or Connect.
    /// A server access key wins over the account token.
    pub fn presented_access(&self) -> String {
        let key = self.server_access_key();
        if !key.is_empty() {
            return key;
        }
        self.network_access_token()
    }

    /// Copies a key saved by Settings into this in-memory config.
    /// Returns true when it changed, so the host can register again.
    pub fn reload_access_key(&mut self) -> bool {
        let Ok(fresh) = Self::load() else { return false };
        if fresh.server_access_key == self.server_access_key {
            return false;
        }
        self.server_access_key = fresh.server_access_key;
        true
    }

    pub fn network_refresh_token(&self) -> String {
        self.network_refresh.as_deref().and_then(secure::reveal).filter(|s| !s.is_empty()).unwrap_or_default()
    }

    /// Ensures this install has a stable id to put in a network access token.
    pub fn network_device(&mut self) -> String {
        if let Some(id) = self.network_device_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return id.to_string();
        }
        let id = hex::encode(pjdesk_proto::random_bytes::<16>());
        self.network_device_id = Some(id.clone());
        id
    }

    pub fn apply_network_session(&mut self, session: &crate::network::Session) {
        self.network_email = Some(session.email.clone());
        if !session.refresh.is_empty() {
            self.network_refresh = Some(secure::protect_machine(&session.refresh));
        }
        if !session.access.is_empty() {
            self.network_access = Some(secure::protect(&session.access));
        }
        self.network_access_exp = session.exp;
        if !session.device_id.is_empty() {
            self.network_device_id = Some(session.device_id.clone());
        }
        if !session.role.is_empty() {
            self.network_role = session.role.clone();
        }
        if let Some(info) = &session.support {
            self.support_grant = Some(SupportGrant {
                owner: info.owner.clone(),
                label: info.label.clone(),
                key_id: info.key_id.clone(),
                unattended: info.unattended,
                consented: info.unattended,
            });
        }
        if !session.address.trim().is_empty() {
            let next = pjdesk_proto::failover::follow_primary(self.server.as_deref().unwrap_or(""), session.address.trim());
            if !next.is_empty() {
                self.server = Some(next);
            }
        }
        self.network_lan = session.lan.clone();
        self.network_public_ip = session.public_ip.clone();
        // A refresh that only lists the current primary must not erase the
        // other server. Re-registering after that primary stops has to work
        // from the addresses already saved.
        self.network_routes = pjdesk_proto::routes::keep_other_server(&self.network_routes, &session.routes);
        if self.network_routes.has_advertised_ip() {
            crate::routes_store::store(&self.network_routes);
        }
        if let Ok(fp) = parse_fingerprint(&session.fingerprint) {
            self.server_fingerprint = Some(pjdesk_proto::tls::format_fingerprint(&fp));
        }
        if !session.standby_address.trim().is_empty() {
            self.network_standby = Some(session.standby_address.trim().to_string());
        }
        if let Ok(fp) = parse_fingerprint(&session.standby_fingerprint) {
            self.network_standby_fingerprint = Some(pjdesk_proto::tls::format_fingerprint(&fp));
        }
        // A device that has not chosen to sync keeps its local list. The
        // account's rows are not merged, and `network_list_merged` stays false
        // so the next launch does not treat an empty list as "already syncing".
        if self.saved_sync != Some(false) {
            for id in &session.removed {
                self.forget_account_computer(id);
            }
            for computer in &session.computers {
                let id = pjdesk_proto::normalize_id(&computer.id);
                let still = session.removed.iter().any(|r| pjdesk_proto::normalize_id(r) == id);
                if !computer.hidden && !still {
                    self.account_removed.retain(|h| h != &id);
                }
            }
            let first = !self.network_list_merged;
            self.merge_computers(&session.computers, first);
            self.merge_presence(&session.presence);
            if !session.email.trim().is_empty() || !session.refresh.is_empty() {
                self.network_list_merged = true;
            }
        }
    }

    /// This device downloads the account's saved computers and their passwords.
    pub fn syncs_saved_computers(&self) -> bool {
        self.saved_sync == Some(true)
    }

    /// Existing installs that already show a saved list keep syncing.
    /// An empty computer that has never merged stays undecided.
    /// Returns true when the caller should save.
    pub fn decide_saved_sync_on_load(&mut self) -> bool {
        if self.saved_sync.is_some() {
            return false;
        }
        if self.local_saved_count() == 0 && !self.network_list_merged {
            return false;
        }
        self.saved_sync = Some(true);
        true
    }

    /// A sign-in downloads the account list only when this device already syncs.
    /// Otherwise the choice is remembered as no, and the list stays as it is.
    pub fn prepare_sign_in_saved_sync(&mut self) {
        if self.syncs_saved_computers() {
            self.allow_saved_merge();
        } else {
            self.saved_sync = Some(false);
        }
    }

    /// Drops the pjdesk.com sign-in. The saved list, nicknames, stars, order,
    /// and per-computer passwords stay on this computer. Syncing stops.
    pub fn app_lock_for(&self, email: &str) -> Option<pjdesk_proto::applock::Verifier> {
        let email = email.trim().to_lowercase();
        if email.is_empty() || !self.app_lock_email.eq_ignore_ascii_case(&email) {
            return None;
        }
        let stored = self.app_lock_verifier.as_deref()?;
        let text = secure::reveal(stored)?;
        serde_json::from_str(&text).ok()
    }

    pub fn write_app_lock(&mut self, email: &str, sealed: &str, updated: u64, verifier: Option<&pjdesk_proto::applock::Verifier>) {
        self.app_lock_email = email.trim().to_lowercase();
        self.app_lock_sealed = sealed.to_string();
        self.app_lock_updated = updated;
        self.app_lock_verifier = verifier.and_then(|v| serde_json::to_string(v).ok()).map(|text| secure::protect(&text));
        if verifier.is_none() {
            self.app_lock_fails = 0;
            self.app_lock_until = 0;
        }
    }

    pub fn clear_app_lock(&mut self) {
        self.app_lock_email.clear();
        self.app_lock_sealed.clear();
        self.app_lock_updated = 0;
        self.app_lock_verifier = None;
        self.app_lock_fails = 0;
        self.app_lock_until = 0;
        self.app_lock_pending = false;
    }

    pub fn note_app_lock_fail(&mut self, fails: u32, until: u64) {
        self.app_lock_fails = fails;
        self.app_lock_until = until;
    }

    pub fn clear_app_lock_fails(&mut self) {
        self.app_lock_fails = 0;
        self.app_lock_until = 0;
    }

    /// A code, a support key, a network key, or a server access key.
    /// The account email can still be known. Those computers are not asked
    /// for the account password or a blocking PIN.
    pub fn joined_by_code_or_key(&self) -> bool {
        if self.joined_without_password {
            return true;
        }
        if self.support_grant.is_some() {
            return true;
        }
        if self.network_key.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            return true;
        }
        if self.server_access_key.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            return true;
        }
        if self.network_role == "support" {
            return true;
        }
        let email = self.network_email.as_deref().unwrap_or("").trim();
        let refresh = self.network_refresh.as_deref().is_some_and(|s| !s.trim().is_empty());
        refresh && email.is_empty()
    }

    fn has_saved_pin_material(&self) -> bool {
        self.app_lock_verifier.as_deref().is_some_and(|s| !s.is_empty()) || !self.app_lock_sealed.trim().is_empty()
    }

    /// A session that already existed, with no PIN, was the stuck Set a PIN screen.
    /// Record that once so the next launch does not show it again.
    /// Returns true when the config should be saved.
    pub fn release_stuck_pin_prompt(&mut self) -> bool {
        let had_session = self.network_refresh.as_deref().is_some_and(|s| !s.trim().is_empty());
        let (declined, save) = pjdesk_proto::applock::pin_prompt_on_launch(
            &self.pin_prompt,
            self.pin_declined,
            had_session,
            self.has_saved_pin_material(),
        );
        if !save {
            return false;
        }
        self.pin_declined = declined;
        self.pin_prompt = pjdesk_proto::applock::PIN_PROMPT_VERSION.into();
        true
    }

    /// The next sign-in may fill the saved list from the account again.
    pub fn allow_saved_merge(&mut self) {
        self.local_saved_cleared = false;
    }

    /// Saved computers currently on this computer. Hidden and account-deleted
    /// rows are not counted.
    pub fn local_saved_count(&self) -> usize {
        self.address_book
            .iter()
            .filter(|e| {
                pjdesk_proto::is_valid_id(&e.id)
                    && !self.is_saved_hidden(&e.id)
                    && !self.account_removed.iter().any(|h| h == &e.id)
            })
            .count()
    }

    /// Drops the saved list, cached passwords, recents, and saved peer
    /// addresses on this computer. The account, device id, and registration
    /// stay. Nothing is marked hidden, so other devices are not affected.
    /// A refresh does not put the account list back until the next sign-in.
    pub fn clear_local_saved(&mut self) {
        self.address_book.clear();
        self.saved_passwords.clear();
        self.password_vault_times.clear();
        self.settings.peers.clear();
        self.settings.direct_pins.clear();
        self.local_saved_cleared = true;
    }

    /// Signs out on this computer. Saved computers, cached passwords, saved
    /// addresses, and recents go with the sign-in. The pjdesk.com account is
    /// not changed, and this computer keeps its ID.
    pub fn sign_out_on_this_computer(&mut self) {
        self.clear_local_saved();
        self.saved_sync = Some(false);
        self.leave_network_sign_in();
    }

    pub fn leave_network_sign_in(&mut self) {
        self.network_email = None;
        self.network_refresh = None;
        self.network_access = None;
        self.network_access_exp = 0;
        self.network_list_merged = false;
        self.password_vault_key = None;
        self.joined_without_password = false;
        self.support_grant = None;
        if self.network_role == "support" {
            self.network_role.clear();
        }
    }

    /// Ben's signed-in computer can create and revoke support keys.
    /// A computer that joined with a key cannot.
    pub fn can_manage_support_keys(&self) -> bool {
        self.support_grant.is_none()
            && !self.network_refresh_token().is_empty()
            && self.network_role != "member"
            && self.network_role != "support"
    }

    pub fn support_banner(&self) -> String {
        self.support_grant.as_ref().map(|grant| crate::support_keys::banner(&grant.owner)).unwrap_or_default()
    }

    /// Unattended support access, after the client ticked the consent box.
    pub fn support_unattended(&self) -> bool {
        self.support_grant.as_ref().is_some_and(|grant| grant.unattended && grant.consented)
    }

    pub(crate) fn password_vault_times(&self) -> &BTreeMap<String, u64> {
        &self.password_vault_times
    }

    pub(crate) fn saved_password_ids(&self) -> Vec<String> {
        self.saved_passwords.keys().cloned().collect()
    }

    /// Stores a password or a deletion at a known time, without treating the
    /// write as a new local edit.
    pub fn set_saved_password_at(&mut self, id: &str, password: &str, updated: u64) {
        let id = pjdesk_proto::normalize_id(id);
        if !pjdesk_proto::is_valid_id(&id) {
            return;
        }
        if password.is_empty() {
            self.saved_passwords.remove(&id);
        } else {
            self.saved_passwords.insert(id.clone(), secure::protect(password));
        }
        self.password_vault_times.insert(id, updated);
    }

    pub fn is_saved_hidden(&self, id: &str) -> bool {
        let id = pjdesk_proto::normalize_id(id);
        self.saved_hidden.iter().any(|h| h == &id)
    }

    /// Removes a computer from the local Saved list and remembers the hide.
    /// The password on this computer is dropped with it. Nothing here is uploaded
    /// until the caller syncs.
    pub fn hide_saved(&mut self, id: &str) {
        let id = pjdesk_proto::normalize_id(id);
        if !pjdesk_proto::is_valid_id(&id) {
            return;
        }
        self.address_book.retain(|e| e.id != id);
        self.saved_passwords.remove(&id);
        self.password_vault_times.insert(id.clone(), unix_now());
        if !self.saved_hidden.iter().any(|h| h == &id) {
            self.saved_hidden.push(id);
        }
    }

    /// Drops a computer the account deleted. The password on this computer goes
    /// with it. A later sign-in on that computer puts the row back.
    pub fn forget_account_computer(&mut self, id: &str) {
        let id = pjdesk_proto::normalize_id(id);
        if !pjdesk_proto::is_valid_id(&id) {
            return;
        }
        self.address_book.retain(|e| e.id != id);
        self.saved_passwords.remove(&id);
        self.password_vault_times.insert(id.clone(), unix_now());
        self.settings.peers.remove(&id);
        self.saved_hidden.retain(|h| h != &id);
        if !self.account_removed.iter().any(|h| h == &id) {
            self.account_removed.push(id);
        }
    }

    /// Adds computers the account's server has seen, including ones saved
    /// earlier with no name. A nickname already typed here is left as it is.
    /// An empty hostname or user does not erase one already stored.
    pub fn merge_presence(&mut self, rows: &[crate::network::HostPresence]) {
        if self.local_saved_cleared {
            return;
        }
        for row in rows {
            let id = pjdesk_proto::normalize_id(&row.id);
            if !pjdesk_proto::is_valid_id(&id) || self.account_removed.iter().any(|h| h == &id) {
                continue;
            }
            let note = self.settings.peers.entry(id.clone()).or_default();
            if !row.hostname.trim().is_empty() {
                note.hostname = row.hostname.trim().to_string();
            }
            if !row.os_family.trim().is_empty() {
                note.os_family = row.os_family.trim().to_string();
            }
            if !row.os_version.trim().is_empty() {
                note.os_version = row.os_version.trim().to_string();
            }
            if !row.console_user.trim().is_empty() {
                note.console_user = row.console_user.trim().to_string();
            }
            if let Some(name) = &row.display_name {
                note.display_name = name.trim().chars().filter(|c| !c.is_control()).take(64).collect();
            }
            if row.last_seen > note.last_seen {
                note.last_seen = row.last_seen;
            }
            if !row.app_version.trim().is_empty() {
                note.app_version = row.app_version.trim().to_string();
            }
            note.reported_online = row.online;
            if self.is_saved_hidden(&id) || self.account_removed.iter().any(|h| h == &id) {
                continue;
            }
            if !self.address_book.iter().any(|e| e.id == id) {
                self.address_book.push(AddressEntry { id, alias: String::new(), ..Default::default() });
            }
        }
    }

    /// Adds the account's saved computers. Local passwords are left as they are.
    ///
    /// `prefer_account` is the first sign-in merge: the same computer keeps one
    /// row, and the account's nickname wins when both sides have one. Later
    /// refreshes leave a nickname typed on this computer alone.
    pub fn merge_computers(&mut self, computers: &[crate::network::SavedComputer], prefer_account: bool) {
        if self.local_saved_cleared && !prefer_account {
            return;
        }
        if prefer_account {
            self.local_saved_cleared = false;
        }
        for computer in computers {
            let id = pjdesk_proto::normalize_id(&computer.id);
            if !pjdesk_proto::is_valid_id(&id) {
                continue;
            }
            if self.account_removed.iter().any(|h| h == &id) {
                continue;
            }
            if computer.hidden {
                self.address_book.retain(|e| e.id != id);
                if !self.saved_hidden.iter().any(|h| h == &id) {
                    self.saved_hidden.push(id);
                }
                continue;
            }
            if prefer_account {
                self.saved_hidden.retain(|h| h != &id);
            }
            if self.is_saved_hidden(&id) {
                continue;
            }
            let note = self.settings.peers.entry(id.clone()).or_default();
            if note.hostname.trim().is_empty() && !computer.hostname.trim().is_empty() {
                note.hostname = computer.hostname.trim().to_string();
            }
            if note.os_family.is_empty() {
                note.os_family = computer.os_family.clone();
            }
            if note.os_version.is_empty() {
                note.os_version = computer.os_version.clone();
            }
            if !computer.display_name.trim().is_empty() {
                note.display_name = computer.display_name.trim().to_string();
            }
            if let Some(starred) = computer.starred {
                note.favorite = starred;
            }
            if let Some(position) = computer.position {
                note.sort_order = position;
            }
            let known_host = note.hostname.clone();
            let incoming = nickname_text(&computer.name, &id, &known_host);
            if let Some(entry) = self.address_book.iter_mut().find(|e| e.id == id) {
                entry.password = None;
                if computer.group == crate::support_keys::CLIENTS && !crate::support_keys::is_clients(&entry.tags) {
                    entry.tags.push(crate::support_keys::CLIENTS.into());
                }
                let local = nickname_text(&entry.alias, &id, &known_host);
                if prefer_account && !incoming.is_empty() {
                    entry.alias = incoming;
                } else if local.is_empty() {
                    if !incoming.is_empty() {
                        entry.alias = incoming;
                    } else if same_id(&entry.alias, &id) {
                        entry.alias.clear();
                    }
                }
            } else if !incoming.is_empty()
                || !known_host.is_empty()
                || !computer.os_family.is_empty()
                || computer.starred.is_some()
                || computer.group == crate::support_keys::CLIENTS
            {
                self.address_book.push(AddressEntry {
                    id: id.clone(),
                    alias: incoming,
                    password: None,
                    tags: if computer.group == crate::support_keys::CLIENTS {
                        vec![crate::support_keys::CLIENTS.into()]
                    } else {
                        Vec::new()
                    },
                });
            }
        }
    }

    /// Re-reads the device identity and pinned fingerprint from disk. The host
    /// thread assigns and saves these; calling this before [`Self::save`] keeps
    /// the GUI from clobbering them when it writes settings.
    pub fn reload_identity_from_disk(&mut self) {
        if let Ok(disk) = Self::load() {
            self.fill_peer_facts_from(&disk);
            if disk.device_id.is_some() {
                self.device_id = disk.device_id;
            }
            if disk.device_secret.is_some() {
                self.device_secret = disk.device_secret;
            }
            if disk.server_fingerprint.is_some() {
                self.server_fingerprint = disk.server_fingerprint;
            }
            if disk.network_refresh.is_some() {
                self.network_email = disk.network_email;
                self.network_refresh = disk.network_refresh;
                self.network_access = disk.network_access;
                self.network_access_exp = disk.network_access_exp;
                self.network_device_id = disk.network_device_id;
                self.network_standby = disk.network_standby;
                self.network_standby_fingerprint = disk.network_standby_fingerprint;
            }
            if self.network_key.as_deref().map(str::trim).unwrap_or("").is_empty() {
                if disk.network_key.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                    self.network_key = disk.network_key;
                }
            }
        }
    }

    // --- Remembered passwords for direct connections -----------------------

    /// Remembers (encrypted) or, with `None`, forgets a device's password.
    pub fn remember_password(&mut self, id: &str, password: Option<&str>) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(1);
        let password = password.unwrap_or("");
        self.set_saved_password_at(id, password, now);
    }

    /// A remembered password for `id`. The encrypted local copy wins. A password
    /// left in the address book by an older build is used only when there is
    /// no local copy.
    pub fn saved_password(&self, id: &str) -> Option<String> {
        if let Some(pw) = self.saved_passwords.get(id).and_then(|s| secure::reveal(s)) {
            return Some(pw);
        }
        self.address_book.iter().find(|e| e.id == id).and_then(|e| e.password.clone()).filter(|p| !p.is_empty())
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
    fn a_code_join_and_an_older_session_skip_the_pin_screen() {
        let mut c = Config::default();
        assert!(!c.joined_by_code_or_key());
        assert!(c.release_stuck_pin_prompt());
        assert!(!c.pin_declined);
        assert_eq!(c.pin_prompt, "0.6.47");
        assert!(!c.release_stuck_pin_prompt());

        let mut stuck = Config::default();
        stuck.network_refresh = Some("plain:refresh".into());
        assert!(stuck.joined_by_code_or_key());
        assert!(stuck.release_stuck_pin_prompt());
        assert!(stuck.pin_declined);

        let mut kept = Config::default();
        kept.network_refresh = Some("plain:refresh".into());
        kept.network_email = Some("ben@spader.us".into());
        kept.app_lock_sealed = "blob".into();
        assert!(!kept.joined_by_code_or_key());
        assert!(kept.release_stuck_pin_prompt());
        assert!(!kept.pin_declined);

        let mut keyed = Config::default();
        keyed.network_email = Some("ben@spader.us".into());
        keyed.network_key = Some("pjdesk-net1:abc".into());
        assert!(keyed.joined_by_code_or_key());
        keyed.network_key = None;
        keyed.server_access_key = Some("sealed".into());
        assert!(keyed.joined_by_code_or_key());
        keyed.server_access_key = None;
        keyed.network_role = "support".into();
        assert!(keyed.joined_by_code_or_key());

        let mut invite = Config::default();
        invite.network_refresh = Some("plain:refresh".into());
        invite.network_email = Some("ben@spader.us".into());
        assert!(!invite.joined_by_code_or_key());
        invite.joined_without_password = true;
        assert!(invite.joined_by_code_or_key());
        assert!(!pjdesk_proto::applock::should_ask_for_pin(true, false, invite.joined_by_code_or_key(), false));
        assert_eq!(
            pjdesk_proto::applock::connected_confirmation(invite.network_email.as_deref().unwrap_or(""), "pjdesk.spader.us"),
            "You are now connected to ben@spader.us."
        );
    }

    #[test]
    fn remembered_password_roundtrips() {
        let mut c = Config::default();
        c.remember_password("123456789", Some("s3cret"));
        assert_eq!(c.saved_password("123456789").as_deref(), Some("s3cret"));
        c.remember_password("123456789", None);
        assert_eq!(c.saved_password("123456789"), None);
    }

    #[test]
    fn local_password_beats_a_plaintext_address_book_copy() {
        let mut c = Config::default();
        c.remember_password("111222333", Some("local"));
        c.address_book =
            vec![AddressEntry { id: "111222333".into(), password: Some("synced".into()), ..Default::default() }];
        assert_eq!(c.saved_password("111222333").as_deref(), Some("local"));
        let stored = c.saved_passwords.get("111222333").unwrap();
        assert_ne!(stored, "local");
        assert!(stored.starts_with("dpapi:") || stored.starts_with("plain:"));
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("\"local\""));
    }

    #[test]
    fn legacy_address_book_password_still_opens() {
        let mut c = Config::default();
        c.address_book =
            vec![AddressEntry { id: "111222333".into(), password: Some("synced".into()), ..Default::default() }];
        assert_eq!(c.saved_password("111222333").as_deref(), Some("synced"));
    }

    #[test]
    fn peer_note_keeps_os_and_view_mode() {
        let mut note = PeerNote::default();
        note.os_family = "linux".into();
        note.os_version = "24.04".into();
        note.view_mode = 1;
        note.last_display = 2;
        let json = serde_json::to_string(&note).unwrap();
        let back: PeerNote = serde_json::from_str(&json).unwrap();
        assert_eq!(back, note);
        let old: PeerNote = serde_json::from_str(r#"{"favorite":true,"last_connected":3}"#).unwrap();
        assert!(old.favorite);
        assert!(old.os_family.is_empty());
        assert!(old.hostname.is_empty());
        assert_eq!(old.view_mode, 0);
    }

    #[test]
    fn a_nickname_is_not_the_id_or_the_hostname() {
        assert_eq!(nickname_text("386 626 882", "386626882", "BEN-PC"), "");
        assert_eq!(nickname_text("BEN-PC", "386626882", "ben-pc"), "");
        assert_eq!(nickname_text("Front desk", "386626882", "BEN-PC"), "Front desk");
        assert_eq!(nickname_text("  ", "386626882", "BEN-PC"), "");
    }

    #[test]
    fn a_save_keeps_the_hostname_and_os_the_viewer_wrote() {
        let mut memory = Config::default();
        memory.settings.peers.insert(
            "123456789".into(),
            PeerNote { last_connected: 50, ..Default::default() },
        );
        let mut disk = Config::default();
        disk.settings.peers.insert(
            "123456789".into(),
            PeerNote {
                hostname: "BEN-PC".into(),
                os_family: "windows".into(),
                os_version: "11".into(),
                last_connected: 10,
                ..Default::default()
            },
        );
        memory.fill_peer_facts_from(&disk);
        let note = memory.settings.peers.get("123456789").unwrap();
        assert_eq!(note.hostname, "BEN-PC");
        assert_eq!(note.os_family, "windows");
        assert_eq!(note.os_version, "11");
        assert_eq!(note.last_connected, 50);

        let mut typed = Config::default();
        typed.settings.peers.insert(
            "123456789".into(),
            PeerNote { hostname: "OFFICE-PC".into(), os_family: "windows".into(), ..Default::default() },
        );
        let empty = Config::default();
        typed.fill_peer_facts_from(&empty);
        assert_eq!(typed.settings.peers.get("123456789").unwrap().hostname, "OFFICE-PC");

        typed.take_peer_reports_from(&disk);
        let note = typed.settings.peers.get("123456789").unwrap();
        assert_eq!(note.hostname, "BEN-PC");
        assert_eq!(note.os_version, "11");
    }

    #[test]
    fn merge_keeps_a_nickname_and_fills_an_empty_hostname() {
        let mut c = Config::default();
        c.address_book = vec![AddressEntry { id: "123456789".into(), alias: "Front desk".into(), ..Default::default() }];
        c.merge_computers(
            &[crate::network::SavedComputer {
                id: "123456789".into(),
                name: "Other name".into(),
                hostname: "OFFICE-PC".into(),
                os_family: "windows".into(),
                os_version: "11".into(),
                ..Default::default()
            }],
            false,
        );
        assert_eq!(c.address_book[0].alias, "Front desk");
        assert_eq!(c.settings.peers.get("123456789").unwrap().hostname, "OFFICE-PC");

        let mut clients = Config::default();
        clients.merge_computers(
            &[crate::network::SavedComputer {
                id: "902488771".into(),
                name: "Acme".into(),
                hostname: "ACME-PC".into(),
                group: "Clients".into(),
                ..Default::default()
            }],
            true,
        );
        assert_eq!(clients.address_book[0].alias, "Acme");
        assert!(crate::support_keys::is_clients(&clients.address_book[0].tags));
        assert_eq!(crate::support_keys::banner("Ben Spader"), "Ben Spader (support) has access");

        let mut fresh = Config::default();
        fresh.merge_computers(
            &[crate::network::SavedComputer {
                id: "386626882".into(),
                name: "386 626 882".into(),
                hostname: "BEN-PC".into(),
                ..Default::default()
            }],
            false,
        );
        assert!(fresh.address_book.iter().all(|e| e.alias.is_empty()));
        assert_eq!(fresh.settings.peers.get("386626882").unwrap().hostname, "BEN-PC");
    }

    #[test]
    fn a_fresh_install_has_no_server_until_sign_in() {
        let fresh = Config::default();
        assert!(fresh.effective_server().is_empty());
        let mut registered = Config::default();
        registered.device_id = Some("123456789".into());
        assert_eq!(registered.effective_server(), pjdesk_proto::LEGACY_REGISTERED_SERVER);
        registered.server = Some("office.example:21200".into());
        assert_eq!(registered.effective_server(), "office.example:21200");
    }

    #[test]
    fn save_password_stays_on_this_computer() {
        let mut c = Config::default();
        c.remember_password("123456789", Some("correct horse"));
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("correct horse"));
        assert!(json.contains("dpapi:") || json.contains("plain:"));
        c.merge_computers(
            &[crate::network::SavedComputer {
                id: "123456789".into(),
                name: "Office".into(),
                hostname: String::new(),
                os_family: "windows".into(),
                os_version: "11".into(),
                ..Default::default()
            }],
            false,
        );
        assert_eq!(c.saved_password("123456789").as_deref(), Some("correct horse"));
        assert!(c.address_book.iter().all(|e| e.password.is_none()));
    }

    #[test]
    fn first_sign_in_merges_local_computers_and_prefers_the_account_nickname() {
        let mut c = Config::default();
        c.remember_password("111111111", Some("local-secret"));
        c.address_book = vec![
            AddressEntry { id: "111111111".into(), alias: "Local name".into(), ..Default::default() },
            AddressEntry { id: "222222222".into(), alias: "Only here".into(), ..Default::default() },
        ];
        c.settings.peers.insert("222222222".into(), PeerNote { favorite: true, sort_order: 20, ..Default::default() });
        c.merge_computers(
            &[
                crate::network::SavedComputer {
                    id: "111111111".into(),
                    name: "Account name".into(),
                    hostname: "CABINDESK".into(),
                    os_family: "windows".into(),
                    os_version: "11".into(),
                    display_name: String::new(),
                    starred: Some(true),
                    position: Some(5),
                    hidden: false,
                    group: String::new(),
                },
                crate::network::SavedComputer {
                    id: "333333333".into(),
                    name: "Pixel".into(),
                    hostname: "Pixel 11".into(),
                    os_family: "android".into(),
                    os_version: "16".into(),
                    ..Default::default()
                },
                crate::network::SavedComputer {
                    id: "444444444".into(),
                    name: "Old".into(),
                    hidden: true,
                    ..Default::default()
                },
            ],
            true,
        );
        assert_eq!(c.address_book.iter().find(|e| e.id == "111111111").unwrap().alias, "Account name");
        assert_eq!(c.address_book.iter().find(|e| e.id == "222222222").unwrap().alias, "Only here");
        assert_eq!(c.address_book.iter().find(|e| e.id == "333333333").unwrap().alias, "Pixel");
        assert!(c.address_book.iter().all(|e| e.id != "444444444"));
        assert!(c.is_saved_hidden("444444444"));
        assert_eq!(c.settings.peers.get("111111111").unwrap().favorite, true);
        assert_eq!(c.settings.peers.get("111111111").unwrap().sort_order, 5);
        assert_eq!(c.settings.peers.get("222222222").unwrap().sort_order, 20);
        assert_eq!(c.saved_password("111111111").as_deref(), Some("local-secret"));
        assert!(c.address_book.iter().all(|e| e.password.is_none()));

        let mut later = c.clone();
        later.address_book.iter_mut().find(|e| e.id == "111111111").unwrap().alias = "Typed here".into();
        later.merge_computers(
            &[crate::network::SavedComputer {
                id: "111111111".into(),
                name: "Account name".into(),
                hostname: "CABINDESK".into(),
                ..Default::default()
            }],
            false,
        );
        assert_eq!(later.address_book.iter().find(|e| e.id == "111111111").unwrap().alias, "Typed here");

        c.leave_network_sign_in();
        assert!(c.network_email.is_none());
        assert!(c.network_refresh.is_none());
        assert!(!c.network_list_merged);
        assert_eq!(c.address_book.len(), 3);
        assert_eq!(c.saved_password("111111111").as_deref(), Some("local-secret"));
        assert!(c.settings.peers.get("222222222").unwrap().favorite);
    }

    #[test]
    fn a_local_remove_stays_and_an_account_delete_does_not_come_back() {
        let mut c = Config::default();
        c.address_book = vec![AddressEntry { id: "333333333".into(), alias: "Pixel".into(), ..Default::default() }];
        c.remember_password("333333333", Some("phone-secret"));
        c.remember_password("692996394", Some("desk-secret"));
        c.settings.peers.insert("692996394".into(), PeerNote { hostname: "BENLENOVODESK".into(), ..Default::default() });
        c.hide_saved("333333333");
        c.merge_computers(
            &[crate::network::SavedComputer {
                id: "333333333".into(),
                name: "Pixel".into(),
                hostname: "Pixel 11".into(),
                ..Default::default()
            }],
            false,
        );
        assert!(c.is_saved_hidden("333333333"));
        assert!(c.address_book.iter().all(|e| e.id != "333333333"));
        assert_eq!(c.saved_password("333333333"), None);
        c.forget_account_computer("692996394");
        c.merge_presence(&[crate::network::HostPresence {
            id: "692996394".into(),
            hostname: "BENLENOVODESK".into(),
            os_family: "windows".into(),
            os_version: "11".into(),
            console_user: String::new(),
            display_name: None,
            last_seen: 50,
            online: false,
            app_version: String::new(),
        }]);
        assert!(c.address_book.iter().all(|e| e.id != "692996394"));
        assert!(c.settings.peers.get("692996394").is_none());
        assert_eq!(c.saved_password("692996394"), None);
    }

    #[test]
    fn presence_fills_a_blank_row_and_keeps_the_nickname_and_user() {
        let mut c = Config::default();
        c.address_book = vec![AddressEntry { id: "386626882".into(), alias: "Pixel".into(), ..Default::default() }];
        c.settings.peers.insert(
            "386626882".into(),
            PeerNote { console_user: "Ben".into(), hostname: "Pixel 11".into(), ..Default::default() },
        );
        c.merge_presence(&[
            crate::network::HostPresence {
                id: "386 626 882".into(),
                hostname: String::new(),
                os_family: "android".into(),
                os_version: "16".into(),
                console_user: String::new(),
                display_name: Some("Pocket".into()),
                last_seen: 20,
                online: true,
                app_version: String::new(),
            },
            crate::network::HostPresence {
                id: "692996394".into(),
                hostname: "BENLENOVODESK".into(),
                os_family: "windows".into(),
                os_version: "11".into(),
                console_user: "SPADER\\Ben".into(),
                display_name: None,
                last_seen: 50,
                online: false,
                app_version: "0.6.43".into(),
            },
        ]);
        assert_eq!(c.address_book.iter().find(|e| e.id == "386626882").unwrap().alias, "Pixel");
        let pixel = c.settings.peers.get("386626882").unwrap();
        assert_eq!(pixel.hostname, "Pixel 11");
        assert_eq!(pixel.display_name, "Pocket");
        assert_eq!(pixel.console_user, "Ben");
        assert_eq!(pixel.os_family, "android");
        assert!(pixel.reported_online);
        let desk = c.settings.peers.get("692996394").unwrap();
        assert_eq!(desk.hostname, "BENLENOVODESK");
        assert_eq!(desk.console_user, "SPADER\\Ben");
        assert_eq!(desk.app_version, "0.6.43");
        assert!(c.address_book.iter().any(|e| e.id == "692996394" && e.alias.is_empty()));
    }

    #[test]
    fn presence_online_follows_the_live_flag_not_last_connected() {
        let mut c = Config::default();
        c.settings.peers.insert(
            "902488771".into(),
            PeerNote {
                last_connected: 1_700_000_000,
                last_seen: 100,
                reported_online: true,
                ..Default::default()
            },
        );
        c.merge_presence(&[crate::network::HostPresence {
            id: "902488771".into(),
            hostname: "CABINDESK".into(),
            os_family: "windows".into(),
            os_version: "11".into(),
            console_user: String::new(),
            display_name: None,
            last_seen: 100,
            online: false,
            app_version: String::new(),
        }]);
        let note = c.settings.peers.get("902488771").unwrap();
        assert!(!note.reported_online);
        assert_eq!(note.last_seen, 100);
        assert_eq!(note.last_connected, 1_700_000_000);
        c.merge_presence(&[crate::network::HostPresence {
            id: "610978221".into(),
            hostname: "CHIP20".into(),
            os_family: "windows".into(),
            os_version: "11".into(),
            console_user: String::new(),
            display_name: None,
            last_seen: 40,
            online: true,
            app_version: String::new(),
        }]);
        let chip = c.settings.peers.get("610978221").unwrap();
        assert!(chip.reported_online);
        assert_eq!(chip.last_connected, 0);
    }

    #[test]
    fn a_temporary_sign_in_does_not_write_the_account() {
        let dir = std::env::temp_dir().join(format!("pjdesk-temp-sign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.json");
        let mut before = Config::default();
        before.address_book =
            vec![AddressEntry { id: "111111111".into(), alias: "Friend".into(), ..Default::default() }];
        before.server = Some("friend.example:21200".into());
        before.save_to(&path).unwrap();

        let mut live = before.clone();
        live.ephemeral_network = true;
        live.network_email = Some("ben@spader.us".into());
        live.network_refresh = Some("plain:session-token".into());
        live.network_access = Some("plain:access-token".into());
        live.address_book.push(AddressEntry { id: "692996394".into(), alias: "Office".into(), ..Default::default() });
        live.server = Some("pjdesk.spader.us:21200".into());
        live.device_id = Some("386626882".into());
        live.save_to(&path).unwrap();

        let disk = Config::load_from(&path).unwrap();
        assert!(disk.network_email.is_none());
        assert!(disk.network_refresh.is_none());
        assert!(disk.network_access.is_none());
        assert_eq!(disk.address_book.len(), 1);
        assert_eq!(disk.address_book[0].alias, "Friend");
        assert_eq!(disk.server.as_deref(), Some("friend.example:21200"));
        assert_eq!(disk.device_id.as_deref(), Some("386626882"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("session-token"));
        assert!(!text.contains("ben@spader.us"));

        live.ephemeral_network = false;
        live.save_to(&path).unwrap();
        let kept = Config::load_from(&path).unwrap();
        assert_eq!(kept.network_email.as_deref(), Some("ben@spader.us"));
        assert_eq!(kept.network_refresh.as_deref(), Some("plain:session-token"));
        assert_eq!(kept.network_access.as_deref(), Some("plain:access-token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_local_clear_drops_this_computer_only_and_a_refresh_does_not_refill_it() {
        let mut c = Config::default();
        c.device_id = Some("386626882".into());
        c.network_email = Some("ben@spader.us".into());
        c.network_list_merged = true;
        c.saved_hidden = vec!["555555555".into()];
        c.address_book = vec![
            AddressEntry { id: "111111111".into(), alias: "Cabin".into(), ..Default::default() },
            AddressEntry { id: "222222222".into(), alias: "Lenovo".into(), ..Default::default() },
        ];
        c.remember_password("111111111", Some("cabin-secret"));
        c.settings.direct_pins.insert("192.168.1.20:21201".into(), "fp".into());
        c.settings.peers.insert(
            "111111111".into(),
            PeerNote {
                last_connected: 90,
                lan_addrs: vec!["192.168.1.20:21201".into()],
                host_fingerprint: "abc".into(),
                ..Default::default()
            },
        );
        assert_eq!(c.local_saved_count(), 2);
        c.clear_local_saved();
        assert_eq!(c.local_saved_count(), 0);
        assert!(c.address_book.is_empty());
        assert!(c.saved_passwords.is_empty());
        assert!(c.password_vault_times.is_empty());
        assert!(c.settings.peers.is_empty());
        assert!(c.settings.direct_pins.is_empty());
        assert!(c.local_saved_cleared);
        assert_eq!(c.saved_hidden, vec!["555555555".to_string()]);
        assert_eq!(c.device_id.as_deref(), Some("386626882"));
        assert_eq!(c.network_email.as_deref(), Some("ben@spader.us"));
        c.sign_out_on_this_computer();
        assert!(c.network_email.is_none());
        assert!(c.network_refresh.is_none());
        assert!(c.password_vault_key.is_none());
        assert!(c.address_book.is_empty());
        assert!(c.saved_passwords.is_empty());
        assert!(c.settings.peers.is_empty());
        assert_eq!(c.device_id.as_deref(), Some("386626882"));
        assert_eq!(c.saved_sync, Some(false));
        c.merge_computers(
            &[crate::network::SavedComputer {
                id: "111111111".into(),
                name: "Cabin".into(),
                hostname: "CABINDESK".into(),
                ..Default::default()
            }],
            false,
        );
        c.merge_presence(&[crate::network::HostPresence {
            id: "222222222".into(),
            hostname: "BENLENOVODESK".into(),
            os_family: "windows".into(),
            os_version: "11".into(),
            console_user: String::new(),
            display_name: None,
            last_seen: 50,
            online: true,
            app_version: String::new(),
        }]);
        assert!(c.address_book.is_empty());
        assert!(c.settings.peers.is_empty());
        assert!(c.local_saved_cleared);
        c.allow_saved_merge();
        c.network_list_merged = false;
        c.merge_computers(
            &[crate::network::SavedComputer {
                id: "111111111".into(),
                name: "Cabin".into(),
                hostname: "CABINDESK".into(),
                ..Default::default()
            }],
            true,
        );
        assert!(!c.local_saved_cleared);
        assert!(c.address_book.iter().any(|e| e.id == "111111111"));
    }

    #[test]
    fn a_temporary_save_writes_a_cleared_list_instead_of_the_one_on_disk() {
        let dir = std::env::temp_dir().join(format!("pjdesk-clear-temp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.json");
        let mut before = Config::default();
        before.address_book = vec![AddressEntry { id: "111111111".into(), alias: "Friend".into(), ..Default::default() }];
        before.saved_passwords.insert("111111111".into(), "kept".into());
        before.save_to(&path).unwrap();

        let mut live = before.clone();
        live.ephemeral_network = true;
        live.network_email = Some("guest@spader.us".into());
        live.clear_local_saved();
        live.save_to(&path).unwrap();

        let disk = Config::load_from(&path).unwrap();
        assert!(disk.address_book.is_empty());
        assert!(disk.saved_passwords.is_empty());
        assert!(disk.local_saved_cleared);
        assert!(disk.network_email.is_none());
        assert!(!disk.ephemeral_network);
        let _ = std::fs::remove_dir_all(&dir);
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

    fn session_with(id: &str, name: &str) -> crate::network::Session {
        crate::network::Session {
            email: "ben@spader.us".into(),
            access: "access".into(),
            exp: 0,
            refresh: "refresh".into(),
            device_id: "dev".into(),
            address: "pjdesk.spader.us:21200".into(),
            fingerprint: String::new(),
            standby_address: String::new(),
            standby_fingerprint: String::new(),
            lan: Vec::new(),
            public_ip: String::new(),
            standby_lan: Vec::new(),
            standby_public_ip: String::new(),
            routes: Default::default(),
            computers: vec![crate::network::SavedComputer {
                id: id.into(),
                name: name.into(),
                ..Default::default()
            }],
            presence: Vec::new(),
            removed: Vec::new(),
            vault_key: None,
            role: String::new(),
            support: None,
        }
    }

    #[test]
    fn an_existing_list_keeps_syncing_and_a_new_sign_in_does_not_download_it() {
        let mut existing = Config::default();
        existing.address_book = vec![AddressEntry { id: "111111111".into(), alias: "Cabin".into(), ..Default::default() }];
        assert!(existing.decide_saved_sync_on_load());
        assert_eq!(existing.saved_sync, Some(true));
        assert!(!existing.decide_saved_sync_on_load());
        assert_eq!(existing.local_saved_count(), 1);

        let mut merged = Config::default();
        merged.network_list_merged = true;
        assert!(merged.decide_saved_sync_on_load());
        assert_eq!(merged.saved_sync, Some(true));
        assert!(merged.address_book.is_empty());

        let mut fresh = Config::default();
        assert!(!fresh.decide_saved_sync_on_load());
        assert!(fresh.saved_sync.is_none());
        fresh.prepare_sign_in_saved_sync();
        assert_eq!(fresh.saved_sync, Some(false));
        fresh.apply_network_session(&session_with("333333333", "Pixel"));
        assert!(fresh.address_book.is_empty());
        assert!(!fresh.network_list_merged);
        assert_eq!(fresh.network_email.as_deref(), Some("ben@spader.us"));
        assert!(!fresh.network_refresh_token().is_empty());

        fresh.saved_sync = Some(true);
        fresh.network_list_merged = false;
        fresh.apply_network_session(&session_with("333333333", "Pixel"));
        assert!(fresh.address_book.iter().any(|e| e.id == "333333333"));
        assert!(fresh.network_list_merged);

        fresh.saved_sync = Some(true);
        fresh.clear_local_saved();
        assert_eq!(fresh.saved_sync, Some(true));
        assert!(fresh.address_book.is_empty());
    }
}
