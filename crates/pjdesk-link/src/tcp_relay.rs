//! QUIC over the signalling TLS connection (0.6.67+).
//!
//! When this side's UDP to the server's relay does not work, the session's
//! QUIC packets travel inside [`ClientMsg::RelayData`] and
//! [`ServerMsg::RelayData`](pjdesk_proto::signal::ServerMsg::RelayData)
//! messages on the TLS connection that is already open to the server (TCP
//! 21200). QUIC, the certificate pin and the password check are unchanged;
//! only the socket underneath is different. Every packet appears to come
//! from, and goes to, the server's relay address.

use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

use pjdesk_proto::signal::{ClientMsg, SessionId};
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tokio::sync::mpsc;

/// Sending budget. Above this, packets are dropped like on a congested link so
/// QUIC backs off instead of queueing behind TCP. About 16 Mbit/s.
pub const SEND_BYTES_PER_SEC: f64 = 2.0 * 1024.0 * 1024.0;
/// Short bursts (a keyframe) may go above the rate by this much.
pub const SEND_BURST_BYTES: f64 = 256.0 * 1024.0;

/// A QUIC socket whose datagrams go over the signalling connection.
pub struct TcpTunnelSocket {
    session_id: SessionId,
    out: mpsc::UnboundedSender<ClientMsg>,
    incoming: Mutex<mpsc::UnboundedReceiver<Vec<u8>>>,
    /// The server's relay address: every packet comes from here.
    server: SocketAddr,
    bucket: Mutex<Bucket>,
}

/// Feeds packets that arrived as `ServerMsg::RelayData` into the socket.
pub type TunnelFeed = mpsc::UnboundedSender<Vec<u8>>;

impl TcpTunnelSocket {
    pub fn new(session_id: SessionId, out: mpsc::UnboundedSender<ClientMsg>, server: SocketAddr) -> (Arc<Self>, TunnelFeed) {
        let (feed, incoming) = mpsc::unbounded_channel();
        let socket = Arc::new(Self {
            session_id,
            out,
            incoming: Mutex::new(incoming),
            server,
            bucket: Mutex::new(Bucket::new(SEND_BYTES_PER_SEC, SEND_BURST_BYTES)),
        });
        (socket, feed)
    }
}

impl fmt::Debug for TcpTunnelSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpTunnelSocket").field("session", &format_args!("{:016x}", self.session_id)).finish()
    }
}

/// Token bucket: `rate` bytes per second, up to `burst` saved.
pub struct Bucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    at: Instant,
}

impl Bucket {
    pub fn new(rate: f64, burst: f64) -> Self {
        Self { rate, burst, tokens: burst, at: Instant::now() }
    }

    /// True when `len` bytes may be sent at `now`.
    pub fn take(&mut self, len: usize, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = now;
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        if self.tokens >= len as f64 {
            self.tokens -= len as f64;
            true
        } else {
            false
        }
    }
}

/// Splits a transmit into one datagram per segment (GSO is off, but be safe).
fn segments<'a>(transmit: &'a Transmit<'a>) -> impl Iterator<Item = &'a [u8]> {
    let size = transmit.segment_size.unwrap_or(transmit.contents.len()).max(1);
    transmit.contents.chunks(size)
}

impl AsyncUdpSocket for TcpTunnelSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(AlwaysWritable)
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        let now = Instant::now();
        for packet in segments(transmit) {
            // Over budget: drop it, as a full link would. QUIC sees the loss.
            if !self.bucket.lock().unwrap().take(packet.len(), now) {
                continue;
            }
            let msg = ClientMsg::RelayData { session_id: self.session_id, data: packet.to_vec() };
            if self.out.send(msg).is_err() {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "the server connection closed"));
            }
        }
        Ok(())
    }

    fn poll_recv(&self, cx: &mut Context, bufs: &mut [IoSliceMut<'_>], meta: &mut [RecvMeta]) -> Poll<io::Result<usize>> {
        let mut incoming = self.incoming.lock().unwrap();
        let mut n = 0;
        while n < bufs.len().min(meta.len()) {
            let packet = if n == 0 {
                match incoming.poll_recv(cx) {
                    Poll::Ready(Some(p)) => p,
                    Poll::Ready(None) => {
                        return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "the server connection closed")))
                    }
                    Poll::Pending => return Poll::Pending,
                }
            } else {
                match incoming.try_recv() {
                    Ok(p) => p,
                    Err(_) => break,
                }
            };
            let len = packet.len().min(bufs[n].len());
            bufs[n][..len].copy_from_slice(&packet[..len]);
            meta[n] = RecvMeta { addr: self.server, len, stride: len, ecn: None, dst_ip: None };
            n += 1;
        }
        Poll::Ready(Ok(n))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
    }
}

