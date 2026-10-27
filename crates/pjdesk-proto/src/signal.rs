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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Identifies one connection attempt between a controller and a host.
pub type SessionId = u64;

/// How often a registered computer pings. One missed reply means the server
/// stopped answering, so the next dial starts within a few seconds.
pub const HEARTBEAT: Duration = Duration::from_secs(2);
/// No reply for this long also means the server stopped answering.
pub const HEARTBEAT_SILENCE: Duration = Duration::from_secs(6);

/// Messages sent by clients (hosts and controllers) to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Host: register this device (or reclaim its ID) and go online.
    ///
    /// The secret proves ownership of the ID; the server stores only its hash.
    Register {
        proto: u16,
        device_id: Option<String>,
        device_secret: [u8; 32],
        hostname: String,
        app_version: String,
    },
    /// Controller: ask for a session with the device `target_id`.
    Connect {
        proto: u16,
        target_id: String,
        controller_name: String,
    },
    /// Host: answer to [`ServerMsg::IncomingSession`].
    SessionResponse {
        session_id: SessionId,
        accept: bool,
        reason: String,
    },
    /// Either peer: my UDP socket is bound (see [`crate::udp`]); here are my
    /// LAN candidate addresses. Hosts also send their QUIC certificate
    /// fingerprint so the controller can pin it.
    UdpReady {
        session_id: SessionId,
        local_addrs: Vec<SocketAddr>,
        cert_fingerprint: Option<[u8; 32]>,
    },
    /// Sign in to an account on the server, with a password or a saved token
    /// (see [`Credential`]). The reply is [`ServerMsg::LoginResult`].
    Login {
        proto: u16,
        username: String,
        credential: Credential,
        device_name: String,
    },
    /// Invalidate the saved token in [`Credential::Token`] (sign out everywhere
    /// is a server setting; this just drops the current one).
    Logout {
        token: String,
    },
    /// Fetch the signed-in account's address book ([`ServerMsg::AddressBook`]).
    GetAddressBook {
        token: String,
    },
    /// Replace the signed-in account's address book.
    SaveAddressBook {
        token: String,
        entries: Vec<AddressEntry>,
    },
    /// Keep-alive.
    Ping,
    /// Version 3. Operator or admin: the registered computers.
    ListDevices {
        token: String,
    },
    /// Version 3. Operator or admin: set or clear a computer's group.
    SetDeviceGroup {
        token: String,
        device_id: String,
        group: String,
    },
    /// Version 3. Any signed-in user may read the policy; only an admin may change it.
    GetPolicy {
        token: String,
    },
    SetPolicy {
        token: String,
        policy: Policy,
    },
    /// Version 3. Operator or admin: recent audit events.
    GetAudit {
        token: String,
        limit: u32,
    },
    /// Version 3. Admin: change an account's role.
    SetRole {
        token: String,
        username: String,
        role: Role,
    },
    /// Version 3. The signed-in user replaces their own password. The server
    /// drops saved tokens, so the client signs in again.
    ChangePassword {
        token: String,
        new_password: String,
    },
    /// Presents a pjdesk.com network access token. Sent only by 0.6.0+ clients
    /// that have one, before Register or Connect. Older servers cannot decode
    /// this variant and drop the connection; those clients retry once without it.
    PresentAccess {
        token: String,
    },
    /// 0.6.2+. Hostname, operating system, and the last interactive user.
    /// Sent only after [`ServerMsg::WantHostReport`]. The user name is stored
    /// for the account owner. It is not forwarded to other peers.
    ReportHost {
        hostname: String,
        os_family: String,
        os_version: String,
        console_user: String,
    },
    /// 0.6.2+. Friendly name for this computer. Sent only after
    /// [`ServerMsg::WantHostReport`]. An empty name clears the stored one
    /// so other computers show the hostname.
    ReportDisplayName {
        name: String,
    },
    /// 0.6.33+. The account this server is signed in to lists, creates, or
    /// revokes a server access key. A guest does not send this. An older
    /// server cannot decode it and drops the connection.
    ManageAccessKey {
        token: String,
        /// `list`, `create`, or `revoke`.
        op: String,
        #[serde(default)]
        label: String,
        /// `0`, one day, 7 days, or 30 days. `0` does not expire.
        #[serde(default)]
        expires_secs: u64,
        #[serde(default)]
        computer_limit: u32,
        #[serde(default)]
        key_id: String,
    },
    /// 0.6.47+. The host has told this relayed session to open again on the
    /// new primary. The server finishes stepping down after these arrive.
    RelayMoved {
        session_id: SessionId,
    },
    /// 0.6.47+. The current primary asks the standby to take the next
    /// generation. Sent only to the paired server, over the shared server
    /// certificate. An older server cannot decode it and drops the connection;
    /// the primary then stays primary.
    TakePrimary {
        epoch: u64,
        at: u64,
        proof: [u8; 32],
    },
    /// 0.6.47+. Register this install. `device_secret` is the legacy secret
    /// (Windows MachineGuid, or the stored random secret). `install_key` is
    /// random and stored with this install, so two cloned images do not share
    /// an ID. An older server cannot decode this and drops the connection;
    /// the client registers again with [`ClientMsg::Register`].
    RegisterInstall {
        proto: u16,
        device_id: Option<String>,
        device_secret: [u8; 32],
        install_key: [u8; 32],
        hostname: String,
        app_version: String,
    },
    /// 0.6.48+. Saved-list edits for the primary to upload in its next batch.
    /// Sent only after this server has sent [`ServerMsg::Fleet`]. An older
    /// server cannot decode it and would drop the connection.
    PublishSaved {
        computers: Vec<FleetComputer>,
        #[serde(default)]
        app_lock: String,
        #[serde(default)]
        app_lock_set: bool,
    },
    /// 0.6.48+. Ask this server to sign in, refresh a token, or read and write
    /// the saved list from its account copy. The password is not sent.
    /// `auth_key` is the same hex the cloud login uses, and only for
    /// `op = "sign-in"`. An older server cannot decode this and drops the
    /// connection; the client then uses pjdesk.com only when no server answers.
    Account {
        /// `sign-in`, `refresh`, `vault-read`, `vault-write`, `lock-read`,
        /// `lock-write`, `saved-write`, or `replica`.
        op: String,
        #[serde(default)]
        email: String,
        #[serde(default)]
        auth_key: String,
        #[serde(default)]
        refresh: String,
        #[serde(default)]
        device_id: String,
        #[serde(default)]
        device_name: String,
        #[serde(default)]
        vault: String,
        #[serde(default)]
        app_lock: String,
        #[serde(default)]
        computers: Vec<FleetComputer>,
        /// Proves a paired server for `replica`. Empty for a person signing in.
        #[serde(default)]
        proof: String,
    },
    /// 0.6.50+. Turn incoming-only on or off for this computer. Sent only after
    /// [`ServerMsg::IncomingOnly`]. `password` is the account owner's password
    /// and is required to turn it off. It is not written to a log.
    SetIncomingOnly {
        on: bool,
        password: String,
    },
}

