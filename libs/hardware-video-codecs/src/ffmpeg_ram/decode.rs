#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use super::Priority;
use super::{AGAIN, END_OF_STREAM};
use crate::{
    common::{DataFormat::*, HwcodecErrno, TEST_TIMEOUT_MS},
    ffmpeg::{
        hwcodec_take_hevc_reference_error, hwcodec_watch_hevc_references, init_av_log, native,
        AVHWDeviceType::{self, *},
        AVPixelFormat,
    },
    ffmpeg_ram::CodecInfo,
};
use std::{ffi::CString, ptr, slice, time::Instant};

const DECODE_ERROR: i32 = HwcodecErrno::HWCODEC_ERR_COMMON as i32;

#[derive(Debug, Clone)]
pub struct DecodeContext {
    pub name: String,
    pub device_type: AVHWDeviceType,
    pub thread_count: i32,
}

pub struct DecodeFrame {
    pub pixfmt: AVPixelFormat,
    pub width: i32,
    pub height: i32,
    pub data: Vec<Vec<u8>>,
    pub linesize: Vec<i32>,
    pub key: bool,
}

impl std::fmt::Display for DecodeFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::from("data:");
        for data in self.data.iter() {
            s.push_str(format!("{} ", data.len()).as_str());
        }
        s.push_str(", linesize:");
        for linesize in self.linesize.iter() {
            s.push_str(format!("{} ", linesize).as_str());
        }

        write!(
            f,
            "fixfmt:{}, width:{}, height:{},key:{}, {}",
            self.pixfmt as i32, self.width, self.height, self.key, s,
        )
    }
}

#[derive(Default)]
struct DecodeOutput {
    frames: Vec<DecodeFrame>,
    reusable: Vec<DecodeFrame>,
}

impl DecodeOutput {
    fn begin_packet(&mut self) {
        // An input packet may produce no output; keep the last batch's spare planes.
        if !self.frames.is_empty() {
            self.reusable.clear();
            self.reusable.append(&mut self.frames);
        }
    }

    // The source must be a live FFmpeg video frame with referenced planes.
    unsafe fn copy_frame(&mut self, source: &native::AVFrame, key: bool) -> Result<(), i32> {
        let (format, planes) = match source.format {
            format if format == AVPixelFormat::AV_PIX_FMT_YUV420P as i32 => {
                (AVPixelFormat::AV_PIX_FMT_YUV420P, 3)
            }
            format if format == AVPixelFormat::AV_PIX_FMT_NV12 as i32 => {
                (AVPixelFormat::AV_PIX_FMT_NV12, 2)
            }
            format => {
                log::trace!("Unsupported decoded pixel format {format}");
                return Err(DECODE_ERROR);
            }
        };
        let width = usize::try_from(source.width).map_err(|_| DECODE_ERROR)?;
        let height = usize::try_from(source.height).map_err(|_| DECODE_ERROR)?;
        if width == 0 || height == 0 {
            return Err(DECODE_ERROR);
        }

        let mut frame = self.reusable.pop().unwrap_or_else(|| DecodeFrame {
            pixfmt: format,
            width: source.width,
            height: source.height,
            data: Vec::new(),
            linesize: Vec::new(),
            key,
        });
        frame.pixfmt = format;
        frame.width = source.width;
        frame.height = source.height;
        frame.key = key;
        frame.data.resize_with(planes, Vec::new);
        frame.linesize.clear();

        for (plane, data) in frame.data.iter_mut().enumerate() {
            let (bytes, rows) = if plane == 0 {
                (width, height)
            } else if format == AVPixelFormat::AV_PIX_FMT_NV12 {
                (width.div_ceil(2) * 2, height.div_ceil(2))
            } else {
                (width.div_ceil(2), height.div_ceil(2))
            };
            frame
                .linesize
                .push(copy_plane(source, plane, bytes, rows, data)?);
        }

        self.frames.push(frame);
        Ok(())
    }
}

