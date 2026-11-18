#[derive(Default)]
pub(crate) struct Pacing {
    last: Option<(i64, i64)>,
}

impl Pacing {
    pub(crate) fn deadline(&mut self, now: i64, fps: i32) -> Option<i64> {
        let fps = i64::from(fps);
        let interval = (1_000_000_000 + fps - 1) / fps;
        // Give SurfaceFlinger a full refresh period of notice and keep bursts inside
        // distinct presentation slots. Rebase when the stream starves or changes rate.
        let deadline = match self.last {
            Some((last, period)) if period == interval && last + period >= now + period => {
                last + period
            }
            _ => now + 2 * interval,
        };
        if deadline > now + 3 * interval {
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
        for burst in 0..120 {
            let now = burst * period * 2;
            deadlines.push(pacing.deadline(now, 120).unwrap());
            deadlines.push(pacing.deadline(now, 120).unwrap());
        }
        assert_eq!(deadlines.len(), 240);
        assert!(deadlines.windows(2).all(|pair| pair[1] - pair[0] == period));
    }

    #[test]
    fn caps_buffering_and_restarts_after_starvation_or_rate_change() {
        let mut pacing = Pacing::default();
        assert_eq!(pacing.deadline(0, 120), Some(16_666_668));
        assert_eq!(pacing.deadline(0, 120), Some(25_000_002));
        assert_eq!(pacing.deadline(0, 120), None);
        assert_eq!(pacing.deadline(1_000_000_000, 120), Some(1_016_666_668));
        assert_eq!(pacing.deadline(1_000_000_000, 90), Some(1_022_222_224));
    }
}