/// Shown when this computer is not allowed to start a session.
pub const INCOMING_ONLY_REFUSAL: &str = "Incoming only is on. This computer can't connect to others.";

/// The checkbox and the server-window button.
pub const INCOMING_ONLY_LABEL: &str = "Incoming only (this computer can't connect to others)";

/// What the server said, against what this computer already stored.
/// The second value is true when this computer should tell the server it is on.
/// A first `on: false` from the server does not unlock a lock this computer
/// already has. A later `on: false`, after this computer has spoken, does.
pub fn reconcile_incoming_only(local_on: bool, server_on: bool, told_server: bool) -> (bool, bool) {
    if server_on {
        (true, false)
    } else if local_on && !told_server {
        (true, true)
    } else {
        (false, false)
    }
}

/// Turning incoming-only on does not need a password. Turning it off does.
pub fn incoming_only_change(want_on: bool, currently_on: bool, password_ok: bool) -> Result<(), &'static str> {
    if want_on || !currently_on || password_ok {
        Ok(())
    } else {
        Err("Enter the account owner's password to allow this computer to connect to others.")
    }
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
    /// 0.6.2+. Asks a host that understands it to send [`ClientMsg::ReportHost`].
    /// A server sends this only when the host's app version can decode it.
    WantHostReport,
    /// 0.6.33+. Reply to [`ClientMsg::ManageAccessKey`]. `secret` is set only
    /// on create, and only in this reply. It is not stored.
    AccessKeys {
        ok: bool,
        message: String,
        #[serde(default)]
        secret: String,
        #[serde(default)]
        keys: Vec<AccessKeyBrief>,
    },
    /// 0.6.45+. The public and LAN addresses of both servers. Sent after
    /// registration, and again when an address changes. Older clients are not
    /// sent this variant.
    Routes(crate::routes::ServerRoutes),
    /// 0.6.47+. This server is leaving the primary role. A relayed session
    /// opens again on `primary` now, before the role change finishes.
    /// Sent only to a client whose version can decode it.
    MoveRelay {
        session_id: SessionId,
        primary: String,
    },
    /// 0.6.47+. Reply to [`ClientMsg::TakePrimary`]. Sent only to the paired
    /// server. Hosts and phones are not sent this variant.
    Handoff {
        ok: bool,
        epoch: u64,
        name: String,
        message: String,
    },
    /// 0.6.48+. Who is online, their versions, and the saved list the primary
    /// last fetched. Sent only to a client whose version can decode it.
    /// Older clients are not sent this variant.
    Fleet {
        presence: Vec<FleetPeer>,
        computers: Vec<FleetComputer>,
        #[serde(default)]
        removed: Vec<String>,
        #[serde(default)]
        app_lock: String,
        #[serde(default)]
        app_lock_updated: u64,
    },
    /// 0.6.48+. Reply to [`ClientMsg::Account`]. Tokens are issued on this
    /// server. `vault` is ciphertext.
    AccountResult {
        ok: bool,
        #[serde(default)]
        op: String,
        #[serde(default)]
        message: String,
        #[serde(default)]
        access: String,
        #[serde(default)]
        exp: u64,
        #[serde(default)]
        refresh: String,
        #[serde(default)]
        email: String,
        #[serde(default)]
        role: String,
        #[serde(default)]
        device_id: String,
        #[serde(default)]
        vault: String,
        #[serde(default)]
        vault_updated: u64,
        #[serde(default)]
        app_lock: String,
        #[serde(default)]
        app_lock_updated: u64,
        #[serde(default)]
        computers: Vec<FleetComputer>,
        #[serde(default)]
        removed: Vec<String>,
        #[serde(default)]
        pause_secs: u64,
        #[serde(default)]
        active_address: String,
        #[serde(default)]
        active_fingerprint: String,
        #[serde(default)]
        standby_address: String,
        #[serde(default)]
        standby_fingerprint: String,
        #[serde(default)]
        active_lan: String,
        #[serde(default)]
        standby_lan: String,
        #[serde(default)]
        active_public_ip: String,
        #[serde(default)]
        standby_public_ip: String,
    },
    /// 0.6.48+. The account copy, for the paired standby only. Not sent to
    /// hosts or phones. `body` is JSON without failed-sign-in counters.
    Replica {
        revision: String,
        body: Vec<u8>,
    },
    /// 0.6.48+. The release the primary last fetched, including installer URLs.
    /// Clients do not call pjdesk.com for this.
    Release {
        version: String,
        #[serde(default)]
        client_url: String,
        #[serde(default)]
        server_url: String,
        #[serde(default)]
        sums_url: String,
        #[serde(default)]
        notes: String,
        #[serde(default)]
        checked_at: u64,
    },
    /// 0.6.50+. This computer may only accept sessions, or it may connect out
    /// again. Sent only to a client whose version can decode it. Older clients
    /// are not sent this variant.
    IncomingOnly { on: bool },
}

