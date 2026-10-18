//! Cloud sync: keeps an encrypted copy of this server — its key, user accounts,
//! address books and computer list — in the cloud, so a backup server can take
//! over by pointing the server's DNS name at it. Two ways to store it:
//! * **PJDesk Cloud** (`mode = "account"`): sign in with a PJDesk account. The
//!   password never leaves this server: an *auth key* derived from it signs in,
//!   and a separate *sync key* derived from it seals the copy, so PJDesk Cloud
//!   can't read it. Every server signed in to the same account shares it.
//! * **Your own bucket** (`mode = "r2"`): any Cloudflare R2 / S3 bucket, shared
//!   with backup servers through a join code.
//!
//! Roles are automatic:
//! * A server with computers connected is **active**: it uploads its state
//!   whenever it changes (the computer list at most every five minutes).
//! * A server with none is on **standby**: it downloads the newest state and
//!   applies it, so it already has the same key, accounts and IDs the moment
//!   DNS sends the computers to it — at which point it becomes active.
//!
//! The copy is sealed with ChaCha20-Poly1305 — under the password-derived
//! sync key (PJDesk Cloud), or a random key made at setup that travels in the
//! join code (own bucket).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bucket settings + encryption key (admin-only).
pub const CONFIG_FILE: &str = "cloud-sync.json";
/// What the running server last did, for the management window.
pub const STATUS_FILE: &str = "cloud-status.json";
const STATE_FILE: &str = "cloud-state.json";
const INSTANCE_FILE: &str = "cloud-instance.txt";
/// The object in the bucket that holds the sealed state.
pub const OBJECT: &str = "pjdesk-server.sync";
const MAGIC: &[u8; 8] = b"PJSYNC1\0";
const JOIN_PREFIX: &str = "pjdesk-sync-1:";
const FORMAT: u32 = 1;

/// The server's state files (relative to the data folder), plus every
/// `addressbook/<user>.json`.
const STATE_FILES: [&str; 4] = ["server-cert.pem", "server-key.pem", "accounts.json", "devices.json"];
/// Upload a changed computer list at most this often (accounts and the key go
/// up straight away).
const DEVICES_PUSH_INTERVAL: u64 = 300;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// This computer's name.
fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "server".into())
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

// ---------------------------------------------------------------------------
// Configuration and join codes

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct CloudConfig {
    /// `"account"` (PJDesk Cloud) or `"r2"` (your own bucket).
    #[serde(default = "mode_r2")]
    pub mode: String,
    /// PJDesk Cloud address, e.g. `https://pjdesk.com`.
    #[serde(default)]
    pub api: String,
    /// The PJDesk account this server is signed in to.
    #[serde(default)]
    pub email: String,
    /// Sign-in token for PJDesk Cloud.
    #[serde(default)]
    pub token: String,
    /// S3 endpoint, e.g. `https://<account-id>.r2.cloudflarestorage.com`.
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub access_key_id: String,
    #[serde(default)]
    pub secret_access_key: String,
    /// 32-byte encryption key, hex.
    pub key: String,
    #[serde(default = "default_region")]
    pub region: String,
}

fn default_region() -> String {
    "auto".into()
}

fn mode_r2() -> String {
    "r2".into()
}

/// The PJDesk Cloud address (`PJDESK_CLOUD_API` overrides it, for testing).
pub fn default_api() -> String {
    std::env::var("PJDESK_CLOUD_API")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://pjdesk.com".into())
}

/// Keys derived from a PJDesk account password (the same way pjdesk.com does
/// it in the browser): `auth` signs in, `sync` seals the server copy and never
/// leaves this server.
pub struct AccountKeys {
    pub auth: [u8; 32],
    pub sync: [u8; 32],
}

pub fn derive_account_keys(email: &str, password: &str) -> AccountKeys {
    let email = email.trim().to_lowercase();
    let salt = Sha256::digest(format!("pjdesk-cloud-v1\0{email}").as_bytes());
    let mut master = [0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(600_000).expect("non-zero"),
        &salt,
        password.as_bytes(),
        &mut master,
    );
    let mut auth = [0u8; 32];
    let mut sync = [0u8; 32];
    auth.copy_from_slice(&hmac(&master, "pjdesk-cloud-auth"));
    sync.copy_from_slice(&hmac(&master, "pjdesk-cloud-sync-key"));
    AccountKeys { auth, sync }
}

/// Signs in to PJDesk Cloud and returns the settings to sync with. The
/// password is only used here to derive the keys.
pub fn sign_in(api: &str, email: &str, password: &str) -> Result<CloudConfig> {
    let email = email.trim().to_lowercase();
    if email.is_empty() || password.is_empty() {
        bail!("enter your PJDesk account email and password");
    }
    let api = api.trim().trim_end_matches('/').to_string();
    let keys = derive_account_keys(&email, password);
    let body = serde_json::json!({
        "email": email,
        "auth_key": hex::encode(keys.auth),
        "device": format!("PJDesk Server on {}", hostname()),
    });
    #[derive(Deserialize)]
    struct LoginReply {
        token: String,
        #[serde(default)]
        sync_enabled: bool,
    }
    let reply: LoginReply = match agent()
        .post(&format!("{api}/api/login"))
        .set("content-type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(resp) => serde_json::from_str(&resp.into_string()?).context("unexpected reply from PJDesk Cloud")?,
        Err(ureq::Error::Status(401, _)) => bail!("wrong email or password"),
        Err(e) => return Err(api_error(&api, &email, e)),
    };
    let cfg = CloudConfig {
        mode: "account".into(),
        api,
        email: email.clone(),
        token: reply.token,
        key: hex::encode(keys.sync),
        region: default_region(),
        ..CloudConfig::blank()
    };
    if !reply.sync_enabled {
        api_sign_out(&cfg);
        bail!("Cloud Sync isn't turned on for {email} yet");
    }
    Ok(cfg)
}

