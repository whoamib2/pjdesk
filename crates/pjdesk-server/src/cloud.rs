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
//! Roles:
//! * Until someone forces a role, a server with computers connected is
//!   **active** and one with none is on **standby** (the same as 0.3.x).
//! * **Promote** (`cloud promote`, or "Make this the primary") pulls the
//!   latest copy, applies it, and writes it back under a higher **epoch**
//!   with this server's instance id. That epoch is a fencing token: uploads
//!   are conditional on it, so two servers never both write. The previous
//!   primary sees the newer epoch on its next check, stops uploading, and
//!   becomes the standby.
//! * **Demote** hands the epoch to the other server and becomes standby only
//!   after that server owns the newer generation. It does not leave zero primaries.
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
/// Small JSON object each server updates with its version and role, so a
/// primary can see that a standby is healthy before both restart for an update.
const HEALTH_OBJECT: &str = "pjdesk-server.health";
const MAGIC: &[u8; 8] = b"PJSYNC1\0";
const JOIN_PREFIX: &str = "pjdesk-sync-1:";
const FORMAT: u32 = 1;

/// The server's state files (relative to the data folder), plus every
/// `addressbook/<user>.json`.
const STATE_FILES: [&str; 6] = [
    "server-cert.pem",
    "server-key.pem",
    "accounts.json",
    "devices.json",
    "access-keys.json",
    "network-access.json",
];
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

/// This computer's name, for the peer line and for ignoring our own health rows.
pub fn computer_name() -> String {
    hostname()
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
    let (auth, sync) = pjdesk_proto::access::derive_account_keys(email, password);
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
    let body = login_body(&email, &hex::encode(keys.auth), &format!("PJDesk Server on {}", hostname()));
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
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            bail!("{}", cloud_error_text(code, &text))
        }
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

    /// Account mode with no saved token. A client sign-in can fill the token
    /// for one mirror pull without replacing `cloud-sync.json`.
    pub(crate) fn blank_account() -> Self {
        Self { mode: "account".into(), ..Self::blank() }
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
    if !valid_endpoint_host(host) {
        bail!("that doesn't look like an S3 endpoint");
    }
    Ok((format!("https://{}", host.to_lowercase()), path))
}

/// Hosts we will put in a signed request. Rejects anything that could inject a
/// header or point the request at a different scheme or userinfo.
fn valid_endpoint_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('.')
        && !host.starts_with('-')
        && !host.contains("..")
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
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
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex::encode(Sha256::digest(canonical_request.as_bytes())));
    let k_date = hmac(format!("AWS4{secret_access_key}").as_bytes(), date);
    let k_region = hmac(&k_date, region);
    let k_service = hmac(&k_region, "s3");
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex::encode(hmac(&k_signing, &string_to_sign));
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    )
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build()
}

