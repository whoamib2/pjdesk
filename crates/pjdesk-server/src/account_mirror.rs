//! The primary's copy of the pjdesk.com account, and the standby's mirror.
//!
//! Sign-in, token refresh, and saved-list edits are answered from this file.
//! pjdesk.com is called from the batch, not from each client request. The
//! password is never written. See `pjdesk_proto::account_mirror`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use pjdesk_proto::account_mirror::{
    self, apply_change, ensure_sign_key, for_replica, generate_sign_key, has_verifier, merge, sign_in, vault_for,
    AccountChange,
    AccountCopy, AuthError, DeviceRow, SavedComputer, SealedRow,
};
use pjdesk_proto::failover::{cloud_edge_limited, next_cloud_wait, CLOUD_BATCH_SECS};
use pjdesk_proto::signal::{ClientMsg, FleetComputer, ServerMsg};
use pjdesk_proto::update::version_at_least;
use serde::Deserialize;
use sha2::Sha256;

use crate::cloud::{self, CloudConfig};

type HmacSha256 = Hmac<Sha256>;

/// Sign-in may wait this long for a cloud copy, then it answers and the copy finishes later.
pub const SIGN_IN_SEED_BUDGET: Duration = Duration::from_secs(5);

const MIRROR_FILE: &str = "account-mirror.json";
const JOURNAL_FILE: &str = "account-journal.json";
const PAUSE_FILE: &str = "cloud-pause.json";
const BATCH_FILE: &str = "account-batch-at.txt";
const REPLICA_FILE: &str = "account-replica-at.txt";
const STATUS_FILE: &str = "account-copy-status.json";
const REENROLL_FILE: &str = "owner-reenroll-until.txt";
const REENROLL_SECS: u64 = 120;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Pause {
    #[serde(default)]
    until: u64,
    #[serde(default)]
    wait: u64,
}

