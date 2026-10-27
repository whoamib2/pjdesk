//! Account app-lock PIN. The PIN itself is never stored.
//!
//! What is stored is a salted PBKDF2-HMAC-SHA256 hash, sealed with the same
//! key as the saved-password vault (`PJLOCK01`). pjdesk.com only sees that
//! ciphertext. Every signed-in computer opens it and checks the PIN locally.

use base64::Engine;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use serde::{Deserialize, Serialize};

pub const MAGIC: &[u8] = b"PJLOCK01";
pub const ITERATIONS: u32 = 200_000;
pub const IDLE_DEFAULT: u64 = 300;
const MAX_ITERATIONS: u32 = 600_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verifier {
    pub salt: String,
    pub hash: String,
    pub iterations: u32,
    pub idle_sec: u64,
    pub updated: u64,
}

pub fn pin_acceptable(pin: &str) -> bool {
    let n = pin.chars().count();
    (4..=8).contains(&n) && pin.bytes().all(|b| b.is_ascii_digit())
}

/// Wait after this many wrong PINs. The first four misses do not wait.
/// Five waits a minute, then 2, 5, 10, and 30 minutes.
pub fn lockout_secs(fails: u32) -> u64 {
    match fails {
        0..=4 => 0,
        5 => 60,
        6 => 120,
        7 => 300,
        8 => 600,
        _ => 1800,
    }
}

/// `fails` is the count before this miss. Returns the new count and the unix
/// time until which further tries wait. `until` is 0 when there is no wait.
pub fn note_miss(fails: u32, now: u64) -> (u32, u64) {
    let fails = fails.saturating_add(1);
    let wait = lockout_secs(fails);
    let until = if wait == 0 { 0 } else { now.saturating_add(wait) };
    (fails, until)
}

/// True when the window has been minimized, hidden, or idle for `idle_secs`.
pub fn relock(away_secs: u64, idle_secs: u64) -> bool {
    idle_secs > 0 && away_secs >= idle_secs
}

pub fn clamp_idle(secs: u64) -> u64 {
    if secs == 0 {
        IDLE_DEFAULT
    } else {
        secs.clamp(60, 24 * 60 * 60)
    }
}

/// What a signed-in Windows window shows before its contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowGate {
    /// No signed-in account, so there is no account PIN to require.
    Open,
    /// Signed in, and a PIN is already stored.
    Unlock,
    /// Signed in, and this account does not have a PIN yet.
    SetPin,
}

/// The management window asks for the account PIN whenever an account is signed in.
pub fn window_gate(signed_in: bool, has_pin: bool) -> WindowGate {
    if !signed_in {
        WindowGate::Open
    } else if has_pin {
        WindowGate::Unlock
    } else {
        WindowGate::SetPin
    }
}

/// Written once so an update can tell a session that already existed from a later sign-in.
pub const PIN_PROMPT_VERSION: &str = "0.6.47";

/// Whether to show the blocking Set a PIN screen.
///
/// A computer that joined with a code or a key is not asked. Skip for now is
/// remembered. An account that already has a PIN uses the unlock screen instead.
pub fn should_ask_for_pin(signed_in: bool, has_pin: bool, code_or_key: bool, declined: bool) -> bool {
    signed_in && !has_pin && !code_or_key && !declined
}

/// First launch of this build. `had_session` with no PIN means the previous
/// build already had them on Set a PIN, so that screen is not shown again.
/// Returns `(declined, save)`. `save` is false when this build already decided.
pub fn pin_prompt_on_launch(saved_prompt: &str, saved_declined: bool, had_session: bool, has_pin: bool) -> (bool, bool) {
    if saved_prompt == PIN_PROMPT_VERSION {
        (saved_declined, false)
    } else if had_session && !has_pin {
        (true, true)
    } else {
        (saved_declined, true)
    }
}

/// A second launch must leave. The first window is the one that opens.
pub fn second_window_should_exit(acquired: bool) -> bool {
    !acquired
}

/// Local window-lock file shared by the PJDesk client and PJDesk Server.
/// The PIN is not in here. `verifier` is the opened salt and hash, already
/// sealed for this Windows user. An older file may still contain a Windows
/// Hello choice. That field is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowLockFile {
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub fails: u32,
    #[serde(default)]
    pub until: u64,
    #[serde(default)]
    pub sealed: String,
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub verifier: String,
    /// Skip for now. The blocking Set a PIN screen stays off.
    #[serde(default)]
    pub pin_declined: bool,
    /// [`PIN_PROMPT_VERSION`] after this build has decided whether to ask.
    #[serde(default)]
    pub pin_prompt: String,
}

