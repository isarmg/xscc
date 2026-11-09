// A failure keeps only owners which already existed before the sticky gate.
// No retry loop, detached thread, growing registry, driver reset, or fake idle ack.
use super::{capture_lifecycle, CapturerInfo, Raii};
use hbb_common::log;
use scrap::codec::Encoder;
use std::{
    cell::Cell,
    ffi::c_void,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    panic::{catch_unwind, AssertUnwindSafe},
    time::Duration,
};

pub(super) struct CaptureOwner {
    capturer: ManuallyDrop<CapturerInfo>,
    raii: ManuallyDrop<Raii>,
    guard: ManuallyDrop<capture_lifecycle::CaptureGuard<'static>>,
    retirement_failed: Cell<bool>,
}

impl CaptureOwner {
    pub(super) fn new(
        capturer: CapturerInfo,
        raii: Raii,
        guard: capture_lifecycle::CaptureGuard<'static>,
    ) -> Self {
        Self {
            capturer: ManuallyDrop::new(capturer),
            raii: ManuallyDrop::new(raii),
            guard: ManuallyDrop::new(guard),
            retirement_failed: Cell::new(false),
        }
    }

    pub(super) fn parts(&mut self) -> (&mut CapturerInfo, &mut Raii, &Cell<bool>) {
        (&mut self.capturer, &mut self.raii, &self.retirement_failed)
    }
}

impl Drop for CaptureOwner {
    fn drop(&mut self) {
        if self.retirement_failed.get() {
            // The encoder owner already latched the global gate before returning.
            // Keep the same capture, its registration, and encoder-selection RAII;
            // allowing their destructors to run would turn a timeout into a fake
            // idle proof (including during unwinding). Host restart is required.
            log::error!("video_gpu_retirement retained_capture=true restart_required=true");
            return;
        }
        unsafe {
            ManuallyDrop::drop(&mut self.capturer);
            ManuallyDrop::drop(&mut self.raii);
            ManuallyDrop::drop(&mut self.guard);
        }
    }
}

pub(super) struct EncoderOwner<'a> {
    encoder: ManuallyDrop<Encoder>,
    capture_device: Option<*mut c_void>,
    retirement_failed: &'a Cell<bool>,
}

impl<'a> EncoderOwner<'a> {
    pub(super) fn new(
        encoder: Encoder,
        capture_device: Option<*mut c_void>,
        retirement_failed: &'a Cell<bool>,
    ) -> Self {
        Self {
            encoder: ManuallyDrop::new(encoder),
            capture_device,
            retirement_failed,
        }
    }
}

impl Deref for EncoderOwner<'_> {
    type Target = Encoder;
    fn deref(&self) -> &Self::Target {
        &self.encoder
    }
}

impl DerefMut for EncoderOwner<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.encoder
    }
}

impl Drop for EncoderOwner<'_> {
    fn drop(&mut self) {
        let supported = self.encoder.retirement_supported() && self.capture_device.is_some();
        if !supported && !capture_lifecycle::change_pending() {
            // Unknown legacy backends keep ordinary disconnect/reconfiguration,
            // but a zero capture count afterward is not a GPU-idle proof. Keep
            // new remote sessions working and deny later topology until restart
            // (or a future implementation supplies this backend's full contract).
            capture_lifecycle::record_unverified_retirement();
            log::warn!("video_gpu_retirement supported=false scope=legacy unverified_history=true");
            unsafe { ManuallyDrop::drop(&mut self.encoder) };
            return;
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            let device = self.capture_device.ok_or_else(|| {
                hbb_common::anyhow::anyhow!(
                    "GPU capture device is unknown; retirement cannot be verified"
                )
            })?;
            self.encoder
                .retire_for_display_change(device, Duration::from_millis(1500))
        }));
        match result {
            Ok(Ok(())) => {
                log::info!("video_gpu_retirement success=true");
                unsafe { ManuallyDrop::drop(&mut self.encoder) };
            }
            error => {
                // Retain the already allocated owner on failure. The sticky gate
                // rejects new capture generations, bounding retention by owners
                // registered when the fault occurred. There is no retry/parking.
                self.retirement_failed.set(true);
                capture_lifecycle::record_retirement_failure();
                match error {
                    Ok(Err(error)) => {
                        log::error!("video_gpu_retirement success=false error={error:#}")
                    }
                    _ => log::error!("video_gpu_retirement success=false panic=true"),
                }
            }
        }
    }
}
