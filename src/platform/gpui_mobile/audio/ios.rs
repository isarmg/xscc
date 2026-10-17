use super::{Failure, State};
use block::ConcreteBlock;
use hbb_common::{anyhow::anyhow, bail, log, ResultType};
use objc::{
    msg_send,
    rc::autoreleasepool,
    runtime::{Class, Object, BOOL, NO, YES},
    sel, sel_impl,
};
use std::{
    ffi::{c_char, CStr},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[link(name = "AVFAudio", kind = "framework")]
extern "C" {
    static AVAudioSessionCategoryPlayback: *mut Object;
    static AVAudioSessionModeDefault: *mut Object;
    static AVAudioSessionInterruptionNotification: *mut Object;
    static AVAudioSessionInterruptionTypeKey: *mut Object;
    static AVAudioSessionRouteChangeNotification: *mut Object;
    static AVAudioSessionRouteChangeReasonKey: *mut Object;
    static AVAudioSessionMediaServicesWereLostNotification: *mut Object;
    static AVAudioSessionMediaServicesWereResetNotification: *mut Object;
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {}

static OWNED: AtomicBool = AtomicBool::new(false);

struct Ownership;

impl Drop for Ownership {
    fn drop(&mut self) {
        OWNED.store(false, Ordering::Release);
    }
}

// The native handles stay on the worker that created them. Notification blocks
// retain their own Arc<State>, so concurrent notification delivery cannot borrow
// or outlive the Rust Session itself.
pub(super) struct Session {
    session: *mut Object,
    center: *mut Object,
    observers: Vec<*mut Object>,
    active: bool,
    _ownership: Ownership,
}

impl Session {
    pub(super) fn activate(state: Arc<State>) -> ResultType<Self> {
        if OWNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            bail!("Another remote session is already using audio playback.");
        }
        let ownership = Ownership;
        autoreleasepool(|| unsafe {
            let class = Class::get("AVAudioSession")
                .ok_or_else(|| anyhow!("The iOS audio session is unavailable."))?;
            let session: *mut Object = msg_send![class, sharedInstance];
            let center_class = Class::get("NSNotificationCenter")
                .ok_or_else(|| anyhow!("The iOS notification center is unavailable."))?;
            let center: *mut Object = msg_send![center_class, defaultCenter];
            if session.is_null() || center.is_null() {
                bail!("The iOS audio session is unavailable.");
            }
            let mut result = Self {
                session: msg_send![session, retain],
                center: msg_send![center, retain],
                observers: Vec::new(),
                active: false,
                _ownership: ownership,
            };
            result.observe(
                AVAudioSessionInterruptionNotification,
                Failure::Interrupted,
                state.clone(),
            )?;
            result.observe(
                AVAudioSessionRouteChangeNotification,
                Failure::RouteLost,
                state.clone(),
            )?;
            result.observe(
                AVAudioSessionMediaServicesWereLostNotification,
                Failure::MediaServicesLost,
                state.clone(),
            )?;
            result.observe(
                AVAudioSessionMediaServicesWereResetNotification,
                Failure::MediaServicesLost,
                state,
            )?;

            let mut error: *mut Object = ptr::null_mut();
            let configured: BOOL = msg_send![session,
                setCategory: AVAudioSessionCategoryPlayback
                mode: AVAudioSessionModeDefault
                options: 0usize
                error: &mut error
            ];
            if configured == NO {
                bail!("Cannot configure iOS playback: {}", error_message(error));
            }
            let activated: BOOL = msg_send![session, setActive: YES error: &mut error];
            if activated == NO {
                bail!("Cannot activate iOS playback: {}", error_message(error));
            }
            result.active = true;
            Ok(result)
        })
    }

    unsafe fn observe(
        &mut self,
        name: *mut Object,
        failure: Failure,
        state: Arc<State>,
    ) -> ResultType<()> {
        let block = ConcreteBlock::new(move |notification: *mut Object| {
            autoreleasepool(|| {
                let should_stop = match failure {
                    Failure::Interrupted => {
                        notification_value(notification, AVAudioSessionInterruptionTypeKey) == 1
                    }
                    Failure::RouteLost => matches!(
                        notification_value(notification, AVAudioSessionRouteChangeReasonKey),
                        2 | 7 // oldDeviceUnavailable or noSuitableRouteForCategory
                    ),
                    _ => true,
                };
                if should_stop {
                    state.stop(failure);
                }
            });
        })
        .copy();
        let block_ptr = &*block as *const _ as *const Object;
        // A nil queue delivers synchronously on the posting thread, closing the
        // callback's atomic gate without waiting for the UI or packet worker.
        let observer: *mut Object = msg_send![self.center,
            addObserverForName: name
            object: self.session
            queue: ptr::null_mut::<Object>()
            usingBlock: block_ptr
        ];
        if observer.is_null() {
            bail!("Cannot observe iOS playback interruptions.");
        }
        let observer: *mut Object = msg_send![observer, retain];
        self.observers.push(observer);
        Ok(())
    }
}

unsafe fn notification_value(notification: *mut Object, key: *mut Object) -> usize {
    let info: *mut Object = msg_send![notification, userInfo];
    let number: *mut Object = msg_send![info, objectForKey: key];
    msg_send![number, unsignedIntegerValue]
}

unsafe fn error_message(error: *mut Object) -> String {
    if !error.is_null() {
        let description: *mut Object = msg_send![error, localizedDescription];
        let bytes: *const c_char = msg_send![description, UTF8String];
        if !bytes.is_null() {
            return CStr::from_ptr(bytes).to_string_lossy().into_owned();
        }
    }
    "The system did not provide an error description.".to_owned()
}

impl Drop for Session {
    fn drop(&mut self) {
        autoreleasepool(|| unsafe {
            for observer in self.observers.drain(..) {
                let _: () = msg_send![self.center, removeObserver: observer];
                let _: () = msg_send![observer, release];
            }
            if self.active {
                let mut error: *mut Object = ptr::null_mut();
                // Notify other audio owners only after the caller dropped its stream.
                let deactivated: BOOL = msg_send![self.session,
                    setActive: NO
                    withOptions: 1usize
                    error: &mut error
                ];
                if deactivated == NO {
                    log::warn!("Cannot deactivate iOS playback: {}", error_message(error));
                }
            }
            let _: () = msg_send![self.center, release];
            let _: () = msg_send![self.session, release];
        });
    }
}
