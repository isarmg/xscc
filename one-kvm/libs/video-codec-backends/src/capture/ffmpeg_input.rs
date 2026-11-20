use super::{copy_rgb_packet, CaptureError, CapturePixelFormat, CaptureStreamInfo};
use std::{
    ffi::{c_void, CStr, CString},
    ptr::{self, NonNull},
    sync::{
        atomic::{AtomicU64, Ordering},
        Once,
    },
    thread,
    time::{Duration, Instant},
};

#[allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/ffmpeg_capture_ffi.rs"));
}

struct InterruptDeadline {
    start: Instant,
    expires_at_ms: AtomicU64,
}

impl InterruptDeadline {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            expires_at_ms: AtomicU64::new(0),
        }
    }

    fn now_ms(&self) -> u64 {
        self.start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn start(&self, timeout: Duration) -> ActiveDeadline<'_> {
        let timeout_ms = timeout.as_millis().try_into().unwrap_or(u64::MAX);
        self.expires_at_ms
            .store(self.now_ms().saturating_add(timeout_ms), Ordering::Relaxed);
        ActiveDeadline(self)
    }

    fn expired(&self) -> bool {
        let deadline = self.expires_at_ms.load(Ordering::Relaxed);
        deadline != 0 && self.now_ms() >= deadline
    }
}

struct ActiveDeadline<'a>(&'a InterruptDeadline);

impl Drop for ActiveDeadline<'_> {
    fn drop(&mut self) {
        self.0.expires_at_ms.store(0, Ordering::Relaxed);
    }
}

unsafe extern "C" fn interrupt_callback(opaque: *mut c_void) -> i32 {
    // The input context releases FFmpeg's callbacks before this allocation is freed.
    unsafe { opaque.cast::<InterruptDeadline>().as_ref() }.is_some_and(InterruptDeadline::expired)
        as i32
}

struct Input {
    context: *mut ffi::AVFormatContext,
    opened: bool,
}

impl Drop for Input {
    fn drop(&mut self) {
        unsafe {
            if self.opened {
                ffi::avformat_close_input(&mut self.context);
            } else {
                ffi::avformat_free_context(self.context);
            }
        }
    }
}

struct Packet(NonNull<ffi::AVPacket>);

impl Drop for Packet {
    fn drop(&mut self) {
        let mut packet = self.0.as_ptr();
        unsafe { ffi::av_packet_free(&mut packet) }
    }
}

#[derive(Default)]
struct Options(*mut ffi::AVDictionary);

impl Options {
    fn set(&mut self, key: &CStr, value: &CStr) -> Result<(), CaptureError> {
        let result = unsafe { ffi::av_dict_set(&mut self.0, key.as_ptr(), value.as_ptr(), 0) };
        if result < 0 {
            return Err(ffmpeg_error(
                result,
                "Failed to set DirectShow capture option",
            ));
        }
        Ok(())
    }
}

impl Drop for Options {
    fn drop(&mut self) {
        unsafe { ffi::av_dict_free(&mut self.0) }
    }
}

fn failure(message: &str) -> CaptureError {
    CaptureError {
        code: -1,
        message: message.to_owned(),
    }
}

fn timeout_error() -> CaptureError {
    CaptureError {
        code: -110,
        message: "Timed out waiting for a DirectShow frame".to_owned(),
    }
}

