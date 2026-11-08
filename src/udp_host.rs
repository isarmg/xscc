//! UDP media paths are leases of an already authenticated application TCP session.
use base::direct_transport::udp_media::{
    peek_probe_kind, peek_session_id, AuthenticatedPacket, DatagramReceiver, DatagramSender,
    Direction, FramePlan, ProbeKind, ReceiveReport, SealedDatagram, SessionId, DEFAULT_MTU,
};
use hbb_common::{
    anyhow::{bail, Context},
    rand, ResultType,
};
use std::{
    collections::BTreeMap,
    io,
    net::SocketAddr,
    sync::{Arc, LazyLock, Mutex, Weak},
    time::{Duration, Instant},
};
use hbb_common::tokio::{
    self,
    net::UdpSocket,
    sync::{mpsc, watch},
    task::JoinHandle,
};

const INCOMING_CAPACITY: usize = 64;
const CHALLENGE_INTERVAL: Duration = Duration::from_millis(100);
const CHALLENGE_TTL: Duration = Duration::from_secs(3);
const SEND_DEADLINE: Duration = Duration::from_millis(100);
static ACTIVE: LazyLock<Mutex<Weak<Hub>>> = LazyLock::new(|| Mutex::new(Weak::new()));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathStatus {
    Pending,
    Ready(SocketAddr),
    Closed,
}

struct Hub {
    socket: UdpSocket,
    sessions: Mutex<BTreeMap<SessionId, Weak<SessionState>>>,
}

pub(crate) struct HostListener {
    hub: Arc<Hub>,
    task: JoinHandle<()>,
}

impl HostListener {
    pub(crate) fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        self.hub.socket.local_addr()
    }
}

impl Drop for HostListener {
    fn drop(&mut self) {
        close_hub(&self.hub);
        self.task.abort();
    }
}

pub(crate) fn bind(port: u16) -> ResultType<HostListener> {
    let socket = hbb_common::udp::bind_any(port)?;
    let hub = Arc::new(Hub {
        socket,
        sessions: Mutex::new(BTreeMap::new()),
    });
    let mut active = ACTIVE.lock().unwrap();
    if active.upgrade().is_some() {
        bail!("A UDP media listener is already active");
    }
    *active = Arc::downgrade(&hub);
    drop(active);
    let task = tokio::spawn(receive(hub.clone()));
    Ok(HostListener { hub, task })
}

struct Challenge {
    address: SocketAddr,
    token: [u8; 16],
    created: Instant,
}
struct Control {
    sender: DatagramSender,
    receiver: DatagramReceiver,
    challenge: Option<Challenge>,
    last_challenge: Option<Instant>,
}
struct SessionState {
    control: Mutex<Control>,
    status: watch::Sender<PathStatus>,
    incoming: mpsc::Sender<AuthenticatedPacket>,
}
impl SessionState {
    fn close(&self) {
        let mut control = self.control.lock().unwrap();
        control.challenge = None;
        self.status.send_replace(PathStatus::Closed);
    }
}

pub(crate) struct HostSession {
    id: SessionId,
    state: Arc<SessionState>,
    hub: Weak<Hub>,
    incoming: mpsc::Receiver<AuthenticatedPacket>,
    status: watch::Receiver<PathStatus>,
}

/// Call only after the application login and management permit checks succeeded.
pub(crate) fn register(seed: [u8; 32]) -> ResultType<Option<HostSession>> {
    let active = ACTIVE.lock().unwrap();
    let Some(hub) = active.upgrade() else {
        return Ok(None);
    };
    let id = SessionId(rand::random());
    let sender = DatagramSender::new(&seed, id, Direction::HostToController)?;
    let receiver = DatagramReceiver::new(&seed, id, Direction::ControllerToHost)?;
    let (status_tx, status) = watch::channel(PathStatus::Pending);
    let (incoming_tx, incoming) = mpsc::channel(INCOMING_CAPACITY);
    let state = Arc::new(SessionState {
        control: Mutex::new(Control {
            sender,
            receiver,
            challenge: None,
            last_challenge: None,
        }),
        status: status_tx,
        incoming: incoming_tx,
    });
    let mut sessions = hub.sessions.lock().unwrap();
    if sessions.contains_key(&id) {
        bail!("UDP media session ID collision");
    }
    sessions.insert(id, Arc::downgrade(&state));
    drop(sessions);
    drop(active);
    Ok(Some(HostSession {
        id,
        state,
        hub: Arc::downgrade(&hub),
        incoming,
        status,
    }))
}

