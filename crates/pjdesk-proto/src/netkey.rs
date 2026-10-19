//! A copy-paste network key (`pjdesk-net1:…`).
//!
//! The string is an encrypted blob. A passphrase-protected key keeps the key
//! out of the string: the recipient types a passphrase that was shared
//! separately, and Argon2id turns it into the XChaCha20-Poly1305 key. A
//! self-contained key carries that key inside the string, so anyone who has
//! the string can open it. Either way the server address is not readable
//! without opening the blob.
//!
//! The cloud stores only a hash of the enrollment secret. This module never
//! logs the blob or the passphrase.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};

use crate::access::normalize_fingerprint;

pub const PREFIX: &str = "pjdesk-net1:";

/// Argon2id memory (KiB), iterations, and lanes used for a new passphrase key.
pub const ARGON_MEM_KIB: u32 = 19_456;
pub const ARGON_ITERATIONS: u32 = 2;
pub const ARGON_LANES: u32 = 1;

pub const KEY_REVOKED: &str = "This network key was revoked.";
pub const KEY_EXPIRED: &str = "This network key has expired.";
pub const KEY_USED_UP: &str = "This network key has already been used the maximum number of times.";
pub const KEY_BOUND: &str = "This network key is for a specific account. Sign in as that account.";
pub const KEY_INVALID: &str = "This network key is not valid.";
pub const KEY_OPEN_FAILED: &str = "That network key could not be opened. Check the text and the passphrase.";

const MODE_PASSPHRASE: u8 = 1;
const MODE_EMBEDDED: u8 = 2;
const VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Passphrase,
    Embedded,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Opened {
    pub network_id: String,
    pub secret_hex: String,
    pub active: String,
    pub active_fingerprint: String,
    pub standby: String,
    pub standby_fingerprint: String,
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened")
            .field("network_id", &self.network_id)
            .field("active", &self.active)
            .field("standby", &self.standby)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetKeyError {
    Format,
    CouldNotOpen,
}

impl std::fmt::Display for NetKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NetKeyError::Format => KEY_INVALID,
            NetKeyError::CouldNotOpen => KEY_OPEN_FAILED,
        })
    }
}

#[derive(Clone)]
pub struct Contents<'a> {
    pub network_id: &'a str,
    pub active: &'a str,
    pub active_fingerprint: &'a str,
    pub standby: &'a str,
    pub standby_fingerprint: &'a str,
}

#[derive(Clone, Copy)]
pub struct Argon {
    pub mem_kib: u32,
    pub iterations: u32,
    pub lanes: u32,
}

impl Default for Argon {
    fn default() -> Self {
        Self { mem_kib: ARGON_MEM_KIB, iterations: ARGON_ITERATIONS, lanes: ARGON_LANES }
    }
}

/// Why a stored network key cannot be redeemed. `None` means it can.
pub fn invite_error(
    revoked: bool,
    expires_at: Option<u64>,
    uses: u32,
    max_uses: Option<u32>,
    now: u64,
    bound_email: &str,
    caller_email: Option<&str>,
) -> Option<&'static str> {
    if revoked {
        return Some(KEY_REVOKED);
    }
    if let Some(exp) = expires_at {
        if now >= exp {
            return Some(KEY_EXPIRED);
        }
    }
    if let Some(max) = max_uses {
        if uses >= max {
            return Some(KEY_USED_UP);
        }
    }
    let bound = bound_email.trim();
    if !bound.is_empty() {
        let caller = caller_email.unwrap_or("").trim();
        if caller.is_empty() || !caller.eq_ignore_ascii_case(bound) {
            return Some(KEY_BOUND);
        }
    }
    None
}

/// SHA-256 of the lowercase hex enrollment secret. This is what the cloud stores.
pub fn secret_hash(secret_hex: &str) -> String {
    hex::encode(Sha256::digest(secret_hex.trim().to_ascii_lowercase().as_bytes()))
}

pub fn needs_passphrase(blob: &str) -> bool {
    matches!(kind(blob), Ok(KeyKind::Passphrase))
}

pub fn is_embedded(blob: &str) -> bool {
    matches!(kind(blob), Ok(KeyKind::Embedded))
}

