//! Sign in to a PJDesk account and receive this account's private server.
//!
//! Key derivation (600,000 rounds of PBKDF2) is slow on purpose. Call these
//! functions from a background thread, never from the UI thread.
//!
//! The saved-computer list uploaded here is the nickname, the hostname, the ID,
//! and the operating system. Passwords travel separately, sealed, in the
//! password vault.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use pjdesk_proto::signal::AddressEntry;
use serde::Deserialize;

use crate::config::{Config, PeerNote};

const DEFAULT_API: &str = "https://pjdesk.com";

pub const NO_SERVER: &str =
    "Your network has no server yet. Sign in to PJDesk Server on your server computer, or set it on pjdesk.com → Account → My network.";

fn is_old_no_server(message: &str) -> bool {
    message.contains("Set the server address in My network first")
        || message.contains("Set the server address on pjdesk.com")
        || message.contains("Set the server address and its fingerprint")
}

/// The owner of an empty network can publish the server this computer already uses.
pub fn offer_current_server(role: &str, server: &str, fingerprint: &str) -> bool {
    (role == "owner" || role.is_empty())
        && !server.trim().is_empty()
        && pjdesk_proto::tls::parse_fingerprint(fingerprint).is_ok()
}

/// Account sign-in finished, or the account's network has no server yet.
pub enum SignIn {
    Ready(Session),
    NoServer { role: String, login_token: String },
    /// This computer was added to the account and the sign-in was dropped.
    Added { email: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SavedComputer {
    pub id: String,
    /// Optional nickname. Empty when the user has not named it.
    pub name: String,
    /// Hostname the host reported. Empty until a computer has been seen.
    pub hostname: String,
    pub os_family: String,
    pub os_version: String,
    /// Installer or Settings name. Empty when this computer uses its hostname.
    pub display_name: String,
    /// Account star, when the account sent one. `None` leaves the local star.
    pub starred: Option<bool>,
    /// Account order, when the account sent one. `None` leaves the local order.
    pub position: Option<u32>,
    /// The account says this computer was removed from Saved.
    pub hidden: bool,
    /// Saved-list group. "Clients" is a support-key computer.
    pub group: String,
}

/// A computer registered on the account's server. The user name is present only
/// for the signed-in owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPresence {
    pub id: String,
    pub hostname: String,
    pub os_family: String,
    pub os_version: String,
    pub console_user: String,
    /// None when an older account service omitted the field. Some("") clears it.
    pub display_name: Option<String>,
    pub last_seen: u64,
    pub online: bool,
    /// PJDesk version that computer last registered. Empty when the account has not reported one.
    pub app_version: String,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub email: String,
    pub access: String,
    pub exp: u64,
    pub refresh: String,
    pub device_id: String,
    pub address: String,
    pub fingerprint: String,
    pub standby_address: String,
    pub standby_fingerprint: String,
    pub lan: Vec<String>,
    pub public_ip: String,
    pub computers: Vec<SavedComputer>,
    pub presence: Vec<HostPresence>,
    /// PJDesk IDs deleted from the account. Empty when the service is older.
    pub removed: Vec<String>,
    /// Present only when this sign-in had the account password. Not serialized.
    pub vault_key: Option<[u8; 32]>,
    /// `owner` or `member` when sign-in could read it. Empty leaves the saved role.
    pub role: String,
    /// Set when this sign-in redeemed a support key.
    pub support: Option<SupportInfo>,
}

/// What the supported computer shows, and how its host admits Ben.
#[derive(Debug, Clone)]
pub struct SupportInfo {
    pub owner: String,
    pub label: String,
    pub key_id: String,
    pub unattended: bool,
}

/// A hostname is kept when a later update only has the raw IP it points at.
/// The account primary uses [`pjdesk_proto::failover::follow_primary`] instead,
/// so a different server's address replaces the saved name.
#[allow(dead_code)]
pub fn keep_hostname(current: &str, incoming: &str) -> String {
    let incoming = incoming.trim();
    if incoming.is_empty() {
        return current.trim().to_string();
    }
    if is_hostname_address(current) && !is_hostname_address(incoming) {
        return current.trim().to_string();
    }
    incoming.to_string()
}

#[allow(dead_code)]
pub fn is_hostname_address(address: &str) -> bool {
    let text = address.trim();
    if text.is_empty() {
        return false;
    }
    let host = if let Some(rest) = text.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else if let Some((host, port)) = text.rsplit_once(':') {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) { host } else { text }
    } else {
        text
    };
    !host.is_empty() && host.parse::<std::net::IpAddr>().is_err()
}

#[derive(Deserialize)]
struct LoginReply {
    token: String,
}

#[derive(Deserialize)]
struct NetworkReply {
    #[serde(default)]
    access: String,
    #[serde(default)]
    exp: u64,
    #[serde(default)]
    refresh: String,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    network: Option<NetworkBody>,
    #[serde(default)]
    computers: Vec<ComputerBody>,
    #[serde(default)]
    presence: Vec<PresenceBody>,
    #[serde(default)]
    removed: Vec<String>,
    #[serde(default)]
    support: Option<SupportBody>,
}

#[derive(Deserialize)]
struct NetworkBody {
    #[serde(default)]
    active_address: String,
    #[serde(default)]
    active_fingerprint: String,
    #[serde(default)]
    standby_address: String,
    #[serde(default)]
    standby_fingerprint: String,
    #[serde(default)]
    active_lan: String,
    #[serde(default)]
    active_public_ip: String,
}

#[derive(Deserialize)]
struct ComputerBody {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    os_family: String,
    #[serde(default)]
    os_version: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    starred: Option<bool>,
    #[serde(default)]
    position: Option<u32>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    group: String,
}

#[derive(Deserialize, Default)]
struct SupportBody {
    #[serde(default)]
    owner: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    unattended: bool,
    #[serde(default)]
    key_id: String,
}

#[derive(Deserialize)]
struct PresenceBody {
    #[serde(default)]
    id: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    os_family: String,
    #[serde(default)]
    os_version: String,
    #[serde(default)]
    console_user: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    last_seen: u64,
    #[serde(default)]
    online: bool,
    #[serde(default)]
    app_version: String,
}

#[derive(Deserialize, Default)]
struct ErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    label: String,
}

#[derive(Clone)]
pub(crate) struct ApiFail {
    pub status: u16,
    pub message: String,
    pub code: String,
    pub role: String,
    pub owner: String,
    pub label: String,
}

/// A failed token refresh. `revoked` is an explicit 401 or a revoked key.
/// Anything else is transient and must not sign the computer out.
#[derive(Debug)]
pub struct RefreshError {
    pub revoked: bool,
    pub message: String,
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RefreshError {}

pub fn refresh_failure_revokes(status: u16, message: &str) -> bool {
    status == 401
        || message.to_ascii_lowercase().contains("revoked")
        || message.contains("Sign in required")
}

pub(crate) fn api_base() -> String {
    std::env::var("PJDESK_CLOUD_API")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_API.into())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(25)).build()
}

