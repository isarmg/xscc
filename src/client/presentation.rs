use std::time::Duration;

// GPUI coalesces its bounded latest-frame mailbox when the window renders.
pub(super) const DECODE_TIME_PACING: bool = !cfg!(feature = "gpui-session");

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
    use super::{DECODE_TIME_PACING, should_present};
    use std::time::Duration;

    #[test]
    fn gpui_pacing_preserves_decoded_frames_with_jitter() {
        let interval = Duration::from_secs_f64(1.0 / 120.);
        let admitted = |decode_time_pacing| {
            let mut since_presented = Duration::ZERO;
            [7, 10, 7, 10].map(|milliseconds| {
                since_presented += Duration::from_millis(milliseconds);
                let present =
                    should_present(decode_time_pacing, 2, Some(since_presented), interval);
                if present {
                    since_presented = Duration::ZERO;
                }
                present
            })
        };
        assert_eq!(admitted(false), [true, true, true, true]);
        assert_eq!(admitted(true), [false, true, false, true]);
        #[cfg(feature = "gpui-session")]
        assert_eq!(admitted(DECODE_TIME_PACING), [true, true, true, true]);
        #[cfg(not(feature = "gpui-session"))]
        assert_eq!(admitted(DECODE_TIME_PACING), [false, true, false, true]);
    }

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
