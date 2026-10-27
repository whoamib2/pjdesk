//! Coordinated primary handoff.
//!
//! Stepping down asks the other server to take the next generation, then
//! checks the sealed cloud copy. This server becomes standby only after that
//! copy names the peer at a higher generation. If the peer does not take it,
//! this server stays the primary.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use hmac::{Hmac, Mac};
use pjdesk_proto::failover::{self, handoff_proof_is_fresh, peer_took_the_epoch, primary_result, standby_result};
use pjdesk_proto::signal::{ClientMsg, ServerMsg};
use pjdesk_proto::tls::{self, PinnedCertVerifier};
use pjdesk_proto::{framing::MAX_SIGNAL_FRAME, ALPN_SIGNAL, DEFAULT_PORT, SIGNAL_SERVER_NAME};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConnection, Stream};

use crate::cloud::{self, CloudStatus};

type HmacSha256 = Hmac<Sha256>;

const ACCEPTED_FILE: &str = "handoff-accepted.json";
const RESULT_FILE: &str = "role-result.json";
const ASK_BUDGET: Duration = Duration::from_secs(25);
const MOVE_BUDGET: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accepted {
    pub epoch: u64,
    pub instance: String,
    pub name: String,
    pub address: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoleResult {
    pub at: u64,
    pub ok: bool,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct TakeReply {
    pub ok: bool,
    pub epoch: u64,
    pub name: String,
    pub message: String,
}

impl TakeReply {
    fn fail(message: impl Into<String>) -> Self {
        Self { ok: false, epoch: 0, name: String::new(), message: message.into() }
    }
}

/// The other server's name, for the result sentence. Our own name is not it.
pub fn other_name(data_dir: &Path) -> String {
    let mine = cloud::computer_name();
    let status = CloudStatus::load(data_dir).unwrap_or_default();
    let candidate = if status.role == "standby" {
        status.active_name
    } else if !status.peer_name.is_empty() {
        status.peer_name
    } else {
        status.active_name
    };
    if candidate.trim().is_empty() || candidate.eq_ignore_ascii_case(&mine) {
        String::new()
    } else {
        candidate
    }
}

pub fn handoff_proof(key: &[u8; 32], epoch: u64, at: u64) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(b"pjdesk-handoff-1");
    mac.update(&epoch.to_le_bytes());
    mac.update(&at.to_le_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

fn proof_matches(key: &[u8; 32], epoch: u64, at: u64, proof: &[u8; 32]) -> bool {
    let expect = handoff_proof(key, epoch, at);
    let mut diff = 0u8;
    for (left, right) in expect.iter().zip(proof.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

/// The standby's answer to a handoff. Promoting is the act of taking the epoch.
pub fn accept_take_primary(data_dir: &Path, epoch: u64, at: u64, proof: &[u8; 32]) -> TakeReply {
    let Some(key) = cloud::sync_key(data_dir) else {
        return TakeReply::fail("Cloud sync is not set up on this server.");
    };
    if !handoff_proof_is_fresh(at, cloud::unix_now()) || !proof_matches(&key, epoch, at, proof) {
        return TakeReply::fail("The handoff was rejected.");
    }
    let local = cloud::local_epoch(data_dir);
    let name = cloud::computer_name();
    if cloud::accepts_host_registrations(data_dir) && local > epoch {
        let notice = CloudStatus::load(data_dir).map(|s| s.notice).unwrap_or_default();
        return TakeReply {
            ok: true,
            epoch: local,
            name,
            message: if notice.is_empty() { primary_result(local, "") } else { notice },
        };
    }
    if cloud::accepts_host_registrations(data_dir) {
        return TakeReply::fail("This server is already the primary.");
    }
    match cloud::promote(data_dir) {
        Ok(()) => {
            let epoch = cloud::local_epoch(data_dir);
            let notice = CloudStatus::load(data_dir).map(|s| s.notice).unwrap_or_default();
            TakeReply {
                ok: true,
                epoch,
                name: cloud::computer_name(),
                message: if notice.is_empty() { primary_result(epoch, "") } else { notice },
            }
        }
        Err(e) => TakeReply::fail(format!("{e:#}")),
    }
}

pub fn save_accepted(data_dir: &Path, accepted: &Accepted) {
    if let Ok(bytes) = serde_json::to_vec(accepted) {
        let _ = std::fs::write(data_dir.join(ACCEPTED_FILE), bytes);
    }
}

pub fn load_accepted(data_dir: &Path) -> Option<Accepted> {
    let bytes = std::fs::read(data_dir.join(ACCEPTED_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn clear_accepted(data_dir: &Path) {
    let _ = std::fs::remove_file(data_dir.join(ACCEPTED_FILE));
}

pub fn clear_result(data_dir: &Path) {
    let _ = std::fs::remove_file(data_dir.join(RESULT_FILE));
}

pub fn write_result(data_dir: &Path, ok: bool, message: &str) {
    let body = RoleResult { at: cloud::unix_now(), ok, message: message.to_string() };
    if let Ok(bytes) = serde_json::to_vec(&body) {
        let _ = std::fs::write(data_dir.join(RESULT_FILE), bytes);
    }
}

pub fn read_result(data_dir: &Path) -> Option<RoleResult> {
    let bytes = std::fs::read(data_dir.join(RESULT_FILE)).ok()?;
    let result: RoleResult = serde_json::from_slice(&bytes).ok()?;
    if result.message.trim().is_empty() {
        return None;
    }
    Some(result)
}

pub fn wait_result(data_dir: &Path, timeout: Duration) -> Option<RoleResult> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(result) = read_result(data_dir) {
            return Some(result);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

/// Pull the latest copy, write it as the next generation, and say so.
pub fn become_primary(data_dir: &Path) -> Result<String> {
    if cloud::status_is_fresh(data_dir) {
        clear_result(data_dir);
        crate::requests::push(data_dir, crate::requests::PROMOTE_FILE, "now")
            .map_err(|e| anyhow!("Could not ask the running server to become the primary: {e}"))?;
        return match wait_result(data_dir, Duration::from_secs(40)) {
            Some(result) if result.ok => Ok(result.message),
            Some(result) => bail!(result.message),
            None => bail!("The running server did not finish becoming the primary."),
        };
    }
    cloud::promote(data_dir)?;
    Ok(CloudStatus::load(data_dir)
        .map(|status| status.notice)
        .filter(|notice| !notice.trim().is_empty())
        .unwrap_or_else(|| primary_result(cloud::local_epoch(data_dir), "")))
}

/// Ask the peer to take the epoch. Stay primary unless the cloud copy says they did.
pub fn step_down(data_dir: &Path) -> Result<String> {
    let before = cloud::local_epoch(data_dir);
    let peer = other_name(data_dir);
    let accepted = ask_and_confirm(data_dir, before, &peer)?;
    let sentence = standby_result(&accepted.name, accepted.epoch);
    if !cloud::accepts_host_registrations(data_dir) {
        clear_accepted(data_dir);
        return Ok(sentence);
    }
    if cloud::status_is_fresh(data_dir) {
        clear_result(data_dir);
        save_accepted(data_dir, &accepted);
        crate::requests::push(data_dir, crate::requests::DEMOTE_FILE, "now")
            .map_err(|e| anyhow!("Could not ask the running server to step down: {e}"))?;
        let start = Instant::now();
        while start.elapsed() < MOVE_BUDGET {
            if let Some(result) = read_result(data_dir) {
                clear_accepted(data_dir);
                return if result.ok { Ok(result.message) } else { bail!(result.message) };
            }
            if !cloud::accepts_host_registrations(data_dir) {
                clear_accepted(data_dir);
                return Ok(sentence);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if !cloud::accepts_host_registrations(data_dir) {
            clear_accepted(data_dir);
            return Ok(sentence);
        }
        bail!("{sentence}, but this server has not finished stepping down yet.");
    }
    if cloud::demote(data_dir).is_err() {
        cloud::yield_to_epoch(data_dir, accepted.epoch, &accepted.instance, &accepted.name);
    }
    let mut applied = sentence.clone();
    if let Some(status) = CloudStatus::load(data_dir) {
        if !status.notice.trim().is_empty() {
            applied = status.notice;
        }
    }
    Ok(applied)
}

fn ask_and_confirm(data_dir: &Path, before: u64, peer: &str) -> Result<Accepted> {
    let who = if peer.trim().is_empty() { "The other server" } else { peer.trim() };
    let targets = cloud::peer_dial_targets(data_dir);
    if targets.is_empty() {
        bail!("There is no other server to hand the primary to. This server stayed the primary (generation {before}).");
    }
    let Some(key) = cloud::sync_key(data_dir) else {
        bail!("Cloud sync is not set up on this server. This server stayed the primary (generation {before}).");
    };
    let Some(fingerprint) = crate::server::read_identity(data_dir).map(|id| id.fingerprint()) else {
        bail!("This server has no key yet. This server stayed the primary (generation {before}).");
    };
    let at = cloud::unix_now();
    let proof = handoff_proof(&key, before, at);
    let msg = ClientMsg::TakePrimary { epoch: before, at, proof };
    let deadline = Instant::now() + ASK_BUDGET;
    let mut last_error = format!("{who} did not answer.");
    let mut reached = false;
    for target in &targets {
        if Instant::now() >= deadline {
            break;
        }
        let Some(addr) = resolve_target(target) else {
            last_error = format!("Could not look up {target}.");
            continue;
        };
        let remain = deadline.saturating_duration_since(Instant::now());
        match dial(addr, fingerprint, &msg, remain) {
            Ok(ServerMsg::Handoff { ok: true, epoch, name, .. }) => {
                reached = true;
                let owner = cloud::cloud_owner(data_dir)?;
                let mine = cloud::instance_of(data_dir);
                if peer_took_the_epoch(before, owner.epoch, &owner.instance, &mine) {
                    let name = if owner.name.trim().is_empty() { name } else { owner.name };
                    tracing::info!("failover: {name} took generation {}", owner.epoch);
                    return Ok(Accepted {
                        epoch: owner.epoch,
                        instance: owner.instance,
                        name,
                        address: target.clone(),
                    });
                }
                let _ = epoch;
                last_error = format!("{who} did not take the primary.");
            }
            Ok(ServerMsg::Handoff { message, .. }) => {
                reached = true;
                last_error = if message.trim().is_empty() {
                    format!("{who} did not take the primary.")
                } else {
                    message
                };
            }
            Ok(_) => {
                reached = true;
                last_error = format!("{who} did not take the primary.");
            }
            Err(e) => {
                tracing::info!("failover: handoff to {target} failed: {e:#}");
                last_error = format!("{who} did not answer.");
            }
        }
    }
    if !reached {
        bail!("{who} did not answer. This server stayed the primary (generation {before}).");
    }
    if last_error.contains("stayed the primary") {
        bail!(last_error);
    }
    bail!("{last_error} This server stayed the primary (generation {before}).");
}

pub(crate) fn resolve_target(address: &str) -> Option<SocketAddr> {
    let text = address.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(addr) = text.parse::<SocketAddr>() {
        return Some(addr);
    }
    let with_port = match text.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => text.to_string(),
        _ => format!("{text}:{DEFAULT_PORT}"),
    };
    with_port.to_socket_addrs().ok()?.next()
}

pub(crate) fn dial(addr: SocketAddr, fingerprint: pjdesk_proto::tls::Fingerprint, msg: &ClientMsg, budget: Duration) -> Result<ServerMsg> {
    let connect_for = budget.min(Duration::from_secs(4));
    if connect_for.is_zero() {
        bail!("timed out");
    }
    let tcp = TcpStream::connect_timeout(&addr, connect_for).with_context(|| format!("connecting to {addr}"))?;
    tcp.set_nodelay(true).ok();
    let read_for = budget.min(Duration::from_secs(20));
    tcp.set_read_timeout(Some(read_for)).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(8))).ok();
    let config = tls::client_config(PinnedCertVerifier::new(Some(fingerprint)), ALPN_SIGNAL)?;
    let name = ServerName::try_from(SIGNAL_SERVER_NAME).map_err(|_| anyhow!("bad server name"))?;
    let mut conn = ClientConnection::new(Arc::new(config), name)?;
    let mut tcp = tcp;
    let mut stream = Stream::new(&mut conn, &mut tcp);
    write_frame(&mut stream, msg)?;
    read_frame(&mut stream)
}

fn write_frame(stream: &mut impl Write, msg: &ClientMsg) -> Result<()> {
    let body = postcard::to_stdvec(msg)?;
    if body.len() > MAX_SIGNAL_FRAME {
        bail!("handoff request is too large");
    }
    stream.write_all(&(body.len() as u32).to_be_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;
    Ok(())
}

fn read_frame(stream: &mut impl Read) -> Result<ServerMsg> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_SIGNAL_FRAME {
        bail!("handoff reply is too large");
    }
    let mut body = vec![0u8; n];
    stream.read_exact(&mut body)?;
    postcard::from_bytes(&body).context("the other server did not accept the handoff")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proof_matches_only_the_same_epoch_and_time() {
        let key = [7u8; 32];
        let proof = handoff_proof(&key, 6, 1_700_000_000);
        assert!(proof_matches(&key, 6, 1_700_000_000, &proof));
        assert!(!proof_matches(&key, 7, 1_700_000_000, &proof));
        assert!(!proof_matches(&key, 6, 1_700_000_001, &proof));
        let other = [8u8; 32];
        assert!(!proof_matches(&other, 6, 1_700_000_000, &proof));
    }

    #[test]
    fn a_missing_peer_does_not_count_as_taking_the_epoch() {
        assert!(!failover::peer_took_the_epoch(6, 6, "chip", "lenovo"));
        assert_eq!(
            standby_result("CHIP20", 7),
            "Now standby; CHIP20 is primary, gen 7"
        );
    }
}
