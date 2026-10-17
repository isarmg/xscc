//! Foreground, output-only ownership of the mobile platform audio session.
use hbb_common::ResultType;
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc,
};

#[cfg(target_os = "android")]
#[path = "audio/android.rs"]
mod platform;
#[cfg(target_os = "ios")]
#[path = "audio/ios.rs"]
mod platform;

#[derive(Clone, Copy)]
enum Failure {
    #[cfg(target_os = "android")]
    FocusLost = 1,
    #[cfg(target_os = "ios")]
    Interrupted = 2,
    RouteLost = 3,
    #[cfg(target_os = "ios")]
    MediaServicesLost = 4,
}

struct State {
    gate: Arc<AtomicBool>,
    failure: AtomicU8,
}

impl State {
    fn stop(&self, failure: Failure) {
        let _ =
            self.failure
                .compare_exchange(0, failure as u8, Ordering::AcqRel, Ordering::Acquire);
        self.gate.store(false, Ordering::Release);
    }

    fn error(&self) -> Option<String> {
        let message = match self.failure.load(Ordering::Acquire) {
            1 => "Audio focus was lost. Turn sound on again to resume.",
            2 => "Audio playback was interrupted. Turn sound on again to resume.",
            3 => "The audio output was disconnected. Turn sound on again to resume.",
            4 => "The system audio service stopped. Turn sound on again to resume.",
            _ => return None,
        };
        Some(message.to_owned())
    }
}

pub struct PlaybackSession {
    state: Arc<State>,
    _platform: platform::Session,
}

/// Call on the playback worker, and keep the result there until its stream is dropped.
/// The caller supplies a fresh generation's gate; platform events only close it.
pub fn activate(gate: Arc<AtomicBool>) -> ResultType<PlaybackSession> {
    if !gate.load(Ordering::Acquire) {
        hbb_common::bail!("Audio playback was cancelled.");
    }
    let state = Arc::new(State {
        gate,
        failure: AtomicU8::new(0),
    });
    match platform::Session::activate(state.clone()) {
        Ok(platform) => Ok(PlaybackSession {
            state,
            _platform: platform,
        }),
        Err(error) => {
            state.gate.store(false, Ordering::Release);
            Err(error)
        }
    }
}

impl PlaybackSession {
    pub fn allowed(&self) -> bool {
        self.state.gate.load(Ordering::Acquire)
    }

    pub fn error(&self) -> Option<String> {
        self.state.error()
    }
}

impl Drop for PlaybackSession {
    fn drop(&mut self) {
        self.state.gate.store(false, Ordering::Release);
    }
}
