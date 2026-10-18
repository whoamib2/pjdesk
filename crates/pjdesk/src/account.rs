//! Signing in to a PJDesk server account, and syncing the address book.
//!
//! The account layer is a thin request/response exchange on the ordinary
//! signaling connection (TLS 1.3, server key pinned). A successful sign-in
//! returns a token that is saved — encrypted — in the client config, so the
//! password is typed once.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use pjdesk_proto::signal::{AddressEntry, ClientMsg, Credential, ServerMsg};
use pjdesk_proto::PROTOCOL_VERSION;
use tokio::time::timeout;

use crate::config::{Account, Config};
use crate::signaling::{ServerAddr, SignalClient};

/// Result of a sign-in attempt.
pub struct LoggedIn {
    pub display_name: String,
    pub is_admin: bool,
    pub address_book: Vec<AddressEntry>,
    /// Set when an older password still works but should be replaced.
    pub notice: String,
}

async fn open(server: &ServerAddr, config: &mut Config) -> Result<SignalClient> {
    SignalClient::connect(server, config).await.context("connecting to the server")
}

/// Signs in with a username and password, saving the account + token to config.
pub async fn login(server: &ServerAddr, config: &mut Config, username: &str, password: &str) -> Result<LoggedIn> {
    let mut signal = open(server, config).await?;
    signal.send(ClientMsg::Login {
        proto: PROTOCOL_VERSION,
        username: username.to_string(),
        credential: Credential::Password(password.to_string()),
        device_name: crate::hostname(),
    })?;
    let result = finish_login(&mut signal, username, config).await?;
    Ok(result)
}

/// Re-validates the saved token (called at startup). Clears the account if the
/// token was rejected; returns `Ok(None)` if there is simply no account.
pub async fn refresh(server: &ServerAddr, config: &mut Config) -> Result<Option<LoggedIn>> {
    let (username, token) = match (&config.account, config.account_token()) {
        (Some(acct), Some(token)) => (acct.username.clone(), token),
        _ => return Ok(None),
    };
    let mut signal = open(server, config).await?;
    signal.send(ClientMsg::Login {
        proto: PROTOCOL_VERSION,
        username: username.clone(),
        credential: Credential::Token(token),
        device_name: crate::hostname(),
    })?;
    match finish_login(&mut signal, &username, config).await {
        Ok(result) => Ok(Some(result)),
        Err(e) => {
            // Token no longer valid: drop it so the UI prompts for a password.
            config.account = None;
            config.reload_identity_from_disk();
            let _ = config.save();
            Err(e)
        }
    }
}

async fn finish_login(signal: &mut SignalClient, username: &str, config: &mut Config) -> Result<LoggedIn> {
    let reply = match timeout(Duration::from_secs(15), signal.recv()).await {
        Ok(Some(msg)) => msg,
        Ok(None) => bail!("the server closed the connection"),
        Err(_) => bail!("the server did not answer the sign-in"),
    };
    let (token, display_name, is_admin, notice) = match reply {
        ServerMsg::LoginResult { ok: true, token, display_name, is_admin, message } => {
            (token, display_name, is_admin, message)
        }
        ServerMsg::LoginResult { ok: false, message, .. } => bail!("{message}"),
        ServerMsg::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected reply from the server: {other:?}"),
    };
    config.account = Some(Account::new(username.to_string(), display_name.clone(), &token, is_admin));

    // Pull the address book in the same session.
    signal.send(ClientMsg::GetAddressBook { token: token.clone() })?;
    let entries = match timeout(Duration::from_secs(10), signal.recv()).await {
        Ok(Some(ServerMsg::AddressBook { entries })) => entries,
        _ => config.address_book.clone(), // keep the cached copy on any hiccup
    };
    config.address_book = entries.clone();
    // Don't clobber the device identity the host thread may have just written.
    config.reload_identity_from_disk();
    config.save()?;
    Ok(LoggedIn { display_name, is_admin, address_book: entries, notice })
}

/// Replaces the signed-in account's password, then signs in again with it.
pub async fn change_password(server: &ServerAddr, config: &mut Config, new_password: &str) -> Result<LoggedIn> {
    let username = config.account.as_ref().map(|a| a.username.clone()).context("not signed in")?;
    let token = config.account_token().context("not signed in")?;
    let mut signal = open(server, config).await?;
    signal.send(ClientMsg::ChangePassword { token, new_password: new_password.to_string() })?;
    let reply = match timeout(Duration::from_secs(15), signal.recv()).await {
        Ok(Some(msg)) => msg,
        Ok(None) => bail!("the server closed the connection"),
        Err(_) => bail!("the server did not answer"),
    };
    match reply {
        ServerMsg::LoginResult { ok: true, .. } => {}
        ServerMsg::LoginResult { ok: false, message, .. } => bail!("{message}"),
        ServerMsg::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected reply from the server: {other:?}"),
    }
    login(server, config, &username, new_password).await
}

/// Pushes the current address book to the server (best effort).
pub async fn push_address_book(server: &ServerAddr, config: &mut Config) -> Result<()> {
    let token = config.account_token().context("not signed in")?;
    let mut signal = open(server, config).await?;
    signal.send(ClientMsg::SaveAddressBook { token, entries: config.address_book.clone() })?;
    // Give the write a moment to reach the server before we drop the link.
    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(())
}

/// Signs out: tells the server to drop the token, then clears the local account.
pub async fn logout(server: &ServerAddr, config: &mut Config) -> Result<()> {
    if let Some(token) = config.account_token() {
        if let Ok(signal) = open(server, config).await {
            let _ = signal.send(ClientMsg::Logout { token });
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    config.account = None;
    config.reload_identity_from_disk();
    config.save()?;
    Ok(())
}
