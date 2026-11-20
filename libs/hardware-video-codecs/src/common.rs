#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use serde_derive::{Deserialize, Serialize};
include!(concat!(env!("OUT_DIR"), "/common_ffi.rs"));

pub(crate) const DATA_H264_720P: &[u8] = include_bytes!("res/720p.h264");
pub(crate) const DATA_H265_720P: &[u8] = include_bytes!("res/720p.h265");

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum Driver {
    NV,
    AMF,
    MFX,
    FFMPEG,
}

#[cfg(any(windows, target_os = "linux"))]
pub(crate) fn supported_gpu(_encode: bool) -> (bool, bool, bool) {
    #[cfg(all(windows, feature = "vram"))]
    unsafe {
        return (
            _encode && crate::vram::nv::nv_encode_driver_support() == 0
                || !_encode && crate::vram::nv::nv_decode_driver_support() == 0,
            crate::vram::amf::amf_driver_support() == 0,
            crate::vram::mfx::mfx_driver_support() == 0,
        );
    }
    #[cfg(all(windows, not(feature = "vram")))]
    return (true, true, true);
    #[cfg(target_os = "linux")]
    linux::supported_gpu(_encode)
}

#[cfg(target_os = "macos")]
pub(crate) fn get_video_toolbox_codec_support() -> (bool, bool, bool, bool) {
    macos::codec_support()
}

pub fn get_gpu_signature() -> u64 {
    #[cfg(windows)]
    {
        windows::gpu_signature()
    }
    #[cfg(target_os = "macos")]
    {
        let (avc_encoder, hevc_encoder, avc_decoder, hevc_decoder) = macos::codec_support();
        u64::from(u32::from_be_bytes([
            avc_encoder.into(),
            hevc_encoder.into(),
            avc_decoder.into(),
            hevc_decoder.into(),
        ]))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        0
    }
}

// called by child process
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn setup_parent_death_signal() {
    use std::sync::Once;

    static INIT: Once = Once::new();

    INIT.call_once(|| {
        #[cfg(target_os = "linux")]
        let result = linux::monitor_parent();
        #[cfg(target_os = "macos")]
        let result = macos::monitor_parent();
        if let Err(error) = result {
            log::warn!("Failed to monitor codec probe parent: {error}");
        }
    });
}

// called by parent process
#[cfg(windows)]
pub fn child_exit_when_parent_exit(child_process_id: u32) -> bool {
    match windows::assign_child_to_job(child_process_id) {
        Ok(()) => true,
        Err(error) => {
            log::error!("Failed to assign codec probe {child_process_id} to its job: {error}");
            false
        }
    }
}