/// Pulls a `pjdesk-net1:` blob out of clipboard text or a file name.
///
/// The returned string is still the encrypted blob. A RustDesk-style
/// `host=` / reversed-base64 config is ignored, so a file name never carries
/// the server address in the clear.
pub fn blob_from_text(text: &str) -> Option<String> {
    let idx = text.find(PREFIX)?;
    let rest = &text[idx + PREFIX.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(rest.len());
    if end < 16 {
        return None;
    }
    Some(format!("{PREFIX}{}", &rest[..end]))
}

pub fn kind(blob: &str) -> Result<KeyKind, NetKeyError> {
    let raw = decode_blob(blob)?;
    match raw.get(1).copied() {
        Some(MODE_PASSPHRASE) => Ok(KeyKind::Passphrase),
        Some(MODE_EMBEDDED) => Ok(KeyKind::Embedded),
        _ => Err(NetKeyError::Format),
    }
}

pub fn seal_passphrase(contents: &Contents<'_>, secret: &[u8; 32], passphrase: &str) -> Result<String, NetKeyError> {
    seal_passphrase_with(contents, secret, passphrase, &crate::random_bytes(), &crate::random_bytes(), Argon::default())
}

pub fn seal_passphrase_with(
    contents: &Contents<'_>,
    secret: &[u8; 32],
    passphrase: &str,
    salt: &[u8; 16],
    nonce: &[u8; 24],
    argon: Argon,
) -> Result<String, NetKeyError> {
    if passphrase.is_empty() {
        return Err(NetKeyError::CouldNotOpen);
    }
    check_argon(argon)?;
    let mut key = derive(passphrase, salt, argon)?;
    let plain = canonical(contents, secret)?;
    let mut header = Vec::with_capacity(30);
    header.push(VERSION);
    header.push(MODE_PASSPHRASE);
    header.extend_from_slice(salt);
    header.extend_from_slice(&argon.mem_kib.to_le_bytes());
    header.extend_from_slice(&argon.iterations.to_le_bytes());
    header.extend_from_slice(&argon.lanes.to_le_bytes());
    let ct = xchacha(&key, nonce, &header, &plain)?;
    key.fill(0);
    let mut body = header;
    body.extend_from_slice(nonce);
    body.extend_from_slice(&ct);
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(body)))
}

pub fn seal_embedded(contents: &Contents<'_>, secret: &[u8; 32]) -> Result<String, NetKeyError> {
    seal_embedded_with(contents, secret, &crate::random_bytes(), &crate::random_bytes())
}

pub fn seal_embedded_with(
    contents: &Contents<'_>,
    secret: &[u8; 32],
    key: &[u8; 32],
    nonce: &[u8; 24],
) -> Result<String, NetKeyError> {
    let plain = canonical(contents, secret)?;
    let mut header = Vec::with_capacity(34);
    header.push(VERSION);
    header.push(MODE_EMBEDDED);
    header.extend_from_slice(key);
    let ct = xchacha(key, nonce, &header, &plain)?;
    let mut body = header;
    body.extend_from_slice(nonce);
    body.extend_from_slice(&ct);
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(body)))
}

pub fn open(blob: &str, passphrase: &str) -> Result<Opened, NetKeyError> {
    let raw = decode_blob(blob)?;
    if raw.first().copied() != Some(VERSION) || raw.len() < 4 {
        return Err(NetKeyError::Format);
    }
    let (key, aad_end, nonce_end) = match raw[1] {
        MODE_PASSPHRASE => {
            if passphrase.is_empty() || raw.len() < 54 + 16 {
                return Err(NetKeyError::CouldNotOpen);
            }
            let salt: [u8; 16] = raw[2..18].try_into().map_err(|_| NetKeyError::Format)?;
            let argon = Argon {
                mem_kib: u32::from_le_bytes(raw[18..22].try_into().map_err(|_| NetKeyError::Format)?),
                iterations: u32::from_le_bytes(raw[22..26].try_into().map_err(|_| NetKeyError::Format)?),
                lanes: u32::from_le_bytes(raw[26..30].try_into().map_err(|_| NetKeyError::Format)?),
            };
            check_argon(argon)?;
            (derive(passphrase, &salt, argon)?, 30, 54)
        }
        MODE_EMBEDDED => {
            if raw.len() < 58 + 16 {
                return Err(NetKeyError::Format);
            }
            let key: [u8; 32] = raw[2..34].try_into().map_err(|_| NetKeyError::Format)?;
            (key, 34, 58)
        }
        _ => return Err(NetKeyError::Format),
    };
    let nonce: [u8; 24] = raw.get(aad_end..nonce_end).ok_or(NetKeyError::Format)?.try_into().map_err(|_| NetKeyError::Format)?;
    let ct = raw.get(nonce_end..).ok_or(NetKeyError::Format)?.to_vec();
    let aad = raw[..aad_end].to_vec();
    let plain = decrypt(&key, &nonce, &aad, &ct);
    let mut key = key;
    key.fill(0);
    parse_plain(&plain?)
}

