//! Client settings, stored as JSON in `%APPDATA%\DeskLink\client.json`
//! (or `~/.config/DeskLink/client.json`). Set `DESKLINK_CONFIG` to use
//! another file (handy for running two instances on one machine).

use std::path::PathBuf;

use anyhow::{Context, Result};
use desklink_proto::tls::{parse_fingerprint, Fingerprint};
use serde::{Deserialize, Serialize};

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
}

impl Config {
    pub fn path() -> PathBuf {
        if let Some(path) = std::env::var_os("DESKLINK_CONFIG") {
            return PathBuf::from(path);
        }
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        };
        base.unwrap_or_else(|| PathBuf::from(".")).join("DeskLink").join("client.json")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// The device secret, created (and saved) on first use.
    pub fn device_secret(&mut self) -> Result<[u8; 32]> {
        if let Some(hex_secret) = &self.device_secret {
            if let Ok(bytes) = hex::decode(hex_secret) {
                if let Ok(secret) = <[u8; 32]>::try_from(bytes.as_slice()) {
                    return Ok(secret);
                }
            }
        }
        let secret = desklink_proto::random_bytes::<32>();
        self.device_secret = Some(hex::encode(secret));
        self.device_id = None; // a new secret can't claim the old ID
        self.save()?;
        Ok(secret)
    }

    pub fn pinned_fingerprint(&self) -> Option<Fingerprint> {
        self.server_fingerprint.as_deref().and_then(|s| parse_fingerprint(s).ok())
    }
}
