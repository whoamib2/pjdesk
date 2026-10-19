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

/// The owner of an empty network can publish the server this computer already uses.
pub fn offer_current_server(role: &str, server: &str, fingerprint: &str) -> bool {
    role == "owner" && !server.trim().is_empty() && pjdesk_proto::tls::parse_fingerprint(fingerprint).is_ok()
}

/// Account sign-in finished, or the account's network has no server yet.
pub enum SignIn {
    Ready(Session),
    NoServer { role: String, login_token: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedComputer {
    pub id: String,
    /// Optional nickname. Empty when the user has not named it.
    pub name: String,
    /// Hostname the host reported. Empty until a computer has been seen.
    pub hostname: String,
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
    hostname: String,
    #[serde(default)]
    os_family: String,
    #[serde(default)]
    os_version: String,
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
    message: String,
    code: String,
    role: String,
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
            Err(ApiFail { message, code: parsed.code, role: parsed.role })
        }
        Err(ureq::Error::Transport(_)) => Err(ApiFail {
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
        computers: reply
            .computers
            .into_iter()
            .map(|c| SavedComputer {
                id: c.id,
                name: c.name,
                hostname: c.hostname,
                os_family: c.os_family,
                os_version: c.os_version,
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
            Ok(SignIn::Ready(session_from(&email, device_id, reply)?))
        }
        Err(fail) if fail.code == "no_server" => Ok(SignIn::NoServer { role: fail.role, login_token: login.token }),
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

/// Nickname, hostname, ID, and OS. A password is dropped. An alias that is
/// only the ID, or only the hostname, is not uploaded as a nickname.
pub fn computers_body(entries: &[AddressEntry], notes: &std::collections::BTreeMap<String, PeerNote>) -> serde_json::Value {
    use std::collections::BTreeSet;
    let mut ids = BTreeSet::new();
    for entry in entries {
        if pjdesk_proto::is_valid_id(&entry.id) {
            ids.insert(entry.id.clone());
        }
    }
    for id in notes.keys() {
        if pjdesk_proto::is_valid_id(id) {
            ids.insert(id.clone());
        }
    }
    let computers: Vec<serde_json::Value> = ids
        .into_iter()
        .map(|id| {
            let alias = entries.iter().find(|e| e.id == id).map(|e| e.alias.as_str()).unwrap_or("");
            let note = notes.get(&id);
            let hostname = note.map(|n| n.hostname.as_str()).unwrap_or("");
            serde_json::json!({
                "id": id,
                "name": crate::config::nickname_text(alias, &id, hostname),
                "hostname": hostname,
                "os_family": note.map(|n| n.os_family.as_str()).unwrap_or(""),
                "os_version": note.map(|n| n.os_version.as_str()).unwrap_or(""),
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
    let body = computers_body(&config.address_book, &config.settings.peers);
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
        let body = computers_body(&entries, &notes);
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
        let uploaded = computers_body(&id_only, &seen);
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
        assert_eq!(
            NO_SERVER,
            "Your network has no server yet. Sign in to PJDesk Server on your server computer, or set it on pjdesk.com → Account → My network."
        );
    }
}
