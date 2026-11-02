use std::{
    fmt,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Pending(Option<usize>);

impl Pending {
    pub(super) fn write_slot(&self) -> usize {
        self.0.map_or(0, |slot| 1 - slot)
    }
    pub(super) fn peek(&self) -> Option<usize> {
        self.0
    }
    pub(super) fn submit(&mut self, slot: usize) -> Option<usize> {
        self.0.replace(slot)
    }
    pub(super) fn drain(&mut self) -> Option<usize> {
        self.0.take()
    }
    pub(super) fn clear(&mut self) {
        self.0 = None;
    }
}

#[derive(Default)]
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
    fn average(&self) -> f64 {
        if self.calls == 0 {
            0.0
        } else {
            self.total.as_secs_f64() * 1000.0 / self.calls as f64
        }
    }
}

pub(super) struct Metrics {
    started: Instant,
    acquire: Stage,
    copy: Stage,
    map: Stage,
}
impl Metrics {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            acquire: Stage::default(),
            copy: Stage::default(),
            map: Stage::default(),
        }
    }
    pub(super) fn acquire(&mut self, elapsed: Duration) {
        self.acquire.record(elapsed);
    }
    pub(super) fn copy(&mut self, elapsed: Duration) {
        self.copy.record(elapsed);
    }
    pub(super) fn map(&mut self, elapsed: Duration) {
        self.map.record(elapsed);
    }
    pub(super) fn sample(&mut self) -> Option<Sample> {
        let elapsed = self.started.elapsed();
        if elapsed < Duration::from_secs(1) {
            return None;
        }
        Some(Sample {
            elapsed,
            values: std::mem::replace(self, Self::new()),
        })
    }
}
pub(super) struct Sample {
    elapsed: Duration,
    values: Metrics,
}
impl fmt::Display for Sample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = &self.values;
        write!(
            f,
            concat!(
                "{{\"schema\":1,\"elapsed_ms\":{:.3},",
                "\"acquire_calls\":{},\"acquire_avg_ms\":{:.3},\"acquire_max_ms\":{:.3},",
                "\"copy_calls\":{},\"copy_avg_ms\":{:.3},\"copy_max_ms\":{:.3},",
                "\"map_calls\":{},\"map_avg_ms\":{:.3},\"map_max_ms\":{:.3}}}"
            ),
            self.elapsed.as_secs_f64() * 1000.0,
            v.acquire.calls,
            v.acquire.average(),
            v.acquire.max.as_secs_f64() * 1000.0,
            v.copy.calls,
            v.copy.average(),
            v.copy.max.as_secs_f64() * 1000.0,
            v.map.calls,
            v.map.average(),
            v.map.max.as_secs_f64() * 1000.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_frames_are_read_once_in_order_with_only_two_buffers() {
        let mut pending = Pending::default();
        let mut buffers = [0, 0];
        let mut shown = Vec::new();
        for frame in 1..=120 {
            let slot = pending.write_slot();
            buffers[slot] = frame;
            if let Some(ready) = pending.submit(slot) {
                shown.push(buffers[ready]);
            }
        }
        if let Some(last) = pending.drain() {
            shown.push(buffers[last]);
        }
        assert_eq!(shown, (1..=120).collect::<Vec<_>>());
        assert_eq!(pending.drain(), None);
    }

    #[test]
    fn a_static_first_frame_and_final_update_are_not_lost() {
        let mut pending = Pending::default();
        let mut buffers = [0, 0];
        for frame in [7, 9] {
            let slot = pending.write_slot();
            buffers[slot] = frame;
            assert_eq!(pending.submit(slot), None);
            assert_eq!(buffers[pending.drain().unwrap()], frame);
        }
    }

    #[test]
    fn a_new_surface_discards_pending_pixels_from_the_old_layout() {
        let mut pending = Pending::default();
        let mut buffers = [0, 0];
        buffers[pending.write_slot()] = 7;
        pending.submit(pending.write_slot());
        pending.clear();
        let slot = pending.write_slot();
        buffers[slot] = 9;
        assert_eq!(pending.submit(slot), None);
        assert_eq!(buffers[pending.drain().unwrap()], 9);
    }
}
