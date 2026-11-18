//! TCP owns authorization and ordered control; authenticated UDP owns all media.
mod client;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod host;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod send_timer;
use crate::client::PendingAssociation;
#[cfg(test)]
use crate::host as udp_host;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::host::{HostSession, PathStatus};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use base::direct_transport::udp_media::{KeyframeCache, Pacer, SendDecision};
use base::{
    direct_transport::udp_media::{
        self as media, AuthenticatedPacket, CompletedFrame, FrameMetadata, FrameNotice, FramePlan,
        KeyframeComplete, KeyframeNack, MediaError, MediaKind, ProbeKind, Reassembler,
        ReceiveOutcome, Recovery, SessionId,
    },
    message_proto::{
        message, misc, udp_media_control, video_frame, Message, Misc, UdpMediaControl,
        UdpMediaOffer,
    },
};
use hbb_common::{
    anyhow::{bail, Context},
    bytes::{Bytes, BytesMut},
    config::CONNECT_TIMEOUT,
    protobuf::Message as _,
    sodiumoxide::randombytes,
    tcp::{FramedStreamReader, FramedStreamWriter},
    tokio::{
        self,
        sync::{mpsc, oneshot, watch, Notify},
        task::JoinHandle,
        time::{interval, timeout, MissedTickBehavior},
    },
    ResultType, Stream,
};
use serde::{Deserialize, Serialize};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::collections::VecDeque;
use std::{
    collections::BTreeMap,
    io,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const CONTROL_QUEUE: usize = 32;
const MEDIA_QUEUE: usize = 2;
const CONTROL_LIMIT: usize = 128 * 1024;
const MAX_STREAMS: usize = 65;
const RELIABLE_BYTES: usize = 64 * 1024 * 1024;
const INTERNAL_BYTES: usize = 1024 * 1024;
type StreamKey = (MediaKind, u32);
type MediaIdentity = (MediaKind, u32, bool, Option<u8>);
fn fail(sender: &watch::Sender<Option<String>>, error: String) {
    sender.send_if_modified(|failure| {
        if failure.is_some() {
            false
        } else {
            *failure = Some(error);
            true
        }
    });
}
fn reserve(counter: &AtomicUsize, amount: usize, limit: usize) -> bool {
    let mut used = counter.load(Ordering::Acquire);
    loop {
        let Some(next) = used.checked_add(amount).filter(|next| *next <= limit) else {
            return false;
        };
        match counter.compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(current) => used = current,
        }
    }
}

pub struct Handshake {
    stream: Stream,
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    registration: Option<host::Registration>,
}
impl std::ops::Deref for Handshake {
    type Target = Stream;
    fn deref(&self) -> &Stream {
        &self.stream
    }
}
impl std::ops::DerefMut for Handshake {
    fn deref_mut(&mut self) -> &mut Stream {
        &mut self.stream
    }
}

pub enum SessionTransport {
    Handshake(Box<Handshake>),
    Hybrid(Box<HybridTransport>),
    Closed,
}
impl From<Stream> for SessionTransport {
    fn from(stream: Stream) -> Self {
        Self::Handshake(Box::new(Handshake {
            stream,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            registration: None,
        }))
    }
}
impl SessionTransport {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn accepted(stream: Stream, registration: Option<host::Registration>) -> Self {
        Self::Handshake(Box::new(Handshake {
            stream,
            registration,
        }))
    }
    pub fn media_sender(&self) -> Option<MediaSender> {
        match self {
            Self::Hybrid(stream) if stream.host => Some(stream.outbound.clone()),
            _ => None,
        }
    }
    pub async fn send_terminal(&mut self, message: &Message) -> ResultType<()> {
        match self {
            Self::Handshake(stream) => stream.send(message).await,
            Self::Hybrid(stream) => stream.reliable_send(message, true).await,
            Self::Closed => bail!("Session transport is closed"),
        }
    }
    pub fn is_secured(&self) -> bool {
        match self {
            Self::Handshake(stream) => stream.is_secured(),
            Self::Hybrid(_) => true,
            Self::Closed => false,
        }
    }
    pub fn protocol(&self) -> &'static str {
        match self {
            Self::Hybrid(stream) if stream.ready.load(Ordering::Acquire) => "TCP+UDP",
            _ => "TCP",
        }
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn video_bitrate_budget(&self) -> Option<u32> {
        match self {
            Self::Hybrid(stream) if stream.host && stream.ready.load(Ordering::Acquire) => {
                let bps = stream.bitrate_budget.load(Ordering::Acquire);
                (bps != 0).then_some((bps / 1000).clamp(64, 1_000_000) as u32)
            }
            _ => None,
        }
    }
    pub fn set_send_timeout(&mut self, ms: u64) {
        match self {
            Self::Handshake(stream) => stream.set_send_timeout(ms),
            Self::Hybrid(stream) => {
                stream.send_timeout.store(ms, Ordering::Release);
            }
            Self::Closed => {}
        }
    }
    pub fn set_max_packet_length(&mut self, limit: usize) {
        match self {
            Self::Handshake(stream) => stream.set_max_packet_length(limit),
            Self::Hybrid(stream) => {
                stream
                    .receive_limit
                    .store(limit.min(media::MAX_VIDEO_FRAME), Ordering::Release);
                stream.limits.send_replace(Some(limit));
            }
            Self::Closed => {}
        }
    }
    pub async fn enable_client(&mut self, offer: &UdpMediaOffer) -> ResultType<()> {
        let Self::Handshake(stream) = self else {
            bail!("UDP media association is already configured");
        };
        let seed = stream
            .udp_media_seed()?
            .context("UDP requires an encrypted TCP session")?;
        let peer = stream
            .peer_addr()
            .context("UDP requires a connected TCP peer")?;
        let pending = crate::client::associate(offer, seed, peer).await?;
        let Self::Handshake(stream) = std::mem::replace(self, Self::Closed) else {
            unreachable!()
        };
        *self = Self::Hybrid(Box::new(HybridTransport::client(stream.stream, pending)?));
        Ok(())
    }
    pub async fn offer_host(&mut self) -> ResultType<Option<UdpMediaOffer>> {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        return Ok(None);
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let Self::Handshake(stream) = self else {
                bail!("UDP media association is already configured");
            };
            let Some(seed) = stream.udp_media_seed()? else {
                return Ok(None);
            };
            let registration = stream
                .registration
                .as_ref()
                .context("TCP session has no UDP listener owner")?;
            let host = registration.register(seed)?;
            let offer = UdpMediaOffer {
                version: u32::from(media::VERSION),
                session_id: host.session_id().0.to_vec().into(),
                mtu: host.mtu() as u32,
                ..Default::default()
            };
            let Self::Handshake(stream) = std::mem::replace(self, Self::Closed) else {
                unreachable!()
            };
            *self = Self::Hybrid(Box::new(HybridTransport::host(stream.stream, host)?));
            Ok(Some(offer))
        }
    }
    pub fn set_media_bitrate_limit(&mut self, kbps: u32) -> ResultType<()> {
        if let Self::Hybrid(stream) = self {
            let bps = if kbps == 0 {
                1_000_000_000
            } else {
                u64::from(kbps)
                    .checked_mul(1000)
                    .context("UDP bitrate overflow")?
            };
            if !(1..=1_000_000_000).contains(&bps) {
                bail!("UDP bitrate exceeds the supported upper limit");
            }
            stream.command(Command::Limit(bps))?;
        }
        Ok(())
    }
    pub fn revoke_media(&mut self) {
        if let Self::Hybrid(stream) = self {
            stream.ready.store(false, Ordering::Release);
            fail(
                &stream.failure_sender,
                "UDP media authorization revoked".into(),
            );
        }
    }
    pub async fn send(&mut self, message: &Message) -> ResultType<()> {
        match self {
            Self::Handshake(stream) => stream.send(message).await,
            Self::Hybrid(stream) => stream.send(message).await,
            Self::Closed => bail!("Session transport is closed"),
        }
    }
    pub async fn send_bytes(&mut self, bytes: Bytes) -> ResultType<()> {
        match self {
            Self::Handshake(stream) => stream.send_bytes(bytes).await,
            Self::Hybrid(stream) => {
                stream.check()?;
                stream.writer.push(bytes.to_vec(), false, false, true, None)
            }
            Self::Closed => bail!("Session transport is closed"),
        }
    }
    pub async fn send_raw(&mut self, bytes: Vec<u8>) -> ResultType<()> {
        match self {
            Self::Handshake(stream) => stream.send_raw(bytes).await,
            Self::Hybrid(stream) => stream.send(&Message::parse_from_bytes(&bytes)?).await,
            Self::Closed => bail!("Session transport is closed"),
        }
    }
    pub async fn next(&mut self) -> Option<io::Result<BytesMut>> {
        match self {
            Self::Handshake(stream) => stream.next().await,
            Self::Hybrid(stream) => Some(
                stream
                    .next()
                    .await
                    .map_err(|error| io::Error::other(format!("{error:#}"))),
            ),
            Self::Closed => Some(Err(io::Error::other("Session transport is closed"))),
        }
    }
    pub async fn next_timeout(&mut self, ms: u64) -> Option<io::Result<BytesMut>> {
        timeout(Duration::from_millis(ms), self.next())
            .await
            .ok()
            .flatten()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Epoch {
    kind: MediaKind,
    display: u32,
    epoch: u64,
    ack: bool,
    format: Option<Vec<u8>>,
}
enum Command {
    Epoch(Epoch),
    Notice(FrameNotice),
    Nack(KeyframeNack),
    Complete(KeyframeComplete),
    Limit(u64),
}
struct WritePacket {
    bytes: Vec<u8>,
    completion: Option<oneshot::Sender<Result<(), String>>>,
    priority: bool,
    internal: bool,
    sealed: bool,
    charged: bool,
}
#[derive(Default)]
struct CoalescedNotices {
    slots: Mutex<BTreeMap<StreamKey, (FrameMetadata, Vec<u8>)>>,
    available: Notify,
}
#[derive(Default)]
struct WriteBudget {
    normal: AtomicUsize,
    internal: AtomicUsize,
    bytes: AtomicUsize,
    control_bytes: AtomicUsize,
}
impl WriteBudget {
    fn release(&self, bytes: usize, internal: bool) {
        let (count, total) = if internal {
            (&self.internal, &self.control_bytes)
        } else {
            (&self.normal, &self.bytes)
        };
        count.fetch_sub(1, Ordering::AcqRel);
        total.fetch_sub(bytes, Ordering::AcqRel);
    }
}
struct ReceivedPacket {
    bytes: BytesMut,
    charged: usize,
    budget: Arc<AtomicUsize>,
}
impl Drop for ReceivedPacket {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.charged, Ordering::AcqRel);
    }
}
impl ReceivedPacket {
    fn into_bytes(mut self) -> BytesMut {
        std::mem::take(&mut self.bytes)
    }
}
#[derive(Clone)]
struct ReliableWriter {
    sender: mpsc::Sender<WritePacket>,
    priority: Arc<AtomicUsize>,
    budget: Arc<WriteBudget>,
    notices: Arc<CoalescedNotices>,
}
impl ReliableWriter {
    fn enqueue(
        &self,
        bytes: Vec<u8>,
        priority: bool,
        completion: Option<oneshot::Sender<Result<(), String>>>,
    ) -> ResultType<()> {
        self.push(bytes, priority, false, false, completion)
    }
    fn push(
        &self,
        bytes: Vec<u8>,
        priority: bool,
        internal: bool,
        sealed: bool,
        completion: Option<oneshot::Sender<Result<(), String>>>,
    ) -> ResultType<()> {
        let (count, total, count_limit, byte_limit) = if internal {
            (
                &self.budget.internal,
                &self.budget.control_bytes,
                8,
                INTERNAL_BYTES,
            )
        } else {
            (
                &self.budget.normal,
                &self.budget.bytes,
                CONTROL_QUEUE,
                RELIABLE_BYTES,
            )
        };
        let size = bytes.len();
        if !reserve(count, 1, count_limit) {
            bail!("TCP reliable queue is full");
        }
        if !reserve(total, size, byte_limit) {
            count.fetch_sub(1, Ordering::AcqRel);
            bail!("TCP reliable queue byte budget exhausted");
        }
        if priority {
            self.priority.fetch_add(1, Ordering::AcqRel);
        }
        if let Err(error) = self.sender.try_send(WritePacket {
            bytes,
            completion,
            priority,
            internal,
            sealed,
            charged: true,
        }) {
            if priority {
                self.priority.fetch_sub(1, Ordering::AcqRel);
            }
            self.budget.release(size, internal);
            bail!("UDP session reliable writer unavailable: {error}");
        }
        Ok(())
    }
    fn control(
        &self,
        sid: SessionId,
        union: udp_media_control::Union,
        priority: bool,
    ) -> ResultType<()> {
        let control = UdpMediaControl {
            session_id: sid.0.to_vec().into(),
            union: Some(union),
            ..Default::default()
        };
        let mut misc = Misc::new();
        misc.set_udp_media(control);
        let mut message = Message::new();
        message.set_misc(misc);
        let bytes = message.write_to_bytes()?;
        if bytes.len() > CONTROL_LIMIT {
            bail!("UDP media internal control exceeds its limit");
        }
        self.push(bytes, priority, true, false, None)
    }
    fn notice(&self, sid: SessionId, notice: FrameNotice) -> ResultType<()> {
        let metadata = match notice {
            FrameNotice::Sent(frame) => frame.metadata,
            FrameNotice::Abandoned(metadata) => metadata,
        };
        if metadata.kind == MediaKind::Audio {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&notice)?;
        if metadata.keyframe {
            return self.control(sid, udp_media_control::Union::Notice(bytes.into()), false);
        }
        let mut misc = Misc::new();
        misc.set_udp_media(UdpMediaControl {
            session_id: sid.0.to_vec().into(),
            union: Some(udp_media_control::Union::Notice(bytes.into())),
            ..Default::default()
        });
        let mut message = Message::new();
        message.set_misc(misc);
        let key = (metadata.kind, metadata.display_id);
        let mut slots = self.notices.slots.lock().unwrap();
        if !slots.contains_key(&key) && slots.len() >= MAX_STREAMS {
            bail!("Too many UDP notice streams");
        }
        if slots.get(&key).is_none_or(|(previous, _)| {
            (previous.epoch, previous.frame_id) <= (metadata.epoch, metadata.frame_id)
        }) {
            slots.insert(key, (metadata, message.write_to_bytes()?));
        }
        drop(slots);
        self.notices.available.notify_one();
        Ok(())
    }
}
struct SendStream {
    epoch: u64,
    next: u64,
    codec: Option<u8>,
}
#[derive(Default)]
struct ReceiveStream {
    epoch: u64,
    last: Option<u64>,
    needs_keyframe: bool,
}
struct QueuedFrame {
    plan: FramePlan,
    queued: Instant,
}
struct WorkerChannels {
    frames: mpsc::Receiver<QueuedFrame>,
    audio: mpsc::Receiver<QueuedFrame>,
    commands: mpsc::Receiver<Command>,
    completed: mpsc::Sender<CompletedFrame>,
    failed: watch::Sender<Option<String>>,
}

