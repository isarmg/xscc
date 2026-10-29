//! Bounded TCP egress for explicitly selected low-latency streaming sessions.
use crate::session_transport::SessionTransport;
use base::message_proto::{login_response, message, misc, video_frame, Message};
use hbb_common::{
    anyhow::{anyhow, Context},
    bytes::BytesMut,
    tcp::{FramedStreamReader, FramedStreamWriter},
    tokio::{
        self,
        sync::{mpsc, oneshot, watch, Notify},
        task::JoinHandle,
        time::{timeout, Duration, Instant},
    },
    ResultType,
};
use std::{
    collections::{HashMap, VecDeque},
    fmt, io,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VideoRestart {
    Refresh,
    NewSubscriber,
    CodecChanged,
    ChromaChanged,
    QualityChanged,
    FrameRateChanged,
    EncoderReconfiguration,
    DisplayChanged,
    PrivacyChanged,
    DesktopChanged,
    PortableChanged,
    ScaleChanged,
    CaptureLost,
    CaptureBackendChanged,
    Screenshot,
}

impl VideoRestart {
    #[cfg(any(windows, test))]
    pub(super) fn retry_after_platform_update(
        self,
        desktop_changed: impl FnOnce() -> bool,
    ) -> bool {
        // OpenInputDesktop/SetThreadDesktop failures leave the old thread desktop selected.
        self != Self::DesktopChanged || !desktop_changed()
    }

    pub(super) fn reuse_capture(self) -> bool {
        matches!(
            self,
            Self::Refresh
                | Self::NewSubscriber
                | Self::CodecChanged
                | Self::ChromaChanged
                | Self::QualityChanged
                | Self::FrameRateChanged
                | Self::EncoderReconfiguration
        )
    }
}

impl fmt::Display for VideoRestart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "video reconfiguration: {self:?}")
    }
}

impl std::error::Error for VideoRestart {}

const CONTROL_QUEUE: usize = 32;
const AUDIO_QUEUE: usize = 16;
const VIDEO_QUEUE: usize = 2;

struct Packet {
    payload: Payload,
    completed: Option<oneshot::Sender<std::result::Result<(), String>>>,
}

enum Payload {
    Message(Arc<Message>),
    Terminal(Arc<Message>),
    Raw(Vec<u8>),
}

const CONTROL_ADMISSION_TIMEOUT: Duration = Duration::from_millis(250);
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct MediaQueues {
    audio: VecDeque<Packet>,
    video: VecDeque<Packet>,
    frames: usize,
    metadata: usize,
    audio_next: bool,
    recovering: HashMap<usize, u64>,
    last_refresh: Option<Instant>,
    progress: u64,
    closed: bool,
}

struct MediaReceiver {
    queues: Arc<Mutex<MediaQueues>>,
    refresh: Arc<Notify>,
}
impl Drop for MediaReceiver {
    fn drop(&mut self) {
        let mut queues = self.queues.lock().unwrap();
        queues.closed = true;
        queues.audio.clear();
        queues.video.clear();
        queues.recovering.clear();
        drop(queues);
        self.refresh.notify_one();
    }
}

#[derive(Clone)]
pub(super) struct VideoWriter {
    control: mpsc::Sender<Packet>,
    media: Arc<Mutex<MediaQueues>>,
    available: Arc<Notify>,
    refresh: Arc<Notify>,
    failure: watch::Sender<Option<String>>,
    send_timeout: Arc<AtomicU64>,
}

fn encoded_video(message: &Message) -> Option<(usize, bool)> {
    let Some(message::Union::VideoFrame(frame)) = &message.union else {
        return None;
    };
    let frames = match &frame.union {
        Some(
            video_frame::Union::Vp8s(frames)
            | video_frame::Union::Vp9s(frames)
            | video_frame::Union::H264s(frames)
            | video_frame::Union::H265s(frames)
            | video_frame::Union::Av1s(frames),
        ) => frames,
        // RGB/YUV are headers for separately sent planes, not independently droppable frames.
        _ => return None,
    };
    Some((
        frame.display as usize,
        frames.frames.first().is_some_and(|frame| frame.key),
    ))
}

fn packet_video(packet: &Packet) -> Option<(usize, bool)> {
    match &packet.payload {
        Payload::Message(message) => encoded_video(message),
        _ => None,
    }
}

fn discard_media(packet: Packet) {
    if let Some(completed) = packet.completed {
        let _ = completed.send(Ok(()));
    }
}

impl VideoWriter {
    fn fail(&self, reason: &str) -> hbb_common::anyhow::Error {
        self.failure.send_replace(Some(reason.to_owned()));
        anyhow!(reason.to_owned())
    }

