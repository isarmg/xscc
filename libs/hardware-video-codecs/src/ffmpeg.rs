#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unused_imports)]

include!(concat!(env!("OUT_DIR"), "/ffmpeg_ffi.rs"));

#[allow(dead_code)]
pub(crate) mod native {
    include!(concat!(env!("OUT_DIR"), "/ffmpeg_native.rs"));
}

use serde_derive::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::{c_char, c_void, CStr},
    sync::{LazyLock, Mutex, Once, PoisonError},
};

#[derive(Debug, Eq, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum AVHWDeviceType {
    AV_HWDEVICE_TYPE_NONE,
    AV_HWDEVICE_TYPE_VDPAU,
    AV_HWDEVICE_TYPE_CUDA,
    AV_HWDEVICE_TYPE_VAAPI,
    AV_HWDEVICE_TYPE_DXVA2,
    AV_HWDEVICE_TYPE_QSV,
    AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
    AV_HWDEVICE_TYPE_D3D11VA,
    AV_HWDEVICE_TYPE_DRM,
    AV_HWDEVICE_TYPE_OPENCL,
    AV_HWDEVICE_TYPE_MEDIACODEC,
    AV_HWDEVICE_TYPE_VULKAN,
}

static HEVC_DECODES: LazyLock<Mutex<HashMap<usize, bool>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[no_mangle]
pub(crate) extern "C" fn hwcodec_watch_hevc_references(context: *const c_void) {
    HEVC_DECODES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(context as usize, false);
}

#[no_mangle]
pub(crate) extern "C" fn hwcodec_take_hevc_reference_error(context: *const c_void) -> bool {
    HEVC_DECODES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&(context as usize))
        .unwrap_or(false)
}

/// FFmpeg provides a NUL-terminated message for the duration of this callback.
///
/// # Safety
/// A non-null message pointer must refer to a readable C string.
#[no_mangle]
pub unsafe extern "C" fn hwcodec_av_log_callback(
    context: *const c_void,
    level: i32,
    message: *const c_char,
) {
    if message.is_null() {
        return;
    }
    let Ok(message) = (unsafe { CStr::from_ptr(message) }).to_str() else {
        return;
    };
    if level == AV_LOG_ERROR as i32 && message.contains("Could not find ref with POC") {
        if let Some(missing) = HEVC_DECODES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&(context as usize))
        {
            *missing = true;
        }
        log::trace!("{message}");
    } else if level <= AV_LOG_ERROR as i32 {
        log::error!("{message}");
    } else if level == AV_LOG_WARNING as i32 {
        log::warn!("{message}");
    } else if level == AV_LOG_INFO as i32 {
        log::info!("{message}");
    } else if level == AV_LOG_VERBOSE as i32 || level == AV_LOG_DEBUG as i32 {
        log::debug!("{message}");
    } else if level == AV_LOG_TRACE as i32 {
        log::trace!("{message}");
    }
}

pub(crate) fn init_av_log() {
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        av_log_set_level(AV_LOG_ERROR as i32);
        hwcodec_set_av_log_callback();
    });
}
