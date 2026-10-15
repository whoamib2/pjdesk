//! Signaling messages exchanged with the server over TLS/TCP.
//!
//! Session setup, step by step:
//!
//! ```text
//!  Controller                     Server                          Host
//!      |                            |  <── Register ───────────────  |  (host stays connected)
//!      |                            |  ── Registered{id} ─────────>  |
//!      | ── Connect{target_id} ──>  |                                |
//!      |                            |  ── IncomingSession{token} ──> |
//!      |                            |  <── SessionResponse{accept} ─ |
//!      | <── SessionReady{token} ── |                                |
//!      | ~~ UDP Bind ~~~~~~~~~~~~>  |  <~~~~~~~~~~~~~~~ UDP Bind ~~~ |  (server learns public addrs)
//!      | ── UdpReady{lan addrs} ─>  |  <── UdpReady{lan, cert fp} ── |
//!      | <── PeerInfo{host addrs} ─ |  ── PeerInfo{ctrl addrs} ───>  |
//!      | ~~~~~~~~~~~~~~ UDP hole punching + QUIC handshake ~~~~~~~~~~ |
//!      |     (or QUIC through the server's UDP relay if that fails)   |
//! ```

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// Identifies one connection attempt between a controller and a host.
pub type SessionId = u64;

/// Messages sent by clients (hosts and controllers) to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Host: register this device (or reclaim its ID) and go online.
    ///
    /// The secret proves ownership of the ID; the server stores only its hash.
    Register { proto: u16, device_id: Option<String>, device_secret: [u8; 32], hostname: String, app_version: String },
    /// Controller: ask for a session with the device `target_id`.
    Connect { proto: u16, target_id: String, controller_name: String },
    /// Host: answer to [`ServerMsg::IncomingSession`].
    SessionResponse { session_id: SessionId, accept: bool, reason: String },
    /// Either peer: my UDP socket is bound (see [`crate::udp`]); here are my
    /// LAN candidate addresses. Hosts also send their QUIC certificate
    /// fingerprint so the controller can pin it.
    UdpReady { session_id: SessionId, local_addrs: Vec<SocketAddr>, cert_fingerprint: Option<[u8; 32]> },
    /// Sign in to an account on the server, with a password or a saved token
    /// (see [`Credential`]). The reply is [`ServerMsg::LoginResult`].
    Login { proto: u16, username: String, credential: Credential, device_name: String },
    /// Invalidate the saved token in [`Credential::Token`] (sign out everywhere
    /// is a server setting; this just drops the current one).
    Logout { token: String },
    /// Fetch the signed-in account's address book ([`ServerMsg::AddressBook`]).
    GetAddressBook { token: String },
    /// Replace the signed-in account's address book.
    SaveAddressBook { token: String, entries: Vec<AddressEntry> },
    /// Keep-alive.
    Ping,
}

/// How a client proves who it is when signing in.
#[derive(Clone, Serialize, Deserialize)]
pub enum Credential {
    /// A password typed by the user (checked against the server's Argon2 hash).
    Password(String),
    /// A token issued by an earlier successful login and saved by the client.
    Token(String),
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret.
        match self {
            Credential::Password(_) => f.write_str("Password(***)"),
            Credential::Token(_) => f.write_str("Token(***)"),
        }
    }
}

/// One saved computer in an account's address book.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddressEntry {
    /// The remote device's nine-digit PJDesk ID.
    pub id: String,
    /// A friendly name the user gave it (may be empty).
    #[serde(default)]
    pub alias: String,
    /// Free-form tags for grouping (may be empty).
    #[serde(default)]
    pub tags: Vec<String>,
    /// The host's access password, if the user asked to remember it. Stored in
    /// the user's own account on their own server; encrypted at rest on disk.
    #[serde(default)]
    pub password: Option<String>,
}

/// Messages sent by the server to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Registration succeeded; this is the device's ID.
    Registered { device_id: String },
    /// To a host: a controller wants to connect.
    IncomingSession {
        session_id: SessionId,
        token: [u8; 16],
        udp_port: u16,
        controller_name: String,
        controller_ip: String,
    },
    /// To a controller: the host accepted; bind a UDP socket with this token.
    SessionReady { session_id: SessionId, token: [u8; 16], udp_port: u16, host_name: String },
    /// To both peers once both have bound: how to reach the other side.
    PeerInfo {
        session_id: SessionId,
        /// The peer's address as seen by the server (its NAT mapping).
        public_addr: SocketAddr,
        /// The peer's LAN addresses (useful when both are behind the same NAT).
        local_addrs: Vec<SocketAddr>,
        /// SHA-256 of the host's QUIC certificate (sent to controllers only).
        cert_fingerprint: Option<[u8; 32]>,
    },
    /// Reply to [`ClientMsg::Login`]. On success, `token` can be saved and used
    /// in a later [`Credential::Token`] so the user needn't retype the password.
    LoginResult { ok: bool, token: String, display_name: String, is_admin: bool, message: String },
    /// Reply to [`ClientMsg::GetAddressBook`].
    AddressBook { entries: Vec<AddressEntry> },
    /// Something went wrong.
    Error { code: ErrorCode, message: String },
    /// Keep-alive reply.
    Pong,
}

/// Machine-readable error categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    BadRequest,
    VersionMismatch,
    /// The requested ID belongs to another device (secret mismatch).
    IdConflict,
    HostOffline,
    HostBusy,
    RateLimited,
    /// Login failed, or an account-only request arrived without a valid token.
    Unauthorized,
    Internal,
}