/// POST /api/login body. The website expects `auth_key` (hex), not `password`.
pub(crate) fn account_login_body(email: &str, auth_key_hex: &str, device: &str) -> serde_json::Value {
    serde_json::json!({
        "email": email,
        "auth_key": auth_key_hex,
        "device": device,
    })
}

fn post_json(url: &str, bearer: Option<&str>, body: &serde_json::Value) -> Result<String> {
    post_api(url, bearer, body).map_err(|fail| anyhow::anyhow!(fail.message))
}

fn post_api(url: &str, bearer: Option<&str>, body: &serde_json::Value) -> std::result::Result<String, ApiFail> {
    let mut req = agent().post(url).set("content-type", "application/json");
    if let Some(token) = bearer {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    match req.send_string(&body.to_string()) {
        Ok(resp) => Ok(resp.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            let parsed: ErrorBody = serde_json::from_str(&text).unwrap_or_default();
            let message = if !parsed.error.is_empty() {
                parsed.error
            } else if code == 401 {
                "Wrong email or password.".into()
            } else {
                format!("PJDesk could not sign in ({code}).")
            };
            Err(ApiFail {
                status: code,
                message,
                code: parsed.code,
                role: parsed.role,
                owner: parsed.owner,
                label: parsed.label,
            })
        }
        Err(ureq::Error::Transport(_)) => Err(ApiFail {
            status: 0,
            message: "Could not reach pjdesk.com.".into(),
            code: String::new(),
            role: String::new(),
            owner: String::new(),
            label: String::new(),
        }),
    }
}

pub fn fetch_password_vault(refresh: &str, device_id: &str) -> Result<String> {
    let text = post_json(
        &format!("{}/api/network/passwords", api_base()),
        None,
        &serde_json::json!({ "refresh": refresh, "device_id": device_id }),
    )?;
    let parsed: VaultReply = serde_json::from_str(&text).context("the account sent an unexpected password vault")?;
    Ok(parsed.vault)
}

pub fn fetch_app_lock(refresh: &str, device_id: &str) -> Result<(String, u64)> {
    let text = post_json(
        &format!("{}/api/network/app-lock", api_base()),
        None,
        &serde_json::json!({ "refresh": refresh, "device_id": device_id }),
    )?;
    let parsed: LockReply = serde_json::from_str(&text).context("the account sent an unexpected app lock")?;
    Ok((parsed.lock, parsed.updated_at))
}

pub fn put_app_lock(refresh: &str, device_id: &str, lock: &str) -> Result<u64> {
    let text = post_json(
        &format!("{}/api/network/app-lock", api_base()),
        None,
        &serde_json::json!({ "refresh": refresh, "device_id": device_id, "lock": lock }),
    )?;
    let parsed: LockReply = serde_json::from_str(&text).unwrap_or(LockReply { lock: String::new(), updated_at: 0 });
    Ok(parsed.updated_at.max(1))
}

/// Checks the account password and drops the short session that check creates.
/// The computer's saved sign-in is left as it was.
pub fn confirm_account_password(email: &str, password: &str) -> Result<[u8; 32]> {
    let email = email.trim().to_lowercase();
    if email.is_empty() || password.is_empty() {
        bail!("Enter your account password.");
    }
    let (auth, _sync) = pjdesk_proto::access::derive_account_keys(&email, password);
    let login: LoginReply = serde_json::from_str(&post_json(
        &format!("{}/api/login", api_base()),
        None,
        &account_login_body(&email, &hex::encode(auth), "PJDesk PIN check"),
    )?)
    .context("unexpected reply from PJDesk")?;
    let _ = post_json(&format!("{}/api/logout", api_base()), Some(&login.token), &serde_json::json!({}));
    Ok(pjdesk_proto::access::derive_password_vault_key(&email, password))
}

pub fn put_password_vault(refresh: &str, device_id: &str, vault: &str) -> Result<()> {
    post_json(
        &format!("{}/api/network/passwords", api_base()),
        None,
        &serde_json::json!({ "refresh": refresh, "device_id": device_id, "vault": vault }),
    )?;
    Ok(())
}

#[derive(Deserialize)]
struct VaultReply {
    #[serde(default)]
    vault: String,
}

#[derive(Deserialize)]
struct LockReply {
    #[serde(default)]
    lock: String,
    #[serde(default)]
    updated_at: u64,
}

fn session_from(email: &str, device_id: &str, reply: NetworkReply) -> Result<Session> {
    let network = reply.network.context("the account has no network")?;
    if network.active_address.trim().is_empty() {
        bail!(NO_SERVER);
    }
    Ok(Session {
        email: email.to_string(),
        access: reply.access,
        exp: reply.exp,
        refresh: reply.refresh,
        device_id: if reply.device_id.is_empty() { device_id.to_string() } else { reply.device_id },
        address: network.active_address,
        fingerprint: network.active_fingerprint,
        standby_address: network.standby_address,
        standby_fingerprint: network.standby_fingerprint,
        lan: network.active_lan.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        public_ip: network.active_public_ip,
        computers: reply
            .computers
            .into_iter()
            .map(|c| SavedComputer {
                id: c.id,
                name: c.name,
                hostname: c.hostname,
                os_family: c.os_family,
                os_version: c.os_version,
                display_name: c.display_name,
                starred: c.starred,
                position: c.position,
                hidden: c.hidden,
                group: if c.group == crate::support_keys::CLIENTS { crate::support_keys::CLIENTS.into() } else { String::new() },
            })
            .collect(),
        presence: reply
            .presence
            .into_iter()
            .map(|c| HostPresence {
                id: c.id,
                hostname: c.hostname,
                os_family: c.os_family,
                os_version: c.os_version,
                console_user: c.console_user,
                display_name: c.display_name,
                last_seen: c.last_seen,
                online: c.online,
                app_version: c.app_version,
            })
            .collect(),
        removed: reply.removed.into_iter().map(|id| pjdesk_proto::normalize_id(&id)).filter(|id| pjdesk_proto::is_valid_id(id)).collect(),
        vault_key: None,
        role: if reply.support.as_ref().is_some_and(|item| !item.owner.trim().is_empty() || !item.key_id.trim().is_empty()) {
            "support".into()
        } else {
            String::new()
        },
        support: reply.support.and_then(|item| {
            if item.owner.trim().is_empty() && item.key_id.trim().is_empty() {
                None
            } else {
                Some(SupportInfo {
                    owner: item.owner,
                    label: item.label,
                    key_id: item.key_id,
                    unattended: item.unattended,
                })
            }
        }),
    })
}

/// Signs in with the PJDesk account password and fetches the private server.
/// An empty network returns [`SignIn::NoServer`] so the owner can publish the
/// server this computer already uses. The login token stays in memory only.
pub fn sign_in(email: &str, password: &str, device_id: &str, device_name: &str) -> Result<SignIn> {
    let email = email.trim().to_lowercase();
    if email.is_empty() || password.is_empty() {
        bail!("Enter your email and password.");
    }
    let (auth, _sync) = pjdesk_proto::access::derive_account_keys(&email, password);
    let login: LoginReply = serde_json::from_str(&post_json(
        &format!("{}/api/login", api_base()),
        None,
        &account_login_body(&email, &hex::encode(auth), &format!("PJDesk on {device_name}")),
    )?)
    .context("unexpected reply from PJDesk")?;
    match post_api(
        &format!("{}/api/network/token", api_base()),
        Some(&login.token),
        &serde_json::json!({ "device_id": device_id, "device_name": device_name }),
    ) {
        Ok(text) => {
            let reply: NetworkReply = serde_json::from_str(&text).context("unexpected reply from PJDesk")?;
            match session_from(&email, device_id, reply) {
                Ok(mut session) => {
                    session.role = account_role(&login.token);
                    session.vault_key = Some(pjdesk_proto::access::derive_password_vault_key(&email, password));
                    Ok(SignIn::Ready(session))
                }
                Err(err) if err.to_string() == NO_SERVER => {
                    Ok(SignIn::NoServer { role: "owner".into(), login_token: login.token })
                }
                Err(err) => Err(err),
            }
        }
        Err(fail) if fail.code == "no_server" || is_old_no_server(&fail.message) => {
            let role = if fail.role.is_empty() { "owner".into() } else { fail.role };
            Ok(SignIn::NoServer { role, login_token: login.token })
        }
        Err(fail) => bail!(fail.message),
    }
}

/// Publishes this computer's current server for an empty network, then signs in.
/// The login token is the one from the sign-in that found no server.
pub fn claim_current_server(login_token: &str, address: &str, fingerprint: &str, device_id: &str, device_name: &str) -> Result<Session> {
    if login_token.is_empty() || address.trim().is_empty() || pjdesk_proto::tls::parse_fingerprint(fingerprint).is_err() {
        bail!(NO_SERVER);
    }
    post_json(
        &format!("{}/api/network/register", api_base()),
        Some(login_token),
        &serde_json::json!({
            "role": "active",
            "address": address.trim(),
            "fingerprint": fingerprint,
            "confirm": true,
            "replace": true,
        }),
    )?;
    let text = post_json(
        &format!("{}/api/network/token", api_base()),
        Some(login_token),
        &serde_json::json!({ "device_id": device_id, "device_name": device_name }),
    )?;
    let reply: NetworkReply = serde_json::from_str(&text).context("unexpected reply from PJDesk")?;
    session_from("", device_id, reply)
}

/// Exchanges a still-valid refresh token for a new access token.
pub fn refresh(refresh_token: &str, device_id: &str, computers: Option<&serde_json::Value>) -> Result<Session> {
    refresh_account(refresh_token, device_id, computers, "", false)
}

fn refresh_account(
    refresh_token: &str,
    device_id: &str,
    computers: Option<&serde_json::Value>,
    pjdesk_id: &str,
    reinstate: bool,
) -> Result<Session> {
    let mut body = serde_json::json!({ "refresh": refresh_token, "device_id": device_id });
    let pjdesk_id = pjdesk_proto::normalize_id(pjdesk_id);
    if pjdesk_proto::is_valid_id(&pjdesk_id) {
        body["pjdesk_id"] = serde_json::Value::String(pjdesk_id);
    }
    if reinstate {
        body["reinstate"] = serde_json::Value::Bool(true);
    }
    if let Some(computers) = computers {
        if let Some(list) = computers.get("computers") {
            body["computers"] = list.clone();
        }
    }
    let text = match post_api(&format!("{}/api/network/refresh", api_base()), None, &body) {
        Ok(text) => text,
        Err(fail) => {
            return Err(RefreshError {
                revoked: refresh_failure_revokes(fail.status, &fail.message),
                message: fail.message,
            }
            .into());
        }
    };
    let reply: NetworkReply = serde_json::from_str(&text).context("unexpected reply from PJDesk")?;
    let email = String::new();
    session_from(&email, device_id, reply)
}

/// Downloads the account list, including computers deleted since the last sync.
/// This computer's id is recorded so a later delete can sign it out. The local
/// list is not uploaded.
pub fn pull_list(refresh_token: &str, device_id: &str, pjdesk_id: &str) -> Result<Session> {
    refresh_account(refresh_token, device_id, None, pjdesk_id, false)
}

/// Deletes one computer from the signed-in owner's account.
pub fn delete_from_account(refresh_token: &str, device_id: &str, pjdesk_id: &str) -> Result<()> {
    let id = pjdesk_proto::normalize_id(pjdesk_id);
    if !pjdesk_proto::is_valid_id(&id) {
        bail!("That is not a PJDesk ID.");
    }
    let body = serde_json::json!({ "refresh": refresh_token, "device_id": device_id, "id": id });
    let text = match post_api(&format!("{}/api/network/computers/delete", api_base()), None, &body) {
        Ok(text) => text,
        Err(fail) => {
            if fail.status == 404 || fail.status == 503 {
                bail!("Could not delete that computer yet. On your PC, in cloud\\, run cloud\\deploy.cmd, then try again.");
            }
            bail!(fail.message);
        }
    };
    let reply: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if reply.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        bail!("Could not delete that computer.");
    }
    Ok(())
}

