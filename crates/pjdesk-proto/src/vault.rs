//! Saved computer passwords, sealed so pjdesk.com only stores ciphertext.
//!
//! The key is [`crate::access::derive_password_vault_key`]. This is not a
//! protocol message. Clients send the blob to the account over HTTPS.

use std::collections::BTreeMap;

use base64::Engine;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use serde::{Deserialize, Serialize};

pub const MAGIC: &[u8] = b"PJVAULT1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub password: String,
    pub updated: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge {
    pub entries: Vec<Entry>,
    pub local_changed: bool,
    pub remote_changed: bool,
}

#[derive(Serialize, Deserialize)]
struct Blob {
    entries: Vec<Wire>,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    id: String,
    password: String,
    updated: u64,
}

pub fn seal(key: &[u8; 32], entries: &[Entry]) -> Result<String, String> {
    let nonce = crate::random_bytes::<12>();
    seal_with_nonce(key, entries, &nonce)
}

pub fn seal_with_nonce(key: &[u8; 32], entries: &[Entry], nonce: &[u8; 12]) -> Result<String, String> {
    let plain = serde_json::to_vec(&blob(entries)).map_err(|_| "could not encode the password vault".to_string())?;
    let cipher = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(key));
    let ct = cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: &plain, aad: MAGIC })
        .map_err(|_| "could not seal the password vault".to_string())?;
    let mut out = Vec::with_capacity(MAGIC.len() + nonce.len() + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

pub fn open(key: &[u8; 32], text: &str) -> Result<Vec<Entry>, String> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| "that password copy is not valid".to_string())?;
    if raw.len() < MAGIC.len() + 12 + 16 || !raw.starts_with(MAGIC) {
        return Err("that password copy is not valid".into());
    }
    let (nonce, ct) = raw[MAGIC.len()..].split_at(12);
    let cipher = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(key));
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad: MAGIC })
        .map_err(|_| "that password copy was sealed with a different account password".to_string())?;
    let blob: Blob = serde_json::from_slice(&plain).map_err(|_| "that password copy is not valid".to_string())?;
    Ok(blob
        .entries
        .into_iter()
        .filter_map(|w| {
            let id = crate::normalize_id(&w.id);
            if !crate::is_valid_id(&id) {
                return None;
            }
            Some(Entry { id, password: w.password, updated: w.updated })
        })
        .collect())
}

/// Newer `updated` wins. An empty password is a deletion. `now` stamps a local
/// password that has never been synced.
pub fn merge(local: &[Entry], remote: &[Entry], now: u64) -> Merge {
    let mut map: BTreeMap<String, (Option<Entry>, Option<Entry>)> = BTreeMap::new();
    for entry in local {
        if !crate::is_valid_id(&entry.id) {
            continue;
        }
        map.entry(entry.id.clone()).or_default().0 = Some(entry.clone());
    }
    for entry in remote {
        if !crate::is_valid_id(&entry.id) {
            continue;
        }
        map.entry(entry.id.clone()).or_default().1 = Some(entry.clone());
    }
    let mut entries = Vec::new();
    let mut local_changed = false;
    let mut remote_changed = false;
    for (id, (l, r)) in map {
        let chosen = match (l, r) {
            (None, Some(remote)) => {
                local_changed = true;
                remote
            }
            (Some(local), None) => {
                if local.password.is_empty() && local.updated == 0 {
                    continue;
                }
                let mut kept = local;
                if kept.updated == 0 {
                    kept.updated = now.max(1);
                    local_changed = true;
                }
                remote_changed = true;
                kept
            }
            (Some(local), Some(remote)) => {
                let pick = if remote.updated > local.updated {
                    remote
                } else if local.updated > remote.updated {
                    local
                } else if remote.password.is_empty() && !local.password.is_empty() {
                    local
                } else if local.password.is_empty() && !remote.password.is_empty() {
                    remote
                } else {
                    local
                };
                pick
            }
            (None, None) => continue,
        };
        let local_same = local_of(local, &id).is_some_and(|e| e == &chosen);
        let remote_same = remote_of(remote, &id).is_some_and(|e| e == &chosen);
        if !local_same {
            local_changed = true;
        }
        if !remote_same {
            remote_changed = true;
        }
        let _ = id;
        entries.push(chosen);
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    Merge { entries, local_changed, remote_changed }
}

fn local_of<'a>(local: &'a [Entry], id: &str) -> Option<&'a Entry> {
    local.iter().find(|e| e.id == id)
}

fn remote_of<'a>(remote: &'a [Entry], id: &str) -> Option<&'a Entry> {
    remote.iter().find(|e| e.id == id)
}

fn blob(entries: &[Entry]) -> Blob {
    let mut wires: Vec<Wire> = entries
        .iter()
        .filter(|e| crate::is_valid_id(&e.id))
        .map(|e| Wire { id: e.id.clone(), password: e.password.clone(), updated: e.updated })
        .collect();
    wires.sort_by(|a, b| a.id.cmp(&b.id));
    Blob { entries: wires }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, password: &str, updated: u64) -> Entry {
        Entry { id: id.into(), password: password.into(), updated }
    }

    #[test]
    fn seal_round_trips_and_rejects_the_wrong_key() {
        let key = crate::access::derive_password_vault_key("Ben@Spader.us", "correct horse");
        let nonce = [7u8; 12];
        let blob = seal_with_nonce(&key, &[entry("902488771", "cabin", 10)], &nonce).unwrap();
        let opened = open(&key, &blob).unwrap();
        assert_eq!(opened, vec![entry("902488771", "cabin", 10)]);
        let other = crate::access::derive_password_vault_key("ben@spader.us", "other password");
        assert!(open(&other, &blob).is_err());
        assert!(blob.starts_with("UEpWQVVMVD"), "{blob}");
    }

    #[test]
    fn merge_uploads_local_keeps_the_newer_remote_and_honors_a_deletion() {
        let only_local = merge(&[entry("902488771", "cabin", 0)], &[], 50);
        assert!(only_local.remote_changed);
        assert_eq!(only_local.entries, vec![entry("902488771", "cabin", 50)]);

        let from_account = merge(&[], &[entry("902488771", "cabin", 10)], 50);
        assert!(from_account.local_changed);
        assert!(!from_account.remote_changed);
        assert_eq!(from_account.entries[0].password, "cabin");

        let newer = merge(&[entry("902488771", "old", 5)], &[entry("902488771", "new", 9)], 50);
        assert_eq!(newer.entries[0].password, "new");
        assert!(newer.local_changed);
        assert!(!newer.remote_changed);

        let deleted = merge(&[entry("902488771", "cabin", 4)], &[entry("902488771", "", 9)], 50);
        assert_eq!(deleted.entries[0].password, "");
        assert!(deleted.local_changed);
    }
}
