//! Client side of the signaling connection (TLS 1.3 over TCP).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use pjdesk_proto::framing::{read_msg, write_msg, MAX_SIGNAL_FRAME};
use pjdesk_proto::signal::{ClientMsg, ServerMsg};
use pjdesk_proto::tls::{self, PinnedCertVerifier};
use pjdesk_proto::{ALPN_SIGNAL, DEFAULT_PORT, SIGNAL_SERVER_NAME};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
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

    /// Resolves to an IPv4 address (the P2P layer is IPv4-only for now).
    /// The lookup is bounded. A stuck resolver used to sit on "Looking up"
    /// forever, so the only registrar never reached Register.
    pub async fn resolve(&self) -> Result<SocketAddr> {
        let lookup =
            tokio::time::timeout(Duration::from_secs(8), tokio::net::lookup_host((self.host.as_str(), self.port)))
                .await
                .map_err(|_| anyhow!("timed out resolving server name \"{}\"", self.host))?;
        let addrs: Vec<SocketAddr> =
            lookup.with_context(|| format!("cannot resolve server name \"{}\"", self.host))?.collect();
        addrs
            .iter()
            .find(|a| a.is_ipv4())
            .copied()
            .ok_or_else(|| anyhow!("\"{}\" has no IPv4 address (IPv6-only servers aren't supported yet)", self.host))
    }
}