/// Opens a network key and enrolls this device. No PJDesk password is required
/// unless the key is limited to one account, in which case `account` is that
/// account's email and password.
pub fn redeem(blob: &str, passphrase: &str, device_id: &str, device_name: &str, account: Option<(&str, &str)>) -> Result<Session> {
    let opened = pjdesk_proto::netkey::open(blob, passphrase).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let bearer = match account {
        Some((email, password)) if !email.trim().is_empty() => Some(login_token(email, password)?),
        _ => None,
    };
    let text = post_json(
        &format!("{}/api/network/redeem", api_base()),
        bearer.as_deref(),
        &serde_json::json!({
            "secret": opened.secret_hex,
            "device_id": device_id,
            "device_name": device_name,
        }),
    )?;
    let reply: NetworkReply = serde_json::from_str(&text).context("unexpected reply from PJDesk")?;
    let mut session = session_from("", device_id, reply)?;
    if session.address.is_empty() {
        session.address = opened.active;
    }
    if session.fingerprint.is_empty() {
        session.fingerprint = opened.active_fingerprint;
    }
    if session.standby_address.is_empty() {
        session.standby_address = opened.standby;
    }
    if session.standby_fingerprint.is_empty() {
        session.standby_fingerprint = opened.standby_fingerprint;
    }
    if let Some((email, password)) = account {
        if !email.trim().is_empty() && !password.is_empty() {
            session.vault_key = Some(pjdesk_proto::access::derive_password_vault_key(email, password));
            if session.email.is_empty() {
                session.email = email.trim().to_lowercase();
            }
        }
    }
    Ok(session)
}

