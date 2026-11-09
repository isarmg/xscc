use std::{
    sync::{Condvar, Mutex, MutexGuard},
    time::Duration,
};

const RETIREMENT_FAILED: &str =
    "GPU retirement was not confirmed; restart the host before reconnecting or changing displays";

const RETIREMENT_UNVERIFIED: &str =
    "A previous GPU retirement was not verified; restart the host before changing displays";

struct State {
    captures: usize,
    cleanup: bool,
    retirement_failed: bool,
    retirement_unverified: bool,
}

struct CaptureLifecycle {
    state: Mutex<State>,
    changed: Condvar,
}

impl CaptureLifecycle {
    const fn new() -> Self {
        Self {
            state: Mutex::new(State {
                captures: 0,
                cleanup: false,
                retirement_failed: false,
                retirement_unverified: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn capture(&self) -> Result<CaptureGuard<'_>, &'static str> {
        let mut state = self
            .changed
            .wait_while(self.state.lock().unwrap(), |state| {
                state.cleanup && !state.retirement_failed
            })
            .unwrap();
        if state.retirement_failed {
            return Err(RETIREMENT_FAILED);
        }
        state.captures += 1;
        Ok(CaptureGuard(self))
    }

    fn retirement_failed(&self) {
        self.state.lock().unwrap().retirement_failed = true;
        self.changed.notify_all();
    }

    fn retirement_unverified(&self) {
        self.state.lock().unwrap().retirement_unverified = true;
        self.changed.notify_all();
    }

    fn idle(&self, timeout: Duration) -> Result<CaptureIdleGuard<'_>, &'static str> {
        let mut state = self.state.lock().unwrap();
        if state.retirement_failed {
            return Err(RETIREMENT_FAILED);
        }
        if state.retirement_unverified {
            return Err(RETIREMENT_UNVERIFIED);
        }
        if state.cleanup {
            return Err("Virtual display cleanup is already in progress");
        }
        state.cleanup = true;
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state.captures != 0 && !state.retirement_failed && !state.retirement_unverified
            })
            .unwrap();
        if state.retirement_failed {
            state.cleanup = false;
            self.changed.notify_all();
            return Err(RETIREMENT_FAILED);
        }
        if state.retirement_unverified {
            state.cleanup = false;
            self.changed.notify_all();
            return Err(RETIREMENT_UNVERIFIED);
        }
        if state.captures != 0 {
            state.cleanup = false;
            self.changed.notify_all();
            return Err(
                "Screen capture has not released its resources; virtual displays were preserved",
            );
        }
        Ok(CaptureIdleGuard(self))
    }

    fn change_pending(&self) -> bool {
        self.state.lock().unwrap().cleanup
    }

    fn connection(&self, timeout: Duration) -> Result<ConnectionStartGuard<'_>, &'static str> {
        let (state, _) = self
            .changed
            .wait_timeout_while(self.state.lock().unwrap(), timeout, |state| {
                state.cleanup && !state.retirement_failed
            })
            .unwrap();
        if state.retirement_failed {
            return Err(RETIREMENT_FAILED);
        }
        if state.cleanup {
            return Err(
                "Virtual display cleanup has not finished; please reconnect after it completes",
            );
        }
        Ok(ConnectionStartGuard { _state: state })
    }
}

#[cfg(windows)]
static CAPTURE_LIFECYCLE: CaptureLifecycle = CaptureLifecycle::new();

pub(crate) struct CaptureGuard<'a>(&'a CaptureLifecycle);
pub(crate) struct CaptureIdleGuard<'a>(&'a CaptureLifecycle);
pub(crate) struct ConnectionStartGuard<'a> {
    _state: MutexGuard<'a, State>,
}

impl Drop for CaptureGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.captures -= 1;
        self.0.changed.notify_all();
    }
}

impl Drop for CaptureIdleGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.cleanup = false;
        self.0.changed.notify_all();
    }
}