fn ffmpeg_error(code: i32, operation: &str) -> CaptureError {
    let mut buffer = [0; 64];
    let result = unsafe { ffi::av_strerror(code, buffer.as_mut_ptr(), buffer.len()) };
    let detail = if result < 0 {
        format!("FFmpeg error {code}")
    } else {
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    CaptureError {
        code,
        message: format!("{operation}: {detail}"),
    }
}

impl Input {
    fn open(
        name: &CStr,
        format: NonNull<ffi::AVInputFormat>,
        options: &mut Options,
        deadline: &InterruptDeadline,
    ) -> Result<Self, CaptureError> {
        let context = NonNull::new(unsafe { ffi::avformat_alloc_context() })
            .ok_or_else(|| failure("Failed to allocate FFmpeg input context"))?;
        let mut input = Self {
            context: context.as_ptr(),
            opened: false,
        };
        unsafe {
            (*input.context).flags |= ffi::AVFMT_FLAG_NONBLOCK as i32;
            (*input.context).interrupt_callback = ffi::AVIOInterruptCB {
                callback: Some(interrupt_callback),
                opaque: ptr::from_ref(deadline).cast_mut().cast(),
            };
        }
        let status = unsafe {
            ffi::avformat_open_input(
                &mut input.context,
                name.as_ptr(),
                format.as_ptr(),
                &mut options.0,
            )
        };
        if status < 0 {
            return Err(ffmpeg_error(status, "Failed to open DirectShow input"));
        }
        input.opened = true;
        Ok(input)
    }

    fn video_stream(&self) -> Option<(i32, &ffi::AVCodecParameters, ffi::AVRational)> {
        let context = unsafe { self.context.as_ref()? };
        if context.nb_streams == 0 || context.streams.is_null() {
            return None;
        }
        // FFmpeg owns nb_streams pointers for the lifetime of this context.
        let streams =
            unsafe { std::slice::from_raw_parts(context.streams, context.nb_streams as usize) };
        streams.iter().enumerate().find_map(|(index, stream)| {
            let stream = unsafe { (*stream).as_ref()? };
            let parameters = unsafe { stream.codecpar.as_ref()? };
            if parameters.codec_type != ffi::AVMEDIA_TYPE_VIDEO {
                return None;
            }
            Some((
                i32::try_from(index).ok()?,
                parameters,
                stream.avg_frame_rate,
            ))
        })
    }
}

struct RequestedMode {
    width: i32,
    height: i32,
    fps: i32,
    pixel_format: CapturePixelFormat,
}

impl RequestedMode {
    fn options(&self, size: bool, framerate: bool) -> Result<Options, CaptureError> {
        let mut options = Options::default();
        if size && self.width > 0 && self.height > 0 {
            let value = CString::new(format!("{}x{}", self.width, self.height))
                .expect("invariant: formatted dimensions contain no NUL");
            options.set(c"video_size", &value)?;
        }
        if framerate && self.fps > 0 {
            let value = CString::new(self.fps.to_string())
                .expect("invariant: formatted FPS contains no NUL");
            options.set(c"framerate", &value)?;
        }
        if size {
            if let Some(value) = self.pixel_format.ffmpeg_name() {
                options.set(c"pixel_format", value)?;
            }
        }
        options.set(c"rtbufsize", c"64M")?;
        Ok(options)
    }
}

impl CapturePixelFormat {
    fn ffmpeg_name(self) -> Option<&'static CStr> {
        Some(match self {
            Self::Yuyv => c"yuyv422",
            Self::Uyvy => c"uyvy422",
            Self::Nv12 => c"nv12",
            Self::Nv21 => c"nv21",
            Self::Rgb24 => c"rgb24",
            Self::Bgr24 => c"bgr24",
            Self::Grey => c"gray",
            _ => return None,
        })
    }

    fn from_codec(parameters: &ffi::AVCodecParameters) -> Self {
        use ffi::*;
        match parameters.codec_id {
            AV_CODEC_ID_MJPEG => Self::Mjpeg,
            AV_CODEC_ID_JPEG2000 => Self::Jpeg,
            AV_CODEC_ID_RAWVIDEO => match parameters.format {
                AV_PIX_FMT_YUYV422 => Self::Yuyv,
                AV_PIX_FMT_YVYU422 => Self::Yvyu,
                AV_PIX_FMT_UYVY422 => Self::Uyvy,
                AV_PIX_FMT_NV12 => Self::Nv12,
                AV_PIX_FMT_NV21 => Self::Nv21,
                AV_PIX_FMT_NV16 => Self::Nv16,
                AV_PIX_FMT_NV24 => Self::Nv24,
                AV_PIX_FMT_YUV420P => {
                    if parameters.codec_tag == u32::from_le_bytes(*b"YV12") {
                        Self::Yvu420
                    } else {
                        Self::Yuv420
                    }
                }
                AV_PIX_FMT_RGB24 => Self::Rgb24,
                AV_PIX_FMT_BGR24 => Self::Bgr24,
                AV_PIX_FMT_GRAY8 => Self::Grey,
                _ => Self::Unknown,
            },
            _ => Self::Unknown,
        }
    }

    fn row_bytes(self, width: i32) -> Option<i32> {
        width.checked_mul(match self {
            Self::Yuyv | Self::Yvyu | Self::Uyvy | Self::Nv24 => 2,
            Self::Rgb24 | Self::Bgr24 => 3,
            _ => 1,
        })
    }
}

pub struct DshowCapture {
    packet: Packet,
    input: Input,
    interrupt: Box<InterruptDeadline>,
    timeout: Duration,
    info: CaptureStreamInfo,
    bottom_up: bool,
    stream_index: i32,
    sequence: u64,
}

