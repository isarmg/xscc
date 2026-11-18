use hbb_common::tokio;
use std::time::Instant;

pub(super) struct SendTimer {
    #[cfg(windows)]
    windows: Option<windows::Timer>,
}

impl SendTimer {
    pub(super) fn new() -> Self {
        Self {
            #[cfg(windows)]
            windows: match windows::Timer::new() {
                Ok(timer) => Some(timer),
                Err(error) => {
                    hbb_common::throttled_log!(
                        std::time::Duration::from_secs(5),
                        warn,
                        "High-resolution UDP timer unavailable: {error}"
                    );
                    None
                }
            },
        }
    }

    pub(super) async fn wait_until(&mut self, deadline: Instant) {
        if deadline <= Instant::now() {
            return;
        }
        #[cfg(windows)]
        if let Some(timer) = &mut self.windows {
            match timer.wait_until(deadline).await {
                Ok(()) => return,
                Err(error) => {
                    hbb_common::throttled_log!(
                        std::time::Duration::from_secs(5),
                        warn,
                        "High-resolution UDP timer failed: {error}"
                    );
                    self.windows = None;
                }
            }
        }
        tokio::time::sleep_until(deadline.into()).await;
    }
}

#[cfg(windows)]
mod windows {
    use hbb_common::tokio::sync::Notify;
    use std::{
        io,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
        sync::Arc,
        time::{Duration, Instant},
    };
    use winapi::um::{
        synchapi::{CreateWaitableTimerExW, SetWaitableTimer},
        threadpoolapiset::{
            CloseThreadpoolWait, CreateThreadpoolWait, SetThreadpoolWait,
            WaitForThreadpoolWaitCallbacks,
        },
        winnt::{
            LARGE_INTEGER, PTP_CALLBACK_INSTANCE, PTP_WAIT, PVOID, SYNCHRONIZE, TIMER_MODIFY_STATE,
            TP_WAIT_RESULT,
        },
    };

    pub(super) struct Timer {
        handle: OwnedHandle,
        wait: PTP_WAIT,
        wake: Arc<Notify>,
        armed_for: Option<Instant>,
    }

    // Windows waits have no thread affinity. The callback only accesses the
    // thread-safe Notify, and Drop drains callbacks before freeing it.
    unsafe impl Send for Timer {}

    impl Timer {
        pub(super) fn new() -> io::Result<Self> {
            const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x2;
            let handle = unsafe {
                CreateWaitableTimerExW(
                    ptr::null_mut(),
                    ptr::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    SYNCHRONIZE | TIMER_MODIFY_STATE,
                )
            };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
            let wake = Arc::new(Notify::new());
            let context = Arc::as_ptr(&wake).cast_mut().cast();
            let wait = unsafe { CreateThreadpoolWait(Some(notify), context, ptr::null_mut()) };
            if wait.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                handle,
                wait,
                wake,
                armed_for: None,
            })
        }

        pub(super) async fn wait_until(&mut self, deadline: Instant) -> io::Result<()> {
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(());
                }
                if self.armed_for != Some(deadline) {
                    self.arm(remaining)?;
                    self.armed_for = Some(deadline);
                }
                self.wake.notified().await;
                // A previous select branch may have left a queued notification.
                self.armed_for = None;
            }
        }

        fn arm(&self, remaining: Duration) -> io::Result<()> {
            let mut due: LARGE_INTEGER = unsafe { std::mem::zeroed() };
            let ticks = remaining.as_nanos().div_ceil(100).min(i64::MAX as u128);
            let handle = self.handle.as_raw_handle().cast();
            unsafe {
                *due.QuadPart_mut() = -(ticks as i64);
                if SetWaitableTimer(handle, &due, 0, None, ptr::null_mut(), 0) == 0 {
                    return Err(io::Error::last_os_error());
                }
                SetThreadpoolWait(self.wait, handle, ptr::null_mut());
            }
            Ok(())
        }
    }

    impl Drop for Timer {
        fn drop(&mut self) {
            unsafe {
                SetThreadpoolWait(self.wait, ptr::null_mut(), ptr::null_mut());
                WaitForThreadpoolWaitCallbacks(self.wait, 1);
                CloseThreadpoolWait(self.wait);
            }
        }
    }

    unsafe extern "system" fn notify(
        _: PTP_CALLBACK_INSTANCE,
        context: PVOID,
        _: PTP_WAIT,
        _: TP_WAIT_RESULT,
    ) {
        unsafe { &*context.cast::<Notify>() }.notify_one();
    }

    #[cfg(test)]
    #[hbb_common::tokio::test(crate = "hbb_common::tokio")]
    async fn cancelled_udp_wait_cannot_wake_a_later_pacing_slot_early() {
        use std::{
            future::Future,
            task::{Context, Waker},
        };

        let mut timer = Timer::new().unwrap();
        {
            let mut cancelled =
                std::pin::pin!(timer.wait_until(Instant::now() + Duration::from_millis(10)));
            let mut context = Context::from_waker(Waker::noop());
            assert!(cancelled.as_mut().poll(&mut context).is_pending());
        }
        hbb_common::tokio::time::sleep(Duration::from_millis(30)).await;
        let deadline = Instant::now() + Duration::from_millis(30);
        timer.wait_until(deadline).await.unwrap();
        assert!(Instant::now() >= deadline);
    }
}