/// Signs this server out of PJDesk Cloud (if it was signed in) and stops syncing.
pub fn sign_out(data_dir: &Path) {
    if let Some(cfg) = CloudConfig::load(data_dir) {
        if cfg.is_account() {
            api_sign_out(&cfg);
        }
    }
    turn_off(data_dir);
}

impl CloudConfig {
    /// New settings for a first-time setup, with a fresh encryption key.
    /// `endpoint` may be the S3 URL Cloudflare shows (with or without the
    /// bucket on the end) or just the account ID.
    pub fn new(endpoint: &str, bucket: &str, access_key_id: &str, secret_access_key: &str) -> Result<Self> {
        let (endpoint, path_bucket) = normalize_endpoint(endpoint)?;
        let bucket = match bucket.trim() {
            "" => path_bucket.unwrap_or_default(),
            b => b.to_string(),
        };
        if bucket.is_empty() {
            bail!("enter the bucket name");
        }
        if access_key_id.trim().is_empty() || secret_access_key.trim().is_empty() {
            bail!("enter the access key ID and the secret access key");
        }
        Ok(Self {
            mode: mode_r2(),
            endpoint,
            bucket,
            access_key_id: access_key_id.trim().to_string(),
            secret_access_key: secret_access_key.trim().to_string(),
            key: hex::encode(pjdesk_proto::random_bytes::<32>()),
            region: default_region(),
            ..Self::blank()
        })
    }

    fn blank() -> Self {
        Self {
            mode: mode_r2(),
            api: String::new(),
            email: String::new(),
            token: String::new(),
            endpoint: String::new(),
            bucket: String::new(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
            key: String::new(),
            region: default_region(),
        }
    }

    /// Signed in to a PJDesk account (rather than using your own bucket).
    pub fn is_account(&self) -> bool {
        self.mode == "account"
    }

    /// Where the copy is kept, for the window and logs.
    pub fn describe(&self) -> String {
        if self.is_account() {
            format!("PJDesk Cloud ({})", self.email)
        } else {
            format!("bucket \"{}\"", self.bucket)
        }
    }

    pub fn load(data_dir: &Path) -> Option<Self> {
        let bytes = std::fs::read(data_dir.join(CONFIG_FILE)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(CONFIG_FILE);
        write_atomic(&path, &serde_json::to_vec_pretty(self)?)?;
        protect(&path);
        Ok(())
    }

    /// One string with everything a backup server needs to join. Treat it
    /// like a password.
    pub fn join_code(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        format!("{JOIN_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
    }

    pub fn from_join_code(code: &str) -> Result<Self> {
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        let body = code.strip_prefix(JOIN_PREFIX).ok_or_else(|| anyhow!("that isn't a PJDesk join code"))?;
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| anyhow!("the join code is incomplete — copy all of it"))?;
        let cfg: Self = serde_json::from_slice(&json).map_err(|_| anyhow!("the join code is damaged"))?;
        cfg.key_bytes()?;
        Ok(cfg)
    }

    fn key_bytes(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(&self.key).map_err(|_| anyhow!("bad encryption key in the sync settings"))?;
        <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| anyhow!("bad encryption key in the sync settings"))
    }

    fn host(&self) -> &str {
        self.endpoint.trim_start_matches("https://").trim_start_matches("http://")
    }

    fn canonical_path(&self) -> String {
        format!("/{}/{}", uri_encode(&self.bucket), uri_encode(OBJECT))
    }

    fn url(&self) -> String {
        format!("{}{}", self.endpoint, self.canonical_path())
    }
}

/// Turns what the user pasted into `https://host`, plus a bucket if the URL
/// ended with one.
fn normalize_endpoint(text: &str) -> Result<(String, Option<String>)> {
    let text = text.trim().trim_end_matches('/');
    if text.is_empty() {
        bail!("enter the S3 endpoint (or account ID) from Cloudflare");
    }
    // A bare Cloudflare account ID.
    if text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok((format!("https://{}.r2.cloudflarestorage.com", text.to_lowercase()), None));
    }
    let rest = text.strip_prefix("https://").or_else(|| text.strip_prefix("http://")).unwrap_or(text);
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, Some(p.trim_matches('/').to_string()).filter(|p| !p.is_empty())),
        None => (rest, None),
    };
    if host.is_empty() || host.contains(' ') {
        bail!("that doesn't look like an S3 endpoint");
    }
    Ok((format!("https://{}", host.to_lowercase()), path))
}

