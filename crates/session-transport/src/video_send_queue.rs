use super::{FrameMetadata, QueuedFrame, VIDEO_QUEUE, VIDEO_QUEUE_BYTES};
use base::direct_transport::udp_media::{MediaKind, MAX_VIDEO_FRAME};
use hbb_common::tokio::sync::Notify;
use std::{collections::VecDeque, sync::Mutex};

#[derive(Default)]
struct Pending {
    frames: VecDeque<QueuedFrame>,
    bytes: usize,
}

#[derive(Default)]
pub(super) struct VideoSendQueue {
    pending: Mutex<Pending>,
    available: Notify,
}

pub(super) fn supersedes(keyframe: FrameMetadata, prior: FrameMetadata) -> bool {
    keyframe.keyframe
        && keyframe.kind == prior.kind
        && keyframe.display_id == prior.display_id
        && (keyframe.epoch, keyframe.frame_id) > (prior.epoch, prior.frame_id)
}

impl VideoSendQueue {
    pub(super) fn discard_before_epoch(&self, display_id: u32, epoch: u64) {
        let mut pending = self.pending.lock().unwrap();
        let mut bytes = 0;
        pending.frames.retain(|frame| {
            let descriptor = frame.plan.descriptor();
            let metadata = descriptor.metadata;
            if metadata.kind == MediaKind::Video
                && metadata.display_id == display_id
                && metadata.epoch < epoch
            {
                return false;
            }
            bytes += descriptor.total_size as usize;
            true
        });
        pending.bytes = bytes;
    }

    pub(super) fn push(&self, frame: QueuedFrame) -> bool {
        let descriptor = frame.plan.descriptor();
        let metadata = descriptor.metadata;
        let size = descriptor.total_size as usize;
        let mut pending = self.pending.lock().unwrap();

        if metadata.keyframe {
            let (count, bytes) = pending.frames.iter().fold((0, 0), |(count, bytes), item| {
                let prior = item.plan.descriptor();
                if supersedes(metadata, prior.metadata) {
                    (count, bytes)
                } else {
                    (count + 1, bytes + prior.total_size as usize)
                }
            });
            if count >= VIDEO_QUEUE || bytes + size > VIDEO_QUEUE_BYTES {
                return false;
            }
            pending
                .frames
                .retain(|item| !supersedes(metadata, item.plan.descriptor().metadata));
            let position = pending
                .frames
                .iter()
                .position(|item| !item.plan.descriptor().metadata.keyframe)
                .unwrap_or(pending.frames.len());
            pending.frames.insert(position, frame);
            pending.bytes = bytes + size;
        } else {
            if pending.frames.len() >= VIDEO_QUEUE - 1
                || pending.bytes + size > VIDEO_QUEUE_BYTES - MAX_VIDEO_FRAME
            {
                return false;
            }
            pending.frames.push_back(frame);
            pending.bytes += size;
        }

        drop(pending);
        self.available.notify_one();
        true
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(super) fn is_empty(&self) -> bool {
        self.pending.lock().unwrap().frames.is_empty()
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(super) fn pop(
        &self,
        current: Option<FrameMetadata>,
        repair: Option<FrameMetadata>,
        acknowledged: &impl Fn(FrameMetadata) -> bool,
    ) -> Option<QueuedFrame> {
        let mut pending = self.pending.lock().unwrap();
        let idle = current.is_none() && repair.is_none();
        let position = pending.frames.iter().position(|frame| {
            let metadata = frame.plan.descriptor().metadata;
            (idle
                || (metadata.keyframe
                    && current.is_none_or(|prior| !prior.keyframe && supersedes(metadata, prior))
                    && repair.is_none_or(|prior| supersedes(metadata, prior))))
                && acknowledged(metadata)
        })?;
        let frame = pending.frames.remove(position)?;
        pending.bytes -= frame.plan.descriptor().total_size as usize;
        Some(frame)
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(super) async fn next(
        &self,
        current: Option<FrameMetadata>,
        repair: Option<FrameMetadata>,
        acknowledged: impl Fn(FrameMetadata) -> bool,
    ) -> QueuedFrame {
        loop {
            let notified = self.available.notified();
            if let Some(frame) = self.pop(current, repair, &acknowledged) {
                return frame;
            }
            notified.await;
        }
    }
}
