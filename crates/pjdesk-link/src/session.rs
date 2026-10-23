//! Password authentication on a fresh QUIC connection (see
//! `pjdesk_proto::auth` for the cryptography).

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use pjdesk_proto::auth;
use pjdesk_proto::framing::{read_msg, write_msg, MAX_CONTROL_FRAME};
use pjdesk_proto::peer::AuthMsg;
use pjdesk_proto::{protocol_ok, version_rejected, PROTOCOL_VERSION};

use crate::lockout;
use quinn::{Connection, RecvStream, SendStream};
use tokio::time::timeout;

/// Returned when the password check fails (on either side).
#[derive(Debug)]
pub struct WrongPassword;

impl fmt::Display for WrongPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("wrong password")
    }
}

impl std::error::Error for WrongPassword {}

/// The host is refusing passwords for a while. The display text is
/// `locked out, try again in N minutes`.
#[derive(Debug)]
pub struct LockedOut {
    pub minutes: u32,
}

impl fmt::Display for LockedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&lockout::message(self.minutes))
    }
}

impl std::error::Error for LockedOut {}

/// The controller asked the host to prompt the person at the computer
/// instead of checking a password. The host has not sent a verdict yet.
#[derive(Debug)]
pub struct NeedsApproval {
    pub send: SendStream,
    pub recv: RecvStream,
    pub peer_name: String,
    pub peer_proto: u16,
    /// pjdesk.com account, when the controller is signed in. Empty for Ask the person.
    pub account: String,
    /// Controller computer name.
    pub computer: String,
    /// Controller device id, digits only. Empty when unknown.
    pub id: String,
    /// True for a view request. Ask the person leaves this false.
    pub view: bool,
}

impl fmt::Display for NeedsApproval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the controller asked the person at this computer")
    }
}

impl std::error::Error for NeedsApproval {}

/// The authenticated control stream plus the peer's display name.
pub struct Authenticated {
    pub send: SendStream,
    pub recv: RecvStream,
    pub peer_name: String,
    /// Protocol version the peer offered. Version 2 is the 0.3.x session.
    pub peer_proto: u16,
    /// Which host password matched. 0 is the one-time password; 1 is the permanent password.
    pub password_slot: u8,
}

/// TLS keying material unique to this QUIC connection (channel binding).
fn channel_binding(connection: &Connection) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    connection
        .export_keying_material(&mut out, auth::EXPORTER_LABEL, b"")
        .map_err(|_| anyhow!("TLS exporter unavailable"))?;
    Ok(out)
}

/// Number of password slots the controller offers — enough for the host's
/// permanent and one-time passwords. The controller uses its single typed
/// password in every slot; the host tests one of its passwords per slot.
const AUTH_SLOTS: usize = 2;
/// Long enough for an Allow prompt. A password verdict still returns as soon
/// as the host sends it.
const VERDICT_WAIT: Duration = Duration::from_secs(90);

fn auth_refusal(message: &str) -> anyhow::Error {
    if let Some(minutes) = lockout::parse_minutes(message) {
        return LockedOut { minutes }.into();
    }
    if message == "wrong password" || message.starts_with("wrong password") {
        return WrongPassword.into();
    }
    anyhow!("{}", message)
}

async fn read_refusal(recv: &mut RecvStream) -> Option<String> {
    match timeout(Duration::from_secs(2), read_msg::<_, AuthMsg>(recv, MAX_CONTROL_FRAME)).await {
        Ok(Ok(AuthMsg::Result { ok: false, message, .. })) => Some(message),
        _ => None,
    }
}

/// Tells the controller the password was accepted and the session may start.
pub async fn grant_auth(send: &mut SendStream, host_name: &str) -> Result<()> {
    write_msg(send, &AuthMsg::Result { ok: true, message: String::new(), host_name: host_name.into() }).await?;
    Ok(())
}

pub async fn refuse_auth(send: &mut SendStream, message: &str) -> Result<()> {
    write_msg(send, &AuthMsg::Result { ok: false, message: message.to_string(), host_name: String::new() }).await?;
    let _ = send.finish();
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(())
}

