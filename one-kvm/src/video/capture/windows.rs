use std::{
    io,
    mem::size_of,
    path::{Path, PathBuf},
    ptr,
    time::{Duration, Instant},
};

use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    HBITMAP, HDC, SRCCOPY,
};

use super::CaptureReadError;
use crate::error::{AppError, Result};
use crate::platform::windows_desktop::{
    monitor_for_path, DesktopMonitor, PhysicalCoordinates, DESKTOP_DEVICE_PREFIX,
};
use crate::video::device::bridge::{CsiBridgeKind, ProbeResult};
use crate::video::device::{
    directshow_display_name_from_path, normalize_windows_device_path, VideoControlMode,
};
use crate::video::format::{PixelFormat, Resolution};

#[derive(Debug, Clone, Copy)]
pub struct CaptureMeta {
    pub bytes_used: usize,
    pub sequence: u64,
}

#[derive(Debug, Clone, Default)]
pub struct BridgeContext {
    pub subdev_path: Option<PathBuf>,
    pub kind: Option<CsiBridgeKind>,
}

impl BridgeContext {
    pub fn from_parts(subdev_path: Option<PathBuf>, kind: Option<CsiBridgeKind>) -> Self {
        Self { subdev_path, kind }
    }

    pub fn has_subdev(&self) -> bool {
        false
    }
}

pub struct CaptureStream {
    capture: WindowsCapture,
    resolution: Resolution,
    format: PixelFormat,
    stride: u32,
}

enum WindowsCapture {
    DirectShow(hwcodec::capture::DshowCapture),
    Desktop(DesktopCapture),
}

unsafe impl Send for CaptureStream {}

impl CaptureStream {
    pub fn open(
        device_path: impl AsRef<Path>,
        resolution: Resolution,
        format: PixelFormat,
        fps: u32,
        buffer_count: u32,
        timeout: Duration,
    ) -> Result<Self> {
        let _ = buffer_count;
        let path = normalize_windows_device_path(device_path);
        if path.to_string_lossy().starts_with(DESKTOP_DEVICE_PREFIX) {
            let capture = DesktopCapture::open(&path.to_string_lossy(), fps)?;
            let resolution = Resolution::new(capture.monitor.width, capture.monitor.height);
            return Ok(Self {
                stride: resolution.width * 3,
                resolution,
                format: PixelFormat::Bgr24,
                capture: WindowsCapture::Desktop(capture),
            });
        }
        let display_name = directshow_display_name_from_path(&path).ok_or_else(|| {
            AppError::VideoError(format!(
                "Unsupported DirectShow device path: {}",
                path.display()
            ))
        })?;
        let capture = hwcodec::capture::DshowCapture::open(
            &display_name,
            resolution.width as i32,
            resolution.height as i32,
            fps as i32,
            map_pixel_format(format),
            timeout.as_millis().clamp(1, i32::MAX as u128) as i32,
        )
        .map_err(|e| AppError::VideoError(format!("Failed to open DirectShow capture: {}", e)))?;
        let info = capture.info().map_err(|e| {
            AppError::VideoError(format!("Failed to query DirectShow capture: {}", e))
        })?;
        let actual_format = map_capture_format(info.pixel_format)?;
        let actual_resolution =
            Resolution::new(info.width.max(1) as u32, info.height.max(1) as u32);

        Ok(Self {
            capture: WindowsCapture::DirectShow(capture),
            resolution: actual_resolution,
            format: actual_format,
            stride: info.stride.max(0) as u32,
        })
    }

    pub fn open_with_bridge(
        device_path: impl AsRef<Path>,
        resolution: Resolution,
        format: PixelFormat,
        fps: u32,
        buffer_count: u32,
        timeout: Duration,
        bridge: BridgeContext,
        control_mode: VideoControlMode,
    ) -> Result<Self> {
        let _ = (bridge, control_mode);
        Self::open(device_path, resolution, format, fps, buffer_count, timeout)
    }

