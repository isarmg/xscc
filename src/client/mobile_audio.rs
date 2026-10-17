use base::message_proto::{AudioFormat, AudioFrame};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const MAX_FRAMES: usize = 64;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_QUEUED_BYTES: usize = 256 * 1024;
// Local residence/capacity budgets, not a network or end-to-end latency estimate.
const PCM_BUDGET_MS: usize = 150;
const QUEUE_AGE_BUDGET: Duration = Duration::from_millis(PCM_BUDGET_MS as u64);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStatus {
    pub enabled: bool,
    pub playing: bool,
    pub error: Option<String>,
}

#[derive(Default)]
struct Pending {
    frames: VecDeque<(Instant, AudioFrame)>,
    discontinuity: bool,
    bytes: usize,
    format: Option<AudioFormat>,
    revision: u64,
    closed: bool,
    status: AudioStatus,
    gate: Option<Arc<AtomicBool>>,
}

impl Pending {
    fn invalidate(&mut self) {
        if let Some(gate) = self.gate.take() {
            gate.store(false, Ordering::Release);
        }
        self.frames.clear();
        self.bytes = 0;
        self.discontinuity = false;
        self.revision = self.revision.wrapping_add(1);
        self.status.playing = false;
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.invalidate();
        self.status.enabled = enabled && !self.closed;
        self.status.error = None;
    }

    fn set_format(&mut self, format: AudioFormat) -> bool {
        let interrupted = self.status.enabled
            && self
                .gate
                .as_ref()
                .is_some_and(|gate| !gate.load(Ordering::Acquire));
        if interrupted {
            // A passive format update cannot reopen a platform-interrupted generation.
            self.set_enabled(false);
            self.status.error = Some("Audio was interrupted. Turn sound on again to retry.".into());
        } else {
            self.invalidate();
        }
        self.format = Some(format);
        interrupted
    }

    fn push(&mut self, frame: AudioFrame) -> bool {
        self.push_at(frame, Instant::now())
    }

    fn push_at(&mut self, frame: AudioFrame, now: Instant) -> bool {
        if self.closed
            || !self.status.enabled
            || self.format.is_none()
            || frame.data.len() > MAX_FRAME_BYTES
        {
            return false;
        }
        // Retain recent audio without ever waiting for the decoder to make room.
        while self.frames.len() >= MAX_FRAMES || self.bytes + frame.data.len() > MAX_QUEUED_BYTES {
            if let Some((_, old)) = self.frames.pop_front() {
                self.bytes -= old.data.len();
                self.discontinuity = true;
            } else {
                return false;
            }
        }
        self.bytes += frame.data.len();
        self.frames.push_back((now, frame));
        true
    }

    fn pop(&mut self) -> Option<(AudioFrame, bool)> {
        self.pop_at(Instant::now())
    }

    fn pop_at(&mut self, now: Instant) -> Option<(AudioFrame, bool)> {
        while let Some((received, frame)) = self.frames.pop_front() {
            self.bytes -= frame.data.len();
            if now.saturating_duration_since(received) > QUEUE_AGE_BUDGET {
                self.discontinuity = true;
                continue;
            }
            return Some((frame, std::mem::take(&mut self.discontinuity)));
        }
        None
    }
}

#[cfg(all(
    any(target_os = "ios", target_os = "android"),
    feature = "gpui-mobile-session"
))]
mod runtime {
    use super::*;
    use crate::{client::AudioHandler, platform::gpui_mobile::audio};
    use hbb_common::ResultType;
    use std::sync::{Condvar, Mutex};

    impl AudioHandler {
        fn mobile_output(
            format: AudioFormat,
            gate: Arc<AtomicBool>,
            failed: Arc<AtomicBool>,
        ) -> ResultType<Self> {
            use crate::client::{is_supported_audio_channel_count, AudioDecoder, Mono, Stereo};
            if !is_supported_audio_channel_count(format.channels) {
                hbb_common::bail!("Unsupported audio channel count: {}", format.channels);
            }
            let decoder = AudioDecoder::new(
                format.sample_rate,
                if format.channels == 1 { Mono } else { Stereo },
            )?;
            let mut handler = Self {
                audio_decoder: Some((
                    decoder,
                    vec![0.0; format.sample_rate as usize * format.channels as usize],
                )),
                channels: format.channels as u16,
                mobile_audio_gate: Some(gate),
                mobile_audio_failed: Some(failed),
                ..Default::default()
            };
            handler.start_audio(format)?;
            Ok(handler)
        }

