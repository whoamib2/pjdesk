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
    /// Version 3. Operator or admin: the registered computers.
    ListDevices { token: String },
    /// Version 3. Operator or admin: set or clear a computer's group.
    SetDeviceGroup { token: String, device_id: String, group: String },
    /// Version 3. Any signed-in user may read the policy; only an admin may change it.
    GetPolicy { token: String },
    SetPolicy { token: String, policy: Policy },
    /// Version 3. Operator or admin: recent audit events.
    GetAudit { token: String, limit: u32 },
    /// Version 3. Admin: change an account's role.
    SetRole { token: String, username: String, role: Role },
    /// Version 3. The signed-in user replaces their own password. The server
    /// drops saved tokens, so the client signs in again.
    ChangePassword { token: String, new_password: String },
}

/// What a signed-in account is allowed to do on the server.
///
/// Stored next to the account. A file written by 0.3.x has no role; an admin
/// flag still means [`Role::Admin`] and everyone else is [`Role::User`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    User,
    Operator,
    Admin,
}

impl Role {
    pub fn is_admin(self) -> bool {
        matches!(self, Role::Admin)
    }

    pub fn can_manage_devices(self) -> bool {
        matches!(self, Role::Operator | Role::Admin)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }
}

/// Rules the server pushes to hosts. A host applies them as a ceiling on
/// whatever the person at the computer allows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// A permanent password may accept a session when nobody is there to click.
    #[serde(default = "yes")]
    pub unattended_allowed: bool,
    /// Always show the approval prompt when someone is signed in at the console.
    #[serde(default)]
    pub force_prompt: bool,
    /// Bits a session is allowed to grant. Missing on old files → everything.
    #[serde(default = "all_perms")]
    pub max_permissions: u32,
    /// TCP ports a tunnel may target. Empty means no forwards.
    #[serde(default)]
    pub tunnel_ports: Vec<u16>,
    #[serde(default = "yes")]
    pub recording_allowed: bool,
    /// When false, a forward may only target loopback on the host.
    #[serde(default)]
    pub tunnel_remote_hosts: bool,
}

fn yes() -> bool {
    true
}
fn all_perms() -> u32 {
    crate::perms::ALL
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            unattended_allowed: true,
            force_prompt: false,
            max_permissions: crate::perms::ALL,
            tunnel_ports: Vec::new(),
            recording_allowed: true,
            tunnel_remote_hosts: false,
        }
    }
}

/// One computer, for the management window and `ListDevices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceBrief {
    pub id: String,
    pub hostname: String,
    #[serde(default)]
    pub group: String,
    pub online: bool,
    #[serde(default)]
    pub version: String,
    pub last_seen: u64,
}

/// One line of the server audit log. Passwords and tokens are never written here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub at: u64,
    pub kind: String,
    pub actor: String,
    pub detail: String,
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
    /// the user's own account on their own server. The file is locked to
    /// SYSTEM and Administrators; the password itself is not separately
    /// encrypted (the server has to hand it back to the signed-in client).
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
    /// To a host: run your update check now (the server operator asked for it).
    /// Added in 0.3.3; older clients never receive it.
    UpdateNow,
    /// Version 3. The server's policy. Sent to a host after it registers, and
    /// in reply to [`ClientMsg::GetPolicy`] / [`ClientMsg::SetPolicy`].
    Policy(Policy),
    /// Version 3. Reply to [`ClientMsg::ListDevices`].
    DeviceList { devices: Vec<DeviceBrief> },
    /// Version 3. Reply to [`ClientMsg::GetAudit`].
    Audit { events: Vec<AuditEvent> },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version2_messages_keep_their_indexes() {
        let ping = postcard::to_stdvec(&ClientMsg::Ping).unwrap();
        assert_eq!(ping[0], 8);
        let update = postcard::to_stdvec(&ServerMsg::UpdateNow).unwrap();
        assert_eq!(update[0], 8);
        let back: ClientMsg = postcard::from_bytes(&ping).unwrap();
        assert!(matches!(back, ClientMsg::Ping));
    }

    #[test]
    fn version3_server_messages_roundtrip() {
        let policy = Policy::default();
        let msg = ServerMsg::Policy(policy.clone());
        let bytes = postcard::to_stdvec(&msg).unwrap();
        match postcard::from_bytes(&bytes).unwrap() {
            ServerMsg::Policy(p) => assert_eq!(p, policy),
            other => panic!("{other:?}"),
        }
        let audit = ServerMsg::Audit {
            events: vec![AuditEvent { at: 1, kind: "login".into(), actor: "ada".into(), detail: "ok".into() }],
        };
        let bytes = postcard::to_stdvec(&audit).unwrap();
        assert!(matches!(postcard::from_bytes::<ServerMsg>(&bytes).unwrap(), ServerMsg::Audit { .. }));
    }

    #[test]
    fn old_policy_defaults_allow_unattended() {
        let p: Policy = serde_json::from_str("{}").unwrap();
        assert!(p.unattended_allowed);
        assert_eq!(p.max_permissions, crate::perms::ALL);
        assert!(!p.force_prompt);
        assert!(p.tunnel_ports.is_empty());
    }
}
