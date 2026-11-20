//! libyuv bindings for high-performance pixel format conversion
//!
//! This crate provides safe Rust bindings to Google's libyuv library,
//! which offers SIMD-accelerated (SSE/AVX/NEON) color space conversion.
use std::fmt;

mod ffi {
    #![allow(non_snake_case)]
    include!(concat!(env!("OUT_DIR"), "/yuv_ffi.rs"));
}

/// Error type for libyuv operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvError {
    /// The format cannot represent the given frame dimensions or stride.
    InvalidDimensions,
    /// Input or output buffer is too small
    BufferTooSmall,
    /// libyuv function returned an error code
    ConversionFailed(i32),
}

impl fmt::Display for YuvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            YuvError::InvalidDimensions => write!(f, "Invalid dimensions or stride"),
            YuvError::BufferTooSmall => write!(f, "Buffer too small"),
            YuvError::ConversionFailed(code) => write!(f, "Conversion failed with code {}", code),
        }
    }
}

impl std::error::Error for YuvError {}

pub type Result<T> = std::result::Result<T, YuvError>;

/// Macro to call libyuv functions and check return value
macro_rules! call_yuv {
    ($func:expr) => {{
        let ret = unsafe { $func };
        if ret != 0 {
            return Err(YuvError::ConversionFailed(ret));
        }
        Ok(())
    }};
}

mod mjpeg;
mod packed;
mod planar;

pub use mjpeg::{mjpg_size, mjpg_to_nv12, mjpg_to_nv12_vec};
pub use packed::{
    bgr24_to_i420, bgr24_to_nv12, h444_to_bgra, rgb24_to_i420, rgb24_to_nv12, uyvy_to_i420,
    yuy2_to_i420, yuy2_to_nv12,
};
pub use planar::{
    i420_to_nv12, i422_to_i420_planar, i444_to_i420_planar, nv12_to_i420, nv21_to_i420,
    nv21_to_nv12, split_uv_plane,
};

fn dimensions(width: i32, height: i32) -> Result<(usize, usize)> {
    // libyuv can coalesce rows and calculate packed strides as signed ints.
    if width <= 0 || height <= 0 || width > (i32::MAX / 4) / height {
        return Err(YuvError::InvalidDimensions);
    }
    Ok((width as usize, height as usize))
}

fn even_dimensions(width: i32, height: i32) -> Result<(usize, usize)> {
    if width % 2 != 0 || height % 2 != 0 {
        return Err(YuvError::InvalidDimensions);
    }
    dimensions(width, height)
}

/// Calculate I420 (YUV420P) buffer size
#[inline]
const fn i420_size(width: usize, height: usize) -> usize {
    width * height + (width / 2) * (height / 2) * 2
}

/// Calculate NV12 buffer size
#[inline]
const fn nv12_size(width: usize, height: usize) -> usize {
    width * height * 3 / 2
}

/// Calculate YUYV/UYVY buffer size
#[inline]
const fn yuyv_size(width: usize, height: usize) -> usize {
    width * height * 2
}

/// Calculate RGB24/BGR24 buffer size
#[inline]
const fn rgb24_size(width: usize, height: usize) -> usize {
    width * height * 3
}

/// Calculate ARGB/BGRA buffer size
#[inline]
const fn argb_size(width: usize, height: usize) -> usize {
    width * height * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_sizes() {
        assert_eq!(i420_size(1920, 1080), 1920 * 1080 * 3 / 2);
        assert_eq!(nv12_size(1920, 1080), 1920 * 1080 * 3 / 2);
        assert_eq!(yuyv_size(1920, 1080), 1920 * 1080 * 2);
        assert_eq!(rgb24_size(1920, 1080), 1920 * 1080 * 3);
        assert_eq!(argb_size(1920, 1080), 1920 * 1080 * 4);
    }

    #[test]
    fn test_invalid_dimensions() {
        let src = vec![0u8; 100];
        let mut dst = vec![0u8; 100];

        for (width, height) in [(3, 2), (4, 3), (-2, -2), (0, 2), (1 << 30, 2)] {
            assert_eq!(
                yuy2_to_nv12(&src, &mut dst, width, height),
                Err(YuvError::InvalidDimensions)
            );
        }
    }

    #[test]
    fn test_buffer_too_small() {
        let src = vec![0u8; 10]; // Too small
        let mut dst = vec![0u8; nv12_size(4, 4)];

        assert!(matches!(
            yuy2_to_nv12(&src, &mut dst, 4, 4),
            Err(YuvError::BufferTooSmall)
        ));
    }

    #[test]
    fn test_bgr24_to_nv12() {
        let src = [0u8; 2 * 2 * 3];
        let mut dst = [0xffu8; 2 * 2 * 3 / 2];

        bgr24_to_nv12(&src, &mut dst, 2, 2).unwrap();

        assert_eq!(&dst[..4], &[16, 16, 16, 16]);
        assert_eq!(&dst[4..], &[128, 128]);
    }
}