        fn mobile_discontinuity(&mut self, format: &AudioFormat) -> ResultType<()> {
            use crate::client::create_audio_resampler;
            use ringbuf::{ring_buffer::RbBase, Rb};
            if let Some((decoder, _)) = self.audio_decoder.as_mut() {
                decoder.reset_state()?;
            }
            self.audio_resampler =
                create_audio_resampler(format.sample_rate, self.sample_rate.1, self.channels)?;
            let mut buffer = self.audio_buffer.0.lock().unwrap();
            let occupied = buffer.occupied_len();
            buffer.skip(occupied);
            self.audio_buffer.signal_discontinuity();
            Ok(())
        }

        pub(in crate::client) fn resize_mobile_audio_buffer(
            &mut self,
            sample_rate: u32,
            channels: u16,
        ) {
            self.audio_buffer.0 = Arc::new(Mutex::new(ringbuf::HeapRb::new(
                sample_rate as usize * channels as usize * PCM_BUDGET_MS / 1000,
            )));
            self.audio_buffer.1 = sample_rate as usize * channels as usize;
        }
    }

    struct Shared {
        pending: Mutex<Pending>,
        wake: Condvar,
        changed: Box<dyn Fn() + Send + Sync>,
    }

    struct Owner {
        shared: Arc<Shared>,
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            let mut pending = self.shared.pending.lock().unwrap();
            pending.closed = true;
            pending.set_enabled(false);
            self.shared.wake.notify_one();
        }
    }

    #[derive(Clone)]
    pub struct MobileAudio(Arc<Owner>);

    impl MobileAudio {
        pub fn new(changed: impl Fn() + Send + Sync + 'static) -> ResultType<Self> {
            let shared = Arc::new(Shared {
                pending: Mutex::new(Pending::default()),
                wake: Condvar::new(),
                changed: Box::new(changed),
            });
            let worker = shared.clone();
            std::thread::Builder::new()
                .name("mobile-audio".into())
                .spawn(move || run(worker))?;
            Ok(Self(Arc::new(Owner { shared })))
        }

        pub fn set_enabled(&self, enabled: bool) {
            self.0.shared.pending.lock().unwrap().set_enabled(enabled);
            self.0.shared.wake.notify_one();
            (self.0.shared.changed)();
        }

        pub fn handle_format(&self, format: AudioFormat) {
            let mut pending = self.0.shared.pending.lock().unwrap();
            if pending.closed {
                return;
            }
            let interrupted = pending.set_format(format);
            drop(pending);
            self.0.shared.wake.notify_one();
            if interrupted {
                (self.0.shared.changed)();
            }
        }

        pub fn push_frame(&self, frame: AudioFrame) {
            if self.0.shared.pending.lock().unwrap().push(frame) {
                self.0.shared.wake.notify_one();
            }
        }

        pub fn stop(&self) {
            let mut pending = self.0.shared.pending.lock().unwrap();
            pending.closed = true;
            pending.set_enabled(false);
            pending.format = None;
            drop(pending);
            self.0.shared.wake.notify_one();
            (self.0.shared.changed)();
        }

        pub fn status(&self) -> AudioStatus {
            self.0.shared.pending.lock().unwrap().status.clone()
        }
    }

    struct Output {
        // Drop the stream before deactivating its platform session.
        handler: AudioHandler,
        session: audio::PlaybackSession,
        gate: Arc<AtomicBool>,
        failed: Arc<AtomicBool>,
        format: AudioFormat,
    }

    enum Work {
        Reset(u64, Option<AudioFormat>, Arc<AtomicBool>),
        Frame(AudioFrame, bool),
        Check,
        Stop,
    }

    fn fail(shared: &Shared, revision: u64, error: String) {
        let mut pending = shared.pending.lock().unwrap();
        if pending.revision != revision || pending.closed {
            return;
        }
        pending.set_enabled(false);
        pending.status.error = Some(error);
        drop(pending);
        (shared.changed)();
    }

    fn open(format: AudioFormat, gate: Arc<AtomicBool>) -> ResultType<Output> {
        let session = audio::activate(gate.clone())?;
        let failed = Arc::new(AtomicBool::new(false));
        let handler = AudioHandler::mobile_output(format.clone(), gate.clone(), failed.clone())?;
        Ok(Output {
            handler,
            session,
            gate,
            failed,
            format,
        })
    }

    fn run(shared: Arc<Shared>) {
        let mut revision = u64::MAX;
        let mut output: Option<Output> = None;
        loop {
            let work = {
                let mut pending = shared.pending.lock().unwrap();
                if pending.closed {
                    Work::Stop
                } else if revision != pending.revision {
                    let gate = Arc::new(AtomicBool::new(pending.status.enabled));
                    pending.gate = Some(gate.clone());
                    Work::Reset(
                        pending.revision,
                        pending
                            .status
                            .enabled
                            .then(|| pending.format.clone())
                            .flatten(),
                        gate,
                    )
                } else if let Some((frame, discontinuity)) = pending.pop() {
                    Work::Frame(frame, discontinuity)
                } else {
                    if pending.status.enabled && pending.format.is_some() {
                        let _guard = shared
                            .wake
                            .wait_timeout(pending, Duration::from_millis(100))
                            .unwrap();
                    } else {
                        let _guard = shared.wake.wait(pending).unwrap();
                    }
                    Work::Check
                }
            };
            match work {
                Work::Stop => break,
                Work::Reset(next, format, gate) => {
                    output = None;
                    revision = next;
                    if let Some(format) = format {
                        match open(format, gate) {
                            Ok(candidate) => {
                                let mut pending = shared.pending.lock().unwrap();
                                if pending.revision == revision && !pending.closed {
                                    pending.status.playing = candidate.gate.load(Ordering::Acquire);
                                    output = Some(candidate);
                                }
                                drop(pending);
                                (shared.changed)();
                            }
                            Err(error) => fail(
                                &shared,
                                revision,
                                format!("Audio playback unavailable: {error:#}"),
                            ),
                        }
                    }
                }
                Work::Frame(frame, discontinuity) => {
                    if let Some(output) = output.as_mut() {
                        if output.gate.load(Ordering::Acquire) {
                            if discontinuity {
                                if let Err(error) =
                                    output.handler.mobile_discontinuity(&output.format)
                                {
                                    output.gate.store(false, Ordering::Release);
                                    fail(
                                        &shared,
                                        revision,
                                        format!("Audio reset failed: {error:#}"),
                                    );
                                    continue;
                                }
                            }
                            output.handler.handle_frame(frame);
                        }
                    }
                }
                Work::Check => {}
            }
            if let Some(current) = output.as_ref() {
                if current.failed.load(Ordering::Acquire) || !current.session.allowed() {
                    current.gate.store(false, Ordering::Release);
                    fail(
                        &shared,
                        revision,
                        current.session.error().unwrap_or_else(|| {
                            "Audio output failed. Turn sound on again to retry.".into()
                        }),
                    );
                    output = None;
                }
            }
        }
    }
}

