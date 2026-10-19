//! Network access tokens signed by pjdesk.com.
//!
//! The canonical body is UTF-8, identical in Rust, the Worker, and the Android app:
//!
//! ```text
//! PJN1\n
//! {email lowercase}\n
//! {network id}\n
//! {comma-separated fingerprints, hex digits only, lowercase, sorted}\n
//! {device id}\n
//! {exp unix seconds}\n
//! ```
//!
//! The token is `hex(body) + "." + hex(64-byte Ed25519 signature)`. It lasts
//! 15 minutes. The fingerprint list is the servers this token may use (active
//! and standby). Revocation is not inside the token: the server checks its
//! own revoked-device and allowed-account lists on every gated message.

use hmac::{Hmac, Mac};
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// How long a network access token stays valid.
pub const TOKEN_TTL_SECS: u64 = 15 * 60;

/// What an unauthorized peer is told. The same string for a missing token, a
/// bad signature, expiry, the wrong server, and a revoked account or device,
/// so the reply never confirms that an ID exists.
pub const SIGN_IN_REQUIRED: &str = "Sign in required";

/// Who a token was issued to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    pub email: String,
    pub network_id: String,
    /// Normalized hex fingerprints of the servers this token may be used on.
    pub servers: Vec<String>,
    pub device_id: String,
    pub exp: u64,
}

/// Why a token was not accepted. Log this. Do not send it to the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    Bad,
    Expired,
    WrongServer,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            VerifyError::Bad => "bad token",
            VerifyError::Expired => "expired",
            VerifyError::WrongServer => "wrong server",
        })
    }
}

/// Hex digits only, lowercase. Colons, dashes, and a `sha256:` prefix drop out.
pub fn normalize_fingerprint(text: &str) -> String {
    let text = text.trim();
    let text = text.strip_prefix("sha256:").or_else(|| text.strip_prefix("SHA256:")).unwrap_or(text);
    text.chars().filter(|c| c.is_ascii_hexdigit()).flat_map(|c| c.to_lowercase()).collect()
}

/// The signed text. Fingerprints are normalized, de-duplicated, and sorted.
pub fn canonical(email: &str, network_id: &str, fingerprints: &[String], device_id: &str, exp: u64) -> String {
    let email = email.trim().to_lowercase();
    let mut fps: Vec<String> = fingerprints.iter().map(|f| normalize_fingerprint(f)).filter(|f| !f.is_empty()).collect();
    fps.sort();
    fps.dedup();
    format!("PJN1\n{email}\n{}\n{}\n{}\n{exp}\n", network_id.trim(), fps.join(","), device_id.trim())
}

/// Signs `claims`. `pkcs8` is the PKCS#8 Ed25519 private key (the form
/// WebCrypto exports). Returns the token and the 32-byte public key.
pub fn sign_pkcs8(pkcs8: &[u8], claims: &Claims) -> Result<(String, [u8; 32]), &'static str> {
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| "bad signing key")?;
    let body = canonical(&claims.email, &claims.network_id, &claims.servers, &claims.device_id, claims.exp);
    let sig = pair.sign(body.as_bytes());
    let mut public = [0u8; 32];
    public.copy_from_slice(pair.public_key().as_ref());
    Ok((format!("{}.{}", hex::encode(body.as_bytes()), hex::encode(sig.as_ref())), public))
}

/// Signs with a 32-byte seed. Tests and tools use this; the Worker stores PKCS#8.
pub fn sign_seed(seed: &[u8; 32], claims: &Claims) -> Result<(String, [u8; 32]), &'static str> {
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| "bad signing key")?;
    let body = canonical(&claims.email, &claims.network_id, &claims.servers, &claims.device_id, claims.exp);
    let sig = pair.sign(body.as_bytes());
    let mut public = [0u8; 32];
    public.copy_from_slice(pair.public_key().as_ref());
    Ok((format!("{}.{}", hex::encode(body.as_bytes()), hex::encode(sig.as_ref())), public))
}

