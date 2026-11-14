use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct IdleRefresh(Option<Instant>);

impl IdleRefresh {
    pub fn captured(&mut self, now: Instant) {
        self.0 = Some(now + Duration::from_millis(750));
    }

    pub fn due(&mut self, now: Instant) -> bool {
        if self.0.is_some_and(|deadline| now >= deadline) {
            self.0 = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_desktop_gets_one_refinement_after_motion_stops() {
        let start = Instant::now();
        let mut refresh = IdleRefresh::default();
        assert!(!refresh.due(start));
        for frame in 0..120 {
            let now = start + Duration::from_millis(frame * 8);
            refresh.captured(now);
            assert!(!refresh.due(now));
        }
        assert!(!refresh.due(start + Duration::from_millis(1600)));
        assert!(refresh.due(start + Duration::from_millis(1800)));
        assert!(!refresh.due(start + Duration::from_secs(30)));
        refresh.captured(start + Duration::from_secs(31));
        assert!(refresh.due(start + Duration::from_secs(32)));
    }
}
