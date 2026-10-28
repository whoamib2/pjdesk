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