type VideoReleased = Box<dyn Fn(u32, Instant) + Send + Sync>;

fn video_released(
    callback: &OnceLock<VideoReleased>,
    kind: MediaKind,
    display: u32,
    queued: Instant,
) {
    if kind == MediaKind::Video {
        if let Some(callback) = callback.get() {
            callback(display, queued);
        }
    }
}

/// A producer lease of the same bounded UDP sender used by the connection.
/// Clones share epoch/frame counters; dropping or revoking the TCP owner closes all clones.
#[derive(Clone)]
pub struct MediaSender {
    inner: Arc<Mutex<Outbound>>,
    failure: watch::Sender<Option<String>>,
    released: Arc<OnceLock<VideoReleased>>,
}
impl MediaSender {
    /// Releases deltas at dequeue and keyframes after sending; the callback must not block.
    pub fn on_video_released(
        &self,
        callback: impl Fn(u32, Instant) + Send + Sync + 'static,
    ) -> ResultType<()> {
        self.released
            .set(Box::new(callback))
            .map_err(|_| hbb_common::anyhow::anyhow!("Video release callback already registered"))
    }
    pub fn send_video(&self, message: Arc<Message>) -> ResultType<()> {
        let result = self
            .inner
            .lock()
            .unwrap()
            .send(&message)
            .and_then(|handled| {
                if !handled {
                    bail!("non-media message submitted by a video producer");
                }
                Ok(())
            });
        if let Err(error) = &result {
            fail(&self.failure, format!("{error:#}"));
        }
        result
    }