fn decode_blob(blob: &str) -> Result<Vec<u8>, NetKeyError> {
    let compact: String = blob.chars().filter(|c| !c.is_whitespace()).collect();
    let rest = compact.strip_prefix(PREFIX).ok_or(NetKeyError::Format)?;
    if rest.is_empty() || rest.len() > 12_000 {
        return Err(NetKeyError::Format);
    }
    let raw = URL_SAFE_NO_PAD.decode(rest).map_err(|_| NetKeyError::Format)?;
    if raw.len() > 8_192 {
        return Err(NetKeyError::Format);
    }
    Ok(raw)
}

fn check_argon(argon: Argon) -> Result<(), NetKeyError> {
    if argon.lanes == 0 || argon.lanes > 4 || argon.iterations == 0 || argon.iterations > 8 {
        return Err(NetKeyError::Format);
    }
    if argon.mem_kib < 8 * argon.lanes || argon.mem_kib > 64 * 1024 {
        return Err(NetKeyError::Format);
    }
    Ok(())
}

fn derive(passphrase: &str, salt: &[u8; 16], argon: Argon) -> Result<[u8; 32], NetKeyError> {
    let params = argon2::Params::new(argon.mem_kib, argon.iterations, argon.lanes, Some(32)).map_err(|_| NetKeyError::Format)?;
    let ctx = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; 32];
    ctx.hash_password_into(passphrase.as_bytes(), salt, &mut key).map_err(|_| NetKeyError::CouldNotOpen)?;
    Ok(key)
}

fn xchacha(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], plain: &[u8]) -> Result<Vec<u8>, NetKeyError> {
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .encrypt(XNonce::from_slice(nonce), Payload { msg: plain, aad })
        .map_err(|_| NetKeyError::CouldNotOpen)
}

fn decrypt(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], ct: &[u8]) -> Result<Vec<u8>, NetKeyError> {
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher.decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad }).map_err(|_| NetKeyError::CouldNotOpen)
}

fn canonical(contents: &Contents<'_>, secret: &[u8; 32]) -> Result<Vec<u8>, NetKeyError> {
    let network_id = contents.network_id.trim();
    let active = contents.active.trim();
    let standby = contents.standby.trim();
    if network_id.is_empty()
        || network_id.len() > 80
        || active.is_empty()
        || active.len() > 200
        || standby.len() > 200
        || network_id.contains('\n')
        || active.contains('\n')
        || standby.contains('\n')
    {
        return Err(NetKeyError::Format);
    }
    let text = format!(
        "PJNK1\n{network_id}\n{}\n{active}\n{}\n{standby}\n{}\n",
        hex::encode(secret),
        normalize_fingerprint(contents.active_fingerprint),
        normalize_fingerprint(contents.standby_fingerprint),
    );
    Ok(text.into_bytes())
}