#[cfg(all(
    any(target_os = "ios", target_os = "android"),
    feature = "gpui-mobile-session"
))]
pub use runtime::MobileAudio;

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(bytes: usize, marker: u8) -> AudioFrame {
        AudioFrame {
            data: vec![marker; bytes].into(),
            ..Default::default()
        }
    }

    fn enabled() -> Pending {
        let mut pending = Pending::default();
        pending.format = Some(AudioFormat::default());
        pending.set_enabled(true);
        pending
    }

    #[test]
    fn audio_queue_is_bounded_by_count_and_bytes() {
        let mut pending = enabled();
        for _ in 0..MAX_FRAMES * 2 {
            assert!(pending.push(frame(1, 1)));
        }
        assert_eq!(pending.frames.len(), MAX_FRAMES);
        for _ in 0..MAX_FRAMES {
            assert!(pending.push(frame(MAX_FRAME_BYTES, 2)));
        }
        assert_eq!(pending.bytes, MAX_QUEUED_BYTES);
        assert!(!pending.push(frame(MAX_FRAME_BYTES + 1, 3)));
        assert_eq!(pending.pop().unwrap().0.data[0], 2);
    }

    #[test]
    fn mute_closes_output_gate_and_discards_backlog_before_reenable() {
        let mut pending = enabled();
        let gate = Arc::new(AtomicBool::new(true));
        pending.gate = Some(gate.clone());
        assert!(pending.push(frame(10, 1)));
        pending.set_enabled(false);
        assert!(!gate.load(Ordering::Acquire));
        assert!(pending.pop().is_none());
        assert!(!pending.push(frame(10, 2)));
        pending.set_enabled(true);
        assert!(pending.pop().is_none());
        assert!(pending.format.is_some());
    }

    #[test]
    fn stale_audio_is_discarded_and_close_cannot_be_reenabled() {
        let mut pending = enabled();
        let now = Instant::now();
        assert!(pending.push_at(frame(10, 1), now));
        assert!(pending.push_at(frame(10, 2), now + QUEUE_AGE_BUDGET));
        let (fresh, discontinuity) = pending
            .pop_at(now + QUEUE_AGE_BUDGET + Duration::from_millis(1))
            .unwrap();
        assert_eq!(fresh.data[0], 2);
        assert!(discontinuity);
        pending.invalidate();
        pending.format = Some(AudioFormat {
            sample_rate: 48000,
            channels: 2,
            ..Default::default()
        });
        assert!(pending.pop().is_none());
        assert_eq!(pending.bytes, 0);
        pending.gate = Some(Arc::new(AtomicBool::new(false)));
        assert!(pending.set_format(AudioFormat::default()));
        assert!(!pending.status.enabled);
        assert!(pending.status.error.is_some());
        pending.closed = true;
        pending.set_enabled(true);
        assert!(!pending.status.enabled);
        assert!(!pending.push(frame(10, 2)));
    }
}