/// Locks a file to SYSTEM and Administrators (it holds the bucket secret and
/// the encryption key). The data folder's permissions are reset when the
/// server starts, so this is re-applied then too.
pub fn protect(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("icacls")
            .arg(path)
            .args(["/inheritance:r", "/grant:r", "*S-1-5-18:F", "*S-1-5-32-544:F", "/Q"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Stops syncing on this server (the copy in the bucket is left alone).
pub fn turn_off(data_dir: &Path) {
    for f in [CONFIG_FILE, STATE_FILE, STATUS_FILE] {
        let _ = std::fs::remove_file(data_dir.join(f));
    }
}

// ---------------------------------------------------------------------------
// S3 (Signature V4) client

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// S3-style URI encoding of one path segment.
fn uri_encode(segment: &str) -> String {
    let mut out = String::new();
    for b in segment.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The `Authorization` header for an S3 request. `headers` are the signed
/// headers (lower-case names); they must include `host`,
/// `x-amz-content-sha256` and `x-amz-date`.
#[allow(clippy::too_many_arguments)]
fn authorization(
    method: &str,
    canonical_path: &str,
    headers: &[(&str, &str)],
    payload_sha256: &str,
    amz_date: &str,
    region: &str,
    access_key_id: &str,
    secret_access_key: &str,
) -> String {
    let mut sorted: Vec<(String, String)> =
        headers.iter().map(|(k, v)| (k.to_lowercase(), v.trim().to_string())).collect();
    sorted.sort();
    let canonical_headers: String = sorted.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = sorted.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let canonical_request =
        format!("{method}\n{canonical_path}\n\n{canonical_headers}\n{signed_headers}\n{payload_sha256}");
    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );
    let k_date = hmac(format!("AWS4{secret_access_key}").as_bytes(), date);
    let k_region = hmac(&k_date, region);
    let k_service = hmac(&k_region, "s3");
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex::encode(hmac(&k_signing, &string_to_sign));
    format!("AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}")
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build()
}

/// A readable reason from an S3 error response.
fn s3_error(code: u16, body: &str) -> anyhow::Error {
    let tag = |t: &str| {
        body.split_once(&format!("<{t}>")).and_then(|(_, r)| r.split_once(&format!("</{t}>"))).map(|(v, _)| v.to_string())
    };
    let what = tag("Code").unwrap_or_default();
    let hint = match (code, what.as_str()) {
        (403, "SignatureDoesNotMatch") => "the secret access key is wrong",
        (403, "InvalidAccessKeyId") | (401, _) => "the access key ID is wrong or was deleted",
        (403, _) => "the API token can't read and write this bucket",
        (404, "NoSuchBucket") => "there is no bucket with that name",
        _ => "",
    };
    match (hint.is_empty(), tag("Message")) {
        (false, _) => anyhow!("cloud storage refused the request: {hint} (HTTP {code} {what})"),
        (true, Some(msg)) => anyhow!("cloud storage error: {msg} (HTTP {code} {what})"),
        (true, None) => anyhow!("cloud storage error (HTTP {code})"),
    }
}

fn signed(cfg: &CloudConfig, method: &str, payload: &[u8]) -> (String, String, String) {
    let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let payload_sha256 = hex::encode(Sha256::digest(payload));
    let auth = authorization(
        method,
        &cfg.canonical_path(),
        &[("host", cfg.host()), ("x-amz-content-sha256", &payload_sha256), ("x-amz-date", &amz_date)],
        &payload_sha256,
        &amz_date,
        &cfg.region,
        &cfg.access_key_id,
        &cfg.secret_access_key,
    );
    (amz_date, payload_sha256, auth)
}

fn connect_error(cfg: &CloudConfig, e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => s3_error(code, &resp.into_string().unwrap_or_default()),
        ureq::Error::Transport(t) => anyhow!("cannot reach cloud storage at {}: {t}", cfg.host()),
    }
}

/// Uploads the sealed state. Returns the new ETag.
fn put(cfg: &CloudConfig, body: &[u8]) -> Result<String> {
    if cfg.is_account() {
        api_put(cfg, body)
    } else {
        s3_put(cfg, body)
    }
}

/// Downloads the sealed state (skipped if it still has `etag`).
fn get(cfg: &CloudConfig, etag: Option<&str>) -> Result<Fetched> {
    if cfg.is_account() {
        api_get(cfg, etag)
    } else {
        s3_get(cfg, etag)
    }
}

fn s3_put(cfg: &CloudConfig, body: &[u8]) -> Result<String> {
    let (amz_date, payload_sha256, auth) = signed(cfg, "PUT", body);
    let resp = agent()
        .put(&cfg.url())
        .set("x-amz-date", &amz_date)
        .set("x-amz-content-sha256", &payload_sha256)
        .set("Authorization", &auth)
        .set("Content-Type", "application/octet-stream")
        .send_bytes(body)
        .map_err(|e| connect_error(cfg, e))?;
    Ok(resp.header("ETag").unwrap_or_default().to_string())
}

pub enum Fetched {
    NotModified,
    Missing,
    Found { body: Vec<u8>, etag: String },
}

fn s3_get(cfg: &CloudConfig, etag: Option<&str>) -> Result<Fetched> {
    let (amz_date, payload_sha256, auth) = signed(cfg, "GET", b"");
    let mut req = agent()
        .get(&cfg.url())
        .set("x-amz-date", &amz_date)
        .set("x-amz-content-sha256", &payload_sha256)
        .set("Authorization", &auth);
    if let Some(tag) = etag.filter(|t| !t.is_empty()) {
        req = req.set("If-None-Match", tag);
    }
    match req.call() {
        Ok(resp) if resp.status() == 304 => Ok(Fetched::NotModified),
        Ok(resp) => {
            let etag = resp.header("ETag").unwrap_or_default().to_string();
            let mut body = Vec::new();
            std::io::Read::read_to_end(&mut resp.into_reader().take(64 << 20), &mut body)?;
            Ok(Fetched::Found { body, etag })
        }
        Err(ureq::Error::Status(404, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            if text.contains("NoSuchBucket") {
                Err(s3_error(404, &text))
            } else {
                Ok(Fetched::Missing)
            }
        }
        Err(ureq::Error::Status(304, _)) => Ok(Fetched::NotModified),
        Err(e) => Err(connect_error(cfg, e)),
    }
}

use std::io::Read as _;

// ---------------------------------------------------------------------------
// PJDesk Cloud client

/// A readable error from PJDesk Cloud.
fn api_error(api: &str, email: &str, e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(401, _) => anyhow!("signed out of PJDesk Cloud — sign in again"),
        ureq::Error::Status(code, resp) => {
            let msg = resp
                .into_string()
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v.get("error").and_then(|m| m.as_str()).map(str::to_string))
                .unwrap_or_default();
            match (code, msg.is_empty()) {
                (403, true) => anyhow!("Cloud Sync isn't turned on for {email} yet"),
                (_, true) => anyhow!("PJDesk Cloud error (HTTP {code})"),
                (_, false) => anyhow!("{}", msg.trim_end_matches('.')),
            }
        }
        ureq::Error::Transport(t) => anyhow!("cannot reach PJDesk Cloud at {api}: {t}"),
    }
}

fn api_put(cfg: &CloudConfig, body: &[u8]) -> Result<String> {
    let resp = agent()
        .put(&format!("{}/api/sync", cfg.api))
        .set("authorization", &format!("Bearer {}", cfg.token))
        .set("content-type", "application/octet-stream")
        .send_bytes(body)
        .map_err(|e| api_error(&cfg.api, &cfg.email, e))?;
    Ok(resp.header("ETag").unwrap_or_default().to_string())
}

fn api_get(cfg: &CloudConfig, etag: Option<&str>) -> Result<Fetched> {
    let mut req = agent()
        .get(&format!("{}/api/sync", cfg.api))
        .set("authorization", &format!("Bearer {}", cfg.token));
    if let Some(tag) = etag.filter(|t| !t.is_empty()) {
        req = req.set("If-None-Match", tag);
    }
    match req.call() {
        Ok(resp) if resp.status() == 304 => Ok(Fetched::NotModified),
        Ok(resp) => {
            let etag = resp.header("ETag").unwrap_or_default().to_string();
            let mut body = Vec::new();
            std::io::Read::read_to_end(&mut resp.into_reader().take(64 << 20), &mut body)?;
            Ok(Fetched::Found { body, etag })
        }
        Err(ureq::Error::Status(304, _)) => Ok(Fetched::NotModified),
        Err(ureq::Error::Status(404, _)) => Ok(Fetched::Missing),
        Err(e) => Err(api_error(&cfg.api, &cfg.email, e)),
    }
}

/// Throws away a sign-in that wasn't used (revokes its token).
pub fn discard_sign_in(cfg: &CloudConfig) {
    api_sign_out(cfg);
}

/// How many computers are connected to the running server (0 if it isn't running).
pub fn computers_online(data_dir: &Path) -> usize {
    let path = data_dir.join("online.json");
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|e| e.as_secs() <= 15).unwrap_or(false))
        .unwrap_or(false);
    if !fresh {
        return 0;
    }
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<serde_json::Value>>(&b).ok())
        .map(|v| v.len())
        .unwrap_or(0)
}

