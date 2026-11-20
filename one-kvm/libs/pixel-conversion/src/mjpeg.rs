use super::{
    even_dimensions,
    ffi::{MJPGSize, MJPGToNV12},
    nv12_size, Result, YuvError,
};

/// Read MJPEG dimensions without decoding the frame.
pub fn mjpg_size(src: &[u8]) -> Result<(i32, i32)> {
    let mut width = 0;
    let mut height = 0;

    call_yuv!(MJPGSize(src.as_ptr(), src.len(), &mut width, &mut height,))?;

    Ok((width, height))
}

/// Decode MJPEG directly to NV12.
pub fn mjpg_to_nv12(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (y_size, output_size) = mjpg_nv12_plane_sizes(width, height)?;
    if dst.len() < output_size {
        return Err(YuvError::BufferTooSmall);
    }

    let (dst_y, dst_uv) = dst.split_at_mut(y_size);
    // SAFETY: the length check above guarantees writable storage for both planes.
    unsafe { mjpg_to_nv12_raw(src, dst_y.as_mut_ptr(), dst_uv.as_mut_ptr(), width, height) }
}

/// Decode MJPEG directly into a reusable `Vec` without zero-filling the output first.
///
/// `Vec::resize` must initialize every byte before libyuv immediately overwrites the
/// complete NV12 frame. This variant lets libyuv initialize spare capacity directly
/// and publishes the new length only after a successful conversion.
pub fn mjpg_to_nv12_vec(src: &[u8], dst: &mut Vec<u8>, width: i32, height: i32) -> Result<()> {
    let (y_size, output_size) = mjpg_nv12_plane_sizes(width, height)?;

    dst.clear();
    dst.reserve(output_size);

    // SAFETY: reserve above guarantees writable capacity for the Y and UV planes.
    // MJPGToNV12 writes the complete output on success; set_len is deliberately
    // delayed until then so callers can never observe partially initialized bytes.
    let result = unsafe {
        let dst_y = dst.as_mut_ptr();
        mjpg_to_nv12_raw(src, dst_y, dst_y.add(y_size), width, height)
    };
    result?;

    // SAFETY: a successful MJPGToNV12 call initialized exactly output_size bytes.
    unsafe { dst.set_len(output_size) };
    Ok(())
}

#[inline]
fn mjpg_nv12_plane_sizes(width: i32, height: i32) -> Result<(usize, usize)> {
    let (width, height) = even_dimensions(width, height)?;
    Ok((width * height, nv12_size(width, height)))
}

/// # Safety
///
/// `dst_y` and `dst_uv` must point to writable planes sized for `width` x `height` NV12.
#[inline]
unsafe fn mjpg_to_nv12_raw(
    src: &[u8],
    dst_y: *mut u8,
    dst_uv: *mut u8,
    width: i32,
    height: i32,
) -> Result<()> {
    call_yuv!(MJPGToNV12(
        src.as_ptr(),
        src.len(),
        dst_y,
        width,
        dst_uv,
        width,
        width,
        height,
        width,
        height,
    ))
}
