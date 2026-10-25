//! Persistent device registry: which ID belongs to which device.
//!
//! Each host proves ownership of its ID with a 32-byte secret (since 0.3.1 it
//! is derived from the computer itself, so the app and the background service
//! share it). The server stores only the SHA-256 of that secret, in
//! `devices.json` inside the data folder. The secret is the device's identity:
//! one secret always maps to one ID, so a computer keeps a single record.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use pjdesk_proto::{format_id, generate_id, is_valid_id};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Default, Serialize, Deserialize)]
struct RegistryFile {
    devices: BTreeMap<String, DeviceRecord>,
    /// Computers Ben removed. The ID and the secret hash stay here so the same
    /// computer cannot register again, including under a new ID.
    #[serde(default)]
    blocked: BTreeMap<String, BlockedRecord>,
}

#[derive(Clone, Serialize, Deserialize)]
struct DeviceRecord {
    secret_sha256: String,
    hostname: String,
    #[serde(default)]
    app_version: String,
    first_seen: u64,
    last_seen: u64,
    /// A label the operator assigned ("Office", "Lab"). Empty if none.
    /// Missing in files written before 0.4.0.
    #[serde(default)]
    group: String,
    /// True after a registration that presented a valid network access token.
    /// Missing in files written before 0.6.0, which means not signed in.
    #[serde(default)]
    signed_in: bool,
    /// "windows", "android", or empty until the host reports it. Missing before 0.6.2.
    #[serde(default)]
    os_family: String,
    #[serde(default)]
    os_version: String,
    /// Last interactive user. Empty is not a replacement for a name already stored.
    /// Missing before 0.6.2. Shown only to the account owner, not to other peers.
    #[serde(default)]
    console_user: String,
    /// Friendly name chosen on the computer. Empty means show the hostname.
    /// An empty report clears it. Missing before the display-name report.
    #[serde(default)]
    display_name: String,
    /// Last address this computer registered or stayed connected from.
    /// Missing before 0.6.33.
    #[serde(default)]
    last_ip: String,
    /// Server access key that admitted this computer. Empty when it used an
    /// account or grace. Missing before 0.6.33. Not a secret.
    #[serde(default)]
    access_key_id: String,
}

/// A computer that was removed and must not register again.
#[derive(Clone, Serialize, Deserialize)]
struct BlockedRecord {
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    secret_sha256: String,
    #[serde(default)]
    last_ip: String,
    #[serde(default)]
    first_seen: u64,
    #[serde(default)]
    last_seen: u64,
    #[serde(default)]
    blocked_at: u64,
}

/// One computer for the account's saved list. The user name stays on this server
/// and is uploaded only to the owner's account.
pub struct PresenceRow {
    pub id: String,
    pub hostname: String,
    pub os_family: String,
    pub os_version: String,
    pub console_user: String,
    pub display_name: String,
    pub last_seen: u64,
    pub signed_in: bool,
}

/// A registered device, for the management window.
#[derive(Clone)]
pub struct DeviceRow {
    pub id: String,
    pub hostname: String,
    pub version: String,
    pub group: String,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub first_seen: u64,
    pub last_seen: u64,
    /// False until this computer registers with a network access token.
    pub signed_in: bool,
    /// Empty until this computer has connected once on 0.6.33 or later.
    pub last_ip: String,
    /// Set when this computer registered with a server access key.
    pub access_key_id: String,
}

/// A removed computer, for the Blocked computers list. No secret.
#[derive(Clone)]
pub struct BlockedRow {
    pub id: String,
    pub hostname: String,
    pub last_ip: String,
    pub first_seen: u64,
    pub last_seen: u64,
    pub blocked_at: u64,
}

pub struct Registry {
    path: PathBuf,
    data: RegistryFile,
}

#[derive(Debug)]
pub enum RegisterError {
    /// The ID exists and the secret does not match.
    Conflict(String),
    /// This computer was removed. The ID and the secret stay blocked.
    Blocked(String),
    Io(String),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegisterError::Conflict(id) => write!(f, "ID {} belongs to another device", format_id(id)),
            RegisterError::Blocked(id) => {
                if id.is_empty() {
                    write!(f, "this computer is blocked")
                } else {
                    write!(f, "this computer ({}) is blocked", format_id(id))
                }
            }
            RegisterError::Io(e) => write!(f, "could not save the device registry: {e}"),
        }
    }
}