/// One computer the primary can see, for the Saved list.
/// `online` is the live registration, not a cloud poll.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetPeer {
    pub id: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub last_seen: u64,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub os_family: String,
    #[serde(default)]
    pub os_version: String,
    #[serde(default)]
    pub console_user: String,
}

/// One saved-list row. Passwords are not included.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetComputer {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os_family: String,
    #[serde(default)]
    pub os_version: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub group: String,
}

/// One server access key, safe to list. The secret and its hash are not here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessKeyBrief {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub created_at: u64,
    /// Unix time, or 0 when the key does not expire.
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub computer_limit: u32,
    /// Unix time, or 0 while the key is still active.
    #[serde(default)]
    pub revoked_at: u64,
    #[serde(default)]
    pub computers: u32,
    #[serde(default)]
    pub names: Vec<String>,
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

/// Sent when a standby is asked to register a computer or start a session.
/// The standby does not keep its own live table. A 0.6.31 client connects to
/// the primary instead. The text stays on the existing error message so an
/// older client can still decode it.
pub const STANDBY_REGISTRATION: &str = "This server is the standby. Connect to the primary.";

pub fn is_standby_refusal(message: &str) -> bool {
    message.contains("This server is the standby")
}

/// A standby tells the client where the primary is, and sends the full catalog.
/// The first sentence stays recognizable to an older client. The `routes` line
/// is for 0.6.45.
pub fn standby_redirect(primary: &str, routes: &crate::routes::ServerRoutes) -> String {
    let primary = crate::routes::canonical(primary);
    let head = if primary.is_empty() {
        STANDBY_REGISTRATION.to_string()
    } else {
        format!("This server is the standby. Connect to the primary at {primary}.")
    };
    let line = routes_line(routes);
    if line.is_empty() {
        head
    } else {
        format!("{head}\n{line}")
    }
}

