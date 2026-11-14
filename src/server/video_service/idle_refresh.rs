use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct IdleRefresh {
    last_capture: Option<Instant>,
    refined: bool,
}

impl IdleRefresh {
    pub fn captured(&mut self, now: Instant) {
        self.last_capture = Some(now);
        self.refined = false;
    }

    pub fn is_idle(&self, now: Instant) -> bool {
        self.last_capture.is_some_and(|captured| {
            now.saturating_duration_since(captured) >= Duration::from_millis(750)
        })
    }

    pub fn due(&mut self, now: Instant) -> bool {
        if !self.refined && self.is_idle(now) {
            self.refined = true;
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
