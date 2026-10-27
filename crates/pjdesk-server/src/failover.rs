//! Fast standby watch. The 30 s cloud sync still moves the sealed copy.
//! This loop only decides whether to promote or step down, and it logs why.

use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::path::Path;
use std::time::Duration;

use pjdesk_proto::failover::{
    confirming_path, is_same_party, next_misses, on_return, takeover, unhealthy_hold_until, Probe, SecondPath, Takeover,
    Watch, Yield, MISSES_TO_PROMOTE,
};
use pjdesk_proto::udp::{self, UdpMsg};

use crate::cloud::{self, PeerHealth};

/// One pass of the watch. The caller promotes, reloads, or keeps waiting.
#[derive(Debug)]
pub enum Step {
    /// Nothing to do. `log` is empty when the reason did not change.
    Stay { misses: u32, log: String },
    Promote { from: String, misses: u32 },
    /// Another server holds a newer generation. Migrate relayed sessions to
    /// `primary`, then record standby. This does not write the role by itself.
    YieldPending { epoch: u64, name: String, instance: String, primary: String },
}

/// TCP connect, then a UDP datagram. An answer on either one means the
/// primary is up. UDP silence is not proof: the relay does not reply to a
/// bare punch. An ICMP unreachable is silence for this purpose too.
pub fn probe_primary(addr: SocketAddr) -> Probe {
    if TcpStream::connect_timeout(&addr, Duration::from_millis(1200)).is_ok() {
        return Probe::Reachable;
    }
    if udp_answered(addr) {
        Probe::UdpOnly
    } else {
        Probe::Silent
    }
}

fn udp_answered(addr: SocketAddr) -> bool {
    let Ok(socket) = UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)) else {
        return false;
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(400)));
    let punch = udp::encode(&UdpMsg::Punch { session_id: 0 });
    if socket.send_to(&punch, addr).is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    matches!(socket.recv_from(&mut buf), Ok((n, _)) if n > 0)
}

