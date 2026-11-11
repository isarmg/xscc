#[cfg(any(all(windows, feature = "vram"), test))]
use std::time::Duration;

#[cfg(any(all(windows, feature = "vram"), test))]
pub(super) fn should_pace_capture(spf: Duration, texture: bool, would_block: bool) -> bool {
    !would_block || !texture || spf >= Duration::from_secs(1) / 60
}

#[cfg(any(all(windows, feature = "gpui-host"), test))]
pub(super) fn budget_start(
    fps: u32,
    iteration: std::time::Instant,
    capture: std::time::Instant,
) -> std::time::Instant {
    // High-refresh pacing must include checks performed before acquiring the next frame.
    if fps > 60 {
        iteration
    } else {
        capture
    }
}

#[cfg(all(windows, feature = "gpui-host"))]
pub(super) fn wait_until(start: std::time::Instant, frame: Duration) {
    let deadline = start + frame;
    let spin_guard = Duration::from_millis(2);
    if let Some(sleep_until) = deadline.checked_sub(spin_guard) {
        let remaining = sleep_until.saturating_duration_since(std::time::Instant::now());
        if !remaining.is_zero() {
            std::thread::sleep(remaining);
        }
    }
    while std::time::Instant::now() < deadline {
        std::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_refresh_checks_do_not_extend_the_requested_frame_period() {
        let iteration = std::time::Instant::now();
        let checks = Duration::from_millis(1);
        let capture = iteration + checks;
        for fps in [61, 90, 120, 240] {
            let spf = Duration::from_secs(1) / fps;
            let done = capture + spf / 2;
            let remaining =
                spf.saturating_sub(done.duration_since(budget_start(fps, iteration, capture)));
            assert_eq!((done + remaining).duration_since(iteration), spf);
        }
    }

    #[test]
    fn ordinary_frame_rates_keep_the_existing_capture_budget() {
        let iteration = std::time::Instant::now();
        let capture = iteration + Duration::from_millis(1);
        for fps in [1, 30, 60] {
            assert_eq!(budget_start(fps, iteration, capture), capture);
        }
    }

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
