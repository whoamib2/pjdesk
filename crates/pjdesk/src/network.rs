//! Sign in to a PJDesk account and receive this account's private server.
//!
//! Key derivation (600,000 rounds of PBKDF2) is slow on purpose. Call these
//! functions from a background thread, never from the UI thread.
//!
//! Saved-computer passwords are not part of this. The list uploaded here is
//! name, ID, and operating system only.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use pjdesk_proto::signal::AddressEntry;
use serde::Deserialize;

use crate::config::{Config, PeerNote};

const DEFAULT_API: &str = "https://pjdesk.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedComputer {
    pub id: String,
    pub name: String,
    pub os_family: String,
    pub os_version: String,
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
    pub computers: Vec<SavedComputer>,
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
}

#[derive(Deserialize)]
struct ComputerBody {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    os_family: String,
    #[serde(default)]
    os_version: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
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

fn error_text(err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, resp) => {
            let text = resp.into_string().unwrap_or_default();
            let message = serde_json::from_str::<ErrorBody>(&text).map(|b| b.error).unwrap_or_default();
            if !message.is_empty() {
                message
            } else if code == 401 {
                "Wrong email or password.".into()
            } else {
                format!("PJDesk could not sign in ({code}).")
            }
        }
        ureq::Error::Transport(_) => "Could not reach pjdesk.com.".into(),
    }
}

fn post_json(url: &str, bearer: Option<&str>, body: &serde_json::Value) -> Result<String> {
    let mut req = agent().post(url).set("content-type", "application/json");
    if let Some(token) = bearer {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    match req.send_string(&body.to_string()) {
        Ok(resp) => Ok(resp.into_string().unwrap_or_default()),
        Err(e) => bail!(error_text(e)),
    }
}

fn session_from(email: &str, device_id: &str, reply: NetworkReply) -> Result<Session> {
    let network = reply.network.context("the account has no network")?;
    if network.active_address.trim().is_empty() {
        bail!("Set the server address on pjdesk.com → My network, then sign in again.");
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
        computers: reply
            .computers
            .into_iter()
            .map(|c| SavedComputer { id: c.id, name: c.name, os_family: c.os_family, os_version: c.os_version })
            .collect(),
    })
}

/// Signs in with the PJDesk account password and fetches the private server.
pub fn sign_in(email: &str, password: &str, device_id: &str, device_name: &str) -> Result<Session> {
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
    let text = post_json(
        &format!("{}/api/network/token", api_base()),
        Some(&login.token),
        &serde_json::json!({ "device_id": device_id, "device_name": device_name }),
    )?;
    let reply: NetworkReply = serde_json::from_str(&text).context("unexpected reply from PJDesk")?;
    session_from(&email, device_id, reply)
}

/// Exchanges a still-valid refresh token for a new access token.
pub fn refresh(refresh_token: &str, device_id: &str, computers: Option<&serde_json::Value>) -> Result<Session> {
    let mut body = serde_json::json!({ "refresh": refresh_token, "device_id": device_id });
    if let Some(computers) = computers {
        if let Some(list) = computers.get("computers") {
            body["computers"] = list.clone();
        }
    }
    let text = post_json(&format!("{}/api/network/refresh", api_base()), None, &body)?;
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

/// Name, ID, and OS only. A password on an address-book entry is dropped.
pub fn computers_body(entries: &[AddressEntry], notes: &std::collections::BTreeMap<String, PeerNote>) -> serde_json::Value {
    let computers: Vec<serde_json::Value> = entries
        .iter()
        .filter(|e| pjdesk_proto::is_valid_id(&e.id))
        .map(|e| {
            let note = notes.get(&e.id);
            serde_json::json!({
                "id": e.id,
                "name": e.alias,
                "os_family": note.map(|n| n.os_family.as_str()).unwrap_or(""),
                "os_version": note.map(|n| n.os_version.as_str()).unwrap_or(""),
            })
        })
        .collect();
    serde_json::json!({ "computers": computers })
}

/// Refreshes the access token when it is close to expiring. Safe to call from
/// the host thread. A failure leaves the current token in place.
pub fn refresh_config(config: &mut Config) {
    let refresh_token = config.network_refresh_token();
    let device = config.network_device_id.clone().unwrap_or_default();
    if refresh_token.is_empty() || device.is_empty() {
        return;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if config.network_access_exp > now + 120 && !config.network_access_token().is_empty() {
        return;
    }
    let body = computers_body(&config.address_book, &config.settings.peers);
    if let Ok(mut session) = refresh(&refresh_token, &device, Some(&body)) {
        if session.email.is_empty() {
            session.email = config.network_email.clone().unwrap_or_default();
        }
        config.apply_network_session(&session);
        let _ = config.save();
    }
}

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
    let _ = cfg.save_to(&path);
}

#[cfg(test)]
mod tests {
    use super::*;

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
            PeerNote { os_family: "windows".into(), os_version: "11".into(), ..Default::default() },
        );
        let body = computers_body(&entries, &notes);
        let text = body.to_string();
        assert!(!text.contains("correct horse"));
        assert!(!text.contains("password"));
        assert!(text.contains("Office"));
        assert!(text.contains("windows"));
    }
}
