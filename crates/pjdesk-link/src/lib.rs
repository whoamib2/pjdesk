//! The connection path every PJDesk client uses: TLS signaling, UDP hole
//! punching, the QUIC relay, and the SPAKE2 password check.
//!
//! Screen capture and the window stay in each client. This crate is the part
//! that has to stay identical, including on Android.

pub mod admit;
pub mod lockout;
pub mod p2p;
pub mod session;
pub mod signal;
pub mod udp_fallback;