/// Checks the signature and the expiry. `server_fingerprint` empty skips the
/// server check (the caller decides whether that is allowed).
pub fn verify(public_key: &[u8], token: &str, now: u64, server_fingerprint: &str) -> Result<Claims, VerifyError> {
    let (body_hex, sig_hex) = token.trim().split_once('.').ok_or(VerifyError::Bad)?;
    let body = hex::decode(body_hex).map_err(|_| VerifyError::Bad)?;
    let sig = hex::decode(sig_hex).map_err(|_| VerifyError::Bad)?;
    if public_key.len() != 32 || sig.len() != 64 {
        return Err(VerifyError::Bad);
    }
    let key = UnparsedPublicKey::new(&ED25519, public_key);
    key.verify(&body, &sig).map_err(|_| VerifyError::Bad)?;
    let text = std::str::from_utf8(&body).map_err(|_| VerifyError::Bad)?;
    parse_body(text, now, server_fingerprint)
}

fn parse_body(text: &str, now: u64, server_fingerprint: &str) -> Result<Claims, VerifyError> {
    let parts: Vec<&str> = text.split('\n').collect();
    if parts.len() != 7 || parts[0] != "PJN1" || !parts[6].is_empty() {
        return Err(VerifyError::Bad);
    }
    let email = parts[1].trim().to_lowercase();
    let network_id = parts[2].trim().to_string();
    let device_id = parts[4].trim().to_string();
    if email.is_empty() || network_id.is_empty() || device_id.is_empty() || email.contains('\n') {
        return Err(VerifyError::Bad);
    }
    let exp: u64 = parts[5].parse().map_err(|_| VerifyError::Bad)?;
    if now >= exp {
        return Err(VerifyError::Expired);
    }
    let servers: Vec<String> = parts[3]
        .split(',')
        .map(normalize_fingerprint)
        .filter(|f| !f.is_empty())
        .collect();
    let want = normalize_fingerprint(server_fingerprint);
    if !want.is_empty() && !servers.iter().any(|s| s == &want) {
        return Err(VerifyError::WrongServer);
    }
    Ok(Claims { email, network_id, servers, device_id, exp })
}

/// The same account keys pjdesk.com derives in the browser and PJDesk Server
/// derives for Cloud Sync. `auth` is sent to sign in. `sync` never leaves the
/// client or the customer's server.
pub fn derive_account_keys(email: &str, password: &str) -> ([u8; 32], [u8; 32]) {
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
    let auth = hmac32(&master, "pjdesk-cloud-auth");
    let sync = hmac32(&master, "pjdesk-cloud-sync-key");
    (auth, sync)
}

fn hmac32(key: &[u8], data: &str) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data.as_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(exp: u64, fps: &[&str]) -> Claims {
        Claims {
            email: "Ben@Spader.us".into(),
            network_id: "net_1".into(),
            servers: fps.iter().map(|s| (*s).to_string()).collect(),
            device_id: "device-1".into(),
            exp,
        }
    }

    #[test]
    fn canonical_matches_the_shared_fixture() {
        let text = canonical(
            "Ben@Spader.us",
            "net_1",
            &["AA:BB:CC:DD".into(), "aabbccdd".into(), "00112233".into()],
            "device-1",
            2_000_000_000,
        );
        assert_eq!(text, "PJN1\nben@spader.us\nnet_1\n00112233,aabbccdd\ndevice-1\n2000000000\n");
        assert_eq!(normalize_fingerprint("sha256:AB-CD"), "abcd");
    }

    #[test]
    fn valid_expired_and_wrong_server() {
        let seed = [7u8; 32];
        let now = 1_700_000_000u64;
        let (token, public) = sign_seed(&seed, &sample(now + 60, &["ab:cd"])).unwrap();
        let ok = verify(&public, &token, now, "AB-CD").unwrap();
        assert_eq!(ok.email, "ben@spader.us");
        assert_eq!(ok.servers, vec!["abcd".to_string()]);
        assert!(matches!(verify(&public, &token, now + 61, "abcd"), Err(VerifyError::Expired)));
        assert!(matches!(verify(&public, &token, now, "ffff"), Err(VerifyError::WrongServer)));
        assert!(matches!(verify(&public, "nope", now, "abcd"), Err(VerifyError::Bad)));
        assert!(matches!(verify(&[0u8; 32], &token, now, "abcd"), Err(VerifyError::Bad)));
        let (other, _) = sign_seed(&[8u8; 32], &sample(now + 60, &["abcd"])).unwrap();
        assert!(matches!(verify(&public, &other, now, "abcd"), Err(VerifyError::Bad)));
    }
}
