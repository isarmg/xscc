use super::{
    dimensions, even_dimensions,
    ffi::{I420ToNV12, I422ToI420, I444ToI420, NV12ToI420, NV21ToI420, NV21ToNV12, SplitUVPlane},
    i420_size, nv12_size, Result, YuvError,
};

fn validate_plane(length: usize, stride: i32, row_bytes: usize, rows: usize) -> Result<()> {
    let stride = usize::try_from(stride).map_err(|_| YuvError::InvalidDimensions)?;
    if stride < row_bytes {
        return Err(YuvError::InvalidDimensions);
    }
    let required = (rows - 1)
        .checked_mul(stride)
        .and_then(|padding| padding.checked_add(row_bytes))
        .ok_or(YuvError::BufferTooSmall)?;
    if length < required {
        return Err(YuvError::BufferTooSmall);
    }
    Ok(())
}

/// Convert I422 (YUV422P) to I420 (YUV420P) with separate planes and explicit strides
/// This performs vertical 2:1 chroma downsampling using SIMD
pub fn i422_to_i420_planar(
    src_y: &[u8],
    src_y_stride: i32,
    src_u: &[u8],
    src_u_stride: i32,
    src_v: &[u8],
    src_v_stride: i32,
    dst: &mut [u8],
    width: i32,
    height: i32,
) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    validate_plane(src_y.len(), src_y_stride, w, h)?;
    validate_plane(src_u.len(), src_u_stride, w / 2, h)?;
    validate_plane(src_v.len(), src_v_stride, w / 2, h)?;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    if dst.len() < i420_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(I422ToI420(
        src_y.as_ptr(),
        src_y_stride,
        src_u.as_ptr(),
        src_u_stride,
        src_v.as_ptr(),
        src_v_stride,
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

/// Convert I444 (YUV444P) to I420 (YUV420P) with separate planes and explicit strides
/// This performs horizontal and vertical chroma downsampling using SIMD
pub fn i444_to_i420_planar(
    src_y: &[u8],
    src_y_stride: i32,
    src_u: &[u8],
    src_u_stride: i32,
    src_v: &[u8],
    src_v_stride: i32,
    dst: &mut [u8],
    width: i32,
    height: i32,
) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    validate_plane(src_y.len(), src_y_stride, w, h)?;
    validate_plane(src_u.len(), src_u_stride, w, h)?;
    validate_plane(src_v.len(), src_v_stride, w, h)?;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    if dst.len() < i420_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(I444ToI420(
        src_y.as_ptr(),
        src_y_stride,
        src_u.as_ptr(),
        src_u_stride,
        src_v.as_ptr(),
        src_v_stride,
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

/// Split an interleaved UV plane into separate U and V planes using libyuv SIMD helpers.
///
/// `width` is the number of chroma samples per row, not the number of source bytes.
pub fn split_uv_plane(
    src_uv: &[u8],
    src_stride_uv: i32,
    dst_u: &mut [u8],
    dst_stride_u: i32,
    dst_v: &mut [u8],
    dst_stride_v: i32,
    width: i32,
    height: i32,
) -> Result<()> {
    let (w, h) = dimensions(width, height)?;
    validate_plane(src_uv.len(), src_stride_uv, 2 * w, h)?;
    validate_plane(dst_u.len(), dst_stride_u, w, h)?;
    validate_plane(dst_v.len(), dst_stride_v, w, h)?;

    // SAFETY: all three slices cover each row libyuv processes.
    unsafe {
        SplitUVPlane(
            src_uv.as_ptr(),
            src_stride_uv,
            dst_u.as_mut_ptr(),
            dst_stride_u,
            dst_v.as_mut_ptr(),
            dst_stride_v,
            width,
            height,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{i422_to_i420_planar, split_uv_plane, YuvError};

    #[test]
    fn rejects_short_chroma_plane() {
        assert_eq!(
            i422_to_i420_planar(&[0; 4], 2, &[], 1, &[0; 2], 1, &mut [0; 6], 2, 2),
            Err(YuvError::BufferTooSmall)
        );
    }

    #[test]
    fn validates_stride_and_accepts_trailing_unpadded_row() {
        let mut u = [0; 4];
        let mut v = [0; 4];
        assert_eq!(
            split_uv_plane(&[0; 8], 3, &mut u, 2, &mut v, 2, 2, 2),
            Err(YuvError::InvalidDimensions)
        );
        split_uv_plane(&[1, 2, 3, 4, 0, 5, 6, 7, 8], 5, &mut u, 2, &mut v, 2, 2, 2).unwrap();
        assert_eq!(u, [1, 3, 5, 7]);
        assert_eq!(v, [2, 4, 6, 8]);
    }
}

/// Convert I420 (YUV420P) to NV12
pub fn i420_to_nv12(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    if src.len() < i420_size(w, h) || dst.len() < nv12_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(I420ToNV12(
        src.as_ptr(),
        width,
        src[y_size..].as_ptr(),
        width / 2,
        src[y_size + uv_size..].as_ptr(),
        width / 2,
        dst.as_mut_ptr(),
        width,
        dst[y_size..].as_mut_ptr(),
        width,
        width,
        height,
    ))
}

/// Convert NV12 to I420 (YUV420P)
pub fn nv12_to_i420(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    if src.len() < nv12_size(w, h) || dst.len() < i420_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(NV12ToI420(
        src.as_ptr(),
        width,
        src[y_size..].as_ptr(),
        width,
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

/// Convert NV21 to I420 (YUV420P)
pub fn nv21_to_i420(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    let y_size = w * h;
    let uv_size = (w / 2) * (h / 2);

    if src.len() < nv12_size(w, h) || dst.len() < i420_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(NV21ToI420(
        src.as_ptr(),
        width,
        src[y_size..].as_ptr(),
        width,
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

/// Convert NV21 to NV12 by swapping interleaved chroma bytes.
pub fn nv21_to_nv12(src: &[u8], dst: &mut [u8], width: i32, height: i32) -> Result<()> {
    let (w, h) = even_dimensions(width, height)?;
    let y_size = w * h;

    if src.len() < nv12_size(w, h) || dst.len() < nv12_size(w, h) {
        return Err(YuvError::BufferTooSmall);
    }

    call_yuv!(NV21ToNV12(
        src.as_ptr(),
        width,
        src[y_size..].as_ptr(),
        width,
        dst.as_mut_ptr(),
        width,
        dst[y_size..].as_mut_ptr(),
        width,
        width,
        height,
    ))
}