    pub fn resolution(&self) -> Resolution {
        self.resolution
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn source_fps(&self) -> Option<f64> {
        match &self.capture {
            WindowsCapture::DirectShow(_) => None,
            WindowsCapture::Desktop(capture) => capture.monitor.refresh_hz.map(f64::from),
        }
    }

    pub fn stride(&self) -> u32 {
        self.stride
    }

    pub fn next_into(
        &mut self,
        dst: &mut Vec<u8>,
    ) -> std::result::Result<CaptureMeta, CaptureReadError> {
        let capture = match &mut self.capture {
            WindowsCapture::Desktop(capture) => return capture.next_into(dst),
            WindowsCapture::DirectShow(capture) => capture,
        };
        match capture.read_packet() {
            Ok((packet, sequence)) => {
                dst.clear();
                dst.extend_from_slice(&packet);
                Ok(CaptureMeta {
                    bytes_used: packet.len(),
                    sequence,
                })
            }
            Err(err) => {
                let kind = if err.code == -110 {
                    io::ErrorKind::TimedOut
                } else {
                    io::ErrorKind::Other
                };
                Err(CaptureReadError::Io(io::Error::new(kind, err.message)))
            }
        }
    }

    pub fn probe_bridge_signal_with_timeout(&self, _limit: Duration) -> Option<ProbeResult> {
        None
    }
}

struct DesktopCapture {
    monitor: DesktopMonitor,
    device_path: String,
    screen_dc: HDC,
    memory_dc: HDC,
    bitmap: HBITMAP,
    interval: Duration,
    next_frame: Instant,
    sequence: u64,
}

impl DesktopCapture {
    fn open(device_path: &str, fps: u32) -> Result<Self> {
        if !(1..=120).contains(&fps) {
            return Err(AppError::BadRequest(
                "Desktop capture FPS must be between 1 and 120".into(),
            ));
        }
        let monitor = monitor_for_path(device_path)?;
        if !Resolution::new(monitor.width, monitor.height).is_valid() {
            return Err(AppError::VideoError(
                "Desktop monitor dimensions exceed supported capture limits".into(),
            ));
        }
        let _coordinates = PhysicalCoordinates::enter()?;
        let mut capture = Self {
            monitor,
            device_path: device_path.to_owned(),
            screen_dc: ptr::null_mut(),
            memory_dc: ptr::null_mut(),
            bitmap: ptr::null_mut(),
            interval: Duration::from_secs_f64(1.0 / f64::from(fps)),
            next_frame: Instant::now(),
            sequence: 0,
        };
        capture.screen_dc = unsafe { GetDC(ptr::null_mut()) };
        if capture.screen_dc.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        capture.memory_dc = unsafe { CreateCompatibleDC(capture.screen_dc) };
        if capture.memory_dc.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        capture.bitmap = unsafe {
            CreateCompatibleBitmap(
                capture.screen_dc,
                capture.monitor.width as i32,
                capture.monitor.height as i32,
            )
        };
        if capture.bitmap.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        Ok(capture)
    }