#[cfg(windows)]
pub(crate) fn change_pending() -> bool {
    CAPTURE_LIFECYCLE.change_pending()
}

#[cfg(windows)]
pub(crate) fn begin_capture() -> Result<CaptureGuard<'static>, &'static str> {
    CAPTURE_LIFECYCLE.capture()
}

#[cfg(windows)]
pub(super) fn record_retirement_failure() {
    CAPTURE_LIFECYCLE.retirement_failed();
}

#[cfg(windows)]
pub(super) fn record_unverified_retirement() {
    CAPTURE_LIFECYCLE.retirement_unverified();
}

#[cfg(windows)]
pub(crate) fn wait_for_capture_idle(
    timeout: Duration,
) -> Result<CaptureIdleGuard<'static>, &'static str> {
    CAPTURE_LIFECYCLE.idle(timeout)
}

#[cfg(windows)]
pub(crate) fn begin_connection(
    timeout: Duration,
) -> Result<ConnectionStartGuard<'static>, &'static str> {
    CAPTURE_LIFECYCLE.connection(timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{mpsc, Arc},
        thread,
    };

    #[test]
    fn cleanup_waits_for_capture_release_and_holds_new_registration_until_finished() {
        let lifecycle = Arc::new(CaptureLifecycle::new());
        let capture = lifecycle.capture().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (idle_tx, idle_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let cleanup = {
            let lifecycle = lifecycle.clone();
            thread::spawn(move || {
                started_tx.send(()).unwrap();
                let idle = lifecycle.idle(Duration::from_secs(2)).unwrap();
                idle_tx.send(()).unwrap();
                finish_rx.recv().unwrap();
                drop(idle);
            })
        };
        started_rx.recv().unwrap();
        assert!(idle_rx.recv_timeout(Duration::from_millis(30)).is_err());
        drop(capture);
        idle_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(lifecycle.change_pending());
        assert!(lifecycle.connection(Duration::from_millis(30)).is_err());
        finish_tx.send(()).unwrap();
        cleanup.join().unwrap();
        assert!(!lifecycle.change_pending());
        assert!(lifecycle.connection(Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn failed_gpu_retirement_is_sticky_and_never_grants_idle_or_a_new_owner() {
        let lifecycle = CaptureLifecycle::new();
        let capture = lifecycle.capture().unwrap();
        lifecycle.retirement_failed();
        assert!(lifecycle.capture().is_err());
        assert!(lifecycle.connection(Duration::ZERO).is_err());
        assert!(lifecycle.idle(Duration::ZERO).is_err());
        drop(capture);
        assert!(lifecycle.capture().is_err());
        assert!(lifecycle.connection(Duration::ZERO).is_err());
        assert!(lifecycle.idle(Duration::ZERO).is_err());
        assert!(!lifecycle.change_pending());
    }

    #[test]
    fn unverified_legacy_retirement_preserves_remote_access_but_never_grants_topology_idle() {
        let lifecycle = CaptureLifecycle::new();
        let capture = lifecycle.capture().unwrap();
        lifecycle.retirement_unverified();
        assert!(lifecycle.capture().is_ok());
        assert!(lifecycle.connection(Duration::ZERO).is_ok());
        assert!(lifecycle.idle(Duration::ZERO).is_err());
        drop(capture);
        assert!(lifecycle.capture().is_ok());
        assert!(lifecycle.connection(Duration::ZERO).is_ok());
        assert!(lifecycle.idle(Duration::ZERO).is_err());
        assert!(!lifecycle.change_pending());
    }

    #[test]
    fn cleanup_timeout_preserves_capture_and_allows_a_later_cleanup() {
        let lifecycle = CaptureLifecycle::new();
        let capture = lifecycle.capture().unwrap();
        assert!(lifecycle.idle(Duration::from_millis(20)).is_err());
        assert!(lifecycle.connection(Duration::from_millis(20)).is_ok());
        drop(capture);
        assert!(lifecycle.idle(Duration::from_millis(20)).is_ok());
    }
}
