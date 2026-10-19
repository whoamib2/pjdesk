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
    let password = if ask.is_some() {
        String::new()
    } else {
        match args.password.clone() {
            Some(password) => password,
            None => rpassword::prompt_password(format!("Password for {label}: "))?,
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?;
    let mut waiting = false;
    let mut tries = 0u32;
    loop {
        let link = if let Some(addr) = direct.clone() {
            runtime.block_on(connect_direct(&mut config, &addr, &password, ask.as_deref(), args.trust && !waiting))
        } else {
            runtime.block_on(connect(&server, &mut config, &target, &password, ask.as_deref(), args.force_relay, waiting))
        };
        let link = match link {
            Ok(link) => link,
            Err(_) if waiting && tries < 40 => {
                tries += 1;
                std::thread::sleep(Duration::from_secs(3));
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
        if !window::run(&runtime, link)? {
            break;
        }
        waiting = true;
        tracing::info!("Waiting for the computer to come back");
        std::thread::sleep(Duration::from_secs(5));
    }
    Ok(())
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
) -> Result<Link> {
    tracing::info!("Connecting to {} through {server}", format_id(target));
    if waiting_retry {
        crate::network::refresh_config_now(config);
    } else {
        crate::network::refresh_config(config);
    }
    let server = ServerAddr::parse(&config.effective_server()).unwrap_or_else(|_| server.clone());
    let mut access = config.network_access_token();
    if access.is_empty() {
        // A temporary sign-in keeps the access token in this process only.
        access = std::env::var("PJDESK_ACCESS").unwrap_or_default();
    }
    let mut signal = SignalClient::connect(&server, config).await?;
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
        match timeout(Duration::from_secs(90), signal.recv()).await {
            Ok(None) if !access.is_empty() && !retried_without_token => {
                retried_without_token = true;
                signal = SignalClient::connect(&server, config).await?;
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
            Ok(Some(ServerMsg::Error { message, .. })) => bail!("{message}"),
            Ok(Some(_)) => continue,
            Ok(None) => bail!("the server closed the connection"),
            Err(_) => bail!("the host did not respond"),
        }
    };

    let server_udp = SocketAddr::new(signal.server_addr.ip(), udp_port);
    let binding = p2p::bind_udp(server_udp, session_id, token, Role::Controller).await?;
    tracing::debug!("our public address for this session: {}", binding.observed);
    signal.send(ClientMsg::UdpReady {
        session_id,
        local_addrs: binding.local_addrs.clone(),
        cert_fingerprint: None,
    })?;

    let (peer_public, peer_local, fingerprint) = loop {
        match timeout(Duration::from_secs(20), signal.recv()).await {
            Ok(Some(ServerMsg::PeerInfo { public_addr, local_addrs, cert_fingerprint, .. })) => {
                tracing::info!(
                    "Host addresses: public {public_addr}, on this network {}",
                    if local_addrs.is_empty() {
                        "none reported".to_string()
                    } else {
                        local_addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ")
                    }
                );
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
                session::controller_ask_person(&connection, &crate::hostname()).await
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
        Asked::Person => session::controller_ask_person(&connection, &crate::hostname()).await,
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