/// Best-effort sign-out (revokes this server's token).
fn api_sign_out(cfg: &CloudConfig) {
    let _ = agent()
        .post(&format!("{}/api/logout", cfg.api))
        .set("authorization", &format!("Bearer {}", cfg.token))
        .call();
}

// ---------------------------------------------------------------------------
// Sealing

fn seal(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>> {
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
    let key = LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| anyhow!("bad key"))?);
    let nonce = pjdesk_proto::random_bytes::<12>();
    let mut buf = plain.to_vec();
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(MAGIC), &mut buf)
        .map_err(|_| anyhow!("encryption failed"))?;
    let mut out = Vec::with_capacity(MAGIC.len() + nonce.len() + buf.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&buf);
    Ok(out)
}

/// The copy can't be opened with this server's key (made with another
/// password or join code).
#[derive(Debug)]
pub struct WrongKey;

impl std::fmt::Display for WrongKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the cloud copy was encrypted with a different password or join code")
    }
}

impl std::error::Error for WrongKey {}

fn open(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>> {
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
    if blob.len() < MAGIC.len() + 12 + 16 || &blob[..MAGIC.len()] != MAGIC {
        bail!("the file in the bucket isn't a PJDesk server copy");
    }
    let nonce: [u8; 12] = blob[MAGIC.len()..MAGIC.len() + 12].try_into().unwrap();
    let key = LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).map_err(|_| anyhow!("bad key"))?);
    let mut buf = blob[MAGIC.len() + 12..].to_vec();
    let plain = key
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(MAGIC), &mut buf)
        .map_err(|_| anyhow::Error::new(WrongKey))?;
    Ok(plain.to_vec())
}

// ---------------------------------------------------------------------------
// The bundle of state files

#[derive(Serialize, Deserialize)]
struct Bundle {
    format: u32,
    written_at: u64,
    /// Computer name of the server that uploaded it.
    written_by: String,
    /// Random ID of that server's data folder (so a server ignores its own upload).
    instance: String,
    version: String,
    /// Relative path → base64 contents.
    files: BTreeMap<String, String>,
}

fn allowed_path(rel: &str) -> bool {
    if STATE_FILES.contains(&rel) {
        return true;
    }
    match rel.strip_prefix("addressbook/").and_then(|n| n.strip_suffix(".json")) {
        Some(name) => {
            !name.is_empty()
                && name.len() <= 64
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                && !name.starts_with('.')
        }
        None => false,
    }
}

