use base::direct_transport::udp_media::FrameMetadata;
use hbb_common::log;
use serde::Serialize;
use std::time::{Duration, Instant};

pub(super) struct VideoReceiveMetrics {
    reported_at: Instant,
    highest_sequence: Option<u64>,
    last_video_fragment: Option<Instant>,
    pub(super) window: VideoReceiveSample,
}

#[derive(Default, Serialize)]
pub(super) struct VideoReceiveSample {
    schema: u32,
    elapsed_ms: f64,
    pub(super) socket_datagrams: u64,
    authenticated_datagrams: u64,
    highest_authenticated_sequence: Option<u64>,
    sequence_gap_slots: u64,
    reordered_datagrams: u64,
    video_fragments: u64,
    max_video_fragment_gap_ms: f64,
    pub(super) reassembled_video_messages: u64,
    pub(super) expired_video_assemblies: usize,
    pub(super) expired_missing_video_fragments: usize,
    pub(super) expired_video_sent_notices: usize,
    pub(super) fec_recovered_fragments: usize,
    pub(super) received_video_parity_datagrams: u64,
    pub(super) sender_abandoned_video_messages: u64,
    pub(super) video_wrong_epoch_fragments: u64,
    pub(super) video_capacity_rejections: u64,
    pub(super) video_receive_queue_drops: u64,
    pub(super) video_receive_queue_recovered: u64,
    pub(super) keyframe_nacks: u64,
    pub(super) superseded_keyframes: u64,
    pub(super) stalled_keyframes: usize,
    pub(super) refresh_retries: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) last_reassembled_video: Option<FrameMetadata>,
}

impl VideoReceiveMetrics {
    pub(super) fn new(now: Instant, highest_sequence: Option<u64>) -> Self {
        Self {
            reported_at: now,
            highest_sequence,
            last_video_fragment: None,
            window: VideoReceiveSample::default(),
        }
    }

    pub(super) fn authenticated(&mut self, highest_sequence: u64) {
        self.window.authenticated_datagrams += 1;
        if let Some(previous) = self.highest_sequence.replace(highest_sequence) {
            self.window.sequence_gap_slots +=
                highest_sequence.saturating_sub(previous).saturating_sub(1);
            // A unique packet leaves the high-water mark unchanged if it arrives out of order.
            self.window.reordered_datagrams += u64::from(highest_sequence == previous);
        }
    }

    pub(super) fn video_fragment(&mut self, now: Instant) {
        self.window.video_fragments += 1;
        if let Some(previous) = self.last_video_fragment.replace(now) {
            let gap_ms = now.saturating_duration_since(previous).as_secs_f64() * 1_000.0;
            self.window.max_video_fragment_gap_ms =
                self.window.max_video_fragment_gap_ms.max(gap_ms);
        }
    }

    fn take_sample(&mut self, now: Instant) -> Option<VideoReceiveSample> {
        let elapsed = now.saturating_duration_since(self.reported_at);
        if elapsed < Duration::from_secs(1) {
            return None;
        }
        self.reported_at = now;
        let mut sample = std::mem::take(&mut self.window);
        if sample.socket_datagrams == 0
            && sample.expired_video_assemblies == 0
            && sample.expired_video_sent_notices == 0
            && sample.sender_abandoned_video_messages == 0
            && sample.keyframe_nacks == 0
            && sample.superseded_keyframes == 0
            && sample.stalled_keyframes == 0
            && sample.refresh_retries == 0
        {
            return None;
        }
        sample.schema = 1;
        sample.elapsed_ms = elapsed.as_secs_f64() * 1_000.0;
        sample.highest_authenticated_sequence = self.highest_sequence;
        Some(sample)
    }

    pub(super) fn report(&mut self, now: Instant) {
        if let Some(sample) = self
            .take_sample(now)
            .filter(|_| log::log_enabled!(target: "xscc::client_udp_metrics", log::Level::Info))
        {
            match serde_json::to_string(&sample) {
                Ok(json) => {
                    log::info!(target: "xscc::client_udp_metrics", "client_udp_video_pipeline {json}")
                }
                Err(error) => log::warn!("Could not serialize UDP receive metrics: {error}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_sample_separates_new_sequence_gaps_from_late_arrivals() {
        let started = Instant::now();
        let mut metrics = VideoReceiveMetrics::new(started, Some(10));
        metrics.window.socket_datagrams = 3;
        metrics.authenticated(13);
        metrics.video_fragment(started);
        metrics.authenticated(13);
        metrics.video_fragment(started + Duration::from_millis(12));

        assert!(metrics
            .take_sample(started + Duration::from_millis(999))
            .is_none());
        let first = metrics
            .take_sample(started + Duration::from_secs(1))
            .unwrap();
        assert_eq!(first.socket_datagrams, 3);
        assert_eq!(first.authenticated_datagrams, 2);
        assert_eq!(first.sequence_gap_slots, 2);
        assert_eq!(first.reordered_datagrams, 1);
        assert_eq!(first.max_video_fragment_gap_ms, 12.0);

        metrics.window.socket_datagrams = 1;
        metrics.authenticated(14);
        let next = metrics
            .take_sample(started + Duration::from_secs(2))
            .unwrap();
        assert_eq!(next.sequence_gap_slots, 0);
        assert_eq!(next.reordered_datagrams, 0);
    }
}
