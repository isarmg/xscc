use std::time::Duration;

pub(super) fn should_pace_capture(spf: Duration, texture: bool, would_block: bool) -> bool {
    !would_block || !texture || spf >= Duration::from_secs(1) / 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_only_updates_do_not_halve_high_refresh_delivery() {
        for fps in [90, 120, 240] {
            let spf = Duration::from_secs(1) / fps;
            let mut elapsed = Duration::ZERO;
            let mut frames = 0;
            let mut cursor_only = true;
            while elapsed < Duration::from_secs(1) {
                let started = elapsed;
                if !cursor_only {
                    frames += 1;
                    elapsed += Duration::from_millis(1);
                }
                if should_pace_capture(spf, true, cursor_only) {
                    elapsed = elapsed.max(started + spf);
                }
                cursor_only = !cursor_only;
            }
            assert!(
                (fps - 1..=fps + 1).contains(&frames),
                "{fps} FPS delivered {frames} frames with intervening cursor updates"
            );
        }
    }
}
