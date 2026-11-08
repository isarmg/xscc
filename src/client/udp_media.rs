//! UDP path validation runs only after the encrypted TCP application login.
use base::{
    direct_transport::udp_media::{
        AuthenticatedPacket, DatagramReceiver, DatagramSender, Direction, ProbeKind, SessionId,
        DEFAULT_MTU, VERSION,
    },
    message_proto::UdpMediaOffer,
};
use hbb_common::{
    anyhow::{bail, Context},
    config::CONNECT_TIMEOUT,
    sodiumoxide::randombytes,
    tokio::{
        net::UdpSocket,
        time::{interval, timeout, Duration, MissedTickBehavior},
    },
    ResultType,
};
use std::{net::SocketAddr, time::Instant};

pub struct PendingAssociation {
    pub socket: UdpSocket,
    pub sender: DatagramSender,
    pub receiver: DatagramReceiver,
    pub session_id: SessionId,
    pub mtu: usize,
}

pub async fn associate(
    offer: &UdpMediaOffer,
    seed: [u8; 32],
    tcp_peer: SocketAddr,
) -> ResultType<PendingAssociation> {
    if offer.version != u32::from(VERSION) {
        bail!("Unsupported UDP media version");
    }
    let sid = SessionId(
        offer
            .session_id
            .as_ref()
            .try_into()
            .context("UDP media session ID must contain 16 bytes")?,
    );
    if offer.mtu != DEFAULT_MTU as u32 {
        bail!("Unsupported UDP media MTU");
    }
    let socket = hbb_common::udp::bind_for_peer(tcp_peer).context("Bind UDP media socket")?;
    socket
        .connect(tcp_peer)
        .await
        .context("Connect UDP media socket")?;
    let mut sender = DatagramSender::new(&seed, sid, Direction::ControllerToHost)?;
    let mut receiver = DatagramReceiver::new(&seed, sid, Direction::HostToController)?;
    let mut token = [0; 16];
    randombytes::randombytes_into(&mut token);
    timeout(Duration::from_millis(CONNECT_TIMEOUT), async {
        let mut retry = interval(Duration::from_millis(250));
        retry.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut packet = [0; DEFAULT_MTU + 1];
        loop {
            hbb_common::tokio::select! {
                _ = retry.tick() => {
                    let probe = sender.seal_probe(ProbeKind::Probe, token)?;
                    socket.send(&probe.bytes).await.context("Send UDP media probe")?;
                }
                received = socket.recv(&mut packet) => {
                    let count = received.context("Receive UDP media challenge")?;
                    if count > DEFAULT_MTU { continue; }
                    let Ok(AuthenticatedPacket::Probe { kind: ProbeKind::Challenge, token }) =
                        receiver.open(&packet[..count], Instant::now()) else { continue; };
                    let response = sender.seal_probe(ProbeKind::Response, token)?;
                    socket.send(&response.bytes).await.context("Send UDP media response")?;
                    return Ok(PendingAssociation {
                        socket, sender, receiver, session_id: sid, mtu: DEFAULT_MTU,
                    });
                }
            }
        }
    })
    .await
    .context("UDP media association timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer() -> UdpMediaOffer {
        UdpMediaOffer {
            version: u32::from(VERSION),
            session_id: vec![7; 16].into(),
            mtu: DEFAULT_MTU as u32,
            ..Default::default()
        }
    }

    #[hbb_common::tokio::test]
    async fn invalid_offers_are_rejected_before_association() {
        let peer = "127.0.0.1:9".parse().unwrap();
        let mut bad = offer();
        bad.version += 1;
        assert!(associate(&bad, [1; 32], peer).await.is_err());
        bad = offer();
        bad.session_id = vec![7; 15].into();
        assert!(associate(&bad, [1; 32], peer).await.is_err());
        bad = offer();
        bad.mtu += 1;
        assert!(associate(&bad, [1; 32], peer).await.is_err());
    }

    #[hbb_common::tokio::test]
    async fn authenticated_loopback_association_retains_packet_sequences() {
        let host = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = host.local_addr().unwrap();
        let seed = [1; 32];
        let sid = SessionId([7; 16]);
        let serving = hbb_common::tokio::spawn(async move {
            let mut sender = DatagramSender::new(&seed, sid, Direction::HostToController).unwrap();
            let mut receiver =
                DatagramReceiver::new(&seed, sid, Direction::ControllerToHost).unwrap();
            let mut packet = [0; DEFAULT_MTU + 1];
            let (count, peer) = host.recv_from(&mut packet).await.unwrap();
            assert!(matches!(
                receiver.open(&packet[..count], Instant::now()).unwrap(),
                AuthenticatedPacket::Probe {
                    kind: ProbeKind::Probe,
                    ..
                }
            ));
            host.send_to(b"unauthenticated", peer).await.unwrap();
            let challenge = sender.seal_probe(ProbeKind::Challenge, [9; 16]).unwrap();
            host.send_to(&challenge.bytes, peer).await.unwrap();
            let (count, _) = host.recv_from(&mut packet).await.unwrap();
            assert!(matches!(
                receiver.open(&packet[..count], Instant::now()).unwrap(),
                AuthenticatedPacket::Probe {
                    kind: ProbeKind::Response,
                    token: [9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9]
                }
            ));
            let (count, _) = host.recv_from(&mut packet).await.unwrap();
            assert!(matches!(
                receiver.open(&packet[..count], Instant::now()).unwrap(),
                AuthenticatedPacket::Probe {
                    kind: ProbeKind::Probe,
                    token: [3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3]
                }
            ));
            let final_challenge = sender.seal_probe(ProbeKind::Challenge, [4; 16]).unwrap();
            host.send_to(&final_challenge.bytes, peer).await.unwrap();
        });
        let mut pending = associate(&offer(), seed, address).await.unwrap();
        assert_eq!(pending.session_id, sid);
        assert_eq!(pending.mtu, DEFAULT_MTU);
        assert_eq!(pending.socket.peer_addr().unwrap(), address);
        let probe = pending
            .sender
            .seal_probe(ProbeKind::Probe, [3; 16])
            .unwrap();
        pending.socket.send(&probe.bytes).await.unwrap();
        let mut packet = [0; DEFAULT_MTU + 1];
        let count = pending.socket.recv(&mut packet).await.unwrap();
        assert!(matches!(
            pending
                .receiver
                .open(&packet[..count], Instant::now())
                .unwrap(),
            AuthenticatedPacket::Probe {
                kind: ProbeKind::Challenge,
                token: [4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4]
            }
        ));
        serving.await.unwrap();
    }
}
