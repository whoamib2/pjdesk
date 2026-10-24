//! Whether a peer may register, look up an ID, or open a relay.
//!
//! The file `network-access.json` is what the running server enforces. The
//! management window writes it immediately, and a cloud check-in copies the
//! account's policy into it. Each gated message reloads the file, so a revoke
//! or the require switch applies without a restart.
//!
//! While `require_accounts` is false (the default), a peer with no token is
//! still admitted. That is the grace period: computers that registered before
//! 0.6.0 keep working until the operator turns the switch on. A token that
//! names a revoked device or an account that is not on a non-empty allow list
//! is refused even during grace, so a revoke cuts that device off now.

use std::path::{Path, PathBuf};

use pjdesk_proto::access::{self, Claims, VerifyError, SIGN_IN_REQUIRED};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "network-access.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Policy {
    #[serde(default)]
    pub require_accounts: bool,
    /// Set when an operator turns the switch on or off. A 0.6.0 file that has
    /// the switch on without this was the upgrade bug, and is put back to grace.
    #[serde(default)]
    pub require_chosen: bool,
    /// Hex of the pjdesk.com Ed25519 public key. Empty until the first cloud pull.
    #[serde(default)]
    pub public_key: String,
    /// This server's certificate fingerprint, hex digits only.
    #[serde(default)]
    pub fingerprint: String,
    /// Empty means any account with a valid token for this server.
    #[serde(default)]
    pub allowed_accounts: Vec<String>,
    #[serde(default)]
    pub revoked_devices: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            require_accounts: false,
            require_chosen: false,
            public_key: String::new(),
            fingerprint: String::new(),
            allowed_accounts: Vec::new(),
            revoked_devices: Vec::new(),
        }
    }
}

impl Policy {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE_NAME);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let path = dir.join(FILE_NAME);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(&tmp, path)
    }
}

/// Cloud `require_accounts` is adopted only after an operator has used the switch.
/// A missing choice, including a 0.5.2 upgrade and a fresh network row, stays off.
pub fn adopt_cloud_require(chosen: bool, cloud_on: bool) -> bool {
    if chosen {
        cloud_on
    } else {
        false
    }
}

/// Writes grace mode when this data folder has no policy yet (a 0.5.2 upgrade)
/// or when 0.6.0 turned the switch on without an operator choosing it.
/// Returns true when a file that said on was put back to off.
pub fn restore_grace(dir: &Path) -> bool {
    let path = dir.join(FILE_NAME);
    let existed = path.exists();
    let mut policy = Policy::load(dir);
    if !existed {
        policy.require_accounts = false;
        policy.require_chosen = false;
        let _ = policy.save(dir);
        return false;
    }
    if policy.require_accounts && !policy.require_chosen {
        policy.require_accounts = false;
        let _ = policy.save(dir);
        return true;
    }
    false
}

/// Records this process's certificate fingerprint when the file does not have one.
pub fn remember_fingerprint(dir: &Path, fingerprint: &pjdesk_proto::tls::Fingerprint) {
    let mut policy = Policy::load(dir);
    let next = access::normalize_fingerprint(&pjdesk_proto::tls::format_fingerprint(fingerprint));
    if policy.fingerprint != next && !next.is_empty() {
        policy.fingerprint = next;
        let _ = policy.save(dir);
    }
}

/// A peer that may continue.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Grace: no usable token, and signed-in accounts are not required yet.
    Open,
    Signed(Claims),
}

/// Why a peer was refused. The peer only ever hears [`SIGN_IN_REQUIRED`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deny {
    Missing,
    Bad,
    Expired,
    WrongServer,
    Revoked,
    Account,
}

impl std::fmt::Display for Deny {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Deny::Missing => "missing token",
            Deny::Bad => "bad token",
            Deny::Expired => "expired",
            Deny::WrongServer => "wrong server",
            Deny::Revoked => "revoked",
            Deny::Account => "account not allowed",
        })
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Decides one Register, Connect, or UdpReady.
pub fn admit(policy: &Policy, token: &str, now: u64) -> Result<Decision, Deny> {
    let token = token.trim();
    if token.is_empty() {
        return if policy.require_accounts { Err(Deny::Missing) } else { Ok(Decision::Open) };
    }
    let key = match hex::decode(policy.public_key.trim()) {
        Ok(bytes) if bytes.len() == 32 => bytes,
        _ => {
            return if policy.require_accounts { Err(Deny::Bad) } else { Ok(Decision::Open) };
        }
    };
    let claims = match access::verify(&key, token, now, &policy.fingerprint) {
        Ok(claims) => claims,
        Err(VerifyError::Expired) => {
            return if policy.require_accounts { Err(Deny::Expired) } else { Ok(Decision::Open) };
        }
        Err(VerifyError::WrongServer) => {
            return if policy.require_accounts { Err(Deny::WrongServer) } else { Ok(Decision::Open) };
        }
        Err(VerifyError::Bad) => {
            return if policy.require_accounts { Err(Deny::Bad) } else { Ok(Decision::Open) };
        }
    };
    if policy.revoked_devices.iter().any(|d| d == &claims.device_id) {
        return Err(Deny::Revoked);
    }
    if !policy.allowed_accounts.is_empty()
        && !policy.allowed_accounts.iter().any(|e| e.eq_ignore_ascii_case(&claims.email))
    {
        return Err(Deny::Account);
    }
    if policy.require_accounts && policy.fingerprint.is_empty() {
        return Err(Deny::WrongServer);
    }
    Ok(Decision::Signed(claims))
}

