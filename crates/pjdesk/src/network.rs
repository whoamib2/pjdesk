//! Sign in to a PJDesk account and receive this account's private server.
//!
//! Key derivation (600,000 rounds of PBKDF2) is slow on purpose. Call these
//! functions from a background thread, never from the UI thread.
//!
//! Saved-computer passwords are not part of this. The list uploaded here is
//! the nickname, the hostname, the ID, and the operating system.

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
}

/// A hostname is kept when a later update only has the raw IP it points at.
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
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    role: String,
}

struct ApiFail {
    status: u16,
    message: String,
    code: String,
    role: String,
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

fn api_base() -> String {
    std::env::var("PJDESK_CLOUD_API")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_API.into())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(25)).build()
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
            let parsed: ErrorBody = serde_json::from_str(&text).unwrap_or(ErrorBody { error: String::new(), code: String::new(), role: String::new() });
            let message = if !parsed.error.is_empty() {
                parsed.error
            } else if code == 401 {
                "Wrong email or password.".into()
            } else {
                format!("PJDesk could not sign in ({code}).")
            };
            Err(ApiFail { status: code, message, code: parsed.code, role: parsed.role })
        }
        Err(ureq::Error::Transport(_)) => Err(ApiFail {
            status: 0,
            message: "Could not reach pjdesk.com.".into(),
            code: String::new(),
            role: String::new(),
        }),
    }
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
            })
            .collect(),
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
        &serde_json::json!({
            "email": email,
            "auth_key": hex::encode(auth),
            "device": format!("PJDesk on {device_name}"),
        }),
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
                Ok(session) => Ok(SignIn::Ready(session)),
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
    let mut body = serde_json::json!({ "refresh": refresh_token, "device_id": device_id });
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
        &serde_json::json!({
            "email": email,
            "auth_key": hex::encode(auth),
            "device": "PJDesk",
        }),
    )?)
    .context("unexpected reply from PJDesk")?;
    Ok(login.token)
}

/// Enrolls this device with a one-time code from My network. No password is typed.
pub fn enroll(code: &str, device_id: &str, device_name: &str) -> Result<Session> {
    let text = post_json(
        &format!("{}/api/network/enroll", api_base()),
        None,
        &serde_json::json!({ "code": code, "device_id": device_id, "device_name": device_name }),
    )?;
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
    let list = enroll_this_computer(&list, this);
    let body = saved_value(&list);
    let mut next = refresh(&session.refresh, &session.device_id, Some(&body))?;
    if next.email.is_empty() {
        next.email = session.email.clone();
    }
    if next.address.trim().is_empty() {
        next.address = session.address.clone();
        next.fingerprint = session.fingerprint.clone();
    }
    Ok(next)
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
    for id in hidden {
        let id = pjdesk_proto::normalize_id(id);
        if pjdesk_proto::is_valid_id(&id) {
            ids.insert(id);
        }
    }
    let computers: Vec<serde_json::Value> = ids
        .into_iter()
        .map(|id| {
            if hidden.iter().any(|h| h == &id) {
                return serde_json::json!({ "id": id, "name": "", "hostname": "", "os_family": "", "os_version": "", "hidden": true });
            }
            let alias = entries.iter().find(|e| e.id == id).map(|e| e.alias.as_str()).unwrap_or("");
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
    match refresh(&refresh_token, &device, Some(&body)) {
        Ok(mut session) => {
            REFRESH_BACKOFF_SECS.store(15, std::sync::atomic::Ordering::Relaxed);
            NEXT_REFRESH_AT.store(0, std::sync::atomic::Ordering::Relaxed);
            if session.email.is_empty() {
                session.email = config.network_email.clone().unwrap_or_default();
            }
            config.apply_network_session(&session);
            let _ = config.save();
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
    let _ = cfg.save_to(&path);
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
}
