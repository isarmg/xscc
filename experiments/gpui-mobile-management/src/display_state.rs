pub fn accepts_display_update(
    current: Option<usize>,
    pending: Option<usize>,
    index: usize,
) -> bool {
    pending.is_none() && current == Some(index)
}

#[cfg(test)]
mod display_tests {
    use super::accepts_display_update;
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