/// The primary address from a standby reply, plus the catalog when one was sent.
pub fn parse_standby_redirect(message: &str) -> Option<(String, crate::routes::ServerRoutes)> {
    if !is_standby_refusal(message) {
        return None;
    }
    let mut primary = String::new();
    if let Some(rest) = message.split("Connect to the primary at ").nth(1) {
        let addr = rest.split([' ', '\n', '\r']).next().unwrap_or("").trim().trim_end_matches('.');
        if !addr.is_empty() {
            primary = crate::routes::canonical(addr);
        }
    }
    let mut routes = crate::routes::ServerRoutes::default();
    for line in message.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("routes ") {
            routes = parse_routes_line(rest);
        }
    }
    if primary.is_empty() {
        primary = routes.primary_public.first().cloned().or_else(|| routes.primary_lan.first().cloned()).unwrap_or_default();
    }
    Some((primary, routes))
}

fn routes_line(routes: &crate::routes::ServerRoutes) -> String {
    if routes.is_empty() {
        return String::new();
    }
    format!(
        "routes primary_lan={} primary_public={} standby_lan={} standby_public={} names={}",
        join_endpoints(&routes.primary_lan),
        join_endpoints(&routes.primary_public),
        join_endpoints(&routes.standby_lan),
        join_endpoints(&routes.standby_public),
        join_endpoints(&routes.names),
    )
}

fn join_endpoints(list: &[String]) -> String {
    if list.is_empty() {
        "-".into()
    } else {
        list.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(",")
    }
}

fn parse_routes_line(rest: &str) -> crate::routes::ServerRoutes {
    let mut routes = crate::routes::ServerRoutes::default();
    for part in rest.split_whitespace() {
        let Some((key, value)) = part.split_once('=') else { continue };
        let list = split_endpoints(value);
        match key {
            "primary_lan" => routes.primary_lan = list,
            "primary_public" => routes.primary_public = list,
            "standby_lan" => routes.standby_lan = list,
            "standby_public" => routes.standby_public = list,
            "names" => routes.names = list,
            _ => {}
        }
    }
    routes
}

fn split_endpoints(value: &str) -> Vec<String> {
    if value == "-" {
        return Vec::new();
    }
    value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string()).collect()
}

/// Backoff after a refused signaling connection: 1s, 2s, 4s, 8s, then 10s.
pub fn next_reconnect_delay(current: Duration) -> Duration {
    let next = current.saturating_mul(2);
    if next < Duration::from_secs(1) {
        Duration::from_secs(1)
    } else if next > Duration::from_secs(10) {
        Duration::from_secs(10)
    } else {
        next
    }
}

