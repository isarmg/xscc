#[cfg(windows)]
pub mod capture;
pub mod common;
pub mod ffmpeg;
#[cfg(any(target_arch = "aarch64", target_arch = "arm", feature = "rkmpp"))]
pub mod ffmpeg_hw;
pub mod ffmpeg_ram;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "aarch64", target_arch = "arm", feature = "rkmpp")
))]
pub mod rkmpp_dmabuf;
#[cfg(windows)]
mod windows_adapter;

#[no_mangle]
/// # Safety
/// A non-null `message` must point to a readable NUL-terminated string.
pub unsafe extern "C" fn hwcodec_log(level: i32, message: *const std::os::raw::c_char) {
    if message.is_null() {
        return;
    }
    let Ok(message) = (unsafe { std::ffi::CStr::from_ptr(message) }).to_str() else {
        return;
    };
    match level {
        0 => log::error!("{message}"),
        1 => log::warn!("{message}"),
        2 => log::info!("{message}"),
        3 => log::debug!("{message}"),
        4 => log::trace!("{message}"),
        _ => {}
    }
}
