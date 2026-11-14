use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct IdleRefresh {
    last_capture: Option<Instant>,
    refined: u8,
}

impl IdleRefresh {
    pub fn captured(&mut self, now: Instant) {
        self.last_capture = Some(now);
        self.refined = 0;
    }

    pub fn is_idle(&self, now: Instant) -> bool {
        self.last_capture.is_some_and(|captured| {
            now.saturating_duration_since(captured) >= Duration::from_millis(750)
        })
    }

    pub fn improve(&mut self) {
        self.refined = 0;
    }

    pub fn due(&mut self, now: Instant) -> bool {
        if self.refined < 8 && self.is_idle(now) {
            self.refined += 1;
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
    fn static_desktop_gets_bounded_progressive_refinement_after_motion_stops() {
        let start = Instant::now();
        let mut refresh = IdleRefresh::default();
        assert!(!refresh.due(start));
        for frame in 0..120 {
            let now = start + Duration::from_millis(frame * 8);
            refresh.captured(now);
            assert!(!refresh.due(now));
        }
        assert!(!refresh.due(start + Duration::from_millis(1600)));
        for frame in 0..8 {
            assert!(refresh.due(start + Duration::from_millis(1800 + frame * 16)));
        }
        assert!(!refresh.due(start + Duration::from_secs(30)));
        refresh.captured(start + Duration::from_secs(31));
        assert!(refresh.due(start + Duration::from_secs(32)));
    }
    #[test]
    fn improved_static_quality_gets_another_bounded_refinement_without_new_capture() {
        let start = Instant::now();
        let mut refresh = IdleRefresh::default();
        refresh.captured(start);
        let now = start + Duration::from_secs(1);
        for _ in 0..8 {
            assert!(refresh.due(now));
        }
        assert!(!refresh.due(now));
        refresh.improve();
        for _ in 0..8 {
            assert!(refresh.due(now));
        }
        assert!(!refresh.due(now));
        refresh.captured(now);
        assert!(!refresh.due(now));
    }
}
