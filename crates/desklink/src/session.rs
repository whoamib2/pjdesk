//! Password authentication on a fresh QUIC connection (see
//! `desklink_proto::auth` for the cryptography).

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use desklink_proto::auth;
use desklink_proto::framing::{read_msg, write_msg, MAX_CONTROL_FRAME};
use desklink_proto::peer::AuthMsg;
use desklink_proto::PROTOCOL_VERSION;
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
}

/// TLS keying material unique to this QUIC connection (channel binding).
fn channel_binding(connection: &Connection) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    connection
        .export_keying_material(&mut out, auth::EXPORTER_LABEL, b"")
        .map_err(|_| anyhow!("TLS exporter unavailable"))?;
    Ok(out)
}

/// Controller side: prove we know the host's password.
pub async fn controller_auth(connection: &Connection, password: &str, my_name: &str) -> Result<Authenticated> {
    let (mut send, mut recv) = connection.open_bi().await?;
    let binding = channel_binding(connection)?;
    let (exchange, spake) = auth::controller_start(password);
    write_msg(&mut send, &AuthMsg::Start { proto: PROTOCOL_VERSION, spake, controller_name: my_name.into() }).await?;

    let reply: AuthMsg = timeout(Duration::from_secs(15), read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the host did not answer")??;
    let (host_spake, host_confirm) = match reply {
        AuthMsg::Challenge { spake, confirm } => (spake, confirm),
        AuthMsg::Result { ok: false, message, .. } => bail!("the host refused the connection: {message}"),
        other => bail!("unexpected message from the host: {other:?}"),
    };
    let key = exchange.finish(&host_spake)?;
    if !key.verify_host(&binding, &host_confirm) {
        // Tell the host (so it counts the failed attempt), then give up.
        let _ = write_msg(&mut send, &AuthMsg::Confirm { confirm: [0; 32] }).await;
        let _ = send.finish();
        return Err(WrongPassword.into());
    }
    write_msg(&mut send, &AuthMsg::Confirm { confirm: key.controller_confirm(&binding) }).await?;

    let verdict: AuthMsg = timeout(Duration::from_secs(15), read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the host did not answer")??;
    match verdict {
        AuthMsg::Result { ok: true, host_name, .. } => Ok(Authenticated { send, recv, peer_name: host_name }),
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
    password: &str,
    my_name: &str,
    attempt_used: &AtomicBool,
) -> Result<Authenticated> {
    let (mut send, mut recv) = timeout(Duration::from_secs(10), connection.accept_bi())
        .await
        .context("the controller did not start authentication")??;
    let start: AuthMsg = timeout(Duration::from_secs(10), read_msg(&mut recv, MAX_CONTROL_FRAME))
        .await
        .context("the controller did not start authentication")??;
    let (proto, controller_spake, controller_name) = match start {
        AuthMsg::Start { proto, spake, controller_name } => (proto, spake, controller_name),
        other => bail!("unexpected message from the controller: {other:?}"),
    };
    if proto != PROTOCOL_VERSION {
        let message = format!("version mismatch (host speaks protocol {PROTOCOL_VERSION}, controller {proto})");
        let _ =
            write_msg(&mut send, &AuthMsg::Result { ok: false, message: message.clone(), host_name: String::new() })
                .await;
        bail!(message);
    }

    if attempt_used.swap(true, Ordering::SeqCst) {
        bail!("only one password attempt is allowed per connection request");
    }

    let binding = channel_binding(connection)?;
    let (exchange, spake) = auth::host_start(password);
    let key = exchange.finish(&controller_spake)?;
    write_msg(&mut send, &AuthMsg::Challenge { spake, confirm: key.host_confirm(&binding) }).await?;

    // From here on, anything but a valid confirmation counts as a failed
    // password attempt (the controller has already been able to test its guess).
    let confirm = match timeout(Duration::from_secs(15), read_msg::<_, AuthMsg>(&mut recv, MAX_CONTROL_FRAME)).await {
        Ok(Ok(AuthMsg::Confirm { confirm })) => confirm,
        _ => return Err(WrongPassword.into()),
    };
    if !key.verify_controller(&binding, &confirm) {
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
    Ok(Authenticated { send, recv, peer_name: controller_name })
}