fn login_token(email: &str, password: &str) -> Result<String> {
    let email = email.trim().to_lowercase();
    if email.is_empty() || password.is_empty() {
        bail!("Enter the email and password for the account this network key is for.");
    }
    let (auth, _sync) = pjdesk_proto::access::derive_account_keys(&email, password);
    let login: LoginReply = serde_json::from_str(&post_json(
        &format!("{}/api/login", api_base()),
        None,
        &account_login_body(&email, &hex::encode(auth), "PJDesk"),
    )?)
    .context("unexpected reply from PJDesk")?;
    Ok(login.token)
}

/// Enrolls this device with an 8-digit invite or a support key. No account password is typed.
/// `consent` is the box the client ticks for an unattended support key.
pub fn enroll(code: &str, device_id: &str, device_name: &str, pjdesk_id: &str, consent: bool) -> Result<Session> {
    let body = serde_json::json!({
        "code": code,
        "device_id": device_id,
        "device_name": device_name,
        "pjdesk_id": pjdesk_id,
        "consent": consent,
    });
    let text = match post_api(&format!("{}/api/network/enroll", api_base()), None, &body) {
        Ok(text) => text,
        Err(fail) => {
            if let Some((owner, label)) = crate::support_keys::consent_from_fail(&fail) {
                bail!(crate::support_keys::consent_wire(&owner, &label));
            }
            bail!(fail.message);
        }
    };
    let reply: NetworkReply = serde_json::from_str(&text).context("unexpected reply from PJDesk")?;
    session_from("", device_id, reply)
}

/// This computer, as it should appear in the account's Saved list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThisComputer {
    pub id: String,
    pub hostname: String,
    pub display_name: String,
    pub os_family: String,
    pub os_version: String,
}

impl ThisComputer {
    /// Facts from this install. An empty device id cannot be enrolled.
    pub fn local(device_id: &str) -> Self {
        let (os_family, os_version) = crate::hostfacts::os_pair();
        let hostname = crate::hostname();
        let display = crate::hostfacts::read_computer_name();
        let display_name = if display.eq_ignore_ascii_case(hostname.trim()) { String::new() } else { display };
        Self { id: device_id.to_string(), hostname, display_name, os_family, os_version }
    }
}

/// Adds this computer, or refreshes its hostname, display name, and OS.
/// A nickname already on the account is kept. Signing in on a computer that
/// was removed puts it back, because this sign-in is that computer.
pub fn enroll_this_computer(computers: &[SavedComputer], this: &ThisComputer) -> Vec<SavedComputer> {
    let id = pjdesk_proto::normalize_id(&this.id);
    if !pjdesk_proto::is_valid_id(&id) {
        return computers.to_vec();
    }
    let mut out = computers.to_vec();
    if let Some(row) = out.iter_mut().find(|c| pjdesk_proto::normalize_id(&c.id) == id) {
        row.id = id;
        row.hidden = false;
        if !this.hostname.trim().is_empty() {
            row.hostname = this.hostname.trim().to_string();
        }
        if !this.os_family.trim().is_empty() {
            row.os_family = this.os_family.trim().to_string();
        }
        if !this.os_version.trim().is_empty() {
            row.os_version = this.os_version.trim().to_string();
        }
        if !this.display_name.trim().is_empty() {
            row.display_name = this.display_name.trim().to_string();
        }
    } else {
        out.push(SavedComputer {
            id,
            name: String::new(),
            hostname: this.hostname.trim().to_string(),
            os_family: this.os_family.trim().to_string(),
            os_version: this.os_version.trim().to_string(),
            display_name: this.display_name.trim().to_string(),
            ..SavedComputer::default()
        });
    }
    out
}

fn saved_value(computers: &[SavedComputer]) -> serde_json::Value {
    let list: Vec<serde_json::Value> = computers
        .iter()
        .filter(|c| pjdesk_proto::is_valid_id(&pjdesk_proto::normalize_id(&c.id)))
        .map(|c| {
            serde_json::json!({
                "id": pjdesk_proto::normalize_id(&c.id),
                "name": c.name,
                "hostname": c.hostname,
                "os_family": c.os_family,
                "os_version": c.os_version,
                "display_name": c.display_name,
                "starred": c.starred.unwrap_or(false),
                "position": c.position.unwrap_or(0),
                "hidden": c.hidden,
                "group": if c.group == crate::support_keys::CLIENTS { crate::support_keys::CLIENTS } else { "" },
            })
        })
        .collect();
    serde_json::json!({ "computers": list })
}

/// Uploads this computer onto the account, then returns the account's list.
/// `local` is this computer's saved list when the sign-in should keep syncing it.
/// An empty `local` uploads only the account's current rows plus this computer,
/// so a friend's list is not added to the account.
pub fn publish_this_computer(session: &Session, this: &ThisComputer, local: &[SavedComputer]) -> Result<Session> {
    if session.refresh.is_empty() {
        bail!("The account did not grant a sign-in for this computer.");
    }
    let mut list = if local.is_empty() { session.computers.clone() } else { local.to_vec() };
    if let Some(account) = session.computers.iter().find(|c| pjdesk_proto::normalize_id(&c.id) == pjdesk_proto::normalize_id(&this.id))
    {
        if let Some(row) = list.iter_mut().find(|c| pjdesk_proto::normalize_id(&c.id) == pjdesk_proto::normalize_id(&this.id)) {
            if row.name.trim().is_empty() {
                row.name = account.name.clone();
            }
        }
    }
    let mut list = enroll_this_computer(&list, this);
    if let Some(info) = &session.support {
        ensure_support_computer(&mut list, &this.id, &info.label);
    }
    let body = saved_value(&list);
    let mut next = refresh_account(&session.refresh, &session.device_id, Some(&body), &this.id, true)?;
    if next.email.is_empty() {
        next.email = session.email.clone();
    }
    if next.address.trim().is_empty() {
        next.address = session.address.clone();
        next.fingerprint = session.fingerprint.clone();
    }
    next.vault_key = session.vault_key;
    next.support = session.support.clone();
    Ok(next)
}

/// Puts this computer on the account under the support key's label, in Clients.
pub fn ensure_support_computer(computers: &mut Vec<SavedComputer>, id: &str, label: &str) {
    let id = pjdesk_proto::normalize_id(id);
    if !pjdesk_proto::is_valid_id(&id) {
        return;
    }
    let label = label.trim();
    if let Some(row) = computers.iter_mut().find(|c| pjdesk_proto::normalize_id(&c.id) == id) {
        if row.name.trim().is_empty() && !label.is_empty() {
            row.name = label.to_string();
        }
        row.group = crate::support_keys::CLIENTS.into();
        row.hidden = false;
        return;
    }
    computers.push(SavedComputer {
        id,
        name: label.to_string(),
        group: crate::support_keys::CLIENTS.into(),
        ..SavedComputer::default()
    });
}

