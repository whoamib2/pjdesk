//! Support keys. An 8-digit code Ben creates for a client.
//!
//! The digits are shown once. pjdesk.com stores only the SHA-256. This is not
//! a server access key (`pjdesk-key1.`) and it is not the 10-minute
//! "let someone connect" code. The hash, expiry, and computer-limit checks
//! follow the same rules as a server access key, in a separate store.

use anyhow::{bail, Result};

use crate::network::{self, ApiFail};

pub const CLIENTS: &str = "Clients";

pub fn is_clients(tags: &[String]) -> bool {
    tags.iter().any(|tag| tag == CLIENTS)
}

/// "{owner} (support) has access".
pub fn banner(owner: &str) -> String {
    let who = owner.trim();
    let who = if who.is_empty() { "Support" } else { who };
    format!("{who} (support) has access")
}

/// Wire form of a consent challenge. The first line is the marker.
pub fn consent_wire(owner: &str, label: &str) -> String {
    let owner = owner.replace(['\n', '\r'], " ");
    let label = label.replace(['\n', '\r'], " ");
    format!("consent\n{owner}\n{label}")
}

pub fn parse_consent(message: &str) -> Option<(String, String)> {
    let mut lines = message.lines();
    if lines.next()? != "consent" {
        return None;
    }
    let owner = lines.next()?.trim().to_string();
    if owner.is_empty() {
        return None;
    }
    let label = lines.next().unwrap_or("").trim().to_string();
    Some((owner, label))
}

pub fn expiry_allowed(seconds: u64) -> bool {
    matches!(seconds, 3600 | 86_400 | 604_800 | 2_592_000 | 0)
}

/// "" when the key can enroll. Otherwise a short reason: revoked, expired, used, limit.
pub fn key_status(revoked: bool, expires_at: u64, reusable: bool, uses: u32, computers: u32, limit: u32, now: u64, same_device: bool) -> &'static str {
    if revoked {
        return "revoked";
    }
    if expires_at > 0 && expires_at <= now {
        return "expired";
    }
    if same_device {
        return "";
    }
    let cap = limit.max(1);
    if !reusable && uses > 0 {
        return "used";
    }
    if computers >= cap {
        return "limit";
    }
    ""
}