pub fn window_lock_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("PJDESK_WINDOW_LOCK") {
        return std::path::PathBuf::from(path);
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    };
    base.unwrap_or_else(|| std::path::PathBuf::from(".")).join("PJDesk").join("window-lock.json")
}

pub fn read_window_lock(path: &std::path::Path) -> WindowLockFile {
    let Ok(bytes) = std::fs::read(path) else {
        return WindowLockFile::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn write_window_lock(path: &std::path::Path, file: &WindowLockFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    if std::fs::rename(&tmp, path).is_err() {
        std::fs::write(path, &bytes).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(())
}

/// Lockout for this account. A file for a different account does not count.
pub fn lockout_for(file: &WindowLockFile, email: &str) -> (u32, u64) {
    if email.is_empty() || !file.email.eq_ignore_ascii_case(email) {
        (0, 0)
    } else {
        (file.fails, file.until)
    }
}

pub fn hash_pin(pin: &str, salt: &[u8], iterations: u32) -> Result<[u8; 32], String> {
    let iterations =
        std::num::NonZeroU32::new(iterations).filter(|n| n.get() <= MAX_ITERATIONS).ok_or("bad pin hash")?;
    let mut out = [0u8; 32];
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, iterations, salt, pin.as_bytes(), &mut out);
    Ok(out)
}

pub fn create(pin: &str, idle_sec: u64, updated: u64) -> Result<Verifier, String> {
    if !pin_acceptable(pin) {
        return Err("Use 4 to 8 digits.".into());
    }
    let salt = crate::random_bytes::<16>();
    let hash = hash_pin(pin, &salt, ITERATIONS)?;
    Ok(Verifier {
        salt: base64::engine::general_purpose::STANDARD.encode(salt),
        hash: hex::encode(hash),
        iterations: ITERATIONS,
        idle_sec: clamp_idle(idle_sec),
        updated: updated.max(1),
    })
}

pub fn matches(verifier: &Verifier, pin: &str) -> bool {
    if !pin_acceptable(pin) {
        return false;
    }
    let Ok(salt) = base64::engine::general_purpose::STANDARD.decode(verifier.salt.trim()) else {
        return false;
    };
    let Ok(want) = hex::decode(verifier.hash.trim()) else {
        return false;
    };
    let Ok(got) = hash_pin(pin, &salt, verifier.iterations) else {
        return false;
    };
    ct_eq(&got, &want)
}

pub fn seal(key: &[u8; 32], verifier: &Verifier) -> Result<String, String> {
    let nonce = crate::random_bytes::<12>();
    seal_with_nonce(key, verifier, &nonce)
}

pub fn seal_with_nonce(key: &[u8; 32], verifier: &Verifier, nonce: &[u8; 12]) -> Result<String, String> {
    let plain = serde_json::to_vec(verifier).map_err(|_| "could not encode the app lock".to_string())?;
    let cipher = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(key));
    let ct = cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: &plain, aad: MAGIC })
        .map_err(|_| "could not seal the app lock".to_string())?;
    let mut out = Vec::with_capacity(MAGIC.len() + nonce.len() + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

pub fn open(key: &[u8; 32], text: &str) -> Result<Verifier, String> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| "that app lock is not valid".to_string())?;
    if raw.len() < MAGIC.len() + 12 + 16 || !raw.starts_with(MAGIC) {
        return Err("that app lock is not valid".into());
    }
    let (nonce, ct) = raw[MAGIC.len()..].split_at(12);
    let cipher = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(key));
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad: MAGIC })
        .map_err(|_| "that app lock was sealed with a different account password".to_string())?;
    let verifier: Verifier = serde_json::from_slice(&plain).map_err(|_| "that app lock is not valid".to_string())?;
    if verifier.iterations == 0 || verifier.iterations > MAX_ITERATIONS {
        return Err("that app lock is not valid".into());
    }
    Ok(verifier)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_rules_and_an_increasing_lockout() {
        assert!(pin_acceptable("1234"));
        assert!(pin_acceptable("12345678"));
        assert!(!pin_acceptable("123"));
        assert!(!pin_acceptable("12345a"));
        assert!(!pin_acceptable(""));
        assert_eq!(lockout_secs(4), 0);
        assert_eq!(lockout_secs(5), 60);
        assert_eq!(lockout_secs(6), 120);
        assert_eq!(lockout_secs(7), 300);
        assert_eq!(lockout_secs(9), 1800);
        assert_eq!(note_miss(4, 1_000), (5, 1_060));
        assert_eq!(note_miss(5, 2_000), (6, 2_120));
        assert!(!relock(299, 300));
        assert!(relock(300, 300));
        assert!(!relock(10, 0));
    }

    #[test]
    fn a_known_pin_hash_and_a_sealed_round_trip() {
        let salt = [9u8; 16];
        let hash = hash_pin("123456", &salt, 1000).unwrap();
        assert_eq!(hex::encode(hash), "c5a49ff1b52892a67356c2d284d98a7b145b9eedfada7621e21b8befa71efa97");
        let slow = hash_pin("123456", &salt, ITERATIONS).unwrap();
        assert_eq!(hex::encode(slow), "a7d083bc95f7969cfc2f3d2d32a1cf3622ed8891720a419d127451f882239fe3");

        let key = [4u8; 32];
        let nonce = [2u8; 12];
        let verifier = Verifier {
            salt: base64::engine::general_purpose::STANDARD.encode(salt),
            hash: hex::encode(slow),
            iterations: ITERATIONS,
            idle_sec: 300,
            updated: 9,
        };
        let blob = seal_with_nonce(&key, &verifier, &nonce).unwrap();
        assert!(blob.starts_with("UEpMT0NLMD"), "{blob}");
        assert!(matches(&open(&key, &blob).unwrap(), "123456"));
        assert!(!matches(&verifier, "1234567"));
        assert!(open(&[5u8; 32], &blob).is_err());
        assert!(create("12", 300, 1).is_err());
        let made = create("1234", 0, 0).unwrap();
        assert_eq!(made.idle_sec, IDLE_DEFAULT);
        assert_eq!(made.iterations, ITERATIONS);
        assert!(matches(&made, "1234"));
    }

    #[test]
    fn a_signed_in_window_requires_a_pin() {
        assert_eq!(window_gate(false, false), WindowGate::Open);
        assert_eq!(window_gate(false, true), WindowGate::Open);
        assert_eq!(window_gate(true, false), WindowGate::SetPin);
        assert_eq!(window_gate(true, true), WindowGate::Unlock);
        assert!(second_window_should_exit(false));
        assert!(!second_window_should_exit(true));
    }

    #[test]
    fn a_code_or_a_skipped_prompt_does_not_ask_for_a_pin() {
        assert!(!should_ask_for_pin(false, false, false, false));
        assert!(!should_ask_for_pin(true, true, false, false));
        assert!(!should_ask_for_pin(true, false, true, false));
        assert!(!should_ask_for_pin(true, false, false, true));
        assert!(should_ask_for_pin(true, false, false, false));
        assert_eq!(pin_prompt_on_launch("", false, true, false), (true, true));
        assert_eq!(pin_prompt_on_launch("", false, true, true), (false, true));
        assert_eq!(pin_prompt_on_launch("", false, false, false), (false, true));
        assert_eq!(pin_prompt_on_launch(PIN_PROMPT_VERSION, false, true, false), (false, false));
        assert_eq!(pin_prompt_on_launch(PIN_PROMPT_VERSION, true, false, false), (true, false));
    }

    #[test]
    fn the_shared_lock_file_keeps_lockout_per_account() {
        let path = std::env::temp_dir().join(format!("pjdesk-window-lock-{}-{}.json", std::process::id(), line!()));
        let _ = std::fs::remove_file(&path);
        let mut file =
            WindowLockFile { email: "Ben@Spader.us".into(), fails: 5, until: 9_000, ..WindowLockFile::default() };
        write_window_lock(&path, &file).unwrap();
        let loaded = read_window_lock(&path);
        assert_eq!(lockout_for(&loaded, "ben@spader.us"), (5, 9_000));
        assert_eq!(lockout_for(&loaded, "other@spader.us"), (0, 0));
        file.fails = 0;
        file.until = 0;
        write_window_lock(&path, &file).unwrap();
        assert_eq!(read_window_lock(&path).fails, 0);
        std::fs::write(&path, r#"{"hello":true,"email":"ben@spader.us","fails":2,"until":3}"#).unwrap();
        let old = read_window_lock(&path);
        assert_eq!(old.email, "ben@spader.us");
        assert_eq!(old.fails, 2);
        assert_eq!(old.until, 3);
        let saved = serde_json::to_string(&old).unwrap();
        assert!(!saved.contains("hello"));
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_window_lock(&path), WindowLockFile::default());
    }
}
