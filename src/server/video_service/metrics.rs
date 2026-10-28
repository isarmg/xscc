use std::{
    fmt,
    time::{Duration, Instant},
};

pub(super) enum CaptureState {
    Ready,
    Empty,
    WouldBlock,
    Failed,
}

pub(super) enum EncodeKind {
    Fresh,
    Replay,
    Repeat,
}

#[derive(Default, Clone, Copy)]
struct Stage {
    calls: u64,
    total: Duration,
    max: Duration,
}

impl Stage {
    fn record(&mut self, elapsed: Duration) {
        self.calls += 1;
        self.total += elapsed;
        self.max = self.max.max(elapsed);
    }

    fn average_ms(&self) -> f64 {
        if self.calls == 0 {
            0.0
        } else {
            milliseconds(self.total) / self.calls as f64
        }
    }
}

pub(super) struct Metrics {
    started: Instant,
    capture: Stage,
    convert: Stage,
    encode_send: Stage,
    outcomes: [u64; 4],
    encodes: [u64; 3],
}

impl Metrics {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            started: now,
            capture: Stage::default(),
            convert: Stage::default(),
            encode_send: Stage::default(),
            outcomes: [0; 4],
            encodes: [0; 3],
        }
    }

    pub(super) fn captured(&mut self, elapsed: Duration, state: CaptureState) {
        self.capture.record(elapsed);
        self.outcomes[state as usize] += 1;
    }

    pub(super) fn converted(&mut self, elapsed: Duration) {
        self.convert.record(elapsed);
    }

    pub(super) fn encoded(&mut self, elapsed: Duration, kind: EncodeKind) {
        self.encode_send.record(elapsed);
        self.encodes[kind as usize] += 1;
    }

    pub(super) fn sample(&mut self, now: Instant, spf: Duration) -> Option<Sample> {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed < Duration::from_secs(1) {
            return None;
        }
        Some(Sample {
            elapsed,
            spf,
            capture_gdi: 2,
            values: std::mem::replace(self, Self::new(now)),
        })
    }
}

pub(super) struct Sample {
    elapsed: Duration,
    spf: Duration,
    capture_gdi: u8,
    values: Metrics,
}

impl Sample {
    #[cfg(any(windows, test))]
    pub(super) fn with_capture_gdi(mut self, gdi: bool) -> Self {
        self.capture_gdi = u8::from(gdi);
        self
    }
}

fn milliseconds(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

impl fmt::Display for Sample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let values = &self.values;
        write!(f, concat!(
            "{{\"schema\":1,\"elapsed_ms\":{:.3},\"spf_target_ms\":{:.3},",
            "\"capture_calls\":{},\"frame_count\":{},\"capture_empty\":{},\"capture_would_block\":{},\"capture_failed\":{},",
            "\"capture_avg_ms\":{:.3},\"capture_max_ms\":{:.3},\"convert_calls\":{},\"convert_avg_ms\":{:.3},\"convert_max_ms\":{:.3},",
            "\"encode_calls\":{},\"fresh_encode_calls\":{},\"replay_encode_calls\":{},\"repeat_encode_calls\":{},",
            "\"encode_send_avg_ms\":{:.3},\"encode_send_max_ms\":{:.3},\"capture_gdi\":{}}}"
        ), milliseconds(self.elapsed), milliseconds(self.spf),
            values.capture.calls, values.outcomes[0], values.outcomes[1], values.outcomes[2], values.outcomes[3],
            values.capture.average_ms(), milliseconds(values.capture.max), values.convert.calls,
            values.convert.average_ms(), milliseconds(values.convert.max), values.encode_send.calls,
            values.encodes[0], values.encodes[1], values.encodes[2],
            values.encode_send.average_ms(), milliseconds(values.encode_send.max), self.capture_gdi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_at_most_once_per_second_without_losing_pending_samples() {
        let started = Instant::now();
        let mut metrics = Metrics::new(started);
        metrics.captured(Duration::from_millis(2), CaptureState::Ready);
        let spf = Duration::from_secs_f64(1.0 / 60.0);
        assert!(metrics
            .sample(started + Duration::from_millis(999), spf)
            .is_none());
        let report = metrics
            .sample(started + Duration::from_secs(1), spf)
            .unwrap();
        assert_eq!(report.values.outcomes[0], 1);
        assert_eq!(report.values.capture.calls, 1);
        assert!(metrics
            .sample(started + Duration::from_millis(1999), spf)
            .is_none());
        let empty = metrics
            .sample(started + Duration::from_secs(2), spf)
            .unwrap();
        assert_eq!(empty.values.capture.calls, 0);
        assert_eq!(empty.values.capture.average_ms(), 0.0);
    }

    #[test]
    fn distinguishes_capture_outcomes_and_encode_attempts_with_phase_durations() {
        let started = Instant::now();
        let mut metrics = Metrics::new(started);
        for state in [
            CaptureState::Ready,
            CaptureState::Empty,
            CaptureState::WouldBlock,
            CaptureState::Failed,
        ] {
            metrics.captured(Duration::from_millis(2), state);
        }
        metrics.converted(Duration::from_millis(3));
        for (elapsed, kind) in [
            (4, EncodeKind::Fresh),
            (8, EncodeKind::Replay),
            (12, EncodeKind::Repeat),
        ] {
            metrics.encoded(Duration::from_millis(elapsed), kind);
        }
        let report = metrics
            .sample(
                started + Duration::from_millis(1500),
                Duration::from_millis(16),
            )
            .unwrap();
        assert_eq!(report.values.outcomes, [1, 1, 1, 1]);
        assert_eq!(report.values.encodes, [1, 1, 1]);
        assert_eq!(report.values.encode_send.average_ms(), 8.0);
        assert_eq!(report.values.encode_send.max, Duration::from_millis(12));
        let line = report.to_string();
        assert!(line.contains("\"elapsed_ms\":1500.000"));
        assert!(line.contains("\"frame_count\":1"));
        assert!(line.contains("\"encode_calls\":3"));
        assert!(line.contains("\"convert_avg_ms\":3.000"));
        assert!(line.contains("\"capture_gdi\":2"));
        let report = report.with_capture_gdi(false);
        assert!(report.to_string().contains("\"capture_gdi\":0"));
        assert!(report
            .with_capture_gdi(true)
            .to_string()
            .contains("\"capture_gdi\":1"));
    }
}