/// True when `connected` is the account's standby, and that standby is not the
/// server this computer was told to use. The same address is not a standby hit.
pub fn reached_standby(connected: SocketAddr, primary: &ServerAddr, standby: &ServerAddr, standby_addr: SocketAddr) -> bool {
    if standby.host.trim().is_empty() {
        return false;
    }
    if standby.host.eq_ignore_ascii_case(primary.host.trim()) && standby.port == primary.port {
        return false;
    }
    connected.ip() == standby_addr.ip() && connected.port() == standby_addr.port()
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

/// How long a LAN handshake may take before the public address is used.
const LAN_HANDSHAKE: Duration = Duration::from_millis(1500);
/// The public path starts this long after the LAN path, so a working LAN wins.
const PUBLIC_DELAY: Duration = Duration::from_millis(200);
const PUBLIC_HANDSHAKE: Duration = Duration::from_secs(10);

/// An open signaling connection. Messages are sent and received through
/// channels so several tasks can use it.
pub struct SignalClient {
    tx: mpsc::UnboundedSender<ClientMsg>,
    rx: mpsc::UnboundedReceiver<ServerMsg>,
    /// The server address this connection actually reached (its UDP port is on the same IP).
    pub server_addr: SocketAddr,
    /// `LAN` or `Internet`.
    pub via: &'static str,
    tasks: Vec<JoinHandle<()>>,
}

struct Ready {
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    addr: SocketAddr,
    via: &'static str,
    verifier: Arc<PinnedCertVerifier>,
}

impl SignalClient {
    /// Connects, verifies the server key on every path, and starts the
    /// reader/writer tasks. A LAN address is used only when it is the current
    /// primary on this computer's network. A local standby is not used.
    pub async fn connect(server: &ServerAddr, config: &mut Config) -> Result<Self> {
        Self::connect_mode(server, config, false).await
    }

    /// `public_only` skips LAN candidates after a standby refused registration.
    /// DNS is resolved on every call.
    pub async fn connect_mode(server: &ServerAddr, config: &mut Config, public_only: bool) -> Result<Self> {
        info!("Looking up {server}");
        let (resolved, heard) = tokio::join!(
            server.resolve(),
            tokio::time::timeout(
                Duration::from_millis(500),
                tokio::task::spawn_blocking(|| pjdesk_proto::discover::listen_servers(Duration::from_millis(250))),
            ),
        );
        let resolved = resolved?;
        let heard = match heard {
            Ok(Ok(list)) => list,
            _ => Vec::new(),
        };
        let same_site = if public_only { false } else { same_site(&config.network_public_ip).await };
        let own = tokio::task::spawn_blocking(pjdesk_proto::discover::private_ipv4s).await.unwrap_or_default();
        let pinned = config.pinned_fingerprint();
        let account = if same_site && !public_only {
            let mut entries = config.network_lan.clone();
            if let Some(cached) = config.lan_path_cache.get(&net_key(resolved)) {
                entries.push(cached.clone());
            }
            lan_targets(&entries, server.port, resolved)
        } else {
            Vec::new()
        };
        let beacons = if public_only {
            Vec::new()
        } else {
            heard.into_iter().map(|(addr, beacon)| (addr, beacon.fingerprint, beacon.role)).collect()
        };
        let plan = pjdesk_proto::discover::reach_plan(
            resolved,
            &config.network_public_ip,
            server.port,
            same_site,
            &account,
            &beacons,
            pinned,
            &own,
        );
        let expected = config.server_fingerprint.clone();
        let ready = if plan.lan.is_empty() {
            handshake(
                plan.public,
                path_label(plan.public),
                PUBLIC_HANDSHAKE,
                &server.to_string(),
                pinned,
                expected.as_deref(),
            )
            .await?
        } else {
            race_paths(&plan.lan, plan.public, &server.to_string(), pinned, expected.as_deref()).await?
        };

        if pinned.is_none() {
            if let Some(fp) = ready.verifier.observed() {
                let text = tls::format_fingerprint(&fp);
                info!("First connection to {server}: trusting its key {text}");
                config.server_fingerprint = Some(text);
                config.save()?;
            }
        }
        let key = config
            .server_fingerprint
            .as_deref()
            .map(pjdesk_proto::discover::fingerprint_prefix)
            .unwrap_or_else(|| "unpinned".into());
        info!("Connected to {server} via {} ({}), key {key}", ready.via, ready.addr);
        if ready.via == "LAN" && same_site {
            let key = net_key(resolved);
            let value = ready.addr.to_string();
            if config.lan_path_cache.get(&key).map(String::as_str) != Some(value.as_str()) {
                config.lan_path_cache.insert(key, value);
                let _ = config.save();
            }
        }

        let addr = ready.addr;
        let via = ready.via;
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

        Ok(Self { tx: out_tx, rx: in_rx, server_addr: addr, via, tasks: vec![writer_task, reader_task] })
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

    /// Dials one address. An IP is not looked up in DNS or the hosts file.
    pub async fn connect_endpoint(target: &str, config: &mut Config, budget: Duration) -> Result<Self> {
        let server = ServerAddr::parse(target)?;
        let label = server.to_string();
        let resolved = if let Some(ip) = target.trim().parse::<std::net::Ipv4Addr>().ok().filter(|_| !target.contains(':')) {
            SocketAddr::new(IpAddr::V4(ip), server.port)
        } else if pjdesk_proto::routes::is_ip_endpoint(target) {
            let ip = pjdesk_proto::routes::endpoint_v4(target).context("missing IP")?;
            SocketAddr::new(IpAddr::V4(ip), server.port)
        } else {
            timeout(Duration::from_secs(2), server.resolve()).await.map_err(|_| anyhow!("timed out resolving {label}"))??
        };
        let pinned = config.pinned_fingerprint();
        let expected = config.server_fingerprint.clone();
        let ready = handshake(resolved, path_label(resolved), budget, &label, pinned, expected.as_deref()).await?;
        finish_client(ready, config, &label)
    }

    /// Tries the fresh advertised IPs, then a saved hostname.
    pub async fn connect_catalog(config: &mut Config, mine: &[std::net::Ipv4Addr]) -> Result<Self> {
        crate::routes_store::absorb(config);
        let saved = config.effective_server();
        let targets = pjdesk_proto::routes::dial_targets(&config.dial_routes(), mine, &saved);
        if targets.is_empty() {
            bail!("the server address is empty");
        }
        let mut last = None;
        for target in &targets {
            let budget = pjdesk_proto::routes::dial_budget(target);
            info!("Dialing {target}");
            match Self::connect_endpoint(target, config, budget).await {
                Ok(signal) => {
                    info!("Server answered at {target} via {} ({})", signal.via, signal.server_addr);
                    crate::routes_store::note_answer(config, target);
                    return Ok(signal);
                }
                Err(err) => {
                    info!("Dial {target} failed: {err:#}");
                    crate::routes_store::note_timeout(config, target);
                    last = Some(err);
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("could not reach a PJDesk server")))
    }
}

fn finish_client(ready: Ready, config: &mut Config, label: &str) -> Result<SignalClient> {
    if config.pinned_fingerprint().is_none() {
        if let Some(fp) = ready.verifier.observed() {
            let text = tls::format_fingerprint(&fp);
            info!("First connection to {label}: trusting its key {text}");
            config.server_fingerprint = Some(text);
            config.save()?;
        }
    }
    let addr = ready.addr;
    let via = ready.via;
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
    Ok(SignalClient { tx: out_tx, rx: in_rx, server_addr: addr, via, tasks: vec![writer_task, reader_task] })
}

impl Drop for SignalClient {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Private IPv4 addresses from the account slot, excluding the public address.
fn lan_targets(entries: &[String], fallback_port: u16, public_addr: SocketAddr) -> Vec<SocketAddr> {
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

/// Local IPv4 used to reach `toward`. A different network gets a different key.
fn net_key(toward: SocketAddr) -> String {
    let ip = std::net::UdpSocket::bind("0.0.0.0:0").ok().and_then(|socket| {
        socket.connect(toward).ok()?;
        match socket.local_addr().ok()?.ip() {
            IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip.to_string()),
            _ => None,
        }
    });
    format!("ip:{}", ip.unwrap_or_else(|| "unknown".into()))
}

fn same_public_ip(mine: Option<&str>, published: &str) -> bool {
    let Some(mine) = mine.map(str::trim).filter(|ip| !ip.is_empty()) else {
        return false;
    };
    let published = published.trim();
    !published.is_empty() && mine == published
}

/// True when this computer is on the primary's public network.
async fn same_site(published_public: &str) -> bool {
    let published = published_public.trim().to_string();
    if published.is_empty() {
        return false;
    }
    let mine = tokio::task::spawn_blocking(my_public_ip).await.ok().flatten();
    same_public_ip(mine.as_deref(), &published)
}

fn my_public_ip() -> Option<String> {
    struct Cache {
        ip: Option<String>,
        at: Instant,
    }
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    if let Ok(guard) = CACHE.lock() {
        if let Some(cache) = guard.as_ref() {
            if cache.at.elapsed() < Duration::from_secs(300) {
                return cache.ip.clone();
            }
        }
    }
    let ip = fetch_my_public_ip();
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(Cache { ip: ip.clone(), at: Instant::now() });
    }
    ip
}

fn fetch_my_public_ip() -> Option<String> {
    // The server already knows the public addresses. A client does not ask
    // pjdesk.com which IP it sees.
    None
}

/// LAN and the public address in parallel. Each path has its own fingerprint check.
async fn race_paths(
    lan: &[SocketAddr],
    public_addr: SocketAddr,
    server_label: &str,
    pinned: Option<pjdesk_proto::tls::Fingerprint>,
    expected: Option<&str>,
) -> Result<Ready> {
    let expected = expected.map(str::to_string);
    let mut set = JoinSet::new();
    for &addr in lan {
        let label = addr.to_string();
        let expected = expected.clone();
        set.spawn(async move { handshake(addr, "LAN", LAN_HANDSHAKE, &label, pinned, expected.as_deref()).await });
    }
    let label = server_label.to_string();
    set.spawn(async move {
        tokio::time::sleep(PUBLIC_DELAY).await;
        handshake(public_addr, path_label(public_addr), PUBLIC_HANDSHAKE, &label, pinned, expected.as_deref()).await
    });
    let mut mismatch = None;
    let mut other = None;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(ready)) => {
                set.abort_all();
                return Ok(ready);
            }
            Ok(Err(err)) if format!("{err:#}").contains("SERVER KEY MISMATCH") => mismatch = Some(err),
            Ok(Err(err)) => other = Some(err),
            Err(_) => {}
        }
    }
    Err(mismatch.or(other).unwrap_or_else(|| anyhow!("could not reach the server at {server_label}")))
}