impl DshowCapture {
    pub fn open(
        device_name: &str,
        width: i32,
        height: i32,
        fps: i32,
        requested_format: CapturePixelFormat,
        timeout_ms: i32,
    ) -> Result<Self, CaptureError> {
        if device_name.is_empty() {
            return Err(failure("DirectShow device name is empty"));
        }
        let name = CString::new(format!("video={device_name}"))
            .map_err(|_| failure("DirectShow device name contains a NUL byte"))?;
        static REGISTER_DEVICES: Once = Once::new();
        REGISTER_DEVICES.call_once(|| unsafe { ffi::avdevice_register_all() });
        let input_format =
            NonNull::new(unsafe { ffi::av_find_input_format(c"dshow".as_ptr()) }.cast_mut())
                .ok_or_else(|| failure("FFmpeg DirectShow input is unavailable"))?;
        let interrupt = Box::new(InterruptDeadline::new());
        let timeout = Duration::from_millis(if timeout_ms > 0 {
            timeout_ms as u64
        } else {
            2_000
        });
        let requested = RequestedMode {
            width,
            height,
            fps,
            pixel_format: requested_format,
        };
        let input = {
            let _deadline = interrupt.start(timeout);
            let mut result = Err(failure("No DirectShow input configuration could be opened"));
            for (size, framerate) in [(true, true), (true, false), (false, false)] {
                let mut options = requested.options(size, framerate)?;
                result = Input::open(&name, input_format, &mut options, &interrupt);
                if result.is_ok() || interrupt.expired() {
                    break;
                }
            }
            let input = result?;
            let status = unsafe { ffi::avformat_find_stream_info(input.context, ptr::null_mut()) };
            if status < 0 {
                return Err(ffmpeg_error(
                    status,
                    "Failed to read DirectShow stream information",
                ));
            }
            input
        };
        let (stream_index, parameters, frame_rate) = input
            .video_stream()
            .ok_or_else(|| failure("No video stream found for DirectShow capture"))?;
        let pixel_format = CapturePixelFormat::from_codec(parameters);
        if pixel_format == CapturePixelFormat::Unknown {
            return Err(failure(
                "DirectShow capture negotiated an unsupported video format",
            ));
        }
        let width = if parameters.width > 0 {
            parameters.width
        } else {
            width
        };
        let height = if parameters.height > 0 {
            parameters.height
        } else {
            height
        };
        let stride = pixel_format
            .row_bytes(width)
            .filter(|value| *value > 0 && height > 0)
            .ok_or_else(|| failure("DirectShow capture negotiated invalid frame dimensions"))?;
        let info = CaptureStreamInfo {
            width,
            height,
            pixel_format,
            stride,
            frame_rate: (frame_rate.num > 0 && frame_rate.den > 0)
                .then(|| f64::from(frame_rate.num) / f64::from(frame_rate.den)),
        };
        let bottom_up = parameters.extradata_size == 9
            && !parameters.extradata.is_null()
            && unsafe { std::slice::from_raw_parts(parameters.extradata, 9) } == b"BottomUp\0";
        let packet = NonNull::new(unsafe { ffi::av_packet_alloc() })
            .map(Packet)
            .ok_or_else(|| failure("Failed to allocate a DirectShow capture packet"))?;
        Ok(Self {
            input,
            packet,
            interrupt,
            timeout,
            info,
            bottom_up,
            stream_index,
            sequence: 0,
        })
    }

    pub fn info(&self) -> CaptureStreamInfo {
        self.info
    }

    pub fn read_packet_into(&mut self, dst: &mut Vec<u8>) -> Result<(usize, u64), CaptureError> {
        let _deadline = self.interrupt.start(self.timeout);
        loop {
            if self.interrupt.expired() {
                return Err(timeout_error());
            }
            let status = unsafe {
                ffi::av_packet_unref(self.packet.0.as_ptr());
                ffi::av_read_frame(self.input.context, self.packet.0.as_ptr())
            };
            if status == -(ffi::EAGAIN as i32) {
                // The DirectShow demuxer only honors cancellation between nonblocking reads.
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            if status < 0 {
                if self.interrupt.expired() {
                    return Err(timeout_error());
                }
                return Err(ffmpeg_error(status, "Failed to read a DirectShow frame"));
            }
            let packet = unsafe { self.packet.0.as_ref() };
            if packet.stream_index != self.stream_index || packet.size <= 0 || packet.data.is_null()
            {
                continue;
            }

            // The borrowed view ends before the next av_packet_unref.
            let data = unsafe { std::slice::from_raw_parts(packet.data, packet.size as usize) };
            if matches!(
                self.info.pixel_format,
                CapturePixelFormat::Rgb24 | CapturePixelFormat::Bgr24
            ) {
                copy_rgb_packet(
                    data,
                    dst,
                    self.info.stride,
                    self.info.height,
                    self.bottom_up,
                )?;
            } else {
                dst.clear();
                dst.extend_from_slice(data);
            }

            let sequence = self.sequence;
            self.sequence = self.sequence.wrapping_add(1);
            return Ok((dst.len(), sequence));
        }
    }
}
