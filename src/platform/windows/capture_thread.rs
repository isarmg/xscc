use std::{marker::PhantomData, rc::Rc};
use windows::{
    core::w,
    Win32::{
        Foundation::HANDLE,
        System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW},
    },
};

pub(crate) struct CaptureThreadScheduling(HANDLE, PhantomData<Rc<()>>);

impl CaptureThreadScheduling {
    pub(crate) fn register() -> Option<Self> {
        let mut index = 0;
        match unsafe { AvSetMmThreadCharacteristicsW(w!("Capture"), &mut index) } {
            Ok(handle) => {
                #[cfg(debug_assertions)]
                hbb_common::log::info!("host_capture_scheduler mmcss_capture_registered");
                Some(Self(handle, PhantomData))
            }
            Err(_error) => {
                #[cfg(debug_assertions)]
                hbb_common::log::debug!("MMCSS capture registration failed: {_error}");
                None
            }
        }
    }
}

impl Drop for CaptureThreadScheduling {
    fn drop(&mut self) {
        let _result = unsafe { AvRevertMmThreadCharacteristics(self.0) };
        #[cfg(debug_assertions)]
        if let Err(error) = _result {
            hbb_common::log::debug!("MMCSS capture reversion failed: {error}");
        }
    }
}