/// A signalling connection the pump can read.
pub trait ServerMessages: Send + 'static {
    fn next_msg(&mut self) -> impl std::future::Future<Output = Option<pjdesk_proto::signal::ServerMsg>> + Send;
}

impl ServerMessages for crate::signal::SignalClient {
    async fn next_msg(&mut self) -> Option<pjdesk_proto::signal::ServerMsg> {
        self.recv().await
    }
}

impl ServerMessages for mpsc::UnboundedReceiver<pjdesk_proto::signal::ServerMsg> {
    async fn next_msg(&mut self) -> Option<pjdesk_proto::signal::ServerMsg> {
        self.recv().await
    }
}

/// Keeps the signalling connection open for the session and hands relayed
/// packets to the tunnel. Ends when the QUIC endpoint is gone or the server
/// closes the connection.
pub async fn pump<S: ServerMessages>(mut signal: S, feed: TunnelFeed) {
    loop {
        tokio::select! {
            _ = feed.closed() => break,
            msg = signal.next_msg() => match msg {
                Some(pjdesk_proto::signal::ServerMsg::RelayData { data, .. }) => {
                    if feed.send(data).is_err() {
                        break;
                    }
                }
                Some(pjdesk_proto::signal::ServerMsg::TcpRelayReady { .. }) => {
                    tracing::info!("The server is relaying this session over TCP");
                }
                Some(_) => {}
                None => {
                    tracing::info!("The server closed the connection that carried the session");
                    break;
                }
            },
        }
    }
}

#[derive(Debug)]
struct AlwaysWritable;

impl UdpPoller for AlwaysWritable {
    fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::p2p;
    use pjdesk_proto::signal::ServerMsg;
    use pjdesk_proto::tls::TlsIdentity;

    #[test]
    fn the_bucket_allows_a_burst_then_the_rate() {
        let start = Instant::now();
        let mut bucket = Bucket::new(1000.0, 3000.0);
        assert!(bucket.take(3000, start));
        assert!(!bucket.take(1, start));
        assert!(bucket.take(500, start + Duration::from_millis(500)));
        assert!(!bucket.take(600, start + Duration::from_millis(500)));
    }

    /// A stand-in server: forwards RelayData between two tunnels, as the
    /// relay does when both sides use TCP.
    fn wire(
        mut from: mpsc::UnboundedReceiver<ClientMsg>,
        to: TunnelFeed,
    ) {
        tokio::spawn(async move {
            while let Some(msg) = from.recv().await {
                if let ClientMsg::RelayData { data, .. } = msg {
                    // What the client's pump does with ServerMsg::RelayData.
                    let relayed = ServerMsg::RelayData { session_id: 1, data };
                    if let ServerMsg::RelayData { data, .. } = relayed {
                        let _ = to.send(data);
                    }
                }
            }
        });
    }

    #[tokio::test]
    async fn quic_runs_over_two_tunnels() {
        let server: SocketAddr = "192.0.2.1:21200".parse().unwrap();
        let (host_out, host_wire) = mpsc::unbounded_channel();
        let (ctrl_out, ctrl_wire) = mpsc::unbounded_channel();
        let (host_sock, host_feed) = TcpTunnelSocket::new(1, host_out, server);
        let (ctrl_sock, ctrl_feed) = TcpTunnelSocket::new(1, ctrl_out, server);
        wire(host_wire, ctrl_feed);
        wire(ctrl_wire, host_feed);

        let identity = TlsIdentity::generate(pjdesk_proto::PEER_SERVER_NAME).unwrap();
        let fingerprint = identity.fingerprint();
        let host = p2p::host_endpoint_on(host_sock, &identity).unwrap();
        let ctrl = p2p::controller_endpoint_on(ctrl_sock, Some(fingerprint)).unwrap().0;

        let accept = tokio::spawn(async move {
            let conn = host.accept().await.unwrap().await.unwrap();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let got = recv.read_to_end(1 << 20).await.unwrap();
            send.write_all(&got).await.unwrap();
            send.finish().unwrap();
            let _ = conn.closed().await;
        });
        let (conn, path) = tokio::time::timeout(Duration::from_secs(10), p2p::connect_to_host(&ctrl, &[], server, true))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(path, p2p::LinkPath::Relay(addr) if addr == server));
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let payload = vec![7u8; 200_000];
        send.write_all(&payload).await.unwrap();
        send.finish().unwrap();
        let echoed = tokio::time::timeout(Duration::from_secs(10), recv.read_to_end(1 << 20)).await.unwrap().unwrap();
        assert_eq!(echoed, payload);
        conn.close(0u32.into(), b"done");
        let _ = accept.await;
    }
}
