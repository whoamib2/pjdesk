//! Server access keys.
//!
//! A key lets one person's computers register while signed-in accounts are
//! required, without that person signing in to the owner's account. The window
//! shows the key once. This file stores only the SHA-256 of the key text.
//! Network invite codes (`pjdesk-net1:`) are a different thing and stay as they are.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use pjdesk_proto::access::{is_access_key, ACCESS_KEY_PREFIX};
use pjdesk_proto::signal::AccessKeyBrief;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FILE_NAME: &str = "access-keys.json";
pub const LIMIT_MESSAGE: &str = "This access key is already used by the allowed number of computers.";

const DAY: u64 = 24 * 60 * 60;
const ALLOWED_EXPIRY: [u64; 4] = [0, DAY, 7 * DAY, 30 * DAY];

#[derive(Clone, Debug)]
pub struct LiveKey {
    pub id: String,
    pub label: String,
    pub computer_limit: u32,
}

#[derive(Debug)]
pub enum CheckError {
    Unknown,
    Revoked,
    Expired,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CheckError::Unknown => "unknown key",
            CheckError::Revoked => "revoked",
            CheckError::Expired => "expired",
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    id: String,
    label: String,
    secret_sha256: String,
    created_at: u64,
    #[serde(default)]
    expires_at: u64,
    #[serde(default = "one")]
    computer_limit: u32,
    #[serde(default)]
    revoked_at: u64,
}

fn one() -> u32 {
    1
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    keys: Vec<Record>,
}

pub struct Store;

impl Store {
    pub fn create(dir: &Path, label: &str, expires_secs: u64, computer_limit: u32) -> Result<String, String> {
        let label = clean_label(label)?;
        if !ALLOWED_EXPIRY.contains(&expires_secs) {
            return Err("Choose never, 1 day, 7 days, or 30 days.".into());
        }
        let computer_limit = normalize_limit(computer_limit)?;
        let mut file = load(dir);
        let secret = mint_secret();
        let now = unix_now();
        file.keys.push(Record {
            id: hex::encode(pjdesk_proto::random_bytes::<8>()),
            label,
            secret_sha256: hash_secret(&secret),
            created_at: now,
            expires_at: if expires_secs == 0 { 0 } else { now.saturating_add(expires_secs) },
            computer_limit,
            revoked_at: 0,
        });
        save(dir, &file)?;
        Ok(secret)
    }

    /// Checks a presented key. A revoked or expired key fails even when
    /// signed-in accounts are not required. The hash is compared in constant time.
    pub fn check(dir: &Path, token: &str, now: u64) -> Result<LiveKey, CheckError> {
        let token = token.trim();
        if !is_access_key(token) {
            return Err(CheckError::Unknown);
        }
        let digest = hash_secret(token);
        let file = load(dir);
        let mut found: Option<&Record> = None;
        for key in &file.keys {
            if same_hash(&key.secret_sha256, &digest) {
                found = Some(key);
                break;
            }
        }
        let Some(key) = found else {
            return Err(CheckError::Unknown);
        };
        if key.revoked_at != 0 {
            return Err(CheckError::Revoked);
        }
        if key.expires_at != 0 && now >= key.expires_at {
            return Err(CheckError::Expired);
        }
        Ok(LiveKey {
            id: key.id.clone(),
            label: key.label.clone(),
            computer_limit: key.computer_limit.max(1).min(100),
        })
    }

    /// Marks the key revoked and keeps the hash, so the same text cannot work again.
    pub fn revoke(dir: &Path, id: &str) -> Result<(), String> {
        let id = id.trim();
        if !is_key_id(id) {
            return Err("That is not an access key.".into());
        }
        let mut file = load(dir);
        let Some(key) = file.keys.iter_mut().find(|key| key.id == id) else {
            return Err("That access key is not on this server.".into());
        };
        if key.revoked_at == 0 {
            key.revoked_at = unix_now().max(1);
            save(dir, &file)?;
        }
        Ok(())
    }

    pub fn public_list(dir: &Path) -> Vec<AccessKeyBrief> {
        let names = computer_names(dir);
        let mut rows: Vec<AccessKeyBrief> = load(dir)
            .keys
            .into_iter()
            .map(|key| {
                let (computers, names) = names.get(&key.id).cloned().unwrap_or_default();
                AccessKeyBrief {
                    id: key.id,
                    label: key.label,
                    created_at: key.created_at,
                    expires_at: key.expires_at,
                    computer_limit: key.computer_limit.max(1),
                    revoked_at: key.revoked_at,
                    computers,
                    names,
                }
            })
            .collect();
        rows.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.label.cmp(&b.label)));
        rows
    }

    /// Key ids that must not keep a computer: revoked, or past their expiry.
    pub fn dead_ids(dir: &Path, now: u64) -> Vec<String> {
        load(dir)
            .keys
            .into_iter()
            .filter(|key| key.revoked_at != 0 || (key.expires_at != 0 && now >= key.expires_at))
            .map(|key| key.id)
            .collect()
    }
}