impl HostSession {
    pub(crate) fn session_id(&self) -> SessionId {
        self.id
    }
    pub(crate) fn mtu(&self) -> usize {
        DEFAULT_MTU
    }
    pub(crate) fn ready(&self) -> watch::Receiver<PathStatus> {
        self.status.clone()
    }
    pub(crate) fn writer(&self) -> HostWriter {
        HostWriter {
            state: Arc::downgrade(&self.state),
            hub: self.hub.clone(),
        }
    }
    pub(crate) async fn next(&mut self) -> Option<AuthenticatedPacket> {
        loop {
            if *self.status.borrow_and_update() == PathStatus::Closed {
                return None;
            }
            tokio::select! {
                packet = self.incoming.recv() => {
                    return if *self.status.borrow() == PathStatus::Closed { None } else { packet };
                },
                changed = self.status.changed() => if changed.is_err() { return None; },
            }
        }
    }
    pub(crate) fn take_report(&self, now: Instant) -> Option<ReceiveReport> {
        let mut control = self.state.control.lock().unwrap();
        if !matches!(*self.state.status.borrow(), PathStatus::Ready(_)) {
            return None;
        }
        control.receiver.take_report(now)
    }
}
impl Drop for HostSession {
    fn drop(&mut self) {
        self.state.close();
        if let Some(hub) = self.hub.upgrade() {
            hub.sessions.lock().unwrap().remove(&self.id);
        }
    }
}

#[derive(Clone)]
pub(crate) struct HostWriter {
    state: Weak<SessionState>,
    hub: Weak<Hub>,
}
impl HostWriter {
    pub(crate) async fn send_fragment(
        &self,
        frame: &FramePlan,
        index: u16,
    ) -> ResultType<SealedDatagram> {
        let state = self
            .state
            .upgrade()
            .context("UDP media TCP owner has closed")?;
        let hub = self
            .hub
            .upgrade()
            .context("UDP media listener has closed")?;
        let (peer, packet) = {
            let mut control = state.control.lock().unwrap();
            let peer = validated_peer(&state)?;
            (peer, control.sender.seal_fragment(frame, index)?)
        };
        send_datagram(&hub.socket, peer, &packet.bytes).await?;
        Ok(packet)
    }
    pub(crate) async fn send_probe(&self, token: [u8; 16]) -> ResultType<()> {
        let state = self
            .state
            .upgrade()
            .context("UDP media TCP owner has closed")?;
        let hub = self
            .hub
            .upgrade()
            .context("UDP media listener has closed")?;
        let (peer, packet) = {
            let mut control = state.control.lock().unwrap();
            let peer = validated_peer(&state)?;
            let packet = control.sender.seal_probe(ProbeKind::Challenge, token)?;
            control.challenge = Some(Challenge {
                address: peer,
                token,
                created: Instant::now(),
            });
            (peer, packet)
        };
        send_datagram(&hub.socket, peer, &packet.bytes).await
    }
    pub(crate) async fn send_report(&self, report: &ReceiveReport) -> ResultType<()> {
        let state = self
            .state
            .upgrade()
            .context("UDP media TCP owner has closed")?;
        let hub = self
            .hub
            .upgrade()
            .context("UDP media listener has closed")?;
        let (peer, packet) = {
            let mut control = state.control.lock().unwrap();
            let peer = validated_peer(&state)?;
            (peer, control.sender.seal_report(report)?)
        };
        send_datagram(&hub.socket, peer, &packet.bytes).await
    }
}
fn validated_peer(state: &SessionState) -> ResultType<SocketAddr> {
    match *state.status.borrow() {
        PathStatus::Ready(peer) => Ok(peer),
        PathStatus::Pending => bail!("UDP media return path has not been validated"),
        PathStatus::Closed => bail!("UDP media TCP owner or listener has closed"),
    }
}
async fn send_datagram(socket: &UdpSocket, peer: SocketAddr, bytes: &[u8]) -> ResultType<()> {
    let length = tokio::time::timeout(SEND_DEADLINE, socket.send_to(bytes, peer))
        .await
        .context("UDP media send timed out")??;
    if length != bytes.len() {
        bail!("UDP media datagram was not sent in full");
    }
    Ok(())
}

