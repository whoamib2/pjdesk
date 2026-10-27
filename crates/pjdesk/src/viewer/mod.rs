//! Controller mode: connect to a host by ID and show its screen.

#[cfg(feature = "ui-shots")]
pub(crate) fn render_monitor_shots(dir: &std::path::Path) {
    toolbar::render_monitor_shots(dir);
}

mod controls;
pub mod decoder;
mod filedrop;
mod keymap;
mod panel;
mod render;
mod toolbar;
mod window;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use pjdesk_proto::framing::write_msg;
use pjdesk_proto::peer::{read_video_frame, ControlMsg};
use pjdesk_proto::signal::{ClientMsg, ServerMsg};
use pjdesk_proto::udp::Role;
use pjdesk_proto::{format_id, is_valid_id, normalize_id, PROTOCOL_VERSION};
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use tokio::time::timeout;

use crate::config::Config;
use crate::p2p::{self, LinkPath};
use crate::session::{self, WrongPassword};
use crate::signaling::{ServerAddr, SignalClient};
use crate::ConnectArgs;

/// An authenticated connection to a host.
pub struct Link {
    pub connection: Connection,
    pub endpoint: Endpoint,
    pub control_send: SendStream,
    pub control_recv: RecvStream,
    pub host_name: String,
    pub host_id: String,
    pub path: LinkPath,
    pub peer_proto: u16,
    /// LAN and public addresses from the last introduction. Tried before the relay.
    pub direct_targets: Vec<SocketAddr>,
    /// Host certificate from that introduction.
    pub host_fingerprint: Option<pjdesk_proto::tls::Fingerprint>,
}

