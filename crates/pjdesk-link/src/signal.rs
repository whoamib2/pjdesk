//! TLS signaling connection. The caller decides what to do with the server's
//! certificate fingerprint (trust on first use lives in the client).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use pjdesk_proto::framing::{read_msg, write_msg, MAX_SIGNAL_FRAME};
use pjdesk_proto::signal::{ClientMsg, ServerMsg};
use pjdesk_proto::tls::{self, Fingerprint, PinnedCertVerifier};
use pjdesk_proto::{ALPN_SIGNAL, DEFAULT_PORT, SIGNAL_SERVER_NAME};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

/// A server address as typed by the user: `host`, `host:port` or `[v6]:port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddr {
    pub host: String,
    pub port: u16,
}

impl ServerAddr {
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let text = text.strip_prefix("pjdesk://").unwrap_or(text);
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

    /// Resolves to an IPv4 address (the peer path is IPv4 for now).
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

/// How long a LAN handshake may take before it is given up.
const LAN_HANDSHAKE: Duration = Duration::from_millis(1500);
/// The public path starts this long after the LAN path, so a working LAN wins.
const PUBLIC_DELAY: Duration = Duration::from_millis(200);
const PUBLIC_HANDSHAKE: Duration = Duration::from_secs(10);

struct Ready {
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    addr: SocketAddr,
    via: &'static str,
    verifier: Arc<PinnedCertVerifier>,
}

/// An open signaling connection. Signaling is TLS over TCP on port 21200.
/// The direct picture path is UDP 21201. Relay UDP uses the IP this TCP
/// handshake actually reached, so a LAN win does not hairpin the public address.
pub struct SignalClient {
    tx: mpsc::UnboundedSender<ClientMsg>,
    rx: mpsc::UnboundedReceiver<ServerMsg>,
    pub server_addr: SocketAddr,
    /// `LAN` or `Internet`.
    pub via: &'static str,
    /// The key the server presented, when this was the first connection (`pinned` was empty).
    pub observed: Option<Fingerprint>,
    tasks: Vec<JoinHandle<()>>,
}

impl SignalClient {
    /// Connects and checks the server key. `pinned` of `None` trusts the key
    /// this connection presents and returns it in [`Self::observed`].
    pub async fn connect(server: &ServerAddr, pinned: Option<Fingerprint>) -> Result<Self> {
        Self::connect_reaching(server, pinned, &[], "", "", false).await
    }

    /// Dials one address. An IP is used as written and is not looked up in DNS.
    pub async fn connect_endpoint(target: &str, pinned: Option<Fingerprint>, budget: Duration) -> Result<Self> {
        let server = ServerAddr::parse(target)?;
        let label = server.to_string();
        let resolved = if pjdesk_proto::routes::is_ip_endpoint(target) {
            let ip = pjdesk_proto::routes::endpoint_v4(target).context("missing IP")?;
            SocketAddr::new(std::net::IpAddr::V4(ip), server.port)
        } else {
            timeout(Duration::from_secs(2), server.resolve()).await.map_err(|_| anyhow!("timed out resolving {label}"))??
        };
        let via = if crate::p2p::is_lan_ip(resolved.ip()) { "LAN" } else { "Internet" };
        let ready = handshake(resolved, via, budget, &label, pinned).await?;
        let observed = pinned.is_none().then(|| ready.verifier.observed()).flatten();
        let addr = ready.addr;
        let (mut reader, mut writer) = tokio::io::split(ready.stream);
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
        Ok(Self { tx: out_tx, rx: in_rx, server_addr: addr, via, observed, tasks: vec![writer_task, reader_task] })
    }

    /// Same handshake as [`Self::connect`]. A LAN address is dialed only when it
    /// is the current primary and this device is on that server's public network.
    /// A standby beacon, a role-less beacon, localhost, and this device's own
    /// addresses are not used. `public_only` is the retry after a standby refusal.
    /// DNS is resolved on every call.
    pub async fn connect_reaching(
        server: &ServerAddr,
        pinned: Option<Fingerprint>,
        lan: &[String],
        published_public_ip: &str,
        my_public_ip: &str,
        public_only: bool,
    ) -> Result<Self> {
        let (resolved, heard) = tokio::join!(
            server.resolve(),
            tokio::task::spawn_blocking(|| pjdesk_proto::discover::listen_servers(Duration::from_millis(250))),
        );
        let resolved = resolved?;
        let same_site = !public_only && same_public_ip(my_public_ip, published_public_ip);
        let own = tokio::task::spawn_blocking(pjdesk_proto::discover::private_ipv4s).await.unwrap_or_default();
        let account = if same_site { lan_targets(lan, server.port, resolved) } else { Vec::new() };
        let beacons = if public_only {
            Vec::new()
        } else {
            heard
                .unwrap_or_default()
                .into_iter()
                .map(|(addr, beacon)| (addr, beacon.fingerprint, beacon.role))
                .collect()
        };
        let plan = pjdesk_proto::discover::reach_plan(
            resolved,
            published_public_ip,
            server.port,
            same_site,
            &account,
            &beacons,
            pinned,
            &own,
        );
        let label = server.to_string();
        let ready = if plan.lan.is_empty() {
            let via = if crate::p2p::is_lan_ip(plan.public.ip()) { "LAN" } else { "Internet" };
            handshake(plan.public, via, PUBLIC_HANDSHAKE, &label, pinned).await?
        } else {
            race_paths(&plan.lan, plan.public, &label, pinned).await?
        };
        let observed = pinned.is_none().then(|| ready.verifier.observed()).flatten();
        let via = ready.via;
        let addr = ready.addr;
        let (mut reader, mut writer) = tokio::io::split(ready.stream);
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
        Ok(Self { tx: out_tx, rx: in_rx, server_addr: addr, via, observed, tasks: vec![writer_task, reader_task] })
    }