/// Answers a controller that has already opened the auth stream, without
/// checking a password. Used while the source is locked out.
pub async fn refuse_before_password(connection: &Connection, message: &str) -> Result<()> {
    let (mut send, mut recv) = timeout(Duration::from_secs(10), connection.accept_bi())
        .await
        .context("the controller did not start authentication")??;
    let _ = timeout(Duration::from_secs(10), read_msg::<_, AuthMsg>(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the controller did not start authentication")??;
    refuse_auth(&mut send, message).await
}

/// Controller side: prove we know one of the host's passwords.
pub async fn controller_auth(connection: &Connection, password: &str, my_name: &str) -> Result<Authenticated> {
    controller_auth_version(connection, password, my_name, PROTOCOL_VERSION).await
}

pub async fn controller_auth_version(
    connection: &Connection,
    password: &str,
    my_name: &str,
    proto: u16,
) -> Result<Authenticated> {
    let (mut send, mut recv) = connection.open_bi().await?;
    let binding = channel_binding(connection)?;

    // One independent SPAKE2 exchange per slot, all with the typed password.
    let mut exchanges = Vec::with_capacity(AUTH_SLOTS);
    let mut spakes = Vec::with_capacity(AUTH_SLOTS);
    for _ in 0..AUTH_SLOTS {
        let (exchange, spake) = auth::controller_start(password);
        exchanges.push(exchange);
        spakes.push(spake);
    }
    write_msg(&mut send, &AuthMsg::Start { proto, spakes, controller_name: my_name.into() }).await?;

    let reply: AuthMsg = timeout(VERDICT_WAIT, read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the host did not answer")??;
    let (host_spakes, host_confirms) = match reply {
        AuthMsg::Challenge { spakes, confirms } => (spakes, confirms),
        AuthMsg::Result { ok: false, message, .. } => {
            if version_rejected(&message) {
                bail!(message);
            }
            return Err(auth_refusal(&message));
        }
        other => bail!("unexpected message from the host: {other:?}"),
    };

    // Find the slot whose confirmation verifies: that's the host password our
    // typed password matched (permanent or one-time).
    let mut matched: Option<(usize, auth::SessionKey)> = None;
    for (i, exchange) in exchanges.into_iter().enumerate() {
        if i >= host_spakes.len() || i >= host_confirms.len() {
            break;
        }
        if let Ok(key) = exchange.finish(&host_spakes[i]) {
            if key.verify_host(&binding, &host_confirms[i]) {
                matched = Some((i, key));
                break;
            }
        }
    }
    let (slot, key) = match matched {
        Some(m) => m,
        None => {
            // Tell the host (so it counts the failed attempt), then read its
            // verdict. A lockout is not reported as a wrong password.
            let _ = write_msg(&mut send, &AuthMsg::Confirm { slot: 0xFF, confirm: [0; 32] }).await;
            if let Some(message) = read_refusal(&mut recv).await {
                return Err(auth_refusal(&message));
            }
            return Err(WrongPassword.into());
        }
    };
    write_msg(&mut send, &AuthMsg::Confirm { slot: slot as u8, confirm: key.controller_confirm(&binding) }).await?;

    let verdict: AuthMsg = timeout(VERDICT_WAIT, read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the host did not answer")??;
    match verdict {
        AuthMsg::Result { ok: true, host_name, .. } => Ok(Authenticated {
            send,
            recv,
            peer_name: host_name,
            peer_proto: proto,
            password_slot: slot as u8,
        }),
        AuthMsg::Result { ok: false, message, .. } => Err(auth_refusal(&message)),
        other => bail!("unexpected message from the host: {other:?}"),
    }
}

/// Ask the host to show "wants to view your screen" instead of checking a password.
///
/// `account` is empty when this computer is not signed in to pjdesk.com.
pub async fn controller_ask_view(connection: &Connection, account: &str, computer: &str, id: &str) -> Result<Authenticated> {
    let (mut send, mut recv) = connection.open_bi().await?;
    let account: String = account.trim().chars().filter(|c| !c.is_control()).take(80).collect();
    let computer: String = computer.trim().chars().filter(|c| !c.is_control()).take(64).collect();
    let id: String = id.chars().filter(|c| c.is_ascii_digit()).take(32).collect();
    write_msg(&mut send, &AuthMsg::AskView { proto: PROTOCOL_VERSION, account, computer, id }).await?;
    let verdict = match timeout(VERDICT_WAIT, read_msg::<_, AuthMsg>(&mut recv, MAX_CONTROL_FRAME)).await {
        Ok(Ok(msg)) => msg,
        Ok(Err(_)) => {
            bail!("The phone closed the view request. Update that phone to PJDesk 0.6.8, then try again.")
        }
        Err(_) => bail!("Timed out. No one at that phone answered the request to view the screen."),
    };
    match verdict {
        AuthMsg::Result { ok: true, host_name, .. } => Ok(Authenticated {
            send,
            recv,
            peer_name: host_name,
            peer_proto: PROTOCOL_VERSION,
            password_slot: 0,
        }),
        AuthMsg::Result { ok: false, message, .. } => Err(auth_refusal(&message)),
        other => bail!("unexpected message from the host: {other:?}"),
    }
}

/// Ask the person at the host instead of sending a password.
/// Password attempts keep [VERDICT_WAIT]. An ask uses a shorter wait.
pub async fn controller_ask_person(connection: &Connection, my_name: &str) -> Result<Authenticated> {
    controller_ask_person_for(connection, my_name, VERDICT_WAIT).await
}

pub async fn controller_ask_person_for(
    connection: &Connection,
    my_name: &str,
    wait: Duration,
) -> Result<Authenticated> {
    let (mut send, mut recv) = connection.open_bi().await?;
    write_msg(&mut send, &AuthMsg::AskPerson { proto: PROTOCOL_VERSION, controller_name: my_name.into() }).await?;
    let verdict: AuthMsg = timeout(wait, read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the host did not answer")??;
    match verdict {
        AuthMsg::Result { ok: true, host_name, .. } => Ok(Authenticated {
            send,
            recv,
            peer_name: host_name,
            peer_proto: PROTOCOL_VERSION,
            password_slot: 0,
        }),
        AuthMsg::Result { ok: false, message, .. } => Err(auth_refusal(&message)),
        other => bail!("unexpected message from the host: {other:?}"),
    }
}

/// Host side: check the controller's password proof.
///
/// `attempt_used` allows one password attempt per session: the controller
/// may race several connections, but only the first to start the exchange
/// gets a SPAKE2 reply (each reply lets the peer test exactly one guess).
pub async fn host_auth(
    connection: &Connection,
    passwords: &[String],
    _my_name: &str,
    attempt_used: &AtomicBool,
    failure_message: &(dyn Fn() -> String + Sync),
) -> Result<Authenticated> {
    let (mut send, mut recv) = timeout(Duration::from_secs(10), connection.accept_bi())
        .await
        .context("the controller did not start authentication")??;
    let start: AuthMsg = timeout(Duration::from_secs(10), read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the controller did not start authentication")??;
    let (proto, controller_spakes, controller_name) = match start {
        AuthMsg::AskPerson { proto, controller_name } => {
            if !protocol_ok(proto) {
                let message = format!("version mismatch (host speaks protocol {PROTOCOL_VERSION}, controller {proto})");
                let _ = refuse_auth(&mut send, &message).await;
                bail!(message);
            }
            if attempt_used.swap(true, Ordering::SeqCst) {
                bail!("only one password attempt is allowed per connection request");
            }
            return Err(NeedsApproval {
                send,
                recv,
                peer_name: controller_name.clone(),
                peer_proto: proto,
                account: String::new(),
                computer: controller_name,
                id: String::new(),
                view: false,
            }
            .into());
        }
        AuthMsg::AskView { proto, account, computer, id } => {
            if !protocol_ok(proto) {
                let message = format!("version mismatch (host speaks protocol {PROTOCOL_VERSION}, controller {proto})");
                let _ = refuse_auth(&mut send, &message).await;
                bail!(message);
            }
            if attempt_used.swap(true, Ordering::SeqCst) {
                bail!("only one password attempt is allowed per connection request");
            }
            let account: String = account.trim().chars().filter(|c| !c.is_control()).take(80).collect();
            let computer: String = computer.trim().chars().filter(|c| !c.is_control()).take(64).collect();
            let id: String = id.chars().filter(|c| c.is_ascii_digit()).take(32).collect();
            let peer_name = if !account.is_empty() { account.clone() } else if !computer.is_empty() { computer.clone() } else { id.clone() };
            return Err(NeedsApproval { send, recv, peer_name, peer_proto: proto, account, computer, id, view: true }.into());
        }
        AuthMsg::Start { proto, spakes, controller_name } => (proto, spakes, controller_name),
        other => bail!("unexpected message from the controller: {other:?}"),
    };
    if !protocol_ok(proto) {
        let message = format!("version mismatch (host speaks protocol {PROTOCOL_VERSION}, controller {proto})");
        let _ =
            write_msg(&mut send, &AuthMsg::Result { ok: false, message: message.clone(), host_name: String::new() })
                .await;
        bail!(message);
    }
    let peer_proto = proto;

    if attempt_used.swap(true, Ordering::SeqCst) {
        bail!("only one password attempt is allowed per connection request");
    }

    let binding = channel_binding(connection)?;
    // Run one SPAKE2 per password the host accepts, each against the matching
    // controller slot; the controller tells us which one its password verified.
    let n = passwords.len().min(controller_spakes.len());
    if n == 0 {
        bail!("the controller sent no authentication material");
    }
    let mut keys = Vec::with_capacity(n);
    let mut spakes = Vec::with_capacity(n);
    let mut confirms = Vec::with_capacity(n);
    for i in 0..n {
        let (exchange, spake) = auth::host_start(&passwords[i]);
        let key = exchange.finish(&controller_spakes[i])?;
        confirms.push(key.host_confirm(&binding));
        spakes.push(spake);
        keys.push(key);
    }
    write_msg(&mut send, &AuthMsg::Challenge { spakes, confirms }).await?;

    // From here on, anything but a valid confirmation counts as a failed
    // password attempt (the controller has already been able to test its guess).
    let (slot, confirm) =
        match timeout(Duration::from_secs(15), read_msg::<_, AuthMsg>(&mut recv, MAX_CONTROL_FRAME)).await {
            Ok(Ok(AuthMsg::Confirm { slot, confirm })) => (slot as usize, confirm),
            _ => return Err(WrongPassword.into()),
        };
    if slot >= keys.len() || !keys[slot].verify_controller(&binding, &confirm) {
        let message = failure_message();
        let _ = refuse_auth(&mut send, &message).await;
        // A lockout is its own result. The password was not accepted, and the
        // caller must not fall through to an Allow prompt.
        return Err(auth_refusal(&message));
    }
    // The caller sends the success verdict after it decides whether to ask
    // the person at this computer. A wrong password has already been answered.
    Ok(Authenticated { send, recv, peer_name: controller_name, peer_proto, password_slot: slot as u8 })
}
