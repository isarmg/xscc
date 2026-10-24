//! Bounded TCP egress for explicitly selected low-latency streaming sessions.
use crate::session_transport::SessionTransport;
use base::message_proto::{login_response, message, misc, Message};
use hbb_common::{
    anyhow::{anyhow, Context},
    bytes::BytesMut,
    tcp::{FramedStreamReader, FramedStreamWriter},
    tokio::{
        self,
        sync::{mpsc, oneshot, watch},
        task::JoinHandle,
        time::{timeout, Duration},
    },
    ResultType,
};
use std::{
    fmt, io,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
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

#[derive(Clone)]
pub(super) struct VideoWriter {
    control: mpsc::Sender<Packet>,
    audio: mpsc::Sender<Packet>,
    video: mpsc::Sender<Packet>,
    failure: watch::Sender<Option<String>>,
    send_timeout: Arc<AtomicU64>,
}

impl VideoWriter {
    fn enqueue(&self, packet: Packet) -> ResultType<()> {
        if self.failure.borrow().is_some() {
            return Err(anyhow!("streaming writer is unavailable"));
        }
        let queue = match &packet.payload {
            Payload::Message(message) if video_message(message) => &self.video,
            Payload::Message(message)
                if matches!(message.union, Some(message::Union::AudioFrame(_))) =>
            {
                &self.audio
            }
            _ => &self.control,
        };
        queue.try_send(packet).map_err(|error| {
            let reason = match error {
                mpsc::error::TrySendError::Full(_) => "streaming receiver queue exhausted",
                mpsc::error::TrySendError::Closed(_) => "streaming writer closed",
            };
            self.failure.send_replace(Some(reason.to_owned()));
            anyhow!(reason)
        })
    }

    pub(super) fn send_video(&self, message: Arc<Message>) -> ResultType<()> {
        self.enqueue(Packet {
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
            self.send_video(message.clone())?;
            fetched(instant, &message);
        }
        Ok(())
    }

    async fn send(&self, payload: Payload, wait: bool) -> ResultType<()> {
        let (completed, received) = if wait {
            let (sender, receiver) = oneshot::channel();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        self.enqueue(Packet { payload, completed })?;
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
    let (audio_tx, mut audio) = mpsc::channel::<Packet>(AUDIO_QUEUE);
    let (video_tx, mut video) = mpsc::channel::<Packet>(VIDEO_QUEUE);
    let (failure, failed) = watch::channel(None);
    let writer_failure = failure.clone();
    let send_timeout = Arc::new(AtomicU64::new(12_000));
    let writer_timeout = send_timeout.clone();
    let task = tokio::spawn(async move {
        loop {
            let packet = tokio::select! {
                biased;
                Some(packet) = control.recv() => packet,
                Some(packet) = audio.recv() => packet,
                Some(packet) = video.recv() => packet,
                else => break,
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
            audio: audio_tx,
            video: video_tx,
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
        assert!(slow_writer.send_video(encoded_frame(3, 32)).is_err());
        assert!(slow.next().await.unwrap().is_err());
        assert!(fast_writer.failure.borrow().is_none());
        drop(slow);
        assert!(timeout(Duration::from_secs(1), blocked)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(
            slow_writer.video.is_closed(),
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
    async fn migration_overflow_fails_the_session_instead_of_skipping_a_reference_frame() {
        let (local, _peer) = tcp_pair(false).await;
        let mut transport = ConnectionTransport::new(local.into());
        let writer = transport.set_streaming(true).unwrap();
        let (old_sender, mut old_receiver) = mpsc::unbounded_channel();
        for index in 0..3 {
            old_sender.send((index, encoded_frame(index, 32))).unwrap();
        }
        assert!(writer.migrate_video(&mut old_receiver, |_, _| {}).is_err());
        assert!(transport.next().await.unwrap().is_err());
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn retryable_login_error_keeps_writer_alive_and_privacy_terminal_flushes_before_drop() {
        use base::message_proto::{back_notification, BackNotification, LoginResponse};
        let (local, mut peer) = tcp_pair(false).await;
        let mut transport = ConnectionTransport::new(local.into());
        transport.set_streaming(true).unwrap();
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
        transport.set_streaming(true).unwrap();
        transport.send_raw(vec![42; 65536]).await.unwrap();
        drop(transport);
        let received = timeout(Duration::from_secs(1), peer.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(&received[..], &[42; 65536]);
    }

    #[tokio::test(crate = "hbb_common::tokio")]
    async fn control_reads_continue_while_a_large_video_write_is_blocked() {
        use base::message_proto::{EncodedVideoFrame, EncodedVideoFrames};
        let (local, remote) = tokio::io::duplex(8);
        let address = "127.0.0.1:21118".parse().unwrap();
        let stream = hbb_common::Stream::Tcp(hbb_common::tcp::FramedStream::from(local, address));
        let mut transport = ConnectionTransport::new(stream.into());
        let writer = transport.set_streaming(true).unwrap();
        let mut peer =
            hbb_common::Stream::Tcp(hbb_common::tcp::FramedStream::from(remote, address));
        let mut frame = VideoFrame::new();
        frame.set_h264s(EncodedVideoFrames {
            frames: vec![EncodedVideoFrame {
                data: vec![7; 65536].into(),
                key: true,
                ..Default::default()
            }],
            ..Default::default()
        });
        let mut message = Message::new();
        message.set_video_frame(frame);
        let blocked =
            tokio::spawn(
                async move { writer.send(Payload::Message(Arc::new(message)), true).await },
            );
        tokio::task::yield_now().await;
        peer.send_raw(vec![9]).await.unwrap();
        let received = timeout(Duration::from_secs(1), transport.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(&received[..], &[9]);
        assert!(
            !blocked.is_finished(),
            "the peer never drained its eight-byte video buffer"
        );
        drop(transport);
        assert!(timeout(Duration::from_secs(1), blocked)
            .await
            .unwrap()
            .unwrap()
            .is_err());
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