unsafe fn copy_plane(
    frame: &native::AVFrame,
    plane: usize,
    row_bytes: usize,
    rows: usize,
    output: &mut Vec<u8>,
) -> Result<i32, i32> {
    let src = frame.data[plane];
    let step = frame.linesize[plane];
    let stride = step.checked_abs().ok_or(DECODE_ERROR)? as usize;
    let buffer = unsafe { native::av_frame_get_plane_buffer(frame, plane as i32).as_ref() }
        .ok_or(DECODE_ERROR)?;
    if src.is_null() || buffer.data.is_null() || stride < row_bytes {
        return Err(DECODE_ERROR);
    }

    let offset = (src as usize)
        .checked_sub(buffer.data as usize)
        .ok_or(DECODE_ERROR)?;
    let span = (rows - 1).checked_mul(stride).ok_or(DECODE_ERROR)?;
    let last_row = if step < 0 {
        offset.checked_sub(span).ok_or(DECODE_ERROR)?;
        offset
    } else {
        offset.checked_add(span).ok_or(DECODE_ERROR)?
    };
    if last_row.checked_add(row_bytes).ok_or(DECODE_ERROR)? > buffer.size {
        return Err(DECODE_ERROR);
    }

    let length = rows.checked_mul(stride).ok_or(DECODE_ERROR)?;
    output
        .try_reserve(length.saturating_sub(output.len()))
        .map_err(|_| DECODE_ERROR)?;
    output.resize(length, 0);
    for (row, destination) in output.chunks_exact_mut(stride).enumerate() {
        // Only visible bytes are initialized by the decoder. Keep the negotiated stride in Rust.
        let source =
            unsafe { slice::from_raw_parts(src.offset(row as isize * step as isize), row_bytes) };
        destination[..row_bytes].copy_from_slice(source);
        destination[row_bytes..].fill(0);
    }
    Ok(stride as i32)
}

struct NativeDecoder {
    codec: *mut native::AVCodecContext,
    picture: *mut native::AVFrame,
    transferred: *mut native::AVFrame,
    packet: *mut native::AVPacket,
    hevc: bool,
}

impl NativeDecoder {
    fn new(ctx: &DecodeContext) -> Result<Self, i32> {
        let name = CString::new(ctx.name.as_str()).map_err(|_| DECODE_ERROR)?;
        let hevc = ctx.name.contains("hevc");
        if !hevc && !ctx.name.contains("h264") {
            return Err(DECODE_ERROR);
        }
        unsafe {
            let codec = native::avcodec_find_decoder_by_name(name.as_ptr());
            if codec.is_null() {
                return Err(DECODE_ERROR);
            }
            let mut decoder = Self {
                codec: native::avcodec_alloc_context3(codec),
                picture: ptr::null_mut(),
                transferred: ptr::null_mut(),
                packet: ptr::null_mut(),
                hevc,
            };
            let config = decoder.codec.as_mut().ok_or(DECODE_ERROR)?;
            config.flags |= native::AV_CODEC_FLAG_LOW_DELAY as i32;
            config.thread_type = native::FF_THREAD_SLICE as i32;
            config.thread_count = ctx.thread_count;

            if ctx.name.contains("qsv") {
                check(native::av_opt_set(
                    config.priv_data,
                    c"async_depth".as_ptr(),
                    c"1".as_ptr(),
                    0,
                ))?;
                config.pkt_timebase = native::AVRational { num: 1, den: 30 };
            }

            if ctx.device_type != AV_HWDEVICE_TYPE_NONE {
                config.thread_count = 1;
                check(native::av_hwdevice_ctx_create(
                    &mut config.hw_device_ctx,
                    ctx.device_type as _,
                    ptr::null(),
                    ptr::null_mut(),
                    0,
                ))?;
                #[cfg(windows)]
                if ctx.device_type == AV_HWDEVICE_TYPE_D3D11VA
                    && !hwcodec_d3d11_supports_ram_decode(config.hw_device_ctx, hevc)
                {
                    return Err(DECODE_ERROR);
                }
                decoder.transferred = native::av_frame_alloc();
                if decoder.transferred.is_null() {
                    return Err(DECODE_ERROR);
                }
            }
            decoder.picture = native::av_frame_alloc();
            decoder.packet = native::av_packet_alloc();
            if decoder.picture.is_null() || decoder.packet.is_null() {
                return Err(DECODE_ERROR);
            }
            check(native::avcodec_open2(decoder.codec, codec, ptr::null_mut()))?;
            Ok(decoder)
        }
    }

    fn decode(&mut self, input: &[u8], output: &mut DecodeOutput) -> Result<(), i32> {
        let size = i32::try_from(input.len()).map_err(|_| DECODE_ERROR)?;
        if size == 0 {
            return Err(DECODE_ERROR);
        }
        unsafe {
            native::av_packet_unref(self.packet);
            // FFmpeg's owned packet supplies the zero padding required by its bit readers.
            check(native::av_new_packet(self.packet, size))?;
            ptr::copy_nonoverlapping(input.as_ptr(), (*self.packet).data, input.len());
        }
        let sent = self.send(output);
        unsafe { native::av_packet_unref(self.packet) };
        sent?;
        self.drain(output).map(|_| ())
    }