    fn enqueue_media(&self, packet: Packet) -> ResultType<()> {
        if self.failure.borrow().is_some() {
            return Err(anyhow!("streaming writer is unavailable"));
        }
        let mut queues = self.media.lock().unwrap();
        if queues.closed {
            return Err(anyhow!("streaming writer closed"));
        }
        if matches!(&packet.payload, Payload::Message(message) if matches!(message.union, Some(message::Union::AudioFrame(_))))
        {
            if queues.audio.len() == AUDIO_QUEUE {
                if let Some(old) = queues.audio.pop_front() {
                    discard_media(old);
                }
            }
            queues.audio.push_back(packet);
        } else if let Some((display, key)) = packet_video(&packet) {
            if queues.recovering.contains_key(&display) && !key {
                discard_media(packet);
                return Ok(());
            }
            if queues.frames == VIDEO_QUEUE {
                if !queues.recovering.contains_key(&display)
                    && queues.recovering.len() == CONTROL_QUEUE
                {
                    drop(queues);
                    return Err(self.fail("streaming recovery display limit exceeded"));
                }
                let progress = queues.progress;
                queues.recovering.entry(display).or_insert(progress);
                discard_media(packet);
                drop(queues);
                self.refresh.notify_one();
                return Ok(());
            }
            queues.recovering.remove(&display);
            queues.frames += 1;
            queues.video.push_back(packet);
        } else {
            if queues.metadata == CONTROL_QUEUE {
                drop(queues);
                return Err(self.fail("streaming display metadata queue exhausted"));
            }
            queues.metadata += 1;
            queues.video.push_back(packet);
        }
        drop(queues);
        self.available.notify_one();
        Ok(())
    }

    pub(super) fn send_video(&self, message: Arc<Message>) -> ResultType<()> {
        self.enqueue_media(Packet {
            payload: Payload::Message(message),
            completed: None,
        })
    }

    // Caller holds the producer publication lock until this FIFO migration completes.
    pub(super) fn migrate_video<T>(
        &self,
        legacy: &mut mpsc::UnboundedReceiver<(T, Arc<Message>)>,
        mut fetched: impl FnMut(T, &Message),
    ) -> ResultType<()> {
        while let Ok((instant, message)) = legacy.try_recv() {
            let result = self.send_video(message.clone());
            fetched(instant, &message);
            result?;
        }
        Ok(())
    }

    async fn refresh_requested(&self) -> Vec<usize> {
        loop {
            let notified = self.refresh.notified();
            let mut next = None;
            let mut ready = Vec::new();
            let closed;
            {
                let mut queues = self.media.lock().unwrap();
                closed = queues.closed;
                if !closed && queues.frames < VIDEO_QUEUE {
                    let progress = queues.progress;
                    let now = Instant::now();
                    let deadline = queues.last_refresh.map(|last| last + REFRESH_INTERVAL);
                    if queues.recovering.values().any(|floor| *floor != progress) {
                        if deadline.is_none_or(|deadline| now >= deadline) {
                            for (display, floor) in &mut queues.recovering {
                                if *floor != progress {
                                    *floor = progress;
                                    ready.push(*display);
                                }
                            }
                            queues.last_refresh = Some(now);
                        } else {
                            next = deadline;
                        }
                    }
                }
            }
            if closed {
                return std::future::pending().await;
            }
            if !ready.is_empty() {
                return ready;
            }
            if let Some(deadline) = next {
                tokio::select! { _ = notified => {}, _ = tokio::time::sleep_until(deadline) => {} }
            } else {
                notified.await;
            }
        }
    }

