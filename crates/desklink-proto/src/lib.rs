//! Wire protocol shared by the DeskLink server and client.
//!
//! DeskLink uses three channels:
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

pub mod auth;
pub mod framing;
pub mod peer;
pub mod signal;
pub mod tls;
pub mod udp;

/// Bumped whenever a wire-incompatible change is made.
pub const PROTOCOL_VERSION: u16 = 1;

/// Default port of the server, used for TCP (signaling) and UDP (rendezvous/relay).
pub const DEFAULT_PORT: u16 = 21200;

/// ALPN protocol id for the signaling TLS connection.
pub const ALPN_SIGNAL: &[u8] = b"desklink-signal/1";
/// ALPN protocol id for the peer-to-peer QUIC connection.
pub const ALPN_PEER: &[u8] = b"desklink-peer/1";

/// TLS server names. Certificates are pinned by fingerprint, so these names are
/// informational only.
pub const SIGNAL_SERVER_NAME: &str = "desklink-server";
pub const PEER_SERVER_NAME: &str = "desklink-host";

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
    fn ids_format_and_validate() {
        assert_eq!(format_id("123456789"), "123 456 789");
        assert_eq!(normalize_id(" 123-456 789 "), "123456789");
        for _ in 0..1000 {
            assert!(is_valid_id(&generate_id()));
        }
        assert!(!is_valid_id("012345678"));
        assert!(!is_valid_id("12345678"));
    }
}
