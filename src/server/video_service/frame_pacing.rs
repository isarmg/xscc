#[cfg(any(feature = "gpui-host", all(windows, feature = "vram"), test))]
use std::time::Duration;
#[cfg(any(feature = "gpui-host", test))]
use std::time::Instant;

#[cfg(any(all(windows, feature = "vram"), test))]
pub(super) fn should_pace_capture(spf: Duration, texture: bool, would_block: bool) -> bool {
    !would_block || !texture || (!cfg!(feature = "gpui-host") && spf >= Duration::from_secs(1) / 60)
}

#[cfg(any(feature = "gpui-host", test))]
#[derive(Default)]
struct CaptureCadence {
    last_deadline: Option<(Instant, Duration)>,
}

#[cfg(any(feature = "gpui-host", test))]
impl CaptureCadence {
    fn deadline(&mut self, start: Instant, interval: Duration) -> Instant {
        if interval >= Duration::from_secs(1) / 60 {
            self.last_deadline = None;
            return start + interval;
        }
        // Recover small scheduling overruns without bunching frames after a stall.
        let anchor = self
            .last_deadline
            .filter(|&(deadline, previous_interval)| {
                previous_interval == interval
                    && start
                        .checked_duration_since(deadline)
                        .is_some_and(|late| late <= interval / 2)
            })
            .map_or(start, |(deadline, _)| deadline);
        let deadline = anchor + interval;
        self.last_deadline = Some((deadline, interval));
        deadline
    }
}

#[cfg(feature = "gpui-host")]
pub(super) struct FramePacer {
    #[cfg(windows)]
    timer: Option<std::os::windows::io::OwnedHandle>,
    cadence: CaptureCadence,
}

#[cfg(all(not(windows), feature = "gpui-host"))]
impl FramePacer {
    pub(super) fn new() -> Self {
        Self {
            cadence: CaptureCadence::default(),
        }
    }

    pub(super) fn wait_until(&mut self, start: Instant, frame: Duration) {
        let deadline = self.cadence.deadline(start, frame);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            std::thread::sleep(remaining);
        }
    }
}

#[cfg(all(windows, feature = "gpui-host"))]
impl FramePacer {
    pub(super) fn new() -> Self {
        use std::os::windows::io::FromRawHandle;
        use winapi::um::{
            synchapi::CreateWaitableTimerExW,
            winnt::{SYNCHRONIZE, TIMER_MODIFY_STATE},
        };

        // winapi 0.3 predates CREATE_WAITABLE_TIMER_HIGH_RESOLUTION (0x2).
        let handle = unsafe {
            CreateWaitableTimerExW(
                std::ptr::null_mut(),
                std::ptr::null(),
                0x2,
                TIMER_MODIFY_STATE | SYNCHRONIZE,
            )
        };
        let timer = if handle.is_null() {
            hbb_common::throttled_log!(
                Duration::from_secs(5),
                warn,
                "High-resolution frame timer unavailable; using thread sleep: {}",
                std::io::Error::last_os_error()
            );
            None
        } else {
            Some(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle.cast()) })
        };
        Self {
            timer,
            cadence: CaptureCadence::default(),
        }
    }

    pub(super) fn wait_until(&mut self, start: Instant, frame: Duration) {
        use std::os::windows::io::AsRawHandle;
        use winapi::{
            shared::ntdef::LARGE_INTEGER,
            um::{
                synchapi::{SetWaitableTimer, WaitForSingleObject},
                winbase::{WAIT_FAILED, WAIT_OBJECT_0},
            },
        };

        let deadline = self.cadence.deadline(start, frame);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        if let Some(timer) = &self.timer {
            // Leave only a short tail for scheduler jitter, not the former 2 ms spin.
            let sleep = remaining.saturating_sub(Duration::from_micros(500));
            let result = if sleep.is_zero() {
                WAIT_OBJECT_0
            } else {
                let mut due: LARGE_INTEGER = unsafe { std::mem::zeroed() };
                unsafe { *due.QuadPart_mut() = -(sleep.as_nanos().div_ceil(100) as i64) };
                unsafe {
                    if SetWaitableTimer(
                        timer.as_raw_handle().cast(),
                        &due,
                        0,
                        None,
                        std::ptr::null_mut(),
                        0,
                    ) == 0
                    {
                        WAIT_FAILED
                    } else {
                        WaitForSingleObject(
                            timer.as_raw_handle().cast(),
                            remaining.as_millis() as u32 + 100,
                        )
                    }
                }
            };
            if result == WAIT_OBJECT_0 {
                while Instant::now() < deadline {
                    std::hint::spin_loop();
                }
                return;
            }
            let error = if result == WAIT_FAILED {
                std::io::Error::last_os_error()
            } else {
                std::io::Error::other(format!("Frame timer wait returned {result}"))
            };
            hbb_common::throttled_log!(
                Duration::from_secs(5),
                warn,
                "High-resolution frame timer failed; using thread sleep: {error}"
            );
            self.timer = None;
        }
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "gpui-host")]
    #[test]
    fn frame_timer_waits_for_the_remaining_budget() {
        let mut pacer = FramePacer::new();
        let start = std::time::Instant::now();
        let frame = Duration::from_millis(2);
        pacer.wait_until(start, frame);
        assert!(start.elapsed() >= frame);
        pacer.wait_until(start, frame);
    }

    #[test]
    fn short_overruns_do_not_lower_the_sustained_high_refresh_rate() {
        let mut cadence = CaptureCadence::default();
        let interval = Duration::from_secs(1) / 120;
        let start = Instant::now();
        let mut next = start;
        let mut frames = 0;
        while next - start < Duration::from_secs(1) {
            let deadline = cadence.deadline(next, interval);
            let work = Duration::from_millis(if frames % 2 == 0 { 10 } else { 6 });
            next = deadline.max(next + work);
            frames += 1;
        }
        assert!((119..=121).contains(&frames), "paced {frames} captures");
    }

    #[test]
    fn rate_changes_and_long_stalls_start_a_fresh_interval() {
        let mut cadence = CaptureCadence::default();
        let interval = Duration::from_secs(1) / 120;
        let start = Instant::now();
        let due = cadence.deadline(start, interval);
        let resumed = due + interval;
        assert_eq!(cadence.deadline(resumed, interval), resumed + interval);
        let new_interval = Duration::from_secs(1) / 90;
        let changed = resumed + interval + Duration::from_millis(1);
        assert_eq!(
            cadence.deadline(changed, new_interval),
            changed + new_interval
        );
    }

    #[test]
    fn cursor_only_updates_do_not_halve_requested_image_delivery() {
        for fps in [1, 30, 60, 90, 120, 240] {
            if fps <= 60 && !cfg!(feature = "gpui-host") {
                continue;
            }
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
