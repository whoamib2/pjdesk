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

/// The authenticated control stream plus the peer's display name.
pub struct Authenticated {
    pub send: SendStream,
    pub recv: RecvStream,
    pub peer_name: String,
    /// Protocol version the peer offered. Version 2 is the 0.3.x session.
    pub peer_proto: u16,
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

    let reply: AuthMsg = timeout(Duration::from_secs(15), read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the host did not answer")??;
    let (host_spakes, host_confirms) = match reply {
        AuthMsg::Challenge { spakes, confirms } => (spakes, confirms),
        AuthMsg::Result { ok: false, message, .. } => {
            if version_rejected(&message) {
                bail!(message);
            }
            bail!("the host refused the connection: {message}");
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
            // Tell the host (so it counts the failed attempt), then give up.
            let _ = write_msg(&mut send, &AuthMsg::Confirm { slot: 0xFF, confirm: [0; 32] }).await;
            let _ = send.finish();
            return Err(WrongPassword.into());
        }
    };
    write_msg(&mut send, &AuthMsg::Confirm { slot: slot as u8, confirm: key.controller_confirm(&binding) }).await?;

    let verdict: AuthMsg = timeout(Duration::from_secs(15), read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the host did not answer")??;
    match verdict {
        AuthMsg::Result { ok: true, host_name, .. } => {
            Ok(Authenticated { send, recv, peer_name: host_name, peer_proto: proto })
        }
        AuthMsg::Result { ok: false, message, .. } => bail!("the host refused the connection: {message}"),
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
    my_name: &str,
    attempt_used: &AtomicBool,
) -> Result<Authenticated> {
    let (mut send, mut recv) = timeout(Duration::from_secs(10), connection.accept_bi())
        .await
        .context("the controller did not start authentication")??;
    let start: AuthMsg = timeout(Duration::from_secs(10), read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the controller did not start authentication")??;
    let (proto, controller_spakes, controller_name) = match start {
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
        let _ = write_msg(
            &mut send,
            &AuthMsg::Result { ok: false, message: "wrong password".into(), host_name: String::new() },
        )
        .await;
        let _ = send.finish();
        // Give the message a moment to be delivered before the connection drops.
        tokio::time::sleep(Duration::from_millis(200)).await;
        return Err(WrongPassword.into());
    }
    write_msg(&mut send, &AuthMsg::Result { ok: true, message: String::new(), host_name: my_name.into() }).await?;
    Ok(Authenticated { send, recv, peer_name: controller_name, peer_proto })
}