    fn next_into(
        &mut self,
        destination: &mut Vec<u8>,
    ) -> std::result::Result<CaptureMeta, CaptureReadError> {
        // CaptureStream is read on VideoStreamer's blocking capture worker.
        if let Some(wait) = self.next_frame.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        self.next_frame = Instant::now() + self.interval;
        let current = monitor_for_path(&self.device_path)
            .map_err(|error| io::Error::new(io::ErrorKind::NotFound, error))?;
        if current != self.monitor {
            return Err(CaptureReadError::SourceChanged);
        }
        let _coordinates = PhysicalCoordinates::enter()?;
        let previous = unsafe { SelectObject(self.memory_dc, self.bitmap) };
        if previous.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        let copied = unsafe {
            BitBlt(
                self.memory_dc,
                0,
                0,
                self.monitor.width as i32,
                self.monitor.height as i32,
                self.screen_dc,
                self.monitor.left,
                self.monitor.top,
                SRCCOPY | CAPTUREBLT,
            )
        };
        let copy_error = (copied == 0).then(io::Error::last_os_error);
        // GetDIBits requires the bitmap to be deselected from every DC.
        let restored = unsafe { SelectObject(self.memory_dc, previous) };
        if restored.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        if let Some(error) = copy_error {
            return Err(error.into());
        }
        let row_bytes = self.monitor.width as usize * 3;
        let stride = (row_bytes + 3) & !3;
        destination.resize(stride * self.monitor.height as usize, 0);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: self.monitor.width as i32,
                biHeight: -(self.monitor.height as i32),
                biPlanes: 1,
                biBitCount: 24,
                biCompression: BI_RGB,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let rows = unsafe {
            GetDIBits(
                self.screen_dc,
                self.bitmap,
                0,
                self.monitor.height,
                destination.as_mut_ptr().cast(),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        if rows != self.monitor.height as i32 {
            return Err(io::Error::other(format!(
                "GetDIBits returned {rows} of {} desktop rows: {}",
                self.monitor.height,
                io::Error::last_os_error()
            ))
            .into());
        }
        compact_rows(destination, row_bytes, stride, self.monitor.height as usize);
        self.sequence = self.sequence.wrapping_add(1);
        Ok(CaptureMeta {
            bytes_used: destination.len(),
            sequence: self.sequence,
        })
    }
}

impl Drop for DesktopCapture {
    fn drop(&mut self) {
        unsafe {
            if !self.memory_dc.is_null() {
                DeleteDC(self.memory_dc);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.screen_dc.is_null() {
                ReleaseDC(ptr::null_mut(), self.screen_dc);
            }
        }
    }
}

fn compact_rows(bytes: &mut Vec<u8>, row_bytes: usize, stride: usize, height: usize) {
    if stride != row_bytes {
        for row in 1..height {
            bytes.copy_within(row * stride..row * stride + row_bytes, row * row_bytes);
        }
    }
    bytes.truncate(row_bytes * height);
}

#[cfg(test)]
mod desktop_tests {
    use super::compact_rows;

    #[test]
    fn removes_gdi_padding_without_reordering_top_down_pixels() {
        let mut pixels = vec![1, 2, 3, 0, 4, 5, 6, 0];
        compact_rows(&mut pixels, 3, 4, 2);
        assert_eq!(pixels, vec![1, 2, 3, 4, 5, 6]);
    }
}

fn map_pixel_format(format: PixelFormat) -> hwcodec::capture::CapturePixelFormat {
    match format {
        PixelFormat::Mjpeg => hwcodec::capture::CapturePixelFormat::Mjpeg,
        PixelFormat::Jpeg => hwcodec::capture::CapturePixelFormat::Jpeg,
        PixelFormat::Yuyv => hwcodec::capture::CapturePixelFormat::Yuyv,
        PixelFormat::Yvyu => hwcodec::capture::CapturePixelFormat::Yvyu,
        PixelFormat::Uyvy => hwcodec::capture::CapturePixelFormat::Uyvy,
        PixelFormat::Nv12 => hwcodec::capture::CapturePixelFormat::Nv12,
        PixelFormat::Nv21 => hwcodec::capture::CapturePixelFormat::Nv21,
        PixelFormat::Nv16 => hwcodec::capture::CapturePixelFormat::Nv16,
        PixelFormat::Nv24 => hwcodec::capture::CapturePixelFormat::Nv24,
        PixelFormat::Yuv420 => hwcodec::capture::CapturePixelFormat::Yuv420,
        PixelFormat::Yvu420 => hwcodec::capture::CapturePixelFormat::Yvu420,
        PixelFormat::Rgb24 => hwcodec::capture::CapturePixelFormat::Rgb24,
        PixelFormat::Bgr24 => hwcodec::capture::CapturePixelFormat::Bgr24,
        PixelFormat::Grey => hwcodec::capture::CapturePixelFormat::Grey,
        PixelFormat::Rgb565 => hwcodec::capture::CapturePixelFormat::Unknown,
    }
}

fn map_capture_format(format: hwcodec::capture::CapturePixelFormat) -> Result<PixelFormat> {
    match format {
        hwcodec::capture::CapturePixelFormat::Mjpeg => Ok(PixelFormat::Mjpeg),
        hwcodec::capture::CapturePixelFormat::Jpeg => Ok(PixelFormat::Jpeg),
        hwcodec::capture::CapturePixelFormat::Yuyv => Ok(PixelFormat::Yuyv),
        hwcodec::capture::CapturePixelFormat::Yvyu => Ok(PixelFormat::Yvyu),
        hwcodec::capture::CapturePixelFormat::Uyvy => Ok(PixelFormat::Uyvy),
        hwcodec::capture::CapturePixelFormat::Nv12 => Ok(PixelFormat::Nv12),
        hwcodec::capture::CapturePixelFormat::Nv21 => Ok(PixelFormat::Nv21),
        hwcodec::capture::CapturePixelFormat::Nv16 => Ok(PixelFormat::Nv16),
        hwcodec::capture::CapturePixelFormat::Nv24 => Ok(PixelFormat::Nv24),
        hwcodec::capture::CapturePixelFormat::Yuv420 => Ok(PixelFormat::Yuv420),
        hwcodec::capture::CapturePixelFormat::Yvu420 => Ok(PixelFormat::Yvu420),
        hwcodec::capture::CapturePixelFormat::Rgb24 => Ok(PixelFormat::Rgb24),
        hwcodec::capture::CapturePixelFormat::Bgr24 => Ok(PixelFormat::Bgr24),
        hwcodec::capture::CapturePixelFormat::Grey => Ok(PixelFormat::Grey),
        hwcodec::capture::CapturePixelFormat::Unknown => Err(AppError::ServiceUnavailable(
            "DirectShow returned an unsupported pixel format".to_string(),
        )),
    }
}
