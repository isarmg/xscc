pub fn accepts_display_update(
    current: Option<usize>,
    pending: Option<usize>,
    index: usize,
) -> bool {
    pending.is_none() && current == Some(index)
}

#[cfg(any(target_os = "android", test))]
pub fn stream_fps(remote_hz: u32, display_limit: Option<u32>) -> u32 {
    if cfg!(all(feature = "test-uncapped-stream", debug_assertions)) {
        // This only changes the upstream request. The video Surface keeps its own display limit.
        return remote_hz;
    }
    display_limit
        .filter(|limit| *limit > 0)
        .map_or(remote_hz, |limit| remote_hz.min(limit))
}

#[cfg(test)]
mod display_tests {
    use super::{accepts_display_update, stream_fps};

    #[cfg(not(all(feature = "test-uncapped-stream", debug_assertions)))]
    #[test]
    fn stream_cadence_respects_the_reported_local_display_without_changing_remote_hz() {
        for (remote, local, expected) in [
            (90, Some(120), 90),
            (120, Some(120), 120),
            (144, Some(120), 120),
            (240, Some(120), 120),
            (244, Some(120), 120),
            (240, Some(240), 240),
            (144, None, 144),
            (90, Some(0), 90),
            (0, Some(120), 0),
        ] {
            assert_eq!(stream_fps(remote, local), expected);
        }
    }

    #[cfg(all(feature = "test-uncapped-stream", debug_assertions))]
    #[test]
    fn uncapped_debug_stream_requests_the_remote_rate_on_a_120hz_phone() {
        assert_eq!(stream_fps(244, Some(120)), 244);
        assert_eq!(stream_fps(60, Some(120)), 60);
    }

    #[test]
    fn pending_and_stale_display_frames_and_metrics_are_rejected() {
        assert!(!accepts_display_update(None, None, 0));
        assert!(accepts_display_update(Some(0), None, 0));
        assert!(!accepts_display_update(Some(0), Some(1), 0));
        assert!(!accepts_display_update(Some(0), Some(1), 1));
        assert!(!accepts_display_update(Some(1), None, 0));
        assert!(accepts_display_update(Some(1), None, 1));
        assert!(!accepts_display_update(None, None, 1));
    }
}
