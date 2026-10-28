//! A PJDesk server's copy of one pjdesk.com account.
//!
//! pjdesk.com remains the source of truth for account creation, password
//! changes, plans, and admin. The primary keeps this copy and answers
//! sign-in, token refresh, and saved-list reads and writes itself. The
//! standby keeps the same copy. The primary pushes local edits and pulls
//! website edits in batches.
//!
//! The password never appears here. Sign-in sends the same auth key the
//! cloud login uses. This copy stores only the salted PBKDF2 hash of that
//! key (100,000 iterations, SHA-256), the same verifier the Worker stores.
//! Refresh tokens are stored as SHA-256 hashes. Saved passwords stay in the
//! sealed vault. The service-wide Ed25519 private key is not in this copy.
//! `sign_pkcs8` is a per-network key so the primary and the standby can
//! issue access tokens for this network only.

use ring::pbkdf2;
use ring::signature::KeyPair;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;

use crate::access::{self, Claims};

/// Worker PBKDF2 iterations for the auth-key verifier. Not the 600,000 used
/// to derive the auth key from the password.
pub const AUTH_HASH_ITERS: u32 = 100_000;
/// A network refresh token is accepted for a year, matching the Worker.
pub const REFRESH_TTL_SECS: u64 = 365 * 24 * 3600;
/// Wrong passwords for one account before sign-in waits.
pub const LOGIN_FAILS: usize = 10;
/// How long those failures count.
pub const LOGIN_WINDOW_SECS: u64 = 15 * 60;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountCopy {
    /// SHA-256 of the cloud snapshot, or empty before the first pull.
    #[serde(default)]
    pub revision: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub auth_hash: String,
    #[serde(default)]
    pub auth_salt: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub plan_expires_at: u64,
    #[serde(default)]
    pub sync_enabled: bool,
    #[serde(default)]
    pub network: NetworkCopy,
    /// Owner and members. The password is not here.
    #[serde(default)]
    pub credentials: Vec<Credential>,
    #[serde(default)]
    pub devices: Vec<DeviceRow>,
    #[serde(default)]
    pub computers: Vec<SavedComputer>,
    #[serde(default)]
    pub removed: Vec<RemovedRow>,
    #[serde(default)]
    pub vaults: Vec<SealedRow>,
    #[serde(default)]
    pub locks: Vec<SealedRow>,
    #[serde(default)]
    pub members: Vec<MemberRow>,
    /// Network invite keys. `secret_hash` only.
    #[serde(default)]
    pub invites: Vec<InviteRow>,
    /// This server's access keys. Not on pjdesk.com. Kept across a pull.
    #[serde(default)]
    pub server_keys: Vec<InviteRow>,
    /// Cloud token public key, hex. Verifies tokens pjdesk.com signed.
    #[serde(default)]
    pub cloud_public: String,
    /// Per-network public key, hex.
    #[serde(default)]
    pub sign_public: String,
    /// Per-network PKCS#8 private key, hex. Not the service-wide key.
    #[serde(default)]
    pub sign_pkcs8: String,
    /// Local failed sign-ins. Not uploaded.
    #[serde(default)]
    pub login_fails: Vec<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkCopy {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub active_address: String,
    #[serde(default)]
    pub active_fingerprint: String,
    #[serde(default)]
    pub standby_address: String,
    #[serde(default)]
    pub standby_fingerprint: String,
    #[serde(default)]
    pub require_accounts: bool,
    #[serde(default)]
    pub active_lan: String,
    #[serde(default)]
    pub standby_lan: String,
    #[serde(default)]
    pub active_public_ip: String,
    #[serde(default)]
    pub standby_public_ip: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub auth_hash: String,
    #[serde(default)]
    pub auth_salt: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRow {
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub name: String,
    /// SHA-256 of the refresh token text. The token itself is not stored.
    #[serde(default)]
    pub refresh_hash: String,
    #[serde(default)]
    pub enrolled_at: u64,
    /// Non-zero when the device is revoked.
    #[serde(default)]
    pub revoked_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedComputer {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os_family: String,
    #[serde(default)]
    pub os_version: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedRow {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedRow {
    #[serde(default)]
    pub account_id: String,
    /// Ciphertext. Empty clears the copy. The vault key is not stored.
    #[serde(default)]
    pub ciphertext: String,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRow {
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub revoked_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteRow {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// Hash of the secret. The secret is not stored.
    #[serde(default)]
    pub secret_hash: String,
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub revoked_at: u64,
    #[serde(default)]
    pub computer_limit: u32,
}

/// A local edit waiting for the next batch. Website-owned fields are not
/// a change: password hashes and the plan always come from pjdesk.com.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AccountChange {
    Computer { computer: SavedComputer },
    Remove { id: String, at: u64 },
    Vault { row: SealedRow },
    Lock { row: SealedRow },
    Device { device: DeviceRow },
    RevokeDevice { device_id: String, at: u64 },
    Address {
        active_address: String,
        active_fingerprint: String,
        at: u64,
    },
    /// Replaces the per-network signing key. The previous private key is discarded.
    SignKey { public_key: String, pkcs8: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionOut {
    pub access: String,
    pub exp: u64,
    pub refresh: String,
    pub email: String,
    pub role: String,
    pub device_id: String,
    pub account_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// Wrong password, or an email this copy does not have.
    BadPassword,
    /// Too many failures. Seconds to wait.
    Pause(u64),
    /// No verifier yet, and the cloud pull did not fill one.
    NoCopy,
    BadRequest(&'static str),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::BadPassword => f.write_str("Wrong email or password."),
            AuthError::Pause(secs) => write!(f, "{}", sign_in_limit_message(*secs)),
            AuthError::NoCopy => f.write_str(
                "This server does not have the account yet, and pjdesk.com did not answer.",
            ),
            AuthError::BadRequest(msg) => f.write_str(msg),
        }
    }
}

pub fn sign_in_limit_message(seconds: u64) -> String {
    let minutes = (seconds.saturating_add(59) / 60).max(1);
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    format!("Too many sign-in attempts. Try again in {minutes} {unit}.")
}

/// Seconds until the oldest failure that still fills the window drops out.
pub fn retry_after(fails: &[u64], now: u64) -> u64 {
    let cutoff = now.saturating_sub(LOGIN_WINDOW_SECS);
    let mut live: Vec<u64> = fails.iter().copied().filter(|at| *at > cutoff).collect();
    if live.len() < LOGIN_FAILS {
        return 0;
    }
    live.sort_unstable();
    let at = live[live.len() - LOGIN_FAILS];
    at.saturating_add(LOGIN_WINDOW_SECS).saturating_sub(now).max(1)
}

pub fn note_failure(fails: &mut Vec<u64>, now: u64) {
    let cutoff = now.saturating_sub(LOGIN_WINDOW_SECS);
    fails.retain(|at| *at > cutoff);
    fails.push(now);
}

/// True when the auth key matches the stored verifier. A bad key is false.
/// The password is not an argument.
pub fn auth_key_matches(auth_key_hex: &str, salt_hex: &str, hash_hex: &str) -> bool {
    let Ok(key) = hex::decode(auth_key_hex.trim()) else {
        return false;
    };
    let Ok(salt) = hex::decode(salt_hex.trim()) else {
        return false;
    };
    let Ok(want) = hex::decode(hash_hex.trim()) else {
        return false;
    };
    if key.len() != 32 || salt.len() != 16 || want.len() != 32 {
        return false;
    }
    let mut out = [0u8; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(AUTH_HASH_ITERS).expect("non-zero"),
        &salt,
        &key,
        &mut out,
    );
    constant_eq(&out, &want)
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// SHA-256 of the refresh token text, hex. Matches the Worker's `sha256Hex`.
pub fn refresh_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn has_verifier(copy: &AccountCopy) -> bool {
    !copy.auth_hash.is_empty() || copy.credentials.iter().any(|row| !row.auth_hash.is_empty())
}

fn verifier<'a>(copy: &'a AccountCopy, email: &str) -> Option<(&'a str, &'a str, String, String)> {
    let email = email.trim().to_lowercase();
    if let Some(row) = copy.credentials.iter().find(|row| row.email.trim().eq_ignore_ascii_case(&email)) {
        return Some((row.auth_hash.as_str(), row.auth_salt.as_str(), row.account_id.clone(), row.email.clone()));
    }
    if copy.email.trim().eq_ignore_ascii_case(&email) && !copy.auth_hash.is_empty() {
        return Some((copy.auth_hash.as_str(), copy.auth_salt.as_str(), copy.account_id.clone(), copy.email.clone()));
    }
    None
}

/// Checks the password verifier and records a device. `refresh` is returned
/// once and stored only as a hash. A dummy hash runs when the email is
/// unknown so the timing is closer to a real check.
pub fn sign_in(
    copy: &mut AccountCopy,
    email: &str,
    auth_key_hex: &str,
    device_id: &str,
    device_name: &str,
    refresh: &str,
    now: u64,
) -> Result<SessionOut, AuthError> {
    let email = email.trim().to_lowercase();
    if email.is_empty() || auth_key_hex.len() != 64 || device_id.trim().is_empty() || refresh.len() != 64 {
        return Err(AuthError::BadRequest("Enter your email and password."));
    }
    let wait = retry_after(&copy.login_fails, now);
    if wait > 0 {
        return Err(AuthError::Pause(wait));
    }
    if !has_verifier(copy) {
        return Err(AuthError::NoCopy);
    }
    let found = verifier(copy, &email);
    let dummy_hash = "00".repeat(32);
    let dummy_salt = "00".repeat(16);
    let (hash, salt) = match found.as_ref() {
        Some((hash, salt, _, _)) => (*hash, *salt),
        None => (dummy_hash.as_str(), dummy_salt.as_str()),
    };
    let ok = auth_key_matches(auth_key_hex, salt, hash);
    let Some((_, _, account_id, stored_email)) = found.filter(|_| ok) else {
        note_failure(&mut copy.login_fails, now);
        return Err(AuthError::BadPassword);
    };
    copy.login_fails.clear();
    let role = role_of(copy, &stored_email);
    if role.is_empty() {
        note_failure(&mut copy.login_fails, now);
        return Err(AuthError::BadPassword);
    }
    let hash = refresh_hash(refresh);
    let device_id = device_id.trim().to_string();
    if let Some(row) = copy.devices.iter_mut().find(|row| row.device_id == device_id) {
        row.account_id = account_id.clone();
        row.email = stored_email.clone();
        row.name = device_name.trim().chars().take(80).collect();
        row.refresh_hash = hash;
        row.enrolled_at = now;
        row.revoked_at = 0;
    } else {
        copy.devices.push(DeviceRow {
            device_id: device_id.clone(),
            account_id: account_id.clone(),
            email: stored_email.clone(),
            name: device_name.trim().chars().take(80).collect(),
            refresh_hash: hash,
            enrolled_at: now,
            revoked_at: 0,
        });
    }
    let access = issue_access(copy, &stored_email, &device_id, now)?;
    Ok(SessionOut {
        access: access.0,
        exp: access.1,
        refresh: refresh.to_string(),
        email: stored_email,
        role,
        device_id,
        account_id,
    })
}

pub fn refresh_session(copy: &AccountCopy, refresh: &str, device_id: &str, now: u64) -> Result<SessionOut, AuthError> {
    let device_id = device_id.trim();
    if device_id.is_empty() || refresh.len() != 64 {
        return Err(AuthError::BadRequest("Sign in required."));
    }
    let hash = refresh_hash(refresh);
    let Some(row) = copy.devices.iter().find(|row| row.device_id == device_id && row.refresh_hash == hash) else {
        return Err(AuthError::BadRequest("Sign in required."));
    };
    if row.revoked_at > 0 || now.saturating_sub(row.enrolled_at) > REFRESH_TTL_SECS {
        return Err(AuthError::BadRequest("Sign in required."));
    }
    let role = role_of(copy, &row.email);
    if role.is_empty() {
        return Err(AuthError::BadRequest("Sign in required."));
    }
    let (access, exp) = issue_access(copy, &row.email, device_id, now)?;
    Ok(SessionOut {
        access,
        exp,
        refresh: refresh.to_string(),
        email: row.email.clone(),
        role,
        device_id: device_id.to_string(),
        account_id: row.account_id.clone(),
    })
}

/// Checks the verifier and does not issue a token or replace the refresh hash.
/// Used when a device already signed in and only needs to prove the password.
pub fn verify_auth(copy: &mut AccountCopy, email: &str, auth_key_hex: &str, now: u64) -> Result<(), AuthError> {
    let email = email.trim().to_lowercase();
    if email.is_empty() || auth_key_hex.len() != 64 {
        return Err(AuthError::BadRequest("Enter your email and password."));
    }
    let wait = retry_after(&copy.login_fails, now);
    if wait > 0 {
        return Err(AuthError::Pause(wait));
    }
    if !has_verifier(copy) {
        return Err(AuthError::NoCopy);
    }
    let found = verifier(copy, &email);
    let dummy_hash = "00".repeat(32);
    let dummy_salt = "00".repeat(16);
    let (hash, salt) = match found.as_ref() {
        Some((hash, salt, _, _)) => (*hash, *salt),
        None => (dummy_hash.as_str(), dummy_salt.as_str()),
    };
    if found.filter(|_| auth_key_matches(auth_key_hex, salt, hash)).is_none() {
        note_failure(&mut copy.login_fails, now);
        return Err(AuthError::BadPassword);
    }
    copy.login_fails.clear();
    Ok(())
}

fn role_of(copy: &AccountCopy, email: &str) -> String {
    if copy.email.trim().eq_ignore_ascii_case(email.trim()) {
        return "owner".into();
    }
    let member = copy.members.iter().find(|row| row.email.trim().eq_ignore_ascii_case(email.trim()));
    match member {
        Some(row) if row.revoked_at == 0 => "member".into(),
        _ => String::new(),
    }
}

fn issue_access(copy: &AccountCopy, email: &str, device_id: &str, now: u64) -> Result<(String, u64), AuthError> {
    let pkcs8 = hex::decode(copy.sign_pkcs8.trim()).map_err(|_| AuthError::NoCopy)?;
    let exp = now.saturating_add(access::TOKEN_TTL_SECS);
    let mut fps = Vec::new();
    if !copy.network.active_fingerprint.is_empty() {
        fps.push(copy.network.active_fingerprint.clone());
    }
    if !copy.network.standby_fingerprint.is_empty() {
        fps.push(copy.network.standby_fingerprint.clone());
    }
    let claims = Claims {
        email: email.to_string(),
        network_id: copy.network.id.clone(),
        servers: fps,
        device_id: device_id.to_string(),
        exp,
    };
    let (token, _) = access::sign_pkcs8(&pkcs8, &claims).map_err(|_| AuthError::NoCopy)?;
    Ok((token, exp))
}

/// Public keys that accept an access token: the cloud key and this network's key.
pub fn verify_keys(copy: &AccountCopy) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for hex_key in [&copy.cloud_public, &copy.sign_public] {
        if let Ok(bytes) = hex::decode(hex_key.trim()) {
            if bytes.len() == 32 {
                out.push(bytes);
            }
        }
    }
    out
}

/// Applies pjdesk.com's snapshot, then local edits that are still newer.
/// Password hashes, the plan, and revoked members come from `cloud`.
/// A newer local saved row, vault, or device enrollment is kept.
/// `server_keys` and `login_fails` stay local.
pub fn merge(cloud: &AccountCopy, local: &AccountCopy, pending: &[AccountChange]) -> AccountCopy {
    let mut next = cloud.clone();
    next.server_keys = local.server_keys.clone();
    next.login_fails = local.login_fails.clone();
    if next.sign_pkcs8.is_empty() {
        next.sign_pkcs8 = local.sign_pkcs8.clone();
        next.sign_public = local.sign_public.clone();
    }
    if next.cloud_public.is_empty() {
        next.cloud_public = local.cloud_public.clone();
    }
    // Replay only edits that beat the cloud row. A website change with a
    // newer stamp wins, including a revocation.
    for change in pending {
        apply_change(&mut next, change);
    }
    next
}

pub fn apply_change(copy: &mut AccountCopy, change: &AccountChange) {
    match change {
        AccountChange::Computer { computer } => {
            let id = computer.id.trim();
            if id.len() != 9 {
                return;
            }
            if let Some(gone) = copy.removed.iter().find(|row| row.id == id) {
                if gone.at >= computer.updated_at {
                    return;
                }
            }
            if let Some(row) = copy.computers.iter_mut().find(|row| row.id == id && row.account_id == computer.account_id)
            {
                if computer.updated_at >= row.updated_at {
                    *row = computer.clone();
                }
            } else {
                copy.computers.push(computer.clone());
            }
        }
        AccountChange::Remove { id, at } => {
            let id = id.trim();
            if id.is_empty() {
                return;
            }
            copy.computers.retain(|row| row.id != id || row.updated_at > *at);
            if let Some(row) = copy.removed.iter_mut().find(|row| row.id == id) {
                if *at >= row.at {
                    row.at = *at;
                }
            } else {
                copy.removed.push(RemovedRow { id: id.to_string(), at: *at });
            }
        }
        AccountChange::Vault { row } => upsert_sealed(&mut copy.vaults, row),
        AccountChange::Lock { row } => upsert_sealed(&mut copy.locks, row),
        AccountChange::Device { device } => {
            if device.device_id.is_empty() || device.refresh_hash.is_empty() {
                return;
            }
            if let Some(row) = copy.devices.iter_mut().find(|row| row.device_id == device.device_id) {
                if row.revoked_at > 0 && device.revoked_at == 0 && row.revoked_at >= device.enrolled_at {
                    return;
                }
                if device.enrolled_at >= row.enrolled_at {
                    *row = device.clone();
                }
            } else {
                copy.devices.push(device.clone());
            }
        }
        AccountChange::RevokeDevice { device_id, at } => {
            if let Some(row) = copy.devices.iter_mut().find(|row| row.device_id == *device_id) {
                if *at >= row.enrolled_at {
                    row.revoked_at = *at;
                    row.refresh_hash.clear();
                }
            }
        }
        AccountChange::Address { active_address, active_fingerprint, at: _ } => {
            if copy.network.active_address.trim().is_empty() && !active_address.trim().is_empty() {
                copy.network.active_address = active_address.clone();
                copy.network.active_fingerprint = active_fingerprint.clone();
            }
        }
        AccountChange::SignKey { public_key, pkcs8 } => {
            let public_key = public_key.trim();
            let pkcs8 = pkcs8.trim();
            if public_key.len() == 64 && !pkcs8.is_empty() {
                copy.sign_public = public_key.to_string();
                copy.sign_pkcs8 = pkcs8.to_string();
            }
        }
    }
}

/// Drop local-only fields before the copy is sent to the standby or the cloud.
pub fn for_replica(copy: &AccountCopy) -> AccountCopy {
    let mut next = copy.clone();
    next.login_fails.clear();
    next
}

pub fn computers_for<'a>(copy: &'a AccountCopy, account_id: &str) -> Vec<&'a SavedComputer> {
    copy.computers.iter().filter(|row| row.account_id == account_id).collect()
}

pub fn vault_for<'a>(copy: &'a AccountCopy, account_id: &str) -> &'a str {
    copy.vaults
        .iter()
        .find(|row| row.account_id == account_id)
        .map(|row| row.ciphertext.as_str())
        .unwrap_or("")
}

/// Generates a per-network signing key when the cloud has not sent one.
/// The service-wide key is never created here.
pub fn ensure_sign_key(copy: &mut AccountCopy) {
    if !copy.sign_pkcs8.is_empty() {
        return;
    }
    if let Some((pkcs8, public_key)) = generate_sign_key() {
        copy.sign_pkcs8 = pkcs8;
        copy.sign_public = public_key;
    }
}

/// A new Ed25519 key, as `(pkcs8 hex, public hex)`. Nothing is logged.
pub fn generate_sign_key() -> Option<(String, String)> {
    let rng = ring::rand::SystemRandom::new();
    let doc = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).ok()?;
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(doc.as_ref()).ok()?;
    Some((hex::encode(doc.as_ref()), hex::encode(pair.public_key().as_ref())))
}

fn upsert_sealed(list: &mut Vec<SealedRow>, row: &SealedRow) {
    if row.account_id.is_empty() {
        return;
    }
    if let Some(existing) = list.iter_mut().find(|item| item.account_id == row.account_id) {
        if row.updated_at >= existing.updated_at {
            *existing = row.clone();
        }
    } else {
        list.push(row.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner_copy() -> AccountCopy {
        let mut copy = AccountCopy {
            revision: "aaa".into(),
            account_id: "acct".into(),
            email: "ben@spader.us".into(),
            auth_hash: "2c6e5e75f9481b8fb097e8d6ac5c11fd0e33654eb5a3cc18ebc53672365e1053".into(),
            auth_salt: "22".repeat(16),
            plan: "cloud".into(),
            sync_enabled: true,
            network: NetworkCopy {
                id: "net".into(),
                active_address: "203.0.113.10:21200".into(),
                active_fingerprint: "ab".repeat(32),
                ..NetworkCopy::default()
            },
            ..AccountCopy::default()
        };
        copy.credentials.push(Credential {
            account_id: "acct".into(),
            email: "ben@spader.us".into(),
            auth_hash: copy.auth_hash.clone(),
            auth_salt: copy.auth_salt.clone(),
        });
        ensure_sign_key(&mut copy);
        copy
    }

    #[test]
    fn the_auth_key_verifier_matches_the_cloud_and_a_password_is_not_stored() {
        let key = "11".repeat(32);
        let salt = "22".repeat(16);
        let hash = "2c6e5e75f9481b8fb097e8d6ac5c11fd0e33654eb5a3cc18ebc53672365e1053";
        assert!(auth_key_matches(&key, &salt, hash));
        assert!(!auth_key_matches(&"33".repeat(32), &salt, hash));
        let raw = serde_json::to_string(&owner_copy()).unwrap();
        assert!(!raw.contains("password"));
        assert!(raw.contains("auth_hash"));
    }

    #[test]
    fn sign_in_issues_a_token_and_stores_only_the_refresh_hash() {
        let mut copy = owner_copy();
        let refresh = "ab".repeat(32);
        let session = sign_in(&mut copy, "Ben@Spader.us", &"11".repeat(32), "device-1", "Surface", &refresh, 1_700_000_000).unwrap();
        assert_eq!(session.role, "owner");
        assert_eq!(session.refresh, refresh);
        assert!(session.access.contains('.'));
        assert_eq!(copy.devices[0].refresh_hash, refresh_hash(&refresh));
        assert_ne!(copy.devices[0].refresh_hash, refresh);
        assert!(copy.login_fails.is_empty());
        let again = refresh_session(&copy, &refresh, "device-1", 1_700_000_100).unwrap();
        assert_eq!(again.email, "ben@spader.us");
        assert!(refresh_session(&copy, &"cd".repeat(32), "device-1", 1_700_000_100).is_err());
    }

    #[test]
    fn ten_failures_pause_and_a_later_cloud_password_replaces_the_old_verifier() {
        let mut copy = owner_copy();
        for n in 0..10 {
            let err = sign_in(&mut copy, "ben@spader.us", &"33".repeat(32), "device-1", "Surface", &"ab".repeat(32), 1_000 + n).unwrap_err();
            assert!(matches!(err, AuthError::BadPassword));
        }
        let paused = sign_in(&mut copy, "ben@spader.us", &"11".repeat(32), "device-1", "Surface", &"ab".repeat(32), 1_020).unwrap_err();
        assert!(matches!(paused, AuthError::Pause(_)));
        assert!(sign_in_limit_message(90).contains("2 minutes"));

        let mut cloud = owner_copy();
        cloud.auth_hash = "ff".repeat(32);
        cloud.credentials[0].auth_hash = cloud.auth_hash.clone();
        cloud.revision = "bbb".into();
        cloud.computers.push(SavedComputer {
            id: "123456789".into(),
            account_id: "acct".into(),
            name: "Website".into(),
            updated_at: 50,
            ..SavedComputer::default()
        });
        let pending = vec![AccountChange::Computer {
            computer: SavedComputer {
                id: "123456789".into(),
                account_id: "acct".into(),
                name: "Server".into(),
                updated_at: 80,
                ..SavedComputer::default()
            },
        }];
        let merged = merge(&cloud, &copy, &pending);
        assert_eq!(merged.auth_hash, cloud.auth_hash, "a password change on the website wins");
        assert_eq!(merged.computers[0].name, "Server", "a newer saved-list edit is kept");
        assert!(!merged.sign_pkcs8.is_empty());
        assert!(merged.login_fails.len() == copy.login_fails.len());
    }

    #[test]
    fn a_rotated_sign_key_replaces_the_key_from_the_cloud() {
        let mut cloud = owner_copy();
        ensure_sign_key(&mut cloud);
        let old = cloud.sign_pkcs8.clone();
        let mut local = cloud.clone();
        local.sign_pkcs8.clear();
        local.sign_public.clear();
        ensure_sign_key(&mut local);
        let public_key = local.sign_public.clone();
        let pkcs8 = local.sign_pkcs8.clone();
        let merged = merge(
            &cloud,
            &local,
            &[AccountChange::SignKey { public_key: public_key.clone(), pkcs8: pkcs8.clone() }],
        );
        assert_eq!(merged.sign_public, public_key);
        assert_eq!(merged.sign_pkcs8, pkcs8);
        assert_ne!(merged.sign_pkcs8, old);
    }

    #[test]
    fn a_removal_newer_than_the_row_sticks_and_a_revoked_device_is_not_revived() {
        let mut copy = owner_copy();
        apply_change(
            &mut copy,
            &AccountChange::Computer {
                computer: SavedComputer {
                    id: "123456789".into(),
                    account_id: "acct".into(),
                    updated_at: 10,
                    ..SavedComputer::default()
                },
            },
        );
        apply_change(&mut copy, &AccountChange::Remove { id: "123456789".into(), at: 20 });
        assert!(copy.computers.is_empty());
        apply_change(
            &mut copy,
            &AccountChange::Device {
                device: DeviceRow {
                    device_id: "d".into(),
                    refresh_hash: "aa".into(),
                    enrolled_at: 5,
                    revoked_at: 0,
                    ..DeviceRow::default()
                },
            },
        );
        apply_change(&mut copy, &AccountChange::RevokeDevice { device_id: "d".into(), at: 30 });
        apply_change(
            &mut copy,
            &AccountChange::Device {
                device: DeviceRow {
                    device_id: "d".into(),
                    refresh_hash: "bb".into(),
                    enrolled_at: 10,
                    revoked_at: 0,
                    ..DeviceRow::default()
                },
            },
        );
        assert_eq!(copy.devices[0].revoked_at, 30);
        assert!(copy.devices[0].refresh_hash.is_empty());
    }
}
