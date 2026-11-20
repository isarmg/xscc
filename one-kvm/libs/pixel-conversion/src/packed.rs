#[cfg(not(windows))]
use super::ffi::RGB24ToNV12;
use super::{
    argb_size, dimensions, even_dimensions,
    ffi::{H444ToARGB, RAWToI420, RGB24ToI420, UYVYToI420, YUY2ToI420, YUY2ToNV12},
    i420_size, nv12_size,
    planar::i420_to_nv12,
    rgb24_size, yuyv_size, Result, YuvError,
};

/// Convert YUYV (YUY2) to I420 (YUV420P)
///
/// # Arguments
/// * `src` - Source YUYV data
/// * `dst` - Destination I420 buffer
/// * `width` - Frame width (must be even)
/// * `height` - Frame height (must be even)
pub fn yuy2_to_i420(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    if src.len() < yuyv_size(w, h) || dst.len() < i420_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(YUY2ToI420(
        src.as_ptr(),
        width * 2,
        dst.as_mut_ptr(),
        width,
        dst[y_size..].as_mut_ptr(),
        width / 2,
        dst[y_size + uv_size..].as_mut_ptr(),
        width / 2,
        width,
        height,
    ))
}

/// Convert YUYV (YUY2) to NV12 (optimal for VAAPI)
///
/// # Arguments
/// * `src` - Source YUYV data
/// * `dst` - Destination NV12 buffer
/// * `width` - Frame width (must be even)
/// * `height` - Frame height (must be even)
pub fn yuy2_to_nv12(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    let y_size = w * h;

    if src.len() < yuyv_size(w, h) || dst.len() < nv12_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(YUY2ToNV12(
        src.as_ptr(),
        width * 2,
        dst.as_mut_ptr(),
        width,
        dst[y_size..].as_mut_ptr(),
        width,
        width,
        height,
    ))
}

/// Convert UYVY to I420
pub fn uyvy_to_i420(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    if src.len() < yuyv_size(w, h) || dst.len() < i420_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(UYVYToI420(
        src.as_ptr(),
        width * 2,
        dst.as_mut_ptr(),
        width,
        dst[y_size..].as_mut_ptr(),
        width / 2,
        dst[y_size + uv_size..].as_mut_ptr(),
        width / 2,
        width,
        height,
    ))
}

/// Convert RGB24 to I420
/// Note: Uses RAWToI420 because libyuv's "RAW" is actually RGB order
pub fn rgb24_to_i420(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    if src.len() < rgb24_size(w, h) || dst.len() < i420_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    // libyuv RAW = RGB byte order in memory
    call_yuv!(RAWToI420(
        src.as_ptr(),
        width * 3,
        dst.as_mut_ptr(),
        width,
        dst[y_size..].as_mut_ptr(),
        width / 2,
        dst[y_size + uv_size..].as_mut_ptr(),
        width / 2,
        width,
        height,
    ))
}

/// Convert BGR24 to I420
/// Note: Uses RGB24ToI420 because libyuv's "RGB24" is actually BGR order
pub fn bgr24_to_i420(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    if src.len() < rgb24_size(w, h) || dst.len() < i420_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    // libyuv RGB24 = BGR byte order in memory
    call_yuv!(RGB24ToI420(
        src.as_ptr(),
        width * 3,
        dst.as_mut_ptr(),
        width,
        dst[y_size..].as_mut_ptr(),
        width / 2,
        dst[y_size + uv_size..].as_mut_ptr(),
        width / 2,
        width,
        height,
    ))
}

/// Convert H444 (BT.709 limited-range YUV444P) to BGRA.
pub fn h444_to_bgra(
    src_y: &[u8],
    src_u: &[u8],
    src_v: &[u8],
    dst: &mut [u8],
    width: i32,
    height: i32,
) -> Result<()> {
    let (w, h) = dimensions(width, height)?;
    let plane_size = w * h;

    if src_y.len() < plane_size || src_u.len() < plane_size || src_v.len() < plane_size {
        return Err(YuvError::BufferTooSmall);
    }

    if dst.len() < argb_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(H444ToARGB(
        src_y.as_ptr(),
        width,
        src_u.as_ptr(),
        width,
        src_v.as_ptr(),
        width,
        dst.as_mut_ptr(),
        width * 4,
        width,
        height,
    ))
}

/// Convert RGB24 to NV12 (via two-step conversion: RGB24 → I420 → NV12)
pub fn rgb24_to_nv12(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;

    if src.len() < rgb24_size(w, h) || dst.len() < nv12_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    // Two-step conversion: RGB24 → I420 → NV12
    let mut i420_buffer = vec![0u8; i420_size(w, h)];
    rgb24_to_i420(src, &mut i420_buffer, width, height)?;
    i420_to_nv12(&i420_buffer, dst, width, height)
}

/// Convert BGR24 to NV12.
pub fn bgr24_to_nv12(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;

    if src.len() < rgb24_size(w, h) || dst.len() < nv12_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    #[cfg(windows)]
    {
        let mut i420_buffer = vec![0u8; i420_size(w, h)];
        bgr24_to_i420(src, &mut i420_buffer, width, height)?;
        return i420_to_nv12(&i420_buffer, dst, width, height);
    }

    #[cfg(not(windows))]
    {
        let y_size = w * h;
        call_yuv!(RGB24ToNV12(
            src.as_ptr(),
            width * 3,
            dst.as_mut_ptr(),
            width,
            dst[y_size..].as_mut_ptr(),
            width,
            width,
            height,
        ))
    }
}