/// A hostname that resolves to a private address is the LAN, even when it was
/// the published server name (`pjdesk.spader.us` → `192.168.0.231`).
fn path_label(addr: SocketAddr) -> &'static str {
    if crate::p2p::is_lan_ip(addr.ip()) {
        "LAN"
    } else {
        "Internet"
    }
}

async fn handshake(
    addr: SocketAddr,
    via: &'static str,
    limit: Duration,
    label: &str,
    pinned: Option<pjdesk_proto::tls::Fingerprint>,
    expected: Option<&str>,
) -> Result<Ready> {
    let tcp = timeout(limit, TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow!("timed out connecting to the server at {label}"))?
        .with_context(|| {
            format!("cannot reach the PJDesk server at {label} (is it running, and is TCP port {} open?)", addr.port())
        })?;
    tcp.set_nodelay(true)?;
    let verifier = PinnedCertVerifier::new(pinned);
    let connector = TlsConnector::from(Arc::new(tls::client_config(verifier.clone(), ALPN_SIGNAL)?));
    let name = ServerName::try_from(SIGNAL_SERVER_NAME)?.to_owned();
    let stream = match timeout(limit, connector.connect(name, tcp)).await {
        Err(_) => bail!("TLS handshake with {label} timed out"),
        Ok(Err(_)) if verifier.mismatch() => bail!(
            "SERVER KEY MISMATCH for {label}.\n\
             The server presented key {} but this computer trusts {}.\n\
             If you reinstalled the server, run `pjdesk config --reset-fingerprint`; \
             otherwise someone may be intercepting the connection.",
            verifier.observed().map(|f| tls::format_fingerprint(&f)).unwrap_or_default(),
            expected.unwrap_or("?")
        ),
        Ok(Err(e)) => return Err(e).context("TLS handshake with the server failed"),
        Ok(Ok(stream)) => stream,
    };
    Ok(Ready { stream, addr, via, verifier })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standby_address_is_not_a_successful_registration() {
        let primary = ServerAddr::parse("pjdesk.spader.us:21200").unwrap();
        let standby = ServerAddr::parse("chip20.example:21200").unwrap();
        let chip: SocketAddr = "192.168.1.50:21200".parse().unwrap();
        let lenovo: SocketAddr = "192.168.1.20:21200".parse().unwrap();
        assert!(reached_standby(chip, &primary, &standby, chip));
        assert!(!reached_standby(lenovo, &primary, &standby, chip));
        let same = ServerAddr::parse("pjdesk.spader.us:21200").unwrap();
        assert!(!reached_standby(chip, &primary, &same, chip), "the configured server is not its own standby");
    }

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
    fn lan_targets_keep_private_addresses_only() {
        let public = "203.0.113.9:21200".parse().unwrap();
        let got = lan_targets(
            &[
                "192.168.1.20:21200".into(),
                "10.0.0.5".into(),
                "203.0.113.9:21200".into(),
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
        assert!(same_public_ip(Some("204.248.122.44"), "204.248.122.44"));
        assert!(!same_public_ip(Some("68.234.75.68"), "204.248.122.44"));
        assert!(!same_public_ip(None, "204.248.122.44"));
        assert!(!same_public_ip(Some("204.248.122.44"), ""));
    }

    #[test]
    fn a_private_resolved_address_is_lan() {
        assert_eq!(path_label("192.168.0.231:21200".parse().unwrap()), "LAN");
        assert_eq!(path_label("10.1.2.3:21200".parse().unwrap()), "LAN");
        assert_eq!(path_label("172.16.0.4:21200".parse().unwrap()), "LAN");
        assert_eq!(path_label("169.254.1.1:21200".parse().unwrap()), "LAN");
        assert_eq!(path_label("203.0.113.8:21200".parse().unwrap()), "Internet");
    }
}