/// The only sentence a refused peer is given.
pub fn peer_message() -> &'static str {
    SIGN_IN_REQUIRED
}

#[cfg(test)]
mod tests {
    use super::*;
    use pjdesk_proto::access::{sign_seed, Claims};

    fn policy_with(require: bool) -> (Policy, String, [u8; 32]) {
        let seed = [9u8; 32];
        let claims = Claims {
            email: "ben@spader.us".into(),
            network_id: "net_1".into(),
            servers: vec!["aabb".into()],
            device_id: "dev-1".into(),
            exp: 1_800_000_000,
        };
        let (token, public) = sign_seed(&seed, &claims).unwrap();
        let policy = Policy {
            require_accounts: require,
            require_chosen: require,
            public_key: hex::encode(public),
            fingerprint: "aabb".into(),
            allowed_accounts: Vec::new(),
            revoked_devices: Vec::new(),
        };
        (policy, token, public)
    }

    #[test]
    fn grace_allows_a_missing_token_until_the_switch_is_on() {
        let (open, _, _) = policy_with(false);
        assert!(matches!(admit(&open, "", 1_700_000_000), Ok(Decision::Open)));
        let (closed, _, _) = policy_with(true);
        assert_eq!(admit(&closed, "", 1_700_000_000), Err(Deny::Missing));
        assert_eq!(peer_message(), "Sign in required");
    }

    #[test]
    fn upgrading_from_0_5_2_and_a_fresh_network_leaves_require_off() {
        let dir = std::env::temp_dir().join(format!("pjdesk-grace-upgrade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!restore_grace(&dir), "a 0.5.2 folder has no policy file");
        let fresh = Policy::load(&dir);
        assert!(!fresh.require_accounts);
        assert!(!adopt_cloud_require(false, false), "a fresh network row is off");
        assert!(!adopt_cloud_require(false, true), "a sync must not turn grace off");

        let mut mistaken = Policy::default();
        mistaken.require_accounts = true;
        mistaken.save(&dir).unwrap();
        assert!(restore_grace(&dir));
        assert!(!Policy::load(&dir).require_accounts);

        let mut chosen = Policy::load(&dir);
        chosen.require_accounts = true;
        chosen.require_chosen = true;
        chosen.save(&dir).unwrap();
        assert!(!restore_grace(&dir));
        assert!(Policy::load(&dir).require_accounts);
        assert!(adopt_cloud_require(true, true));
        assert!(!adopt_cloud_require(true, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn valid_expired_wrong_server_and_revoked() {
        let now = 1_700_000_000u64;
        let (policy, token, _) = policy_with(true);
        assert!(matches!(admit(&policy, &token, now), Ok(Decision::Signed(_))));

        let (mut grace, token, _) = policy_with(false);
        assert!(matches!(admit(&grace, &token, now), Ok(Decision::Signed(_))));

        assert_eq!(admit(&policy, &token, 1_800_000_000), Err(Deny::Expired));
        assert!(matches!(admit(&grace, &token, 1_800_000_000), Ok(Decision::Open)));

        let mut wrong = policy.clone();
        wrong.fingerprint = "ffff".into();
        assert_eq!(admit(&wrong, &token, now), Err(Deny::WrongServer));
        grace.fingerprint = "ffff".into();
        assert!(matches!(admit(&grace, &token, now), Ok(Decision::Open)));

        let mut revoked = policy.clone();
        revoked.revoked_devices.push("dev-1".into());
        revoked.require_accounts = false;
        assert_eq!(admit(&revoked, &token, now), Err(Deny::Revoked));

        let mut allowed = policy.clone();
        allowed.require_accounts = false;
        allowed.allowed_accounts.push("ada@example.com".into());
        assert_eq!(admit(&allowed, &token, now), Err(Deny::Account));
        allowed.allowed_accounts.push("ben@spader.us".into());
        assert!(matches!(admit(&allowed, &token, now), Ok(Decision::Signed(_))));
    }

    #[test]
    fn a_refused_peer_learns_nothing_about_which_check_failed() {
        for deny in [Deny::Missing, Deny::Bad, Deny::Expired, Deny::WrongServer, Deny::Revoked, Deny::Account] {
            let _ = deny;
            assert_eq!(peer_message(), "Sign in required");
        }
    }
}
