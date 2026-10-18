//! Controller mode: connect to a host by ID and show its screen.

pub mod decoder;
mod controls;
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
    let password = match args.password.clone() {
        Some(password) => password,
        None => rpassword::prompt_password(format!("Password for {label}: "))?,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let mut waiting = false;
    let mut tries = 0u32;
    loop {
        let link = if let Some(addr) = direct.clone() {
            runtime.block_on(connect_direct(&mut config, &addr, &password, args.trust && !waiting))
        } else {
            runtime.block_on(connect(&server, &mut config, &target, &password, args.force_relay))
        };
        let link = match link {
            Ok(link) => link,
            Err(_) if waiting && tries < 40 => {
                tries += 1;
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
            Err(e) => return Err(e),
        };
        tries = 0;
        println!("Connected to \"{}\" ({}, {})", link.host_name, format_id(&link.host_id), link.path);
        if args.headless {
            return runtime.block_on(headless(link, args.frames));
        }
        if !window::run(&runtime, link)? {
            break;
        }
        waiting = true;
        println!("Waiting for the computer to come back…");
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
    force_relay: bool,
) -> Result<Link> {
    println!("Connecting to {} through {server}...", format_id(target));
    let mut signal = SignalClient::connect(server, config).await?;
    signal.send(ClientMsg::Connect {
        proto: PROTOCOL_VERSION,
        target_id: target.into(),
        controller_name: crate::hostname(),
    })?;

    let (session_id, token, udp_port, host_name) = loop {
        match timeout(Duration::from_secs(30), signal.recv()).await {
            Ok(Some(ServerMsg::SessionReady { session_id, token, udp_port, host_name })) => {
                break (session_id, token, udp_port, host_name)
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
    let punching = p2p::spawn_punching(binding.punch_socket, candidates.clone(), session_id, Duration::from_secs(5));
    let connected = p2p::connect_to_host(&endpoint, &candidates, server_udp, force_relay).await;
    punching.abort();
    let (connection, path) = connected?;

    let auth = match session::controller_auth(&connection, password, &crate::hostname()).await {
        Ok(auth) => auth,
        Err(e) => {
            // Hang up properly so the host is free again right away.
            connection.close(1u32.into(), b"authentication failed");
            let _ = timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
            return Err(if e.is::<WrongPassword>() { anyhow!("wrong password for {}", format_id(target)) } else { e });
        }
    };
    Ok(Link {
        connection,
        endpoint,
        control_send: auth.send,
        control_recv: auth.recv,
        host_name: if auth.peer_name.is_empty() { host_name } else { auth.peer_name },
        host_id: target.to_string(),
        path,
        peer_proto: auth.peer_proto,
    })
}

/// Direct IP: QUIC to `host:port` with the saved certificate pin (or first use).
async fn connect_direct(config: &mut Config, addr: &str, password: &str, trust: bool) -> Result<Link> {
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
    let connecting = endpoint.connect(socket_addr, pjdesk_proto::PEER_SERVER_NAME).context("starting the direct connection")?;
    let connection = timeout(Duration::from_secs(8), connecting).await.context("the computer did not answer")??;
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
    let auth = match session::controller_auth(&connection, password, &crate::hostname()).await {
        Ok(auth) => auth,
        Err(e) => {
            connection.close(1u32.into(), b"authentication failed");
            return Err(if e.is::<WrongPassword>() { anyhow!("wrong password") } else { e });
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
