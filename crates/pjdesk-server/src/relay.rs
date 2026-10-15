//! UDP rendezvous and relay.
//!
//! One UDP socket does three jobs:
//! * answers `Bind` datagrams, recording each peer's public address;
//! * once both peers of a session are bound, forwards every non-control
//!   datagram from one peer's address to the other's (the relay);
//! * expires idle relay routes.
//!
//! Peers send their QUIC packets to this port only when a direct connection
//! failed. The packets are end-to-end encrypted; the relay never sees content.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use pjdesk_proto::signal::SessionId;
use pjdesk_proto::udp::{self, Role, UdpMsg};
use tokio::net::UdpSocket;
use tracing::{debug, info};

use crate::server::Shared;

/// Relay routes with no traffic for this long are removed.
const ROUTE_IDLE: Duration = Duration::from_secs(120);

struct Route {
    to: SocketAddr,
    session_id: SessionId,
    last_seen: Instant,
    bytes: u64,
}

pub async fn run(socket: Arc<UdpSocket>, state: Shared) -> Result<()> {
    let mut buf = vec![0u8; 65_535];
    let mut routes: HashMap<SocketAddr, Route> = HashMap::new();
    let mut last_sweep = Instant::now();

    loop {
        let (len, src) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                // Windows reports an ICMP "port unreachable" caused by an
                // earlier send as an error on the next receive. Not fatal.
                debug!("udp recv error (ignored): {e}");
                continue;
            }
        };
        let packet = &buf[..len];

        if udp::is_control(packet) {
            if let Some(UdpMsg::Bind { session_id, token, role }) = udp::decode(packet) {
                if let Some(reply) = handle_bind(&state, &mut routes, src, session_id, token, role) {
                    let _ = socket.send_to(&reply, src).await;
                }
            }
        } else if let Some(route) = routes.get_mut(&src) {
            route.last_seen = Instant::now();
            route.bytes += len as u64;
            let to = route.to;
            let _ = socket.send_to(packet, to).await;
        }

        if last_sweep.elapsed() > Duration::from_secs(10) {
            last_sweep = Instant::now();
            routes.retain(|addr, route| {
                let keep = route.last_seen.elapsed() < ROUTE_IDLE;
                if !keep && route.bytes > 0 {
                    info!(
                        "Session {:016x}: relay route {addr} → {} closed after {:.1} MB",
                        route.session_id,
                        route.to,
                        route.bytes as f64 / 1e6
                    );
                }
                keep
            });
        }
    }
}

/// Validates a Bind, records the peer's public address and, when both sides
/// are known, installs the relay routes. Returns the BindAck to send.
fn handle_bind(
    state: &Shared,
    routes: &mut HashMap<SocketAddr, Route>,
    src: SocketAddr,
    session_id: SessionId,
    token: [u8; 16],
    role: Role,
) -> Option<Vec<u8>> {
    let mut st = state.lock().unwrap();
    let session = st.sessions.get_mut(&session_id)?;
    if !session.accepted || !constant_time_eq(&session.token_for(role), &token) {
        return None;
    }
    // Never hijack an address that is actively relaying another session
    // (live relays carry QUIC keep-alives every few seconds).
    if let Some(existing) = routes.get(&src) {
        if existing.session_id != session_id && existing.last_seen.elapsed() < Duration::from_secs(10) {
            return None;
        }
    }
    let first_time = session.slot_mut(role).public != Some(src);
    session.slot_mut(role).public = Some(src);

    if let (Some(host), Some(ctrl)) = (session.host.public, session.controller.public) {
        if first_time {
            let now = Instant::now();
            routes.insert(host, Route { to: ctrl, session_id, last_seen: now, bytes: 0 });
            routes.insert(ctrl, Route { to: host, session_id, last_seen: now, bytes: 0 });
            debug!("Session {session_id:016x}: relay ready between {host} and {ctrl}");
        }
    }
    session.maybe_send_peer_info(session_id);

    Some(udp::encode(&UdpMsg::BindAck { session_id, observed: src }))
}

/// Compares tokens without revealing (through timing) how many bytes matched.
fn constant_time_eq(a: &[u8; 16], b: &[u8; 16]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
