use std::time::Duration;

pub fn poll_interval(target_fps: Option<u32>) -> Duration {
    match target_fps {
        Some(fps) => Duration::from_nanos(1_000_000_000 / u64::from(fps.clamp(30, 120))),
        None => Duration::from_millis(250),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_follows_requested_fps_and_remains_slow_when_idle() {
        for (fps, nanos) in [
            (30, 33_333_333),
            (60, 16_666_666),
            (90, 11_111_111),
            (120, 8_333_333),
        ] {
            assert_eq!(poll_interval(Some(fps)), Duration::from_nanos(nanos));
        }
        assert_eq!(poll_interval(None), Duration::from_millis(250));
    }
}