pub fn is_key_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn computer_names(dir: &Path) -> std::collections::HashMap<String, (u32, Vec<String>)> {
    #[derive(Deserialize)]
    struct Reg {
        #[serde(default)]
        devices: std::collections::BTreeMap<String, Dev>,
    }
    #[derive(Deserialize)]
    struct Dev {
        #[serde(default)]
        hostname: String,
        #[serde(default)]
        access_key_id: String,
    }
    let mut out: std::collections::HashMap<String, (u32, Vec<String>)> = std::collections::HashMap::new();
    let Ok(bytes) = std::fs::read(dir.join("devices.json")) else {
        return out;
    };
    let Ok(file) = serde_json::from_slice::<Reg>(&bytes) else {
        return out;
    };
    for dev in file.devices.values() {
        if dev.access_key_id.is_empty() {
            continue;
        }
        let entry = out.entry(dev.access_key_id.clone()).or_insert((0, Vec::new()));
        entry.0 = entry.0.saturating_add(1);
        if entry.1.len() < 8 {
            let name = dev.hostname.trim();
            if !name.is_empty() {
                entry.1.push(name.to_string());
            }
        }
    }
    out
}

fn clean_label(label: &str) -> Result<String, String> {
    let label: String = label.trim().chars().filter(|c| !c.is_control()).take(64).collect();
    if label.is_empty() {
        return Err("Give the key a label, such as the person's name.".into());
    }
    Ok(label)
}

fn normalize_limit(limit: u32) -> Result<u32, String> {
    let limit = if limit == 0 { 1 } else { limit };
    if !(1..=100).contains(&limit) {
        return Err("The computer limit is from 1 to 100.".into());
    }
    Ok(limit)
}

fn mint_secret() -> String {
    let raw = pjdesk_proto::random_bytes::<32>();
    format!("{ACCESS_KEY_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw))
}

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.trim().as_bytes()))
}

fn same_hash(stored: &str, computed: &str) -> bool {
    let Ok(left) = hex::decode(stored.trim()) else {
        return false;
    };
    let Ok(right) = hex::decode(computed.trim()) else {
        return false;
    };
    if left.len() != right.len() || left.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn path_of(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

fn load(dir: &Path) -> File {
    match std::fs::read(path_of(dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => File::default(),
    }
}

fn save(dir: &Path, file: &File) -> Result<(), String> {
    let path = path_of(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    let bytes = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    crate::cloud::protect(&path);
    let written = std::fs::read_to_string(&path).unwrap_or_default();
    if written.contains(ACCESS_KEY_PREFIX) {
        let _ = std::fs::remove_file(&path);
        return Err("Refused to store an access key. Only its hash is kept.".into());
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    fn dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pjdesk-keys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn create_stores_a_hash_and_shows_the_key_only_to_the_caller() {
        let dir = dir("create");
        let secret = Store::create(&dir, " Ada ", 0, 0).unwrap();
        assert!(secret.starts_with(ACCESS_KEY_PREFIX));
        let file = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(!file.contains(&secret));
        assert!(!file.contains(ACCESS_KEY_PREFIX));
        assert!(file.contains(&hash_secret(&secret)));
        let now = unix_now();
        let live = Store::check(&dir, &secret, now).unwrap();
        assert_eq!(live.label, "Ada");
        assert_eq!(live.computer_limit, 1);
        assert!(Store::check(&dir, &format!("{secret}x"), now).is_err());
        assert!(Store::check(&dir, "not-a-key", now).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expiry_revoke_and_a_second_computer_are_refused() {
        let dir = dir("rules");
        let now = 1_700_000_000u64;
        let lasting = Store::create(&dir, "Ada", 0, 1).unwrap();
        let brief = Store::create(&dir, "Bea", DAY, 1).unwrap();
        let mut file = load(&dir);
        for key in &mut file.keys {
            if same_hash(&key.secret_sha256, &hash_secret(&brief)) {
                key.created_at = now;
                key.expires_at = now + DAY;
            }
        }
        save(&dir, &file).unwrap();
        assert!(Store::check(&dir, &brief, now + DAY - 1).is_ok());
        assert!(matches!(Store::check(&dir, &brief, now + DAY), Err(CheckError::Expired)));

        let live = Store::check(&dir, &lasting, now).unwrap();
        let mut reg = Registry::load(dir.join("devices.json")).unwrap();
        let first = [1u8; 32];
        let second = [2u8; 32];
        let id = reg.register(None, &first, "Ada-PC", "0.6.33").unwrap();
        reg.note_access_key(&id, &live.id);
        assert!(reg.key_has_room(&live.id, &first, 1));
        assert!(!reg.key_has_room(&live.id, &second, 1));
        assert_eq!(reg.list().iter().find(|row| row.id == id).unwrap().access_key_id, live.id);

        Store::revoke(&dir, &live.id).unwrap();
        assert!(matches!(Store::check(&dir, &lasting, now), Err(CheckError::Revoked)));
        let removed = reg.take_access_key(&live.id);
        assert_eq!(removed, vec![id.clone()]);
        assert!(reg.list().iter().all(|row| row.id != id));
        assert!(reg.blocked().is_empty(), "revoke removes the computer and does not block it");
        let again = Store::check(&dir, &lasting, now);
        assert!(again.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_registry_has_no_access_key() {
        let dir = dir("old");
        std::fs::write(
            dir.join("devices.json"),
            r#"{"devices":{"123456789":{"secret_sha256":"aa","hostname":"Office","first_seen":1,"last_seen":2}}}"#,
        )
        .unwrap();
        let reg = Registry::load(dir.join("devices.json")).unwrap();
        let row = &reg.list()[0];
        assert!(!row.signed_in);
        assert!(row.access_key_id.is_empty());
        assert!(Store::public_list(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
