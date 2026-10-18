//! Direct IP connections and LAN beacons.
//!
//! A host that allows it keeps a QUIC listener on [`pjdesk_proto::DIRECT_PORT`].
//! The controller pins the certificate on first use. A mismatch is refused.
//! The beacon carries the fingerprint and never a password.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use pjdesk_proto::discover::{self, LanBeacon};
use pjdesk_proto::{DIRECT_PORT, DISCOVERY_PORT, PROTOCOL_VERSION};
use tracing::{info, warn};

use super::{approve, ctrl, stream, HostState};
use crate::p2p;
use crate::session::{self, WrongPassword};

pub async fn serve(state: Arc<HostState>) -> Result<()> {
    let socket = std::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], DIRECT_PORT)))?;
    socket.set_nonblocking(true)?;
    let endpoint = p2p::host_endpoint(socket, &state.identity)?;
    info!("Listening for direct connections on UDP {DIRECT_PORT}");
    let beacon = state.clone();
    tokio::spawn(async move { announce(beacon).await });

    while let Some(incoming) = endpoint.accept().await {
        if state.refuse_reason().is_some() {
            incoming.refuse();
            continue;
        }
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = one(state, incoming).await {
                if e.is::<WrongPassword>() {
                    warn!("Direct connection used a wrong password");
                } else {
                    warn!("Direct connection ended: {e:#}");
                }
            }
        });
    }
    Ok(())
}

async fn one(state: Arc<HostState>, incoming: quinn::Incoming) -> Result<()> {
    let connection = incoming.await?;
    let attempt = Arc::new(AtomicBool::new(false));
    let passwords = state.auth_passwords();
    let auth = session::host_auth(&connection, &passwords, &state.name, &attempt).await?;
    state.reset_failures();
    let ip = connection.remote_address().to_string();
    let Some((mut ceiling, receive_dir)) = approve(&state, &auth.peer_name, &ip).await else {
        connection.close(3u32.into(), b"declined");
        return Ok(());
    };
    if state.streaming.swap(true, Ordering::SeqCst) {
        connection.close(2u32.into(), b"another controller is already connected");
        return Ok(());
    }
    if let Some(shared) = &state.shared {
        *shared.connected.lock().unwrap() = Some(auth.peer_name.clone());
    }
    let policy = state.policy.lock().unwrap().clone();
    ceiling &= policy.max_permissions;
    if !policy.recording_allowed {
        ceiling &= !pjdesk_proto::perms::RECORD;
    }
    let mut ports = state.args.tunnel_ports.clone();
    ports.extend(policy.tunnel_ports.iter().copied());
    ports.sort_unstable();
    ports.dedup();
    let grant = ctrl::Grant {
        ceiling,
        peer_proto: auth.peer_proto,
        peer_name: auth.peer_name,
        tunnel_remote: policy.tunnel_remote_hosts,
        tunnel_ports: ports,
        receive_dir,
    };
    let result = stream::run(connection.clone(), auth.send, auth.recv, state.args.clone(), grant, state.shared.clone()).await;
    state.streaming.store(false, Ordering::SeqCst);
    if let Some(shared) = &state.shared {
        *shared.connected.lock().unwrap() = None;
    }
    connection.close(0u32.into(), b"session over");
    result
}

async fn announce(state: Arc<HostState>) {
    let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") else { return };
    let _ = socket.set_broadcast(true);
    let dest = SocketAddr::from(([255, 255, 255, 255], DISCOVERY_PORT));
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let id = state.device_id.lock().unwrap().clone();
        if id.is_empty() {
            continue;
        }
        let beacon = LanBeacon {
            proto: PROTOCOL_VERSION,
            id,
            port: DIRECT_PORT,
            name: state.name.clone(),
            fingerprint: state.identity.fingerprint(),
        };
        if let Ok(bytes) = discover::encode(&beacon) {
            let _ = socket.send_to(&bytes, dest);
        }
    }
}

/// Listens for LAN beacons for a few seconds. Used by the connect screen.
pub fn discover_for(limit: Duration) -> Vec<(LanBeacon, SocketAddr)> {
    let socket = match std::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], DISCOVERY_PORT))) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let _ = socket.set_nonblocking(false);
    let _ = socket.set_read_timeout(Some(Duration::from_millis(400)));
    let deadline = std::time::Instant::now() + limit;
    let mut found = Vec::new();
    let mut buf = [0u8; 512];
    while std::time::Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Ok(beacon) = discover::decode(&buf[..n]) {
                    if !found.iter().any(|(b, _): &(LanBeacon, SocketAddr)| b.id == beacon.id && b.fingerprint == beacon.fingerprint)
                    {
                        found.push((beacon, src));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    found
}