    /// Caller holds its producer publication lock while draining the previous FIFO.
    pub fn migrate_video<T>(
        &self,
        previous: &mut mpsc::UnboundedReceiver<(T, Arc<Message>)>,
    ) -> ResultType<()> {
        while let Ok((_, message)) = previous.try_recv() {
            self.send_video(message)?;
        }
        Ok(())
    }
}
struct Outbound {
    session_id: SessionId,
    host: bool,
    ready: Arc<AtomicBool>,
    failure: watch::Receiver<Option<String>>,
    writer: ReliableWriter,
    commands: mpsc::Sender<Command>,
    outgoing: mpsc::Sender<QueuedFrame>,
    audio: mpsc::Sender<QueuedFrame>,
    sending: BTreeMap<StreamKey, SendStream>,
    released: Arc<OnceLock<VideoReleased>>,
}
impl Outbound {
    fn command(&self, command: Command) -> ResultType<()> {
        self.commands.try_send(command).map_err(|error| {
            hbb_common::anyhow::anyhow!("UDP media control queue unavailable: {error}")
        })
    }
    fn epoch(
        &mut self,
        kind: MediaKind,
        display: u32,
        format: Option<Vec<u8>>,
        codec: Option<u8>,
    ) -> ResultType<u64> {
        if !self.sending.contains_key(&(kind, display)) && self.sending.len() >= MAX_STREAMS {
            bail!("Too many UDP media streams");
        }
        let stream = self.sending.entry((kind, display)).or_insert(SendStream {
            epoch: 0,
            next: 0,
            codec: None,
        });
        stream.epoch = stream
            .epoch
            .checked_add(1)
            .context("UDP media epoch exhausted")?;
        stream.codec = codec;
        let epoch = Epoch {
            kind,
            display,
            epoch: stream.epoch,
            ack: false,
            format,
        };
        // Install the local generation before the TCP actor can publish it. An
        // ACK arriving on another runtime thread must never precede its owner.
        self.command(Command::Epoch(epoch.clone()))?;
        self.writer.control(
            self.session_id,
            udp_media_control::Union::Epoch(serde_json::to_vec(&epoch)?.into()),
            true,
        )?;
        Ok(epoch.epoch)
    }
    fn send(&mut self, message: &Message) -> ResultType<bool> {
        if let Some(error) = self.failure.borrow().as_ref() {
            bail!("{error}");
        }
        let boundary = media::video_from_keyframe_boundary(message);
        let message = boundary.as_ref().unwrap_or(message);
        if let Some((kind, display, keyframe, codec)) = media_metadata(message)? {
            if !self.host {
                bail!("Controller media transmission is unavailable");
            }
            if !self.ready.load(Ordering::Acquire) {
                video_released(&self.released, kind, display, Instant::now());
                return Ok(true);
            }
            let current = self.sending.get(&(kind, display));
            if current.is_none()
                || (kind == MediaKind::Video && current.is_some_and(|stream| stream.codec != codec))
            {
                self.epoch(kind, display, None, codec)?;
            }
            let stream = self
                .sending
                .get_mut(&(kind, display))
                .expect("invariant: media epoch initialized");
            let metadata = FrameMetadata {
                kind,
                display_id: display,
                epoch: stream.epoch,
                frame_id: stream.next,
                keyframe,
            };
            stream.next = stream
                .next
                .checked_add(1)
                .context("UDP media frame sequence exhausted")?;
            let plan = FramePlan::new(
                metadata,
                message.write_to_bytes()?.into(),
                media::DEFAULT_MTU,
            )?;
            let sender = if kind == MediaKind::Audio {
                &self.audio
            } else {
                &self.outgoing
            };
            match sender.try_send(QueuedFrame {
                plan,
                queued: Instant::now(),
            }) {
                Ok(()) => Ok(true),
                Err(mpsc::error::TrySendError::Full(frame)) => {
                    video_released(&self.released, kind, display, frame.queued);
                    if kind == MediaKind::Video {
                        self.notice(FrameNotice::Abandoned(frame.plan.descriptor().metadata))?;
                    }
                    Ok(true)
                }
                Err(mpsc::error::TrySendError::Closed(_)) => bail!("UDP media writer stopped"),
            }
        } else if self.host {
            let format = match &message.union {
                Some(message::Union::Misc(misc)) => match &misc.union {
                    Some(misc::Union::AudioFormat(_)) => Some((MediaKind::Audio, 0)),
                    Some(misc::Union::SwitchDisplay(display)) => Some((
                        MediaKind::Video,
                        display.display.try_into().context("Invalid display ID")?,
                    )),
                    _ => None,
                },
                _ => None,
            };
            if let Some((kind, display)) = format {
                let codec = self
                    .sending
                    .get(&(kind, display))
                    .and_then(|stream| stream.codec);
                self.epoch(kind, display, Some(message.write_to_bytes()?), codec)?;
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }
    fn notice(&self, notice: FrameNotice) -> ResultType<()> {
        self.writer.notice(self.session_id, notice)
    }
}

pub struct HybridTransport {
    session_id: SessionId,
    host: bool,
    writer: ReliableWriter,
    reliable: mpsc::Receiver<ReceivedPacket>,
    media: mpsc::Receiver<CompletedFrame>,
    commands: mpsc::Sender<Command>,
    ready: Arc<AtomicBool>,
    failure: watch::Receiver<Option<String>>,
    failure_sender: watch::Sender<Option<String>>,
    limits: watch::Sender<Option<usize>>,
    receive_limit: Arc<AtomicUsize>,
    send_timeout: Arc<AtomicU64>,
    bitrate_budget: Arc<AtomicU64>,
    tasks: Vec<JoinHandle<()>>,
    outbound: MediaSender,
    receiving: BTreeMap<StreamKey, ReceiveStream>,
    video_recovery: Arc<Mutex<media::VideoRecovery>>,
}
impl Drop for HybridTransport {
    fn drop(&mut self) {
        self.ready.store(false, Ordering::Release);
        fail(
            &self.failure_sender,
            "session transport owner closed".into(),
        );
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl HybridTransport {
    fn new(stream: Stream, sid: SessionId, host: bool) -> ResultType<(Self, WorkerChannels)> {
        let Stream::Tcp(stream) = stream else {
            bail!("UDP media requires TCP");
        };
        if !stream.is_secured() {
            bail!("UDP media requires an encrypted TCP session");
        }
        let send_timeout = Arc::new(AtomicU64::new(stream.3));
        let receive_limit = Arc::new(AtomicUsize::new(media::MAX_VIDEO_FRAME));
        let ready = Arc::new(AtomicBool::new(false));
        let (write, writes) = mpsc::channel(CONTROL_QUEUE + 8);
        let writer = ReliableWriter {
            sender: write,
            priority: Arc::new(AtomicUsize::new(0)),
            budget: Arc::new(WriteBudget::default()),
            notices: Arc::new(CoalescedNotices::default()),
        };
        let (incoming, reliable) = mpsc::channel(CONTROL_QUEUE);
        let (completed, media) = mpsc::channel(MEDIA_QUEUE + 4);
        let (outgoing, frames) = mpsc::channel(MEDIA_QUEUE);
        let (audio, audio_rx) = mpsc::channel(16);
        let (commands, command_rx) = mpsc::channel(CONTROL_QUEUE);
        let (failed, failure) = watch::channel(None);
        let (limits, limit_rx) = watch::channel(None);
        let (reader, tcp_writer) = stream.into_split();
        let released = Arc::new(OnceLock::new());
        let tasks = vec![
            tokio::spawn(tcp_write(
                tcp_writer,
                writes,
                writer.clone(),
                send_timeout.clone(),
                failed.clone(),
            )),
            tokio::spawn(tcp_read(reader, incoming, limit_rx, failed.clone())),
        ];
        Ok((
            Self {
                outbound: MediaSender {
                    inner: Arc::new(Mutex::new(Outbound {
                        session_id: sid,
                        host,
                        ready: ready.clone(),
                        failure: failure.clone(),
                        writer: writer.clone(),
                        commands: commands.clone(),
                        outgoing,
                        audio,
                        sending: BTreeMap::new(),
                        released: released.clone(),
                    })),
                    failure: failed.clone(),
                    released,
                },
                session_id: sid,
                host,
                writer,
                reliable,
                media,
                commands,
                ready,
                failure,
                failure_sender: failed.clone(),
                limits,
                receive_limit,
                send_timeout,
                bitrate_budget: Arc::new(AtomicU64::new(0)),
                tasks,
                receiving: BTreeMap::new(),
                video_recovery: Arc::new(Mutex::new(media::VideoRecovery::default())),
            },
            WorkerChannels {
                frames,
                audio: audio_rx,
                commands: command_rx,
                completed,
                failed,
            },
        ))
    }
    fn client(stream: Stream, pending: PendingAssociation) -> ResultType<Self> {
        let (mut transport, channels) = Self::new(stream, pending.session_id, false)?;
        transport.tasks.push(tokio::spawn(client_media(
            pending,
            channels,
            transport.writer.clone(),
            transport.ready.clone(),
            transport.receive_limit.clone(),
            transport.video_recovery.clone(),
        )));
        Ok(transport)
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn host(stream: Stream, session: HostSession) -> ResultType<Self> {
        let (mut transport, channels) = Self::new(stream, session.session_id(), true)?;
        transport.tasks.push(tokio::spawn(host_media(
            session,
            channels,
            transport.writer.clone(),
            transport.ready.clone(),
            transport.bitrate_budget.clone(),
            transport.outbound.released.clone(),
        )));
        Ok(transport)
    }
    fn command(&self, command: Command) -> ResultType<()> {
        self.commands.try_send(command).map_err(|error| {
            hbb_common::anyhow::anyhow!("UDP media control queue unavailable: {error}")
        })
    }
    fn check(&self) -> ResultType<()> {
        if let Some(error) = self.failure.borrow().as_ref() {
            bail!("{error}");
        }
        Ok(())
    }
    async fn reliable_send(&self, message: &Message, priority: bool) -> ResultType<()> {
        if !priority {
            self.check()?;
        }
        if !priority {
            return self.writer.enqueue(message.write_to_bytes()?, false, None);
        }
        let (completed, written) = oneshot::channel();
        self.writer
            .enqueue(message.write_to_bytes()?, priority, Some(completed))?;
        let deadline = self.send_timeout.load(Ordering::Acquire);
        timeout(
            Duration::from_millis(if deadline == 0 {
                CONNECT_TIMEOUT
            } else {
                deadline
            }),
            written,
        )
        .await
        .context("TCP control write timed out")?
        .context("TCP control writer stopped")?
        .map_err(hbb_common::anyhow::Error::msg)
    }
    async fn send(&mut self, message: &Message) -> ResultType<()> {
        if terminal(message) {
            return self.reliable_send(message, true).await;
        }
        self.check()?;
        if !self.host {
            if let Some(message::Union::Misc(misc)) = &message.union {
                if let Some(misc::Union::Option(option)) = &misc.union {
                    if let Some(kbps) = option.bitrate_limit_kbps {
                        let bps = if kbps == 0 {
                            1_000_000_000
                        } else {
                            u64::from(kbps) * 1000
                        };
                        self.command(Command::Limit(bps))?;
                    }
                }
            }
        }
        let handled = self.outbound.inner.lock().unwrap().send(message)?;
        if handled {
            Ok(())
        } else {
            self.reliable_send(message, false).await
        }
    }
    fn control(
        &mut self,
        control: UdpMediaControl,
        permit: mpsc::OwnedPermit<Command>,
    ) -> ResultType<Option<BytesMut>> {
        if control.session_id.as_ref() != self.session_id.0 {
            bail!("UDP media control session mismatch");
        }
        match control.union.context("Missing UDP media control payload")? {
            udp_media_control::Union::Ready(ready) => {
                if self.host || !ready {
                    bail!("Invalid UDP media readiness confirmation");
                }
                if !self.ready.swap(true, Ordering::AcqRel) {
                    let mut misc = Misc::new();
                    misc.set_refresh_video(true);
                    let mut message = Message::new();
                    message.set_misc(misc);
                    self.writer
                        .enqueue(message.write_to_bytes()?, false, None)?;
                }
            }
            udp_media_control::Union::Epoch(bytes) => {
                let epoch: Epoch = bounded_json(&bytes)?;
                if epoch.kind == MediaKind::Audio && epoch.display != 0 {
                    bail!("Invalid audio epoch display");
                }
                if self.host {
                    if !epoch.ack || epoch.format.is_some() {
                        bail!("Expected UDP media epoch acknowledgement");
                    }
                    permit.send(Command::Epoch(epoch));
                } else {
                    if epoch.ack {
                        bail!("Unexpected UDP media epoch acknowledgement");
                    }
                    if !self.receiving.contains_key(&(epoch.kind, epoch.display))
                        && self.receiving.len() >= MAX_STREAMS
                    {
                        bail!("Too many UDP media receive streams");
                    }
                    if self
                        .receiving
                        .get(&(epoch.kind, epoch.display))
                        .is_some_and(|stream| stream.epoch >= epoch.epoch)
                    {
                        bail!("UDP media epoch did not advance");
                    }
                    if let Some(format) = &epoch.format {
                        validate_format(&Message::parse_from_bytes(format)?, &epoch)?;
                    }
                    // The worker consumes queued commands before receiving UDP packets.
                    // Queue the epoch before its ACK without yielding to a cancellable caller.
                    permit.send(Command::Epoch(epoch.clone()));
                    if epoch.kind == MediaKind::Video {
                        self.video_recovery.lock().unwrap().set_epoch(
                            epoch.display,
                            epoch.epoch,
                            Instant::now(),
                        )?;
                    }
                    self.receiving.insert(
                        (epoch.kind, epoch.display),
                        ReceiveStream {
                            epoch: epoch.epoch,
                            needs_keyframe: true,
                            ..Default::default()
                        },
                    );
                    let ack = Epoch {
                        ack: true,
                        format: None,
                        ..epoch.clone()
                    };
                    self.writer.control(
                        self.session_id,
                        udp_media_control::Union::Epoch(serde_json::to_vec(&ack)?.into()),
                        true,
                    )?;
                    return Ok(epoch.format.map(|bytes| bytes.as_slice().into()));
                }
            }
            udp_media_control::Union::Notice(bytes) => {
                if self.host {
                    bail!("Unexpected UDP media frame notice");
                }
                permit.send(Command::Notice(bounded_json(&bytes)?));
            }
            udp_media_control::Union::Nack(bytes) => {
                if !self.host {
                    bail!("Unexpected UDP media NACK");
                }
                let nack: KeyframeNack = bounded_json(&bytes)?;
                nack.validate()?;
                permit.send(Command::Nack(nack));
            }
            udp_media_control::Union::Complete(bytes) => {
                if !self.host {
                    bail!("Unexpected UDP media completion");
                }
                let complete: KeyframeComplete = bounded_json(&bytes)?;
                if complete.metadata.kind != MediaKind::Video || !complete.metadata.keyframe {
                    bail!("Invalid UDP keyframe completion");
                }
                permit.send(Command::Complete(complete));
            }
            _ => bail!("Unsupported UDP media control payload"),
        }
        Ok(None)
    }
    async fn next(&mut self) -> ResultType<BytesMut> {
        loop {
            self.check()?;
            tokio::select! {
                biased;
                changed = self.failure.changed() => { changed.context("UDP session workers stopped")?; self.check()?; }
                packet = async {
                    // Reserve before consuming TCP: cancelling next() must not lose an epoch.
                    let permit = self.commands.clone().reserve_owned().await
                        .context("UDP media command owner stopped")?;
                    let bytes = self.reliable.recv().await.context("TCP peer closed")?;
                    Ok::<_, hbb_common::anyhow::Error>((bytes, permit))
                } => {
                    let (bytes, permit) = packet?;
                    let bytes = bytes.into_bytes();
                    let message = Message::parse_from_bytes(&bytes)?;
                    let control = match &message.union {
                        Some(message::Union::Misc(misc)) => match &misc.union {
                            Some(misc::Union::UdpMedia(control)) => Some(control.clone()), _ => None,
                        }, _ => None,
                    };
                    if let Some(control) = control {
                        let ready = matches!(control.union, Some(udp_media_control::Union::Ready(_)));
                        if let Some(format) = self.control(control, permit)? { return Ok(format); }
                        if ready { return Ok(bytes); }
                        continue;
                    }
                    if media_metadata(&message)?.is_some() { bail!("Media received over TCP after UDP negotiation"); }
                    if self.host {
                        if let Some(message::Union::Misc(misc)) = &message.union {
                            if let Some(misc::Union::Option(option)) = &misc.union {
                                if let Some(kbps) = option.bitrate_limit_kbps {
                                    let bps = if kbps == 0 { 1_000_000_000 } else { u64::from(kbps) * 1000 };
                                    permit.send(Command::Limit(bps));
                                }
                            }
                        }
                    }
                    return Ok(bytes);
                }
                frame = self.media.recv(), if !self.host => {
                    let frame = frame.context("UDP media receiver stopped")?;
                    if let Some(bytes) = self.deliver(frame)? { return Ok(bytes); }
                }
            }
        }
    }
    fn deliver(&mut self, frame: CompletedFrame) -> ResultType<Option<BytesMut>> {
        if !self.ready.load(Ordering::Acquire) {
            return Ok(None);
        }
        let metadata = frame.metadata;
        let Some(state) = self
            .receiving
            .get_mut(&(metadata.kind, metadata.display_id))
        else {
            return Ok(None);
        };
        if metadata.epoch != state.epoch || state.last.is_some_and(|last| metadata.frame_id <= last)
        {
            return Ok(None);
        }
        if media_metadata(&Message::parse_from_tokio_bytes(&frame.payload)?)?
            .map(|(kind, display, key, _)| (kind, display, key))
            != Some((metadata.kind, metadata.display_id, metadata.keyframe))
        {
            bail!("UDP media payload metadata mismatch");
        }
        if metadata.kind == MediaKind::Video {
            if state
                .last
                .is_some_and(|last| last.checked_add(1) != Some(metadata.frame_id))
            {
                state.needs_keyframe = true;
            }
            if metadata.keyframe {
                state.needs_keyframe = false;
            }
            if state.needs_keyframe {
                if self
                    .video_recovery
                    .lock()
                    .unwrap()
                    .request(metadata, false, Instant::now())
                {
                    request_refresh(&self.writer, metadata.display_id)?;
                }
                return Ok(None);
            }
        }
        state.last = Some(metadata.frame_id);
        self.video_recovery
            .lock()
            .unwrap()
            .frame_delivered(metadata);
        Ok(Some(frame.payload.try_into_mut().unwrap_or_else(
            |payload| BytesMut::from(payload.as_ref()),
        )))
    }
}

async fn tcp_write(
    mut writer: FramedStreamWriter,
    mut packets: mpsc::Receiver<WritePacket>,
    queue: ReliableWriter,
    send_timeout: Arc<AtomicU64>,
    failed: watch::Sender<Option<String>>,
) {
    loop {
        let packet = tokio::select! {
            biased;
            packet = packets.recv() => match packet { Some(packet) => packet, None => return },
            _ = queue.notices.available.notified() => {
                let mut slots = queue.notices.slots.lock().unwrap();
                let Some((_, (_, bytes))) = slots.pop_first() else { continue; };
                if !slots.is_empty() { queue.notices.available.notify_one(); }
                WritePacket { bytes, completion: None, priority: false, internal: false, sealed: false, charged: false }
            }
        };
        writer.set_send_timeout(send_timeout.load(Ordering::Acquire));
        let size = packet.bytes.len();
        let result = if packet.sealed {
            writer.send_bytes(packet.bytes.into()).await
        } else {
            writer.send_raw(packet.bytes).await
        }
        .map_err(|error| format!("TCP writer failed: {error:#}"));
        if packet.charged {
            queue.budget.release(size, packet.internal);
        }
        if packet.priority {
            queue.priority.fetch_sub(1, Ordering::AcqRel);
        }
        if let Some(completed) = packet.completion {
            let _ = completed.send(result.clone());
        }
        if let Err(error) = result {
            fail(&failed, error);
            return;
        }
    }
}
async fn tcp_read(
    mut reader: FramedStreamReader,
    incoming: mpsc::Sender<ReceivedPacket>,
    mut limits: watch::Receiver<Option<usize>>,
    failed: watch::Sender<Option<String>>,
) {
    let budget = Arc::new(AtomicUsize::new(0));
    reader.set_max_packet_length(RELIABLE_BYTES);
    loop {
        tokio::select! {
            changed = limits.changed() => {
                if changed.is_err() { return; }
                if let Some(limit) = *limits.borrow_and_update() { reader.set_max_packet_length(limit.min(RELIABLE_BYTES)); }
            }
            bytes = reader.next() => match bytes {
                Some(Ok(bytes)) => {
                    let charged = bytes.len();
                    if !reserve(&budget, charged, RELIABLE_BYTES) {
                        fail(&failed, "TCP receive queue byte budget exhausted".into()); return;
                    }
                    match timeout(Duration::from_secs(1), incoming.send(ReceivedPacket { bytes, charged, budget: budget.clone() })).await {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => return,
                        Err(_) => { fail(&failed, "TCP receive queue admission timed out".into()); return; }
                    }
                }
                Some(Err(error)) => { fail(&failed, format!("TCP reader failed: {error}")); return; }
                None => { fail(&failed, "TCP peer closed".into()); return; }
            }
        }
    }
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::*;
    use base::message_proto::{
        AudioFormat, AudioFrame, Clipboard, EncodedVideoFrame, EncodedVideoFrames, VideoFrame,
    };
    use hbb_common::{sodiumoxide::crypto::secretbox::Key, tcp::FramedStream};

    async fn streams() -> (Stream, Stream, tokio::net::TcpListener) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = Stream::Tcp(FramedStream::new(address, None, 3000).await.unwrap());
        let (tcp, _) = listener.accept().await.unwrap();
        let mut host = Stream::from(tcp, address);
        client.set_key(Key([11; 32]));
        host.set_key(Key([11; 32]));
        (client, host, listener)
    }
    async fn media_streams<T>(
        mut bind: impl FnMut(u16) -> ResultType<T>,
    ) -> (Stream, Stream, tokio::net::TcpListener, T) {
        for _ in 0..16 {
            let (client, host, listener) = streams().await;
            match bind(listener.local_addr().unwrap().port()) {
                Ok(media) => return (client, host, listener, media),
                Err(error)
                    if error
                        .downcast_ref::<io::Error>()
                        .is_some_and(|error| error.kind() == io::ErrorKind::AddrInUse) => {}
                Err(error) => panic!("cannot reserve TCP/UDP test endpoints: {error:#}"),
            }
        }
        panic!("cannot reserve a common TCP/UDP test port");
    }
    fn backpressured_streams() -> (Stream, Stream) {
        let (client, host) = tokio::io::duplex(8192);
        let local = std::net::SocketAddr::from(([0, 0, 0, 0], 0));
        let mut client = Stream::Tcp(FramedStream::from(client, local));
        let mut host = Stream::Tcp(FramedStream::from(host, local));
        client.set_key(Key([11; 32]));
        host.set_key(Key([11; 32]));
        (client, host)
    }
    fn video(key: bool, bytes: usize) -> Message {
        let mut video = VideoFrame::new();
        video.set_h264s(EncodedVideoFrames {
            frames: vec![EncodedVideoFrame {
                data: vec![2; bytes].into(),
                key,
                ..Default::default()
            }],
            ..Default::default()
        });
        let mut message = Message::new();
        message.set_video_frame(video);
        message
    }
    fn close() -> Message {
        let mut misc = Misc::new();
        misc.set_close_reason("test complete".into());
        let mut message = Message::new();
        message.set_misc(misc);
        message
    }
    #[tokio::test]
    async fn reliable_raw_messages_preserve_tcp_encryption() {
        let (client, mut host, _listener) = streams().await;
        let mut client: SessionTransport = client.into();
        let message = close();
        client
            .send_raw(message.write_to_bytes().unwrap())
            .await
            .unwrap();
        let bytes = host.next().await.unwrap().unwrap();
        assert_eq!(Message::parse_from_bytes(&bytes).unwrap(), message);
        assert_eq!(client.protocol(), "TCP");
    }
    #[tokio::test]
    async fn format_epoch_survives_a_competing_ui_cancel_and_is_delivered_once() {
        let (client, mut host) = backpressured_streams();
        let sid = SessionId([17; 16]);
        let (mut client, mut channels) = HybridTransport::new(client, sid, false).unwrap();
        let mut format = Misc::new();
        format.set_switch_display(base::message_proto::SwitchDisplay {
            width: 2560,
            height: 1440,
            ..Default::default()
        });
        let mut expected = Message::new();
        expected.set_misc(format);
        let epoch = Epoch {
            kind: MediaKind::Video,
            display: 0,
            epoch: 1,
            ack: false,
            format: Some(expected.write_to_bytes().unwrap()),
        };
        let mut control = Misc::new();
        control.set_udp_media(UdpMediaControl {
            session_id: sid.0.to_vec().into(),
            union: Some(udp_media_control::Union::Epoch(
                serde_json::to_vec(&epoch).unwrap().into(),
            )),
            ..Default::default()
        });
        let mut message = Message::new();
        message.set_misc(control);
        host.send(&message).await.unwrap();
        host.send(&close()).await.unwrap();

        for _ in 0..CONTROL_QUEUE {
            client.command(Command::Limit(8_000_000)).unwrap();
        }
        // Backpressure must wait, and cancelling that wait must leave the epoch on TCP.
        assert!(timeout(Duration::from_millis(20), client.next())
            .await
            .is_err());
        for _ in 0..CONTROL_QUEUE {
            channels.commands.recv().await.unwrap();
        }

        // A queued worker command makes the competing branch ready before an async ACK wait.
        let delivered = timeout(Duration::from_secs(2), async {
            tokio::select! {
                biased;
                packet = client.next() => packet.unwrap(),
                _ = channels.commands.recv() => client.next().await.unwrap(),
            }
        })
        .await
        .unwrap();
        assert_eq!(Message::parse_from_bytes(&delivered).unwrap(), expected);
        assert_eq!(
            Message::parse_from_bytes(&client.next().await.unwrap()).unwrap(),
            close()
        );
        let acknowledgement = host.next().await.unwrap().unwrap();
        let acknowledgement = Message::parse_from_bytes(&acknowledgement).unwrap();
        let Some(message::Union::Misc(misc)) = acknowledgement.union else {
            panic!("expected epoch acknowledgement");
        };
        let Some(misc::Union::UdpMedia(control)) = misc.union else {
            panic!("expected UDP epoch control");
        };
        let Some(udp_media_control::Union::Epoch(bytes)) = control.union else {
            panic!("expected epoch acknowledgement");
        };
        let acknowledgement: Epoch = bounded_json(&bytes).unwrap();
        assert!(acknowledgement.ack);
        assert_eq!(acknowledgement.epoch, 1);
        assert!(acknowledgement.format.is_none());
        assert!(timeout(Duration::from_millis(10), host.next())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn expired_audio_backlog_drains_before_the_next_live_packet() {
        let (client, host, _listener, udp) = media_streams(udp_host::bind).await;
        let mut host = SessionTransport::accepted(host, Some(udp.registration()));
        let offer = host.offer_host().await.unwrap().unwrap();
        let mut client: SessionTransport = client.into();
        client.enable_client(&offer).await.unwrap();
        client.next().await.unwrap().unwrap();
        let mut format = Misc::new();
        format.set_audio_format(AudioFormat {
            channels: 2,
            sample_rate: 48_000,
            ..Default::default()
        });
        let mut message = Message::new();
        message.set_misc(format);
        host.send(&message).await.unwrap();
        client.next().await.unwrap().unwrap();
        let mut barrier = Message::new();
        barrier.set_test_delay(base::message_proto::TestDelay {
            time: 88,
            ..Default::default()
        });
        client.send(&barrier).await.unwrap();
        loop {
            let message = Message::parse_from_bytes(&host.next().await.unwrap().unwrap()).unwrap();
            if matches!(message.union,Some(message::Union::TestDelay(ref delay)) if delay.time==88)
            {
                break;
            }
        }
        let mut audio = Message::new();
        audio.set_audio_frame(AudioFrame {
            data: vec![8; 100].into(),
            ..Default::default()
        });
        let SessionTransport::Hybrid(transport) = &host else {
            unreachable!()
        };
        {
            let mut ledger = transport.outbound.inner.lock().unwrap();
            let epoch = ledger.sending.get(&(MediaKind::Audio, 0)).unwrap().epoch;
            // A suspended producer can leave every queued packet older than its
            // playout deadline. These still use real signed UDP frame plans.
            for id in 0..16 {
                ledger
                    .audio
                    .try_send(QueuedFrame {
                        plan: FramePlan::new(
                            FrameMetadata {
                                kind: MediaKind::Audio,
                                display_id: 0,
                                epoch,
                                frame_id: id,
                                keyframe: false,
                            },
                            audio.write_to_bytes().unwrap().into(),
                            media::DEFAULT_MTU,
                        )
                        .unwrap(),
                        queued: Instant::now() - Duration::from_secs(1),
                    })
                    .unwrap_or_else(|_| panic!("fixture audio queue must fit"));
            }
            ledger.sending.get_mut(&(MediaKind::Audio, 0)).unwrap().next = 16;
        }
        let serving = tokio::spawn(async move {
            let mut tick = interval(Duration::from_millis(20));
            loop {
                tokio::select! {
                    _=tick.tick()=>host.send(&audio).await.unwrap(),
                    packet=host.next()=>{if packet.is_none() || packet.is_some_and(|p|p.is_err()){break;}}
                }
            }
        });
        timeout(Duration::from_millis(500), async {
            loop {
                let bytes = client.next().await.unwrap().unwrap();
                if matches!(
                    Message::parse_from_bytes(&bytes).unwrap().union,
                    Some(message::Union::AudioFrame(_))
                ) {
                    break;
                }
            }
        })
        .await
        .expect("stale packets must not keep a live audio stream behind a timer-sized backlog");
        serving.abort();
        let _ = serving.await;
    }

    #[tokio::test]
    async fn hybrid_fragmented_video_audio_and_control_use_real_sockets() {
        let (client, host, _listener, _udp) = media_streams(udp_host::bind).await;
        let mut host = SessionTransport::accepted(host, Some(_udp.registration()));
        let offer = host.offer_host().await.unwrap().unwrap();
        let mut client: SessionTransport = client.into();
        client.enable_client(&offer).await.unwrap();
        assert_eq!(client.protocol(), "TCP");
        timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(client.protocol(), "TCP+UDP");
        assert_eq!(host.protocol(), "TCP+UDP");
        let started = Instant::now();
        let payload_size = 4 * 1024 * 1024;
        let mut audio_format = Misc::new();
        audio_format.set_audio_format(AudioFormat {
            channels: 2,
            sample_rate: 48_000,
            ..Default::default()
        });
        let mut message = Message::new();
        message.set_misc(audio_format);
        host.send(&message).await.unwrap();
        let format = Message::parse_from_bytes(&client.next().await.unwrap().unwrap()).unwrap();
        assert!(
            matches!(format.union, Some(message::Union::Misc(misc)) if matches!(misc.union, Some(misc::Union::AudioFormat(_))))
        );
        let mut barrier = Message::new();
        barrier.set_test_delay(base::message_proto::TestDelay {
            time: 77,
            ..Default::default()
        });
        client.send(&barrier).await.unwrap();
        loop {
            let message = Message::parse_from_bytes(&host.next().await.unwrap().unwrap()).unwrap();
            if matches!(message.union, Some(message::Union::TestDelay(ref delay)) if delay.time == 77)
            {
                break;
            }
        }
        host.send(&video(true, payload_size)).await.unwrap();
        message = Message::new();
        message.set_audio_frame(AudioFrame {
            data: vec![3; 100].into(),
            ..Default::default()
        });
        host.send(&message).await.unwrap();
        message = Message::new();
        message.set_clipboard(Clipboard {
            content: b"ordered TCP control".to_vec().into(),
            ..Default::default()
        });
        host.send(&message).await.unwrap();
        let SessionTransport::Hybrid(host_transport) = &host else {
            unreachable!()
        };
        let host_failure = host_transport.failure.clone();
        let host_ready = host_transport.ready.clone();
        let serving = tokio::spawn(async move {
            let mut audio = Message::new();
            audio.set_audio_frame(AudioFrame {
                data: vec![3; 100].into(),
                ..Default::default()
            });
            let mut audio_tick = interval(Duration::from_millis(20));
            audio_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = audio_tick.tick() => host.send(&audio).await.unwrap(),
                    bytes = host.next() => {
                        let bytes = bytes.unwrap().unwrap();
                        if matches!(Message::parse_from_bytes(&bytes).unwrap().union,
                            Some(message::Union::Misc(misc)) if matches!(misc.union, Some(misc::Union::CloseReason(_))))
                        { break; }
                    }
                }
            }
        });
        let mut audio = false;
        let mut clipboard = false;
        let mut received_video = false;
        let result = timeout(Duration::from_secs(20), async {
            while !(audio && clipboard && received_video) {
                let bytes = client.next().await.unwrap().unwrap();
                match Message::parse_from_bytes(&bytes).unwrap().union {
                    Some(message::Union::AudioFrame(frame)) => {
                        assert_eq!(frame.data.len(), 100);
                        if !audio {
                            assert!(!received_video, "audio must preempt a large video transfer");
                        }
                        audio = true;
                    }
                    Some(message::Union::Clipboard(frame)) => {
                        assert_eq!(frame.content.as_ref(), b"ordered TCP control");
                        clipboard = true;
                    }
                    Some(message::Union::VideoFrame(frame)) => {
                        assert_eq!(frame.h264s().frames[0].data.len(), payload_size);
                        received_video = true;
                    }
                    _ => {}
                }
            }
        })
        .await;
        let client_failure = match &client {
            SessionTransport::Hybrid(transport) => transport.failure.borrow().clone(),
            _ => None,
        };
        let host_protocol = if host_ready.load(Ordering::Acquire) {
            "TCP+UDP"
        } else {
            "TCP"
        };
        eprintln!("actual hybrid receive result={result:?} audio={audio} clipboard={clipboard} video={received_video} host_protocol={host_protocol} host_failure={:?} host_task_finished={} client_protocol={} client_failure={client_failure:?}",
            *host_failure.borrow(), serving.is_finished(), client.protocol());
        result.expect(
            "hybrid video/audio/control must arrive within the existing 20-second deadline",
        );
        let elapsed = started.elapsed();
        eprintln!(
            "actual UDP fragmented payload_bytes={payload_size} elapsed_ms={} payload_mbps={:.3}",
            elapsed.as_millis(),
            payload_size as f64 * 8.0 / elapsed.as_secs_f64() / 1_000_000.0
        );
        client.send(&close()).await.unwrap();
        timeout(Duration::from_secs(3), serving)
            .await
            .unwrap()
            .unwrap();
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[tokio::test]
    async fn udp_encoder_budget_tracks_backlogged_deltas_without_limiting_idle_keyframes() {
        let (client, host, _listener, _udp) = media_streams(udp_host::bind).await;
        let mut host = SessionTransport::accepted(host, Some(_udp.registration()));
        let offer = host.offer_host().await.unwrap().unwrap();
        let mut client: SessionTransport = client.into();
        client.enable_client(&offer).await.unwrap();
        timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(host.video_bitrate_budget(), None);
        host.set_media_bitrate_limit(1_000).unwrap();
        let released = Arc::new(Notify::new());
        let notify = released.clone();
        host.media_sender()
            .unwrap()
            .on_video_released(move |_, _| notify.notify_one())
            .unwrap();
        let mut serving = tokio::spawn(async move {
            host.send(&video(true, 100)).await.unwrap();
            // A producer waiting for wire completion has no queued successor.
            let mut fetched = false;
            let mut tick = interval(Duration::from_millis(8));
            let until = tokio::time::sleep(Duration::from_millis(800));
            tokio::pin!(until);
            loop {
                tokio::select! {
                    _ = &mut until => return (host.video_bitrate_budget(), host),
                    _ = released.notified() => fetched = true,
                    _ = tick.tick(), if fetched => {
                        fetched = false;
                        host.send(&video(false, 1024)).await.unwrap();
                    }
                    packet = host.next() => { packet.unwrap().unwrap(); }
                }
            }
        });
        let budget = timeout(Duration::from_secs(3), async {
            loop {
                tokio::select! {
                    result = &mut serving => break result.unwrap().0,
                    packet = client.next() => { packet.unwrap().unwrap(); }
                }
            }
        })
        .await
        .unwrap()
        .expect("authenticated feedback from a backlogged UDP stream");
        assert!((64..=1_000).contains(&budget), "budget={budget}");
        assert_eq!(client.video_bitrate_budget(), None);
    }

    #[tokio::test]
    async fn dropped_udp_response_is_retried_before_tcp_ready() {
        let (client, mut host, _listener, socket) = media_streams(|port| {
            let socket = std::net::UdpSocket::bind(("127.0.0.1", port))?;
            socket.set_nonblocking(true)?;
            Ok(tokio::net::UdpSocket::from_std(socket)?)
        })
        .await;
        let seed = host.udp_media_seed().unwrap().unwrap();
        let sid = SessionId([8; 16]);
        let offer = UdpMediaOffer {
            version: 1,
            session_id: sid.0.to_vec().into(),
            mtu: media::DEFAULT_MTU as u32,
            ..Default::default()
        };
        let serving = tokio::spawn(async move {
            let mut sender =
                media::DatagramSender::new(&seed, sid, media::Direction::HostToController).unwrap();
            let mut receiver =
                media::DatagramReceiver::new(&seed, sid, media::Direction::ControllerToHost)
                    .unwrap();
            let mut packet = [0; media::DEFAULT_MTU + 1];
            let mut responses = 0;
            loop {
                let (count, peer) = socket.recv_from(&mut packet).await.unwrap();
                match receiver.open(&packet[..count], Instant::now()).unwrap() {
                    AuthenticatedPacket::Probe {
                        kind: ProbeKind::Probe,
                        ..
                    } => {
                        let challenge = sender.seal_probe(ProbeKind::Challenge, [5; 16]).unwrap();
                        socket.send_to(&challenge.bytes, peer).await.unwrap();
                    }
                    AuthenticatedPacket::Probe {
                        kind: ProbeKind::Response,
                        token,
                    } => {
                        assert_eq!(token, [5; 16]);
                        responses += 1;
                        if responses == 2 {
                            let mut misc = Misc::new();
                            misc.set_udp_media(UdpMediaControl {
                                session_id: sid.0.to_vec().into(),
                                union: Some(udp_media_control::Union::Ready(true)),
                                ..Default::default()
                            });
                            let mut message = Message::new();
                            message.set_misc(misc);
                            host.send(&message).await.unwrap();
                            // Retain TCP until the client's acknowledgement proves it read Ready.
                            host.next().await.unwrap().unwrap();
                            return;
                        }
                    }
                    _ => {}
                }
            }
        });
        let mut client: SessionTransport = client.into();
        client.enable_client(&offer).await.unwrap();
        assert_eq!(client.protocol(), "TCP");
        timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(client.protocol(), "TCP+UDP");
        serving.await.unwrap();
    }
    #[tokio::test]
    async fn gpui_delta_gap_preserves_late_keyframes_and_reuses_payload() {
        let (client, _host, _listener) = streams().await;
        let (mut transport, _channels) =
            HybridTransport::new(client, SessionId([9; 16]), false).unwrap();
        transport.ready.store(true, Ordering::Release);
        transport.receiving.insert(
            (MediaKind::Video, 0),
            ReceiveStream {
                epoch: 1,
                needs_keyframe: true,
                ..Default::default()
            },
        );
        let frame = |frame_id, keyframe| CompletedFrame {
            metadata: FrameMetadata {
                kind: MediaKind::Video,
                display_id: 0,
                epoch: 1,
                frame_id,
                keyframe,
            },
            payload: video(keyframe, 100).write_to_bytes().unwrap().into(),
        };
        let first = frame(0, true);
        let allocation = first.payload.as_ptr();
        let delivered = transport.deliver(first).unwrap().unwrap();
        assert_eq!(delivered.as_ptr(), allocation);
        assert!(transport.deliver(frame(2, false)).unwrap().is_none());
        assert!(transport.deliver(frame(1, true)).unwrap().is_some());
        assert!(transport.deliver(frame(2, false)).unwrap().is_some());
        assert!(transport.deliver(frame(1, true)).unwrap().is_none());
        let next = frame(3, false);
        let shared = next.payload.clone();
        let mut delivered = transport.deliver(next).unwrap().unwrap();
        assert_eq!(delivered.as_ref(), shared.as_ref());
        delivered[0] ^= 1;
        assert_ne!(delivered.as_ref(), shared.as_ref());
        let mut invalid = frame(4, false);
        invalid.metadata.keyframe = true;
        assert!(transport.deliver(invalid).is_err());
    }
    #[tokio::test]
    async fn blocked_reliable_writer_does_not_stop_udp_delta_frames() {
        let (mut client, host) = backpressured_streams();
        let _udp = udp_host::bind(0).unwrap();
        let endpoint =
            std::net::SocketAddr::from(([127, 0, 0, 1], _udp.local_addr().unwrap().port()));
        let mut host = SessionTransport::accepted(host, Some(_udp.registration()));
        let offer = host.offer_host().await.unwrap().unwrap();
        let seed = client.udp_media_seed().unwrap().unwrap();
        let mut pending = crate::client::associate(&offer, seed, endpoint)
            .await
            .unwrap();
        // Retain the encrypted reliable reader without draining its bounded duplex byte stream.
        timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        host.send(&video(true, 64)).await.unwrap();
        let (start, mut begin) = mpsc::channel::<()>(1);
        let (stop, mut finish) = mpsc::channel::<()>(1);
        let serving = tokio::spawn(async move {
            let mut sending = false;
            let mut timer = interval(Duration::from_millis(1));
            loop {
                tokio::select! {
                    _ = begin.recv(), if !sending => {
                        let mut message = Message::new();
                        message.set_clipboard(Clipboard { content: vec![7; RELIABLE_BYTES / 2].into(), ..Default::default() });
                        host.send(&message).await.unwrap();
                        // This audio epoch cannot be ACKed while the earlier bulk reliable message is unread.
                        let mut misc = Misc::new(); misc.set_audio_format(AudioFormat { channels: 2, sample_rate: 48_000, ..Default::default() });
                        let mut message = Message::new(); message.set_misc(misc); host.send(&message).await.unwrap();
                        let mut message = Message::new(); message.set_audio_frame(AudioFrame { data: vec![3; 100].into(), ..Default::default() }); host.send(&message).await.unwrap();
                        sending = true;
                    }
                    _ = timer.tick(), if sending => host.send(&video(false, 10 * 1024)).await.unwrap(),
                    _ = finish.recv() => {
                        let SessionTransport::Hybrid(transport) = host else { unreachable!() };
                        let failure = transport.failure.borrow().clone();
                        let pending_bytes = transport.writer.budget.bytes.load(Ordering::Acquire);
                        eprintln!("actual bounded duplex stall capacity=8192 pending_bytes={pending_bytes} failure={failure:?}");
                        assert!(failure.is_none(), "Reliable/UDP worker failed before pending check: {failure:?}");
                        assert!(pending_bytes >= RELIABLE_BYTES / 2,
                            "the large encrypted duplex write must still be pending, pending_bytes={pending_bytes}");
                        return;
                    }
                    bytes = host.next() => { bytes.unwrap().unwrap(); }
                }
            }
        });
        // Acknowledge the format generation before exercising reliable-write backpressure.
        loop {
            let bytes = timeout(Duration::from_secs(3), client.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let message = Message::parse_from_bytes(&bytes).unwrap();
            if let Some(message::Union::Misc(misc)) = message.union {
                if let Some(misc::Union::UdpMedia(control)) = misc.union {
                    if let Some(udp_media_control::Union::Epoch(bytes)) = control.union {
                        let epoch: Epoch = bounded_json(&bytes).unwrap();
                        let ack = Epoch {
                            ack: true,
                            format: None,
                            ..epoch
                        };
                        let mut misc = Misc::new();
                        misc.set_udp_media(UdpMediaControl {
                            session_id: offer.session_id.clone(),
                            union: Some(udp_media_control::Union::Epoch(
                                serde_json::to_vec(&ack).unwrap().into(),
                            )),
                            ..Default::default()
                        });
                        let mut message = Message::new();
                        message.set_misc(misc);
                        client.send(&message).await.unwrap();
                        break;
                    }
                }
            }
        }
        start.send(()).await.unwrap();
        // Allow bulk serialization/encryption to finish its synchronous work before timing the stall.
        let mut packet = [0; media::DEFAULT_MTU + 1];
        let bytes = timeout(Duration::from_secs(5), pending.socket.recv(&mut packet))
            .await
            .unwrap()
            .unwrap();
        let AuthenticatedPacket::Fragment(first) = pending
            .receiver
            .open(&packet[..bytes], Instant::now())
            .unwrap()
        else {
            panic!("expected initial keyframe");
        };
        assert!(first.descriptor.metadata.keyframe);
        // Without a report the sender has credit for only a prefix of this delta.
        // Ask for the missing keyframe fragment before granting more credit.
        let delta = timeout(Duration::from_secs(5), async {
            loop {
                let bytes = pending.socket.recv(&mut packet).await.unwrap();
                match pending.receiver.open(&packet[..bytes], Instant::now()).unwrap() {
                    AuthenticatedPacket::Fragment(fragment) => break fragment,
                    AuthenticatedPacket::Probe { kind: ProbeKind::Challenge, token } => {
                        let response = pending.sender.seal_probe(ProbeKind::Response, token).unwrap();
                        pending.socket.send(&response.bytes).await.unwrap();
                    }
                    _ => {}
                }
            }
        }).await.unwrap();
        assert!(!delta.descriptor.metadata.keyframe);
        assert_eq!(delta.index, 0);
        let nack = KeyframeNack {
            descriptor: first.descriptor,
            missing: vec![1],
        };
        let mut misc = Misc::new();
        misc.set_udp_media(UdpMediaControl {
            session_id: offer.session_id.clone(),
            union: Some(udp_media_control::Union::Nack(
                serde_json::to_vec(&nack).unwrap().into(),
            )),
            ..Default::default()
        });
        let mut message = Message::new();
        message.set_misc(misc);
        client.send(&message).await.unwrap();
        let started = Instant::now();
        let mut count = 2;
        let mut completed_delta = false;
        let mut repaired_before_delta = false;
        let mut timer = interval(media::REPORT_INTERVAL);
        let mut packet = [0; media::DEFAULT_MTU + 1];
        timeout(Duration::from_secs(3), async {
            while started.elapsed() < Duration::from_millis(500) {
                tokio::select! {
                    bytes = pending.socket.recv(&mut packet) => {
                        let bytes = bytes.unwrap();
                        match pending.receiver.open(&packet[..bytes], Instant::now()).unwrap() {
                            AuthenticatedPacket::Fragment(fragment) => {
                                count += 1;
                                if fragment.descriptor == first.descriptor && !completed_delta {
                                    repaired_before_delta = true;
                                }
                                if fragment.descriptor == delta.descriptor && fragment.index + 1 == delta.descriptor.fragment_count {
                                    completed_delta = true;
                                }
                            }
                            AuthenticatedPacket::Probe { kind: ProbeKind::Challenge, token } => {
                                let response = pending.sender.seal_probe(ProbeKind::Response, token).unwrap();
                                pending.socket.send(&response.bytes).await.unwrap();
                            }
                            _ => {}
                        }
                    }
                    _ = timer.tick() => if let Some(report) = pending.receiver.take_report(Instant::now()) {
                        let report = pending.sender.seal_report(&report).unwrap();
                        pending.socket.send(&report.bytes).await.unwrap();
                    }
                }
            }
        }).await.unwrap();
        assert!(
            count >= 20,
            "UDP must advance despite the blocked reliable writer, received {count}"
        );
        assert!(
            repaired_before_delta && completed_delta,
            "The repair must arrive before the delta tail and the delta must finish: repair_first={repaired_before_delta}, delta_finished={completed_delta}"
        );
        eprintln!("actual bounded duplex capacity=8192 bulk_bytes={} UDP_datagrams={count} repair_preempted_delta={repaired_before_delta} unacked_audio_did_not_block_video=true elapsed_ms={}", RELIABLE_BYTES / 2, started.elapsed().as_millis());
        stop.send(()).await.unwrap();
        timeout(Duration::from_secs(3), serving)
            .await
            .unwrap()
            .unwrap();
    }
    #[test]
    fn coalesced_notice_keeps_a_newer_unsent_frame_watermark() {
        let (sender, _receiver) = mpsc::channel(CONTROL_QUEUE + 8);
        let writer = ReliableWriter {
            sender,
            priority: Arc::new(AtomicUsize::new(0)),
            budget: Arc::new(WriteBudget::default()),
            notices: Arc::new(CoalescedNotices::default()),
        };
        let metadata = FrameMetadata {
            kind: MediaKind::Video,
            display_id: 0,
            epoch: 1,
            frame_id: 3,
            keyframe: false,
        };
        writer
            .notice(SessionId([1; 16]), FrameNotice::Abandoned(metadata))
            .unwrap();
        for frame_id in [1, 2] {
            writer
                .notice(
                    SessionId([1; 16]),
                    FrameNotice::Sent(media::FrameDescriptor {
                        metadata: FrameMetadata {
                            frame_id,
                            ..metadata
                        },
                        total_size: 100,
                        fragment_count: 1,
                    }),
                )
                .unwrap();
        }
        let (_, (_, bytes)) = writer.notices.slots.lock().unwrap().pop_first().unwrap();
        let message = Message::parse_from_bytes(&bytes).unwrap();
        let Some(message::Union::Misc(misc)) = message.union else {
            panic!("expected media control");
        };
        let Some(misc::Union::UdpMedia(control)) = misc.union else {
            panic!("expected media control");
        };
        let Some(udp_media_control::Union::Notice(bytes)) = control.union else {
            panic!("expected frame notice");
        };
        assert_eq!(
            bounded_json::<FrameNotice>(&bytes).unwrap(),
            FrameNotice::Abandoned(metadata)
        );
    }
    #[tokio::test]
    async fn tcp_eof_or_revoke_unregisters_udp_before_transport_drop() {
        for mode in 0..3 {
            let (mut client, host, _listener, _udp) = media_streams(udp_host::bind).await;
            let session = _udp
                .registration()
                .register(host.udp_media_seed().unwrap().unwrap())
                .unwrap();
            let stale = session.writer();
            let mut status = session.ready();
            let mut host =
                SessionTransport::Hybrid(Box::new(HybridTransport::host(host, session).unwrap()));
            if mode == 1 {
                host.revoke_media();
            }
            if mode == 2 {
                for _ in 0..40 {
                    client.send(&close()).await.unwrap();
                }
            }
            drop(client);
            timeout(Duration::from_secs(2), async {
                while *status.borrow_and_update() != PathStatus::Closed {
                    status.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
            assert!(stale
                .send_probe([0; 16])
                .await
                .unwrap_err()
                .to_string()
                .contains("TCP owner has closed"));
            assert_eq!(host.protocol(), "TCP");
            // Keep the Hybrid object alive: native display teardown must not own the SID lifetime.
            assert!(matches!(host, SessionTransport::Hybrid(_)));
        }
    }

    #[tokio::test]
    async fn producer_clones_share_frame_sequences_and_stop_with_the_transport_owner() {
        let (_client, host) = backpressured_streams();
        let (transport, mut channels) =
            HybridTransport::new(host, SessionId([5; 16]), true).unwrap();
        transport.ready.store(true, Ordering::Release);
        let first = transport.outbound.clone();
        let second = first.clone();
        first.send_video(Arc::new(video(true, 64))).unwrap();
        second.send_video(Arc::new(video(false, 64))).unwrap();
        // Full admission must consume an ID and issue an abandoned notice. The
        // next admitted delta cannot conceal a gap in the reference chain.
        first.send_video(Arc::new(video(false, 64))).unwrap();
        let a = channels
            .frames
            .recv()
            .await
            .unwrap()
            .plan
            .descriptor()
            .metadata;
        let b = channels
            .frames
            .recv()
            .await
            .unwrap()
            .plan
            .descriptor()
            .metadata;
        assert_eq!((a.epoch, a.frame_id, b.epoch, b.frame_id), (1, 0, 1, 1));
        second.send_video(Arc::new(video(false, 64))).unwrap();
        let after_gap = channels
            .frames
            .recv()
            .await
            .unwrap()
            .plan
            .descriptor()
            .metadata;
        assert_eq!((after_gap.epoch, after_gap.frame_id), (1, 3));
        drop(transport);
        assert!(first.send_video(Arc::new(video(true, 64))).is_err());
        assert!(second.send_video(Arc::new(video(true, 64))).is_err());
    }
    #[tokio::test]
    async fn video_producer_is_released_for_pre_ready_and_full_queue_discards() {
        let (_client, host) = backpressured_streams();
        let (transport, _channels) = HybridTransport::new(host, SessionId([8; 16]), true).unwrap();
        let producer = transport.outbound.clone();
        let (released, mut notifications) = mpsc::unbounded_channel();
        producer
            .on_video_released(move |display, _| {
                released.send(display).unwrap();
            })
            .unwrap();
        producer.send_video(Arc::new(video(true, 64))).unwrap();
        assert_eq!(notifications.try_recv().unwrap(), 0);
        transport.ready.store(true, Ordering::Release);
        producer.send_video(Arc::new(video(true, 64))).unwrap();
        producer.send_video(Arc::new(video(false, 64))).unwrap();
        assert!(
            notifications.try_recv().is_err(),
            "enqueue must not release the producer before sending"
        );
        producer.send_video(Arc::new(video(false, 64))).unwrap();
        assert_eq!(notifications.try_recv().unwrap(), 0);
        drop(transport);
        assert!(producer.send_video(Arc::new(video(true, 64))).is_err());
        assert!(notifications.try_recv().is_err());
    }

    #[tokio::test]
    async fn due_udp_send_slots_do_not_wait_for_timer_ticks() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let mut context = Context::from_waker(Waker::noop());
        let mut timer = send_timer::SendTimer::new();
        for _ in 0..32 {
            let mut due = std::pin::pin!(timer.wait_until(Instant::now()));
            assert_eq!(due.as_mut().poll(&mut context), Poll::Ready(()));
        }
        let mut later = std::pin::pin!(timer.wait_until(Instant::now() + Duration::from_secs(30)));
        assert_eq!(later.as_mut().poll(&mut context), Poll::Pending);
    }

    #[tokio::test]
    async fn continuous_video_recovers_from_a_large_initial_keyframe() {
        let (client, host, _listener, _udp) = media_streams(udp_host::bind).await;
        let mut host = SessionTransport::accepted(host, Some(_udp.registration()));
        let offer = host.offer_host().await.unwrap().unwrap();
        let mut client: SessionTransport = client.into();
        client.enable_client(&offer).await.unwrap();
        timeout(Duration::from_secs(3), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        host.set_media_bitrate_limit(8_000).unwrap();
        let released = Arc::new(Notify::new());
        let notify = released.clone();
        host.media_sender()
            .unwrap()
            .on_video_released(move |_, _| notify.notify_one())
            .unwrap();
        let serving = tokio::spawn(async move {
            let mut refresh = true;
            let mut refreshes = 0;
            let mut produced = 0;
            let mut fetched = true;
            let mut tick = interval(Duration::from_nanos(8_333_333));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = released.notified() => fetched = true,
                    _ = tick.tick(), if fetched => {
                        fetched = false;
                        let key = std::mem::take(&mut refresh);
                        let bytes = if key { 300_000 } else if produced == 1 { 60_000 } else { 4_000 };
                        host.send(&video(key, bytes)).await.unwrap();
                        if !key && produced == 1 {
                            timeout(Duration::from_millis(20), released.notified()).await
                                .expect("delta production must overlap the ongoing initial UDP send");
                            fetched = true;
                        }
                        produced += 1;
                    }
                    packet = host.next() => {
                        let Some(Ok(bytes)) = packet else { break; };
                        let message = Message::parse_from_bytes(&bytes).unwrap();
                        if terminal(&message) { break; }
                        if matches!(message.union, Some(message::Union::Misc(misc))
                            if matches!(misc.union, Some(misc::Union::RefreshVideoDisplay(_)))) {
                            refresh = true;
                            refreshes += 1;
                        }
                    }
                }
            }
            (produced, refreshes)
        });
        let end = tokio::time::sleep(Duration::from_secs(3));
        tokio::pin!(end);
        let mut delivered = 0;
        loop {
            tokio::select! {
                _ = &mut end => break,
                packet = client.next() => {
                    let bytes = packet.unwrap().unwrap();
                    if matches!(Message::parse_from_bytes(&bytes).unwrap().union,
                        Some(message::Union::VideoFrame(_))) { delivered += 1; }
                }
            }
        }
        client.send(&close()).await.unwrap();
        let (produced, refreshes) = timeout(Duration::from_secs(3), serving)
            .await
            .unwrap()
            .unwrap();
        eprintln!(
            "continuous video produced={produced} delivered={delivered} refreshes={refreshes}"
        );
        assert!(produced >= 120, "producer stalled: produced={produced}");
        assert!(delivered + 3 >= produced && refreshes <= 1,
            "large initial keyframe must not trap the stream in recovery: produced={produced}, delivered={delivered}, refreshes={refreshes}");
    }
}
fn bounded_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> ResultType<T> {
    if bytes.len() > CONTROL_LIMIT {
        bail!("UDP media control exceeds its limit");
    }
    Ok(serde_json::from_slice(bytes)?)
}
fn validate_format(message: &Message, epoch: &Epoch) -> ResultType<()> {
    match (&message.union, epoch.kind) {
        (Some(message::Union::Misc(misc)), MediaKind::Audio)
            if epoch.display == 0 && matches!(misc.union, Some(misc::Union::AudioFormat(_))) =>
        {
            Ok(())
        }
        (Some(message::Union::Misc(misc)), MediaKind::Video) => match &misc.union {
            Some(misc::Union::SwitchDisplay(display))
                if u32::try_from(display.display).ok() == Some(epoch.display) =>
            {
                Ok(())
            }
            _ => bail!("UDP media epoch contains an invalid display format"),
        },
        _ => bail!("UDP media epoch contains an invalid format"),
    }
}
fn media_metadata(message: &Message) -> ResultType<Option<MediaIdentity>> {
    match &message.union {
        Some(message::Union::AudioFrame(_)) => Ok(Some((MediaKind::Audio, 0, false, None))),
        Some(message::Union::VideoFrame(video)) => {
            let display = video
                .display
                .try_into()
                .context("Invalid UDP video display")?;
            let (frames, codec) = match &video.union {
                Some(video_frame::Union::Vp8s(frames)) => (frames, 1),
                Some(video_frame::Union::Vp9s(frames)) => (frames, 2),
                Some(video_frame::Union::H264s(frames)) => (frames, 3),
                Some(video_frame::Union::H265s(frames)) => (frames, 4),
                Some(video_frame::Union::Av1s(frames)) => (frames, 5),
                _ => bail!("UDP video requires encoded frames"),
            };
            let first = frames.frames.first().context("Empty UDP video frame")?;
            Ok(Some((MediaKind::Video, display, first.key, Some(codec))))
        }
        _ => Ok(None),
    }
}
fn terminal(message: &Message) -> bool {
    matches!(&message.union, Some(message::Union::Misc(misc)) if matches!(misc.union, Some(misc::Union::CloseReason(_))))
}
fn request_refresh(writer: &ReliableWriter, display: u32) -> ResultType<()> {
    let mut misc = Misc::new();
    misc.set_refresh_video_display(display.try_into().context("Invalid UDP display")?);
    let mut message = Message::new();
    message.set_misc(misc);
    writer.enqueue(message.write_to_bytes()?, false, None)
}
fn recovery(
    writer: &ReliableWriter,
    sid: SessionId,
    outcome: ReceiveOutcome,
    completed: &mpsc::Sender<CompletedFrame>,
    gate: &Mutex<media::VideoRecovery>,
) -> ResultType<()> {
    for recovery in outcome.recovery {
        match recovery {
            Recovery::RefreshVideo(metadata) => refresh(writer, metadata, metadata.keyframe, gate)?,
            Recovery::KeyframeNack(nack) => writer.control(
                sid,
                udp_media_control::Union::Nack(serde_json::to_vec(&nack)?.into()),
                false,
            )?,
            Recovery::KeyframeComplete(complete) => writer.control(
                sid,
                udp_media_control::Union::Complete(serde_json::to_vec(&complete)?.into()),
                false,
            )?,
        }
    }
    if let Some(frame) = outcome.frame {
        match completed.try_send(frame) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(frame))
                if frame.metadata.kind == MediaKind::Video =>
            {
                refresh(writer, frame.metadata, frame.metadata.keyframe, gate)?
            }
            Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => bail!("UDP media consumer stopped"),
        }
    }
    Ok(())
}
fn refresh(
    writer: &ReliableWriter,
    metadata: FrameMetadata,
    failed_keyframe: bool,
    gate: &Mutex<media::VideoRecovery>,
) -> ResultType<()> {
    let request = gate
        .lock()
        .unwrap()
        .request(metadata, failed_keyframe, Instant::now());
    if request {
        request_refresh(writer, metadata.display_id)?;
    }
    Ok(())
}

/// Sample authenticated receive throughput while packets are arriving. Idle desktop
/// gaps are excluded; the negotiated wire cap remains an independent upper bound.
struct ReceiveRate {
    limit: u64,
    effective: u64,
    started: Instant,
    last: Instant,
    bytes: u64,
}
impl ReceiveRate {
    fn new(now: Instant) -> Self {
        Self {
            limit: 1_000_000_000,
            effective: 1_000_000_000,
            started: now,
            last: now,
            bytes: 0,
        }
    }
    fn set_limit(&mut self, bps: u64) -> Result<u64, MediaError> {
        if bps == 0 || bps > 1_000_000_000 {
            return Err(MediaError::InvalidFeedback);
        }
        self.limit = bps;
        self.effective = self.effective.min(bps);
        Ok(self.effective)
    }
    fn received(&mut self, bytes: usize, now: Instant) -> Option<u64> {
        if now.duration_since(self.last) > Duration::from_millis(250) {
            self.started = now;
            self.bytes = 0;
        }
        self.last = now;
        self.bytes += bytes as u64;
        let elapsed = now.duration_since(self.started);
        if elapsed < Duration::from_millis(250) {
            return None;
        }
        let sampled = (self.bytes as f64 * 8.0 / elapsed.as_secs_f64()) as u64;
        self.effective = sampled.max(1).min(self.limit);
        self.started = now;
        self.bytes = 0;
        Some(self.effective)
    }
}

async fn client_media(
    mut pending: PendingAssociation,
    channels: WorkerChannels,
    writer: ReliableWriter,
    ready: Arc<AtomicBool>,
    receive_limit: Arc<AtomicUsize>,
    video_recovery: Arc<Mutex<media::VideoRecovery>>,
) {
    let WorkerChannels {
        mut commands,
        completed,
        failed,
        frames,
        audio,
    } = channels;
    drop(frames);
    drop(audio);
    let mut closing = failed.subscribe();
    let result: ResultType<()> = async {
        let mut reassembler = Reassembler::default();
        let mut receive_rate = ReceiveRate::new(Instant::now());
        reassembler.set_effective_rate(receive_rate.effective)?;
        let mut timer = interval(media::REPORT_INTERVAL); timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let started = Instant::now();
        let mut next_probe = started + Duration::from_millis(250);
        let mut probe_token = [0; 16]; randombytes::randombytes_into(&mut probe_token);
        let mut packet = [0; media::DEFAULT_MTU + 1];
        loop {
            if closing.borrow().is_some() { return Ok(()); }
            tokio::select! {
                biased;
                changed = closing.changed() => { changed.context("UDP TCP owner stopped")?; if closing.borrow().is_some() { return Ok(()); } }
                command = commands.recv() => match command.context("UDP media command owner stopped")? {
                    Command::Epoch(epoch) => {
                        reassembler.set_epoch(epoch.kind, epoch.display, epoch.epoch)?;
                        if epoch.kind == MediaKind::Video {
                            video_recovery.lock().unwrap().set_epoch(epoch.display, epoch.epoch, Instant::now())?;
                        }
                    }
                    Command::Notice(notice) => {
                        let now = Instant::now();
                        match reassembler.note_notice(notice, now) {
                            Ok(outcome) => {
                                match notice {
                                    FrameNotice::Abandoned(metadata) if metadata.keyframe =>
                                        video_recovery.lock().unwrap().keyframe_abandoned(metadata),
                                    FrameNotice::Sent(descriptor) if descriptor.metadata.keyframe => {
                                        if let Some(bytes) = reassembler.received_bytes(descriptor.metadata) {
                                            let replaced = video_recovery.lock().unwrap().keyframe_progress(descriptor, bytes, now)?;
                                            if let Some(metadata) = replaced { reassembler.discard(metadata); }
                                        }
                                    }
                                    _ => {}
                                }
                                recovery(&writer, pending.session_id, outcome, &completed, &video_recovery)?;
                            }
                            Err(MediaError::WrongEpoch) => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                    Command::Limit(bps) => {
                        let effective = receive_rate.set_limit(bps)?;
                        reassembler.set_effective_rate(effective)?;
                        video_recovery.lock().unwrap().set_effective_rate(effective)?;
                    }
                    _ => bail!("Invalid controller UDP media command"),
                },
                _ = timer.tick() => {
                    let now = Instant::now();
                    let path_ready = ready.load(Ordering::Acquire);
                    if !path_ready && now.duration_since(started) >= Duration::from_millis(CONNECT_TIMEOUT) { bail!("UDP media readiness confirmation timed out"); }
                    if now >= next_probe {
                        let probe = pending.sender.seal_probe(ProbeKind::Probe, probe_token)?;
                        pending.socket.send(&probe.bytes).await.context("Validate UDP media path")?;
                        next_probe = now + if path_ready { Duration::from_secs(15) } else { Duration::from_millis(250) };
                    }
                    if let Some(report) = pending.receiver.take_report(now) {
                        let report = pending.sender.seal_report(&report)?;
                        pending.socket.send(&report.bytes).await.context("Send UDP receive report")?;
                    }
                    recovery(&writer, pending.session_id, reassembler.expire(now), &completed, &video_recovery)?;
                    let tick = video_recovery.lock().unwrap().tick(now);
                    for metadata in tick.abandon { reassembler.discard(metadata); }
                    for metadata in tick.refresh { request_refresh(&writer, metadata.display_id)?; }
                }
                count = pending.socket.recv(&mut packet) => {
                    let count = count.context("Receive UDP media")?;
                    if count > pending.mtu { continue; }
                    let Ok(packet) = pending.receiver.open(&packet[..count], Instant::now()) else { continue; };
                    match packet {
                        AuthenticatedPacket::Probe { kind: ProbeKind::Challenge, token } => {
                            let response = pending.sender.seal_probe(ProbeKind::Response, token)?;
                            pending.socket.send(&response.bytes).await.context("Reply to UDP media challenge")?;
                        }
                        AuthenticatedPacket::Fragment(fragment) if ready.load(Ordering::Acquire) => {
                            if fragment.descriptor.total_size as usize > receive_limit.load(Ordering::Acquire) { bail!("UDP frame exceeds application receive limit"); }
                            let now = Instant::now();
                            let descriptor = fragment.descriptor;
                            match reassembler.push(fragment, now) {
                                Ok(outcome) => {
                                    if let Some(bps) = receive_rate.received(count, now) {
                                        reassembler.set_effective_rate(bps)?;
                                        video_recovery.lock().unwrap().set_effective_rate(bps)?;
                                    }
                                    if descriptor.metadata.keyframe {
                                        let bytes = outcome.frame.as_ref().map(|frame| frame.payload.len())
                                            .or_else(|| reassembler.received_bytes(descriptor.metadata));
                                        if let Some(bytes) = bytes {
                                            let replaced = video_recovery.lock().unwrap().keyframe_progress(descriptor, bytes, now)?;
                                            if let Some(metadata) = replaced { reassembler.discard(metadata); }
                                        }
                                    }
                                    recovery(&writer, pending.session_id, outcome, &completed, &video_recovery)?;
                                }
                                Err(MediaError::WrongEpoch | MediaError::Capacity) => {}
                                Err(error) => return Err(error.into()),
                            }
                        }
                        _ => {}
                    }
                }

            }
        }
    }.await;
    ready.store(false, Ordering::Release);
    if let Err(error) = result {
        fail(&failed, format!("UDP receiver failed: {error:#}"));
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct Transmission {
    plan: FramePlan,
    indices: VecDeque<u16>,
    deadline: Instant,
    initial: bool,
    queued: Instant,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct VideoSendMetrics {
    started: Instant,
    sent_messages: u32,
    expired_messages: u32,
    max_send_time: Duration,
    max_keyframe_bytes: u32,
    max_keyframe_send_time: Duration,
    max_keyframe_repair_time: Duration,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl VideoSendMetrics {
    fn new(started: Instant) -> Self {
        Self {
            started,
            sent_messages: 0,
            expired_messages: 0,
            max_send_time: Duration::ZERO,
            max_keyframe_bytes: 0,
            max_keyframe_send_time: Duration::ZERO,
            max_keyframe_repair_time: Duration::ZERO,
        }
    }

    fn report(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed < Duration::from_secs(1) {
            return;
        }
        if self.sent_messages > 0
            || self.expired_messages > 0
            || !self.max_keyframe_repair_time.is_zero()
        {
            hbb_common::log::info!(target: "xscc::host_udp_metrics",
                "host_udp_video_pipeline {{\"elapsed_ms\":{:.3},\"sent_video_messages\":{},\"expired_video_messages\":{},\"max_send_ms\":{:.3},\"max_keyframe_bytes\":{},\"max_keyframe_send_ms\":{:.3},\"max_keyframe_repair_ms\":{:.3}}}",
                elapsed.as_secs_f64() * 1_000.0, self.sent_messages, self.expired_messages,
                self.max_send_time.as_secs_f64() * 1_000.0, self.max_keyframe_bytes,
                self.max_keyframe_send_time.as_secs_f64() * 1_000.0,
                self.max_keyframe_repair_time.as_secs_f64() * 1_000.0);
        }
        *self = Self::new(now);
    }
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn transmission(
    plan: FramePlan,
    initial: bool,
    indices: Vec<u16>,
    effective_bps: u64,
    queued: Instant,
) -> Transmission {
    let descriptor = plan.descriptor();
    let ttl = match (descriptor.metadata.kind, descriptor.metadata.keyframe) {
        (MediaKind::Audio, _) => media::AUDIO_TTL,
        (_, true) => media::keyframe_deadline(descriptor.total_size as usize, effective_bps),
        _ => media::DELTA_TTL,
    };
    Transmission {
        plan,
        indices: indices.into(),
        deadline: queued + ttl,
        initial,
        queued,
    }
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn frame_notice(writer: &ReliableWriter, sid: SessionId, notice: FrameNotice) -> ResultType<()> {
    writer.notice(sid, notice)
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn host_media(
    mut session: HostSession,
    channels: WorkerChannels,
    writer: ReliableWriter,
    ready: Arc<AtomicBool>,
    bitrate_budget: Arc<AtomicU64>,
    released: Arc<OnceLock<VideoReleased>>,
) {
    let WorkerChannels {
        mut frames,
        mut audio,
        mut commands,
        failed,
        ..
    } = channels;
    let mut closing = failed.subscribe();
    let result: ResultType<()> = async {
        let sid = session.session_id();
        let udp = session.writer();
        let mut path = session.ready();
        let mut pacer = Pacer::new(session.mtu(), 1_000_000_000, Instant::now())?;
        let mut cache = KeyframeCache::default();
        let mut cached = Vec::<FrameMetadata>::new();
        let mut epochs = BTreeMap::<StreamKey, (u64, bool)>::new();
        let mut current: Option<Transmission> = None;
        let mut sound: Option<Transmission> = None;
        let mut repair: Option<Transmission> = None;
        let mut retries = VecDeque::new();
        let mut probe: Option<([u8; 16], Instant)> = None;
        let started = Instant::now();
        let mut next_send = started;
        let mut send_timer = send_timer::SendTimer::new();
        let mut video_metrics = VideoSendMetrics::new(started);
        let mut timer = interval(media::REPORT_INTERVAL); timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            if closing.borrow().is_some() { return Ok(()); }
            if repair.is_none() && !retries.is_empty() {
                repair = retries.pop_front();
                next_send = Instant::now();
            }
            tokio::select! {
                biased;
                changed = closing.changed() => { changed.context("UDP TCP owner stopped")?; if closing.borrow().is_some() { return Ok(()); } }
                command = commands.recv() => match command.context("UDP media command owner stopped")? {
                    Command::Epoch(epoch) => {
                        let key = (epoch.kind, epoch.display);
                        if epoch.ack {
                            if let Some((generation, acknowledged)) = epochs.get_mut(&key) {
                                if *generation == epoch.epoch { *acknowledged = true; next_send = Instant::now(); }
                            }
                        } else {
                            epochs.insert(key, (epoch.epoch, false));
                            for metadata in cached.iter().copied().filter(|metadata| (metadata.kind, metadata.display_id) == key) { cache.complete(KeyframeComplete { metadata }); }
                            cached.retain(|metadata| (metadata.kind, metadata.display_id) != key);
                            retries.retain(|retry: &Transmission| (retry.plan.descriptor().metadata.kind, retry.plan.descriptor().metadata.display_id) != key);
                            if current.as_ref().is_some_and(|frame| (frame.plan.descriptor().metadata.kind, frame.plan.descriptor().metadata.display_id) == key) {
                                if current.as_ref().is_some_and(|frame| frame.initial && frame.plan.descriptor().metadata.keyframe) {
                                    video_released(&released, epoch.kind, epoch.display, Instant::now());
                                }
                                current = None;
                            }
                            if sound.as_ref().is_some_and(|frame| (frame.plan.descriptor().metadata.kind, frame.plan.descriptor().metadata.display_id) == key) { sound = None; }
                            if repair.as_ref().is_some_and(|frame| (frame.plan.descriptor().metadata.kind, frame.plan.descriptor().metadata.display_id) == key) { repair = None; }
                        }
                    }
                    Command::Nack(nack) => {
                        if retries.len() < 2 {
                            let now = Instant::now();
                            match cache.retransmit(&nack, now) {
                                Ok(retry) if !retry.indices.is_empty() => retries.push_back(transmission(retry.frame, false, retry.indices, pacer.effective_bitrate(), now)),
                                Ok(_) | Err(MediaError::Expired) => {}
                                Err(error) => return Err(error.into()),
                            }
                        }
                    }
                    Command::Complete(complete) => {
                        cache.complete(complete);
                        cached.retain(|metadata| *metadata != complete.metadata);
                        retries.retain(|frame| frame.plan.descriptor().metadata != complete.metadata);
                        if repair.as_ref().is_some_and(|frame| frame.plan.descriptor().metadata == complete.metadata) {
                            repair = None;
                            next_send = Instant::now();
                        }
                    }
                    Command::Limit(bps) => pacer.set_bitrate_limit(bps)?,
                    Command::Notice(_) => bail!("Invalid host UDP media command"),
                },
                changed = path.changed() => {
                    changed.context("UDP host path owner stopped")?;
                    let status = *path.borrow_and_update();
                    match status {
                        PathStatus::Ready(_) => {
                            if !ready.swap(true, Ordering::AcqRel) { writer.control(sid, udp_media_control::Union::Ready(true), true)?; }
                        }
                        PathStatus::Closed => bail!("UDP host path closed"),
                        PathStatus::Pending => {}
                    }
                }
                packet = session.next() => match packet.context("UDP host session closed")? {
                    AuthenticatedPacket::Report(report) => {
                        let now = Instant::now();
                        pacer.on_report(&report, now)?;
                        let estimate = pacer.effective_bitrate();
                        let previous = bitrate_budget.load(Ordering::Acquire);
                        // Idle restart windows and large IDRs are not evidence of a slow path.
                        let backlogged = current.as_ref().is_some_and(|frame|
                            !frame.plan.descriptor().metadata.keyframe
                                && (released.get().is_some() || !frames.is_empty()
                                    || frame.deadline.saturating_duration_since(now) <= Duration::from_millis(25)));
                        if backlogged || (previous != 0 && estimate > previous) {
                            bitrate_budget.store(estimate, Ordering::Release);
                        }
                        next_send = now;
                    }
                    AuthenticatedPacket::Probe { kind: ProbeKind::Response, token } => {
                        if let Some((expected, sent)) = probe {
                            if token == expected { pacer.on_probe_response(sent.elapsed().max(Duration::from_micros(1)), Instant::now())?; probe = None; next_send = Instant::now(); }
                        }
                    }
                    _ => {}
                },
                frame = audio.recv(), if sound.is_none() => {
                    let frame = frame.context("UDP audio producer stopped")?;
                    let indices = (0..frame.plan.fragment_count()).collect();
                    let next = transmission(frame.plan, true, indices, pacer.effective_bitrate(), frame.queued);
                    if next.deadline <= Instant::now() { continue; }
                    sound = Some(next);
                    next_send = Instant::now();
                }
                frame = frames.recv(), if current.is_none() && repair.is_none() => {
                    let frame = frame.context("UDP media producer stopped")?;
                    let queued = frame.queued;
                    let frame = frame.plan;
                    let metadata = frame.descriptor().metadata;
                    if epochs.get(&(metadata.kind, metadata.display_id)).is_none_or(|(epoch, _)| *epoch != metadata.epoch) {
                        video_released(&released, metadata.kind, metadata.display_id, queued);
                        continue;
                    }
                    if !metadata.keyframe {
                        video_released(&released, metadata.kind, metadata.display_id, queued);
                    }
                    if metadata.keyframe {
                        for evicted in cache.insert(frame.clone(), Instant::now())? {
                            cached.retain(|metadata| *metadata != evicted); frame_notice(&writer, sid, FrameNotice::Abandoned(evicted))?;
                        }
                        cached.push(metadata);
                    }
                    let indices = (0..frame.fragment_count()).collect();
                    current = Some(transmission(frame, true, indices, pacer.effective_bitrate(), queued));
                    next_send = Instant::now();
                }
                _ = timer.tick() => {
                    let now = Instant::now();
                    if !ready.load(Ordering::Acquire) && now.duration_since(started) >= Duration::from_millis(CONNECT_TIMEOUT) { bail!("UDP host path validation timed out"); }
                    for slot in [&mut current, &mut sound, &mut repair] {
                        if slot.as_ref().is_some_and(|frame| frame.deadline <= now) {
                            let expired = slot.take().expect("invariant: expired transmission exists");
                            let metadata = expired.plan.descriptor().metadata;
                            if expired.initial && metadata.kind == MediaKind::Video { video_metrics.expired_messages += 1; }
                            if expired.initial && metadata.keyframe { video_released(&released, metadata.kind, metadata.display_id, now); }
                            frame_notice(&writer, sid, FrameNotice::Abandoned(metadata))?;
                            cache.complete(KeyframeComplete { metadata }); cached.retain(|entry| *entry != metadata);
                        }
                    }
                    for expired in cache.expire(now) {
                        cached.retain(|metadata| *metadata != expired); frame_notice(&writer, sid, FrameNotice::Abandoned(expired))?;
                    }
                    if let Some(report) = session.take_report(now) { udp.send_report(&report).await?; }
                    video_metrics.report(now);
                }
                _ = send_timer.wait_until(next_send), if current.is_some() || sound.is_some() || repair.is_some() => {
                    for _ in 0..16 {
                    let now = Instant::now();
                    let sound_ready = sound.as_ref().is_some_and(|frame| {
                        let metadata = frame.plan.descriptor().metadata;
                        frame.deadline > now && epochs.get(&(metadata.kind, metadata.display_id)).is_some_and(|(epoch, ack)| *epoch == metadata.epoch && *ack)
                    });
                    let initial_keyframe = current.as_ref().is_some_and(|frame| frame.plan.descriptor().metadata.keyframe);
                    let active = if sound_ready {
                        &mut sound
                    } else if !initial_keyframe && repair.is_some() {
                        // Restore the reference before spending more of the window on dependent deltas.
                        &mut repair
                    } else {
                        &mut current
                    };
                    let Some(frame) = active.as_mut() else { next_send = now + media::REPORT_INTERVAL; break; };
                    let metadata = frame.plan.descriptor().metadata;
                    if !ready.load(Ordering::Acquire) || !epochs.get(&(metadata.kind, metadata.display_id)).is_some_and(|(epoch, ack)| *epoch == metadata.epoch && *ack) { next_send = now + media::REPORT_INTERVAL; break; }
                    let Some(&index) = frame.indices.front() else { *active = None; break; };
                    match pacer.decide(frame.plan.packet_len(index)?, frame.deadline, now, false) {
                        SendDecision::Wait(wait) => { next_send = now + wait; break; }
                        SendDecision::FeedbackRequired => {
                            if probe.is_none_or(|(_, sent)| sent.elapsed() >= Duration::from_millis(500)) {
                                let mut token = [0; 16]; randombytes::randombytes_into(&mut token);
                                udp.send_probe(token).await?; probe = Some((token, Instant::now()));
                            }
                            next_send = now + media::REPORT_INTERVAL; break;
                        }
                        SendDecision::Expired => {
                            if frame.initial && metadata.kind == MediaKind::Video { video_metrics.expired_messages += 1; }
                            if frame.initial && metadata.keyframe { video_released(&released, metadata.kind, metadata.display_id, now); }
                            frame_notice(&writer, sid, FrameNotice::Abandoned(metadata))?;
                            cache.complete(KeyframeComplete { metadata }); cached.retain(|entry| *entry != metadata); *active = None; next_send = now; break;
                        }
                        SendDecision::Send => {
                            let packet = udp.send_fragment(&frame.plan, index).await?;
                            pacer.on_sent(packet.sequence, packet.bytes.len(), Instant::now())?;
                            frame.indices.pop_front();
                            if frame.indices.is_empty() {
                                let elapsed = frame.queued.elapsed();
                                if frame.initial {
                                    if metadata.kind == MediaKind::Video {
                                        video_metrics.sent_messages += 1;
                                        video_metrics.max_send_time = video_metrics.max_send_time.max(elapsed);
                                    }
                                    if metadata.keyframe {
                                        video_metrics.max_keyframe_bytes = video_metrics.max_keyframe_bytes.max(frame.plan.descriptor().total_size);
                                        video_metrics.max_keyframe_send_time = video_metrics.max_keyframe_send_time.max(elapsed);
                                        cache.initial_sent(metadata, Instant::now(), pacer.effective_bitrate())?;
                                    }
                                    frame_notice(&writer, sid, FrameNotice::Sent(frame.plan.descriptor()))?;
                                    if metadata.keyframe { video_released(&released, metadata.kind, metadata.display_id, Instant::now()); }
                                } else {
                                    video_metrics.max_keyframe_repair_time = video_metrics.max_keyframe_repair_time.max(elapsed);
                                }
                                let next_repair = !frame.initial && !retries.is_empty();
                                *active = None;
                                if next_repair { break; }
                            }
                            next_send = Instant::now();
                        }
                    }
                    }
                    tokio::task::yield_now().await;
                }
            }
        }
    }.await;
    ready.store(false, Ordering::Release);
    if let Err(error) = result {
        fail(&failed, format!("UDP sender failed: {error:#}"));
    }
}
