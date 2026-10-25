//! Owner controls for server access keys, over the signaling connection.
//!
//! A guest does not use this. The guest puts the key in Network settings and
//! the host sends it with the existing PresentAccess message.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use pjdesk_proto::signal::{AccessKeyBrief, ClientMsg, ServerMsg};
use tokio::time::timeout;

use crate::config::Config;
use crate::signaling::{ServerAddr, SignalClient};

#[derive(Debug, Default)]
pub struct Panel {
    pub label: String,
    pub days: u64,
    pub limit: String,
    pub secret: String,
    pub show: bool,
    pub keys: Vec<AccessKeyBrief>,
    pub confirm: Option<String>,
    pub busy: bool,
    pub note: String,
    pub loaded: bool,
    /// True after the server says this account is not the owner.
    pub hidden: bool,
    pub rx: Option<std::sync::mpsc::Receiver<Job>>,
}

pub enum Job {
    Ready(Reply),
    Failed(String),
}

pub struct Reply {
    pub message: String,
    pub secret: String,
    pub keys: Vec<AccessKeyBrief>,
}

pub async fn manage(
    server: &ServerAddr,
    config: &mut Config,
    op: &str,
    label: &str,
    expires_secs: u64,
    computer_limit: u32,
    key_id: &str,
) -> Result<Reply> {
    crate::network::refresh_config(config);
    let token = config.network_access_token();
    if token.is_empty() {
        bail!("Sign in on this computer as the owner of this server.");
    }
    let mut signal = SignalClient::connect(server, config).await.context("connecting to the server")?;
    signal.send(ClientMsg::ManageAccessKey {
        token,
        op: op.to_string(),
        label: label.to_string(),
        expires_secs,
        computer_limit,
        key_id: key_id.to_string(),
    })?;
    match timeout(Duration::from_secs(12), signal.recv()).await {
        Ok(Some(ServerMsg::AccessKeys { ok, message, secret, keys })) => {
            if ok {
                Ok(Reply { message, secret, keys })
            } else {
                bail!(if message.is_empty() { "Could not change access keys.".to_string() } else { message })
            }
        }
        Ok(Some(ServerMsg::Error { message, .. })) => bail!(message),
        Ok(None) => bail!("This server does not have access keys yet."),
        Err(_) => bail!("This server does not have access keys yet."),
        Ok(Some(_)) => bail!("unexpected reply from the server"),
    }
}

pub fn expires_secs(days: u64) -> u64 {
    match days {
        1 => 86_400,
        7 => 7 * 86_400,
        30 => 30 * 86_400,
        _ => 0,
    }
}