    async fn send(&self, payload: Payload, wait: bool) -> ResultType<()> {
        if self.failure.borrow().is_some() {
            return Err(anyhow!("streaming writer is unavailable"));
        }
        let (completed, received) = if wait {
            let (sender, receiver) = oneshot::channel();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let packet = Packet { payload, completed };
        if matches!(&packet.payload, Payload::Message(message) if video_message(message)
            || matches!(message.union, Some(message::Union::AudioFrame(_))))
        {
            self.enqueue_media(packet)?;
        } else {
            let terminal = matches!(&packet.payload, Payload::Terminal(_))
                || matches!(&packet.payload, Payload::Message(message) if terminal_message(message));
            if terminal {
                self.control.try_send(packet).map_err(|error| {
                    self.fail(match error {
                        mpsc::error::TrySendError::Full(_) => "streaming terminal queue exhausted",
                        mpsc::error::TrySendError::Closed(_) => "streaming writer closed",
                    })
                })?;
                self.available.notify_one();
            } else {
                // This bounds admission's pause of the connection's business/input loop.
                match timeout(CONTROL_ADMISSION_TIMEOUT, self.control.send(packet)).await {
                    Ok(Ok(())) => self.available.notify_one(),
                    Ok(Err(_)) => return Err(self.fail("streaming writer closed")),
                    Err(_) => return Err(self.fail("streaming control admission timed out")),
                }
            }
        }
        if let Some(received) = received {
            timeout(
                Duration::from_millis(self.send_timeout.load(Ordering::Acquire)),
                received,
            )
            .await
            .context("streaming writer completion timed out")?
            .context("streaming writer stopped")?
            .map_err(hbb_common::anyhow::Error::msg)?;
        }
        Ok(())
    }
}

fn video_message(message: &Message) -> bool {
    match &message.union {
        Some(message::Union::VideoFrame(_)) => true,
        Some(message::Union::Misc(misc)) => {
            matches!(misc.union, Some(misc::Union::SwitchDisplay(_)))
        }
        _ => false,
    }
}

fn terminal_message(message: &Message) -> bool {
    match &message.union {
        Some(message::Union::Misc(misc)) => {
            matches!(misc.union, Some(misc::Union::CloseReason(_)))
        }
        Some(message::Union::LoginResponse(response)) => match &response.union {
            Some(login_response::Union::Error(error)) => {
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

pub(super) struct SplitTransport {
    reader: FramedStreamReader,
    writer: VideoWriter,
    failed: watch::Receiver<Option<String>>,
    task: JoinHandle<()>,
    streaming: bool,
}

impl Drop for SplitTransport {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) enum ConnectionTransport {
    Legacy(SessionTransport),
    Split(Box<SplitTransport>),
    Vacant,
}

impl ConnectionTransport {
    pub(super) fn new(stream: SessionTransport) -> Self {
        Self::Legacy(stream)
    }

    pub(super) fn set_streaming(&mut self, enabled: bool) -> Option<VideoWriter> {
        if let Self::Split(transport) = self {
            transport.streaming = enabled;
            return enabled.then(|| transport.writer.clone());
        }
        if !enabled {
            return None;
        }
        let Self::Legacy(stream) = std::mem::replace(self, Self::Vacant) else {
            return None;
        };
        #[cfg(feature = "experimental-quic")]
        let stream = match stream {
            SessionTransport::Legacy(stream) => *stream,
            stream @ SessionTransport::Quic(_) => {
                *self = Self::Legacy(stream);
                return None;
            }
        };
        let stream = match stream {
            hbb_common::Stream::Tcp(stream) => stream,
            stream @ hbb_common::Stream::WebSocket(_) => {
                // WebSocket framing must remain intact; it retains the existing flow control.
                *self = Self::Legacy(stream.into());
                return None;
            }
        };
        let (reader, writer) = stream.into_split();
        let transport = split_transport(reader, writer);
        let handle = transport.writer.clone();
        *self = Self::Split(Box::new(transport));
        Some(handle)
    }

    pub(super) fn set_send_timeout(&mut self, milliseconds: u64) {
        match self {
            Self::Legacy(stream) => stream.set_send_timeout(milliseconds),
            Self::Split(transport) => transport
                .writer
                .send_timeout
                .store(milliseconds, Ordering::Release),
            Self::Vacant => {}
        }
    }

    pub(super) fn set_max_packet_length(&mut self, limit: usize) {
        match self {
            Self::Legacy(stream) => stream.set_max_packet_length(limit),
            Self::Split(transport) => transport.reader.set_max_packet_length(limit),
            Self::Vacant => {}
        }
    }

    pub(super) async fn send(&mut self, message: &Message) -> ResultType<()> {
        match self {
            Self::Legacy(stream) => stream.send(message).await,
            Self::Split(transport) => transport
                .writer
                .send(
                    Payload::Message(Arc::new(message.clone())),
                    !transport.streaming
                        || terminal_message(message)
                        || matches!(&message.union, Some(message::Union::LoginResponse(response))
                        if matches!(response.union, Some(login_response::Union::Error(_)))),
                )
                .await,
            Self::Vacant => Err(anyhow!("connection transport unavailable")),
        }
    }

    pub(super) async fn send_terminal(&mut self, message: &Message) -> ResultType<()> {
        match self {
            Self::Legacy(stream) => stream.send(message).await,
            Self::Split(transport) => {
                transport
                    .writer
                    .send(Payload::Terminal(Arc::new(message.clone())), true)
                    .await
            }
            Self::Vacant => Err(anyhow!("connection transport unavailable")),
        }
    }

    pub(super) async fn send_raw(&mut self, bytes: Vec<u8>) -> ResultType<()> {
        match self {
            Self::Legacy(stream) => stream.send_raw(bytes).await,
            Self::Split(transport) => transport.writer.send(Payload::Raw(bytes), true).await,
            Self::Vacant => Err(anyhow!("connection transport unavailable")),
        }
    }

    pub(super) fn video_refresh_requested(
        &self,
    ) -> impl std::future::Future<Output = Vec<usize>> + Send + 'static {
        let writer = match self {
            Self::Split(transport) => Some(transport.writer.clone()),
            _ => None,
        };
        async move {
            match writer {
                Some(writer) => writer.refresh_requested().await,
                None => std::future::pending().await,
            }
        }
    }

    pub(super) async fn next(&mut self) -> Option<io::Result<BytesMut>> {
        match self {
            Self::Legacy(stream) => stream.next().await,
            Self::Split(transport) => {
                if let Some(reason) = transport.failed.borrow().as_ref() {
                    return Some(Err(io::Error::other(reason.clone())));
                }
                tokio::select! {
                    biased;
                    _ = transport.failed.changed() => {
                        Some(Err(io::Error::other(transport.failed.borrow().as_deref().unwrap_or("streaming writer stopped").to_owned())))
                    }
                    message = transport.reader.next() => message,
                }
            }
            Self::Vacant => None,
        }
    }
}

fn split_transport(reader: FramedStreamReader, mut writer: FramedStreamWriter) -> SplitTransport {
    let (control_tx, mut control) = mpsc::channel::<Packet>(CONTROL_QUEUE);
    let media = Arc::new(Mutex::new(MediaQueues::default()));
    let available = Arc::new(Notify::new());
    let refresh = Arc::new(Notify::new());
    let media_receiver = MediaReceiver {
        queues: media.clone(),
        refresh: refresh.clone(),
    };
    let writer_available = available.clone();
    let (failure, failed) = watch::channel(None);
    let writer_failure = failure.clone();
    let send_timeout = Arc::new(AtomicU64::new(12_000));
    let writer_timeout = send_timeout.clone();
    let task = tokio::spawn(async move {
        let receiver = media_receiver;
        loop {
            let packet = loop {
                let notified = writer_available.notified();
                let packet = {
                    let mut queues = receiver.queues.lock().unwrap();
                    if let Ok(packet) = control.try_recv() {
                        Some(packet)
                    } else if !queues.audio.is_empty()
                        && (queues.video.is_empty() || queues.audio_next)
                    {
                        queues.audio_next = false;
                        queues.audio.pop_front()
                    } else if let Some(packet) = queues.video.pop_front() {
                        queues.audio_next = true;
                        if packet_video(&packet).is_some() {
                            queues.frames -= 1;
                        } else {
                            queues.metadata -= 1;
                        }
                        Some(packet)
                    } else {
                        None
                    }
                };
                if let Some(packet) = packet {
                    break packet;
                }
                notified.await;
            };
            writer.set_send_timeout(writer_timeout.load(Ordering::Acquire));
            let terminal = matches!(&packet.payload, Payload::Terminal(_))
                || matches!(&packet.payload, Payload::Message(message) if terminal_message(message));
            let mut result = match packet.payload {
                Payload::Message(message) | Payload::Terminal(message) => {
                    writer.send(message.as_ref()).await
                }
                Payload::Raw(bytes) => writer.send_raw(bytes).await,
            }
            .map_err(|error| error.to_string());
            if terminal && result.is_ok() {
                result = writer.close().await.map_err(|error| error.to_string());
            }
            if result.is_ok() {
                let recovering = {
                    let mut queues = receiver.queues.lock().unwrap();
                    queues.progress = queues.progress.wrapping_add(1);
                    !queues.recovering.is_empty()
                };
                if recovering {
                    receiver.refresh.notify_one();
                }
            }
            if let Some(completed) = packet.completed {
                let _ = completed.send(result.clone());
            }
            if let Err(reason) = result {
                writer_failure.send_replace(Some(reason));
                break;
            }
            if terminal {
                break;
            }
        }
    });
    SplitTransport {
        reader,
        writer: VideoWriter {
            control: control_tx,
            media,
            available,
            refresh,
            failure,
            send_timeout,
        },
        failed,
        task,
        streaming: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base::message_proto::{Misc, SwitchDisplay, VideoFrame};

    #[test]
    fn only_encoder_reconfiguration_reuses_capture() {
        assert!(VideoRestart::NewSubscriber.reuse_capture());
        assert!(VideoRestart::FrameRateChanged.reuse_capture());
        for reason in [
            VideoRestart::DisplayChanged,
            VideoRestart::PrivacyChanged,
            VideoRestart::CaptureLost,
        ] {
            assert!(!reason.reuse_capture());
        }
    }

    #[test]
    fn an_unavailable_input_desktop_keeps_using_error_backoff_until_selection_succeeds() {
        // The Windows API may repeatedly deny opening the secure desktop without changing state.
        for _ in 0..3 {
            assert!(!VideoRestart::DesktopChanged.retry_after_platform_update(|| true));
        }
        assert!(VideoRestart::DesktopChanged.retry_after_platform_update(|| false));
        assert!(VideoRestart::Refresh.retry_after_platform_update(|| panic!("unrelated restart")));
    }

    async fn tcp_pair(small_buffers: bool) -> (hbb_common::Stream, hbb_common::Stream) {
        let listener = tokio::net::TcpSocket::new_v4().unwrap();
        if small_buffers {
            listener.set_recv_buffer_size(1024).unwrap();
        }
        listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = listener.listen(1).unwrap();
        let address = listener.local_addr().unwrap();
        let outgoing = tokio::net::TcpSocket::new_v4().unwrap();
        if small_buffers {
            outgoing.set_send_buffer_size(1024).unwrap();
        }
        let (local, remote) = tokio::join!(outgoing.connect(address), listener.accept());
        let local = local.unwrap();
        let (remote, peer) = remote.unwrap();
        local.set_nodelay(true).unwrap();
        remote.set_nodelay(true).unwrap();
        (
            hbb_common::Stream::from(local, address),
            hbb_common::Stream::from(remote, peer),
        )
    }

    fn encoded_frame(index: i64, size: usize) -> Arc<Message> {
        use base::message_proto::{EncodedVideoFrame, EncodedVideoFrames};
        let mut frame = VideoFrame::new();
        frame.set_h264s(EncodedVideoFrames {
            frames: vec![EncodedVideoFrame {
                data: vec![7; size].into(),
                key: index == 0,
                pts: index,
                ..Default::default()
            }],
            ..Default::default()
        });
        let mut message = Message::new();
        message.set_video_frame(frame);
        Arc::new(message)
    }

    async fn read_message(peer: &mut hbb_common::Stream) -> Message {
        use hbb_common::protobuf::Message as _;
        let bytes = timeout(Duration::from_secs(3), peer.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        Message::parse_from_bytes(&bytes).unwrap()
    }

    async fn assert_writer_pending(task: &mut JoinHandle<ResultType<()>>, phase: &str) {
        if !task.is_finished() {
            return;
        }
        match task.await {
            Ok(Ok(())) => panic!("slow writer unexpectedly completed {phase}"),
            Ok(Err(error)) => panic!("slow writer send failed {phase}: {error:#}"),
            Err(error) => panic!("slow writer task failed {phase}: {error}"),
        }
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn two_tcp_viewers_isolate_a_stalled_writer_and_preserve_the_fast_reference_chain() {
        let (slow_local, mut slow_peer) = tcp_pair(true).await;
        let (fast_local, mut fast_peer) = tcp_pair(false).await;
        let mut slow = ConnectionTransport::new(slow_local.into());
        let mut fast = ConnectionTransport::new(fast_local.into());
        let slow_writer = slow.set_streaming(true).unwrap();
        let fast_writer = fast.set_streaming(true).unwrap();
        let blocked_writer = slow_writer.clone();
        let completed = Arc::new(AtomicU64::new(0));
        let writer_completed = completed.clone();
        let mut blocked: JoinHandle<ResultType<()>> = tokio::spawn(async move {
            // Reuse one allocation; kernel TCP buffers differ substantially across platforms.
            let frame = encoded_frame(0, 65536);
            for _ in 0..1024 {
                blocked_writer
                    .send(Payload::Message(frame.clone()), true)
                    .await?;
                writer_completed.fetch_add(1, Ordering::Release);
            }
            Err(anyhow!(
                "slow TCP writer accepted the entire 64-MiB safety limit"
            ))
        });
        let observation_started = std::time::Instant::now();
        let mut unchanged_since = observation_started;
        let mut last_completed = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert_writer_pending(&mut blocked, "before backpressure").await;
            let count = completed.load(Ordering::Acquire);
            if count != last_completed {
                last_completed = count;
                unchanged_since = std::time::Instant::now();
            }
            if unchanged_since.elapsed() >= Duration::from_millis(50) {
                break;
            }
            assert!(
                observation_started.elapsed() < Duration::from_secs(3),
                "no sustained TCP backpressure; {count} complete 64-KiB writes"
            );
        }
        println!("slow TCP backpressure observed after {last_completed} complete 64-KiB writes");
        // The slow peer does not consume video, but its input/control direction still works.
        slow_peer.send_raw(vec![55]).await.unwrap();
        assert_eq!(
            &timeout(Duration::from_secs(1), slow.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()[..],
            &[55]
        );
        let started = std::time::Instant::now();
        for index in 0..120 {
            fast_writer
                .send(Payload::Message(encoded_frame(index, 32768)), true)
                .await
                .unwrap();
            let received = read_message(&mut fast_peer).await;
            let Some(message::Union::VideoFrame(frame)) = received.union else {
                panic!("missing video")
            };
            assert_eq!(frame.h264s().frames[0].pts, index);
            assert_eq!(frame.h264s().frames[0].key, index == 0);
        }
        println!(
            "healthy TCP viewer: 120 ordered 32-KiB frames in {:?}, stalled peer remained blocked",
            started.elapsed()
        );
        assert_writer_pending(&mut blocked, "during healthy viewer transfer").await;
        for index in 1..=VIDEO_QUEUE {
            slow_writer
                .send_video(encoded_frame(index as i64, 32))
                .unwrap();
        }
        slow_writer.send_video(encoded_frame(3, 32)).unwrap();
        assert!(timeout(Duration::from_millis(50), slow.next())
            .await
            .is_err());
        assert!(fast_writer.failure.borrow().is_none());
        drop(slow);
        assert!(timeout(Duration::from_secs(1), blocked)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(
            slow_writer.control.is_closed(),
            "drop cancels the blocked write and queued buffers"
        );
        fast_writer
            .send(Payload::Raw(vec![56]), true)
            .await
            .unwrap();
        assert_eq!(&fast_peer.next().await.unwrap().unwrap()[..], &[56]);
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn enabling_streaming_migrates_legacy_a_before_direct_b_on_the_wire() {
        let (local, mut peer) = tcp_pair(false).await;
        let mut transport = ConnectionTransport::new(local.into());
        let (old_sender, mut old_receiver) = mpsc::unbounded_channel();
        old_sender.send((0, encoded_frame(0, 32))).unwrap();
        let writer = transport.set_streaming(true).unwrap();
        let mut acknowledged = Vec::new();
        writer
            .migrate_video(&mut old_receiver, |index, _| acknowledged.push(index))
            .unwrap();
        writer.send_video(encoded_frame(1, 32)).unwrap();
        for index in 0..2 {
            let received = read_message(&mut peer).await;
            let Some(message::Union::VideoFrame(frame)) = received.union else {
                panic!("missing video")
            };
            assert_eq!(frame.h264s().frames[0].pts, index);
        }
        assert_eq!(acknowledged, [0]);
        assert!(transport.set_streaming(false).is_none());
        transport.send(&encoded_frame(2, 32)).await.unwrap();
        let received = read_message(&mut peer).await;
        let Some(message::Union::VideoFrame(frame)) = received.union else {
            panic!("missing video")
        };
        assert_eq!(frame.h264s().frames[0].pts, 2);
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn migration_burst_recovers_at_a_new_first_key_and_preserves_switch_fifo() {
        use base::message_proto::{EncodedVideoFrame, EncodedVideoFrames};
        let (local, mut peer) = tcp_pair(false).await;
        let mut transport = ConnectionTransport::new(local.into());
        let writer = transport.set_streaming(true).unwrap();
        let (old_sender, mut old_receiver) = mpsc::unbounded_channel();
        for index in 0..3 {
            old_sender.send((index, encoded_frame(index, 32))).unwrap();
        }
        let mut misc = Misc::new();
        misc.set_switch_display(SwitchDisplay::new());
        let mut switch = Message::new();
        switch.set_misc(misc);
        old_sender.send((3, Arc::new(switch.clone()))).unwrap();
        let mut fetched = Vec::new();
        writer
            .migrate_video(&mut old_receiver, |index, _| fetched.push(index))
            .unwrap();
        assert_eq!(fetched, [0, 1, 2, 3]);
        writer.send_video(encoded_frame(3, 32)).unwrap();
        for index in 0..2 {
            assert_eq!(
                read_message(&mut peer).await.video_frame().h264s().frames[0].pts,
                index
            );
        }
        assert_eq!(read_message(&mut peer).await, switch);
        assert_eq!(
            timeout(Duration::from_secs(1), transport.video_refresh_requested())
                .await
                .unwrap(),
            [0]
        );
        let mut mixed = encoded_frame(3, 32).as_ref().clone();
        mixed
            .mut_video_frame()
            .mut_h264s()
            .frames
            .push(EncodedVideoFrame {
                key: true,
                ..Default::default()
            });
        writer.send_video(Arc::new(mixed)).unwrap();
        let key = |pts| {
            let mut frame = VideoFrame::new();
            frame.set_h264s(EncodedVideoFrames {
                frames: vec![EncodedVideoFrame {
                    key: true,
                    pts,
                    data: vec![7; 32].into(),
                    ..Default::default()
                }],
                ..Default::default()
            });
            let mut msg = Message::new();
            msg.set_video_frame(frame);
            Arc::new(msg)
        };
        writer.send_video(key(4)).unwrap();
        writer.send_video(encoded_frame(5, 32)).unwrap();
        for index in [4, 5] {
            assert_eq!(
                read_message(&mut peer).await.video_frame().h264s().frames[0].pts,
                index
            );
        }
        assert!(writer.failure.borrow().is_none());
        // A new gap immediately after recovery must retain the previous refresh rate limit.
        for index in 6..9 {
            writer.send_video(encoded_frame(index, 32)).unwrap();
        }
        for index in 6..8 {
            assert_eq!(
                read_message(&mut peer).await.video_frame().h264s().frames[0].pts,
                index
            );
        }
        assert!(timeout(
            Duration::from_millis(50),
            transport.video_refresh_requested()
        )
        .await
        .is_err());
        assert_eq!(
            timeout(Duration::from_secs(2), transport.video_refresh_requested())
                .await
                .unwrap(),
            [0]
        );
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn transient_media_and_control_bursts_keep_the_transport_live() {
        use base::message_proto::AudioFrame;
        let (local, mut peer) = tcp_pair(false).await;
        let mut transport = ConnectionTransport::new(local.into());
        let writer = transport.set_streaming(true).unwrap();
        // No yield between these calls: the old implementation disconnects at frame three.
        for index in 0..4 {
            assert!(writer.send_video(encoded_frame(index, 32)).is_ok());
        }
        for index in 0..AUDIO_QUEUE + 3 {
            let mut msg = Message::new();
            msg.set_audio_frame(AudioFrame {
                data: vec![index as u8].into(),
                ..Default::default()
            });
            writer
                .send(Payload::Message(Arc::new(msg)), false)
                .await
                .unwrap();
        }
        let mut video_pts = Vec::new();
        let mut audio_bytes = Vec::new();
        let mut second_video_at = None;
        for position in 0..AUDIO_QUEUE + VIDEO_QUEUE {
            match read_message(&mut peer).await.union {
                Some(message::Union::VideoFrame(frame)) => {
                    video_pts.push(frame.h264s().frames[0].pts);
                    if video_pts.len() == 2 {
                        second_video_at = Some(position);
                    }
                }
                Some(message::Union::AudioFrame(frame)) => audio_bytes.push(frame.data[0]),
                _ => panic!("unexpected burst packet"),
            }
        }
        assert_eq!(video_pts, [0, 1]);
        assert_eq!(
            audio_bytes,
            (3..AUDIO_QUEUE + 3)
                .map(|index| index as u8)
                .collect::<Vec<_>>()
        );
        assert!(
            second_video_at.unwrap() <= 2,
            "queued audio must not starve video"
        );
        for index in 0..CONTROL_QUEUE + 3 {
            writer
                .send(Payload::Raw(vec![index as u8]), false)
                .await
                .unwrap();
        }
        for index in 0..CONTROL_QUEUE + 3 {
            assert_eq!(&peer.next().await.unwrap().unwrap()[..], &[index as u8]);
        }
        peer.send_raw(vec![99]).await.unwrap();
        assert_eq!(&transport.next().await.unwrap().unwrap()[..], &[99]);
        assert!(writer.failure.borrow().is_none());
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn control_admission_has_a_short_bound_without_blocking_the_split_reader() {
        let (local, remote) = tokio::io::duplex(8);
        let address = "127.0.0.1:21118".parse().unwrap();
        let mut transport = ConnectionTransport::new(
            hbb_common::Stream::Tcp(hbb_common::tcp::FramedStream::from(local, address)).into(),
        );
        let writer = transport.set_streaming(true).unwrap();
        let mut peer =
            hbb_common::Stream::Tcp(hbb_common::tcp::FramedStream::from(remote, address));
        writer
            .send(Payload::Raw(vec![1; 65536]), false)
            .await
            .unwrap();
        tokio::task::yield_now().await;
        for _ in 0..CONTROL_QUEUE {
            writer.send(Payload::Raw(vec![2]), false).await.unwrap();
        }
        let started = Instant::now();
        let pending = tokio::spawn(async move { writer.send(Payload::Raw(vec![3]), false).await });
        peer.send_raw(vec![4]).await.unwrap();
        assert_eq!(
            &timeout(Duration::from_millis(100), transport.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()[..],
            &[4]
        );
        assert!(timeout(Duration::from_secs(1), pending)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(
            started.elapsed() >= CONTROL_ADMISSION_TIMEOUT
                && started.elapsed() < Duration::from_secs(1)
        );
        assert!(transport.next().await.unwrap().is_err());
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn retryable_login_error_keeps_writer_alive_and_privacy_terminal_flushes_before_drop() {
        use base::message_proto::{back_notification, BackNotification, LoginResponse};
        let (local, mut peer) = tcp_pair(false).await;
        let mut transport = ConnectionTransport::new(local.into());
        let writer = transport.set_streaming(true).unwrap();
        let mut response = LoginResponse::new();
        response.set_error(base::session_protocol::LOGIN_MSG_PASSWORD_WRONG.to_owned());
        let mut retry = Message::new();
        retry.set_login_response(response);
        transport.send(&retry).await.unwrap();
        assert_eq!(read_message(&mut peer).await, retry);
        transport.send(&encoded_frame(0, 32)).await.unwrap();
        assert!(matches!(
            read_message(&mut peer).await.union,
            Some(message::Union::VideoFrame(_))
        ));
        let mut notice = BackNotification::new();
        notice.set_privacy_mode_state(back_notification::PrivacyModeState::PrvOnByOther);
        let mut misc = Misc::new();
        misc.set_back_notification(notice);
        let mut terminal = Message::new();
        terminal.set_misc(misc);
        writer.send_video(encoded_frame(1, 32)).unwrap();
        writer.send_video(encoded_frame(2, 32)).unwrap();
        transport.send_terminal(&terminal).await.unwrap();
        drop(transport);
        assert_eq!(read_message(&mut peer).await, terminal);
        assert!(timeout(Duration::from_secs(1), peer.next())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn raw_send_awaits_completion_before_the_transport_is_dropped() {
        let (local, mut peer) = tcp_pair(false).await;
        let mut transport = ConnectionTransport::new(local.into());
        let writer = transport.set_streaming(true).unwrap();
        transport.send_raw(vec![42; 65536]).await.unwrap();
        drop(transport);
        let received = timeout(Duration::from_secs(1), peer.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(&received[..], &[42; 65536]);
        assert!(
            timeout(Duration::from_millis(50), writer.refresh_requested())
                .await
                .is_err(),
            "a closed writer must not repeatedly return an empty refresh"
        );
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn input_and_recovery_follow_write_progress_even_while_the_next_frame_is_blocked() {
        let (local, remote) = tokio::io::duplex(8);
        let address = "127.0.0.1:21118".parse().unwrap();
        let mut transport = ConnectionTransport::new(
            hbb_common::Stream::Tcp(hbb_common::tcp::FramedStream::from(local, address)).into(),
        );
        let writer = transport.set_streaming(true).unwrap();
        let mut peer =
            hbb_common::Stream::Tcp(hbb_common::tcp::FramedStream::from(remote, address));
        for index in 0..3 {
            writer.send_video(encoded_frame(index, 65536)).unwrap();
        }
        tokio::task::yield_now().await;
        peer.send_raw(vec![9]).await.unwrap();
        assert_eq!(
            &timeout(Duration::from_secs(1), transport.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()[..],
            &[9]
        );
        assert_eq!(
            read_message(&mut peer).await.video_frame().h264s().frames[0].pts,
            0
        );
        tokio::task::yield_now().await;
        assert_eq!(
            timeout(Duration::from_secs(1), transport.video_refresh_requested())
                .await
                .unwrap(),
            [0]
        );
        assert!(
            timeout(
                Duration::from_millis(1100),
                transport.video_refresh_requested()
            )
            .await
            .is_err(),
            "a fully stalled socket must not request another global refresh on a timer"
        );
        assert!(transport.set_streaming(false).is_none());
        timeout(
            Duration::from_millis(100),
            transport.send(&encoded_frame(3, 32)),
        )
        .await
        .unwrap()
        .unwrap();
        let mut key = encoded_frame(4, 32).as_ref().clone();
        key.mut_video_frame().mut_h264s().frames[0].key = true;
        let completed =
            tokio::spawn(async move { writer.send(Payload::Message(Arc::new(key)), true).await });
        assert_eq!(
            read_message(&mut peer).await.video_frame().h264s().frames[0].pts,
            1
        );
        assert_eq!(
            read_message(&mut peer).await.video_frame().h264s().frames[0].pts,
            4,
            "completing the old in-flight key must not reopen the broken reference chain"
        );
        completed.await.unwrap().unwrap();
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn terminal_does_not_wait_for_admission_to_a_saturated_control_queue() {
        let (local, _remote) = tokio::io::duplex(8);
        let address = "127.0.0.1:21118".parse().unwrap();
        let mut transport = ConnectionTransport::new(
            hbb_common::Stream::Tcp(hbb_common::tcp::FramedStream::from(local, address)).into(),
        );
        let writer = transport.set_streaming(true).unwrap();
        writer
            .send(Payload::Raw(vec![1; 65536]), false)
            .await
            .unwrap();
        tokio::task::yield_now().await;
        for _ in 0..CONTROL_QUEUE {
            writer.send(Payload::Raw(vec![2]), false).await.unwrap();
        }
        let mut misc = Misc::new();
        misc.set_close_reason("revoked".to_owned());
        let mut terminal = Message::new();
        terminal.set_misc(misc);
        let started = Instant::now();
        assert!(transport.send_terminal(&terminal).await.is_err());
        assert!(started.elapsed() < CONTROL_ADMISSION_TIMEOUT);
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn control_has_priority_but_display_changes_stay_after_their_video() {
        use hbb_common::protobuf::Message as _;
        let (local, remote) = tokio::io::duplex(4096);
        let address = "127.0.0.1:21118".parse().unwrap();
        let mut transport = ConnectionTransport::new(
            hbb_common::Stream::Tcp(hbb_common::tcp::FramedStream::from(local, address)).into(),
        );
        let writer = transport.set_streaming(true).unwrap();
        let mut peer =
            hbb_common::Stream::Tcp(hbb_common::tcp::FramedStream::from(remote, address));
        let mut frame = Message::new();
        frame.set_video_frame(VideoFrame {
            display: 7,
            ..Default::default()
        });
        writer.send_video(Arc::new(frame)).unwrap();
        let mut misc = Misc::new();
        misc.set_switch_display(SwitchDisplay::new());
        let mut display = Message::new();
        display.set_misc(misc);
        writer.send_video(Arc::new(display)).unwrap();
        writer.send(Payload::Raw(vec![9]), false).await.unwrap();
        assert_eq!(&peer.next().await.unwrap().unwrap()[..], &[9]);
        let first = Message::parse_from_bytes(&peer.next().await.unwrap().unwrap()).unwrap();
        let second = Message::parse_from_bytes(&peer.next().await.unwrap().unwrap()).unwrap();
        assert!(matches!(first.union, Some(message::Union::VideoFrame(_))));
        assert!(matches!(second.union, Some(message::Union::Misc(_))));
    }
}