pub fn load(dir: &Path) -> AccountCopy {
    std::fs::read(dir.join(MIRROR_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(dir: &Path, copy: &AccountCopy) {
    let Ok(bytes) = serde_json::to_vec_pretty(copy) else { return };
    let path = dir.join(MIRROR_FILE);
    if std::fs::write(&path, bytes).is_ok() {
        cloud::protect(&path);
    }
}

fn journal(dir: &Path) -> Vec<AccountChange> {
    std::fs::read(dir.join(JOURNAL_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn store_journal(dir: &Path, changes: &[AccountChange]) {
    let path = dir.join(JOURNAL_FILE);
    if changes.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(changes) {
        if std::fs::write(&path, bytes).is_ok() {
            cloud::protect(&path);
        }
    }
}

fn remember(dir: &Path, change: AccountChange) {
    let mut copy = load(dir);
    apply_change(&mut copy, &change);
    ensure_sign_key(&mut copy);
    save(dir, &copy);
    let mut pending = journal(dir);
    pending.push(change);
    if pending.len() > 400 {
        pending.drain(0..pending.len() - 400);
    }
    store_journal(dir, &pending);
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn pause(dir: &Path) -> Pause {
    std::fs::read(dir.join(PAUSE_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn cloud_paused(dir: &Path) -> bool {
    pause(dir).until > now()
}

fn note_edge(dir: &Path) {
    let mut row = pause(dir);
    row.wait = next_cloud_wait(row.wait);
    row.until = now().saturating_add(row.wait);
    if let Ok(bytes) = serde_json::to_vec(&row) {
        let _ = std::fs::write(dir.join(PAUSE_FILE), bytes);
    }
    tracing::warn!("pjdesk.com is rate limited; the account copy stays local for {}s", row.wait);
}

fn note_cloud_ok(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(PAUSE_FILE));
}

fn stamp(path: &Path) -> u64 {
    std::fs::read_to_string(path).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0)
}

fn write_stamp(path: &Path, at: u64) {
    let _ = std::fs::write(path, format!("{at}\n"));
}

/// One batch. The standby does not call pjdesk.com here.
pub fn batch(dir: &Path, force: bool) -> bool {
    if !cloud::accepts_host_registrations(dir) {
        // The standby does not upload, but it still needs the password check.
        let _ = pull_now(dir);
        return false;
    }
    let at = now();
    if !force && at.saturating_sub(stamp(&dir.join(BATCH_FILE))) < CLOUD_BATCH_SECS && journal(dir).is_empty() {
        return false;
    }
    if cloud_paused(dir) && !force {
        return false;
    }
    write_stamp(&dir.join(BATCH_FILE), at);
    let Some(cfg) = CloudConfig::load(dir) else { return false };
    if !cfg.is_account() || cfg.token.is_empty() {
        return false;
    }
    let mut local = load(dir);
    ensure_sign_key(&mut local);
    let pending = journal(dir);
    if !pending.is_empty() {
        match push(&cfg, &local.revision, &pending) {
            Ok(fresh) => {
                note_cloud_ok(dir);
                let kept = unfinished_sign_keys(&pending, &fresh);
                local = merge(&fresh, &local, &kept);
                store_journal(dir, &kept);
            }
            Err(Push::Conflict(fresh)) => {
                local = merge(&fresh, &local, &pending);
                let again_changes = journal_from_merge(&fresh, &local);
                if let Ok(again) = push(&cfg, &local.revision, &again_changes) {
                    let kept = unfinished_sign_keys(&pending, &again);
                    local = merge(&again, &local, &kept);
                    store_journal(dir, &kept);
                }
            }
            Err(Push::Paused) => {
                note_edge(dir);
            }
            Err(Push::Missing) => {
                tracing::info!("account mirror is not on pjdesk.com yet; this server keeps the local copy");
            }
            Err(Push::Other(err)) => tracing::warn!("account mirror push: {err}"),
        }
    }
    match pull(&cfg) {
        Ok(fresh) => {
            note_cloud_ok(dir);
            let pending = journal(dir);
            local = merge(&fresh, &local, &pending);
        }
        Err(Pull::Paused) => {
            note_edge(dir);
            let _ = record_status(dir, 0, "pjdesk.com is rate limited");
        }
        Err(Pull::Missing) => {
            let _ = record_status(dir, 0, "HTTP 404: pjdesk.com has no account copy for this sign-in");
        }
        Err(Pull::Other(err)) => {
            note_pull_failure(&err);
            let _ = record_status(dir, 0, &err);
        }
    }
    ensure_sign_key(&mut local);
    save(dir, &local);
    if has_verifier(&local) {
        record_status(dir, now(), "");
    }
    // A fresh install does not turn Require signed-in accounts on from the copy.
    let _ = crate::access::restore_grace(dir);
    true
}

/// Pulls the account copy now. Sign-in and service start call this. The
/// five-minute batch is not the first pull.
pub fn pull_now(dir: &Path) -> String {
    let Some(mut cfg) = CloudConfig::load(dir) else {
        return record_status(dir, 0, "not signed in");
    };
    if !cfg.is_account() || cfg.token.is_empty() {
        return record_status(dir, 0, "not signed in");
    }
    if cfg.api.trim().is_empty() {
        cfg.api = cloud::default_api();
    }
    let line = match pull(&cfg) {
        Ok(fresh) => {
            note_cloud_ok(dir);
            let mut local = merge(&fresh, &load(dir), &journal(dir));
            ensure_sign_key(&mut local);
            save(dir, &local);
            let _ = crate::access::restore_grace(dir);
            record_status(dir, now(), "")
        }
        Err(Pull::Paused) => {
            note_edge(dir);
            record_status(dir, 0, "pjdesk.com is rate limited")
        }
        Err(Pull::Missing) => record_status(dir, 0, "HTTP 404: pjdesk.com has no account copy for this sign-in"),
        Err(Pull::Other(err)) => {
            note_pull_failure(&err);
            record_status(dir, 0, &err)
        }
    };
    tracing::info!("{line}");
    line
}

fn note_pull_failure(err: &str) {
    if auth_rejected(err) {
        tracing::info!("account mirror pull failed: {err}. Server sign-in needed");
    } else {
        tracing::info!("account mirror pull failed: {err}");
    }
}

/// Shown when pjdesk.com rejected this server's token. A password sign-in replaces it.
pub const SERVER_SIGN_IN_NEEDED: &str = "Server sign-in needed";

/// `Account copy: synced <time>`, `Account copy: error <reason>`, or [`SERVER_SIGN_IN_NEEDED`].
pub fn status_line(dir: &Path) -> String {
    let status = read_status(dir);
    if auth_rejected(&status.error) {
        let code = if status.error.to_ascii_lowercase().contains("http 403") { "403" } else { "401" };
        return format!("{SERVER_SIGN_IN_NEEDED} (HTTP {code})");
    }
    if !status.error.is_empty() {
        format!("Account copy: error {}", one_line(&status.error))
    } else if status.synced_at > 0 {
        format!("Account copy: synced {}", fmt_time(status.synced_at))
    } else {
        "Account copy: error not synced yet".into()
    }
}

fn auth_rejected(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("http 401") || lower.contains("http 403")
}

/// The account file has no password check. Names the last copy failure instead of
/// claiming pjdesk.com was silent when this server never called it, or when it rejected the token.
pub fn empty_copy_message(dir: &Path) -> String {
    let status = read_status(dir);
    let why = if auth_rejected(&status.error) {
        "Its pjdesk.com sign-in was rejected. Server sign-in needed.".to_string()
    } else if status.error.contains("not signed in") || !server_has_cloud_token(dir) {
        "This server is not signed in to pjdesk.com. Server sign-in needed.".to_string()
    } else if status.error.contains("404") {
        "pjdesk.com has no account copy for this sign-in.".to_string()
    } else if status.error.contains("rate limited") || cloud_paused(dir) {
        "pjdesk.com is rate limited.".to_string()
    } else if !status.error.is_empty() {
        format!("The last copy failed ({}).", one_line(&status.error))
    } else {
        "No copy has been loaded.".to_string()
    };
    format!("This server's account copy is empty. {why}")
}

fn server_has_cloud_token(dir: &Path) -> bool {
    CloudConfig::load(dir).is_some_and(|cfg| cfg.is_account() && !cfg.token.trim().is_empty())
}

fn sign_in_failure_is_final(err: &str) -> bool {
    err.contains("Wrong email or password")
        || err.contains("Enter your email")
        || err.contains("Too many sign-in")
        || err.contains("Too many failed")
        || err.contains("Too many tries")
}

/// The owner just signed in with email and password. A blocked computer may
/// register again for the next two minutes.
pub fn note_owner_reenroll(dir: &Path) {
    let until = now().saturating_add(REENROLL_SECS);
    let _ = std::fs::write(dir.join(REENROLL_FILE), format!("{until}\n"));
}

pub fn owner_reenroll_open(dir: &Path) -> bool {
    stamp(&dir.join(REENROLL_FILE)) > now()
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct CopyStatus {
    #[serde(default)]
    synced_at: u64,
    #[serde(default)]
    error: String,
}

fn read_status(dir: &Path) -> CopyStatus {
    std::fs::read(dir.join(STATUS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn record_status(dir: &Path, synced_at: u64, error: &str) -> String {
    let row = if error.is_empty() {
        CopyStatus { synced_at, error: String::new() }
    } else {
        let previous = read_status(dir);
        CopyStatus { synced_at: previous.synced_at, error: one_line(error) }
    };
    if let Ok(bytes) = serde_json::to_vec(&row) {
        let _ = std::fs::write(dir.join(STATUS_FILE), bytes);
    }
    status_line(dir)
}

fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(180).collect()
}

fn fmt_time(secs: u64) -> String {
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| secs.to_string())
}

/// Changes that are in `local` and not already the cloud row. Used after a
/// conflict so a website password change is not pushed back.
fn journal_from_merge(cloud: &AccountCopy, local: &AccountCopy) -> Vec<AccountChange> {
    let mut out = Vec::new();
    for computer in &local.computers {
        let cloud_row = cloud.computers.iter().find(|row| row.id == computer.id && row.account_id == computer.account_id);
        if cloud_row.map(|row| row.updated_at < computer.updated_at).unwrap_or(true) {
            out.push(AccountChange::Computer { computer: computer.clone() });
        }
    }
    for device in &local.devices {
        let cloud_row = cloud.devices.iter().find(|row| row.device_id == device.device_id);
        if cloud_row.map(|row| row.enrolled_at < device.enrolled_at && row.revoked_at == 0).unwrap_or(true) {
            out.push(AccountChange::Device { device: device.clone() });
        }
    }
    out
}

enum Push {
    Conflict(AccountCopy),
    Paused,
    Missing,
    Other(String),
}

enum Pull {
    Paused,
    Missing,
    Other(String),
}

fn endpoint(cfg: &CloudConfig) -> String {
    format!("{}/api/account/mirror", cfg.api.trim_end_matches('/'))
}

fn mirror_agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(8)).build()
}

fn pull(cfg: &CloudConfig) -> std::result::Result<AccountCopy, Pull> {
    let response = mirror_agent().get(&endpoint(cfg)).set("authorization", &format!("Bearer {}", cfg.token)).call();
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => return Err(classify_status(code, response)),
        Err(err) => return Err(Pull::Other(err.to_string())),
    };
    let text = response.into_string().map_err(|err| Pull::Other(err.to_string()))?;
    serde_json::from_str(&text).map_err(|err| Pull::Other(err.to_string()))
}

fn classify_status(code: u16, response: ureq::Response) -> Pull {
    let ctype = response.header("content-type").unwrap_or("").to_string();
    let body = response.into_string().unwrap_or_default();
    if code == 429 && (ctype.to_ascii_lowercase().contains("text/html") || cloud_edge_limited(code, &ctype, &body)) {
        return Pull::Paused;
    }
    if code == 404 {
        return Pull::Missing;
    }
    Pull::Other(mirror_http_error(code, &body))
}

/// 401/403 is the session (auth). 500 that mentions the signing key is that key.
pub fn mirror_http_error(code: u16, body: &str) -> String {
    let detail = json_error(body).unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.is_empty() { "empty response".into() } else { one_line(trimmed) }
    });
    let lower = format!("{detail} {body}").to_ascii_lowercase();
    let kind = if code == 401 || code == 403 {
        "auth"
    } else if code >= 500 && (lower.contains("sign") || lower.contains("network_sign_keys") || lower.contains("ed25519")) {
        "signing key"
    } else if code >= 500 {
        "server"
    } else {
        "http"
    };
    format!("HTTP {code} {kind}: {detail}")
}

fn json_error(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    for key in ["error", "message"] {
        if let Some(msg) = value.get(key).and_then(|item| item.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            return Some(one_line(msg));
        }
    }
    None
}

fn push(cfg: &CloudConfig, revision: &str, changes: &[AccountChange]) -> std::result::Result<AccountCopy, Push> {
    let body = serde_json::json!({ "base_revision": revision, "changes": changes });
    let response = cloud::agent()
        .put(&endpoint(cfg))
        .set("authorization", &format!("Bearer {}", cfg.token))
        .set("content-type", "application/json")
        .send_string(&body.to_string());
    match response {
        Ok(response) => {
            let text = response.into_string().map_err(|err| Push::Other(err.to_string()))?;
            serde_json::from_str(&text).map_err(|err| Push::Other(err.to_string()))
        }
        Err(ureq::Error::Status(412, response)) => {
            let text = response.into_string().unwrap_or_default();
            let fresh = serde_json::from_str::<ConflictBody>(&text)
                .ok()
                .and_then(|body| body.mirror)
                .ok_or_else(|| Push::Other("the account changed".into()))?;
            Err(Push::Conflict(fresh))
        }
        Err(ureq::Error::Status(404, _)) => Err(Push::Missing),
        Err(err) => Err(classify_push(err)),
    }
}

#[derive(Deserialize)]
struct ConflictBody {
    #[serde(default)]
    mirror: Option<AccountCopy>,
}

fn classify_push(err: ureq::Error) -> Push {
    if is_edge(&err) {
        Push::Paused
    } else {
        Push::Other(err.to_string())
    }
}

fn is_edge(err: &ureq::Error) -> bool {
    let ureq::Error::Status(status, response) = err else {
        return false;
    };
    let ctype = response.header("content-type").unwrap_or("");
    // The body was not read. Status 429 with an HTML content type is the edge.
    *status == 429 && ctype.to_ascii_lowercase().contains("text/html")
        || cloud_edge_limited(*status, ctype, "")
}

pub fn has_owner_copy(dir: &Path) -> bool {
    account_mirror::has_verifier(&load(dir))
}

pub fn read_owner_lock(dir: &Path) -> (String, u64) {
    let copy = load(dir);
    let id = copy.account_id.clone();
    copy.locks
        .into_iter()
        .find(|row| row.account_id == id)
        .map(|row| (row.ciphertext, row.updated_at))
        .unwrap_or_default()
}

pub fn write_owner_lock(dir: &Path, ciphertext: &str) -> Result<u64, String> {
    let copy = load(dir);
    if copy.account_id.is_empty() {
        return Err(empty_copy_message(dir));
    }
    let at = now();
    remember(
        dir,
        AccountChange::Lock {
            row: SealedRow { account_id: copy.account_id, ciphertext: ciphertext.to_string(), updated_at: at },
        },
    );
    Ok(at)
}

pub fn confirm_owner(dir: &Path, email: &str, password: &str) -> Result<[u8; 32], String> {
    let email = email.trim().to_lowercase();
    if email.is_empty() || password.is_empty() {
        return Err("Enter your account password.".into());
    }
    let keys = crate::cloud::derive_account_keys(&email, password);
    let mut copy = load(dir);
    match account_mirror::verify_auth(&mut copy, &email, &hex::encode(keys.auth), now()) {
        Ok(()) => {
            save(dir, &copy);
            Ok(pjdesk_proto::access::derive_password_vault_key(&email, password))
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Sign-in, a token refresh, a password check, and a code join are answered
/// before this computer is registered. Require signed-in accounts does not
/// apply to them. The password, the refresh token, or the code is checked
/// inside the handler.
pub fn open_before_registration(op: &str) -> bool {
    matches!(op.trim(), "sign-in" | "verify" | "refresh" | "join" | "enroll")
}

/// Answers one client account request from the local copy.
pub fn answer(dir: &Path, msg: ClientMsg) -> ServerMsg {
    let started = std::time::Instant::now();
    let ClientMsg::Account {
        op,
        email,
        auth_key,
        refresh,
        device_id,
        device_name,
        vault,
        app_lock,
        computers,
        proof,
    } = msg
    else {
        return fail("sign-in", "That was not understood.");
    };
    if op == "replica" {
        return replica(dir, &proof);
    }
    if !has_verifier(&load(dir)) && (op == "sign-in" || op == "verify") {
        match seed_for_sign_in(dir, &email, &auth_key, SIGN_IN_SEED_BUDGET) {
            SeedWait::Ready => {}
            SeedWait::Failed(err) => {
                tracing::warn!("account copy proxy: {err}");
                let reply = name_account_failure(dir, fail(&op, &err));
                log_sign_in(&op, &reply, started);
                return reply;
            }
            SeedWait::Loading => {
                let reply = name_account_failure(
                    dir,
                    fail(&op, "The account copy is still loading from pjdesk.com; try again in a moment."),
                );
                log_sign_in(&op, &reply, started);
                return reply;
            }
        }
    }
    let mut copy = load(dir);
    ensure_sign_key(&mut copy);
    save(dir, &copy);
    let reply = match op.as_str() {
        "sign-in" => answer_sign_in(dir, &email, &auth_key, &device_id, &device_name),
        "verify" => answer_verify(dir, &email, &auth_key),
        "refresh" => answer_refresh(dir, &refresh, &device_id, &computers),
        "vault-read" => answer_sealed(dir, &refresh, &device_id, true, None),
        "vault-write" => answer_sealed(dir, &refresh, &device_id, true, Some(vault)),
        "lock-read" => answer_sealed(dir, &refresh, &device_id, false, None),
        "lock-write" => answer_sealed(dir, &refresh, &device_id, false, Some(app_lock)),
        "saved-write" => answer_saved(dir, &refresh, &device_id, &computers),
        _ => fail(&op, "That was not understood."),
    };
    let reply = name_account_failure(dir, reply);
    log_sign_in(&op, &reply, started);
    reply
}

enum SeedWait {
    Ready,
    Failed(String),
    Loading,
}

/// Pulls or seeds the account copy, but the sign-in reply does not wait past `budget`.
/// A seed that is still running keeps going after this returns [`SeedWait::Loading`].
fn seed_for_sign_in(dir: &Path, email: &str, auth_key: &str, budget: Duration) -> SeedWait {
    if has_verifier(&load(dir)) {
        return SeedWait::Ready;
    }
    let dir_bg = dir.to_path_buf();
    let email_bg = email.to_string();
    let auth_bg = auth_key.to_string();
    let started = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("account-seed".into())
        .spawn(move || {
            let result = seed_blocking(&dir_bg, &email_bg, &auth_bg);
            let ms = started.elapsed().as_millis();
            match &result {
                Ok(()) => tracing::info!("account copy seed finished ok=true ms={ms}"),
                Err(err) => tracing::info!("account copy seed finished ok=false ms={ms} message={}", one_line(err)),
            }
            let _ = tx.send(result);
        })
        .ok();
    match rx.recv_timeout(budget) {
        Ok(Ok(())) => SeedWait::Ready,
        Ok(Err(err)) => {
            if has_verifier(&load(dir)) {
                SeedWait::Ready
            } else {
                SeedWait::Failed(err)
            }
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => SeedWait::Loading,
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            if has_verifier(&load(dir)) {
                SeedWait::Ready
            } else {
                SeedWait::Failed("The account copy could not be loaded.".into())
            }
        }
    }
}

fn seed_blocking(dir: &Path, email: &str, auth_key: &str) -> std::result::Result<(), String> {
    if !has_verifier(&load(dir)) && !cloud_paused(dir) {
        let _ = pull_now(dir);
    }
    if !has_verifier(&load(dir)) {
        seed_from_peer(dir);
    }
    if has_verifier(&load(dir)) {
        return Ok(());
    }
    if cloud_paused(dir) {
        return Err("pjdesk.com is rate limited".into());
    }
    match proxy_login_seed(dir, email, auth_key) {
        Ok(()) => Ok(()),
        Err(_) if has_verifier(&load(dir)) => Ok(()),
        Err(err) if sign_in_failure_is_final(&err) => Err(err),
        Err(_) => Err(empty_copy_message(dir)),
    }
}

fn log_sign_in(op: &str, reply: &ServerMsg, started: std::time::Instant) {
    if op != "sign-in" {
        return;
    }
    let ms = started.elapsed().as_millis();
    match reply {
        ServerMsg::AccountResult { ok, message, .. } => {
            tracing::info!("account op=sign-in ok={ok} ms={ms} message={}", one_line(message));
        }
        _ => tracing::info!("account op=sign-in ok=false ms={ms} message=no account result"),
    }
}

/// Replaces the account-mirror signing key. The new private key is not returned.
pub fn rotate_sign_key(dir: &Path) -> std::result::Result<String, String> {
    let copy = load(dir);
    if copy.sign_pkcs8.is_empty() && copy.account_id.is_empty() && !has_verifier(&copy) {
        return Err("This server does not have an account copy yet.".into());
    }
    let (pkcs8, public_key) = generate_sign_key().ok_or_else(|| "Could not generate a signing key.".to_string())?;
    remember(
        dir,
        AccountChange::SignKey { public_key, pkcs8 },
    );
    let _ = batch(dir, true);
    Ok("Replaced the account signing key on this server. New sessions use the new key.".into())
}

fn unfinished_sign_keys(pending: &[AccountChange], fresh: &AccountCopy) -> Vec<AccountChange> {
    pending
        .iter()
        .filter(|change| match change {
            AccountChange::SignKey { public_key, .. } => fresh.sign_public != *public_key,
            _ => false,
        })
        .cloned()
        .collect()
}

/// Puts the answering server's name on a failed account reply.
fn name_account_failure(dir: &Path, mut msg: ServerMsg) -> ServerMsg {
    if let ServerMsg::AccountResult { ok: false, message, .. } = &mut msg {
        if !message.contains(" answered") {
            let who = crate::cloud::role_label(dir);
            *message = format!("{who} answered. {message}");
        }
    }
    msg
}

/// The server has no copy. Sign in to pjdesk.com with the client's auth key
/// (never the password) and store the mirror that token can read.
fn proxy_login_seed(dir: &Path, email: &str, auth_key: &str) -> Result<(), String> {
    if auth_key.len() != 64 || !auth_key.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Enter your email and password.".into());
    }
    let api = CloudConfig::load(dir)
        .map(|cfg| cfg.api)
        .filter(|api| !api.trim().is_empty())
        .unwrap_or_else(cloud::default_api);
    let api = api.trim_end_matches('/').to_string();
    let body = cloud::login_body(email, auth_key, "PJDesk Server account copy");
    let response = mirror_agent()
        .post(&format!("{api}/api/login"))
        .set("content-type", "application/json")
        .send_string(&body.to_string());
    let token = match response {
        Ok(resp) => {
            let text = resp.into_string().map_err(|err| err.to_string())?;
            serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|value| value.get("token").and_then(|item| item.as_str()).map(str::to_string))
                .filter(|token| !token.is_empty())
                .ok_or_else(|| format!("HTTP 200 login: {}", one_line(&text)))?
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(cloud::cloud_error_text(code, &body));
        }
        Err(err) => return Err(format!("login: {err}")),
    };
    let mut cfg = CloudConfig::load(dir).unwrap_or_else(CloudConfig::blank_account);
    cfg.api = api.clone();
    cfg.token = token;
    cfg.mode = "account".into();
    match pull(&cfg) {
        Ok(fresh) => {
            note_cloud_ok(dir);
            let mut local = merge(&fresh, &load(dir), &journal(dir));
            ensure_sign_key(&mut local);
            if !has_verifier(&local) {
                let _ = record_status(dir, 0, "pjdesk.com sent an account copy with no password check");
                return Err(format!(
                    "{api}/api/account/mirror answered, but the copy has no password check for this account"
                ));
            }
            save(dir, &local);
            let _ = crate::access::restore_grace(dir);
            let line = record_status(dir, now(), "");
            tracing::info!("account copy seeded from the client's sign-in; {line}");
            Ok(())
        }
        Err(Pull::Paused) => Err("pjdesk.com is rate limited".into()),
        Err(Pull::Missing) => Err("HTTP 404: pjdesk.com has no account copy for this sign-in".into()),
        Err(Pull::Other(err)) => Err(err),
    }
}

fn answer_sign_in(dir: &Path, email: &str, auth_key: &str, device_id: &str, device_name: &str) -> ServerMsg {
    // Reject a body that is not an auth key before touching the verifier.
    if auth_key.len() != 64 || !auth_key.chars().all(|c| c.is_ascii_hexdigit()) {
        return fail("sign-in", "Enter your email and password.");
    }
    let mut copy = load(dir);
    let refresh = random_hex(32);
    let result = sign_in(&mut copy, email, auth_key, device_id, device_name, &refresh, now());
    match result {
        Ok(session) => {
            if let Some(device) = copy.devices.iter().find(|row| row.device_id == session.device_id).cloned() {
                save(dir, &copy);
                remember_device(dir, device);
            }
            let saved = crate::cloud::saved_account_email(dir);
            if saved.is_empty() || saved.eq_ignore_ascii_case(email.trim()) {
                note_owner_reenroll(dir);
            }
            session_msg(&copy, "sign-in", &session, &refresh)
        }
        Err(AuthError::Pause(secs)) => {
            save(dir, &copy);
            paused("sign-in", account_mirror::sign_in_limit_message(secs), secs)
        }
        Err(err) => {
            save(dir, &copy);
            let text = match &err {
                AuthError::NoCopy if !has_verifier(&copy) => empty_copy_message(dir),
                other => other.to_string(),
            };
            fail("sign-in", &text)
        }
    }
}

fn answer_verify(dir: &Path, email: &str, auth_key: &str) -> ServerMsg {
    if auth_key.len() != 64 || !auth_key.chars().all(|c| c.is_ascii_hexdigit()) {
        return fail("verify", "Enter your email and password.");
    }
    let mut copy = load(dir);
    match account_mirror::verify_auth(&mut copy, email, auth_key, now()) {
        Ok(()) => {
            save(dir, &copy);
            let mut msg = empty_result();
            if let ServerMsg::AccountResult { ok, op, .. } = &mut msg {
                *ok = true;
                *op = "verify".into();
            }
            msg
        }
        Err(AuthError::Pause(secs)) => {
            save(dir, &copy);
            paused("verify", account_mirror::sign_in_limit_message(secs), secs)
        }
        Err(err) => {
            save(dir, &copy);
            let text = match &err {
                AuthError::NoCopy if !has_verifier(&copy) => empty_copy_message(dir),
                other => other.to_string(),
            };
            fail("verify", &text)
        }
    }
}

fn remember_device(dir: &Path, device: DeviceRow) {
    let mut pending = journal(dir);
    pending.push(AccountChange::Device { device });
    store_journal(dir, &pending);
}

fn answer_refresh(dir: &Path, refresh: &str, device_id: &str, computers: &[FleetComputer]) -> ServerMsg {
    let copy = load(dir);
    match account_mirror::refresh_session(&copy, refresh, device_id, now()) {
        Ok(session) => {
            if !computers.is_empty() {
                write_computers(dir, &session.account_id, computers);
            }
            let copy = load(dir);
            session_msg(&copy, "refresh", &session, refresh)
        }
        Err(err) => fail("refresh", &err.to_string()),
    }
}

fn answer_saved(dir: &Path, refresh: &str, device_id: &str, computers: &[FleetComputer]) -> ServerMsg {
    let copy = load(dir);
    let Ok(session) = account_mirror::refresh_session(&copy, refresh, device_id, now()) else {
        return fail("saved-write", "Sign in required.");
    };
    write_computers(dir, &session.account_id, computers);
    let copy = load(dir);
    session_msg(&copy, "saved-write", &session, refresh)
}

fn write_computers(dir: &Path, account_id: &str, computers: &[FleetComputer]) {
    let at = now();
    for computer in computers {
        let id = computer.id.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
        if id.len() != 9 {
            continue;
        }
        remember(
            dir,
            AccountChange::Computer {
                computer: SavedComputer {
                    id,
                    account_id: account_id.to_string(),
                    name: computer.name.clone(),
                    hostname: computer.hostname.clone(),
                    os_family: computer.os_family.clone(),
                    os_version: computer.os_version.clone(),
                    display_name: computer.display_name.clone(),
                    starred: computer.starred,
                    position: computer.position,
                    hidden: computer.hidden,
                    group: computer.group.clone(),
                    updated_at: at,
                },
            },
        );
    }
}

fn answer_sealed(dir: &Path, refresh: &str, device_id: &str, vault: bool, write: Option<String>) -> ServerMsg {
    let copy = load(dir);
    let Ok(session) = account_mirror::refresh_session(&copy, refresh, device_id, now()) else {
        return fail(if vault { "vault-read" } else { "lock-read" }, "Sign in required.");
    };
    if let Some(text) = write {
        let row = SealedRow { account_id: session.account_id.clone(), ciphertext: text, updated_at: now() };
        remember(
            dir,
            if vault { AccountChange::Vault { row } } else { AccountChange::Lock { row } },
        );
    }
    let copy = load(dir);
    let mut msg = session_msg(&copy, if vault { "vault-read" } else { "lock-read" }, &session, refresh);
    if let ServerMsg::AccountResult { vault: slot, app_lock, vault_updated, app_lock_updated, .. } = &mut msg {
        if vault {
            *slot = vault_for(&copy, &session.account_id).to_string();
            *vault_updated = copy
                .vaults
                .iter()
                .find(|row| row.account_id == session.account_id)
                .map(|row| row.updated_at)
                .unwrap_or(0);
        } else {
            *app_lock = copy
                .locks
                .iter()
                .find(|row| row.account_id == session.account_id)
                .map(|row| row.ciphertext.clone())
                .unwrap_or_default();
            *app_lock_updated = copy
                .locks
                .iter()
                .find(|row| row.account_id == session.account_id)
                .map(|row| row.updated_at)
                .unwrap_or(0);
        }
    }
    msg
}

fn session_msg(copy: &AccountCopy, op: &str, session: &account_mirror::SessionOut, refresh: &str) -> ServerMsg {
    let computers = copy
        .computers
        .iter()
        .filter(|row| row.account_id == session.account_id)
        .map(fleet_computer)
        .collect();
    let removed = copy.removed.iter().map(|row| row.id.clone()).collect();
    ServerMsg::AccountResult {
        ok: true,
        op: op.into(),
        message: String::new(),
        access: session.access.clone(),
        exp: session.exp,
        refresh: refresh.to_string(),
        email: session.email.clone(),
        role: session.role.clone(),
        device_id: session.device_id.clone(),
        computers,
        removed,
        active_address: copy.network.active_address.clone(),
        active_fingerprint: copy.network.active_fingerprint.clone(),
        standby_address: copy.network.standby_address.clone(),
        standby_fingerprint: copy.network.standby_fingerprint.clone(),
        active_lan: copy.network.active_lan.clone(),
        standby_lan: copy.network.standby_lan.clone(),
        active_public_ip: copy.network.active_public_ip.clone(),
        standby_public_ip: copy.network.standby_public_ip.clone(),
        vault: String::new(),
        vault_updated: 0,
        app_lock: String::new(),
        app_lock_updated: 0,
        pause_secs: 0,
    }
}

fn fleet_computer(row: &SavedComputer) -> FleetComputer {
    FleetComputer {
        id: row.id.clone(),
        name: row.name.clone(),
        hostname: row.hostname.clone(),
        os_family: row.os_family.clone(),
        os_version: row.os_version.clone(),
        display_name: row.display_name.clone(),
        starred: row.starred,
        position: row.position,
        hidden: row.hidden,
        group: row.group.clone(),
    }
}

fn empty_result() -> ServerMsg {
    ServerMsg::AccountResult {
        ok: false,
        op: String::new(),
        message: String::new(),
        access: String::new(),
        exp: 0,
        refresh: String::new(),
        email: String::new(),
        role: String::new(),
        device_id: String::new(),
        vault: String::new(),
        vault_updated: 0,
        app_lock: String::new(),
        app_lock_updated: 0,
        computers: Vec::new(),
        removed: Vec::new(),
        pause_secs: 0,
        active_address: String::new(),
        active_fingerprint: String::new(),
        standby_address: String::new(),
        standby_fingerprint: String::new(),
        active_lan: String::new(),
        standby_lan: String::new(),
        active_public_ip: String::new(),
        standby_public_ip: String::new(),
    }
}

fn paused(op: &str, message: String, secs: u64) -> ServerMsg {
    let mut msg = empty_result();
    if let ServerMsg::AccountResult { op: slot, message: text, pause_secs, .. } = &mut msg {
        *slot = op.to_string();
        *text = message;
        *pause_secs = secs;
    }
    msg
}

fn fail(op: &str, message: &str) -> ServerMsg {
    let mut msg = empty_result();
    if let ServerMsg::AccountResult { op: slot, message: text, .. } = &mut msg {
        *slot = op.to_string();
        *text = message.to_string();
    }
    msg
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    let rng = ring::rand::SystemRandom::new();
    let _ = ring::rand::SecureRandom::fill(&rng, &mut buf);
    hex::encode(buf)
}

pub fn replica_proof(key: &[u8; 32], at: u64) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(b"pjdesk-replica-1\n");
    mac.update(&(at / 60).to_string().as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn proof_ok(dir: &Path, proof: &str) -> bool {
    let Some(key) = cloud::sync_key(dir) else { return false };
    let at = now();
    let proof = proof.trim();
    replica_proof(&key, at) == proof || replica_proof(&key, at.saturating_sub(60)) == proof
}

fn replica(dir: &Path, proof: &str) -> ServerMsg {
    if !proof_ok(dir, proof) {
        return fail("replica", "Sign in required.");
    }
    let copy = for_replica(&load(dir));
    let Ok(body) = serde_json::to_vec(&copy) else {
        return fail("replica", "The account copy could not be sent.");
    };
    if body.len() > 200_000 {
        return fail("replica", "The account copy is too large to send.");
    }
    ServerMsg::Replica { revision: copy.revision.clone(), body }
}

/// Takes a peer copy only when this server has no password check and the peer does.
pub fn adopt_peer_copy(local: &AccountCopy, peer: &AccountCopy) -> Option<AccountCopy> {
    if has_verifier(local) || !has_verifier(peer) {
        return None;
    }
    Some(merge(peer, local, &[]))
}

const PEER_FP_FILE: &str = "peer-fingerprint.txt";

/// The standby asks the primary for the account copy. A primary whose copy is
/// empty asks the standby. This is not a pjdesk.com call.
pub fn pull_replica(dir: &Path) {
    fetch_peer_copy(dir, false, Duration::from_secs(8));
}

/// Sign-in calls this without waiting for the five-minute replica cadence.
pub fn seed_from_peer(dir: &Path) {
    fetch_peer_copy(dir, true, Duration::from_secs(2));
}

fn fetch_peer_copy(dir: &Path, force: bool, budget: Duration) {
    let primary = cloud::accepts_host_registrations(dir);
    if primary && has_verifier(&load(dir)) {
        return;
    }
    let at = now();
    if !force && at.saturating_sub(stamp(&dir.join(REPLICA_FILE))) < CLOUD_BATCH_SECS {
        return;
    }
    let Some(key) = cloud::sync_key(dir) else { return };
    let targets = if primary { standby_targets(dir) } else { cloud::peer_dial_targets(dir) };
    if targets.is_empty() {
        return;
    }
    if !force {
        write_stamp(&dir.join(REPLICA_FILE), at);
    }
    let pin = peer_pin(dir);
    let proof = replica_proof(&key, at);
    let msg = ClientMsg::Account {
        op: "replica".into(),
        proof,
        email: String::new(),
        auth_key: String::new(),
        refresh: String::new(),
        device_id: String::new(),
        device_name: String::new(),
        vault: String::new(),
        app_lock: String::new(),
        computers: Vec::new(),
    };
    tracing::info!("account copy seed targets={}", targets.len());
    for target in targets {
        let Some(addr) = crate::role_handoff::resolve_target(&target) else { continue };
        let private = pjdesk_proto::routes::endpoint_v4(&target).is_some_and(|ip| ip.is_private() || ip.is_loopback());
        // A saved fingerprint belongs to one server. A private peer on this LAN
        // is dialed without that pin, or the Lenovo is rejected as the cabin.
        let use_pin = if private { None } else { pin };
        if use_pin.is_none() && !private {
            tracing::info!("account copy seed skip {target}: public address without a fingerprint");
            continue;
        }
        tracing::info!("account copy seed dial {target}");
        match crate::role_handoff::dial_observed(addr, use_pin, &msg, budget) {
            Ok((ServerMsg::Replica { body, .. }, observed)) => {
                if let Some(fp) = observed {
                    let _ = std::fs::write(dir.join(PEER_FP_FILE), pjdesk_proto::tls::format_fingerprint(&fp));
                }
                if let Ok(fresh) = serde_json::from_slice::<AccountCopy>(&body) {
                    if store_peer_copy(dir, primary, &fresh) {
                        return;
                    }
                    tracing::info!("account copy seed {target} had no password check");
                }
            }
            Ok(_) => tracing::info!("account copy seed {target} did not send a copy"),
            Err(err) => tracing::info!("account copy seed {target} failed: {err:#}"),
        }
    }
}

fn store_peer_copy(dir: &Path, primary: bool, fresh: &AccountCopy) -> bool {
    let local = load(dir);
    if primary {
        let Some(merged) = adopt_peer_copy(&local, fresh) else { return false };
        save(dir, &merged);
        let _ = record_status(dir, now(), "");
        tracing::info!("account copy seeded from another server");
        return true;
    }
    if !has_verifier(fresh) && !has_verifier(&local) {
        return false;
    }
    save(dir, &merge(fresh, &local, &journal(dir)));
    tracing::info!("account copy mirrored from the primary");
    true
}

fn standby_targets(dir: &Path) -> Vec<String> {
    let routes = cloud::advertised_routes(dir);
    let mut extras = crate::public_addr::lan_peer_addrs(dir);
    let mine = crate::public_addr::discover_gateway_lans();
    let have_local = extras.iter().any(|addr| pjdesk_proto::routes::on_subnet(addr, &mine));
    if !have_local {
        for (addr, beacon) in pjdesk_proto::discover::listen_servers(Duration::from_millis(1000)) {
            extras.push(format!("{}:{}", addr.ip(), beacon.port));
        }
    }
    let cached = cloud::cached_peer_address(dir);
    if !cached.is_empty() {
        extras.push(cached);
    }
    let own = crate::public_addr::lan_endpoints(crate::public_addr::remembered_port(dir));
    let order = pjdesk_proto::routes::sign_in_peer_order(&routes, &mine, &extras, "");
    order.into_iter().filter(|addr| !own.iter().any(|mine| pjdesk_proto::failover::same_server(mine, addr))).collect()
}

fn peer_pin(dir: &Path) -> Option<pjdesk_proto::tls::Fingerprint> {
    let mine = crate::server::read_identity(dir).map(|id| id.fingerprint());
    let copy = load(dir);
    for text in [&copy.network.standby_fingerprint, &copy.network.active_fingerprint] {
        if let Ok(fp) = pjdesk_proto::tls::parse_fingerprint(text) {
            if mine.is_none_or(|mine| mine != fp) {
                return Some(fp);
            }
        }
    }
    let saved = std::fs::read_to_string(dir.join(PEER_FP_FILE)).unwrap_or_default();
    pjdesk_proto::tls::parse_fingerprint(saved.trim()).ok()
}

pub fn apply_replica_body(dir: &Path, body: &[u8]) {
    let Ok(fresh) = serde_json::from_slice::<AccountCopy>(body) else { return };
    let local = load(dir);
    save(dir, &merge(&fresh, &local, &journal(dir)));
}

pub fn fleet_for(dir: &Path, account_email: &str) -> Option<ServerMsg> {
    let copy = load(dir);
    if !has_verifier(&copy) && copy.computers.is_empty() {
        return None;
    }
    let account_id = if account_email.is_empty() {
        copy.account_id.clone()
    } else {
        copy.credentials
            .iter()
            .find(|row| row.email.eq_ignore_ascii_case(account_email))
            .map(|row| row.account_id.clone())
            .unwrap_or_else(|| copy.account_id.clone())
    };
    let computers = copy.computers.iter().filter(|row| row.account_id == account_id).map(fleet_computer).collect();
    let removed = copy.removed.iter().map(|row| row.id.clone()).collect();
    let lock = copy.locks.iter().find(|row| row.account_id == account_id);
    Some(ServerMsg::Fleet {
        presence: Vec::new(),
        computers,
        removed,
        app_lock: lock.map(|row| row.ciphertext.clone()).unwrap_or_default(),
        app_lock_updated: lock.map(|row| row.updated_at).unwrap_or(0),
    })
}

pub fn client_can_account(version: &str) -> bool {
    version_at_least(version, "0.6.48")
}

pub fn release_message(dir: &Path) -> Option<ServerMsg> {
    let path = dir.join("release-cache.json");
    let bytes = std::fs::read(path).ok()?;
    #[derive(Deserialize)]
    struct Cached {
        #[serde(default)]
        version: String,
        #[serde(default)]
        client: String,
        #[serde(default)]
        server: String,
        #[serde(default)]
        sums: String,
        #[serde(default)]
        notes: String,
        #[serde(default)]
        at: u64,
    }
    let cached: Cached = serde_json::from_slice(&bytes).ok()?;
    if cached.version.is_empty() {
        return None;
    }
    Some(ServerMsg::Release {
        version: cached.version,
        client_url: cached.client,
        server_url: cached.server,
        sums_url: cached.sums,
        notes: cached.notes,
        checked_at: cached.at,
    })
}

/// The primary fetches the release feed at most every few hours.
pub fn refresh_release(dir: &Path) {
    if !cloud::accepts_host_registrations(dir) {
        return;
    }
    let path = dir.join("release-cache.json");
    #[derive(serde::Serialize, Deserialize)]
    struct Cached {
        #[serde(default)]
        version: String,
        #[serde(default)]
        client: String,
        #[serde(default)]
        server: String,
        #[serde(default)]
        sums: String,
        #[serde(default)]
        notes: String,
        #[serde(default)]
        at: u64,
    }
    let existing: Cached = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Cached {
        version: String::new(),
        client: String::new(),
        server: String::new(),
        sums: String::new(),
        notes: String::new(),
        at: 0,
    });
    if existing.at > 0 && now().saturating_sub(existing.at) < pjdesk_proto::failover::RELEASE_CHECK_SECS {
        return;
    }
    let Ok(body) = (|| -> Result<Vec<u8>> {
        let response = cloud::agent().get("https://pjdesk.com/api/release").call().map_err(|err| anyhow!("{err}"))?;
        Ok(response.into_string().unwrap_or_default().into_bytes())
    })() else {
        return;
    };
    #[derive(Deserialize)]
    struct Feed {
        #[serde(default)]
        version: String,
        #[serde(default)]
        client: String,
        #[serde(default)]
        server: String,
        #[serde(default)]
        sums: String,
        #[serde(default)]
        notes: String,
    }
    let Ok(feed) = serde_json::from_slice::<Feed>(&body) else { return };
    if feed.version.is_empty() {
        return;
    }
    let cached = Cached {
        version: feed.version,
        client: feed.client,
        server: feed.server,
        sums: feed.sums,
        notes: feed.notes,
        at: now(),
    };
    if let Ok(bytes) = serde_json::to_vec(&cached) {
        let _ = std::fs::write(path, bytes);
    }
}

pub fn mirror_path(dir: &Path) -> PathBuf {
    dir.join(MIRROR_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_http_error_names_auth_and_the_signing_key() {
        let auth = mirror_http_error(401, r#"{"error":"Please sign in."}"#);
        assert!(auth.contains("HTTP 401 auth"), "{auth}");
        assert!(auth.contains("Please sign in."), "{auth}");
        let key = mirror_http_error(500, "network_sign_keys ed25519 failed");
        assert!(key.contains("HTTP 500 signing key"), "{key}");
        let server = mirror_http_error(500, r#"{"error":"Something went wrong."}"#);
        assert!(server.contains("HTTP 500 server"), "{server}");
    }

    #[test]
    fn rotate_sign_key_replaces_the_key_and_does_not_return_it() {
        let dir = std::env::temp_dir().join(format!("pjdesk-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut copy = AccountCopy::default();
        copy.account_id = "acct".into();
        ensure_sign_key(&mut copy);
        let old = copy.sign_pkcs8.clone();
        save(&dir, &copy);
        let sentence = rotate_sign_key(&dir).unwrap();
        assert!(!sentence.contains(&old), "{sentence}");
        assert!(!sentence.to_ascii_lowercase().contains("pkcs8"));
        let now = load(&dir);
        assert_ne!(now.sign_pkcs8, old);
        assert!(!now.sign_pkcs8.is_empty());
        assert!(!sentence.contains(&now.sign_pkcs8));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_line_names_the_sync_or_the_error() {
        let dir = std::env::temp_dir().join(format!("pjdesk-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(status_line(&dir), "Account copy: error not synced yet");
        std::fs::write(dir.join(STATUS_FILE), r#"{"synced_at":1700000000,"error":""}"#).unwrap();
        assert_eq!(status_line(&dir), "Account copy: synced 2023-11-14 22:13:20 UTC");
        std::fs::write(dir.join(STATUS_FILE), r#"{"synced_at":1700000000,"error":"HTTP 401 auth: Please sign in."}"#).unwrap();
        assert_eq!(status_line(&dir), "Server sign-in needed (HTTP 401)");
        std::fs::write(dir.join(STATUS_FILE), r#"{"synced_at":1700000000,"error":"HTTP 403 auth: Please sign in."}"#).unwrap();
        assert_eq!(status_line(&dir), "Server sign-in needed (HTTP 403)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mirror_that_requires_accounts_does_not_turn_the_switch_on() {
        let dir = std::env::temp_dir().join(format!("pjdesk-require-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut copy = AccountCopy::default();
        copy.network.require_accounts = true;
        save(&dir, &copy);
        let policy = crate::access::Policy { require_accounts: true, require_chosen: false, ..crate::access::Policy::default() };
        policy.save(&dir).unwrap();
        assert!(crate::access::restore_grace(&dir));
        assert!(!crate::access::Policy::load(&dir).require_accounts);
        assert!(load(&dir).network.require_accounts);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_replica_proof_matches_this_minute_and_not_an_empty_key() {
        let key = [7u8; 32];
        let proof = replica_proof(&key, 1_700_000_000);
        assert_eq!(proof, replica_proof(&key, 1_700_000_030));
        assert_ne!(proof, replica_proof(&key, 1_700_000_060));
        assert_ne!(proof, replica_proof(&[8u8; 32], 1_700_000_000));
    }

    #[test]
    fn sign_in_refresh_and_join_are_answered_while_require_is_on() {
        assert!(open_before_registration("sign-in"));
        assert!(open_before_registration("verify"));
        assert!(open_before_registration("refresh"));
        assert!(open_before_registration("join"));
        assert!(open_before_registration("enroll"));
        assert!(!open_before_registration("vault-read"));
        let dir = std::env::temp_dir().join(format!("pjdesk-open-account-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::access::Policy { require_accounts: true, require_chosen: true, ..crate::access::Policy::default() }
            .save(&dir)
            .unwrap();
        let sign_in = answer(
            &dir,
            ClientMsg::Account {
                op: "sign-in".into(),
                email: "ben@spader.us".into(),
                auth_key: "zz".into(),
                refresh: String::new(),
                device_id: "phone".into(),
                device_name: "Android".into(),
                vault: String::new(),
                app_lock: String::new(),
                computers: Vec::new(),
                proof: String::new(),
            },
        );
        match sign_in {
            ServerMsg::AccountResult { ok, op, message, .. } => {
                assert!(!ok);
                assert_eq!(op, "sign-in");
                assert!(message.contains("Enter your email and password."), "{message}");
                assert!(message.contains("answered"), "{message}");
                assert!(!message.contains(crate::access::peer_message()));
            }
            other => panic!("{other:?}"),
        }
        let join = answer(
            &dir,
            ClientMsg::Account {
                op: "join".into(),
                email: String::new(),
                auth_key: String::new(),
                refresh: String::new(),
                device_id: "phone".into(),
                device_name: "Android".into(),
                vault: String::new(),
                app_lock: String::new(),
                computers: Vec::new(),
                proof: String::new(),
            },
        );
        match join {
            ServerMsg::AccountResult { message, .. } => assert_ne!(message, crate::access::peer_message()),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_standby_still_downloads_the_account_copy() {
        let dir = std::env::temp_dir().join(format!("pjdesk-standby-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(crate::cloud::CONFIG_FILE),
            r#"{"mode":"account","api":"https://pjdesk.com","email":"ben@spader.us","token":"","endpoint":"","bucket":"","access_key_id":"","secret_access_key":"","key":"","region":"auto"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("cloud-state.json"), r#"{"desired":"standby","epoch":4,"active_instance":"other"}"#).unwrap();
        assert!(!crate::cloud::accepts_host_registrations(&dir));
        assert!(!batch(&dir, false));
        let status = std::fs::read_to_string(dir.join("account-copy-status.json")).unwrap_or_default();
        assert!(status.contains("not signed in"), "{status}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_primary_takes_the_standby_copy_and_names_a_rejected_sign_in() {
        let dir = std::env::temp_dir().join(format!("pjdesk-empty-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(STATUS_FILE), r#"{"synced_at":0,"error":"HTTP 401 auth: Please sign in."}"#).unwrap();
        assert_eq!(status_line(&dir), "Server sign-in needed (HTTP 401)");
        let msg = empty_copy_message(&dir);
        assert!(msg.contains("account copy is empty"), "{msg}");
        assert!(msg.contains("Server sign-in needed"), "{msg}");
        assert!(!msg.contains("did not answer"), "{msg}");
        let err = write_owner_lock(&dir, "sealed").unwrap_err();
        assert_eq!(err, msg);
        let mut peer = AccountCopy::default();
        peer.account_id = "acct".into();
        peer.email = "ben@spader.us".into();
        peer.auth_hash = "ab".repeat(32);
        peer.auth_salt = "cd".repeat(16);
        let adopted = adopt_peer_copy(&AccountCopy::default(), &peer).expect("standby copy");
        assert!(has_verifier(&adopted));
        assert_eq!(adopted.email, "ben@spader.us");
        assert!(adopt_peer_copy(&adopted, &peer).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
