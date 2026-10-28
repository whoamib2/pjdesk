# TCP relay fallback (deferred from 0.6.65)

## Problem
Relay sessions need both peers to bind over UDP to the server's port 21200. If UDP 21200 is blocked or not forwarded (for example CHIP23's remote hosts, or a cabin behind a UDP-hostile NAT), the session fails. 0.6.65 now reports this clearly, but it can't get the session through.

## Proposal
1. After SessionReady, each peer tries the UDP Bind as it does today. If it gets no UdpReady within 4 s, or `bind_udp` fails, it sends `ClientMsg::RelayOverTcp{session}` on its existing signalling TCP connection.
2. The server marks that side as `Transport::Tcp`. For each session, it relays frames between the two sides whichever transport each uses (UDP↔UDP, UDP↔TCP, TCP↔TCP). TCP frames are length-prefixed `SignalMsg::RelayData{bytes}`, reusing the existing TLS stream, so no new port is needed.
3. PeerInfo carries `transport` for each side, and the link layer picks a `TcpRelayLink` that implements the same `Link` trait as `p2p::UdpLink`.
4. Media over TCP: turn off FEC and NACK retransmit, cap the bitrate (start at 2 Mbps), and drop late video frames on the sender side to avoid head-of-line latency build-up.
5. Compatibility: this is gated on `perms::TCP_RELAY` in the handshake. Old servers never answer, so the existing 17 s timeout and error text still apply.

## Why it was deferred
It changes the link layer, the server relay loop and the protocol on all three platforms. It needs end-to-end testing across real NATs and latency tuning. That is too much for one unreviewed release, so it is planned for 0.6.66.

## Implemented in 0.6.67
- Trigger: `p2p::bind_udp` fails (8 s of Bind attempts with no BindAck). The peer then sends `ClientMsg::RelayOverTcp{session_id}` and a `UdpReady` (no UDP addresses; the host still sends its fingerprint and LAN 21201 addresses). It only does this when the server reported version 0.6.67+ (`signal::server_relays_over_tcp`; unknown is tried).
- Server: the side is marked `tcp`, its public address becomes the placeholder `0.0.0.0:0` (peers cannot dial it, so they use the relay), `TcpRelayReady` is sent back and PeerInfo goes out as usual. Packets: TCP→UDP via the relay socket, UDP→TCP via `tcp_routes`, TCP→TCP directly. Per-side cap of 3 MiB/s and the existing 1 GiB per session. `RelayData` is exempt from the 40 messages/s burst limit.
- Client: `pjdesk_link::tcp_relay::TcpTunnelSocket` is a quinn `AsyncUdpSocket` that sends each datagram as `ClientMsg::RelayData` and receives `ServerMsg::RelayData` as if it came from the server's UDP port; QUIC on top is unchanged. Sender budget about 16 Mbit/s with a 256 KiB burst; excess is dropped as loss.
- Compatibility: messages are appended (index test). New server messages go only to a client that asked. An old server would close the connection on `RelayOverTcp`, which is why clients check the server version first.
- Not done: FEC/bitrate changes for TCP (QUIC congestion control handles it), and the transport flag in PeerInfo (the placeholder address carries the same meaning).
