//! Application transport boundary. Existing TCP stays unchanged when QUIC is off.
//! This module does not grant application login, input or clipboard permission.
#[cfg(not(feature = "experimental-quic"))]
pub type SessionTransport = hbb_common::Stream;
#[cfg(feature = "experimental-quic")]
pub use experimental::SessionTransport;

#[cfg(feature = "experimental-quic")]
mod experimental {
    use base::{
        direct_transport::{
            quic::{Lane, Session, MAX_RELIABLE_MESSAGE},
            quic_media::{fragment_frame, FrameMetadata, Reassembler, MAX_FRAME_SIZE},
        },
        message_proto::{message, video_frame, Message, Misc, VideoFrame},
    };
    use hbb_common::{
        anyhow::{bail, Context},
        bytes::{Bytes, BytesMut},
        protobuf::Message as _,
        ResultType, Stream,
    };
    use std::{
        collections::BTreeMap,
        convert::TryInto,
        future::Future,
        io,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    use tokio::{
        sync::{mpsc, oneshot, watch},
        task::JoinHandle,
        time::{interval, timeout, Interval, MissedTickBehavior},
    };

    const LANES: [Lane; 3] = [Lane::Control, Lane::Input, Lane::Clipboard];
    const RELIABLE_QUEUE: usize = 32;
    const VIDEO_QUEUE: usize = 2;
    const MAX_DISPLAYS: usize = 64;
    const ASSEMBLY_TTL: Duration = Duration::from_millis(100);
    const REFRESH_INTERVAL: Duration = Duration::from_millis(250);

    pub enum SessionTransport {
        Legacy(Box<Stream>),
        Quic(Box<QuicTransport>),
    }
    impl From<Stream> for SessionTransport {
        fn from(stream: Stream) -> Self {
            Self::Legacy(Box::new(stream))
        }
    }
    impl SessionTransport {
        /// Call only after management authentication. The server must still run
        /// its normal Connection login and application permission checks.
        pub async fn from_quic(session: Session) -> ResultType<Self> {
            let session = Arc::new(session);
            let result =
                timeout(Duration::from_secs(10), QuicTransport::new(session.clone())).await;
            match result {
                Ok(Ok(transport)) => Ok(Self::Quic(Box::new(transport))),
                Ok(Err(error)) => {
                    session.close();
                    Err(error)
                }
                Err(_) => {
                    session.close();
                    bail!("QUIC application lanes timed out")
                }
            }
        }
        /// Call after successful application LoginResponse/PeerInfo processing,
        /// never merely after TLS or management authentication. Early DATAGRAMs
        /// are discarded and this requests a new decoder reference frame.
        pub fn enable_media(&mut self) -> ResultType<()> {
            let Self::Quic(stream) = self else {
                return Ok(());
            };
            if !stream.media_enabled {
                let mut misc = Misc::new();
                misc.set_refresh_video(true);
                let mut message = Message::new();
                message.set_misc(misc);
                stream.enqueue(0, message.write_to_bytes()?.into())?;
                stream.media_enabled = true;
            }
            Ok(())
        }
        pub fn is_secured(&self) -> bool {
            match self {
                Self::Legacy(stream) => stream.is_secured(),
                Self::Quic(_) => true,
            }
        }
        pub fn set_send_timeout(&mut self, ms: u64) {
            match self {
                Self::Legacy(stream) => stream.set_send_timeout(ms),
                Self::Quic(stream) => stream.send_timeout.store(ms, Ordering::Release),
            }
        }
        pub fn set_max_packet_length(&mut self, limit: usize) {
            match self {
                Self::Legacy(stream) => stream.set_max_packet_length(limit),
                Self::Quic(stream) => stream.receive_limit = limit.min(MAX_FRAME_SIZE),
            }
        }
        /// Reliable sends enqueue into bounded independent lane writers; a full
        /// reliable queue fails the session rather than dropping input/control.
        /// Video is best effort: a full two-frame queue drops the new whole frame.
        pub async fn send(&mut self, message: &Message) -> ResultType<()> {
            match self {
                Self::Legacy(stream) => stream.send(message).await,
                Self::Quic(stream) => stream.send_or_enqueue(message).await,
            }
        }
        pub async fn send_bytes(&mut self, bytes: Bytes) -> ResultType<()> {
            match self {
                Self::Legacy(stream) => stream.send_bytes(bytes).await,
                Self::Quic(stream) => {
                    stream
                        .send_or_enqueue(&Message::parse_from_bytes(&bytes)?)
                        .await
                }
            }
        }
        pub async fn send_raw(&mut self, bytes: Vec<u8>) -> ResultType<()> {
            match self {
                Self::Legacy(stream) => stream.send_raw(bytes).await,
                Self::Quic(stream) => {
                    stream
                        .send_or_enqueue(&Message::parse_from_bytes(&bytes)?)
                        .await
                }
            }
        }
        pub async fn next(&mut self) -> Option<io::Result<BytesMut>> {
            match self {
                Self::Legacy(stream) => stream.next().await,
                Self::Quic(stream) => match stream.next().await {
                    Ok(bytes) => Some(Ok(bytes)),
                    Err(error) => {
                        stream.session.close();
                        Some(Err(io::Error::other(error.to_string())))
                    }
                },
            }
        }
        pub async fn next_timeout(&mut self, ms: u64) -> Option<io::Result<BytesMut>> {
            timeout(Duration::from_millis(ms), self.next())
                .await
                .ok()
                .flatten()
        }
    }

    struct VideoPacket {
        metadata: FrameMetadata,
        bytes: Bytes,
    }
    struct ReliablePacket {
        bytes: Bytes,
        completion: Option<oneshot::Sender<Result<(), String>>>,
        terminal: bool,
    }
    struct ReceiveLane {
        receiver: mpsc::Receiver<Bytes>,
        sender: mpsc::Sender<ReliablePacket>,
    }
    struct DisplayState {
        last: Option<u64>,
        needs_keyframe: bool,
        refresh_at: Option<Instant>,
    }
    impl Default for DisplayState {
        fn default() -> Self {
            Self {
                last: None,
                needs_keyframe: true,
                refresh_at: None,
            }
        }
    }
    pub struct QuicTransport {
        session: Arc<Session>,
        lanes: Vec<ReceiveLane>,
        video: mpsc::Sender<VideoPacket>,
        errors: mpsc::Receiver<String>,
        tasks: Vec<JoinHandle<()>>,
        shutdown: watch::Sender<u8>,
        final_acknowledged: bool,
        send_timeout: Arc<AtomicU64>,
        receive_limit: usize,
        media_enabled: bool,
        frame_ids: BTreeMap<u32, u64>,
        displays: BTreeMap<u32, DisplayState>,
        reassembler: Reassembler,
        expiry: Interval,
    }
    impl Drop for QuicTransport {
        fn drop(&mut self) {
            if self.final_acknowledged {
                self.shutdown.send_replace(2);
                let session = self.session.clone();
                let tasks = std::mem::take(&mut self.tasks);
                let deadline = Duration::from_millis(self.send_timeout.load(Ordering::Acquire));
                // Keep stream handles alive without polling business IO: aborting
                // them here would RESET the peer before it reads the final reason.
                tokio::spawn(async move {
                    let _ = timeout(deadline, session.closed()).await;
                    session.close();
                    for task in tasks {
                        task.abort();
                    }
                });
            } else {
                self.session.close();
                for task in &self.tasks {
                    task.abort();
                }
            }
        }
    }
    impl QuicTransport {
        async fn new(session: Arc<Session>) -> ResultType<Self> {
            // Each side owns outgoing streams. Separate incoming streams avoid
            // sharing a mutable bidi stream between a blocked writer and reader.
            let mut outgoing = Vec::new();
            for kind in LANES {
                outgoing.push(session.open_lane(kind).await?);
            }
            let mut incoming = BTreeMap::new();
            for _ in LANES {
                let lane = session.accept_lane().await?;
                if !LANES.contains(&lane.kind())
                    || incoming.insert(lane.kind() as u8, lane).is_some()
                {
                    bail!("Unexpected or duplicate QUIC application lane");
                }
            }
            let (errors_tx, errors) = mpsc::channel(1);
            let send_timeout = Arc::new(AtomicU64::new(10_000));
            let mut lanes = Vec::new();
            let mut tasks = Vec::new();
            let (shutdown, _) = watch::channel(0);
            for (kind, mut writer) in LANES.into_iter().zip(outgoing) {
                let mut reader = incoming
                    .remove(&(kind as u8))
                    .context("Missing QUIC lane")?;
                let (sender, mut queue) = mpsc::channel::<ReliablePacket>(RELIABLE_QUEUE);
                let (received, receiver) = mpsc::channel(RELIABLE_QUEUE);
                let send_session = session.clone();
                let send_errors = errors_tx.clone();
                let deadline = send_timeout.clone();
                tasks.push(spawn_worker(
                    session.clone(),
                    shutdown.subscribe(),
                    if kind == Lane::Control { 2 } else { 1 },
                    async move {
                        while let Some(packet) = queue.recv().await {
                            let result = timeout(
                                Duration::from_millis(deadline.load(Ordering::Acquire)),
                                async {
                                    writer.send(&packet.bytes).await?;
                                    if packet.terminal {
                                        writer.finish().await?;
                                    }
                                    Ok::<_, hbb_common::anyhow::Error>(())
                                },
                            )
                            .await;
                            let error = match result {
                                Ok(Ok(())) => {
                                    if let Some(completion) = packet.completion {
                                        let _ = completion.send(Ok(()));
                                    }
                                    continue;
                                }
                                Ok(Err(error)) => error.to_string(),
                                Err(_) => "QUIC reliable send timed out".to_owned(),
                            };
                            if let Some(completion) = packet.completion {
                                let _ = completion.send(Err(error.clone()));
                            }
                            fail(&send_session, &send_errors, error);
                            break;
                        }
                    },
                ));
                let receive_session = session.clone();
                let receive_errors = errors_tx.clone();
                tasks.push(spawn_worker(
                    session.clone(),
                    shutdown.subscribe(),
                    1,
                    async move {
                        loop {
                            match reader.recv().await {
                                Ok(bytes) => {
                                    let terminal = Message::parse_from_bytes(&bytes)
                                        .is_ok_and(|message| terminal_control(&message));
                                    if received.send(bytes).await.is_err() {
                                        break;
                                    }
                                    if terminal {
                                        // This direction ends normally after its final reason;
                                        // consuming FIN must not close the remaining connection.
                                        let _ = reader.recv().await;
                                        break;
                                    }
                                }
                                Err(error) => {
                                    fail(&receive_session, &receive_errors, error.to_string());
                                    break;
                                }
                            }
                        }
                    },
                ));
                lanes.push(ReceiveLane { receiver, sender });
            }
            let (video, mut packets) = mpsc::channel::<VideoPacket>(VIDEO_QUEUE);
            let video_session = session.clone();
            tasks.push(spawn_worker(
                session.clone(),
                shutdown.subscribe(),
                1,
                async move {
                    while let Some(packet) = packets.recv().await {
                        let result = async {
                            let mtu = video_session
                                .max_datagram_size()
                                .context("QUIC DATAGRAM unavailable")?;
                            for fragment in fragment_frame(packet.metadata, &packet.bytes, mtu)? {
                                video_session.send_media_wait(fragment.into()).await?;
                            }
                            Ok::<_, hbb_common::anyhow::Error>(())
                        }
                        .await;
                        if let Err(error) = result {
                            fail(&video_session, &errors_tx, error.to_string());
                            break;
                        }
                    }
                },
            ));
            let mut expiry = interval(Duration::from_millis(25));
            expiry.set_missed_tick_behavior(MissedTickBehavior::Skip);
            Ok(Self {
                session,
                lanes,
                video,
                errors,
                tasks,
                shutdown,
                final_acknowledged: false,
                send_timeout,
                receive_limit: MAX_RELIABLE_MESSAGE,
                media_enabled: false,
                frame_ids: BTreeMap::new(),
                displays: BTreeMap::new(),
                reassembler: Reassembler::with_ttl(ASSEMBLY_TTL)?,
                expiry,
            })
        }
        async fn send_or_enqueue(&mut self, message: &Message) -> ResultType<()> {
            let final_control = match &message.union {
                Some(message::Union::Misc(misc)) => matches!(
                    misc.union,
                    Some(base::message_proto::misc::Union::CloseReason(_))
                ),
                Some(message::Union::LoginResponse(response)) => matches!(
                    response.union,
                    Some(base::message_proto::login_response::Union::Error(_))
                ),
                _ => false,
            };
            if !final_control {
                return self.send(message);
            }
            self.session.ensure_authorized()?;
            if *self.shutdown.borrow() != 0 {
                bail!("QUIC session is shutting down");
            }
            let bytes = message.write_to_bytes()?;
            if bytes.len() > MAX_RELIABLE_MESSAGE {
                bail!("QUIC final control message exceeds limit");
            }
            let terminal = terminal_control(message);
            if terminal {
                self.shutdown.send_replace(1);
            }
            let (completion, written) = oneshot::channel();
            if self.lanes[0]
                .sender
                .try_send(ReliablePacket {
                    bytes: bytes.into(),
                    completion: Some(completion),
                    terminal,
                })
                .is_err()
            {
                self.session.close();
                bail!("QUIC final control queue unavailable");
            }
            let result = timeout(
                Duration::from_millis(self.send_timeout.load(Ordering::Acquire)),
                written,
            )
            .await
            .context("QUIC final control send timed out")?
            .context("QUIC final control writer closed")?
            .map_err(hbb_common::anyhow::Error::msg);
            if terminal {
                self.final_acknowledged = result.is_ok();
                if result.is_err() {
                    self.session.close();
                }
            }
            result
        }

        fn send(&mut self, message: &Message) -> ResultType<()> {
            self.session.ensure_authorized()?;
            if *self.shutdown.borrow() != 0 {
                bail!("QUIC session is shutting down");
            }
            if let Some(message::Union::VideoFrame(video)) = &message.union {
                let (display, keyframe) = video_metadata(video)?;
                if !self.frame_ids.contains_key(&display) && self.frame_ids.len() >= MAX_DISPLAYS {
                    bail!("Too many QUIC video displays");
                }
                let frame = self.frame_ids.entry(display).or_insert(0);
                let metadata = FrameMetadata {
                    frame_id: *frame,
                    display_id: display,
                    keyframe,
                };
                *frame = frame
                    .checked_add(1)
                    .context("QUIC frame sequence exhausted")?;
                let bytes = message.write_to_bytes()?;
                if bytes.len() > MAX_FRAME_SIZE {
                    bail!("QUIC video frame exceeds limit");
                }
                match self.video.try_send(VideoPacket {
                    metadata,
                    bytes: bytes.into(),
                }) {
                    Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
                    Err(mpsc::error::TrySendError::Closed(_)) => bail!("QUIC video writer closed"),
                }
            } else {
                let lane = match &message.union {
                    Some(
                        message::Union::MouseEvent(_)
                        | message::Union::KeyEvent(_)
                        | message::Union::PointerDeviceEvent(_),
                    ) => 1,
                    Some(message::Union::Clipboard(_) | message::Union::MultiClipboards(_)) => 2,
                    _ => 0,
                };
                let bytes = message.write_to_bytes()?;
                if bytes.len() > MAX_RELIABLE_MESSAGE {
                    bail!("QUIC reliable message exceeds limit");
                }
                self.enqueue(lane, bytes.into())
            }
        }
        fn enqueue(&self, lane: usize, bytes: Bytes) -> ResultType<()> {
            self.session.ensure_authorized()?;
            if *self.shutdown.borrow() != 0 {
                bail!("QUIC session is shutting down");
            }
            if let Err(error) = self.lanes[lane].sender.try_send(ReliablePacket {
                bytes,
                completion: None,
                terminal: false,
            }) {
                self.session.close();
                bail!("QUIC reliable queue unavailable: {error}");
            }
            Ok(())
        }
        fn recover(&mut self, display: u32, frame: u64, now: Instant) -> ResultType<()> {
            let state = self.display(display)?;
            if state.last.is_some_and(|last| frame <= last) {
                return Ok(());
            }
            state.needs_keyframe = true;
            self.request_refresh(display, now)
        }
        fn display(&mut self, display: u32) -> ResultType<&mut DisplayState> {
            if !self.displays.contains_key(&display) && self.displays.len() >= MAX_DISPLAYS {
                bail!("Too many QUIC video displays");
            }
            Ok(self.displays.entry(display).or_default())
        }
        fn request_refresh(&mut self, display: u32, now: Instant) -> ResultType<()> {
            let state = self.display(display)?;
            if state
                .refresh_at
                .is_some_and(|at| now.saturating_duration_since(at) < REFRESH_INTERVAL)
            {
                return Ok(());
            }
            state.refresh_at = Some(now);
            let mut misc = Misc::new();
            misc.set_refresh_video_display(display.try_into().context("Invalid QUIC display")?);
            let mut message = Message::new();
            message.set_misc(misc);
            self.enqueue(0, message.write_to_bytes()?.into())
        }
        fn datagram(&mut self, bytes: &[u8], now: Instant) -> ResultType<Option<BytesMut>> {
            // Enforce the current pre-login application cap before allocating
            // the fragmented frame, not just after successful reassembly.
            let total = bytes.get(18..22).context("Invalid QUIC video envelope")?;
            if u32::from_be_bytes(total.try_into()?) as usize > self.receive_limit {
                bail!("QUIC video exceeds application receive limit");
            }
            let outcome = self.reassembler.push(bytes, now)?;
            for recovery in outcome.recovery {
                self.recover(
                    recovery.metadata.display_id,
                    recovery.metadata.frame_id,
                    now,
                )?;
            }
            let Some(frame) = outcome.frame else {
                return Ok(None);
            };
            let message = Message::parse_from_bytes(&frame.payload)?;
            let Some(message::Union::VideoFrame(video)) = &message.union else {
                bail!("Non-video QUIC media payload");
            };
            let (display, keyframe) = video_metadata(video)?;
            if display != frame.metadata.display_id || keyframe != frame.metadata.keyframe {
                bail!("QUIC video metadata mismatch");
            }
            let state = self.display(display)?;
            if state
                .last
                .is_some_and(|last| frame.metadata.frame_id <= last)
            {
                return Ok(None);
            }
            if state
                .last
                .is_some_and(|last| last.checked_add(1) != Some(frame.metadata.frame_id))
            {
                state.needs_keyframe = true;
            }
            state.last = Some(frame.metadata.frame_id);
            if keyframe {
                state.needs_keyframe = false;
                state.refresh_at = None;
            }
            if state.needs_keyframe {
                self.request_refresh(display, now)?;
                return Ok(None);
            }
            self.session.ensure_authorized()?;
            Ok(Some(frame.payload.as_slice().into()))
        }
        async fn next(&mut self) -> ResultType<BytesMut> {
            loop {
                self.session.ensure_authorized()?;
                let [control, input, clipboard] = self.lanes.as_mut_slice() else {
                    bail!("Missing QUIC application lanes");
                };
                tokio::select! {
                    error = self.errors.recv() => bail!("{}", error.unwrap_or_else(|| "QUIC writers closed".to_owned())),
                    bytes = control.receiver.recv() => return self.reliable(bytes),
                    bytes = input.receiver.recv() => return self.reliable(bytes),
                    bytes = clipboard.receiver.recv() => return self.reliable(bytes),
                    _ = self.expiry.tick() => {
                        let now = Instant::now();
                        for recovery in self.reassembler.expire(now) {
                            self.recover(recovery.metadata.display_id, recovery.metadata.frame_id, now)?;
                        }
                    }
                    bytes = self.session.recv_media() => {
                        let bytes = bytes?;
                        if !self.media_enabled { continue; }
                        if let Some(frame) = self.datagram(&bytes, Instant::now())? { return Ok(frame); }
                    }
                }
            }
        }
        fn reliable(&self, bytes: Option<Bytes>) -> ResultType<BytesMut> {
            self.session.ensure_authorized()?;
            let bytes = bytes.context("QUIC reliable lane closed")?;
            if bytes.len() > self.receive_limit {
                bail!("QUIC reliable message exceeds application receive limit");
            }
            // Video uses only DATAGRAM in this version; otherwise a malicious
            // peer could bypass the decoder reference-chain gate over a stream.
            if matches!(
                Message::parse_from_bytes(&bytes)?.union,
                Some(message::Union::VideoFrame(_))
            ) {
                bail!("Video on a QUIC reliable lane");
            }
            Ok(bytes.as_ref().into())
        }
    }
    fn spawn_worker(
        session: Arc<Session>,
        mut shutdown: watch::Receiver<u8>,
        stop_at: u8,
        work: impl Future<Output = ()> + Send + 'static,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            tokio::pin!(work);
            loop {
                if *shutdown.borrow() >= stop_at {
                    session.closed().await;
                    return;
                }
                tokio::select! {
                    biased;
                    changed = shutdown.changed() => { if changed.is_err() { return; } },
                    _ = &mut work => return,
                }
            }
        })
    }
    fn terminal_control(message: &Message) -> bool {
        match &message.union {
            Some(message::Union::Misc(misc)) => matches!(
                misc.union,
                Some(base::message_proto::misc::Union::CloseReason(_))
            ),
            Some(message::Union::LoginResponse(response)) => match &response.union {
                Some(base::message_proto::login_response::Union::Error(error)) => {
                    use base::session_protocol::*;
                    !matches!(
                        error.as_str(),
                        LOGIN_MSG_PASSWORD_EMPTY
                            | LOGIN_MSG_PASSWORD_WRONG
                            | LOGIN_MSG_2FA_WRONG
                            | REQUIRE_2FA
                            | LOGIN_MSG_NO_PASSWORD_ACCESS
                            | LOGIN_SCREEN_WAYLAND
                    )
                }
                _ => false,
            },
            _ => false,
        }
    }
    fn fail(session: &Session, errors: &mpsc::Sender<String>, error: String) {
        session.close();
        // One terminal error is sufficient; a full/closed channel has an owner
        // already observing failure or an application which has been dropped.
        let _ = errors.try_send(error);
    }
    fn video_metadata(video: &VideoFrame) -> ResultType<(u32, bool)> {
        let display = video
            .display
            .try_into()
            .context("Invalid QUIC video display")?;
        let frames = match &video.union {
            Some(
                video_frame::Union::Vp8s(frames)
                | video_frame::Union::Vp9s(frames)
                | video_frame::Union::Av1s(frames)
                | video_frame::Union::H264s(frames)
                | video_frame::Union::H265s(frames),
            ) => frames,
            _ => bail!("QUIC video requires encoded frames"),
        };
        let first = frames.frames.first().context("Empty QUIC video frame")?;
        Ok((display, first.key))
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use base::{
            direct_transport::quic::{
                Authorization, ChannelBinding, Client, Listener, PrivatePkcs8KeyDer, RootCertStore,
            },
            message_proto::{
                Clipboard, EncodedVideoFrame, EncodedVideoFrames, MouseEvent, TestDelay,
            },
        };
        use hbb_common::sodiumoxide::crypto::sign;

        struct TestAuthorization(ChannelBinding, Arc<std::sync::atomic::AtomicBool>);
        impl Authorization for TestAuthorization {
            fn check(&self, channel: &ChannelBinding) -> ResultType<()> {
                if channel != &self.0 || !self.1.load(Ordering::Acquire) {
                    bail!("test channel mismatch");
                }
                Ok(())
            }
        }
        async fn transports() -> (SessionTransport, SessionTransport) {
            let (client, host, _) = transports_with_lease(true).await;
            (client, host)
        }
        async fn transports_with_lease(
            enable_media: bool,
        ) -> (
            SessionTransport,
            SessionTransport,
            Arc<std::sync::atomic::AtomicBool>,
        ) {
            let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
            let mut roots = RootCertStore::empty();
            roots.add(certificate.cert.der().clone()).unwrap();
            let (public, secret) = sign::gen_keypair();
            let host = Listener::bind(
                "127.0.0.1:0".parse().unwrap(),
                vec![certificate.cert.der().clone()],
                PrivatePkcs8KeyDer::from(certificate.key_pair.serialize_der()).into(),
                secret,
            )
            .unwrap();
            let client = Client::bind("127.0.0.1:0".parse().unwrap(), roots).unwrap();
            let (client, host) = tokio::join!(
                client.connect(host.local_addr().unwrap(), "localhost", public.0),
                host.accept()
            );
            let client = client.unwrap();
            let host = host.unwrap();
            let valid = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let client_auth = Arc::new(TestAuthorization(
                client.binding().clone(),
                Arc::new(std::sync::atomic::AtomicBool::new(true)),
            ));
            let host_auth = Arc::new(TestAuthorization(host.binding().clone(), valid.clone()));
            let (client, host) = tokio::join!(
                SessionTransport::from_quic(client.authorize(client_auth).unwrap()),
                SessionTransport::from_quic(host.authorize(host_auth).unwrap())
            );
            let mut client = client.unwrap();
            let mut host = host.unwrap();
            client.set_max_packet_length(usize::MAX);
            host.set_max_packet_length(usize::MAX);
            if enable_media {
                client.enable_media().unwrap();
                host.enable_media().unwrap();
                let _ = received(&mut client).await;
                let _ = received(&mut host).await;
            }
            (client, host, valid)
        }
        fn video(key: bool, size: usize) -> Message {
            let mut video = VideoFrame {
                display: 0,
                ..Default::default()
            };
            video.set_h264s(EncodedVideoFrames {
                frames: vec![EncodedVideoFrame {
                    data: vec![7; size].into(),
                    key,
                    pts: 1,
                    ..Default::default()
                }],
                ..Default::default()
            });
            let mut message = Message::new();
            message.set_video_frame(video);
            message
        }
        async fn received(transport: &mut SessionTransport) -> Message {
            let bytes = timeout(Duration::from_secs(2), transport.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            Message::parse_from_bytes(&bytes).unwrap()
        }
        fn raw(transport: &SessionTransport) -> Arc<Session> {
            let SessionTransport::Quic(transport) = transport else {
                panic!("expected QUIC");
            };
            transport.session.clone()
        }
        async fn inject(session: &Session, frame: u64, key: bool) {
            let message = video(key, 2000);
            for fragment in fragment_frame(
                FrameMetadata {
                    frame_id: frame,
                    display_id: 0,
                    keyframe: key,
                },
                &message.write_to_bytes().unwrap(),
                session.max_datagram_size().unwrap(),
            )
            .unwrap()
            {
                session.send_media_wait(fragment.into()).await.unwrap();
            }
        }

        #[tokio::test]
        async fn protobuf_control_input_clipboard_and_video_round_trip() {
            let (mut client, mut host) = transports().await;
            let mut control = Message::new();
            control.set_test_delay(TestDelay {
                time: 42,
                ..Default::default()
            });
            client.send(&control).await.unwrap();
            assert_eq!(received(&mut host).await, control);
            let mut input = Message::new();
            input.set_mouse_event(MouseEvent {
                x: 12,
                y: 34,
                ..Default::default()
            });
            client.send(&input).await.unwrap();
            assert_eq!(received(&mut host).await, input);
            let mut clipboard = Message::new();
            clipboard.set_clipboard(Clipboard {
                content: b"clipboard".to_vec().into(),
                ..Default::default()
            });
            host.send(&clipboard).await.unwrap();
            assert_eq!(received(&mut client).await, clipboard);
            let frame = video(true, 4000);
            host.send(&frame).await.unwrap();
            host.send(&control).await.unwrap();
            let first = received(&mut client).await;
            let second = received(&mut client).await;
            assert!((first == control && second == frame) || (first == frame && second == control));
        }

        #[tokio::test]
        async fn lost_fragments_and_whole_frames_request_keyframes_without_decoding_deltas() {
            let (mut host, mut client) = transports().await;
            let sender = raw(&host);
            inject(&sender, 0, true).await;
            assert_eq!(received(&mut client).await, video(true, 2000));
            let fragments = fragment_frame(
                FrameMetadata {
                    frame_id: 1,
                    display_id: 0,
                    keyframe: false,
                },
                &video(false, 2000).write_to_bytes().unwrap(),
                sender.max_datagram_size().unwrap(),
            )
            .unwrap();
            sender
                .send_media_wait(fragments[0].clone().into())
                .await
                .unwrap();
            assert!(timeout(Duration::from_millis(160), client.next())
                .await
                .is_err());
            assert!(matches!(
                received(&mut host).await.misc().union,
                Some(base::message_proto::misc::Union::RefreshVideoDisplay(0))
            ));
            inject(&sender, 2, false).await;
            assert!(timeout(Duration::from_millis(30), client.next())
                .await
                .is_err());
            inject(&sender, 3, true).await;
            assert_eq!(received(&mut client).await, video(true, 2000));
            inject(&sender, 1, false).await;
            inject(&sender, 4, false).await;
            assert_eq!(received(&mut client).await, video(false, 2000));
            // No fragments whatsoever for frame 5: completion of 6 reveals the gap.
            inject(&sender, 6, false).await;
            assert!(timeout(Duration::from_millis(30), client.next())
                .await
                .is_err());
            assert!(matches!(
                received(&mut host).await.misc().union,
                Some(base::message_proto::misc::Union::RefreshVideoDisplay(0))
            ));
            inject(&sender, 7, true).await;
            assert_eq!(received(&mut client).await, video(true, 2000));
        }

        #[tokio::test]
        async fn reliable_limits_queue_pressure_and_media_path_cannot_bypass_receive_gate() {
            let (mut client, mut host) = transports().await;
            let mut clipboard = Message::new();
            clipboard.set_clipboard(Clipboard {
                content: vec![0; MAX_RELIABLE_MESSAGE + 1].into(),
                ..Default::default()
            });
            assert!(client.send(&clipboard).await.is_err());
            host.set_max_packet_length(4);
            raw(&client)
                .send_media_wait(
                    fragment_frame(
                        FrameMetadata {
                            frame_id: 0,
                            display_id: 0,
                            keyframe: true,
                        },
                        &video(true, 20).write_to_bytes().unwrap(),
                        1200,
                    )
                    .unwrap()
                    .remove(0)
                    .into(),
                )
                .await
                .unwrap();
            assert!(timeout(Duration::from_secs(2), host.next())
                .await
                .unwrap()
                .unwrap()
                .is_err());

            let (mut client, _host) = transports().await;
            let mut control = Message::new();
            control.set_test_delay(TestDelay {
                time: 42,
                ..Default::default()
            });
            for _ in 0..RELIABLE_QUEUE {
                client.send(&control).await.unwrap();
            }
            assert!(client.send(&control).await.is_err());

            let (client, mut host) = transports().await;
            let SessionTransport::Quic(client) = client else {
                panic!("expected QUIC");
            };
            client
                .enqueue(0, video(true, 20).write_to_bytes().unwrap().into())
                .unwrap();
            assert!(timeout(Duration::from_secs(2), host.next())
                .await
                .unwrap()
                .unwrap()
                .is_err());
        }
        #[tokio::test]
        async fn revoked_authority_cannot_deliver_already_buffered_input() {
            let (mut client, mut host, valid) = transports_with_lease(true).await;
            let mut input = Message::new();
            input.set_mouse_event(MouseEvent {
                x: 12,
                ..Default::default()
            });
            client.send(&input).await.unwrap();
            let SessionTransport::Quic(transport) = &host else {
                panic!("expected QUIC");
            };
            timeout(Duration::from_secs(2), async {
                while transport.lanes[1].receiver.is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            valid.store(false, Ordering::Release);
            assert!(host.next().await.unwrap().is_err());
            assert!(host.send(&input).await.is_err());
        }
        #[tokio::test]
        async fn media_waits_for_application_login_and_a_fresh_reference_frame() {
            let (mut host, mut client, _) = transports_with_lease(false).await;
            let sender = raw(&host);
            inject(&sender, 0, true).await;
            assert!(timeout(Duration::from_millis(30), client.next())
                .await
                .is_err());
            let mut response = base::message_proto::LoginResponse::new();
            response.set_peer_info(base::message_proto::PeerInfo::default());
            let mut login = Message::new();
            login.set_login_response(response);
            host.send(&login).await.unwrap();
            assert_eq!(received(&mut client).await, login);
            client.enable_media().unwrap();
            assert!(matches!(
                received(&mut host).await.misc().union,
                Some(base::message_proto::misc::Union::RefreshVideo(true))
            ));
            inject(&sender, 1, false).await;
            assert!(timeout(Duration::from_millis(30), client.next())
                .await
                .is_err());
            inject(&sender, 2, true).await;
            assert_eq!(received(&mut client).await, video(true, 2000));
        }
        #[tokio::test]
        async fn terminal_shutdown_freezes_queued_business_and_expires() {
            let (mut sender, mut receiver) = transports().await;
            sender.set_send_timeout(200);
            let mut input = Message::new();
            input.set_mouse_event(MouseEvent::default());
            // These synchronous enqueues cannot poll their worker before the
            // terminal send freezes it on this current-thread runtime.
            sender.send(&input).await.unwrap();
            sender.send(&video(true, 2000)).await.unwrap();
            let mut close = Message::new();
            let mut misc = Misc::new();
            misc.set_close_reason("Closed by owner".into());
            close.set_misc(misc);
            sender.send(&close).await.unwrap();
            assert!(sender.send(&input).await.is_err());
            drop(sender);
            assert_eq!(received(&mut receiver).await, close);
            let SessionTransport::Quic(transport) = &mut receiver else {
                unreachable!()
            };
            assert!(transport.lanes[1].receiver.try_recv().is_err());
            assert!(
                timeout(Duration::from_millis(30), transport.session.recv_media())
                    .await
                    .is_err()
            );
            timeout(Duration::from_secs(1), transport.session.closed())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn retryable_login_errors_leave_control_direction_open() {
            let (mut sender, mut receiver) = transports().await;
            for error in [
                base::session_protocol::LOGIN_MSG_PASSWORD_EMPTY,
                base::session_protocol::LOGIN_MSG_PASSWORD_WRONG,
                base::session_protocol::LOGIN_MSG_2FA_WRONG,
                base::session_protocol::REQUIRE_2FA,
                base::session_protocol::LOGIN_MSG_NO_PASSWORD_ACCESS,
                base::session_protocol::LOGIN_SCREEN_WAYLAND,
            ] {
                let mut response = base::message_proto::LoginResponse::new();
                response.set_error(error.into());
                let mut message = Message::new();
                message.set_login_response(response);
                sender.send(&message).await.unwrap();
                assert_eq!(received(&mut receiver).await, message);
            }
            let mut message = Message::new();
            message.set_test_delay(TestDelay::default());
            sender.send(&message).await.unwrap();
            assert_eq!(received(&mut receiver).await, message);
        }

        #[tokio::test]
        async fn final_control_is_written_before_immediate_transport_drop() {
            for login_error in [false, true] {
                let (mut sender, mut receiver) = transports().await;
                let mut final_message = Message::new();
                if login_error {
                    let mut response = base::message_proto::LoginResponse::new();
                    response.set_error("Permission denied".into());
                    final_message.set_login_response(response);
                } else {
                    let mut misc = Misc::new();
                    misc.set_close_reason("Closed by owner".into());
                    final_message.set_misc(misc);
                }
                sender.send(&final_message).await.unwrap();
                drop(sender);
                assert_eq!(received(&mut receiver).await, final_message);
            }
        }
    }
}