/// A readable reason from an S3 error response.
fn s3_error(code: u16, body: &str) -> anyhow::Error {
    let tag = |t: &str| {
        body.split_once(&format!("<{t}>"))
            .and_then(|(_, r)| r.split_once(&format!("</{t}>")))
            .map(|(v, _)| v.to_string())
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
    signed_for(cfg, OBJECT, method, payload, &[])
}

/// Like [`signed`], for `object`, with extra signed headers (lower-case names).
/// Conditional uploads sign `if-match` / `if-none-match`; leaving them out of
/// the signature makes R2 reject the request.
fn signed_for(
    cfg: &CloudConfig,
    object: &str,
    method: &str,
    payload: &[u8],
    extra: &[(&str, &str)],
) -> (String, String, String) {
    let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let payload_sha256 = hex::encode(Sha256::digest(payload));
    let mut headers = vec![
        ("host", cfg.host()),
        ("x-amz-content-sha256", payload_sha256.as_str()),
        ("x-amz-date", amz_date.as_str()),
    ];
    headers.extend_from_slice(extra);
    let path = format!("/{}/{}", uri_encode(&cfg.bucket), uri_encode(object));
    let auth = authorization(
        method,
        &path,
        &headers,
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

/// How a write is allowed to land. Omitted headers stay unconditional, which
/// is what a 0.3.x server sends.
#[derive(Clone, Debug)]
enum Guard {
    /// Replace whatever is there.
    None,
    /// Replace only while the object still has this ETag.
    IfMatch(String),
    /// Create the object only when it is absent (`If-None-Match: *`).
    IfAbsent,
}

/// The cloud copy changed between the read and the write. Another server won.
#[derive(Debug)]
struct LostRace;

impl std::fmt::Display for LostRace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the cloud copy changed while this server was writing")
    }
}

impl std::error::Error for LostRace {}

fn lost_race(err: &anyhow::Error) -> bool {
    err.downcast_ref::<LostRace>().is_some()
}

/// Uploads `body` as `object`. Returns the new ETag.
fn put_object(cfg: &CloudConfig, object: &str, body: &[u8], guard: &Guard) -> Result<String> {
    if cfg.is_account() {
        if object != OBJECT {
            api_put_health_raw(cfg, body)
        } else {
            api_put(cfg, body, guard)
        }
    } else {
        s3_put(cfg, object, body, guard)
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

fn s3_put(cfg: &CloudConfig, object: &str, body: &[u8], guard: &Guard) -> Result<String> {
    let extra: Vec<(&str, &str)> = match guard {
        Guard::None => Vec::new(),
        Guard::IfMatch(tag) => vec![("if-match", tag.as_str())],
        Guard::IfAbsent => vec![("if-none-match", "*")],
    };
    let (amz_date, payload_sha256, auth) = signed_for(cfg, object, "PUT", body, &extra);
    let path = format!("/{}/{}", uri_encode(&cfg.bucket), uri_encode(object));
    let mut req = agent()
        .put(&format!("{}{path}", cfg.endpoint))
        .set("x-amz-date", &amz_date)
        .set("x-amz-content-sha256", &payload_sha256)
        .set("Authorization", &auth)
        .set("Content-Type", "application/octet-stream");
    match guard {
        Guard::None => {}
        Guard::IfMatch(tag) => req = req.set("If-Match", tag),
        Guard::IfAbsent => req = req.set("If-None-Match", "*"),
    }
    match req.send_bytes(body) {
        Ok(resp) => Ok(resp.header("ETag").unwrap_or_default().to_string()),
        Err(ureq::Error::Status(412 | 409, _)) => Err(LostRace.into()),
        Err(e) => Err(connect_error(cfg, e)),
    }
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

/// POST /api/login body. pjdesk.com expects `auth_key`, the hex HMAC derived
/// from the password. A `password` field is rejected with
/// "Enter your email and password."
pub fn login_body(email: &str, auth_key_hex: &str, device: &str) -> serde_json::Value {
    serde_json::json!({
        "email": email,
        "auth_key": auth_key_hex,
        "device": device,
    })
}

/// The `error` string from a PJDesk Cloud HTTP response, unchanged.
/// A 400 from login is "Enter your email and password." A 401 is
/// "Wrong email or password."
pub fn cloud_error_text(status: u16, body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(msg) = v.get("error").and_then(|m| m.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            return msg.to_string();
        }
    }
    if status == 401 {
        "Wrong email or password.".into()
    } else if body.trim().is_empty() {
        format!("PJDesk Cloud error (HTTP {status})")
    } else {
        body.trim().to_string()
    }
}

/// Email stored in `cloud-sync.json`, when that file is an account sign-in.
/// The token stays in the file. Callers that show this to the window must not
/// copy the token out with it.
pub fn saved_account_email(data_dir: &Path) -> String {
    let Ok(bytes) = std::fs::read(data_dir.join(CONFIG_FILE)) else {
        return String::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return String::new();
    };
    if value.get("mode").and_then(|m| m.as_str()) == Some("r2") {
        return String::new();
    }
    let email = value.get("email").and_then(|e| e.as_str()).unwrap_or("").trim();
    let token = value.get("token").and_then(|t| t.as_str()).unwrap_or("").trim();
    if email.is_empty() || token.is_empty() {
        return String::new();
    }
    email.to_string()
}

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

fn api_put(cfg: &CloudConfig, body: &[u8], guard: &Guard) -> Result<String> {
    let mut req = agent()
        .put(&format!("{}/api/sync", cfg.api))
        .set("authorization", &format!("Bearer {}", cfg.token))
        .set("content-type", "application/octet-stream");
    match guard {
        Guard::None => {}
        Guard::IfMatch(tag) => req = req.set("If-Match", tag),
        Guard::IfAbsent => req = req.set("If-None-Match", "*"),
    }
    match req.send_bytes(body) {
        Ok(resp) => Ok(resp.header("ETag").unwrap_or_default().to_string()),
        Err(ureq::Error::Status(412 | 409, _)) => Err(LostRace.into()),
        Err(e) => Err(api_error(&cfg.api, &cfg.email, e)),
    }
}

/// Account-mode health is a JSON document the Worker merges per instance.
/// A 404 means the website has not been redeployed with the health route yet.
fn api_put_health_raw(cfg: &CloudConfig, body: &[u8]) -> Result<String> {
    agent()
        .put(&format!("{}/api/sync/health", cfg.api))
        .set("authorization", &format!("Bearer {}", cfg.token))
        .set("content-type", "application/json")
        .send_bytes(body)
        .map(|_| String::new())
        .map_err(|e| api_error(&cfg.api, &cfg.email, e))
}

fn api_get(cfg: &CloudConfig, etag: Option<&str>) -> Result<Fetched> {
    let mut req = agent().get(&format!("{}/api/sync", cfg.api)).set("authorization", &format!("Bearer {}", cfg.token));
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
    if !online_file_is_fresh(&path) {
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
    let _ =
        agent().post(&format!("{}/api/logout", cfg.api)).set("authorization", &format!("Bearer {}", cfg.token)).call();
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
    /// Fencing token. A promote adds one. Copies from before 0.4 omit it (0),
    /// and a 0.3 server ignores the field when it reads a newer copy.
    #[serde(default)]
    epoch: u64,
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

const MAX_BUNDLE_FILE: usize = 4 * 1024 * 1024;
const MAX_BUNDLE_TOTAL: usize = 8 * 1024 * 1024;
/// A standby ignores a copy dated further ahead than this. A holder of the
/// sync key could otherwise stamp a copy with a far-future time and make
/// every later, genuine copy look older.
const MAX_CLOCK_SKEW: u64 = 24 * 60 * 60;

fn timestamp_ok(written_at: u64) -> bool {
    written_at <= now().saturating_add(MAX_CLOCK_SKEW)
}

/// Checks a downloaded set of files and writes them into the data folder.
fn apply(data_dir: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let mut total = 0usize;
    for (rel, bytes) in files {
        if !allowed_path(rel) {
            bail!("the cloud copy contains an unexpected file \"{rel}\"");
        }
        if bytes.len() > MAX_BUNDLE_FILE {
            bail!("{rel} in the cloud copy is too large");
        }
        total = total.saturating_add(bytes.len());
    }
    if total > MAX_BUNDLE_TOTAL {
        bail!("the cloud copy is too large");
    }
    let (Some(cert), Some(key)) = (files.get("server-cert.pem"), files.get("server-key.pem")) else {
        bail!("the cloud copy has no server key");
    };
    // The key pair must load before anything is replaced.
    let check = data_dir.join("cloud-incoming");
    std::fs::create_dir_all(&check)?;
    std::fs::write(check.join("cert.pem"), cert)?;
    std::fs::write(check.join("key.pem"), key)?;
    // The key is on disk for the load check below. Lock it before that, in
    // case the data folder's ACL has not been repaired yet.
    protect(&check.join("cert.pem"));
    protect(&check.join("key.pem"));
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
        let path = data_dir.join(rel);
        write_atomic(&path, bytes)?;
        // The private key and the account file must not be left world-readable
        // on a machine whose data folder still has the old inherited ACL.
        if rel.ends_with(".pem") || rel.ends_with(".json") {
            protect(&path);
        }
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
    /// `""` follows whether computers are connected, `"active"` stays primary,
    /// `"standby"` never uploads.
    #[serde(default)]
    desired: String,
    /// Highest epoch this server has written or accepted.
    #[serde(default)]
    epoch: u64,
    /// Instance id of the server that last wrote the copy we know about.
    #[serde(default)]
    active_instance: String,
    /// Computer name of that server.
    #[serde(default)]
    active_name: String,
    /// Shown in the window until the next role change replaces it.
    #[serde(default)]
    notice: String,
    /// Generation that made this server step down. Higher than `epoch` until
    /// this server accepts that copy or promotes past it.
    #[serde(default)]
    fenced_epoch: u64,
    /// Instance that owns `fenced_epoch`.
    #[serde(default)]
    fenced_instance: String,
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
pub fn instance_of(data_dir: &Path) -> String {
    instance_id(data_dir)
}

pub fn local_epoch(data_dir: &Path) -> u64 {
    SyncState::load(data_dir).epoch
}

pub fn unix_now() -> u64 {
    now()
}

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
    /// Fencing generation of the cloud copy this server last saw.
    #[serde(default)]
    pub epoch: u64,
    /// Instance id of the server that owns that generation.
    #[serde(default)]
    pub active_instance: String,
    /// Computer name of the server that owns that generation.
    #[serde(default)]
    pub active_name: String,
    /// Takeover / step-down note for the window and the log.
    #[serde(default)]
    pub notice: String,
    /// The other server in this Cloud Sync pair, from its health report.
    #[serde(default)]
    pub peer_name: String,
    #[serde(default)]
    pub peer_version: String,
    #[serde(default)]
    pub peer_role: String,
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

/// What an upload is allowed to do, given the cloud copy's fencing token.
#[derive(Debug, PartialEq, Eq)]
pub enum Fence {
    /// This instance may write.
    MayUpload,
    /// A newer epoch from another instance is primary. Stop writing and apply.
    Superseded,
    /// This epoch already belongs to another instance. Do not write.
    Held,
}

/// `remote_present` is false when the object is missing. Epoch 0 is a copy
/// from before fencing existed: either server may still upload, and the
/// conditional put stops them writing at the same moment. Once the epoch is
/// above zero, only the instance that owns it (or a promote that raises it)
/// may write.
pub fn fence(remote_present: bool, remote_epoch: u64, remote_instance: &str, mine: &str, local_epoch: u64) -> Fence {
    if !remote_present || remote_instance.is_empty() || remote_instance == mine {
        return Fence::MayUpload;
    }
    if remote_epoch > local_epoch {
        return Fence::Superseded;
    }
    if remote_epoch > 0 && remote_epoch >= local_epoch {
        return Fence::Held;
    }
    Fence::MayUpload
}

/// The epoch a promote writes. Always at least 1, and never wraps.
pub fn next_epoch(remote_epoch: u64) -> u64 {
    remote_epoch.saturating_add(1).max(1)
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Active => "active",
        Role::Standby => "standby",
    }
}

fn effective_role(st: &SyncState, mine: &str, heuristic: Role) -> Role {
    if pjdesk_proto::failover::generation_is_primary(
        &st.desired,
        st.epoch,
        &st.active_instance,
        mine,
        st.fenced_epoch,
        &st.fenced_instance,
        heuristic == Role::Active,
    ) {
        Role::Active
    } else {
        Role::Standby
    }
}

/// True when this process may keep the live computer table.
/// A server with no cloud sync is the only server. A cloud standby, including
/// one whose desired role is standby or whose epoch belongs to another
/// instance, must not accept host registrations as authoritative.
///
/// `cloud-status.json` saying standby is not enough to refuse. After a
/// restart the live table is empty, and an older sync wrote that file from
/// the empty table. The server that owns the cloud copy, and a server with
/// no owner recorded yet, still accepts hosts so the table can fill again.
/// "The primary BENLENOVODESK" or "The standby CHIP20", for a sentence a person reads.
pub fn role_label(data_dir: &Path) -> String {
    let role = if accepts_host_registrations(data_dir) { "primary" } else { "standby" };
    format!("The {role} {}", computer_name())
}

pub fn accepts_host_registrations(data_dir: &Path) -> bool {
    if CloudConfig::load(data_dir).is_none() {
        return true;
    }
    let st = SyncState::load(data_dir);
    let mine = instance_id(data_dir);
    pjdesk_proto::failover::generation_is_primary(
        &st.desired,
        st.epoch,
        &st.active_instance,
        &mine,
        st.fenced_epoch,
        &st.fenced_instance,
        true,
    )
}

/// True when the running server wrote `cloud-status.json` recently.
pub fn status_is_fresh(data_dir: &Path) -> bool {
    CloudStatus::load(data_dir).is_some_and(|s| s.updated_at + 90 >= now())
}

/// One sync pass, run by the primary every 5 minutes (blocking; call from a worker
/// thread). Returns true if a newer copy from another server was applied, in
/// which case the caller reloads the key, accounts and computer list.
///
/// `role` is only the hint from "are any computers connected". A sticky
/// promote or demote, and a newer cloud epoch, override it.
pub fn sync_once(data_dir: &Path, role: Role, force_push: bool) -> bool {
    let Some(cfg) = CloudConfig::load(data_dir) else {
        // Sign-out removes the file. An upgrade, a reinstall, and a bad read
        // leave it in place. Do not clear the status the window uses to show
        // the account while that file is still there.
        if !data_dir.join(CONFIG_FILE).exists() {
            let _ = std::fs::remove_file(data_dir.join(STATUS_FILE));
        }
        return false;
    };
    let mut applied = false;
    let mine = instance_id(data_dir);
    let mut effective = effective_role(&SyncState::load(data_dir), &mine, role);
    let result = (|| -> Result<()> {
        match absorb(data_dir, &cfg)? {
            Fence::Superseded => {
                applied = true;
                effective = Role::Standby;
            }
            Fence::Held => {
                effective = Role::Standby;
                if pull(data_dir, &cfg, false)? {
                    applied = true;
                }
            }
            Fence::MayUpload => {
                effective = effective_role(&SyncState::load(data_dir), &mine, role);
                match effective {
                    Role::Standby => {
                        if pull(data_dir, &cfg, false)? {
                            applied = true;
                        }
                    }
                    Role::Active => push(data_dir, &cfg, force_push)?,
                }
            }
        }
        Ok(())
    })();
    publish_health(data_dir, effective);
    let st = SyncState::load(data_dir);
    let mut status = CloudStatus::load(data_dir).unwrap_or_default();
    status.enabled = true;
    status.bucket = cfg.describe();
    status.account = if cfg.is_account() { cfg.email.clone() } else { String::new() };
    status.role = role_name(effective).into();
    status.last_upload_at = st.last_push_at;
    status.applied_from = st.applied_from.clone();
    status.applied_written_at = st.applied_written_at;
    status.epoch = st.epoch;
    status.active_instance = st.active_instance.clone();
    status.active_name = st.active_name.clone();
    status.notice = st.notice.clone();
    status.last_check_at = now();
    status.updated_at = now();
    if let Some((peer_name, peer_version, peer_role)) = other_peer(data_dir) {
        status.peer_name = peer_name;
        status.peer_version = peer_version;
        status.peer_role = peer_role;
    }
    match result {
        Ok(()) => status.error.clear(),
        Err(e) => {
            tracing::warn!("cloud sync: {e:#}");
            status.error = format!("{e:#}");
            applied = false;
        }
    }
    status.save(data_dir);
    applied
}

/// Reads the cloud copy and steps down when another instance holds a newer
/// generation, or the same non-zero generation.
fn absorb(data_dir: &Path, cfg: &CloudConfig) -> Result<Fence> {
    let mine = instance_id(data_dir);
    let st = SyncState::load(data_dir);
    let Remote::Copy { bundle, etag } = fetch(cfg, None)? else {
        return Ok(Fence::MayUpload);
    };
    let decision = fence(true, bundle.epoch, &bundle.instance, &mine, st.epoch);
    match decision {
        Fence::Superseded => {
            let name = bundle.written_by.clone();
            let epoch = bundle.epoch;
            apply_bundle(data_dir, &bundle, etag)?;
            let mut st = SyncState::load(data_dir);
            st.desired = "standby".into();
            st.fenced_epoch = epoch;
            st.fenced_instance = bundle.instance.clone();
            st.notice = format!(
                "{name} is now the primary (generation {epoch}). This server switched to standby and will not take the primary back on its own."
            );
            st.save(data_dir);
            tracing::warn!("failover: {}", st.notice);
            Ok(Fence::Superseded)
        }
        Fence::Held => {
            let name = bundle.written_by.clone();
            let epoch = bundle.epoch;
            if should_apply(&st, bundle.written_at, &bundle.instance, &mine) {
                apply_bundle(data_dir, &bundle, etag)?;
            } else {
                let mut st = SyncState::load(data_dir);
                st.etag = etag;
                st.epoch = epoch;
                st.active_instance = bundle.instance.clone();
                st.active_name = name.clone();
                st.save(data_dir);
            }
            let mut st = SyncState::load(data_dir);
            if st.desired == "active" {
                st.desired = "standby".into();
                st.notice = format!("{name} holds generation {epoch}. This server will not upload.");
                tracing::warn!("{}", st.notice);
            }
            st.save(data_dir);
            Ok(Fence::Held)
        }
        Fence::MayUpload => {
            let mut st = SyncState::load(data_dir);
            st.etag = etag;
            if bundle.instance == mine {
                st.epoch = bundle.epoch.max(st.epoch);
                st.active_instance = mine;
                st.active_name = bundle.written_by;
            }
            st.save(data_dir);
            Ok(Fence::MayUpload)
        }
    }
}

/// Uploads this server's state if it changed (or `force`). The put matches
/// the ETag just observed, and is skipped when that put loses the race.
fn push(data_dir: &Path, cfg: &CloudConfig, force: bool) -> Result<()> {
    let files = collect(data_dir)?;
    let (core, devices) = hashes(&files);
    let st = SyncState::load(data_dir);
    let now = now();
    if !force && !push_due(&st, &core, &devices, now) {
        return Ok(());
    }
    let guard = if st.etag.is_empty() { Guard::IfAbsent } else { Guard::IfMatch(st.etag.clone()) };
    match upload(data_dir, cfg, &files, core, devices, st.epoch, &guard) {
        Err(e) if lost_race(&e) => {
            tracing::info!("cloud sync: skipped an upload because the cloud copy changed");
            let _ = absorb(data_dir, cfg);
            Ok(())
        }
        other => other,
    }
}

/// Seals `files` at `epoch` and writes them. A follow-up read that does not
/// show this instance at that epoch is [`LostRace`] (the other writer won).
fn upload(
    data_dir: &Path,
    cfg: &CloudConfig,
    files: &BTreeMap<String, Vec<u8>>,
    core: String,
    devices: String,
    epoch: u64,
    guard: &Guard,
) -> Result<()> {
    let now = now();
    let mine = instance_id(data_dir);
    let bundle = Bundle {
        format: FORMAT,
        written_at: now,
        written_by: hostname(),
        instance: mine.clone(),
        version: env!("CARGO_PKG_VERSION").into(),
        epoch,
        files: files.iter().map(|(k, v)| (k.clone(), b64().encode(v))).collect(),
    };
    let sealed = seal(&cfg.key_bytes()?, &serde_json::to_vec(&bundle)?)?;
    let etag = put_object(cfg, OBJECT, &sealed, guard)?;
    match fetch(cfg, None)? {
        Remote::Copy { bundle: got, etag: got_etag } if got.instance == mine && got.epoch >= epoch => {
            let mut st = SyncState::load(data_dir);
            st.last_push_at = now;
            st.last_push_core = core;
            st.last_push_devices = devices;
            st.etag = if got_etag.is_empty() { etag } else { got_etag };
            st.epoch = got.epoch;
            st.active_instance = got.instance;
            st.active_name = got.written_by;
            st.save(data_dir);
            tracing::info!("cloud sync: uploaded this server's state to {} (generation {})", cfg.describe(), got.epoch);
            Ok(())
        }
        _ => Err(LostRace.into()),
    }
}

/// Pulls the latest copy, applies it when it came from another server, bumps
/// the epoch, and writes this server as the primary. Usable whether or not
/// the current primary is online. A lost race leaves this server on standby.
pub fn promote(data_dir: &Path) -> Result<()> {
    let cfg = CloudConfig::load(data_dir).context("Cloud sync is not set up on this server.")?;
    let mine = instance_id(data_dir);
    let mut previous_primary = String::new();
    let (epoch, guard) = match fetch(&cfg, None)? {
        Remote::Missing => (1, Guard::IfAbsent),
        Remote::NotModified => bail!("could not read the cloud copy"),
        Remote::Copy { bundle, etag } => {
            if bundle.instance != mine {
                previous_primary = bundle.written_by.clone();
                apply_bundle(data_dir, &bundle, etag.clone())?;
            }
            (next_epoch(bundle.epoch), Guard::IfMatch(etag))
        }
    };
    let files = collect(data_dir)?;
    let (core, devices) = hashes(&files);
    if let Err(e) = upload(data_dir, &cfg, &files, core, devices, epoch, &guard) {
        if lost_race(&e) {
            let _ = absorb(data_dir, &cfg);
            let mut st = SyncState::load(data_dir);
            st.desired = "standby".into();
            st.notice = "Another server became the primary at the same time. This server stayed on standby.".into();
            st.save(data_dir);
            let _ = sync_status_now(data_dir, Role::Standby);
            bail!("Another server became the primary at the same time. This server stayed on standby.");
        }
        return Err(e);
    }
    let mut st = SyncState::load(data_dir);
    st.desired = "active".into();
    st.fenced_epoch = 0;
    st.fenced_instance.clear();
    let other = if previous_primary.eq_ignore_ascii_case(&hostname()) { String::new() } else { previous_primary };
    st.notice = pjdesk_proto::failover::primary_result(st.epoch, &other);
    st.save(data_dir);
    let _ = sync_status_now(data_dir, Role::Active);
    tracing::info!("failover: {}", st.notice);
    register_network(data_dir, Role::Active, true);
    publish_heartbeat(data_dir);
    Ok(())
}

/// Becomes standby only when another server already owns a newer generation.
/// A demote that would leave no primary is refused.
pub fn demote(data_dir: &Path) -> Result<()> {
    if CloudConfig::load(data_dir).is_none() {
        bail!("Cloud sync is not set up on this server.");
    }
    let local = SyncState::load(data_dir).epoch;
    let mine = instance_id(data_dir);
    let owner = cloud_owner(data_dir)?;
    if !pjdesk_proto::failover::peer_took_the_epoch(local, owner.epoch, &owner.instance, &mine) {
        bail!(
            "The other server has not taken the primary. This server stayed the primary (generation {local})."
        );
    }
    let _ = yield_to_epoch(data_dir, owner.epoch, &owner.instance, &owner.name);
    let mut st = SyncState::load(data_dir);
    st.notice = pjdesk_proto::failover::standby_result(&owner.name, owner.epoch);
    st.save(data_dir);
    let _ = sync_status_now(data_dir, Role::Standby);
    tracing::info!("{}", st.notice);
    Ok(())
}

/// Who currently owns the sealed cloud copy.
pub struct CloudOwner {
    pub epoch: u64,
    pub instance: String,
    pub name: String,
}

pub fn cloud_owner(data_dir: &Path) -> Result<CloudOwner> {
    let cfg = CloudConfig::load(data_dir).context("Cloud sync is not set up on this server.")?;
    match fetch(&cfg, None)? {
        Remote::Copy { bundle, .. } => Ok(CloudOwner {
            epoch: bundle.epoch,
            instance: bundle.instance,
            name: bundle.written_by,
        }),
        Remote::Missing => Ok(CloudOwner { epoch: 0, instance: String::new(), name: String::new() }),
        Remote::NotModified => bail!("could not read the cloud copy"),
    }
}

/// The sync key both servers of this account share. Used to prove a handoff.
pub fn sync_key(data_dir: &Path) -> Option<[u8; 32]> {
    CloudConfig::load(data_dir)?.key_bytes().ok()
}

/// Addresses to try when asking the other server to take the primary.
/// LAN first, then public, then the account's other published address.
pub fn peer_dial_targets(data_dir: &Path) -> Vec<String> {
    let (public_addrs, lan_addrs, names) = peer_endpoints(data_dir);
    let mut out = Vec::new();
    for addr in lan_addrs.into_iter().chain(public_addrs).chain(names) {
        let addr = addr.trim().to_string();
        if addr.is_empty() || out.iter().any(|have: &String| have.eq_ignore_ascii_case(&addr)) {
            continue;
        }
        out.push(addr);
    }
    if let Some(extra) = peer_target(data_dir) {
        if !out.iter().any(|have| pjdesk_proto::failover::same_server(have, &extra)) {
            out.push(extra);
        }
    }
    out
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
    if !timestamp_ok(bundle.written_at) {
        bail!("the cloud copy is dated too far in the future, so it was not applied");
    }
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
    st.epoch = bundle.epoch;
    st.active_instance = bundle.instance.clone();
    st.active_name = bundle.written_by.clone();
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
                st.epoch = bundle.epoch;
                st.active_instance = bundle.instance.clone();
                st.active_name = bundle.written_by.clone();
                st.save(data_dir);
                let _ = sync_status_now(data_dir, if online > 0 { Role::Active } else { Role::Standby });
                Ok(Connected::Resumed)
            } else if choice == Some(Choice::Join) || (choice.is_none() && online == 0) {
                let _ = std::fs::remove_file(data_dir.join(STATE_FILE));
                apply_bundle(data_dir, &bundle, etag)?;
                let mut st = SyncState::load(data_dir);
                st.desired = "standby".into();
                st.save(data_dir);
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

/// Turns sync on as the main server: saves the settings and uploads now,
/// starting fencing at generation 1 (or one past whatever is already there).
pub fn setup(data_dir: &Path, cfg: &CloudConfig) -> Result<()> {
    // A fresh setup starts fresh bookkeeping.
    let _ = std::fs::remove_file(data_dir.join(STATE_FILE));
    cfg.save(data_dir)?;
    let (epoch, guard) = match fetch(cfg, None)? {
        Remote::Missing | Remote::NotModified => (1, Guard::IfAbsent),
        Remote::Copy { bundle, etag } => (next_epoch(bundle.epoch), Guard::IfMatch(etag)),
    };
    let files = collect(data_dir)?;
    let (core, devices) = hashes(&files);
    upload(data_dir, cfg, &files, core, devices, epoch, &guard)?;
    let mut st = SyncState::load(data_dir);
    st.desired = "active".into();
    st.save(data_dir);
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
    let mut st = SyncState::load(data_dir);
    st.desired = "standby".into();
    st.save(data_dir);
    let _ = sync_status_now(data_dir, Role::Standby);
    Ok(SyncState::load(data_dir).applied_from)
}

/// Writes a status file right away (so the window updates before the next pass).
fn sync_status_now(data_dir: &Path, role: Role) -> Option<()> {
    let cfg = CloudConfig::load(data_dir)?;
    let st = SyncState::load(data_dir);
    let (peer_name, peer_version, peer_role) = other_peer(data_dir).unwrap_or_default();
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
        epoch: st.epoch,
        active_instance: st.active_instance,
        active_name: st.active_name,
        notice: st.notice,
        peer_name,
        peer_version,
        peer_role,
    }
    .save(data_dir);
    Some(())
}

/// Name, version, and role of a different server that published health.
/// An older instance of this computer is not the peer. The role word follows
/// this server's generation, not a stale health label.
fn other_peer(data_dir: &Path) -> Option<(String, String, String)> {
    let (ok, mut peers) = peer_health_result(data_dir);
    if !ok {
        return None;
    }
    let mine = instance_id(data_dir);
    let mine_name = hostname();
    peers.retain(|p| {
        pjdesk_proto::failover::counts_as_other_server(&p.instance, &p.name, &p.version, &mine, &mine_name)
    });
    peers.sort_by_key(|p| std::cmp::Reverse(p.at));
    let role = pjdesk_proto::failover::peer_role_label(accepts_host_registrations(data_dir));
    Some(match peers.into_iter().next() {
        Some(p) => (p.name, p.version, role.into()),
        None => (String::new(), String::new(), role.into()),
    })
}

// ---------------------------------------------------------------------------
// Health reports (version + role), so a primary can see a standby before
// both restart for an update. Failure here never fails a sync.

/// One server's view of itself, published next to the sealed copy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerHealth {
    pub instance: String,
    pub name: String,
    pub version: String,
    pub role: String,
    pub epoch: u64,
    pub healthy: bool,
    pub at: u64,
    /// Unix seconds until which a standby must not promote. Missing on older reports.
    #[serde(default)]
    pub maintenance_until: u64,
    /// This server's public IP endpoints. Missing on older reports.
    #[serde(default)]
    pub public_addrs: Vec<String>,
    /// This server's LAN endpoints. Missing on older reports.
    #[serde(default)]
    pub lan_addrs: Vec<String>,
    /// Hostnames this server still publishes. Missing on older reports.
    #[serde(default)]
    pub names: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct HealthDoc {
    #[serde(default)]
    peers: BTreeMap<String, PeerHealth>,
}

/// Health, and the computer list, every few seconds. The full sync still
/// publishes the network address on its own pass.
pub fn publish_heartbeat(data_dir: &Path) {
    let changed = refresh_advertisement(data_dir);
    let Some(cfg) = CloudConfig::load(data_dir) else { return };
    let role = if accepts_host_registrations(data_dir) { Role::Active } else { Role::Standby };
    let me = health_report(data_dir, role);
    if let Err(e) = write_health(&cfg, &me) {
        tracing::debug!("failover heartbeat: {e:#}");
    }
    stamp_live_peer(data_dir, role);
    if changed {
        register_network(data_dir, role, false);
    }
    // A version change reaches the account on this heartbeat, not the 30 s sync.
    publish_presence(data_dir);
    // The standby does not host the live registrations. The primary published
    // them; read that list back so this window's online count matches.
    if !accepts_host_registrations(data_dir) {
        refresh_cloud_presence(data_dir);
    }
}

const MAINT_FILE: &str = "maintenance-until.txt";
const SEEN_FILE: &str = "maintenance-seen.txt";

fn maintenance_deadline(data_dir: &Path) -> u64 {
    std::fs::read_to_string(data_dir.join(MAINT_FILE)).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0)
}

fn health_report(data_dir: &Path, role: Role) -> PeerHealth {
    let st = SyncState::load(data_dir);
    let until = maintenance_deadline(data_dir);
    let holding = until > now();
    let mine = local_advertised(data_dir);
    PeerHealth {
        instance: instance_id(data_dir),
        name: hostname(),
        version: env!("CARGO_PKG_VERSION").into(),
        role: role_name(role).into(),
        epoch: st.epoch,
        healthy: !holding,
        maintenance_until: if holding { until } else { 0 },
        at: now(),
        public_addrs: mine.public,
        lan_addrs: mine.lan,
        names: mine.names,
    }
}

const ROUTES_FILE: &str = "server-routes.json";
const ROUTES_GEN: &str = "routes-generation.txt";

/// Rewrites the cached catalog when this server's addresses or the peer's
/// addresses changed. Returns true when clients and the cloud should be told now.
pub fn refresh_advertisement(data_dir: &Path) -> bool {
    let _ = crate::public_addr::poll_public_ip(data_dir, now());
    let routes = assemble_routes(data_dir);
    let changed = store_routes(data_dir, &routes);
    let _ = crate::public_addr::refresh_address_warning(data_dir);
    changed
}

fn store_routes(data_dir: &Path, routes: &pjdesk_proto::routes::ServerRoutes) -> bool {
    let bytes = serde_json::to_vec(routes).unwrap_or_default();
    let prev = std::fs::read(data_dir.join(ROUTES_FILE)).unwrap_or_default();
    if bytes == prev {
        return false;
    }
    if std::fs::write(data_dir.join(ROUTES_FILE), &bytes).is_err() {
        return false;
    }
    let gen = std::fs::read_to_string(data_dir.join(ROUTES_GEN)).ok().and_then(|text| text.trim().parse::<u64>().ok()).unwrap_or(0);
    let _ = std::fs::write(data_dir.join(ROUTES_GEN), gen.saturating_add(1).to_string());
    true
}

pub fn advertised_routes(data_dir: &Path) -> pjdesk_proto::routes::ServerRoutes {
    std::fs::read(data_dir.join(ROUTES_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| assemble_routes(data_dir))
}

pub fn routes_line(data_dir: &Path) -> String {
    let routes = std::fs::read(data_dir.join(ROUTES_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    pjdesk_proto::routes::routes_text(&routes)
}

/// The catalog to push, once, when `last` is behind the generation on disk.
pub fn take_routes_push(data_dir: &Path, last: &mut u64) -> Option<pjdesk_proto::routes::ServerRoutes> {
    let gen = std::fs::read_to_string(data_dir.join(ROUTES_GEN)).ok().and_then(|text| text.trim().parse::<u64>().ok()).unwrap_or(0);
    if gen == 0 || gen == *last {
        return None;
    }
    *last = gen;
    Some(advertised_routes(data_dir))
}

pub fn redirect_text(data_dir: &Path) -> String {
    let routes = advertised_routes(data_dir);
    let primary = routes
        .primary_public
        .first()
        .cloned()
        .or_else(|| routes.primary_lan.first().cloned())
        .unwrap_or_default();
    pjdesk_proto::signal::standby_redirect(&primary, &routes)
}

fn local_advertised(data_dir: &Path) -> pjdesk_proto::routes::Advertised {
    let port = crate::public_addr::remembered_port(data_dir);
    let published = crate::public_addr::load(data_dir).map(|record| record.address).unwrap_or_default();
    let watch = crate::public_addr::IpWatch::load(data_dir);
    let lan = crate::public_addr::lan_endpoints(port);
    pjdesk_proto::routes::advertise(&published, &watch.observed, &lan, port)
}

fn assemble_routes(data_dir: &Path) -> pjdesk_proto::routes::ServerRoutes {
    let built = assemble_routes_now(data_dir);
    let previous = std::fs::read(data_dir.join(ROUTES_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    pjdesk_proto::routes::keep_other_server(&previous, &built)
}

fn assemble_routes_now(data_dir: &Path) -> pjdesk_proto::routes::ServerRoutes {
    let mine = local_advertised(data_dir);
    let (peer_public, peer_lan, peer_names) = peer_endpoints(data_dir);
    let primary = accepts_host_registrations(data_dir);
    let mut names = mine.names.clone();
    for name in &peer_names {
        if !names.iter().any(|have| have.eq_ignore_ascii_case(name)) {
            names.push(name.clone());
        }
    }
    if primary {
        return pjdesk_proto::routes::ServerRoutes {
            primary_lan: mine.lan,
            primary_public: mine.public,
            standby_lan: peer_lan,
            standby_public: peer_public,
            names,
            ..Default::default()
        };
    }
    let mut primary_public = peer_public;
    let primary_lan = peer_lan;
    if primary_public.is_empty() && primary_lan.is_empty() {
        if let Some(addr) = peer_target(data_dir) {
            let legacy = pjdesk_proto::routes::ServerRoutes::from_legacy(&addr, &[], "", "", &[], "");
            primary_public = legacy.primary_public;
            names.extend(legacy.names);
        }
    }
    pjdesk_proto::routes::ServerRoutes {
        primary_lan,
        primary_public,
        standby_lan: mine.lan,
        standby_public: mine.public,
        names,
        ..Default::default()
    }
}

fn peer_endpoints(data_dir: &Path) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (ok, mut peers) = peer_health_result(data_dir);
    if !ok {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let mine = instance_id(data_dir);
    let mine_name = hostname();
    peers.retain(|peer| {
        pjdesk_proto::failover::counts_as_other_server(&peer.instance, &peer.name, &peer.version, &mine, &mine_name)
    });
    peers.sort_by_key(|peer| std::cmp::Reverse(peer.at));
    match peers.into_iter().next() {
        Some(peer) => (peer.public_addrs, peer.lan_addrs, peer.names),
        None => (Vec::new(), Vec::new(), Vec::new()),
    }
}

/// Asks the standby not to promote during a planned update or restart.
/// A standby, and a server with no account, does not announce.
pub fn announce_maintenance(data_dir: &Path) {
    if CloudConfig::load(data_dir).is_none() || !accepts_host_registrations(data_dir) {
        return;
    }
    let until = now().saturating_add(pjdesk_proto::failover::MAINTENANCE_HOLD_SECS);
    if std::fs::write(data_dir.join(MAINT_FILE), until.to_string()).is_err() {
        return;
    }
    tracing::info!(
        "failover: planned restart; this primary announced maintenance so the standby does not take over"
    );
    let Some(cfg) = CloudConfig::load(data_dir) else { return };
    let me = health_report(data_dir, Role::Active);
    if let Err(e) = write_health(&cfg, &me) {
        tracing::debug!("failover maintenance: {e:#}");
    }
}

/// Remembers a maintenance sighting so a later unread cloud does not forget it.
/// `clear` drops it because the primary is healthy again. Returns the deadline still in force.
pub fn note_peer_maintenance(data_dir: &Path, observed_until: u64, clear: bool) -> u64 {
    let path = data_dir.join(SEEN_FILE);
    if clear {
        let _ = std::fs::remove_file(&path);
        return 0;
    }
    let now = now();
    if observed_until > now {
        let _ = std::fs::write(&path, observed_until.to_string());
        return observed_until;
    }
    std::fs::read_to_string(&path).ok().and_then(|text| text.trim().parse().ok()).filter(|until: &u64| *until > now).unwrap_or(0)
}

/// Rewrites the window's peer line from the latest health, and republishes
/// the network slot when the saved "Published as" sentence disagrees.
fn stamp_live_peer(data_dir: &Path, role: Role) {
    let mut status = CloudStatus::load(data_dir).unwrap_or_default();
    let fresh_peer = other_peer(data_dir);
    let st = SyncState::load(data_dir);
    let role_text = role_name(role);
    status.role = role_text.into();
    status.epoch = st.epoch;
    status.active_instance = st.active_instance;
    status.active_name = st.active_name;
    status.notice = st.notice;
    if let Some((name, version, peer_role)) = fresh_peer {
        status.peer_name = name;
        status.peer_version = version;
        status.peer_role = peer_role;
    }
    status.updated_at = now();
    status.enabled = true;
    if let Some(cfg) = CloudConfig::load(data_dir) {
        if status.account.is_empty() && cfg.is_account() {
            status.account = cfg.email.clone();
        }
        if status.bucket.is_empty() {
            status.bucket = cfg.describe();
        }
    }
    status.save(data_dir);
    if note_says_other_role(data_dir, role) {
        let _ = std::fs::remove_file(data_dir.join("network-publish.json"));
        register_network(data_dir, role, false);
    }
}

fn note_says_other_role(data_dir: &Path, role: Role) -> bool {
    let Some(record) = crate::public_addr::load(data_dir) else { return false };
    let other = if role == Role::Active {
        "Published as the standby server."
    } else {
        "Published as the active server."
    };
    record.note == other
}

/// Steps down because another server holds a newer generation.
/// Returns false when this server already recorded that generation.
pub fn yield_to_epoch(data_dir: &Path, remote_epoch: u64, remote_instance: &str, remote_name: &str) -> bool {
    let mut st = SyncState::load(data_dir);
    if st.desired == "standby" && st.fenced_epoch == remote_epoch && st.fenced_instance == remote_instance {
        return false;
    }
    let name = if remote_name.trim().is_empty() { "The other server" } else { remote_name.trim() };
    st.desired = "standby".into();
    st.fenced_epoch = remote_epoch;
    st.fenced_instance = remote_instance.to_string();
    st.notice = pjdesk_proto::failover::standby_result(name, remote_epoch);
    st.save(data_dir);
    tracing::warn!("failover: {}", st.notice);
    let _ = sync_status_now(data_dir, Role::Standby);
    register_network(data_dir, Role::Standby, false);
    true
}

/// True when this server already recorded that generation as someone else's.
pub fn already_fenced(data_dir: &Path, epoch: u64, instance: &str) -> bool {
    let st = SyncState::load(data_dir);
    st.desired == "standby" && st.fenced_epoch == epoch && st.fenced_instance == instance
}

const PEER_FILE: &str = "failover-peer.txt";

/// The other server's address. When this server is the published primary,
/// that is the standby. Otherwise it is the primary. Empty when the account
/// has no other address.
pub fn migration_target(data_dir: &Path) -> String {
    let cached = std::fs::read_to_string(data_dir.join(PEER_FILE))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let Some(cfg) = CloudConfig::load(data_dir) else {
        return cached.unwrap_or_default();
    };
    if !cfg.is_account() || cfg.token.is_empty() {
        return cached.unwrap_or_default();
    }
    let mine = crate::public_addr::load(data_dir).map(|record| record.address).unwrap_or_default();
    match network_addresses(&cfg) {
        Ok((active, standby)) => {
            let active = active.trim();
            let standby = standby.trim();
            let other = if !active.is_empty() && !pjdesk_proto::failover::same_server(&mine, active) {
                active
            } else if !standby.is_empty() && !pjdesk_proto::failover::same_server(&mine, standby) {
                standby
            } else {
                ""
            };
            if other.is_empty() {
                cached.unwrap_or_default()
            } else {
                let _ = std::fs::write(data_dir.join(PEER_FILE), other);
                other.to_string()
            }
        }
        Err(e) => {
            tracing::debug!("failover: could not read the account server list: {e:#}");
            cached.unwrap_or_default()
        }
    }
}

/// The other server's address from the last successful lookup. No network call.
pub fn cached_peer_address(data_dir: &Path) -> String {
    std::fs::read_to_string(data_dir.join(PEER_FILE))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_default()
}

/// Address of the account's active server, when it is not this server.
/// A failed lookup keeps the last address this server learned.
pub fn peer_target(data_dir: &Path) -> Option<String> {
    let cached = std::fs::read_to_string(data_dir.join(PEER_FILE))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    let Some(cfg) = CloudConfig::load(data_dir) else {
        return cached;
    };
    if !cfg.is_account() || cfg.token.is_empty() {
        return cached;
    }
    let mine = crate::public_addr::load(data_dir).map(|record| record.address).unwrap_or_default();
    match network_addresses(&cfg) {
        Ok((active, standby)) => match pjdesk_proto::failover::choose_peer(&mine, &active, &standby) {
            Some(addr) => {
                let _ = std::fs::write(data_dir.join(PEER_FILE), &addr);
                Some(addr)
            }
            None => cached,
        },
        Err(e) => {
            tracing::debug!("failover: could not read the account server list: {e:#}");
            cached
        }
    }
}

fn network_addresses(cfg: &CloudConfig) -> Result<(String, String)> {
    #[derive(Deserialize)]
    struct Net {
        #[serde(default)]
        active_address: String,
        #[serde(default)]
        standby_address: String,
    }
    #[derive(Deserialize)]
    struct Doc {
        #[serde(default)]
        network: Option<Net>,
    }
    let text = match agent()
        .get(&format!("{}/api/network", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .call()
    {
        Ok(resp) => resp.into_string().unwrap_or_default(),
        Err(e) => return Err(api_error(&cfg.api, &cfg.email, e)),
    };
    let doc: Doc = serde_json::from_str(&text).context("unexpected network reply")?;
    let net = doc.network.unwrap_or(Net { active_address: String::new(), standby_address: String::new() });
    Ok((net.active_address, net.standby_address))
}

const HEALTH_CACHE: &str = "health-cache.json";

#[derive(Default, Serialize, Deserialize)]
struct HealthCache {
    #[serde(default)]
    at: u64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    peers: Vec<PeerHealth>,
}

/// The last cloud health read. `allow_network` false never calls pjdesk.com.
/// A cache younger than `max_age` is reused.
pub fn peer_health_cached(data_dir: &Path, max_age: u64, allow_network: bool) -> (bool, Vec<PeerHealth>) {
    let path = data_dir.join(HEALTH_CACHE);
    let cached: Option<HealthCache> = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
    if let Some(cache) = &cached {
        if cache.at > 0 && now().saturating_sub(cache.at) < max_age {
            return (cache.ok, cache.peers.clone());
        }
    }
    if !allow_network {
        return cached.map(|cache| (cache.ok, cache.peers)).unwrap_or((false, Vec::new()));
    }
    let result = peer_health_result(data_dir);
    let row = HealthCache { at: now(), ok: result.0, peers: result.1.clone() };
    if let Ok(bytes) = serde_json::to_vec(&row) {
        let _ = std::fs::write(path, bytes);
    }
    result
}

/// `(read succeeded, peers)`. A failed read is not an empty fleet.
pub fn peer_health_result(data_dir: &Path) -> (bool, Vec<PeerHealth>) {
    let Some(cfg) = CloudConfig::load(data_dir) else {
        return (false, Vec::new());
    };
    match read_health(&cfg) {
        Ok(doc) => (true, doc.peers.into_values().collect()),
        Err(e) => {
            tracing::debug!("failover: cloud health: {e:#}");
            (false, Vec::new())
        }
    }
}

fn publish_health(data_dir: &Path, role: Role) {
    let Some(cfg) = CloudConfig::load(data_dir) else { return };
    let me = health_report(data_dir, role);
    if let Err(e) = write_health(&cfg, &me) {
        tracing::debug!("cloud health: {e:#}");
    }
    if let Err(e) = pull_network_policy(data_dir, &cfg) {
        tracing::debug!("network policy: {e:#}");
    }
    let _ = crate::public_addr::poll_public_ip(data_dir, now());
    register_network(data_dir, role, false);
    publish_presence(data_dir);
}

/// Computers the primary published to the account. The standby has no local
/// `online.json` rows for them. Missing when this server is not the account
/// owner, or the account could not be read. A failed read leaves the last file.
pub const PRESENCE_FILE: &str = "cloud-presence.json";
const PRESENCE_FRESH_SECS: u64 = 90;

#[derive(Default, Serialize, Deserialize, PartialEq, Eq)]
struct PresenceCache {
    #[serde(default)]
    at: u64,
    #[serde(default)]
    online: Vec<String>,
}

pub fn refresh_cloud_presence(data_dir: &Path) {
    let Some(online) = fetch_presence_online(data_dir) else { return };
    let path = data_dir.join(PRESENCE_FILE);
    let previous = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<PresenceCache>(&bytes).ok())
        .map(|cache| cache.online)
        .unwrap_or_default();
    if previous != online {
        let listed = if online.is_empty() {
            "none".to_string()
        } else {
            online.iter().map(|id| pjdesk_proto::format_id(id)).collect::<Vec<_>>().join(", ")
        };
        tracing::info!("presence: the cloud reports {} online: {listed}", online.len());
    }
    let cache = PresenceCache { at: now(), online };
    if let Ok(bytes) = serde_json::to_vec(&cache) {
        let _ = std::fs::write(path, bytes);
    }
}

/// Ids the cloud still calls online. Empty when the file is missing or older
/// than [`PRESENCE_FRESH_SECS`], so a dead primary does not stay "online".
pub fn cloud_presence_online(data_dir: &Path) -> std::collections::HashSet<String> {
    let path = data_dir.join(PRESENCE_FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        return std::collections::HashSet::new();
    };
    let Ok(cache) = serde_json::from_slice::<PresenceCache>(&bytes) else {
        return std::collections::HashSet::new();
    };
    if now().saturating_sub(cache.at) > PRESENCE_FRESH_SECS {
        return std::collections::HashSet::new();
    }
    cache.online.into_iter().collect()
}

fn fetch_presence_online(data_dir: &Path) -> Option<Vec<String>> {
    let cfg = CloudConfig::load(data_dir)?;
    if !cfg.is_account() || cfg.token.is_empty() {
        return None;
    }
    let text = agent()
        .get(&format!("{}/api/network", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
    let rows = doc.get("presence")?.as_array()?;
    let mut ids = Vec::new();
    for row in rows {
        if !row.get("online").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let Some(id) = row.get("id").and_then(|v| v.as_str()) else { continue };
        if id.len() == 9 && id.bytes().all(|b| b.is_ascii_digit()) {
            ids.push(id.to_string());
        }
    }
    ids.sort();
    ids.dedup();
    Some(ids)
}

/// Uploads this server's computer list to the owner's account. The user name
/// is in the body the account API stores; it is not written to the log.
fn publish_presence(data_dir: &Path) {
    let Some(cfg) = CloudConfig::load(data_dir) else { return };
    if !cfg.is_account() || cfg.token.is_empty() {
        return;
    }
    if !accepts_host_registrations(data_dir) {
        tracing::info!("presence: this server is the standby and did not publish the computer list");
        return;
    }
    let Ok(reg) = crate::registry::Registry::load(data_dir.join("devices.json")) else { return };
    let Some(online) = online_ids(data_dir) else { return };
    let online_path = data_dir.join("online.json");
    let snapshot_stale = online_path.exists() && !online_file_is_fresh(&online_path);
    let now = now();
    let versions = live_versions(data_dir);
    let mut devices = Vec::new();
    for row in reg.presence() {
        if row.last_seen == 0 || now.saturating_sub(row.last_seen) > 90 * 24 * 3600 {
            continue;
        }
        let live = online.contains(&row.id);
        if pjdesk_proto::presence_stale(row.last_seen, live, now) {
            continue;
        }
        // The connected process's version wins over an older row in devices.json.
        let app_version = versions.get(&row.id).cloned().filter(|v| !v.is_empty()).unwrap_or(row.app_version);
        devices.push(serde_json::json!({
            "id": row.id,
            "hostname": row.hostname,
            "os_family": row.os_family,
            "os_version": row.os_version,
            "console_user": row.console_user,
            "display_name": row.display_name,
            "last_seen": row.last_seen,
            "online": live,
            "signed_in": row.signed_in,
            "app_version": app_version,
            "install_key": row.install_sha256,
        }));
        if devices.len() >= 500 {
            break;
        }
    }
    let body = serde_json::json!({ "devices": devices });
    let stamp = data_dir.join("presence-publish.json");
    let previous =
        std::fs::read(&stamp).ok().and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let signature = |value: &serde_json::Value| {
        let mut copy = value.clone();
        if let Some(rows) = copy.get_mut("devices").and_then(|v| v.as_array_mut()) {
            for row in rows {
                if let Some(obj) = row.as_object_mut() {
                    obj.remove("last_seen");
                }
            }
        }
        copy
    };
    let same = previous.as_ref().map(|prev| signature(prev) == signature(&body)).unwrap_or(false);
    let posted_at = stamp.metadata().ok().and_then(|meta| meta.modified().ok()).and_then(|time| time.elapsed().ok());
    if same && posted_at.map(|age| age.as_secs() < pjdesk_proto::failover::CLOUD_PRESENCE_REFRESH_SECS).unwrap_or(false) {
        return;
    }
    if snapshot_stale {
        tracing::warn!("online.json is older than {ONLINE_FRESH_SECS}s; publishing every computer offline");
    }
    if let Some(prev) = previous.as_ref() {
        log_presence_changes(prev, &body);
    }
    let result = agent()
        .post(&format!("{}/api/network/presence", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set("content-type", "application/json")
        .send_string(&body.to_string());
    match result {
        Ok(_) => {
            if let Ok(bytes) = serde_json::to_vec(&body) {
                let _ = std::fs::write(&stamp, bytes);
            }
            let online_names: Vec<String> = devices
                .iter()
                .filter(|row| row.get("online").and_then(|v| v.as_bool()).unwrap_or(false))
                .filter_map(|row| row.get("id").and_then(|v| v.as_str()))
                .map(pjdesk_proto::format_id)
                .collect();
            let listed = if online_names.is_empty() { "none".to_string() } else { online_names.join(", ") };
            tracing::info!(
                "presence: published {} computers from this primary, online: {listed}",
                devices.len()
            );
        }
        Err(ureq::Error::Status(code, _)) => {
            tracing::debug!("presence: the account did not accept the list ({code})");
        }
        Err(_) => tracing::debug!("presence: could not reach the account"),
    }
}

/// How long `online.json` may sit before it no longer counts as the live set.
/// The server rewrites it about every 2 seconds. 45 seconds still rejects a
/// dead process, and it leaves room for a slow rename or a stalled permission
/// update on Windows.
pub const ONLINE_FRESH_SECS: u64 = 45;

/// Versions from a fresh `online.json`. The connected process wrote these.
/// A stale file contributes nothing, so an old version cannot win.
fn live_versions(data_dir: &Path) -> std::collections::HashMap<String, String> {
    let path = data_dir.join("online.json");
    if !online_file_is_fresh(&path) {
        return std::collections::HashMap::new();
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return std::collections::HashMap::new();
    };
    let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes) else {
        return std::collections::HashMap::new();
    };
    let mut out = std::collections::HashMap::new();
    for row in rows {
        let Some(id) = row.get("id").and_then(|id| id.as_str()) else { continue };
        let Some(version) = row.get("version").and_then(|v| v.as_str()).map(str::trim).filter(|v| !v.is_empty()) else {
            continue;
        };
        out.insert(id.to_string(), version.to_string());
    }
    out
}

/// Ids in a fresh `online.json`. `None` means the file was there but could not
/// be read, so the caller must leave the published list alone. An empty set
/// means nothing is online. `last_seen` in the registry is not read.
fn online_ids(data_dir: &Path) -> Option<std::collections::HashSet<String>> {
    let path = data_dir.join("online.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(std::collections::HashSet::new()),
        Err(e) => {
            tracing::warn!("online.json could not be read ({e}); presence was not changed");
            return None;
        }
    };
    if !online_file_is_fresh(&path) {
        return Some(std::collections::HashSet::new());
    }
    match serde_json::from_slice::<Vec<serde_json::Value>>(&bytes) {
        Ok(rows) => Some(
            rows.into_iter()
                .filter_map(|row| row.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                .collect(),
        ),
        Err(e) => {
            tracing::warn!("online.json could not be read ({e}); presence was not changed");
            None
        }
    }
}

/// True when `online.json` was written recently. The file time counts, and so
/// does the newest `at` stamp inside the file, so a stalled modification time
/// does not mark every computer offline.
pub fn online_file_is_fresh(path: &Path) -> bool {
    if online_mtime_age(path).is_some_and(|age| age <= ONLINE_FRESH_SECS) {
        return true;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    embedded_at_is_fresh(&bytes)
}

fn online_mtime_age(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.elapsed().map(|e| e.as_secs()).unwrap_or(u64::MAX))
}

fn embedded_at_is_fresh(bytes: &[u8]) -> bool {
    let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(bytes) else {
        return false;
    };
    let Some(at) = rows.iter().filter_map(|row| row.get("at").and_then(|v| v.as_u64())).max() else {
        return false;
    };
    now().saturating_sub(at) <= ONLINE_FRESH_SECS
}

fn presence_flags(body: &serde_json::Value) -> std::collections::BTreeMap<String, bool> {
    let mut flags = std::collections::BTreeMap::new();
    let Some(devices) = body.get("devices").and_then(|d| d.as_array()) else {
        return flags;
    };
    for row in devices {
        let Some(id) = row.get("id").and_then(|v| v.as_str()) else { continue };
        flags.insert(id.to_string(), row.get("online").and_then(|v| v.as_bool()).unwrap_or(false));
    }
    flags
}

fn log_presence_changes(previous: &serde_json::Value, next: &serde_json::Value) {
    let before = presence_flags(previous);
    let after = presence_flags(next);
    let mut ids: Vec<String> = before.keys().chain(after.keys()).cloned().collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        let was = before.get(&id).copied();
        let now_online = after.get(&id).copied();
        if was == now_online {
            continue;
        }
        let label = pjdesk_proto::format_id(&id);
        match now_online {
            Some(true) => tracing::info!("presence: {label} is online"),
            Some(false) => tracing::info!("presence: {label} is offline"),
            None => tracing::info!("presence: {label} left the list"),
        }
    }
}

fn online_ids_from(bytes: &[u8]) -> std::collections::HashSet<String> {
    let Ok(rows) = serde_json::from_slice::<Vec<serde_json::Value>>(bytes) else {
        return std::collections::HashSet::new();
    };
    rows.into_iter().filter_map(|row| row.get("id").and_then(|id| id.as_str()).map(|s| s.to_string())).collect()
}

/// Copies the account's network policy into `network-access.json`. The local
/// certificate fingerprint is kept. Failure leaves the current file alone.
fn pull_network_policy(data_dir: &Path, cfg: &CloudConfig) -> Result<()> {
    if !cfg.is_account() || cfg.token.is_empty() {
        return Ok(());
    }
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        require_accounts: bool,
        #[serde(default)]
        public_key: String,
        #[serde(default)]
        allowed_accounts: Vec<String>,
        #[serde(default)]
        revoked_devices: Vec<String>,
    }
    let reply = match agent()
        .get(&format!("{}/api/network/policy", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .call()
    {
        Ok(resp) => {
            let text = resp.into_string().unwrap_or_default();
            serde_json::from_str::<Reply>(&text).context("unexpected network policy")?
        }
        Err(ureq::Error::Status(404, _)) => return Ok(()),
        Err(e) => return Err(api_error(&cfg.api, &cfg.email, e)),
    };
    let mut policy = crate::access::Policy::load(data_dir);
    let cloud_on = reply.require_accounts;
    let push_off = cloud_on && !policy.require_chosen;
    policy.require_accounts = crate::access::adopt_cloud_require(policy.require_chosen, cloud_on);
    if reply.public_key.len() == 64 {
        policy.public_key = reply.public_key;
    }
    policy.allowed_accounts = reply.allowed_accounts;
    policy.revoked_devices = reply.revoked_devices;
    policy.save(data_dir).context("saving network policy")?;
    if push_off {
        push_require_accounts(data_dir, false);
    }
    Ok(())
}

/// Publishes this server into the owner's network using the role it has now.
/// `online` is how many computers are connected, used only when no role has
/// been chosen yet. `replace` is the owner's confirmation that a different
/// server already listed there should be overwritten.
pub fn publish_network(data_dir: &Path, online: usize, replace: bool) {
    let hint = if online > 0 { Role::Active } else { Role::Standby };
    let role = effective_role(&SyncState::load(data_dir), &instance_id(data_dir), hint);
    register_network(data_dir, role, replace);
}

/// Registers this server's fingerprint and public address in the slot for
/// `role`. A missing cloud sign-in, a private address that has not been
/// confirmed, and a network error leave the local note for the Server window.
fn register_network(data_dir: &Path, role: Role, replace: bool) {
    let Some(cfg) = CloudConfig::load(data_dir) else { return };
    if !cfg.is_account() || cfg.token.is_empty() {
        return;
    }
    let Some(identity) = crate::server::read_identity(data_dir) else { return };
    let fingerprint = pjdesk_proto::tls::format_fingerprint(&identity.fingerprint());
    let port = crate::public_addr::remembered_port(data_dir);
    let mut record = crate::public_addr::ensure(data_dir, port);
    if record.address.trim().is_empty() {
        record.note = "Type the public address computers use to reach this server.".into();
        crate::public_addr::save(data_dir, &record);
        return;
    }
    if crate::public_addr::is_private_address(&record.address) && !record.confirmed {
        record.needs_confirm = false;
        record.note =
            "This is a local network address. Confirm it before it is published. It is not sent until you do.".into();
        crate::public_addr::save(data_dir, &record);
        return;
    }
    let watch = crate::public_addr::IpWatch::load(data_dir);
    let lan = crate::public_addr::lan_endpoints(port);
    let endpoints = pjdesk_proto::routes::advertise(record.address.trim(), &watch.observed, &lan, port);
    let publication = serde_json::json!({
        "role": role_name(role),
        "address": record.address.trim(),
        "fingerprint": fingerprint,
        "instance": instance_id(data_dir),
        "lan": lan,
        "public_ip": watch.observed,
        "checked_at": watch.checked_at,
        "replace": replace,
        "endpoints": {
            "public": endpoints.public,
            "lan": endpoints.lan,
            "names": endpoints.names,
        },
    });
    let stamp = data_dir.join("network-publish.json");
    if !replace {
        if let Ok(prev) = std::fs::read(&stamp) {
            if serde_json::from_slice::<serde_json::Value>(&prev).ok().as_ref() == Some(&publication) {
                return;
            }
        }
    }
    let body = serde_json::json!({
        "role": role_name(role),
        "address": record.address.trim(),
        "fingerprint": fingerprint,
        "instance": instance_id(data_dir),
        "lan": lan,
        "public_ip": watch.observed,
        "checked_at": watch.checked_at,
        "confirm": record.confirmed || !crate::public_addr::is_private_address(&record.address),
        "replace": replace,
        "endpoints": {
            "public": endpoints.public,
            "lan": endpoints.lan,
            "names": endpoints.names,
        },
    });
    let result = agent()
        .post(&format!("{}/api/network/register", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set("content-type", "application/json")
        .send_string(&body.to_string());
    match result {
        Ok(resp) => {
            let text = resp.into_string().unwrap_or_default();
            let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            let needs = value.get("needs_confirm").and_then(|v| v.as_bool()).unwrap_or(false);
            record.needs_confirm = needs;
            record.note = if needs {
                value
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("My network already lists a different server. Confirm in the PJDesk Server window to replace it.")
                    .to_string()
            } else if role == Role::Active {
                "Published as the active server.".into()
            } else {
                "Published as the standby server.".into()
            };
            crate::public_addr::save(data_dir, &record);
            if !needs {
                if let Some(routes) = value.get("network").and_then(|net| net.get("routes")) {
                    if let Ok(parsed) = serde_json::from_value::<pjdesk_proto::routes::ServerRoutes>(routes.clone()) {
                        if !parsed.is_empty() {
                            let previous = advertised_routes(data_dir);
                            let _ = store_routes(data_dir, &pjdesk_proto::routes::keep_other_server(&previous, &parsed));
                        }
                    }
                }
                let changed = value.get("changed").and_then(|v| v.as_bool()).unwrap_or(true);
                if changed {
                    tracing::info!("network: published this server as {}", role_name(role));
                }
                if let Ok(bytes) = serde_json::to_vec(&publication) {
                    let _ = std::fs::write(&stamp, bytes);
                }
            }
        }
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            let message =
                value.get("error").and_then(|v| v.as_str()).unwrap_or("Could not publish this server.").to_string();
            record.needs_confirm = value.get("needs_confirm").and_then(|v| v.as_bool()).unwrap_or(code == 409);
            record.note = message;
            crate::public_addr::save(data_dir, &record);
            tracing::debug!("network register: {code}");
        }
        Err(e) => {
            record.note = "Could not reach pjdesk.com to publish this server.".into();
            crate::public_addr::save(data_dir, &record);
            tracing::debug!("network register: {e}");
        }
    }
}

/// Tells pjdesk.com that this server now requires (or no longer requires) a
/// signed-in account. The local file is already updated by the caller.
pub fn push_require_accounts(data_dir: &Path, require: bool) {
    let Some(cfg) = CloudConfig::load(data_dir) else { return };
    if !cfg.is_account() || cfg.token.is_empty() {
        return;
    }
    let body = serde_json::json!({ "require_accounts": require }).to_string();
    let result = agent()
        .put(&format!("{}/api/network", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set("content-type", "application/json")
        .send_string(&body);
    if let Err(e) = result {
        tracing::debug!("could not publish the signed-in-accounts switch: {e}");
    }
}

/// A network key the owner created. The blob itself is not stored.
#[derive(Clone, Deserialize)]
pub struct NetworkInvite {
    pub id: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub bound_email: String,
    #[serde(default)]
    pub max_uses: Option<u32>,
    #[serde(default)]
    pub uses: u32,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub revoked_at: Option<u64>,
    #[serde(default)]
    pub history: Vec<InviteUse>,
}

#[derive(Clone, Deserialize)]
pub struct InviteUse {
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub used_at: u64,
}

pub struct CreatedKey {
    pub blob: String,
    pub embedded: bool,
}

fn account_cfg(data_dir: &Path) -> Result<CloudConfig> {
    let cfg = CloudConfig::load(data_dir)
        .context("Sign in to PJDesk Cloud on this server, or create the key on pjdesk.com → Account → My network.")?;
    if !cfg.is_account() || cfg.token.is_empty() {
        bail!("Sign in to PJDesk Cloud on this server, or create the key on pjdesk.com → Account → My network.");
    }
    Ok(cfg)
}

fn cloud_api_error(err: ureq::Error) -> anyhow::Error {
    match err {
        ureq::Error::Status(_, resp) => {
            let text = resp.into_string().unwrap_or_default();
            let message = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()))
                .unwrap_or_else(|| "PJDesk could not update the network key.".into());
            anyhow::anyhow!(message)
        }
        ureq::Error::Transport(_) => anyhow::anyhow!("Could not reach pjdesk.com."),
    }
}

/// Creates a network key. The blob is returned once and is not written to disk.
/// Call this off the UI thread: a passphrase key runs Argon2id.
pub fn create_network_key(
    data_dir: &Path,
    passphrase: Option<&str>,
    days: Option<u64>,
    max_uses: Option<u32>,
    bound_email: &str,
) -> Result<CreatedKey> {
    let cfg = account_cfg(data_dir)?;
    #[derive(Deserialize)]
    struct Net {
        #[serde(default)]
        id: String,
        #[serde(default)]
        active_address: String,
        #[serde(default)]
        active_fingerprint: String,
        #[serde(default)]
        standby_address: String,
        #[serde(default)]
        standby_fingerprint: String,
    }
    #[derive(Deserialize)]
    struct Doc {
        #[serde(default)]
        network: Option<Net>,
    }
    let doc: Doc = match agent()
        .get(&format!("{}/api/network", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .call()
    {
        Ok(resp) => {
            serde_json::from_str(&resp.into_string().unwrap_or_default()).context("unexpected reply from PJDesk")?
        }
        Err(e) => return Err(cloud_api_error(e)),
    };
    let net = doc.network.context(
        "Your network has no server yet. Sign in to PJDesk Server on your server computer, or set it on pjdesk.com → Account → My network.",
    )?;
    if net.active_address.trim().is_empty() || net.active_fingerprint.trim().is_empty() {
        bail!(
            "Your network has no server yet. Sign in to PJDesk Server on your server computer, or set it on pjdesk.com → Account → My network."
        );
    }
    let secret = pjdesk_proto::random_bytes::<32>();
    let secret_hex = hex::encode(secret);
    let hash = pjdesk_proto::netkey::secret_hash(&secret_hex);
    let expires_at = days.map(|d| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_secs()).unwrap_or(0)
            + d.saturating_mul(86_400)
    });
    let body = serde_json::json!({
        "secret_hash": hash,
        "mode": if passphrase.is_some() { "passphrase" } else { "embedded" },
        "bound_email": bound_email.trim(),
        "max_uses": max_uses,
        "expires_at": expires_at,
    });
    match agent()
        .post(&format!("{}/api/network/invites", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set("content-type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(_) => {}
        Err(e) => return Err(cloud_api_error(e)),
    }
    let contents = pjdesk_proto::netkey::Contents {
        network_id: &net.id,
        active: &net.active_address,
        active_fingerprint: &net.active_fingerprint,
        standby: &net.standby_address,
        standby_fingerprint: &net.standby_fingerprint,
    };
    let embedded = passphrase.is_none();
    let blob = if let Some(pass) = passphrase {
        pjdesk_proto::netkey::seal_passphrase(&contents, &secret, pass).map_err(|e| anyhow::anyhow!(e.to_string()))?
    } else {
        pjdesk_proto::netkey::seal_embedded(&contents, &secret).map_err(|e| anyhow::anyhow!(e.to_string()))?
    };
    Ok(CreatedKey { blob, embedded })
}

pub fn list_network_keys(data_dir: &Path) -> Result<Vec<NetworkInvite>> {
    let cfg = account_cfg(data_dir)?;
    #[derive(Deserialize)]
    struct Doc {
        #[serde(default)]
        invites: Vec<NetworkInvite>,
    }
    match agent()
        .get(&format!("{}/api/network", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .call()
    {
        Ok(resp) => {
            let doc: Doc = serde_json::from_str(&resp.into_string().unwrap_or_default())
                .context("unexpected reply from PJDesk")?;
            Ok(doc.invites)
        }
        Err(e) => Err(cloud_api_error(e)),
    }
}

pub fn revoke_network_key(data_dir: &Path, id: &str) -> Result<()> {
    let cfg = account_cfg(data_dir)?;
    let body = serde_json::json!({ "id": id }).to_string();
    match agent()
        .post(&format!("{}/api/network/invites/revoke", cfg.api.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .set("content-type", "application/json")
        .send_string(&body)
    {
        Ok(_) => Ok(()),
        Err(e) => Err(cloud_api_error(e)),
    }
}

/// Other servers' health reports. Empty when sync is off or the website has
/// not been redeployed with the health route yet.
pub fn peer_health(data_dir: &Path) -> Vec<PeerHealth> {
    let Some(cfg) = CloudConfig::load(data_dir) else { return Vec::new() };
    match read_health(&cfg) {
        Ok(doc) => doc.peers.into_values().collect(),
        Err(e) => {
            tracing::debug!("cloud health: {e:#}");
            Vec::new()
        }
    }
}

fn write_health(cfg: &CloudConfig, me: &PeerHealth) -> Result<()> {
    if cfg.is_account() {
        let _ = put_object(cfg, HEALTH_OBJECT, &serde_json::to_vec(me)?, &Guard::None)?;
        return Ok(());
    }
    // Own bucket: one JSON document, updated with If-Match so two servers
    // don't erase each other's rows. One retry covers the usual race.
    for _ in 0..2 {
        let (mut doc, etag) = read_health_doc(cfg)?;
        let cutoff = now().saturating_sub(7 * 24 * 3600);
        doc.peers.retain(|_, p| p.at >= cutoff);
        doc.peers.insert(me.instance.clone(), me.clone());
        let body = serde_json::to_vec(&doc)?;
        let guard = match &etag {
            Some(tag) if !tag.is_empty() => Guard::IfMatch(tag.clone()),
            _ => Guard::IfAbsent,
        };
        match put_object(cfg, HEALTH_OBJECT, &body, &guard) {
            Ok(_) => return Ok(()),
            Err(e) if lost_race(&e) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn read_health(cfg: &CloudConfig) -> Result<HealthDoc> {
    if cfg.is_account() {
        let text = match agent()
            .get(&format!("{}/api/sync/health", cfg.api))
            .set("authorization", &format!("Bearer {}", cfg.token))
            .call()
        {
            Ok(resp) => resp.into_string().unwrap_or_default(),
            Err(ureq::Error::Status(404, _)) => return Ok(HealthDoc::default()),
            Err(e) => return Err(api_error(&cfg.api, &cfg.email, e)),
        };
        #[derive(Deserialize)]
        struct Reply {
            #[serde(default)]
            peers: Vec<PeerHealth>,
        }
        let reply: Reply = serde_json::from_str(&text).unwrap_or(Reply { peers: Vec::new() });
        let mut doc = HealthDoc::default();
        for peer in reply.peers {
            doc.peers.insert(peer.instance.clone(), peer);
        }
        return Ok(doc);
    }
    Ok(read_health_doc(cfg)?.0)
}

fn read_health_doc(cfg: &CloudConfig) -> Result<(HealthDoc, Option<String>)> {
    let (amz_date, payload_sha256, auth) = signed_for(cfg, HEALTH_OBJECT, "GET", b"", &[]);
    let path = format!("/{}/{}", uri_encode(&cfg.bucket), uri_encode(HEALTH_OBJECT));
    match agent()
        .get(&format!("{}{path}", cfg.endpoint))
        .set("x-amz-date", &amz_date)
        .set("x-amz-content-sha256", &payload_sha256)
        .set("Authorization", &auth)
        .call()
    {
        Ok(resp) => {
            let etag = resp.header("ETag").unwrap_or_default().to_string();
            let mut body = Vec::new();
            std::io::Read::read_to_end(&mut resp.into_reader().take(1 << 20), &mut body)?;
            let doc = serde_json::from_slice(&body).unwrap_or_default();
            Ok((doc, Some(etag).filter(|t| !t.is_empty())))
        }
        Err(ureq::Error::Status(404, _)) => Ok((HealthDoc::default(), None)),
        Err(e) => Err(connect_error(cfg, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standby_does_not_keep_the_live_table() {
        let dir = std::env::temp_dir().join(format!("pjdesk-role-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(accepts_host_registrations(&dir), "a server with no cloud sync is the primary");
        std::fs::write(dir.join(CONFIG_FILE), r#"{"mode":"account","key":"abcd","email":"ben@spader.us"}"#).unwrap();
        std::fs::write(
            dir.join(STATE_FILE),
            r#"{"desired":"standby","active_instance":"other","active_name":"BENLENOVODESK"}"#,
        )
        .unwrap();
        assert!(!accepts_host_registrations(&dir));
        std::fs::write(
            dir.join(STATE_FILE),
            r#"{"desired":"active","active_instance":"other","active_name":"BENLENOVODESK"}"#,
        )
        .unwrap();
        assert!(accepts_host_registrations(&dir));
        std::fs::write(
            dir.join(STATE_FILE),
            r#"{"desired":"active","epoch":5,"active_instance":"other","active_name":"CHIP20"}"#,
        )
        .unwrap();
        assert!(
            !accepts_host_registrations(&dir),
            "a recorded generation owned by another server is not this server's to keep"
        );
        std::fs::write(
            dir.join(STATE_FILE),
            r#"{"desired":"active","epoch":4,"fenced_epoch":5,"fenced_instance":"cccccccccccccccccccccccccccccccc","active_instance":"cccccccccccccccccccccccccccccccc"}"#,
        )
        .unwrap();
        assert!(
            !accepts_host_registrations(&dir),
            "a higher generation demotes a server that still wants to be primary"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restarted_primary_accepts_hosts_when_the_status_file_says_standby() {
        let dir = std::env::temp_dir().join(format!("pjdesk-role-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mine = "0123456789abcdef0123456789abcdef";
        std::fs::write(dir.join(CONFIG_FILE), r#"{"mode":"account","key":"abcd","email":"ben@spader.us"}"#).unwrap();
        std::fs::write(dir.join(INSTANCE_FILE), mine).unwrap();
        std::fs::write(dir.join(STATE_FILE), r#"{"desired":"","active_instance":"0123456789abcdef0123456789abcdef"}"#)
            .unwrap();
        std::fs::write(dir.join(STATUS_FILE), r#"{"role":"standby"}"#).unwrap();
        assert!(
            accepts_host_registrations(&dir),
            "the server that owns the cloud copy still accepts hosts after a restart"
        );
        // No owner recorded, and the status file only says standby because the
        // table was empty. That must not lock the only server out.
        std::fs::write(dir.join(STATE_FILE), r#"{"desired":""}"#).unwrap();
        assert!(accepts_host_registrations(&dir));
        std::fs::write(dir.join(STATE_FILE), r#"{"desired":"","active_instance":"fedcba9876543210fedcba9876543210"}"#)
            .unwrap();
        assert!(!accepts_host_registrations(&dir), "another instance owns the copy");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
    fn login_posts_auth_key_and_keeps_the_worker_error() {
        let body = login_body("ben@spader.us", &"ab".repeat(32), "PJDesk Server on BenLenovoDesk");
        assert!(body.get("password").is_none());
        assert_eq!(body["email"], "ben@spader.us");
        assert_eq!(body["auth_key"].as_str().unwrap().len(), 64);
        assert!(body["device"].as_str().unwrap().starts_with("PJDesk Server"));
        assert!(!body.to_string().contains("password"));
        assert_eq!(
            cloud_error_text(400, r#"{"error":"Enter your email and password."}"#),
            "Enter your email and password."
        );
        assert_eq!(
            cloud_error_text(401, r#"{"error":"Wrong email or password."}"#),
            "Wrong email or password."
        );
    }

    #[test]
    fn a_failed_read_does_not_clear_the_saved_sign_in() {
        let dir = std::env::temp_dir().join(format!("pjdesk-keep-account-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CONFIG_FILE), b"{not-json").unwrap();
        std::fs::write(dir.join(STATUS_FILE), br#"{"enabled":true,"account":"ben@spader.us"}"#).unwrap();
        assert!(!sync_once(&dir, Role::Standby, false));
        assert!(dir.join(CONFIG_FILE).exists());
        let status = std::fs::read_to_string(dir.join(STATUS_FILE)).unwrap();
        assert!(status.contains("ben@spader.us"), "{status}");
        assert_eq!(saved_account_email(&dir), "");
        std::fs::write(
            dir.join(CONFIG_FILE),
            r#"{"mode":"account","email":"ben@spader.us","token":"SECRETTOKENVALUE1234567890"}"#,
        )
        .unwrap();
        assert_eq!(saved_account_email(&dir), "ben@spader.us");
        let _ = std::fs::remove_file(dir.join(CONFIG_FILE));
        assert!(sync_once(&dir, Role::Standby, false) == false);
        assert!(!dir.join(STATUS_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upgrade_code_does_not_sign_the_server_out() {
        let handoff = include_str!("handoff.rs");
        let update = include_str!("self_update.rs");
        let service = include_str!("service.rs");
        for (name, src) in [("handoff", handoff), ("update", update), ("service", service)] {
            assert!(!src.contains("turn_off"), "{name} clears cloud sync");
            assert!(!src.contains("sign_out"), "{name} signs out");
        }
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
        assert_eq!(
            normalize_endpoint(id).unwrap().0,
            "https://0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com"
        );
        assert_eq!(
            normalize_endpoint("x.r2.cloudflarestorage.com/").unwrap(),
            ("https://x.r2.cloudflarestorage.com".into(), None)
        );
        assert!(normalize_endpoint(" ").is_err());
        assert!(normalize_endpoint("https://evil.example.com\r\nX: y").is_err());
        assert!(normalize_endpoint("https://user@host.example/bucket").is_err());
    }

    #[test]
    fn only_state_files_are_accepted() {
        assert!(allowed_path("accounts.json"));
        assert!(allowed_path("devices.json"));
        assert!(allowed_path("access-keys.json"));
        assert!(allowed_path("network-access.json"));
        assert!(allowed_path("addressbook/ben.json"));
        assert!(!allowed_path("addressbook/../server-key.pem"));
        assert!(!allowed_path("../evil.json"));
        assert!(!allowed_path("cloud-sync.json"));
        assert!(!allowed_path("addressbook/.json"));
    }

    #[test]
    fn decisions() {
        let mut st = SyncState {
            last_push_core: "c".into(),
            last_push_devices: "d".into(),
            last_push_at: 1000,
            ..Default::default()
        };
        assert!(!push_due(&st, "c", "d", 2000));
        assert!(push_due(&st, "c2", "d", 1001)); // accounts changed: right away
        assert!(!push_due(&st, "c", "d2", 1100)); // only the computer list: wait
        assert!(push_due(&st, "c", "d2", 1000 + DEVICES_PUSH_INTERVAL));
        // A standby takes another server's newer copy, never its own or an older one.
        assert!(timestamp_ok(now()));
        assert!(!timestamp_ok(now().saturating_add(MAX_CLOCK_SKEW + 120)));
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
        let id = pjdesk_proto::tls::TlsIdentity::load_or_create(
            &a.join("server-cert.pem"),
            &a.join("server-key.pem"),
            "pjdesk",
        )
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
        let copied =
            pjdesk_proto::tls::TlsIdentity::load(&b.join("server-cert.pem"), &b.join("server-key.pem")).unwrap();
        assert_eq!(copied.fingerprint(), id.fingerprint());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fencing_token_lets_one_server_write() {
        assert_eq!(fence(false, 0, "", "me", 0), Fence::MayUpload);
        assert_eq!(fence(true, 4, "me", "me", 4), Fence::MayUpload);
        assert_eq!(fence(true, 5, "other", "me", 4), Fence::Superseded);
        assert_eq!(fence(true, 4, "other", "me", 4), Fence::Held);
        // A pre-0.4 copy has no epoch. Either server may still upload.
        assert_eq!(fence(true, 0, "other", "me", 0), Fence::MayUpload);
        // We already hold a higher generation than the copy we just read.
        assert_eq!(fence(true, 3, "other", "me", 4), Fence::MayUpload);
        assert_eq!(next_epoch(0), 1);
        assert_eq!(next_epoch(7), 8);
        assert_eq!(next_epoch(u64::MAX), u64::MAX);
    }

    #[test]
    fn the_server_that_owns_the_cloud_copy_stays_active_with_no_computers_online() {
        let mine = "1b401af7870a921b0629820f04e82433";
        let st =
            SyncState { active_instance: mine.into(), active_name: "BENLENOVODESK".into(), ..SyncState::default() };
        assert_eq!(effective_role(&st, mine, Role::Standby), Role::Active);
        let other = SyncState { active_instance: mine.into(), ..SyncState::default() };
        assert_eq!(effective_role(&other, "someone-else", Role::Active), Role::Standby);
        let stepped = SyncState { desired: "standby".into(), active_instance: mine.into(), ..SyncState::default() };
        assert_eq!(effective_role(&stepped, mine, Role::Active), Role::Standby);
        let fresh = SyncState::default();
        assert_eq!(effective_role(&fresh, mine, Role::Standby), Role::Standby);
        assert_eq!(effective_role(&fresh, mine, Role::Active), Role::Active);
    }

    #[test]
    fn old_bundle_epoch_defaults_to_zero() {
        let json = r#"{"format":1,"written_at":1,"written_by":"a","instance":"abc","version":"0.3.7","files":{}}"#;
        let bundle: Bundle = serde_json::from_str(json).unwrap();
        assert_eq!(bundle.epoch, 0);
    }

    #[test]
    fn if_match_is_part_of_the_signature() {
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let with_match = authorization(
            "PUT",
            "/bucket/pjdesk-server.sync",
            &[
                ("host", "example.r2.cloudflarestorage.com"),
                ("if-match", "\"abc\""),
                ("x-amz-content-sha256", empty),
                ("x-amz-date", "20130524T000000Z"),
            ],
            empty,
            "20130524T000000Z",
            "auto",
            "AKID",
            "SECRET",
        );
        let without = authorization(
            "PUT",
            "/bucket/pjdesk-server.sync",
            &[
                ("host", "example.r2.cloudflarestorage.com"),
                ("x-amz-content-sha256", empty),
                ("x-amz-date", "20130524T000000Z"),
            ],
            empty,
            "20130524T000000Z",
            "auto",
            "AKID",
            "SECRET",
        );
        assert!(with_match.contains("SignedHeaders=host;if-match;x-amz-content-sha256;x-amz-date"));
        assert_ne!(with_match, without);
    }

    #[test]
    fn presence_online_is_the_live_set_not_last_seen() {
        let dir = std::env::temp_dir().join(format!("pjdesk-online-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("devices.json"),
            br#"{"devices":{"902488771":{"hostname":"CABINDESK","last_seen":1700000000}}}"#,
        )
        .unwrap();
        assert!(online_ids(&dir).unwrap().is_empty(), "a last_seen time is not presence");
        std::fs::write(dir.join("online.json"), br#"[{"id":"610978221","hostname":"CHIP20","version":"0.6.27"}]"#)
            .unwrap();
        let live = online_ids(&dir).unwrap();
        assert!(live.contains("610978221"));
        assert!(!live.contains("902488771"));
        #[cfg(unix)]
        {
            let path = dir.join("online.json");
            assert!(std::process::Command::new("touch")
                .arg("-d")
                .arg("1 hour ago")
                .arg(&path)
                .status()
                .unwrap()
                .success());
            assert!(online_ids(&dir).unwrap().is_empty(), "a stale online.json is not live presence");
        }
        assert!(online_ids_from(br#"[{"id":"902488771"}]"#).contains("902488771"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn presence_online_at_stamp_counts_when_mtime_is_old() {
        let dir = std::env::temp_dir().join(format!("pjdesk-online-at-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let at = now();
        let path = dir.join("online.json");
        std::fs::write(&path, format!(r#"[{{"id":"902488771","hostname":"CABINDESK","version":"0.6.28","at":{at}}}]"#))
            .unwrap();
        #[cfg(unix)]
        {
            assert!(std::process::Command::new("touch")
                .arg("-d")
                .arg("1 hour ago")
                .arg(&path)
                .status()
                .unwrap()
                .success());
            assert!(!online_mtime_age(&path).is_some_and(|age| age <= ONLINE_FRESH_SECS));
        }
        let live = online_ids(&dir).unwrap();
        assert!(live.contains("902488771"), "a recent at stamp keeps the computer online");
        std::fs::write(
            &path,
            format!(
                r#"[{{"id":"902488771","hostname":"CABINDESK","version":"0.6.28","at":{}}}]"#,
                at.saturating_sub(3600)
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            assert!(std::process::Command::new("touch")
                .arg("-d")
                .arg("1 hour ago")
                .arg(&path)
                .status()
                .unwrap()
                .success());
            assert!(online_ids(&dir).unwrap().is_empty(), "an old at stamp is not live presence");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_presence_counts_only_while_the_fetch_is_fresh() {
        let dir = std::env::temp_dir().join(format!("pjdesk-cloud-presence-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fresh = serde_json::json!({ "at": now(), "online": ["111111111", "222222222"] });
        std::fs::write(dir.join(PRESENCE_FILE), fresh.to_string()).unwrap();
        let live = cloud_presence_online(&dir);
        assert!(live.contains("111111111") && live.contains("222222222"));
        let stale = serde_json::json!({ "at": now().saturating_sub(PRESENCE_FRESH_SECS + 5), "online": ["111111111"] });
        std::fs::write(dir.join(PRESENCE_FILE), stale.to_string()).unwrap();
        assert!(cloud_presence_online(&dir).is_empty(), "a stale cloud list is not online");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
