#[cfg(any(all(windows, any(feature = "vram", feature = "gpui-host")), test))]
use std::time::Duration;

#[cfg(any(all(windows, feature = "vram"), test))]
pub(super) fn should_pace_capture(spf: Duration, texture: bool, would_block: bool) -> bool {
    !would_block || !texture || (!cfg!(feature = "gpui-host") && spf >= Duration::from_secs(1) / 60)
}

#[cfg(all(windows, feature = "gpui-host"))]
pub(super) struct FramePacer {
    timer: Option<std::os::windows::io::OwnedHandle>,
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
        Self { timer }
    }

    pub(super) fn wait_until(&mut self, start: std::time::Instant, frame: Duration) {
        use std::os::windows::io::AsRawHandle;
        use winapi::{
            shared::ntdef::LARGE_INTEGER,
            um::{
                synchapi::{SetWaitableTimer, WaitForSingleObject},
                winbase::{WAIT_FAILED, WAIT_OBJECT_0},
            },
        };

        let remaining = frame.saturating_sub(start.elapsed());
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
                while start.elapsed() < frame {
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
        std::thread::sleep(frame.saturating_sub(start.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(windows, feature = "gpui-host"))]
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
