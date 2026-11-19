use base::message_proto::VideoFrame;
use std::{
    collections::VecDeque,
    sync::{mpsc::RecvTimeoutError, Condvar, Mutex},
    time::Duration,
};

pub(super) struct VideoPacket {
    pub(super) frame: VideoFrame,
    pub(super) keyframe: bool,
    #[cfg(feature = "mediacodec")]
    pub(super) dimensions: Option<(i32, i32)>,
}

pub(super) enum VideoItem {
    Frame(VideoPacket),
    Reset,
}

#[derive(Default)]
struct Pending {
    frames: VecDeque<VideoPacket>,
    reset: bool,
    needs_keyframe: bool,
    closed: bool,
}

impl Pending {
    fn available(&self) -> bool {
        self.closed || self.reset || !self.frames.is_empty()
    }

    fn pop(&mut self) -> Result<VideoItem, RecvTimeoutError> {
        if self.closed {
            Err(RecvTimeoutError::Disconnected)
        } else if std::mem::take(&mut self.reset) {
            Ok(VideoItem::Reset)
        } else {
            self.frames
                .pop_front()
                .map(VideoItem::Frame)
                .ok_or(RecvTimeoutError::Timeout)
        }
    }

    fn invalidate(&mut self) -> bool {
        self.frames.clear();
        !std::mem::replace(&mut self.needs_keyframe, true)
    }
}

pub(super) struct VideoQueue {
    capacity: usize,
    pending: Mutex<Pending>,
    ready: Condvar,
}

impl VideoQueue {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            pending: Mutex::new(Pending::default()),
            ready: Condvar::new(),
        }
    }

    pub(super) fn capacity(&self) -> usize {
        self.capacity
    }

    pub(super) fn len(&self) -> usize {
        self.pending.lock().unwrap().frames.len()
    }

    /// Reports the first loss of the compressed reference chain.
    pub(super) fn push(&self, packet: VideoPacket) -> bool {
        let mut pending = self.pending.lock().unwrap();
        if pending.closed || (pending.needs_keyframe && !packet.keyframe) {
            return false;
        }
        if pending.frames.len() == self.capacity {
            if !packet.keyframe {
                return pending.invalidate();
            }
            pending.frames.clear();
        }
        pending.needs_keyframe = false;
        pending.frames.push_back(packet);
        self.ready.notify_one();
        false
    }

    pub(super) fn invalidate(&self) -> bool {
        self.pending.lock().unwrap().invalidate()
    }

    pub(super) fn reset(&self) {
        let mut pending = self.pending.lock().unwrap();
        pending.invalidate();
        pending.reset = true;
        self.ready.notify_one();
    }

    pub(super) fn close(&self) {
        let mut pending = self.pending.lock().unwrap();
        pending.closed = true;
        pending.frames.clear();
        self.ready.notify_one();
    }

    pub(super) fn recv(&self) -> Result<VideoItem, RecvTimeoutError> {
        self.ready
            .wait_while(self.pending.lock().unwrap(), |pending| !pending.available())
            .unwrap()
            .pop()
    }

    pub(super) fn recv_timeout(&self, wait: Duration) -> Result<VideoItem, RecvTimeoutError> {
        self.ready
            .wait_timeout_while(self.pending.lock().unwrap(), wait, |pending| {
                !pending.available()
            })
            .unwrap()
            .0
            .pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: i32, keyframe: bool) -> VideoPacket {
        VideoPacket {
            frame: VideoFrame {
                display: id,
                ..Default::default()
            },
            keyframe,
            #[cfg(feature = "mediacodec")]
            dimensions: None,
        }
    }

    #[test]
    fn keyframe_and_its_followup_survive_a_full_queue() {
        let queue = VideoQueue::new(2);
        assert!(!queue.push(frame(0, false)));
        assert!(!queue.push(frame(1, false)));
        assert!(!queue.push(frame(2, true)));
        assert!(!queue.push(frame(3, false)));
        for id in [2, 3] {
            assert!(
                matches!(queue.recv_timeout(Duration::ZERO), Ok(VideoItem::Frame(packet))
                if packet.frame.display == id)
            );
        }
    }

    #[test]
    fn overflow_discards_the_broken_chain_and_requests_only_one_refresh() {
        let queue = VideoQueue::new(1);
        let other = VideoQueue::new(1);
        assert!(!queue.push(frame(0, false)));
        assert!(queue.push(frame(1, false)));
        assert!(!queue.push(frame(2, false)));
        assert!(queue.recv_timeout(Duration::ZERO).is_err());
        assert!(!other.push(frame(9, false)));
        assert!(
            matches!(other.recv_timeout(Duration::ZERO), Ok(VideoItem::Frame(packet))
            if packet.frame.display == 9)
        );
        assert!(!queue.push(frame(3, true)));
        assert!(
            matches!(queue.recv_timeout(Duration::ZERO), Ok(VideoItem::Frame(packet))
            if packet.frame.display == 3)
        );
    }

    #[test]
    fn reset_precedes_the_new_generation_and_discards_the_old_frames() {
        let queue = VideoQueue::new(2);
        assert!(!queue.push(frame(0, true)));
        queue.reset();
        assert!(!queue.push(frame(1, false)));
        assert!(!queue.push(frame(2, true)));
        assert!(matches!(
            queue.recv_timeout(Duration::ZERO),
            Ok(VideoItem::Reset)
        ));
        assert!(
            matches!(queue.recv_timeout(Duration::ZERO), Ok(VideoItem::Frame(packet))
            if packet.frame.display == 2)
        );
        assert!(queue.recv_timeout(Duration::ZERO).is_err());
    }
}
