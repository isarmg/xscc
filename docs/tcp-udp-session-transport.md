# TCP management and UDP media

The native product enables `udp-media` through `gpui-session` on Windows,
Linux, macOS, Android and iOS. The historical `experimental-quic` feature is
mutually exclusive with this product path and is not enabled by product CI.

## Network layout

- xscs management, authentication and authorization: existing HTTP/WebSocket
  over TCP. HTTPS termination remains a deployment concern for Caddy or another
  reverse proxy; no new TLS requirement is introduced by this change.
- Direct session login, keyboard/buttons, cursor metadata, clipboard and other
  reliable content: the existing encrypted TCP connection.
- Encoded video and audio, including video keyframes: authenticated encrypted
  UDP datagrams. UDP congestion reports do not wait behind TCP clipboard writes.
- Small media readiness, format-epoch and recovery messages: encrypted TCP.

The Host binds TCP and UDP to the same configured direct port (default 21118).
The controller uses the IP and port of its actual connected TCP socket, not a
second DNS lookup or the Host's local port. NAT deployments must forward both
protocols on that external numeric port. An HTTP reverse proxy or management
tunnel alone does not forward session media. No new xscs media relay is added.
The Windows Host installer owns program-scoped TCP/UDP inbound rules; portable
or other-platform deployments must permit the same direct port themselves.

## Authentication and path validation

The application login advertises UDP capability. Only after normal login and
permission checks succeed does the Host register a random 128-bit session ID
and include a versioned public offer in PeerInfo. Session keys are derived
locally from the existing encrypted TCP session with separate namespaces for
UDP and each traffic direction. Keys are never included in the offer or logs.

An encrypted probe/challenge/response validates the return path. The Host then
confirms readiness over the authenticated TCP connection. The UI reports
`TCP+UDP` only after that confirmation. Each direction keeps one continuous
AEAD sequence across handshake, reports and media, with authenticated replay
protection. Unvalidated traffic cannot enable media or change the destination.
Closing the TCP session revokes its UDP registration. Dropping/restarting the
TCP listener also drops its UDP listener and all registrations.

Peers without capability/offer retain the existing TCP compatibility path.
Once a UDP offer is accepted, failed association is an explicit connection
error; negotiated media is not silently relabeled or sent through TCP.

## Bounded realtime delivery

Datagrams default to 1200 bytes. Video/audio have separate size, frame-count,
memory and expiry limits; whole stale delta frames are discarded. Format epochs
are acknowledged over TCP before sending a new stream generation. Video loss
requests a new reference frame; missing keyframe fragments can be selectively
retried with fresh packet sequences and bounded retry/cache budgets. Reliable
frame notices also detect a wholly lost final frame when the desktop goes idle.

The UDP sender has congestion feedback, bounded in-flight metadata, pacing,
loss backoff and a feedback timeout. An authenticated new probe can recover a
stalled path. The existing 1 Mbps–1 Gbps setting remains an **upper limit**;
network capacity and congestion control may choose a lower actual rate.
These loopback/source tests do not establish a particular remote FPS, GPU
capability or virtual-display teardown safety on a physical computer.

Design guidance: [RFC 8085, UDP Usage Guidelines](https://www.rfc-editor.org/rfc/rfc8085).
