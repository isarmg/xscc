use base::message_proto::{AudioFormat, AudioFrame};
use std::{
    collections::VecDeque,
    sync::{mpsc::RecvTimeoutError, Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

const MAX_FRAMES: usize = 16;
const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_AGE: Duration = Duration::from_millis(150);

pub(super) enum AudioItem {
    Format(AudioFormat),
    Frame(AudioFrame, bool),
}

#[derive(Default)]
struct Pending {
    format: Option<AudioFormat>,
    frames: VecDeque<(Instant, AudioFrame)>,
    discontinuity: bool,
    closed: bool,
}

impl Pending {
    fn available(&self) -> bool {
        self.closed || self.format.is_some() || !self.frames.is_empty()
    }

    fn pop(&mut self, now: Instant) -> Result<AudioItem, RecvTimeoutError> {
        if self.closed {
            return Err(RecvTimeoutError::Disconnected);
        }
        if let Some(format) = self.format.take() {
            return Ok(AudioItem::Format(format));
        }
        while let Some((received, frame)) = self.frames.pop_front() {
            if now.saturating_duration_since(received) <= MAX_AGE {
                return Ok(AudioItem::Frame(
                    frame,
                    std::mem::take(&mut self.discontinuity),
                ));
            }
            self.discontinuity = true;
        }
        Err(RecvTimeoutError::Timeout)
    }
}

pub(super) struct AudioSender(Arc<AudioQueue>);

impl AudioSender {
    pub(super) fn set_format(&self, format: AudioFormat) {
        let mut pending = self.0.pending.lock().unwrap();
        pending.frames.clear();
        pending.discontinuity = false;
        pending.format = Some(format);
        self.0.ready.notify_one();
    }

    pub(super) fn push(&self, frame: AudioFrame) {
        self.push_at(frame, Instant::now());
    }

    fn push_at(&self, frame: AudioFrame, now: Instant) {
        let mut pending = self.0.pending.lock().unwrap();
        if frame.data.len() > MAX_FRAME_BYTES {
            return;
        }
        if pending.frames.len() == MAX_FRAMES {
            pending.frames.pop_front();
            pending.discontinuity = true;
        }
        pending.frames.push_back((now, frame));
        self.0.ready.notify_one();
    }
}

impl Drop for AudioSender {
    fn drop(&mut self) {
        let mut pending = self.0.pending.lock().unwrap();
        pending.closed = true;
        pending.frames.clear();
        pending.format = None;
        self.0.ready.notify_one();
    }
}

#[derive(Default)]
pub(super) struct AudioQueue {
    pending: Mutex<Pending>,
    ready: Condvar,
}

impl AudioQueue {
    pub(super) fn new() -> (AudioSender, Arc<Self>) {
        let queue = Arc::new(Self::default());
        (AudioSender(queue.clone()), queue)
    }

    #[cfg(not(windows))]
    pub(super) fn recv(&self) -> Result<AudioItem, RecvTimeoutError> {
        self.ready
            .wait_while(self.pending.lock().unwrap(), |pending| !pending.available())
            .unwrap()
            .pop(Instant::now())
    }

    #[cfg(any(test, windows))]
    pub(super) fn recv_timeout(&self, timeout: Duration) -> Result<AudioItem, RecvTimeoutError> {
        self.ready
            .wait_timeout_while(self.pending.lock().unwrap(), timeout, |pending| {
                !pending.available()
            })
            .unwrap()
            .0
            .pop(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(marker: u8) -> AudioFrame {
        AudioFrame {
            data: vec![marker].into(),
            ..Default::default()
        }
    }

    #[test]
    fn stalled_playback_keeps_recent_audio_and_reports_the_gap() {
        let (sender, queue) = AudioQueue::new();
        let now = Instant::now();
        for marker in 0..=MAX_FRAMES as u8 {
            sender.push_at(frame(marker), now);
        }
        assert!(matches!(queue.pending.lock().unwrap().pop(now),
            Ok(AudioItem::Frame(frame, true)) if frame.data[0] == 1));

        let resumed = now + MAX_AGE + Duration::from_millis(1);
        sender.push_at(frame(42), resumed);
        assert!(matches!(queue.pending.lock().unwrap().pop(resumed),
            Ok(AudioItem::Frame(frame, true)) if frame.data[0] == 42));
    }

    #[test]
    fn replacement_format_and_disconnect_discard_queued_audio() {
        let (sender, queue) = AudioQueue::new();
        sender.push(frame(1));
        sender.set_format(AudioFormat {
            channels: 2,
            ..Default::default()
        });
        sender.push(frame(2));
        assert!(matches!(queue.recv_timeout(Duration::ZERO),
            Ok(AudioItem::Format(format)) if format.channels == 2));
        assert!(matches!(queue.recv_timeout(Duration::ZERO),
            Ok(AudioItem::Frame(frame, false)) if frame.data[0] == 2));
        sender.push(frame(3));
        drop(sender);
        assert!(matches!(
            queue.recv_timeout(Duration::ZERO),
            Err(RecvTimeoutError::Disconnected)
        ));
    }
}
