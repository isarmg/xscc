use android_activity::{AndroidApp, AndroidAppWaker};
use std::{
    cell::RefCell,
    ffi::c_void,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};

static NEXT_TOKEN: AtomicUsize = AtomicUsize::new(1);

thread_local! {
    static CALLBACK: RefCell<Option<Registration>> = const { RefCell::new(None) };
}

struct Registration {
    token: usize,
    signal: Weak<Signal>,
}

#[derive(Default)]
enum FrameState {
    #[default]
    Idle,
    Pending(usize),
    Ready,
    Failed(Option<&'static str>),
}

struct Signal {
    state: Mutex<FrameState>,
    waker: AndroidAppWaker,
}

pub(crate) struct FrameClock {
    app: AndroidApp,
    signal: Arc<Signal>,
}

impl FrameClock {
    pub(crate) fn new(app: &AndroidApp) -> Self {
        Self {
            app: app.clone(),
            signal: Arc::new(Signal {
                state: Mutex::new(FrameState::Idle),
                waker: app.create_waker(),
            }),
        }
    }

    pub(crate) fn request(&self) {
        let token = {
            let mut state = self.signal.state.lock().unwrap();
            if !matches!(*state, FrameState::Idle) {
                return;
            }
            let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
            *state = FrameState::Pending(token);
            token
        };
        let signal = Arc::downgrade(&self.signal);
        // android-activity 0.6.1 misreports native ALooper callback wakes as errors.
        // Java's existing Looper delivers vsync; drawing stays on android_main.
        self.app.run_on_java_main_thread(Box::new(move || {
            let Some(signal) = signal.upgrade() else {
                return;
            };
            if !matches!(*signal.state.lock().unwrap(), FrameState::Pending(id) if id == token) {
                return;
            }
            let choreographer = unsafe { ndk_sys::AChoreographer_getInstance() };
            if choreographer.is_null() {
                let failed = {
                    let mut state = signal.state.lock().unwrap();
                    if matches!(*state, FrameState::Pending(id) if id == token) {
                        *state = FrameState::Failed(Some("Android Choreographer is unavailable"));
                        true
                    } else {
                        false
                    }
                };
                if failed {
                    signal.waker.wake();
                }
                return;
            }
            CALLBACK.with(|slot| {
                slot.replace(Some(Registration {
                    token,
                    signal: Arc::downgrade(&signal),
                }));
            });
            // The NDK has no removeFrameCallback64. This is only an opaque token,
            // never a pointer to State or to callback-owned heap memory.
            unsafe {
                ndk_sys::AChoreographer_postFrameCallback64(
                    choreographer,
                    Some(on_frame),
                    token as *mut c_void,
                );
            }
        }));
    }

    pub(crate) fn take_ready(&self) -> anyhow::Result<bool> {
        let mut state = self.signal.state.lock().unwrap();
        match *state {
            FrameState::Ready => {
                *state = FrameState::Idle;
                Ok(true)
            }
            FrameState::Failed(ref mut error) => match error.take() {
                Some(error) => anyhow::bail!(error),
                None => Ok(false),
            },
            FrameState::Idle | FrameState::Pending(_) => Ok(false),
        }
    }

    pub(crate) fn cancel(&self) {
        let previous = {
            let mut state = self.signal.state.lock().unwrap();
            if matches!(*state, FrameState::Failed(_)) {
                return;
            }
            std::mem::take(&mut *state)
        };
        let FrameState::Pending(token) = previous else {
            return;
        };
        self.app.run_on_java_main_thread(Box::new(move || {
            CALLBACK.with(|slot| {
                let mut slot = slot.borrow_mut();
                if slot.as_ref().is_some_and(|entry| entry.token == token) {
                    slot.take();
                }
            });
        }));
    }
}

impl Drop for FrameClock {
    fn drop(&mut self) {
        self.cancel();
    }
}

unsafe extern "C" fn on_frame(_: i64, data: *mut c_void) {
    let token = data as usize;
    let registration = CALLBACK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|entry| entry.token == token) {
            slot.take()
        } else {
            None
        }
    });
    let Some(signal) = registration.and_then(|entry| entry.signal.upgrade()) else {
        return;
    };
    let ready = {
        let mut state = signal.state.lock().unwrap();
        if matches!(*state, FrameState::Pending(id) if id == token) {
            *state = FrameState::Ready;
            true
        } else {
            false
        }
    };
    if ready {
        signal.waker.wake();
    }
}
