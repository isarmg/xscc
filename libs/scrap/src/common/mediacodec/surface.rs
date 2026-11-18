use std::{
    ffi::c_void,
    sync::atomic::{AtomicU64, Ordering},
};

use hbb_common::{anyhow::Context, libc, ResultType};
use ndk::{
    media::media_codec::{MediaCodec, OutputBuffer},
    native_window::NativeWindow,
};

const INVALID: u64 = u64::MAX;
const CODEC_CONFIG: u32 = 2;

/// The owned output window for one Android Surface lifetime.
pub struct VideoSurface {
    pub(super) window: NativeWindow,
    size: AtomicU64,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl VideoSurface {
    /// # Safety
    /// The environment and local Surface reference must be valid on this thread.
    pub unsafe fn from_java(
        env: *mut c_void,
        surface: *mut c_void,
        wake: Box<dyn Fn() + Send + Sync>,
    ) -> Option<Self> {
        let window = unsafe { NativeWindow::from_surface(env.cast(), surface.cast()) }?;
        Some(Self {
            window,
            size: AtomicU64::new(0),
            wake,
        })
    }

    pub fn is_valid(&self) -> bool {
        self.size.load(Ordering::Acquire) != INVALID
    }

    pub fn size(&self) -> Option<(u32, u32)> {
        match self.size.load(Ordering::Acquire) {
            0 | INVALID => None,
            size => Some(((size >> 32) as u32, size as u32)),
        }
    }

    pub fn invalidate(&self) {
        if self.size.swap(INVALID, Ordering::AcqRel) != INVALID {
            (self.wake)();
        }
    }

    pub(super) fn render(
        &self,
        codec: &MediaCodec,
        buffer: OutputBuffer<'_>,
        (width, height): (i32, i32),
    ) -> ResultType<bool> {
        if !self.is_valid() || buffer.flags() & CODEC_CONFIG != 0 {
            codec.release_output_buffer(buffer, false)?;
            return Ok(false);
        }
        // Sender PTS is relative to its capture session, not Android's monotonic clock.
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 {
            let error = std::io::Error::last_os_error();
            codec.release_output_buffer(buffer, false)?;
            return Err(error).context("Read Android presentation clock");
        }
        codec.release_output_buffer_at_time(buffer, now.tv_sec * 1_000_000_000 + now.tv_nsec)?;
        let next = (width as u64) << 32 | height as u64;
        let previous = self.size.load(Ordering::Acquire);
        if previous != INVALID
            && previous != next
            && self
                .size
                .compare_exchange(previous, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            (self.wake)();
        }
        Ok(true)
    }
}
