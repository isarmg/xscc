use super::{FrameMetadata, QueuedFrame, VIDEO_QUEUE, VIDEO_QUEUE_AGE, VIDEO_QUEUE_BYTES};
use base::direct_transport::udp_media::{MediaKind, MAX_VIDEO_FRAME};
use hbb_common::tokio::sync::Notify;
use std::{collections::VecDeque, sync::Mutex};

#[derive(Default)]
struct Pending {
    frames: VecDeque<QueuedFrame>,
    bytes: usize,
}

impl Pending {
    fn usage_excluding(&self, discard: impl Fn(&QueuedFrame) -> bool) -> (usize, usize) {
        self.frames
            .iter()
            .filter(|frame| !discard(frame))
            .fold((0, 0), |(count, bytes), frame| {
                (count + 1, bytes + frame.plan.descriptor().total_size as usize)
            })
    }
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

    /// Returns frames that need an abandonment notice.
    pub(super) fn push(
        &self,
        frame: QueuedFrame,
        acknowledged: impl Fn(FrameMetadata) -> bool,
    ) -> Vec<FrameMetadata> {
        let descriptor = frame.plan.descriptor();
        let metadata = descriptor.metadata;
        let size = descriptor.total_size as usize;
        let mut pending = self.pending.lock().unwrap();
        let (count_limit, byte_limit) = if metadata.keyframe {
            (VIDEO_QUEUE, VIDEO_QUEUE_BYTES)
        } else {
            (VIDEO_QUEUE - 1, VIDEO_QUEUE_BYTES - MAX_VIDEO_FRAME)
        };
        let fits = |(count, bytes): (usize, usize)| count < count_limit && bytes + size <= byte_limit;
        let superseded = |item: &QueuedFrame| supersedes(metadata, item.plan.descriptor().metadata);
        let mut usage = if metadata.keyframe {
            pending.usage_excluding(&superseded)
        } else {
            (pending.frames.len(), pending.bytes)
        };
        let mut abandoned = Vec::new();

        if !fits(usage) {
            let ready = acknowledged(metadata);
            let mut candidates = Vec::new();
            for (position, item) in pending.frames.iter().enumerate().rev() {
                if superseded(item) {
                    continue;
                }
                let prior = item.plan.descriptor().metadata;
                let expired = !prior.keyframe
                    && frame.queued.saturating_duration_since(item.queued) >= VIDEO_QUEUE_AGE;
                if expired {
                    candidates.push((0, position));
                } else if ready && !acknowledged(prior) {
                    // Keep the waiting stream's decodable prefix when reclaiming space.
                    let priority = if prior.keyframe { 2 } else { 1 };
                    candidates.push((priority, position));
                }
            }
            candidates.sort_by_key(|(priority, _)| *priority);
            let mut count = 0;
            for &(_, position) in &candidates {
                usage.0 -= 1;
                usage.1 -= pending.frames[position].plan.descriptor().total_size as usize;
                count += 1;
                if fits(usage) {
                    break;
                }
            }
            if !fits(usage) {
                return vec![metadata];
            }
            candidates.truncate(count);
            candidates.sort_unstable_by_key(|(_, position)| *position);
            let mut victims = candidates
                .into_iter()
                .map(|(_, position)| position)
                .peekable();
            let mut position = 0;
            pending.frames.retain(|item| {
                let dropped = victims.next_if_eq(&position).is_some();
                position += 1;
                if dropped {
                    abandoned.push(item.plan.descriptor().metadata);
                }
                !dropped && !superseded(item)
            });
        } else if metadata.keyframe {
            pending.frames.retain(|item| !superseded(item));
        }

        if metadata.keyframe {
            let position = pending
                .frames
                .iter()
                .position(|item| !item.plan.descriptor().metadata.keyframe)
                .unwrap_or(pending.frames.len());
            pending.frames.insert(position, frame);
        } else {
            pending.frames.push_back(frame);
        }
        pending.bytes = usage.1 + size;

        drop(pending);
        self.available.notify_one();
        abandoned
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
