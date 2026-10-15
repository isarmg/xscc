use std::time::Duration;

pub(super) fn should_present(
    enabled: bool,
    queued: usize,
    since_presented: Option<Duration>,
    frame_interval: Duration,
) -> bool {
    // Decode every reference frame; skip only redundant submissions within a display interval.
    !enabled || queued < 2 || since_presented.map_or(true, |elapsed| elapsed >= frame_interval)
}

#[cfg(test)]
mod tests {
    use super::should_present;
    use std::time::Duration;

    #[test]
    fn fast_backlog_coalesces_until_next_display_interval() {
        let interval = Duration::from_millis(16);
        assert!(should_present(true, 120, None, interval));
        assert!(!should_present(
            true,
            120,
            Some(Duration::from_millis(5)),
            interval
        ));
        assert!(should_present(true, 120, Some(interval), interval));
    }

    #[test]
    fn slow_decode_always_presents_despite_backlog() {
        assert!(should_present(
            true,
            120,
            Some(Duration::from_millis(100)),
            Duration::from_millis(16),
        ));
    }

    #[test]
    fn drained_queue_or_disabled_mode_presents_immediately() {
        let interval = Duration::from_millis(16);
        for queued in 0..2 {
            assert!(should_present(true, queued, Some(Duration::ZERO), interval));
        }
        assert!(should_present(false, 120, Some(Duration::ZERO), interval));
    }
}
