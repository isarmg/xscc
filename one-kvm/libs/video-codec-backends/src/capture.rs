mod dshow;

mod ffmpeg_input;

pub use ffmpeg_input::DshowCapture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePixelFormat {
    Unknown,
    Mjpeg,
    Jpeg,
    Yuyv,
    Yvyu,
    Uyvy,
    Nv12,
    Nv21,
    Nv16,
    Nv24,
    Yuv420,
    Yvu420,
    Rgb24,
    Bgr24,
    Grey,
}

#[derive(Debug, Clone)]
pub struct DshowCapability {
    pub format: CapturePixelFormat,
    pub width: u32,
    pub height: u32,
    pub fps: Vec<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureStreamInfo {
    pub width: i32,
    pub height: i32,
    pub pixel_format: CapturePixelFormat,
    pub stride: i32,
    pub frame_rate: Option<f64>,
}

#[derive(Debug)]
pub struct CaptureError {
    pub code: i32,
    pub message: String,
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CaptureError {}

pub fn list_dshow_video_devices() -> Result<Vec<String>, CaptureError> {
    dshow::list_devices()
}

pub fn list_dshow_device_capabilities(
    device_name: &str,
) -> Result<Vec<DshowCapability>, CaptureError> {
    dshow::device_capabilities(device_name)
}

fn copy_rgb_packet(
    source: &[u8],
    destination: &mut Vec<u8>,
    row_bytes: i32,
    rows: i32,
    bottom_up: bool,
) -> Result<(), CaptureError> {
    let invalid = || CaptureError {
        code: -1,
        message: "invalid DirectShow RGB packet dimensions or stride".to_owned(),
    };
    if row_bytes <= 0 || rows <= 0 {
        return Err(invalid());
    }
    let (row_bytes, rows) = (row_bytes as usize, rows as usize);
    let packed_size = row_bytes.checked_mul(rows).ok_or_else(invalid)?;
    let padded_stride = row_bytes.next_multiple_of(4);
    let input_stride = if source.len() == packed_size {
        row_bytes
    } else if padded_stride.checked_mul(rows) == Some(source.len()) {
        padded_stride
    } else {
        return Err(invalid());
    };

    destination.clear();
    if !bottom_up && input_stride == row_bytes {
        destination.extend_from_slice(source);
    } else {
        destination.reserve(packed_size);
        for row in 0..rows {
            let index = if bottom_up { rows - 1 - row } else { row };
            let start = index * input_stride;
            destination.extend_from_slice(&source[start..start + row_bytes]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_padded_bottom_up_rgb_in_top_down_order() {
        let mut dst = Vec::new();
        let source = [1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0];
        copy_rgb_packet(&source, &mut dst, 6, 2, true).unwrap();
        assert_eq!(dst, [7, 8, 9, 10, 11, 12, 1, 2, 3, 4, 5, 6]);
        assert!(copy_rgb_packet(&source[..15], &mut dst, 6, 2, true).is_err());
    }
}