impl std::error::Error for RegisterError {}

fn now() -> u64 {
    unix_now()
}

/// Unix seconds. Used by the signaling server as well as the registry.
pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn clip(text: &str, max: usize) -> String {
    text.trim().chars().filter(|c| !c.is_control()).take(max).collect()
}

/// True when this app version can decode [`pjdesk_proto::signal::ServerMsg::WantHostReport`].
///
/// Desktop 0.6.2 can. Android 0.6.3 cannot. A desktop that updated from 0.6.2
/// to 0.6.3 already has an operating system on its record, and that record is
/// not Android, so it is asked. A 0.6.3 with no operating system, or Android,
/// is not asked. Android 0.6.4 and every later build can.
#[cfg_attr(not(test), allow(dead_code))]
pub fn supports_host_report(app_version: &str) -> bool {
    supports_host_report_for(app_version, "")
}

pub fn supports_host_report_for(app_version: &str, os_family: &str) -> bool {
    let v = parse_version(app_version);
    if v == (0, 6, 2) || v >= (0, 6, 4) {
        return true;
    }
    v == (0, 6, 3) && !os_family.is_empty() && !os_family.eq_ignore_ascii_case("android")
}

fn parse_version(text: &str) -> (u32, u32, u32) {
    let text = text.trim().trim_start_matches('v');
    let mut parts = text.split('.');
    let major = parts.next().and_then(|s| leading_u32(s)).unwrap_or(0);
    let minor = parts.next().and_then(|s| leading_u32(s)).unwrap_or(0);
    let patch = parts.next().and_then(|s| leading_u32(s)).unwrap_or(0);
    (major, minor, patch)
}

fn leading_u32(text: &str) -> Option<u32> {
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() { None } else { digits.parse().ok() }
}