    fn send(&mut self, output: &mut DecodeOutput) -> Result<(), i32> {
        let mut result = unsafe { native::avcodec_send_packet(self.codec, self.packet) };
        if result == AGAIN && self.drain(output)? {
            result = unsafe { native::avcodec_send_packet(self.codec, self.packet) };
        }
        check(result)
    }

    fn drain(&mut self, output: &mut DecodeOutput) -> Result<bool, i32> {
        let mut received = false;
        loop {
            let result = unsafe { native::avcodec_receive_frame(self.codec, self.picture) };
            if result == AGAIN || result == END_OF_STREAM {
                return Ok(received);
            }
            check(result)?;
            received = true;
            unsafe {
                let frame = &*self.picture;
                let source = if self.transferred.is_null() {
                    frame
                } else {
                    if frame.hw_frames_ctx.is_null() {
                        return Err(DECODE_ERROR);
                    }
                    // The decoder may have changed resolution or output format.
                    native::av_frame_unref(self.transferred);
                    check(native::av_hwframe_transfer_data(self.transferred, frame, 0))?;
                    &*self.transferred
                };
                output.copy_frame(source, frame.flags & native::AV_FRAME_FLAG_KEY as i32 != 0)?;
            }
        }
    }
}

impl Drop for NativeDecoder {
    fn drop(&mut self) {
        unsafe {
            native::av_frame_free(&mut self.picture);
            native::av_frame_free(&mut self.transferred);
            native::av_packet_free(&mut self.packet);
            native::avcodec_free_context(&mut self.codec);
        }
    }
}

#[cfg(windows)]
extern "C" {
    fn hwcodec_d3d11_supports_ram_decode(reference: *const native::AVBufferRef, hevc: bool)
        -> bool;
}

fn check(result: i32) -> Result<(), i32> {
    if result < 0 {
        Err(result)
    } else {
        Ok(())
    }
}

pub struct Decoder {
    native: NativeDecoder,
    output: DecodeOutput,
    pub ctx: DecodeContext,
}

// FFmpeg's context belongs to this decoder. Native processing requires exclusive access.
unsafe impl Send for Decoder {}
unsafe impl Sync for Decoder {}

impl Decoder {
    pub fn new(ctx: DecodeContext) -> Result<Self, ()> {
        init_av_log();
        let native = NativeDecoder::new(&ctx).map_err(|error| {
            log::debug!("Failed to initialize {} decoder: {error}", ctx.name);
        })?;
        Ok(Self {
            native,
            output: DecodeOutput::default(),
            ctx,
        })
    }

    pub fn decode(&mut self, packet: &[u8]) -> Result<&mut Vec<DecodeFrame>, i32> {
        self.output.begin_packet();
        let context = self.native.codec.cast();
        if self.native.hevc {
            hwcodec_watch_hevc_references(context);
        }
        let result = self.native.decode(packet, &mut self.output);
        if self.native.hevc && hwcodec_take_hevc_reference_error(context) {
            return Err(HwcodecErrno::HWCODEC_ERR_HEVC_COULD_NOT_FIND_POC as i32);
        }
        result.map_err(|error| {
            log::trace!("{} decode failed: {error}", self.ctx.name);
            DECODE_ERROR
        })?;
        Ok(&mut self.output.frames)
    }

