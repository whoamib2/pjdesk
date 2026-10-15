//! Client side of the signaling connection (TLS 1.3 over TCP).

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use desklink_proto::framing::{read_msg, write_msg, MAX_SIGNAL_FRAME};
use desklink_proto::signal::{ClientMsg, ServerMsg};
use desklink_proto::tls::{self, PinnedCertVerifier};
use desklink_proto::{ALPN_SIGNAL, DEFAULT_PORT, SIGNAL_SERVER_NAME};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tracing::info;

use crate::config::Config;

/// A server address as typed by the user: `host`, `host:port` or `[v6]:port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddr {
    pub host: String,
    pub port: u16,
}

impl ServerAddr {
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let text = text.strip_prefix("desklink://").unwrap_or(text);
        if text.is_empty() {
            bail!("the server address is empty");
        }
        if let Some(rest) = text.strip_prefix('[') {
            let (host, tail) = rest.split_once(']').context("missing ']' in IPv6 address")?;
            let port = match tail.strip_prefix(':') {
                Some(p) => p.parse().context("invalid port")?,
                None => DEFAULT_PORT,
            };
            return Ok(Self { host: host.to_string(), port });
        }
        match text.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => Ok(Self {
                host: host.to_string(),
                port: port.parse().with_context(|| format!("invalid port \"{port}\""))?,
            }),
            _ => Ok(Self { host: text.to_string(), port: DEFAULT_PORT }),
        }
    }

    /// Resolves to an IPv4 address (the P2P layer is IPv4-only for now).
    pub async fn resolve(&self) -> Result<SocketAddr> {
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((self.host.as_str(), self.port))
            .await
            .with_context(|| format!("cannot resolve server name \"{}\"", self.host))?
            .collect();
        addrs
            .iter()
            .find(|a| a.is_ipv4())
            .copied()
            .ok_or_else(|| anyhow!("\"{}\" has no IPv4 address (IPv6-only servers aren't supported yet)", self.host))
    }
}

impl fmt::Display for ServerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// An open signaling connection. Messages are sent and received through
/// channels so several tasks can use it.
pub struct SignalClient {
    tx: mpsc::UnboundedSender<ClientMsg>,
    rx: mpsc::UnboundedReceiver<ServerMsg>,
    /// The server's resolved address (its UDP port is on the same IP).
    pub server_addr: SocketAddr,
    tasks: Vec<JoinHandle<()>>,
}

impl SignalClient {
    /// Connects, verifies the server key (trust on first use) and starts the
    /// reader/writer tasks.
    pub async fn connect(server: &ServerAddr, config: &mut Config) -> Result<Self> {
        let addr = server.resolve().await?;
        let tcp = timeout(Duration::from_secs(10), TcpStream::connect(addr))
            .await
            .map_err(|_| anyhow!("timed out connecting to the server at {server}"))?
            .with_context(|| {
                format!(
                    "cannot reach the DeskLink server at {server} (is it running, and is TCP port {} open?)",
                    server.port
                )
            })?;
        tcp.set_nodelay(true)?;

        let pinned = config.pinned_fingerprint();
        let verifier = PinnedCertVerifier::new(pinned);
        let connector = TlsConnector::from(Arc::new(tls::client_config(verifier.clone(), ALPN_SIGNAL)?));
        let name = ServerName::try_from(SIGNAL_SERVER_NAME)?.to_owned();
        let stream = match timeout(Duration::from_secs(10), connector.connect(name, tcp)).await {
            Err(_) => bail!("TLS handshake with {server} timed out"),
            Ok(Err(_)) if verifier.mismatch() => bail!(
                "SERVER KEY MISMATCH for {server}.\n\
                 The server presented key {} but this computer trusts {}.\n\
                 If you reinstalled the server, run `desklink config --reset-fingerprint`; \
                 otherwise someone may be intercepting the connection.",
                verifier.observed().map(|f| tls::format_fingerprint(&f)).unwrap_or_default(),
                config.server_fingerprint.as_deref().unwrap_or("?")
            ),
            Ok(Err(e)) => return Err(e).context("TLS handshake with the server failed"),
            Ok(Ok(stream)) => stream,
        };

        if pinned.is_none() {
            if let Some(fp) = verifier.observed() {
                let text = tls::format_fingerprint(&fp);
                info!("First connection to {server}: trusting its key {text}");
                config.server_fingerprint = Some(text);
                config.save()?;
            }
        }

        let (mut reader, mut writer) = tokio::io::split(stream);
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ClientMsg>();
        let (in_tx, in_rx) = mpsc::unbounded_channel::<ServerMsg>();
        let writer_task = tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if write_msg(&mut writer, &msg).await.is_err() {
                    break;
                }
            }
        });
        let reader_task = tokio::spawn(async move {
            while let Ok(msg) = read_msg::<_, ServerMsg>(&mut reader, MAX_SIGNAL_FRAME).await {
                if in_tx.send(msg).is_err() {
                    break;
                }
            }
        });

        Ok(Self { tx: out_tx, rx: in_rx, server_addr: addr, tasks: vec![writer_task, reader_task] })
    }

    pub fn send(&self, msg: ClientMsg) -> Result<()> {
        self.tx.send(msg).map_err(|_| anyhow!("the connection to the server was lost"))
    }

    /// A sender other tasks can use.
    pub fn sender(&self) -> mpsc::UnboundedSender<ClientMsg> {
        self.tx.clone()
    }

    /// Next message from the server, or `None` once the connection closed.
    pub async fn recv(&mut self) -> Option<ServerMsg> {
        self.rx.recv().await
    }
}

impl Drop for SignalClient {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_addresses() {
        assert_eq!(
            ServerAddr::parse("example.com").unwrap(),
            ServerAddr { host: "example.com".into(), port: DEFAULT_PORT }
        );
        assert_eq!(ServerAddr::parse(" 10.0.0.5:9000 ").unwrap(), ServerAddr { host: "10.0.0.5".into(), port: 9000 });
        assert_eq!(ServerAddr::parse("[::1]:7").unwrap(), ServerAddr { host: "::1".into(), port: 7 });
        assert_eq!(ServerAddr::parse("[::1]:7").unwrap().to_string(), "[::1]:7");
        assert!(ServerAddr::parse("host:notaport").is_err());
        assert!(ServerAddr::parse("").is_err());
    }
}
