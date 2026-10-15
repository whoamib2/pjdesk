//! Password authentication with SPAKE2, bound to the QUIC/TLS session.
//!
//! The host's one-time password never leaves the host. Instead both sides run
//! SPAKE2 (a password-authenticated key exchange): each derives a key that only
//! matches if both used the same password, and an eavesdropper — or a
//! malicious relay/server — learns nothing that lets it test passwords offline.
//!
//! Both sides then prove they hold the key by sending an HMAC over the TLS
//! exporter value of *this* connection ("channel binding"). If anyone sat in
//! the middle with their own TLS certificate, the two sides would have
//! different exporter values and the proofs would not verify.

use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};

const ID_CONTROLLER: &[u8] = b"pjdesk-controller";
const ID_HOST: &[u8] = b"pjdesk-host";

/// Label for the TLS keying-material exporter used as channel binding.
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-pjdesk-auth-v1";

const TAG_HOST: &[u8] = b"pjdesk host confirm";
const TAG_CONTROLLER: &[u8] = b"pjdesk controller confirm";

/// In-progress SPAKE2 exchange.
pub struct Exchange(Spake2<Ed25519Group>);

fn password(pw: &str) -> Password {
    Password::new(pw.trim().as_bytes())
}

/// Controller side: returns the exchange state and the message to send.
pub fn controller_start(pw: &str) -> (Exchange, Vec<u8>) {
    let (state, msg) =
        Spake2::<Ed25519Group>::start_a(&password(pw), &Identity::new(ID_CONTROLLER), &Identity::new(ID_HOST));
    (Exchange(state), msg)
}

/// Host side: returns the exchange state and the message to send.
pub fn host_start(pw: &str) -> (Exchange, Vec<u8>) {
    let (state, msg) =
        Spake2::<Ed25519Group>::start_b(&password(pw), &Identity::new(ID_CONTROLLER), &Identity::new(ID_HOST));
    (Exchange(state), msg)
}

/// Shared session key derived from SPAKE2.
pub struct SessionKey(Vec<u8>);

impl Exchange {
    /// Completes the exchange with the peer's message.
    pub fn finish(self, peer_msg: &[u8]) -> Result<SessionKey> {
        self.0.finish(peer_msg).map(SessionKey).map_err(|e| anyhow!("invalid SPAKE2 message: {e:?}"))
    }
}

impl SessionKey {
    fn tag(&self, label: &[u8], binding: &[u8]) -> Hmac<Sha256> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.0).expect("HMAC accepts any key length");
        mac.update(label);
        mac.update(binding);
        mac
    }

    /// The host's key confirmation for this channel.
    pub fn host_confirm(&self, binding: &[u8]) -> [u8; 32] {
        self.tag(TAG_HOST, binding).finalize().into_bytes().into()
    }

    /// The controller's key confirmation for this channel.
    pub fn controller_confirm(&self, binding: &[u8]) -> [u8; 32] {
        self.tag(TAG_CONTROLLER, binding).finalize().into_bytes().into()
    }

    /// Constant-time check of the host's confirmation.
    pub fn verify_host(&self, binding: &[u8], tag: &[u8; 32]) -> bool {
        self.tag(TAG_HOST, binding).verify_slice(tag).is_ok()
    }

    /// Constant-time check of the controller's confirmation.
    pub fn verify_controller(&self, binding: &[u8], tag: &[u8; 32]) -> bool {
        self.tag(TAG_CONTROLLER, binding).verify_slice(tag).is_ok()
    }
}

/// Generates a random one-time password like `k7q2m9xp`
/// (lowercase, without easily confused characters).
pub fn generate_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    (0..8).map(|_| ALPHABET[crate::random_below(ALPHABET.len() as u32) as usize] as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(host_pw: &str, ctrl_pw: &str, host_binding: &[u8], ctrl_binding: &[u8]) -> (bool, bool) {
        let (c_state, c_msg) = controller_start(ctrl_pw);
        let (h_state, h_msg) = host_start(host_pw);
        let h_key = h_state.finish(&c_msg).unwrap();
        let c_key = c_state.finish(&h_msg).unwrap();
        let host_ok = c_key.verify_host(ctrl_binding, &h_key.host_confirm(host_binding));
        let ctrl_ok = h_key.verify_controller(host_binding, &c_key.controller_confirm(ctrl_binding));
        (host_ok, ctrl_ok)
    }

    #[test]
    fn same_password_authenticates() {
        assert_eq!(run("k7q2m9xp", "k7q2m9xp ", b"chan", b"chan"), (true, true));
    }

    #[test]
    fn wrong_password_fails() {
        assert_eq!(run("k7q2m9xp", "k7q2m9xq", b"chan", b"chan"), (false, false));
    }

    #[test]
    fn different_channel_fails() {
        // Same password, but the two sides are on different TLS sessions (MITM).
        assert_eq!(run("pw", "pw", b"chan-a", b"chan-b"), (false, false));
    }

    #[test]
    fn passwords_look_right() {
        let pw = generate_password();
        assert_eq!(pw.len(), 8);
        assert!(pw.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }
}