#[derive(Clone, Debug, Default)]
pub struct SupportComputer {
    pub device_id: String,
    pub pjdesk_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct SupportKeyBrief {
    pub id: String,
    pub label: String,
    pub reusable: bool,
    pub expires_at: u64,
    pub computer_limit: u32,
    pub unattended: bool,
    pub computers: Vec<SupportComputer>,
}

#[derive(Clone, Debug, Default)]
pub struct CreatedKey {
    #[allow(dead_code)]
    pub id: String,
    pub code: String,
    #[allow(dead_code)]
    pub expires_at: u64,
}

#[derive(Default)]
pub struct Panel {
    pub label: String,
    pub reusable: bool,
    /// Seconds. 0 means the key does not expire.
    pub expiry: u64,
    pub limit: String,
    pub unattended: bool,
    pub secret: String,
    pub show: bool,
    pub keys: Vec<SupportKeyBrief>,
    pub confirm: Option<String>,
    pub confirm_computer: Option<(String, String)>,
    pub busy: bool,
    pub note: String,
    pub loaded: bool,
    pub hidden: bool,
    pub rx: Option<std::sync::mpsc::Receiver<Job>>,
    pub enter_code: String,
    pub enter_show: bool,
    pub enter_consent: bool,
    pub need_consent: bool,
    pub enter_owner: String,
    pub enter_label: String,
    pub enter_note: String,
    pub leave_rx: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    pub leaving: bool,
}

pub enum Job {
    Created { code: String, keys: Vec<SupportKeyBrief> },
    Listed(Vec<SupportKeyBrief>),
    Failed(String),
}

pub fn list_keys(refresh: &str, device_id: &str) -> Result<Vec<SupportKeyBrief>> {
    let text = network::post_saved(&format!("{}/api/network/support-key", network::api_base()), refresh, device_id, None)?;
    parse_keys(&text)
}

pub fn create_key(refresh: &str, device_id: &str, label: &str, reusable: bool, expires_in: u64, computer_limit: u32, unattended: bool) -> Result<CreatedKey> {
    if !expiry_allowed(expires_in) {
        bail!("Choose 1 hour, 1 day, 7 days, 30 days, or never.");
    }
    let text = network::post_saved(
        &format!("{}/api/network/support-key", network::api_base()),
        refresh,
        device_id,
        Some(&serde_json::json!({
            "refresh": refresh,
            "device_id": device_id,
            "label": label,
            "reusable": reusable,
            "expires_in": expires_in,
            "computer_limit": computer_limit,
            "unattended": unattended,
        })),
    )?;
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let code = parsed.get("code").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let id = parsed.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !id.starts_with("sup_") || code.chars().filter(|c| c.is_ascii_digit()).count() != 8 {
        bail!("PJDesk did not return a support key.");
    }
    Ok(CreatedKey {
        id,
        code: code.chars().filter(|c| c.is_ascii_digit()).collect(),
        expires_at: parsed.get("expires_at").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

pub fn revoke_key(refresh: &str, device_id: &str, id: &str) -> Result<()> {
    network::post_saved(
        &format!("{}/api/network/support-key/revoke", network::api_base()),
        refresh,
        device_id,
        Some(&serde_json::json!({ "refresh": refresh, "device_id": device_id, "id": id })),
    )?;
    Ok(())
}

pub fn remove_computer_by_id(refresh: &str, device_id: &str, pjdesk_id: &str) -> Result<()> {
    let want = pjdesk_proto::normalize_id(pjdesk_id);
    let keys = list_keys(refresh, device_id)?;
    let Some(key) = keys.into_iter().find(|key| {
        key.computers.iter().any(|computer| pjdesk_proto::normalize_id(&computer.pjdesk_id) == want)
    }) else {
        bail!("That computer is not on a support key.");
    };
    remove_computer(refresh, device_id, &key.id, &want, "")
}

pub fn remove_computer(refresh: &str, device_id: &str, key_id: &str, pjdesk_id: &str, target_device: &str) -> Result<()> {
    network::post_saved(
        &format!("{}/api/network/support-key/remove", network::api_base()),
        refresh,
        device_id,
        Some(&serde_json::json!({
            "refresh": refresh,
            "device_id": device_id,
            "id": key_id,
            "pjdesk_id": pjdesk_id,
            "target_device": target_device,
        })),
    )?;
    Ok(())
}

pub fn leave(refresh: &str, device_id: &str) -> Result<()> {
    network::post_saved(
        &format!("{}/api/network/support-key/leave", network::api_base()),
        refresh,
        device_id,
        Some(&serde_json::json!({ "refresh": refresh, "device_id": device_id })),
    )?;
    Ok(())
}

pub fn log_connection(refresh: &str, device_id: &str, pjdesk_id: &str, from_name: &str) -> Result<()> {
    network::post_saved(
        &format!("{}/api/network/support-key/connect", network::api_base()),
        refresh,
        device_id,
        Some(&serde_json::json!({
            "refresh": refresh,
            "device_id": device_id,
            "pjdesk_id": pjdesk_id,
            "from_name": from_name,
        })),
    )?;
    Ok(())
}

/// The consent challenge carried by an enroll failure, when the key is unattended.
pub fn consent_from_fail(fail: &ApiFail) -> Option<(String, String)> {
    if fail.code != "consent" {
        return None;
    }
    let owner = if fail.owner.trim().is_empty() { fail.message.clone() } else { fail.owner.clone() };
    if owner.trim().is_empty() {
        return None;
    }
    Some((owner, fail.label.clone()))
}

fn parse_keys(text: &str) -> Result<Vec<SupportKeyBrief>> {
    let parsed: serde_json::Value = serde_json::from_str(text).map_err(|_| anyhow::anyhow!("unexpected reply from PJDesk"))?;
    let mut out = Vec::new();
    for item in parsed.get("keys").and_then(|v| v.as_array()).into_iter().flatten() {
        let mut computers = Vec::new();
        for row in item.get("computers").and_then(|v| v.as_array()).into_iter().flatten() {
            computers.push(SupportComputer {
                device_id: row.get("device_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                pjdesk_id: row.get("pjdesk_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                name: row.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            });
        }
        out.push(SupportKeyBrief {
            id: item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            label: item.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            reusable: item.get("reusable").and_then(|v| v.as_bool()).unwrap_or(false),
            expires_at: item.get("expires_at").and_then(|v| v.as_u64()).unwrap_or(0),
            computer_limit: item.get("computer_limit").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
            unattended: item.get("unattended").and_then(|v| v.as_bool()).unwrap_or(false),
            computers,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_names_the_person_who_has_access() {
        assert_eq!(banner("Ben Spader"), "Ben Spader (support) has access");
        assert_eq!(banner("  "), "Support (support) has access");
    }

    #[test]
    fn consent_round_trips_and_ignores_other_errors() {
        let wire = consent_wire("Ben Spader", "Acme");
        assert_eq!(parse_consent(&wire), Some(("Ben Spader".into(), "Acme".into())));
        assert_eq!(parse_consent("That code is not valid."), None);
    }

    #[test]
    fn status_matches_single_use_limit_and_the_same_computer() {
        assert_eq!(key_status(false, 0, false, 0, 0, 1, 1_000, false), "");
        assert_eq!(key_status(true, 0, false, 0, 0, 1, 1_000, true), "revoked");
        assert_eq!(key_status(false, 1_000, false, 0, 0, 1, 1_000, false), "expired");
        assert_eq!(key_status(false, 0, false, 1, 1, 1, 1_000, false), "used");
        assert_eq!(key_status(false, 0, false, 1, 1, 1, 1_000, true), "");
        assert_eq!(key_status(false, 0, true, 1, 1, 1, 1_000, false), "limit");
        assert_eq!(key_status(false, 0, true, 1, 0, 1, 1_000, false), "");
        assert!(expiry_allowed(3600));
        assert!(expiry_allowed(0));
        assert!(!expiry_allowed(120));
    }
}
