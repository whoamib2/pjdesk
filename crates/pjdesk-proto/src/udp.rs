//! UDP rendezvous datagrams.
//!
//! Every peer creates one UDP socket per session and uses it for everything:
//!
//! 1. **Bind** – sent to the server's UDP port with the session token. The
//!    server records the source address (the peer's public NAT mapping),
//!    replies with [`UdpMsg::BindAck`], and remembers the pair so it can relay.
//! 2. **Punch** – sent directly to the other peer's candidate addresses to
//!    open NAT pinholes before the QUIC handshake.
//! 3. **QUIC** – the same socket then carries the QUIC connection, either
//!    directly to the peer or to the server, which forwards every packet from
//!    one bound address to the other (a minimal TURN-like relay). QUIC is
//!    end-to-end encrypted, so the relay only ever sees ciphertext.
//!
//! Control datagrams start with [`MAGIC`]. Its first byte is `0x00`, and QUIC
//! packets always have the "fixed bit" (`0x40`) set in their first byte (we
//! disable QUIC-bit greasing), so the two can never be confused.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

use crate::signal::SessionId;

/// Prefix of every PJDesk control datagram.
pub const MAGIC: [u8; 4] = [0x00, b'D', b'L', b'K'];

/// Which side of a session a datagram comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    Host,
    Controller,
}

/// Control datagrams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UdpMsg {
    /// Peer → server: "this socket belongs to `role` in `session_id`".
    Bind { session_id: SessionId, token: [u8; 16], role: Role },
    /// Server → peer: bind accepted; `observed` is the address the server saw.
    BindAck { session_id: SessionId, observed: SocketAddr },
    /// Peer → peer: opens NAT mappings; ignored on receipt.
    Punch { session_id: SessionId },
}

/// Encodes a control datagram.
pub fn encode(msg: &UdpMsg) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&postcard::to_stdvec(msg).expect("UdpMsg always serializes"));
    out
}

/// Returns true if the datagram is a PJDesk control message (not QUIC).
pub fn is_control(datagram: &[u8]) -> bool {
    datagram.starts_with(&MAGIC)
}

/// Decodes a control datagram, or `None` if it is not one.
pub fn decode(datagram: &[u8]) -> Option<UdpMsg> {
    datagram.strip_prefix(&MAGIC[..]).and_then(|rest| postcard::from_bytes(rest).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_detection() {
        let msg = UdpMsg::Bind { session_id: 42, token: [7; 16], role: Role::Host };
        let bytes = encode(&msg);
        assert!(is_control(&bytes));
        match decode(&bytes) {
            Some(UdpMsg::Bind { session_id, token, role }) => {
                assert_eq!(session_id, 42);
                assert_eq!(token, [7; 16]);
                assert_eq!(role, Role::Host);
            }
            other => panic!("unexpected {other:?}"),
        }
        // A QUIC long-header packet (first byte 0b11xxxxxx) is never control.
        assert!(!is_control(&[0xc3, b'D', b'L', b'K', 1, 2, 3]));
        assert!(decode(&[0x40, 0, 0]).is_none());
    }
}
