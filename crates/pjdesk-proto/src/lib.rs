//! Wire protocol shared by the PJDesk server and client.
//!
//! PJDesk uses three channels:
//!
//! * **Signaling** ([`signal`]) – TLS 1.3 over TCP between every client and the
//!   server. Hosts stay connected so the server can tell them about incoming
//!   sessions; controllers use it to ask for a session and to learn how to
//!   reach the host.
//! * **Rendezvous / relay** ([`udp`]) – small UDP datagrams sent to the same
//!   port on the server. They reveal each peer's public (NAT-mapped) address
//!   and, if hole punching fails, the server forwards the peers' encrypted
//!   QUIC packets between them.
//! * **Peer** ([`peer`]) – a QUIC connection (TLS 1.3, end-to-end encrypted)
//!   between controller and host, either direct or through the relay. The
//!   password is checked with SPAKE2 ([`auth`]) so it never crosses the wire.

pub mod access;
pub mod applock;
pub mod netkey;
pub mod vault;
pub mod auth;
pub mod discover;
pub mod filesafe;
pub mod framing;
pub mod gesture;
pub mod handoff;
pub mod layout;
pub mod peer;
pub mod perms;
pub mod quality;
pub mod record;
pub mod signal;
pub mod tls;
pub mod trayshow;
pub mod traywatch;
pub mod udp;
pub mod update;

/// GitHub repo PJDesk updates itself from.
pub const UPDATE_REPO: &str = "whoamib2/pjdesk";

/// Highest protocol version this build speaks.
///
/// Version 2 is the 0.3.x session (screen, keyboard, mouse). Version 3 adds
/// permissions, file transfer, clipboard, chat, audio, monitors and the other
/// session features. Peers that only speak version 2 still connect; the new
/// messages are sent only after both sides have agreed on version 3.
pub const PROTOCOL_VERSION: u16 = 3;

/// Oldest protocol version this build still accepts.
pub const MIN_PROTOCOL_VERSION: u16 = 2;

/// True when `peer` is a version we can still talk to.
pub fn protocol_ok(peer: u16) -> bool {
    (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&peer)
}

/// True when a peer rejected our protocol version and a retry at
/// [`MIN_PROTOCOL_VERSION`] is worth attempting. Matches both this server's
/// wording and the 0.3.x wording ("server speaks protocol 2").
pub fn version_rejected(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("version mismatch") || m.contains("speaks protocol")
}

/// UDP port a host listens on for direct IP connections (no signaling server).
pub const DIRECT_PORT: u16 = 21201;
/// UDP port used for LAN discovery beacons.
pub const DISCOVERY_PORT: u16 = 21202;

/// Default port of the server, used for TCP (signaling) and UDP (rendezvous/relay).
pub const DEFAULT_PORT: u16 = 21200;

/// A fresh install has no server until the user signs in to their PJDesk account.
/// The account supplies the address and the pinned key. Nothing is baked in.
pub const DEFAULT_SERVER: &str = "";

/// Address an install that already registered (it has a device id) keeps using
/// when its saved server is empty. This is only so an update does not take an
/// existing computer offline. A fresh install has no device id and does not use it.
/// Do not show this name in the client, the website, or the installer.
pub const LEGACY_REGISTERED_SERVER: &str = "pjdesk.spader.us:21200";

/// ALPN protocol id for the signaling TLS connection.
pub const ALPN_SIGNAL: &[u8] = b"pjdesk-signal/1";
/// ALPN protocol id for the peer-to-peer QUIC connection.
pub const ALPN_PEER: &[u8] = b"pjdesk-peer/1";

/// TLS server names. Certificates are pinned by fingerprint, so these names are
/// informational only.
pub const SIGNAL_SERVER_NAME: &str = "pjdesk-server";
pub const PEER_SERVER_NAME: &str = "pjdesk-host";

/// Derives a stable 32-byte device secret from a machine-unique string (such as
/// the Windows MachineGuid). It is deterministic, so the same machine always
/// yields the same secret: the GUI and the background service then register as
/// one device (no duplicates) and the assigned ID survives reinstalls.
pub fn derive_device_secret(machine_id: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"pjdesk-device-identity-v1\0");
    h.update(machine_id.trim().to_ascii_lowercase().as_bytes());
    h.finalize().into()
}

/// Fills an array with bytes from the operating system's secure RNG.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).expect("operating system random number generator unavailable");
    buf
}

/// A random `u64` from the operating system's secure RNG.
pub fn random_u64() -> u64 {
    u64::from_le_bytes(random_bytes::<8>())
}

/// `count` decimal digits from the operating system's secure RNG.
pub fn random_digits(count: usize) -> String {
    (0..count).map(|_| char::from(b'0' + random_below(10) as u8)).collect()
}

/// Lowercase hex SHA-256. Invite and support codes are stored as this, never as digits.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// A random integer in `0..bound` (rejection sampling, no modulo bias).
pub fn random_below(bound: u32) -> u32 {
    assert!(bound > 0);
    let zone = u32::MAX - (u32::MAX % bound);
    loop {
        let v = u32::from_le_bytes(random_bytes::<4>());
        if v < zone {
            return v % bound;
        }
    }
}

/// Keeps only the digits of a user-typed device ID ("123 456 789" → "123456789").
pub fn normalize_id(input: &str) -> String {
    input.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Formats a device ID in groups of three ("123456789" → "123 456 789").
pub fn format_id(id: &str) -> String {
    let digits = normalize_id(id);
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Device IDs are nine digits and never start with zero.
pub fn is_valid_id(id: &str) -> bool {
    id.len() == 9 && id.bytes().all(|b| b.is_ascii_digit()) && !id.starts_with('0')
}

/// Generates a fresh nine-digit device ID.
pub fn generate_id() -> String {
    (100_000_000 + random_below(900_000_000)).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_codes_are_eight_digits() {
        let code = random_digits(8);
        assert_eq!(code.len(), 8);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(
            sha256_hex(b"12345678"),
            "ef797c8118f02dfb649607dd5d3f8c7623048c9c063d532cc95c5ed7a898a64f"
        );
    }

    #[test]
    fn ids_format_and_validate() {
        assert_eq!(format_id("123456789"), "123 456 789");
        assert_eq!(normalize_id(" 123-456 789 "), "123456789");
        for _ in 0..1000 {
            assert!(is_valid_id(&generate_id()));
        }
        assert!(!is_valid_id("012345678"));
        assert!(!is_valid_id("12345678"));
    }

    #[test]
    fn protocol_range_accepts_current_and_previous() {
        assert!(protocol_ok(MIN_PROTOCOL_VERSION));
        assert!(protocol_ok(PROTOCOL_VERSION));
        assert!(!protocol_ok(1));
        assert!(!protocol_ok(PROTOCOL_VERSION + 1));
    }

    #[test]
    fn version_rejection_is_recognized_for_old_and_new_wording() {
        assert!(version_rejected("version mismatch (host speaks protocol 3, controller 9)"));
        assert!(version_rejected("server speaks protocol 2, client 3"));
        assert!(!version_rejected("wrong password"));
    }
}