fn close_hub(hub: &Arc<Hub>) {
    let mut active = ACTIVE.lock().unwrap();
    if active.ptr_eq(&Arc::downgrade(hub)) {
        *active = Weak::new();
    }
    drop(active);
    let sessions = std::mem::take(&mut *hub.sessions.lock().unwrap());
    for state in sessions.into_values().filter_map(|state| state.upgrade()) {
        state.close();
    }
}

async fn receive(hub: Arc<Hub>) {
    let mut bytes = vec![0u8; 65_536];
    loop {
        let (length, address) = match hub.socket.recv_from(&mut bytes).await {
            Ok(value) => value,
            // A failed destination must not terminate a socket shared by other clients.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused
                ) =>
            {
                continue
            }
            Err(error) => {
                hbb_common::log::warn!("UDP media listener receive failed: {error}");
                break;
            }
        };
        let packet = &bytes[..length];
        let Ok(id) = peek_session_id(packet) else {
            continue;
        };
        let state = hub
            .sessions
            .lock()
            .unwrap()
            .get(&id)
            .and_then(Weak::upgrade);
        let Some(state) = state else {
            continue;
        };
        if let Some(challenge) = process(&state, packet, address, Instant::now()) {
            // Challenge loss is retried by the client; never block all session reads on it.
            let _ = hub.socket.try_send_to(&challenge.bytes, address);
        }
    }
    close_hub(&hub);
}