pub fn run(server: ServerAddr, mut config: Config, args: ConnectArgs) -> Result<()> {
    let direct = args.direct.clone();
    let typed = args.id.join(" ");
    let target = normalize_id(&typed);
    if direct.is_none() && !is_valid_id(&target) {
        bail!("\"{typed}\" is not a PJDesk ID (IDs have 9 digits). For a direct connection, pass --direct host:port.");
    }
    let label = direct.clone().unwrap_or_else(|| format_id(&target));
    let ask = std::env::var("PJDESK_ASK").ok().map(std::path::PathBuf::from).filter(|p| !p.as_os_str().is_empty());
    // View phone sets PJDESK_ASK_VIEW and no password, and it does not set
    // PJDESK_ASK. That is an empty password, the same as the 0.6.15 hotfix.
    // rpassword opens CONIN$ and fails with os error 6 when there is no console.
    let ask_view = std::env::var("PJDESK_ASK_VIEW").ok().as_deref() == Some("1");
    let ask_person = std::env::var("PJDESK_ASK_PERSON").ok().as_deref() == Some("1");
    let stdin_is_terminal = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let password = match password_source(args.password.as_deref(), ask.is_some(), ask_view, ask_person, stdin_is_terminal)
    {
        PasswordSource::Given(password) => password,
        PasswordSource::Empty => String::new(),
        PasswordSource::Prompt => rpassword::prompt_password(format!("Password for {label}: "))?,
        PasswordSource::NoConsole => {
            bail!("No password for {label}, and there is no console to ask for one.");
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
    let mut waiting = false;
    let mut prefer_standby = false;
    let mut move_to: Option<String> = None;
    let mut tries = 0u32;
    loop {
        let link = if let Some(addr) = direct.clone() {
            runtime.block_on(connect_direct(&mut config, &addr, &password, ask.as_deref(), args.trust && !waiting))
        } else {
            runtime.block_on(connect(
                &server,
                &mut config,
                &target,
                &password,
                ask.as_deref(),
                args.force_relay,
                waiting,
                prefer_standby,
                move_to.as_deref(),
                Duration::from_secs(90),
            ))
        };
        let link = match link {
            Ok(link) => {
                move_to = None;
                link
            }
            Err(e) if waiting && tries < 40 => {
                let text = format!("{e:#}");
                if pjdesk_proto::signal::is_standby_refusal(&text) {
                    tracing::info!("failover: standby has not taken over; trying the primary again");
                    prefer_standby = false;
                } else if config.network_standby.as_deref().is_some_and(|s| !s.trim().is_empty()) && !prefer_standby
                {
                    tracing::info!("failover: primary did not answer; reconnecting through the standby");
                    prefer_standby = true;
                }
                tries += 1;
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            Err(e) => {
                let wrong = e.to_string().to_ascii_lowercase().contains("wrong password");
                if wrong && ask.is_some() {
                    crate::note_failure(&format!("{e:#}"));
                    std::process::exit(3);
                }
                if wrong && std::env::var("PJDESK_SAVED").ok().as_deref() == Some("1") && is_valid_id(&target) {
                    config.remember_password(&target, None);
                    if let Some(entry) = config.address_book.iter_mut().find(|e| e.id == target) {
                        entry.password = None;
                    }
                    let _ = config.save();
                    crate::note_failure(&format!("{e:#}"));
                    std::process::exit(3);
                }
                return Err(e);
            }
        };
        tries = 0;
        if std::env::var("PJDESK_REMEMBER").ok().as_deref() == Some("1") && !password.is_empty() && is_valid_id(&target)
        {
            config.remember_password(&target, Some(&password));
            if let Some(entry) = config.address_book.iter_mut().find(|e| e.id == target) {
                entry.password = None;
            }
            let _ = config.save();
        }
        tracing::info!("Connected to \"{}\" ({}, {})", link.host_name, format_id(&link.host_id), link.path);
        if args.headless {
            return runtime.block_on(headless(link, args.frames));
        }
        let display = std::env::var("PJDESK_DISPLAY").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let view = std::env::var("PJDESK_VIEW").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let seed = window::Seed {
            server: format!("{}:{}", server.host, server.port),
            target: target.clone(),
            password: password.clone(),
            direct: direct.clone(),
            force_relay: args.force_relay,
            display,
            view,
            remember: false,
            from_store: std::env::var("PJDESK_SAVED").ok().as_deref() == Some("1"),
            files: std::env::var("PJDESK_OPEN_PANEL").ok().as_deref() == Some("files"),
            label: String::new(),
            direct_targets: Vec::new(),
            host_fingerprint: None,
        };
        match window::run(&runtime, link, seed)? {
            window::ViewerStop::UserClosed => break,
            window::ViewerStop::HostRestart => {
                prefer_standby = false;
                if std::env::var("PJDESK_ASK_VIEW").ok().as_deref() == Some("1") {
                    break;
                }
                waiting = true;
                tracing::info!("Waiting for the computer to come back");
                std::thread::sleep(Duration::from_secs(5));
            }
            window::ViewerStop::RelayResume { primary } => {
                waiting = true;
                prefer_standby = false;
                move_to = if primary.trim().is_empty() { None } else { Some(primary.clone()) };
                if primary.trim().is_empty() {
                    tracing::info!(
                        "failover: relayed session moving to the current primary; reconnecting with the password already entered"
                    );
                } else {
                    tracing::info!(
                        "failover: relayed session moving to the primary {primary}; reconnecting with the password already entered"
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    Ok(())
}

/// How the viewer gets the computer password.
///
/// `PJDESK_ASK` and `PJDESK_ASK_VIEW` send no password. The window asks later,
/// or the phone shows its share prompt. A console prompt is only for
/// `pjdesk connect` when stdin is a terminal. Otherwise `rpassword` opens
/// CONIN$ and fails with os error 6.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PasswordSource {
    Given(String),
    Empty,
    Prompt,
    NoConsole,
}

pub(crate) fn password_source(
    supplied: Option<&str>,
    ask: bool,
    ask_view: bool,
    ask_person: bool,
    stdin_is_terminal: bool,
) -> PasswordSource {
    if ask || ask_view || ask_person {
        return PasswordSource::Empty;
    }
    match supplied {
        Some(password) => PasswordSource::Given(password.to_string()),
        None if stdin_is_terminal => PasswordSource::Prompt,
        None => PasswordSource::NoConsole,
    }
}

/// Opens one more computer inside a session window that is already running.
pub(crate) async fn open_admitted(
    admit: &crate::session_window::Admit,
    move_to: Option<&str>,
    waiting_retry: bool,
) -> Result<Link> {
    open_for(admit, move_to, waiting_retry, Duration::from_secs(90)).await
}

/// Same as [`open_admitted`], with a shorter wait for the host to answer.
async fn open_for(
    admit: &crate::session_window::Admit,
    move_to: Option<&str>,
    waiting_retry: bool,
    host_wait: Duration,
) -> Result<Link> {
    let mut config = Config::load().context("reading settings")?;
    if !admit.direct.trim().is_empty() {
        return connect_direct(&mut config, admit.direct.trim(), &admit.password, None, false).await;
    }
    let server = ServerAddr::parse(&admit.server).unwrap_or(ServerAddr {
        host: admit.server.clone(),
        port: pjdesk_proto::DEFAULT_PORT,
    });
    connect(
        &server,
        &mut config,
        &admit.id,
        &admit.password,
        None,
        admit.force_relay,
        waiting_retry,
        false,
        move_to,
        host_wait,
    )
    .await
}

/// Direct first, then the current primary, then the other server. The password
/// already entered is reused. `move_to` is the primary a role change named.
pub(crate) async fn resume_session(
    admit: &crate::session_window::Admit,
    move_to: Option<&str>,
    targets: &[SocketAddr],
    fingerprint: Option<pjdesk_proto::tls::Fingerprint>,
) -> Result<Link> {
    if !admit.force_relay {
        if let Some(fp) = fingerprint {
            match try_direct_resume(&admit.password, targets, Some(fp), &admit.id, &admit.label).await {
                Ok(Some(link)) => return Ok(link),
                Ok(None) => {}
                Err(e) if already_in_session(&e) => {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    if let Ok(Some(link)) = try_direct_resume(&admit.password, targets, Some(fp), &admit.id, &admit.label).await {
                        return Ok(link);
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    let wait = Duration::from_secs(8);
    let mut last = None;
    for attempt in 0..2 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        match open_for(admit, move_to, false, wait).await {
            Ok(link) => return Ok(link),
            Err(e) if already_in_session(&e) => last = Some(e),
            Err(e) => {
                let wrong = e.to_string().to_ascii_lowercase().contains("wrong password");
                last = Some(e);
                if wrong {
                    break;
                }
            }
        }
    }
    if move_to.is_some() {
        if let Ok(link) = open_for(admit, None, false, wait).await {
            return Ok(link);
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("could not reconnect")))
}

fn already_in_session(err: &anyhow::Error) -> bool {
    format!("{err:#}").to_ascii_lowercase().contains("already connected")
}

/// QUIC straight to an address this session already reached, with the same key.
async fn try_direct_resume(
    password: &str,
    targets: &[SocketAddr],
    fingerprint: Option<pjdesk_proto::tls::Fingerprint>,
    host_id: &str,
    host_label: &str,
) -> Result<Option<Link>> {
    if targets.is_empty() || password.is_empty() {
        return Ok(None);
    }
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").context("opening a UDP socket")?;
    let (endpoint, verifier) = p2p::controller_endpoint_pin(udp, fingerprint)?;
    let mut attempts = tokio::task::JoinSet::new();
    for (addr, delay) in p2p::dial_order(targets) {
        let endpoint = endpoint.clone();
        attempts.spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let connection = endpoint.connect(addr, pjdesk_proto::PEER_SERVER_NAME)?.await?;
            Ok::<_, anyhow::Error>((connection, addr))
        });
    }
    let winner = timeout(Duration::from_secs(3), async {
        while let Some(result) = attempts.join_next().await {
            if let Ok(Ok(found)) = result {
                return Some(found);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    attempts.abort_all();
    let Some((connection, addr)) = winner else {
        return Ok(None);
    };
    let auth = match session::controller_auth(&connection, password, &crate::hostname()).await {
        Ok(auth) => auth,
        Err(e) if e.is::<session::WrongPassword>() || already_in_session(&e) => {
            connection.close(1u32.into(), b"resume");
            return Err(e);
        }
        Err(e) => {
            tracing::info!("direct resume did not stay up: {e:#}");
            connection.close(1u32.into(), b"resume");
            return Ok(None);
        }
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    if let Some(reason) = connection.close_reason() {
        let text = reason.to_string();
        connection.close(1u32.into(), b"resume");
        if already_in_session(&anyhow!(text.clone())) {
            bail!(text);
        }
        tracing::info!("direct resume closed: {text}");
        return Ok(None);
    }
    tracing::info!("Reconnected directly to {addr}");
    Ok(Some(Link {
        connection,
        endpoint,
        control_send: auth.send,
        control_recv: auth.recv,
        host_name: if auth.peer_name.trim().is_empty() { host_label.trim().to_string() } else { auth.peer_name },
        host_id: host_id.to_string(),
        path: LinkPath::Direct(addr),
        peer_proto: auth.peer_proto,
        direct_targets: targets.to_vec(),
        host_fingerprint: verifier.observed().or(fingerprint),
    }))
}

/// Dials a saved LAN address for this computer before opening a relay session.
/// Same-subnet peers connect on UDP 21201 and do not need the server's relay.
async fn try_cached_lan(config: &Config, target: &str, password: &str, mine: &[std::net::Ipv4Addr]) -> Result<Option<Link>> {
    let target = normalize_id(target);
    if password.is_empty() || !is_valid_id(&target) {
        return Ok(None);
    }
    let Some(note) = config.settings.peers.get(&target) else {
        return Ok(None);
    };
    let listed = pjdesk_proto::routes::same_subnet_direct(&note.lan_addrs, mine);
    if listed.is_empty() {
        return Ok(None);
    }
    let targets: Vec<SocketAddr> = listed.iter().filter_map(|addr| addr.parse().ok()).collect();
    if targets.is_empty() {
        return Ok(None);
    }
    let fingerprint = pjdesk_proto::tls::parse_fingerprint(&note.host_fingerprint).ok();
    tracing::info!("Trying the host on this network before the relay: {}", listed.join(", "));
    let found = try_direct_resume(password, &targets, fingerprint, &target, "").await?;
    if let Some(link) = &found {
        let addrs: Vec<String> = link.direct_targets.iter().map(|addr| addr.to_string()).collect();
        let fp = link.host_fingerprint.map(|fp| pjdesk_proto::tls::format_fingerprint(&fp));
        Config::remember_lan(&target, &addrs, fp.as_deref());
    }
    Ok(found)
}

/// Listens briefly for a LAN beacon from `target` and stores its address.
fn note_lan_beacon(target: &str) {
    let want = normalize_id(target);
    if !is_valid_id(&want) {
        return;
    }
    let found = crate::host::direct::discover_for(Duration::from_millis(800));
    let mut addrs = Vec::new();
    let mut fingerprint = None;
    for (beacon, src) in found {
        if normalize_id(&beacon.id) != want || !pjdesk_proto::discover::accept_lan_ip(src.ip()) {
            continue;
        }
        addrs.push(SocketAddr::new(src.ip(), beacon.port).to_string());
        fingerprint = Some(pjdesk_proto::tls::format_fingerprint(&beacon.fingerprint));
    }
    if !addrs.is_empty() {
        tracing::info!("Heard {} on this network at {}", format_id(&want), addrs.join(", "));
        Config::remember_lan(&want, &addrs, fingerprint.as_deref());
    }
}

/// Signaling → UDP binding → hole punching / relay → QUIC → password check.
async fn connect(
    server: &ServerAddr,
    config: &mut Config,
    target: &str,
    password: &str,
    ask: Option<&std::path::Path>,
    force_relay: bool,
    waiting_retry: bool,
    prefer_standby: bool,
    move_to: Option<&str>,
    host_wait: Duration,
) -> Result<Link> {
    tracing::info!("Connecting to {} through the advertised servers", format_id(target));
    let _ = (server, prefer_standby);
    crate::network::refresh_config_now(config);
    let _ = waiting_retry;
    let mut access = config.presented_access();
    if access.is_empty() {
        // A temporary sign-in keeps the access token in this process only.
        access = std::env::var("PJDESK_ACCESS").unwrap_or_default();
    }
    let access_key = pjdesk_proto::access::is_access_key(&access);
    let mine = tokio::task::spawn_blocking(pjdesk_proto::discover::private_ipv4s);
    let mine = mine.await.unwrap_or_default();
    if !force_relay {
        match try_cached_lan(config, target, password, &mine).await {
            Ok(Some(link)) => {
                tracing::info!("Connected over the LAN; the relay was not used");
                return Ok(link);
            }
            Ok(None) => {}
            Err(e) if e.is::<WrongPassword>() || already_in_session(&e) => return Err(e),
            Err(e) => tracing::info!("LAN connection did not start: {e:#}"),
        }
    }
    let mut signal = if let Some(addr) = move_to.filter(|addr| !addr.trim().is_empty()) {
        tracing::info!("failover: dialing the new primary {addr}");
        SignalClient::connect_endpoint(addr, config, pjdesk_proto::routes::dial_budget(addr)).await?
    } else {
        SignalClient::connect_catalog(config, &mine).await?
    };
    let mut followed_redirect = false;
    tracing::info!("Reached the server via {}", signal.via);
    if !access.is_empty() {
        let _ = signal.send(ClientMsg::PresentAccess { token: access.clone() });
    }
    let connect = ClientMsg::Connect {
        proto: PROTOCOL_VERSION,
        target_id: target.into(),
        controller_name: crate::hostname(),
    };
    signal.send(connect.clone())?;

    tracing::info!("Waiting for {} to answer", format_id(target));
    let mut retried_without_token = false;
    let (session_id, token, udp_port, host_name) = loop {
        match timeout(host_wait, signal.recv()).await {
            Ok(None) if !access.is_empty() && !access_key && !retried_without_token => {
                retried_without_token = true;
                signal = SignalClient::connect_catalog(config, &mine).await?;
                signal.send(connect.clone())?;
                continue;
            }
            Ok(Some(ServerMsg::Error { message, .. }))
                if !followed_redirect && pjdesk_proto::signal::is_standby_refusal(&message) =>
            {
                followed_redirect = true;
                let Some((addr, routes)) = pjdesk_proto::signal::parse_standby_redirect(&message) else {
                    bail!("{message}");
                };
                if !routes.is_empty() {
                    config.network_routes = pjdesk_proto::routes::keep_other_server(&config.network_routes, &routes);
                    crate::routes_store::store(&config.network_routes);
                    let _ = config.save();
                }
                if addr.is_empty() {
                    bail!("{message}");
                }
                tracing::info!("Standby redirected the session to {addr}");
                signal = SignalClient::connect_endpoint(&addr, config, pjdesk_proto::routes::dial_budget(&addr)).await?;
                if !access.is_empty() {
                    let _ = signal.send(ClientMsg::PresentAccess { token: access.clone() });
                }
                signal.send(connect.clone())?;
                continue;
            }
            Ok(Some(ServerMsg::SessionReady { session_id, token, udp_port, host_name })) => {
                tracing::info!("Host accepted session {session_id:x}");
                cache_host_name(target, &host_name);
                if let Some(path) = ask {
                    let _ = std::fs::write(path, format!("ready\n{host_name}\n"));
                }
                break (session_id, token, udp_port, host_name);
            }
            Ok(Some(ServerMsg::Routes(routes))) => {
                if !routes.is_empty() {
                    config.network_routes = pjdesk_proto::routes::keep_other_server(&config.network_routes, &routes);
                    crate::routes_store::store(&config.network_routes);
                    let _ = config.save();
                }
            }
            Ok(Some(ServerMsg::Error { message, .. })) => bail!("{message}"),
            Ok(Some(_)) => continue,
            Ok(None) => bail!("the server closed the connection"),
            Err(_) => bail!("the host did not respond"),
        }
    };

    let server_udp = SocketAddr::new(signal.server_addr.ip(), udp_port);
    let binding = match p2p::bind_udp(server_udp, session_id, token, Role::Controller).await {
        Ok(binding) => binding,
        Err(e) => {
            tracing::info!("UDP to the relay failed: {e:#}. Trying the host on this network.");
            if !force_relay && !password.is_empty() {
                let heard = tokio::task::spawn_blocking({
                    let id = target.to_string();
                    move || note_lan_beacon(&id)
                })
                .await;
                if heard.is_ok() {
                    if let Ok(disk) = Config::load() {
                        config.fill_peer_facts_from(&disk);
                    }
                    match try_cached_lan(config, target, password, &mine).await {
                        Ok(Some(link)) => {
                            tracing::info!("Connected over the LAN after the relay's UDP handshake failed");
                            return Ok(link);
                        }
                        Ok(None) => {}
                        Err(err) if err.is::<WrongPassword>() || already_in_session(&err) => return Err(err),
                        Err(err) => tracing::info!("LAN connection did not start: {err:#}"),
                    }
                }
            }
            return Err(e);
        }
    };
    tracing::debug!("our public address for this session: {}", binding.observed);
    signal.send(ClientMsg::UdpReady {
        session_id,
        local_addrs: binding.local_addrs.clone(),
        cert_fingerprint: None,
    })?;

    let (peer_public, peer_local, fingerprint) = loop {
        match timeout(host_wait.min(Duration::from_secs(20)), signal.recv()).await {
            Ok(Some(ServerMsg::PeerInfo { public_addr, local_addrs, cert_fingerprint, .. })) => {
                tracing::info!(
                    "Host addresses: public {public_addr}, on this network {}",
                    if local_addrs.is_empty() {
                        "none reported".to_string()
                    } else {
                        local_addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ")
                    }
                );
                if let Some(fp) = cert_fingerprint {
                    let lan: Vec<String> = local_addrs
                        .iter()
                        .filter(|addr| p2p::is_lan_ip(addr.ip()))
                        .map(|addr| addr.to_string())
                        .collect();
                    Config::remember_lan(target, &lan, Some(&pjdesk_proto::tls::format_fingerprint(&fp)));
                }
                break (public_addr, local_addrs, cert_fingerprint.context("the server didn't send the host's key")?)
            }
            Ok(Some(ServerMsg::Error { message, .. })) => bail!("{message}"),
            Ok(Some(_)) => continue,
            Ok(None) => bail!("the server closed the connection"),
            Err(_) => bail!("the host did not finish setting up the connection"),
        }
    };
    // Signaling is done; everything else goes peer to peer.
    drop(signal);

    let endpoint = p2p::controller_endpoint(binding.socket, fingerprint)?;
    let candidates = p2p::candidates(peer_public, &peer_local);
    let ask_person = std::env::var("PJDESK_ASK_PERSON").ok().as_deref() == Some("1");
    let ask_view = std::env::var("PJDESK_ASK_VIEW").ok().as_deref() == Some("1");
    let mut attempt = if ask_view {
        let label = std::env::var("PJDESK_PEER_LABEL").unwrap_or_default();
        let label = if label.trim().is_empty() { host_name.clone() } else { label };
        crate::note_status(&format!("Waiting for {label} to accept"));
        Asked::View
    } else if ask_person {
        Asked::Person
    } else if let Some(path) = ask {
        wait_asked_password(path, &host_name).await?
    } else {
        Asked::Password(password.to_string())
    };
    let mut from_store = std::env::var("PJDESK_SAVED").ok().as_deref() == Some("1");
    let mut tries = 0u32;
    let (connection, path, auth) = loop {
        let punching = binding.punch_socket.try_clone().ok().map(|sock| {
            p2p::spawn_punching(sock, candidates.clone(), session_id, Duration::from_secs(8))
        });
        let connected = p2p::connect_to_host(&endpoint, &candidates, server_udp, force_relay).await;
        if let Some(punching) = punching {
            punching.abort();
        }
        let (connection, path) = connected?;
        let authed = match &attempt {
            Asked::View => {
                tracing::info!("Requesting view access to {}", format_id(target));
                let account = signed_in_account(config);
                let id = config.device_id.clone().unwrap_or_default();
                session::controller_ask_view(&connection, &account, &crate::hostname(), &id).await
            }
            Asked::Person => {
                tracing::info!("Asking the person at {}", format_id(target));
                let who = if host_name.trim().is_empty() { format_id(target) } else { host_name.clone() };
                crate::note_status(&format!("Waiting for {who} to accept..."));
                session::controller_ask_person_for(&connection, &presenter_name(), std::time::Duration::from_secs(60)).await
            }
            Asked::Password(password) => {
                tracing::info!("Checking the password with {}", format_id(target));
                session::controller_auth(&connection, password, &crate::hostname()).await
            }
        };
        match authed {
            Ok(auth) => {
                if let Some(path) = ask {
                    let _ = std::fs::write(path, "accepted\n");
                }
                break (connection, path, auth);
            }
            Err(e) if e.is::<session::LockedOut>() => {
                connection.close(1u32.into(), b"locked out");
                let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
                crate::note_failure(&e.to_string());
                std::process::exit(4);
            }
            Err(e) if e.is::<session::WrongPassword>() && ask.is_some() && tries < 4 => {
                connection.close(1u32.into(), b"authentication failed");
                tries += 1;
                let path = ask.unwrap();
                let _ = std::fs::write(path, format!("rejected\n{host_name}\n"));
                let _ = std::fs::remove_file(ask_reply_path(path));
                from_store = false;
                attempt = wait_asked_password(path, &host_name).await?;
            }
            Err(e) if e.is::<session::WrongPassword>() => {
                connection.close(1u32.into(), b"authentication failed");
                let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
                let mut text = format!("Incorrect password for {}", format_id(target));
                if from_store && is_valid_id(target) {
                    config.remember_password(target, None);
                    if let Some(entry) = config.address_book.iter_mut().find(|e| e.id == target) {
                        entry.password = None;
                    }
                    let _ = config.save();
                    text.push_str(". The saved password on this computer was cleared.");
                }
                crate::note_failure(&text);
                std::process::exit(3);
            }
            Err(e) => {
                connection.close(1u32.into(), b"authentication failed");
                let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
                tracing::error!("session failed: {e:#}");
                return Err(e);
            }
        }
    };
    let opened = if matches!(attempt, Asked::View) {
        format!("{} accepted the view. Starting the picture ({path}).", auth.peer_name)
    } else {
        format!("Password accepted by {}. Starting the picture ({path}).", auth.peer_name)
    };
    tracing::info!("{opened}");
    tracing::info!("{}", crate::host_log_hint());
    crate::note_status(&opened);
    tokio::time::sleep(Duration::from_millis(50)).await;
    if let Some(reason) = connection.close_reason() {
        let text = format!(
            "Password accepted by {}, then the host closed the session: {reason}",
            auth.peer_name
        );
        crate::note_failure(&text);
        bail!(text);
    }
    let reported = if auth.peer_name.is_empty() { host_name } else { auth.peer_name };
    cache_host_name(target, &reported);
    Ok(Link {
        connection,
        endpoint,
        control_send: auth.send,
        control_recv: auth.recv,
        host_name: reported,
        host_id: target.to_string(),
        path,
        peer_proto: auth.peer_proto,
        direct_targets: candidates,
        host_fingerprint: Some(fingerprint),
    })
}

fn ask_reply_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".reply");
    std::path::PathBuf::from(name)
}

enum Asked {
    Password(String),
    Person,
    View,
}

/// The pjdesk.com account when this computer is signed in. Empty otherwise,
/// so the phone names this computer instead.
fn signed_in_account(cfg: &crate::config::Config) -> String {
    let from_parent = std::env::var("PJDESK_ACCOUNT").unwrap_or_default();
    let email = if !from_parent.trim().is_empty() { from_parent } else { cfg.network_email.clone().unwrap_or_default() };
    let signed_in = !email.trim().is_empty()
        || !cfg.network_refresh_token().trim().is_empty()
        || !std::env::var("PJDESK_ACCESS").unwrap_or_default().trim().is_empty();
    if signed_in { email.trim().chars().filter(|c| !c.is_control()).take(80).collect() } else { String::new() }
}

/// Account and this computer's name, packed into the one field an ask can send.
fn presenter_name() -> String {
    let email = std::env::var("PJDESK_ACCOUNT").unwrap_or_default();
    let email = email.trim().to_string();
    let device = crate::hostname();
    if email.contains('@') {
        format!("{email} — {device}").chars().take(120).collect()
    } else {
        device
    }
}

/// Blocks until the window writes a password, asks the person, or cancels.
/// The host has already answered; this is the only password prompt for the attempt.
async fn wait_asked_password(path: &std::path::Path, host_name: &str) -> Result<Asked> {
    let current = std::fs::read_to_string(path).unwrap_or_default();
    if !current.starts_with("rejected") {
        let _ = std::fs::write(path, format!("ready\n{host_name}\n"));
    }
    let deadline = Instant::now() + Duration::from_secs(90);
    let reply = ask_reply_path(path);
    loop {
        if Instant::now() > deadline {
            bail!("the password was not entered in time");
        }
        if let Ok(text) = std::fs::read_to_string(&reply) {
            let _ = std::fs::remove_file(&reply);
            if text.starts_with("cancel") {
                bail!("cancelled");
            }
            if text.starts_with("ask") {
                return Ok(Asked::Person);
            }
            if let Some(hexed) = text.strip_prefix("password\n") {
                let bytes = hex::decode(hexed.trim()).unwrap_or_default();
                if let Ok(password) = String::from_utf8(bytes) {
                    if !password.is_empty() {
                        return Ok(Asked::Password(password));
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Writes the host's name onto the saved computer. SessionReady supplies it
/// before the password, and the handshake refreshes it after the password is accepted.
fn cache_host_name(id: &str, name: &str) {
    crate::config::Config::remember_hostname(id, name);
}

/// Direct IP: QUIC to `host:port` with the saved certificate pin (or first use).
async fn connect_direct(
    config: &mut Config,
    addr: &str,
    password: &str,
    ask: Option<&std::path::Path>,
    trust: bool,
) -> Result<Link> {
    let socket_addr: SocketAddr = addr
        .parse()
        .or_else(|_| format!("{addr}:{}", pjdesk_proto::DIRECT_PORT).parse())
        .with_context(|| format!("\"{addr}\" is not a host:port"))?;
    let key = socket_addr.to_string();
    let pinned = if trust {
        None
    } else {
        config.settings.direct_pins.get(&key).and_then(|s| pjdesk_proto::tls::parse_fingerprint(s).ok())
    };
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").context("opening a UDP socket")?;
    let (endpoint, verifier) = p2p::controller_endpoint_pin(udp, pinned)?;
    let connecting =
        endpoint.connect(socket_addr, pjdesk_proto::PEER_SERVER_NAME).context("starting the direct connection")?;
    let connection = timeout(Duration::from_secs(8), connecting).await.context("the computer did not answer")??;
    let label = socket_addr.to_string();
    if let Some(path) = ask {
        let _ = std::fs::write(path, format!("ready\n{label}\n"));
    }
    let asked = if let Some(path) = ask {
        wait_asked_password(path, &label).await?
    } else {
        Asked::Password(password.to_string())
    };
    if verifier.mismatch() {
        connection.close(1u32.into(), b"certificate mismatch");
        bail!("this computer's key changed. Connect again and choose to trust it.");
    }
    if pinned.is_none() {
        if let Some(fp) = verifier.observed() {
            config.settings.direct_pins.insert(key, pjdesk_proto::tls::format_fingerprint(&fp));
            let _ = config.save();
        }
    }
    let authed = match &asked {
        Asked::Person => session::controller_ask_person_for(&connection, &presenter_name(), std::time::Duration::from_secs(60)).await,
        Asked::View => {
            let account = signed_in_account(config);
            let id = config.device_id.clone().unwrap_or_default();
            session::controller_ask_view(&connection, &account, &crate::hostname(), &id).await
        }
        Asked::Password(password) => session::controller_auth(&connection, password, &crate::hostname()).await,
    };
    let auth = match authed {
        Ok(auth) => {
            if let Some(path) = ask {
                let _ = std::fs::write(path, "accepted\n");
            }
            auth
        }
        Err(e) if e.is::<session::LockedOut>() => {
            connection.close(1u32.into(), b"locked out");
            crate::note_failure(&e.to_string());
            std::process::exit(4);
        }
        Err(e) => {
            connection.close(1u32.into(), b"authentication failed");
            if e.is::<WrongPassword>() {
                if let Some(path) = ask {
                    let _ = std::fs::write(path, format!("rejected\n{label}\n"));
                }
                return Err(anyhow!("wrong password"));
            }
            return Err(e);
        }
    };
    Ok(Link {
        connection,
        endpoint,
        control_send: auth.send,
        control_recv: auth.recv,
        host_name: if auth.peer_name.is_empty() { socket_addr.to_string() } else { auth.peer_name },
        host_id: "000000000".into(),
        path: LinkPath::Direct(socket_addr),
        peer_proto: auth.peer_proto,
        direct_targets: vec![socket_addr],
        host_fingerprint: verifier.observed(),
    })
}

/// Test mode: receive and decode `frames` frames, then print statistics.
async fn headless(link: Link, frames: u32) -> Result<()> {
    let Link { connection, endpoint, mut control_send, .. } = link;
    let result = receive_frames(&connection, &mut control_send, frames).await;
    // Hang up on success and failure alike, so the host is free again at once.
    connection.close(0u32.into(), b"done");
    let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    result
}

async fn receive_frames(connection: &Connection, control_send: &mut SendStream, frames: u32) -> Result<()> {
    let mut video =
        timeout(Duration::from_secs(15), connection.accept_uni()).await.context("no video from the host")??;
    let mut decoder = decoder::VideoDecoder::new()?;
    let mut data = Vec::new();
    let mut rgb = Vec::new();
    let (mut decoded, mut keyframes, mut bytes, mut size) = (0u32, 0u32, 0usize, (0usize, 0usize));
    let started = Instant::now();

    while decoded < frames {
        // The host only sends frames when its screen changes.
        let header = timeout(Duration::from_secs(10), read_video_frame(&mut video, &mut data))
            .await
            .with_context(|| format!("no new frame for 10 s after {decoded} (is the host's screen idle?)"))??;
        bytes += data.len();
        keyframes += header.keyframe as u32;
        if let Some(dims) = decoder.decode(&data, &mut rgb)? {
            decoded += 1;
            size = dims;
        }
        write_msg(control_send, &ControlMsg::FrameAck { seq: header.seq }).await?;
    }
    let secs = started.elapsed().as_secs_f64();
    println!(
        "Decoded {decoded} frames of {}x{} in {secs:.1} s: {:.1} fps, {:.0} kbit/s, {keyframes} keyframe(s), RTT {} ms",
        size.0,
        size.1,
        decoded as f64 / secs,
        bytes as f64 * 8.0 / secs / 1000.0,
        connection.rtt().as_millis()
    );
    // Sample the middle row (the test pattern has red, green, blue bars there).
    if size.0 > 0 {
        let row = size.1 / 2 * size.0;
        let samples: Vec<String> =
            [1usize, 3, 5, 7].iter().map(|q| format!("#{:06x}", rgb[row + size.0 * q / 8])).collect();
        println!("Center-row colors: {}", samples.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{password_source, PasswordSource};

    #[test]
    fn view_phone_does_not_ask_the_console_for_a_password() {
        assert_eq!(password_source(None, false, true, false, false), PasswordSource::Empty);
        assert_eq!(password_source(None, false, false, true, false), PasswordSource::Empty);
        assert_eq!(password_source(None, true, false, false, false), PasswordSource::Empty);
    }

    #[test]
    fn a_window_with_no_console_does_not_open_conin() {
        assert_eq!(password_source(None, false, false, false, false), PasswordSource::NoConsole);
        assert_eq!(
            password_source(Some("secret"), false, false, false, false),
            PasswordSource::Given("secret".into())
        );
        assert_eq!(password_source(None, false, false, false, true), PasswordSource::Prompt);
    }
}