/// Enrolls this computer on a sign-in that already has a refresh token.
/// `local` is uploaded only for a sign-in that stays on this computer.
/// An empty `local` sends the account's rows plus this computer.
pub fn enroll_known(
    refresh: &str,
    network_device: &str,
    account: &[SavedComputer],
    local: &[SavedComputer],
    this: &ThisComputer,
) -> Result<()> {
    let session = Session {
        email: String::new(),
        access: String::new(),
        exp: 0,
        refresh: refresh.to_string(),
        device_id: network_device.to_string(),
        address: String::new(),
        fingerprint: String::new(),
        standby_address: String::new(),
        standby_fingerprint: String::new(),
        lan: Vec::new(),
        public_ip: String::new(),
        computers: account.to_vec(),
        presence: Vec::new(),
        removed: Vec::new(),
        vault_key: None,
        role: String::new(),
        support: None,
    };
    let _ = publish_this_computer(&session, this, local)?;
    Ok(())
}

/// Enrolls this computer, then either keeps the session or drops it.
pub fn finish_account_sign_in(session: Session, this: &ThisComputer, local: &[SavedComputer], enroll_only: bool) -> Result<SignIn> {
    let email = session.email.clone();
    let session = publish_this_computer(&session, this, local)?;
    if enroll_only {
        Ok(SignIn::Added { email: if session.email.is_empty() { email } else { session.email } })
    } else {
        Ok(SignIn::Ready(session))
    }
}

/// The saved list this computer would upload, including nicknames and hide flags.
pub fn saved_from_config(config: &Config) -> Vec<SavedComputer> {
    let body = computers_body(&config.address_book, &config.settings.peers, &config.saved_hidden);
    let Some(list) = body.get("computers").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|row| {
            let id = row.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if !pjdesk_proto::is_valid_id(id) {
                return None;
            }
            Some(SavedComputer {
                id: id.to_string(),
                name: row.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                hostname: row.get("hostname").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                os_family: row.get("os_family").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                os_version: row.get("os_version").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                display_name: row.get("display_name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                starred: Some(row.get("starred").and_then(|v| v.as_bool()).unwrap_or(false)),
                position: Some(row.get("position").and_then(|v| v.as_u64()).unwrap_or(0) as u32),
                hidden: row.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false),
                group: if row.get("group").and_then(|v| v.as_str()) == Some(crate::support_keys::CLIENTS) {
                    crate::support_keys::CLIENTS.into()
                } else {
                    String::new()
                },
            })
        })
        .collect()
}

/// Nickname, hostname, ID, and OS. A password is dropped. An alias that is
/// only the ID, or only the hostname, is not uploaded as a nickname.
pub fn computers_body(
    entries: &[AddressEntry],
    notes: &std::collections::BTreeMap<String, PeerNote>,
    hidden: &[String],
) -> serde_json::Value {
    use std::collections::BTreeSet;
    let mut ids = BTreeSet::new();
    for entry in entries {
        if pjdesk_proto::is_valid_id(&entry.id) {
            ids.insert(entry.id.clone());
        }
    }
    for id in notes.keys() {
        if pjdesk_proto::is_valid_id(id) && !hidden.iter().any(|h| h == id) {
            ids.insert(id.clone());
        }
    }
    let computers: Vec<serde_json::Value> = ids
        .into_iter()
        .map(|id| {
            if hidden.iter().any(|h| h == &id) {
                return serde_json::json!({ "id": id, "name": "", "hostname": "", "os_family": "", "os_version": "", "hidden": true, "group": "" });
            }
            let entry = entries.iter().find(|e| e.id == id);
            let alias = entry.map(|e| e.alias.as_str()).unwrap_or("");
            let group = if entry.is_some_and(|e| crate::support_keys::is_clients(&e.tags)) { crate::support_keys::CLIENTS } else { "" };
            let note = notes.get(&id);
            let hostname = note.map(|n| n.hostname.as_str()).unwrap_or("");
            serde_json::json!({
                "id": id,
                "name": crate::config::nickname_text(alias, &id, hostname),
                "hostname": hostname,
                "os_family": note.map(|n| n.os_family.as_str()).unwrap_or(""),
                "os_version": note.map(|n| n.os_version.as_str()).unwrap_or(""),
                "display_name": note.map(|n| n.display_name.as_str()).unwrap_or(""),
                "starred": note.is_some_and(|n| n.favorite),
                "position": note.map(|n| n.sort_order).unwrap_or(0),
                "hidden": false,
                "group": group,
            })
        })
        .collect();
    serde_json::json!({ "computers": computers })
}

/// Uploads the saved list on this computer's refresh token. A failure leaves
/// the local list as it is. The passphrase and computer passwords are not sent.
pub fn push_saved_computers_from_disk() -> Result<()> {
    let mut config = Config::load().unwrap_or_default();
    let refresh_token = config.network_refresh_token();
    let device = config.network_device_id.clone().unwrap_or_default();
    if refresh_token.is_empty() || device.is_empty() {
        return Ok(());
    }
    let body = computers_body(&config.address_book, &config.settings.peers, &config.saved_hidden);
    let mut session = refresh(&refresh_token, &device, Some(&body))?;
    if let Ok(disk) = Config::load() {
        config.fill_peer_facts_from(&disk);
        if session.email.is_empty() {
            session.email = disk.network_email.clone().unwrap_or_default();
        }
    }
    config.apply_network_session(&session);
    if let Ok(disk) = Config::load() {
        config.fill_peer_facts_from(&disk);
    }
    config.save()
}

/// Refreshes the access token when it is close to expiring. Safe to call from
/// the host thread. A failure leaves the current token in place.
pub fn refresh_config(config: &mut Config) {
    refresh_config_inner(config, false);
}

/// Refreshes even when the access token is still fresh, so a reconnect picks
/// up a new server address, LAN address, or public IP.
pub fn refresh_config_now(config: &mut Config) {
    refresh_config_inner(config, true);
}