    pub fn available_decoders() -> Vec<CodecInfo> {
        use log::debug;

        #[allow(unused_mut)]
        let mut codecs: Vec<CodecInfo> = vec![];
        // windows disable nvdec to avoid gpu stuck
        #[cfg(target_os = "linux")]
        {
            let (nv, _, _) = crate::common::supported_gpu(false);
            debug!("Linux GPU support detected - NV: {}", nv);
            if nv {
                codecs.push(CodecInfo {
                    name: "h264".to_owned(),
                    format: H264,
                    hwdevice: AV_HWDEVICE_TYPE_CUDA,
                    priority: Priority::Good as _,
                    ..Default::default()
                });
                codecs.push(CodecInfo {
                    name: "hevc".to_owned(),
                    format: H265,
                    hwdevice: AV_HWDEVICE_TYPE_CUDA,
                    priority: Priority::Good as _,
                    ..Default::default()
                });
            }
        }

        #[cfg(target_os = "windows")]
        {
            codecs.append(&mut vec![
                CodecInfo {
                    name: "h264".to_owned(),
                    format: H264,
                    hwdevice: AV_HWDEVICE_TYPE_D3D11VA,
                    priority: Priority::Best as _,
                    ..Default::default()
                },
                CodecInfo {
                    name: "hevc".to_owned(),
                    format: H265,
                    hwdevice: AV_HWDEVICE_TYPE_D3D11VA,
                    priority: Priority::Best as _,
                    ..Default::default()
                },
            ]);
        }

        #[cfg(target_os = "linux")]
        {
            codecs.append(&mut vec![
                CodecInfo {
                    name: "h264".to_owned(),
                    format: H264,
                    hwdevice: AV_HWDEVICE_TYPE_VAAPI,
                    priority: Priority::Good as _,
                    ..Default::default()
                },
                CodecInfo {
                    name: "hevc".to_owned(),
                    format: H265,
                    hwdevice: AV_HWDEVICE_TYPE_VAAPI,
                    priority: Priority::Good as _,
                    ..Default::default()
                },
            ]);
        }

        #[cfg(target_os = "macos")]
        {
            let (_, _, h264, h265) = crate::common::get_video_toolbox_codec_support();
            debug!(
                "VideoToolbox decode support - H264: {}, H265: {}",
                h264, h265
            );
            if h264 {
                codecs.push(CodecInfo {
                    name: "h264".to_owned(),
                    format: H264,
                    hwdevice: AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
                    priority: Priority::Best as _,
                    ..Default::default()
                });
            }
            if h265 {
                codecs.push(CodecInfo {
                    name: "hevc".to_owned(),
                    format: H265,
                    hwdevice: AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
                    priority: Priority::Best as _,
                    ..Default::default()
                });
            }
        }

        let mut res = Vec::<CodecInfo>::new();
        let buf264 = &crate::common::DATA_H264_720P[..];
        let buf265 = &crate::common::DATA_H265_720P[..];

        for codec in codecs {
            // Skip if this format already exists in results
            if res
                .iter()
                .any(|existing: &CodecInfo| existing.format == codec.format)
            {
                continue;
            }

            debug!(
                "Testing decoder: {} (hwdevice: {:?})",
                codec.name, codec.hwdevice
            );

            let c = DecodeContext {
                name: codec.name.clone(),
                device_type: codec.hwdevice,
                thread_count: 4,
            };

            match Decoder::new(c) {
                Ok(mut decoder) => {
                    debug!("Decoder {} created successfully", codec.name);
                    let data = match codec.format {
                        H264 => buf264,
                        H265 => buf265,
                        _ => {
                            log::error!("Unsupported format: {:?}, skipping", codec.format);
                            continue;
                        }
                    };

                    let start = Instant::now();

                    match decoder.decode(data) {
                        Ok(frames) if !frames.is_empty() => {
                            let elapsed = start.elapsed().as_millis();

                            if elapsed < TEST_TIMEOUT_MS as _ {
                                debug!("Decoder {} test passed", codec.name);
                                res.push(codec);
                            } else {
                                debug!(
                                    "Decoder {} test failed - timeout: {}ms",
                                    codec.name, elapsed
                                );
                            }
                        }
                        Ok(_) => debug!("Decoder {} test produced no frame", codec.name),
                        Err(err) => {
                            debug!("Decoder {} test failed with error: {}", codec.name, err);
                        }
                    }
                }
                Err(_) => {
                    debug!("Failed to create decoder {}", codec.name);
                }
            }
        }

        let soft = CodecInfo::soft();
        if let Some(c) = soft.h264 {
            res.push(c);
        }
        if let Some(c) = soft.h265 {
            res.push(c);
        }

        res
    }
}

#[cfg(test)]
mod decode_output_tests {
    use super::*;

    fn emit(output: &mut DecodeOutput, width: i32, height: i32, format: AVPixelFormat, value: u8) {
        unsafe {
            let mut raw = native::av_frame_alloc();
            let frame = raw.as_mut().expect("test frame");
            frame.format = format as i32;
            frame.width = width;
            frame.height = height;
            assert_eq!(native::av_frame_get_buffer(raw, 64), 0);

            let planes = if format == AVPixelFormat::AV_PIX_FMT_NV12 {
                2
            } else {
                3
            };
            for plane in 0..planes {
                let rows = if plane == 0 {
                    height as usize
                } else {
                    (height as usize).div_ceil(2)
                };
                let stride = frame.linesize[plane] as usize;
                ptr::write_bytes(frame.data[plane], value + (plane != 0) as u8, stride * rows);
            }
            let result = output.copy_frame(frame, false);
            native::av_frame_free(&mut raw);
            result.unwrap();
        }
    }

