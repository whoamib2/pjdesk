//! A plain UDP socket for QUIC.
//!
//! quinn normally enables a few advanced socket options (ECN, packet info,
//! don't-fragment, segmentation offload). If the OS rejects them — older or
//! unusual network stacks, or Wine — this simple wrapper is used instead. It
//! speaks the same protocol, just without those optimizations.

use std::fmt;
use std::future::Future;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};

pub struct PlainUdpSocket {
    io: tokio::net::UdpSocket,
}

impl PlainUdpSocket {
    /// Must be called inside a Tokio runtime.
    pub fn new(socket: std::net::UdpSocket) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        Ok(Self { io: tokio::net::UdpSocket::from_std(socket)? })
    }
}

impl fmt::Debug for PlainUdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlainUdpSocket").field("local_addr", &self.io.local_addr().ok()).finish()
    }
}

impl AsyncUdpSocket for PlainUdpSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(WritablePoller { socket: self, pending: None })
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        match self.io.try_send_to(transmit.contents, transmit.destination) {
            Ok(_) => Ok(()),
            // Windows reports ICMP errors from earlier datagrams here; treat
            // it like packet loss, which QUIC recovers from.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
            ready!(self.io.poll_recv_ready(cx))?;
            match self.io.try_recv_from(&mut bufs[0][..]) {
                Ok((len, addr)) => {
                    meta[0] = RecvMeta { addr, len, stride: len, ecn: None, dst_ip: None };
                    return Poll::Ready(Ok(1));
                }
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::ConnectionReset) => continue,
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.local_addr()
    }
}

type WritableFuture = Pin<Box<dyn Future<Output = io::Result<()>> + Send + Sync>>;

/// Wakes one task when the socket becomes writable (each poller keeps its own waker).
struct WritablePoller {
    socket: Arc<PlainUdpSocket>,
    pending: Option<WritableFuture>,
}

impl fmt::Debug for WritablePoller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WritablePoller").finish_non_exhaustive()
    }
}

impl UdpPoller for WritablePoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let pending = this.pending.get_or_insert_with(|| {
            let socket = this.socket.clone();
            Box::pin(async move { socket.io.writable().await })
        });
        let result = pending.as_mut().poll(cx);
        if result.is_ready() {
            this.pending = None;
        }
        result
    }
}
