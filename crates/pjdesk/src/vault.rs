//! Upload and download the sealed saved-password vault.
//!
//! The account password never leaves this computer. A temporary sign-in keeps
//! the key in memory and does not write it. "Sign me out when I'm done" drops
//! that key; the visit's passwords are removed with the rest of the visit.

use std::sync::Mutex;

use pjdesk_proto::vault::{self, Entry};

use crate::config::Config;

static SESSION_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);
static RESULT: Mutex<Option<Result<Vec<Entry>, String>>> = Mutex::new(None);
static BUSY: Mutex<()> = Mutex::new(());

pub fn set_session_key(key: [u8; 32]) {
    *SESSION_KEY.lock().unwrap() = Some(key);
}

/// Writes the in-memory vault key the way a normal sign-in does.
pub fn store_session_key(config: &mut Config) {
    let Some(key) = *SESSION_KEY.lock().unwrap() else { return };
    if config.password_vault_key.is_some() {
        return;
    }
    config.password_vault_key = Some(crate::secure::protect(&hex::encode(key)));
}

pub fn clear_session_key() {
    *SESSION_KEY.lock().unwrap() = None;
}

pub fn key_from(config: &Config) -> Option<[u8; 32]> {
    if let Some(key) = *SESSION_KEY.lock().unwrap() {
        return Some(key);
    }
    let stored = config.password_vault_key.as_deref()?;
    let text = crate::secure::reveal(stored)?;
    let bytes = hex::decode(text.trim()).ok()?;
    <[u8; 32]>::try_from(bytes).ok()
}

pub fn local_entries(config: &Config) -> Vec<Entry> {
    let mut ids: Vec<String> = config.password_vault_times().keys().cloned().collect();
    for id in config.saved_password_ids() {
        if !ids.iter().any(|e| e == &id) {
            ids.push(id);
        }
    }
    ids.retain(|id| pjdesk_proto::is_valid_id(id));
    ids.sort();
    ids.into_iter()
        .map(|id| Entry {
            password: config.saved_password(&id).unwrap_or_default(),
            updated: config.password_vault_times().get(&id).copied().unwrap_or(0),
            id,
        })
        .collect()
}

pub fn apply_entries(config: &mut Config, entries: &[Entry]) {
    for entry in entries {
        config.set_saved_password_at(&entry.id, &entry.password, entry.updated);
    }
}

/// `upload` is false for the first pull of a temporary sign-in, so passwords
/// that were already on this computer are not sent.
pub fn schedule(refresh: String, device: String, key: [u8; 32], local: Vec<Entry>, upload: bool) {
    if refresh.is_empty() || device.is_empty() {
        return;
    }
    std::thread::Builder::new()
        .name("password-vault".into())
        .spawn(move || {
            let _guard = BUSY.lock().unwrap();
            let result = pull_and_maybe_put(&refresh, &device, &key, &local, upload);
            *RESULT.lock().unwrap() = Some(result);
        })
        .ok();
}

pub fn take_result() -> Option<Result<Vec<Entry>, String>> {
    RESULT.lock().unwrap().take()
}

fn pull_and_maybe_put(refresh: &str, device: &str, key: &[u8; 32], local: &[Entry], upload: bool) -> Result<Vec<Entry>, String> {
    let text = crate::network::fetch_password_vault(refresh, device).map_err(|e| e.to_string())?;
    let remote = if text.trim().is_empty() {
        Vec::new()
    } else {
        vault::open(key, text.trim()).map_err(|e| e.to_string())?
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(1);
    let merged = vault::merge(local, &remote, now);
    if upload && merged.remote_changed {
        let blob = vault::seal(key, &merged.entries)?;
        crate::network::put_password_vault(refresh, device, &blob).map_err(|e| e.to_string())?;
    }
    let n = merged.entries.iter().filter(|e| !e.password.is_empty()).count();
    tracing::info!("password vault synced ({n} computers)");
    Ok(merged.entries)
}