    #[test]
    fn nv12_output_reuses_plane_allocations_and_copies_only_visible_bytes() {
        let mut output = DecodeOutput::default();
        emit(&mut output, 32, 16, AVPixelFormat::AV_PIX_FMT_NV12, 10);
        let pointers: Vec<_> = output.frames[0].data.iter().map(|p| p.as_ptr()).collect();
        output.begin_packet();
        emit(&mut output, 32, 16, AVPixelFormat::AV_PIX_FMT_NV12, 20);
        let frame = &output.frames[0];
        assert_eq!(
            frame.data.iter().map(|p| p.as_ptr()).collect::<Vec<_>>(),
            pointers
        );
        for (index, plane) in frame.data.iter().enumerate() {
            for row in plane.chunks_exact(frame.linesize[index] as usize) {
                assert!(row[..32].iter().all(|value| *value == 20 + index as u8));
                assert!(row[32..].iter().all(|value| *value == 0));
            }
        }
    }

    #[test]
    fn decoded_output_preserves_packet_order_and_resizes_odd_height_planes() {
        let mut output = DecodeOutput::default();
        emit(&mut output, 16, 8, AVPixelFormat::AV_PIX_FMT_NV12, 1);
        emit(&mut output, 16, 8, AVPixelFormat::AV_PIX_FMT_NV12, 2);
        output.begin_packet();
        emit(&mut output, 24, 9, AVPixelFormat::AV_PIX_FMT_YUV420P, 3);
        emit(&mut output, 24, 9, AVPixelFormat::AV_PIX_FMT_YUV420P, 4);
        assert_eq!(output.frames.len(), 2);
        for (frame, value) in output.frames.iter().zip([3, 4]) {
            assert_eq!((frame.width, frame.height), (24, 9));
            for (plane, (width, rows)) in [(24, 9), (12, 5), (12, 5)].into_iter().enumerate() {
                let stride = frame.linesize[plane] as usize;
                assert_eq!(frame.data[plane].len(), stride * rows);
                for row in frame.data[plane].chunks_exact(stride) {
                    assert!(row[..width]
                        .iter()
                        .all(|byte| *byte == value + (plane != 0) as u8));
                }
            }
        }
    }

    #[test]
    fn packet_without_decoded_output_exposes_no_stale_frame() {
        let mut output = DecodeOutput::default();
        emit(&mut output, 16, 8, AVPixelFormat::AV_PIX_FMT_NV12, 1);
        output.frames[0].data[0].reserve(4096);
        let capacity = output.frames[0].data[0].capacity();
        for _ in 0..8 {
            output.begin_packet();
            assert!(output.frames.is_empty());
        }
        emit(&mut output, 16, 8, AVPixelFormat::AV_PIX_FMT_NV12, 2);
        assert_eq!(output.frames[0].data[0].capacity(), capacity);
        assert_eq!(output.frames.len(), 1);
        assert_eq!(&output.frames[0].data[0][..16], &[2; 16]);
    }

    #[test]
    fn software_hevc_accepts_packets_that_need_more_input() {
        let mut decoder = Decoder::new(DecodeContext {
            name: "hevc".to_owned(),
            device_type: AV_HWDEVICE_TYPE_NONE,
            thread_count: 1,
        })
        .unwrap();
        let access_unit_delimiter = [0, 0, 0, 1, 0x46, 0x01, 0x10];
        assert!(decoder.decode(&access_unit_delimiter).unwrap().is_empty());
        let decoded = decoder.decode(crate::common::DATA_H265_720P).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!((decoded[0].width, decoded[0].height), (1280, 720));
        assert!(decoded[0].key);
        assert!(decoder.decode(&access_unit_delimiter).unwrap().is_empty());
    }

    #[test]
    fn missing_hevc_reference_is_attributed_only_to_its_decoder() {
        let (left, right) = (1_u8, 2_u8);
        let left = ptr::from_ref(&left).cast();
        let right = ptr::from_ref(&right).cast();
        hwcodec_watch_hevc_references(left);
        hwcodec_watch_hevc_references(right);
        unsafe {
            crate::ffmpeg::hwcodec_av_log_callback(
                right,
                crate::ffmpeg::AV_LOG_ERROR as i32,
                c"Could not find ref with POC 1".as_ptr(),
            );
        }
        assert!(!hwcodec_take_hevc_reference_error(left));
        assert!(hwcodec_take_hevc_reference_error(right));
        assert!(!hwcodec_take_hevc_reference_error(right));
    }
}