/// Spreads reconnects so every computer does not dial on the same second.
pub fn jitter_reconnect(base: Duration) -> Duration {
    let ms = base.as_millis() as u64;
    let swing = ms / 5;
    if swing == 0 {
        return base;
    }
    let tick = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64).unwrap_or(0);
    let delta = (tick % (swing * 2 + 1)) as i64 - swing as i64;
    Duration::from_millis((ms as i64 + delta).clamp(200, 10_000) as u64)
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
        let access = postcard::to_stdvec(&ClientMsg::PresentAccess { token: "t".into() }).unwrap();
        assert_eq!(access[0], 16, "PresentAccess is appended after ChangePassword");
        let report = postcard::to_stdvec(&ClientMsg::ReportHost {
            hostname: "PC".into(),
            os_family: "windows".into(),
            os_version: "11".into(),
            console_user: "BEN".into(),
        })
        .unwrap();
        assert_eq!(report[0], 17, "ReportHost is appended after PresentAccess");
        let named = postcard::to_stdvec(&ClientMsg::ReportDisplayName { name: "Cabin".into() }).unwrap();
        assert_eq!(named[0], 18, "ReportDisplayName is appended after ReportHost");
        let ask = postcard::to_stdvec(&ServerMsg::WantHostReport).unwrap();
        assert_eq!(ask[0], 12, "WantHostReport is appended after Audit");
        let manage = postcard::to_stdvec(&ClientMsg::ManageAccessKey {
            token: "t".into(),
            op: "list".into(),
            label: String::new(),
            expires_secs: 0,
            computer_limit: 1,
            key_id: String::new(),
        })
        .unwrap();
        assert_eq!(manage[0], 19, "ManageAccessKey is appended after ReportDisplayName");
        let keys = postcard::to_stdvec(&ServerMsg::AccessKeys {
            ok: true,
            message: String::new(),
            secret: String::new(),
            keys: vec![AccessKeyBrief { id: "ab".into(), label: "Ada".into(), computer_limit: 1, ..AccessKeyBrief::default() }],
        })
        .unwrap();
        assert_eq!(keys[0], 13, "AccessKeys is appended after WantHostReport");
        let routes = postcard::to_stdvec(&ServerMsg::Routes(crate::routes::ServerRoutes::default())).unwrap();
        assert_eq!(routes[0], 14, "Routes is appended after AccessKeys");
        let moved = postcard::to_stdvec(&ClientMsg::RelayMoved { session_id: 1 }).unwrap();
        let take = postcard::to_stdvec(&ClientMsg::TakePrimary { epoch: 6, at: 1, proof: [9; 32] }).unwrap();
        assert_eq!(take[0], moved[0] + 1, "TakePrimary is appended after RelayMoved");
        let install = postcard::to_stdvec(&ClientMsg::RegisterInstall {
            proto: 3,
            device_id: None,
            device_secret: [1u8; 32],
            install_key: [2u8; 32],
            hostname: "PC".into(),
            app_version: "0.6.46".into(),
        })
        .unwrap();
        assert_eq!(install[0], take[0] + 1, "RegisterInstall is appended after TakePrimary");
        let move_relay = postcard::to_stdvec(&ServerMsg::MoveRelay { session_id: 1, primary: "x".into() }).unwrap();
        let handoff = postcard::to_stdvec(&ServerMsg::Handoff {
            ok: true,
            epoch: 7,
            name: "CHIP20".into(),
            message: String::new(),
        })
        .unwrap();
        assert_eq!(handoff[0], move_relay[0] + 1, "Handoff is appended after MoveRelay");
        let fleet = postcard::to_stdvec(&ServerMsg::Fleet {
            presence: Vec::new(),
            computers: Vec::new(),
            removed: Vec::new(),
            app_lock: String::new(),
            app_lock_updated: 0,
        })
        .unwrap();
        assert_eq!(fleet[0], handoff[0] + 1, "Fleet is appended after Handoff");
        let saved = postcard::to_stdvec(&ClientMsg::PublishSaved {
            computers: Vec::new(),
            app_lock: String::new(),
            app_lock_set: false,
        })
        .unwrap();
        assert_eq!(saved[0], install[0] + 1, "PublishSaved is appended after RegisterInstall");
        let account = postcard::to_stdvec(&ClientMsg::Account {
            op: "sign-in".into(),
            email: "a@b.co".into(),
            auth_key: String::new(),
            refresh: String::new(),
            device_id: "d".into(),
            device_name: String::new(),
            vault: String::new(),
            app_lock: String::new(),
            computers: Vec::new(),
            proof: String::new(),
        })
        .unwrap();
        assert_eq!(account[0], saved[0] + 1, "Account is appended after PublishSaved");
        let release = postcard::to_stdvec(&ServerMsg::Release {
            version: "0.6.48".into(),
            client_url: String::new(),
            server_url: String::new(),
            sums_url: String::new(),
            notes: String::new(),
            checked_at: 1,
        })
        .unwrap();
        let replica = postcard::to_stdvec(&ServerMsg::Replica { revision: "1".into(), body: Vec::new() }).unwrap();
        let result = postcard::to_stdvec(&ServerMsg::AccountResult {
            ok: true,
            op: "sign-in".into(),
            message: String::new(),
            access: String::new(),
            exp: 1,
            refresh: String::new(),
            email: String::new(),
            role: "owner".into(),
            device_id: String::new(),
            vault: String::new(),
            vault_updated: 0,
            app_lock: String::new(),
            app_lock_updated: 0,
            computers: Vec::new(),
            removed: Vec::new(),
            pause_secs: 0,
            active_address: String::new(),
            active_fingerprint: String::new(),
            standby_address: String::new(),
            standby_fingerprint: String::new(),
            active_lan: String::new(),
            standby_lan: String::new(),
            active_public_ip: String::new(),
            standby_public_ip: String::new(),
        })
        .unwrap();
        assert_eq!(result[0], fleet[0] + 1, "AccountResult is appended after Fleet");
        assert_eq!(replica[0], result[0] + 1, "Replica is appended after AccountResult");
        assert_eq!(release[0], replica[0] + 1, "Release is appended after Replica");
        let set = postcard::to_stdvec(&ClientMsg::SetIncomingOnly { on: true, password: String::new() }).unwrap();
        assert_eq!(set[0], account[0] + 1, "SetIncomingOnly is appended after Account");
        let only = postcard::to_stdvec(&ServerMsg::IncomingOnly { on: true }).unwrap();
        assert_eq!(only[0], release[0] + 1, "IncomingOnly is appended after Release");
        assert_eq!(INCOMING_ONLY_REFUSAL, "Incoming only is on. This computer can't connect to others.");
        assert_eq!(reconcile_incoming_only(true, false, false), (true, true));
        assert_eq!(reconcile_incoming_only(true, false, true), (false, false));
        assert_eq!(reconcile_incoming_only(false, true, false), (true, false));
        assert!(incoming_only_change(false, true, false).is_err());
        assert!(incoming_only_change(true, false, false).is_ok());
        assert!(incoming_only_change(false, true, true).is_ok());
        let back: ClientMsg = postcard::from_bytes(&ping).unwrap();
        assert!(matches!(back, ClientMsg::Ping));
        match postcard::from_bytes::<ServerMsg>(&keys).unwrap() {
            ServerMsg::AccessKeys { keys, .. } => assert_eq!(keys[0].label, "Ada"),
            other => panic!("{other:?}"),
        }
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

    #[test]
    fn reconnect_backoff_caps_at_ten_seconds() {
        assert!(is_standby_refusal(STANDBY_REGISTRATION));
        assert!(!is_standby_refusal("device 902 488 771 is not online"));
        let routes = crate::routes::ServerRoutes {
            primary_public: vec!["204.248.122.44:21200".into()],
            primary_lan: vec!["192.168.0.20:21200".into()],
            standby_public: vec!["198.51.100.8:21200".into()],
            ..crate::routes::ServerRoutes::default()
        };
        let text = standby_redirect("204.248.122.44:21200", &routes);
        assert!(is_standby_refusal(&text));
        let (addr, parsed) = parse_standby_redirect(&text).unwrap();
        assert_eq!(addr, "204.248.122.44:21200");
        assert_eq!(parsed.primary_lan, vec!["192.168.0.20:21200".to_string()]);
        let (old, empty) = parse_standby_redirect(STANDBY_REGISTRATION).unwrap();
        assert!(old.is_empty());
        assert!(empty.is_empty());
        let mut delay = Duration::from_secs(1);
        let mut seen = vec![delay];
        for _ in 0..6 {
            delay = next_reconnect_delay(delay);
            seen.push(delay);
        }
        assert_eq!(
            seen,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(10),
                Duration::from_secs(10),
                Duration::from_secs(10),
            ]
        );
        let paused = jitter_reconnect(Duration::from_secs(4));
        assert!(paused >= Duration::from_millis(3200) && paused <= Duration::from_millis(4800), "{paused:?}");
    }
}