/// Reads this server's state files.
fn collect(data_dir: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for name in STATE_FILES {
        match std::fs::read(data_dir.join(name)) {
            Ok(bytes) => {
                files.insert(name.to_string(), bytes);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("reading {name}")),
        }
    }
    if let Ok(dir) = std::fs::read_dir(data_dir.join("addressbook")) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = format!("addressbook/{name}");
            if allowed_path(&rel) {
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    files.insert(rel, bytes);
                }
            }
        }
    }
    if !files.contains_key("server-cert.pem") || !files.contains_key("server-key.pem") {
        bail!("this server has no key yet — start it once before syncing");
    }
    Ok(files)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("sync.{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Checks a downloaded set of files and writes them into the data folder.
fn apply(data_dir: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    for rel in files.keys() {
        if !allowed_path(rel) {
            bail!("the cloud copy contains an unexpected file \"{rel}\"");
        }
    }
    let (Some(cert), Some(key)) = (files.get("server-cert.pem"), files.get("server-key.pem")) else {
        bail!("the cloud copy has no server key");
    };
    // The key pair must load before anything is replaced.
    let check = data_dir.join("cloud-incoming");
    std::fs::create_dir_all(&check)?;
    std::fs::write(check.join("cert.pem"), cert)?;
    std::fs::write(check.join("key.pem"), key)?;
    let ok = pjdesk_proto::tls::TlsIdentity::load(&check.join("cert.pem"), &check.join("key.pem"));
    let _ = std::fs::remove_dir_all(&check);
    ok.map_err(|e| anyhow!("the server key in the cloud copy doesn't load: {e}"))?;
    for name in ["accounts.json", "devices.json"] {
        if let Some(bytes) = files.get(name) {
            serde_json::from_slice::<serde_json::Value>(bytes)
                .map_err(|_| anyhow!("{name} in the cloud copy is damaged"))?;
        }
    }

    for (rel, bytes) in files {
        write_atomic(&data_dir.join(rel), bytes)?;
    }
    // Address books of accounts removed on the other server.
    if let Ok(dir) = std::fs::read_dir(data_dir.join("addressbook")) {
        for entry in dir.flatten() {
            let rel = format!("addressbook/{}", entry.file_name().to_string_lossy());
            if allowed_path(&rel) && !files.contains_key(&rel) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

fn hash_files<'a>(files: impl Iterator<Item = (&'a String, &'a Vec<u8>)>) -> String {
    let mut h = Sha256::new();
    for (name, bytes) in files {
        h.update((name.len() as u64).to_le_bytes());
        h.update(name.as_bytes());
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    hex::encode(h.finalize())
}

/// (everything but the computer list, the computer list)
fn hashes(files: &BTreeMap<String, Vec<u8>>) -> (String, String) {
    (
        hash_files(files.iter().filter(|(n, _)| n.as_str() != "devices.json")),
        hash_files(files.iter().filter(|(n, _)| n.as_str() == "devices.json")),
    )
}

// ---------------------------------------------------------------------------
// Local bookkeeping

#[derive(Default, Serialize, Deserialize)]
struct SyncState {
    #[serde(default)]
    last_push_at: u64,
    #[serde(default)]
    last_push_core: String,
    #[serde(default)]
    last_push_devices: String,
    /// ETag of the bucket copy we last uploaded or looked at.
    #[serde(default)]
    etag: String,
    #[serde(default)]
    applied_at: u64,
    #[serde(default)]
    applied_written_at: u64,
    #[serde(default)]
    applied_from: String,
}

impl SyncState {
    fn load(data_dir: &Path) -> Self {
        std::fs::read(data_dir.join(STATE_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, data_dir: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = write_atomic(&data_dir.join(STATE_FILE), &bytes);
        }
    }
}

/// A random ID for this data folder, made once.
fn instance_id(data_dir: &Path) -> String {
    let path = data_dir.join(INSTANCE_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let text = text.trim();
        if text.len() == 32 {
            return text.to_string();
        }
    }
    let id = hex::encode(pjdesk_proto::random_bytes::<16>());
    let _ = std::fs::write(&path, &id);
    id
}

fn push_due(state: &SyncState, core: &str, devices: &str, now: u64) -> bool {
    core != state.last_push_core
        || (devices != state.last_push_devices && now.saturating_sub(state.last_push_at) >= DEVICES_PUSH_INTERVAL)
}

/// Whether a standby should take a copy uploaded by another server.
fn should_apply(state: &SyncState, written_at: u64, instance: &str, mine: &str) -> bool {
    instance != mine && written_at > state.last_push_at && written_at > state.applied_written_at
}

// ---------------------------------------------------------------------------
// Status for the window

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct CloudStatus {
    #[serde(default)]
    pub enabled: bool,
    /// "active" or "standby".
    #[serde(default)]
    pub role: String,
    /// Where the copy is kept (see [`CloudConfig::describe`]).
    #[serde(default)]
    pub bucket: String,
    /// The PJDesk account, when signed in to one (empty for your own bucket).
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub last_upload_at: u64,
    #[serde(default)]
    pub last_check_at: u64,
    #[serde(default)]
    pub applied_from: String,
    #[serde(default)]
    pub applied_written_at: u64,
    #[serde(default)]
    pub error: String,
    /// When the running server wrote this (the window treats an old file as
    /// "the server isn't running").
    #[serde(default)]
    pub updated_at: u64,
}

impl CloudStatus {
    pub fn load(data_dir: &Path) -> Option<Self> {
        std::fs::read(data_dir.join(STATUS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok())
    }

    fn save(&self, data_dir: &Path) {
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = write_atomic(&data_dir.join(STATUS_FILE), &bytes);
        }
    }
}

// ---------------------------------------------------------------------------
// Operations

/// Whether this server is in use (computers connected) or on standby.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Active,
    Standby,
}

/// One sync pass, run by the server every 30 s (blocking; call from a worker
/// thread). Returns true if a newer copy from another server was applied, in
/// which case the caller reloads the key, accounts and computer list.
pub fn sync_once(data_dir: &Path, role: Role, force_push: bool) -> bool {
    let Some(cfg) = CloudConfig::load(data_dir) else {
        let _ = std::fs::remove_file(data_dir.join(STATUS_FILE));
        return false;
    };
    let mut status = CloudStatus::load(data_dir).unwrap_or_default();
    status.enabled = true;
    status.bucket = cfg.describe();
    status.account = if cfg.is_account() { cfg.email.clone() } else { String::new() };
    status.role = match role {
        Role::Active => "active",
        Role::Standby => "standby",
    }
    .into();
    let result = match role {
        Role::Active => push(data_dir, &cfg, force_push).map(|_| false),
        Role::Standby => pull(data_dir, &cfg, false),
    };
    let st = SyncState::load(data_dir);
    status.last_upload_at = st.last_push_at;
    status.applied_from = st.applied_from.clone();
    status.applied_written_at = st.applied_written_at;
    status.last_check_at = now();
    status.updated_at = now();
    let applied = match result {
        Ok(applied) => {
            status.error.clear();
            applied
        }
        Err(e) => {
            tracing::warn!("cloud sync: {e:#}");
            status.error = format!("{e:#}");
            false
        }
    };
    status.save(data_dir);
    applied
}

/// Uploads this server's state if it changed (or `force`).
fn push(data_dir: &Path, cfg: &CloudConfig, force: bool) -> Result<()> {
    let files = collect(data_dir)?;
    let (core, devices) = hashes(&files);
    let mut st = SyncState::load(data_dir);
    let now = now();
    if !force && !push_due(&st, &core, &devices, now) {
        return Ok(());
    }
    let bundle = Bundle {
        format: FORMAT,
        written_at: now,
        written_by: hostname(),
        instance: instance_id(data_dir),
        version: env!("CARGO_PKG_VERSION").into(),
        files: files.iter().map(|(k, v)| (k.clone(), b64().encode(v))).collect(),
    };
    let sealed = seal(&cfg.key_bytes()?, &serde_json::to_vec(&bundle)?)?;
    let etag = put(cfg, &sealed)?;
    st.last_push_at = now;
    st.last_push_core = core;
    st.last_push_devices = devices;
    st.etag = etag;
    st.save(data_dir);
    tracing::info!("cloud sync: uploaded this server's state to {}", cfg.describe());
    Ok(())
}

/// What the cloud holds.
enum Remote {
    NotModified,
    Missing,
    Copy { bundle: Bundle, etag: String },
}

/// Downloads and opens the cloud copy.
fn fetch(cfg: &CloudConfig, etag: Option<&str>) -> Result<Remote> {
    let (body, etag) = match get(cfg, etag)? {
        Fetched::NotModified => return Ok(Remote::NotModified),
        Fetched::Missing => return Ok(Remote::Missing),
        Fetched::Found { body, etag } => (body, etag),
    };
    let plain = open(&cfg.key_bytes()?, &body)?;
    let bundle: Bundle = serde_json::from_slice(&plain).context("the cloud copy is damaged")?;
    if bundle.format != FORMAT {
        bail!("the cloud copy was made by a newer PJDesk Server ({}) — update this server", bundle.version);
    }
    Ok(Remote::Copy { bundle, etag })
}

/// Writes a downloaded copy into the data folder and records it.
fn apply_bundle(data_dir: &Path, bundle: &Bundle, etag: String) -> Result<()> {
    let mut files = BTreeMap::new();
    for (rel, text) in &bundle.files {
        files.insert(rel.clone(), b64().decode(text).map_err(|_| anyhow!("the cloud copy is damaged"))?);
    }
    apply(data_dir, &files)?;
    let mut st = SyncState::load(data_dir);
    st.etag = etag;
    st.applied_at = now();
    st.applied_written_at = bundle.written_at;
    st.applied_from = bundle.written_by.clone();
    // What we now hold is what's in the cloud: no need to upload it back.
    let (core, devices) = hashes(&files);
    st.last_push_core = core;
    st.last_push_devices = devices;
    st.save(data_dir);
    tracing::info!("cloud sync: applied the state uploaded by {} at {}", bundle.written_by, bundle.written_at);
    Ok(())
}

/// Downloads and applies a newer copy uploaded by another server. With
/// `always`, applies whatever is there (joining).
fn pull(data_dir: &Path, cfg: &CloudConfig, always: bool) -> Result<bool> {
    let mut st = SyncState::load(data_dir);
    let etag = if always { None } else { Some(st.etag.clone()) };
    let (bundle, etag) = match fetch(cfg, etag.as_deref())? {
        Remote::NotModified => return Ok(false),
        Remote::Missing if always => {
            bail!("nothing has been uploaded yet — set up sync on the main server first")
        }
        Remote::Missing => return Ok(false),
        Remote::Copy { bundle, etag } => (bundle, etag),
    };
    let mine = instance_id(data_dir);
    if !always && !should_apply(&st, bundle.written_at, &bundle.instance, &mine) {
        st.etag = etag;
        st.save(data_dir);
        return Ok(false);
    }
    apply_bundle(data_dir, &bundle, etag)?;
    Ok(true)
}

/// After signing in, which way to go when the account already holds another
/// server's copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    /// Take the cloud copy: this server becomes a backup.
    Join,
    /// Replace the cloud copy with this server's data: this server is the main one.
    Replace,
}

/// What signing in did.
#[derive(Debug)]
pub enum Connected {
    /// The account had no copy: this server is the main one, now uploaded.
    Uploaded,
    /// The account's copy came from this server: carrying on as before.
    Resumed,
    /// This server took the main server's data and is on standby.
    Joined { from: String },
    /// This server's data replaced the cloud copy.
    Replaced,
    /// The account holds another server's copy and this server is in use (or
    /// the copy was made with an older password): ask which way to go.
    Ask { from: String, written_at: u64, readable: bool },
}

/// Connects a signed-in PJDesk account. The first server uploads; a server
/// with no computers connected joins (becomes a backup) automatically; a
/// server in use asks before replacing either side.
pub fn connect_account(data_dir: &Path, cfg: &CloudConfig, online: usize, choice: Option<Choice>) -> Result<Connected> {
    std::fs::create_dir_all(data_dir)?;
    if choice == Some(Choice::Replace) {
        setup(data_dir, cfg)?;
        return Ok(Connected::Replaced);
    }
    let remote = match fetch(cfg, None) {
        Ok(r) => Some(r),
        Err(e) if e.downcast_ref::<WrongKey>().is_some() => None,
        Err(e) => return Err(e),
    };
    match remote {
        Some(Remote::Missing) | Some(Remote::NotModified) => {
            setup(data_dir, cfg)?;
            Ok(Connected::Uploaded)
        }
        Some(Remote::Copy { bundle, etag }) => {
            if bundle.instance == instance_id(data_dir) {
                cfg.save(data_dir)?;
                let mut st = SyncState::load(data_dir);
                st.etag = etag;
                st.save(data_dir);
                let _ = sync_status_now(data_dir, if online > 0 { Role::Active } else { Role::Standby });
                Ok(Connected::Resumed)
            } else if choice == Some(Choice::Join) || (choice.is_none() && online == 0) {
                let _ = std::fs::remove_file(data_dir.join(STATE_FILE));
                apply_bundle(data_dir, &bundle, etag)?;
                cfg.save(data_dir)?;
                let _ = sync_status_now(data_dir, Role::Standby);
                Ok(Connected::Joined { from: bundle.written_by })
            } else {
                Ok(Connected::Ask { from: bundle.written_by, written_at: bundle.written_at, readable: true })
            }
        }
        None if choice == Some(Choice::Join) => {
            bail!("the cloud copy was made with an older password, so it can't be opened — replace it with this server's data instead")
        }
        None => Ok(Connected::Ask { from: String::new(), written_at: 0, readable: false }),
    }
}

/// What `setup` found in the bucket.
pub enum SetupCheck {
    Empty,
    /// The bucket already holds a copy from another sync set (or another
    /// key); setting up would replace it.
    Occupied,
}

/// Checks the bucket before a first-time setup (also proves the credentials).
pub fn check_bucket(cfg: &CloudConfig) -> Result<SetupCheck> {
    match get(cfg, None)? {
        Fetched::Missing => Ok(SetupCheck::Empty),
        Fetched::Found { .. } | Fetched::NotModified => Ok(SetupCheck::Occupied),
    }
}

/// Turns sync on as the main server: saves the settings and uploads now.
pub fn setup(data_dir: &Path, cfg: &CloudConfig) -> Result<()> {
    // A fresh setup starts fresh bookkeeping.
    let _ = std::fs::remove_file(data_dir.join(STATE_FILE));
    cfg.save(data_dir)?;
    push(data_dir, cfg, true)?;
    let _ = sync_status_now(data_dir, Role::Active);
    Ok(())
}

/// Joins as a backup server: downloads the main server's key, accounts and
/// computer list, replacing this server's, and keeps them in sync.
/// Returns the name of the server the copy came from.
pub fn join(data_dir: &Path, cfg: &CloudConfig) -> Result<String> {
    std::fs::create_dir_all(data_dir)?;
    let _ = std::fs::remove_file(data_dir.join(STATE_FILE));
    pull(data_dir, cfg, true)?;
    cfg.save(data_dir)?;
    let _ = sync_status_now(data_dir, Role::Standby);
    Ok(SyncState::load(data_dir).applied_from)
}

/// Writes a status file right away (so the window updates before the next pass).
fn sync_status_now(data_dir: &Path, role: Role) -> Option<()> {
    let cfg = CloudConfig::load(data_dir)?;
    let st = SyncState::load(data_dir);
    CloudStatus {
        enabled: true,
        role: if role == Role::Active { "active" } else { "standby" }.into(),
        bucket: cfg.describe(),
        account: if cfg.is_account() { cfg.email.clone() } else { String::new() },
        last_upload_at: st.last_push_at,
        last_check_at: now(),
        applied_from: st.applied_from,
        applied_written_at: st.applied_written_at,
        error: String::new(),
        updated_at: now(),
    }
    .save(data_dir);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigv4_matches_the_aws_example() {
        // "GET Object" example from the AWS Signature Version 4 docs.
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let auth = authorization(
            "GET",
            "/test.txt",
            &[
                ("host", "examplebucket.s3.amazonaws.com"),
                ("range", "bytes=0-9"),
                ("x-amz-content-sha256", empty),
                ("x-amz-date", "20130524T000000Z"),
            ],
            empty,
            "20130524T000000Z",
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn account_keys_match_the_website() {
        // Same vector as cloud/ (computed with the browser's WebCrypto).
        let k = derive_account_keys("Test@Example.com ", "correct horse battery staple");
        assert_eq!(hex::encode(k.auth), "42616cb9b45a3c4c44fb66e9ab20502f7085aca2d593e673687b4347b6954a28");
        assert_eq!(hex::encode(k.sync), "8e6f31878360a88ccc3ef542c6ac825ea725afd4b0775b1d6d91fea4ecabc015");
    }

    #[test]
    fn old_settings_files_still_load() {
        // A 0.3.5 own-bucket settings file has no "mode".
        let json = r#"{"endpoint":"https://x.r2.cloudflarestorage.com","bucket":"b","access_key_id":"k","secret_access_key":"s","key":"00","region":"auto"}"#;
        let cfg: CloudConfig = serde_json::from_str(json).unwrap();
        assert!(!cfg.is_account());
        assert_eq!(cfg.describe(), "bucket \"b\"");
    }

    #[test]
    fn seal_round_trips_and_rejects_the_wrong_key() {
        let key = [3u8; 32];
        let blob = seal(&key, b"hello").unwrap();
        assert_eq!(open(&key, &blob).unwrap(), b"hello");
        assert!(open(&[4u8; 32], &blob).is_err());
        assert!(open(&key, b"nope").is_err());
    }

    #[test]
    fn join_code_round_trips() {
        let cfg = CloudConfig::new("https://abc.r2.cloudflarestorage.com/pjdesk-sync", "", "AKID", "SECRET").unwrap();
        assert_eq!(cfg.bucket, "pjdesk-sync");
        assert_eq!(cfg.endpoint, "https://abc.r2.cloudflarestorage.com");
        let code = cfg.join_code();
        assert!(code.starts_with(JOIN_PREFIX));
        // Pasting with stray line breaks still works.
        let wrapped = format!("{}\n{}", &code[..20], &code[20..]);
        assert_eq!(CloudConfig::from_join_code(&wrapped).unwrap(), cfg);
        assert!(CloudConfig::from_join_code("pjdesk-sync-1:abc").is_err());
        assert!(CloudConfig::from_join_code("hello").is_err());
    }

    #[test]
    fn endpoint_accepts_what_cloudflare_shows() {
        let id = "0123456789abcdef0123456789ABCDEF";
        assert_eq!(normalize_endpoint(id).unwrap().0, "https://0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com");
        assert_eq!(normalize_endpoint("x.r2.cloudflarestorage.com/").unwrap(), ("https://x.r2.cloudflarestorage.com".into(), None));
        assert!(normalize_endpoint(" ").is_err());
    }

    #[test]
    fn only_state_files_are_accepted() {
        assert!(allowed_path("accounts.json"));
        assert!(allowed_path("addressbook/ben.json"));
        assert!(!allowed_path("addressbook/../server-key.pem"));
        assert!(!allowed_path("../evil.json"));
        assert!(!allowed_path("cloud-sync.json"));
        assert!(!allowed_path("addressbook/.json"));
    }

    #[test]
    fn decisions() {
        let mut st = SyncState { last_push_core: "c".into(), last_push_devices: "d".into(), last_push_at: 1000, ..Default::default() };
        assert!(!push_due(&st, "c", "d", 2000));
        assert!(push_due(&st, "c2", "d", 1001)); // accounts changed: right away
        assert!(!push_due(&st, "c", "d2", 1100)); // only the computer list: wait
        assert!(push_due(&st, "c", "d2", 1000 + DEVICES_PUSH_INTERVAL));
        // A standby takes another server's newer copy, never its own or an older one.
        assert!(should_apply(&st, 1500, "other", "me"));
        assert!(!should_apply(&st, 1500, "me", "me"));
        assert!(!should_apply(&st, 900, "other", "me"));
        st.applied_written_at = 1600;
        assert!(!should_apply(&st, 1500, "other", "me"));
    }

    #[test]
    fn collect_and_apply_round_trip() {
        let base = std::env::temp_dir().join(format!("pjdesk-cloud-test-{}", std::process::id()));
        let (a, b) = (base.join("a"), base.join("b"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(a.join("addressbook")).unwrap();
        let id = pjdesk_proto::tls::TlsIdentity::load_or_create(&a.join("server-cert.pem"), &a.join("server-key.pem"), "pjdesk")
            .unwrap();
        std::fs::write(a.join("accounts.json"), b"{}").unwrap();
        std::fs::write(a.join("devices.json"), b"{\"devices\":{}}").unwrap();
        std::fs::write(a.join("addressbook/ben.json"), b"[]").unwrap();
        // b has a stale address book for a removed account.
        std::fs::create_dir_all(b.join("addressbook")).unwrap();
        std::fs::write(b.join("addressbook/gone.json"), b"[]").unwrap();

        let files = collect(&a).unwrap();
        apply(&b, &files).unwrap();
        assert!(b.join("addressbook/ben.json").exists());
        assert!(!b.join("addressbook/gone.json").exists());
        let copied = pjdesk_proto::tls::TlsIdentity::load(&b.join("server-cert.pem"), &b.join("server-key.pem")).unwrap();
        assert_eq!(copied.fingerprint(), id.fingerprint());
        let _ = std::fs::remove_dir_all(&base);
    }
}