/// Publishes this server's health, then either steps down for a higher
/// generation or counts probes against the primary.
pub fn watch_once(data_dir: &Path, misses: u32) -> Step {
    if cloud::CloudConfig::load(data_dir).is_none() {
        return Step::Stay { misses: 0, log: String::new() };
    }
    // The 3-second probe stays on the LAN. pjdesk.com is the 5-minute batch.
    if !cloud::accepts_host_registrations(data_dir) {
        crate::account_mirror::pull_replica(data_dir);
    }
    let mine = cloud::instance_of(data_dir);
    let local_epoch = cloud::local_epoch(data_dir);
    let primary = cloud::accepts_host_registrations(data_dir);
    // The primary reads cloud health on the batch cadence. The standby reads
    // it only after the direct probe has already failed, and then at most
    // once a minute.
    let (read_ok, peers) = if primary {
        cloud::peer_health_cached(data_dir, pjdesk_proto::failover::CLOUD_BATCH_SECS, true)
    } else {
        cloud::peer_health_cached(data_dir, u64::MAX, false)
    };
    if primary {
        // A standby is supposed to see a higher generation on the primary.
        // Only the server that is currently accepting computers steps down.
        if let Some(peer) = higher_peer(&peers, &mine, local_epoch) {
            let healthy_for = Duration::from_secs(0);
            if on_return(local_epoch, peer.epoch, false, healthy_for, false) == Yield::Demote
                && !cloud::already_fenced(data_dir, peer.epoch, &peer.instance)
            {
                let primary = pjdesk_proto::failover::move_target(&peer.public_addrs, &peer.lan_addrs, &peer.names);
                return Step::YieldPending {
                    epoch: peer.epoch,
                    name: peer.name.clone(),
                    instance: peer.instance.clone(),
                    primary,
                };
            }
        }
        return Step::Stay { misses: 0, log: String::new() };
    }
    let Some(address) = cloud::peer_target(data_dir) else {
        return Step::Stay {
            misses,
            log: "failover: no primary address yet; staying standby".into(),
        };
    };
    let Ok(addr) = resolve_probe(&address) else {
        return Step::Stay {
            misses,
            log: format!("failover: could not look up {address}; staying standby"),
        };
    };
    let probe = probe_primary(addr);
    let counted = next_misses(misses, probe);
    let (read_ok, peers) = if counted >= MISSES_TO_PROMOTE && matches!(probe, Probe::Silent) {
        cloud::peer_health_cached(data_dir, 60, true)
    } else {
        (read_ok, peers)
    };
    let now = cloud::unix_now();
    let mine_name = cloud::computer_name();
    let other = newest_other(&peers, &mine, &mine_name);
    let observed = match other {
        Some(peer) if peer.maintenance_until > now => peer.maintenance_until,
        Some(peer) if !peer.healthy => unhealthy_hold_until(peer.at),
        _ => 0,
    };
    let clear = read_ok && other.is_some_and(|peer| peer.healthy && peer.maintenance_until <= now);
    let until = if !read_ok {
        cloud::note_peer_maintenance(data_dir, 0, false)
    } else if clear {
        cloud::note_peer_maintenance(data_dir, 0, true)
    } else {
        cloud::note_peer_maintenance(data_dir, observed, false)
    };
    let other_at = other.map(|peer| peer.at);
    let second = confirming_path(read_ok, other_at, until, now);
    let decision = takeover(&Watch {
        misses: counted,
        probe,
        second,
        already_primary: false,
        have_address: true,
    });
    match decision {
        Takeover::Promote => Step::Promote { from: address, misses: counted },
        Takeover::Stay(reason) => {
            let age = other_at.map(|at| cloud::unix_now().saturating_sub(at));
            let log = match (probe, second) {
                (Probe::Silent, _) if counted > 0 && counted < MISSES_TO_PROMOTE => {
                    format!("failover: missed heartbeat {counted}/{MISSES_TO_PROMOTE} to {address}")
                }
                (Probe::Silent, SecondPath::Fresh) if until > cloud::unix_now() => {
                    format!("failover: primary announced maintenance; not taking over")
                }
                (Probe::Silent, SecondPath::Fresh) => format!(
                    "failover: primary still alive via cloud health ({} s old); not taking over",
                    age.unwrap_or(0)
                ),
                (Probe::Silent, SecondPath::Unknown) => {
                    format!("failover: cannot reach {address} and cloud health could not be read; not taking over")
                }
                (Probe::Reachable, _) | (Probe::UdpOnly, _) if misses > 0 => {
                    format!("failover: primary {address} answered; staying standby")
                }
                (Probe::Reachable, _) | (Probe::UdpOnly, _) => String::new(),
                _ => format!("failover: {reason}"),
            };
            Step::Stay { misses: counted, log }
        }
    }
}

fn higher_peer<'a>(peers: &'a [PeerHealth], mine: &str, local_epoch: u64) -> Option<&'a PeerHealth> {
    let mine_name = cloud::computer_name();
    peers
        .iter()
        .filter(|p| !is_same_party(&p.instance, &p.name, mine, &mine_name) && p.epoch > local_epoch)
        .max_by_key(|p| p.epoch)
}

fn newest_other<'a>(peers: &'a [PeerHealth], mine: &str, mine_name: &str) -> Option<&'a PeerHealth> {
    peers.iter().filter(|p| !is_same_party(&p.instance, &p.name, mine, mine_name)).max_by_key(|p| p.at)
}

fn resolve_probe(address: &str) -> Result<SocketAddr, ()> {
    let text = address.trim();
    let with_port = if host_has_port(text) {
        text.to_string()
    } else {
        format!("{text}:{}", pjdesk_proto::DEFAULT_PORT)
    };
    with_port.to_socket_addrs_fallback()
}

fn host_has_port(text: &str) -> bool {
    match text.rsplit_once(':') {
        Some((host, port)) => !host.is_empty() && !host.contains(']') && port.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

trait ToSocket {
    fn to_socket_addrs_fallback(&self) -> Result<SocketAddr, ()>;
}

impl ToSocket for String {
    fn to_socket_addrs_fallback(&self) -> Result<SocketAddr, ()> {
        use std::net::ToSocketAddrs;
        self.to_socket_addrs().map_err(|_| ())?.next().ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn an_open_port_answers_and_a_closed_port_does_not_promote_by_itself() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        assert_eq!(probe_primary(addr), Probe::Reachable);
        drop(listener);
        assert_eq!(probe_primary(addr), Probe::Silent);
    }
}