    pub fn send(&self, msg: ClientMsg) -> Result<()> {
        self.tx.send(msg).map_err(|_| anyhow!("the connection to the server was lost"))
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<ClientMsg> {
        self.tx.clone()
    }

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

/// Private IPv4 addresses from the account, excluding the public address.
pub fn lan_targets(entries: &[String], fallback_port: u16, public_addr: SocketAddr) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    for entry in entries {
        let text = entry.trim();
        if text.is_empty() {
            continue;
        }
        let parsed = if let Ok(addr) = text.parse::<SocketAddr>() {
            addr
        } else if let Ok(ip) = text.parse::<Ipv4Addr>() {
            SocketAddr::new(IpAddr::V4(ip), fallback_port)
        } else {
            continue;
        };
        let private = match parsed.ip() {
            IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
            IpAddr::V6(_) => false,
        };
        if private && parsed != public_addr && !out.contains(&parsed) {
            out.push(parsed);
        }
    }
    out
}

fn same_public_ip(mine: &str, published: &str) -> bool {
    let mine = mine.trim();
    let published = published.trim();
    !mine.is_empty() && mine == published
}

async fn race_paths(
    lan: &[SocketAddr],
    public_addr: SocketAddr,
    server_label: &str,
    pinned: Option<Fingerprint>,
) -> Result<Ready> {
    let mut set = JoinSet::new();
    for &addr in lan {
        let label = addr.to_string();
        set.spawn(async move { handshake(addr, "LAN", LAN_HANDSHAKE, &label, pinned).await });
    }
    let label = server_label.to_string();
    let public_via = if crate::p2p::is_lan_ip(public_addr.ip()) { "LAN" } else { "Internet" };
    set.spawn(async move {
        tokio::time::sleep(PUBLIC_DELAY).await;
        handshake(public_addr, public_via, PUBLIC_HANDSHAKE, &label, pinned).await
    });
    let mut mismatch = None;
    let mut other = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(ready)) => {
                set.abort_all();
                return Ok(ready);
            }
            Ok(Err(err)) if format!("{err:#}").contains("SERVER KEY MISMATCH") => mismatch = Some(err),
            Ok(Err(err)) => other.push(format!("{err:#}")),
            Err(_) => {}
        }
    }
    if let Some(err) = mismatch {
        return Err(err);
    }
    if other.is_empty() {
        bail!("could not reach the PJDesk server at {server_label}")
    }
    bail!("{}", other.join("; "))
}

async fn handshake(
    addr: SocketAddr,
    via: &'static str,
    limit: Duration,
    label: &str,
    pinned: Option<Fingerprint>,
) -> Result<Ready> {
    let tcp = timeout(limit, TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow!("timed out connecting to the server at {label}"))?
        .with_context(|| format!("cannot reach the PJDesk server at {label} (TCP port {})", addr.port()))?;
    tcp.set_nodelay(true)?;
    let verifier = PinnedCertVerifier::new(pinned);
    let connector = TlsConnector::from(Arc::new(tls::client_config(verifier.clone(), ALPN_SIGNAL)?));
    let name = ServerName::try_from(SIGNAL_SERVER_NAME)?.to_owned();
    let stream = match timeout(limit, connector.connect(name, tcp)).await {
        Err(_) => bail!("TLS handshake with {label} timed out"),
        Ok(Err(_)) if verifier.mismatch() => {
            let saw = verifier.observed().map(|fp| tls::format_fingerprint(&fp)).unwrap_or_default();
            let trusted = pinned.map(|fp| tls::format_fingerprint(&fp)).unwrap_or_else(|| "?".into());
            bail!("SERVER KEY MISMATCH for {label}. The server presented key {saw} but this phone trusts {trusted}.")
        }
        Ok(Err(e)) => return Err(e).context(format!("TLS handshake with {label} failed")),
        Ok(Ok(stream)) => stream,
    };
    Ok(Ready { stream, addr, via, verifier })
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

    #[test]
    fn lan_targets_keep_private_ipv4_only() {
        let public = "204.248.122.44:21200".parse().unwrap();
        let got = lan_targets(
            &[
                "192.168.1.20:21200".into(),
                "10.0.0.5".into(),
                "204.248.122.44:21200".into(),
                "pjdesk.spader.us:21200".into(),
                "192.168.1.20:21200".into(),
            ],
            21200,
            public,
        );
        assert_eq!(got, vec!["192.168.1.20:21200".parse().unwrap(), "10.0.0.5:21200".parse().unwrap()]);
    }

    #[test]
    fn the_same_public_ip_selects_the_lan_path() {
        assert!(same_public_ip("204.248.122.44", "204.248.122.44"));
        assert!(!same_public_ip("68.234.75.68", "204.248.122.44"));
        assert!(!same_public_ip("", "204.248.122.44"));
    }
}