fn process(
    state: &SessionState,
    bytes: &[u8],
    address: SocketAddr,
    now: Instant,
) -> Option<SealedDatagram> {
    let mut control = state.control.lock().unwrap();
    let status = *state.status.borrow();
    if status == PathStatus::Closed {
        return None;
    }
    let pinned = matches!(status, PathStatus::Ready(peer) if peer == address);
    if !pinned && !matches!(peek_probe_kind(bytes), Ok(Some(_))) {
        return None;
    }
    let Ok(packet) = control.receiver.open(bytes, now) else {
        return None;
    };
    match packet {
        AuthenticatedPacket::Probe {
            kind: ProbeKind::Probe,
            ..
        } => {
            if control
                .last_challenge
                .is_some_and(|last| now.duration_since(last) < CHALLENGE_INTERVAL)
            {
                return None;
            }
            let token = match &control.challenge {
                Some(challenge)
                    if challenge.address == address
                        && now.duration_since(challenge.created) < CHALLENGE_TTL =>
                {
                    challenge.token
                }
                _ => {
                    let token = rand::random();
                    control.challenge = Some(Challenge {
                        address,
                        token,
                        created: now,
                    });
                    token
                }
            };
            control.last_challenge = Some(now);
            match control.sender.seal_probe(ProbeKind::Challenge, token) {
                Ok(reply) => (reply.bytes.len() <= bytes.len()).then_some(reply),
                Err(error) => {
                    state.status.send_replace(PathStatus::Closed);
                    hbb_common::throttled_log!(
                        Duration::from_secs(60),
                        warn,
                        "UDP media challenge encryption failed: {error}"
                    );
                    None
                }
            }
        }
        AuthenticatedPacket::Probe {
            kind: ProbeKind::Response,
            token,
        } => {
            if control.challenge.as_ref().is_some_and(|challenge| {
                challenge.address == address
                    && challenge.token == token
                    && now.duration_since(challenge.created) < CHALLENGE_TTL
            }) {
                control.challenge = None;
                state.status.send_replace(PathStatus::Ready(address));
                let _ = state.incoming.try_send(AuthenticatedPacket::Probe {
                    kind: ProbeKind::Response,
                    token,
                });
            }
            None
        }
        AuthenticatedPacket::Probe { .. } => None,
        packet if pinned => {
            let _ = state.incoming.try_send(packet);
            None
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) static TEST_LISTENER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests {
    use super::*;
    use base::direct_transport::udp_media::{FrameMetadata, MediaKind};
    use hbb_common::bytes::Bytes;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn udp_host_validates_return_path_and_releases_only_the_tcp_owner() {
        let _guard = TEST_LISTENER_LOCK.lock().await;
        assert!(register([1; 32]).unwrap().is_none());
        let listener = bind(0).unwrap();
        let endpoint =
            SocketAddr::from((Ipv4Addr::LOCALHOST, listener.local_addr().unwrap().port()));
        let client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let wrong_path = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let mut owner = register([1; 32]).unwrap().unwrap();
        let other_owner = register([2; 32]).unwrap().unwrap();
        let mut status = owner.ready();
        let writer = owner.writer();
        let mut sender =
            DatagramSender::new(&[1; 32], owner.session_id(), Direction::ControllerToHost).unwrap();
        let mut receiver =
            DatagramReceiver::new(&[1; 32], owner.session_id(), Direction::HostToController)
                .unwrap();
        let plan = FramePlan::new(
            FrameMetadata {
                kind: MediaKind::Video,
                display_id: 0,
                epoch: 1,
                frame_id: 1,
                keyframe: true,
            },
            Bytes::from_static(b"the remote frame"),
            owner.mtu(),
        )
        .unwrap();
        assert!(writer.send_fragment(&plan, 0).await.is_err());
        let probe = sender.seal_probe(ProbeKind::Probe, [3; 16]).unwrap();
        client.send_to(&probe.bytes, endpoint).await.unwrap();
        let mut bytes = [0; 2048];
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut bytes))
                .await
                .unwrap()
                .unwrap();
        assert!(length <= probe.bytes.len());
        let AuthenticatedPacket::Probe {
            kind: ProbeKind::Challenge,
            token,
        } = receiver.open(&bytes[..length], Instant::now()).unwrap()
        else {
            panic!("host did not issue an authenticated challenge");
        };
        assert_eq!(*status.borrow_and_update(), PathStatus::Pending);
        let mut incorrect = token;
        incorrect[0] ^= 1;
        let response = sender.seal_probe(ProbeKind::Response, incorrect).unwrap();
        client.send_to(&response.bytes, endpoint).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), status.changed())
                .await
                .is_err()
        );
        let response = sender.seal_probe(ProbeKind::Response, token).unwrap();
        wrong_path.send_to(&response.bytes, endpoint).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), status.changed())
                .await
                .is_err()
        );
        let response = sender.seal_probe(ProbeKind::Response, token).unwrap();
        client.send_to(&response.bytes, endpoint).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), status.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(*status.borrow(), PathStatus::Ready(peer) if peer.port() == client.local_addr().unwrap().port())
        );
        assert!(matches!(
            owner.next().await,
            Some(AuthenticatedPacket::Probe {
                kind: ProbeKind::Response,
                ..
            })
        ));
        writer.send_fragment(&plan, 0).await.unwrap();
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut bytes))
                .await
                .unwrap()
                .unwrap();
        let AuthenticatedPacket::Fragment(fragment) =
            receiver.open(&bytes[..length], Instant::now()).unwrap()
        else {
            panic!("host did not send the media fragment");
        };
        assert_eq!(&fragment.payload[..], b"the remote frame");

        writer.send_probe([5; 16]).await.unwrap();
        let (length, _) =
            tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut bytes))
                .await
                .unwrap()
                .unwrap();
        let AuthenticatedPacket::Probe {
            kind: ProbeKind::Challenge,
            token,
        } = receiver.open(&bytes[..length], Instant::now()).unwrap()
        else {
            panic!("host did not send its recovery probe");
        };
        assert_eq!(token, [5; 16]);
        let response = sender.seal_probe(ProbeKind::Response, token).unwrap();
        client.send_to(&response.bytes, endpoint).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), owner.next())
                .await
                .unwrap(),
            Some(AuthenticatedPacket::Probe {
                kind: ProbeKind::Response,
                token: [5, ..]
            })
        ));
        drop(owner);
        assert_eq!(*status.borrow(), PathStatus::Closed);
        assert!(writer.send_fragment(&plan, 0).await.is_err());
        assert_eq!(*other_owner.ready().borrow(), PathStatus::Pending);
        drop(listener);
        assert_eq!(*other_owner.ready().borrow(), PathStatus::Closed);
        assert!(register([1; 32]).unwrap().is_none());
    }
}
