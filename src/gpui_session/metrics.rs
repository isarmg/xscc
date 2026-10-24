//! Wall-clock pipeline telemetry, separate from adaptive decoder work capacity.
use std::time::{Duration, Instant};

const SAMPLE_COUNT: usize = 128;
struct Samples {
    values: [u32; SAMPLE_COUNT],
    next: usize,
    len: usize,
}
impl Default for Samples {
    fn default() -> Self {
        Self {
            values: [0; SAMPLE_COUNT],
            next: 0,
            len: 0,
        }
    }
}
impl Samples {
    fn push(&mut self, duration: Duration) {
        self.values[self.next] = duration.as_micros().min(u32::MAX as u128) as u32;
        self.next = (self.next + 1) % SAMPLE_COUNT;
        self.len = (self.len + 1).min(SAMPLE_COUNT);
    }
    fn percentile_ms(&self, percentile: usize) -> Option<f32> {
        if self.len == 0 {
            return None;
        }
        let mut values = self.values;
        values[..self.len].sort_unstable();
        let index = (self.len * percentile).div_ceil(100).saturating_sub(1);
        Some(values[index.min(self.len - 1)] as f32 / 1000.)
    }
}

pub(super) struct Metrics {
    start: Instant,
    epoch: u64,
    received: u64,
    decoded: u64,
    submitted: u64,
    dropped: u64,
    decode_time: Samples,
    ui_queue_time: Samples,
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            epoch: 0,
            received: 0,
            decoded: 0,
            submitted: 0,
            dropped: 0,
            decode_time: Samples::default(),
            ui_queue_time: Samples::default(),
        }
    }
}
pub(super) struct Sample {
    pub received_fps: i32,
    pub decoded_fps: i32,
    pub submitted_fps: i32,
    pub decode_p50_ms: Option<f32>,
    pub decode_p95_ms: Option<f32>,
    pub ui_queue_p95_ms: Option<f32>,
    pub mailbox_dropped: u64,
}
impl Metrics {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn reset(&mut self) {
        *self = Self {
            epoch: self.epoch.wrapping_add(1),
            ..Self::default()
        };
    }
    pub fn received(&mut self, frames: usize) {
        self.received = self.received.saturating_add(frames as u64);
    }
    pub fn decoded(&mut self, duration: Duration) {
        self.decoded = self.decoded.saturating_add(1);
        self.decode_time.push(duration);
    }
    pub fn submitted(&mut self, queue_time: Duration) {
        self.submitted = self.submitted.saturating_add(1);
        self.ui_queue_time.push(queue_time);
    }
    pub fn dropped(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
    }
    pub fn snapshot(&mut self, now: Instant) -> Option<Sample> {
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed < Duration::from_secs(1) {
            return None;
        }
        let rate = |count: u64| {
            ((count as f64 / elapsed.as_secs_f64())
                .round()
                .min(i32::MAX as f64)) as i32
        };
        let sample = Sample {
            received_fps: rate(self.received),
            decoded_fps: rate(self.decoded),
            submitted_fps: rate(self.submitted),
            decode_p50_ms: self.decode_time.percentile_ms(50),
            decode_p95_ms: self.decode_time.percentile_ms(95),
            ui_queue_p95_ms: self.ui_queue_time.percentile_ms(95),
            mailbox_dropped: self.dropped,
        };
        *self = Self {
            start: now,
            epoch: self.epoch,
            ..Self::default()
        };
        Some(sample)
    }
    #[cfg(test)]
    pub fn restart(&mut self, now: Instant) {
        self.start = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_network_decode_and_ui_rates_from_decode_work_capacity() {
        let mut m = Metrics::default();
        let start = m.start;
        m.restart(start);
        m.received(240);
        for _ in 0..120 {
            m.decoded(Duration::from_millis(2));
        }
        for _ in 0..60 {
            m.submitted(Duration::from_millis(4));
        }
        m.dropped();
        assert!(m.snapshot(start + Duration::from_millis(999)).is_none());
        let s = m.snapshot(start + Duration::from_secs(2)).unwrap();
        assert_eq!(
            (s.received_fps, s.decoded_fps, s.submitted_fps),
            (120, 60, 30)
        );
        assert_eq!(s.decode_p50_ms, Some(2.));
        assert_eq!(s.decode_p95_ms, Some(2.));
        assert_eq!(s.ui_queue_p95_ms, Some(4.));
        assert_eq!(s.mailbox_dropped, 1);
        let idle = m.snapshot(start + Duration::from_secs(3)).unwrap();
        assert_eq!(
            (idle.received_fps, idle.decoded_fps, idle.submitted_fps),
            (0, 0, 0)
        );
        assert!(idle.decode_p95_ms.is_none());
    }
    #[test]
    fn monitor_reset_discards_old_samples_and_advances_generation() {
        let mut m = Metrics::default();
        let old = m.epoch();
        m.received(120);
        m.decoded(Duration::from_millis(30));
        m.reset();
        assert_ne!(m.epoch(), old);
        let sample = m.snapshot(Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(sample.received_fps, 0);
        assert_eq!(sample.decoded_fps, 0);
        assert!(sample.decode_p95_ms.is_none());
        assert_ne!(m.epoch(), old);
    }

    #[test]
    fn rolling_percentiles_are_bounded_and_keep_recent_tail_latency() {
        let mut samples = Samples::default();
        for _ in 0..256 {
            samples.push(Duration::from_secs(1));
        }
        for millis in 1..=128 {
            samples.push(Duration::from_millis(millis));
        }
        assert_eq!(samples.len, 128);
        assert_eq!(samples.percentile_ms(50), Some(64.));
        assert_eq!(samples.percentile_ms(95), Some(122.));
    }
}