impl Registry {
    /// Loads the registry, starting an empty one if the file is missing,
    /// unreadable or corrupt.
    ///
    /// A missing file is normal (first run). An unreadable or corrupt file is
    /// not fatal either: the registry only maps device IDs to their secrets, so
    /// starting empty just means devices re-register on their next connect —
    /// far better than refusing to start. (An earlier permissions bug left
    /// `devices.json` unreadable and took the whole server down with it.)
    pub fn load(path: PathBuf) -> Result<Self> {
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("ignoring unreadable device registry {} ({e}); starting empty", path.display());
                RegistryFile::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RegistryFile::default(),
            Err(e) => {
                tracing::warn!("ignoring unreadable device registry {} ({e}); starting empty", path.display());
                RegistryFile::default()
            }
        };
        Ok(Self { path, data })
    }

    pub fn len(&self) -> usize {
        self.data.devices.len()
    }

    /// Rows for the management window, most recently seen first.
    pub fn list(&self) -> Vec<DeviceRow> {
        let mut rows: Vec<DeviceRow> = self
            .data
            .devices
            .iter()
            .map(|(id, d)| DeviceRow {
                id: id.clone(),
                hostname: d.hostname.clone(),
                version: d.app_version.clone(),
                group: d.group.clone(),
                first_seen: d.first_seen,
                last_seen: d.last_seen,
                signed_in: d.signed_in,
                last_ip: d.last_ip.clone(),
                access_key_id: d.access_key_id.clone(),
            })
            .collect();
        rows.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        rows
    }

    /// Forgets a device (the window's "Forget"). If that computer connects
    /// again it gets the same ID back (it still holds the ID and its secret).
    /// Sets the group label for a computer. An empty group clears it.
    /// The name is a short label, not a path.
    pub fn set_group(&mut self, id: &str, group: &str) -> Result<()> {
        let group = group.trim();
        if group.chars().count() > 40 || group.chars().any(|c| c.is_control() || c == '/' || c == '\\') {
            anyhow::bail!("a group name is at most 40 characters and cannot contain a path");
        }
        let record = self.data.devices.get_mut(id).context("no such computer")?;
        record.group = group.to_string();
        self.save()?;
        Ok(())
    }

    #[cfg(test)]
    pub fn group_of(&self, id: &str) -> Option<String> {
        self.data.devices.get(id).map(|d| d.group.clone())
    }

    /// Operating system last reported by this computer. Empty until a host report.
    pub fn os_family(&self, id: &str) -> String {
        self.data.devices.get(id).map(|d| d.os_family.clone()).unwrap_or_default()
    }

    /// How many computers are using this key, and whether `secret` is already one of them.
    pub fn key_has_room(&self, key_id: &str, secret: &[u8; 32], limit: u32) -> bool {
        let hash = hex::encode(Sha256::digest(secret));
        let limit = limit.max(1) as usize;
        let mut count = 0usize;
        let mut same = false;
        for record in self.data.devices.values() {
            if record.access_key_id == key_id {
                count += 1;
                if record.secret_sha256 == hash {
                    same = true;
                }
            }
        }
        if same { count <= limit } else { count + 1 <= limit }
    }

    /// Remembers or clears the access key on a registration. An empty id clears it.
    pub fn note_access_key(&mut self, id: &str, key_id: &str) {
        let Some(record) = self.data.devices.get_mut(id) else { return };
        if record.access_key_id == key_id {
            return;
        }
        record.access_key_id = key_id.to_string();
        if let Err(e) = self.save() {
            tracing::warn!("could not save the access key for {id}: {e}");
        }
    }

    /// Removes every computer that registered with this key. Does not block them.
    pub fn take_access_key(&mut self, key_id: &str) -> Vec<String> {
        let ids: Vec<String> = self
            .data
            .devices
            .iter()
            .filter(|(_, record)| record.access_key_id == key_id)
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            return ids;
        }
        for id in &ids {
            self.data.devices.remove(id);
        }
        if let Err(e) = self.save() {
            tracing::warn!("could not remove computers for an access key: {e}");
        }
        ids
    }

    /// Marks whether the latest registration presented a valid network token.
    pub fn mark_signed_in(&mut self, id: &str, signed_in: bool) {
        let Some(record) = self.data.devices.get_mut(id) else { return };
        if record.signed_in == signed_in {
            return;
        }
        record.signed_in = signed_in;
        if let Err(e) = self.save() {
            tracing::warn!("could not record sign-in state for {id}: {e}");
        }
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        if self.data.devices.remove(id).is_some() {
            self.save()?;
        }
        Ok(())
    }

    /// Rows for Blocked computers, most recently blocked first.
    pub fn blocked(&self) -> Vec<BlockedRow> {
        let mut rows: Vec<BlockedRow> = self
            .data
            .blocked
            .iter()
            .map(|(id, d)| BlockedRow {
                id: id.clone(),
                hostname: d.hostname.clone(),
                last_ip: d.last_ip.clone(),
                first_seen: d.first_seen,
                last_seen: d.last_seen,
                blocked_at: d.blocked_at,
            })
            .collect();
        rows.sort_by(|a, b| b.blocked_at.cmp(&a.blocked_at).then(a.id.cmp(&b.id)));
        rows
    }

    /// Disconnect is the caller's job. This deletes the registration and keeps
    /// the ID and the secret hash so the same computer cannot come back.
    pub fn block(&mut self, id: &str) -> Result<bool> {
        if !is_valid_id(id) {
            anyhow::bail!("that is not a PJDesk computer ID");
        }
        if let Some(device) = self.data.devices.remove(id) {
            self.data.blocked.insert(
                id.to_string(),
                BlockedRecord {
                    hostname: device.hostname,
                    secret_sha256: device.secret_sha256,
                    last_ip: device.last_ip,
                    first_seen: device.first_seen,
                    last_seen: device.last_seen,
                    blocked_at: now(),
                },
            );
            self.save()?;
            return Ok(true);
        }
        Ok(self.data.blocked.contains_key(id))
    }

    /// Computers that registered with neither a PJDesk account nor a server access key.
    /// Turning Require signed-in accounts on disconnects these if they are online.
    /// An access key stays connected.
    pub fn grace_ids(&self) -> Vec<String> {
        self.data
            .devices
            .iter()
            .filter(|(_, device)| !device.signed_in && device.access_key_id.is_empty())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Lets that computer register again. It is not put back on the list until it connects.
    pub fn unblock(&mut self, id: &str) -> Result<bool> {
        if !is_valid_id(id) {
            anyhow::bail!("that is not a PJDesk computer ID");
        }
        if self.data.blocked.remove(id).is_some() {
            self.save()?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Remembers the address a live computer is using. A repeat of the same address does not write.
    pub fn note_address(&mut self, id: &str, ip: &str) {
        let ip = ip.trim();
        if ip.is_empty() {
            return;
        }
        let Some(record) = self.data.devices.get_mut(id) else { return };
        if record.last_ip == ip {
            return;
        }
        record.last_ip = ip.to_string();
        if let Err(e) = self.save() {
            tracing::warn!("could not save the last address for {id}: {e}");
        }
    }

    fn secret_is_blocked(&self, secret_sha256: &str) -> bool {
        !secret_sha256.is_empty()
            && self.data.blocked.values().any(|row| !row.secret_sha256.is_empty() && row.secret_sha256 == secret_sha256)
    }

    /// Registers a device. Returns its ID.
    ///
    /// * Secret already known → that device's ID, whatever ID (if any) the
    ///   client asked for. Extra records with the same secret — older servers
    ///   handed out a fresh ID whenever a config file had none, so the app and
    ///   the background service could end up with different IDs — are merged
    ///   into the one kept (the requested ID if it is one of them, otherwise
    ///   the most recently seen).
    /// * Known ID + someone else's secret → [`RegisterError::Conflict`].
    /// * Unknown but well-formed ID → claimed (keeps IDs stable if the
    ///   server's data folder was lost, or the record was forgotten).
    /// * No ID → a fresh random nine-digit ID.
    pub fn register(
        &mut self,
        requested: Option<&str>,
        secret: &[u8; 32],
        hostname: &str,
        app_version: &str,
    ) -> Result<String, RegisterError> {
        let secret_sha256 = hex::encode(Sha256::digest(secret));
        let now = now();
        if self.secret_is_blocked(&secret_sha256) {
            return Err(RegisterError::Blocked(requested.unwrap_or("").to_string()));
        }
        if requested.is_some_and(|id| self.data.blocked.contains_key(id)) {
            return Err(RegisterError::Blocked(requested.unwrap_or("").to_string()));
        }

        let mut owned: Vec<(String, u64)> = self
            .data
            .devices
            .iter()
            .filter(|(_, d)| d.secret_sha256 == secret_sha256)
            .map(|(id, d)| (id.clone(), d.last_seen))
            .collect();
        if !owned.is_empty() {
            let keep = match requested.filter(|r| owned.iter().any(|(id, _)| id == r)) {
                Some(r) => r.to_string(),
                None => {
                    owned.sort_by(|a, b| b.1.cmp(&a.1));
                    owned[0].0.clone()
                }
            };
            for (id, _) in &owned {
                if *id != keep {
                    self.data.devices.remove(id);
                }
            }
            if let Some(record) = self.data.devices.get_mut(&keep) {
                record.last_seen = now;
                record.hostname = hostname.to_string();
                record.app_version = app_version.to_string();
            }
            self.save().map_err(|e| RegisterError::Io(e.to_string()))?;
            return Ok(keep);
        }

        let id = match requested.filter(|id| is_valid_id(id)) {
            Some(id) => {
                if self.data.devices.contains_key(id) {
                    return Err(RegisterError::Conflict(id.to_string()));
                }
                id.to_string()
            }
            None => loop {
                let candidate = generate_id();
                if !self.data.devices.contains_key(&candidate) && !self.data.blocked.contains_key(&candidate) {
                    break candidate;
                }
            },
        };

        self.data.devices.insert(
            id.clone(),
            DeviceRecord {
                secret_sha256,
                hostname: hostname.to_string(),
                app_version: app_version.to_string(),
                first_seen: now,
                last_seen: now,
                group: String::new(),
                signed_in: false,
                os_family: String::new(),
                os_version: String::new(),
                console_user: String::new(),
                display_name: String::new(),
                last_ip: String::new(),
                access_key_id: String::new(),
            },
        );
        self.save().map_err(|e| RegisterError::Io(e.to_string()))?;
        Ok(id)
    }

    /// Removes the other records with the same computer name as `keep` that
    /// aren't online — leftovers from before IDs were tied to the computer.
    /// (A different computer that happens to share the name loses nothing: it
    /// still holds its ID and gets it back the next time it connects.)
    /// Returns how many were removed.
    pub fn remove_stale_duplicates(&mut self, keep: &str, online: &HashSet<String>) -> usize {
        let Some(name) = self.data.devices.get(keep).map(|d| d.hostname.trim().to_lowercase()) else {
            return 0;
        };
        if name.is_empty() {
            return 0;
        }
        let stale: Vec<String> = self
            .data
            .devices
            .iter()
            .filter(|(id, d)| id.as_str() != keep && !online.contains(*id) && d.hostname.trim().to_lowercase() == name)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &stale {
            self.data.devices.remove(id);
        }
        if !stale.is_empty() {
            if let Err(e) = self.save() {
                tracing::warn!("could not save the device registry: {e}");
            }
        }
        stale.len()
    }

    /// Stores a host report. An empty hostname or user leaves the previous value.
    pub fn note_report(&mut self, id: &str, hostname: &str, os_family: &str, os_version: &str, console_user: &str) {
        let Some(record) = self.data.devices.get_mut(id) else { return };
        let hostname = clip(hostname, 64);
        if !hostname.is_empty() {
            record.hostname = hostname;
        }
        let os_family = clip(os_family, 32);
        if !os_family.is_empty() {
            record.os_family = os_family;
        }
        let os_version = clip(os_version, 40);
        if !os_version.is_empty() {
            record.os_version = os_version;
        }
        let console_user = clip(console_user, 80);
        if !console_user.is_empty() {
            record.console_user = console_user;
        }
        record.last_seen = now();
        if let Err(e) = self.save() {
            tracing::warn!("could not save a host report: {e}");
        }
    }

    /// Stores the friendly name. Unlike the hostname, an empty name clears it.
    pub fn note_display_name(&mut self, id: &str, name: &str) {
        let Some(record) = self.data.devices.get_mut(id) else { return };
        record.display_name = clip(name, 64);
        record.last_seen = now();
        if let Err(e) = self.save() {
            tracing::warn!("could not save a display name: {e}");
        }
    }

    /// Last time this id registered or was touched, or 0 when it is unknown.
    pub fn last_seen_of(&self, id: &str) -> u64 {
        self.data.devices.get(id).map(|d| d.last_seen).unwrap_or(0)
    }

    /// Every registered computer, for the account list. Online is decided by the caller.
    pub fn presence(&self) -> Vec<PresenceRow> {
        self.data
            .devices
            .iter()
            .map(|(id, d)| PresenceRow {
                id: id.clone(),
                hostname: d.hostname.clone(),
                os_family: d.os_family.clone(),
                os_version: d.os_version.clone(),
                console_user: d.console_user.clone(),
                display_name: d.display_name.clone(),
                last_seen: d.last_seen,
                signed_in: d.signed_in,
            })
            .collect()
    }

    /// Records that these devices are connected right now (their last-seen
    /// time), and saves it so it survives restarts.
    pub fn touch(&mut self, ids: &[String]) {
        let now = now();
        let mut changed = false;
        for id in ids {
            if let Some(d) = self.data.devices.get_mut(id) {
                d.last_seen = now;
                changed = true;
            }
        }
        if changed {
            if let Err(e) = self.save() {
                tracing::warn!("could not save the device registry: {e}");
            }
        }
    }

    /// Writes atomically (temp file + rename).
    fn save(&self) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        std::fs::rename(&tmp, &self.path)?;
        crate::cloud::protect(&self.path);
        Ok(())
    }
}

/// Formats a Unix time as a local date and time, e.g. "Oct 17, 2026 8:15 AM".
pub fn format_local(unix: u64) -> String {
    use chrono::{Local, TimeZone};
    if unix == 0 {
        return "never".into();
    }
    match Local.timestamp_opt(unix as i64, 0).single() {
        Some(t) => t.format("%b %-d, %Y %-I:%M %p").to_string(),
        None => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(name: &str) -> (PathBuf, Registry) {
        let dir = std::env::temp_dir().join(format!("pjdesk-registry-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("devices.json");
        let _ = std::fs::remove_file(&path);
        let reg = Registry::load(path.clone()).unwrap();
        (path, reg)
    }

    #[test]
    fn register_assigns_and_protects_ids() {
        let (path, mut reg) = fresh("protect");
        let secret = [1u8; 32];
        let id = reg.register(None, &secret, "pc", "0.3.4").unwrap();
        assert!(is_valid_id(&id));
        // Same secret keeps the ID.
        assert_eq!(reg.register(Some(&id), &secret, "pc", "0.3.4").unwrap(), id);
        // Another device cannot take it.
        assert!(matches!(reg.register(Some(&id), &[2u8; 32], "evil", "0.3.4"), Err(RegisterError::Conflict(_))));
        // Survives a reload.
        let mut reg = Registry::load(path.clone()).unwrap();
        assert_eq!(reg.register(Some(&id), &secret, "pc", "0.3.4").unwrap(), id);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn same_computer_without_saved_id_keeps_its_record() {
        // The app and the background service each have their own config; one
        // of them (or a reinstall) can connect without a saved ID.
        let (path, mut reg) = fresh("noid");
        let secret = [7u8; 32];
        let id = reg.register(None, &secret, "CABINDESK", "0.3.4").unwrap();
        assert_eq!(reg.register(None, &secret, "CABINDESK", "0.3.4").unwrap(), id);
        assert_eq!(reg.register(Some("123456789"), &secret, "CABINDESK", "0.3.4").unwrap(), id);
        assert_eq!(reg.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn duplicate_records_for_one_secret_are_merged() {
        let (path, mut reg) = fresh("merge");
        let secret = [9u8; 32];
        // Two records with the same secret, as older servers could leave.
        let a = reg.register(Some("111111111"), &[1u8; 32], "PC", "").unwrap();
        let b = reg.register(Some("222222222"), &[2u8; 32], "PC", "").unwrap();
        let sha = hex::encode(Sha256::digest(secret));
        for id in [&a, &b] {
            reg.data.devices.get_mut(id).unwrap().secret_sha256 = sha.clone();
        }
        // The client's own ID wins and the other record goes away.
        assert_eq!(reg.register(Some(&b), &secret, "PC", "0.3.4").unwrap(), b);
        assert_eq!(reg.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stale_records_with_the_same_name_are_removed() {
        let (path, mut reg) = fresh("stale");
        let old1 = reg.register(None, &[1u8; 32], "BENLENOVODESK", "").unwrap();
        let old2 = reg.register(None, &[2u8; 32], "benlenovodesk", "").unwrap();
        let other = reg.register(None, &[3u8; 32], "CABINDESK", "").unwrap();
        let live = reg.register(None, &[4u8; 32], "BENLENOVODESK", "0.3.4").unwrap();
        let mut online = HashSet::new();
        online.insert(live.clone());
        assert_eq!(reg.remove_stale_duplicates(&live, &online), 2);
        let ids: Vec<String> = reg.list().into_iter().map(|r| r.id).collect();
        assert!(ids.contains(&live) && ids.contains(&other));
        assert!(!ids.contains(&old1) && !ids.contains(&old2));
        // A removed computer that is really still around gets its ID back.
        assert_eq!(reg.register(Some(&old1), &[1u8; 32], "BENLENOVODESK", "0.3.4").unwrap(), old1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn touch_updates_last_seen_and_persists() {
        let (path, mut reg) = fresh("touch");
        let id = reg.register(None, &[5u8; 32], "pc", "0.3.4").unwrap();
        reg.data.devices.get_mut(&id).unwrap().last_seen = 1;
        reg.touch(&[id.clone()]);
        let reg = Registry::load(path.clone()).unwrap();
        assert!(reg.list()[0].last_seen > 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn formats_a_readable_local_time() {
        let text = format_local(1_792_243_800); // Oct 17, 2026 (UTC)
        assert!(text.contains("2026"), "{text}");
        assert!(text.ends_with("AM") || text.ends_with("PM"), "{text}");
        assert_eq!(format_local(0), "never");
    }

    #[test]
    fn groups_persist_and_reject_paths() {
        let (path, mut reg) = fresh("group");
        let id = reg.register(None, &[8u8; 32], "pc", "0.4.0").unwrap();
        reg.set_group(&id, "Office").unwrap();
        assert_eq!(reg.group_of(&id).as_deref(), Some("Office"));
        assert!(reg.set_group(&id, "../etc").is_err());
        let reg = Registry::load(path.clone()).unwrap();
        assert_eq!(reg.list()[0].group, "Office");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_blank_report_does_not_erase_the_user_or_the_name() {
        let (path, mut reg) = fresh("report");
        let id = reg.register(None, &[6u8; 32], "BENLENOVODESK", "0.6.2").unwrap();
        reg.note_report(&id, "BENLENOVODESK", "windows", "11 24H2", "SPADER\\Ben");
        reg.note_report(&id, "", "", "", "");
        let row = reg.presence().into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.hostname, "BENLENOVODESK");
        assert_eq!(row.os_family, "windows");
        assert_eq!(row.console_user, "SPADER\\Ben");
        let reg = Registry::load(path.clone()).unwrap();
        assert_eq!(reg.presence()[0].console_user, "SPADER\\Ben");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn only_builds_that_understand_the_ask_are_asked() {
        assert!(!supports_host_report("0.6.1"));
        assert!(supports_host_report("0.6.2"));
        assert!(!supports_host_report("0.6.3"));
        assert!(!supports_host_report_for("0.6.3", ""));
        assert!(!supports_host_report_for("0.6.3", "android"));
        assert!(supports_host_report_for("0.6.3", "windows"));
        assert!(supports_host_report("0.6.4"));
        assert!(supports_host_report("0.7.0"));
    }

    #[test]
    fn a_removed_computer_stays_blocked_until_it_is_unblocked() {
        let (path, mut reg) = fresh("block");
        let secret = [4u8; 32];
        let id = reg.register(None, &secret, "UNKNOWN-PC", "0.6.32").unwrap();
        reg.note_address(&id, "203.0.113.9");
        assert!(reg.block(&id).unwrap());
        assert!(reg.list().is_empty());
        let blocked = reg.blocked();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].id, id);
        assert_eq!(blocked[0].hostname, "UNKNOWN-PC");
        assert_eq!(blocked[0].last_ip, "203.0.113.9");
        assert!(blocked[0].first_seen > 0 && blocked[0].last_seen > 0);
        assert!(matches!(reg.register(Some(&id), &secret, "UNKNOWN-PC", "0.6.33"), Err(RegisterError::Blocked(_))));
        assert!(matches!(reg.register(None, &secret, "UNKNOWN-PC", "0.6.33"), Err(RegisterError::Blocked(_))));
        assert!(matches!(reg.register(Some(&id), &[9u8; 32], "other", "0.6.33"), Err(RegisterError::Blocked(_))));
        let fresh_id = reg.register(None, &[9u8; 32], "other", "0.6.33").unwrap();
        assert_ne!(fresh_id, id);
        assert!(reg.unblock(&id).unwrap());
        assert!(reg.blocked().is_empty());
        assert_eq!(reg.register(Some(&id), &secret, "UNKNOWN-PC", "0.6.33").unwrap(), id);
        let reg = Registry::load(path.clone()).unwrap();
        assert_eq!(reg.list().iter().find(|row| row.id == id).unwrap().last_ip, "");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn grace_ids_skip_signed_in_computers_and_access_keys() {
        let (path, mut reg) = fresh("grace");
        let grace = reg.register(None, &[1u8; 32], "DESKTOP-D7E4AOR", "0.6.33").unwrap();
        let signed = reg.register(None, &[2u8; 32], "Office", "0.6.33").unwrap();
        reg.mark_signed_in(&signed, true);
        let keyed = reg.register(None, &[3u8; 32], "Ada-PC", "0.6.33").unwrap();
        reg.note_access_key(&keyed, "key-ada");
        let ids = reg.grace_ids();
        assert_eq!(ids, vec![grace]);
        assert!(!ids.contains(&signed));
        assert!(!ids.contains(&keyed));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_pre_060_registry_loads_as_not_signed_in() {
        let dir = std::env::temp_dir().join(format!("pjdesk-reg-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("devices.json");
        std::fs::write(
            &path,
            r#"{"devices":{"123456789":{"secret_sha256":"aa","hostname":"Office","app_version":"0.5.2","first_seen":1,"last_seen":2,"group":""}}}"#,
        )
        .unwrap();
        let reg = Registry::load(path).unwrap();
        assert!(!reg.list()[0].signed_in);
        assert!(reg.list()[0].access_key_id.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
