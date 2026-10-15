//! Persistent device registry: which ID belongs to which device.
//!
//! Each host proves ownership of its ID with a random 32-byte secret created
//! on first run. The server stores only the SHA-256 of that secret, in
//! `devices.json` inside the data folder.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use pjdesk_proto::{format_id, generate_id, is_valid_id};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Default, Serialize, Deserialize)]
struct RegistryFile {
    devices: BTreeMap<String, DeviceRecord>,
}

#[derive(Clone, Serialize, Deserialize)]
struct DeviceRecord {
    secret_sha256: String,
    hostname: String,
    first_seen: u64,
    last_seen: u64,
}

pub struct Registry {
    path: PathBuf,
    data: RegistryFile,
}

#[derive(Debug)]
pub enum RegisterError {
    /// The ID exists and the secret does not match.
    Conflict(String),
    Io(String),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegisterError::Conflict(id) => write!(f, "ID {} belongs to another device", format_id(id)),
            RegisterError::Io(e) => write!(f, "could not save the device registry: {e}"),
        }
    }
}

impl std::error::Error for RegisterError {}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Registry {
    /// Loads the registry, or starts an empty one if the file doesn't exist.
    pub fn load(path: PathBuf) -> Result<Self> {
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RegistryFile::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self { path, data })
    }

    pub fn len(&self) -> usize {
        self.data.devices.len()
    }

    /// Rows for the management window: (id, hostname, last-seen unix seconds),
    /// most recently seen first.
    pub fn list(&self) -> Vec<(String, String, u64)> {
        let mut rows: Vec<_> =
            self.data.devices.iter().map(|(id, d)| (id.clone(), d.hostname.clone(), d.last_seen)).collect();
        rows.sort_by(|a, b| b.2.cmp(&a.2));
        rows
    }

    /// Forgets a device (the GUI's "remove"). The device can re-register and
    /// will get a new ID.
    pub fn remove(&mut self, id: &str) -> Result<()> {
        if self.data.devices.remove(id).is_some() {
            self.save()?;
        }
        Ok(())
    }

    /// Registers a device. Returns its (possibly newly assigned) ID.
    ///
    /// * Known ID + matching secret → same ID.
    /// * Known ID + wrong secret → [`RegisterError::Conflict`].
    /// * Unknown but well-formed ID → claimed (keeps IDs stable if the
    ///   server's data folder was lost).
    /// * No ID → a fresh random nine-digit ID.
    pub fn register(
        &mut self,
        requested: Option<&str>,
        secret: &[u8; 32],
        hostname: &str,
    ) -> Result<String, RegisterError> {
        let secret_sha256 = hex::encode(Sha256::digest(secret));
        let now = now();

        let id = match requested.filter(|id| is_valid_id(id)) {
            Some(id) => {
                if let Some(record) = self.data.devices.get_mut(id) {
                    if record.secret_sha256 != secret_sha256 {
                        return Err(RegisterError::Conflict(id.to_string()));
                    }
                    record.last_seen = now;
                    record.hostname = hostname.to_string();
                    let id = id.to_string();
                    self.save().map_err(|e| RegisterError::Io(e.to_string()))?;
                    return Ok(id);
                }
                id.to_string()
            }
            None => loop {
                let candidate = generate_id();
                if !self.data.devices.contains_key(&candidate) {
                    break candidate;
                }
            },
        };

        self.data.devices.insert(
            id.clone(),
            DeviceRecord { secret_sha256, hostname: hostname.to_string(), first_seen: now, last_seen: now },
        );
        self.save().map_err(|e| RegisterError::Io(e.to_string()))?;
        Ok(id)
    }

    /// Writes atomically (temp file + rename).
    fn save(&self) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_assigns_and_protects_ids() {
        let dir = std::env::temp_dir().join(format!("pjdesk-registry-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("devices.json");
        let _ = std::fs::remove_file(&path);

        let mut reg = Registry::load(path.clone()).unwrap();
        let secret = [1u8; 32];
        let id = reg.register(None, &secret, "pc").unwrap();
        assert!(is_valid_id(&id));
        // Same secret keeps the ID.
        assert_eq!(reg.register(Some(&id), &secret, "pc").unwrap(), id);
        // Another device cannot take it.
        assert!(matches!(reg.register(Some(&id), &[2u8; 32], "evil"), Err(RegisterError::Conflict(_))));
        // Survives a reload.
        let mut reg = Registry::load(path.clone()).unwrap();
        assert_eq!(reg.register(Some(&id), &secret, "pc").unwrap(), id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
