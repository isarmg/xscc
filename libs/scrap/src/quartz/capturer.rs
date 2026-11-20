use std::{
    ptr,
    sync::{Arc, Mutex},
};

use block::{Block, ConcreteBlock};
use hbb_common::libc::c_void;

use super::config::Config;
use super::display::Display;
use super::ffi::*;
use super::frame::Frame;

pub struct Capturer {
    resources: Option<StreamResources>,
    width: usize,
    height: usize,
    format: PixelFormat,
    display: Display,
    state: Arc<Mutex<StreamState>>,
}

struct StreamResources {
    stream: CGDisplayStreamRef,
    queue: DispatchQueue,
}

// The owner can transfer these references to the stream's terminal callback.
unsafe impl Send for StreamResources {}

impl Drop for StreamResources {
    fn drop(&mut self) {
        unsafe {
            if !self.stream.is_null() {
                CFRelease(self.stream);
            }
            dispatch_release(self.queue);
        }
    }
}

#[derive(Default)]
struct StreamState {
    stopped: bool,
    pending_release: Option<StreamResources>,
}

impl Capturer {
    pub fn new<F: Fn(std::io::Result<Frame>) + Send + 'static>(
        display: Display,
        width: usize,
        height: usize,
        format: PixelFormat,
        config: Config,
        handler: F,
    ) -> Result<Capturer, CGError> {
        let state = Arc::new(Mutex::new(StreamState::default()));
        let callback_state = Arc::clone(&state);
        let handler: FrameAvailableHandler = ConcreteBlock::new(move |status, _, surface, _| {
            use self::CGDisplayStreamFrameStatus::*;
            if status == Stopped {
                let resources = {
                    let mut state = callback_state.lock().unwrap();
                    state.stopped = true;
                    state.pending_release.take()
                };
                drop(resources);
                return;
            }
            if status == FrameComplete && callback_state.lock().unwrap().pending_release.is_none() {
                handler(unsafe { Frame::new(surface) });
            }
        })
        .copy();

        let queue = unsafe {
            dispatch_queue_create(
                b"quadrupleslap.scrap\0".as_ptr() as *const i8,
                ptr::null_mut(),
            )
        };
        if queue.is_null() {
            return Err(CGError::Failure);
        }
        let mut resources = StreamResources {
            stream: ptr::null_mut(),
            queue,
        };

        resources.stream = unsafe {
            let config = config.build();
            let stream = CGDisplayStreamCreateWithDispatchQueue(
                display.id(),
                width,
                height,
                format,
                config,
                queue,
                &*handler as *const Block<_, _> as *const c_void,
            );
            CFRelease(config);
            stream
        };
        if resources.stream.is_null() {
            return Err(CGError::CannotComplete);
        }

        match unsafe { CGDisplayStreamStart(resources.stream) } {
            CGError::Success => Ok(Capturer {
                resources: Some(resources),
                width,
                height,
                format,
                display,
                state,
            }),
            x => Err(x),
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn format(&self) -> PixelFormat {
        self.format
    }
    pub fn display(&self) -> Display {
        self.display
    }

    pub fn is_stopped(&self) -> bool {
        self.state.lock().unwrap().stopped
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        let Some(resources) = self.resources.take() else {
            return;
        };
        if self.is_stopped() {
            return;
        }
        let result = unsafe { CGDisplayStreamStop(resources.stream) };
        let mut state = self.state.lock().unwrap();
        if !state.stopped {
            // CoreGraphics permits releasing the active stream only after Stopped.
            state.pending_release = Some(resources);
        }
        drop(state);
        if result != CGError::Success && result != CGError::InvalidOperation {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(10),
                warn,
                "Could not stop Quartz display stream: {:?}",
                result
            );
        }
    }
}
