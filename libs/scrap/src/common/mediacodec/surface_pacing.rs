#[derive(Default)]
pub(crate) struct Pacing {
    last: Option<(i64, i64)>,
}

impl Pacing {
    pub(crate) fn deadline(&mut self, now: i64, fps: i32, display_limit: i32) -> Option<i64> {
        let fps = i64::from(fps.min(display_limit));
        let interval = (1_000_000_000 + fps - 1) / fps;
        // Give SurfaceFlinger a full refresh period of notice and keep bursts inside
        // distinct presentation slots. Rebase when the stream starves or changes rate.
        let deadline = match self.last {
            Some((last, period)) if period == interval && last >= now => last + interval,
            Some((last, _)) if last >= now => (last + interval).max(now + 2 * interval),
            _ => now + 2 * interval,
        };
        // Real transports can deliver four or more outputs together. Keep extra
        // room for a burst without changing the regular two-refresh lead.
        let horizon = 3 * interval + (2 * interval).min(16_666_668);
        if deadline > now + horizon {
            return None;
        }
        self.last = Some((deadline, interval));
        Some(deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_are_presented_on_distinct_refreshes() {
        let mut pacing = Pacing::default();
        let period = 8_333_334;
        let mut deadlines = Vec::new();
        for burst in 0..60 {
            let now = burst * period * 4;
            for _ in 0..4 {
                deadlines.push(pacing.deadline(now, 120, 120).unwrap());
            }
        }
        assert_eq!(deadlines.len(), 240);
        assert!(deadlines.windows(2).all(|pair| pair[1] - pair[0] == period));
    }

    #[test]
    fn caps_buffering_and_restarts_after_starvation_or_rate_change() {
        let mut pacing = Pacing::default();
        assert_eq!(pacing.deadline(0, 120, 120), Some(16_666_668));
        assert_eq!(pacing.deadline(0, 120, 120), Some(25_000_002));
        assert_eq!(pacing.deadline(0, 120, 120), Some(33_333_336));
        assert_eq!(pacing.deadline(0, 120, 120), Some(41_666_670));
        assert_eq!(pacing.deadline(0, 120, 120), None);
        assert_eq!(
            pacing.deadline(1_000_000_000, 120, 120),
            Some(1_016_666_668)
        );
        assert_eq!(pacing.deadline(1_000_000_000, 90, 120), Some(1_027_777_780));
    }

    #[test]
    fn increasing_rate_keeps_already_submitted_frames_in_order() {
        let mut pacing = Pacing::default();
        assert_eq!(pacing.deadline(0, 90, 120), Some(22_222_224));
        assert_eq!(pacing.deadline(0, 90, 120), Some(33_333_336));
        assert_eq!(pacing.deadline(1_000_000, 120, 120), Some(41_666_670));
    }

    #[test]
    fn a_faster_source_uses_the_display_refresh_interval() {
        let mut pacing = Pacing::default();
        assert_eq!(pacing.deadline(0, 240, 120), Some(16_666_668));
        assert_eq!(pacing.deadline(0, 240, 120), Some(25_000_002));
    }
}