fn refresh_config_inner(config: &mut Config, force: bool) {
    let refresh_token = config.network_refresh_token();
    let device = config.network_device_id.clone().unwrap_or_default();
    if refresh_token.is_empty() || device.is_empty() {
        return;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if !force && config.network_access_exp > now + 120 && !config.network_access_token().is_empty() {
        return;
    }
    if !force && now < NEXT_REFRESH_AT.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let body = computers_body(&config.address_book, &config.settings.peers, &config.saved_hidden);
    let pjdesk_id = config.device_id.clone().unwrap_or_default();
    match refresh_account(&refresh_token, &device, Some(&body), &pjdesk_id, false) {
        Ok(mut session) => {
            REFRESH_BACKOFF_SECS.store(15, std::sync::atomic::Ordering::Relaxed);
            NEXT_REFRESH_AT.store(0, std::sync::atomic::Ordering::Relaxed);
            if session.email.is_empty() {
                session.email = config.network_email.clone().unwrap_or_default();
            }
            config.apply_network_session(&session);
            let _ = config.save();
            if let Some(key) = crate::vault::key_from(config) {
                let local = crate::vault::local_entries(config);
                let refresh = config.network_refresh_token();
                let device = config.network_device_id.clone().unwrap_or_default();
                crate::vault::schedule(refresh, device, key, local, !config.ephemeral_network);
            }
        }
        Err(err) => {
            let revoked = err.downcast_ref::<RefreshError>().is_some_and(|e| e.revoked);
            if revoked {
                config.leave_network_sign_in();
                let _ = config.save();
                NEXT_REFRESH_AT.store(0, std::sync::atomic::Ordering::Relaxed);
            } else {
                let wait = REFRESH_BACKOFF_SECS.load(std::sync::atomic::Ordering::Relaxed).clamp(15, 300);
                NEXT_REFRESH_AT.store(now.saturating_add(wait), std::sync::atomic::Ordering::Relaxed);
                REFRESH_BACKOFF_SECS.store((wait.saturating_mul(2)).min(300), std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

static REFRESH_BACKOFF_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(15);
static NEXT_REFRESH_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Copies the signed-in network onto the service config so an unattended host
/// keeps the server after this user signs out of Windows.
pub fn copy_to_service(user: &Config) {
    let path = crate::syspaths::service_config_path();
    let mut cfg = Config::load_from(&path).unwrap_or_default();
    cfg.server = user.server.clone();
    cfg.server_fingerprint = user.server_fingerprint.clone();
    cfg.network_email = user.network_email.clone();
    cfg.network_refresh = user.network_refresh.clone();
    cfg.network_access = user.network_access.clone();
    cfg.network_access_exp = user.network_access_exp;
    cfg.network_device_id = user.network_device_id.clone();
    cfg.network_standby = user.network_standby.clone();
    cfg.network_standby_fingerprint = user.network_standby_fingerprint.clone();
    cfg.network_lan = user.network_lan.clone();
    cfg.network_public_ip = user.network_public_ip.clone();
    cfg.server_access_key = user.server_access_key.clone();
    cfg.network_role = user.network_role.clone();
    let grant_changed = cfg.support_grant != user.support_grant;
    cfg.support_grant = user.support_grant.clone();
    let _ = cfg.save_to(&path);
    if grant_changed {
        let marker = crate::syspaths::service_dir().join("support-key.reload");
        let _ = std::fs::write(marker, b"1\n");
    }
}

/// `owner` or `member` from GET /api/network. Empty when pjdesk.com does not say.
pub fn account_role(login_token: &str) -> String {
    if login_token.trim().is_empty() {
        return String::new();
    }
    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        role: Option<String>,
    }
    let url = format!("{}/api/network", api_base());
    let response = agent().get(&url).set("Authorization", &format!("Bearer {login_token}")).call();
    let Ok(response) = response else { return String::new() };
    let Ok(text) = response.into_string() else { return String::new() };
    serde_json::from_str::<Body>(&text).ok().and_then(|body| body.role).unwrap_or_default()
}

/// A network key carried by `--config`, by this executable's file name
/// (`PJDesk-Setup-0.6.0--pjdesk-net1:….exe`), or by `network-key.name` written
/// next to the exe at install. Only an encrypted `pjdesk-net1:` blob matches.
pub fn installer_network_key(explicit: Option<&str>) -> Option<String> {
    if let Some(text) = explicit {
        if let Some(blob) = pjdesk_proto::netkey::blob_from_text(text) {
            return Some(blob);
        }
    }
    let exe = std::env::current_exe().ok()?;
    if let Some(name) = exe.file_name().and_then(|s| s.to_str()) {
        if let Some(blob) = pjdesk_proto::netkey::blob_from_text(name) {
            return Some(blob);
        }
    }
    let path = exe.parent()?.join("network-key.name");
    let text = std::fs::read_to_string(path).ok()?;
    pjdesk_proto::netkey::blob_from_text(&text)
}

/// An 8-digit invite. `code` is empty when the server listed a code this
/// process did not create (the digits are not stored).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinCode {
    pub id: String,
    pub code: String,
    pub expires_at: u64,
}

/// Body for create and revoke. This is the network refresh token and device id,
/// the same pair the saved list sends. The relay access token is not included.
pub fn join_code_body(refresh: &str, device_id: &str, id: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({ "refresh": refresh, "device_id": device_id });
    if let Some(id) = id {
        body["id"] = serde_json::Value::String(id.to_string());
    }
    body
}

fn require_saved_sign_in(refresh: &str, device_id: &str) -> Result<()> {
    if refresh.trim().is_empty() || device_id.trim().is_empty() {
        bail!("Sign in again.");
    }
    Ok(())
}

/// The message pjdesk.com returned. A body that is not JSON keeps the status
/// and the text.
pub fn invite_failure_message(status: u16, body: &str) -> String {
    let parsed: ErrorBody = serde_json::from_str(body).unwrap_or_default();
    if !parsed.error.is_empty() {
        return parsed.error;
    }
    let snippet: String = body.chars().filter(|c| !c.is_control()).take(160).collect();
    let snippet = snippet.trim();
    if snippet.is_empty() {
        format!("PJDesk replied {status}.")
    } else {
        format!("PJDesk replied {status}: {snippet}")
    }
}

/// Creates a single-use 8-digit code for this account's network. Older unused
/// codes for that network are revoked. The digits are returned once.
pub fn create_join_code(refresh: &str, device_id: &str) -> Result<JoinCode> {
    require_saved_sign_in(refresh, device_id)?;
    let text = post_join(&format!("{}/api/network/join-code", api_base()), &join_code_body(refresh, device_id, None))?;
    parse_join(&text, true)
}

/// Active invite codes this account can revoke. Digits are not included.
pub fn list_join_codes(refresh: &str, device_id: &str) -> Result<Vec<JoinCode>> {
    require_saved_sign_in(refresh, device_id)?;
    let text = get_join(&format!("{}/api/network/join-code", api_base()), refresh, device_id)?;
    let parsed: serde_json::Value = serde_json::from_str(&text).context("unexpected reply from PJDesk")?;
    let mut out = Vec::new();
    for item in parsed.get("codes").and_then(|v| v.as_array()).into_iter().flatten() {
        out.push(JoinCode {
            id: item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            code: String::new(),
            expires_at: item.get("expires_at").and_then(|v| v.as_u64()).unwrap_or(0),
        });
    }
    Ok(out)
}

pub fn revoke_join_code(refresh: &str, device_id: &str, id: &str) -> Result<()> {
    require_saved_sign_in(refresh, device_id)?;
    post_join(&format!("{}/api/network/join-code/revoke", api_base()), &join_code_body(refresh, device_id, Some(id)))?;
    Ok(())
}

/// Publishes the hash of an 8-digit support code for this computer. The
/// temporary password is not sent.
pub fn offer_support(code: &str, pjdesk_id: &str) -> Result<u64> {
    let digits = digits_only(code);
    let id = pjdesk_proto::normalize_id(pjdesk_id);
    if digits.len() != 8 || !pjdesk_proto::is_valid_id(&id) {
        bail!("This computer is not online yet, so a support code cannot be used.");
    }
    let text = post_json(
        &format!("{}/api/support/offer", api_base()),
        None,
        &serde_json::json!({
            "code_hash": pjdesk_proto::sha256_hex(digits.as_bytes()),
            "pjdesk_id": id,
        }),
    )?;
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    Ok(parsed.get("expires_at").and_then(|v| v.as_u64()).unwrap_or(0))
}

/// Drops a published support code. A failure still expires after about 10 minutes.
pub fn revoke_support(code: &str, pjdesk_id: &str) -> Result<()> {
    let digits = digits_only(code);
    let id = pjdesk_proto::normalize_id(pjdesk_id);
    if digits.len() != 8 || !pjdesk_proto::is_valid_id(&id) {
        return Ok(());
    }
    post_json_timeout(
        &format!("{}/api/support/revoke", api_base()),
        None,
        &serde_json::json!({
            "code_hash": pjdesk_proto::sha256_hex(digits.as_bytes()),
            "pjdesk_id": id,
        }),
        3,
    )?;
    Ok(())
}

/// The 9-digit ID behind an 8-digit support code. The password is checked by
/// that computer, not by pjdesk.com.
pub fn lookup_support(code: &str) -> Result<String> {
    let digits = digits_only(code);
    if digits.len() != 8 {
        bail!("Enter the 8-digit code.");
    }
    let text = post_json(
        &format!("{}/api/support/lookup", api_base()),
        None,
        &serde_json::json!({ "code": digits }),
    )?;
    let parsed: serde_json::Value = serde_json::from_str(&text).context("unexpected reply from PJDesk")?;
    let id = parsed.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !pjdesk_proto::is_valid_id(&id) {
        bail!("That code is not valid.");
    }
    Ok(id)
}

fn digits_only(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn parse_join(text: &str, with_code: bool) -> Result<JoinCode> {
    let parsed: serde_json::Value = serde_json::from_str(text).context("unexpected reply from PJDesk")?;
    let code = if with_code { parsed.get("code").and_then(|v| v.as_str()).unwrap_or("").to_string() } else { String::new() };
    let id = parsed.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !id.starts_with("jnc_") || (with_code && digits_only(&code).len() != 8) {
        bail!("PJDesk did not return a code.");
    }
    Ok(JoinCode { id, code: digits_only(&code), expires_at: parsed.get("expires_at").and_then(|v| v.as_u64()).unwrap_or(0) })
}

/// How long an invite code has left, for the countdown Ben sees.
pub fn remaining_label(expires_unix: u64, now_unix: u64) -> String {
    if expires_unix == 0 || expires_unix <= now_unix {
        return "Expired".into();
    }
    let left = expires_unix - now_unix;
    let hours = left / 3600;
    let mins = (left % 3600) / 60;
    let secs = left % 60;
    if hours > 0 {
        format!("{hours} h {mins} min")
    } else if mins > 0 {
        format!("{mins} min {secs} s")
    } else {
        format!("{secs} s")
    }
}

/// "12345678" → "1234 5678".
pub fn group_digits(code: &str) -> String {
    let digits = digits_only(code);
    if digits.len() == 8 {
        format!("{} {}", &digits[..4], &digits[4..])
    } else {
        digits
    }
}

pub(crate) fn post_saved(url: &str, refresh: &str, device_id: &str, body: Option<&serde_json::Value>) -> Result<String> {
    if refresh.trim().is_empty() || device_id.trim().is_empty() {
        bail!("Sign in again.");
    }
    match body {
        Some(body) => post_join(url, body),
        None => get_join(url, refresh, device_id),
    }
}

fn post_join(url: &str, body: &serde_json::Value) -> Result<String> {
    let req = agent().post(url).set("content-type", "application/json");
    match req.send_string(&body.to_string()) {
        Ok(resp) => Ok(resp.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, resp)) => bail!(invite_failure_message(code, &resp.into_string().unwrap_or_default())),
        Err(ureq::Error::Transport(_)) => bail!("Could not reach pjdesk.com."),
    }
}

fn get_join(url: &str, refresh: &str, device_id: &str) -> Result<String> {
    let req = agent().get(url).set("Authorization", &format!("Bearer {refresh}")).set("X-PJDesk-Device", device_id);
    match req.call() {
        Ok(resp) => Ok(resp.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, resp)) => bail!(invite_failure_message(code, &resp.into_string().unwrap_or_default())),
        Err(ureq::Error::Transport(_)) => bail!("Could not reach pjdesk.com."),
    }
}

fn post_json_timeout(url: &str, bearer: Option<&str>, body: &serde_json::Value, secs: u64) -> Result<String> {
    let mut req = ureq::AgentBuilder::new().timeout(Duration::from_secs(secs)).build().post(url).set("content-type", "application/json");
    if let Some(token) = bearer {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    match req.send_string(&body.to_string()) {
        Ok(resp) => Ok(resp.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(_, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            let parsed: ErrorBody = serde_json::from_str(&text).unwrap_or_default();
            bail!(if parsed.error.is_empty() { "Could not reach pjdesk.com.".to_string() } else { parsed.error })
        }
        Err(ureq::Error::Transport(_)) => bail!("Could not reach pjdesk.com."),
    }
}

/// Removes the installer's pending file after the key has been applied.
pub fn clear_installer_network_key() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(path) = exe.parent().map(|dir| dir.join("network-key.name")) else { return };
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_login_uses_auth_key_not_password() {
        let body = account_login_body("ben@spader.us", &"ab".repeat(32), "PJDesk on desk");
        assert!(body.get("password").is_none());
        assert_eq!(body["email"], "ben@spader.us");
        assert_eq!(body["auth_key"].as_str().unwrap().len(), 64);
        assert_eq!(body["device"], "PJDesk on desk");
        assert!(!body.to_string().contains("password"));
    }

    #[test]
    fn a_transport_error_does_not_count_as_signed_out() {
        assert!(!refresh_failure_revokes(0, "Could not reach pjdesk.com."));
        assert!(!refresh_failure_revokes(503, "PJDesk could not sign in (503)."));
        assert!(refresh_failure_revokes(401, "Sign in required."));
        assert!(refresh_failure_revokes(401, "This network key was revoked."));
    }

    #[test]
    fn an_old_config_file_is_still_signed_in() {
        let raw = r#"{"network_email":"ben@spader.us","network_refresh":"plain:616263","device_id":"123456789","address_book":[{"id":"902488771","alias":"Cabin","password":null,"group":""}]}"#;
        let cfg: crate::config::Config = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.network_email.as_deref(), Some("ben@spader.us"));
        assert!(!cfg.network_refresh_token().is_empty() || cfg.network_refresh.is_some());
        assert_eq!(cfg.address_book.len(), 1);
        assert_eq!(cfg.device_id.as_deref(), Some("123456789"));
    }

    #[test]
    fn the_uploaded_computer_list_has_no_password() {
        let entries = vec![AddressEntry {
            id: "123456789".into(),
            alias: "Office".into(),
            password: Some("correct horse".into()),
            ..Default::default()
        }];
        let mut notes = std::collections::BTreeMap::new();
        notes.insert(
            "123456789".into(),
            PeerNote {
                hostname: "OFFICE-PC".into(),
                os_family: "windows".into(),
                os_version: "11".into(),
                ..Default::default()
            },
        );
        let body = computers_body(&entries, &notes, &[]);
        let text = body.to_string();
        assert!(!text.contains("correct horse"));
        assert!(!text.contains("password"));
        assert!(text.contains("Office"));
        assert!(text.contains("OFFICE-PC"));
        assert!(text.contains("windows"));

        let id_only = vec![AddressEntry {
            id: "386626882".into(),
            alias: "386 626 882".into(),
            password: Some("secret".into()),
            ..Default::default()
        }];
        let mut seen = std::collections::BTreeMap::new();
        seen.insert("386626882".into(), PeerNote { hostname: "BEN-PC".into(), ..Default::default() });
        let uploaded = computers_body(&id_only, &seen, &[]);
        let name = uploaded["computers"][0]["name"].as_str().unwrap();
        assert_eq!(name, "");
        assert_eq!(uploaded["computers"][0]["hostname"].as_str().unwrap(), "BEN-PC");
        assert!(!uploaded.to_string().contains("secret"));
    }

    #[test]
    fn only_the_owner_with_a_pinned_server_is_offered_that_server() {
        let fp = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        assert!(offer_current_server("owner", "pjdesk.spader.us:21200", fp));
        assert!(!offer_current_server("member", "pjdesk.spader.us:21200", fp));
        assert!(!offer_current_server("owner", "", fp));
        assert!(!offer_current_server("owner", "pjdesk.spader.us:21200", ""));
        assert!(offer_current_server("", "pjdesk.spader.us:21200", fp));
        assert_eq!(
            NO_SERVER,
            "Your network has no server yet. Sign in to PJDesk Server on your server computer, or set it on pjdesk.com → Account → My network."
        );
    }

    #[test]
    fn signing_in_adds_this_computer_and_keeps_a_nickname() {
        let existing = vec![SavedComputer {
            id: "692996394".into(),
            name: "Office".into(),
            hostname: "OLD".into(),
            os_family: "windows".into(),
            os_version: "10".into(),
            display_name: String::new(),
            ..SavedComputer::default()
        }];
        let this = ThisComputer {
            id: "692 996 394".into(),
            hostname: "BENLENOVODESK".into(),
            display_name: "Ben's desk".into(),
            os_family: "windows".into(),
            os_version: "11".into(),
        };
        let updated = enroll_this_computer(&existing, &this);
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].name, "Office");
        assert_eq!(updated[0].hostname, "BENLENOVODESK");
        assert_eq!(updated[0].display_name, "Ben's desk");
        assert_eq!(updated[0].os_version, "11");
        assert!(!updated[0].hidden);

        let fresh = enroll_this_computer(&[], &this);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, "692996394");
        assert_eq!(fresh[0].name, "");
        assert_eq!(fresh[0].hostname, "BENLENOVODESK");

        let removed = vec![SavedComputer {
            id: "692996394".into(),
            name: "Office".into(),
            hidden: true,
            ..SavedComputer::default()
        }];
        let back = enroll_this_computer(&removed, &this);
        assert!(!back[0].hidden);
        assert_eq!(back[0].name, "Office");

        let skipped = enroll_this_computer(&existing, &ThisComputer { id: "nope".into(), ..this });
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].hostname, "OLD");
    }

    #[test]
    fn a_hostname_is_kept_when_the_update_is_only_an_ip() {
        assert_eq!(keep_hostname("pjdesk.spader.us:21200", "204.248.122.44:21200"), "pjdesk.spader.us:21200");
        assert_eq!(keep_hostname("204.248.122.44:21200", "pjdesk.spader.us:21200"), "pjdesk.spader.us:21200");
        assert_eq!(keep_hostname("pjdesk.spader.us:21200", ""), "pjdesk.spader.us:21200");
        assert!(is_old_no_server("Set the server address in My network first."));
        assert!(!is_old_no_server(NO_SERVER));
    }

    #[test]
    fn an_invite_countdown_and_grouped_code() {
        assert_eq!(remaining_label(3_700, 100), "1 h 0 min");
        assert_eq!(remaining_label(190, 100), "1 min 30 s");
        assert_eq!(remaining_label(100, 100), "Expired");
        assert_eq!(remaining_label(0, 50), "Expired");
        assert_eq!(group_digits("12 345 678"), "1234 5678");
        assert_eq!(group_digits("abcd"), "");
    }

    #[test]
    fn an_invite_sends_the_saved_sign_in_and_shows_the_server_error() {
        let access = format!("{}.{}", "aa".repeat(80), "bb".repeat(64));
        let refresh = "cd".repeat(32);
        let body = join_code_body(&refresh, "device-1", None);
        assert_eq!(body["refresh"].as_str(), Some(refresh.as_str()));
        assert_eq!(body["device_id"].as_str(), Some("device-1"));
        assert!(body.get("access").is_none());
        assert!(!body.to_string().contains(&access));
        let revoke = join_code_body(&refresh, "device-1", Some("jnc_abc"));
        assert_eq!(revoke["id"].as_str(), Some("jnc_abc"));
        assert_eq!(invite_failure_message(401, r#"{"error":"Please sign in."}"#), "Please sign in.");
        assert_eq!(
            invite_failure_message(404, r#"{"error":"Set your network before creating a code."}"#),
            "Set your network before creating a code."
        );
        assert_eq!(invite_failure_message(401, ""), "PJDesk replied 401.");
        assert!(!invite_failure_message(401, "upstream down").contains("Wrong email"));
        assert!(invite_failure_message(502, "upstream down").contains("502"));
    }
}