fn parse_plain(plain: &[u8]) -> Result<Opened, NetKeyError> {
    let text = std::str::from_utf8(plain).map_err(|_| NetKeyError::Format)?;
    let mut lines = text.split('\n');
    if lines.next() != Some("PJNK1") {
        return Err(NetKeyError::Format);
    }
    let network_id = lines.next().unwrap_or("").to_string();
    let secret_hex = lines.next().unwrap_or("").to_string();
    let active = lines.next().unwrap_or("").to_string();
    let active_fingerprint = lines.next().unwrap_or("").to_string();
    let standby = lines.next().unwrap_or("").to_string();
    let standby_fingerprint = lines.next().unwrap_or("").to_string();
    if network_id.is_empty() || active.is_empty() || secret_hex.len() != 64 || hex::decode(&secret_hex).map(|b| b.len() != 32).unwrap_or(true)
    {
        return Err(NetKeyError::Format);
    }
    Ok(Opened { network_id, secret_hex, active, active_fingerprint, standby, standby_fingerprint })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Contents<'static> {
        Contents {
            network_id: "net_1",
            active: "office.example:21200",
            active_fingerprint: "sha256:AB-CD",
            standby: "cabin.example:21200",
            standby_fingerprint: "00112233",
        }
    }

    fn secret() -> [u8; 32] {
        [0x11; 32]
    }

    fn argon_test() -> Argon {
        Argon { mem_kib: 32, iterations: 2, lanes: 1 }
    }

    #[test]
    fn round_trip_passphrase_and_embedded() {
        let salt = [0x22; 16];
        let nonce = [0x33; 24];
        let blob = seal_passphrase_with(&sample(), &secret(), "correct horse", &salt, &nonce, argon_test()).unwrap();
        assert!(blob.starts_with(PREFIX));
        assert!(!blob.contains("office.example"));
        assert!(!blob.contains("cabin.example"));
        assert_eq!(kind(&blob).unwrap(), KeyKind::Passphrase);
        let opened = open(&blob, "correct horse").unwrap();
        assert_eq!(opened.network_id, "net_1");
        assert_eq!(opened.active, "office.example:21200");
        assert_eq!(opened.active_fingerprint, "abcd");
        assert_eq!(opened.standby, "cabin.example:21200");
        assert_eq!(opened.standby_fingerprint, "00112233");
        assert_eq!(opened.secret_hex, hex::encode(secret()));
        assert!(!format!("{opened:?}").contains(&opened.secret_hex));

        let key = [0x44; 32];
        let embedded = seal_embedded_with(&sample(), &secret(), &key, &nonce).unwrap();
        assert!(!embedded.contains("office.example"));
        assert!(is_embedded(&embedded));
        let opened = open(&embedded, "").unwrap();
        assert_eq!(opened.active, "office.example:21200");
        assert_eq!(opened.secret_hex, hex::encode(secret()));
    }

    #[test]
    fn wrong_passphrase_and_tampered_blob_do_not_open() {
        let salt = [0x22; 16];
        let nonce = [0x33; 24];
        let blob = seal_passphrase_with(&sample(), &secret(), "correct horse", &salt, &nonce, argon_test()).unwrap();
        assert_eq!(open(&blob, "wrong passphrase"), Err(NetKeyError::CouldNotOpen));
        assert_eq!(open(&blob, ""), Err(NetKeyError::CouldNotOpen));

        let mut raw = URL_SAFE_NO_PAD.decode(blob.strip_prefix(PREFIX).unwrap()).unwrap();
        let n = raw.len() - 1;
        raw[n] ^= 0x01;
        let tampered = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(raw));
        assert_eq!(open(&tampered, "correct horse"), Err(NetKeyError::CouldNotOpen));
    }

    #[test]
    fn filename_and_clipboard_keep_the_encrypted_blob() {
        let blob = seal_embedded_with(&sample(), &secret(), &[0x44; 32], &[0x33; 24]).unwrap();
        assert!(!blob.contains("office.example"));
        let name = format!("PJDesk-Setup-0.6.0--{blob}.exe");
        // NTFS allows 255 characters in one file name. A real encrypted blob
        // plus the setup prefix is longer than that, so the rename often
        // cannot be created. The parser still has to accept it when the
        // name does fit, and must not fall back to a plaintext host string.
        assert!(name.len() > 255);
        assert_eq!(blob_from_text(&name).as_deref(), Some(blob.as_str()));
        let copied = format!(r"C:\Users\Ben\Downloads\PJDesk-Setup-0.6.0--{blob} (1).exe");
        assert_eq!(blob_from_text(&copied).as_deref(), Some(blob.as_str()));
        assert_eq!(blob_from_text(&format!("\n{blob} \n")).as_deref(), Some(blob.as_str()));
        assert!(blob_from_text("PJDesk-Setup-0.6.0--host=office.example,key=abc.exe").is_none());
        assert!(blob_from_text("just some text").is_none());
    }

    #[test]
    fn invite_rules_expired_revoked_and_used_up() {
        assert_eq!(invite_error(true, None, 0, None, 10, "", None), Some(KEY_REVOKED));
        assert_eq!(invite_error(false, Some(10), 0, None, 10, "", None), Some(KEY_EXPIRED));
        assert_eq!(invite_error(false, Some(11), 0, None, 10, "", None), None);
        assert_eq!(invite_error(false, None, 2, Some(2), 10, "", None), Some(KEY_USED_UP));
        assert_eq!(invite_error(false, None, 1, Some(2), 10, "", None), None);
        assert_eq!(
            invite_error(false, None, 0, None, 10, "Ben@Spader.us", None),
            Some(KEY_BOUND)
        );
        assert_eq!(invite_error(false, None, 0, None, 10, "ben@spader.us", Some("ben@spader.us")), None);
        assert_eq!(
            invite_error(false, None, 0, None, 10, "ben@spader.us", Some("ada@example.com")),
            Some(KEY_BOUND)
        );
    }
}
