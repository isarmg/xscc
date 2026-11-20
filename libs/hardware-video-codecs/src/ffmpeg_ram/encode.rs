use super::{ffmpeg_linesize_offset_length, CodecInfo, Priority, AGAIN, END_OF_STREAM};
#[cfg(any(windows, target_os = "linux"))]
use crate::common::Driver;
use crate::{
    common::{
        DataFormat::{self, *},
        Quality, RateControl, TEST_TIMEOUT_MS,
    },
    ffmpeg::{init_av_log, native, AVPixelFormat},
};
#[cfg(windows)]
use std::ffi::c_void;
use std::{
    ffi::{c_char, CString},
    fmt::Display,
    ptr, slice,
};

const ENCODE_ERROR: i32 = -1;
const PLANE_COUNT: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct EncodeContext {
    pub name: String,
    pub mc_name: Option<String>,
    pub width: i32,
    pub height: i32,
    pub pixfmt: AVPixelFormat,
    pub align: i32,
    pub fps: i32,
    pub gop: i32,
    pub rc: RateControl,
    pub quality: Quality,
    pub kbs: i32,
    pub q: i32,
    pub thread_count: i32,
}

pub struct EncodeFrame {
    pub data: Vec<u8>,
    pub pts: i64,
    pub key: i32,
}

impl Display for EncodeFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "encode len:{}, pts:{}", self.data.len(), self.pts)
    }
}

struct InputLayout {
    linesize: [i32; PLANE_COUNT],
    offsets: [i32; PLANE_COUNT],
    length: i32,
}

impl InputLayout {
    fn from_frame(frame: &native::AVFrame) -> Result<Self, i32> {
        let height = frame.height;
        let chroma_height = height / 2 + height % 2;
        let mut offsets = [0; PLANE_COUNT];
        offsets[0] = frame.linesize[0].checked_mul(height).ok_or(ENCODE_ERROR)?;
        let second_end = offsets[0]
            .checked_add(
                frame.linesize[1]
                    .checked_mul(chroma_height)
                    .ok_or(ENCODE_ERROR)?,
            )
            .ok_or(ENCODE_ERROR)?;
        let length = if frame.format == AVPixelFormat::AV_PIX_FMT_YUV420P as i32 {
            offsets[1] = second_end;
            second_end
                .checked_add(
                    frame.linesize[2]
                        .checked_mul(chroma_height)
                        .ok_or(ENCODE_ERROR)?,
                )
                .ok_or(ENCODE_ERROR)?
        } else {
            second_end
        };
        Ok(Self {
            linesize: frame.linesize,
            offsets,
            length,
        })
    }

    // The frame must own writable FFmpeg storage and match the input layout's format and size.
    unsafe fn copy_to(&self, src: &[u8], frame: &mut native::AVFrame) -> Result<(), i32> {
        if src.len() < self.length as usize {
            return Err(ENCODE_ERROR);
        }
        let width = frame.width as usize;
        let height = frame.height as usize;
        let format = frame.format;
        let luma = (0, width, height);
        let chroma = if format == AVPixelFormat::AV_PIX_FMT_NV12 as i32 {
            [
                luma,
                (
                    self.offsets[0] as usize,
                    2 * width.div_ceil(2),
                    height.div_ceil(2),
                ),
                (0, 0, 0),
            ]
        } else {
            [
                luma,
                (
                    self.offsets[0] as usize,
                    width.div_ceil(2),
                    height.div_ceil(2),
                ),
                (
                    self.offsets[1] as usize,
                    width.div_ceil(2),
                    height.div_ceil(2),
                ),
            ]
        };
        for (plane, (offset, bytes, rows)) in chroma.into_iter().enumerate() {
            if rows == 0 {
                break;
            }
            let src_stride = self.linesize[plane] as usize;
            let dst_stride = usize::try_from(frame.linesize[plane]).map_err(|_| ENCODE_ERROR)?;
            if dst_stride < bytes || frame.data[plane].is_null() {
                return Err(ENCODE_ERROR);
            }
            for row in 0..rows {
                let start = offset + row * src_stride;
                // Keep AVFrame's refcounted data pointers. Hardware may retain the frame after send.
                unsafe {
                    let destination = frame.data[plane].add(row * dst_stride);
                    ptr::copy_nonoverlapping(
                        src[start..start + bytes].as_ptr(),
                        destination,
                        bytes,
                    );
                    ptr::write_bytes(destination.add(bytes), 0, dst_stride - bytes);
                }
            }
        }
        Ok(())
    }
}

struct NativeEncoder {
    codec: *mut native::AVCodecContext,
    frame: *mut native::AVFrame,
    hardware_frame: *mut native::AVFrame,
    packet: *mut native::AVPacket,
    #[cfg(windows)]
    retirement: *mut c_void,
    #[cfg(windows)]
    retiring: bool,
}

impl NativeEncoder {
    fn new(ctx: &EncodeContext) -> Result<(Self, InputLayout), i32> {
        if ctx.width <= 0
            || ctx.height <= 0
            || ctx.width % 2 != 0
            || ctx.height % 2 != 0
            || ctx.fps <= 0
            || !valid_alignment(ctx.align)
        {
            return Err(ENCODE_ERROR);
        }
        let name = CString::new(ctx.name.as_str()).map_err(|_| ENCODE_ERROR)?;
        let media_codec =
            CString::new(ctx.mc_name.as_deref().unwrap_or_default()).map_err(|_| ENCODE_ERROR)?;
        let gpu = std::env::var("RUSTDESK_HWCODEC_NVENC_GPU")
            .ok()
            .and_then(|gpu| gpu.parse().ok())
            .unwrap_or(-1);
        unsafe {
            let codec = native::avcodec_find_encoder_by_name(name.as_ptr());
            if codec.is_null() {
                return Err(ENCODE_ERROR);
            }
            let mut encoder = Self {
                codec: native::avcodec_alloc_context3(codec),
                frame: native::av_frame_alloc(),
                hardware_frame: ptr::null_mut(),
                packet: native::av_packet_alloc(),
                #[cfg(windows)]
                retirement: ptr::null_mut(),
                #[cfg(windows)]
                retiring: false,
            };
            if encoder.codec.is_null() || encoder.frame.is_null() || encoder.packet.is_null() {
                return Err(ENCODE_ERROR);
            }
            let target = &mut *encoder.codec;
            target.width = ctx.width;
            target.height = ctx.height;
            target.pix_fmt = ctx.pixfmt as _;
            target.sw_pix_fmt = ctx.pixfmt as _;

            if ctx.name.contains("vaapi") {
                encoder.init_hardware(
                    native::AVHWDeviceType_AV_HWDEVICE_TYPE_VAAPI,
                    native::AVPixelFormat_AV_PIX_FMT_VAAPI,
                    ptr::null(),
                )?;
            }
            #[cfg(windows)]
            if ctx.name.contains("nvenc") {
                let index = hwcodec_nvidia_adapter_index();
                let device = (index >= 0)
                    .then(|| CString::new(index.to_string()).expect("numeric adapter index"));
                encoder.init_hardware(
                    native::AVHWDeviceType_AV_HWDEVICE_TYPE_D3D11VA,
                    native::AVPixelFormat_AV_PIX_FMT_D3D11,
                    device
                        .as_ref()
                        .map_or(ptr::null(), |device| device.as_ptr()),
                )?;
            }

            let frame = &mut *encoder.frame;
            frame.format = ctx.pixfmt as _;
            frame.width = ctx.width;
            frame.height = ctx.height;
            check(native::av_frame_get_buffer(encoder.frame, ctx.align))?;
            let layout = InputLayout::from_frame(&*encoder.frame)?;

            if !hwcodec_configure_ram_encoder(
                encoder.codec,
                name.as_ptr(),
                media_codec.as_ptr(),
                ctx.kbs,
                ctx.gop,
                ctx.fps,
                ctx.rc as i32,
                ctx.q,
                gpu,
            ) {
                return Err(ENCODE_ERROR);
            }
            check(native::avcodec_open2(encoder.codec, codec, ptr::null_mut()))?;
            Ok((encoder, layout))
        }
    }

    unsafe fn init_hardware(
        &mut self,
        kind: native::AVHWDeviceType,
        format: native::AVPixelFormat,
        device: *const c_char,
    ) -> Result<(), i32> {
        let mut reference = ptr::null_mut();
        check(native::av_hwdevice_ctx_create(
            &mut reference,
            kind,
            device,
            ptr::null_mut(),
            0,
        ))?;
        let frames = native::av_hwframe_ctx_alloc(reference);
        native::av_buffer_unref(&mut reference);
        if frames.is_null() {
            return Err(ENCODE_ERROR);
        }
        let target = &mut *self.codec;
        target.hw_frames_ctx = frames;
        target.pix_fmt = format;
        let context = &mut *(*frames).data.cast::<native::AVHWFramesContext>();
        context.format = format;
        context.sw_format = target.sw_pix_fmt;
        context.width = target.width;
        context.height = target.height;
        // The frame pool must be able to hand out a new buffer while the encoder holds a prior input.
        context.initial_pool_size = 0;
        check(native::av_hwframe_ctx_init(frames))?;
        self.hardware_frame = native::av_frame_alloc();
        if self.hardware_frame.is_null() {
            return Err(ENCODE_ERROR);
        }
        check(native::av_hwframe_get_buffer(
            frames,
            self.hardware_frame,
            0,
        ))
    }

    fn encode(
        &mut self,
        input: &[u8],
        layout: &InputLayout,
        pts: i64,
        frames: &mut Vec<EncodeFrame>,
    ) -> Result<(), i32> {
        #[cfg(windows)]
        if self.retiring {
            return Err(-(native::EPIPE as i32));
        }
        let frame = unsafe {
            check(native::av_frame_make_writable(self.frame))?;
            layout.copy_to(input, &mut *self.frame)?;
            if self.hardware_frame.is_null() {
                self.frame
            } else {
                if native::av_frame_is_writable(self.hardware_frame) == 0 {
                    native::av_frame_unref(self.hardware_frame);
                    check(native::av_hwframe_get_buffer(
                        (*self.codec).hw_frames_ctx,
                        self.hardware_frame,
                        0,
                    ))?;
                }
                check(native::av_hwframe_transfer_data(
                    self.hardware_frame,
                    self.frame,
                    0,
                ))?;
                self.hardware_frame
            }
        };
        unsafe { (*frame).pts = pts };
        let mut result = unsafe { native::avcodec_send_frame(self.codec, frame) };
        if result == AGAIN && self.drain(frames)? {
            result = unsafe { native::avcodec_send_frame(self.codec, frame) };
        }
        check(result)?;
        self.drain(frames).map(|_| ())
    }

    fn drain(&mut self, frames: &mut Vec<EncodeFrame>) -> Result<bool, i32> {
        let mut drained = false;
        loop {
            let result = unsafe { native::avcodec_receive_packet(self.codec, self.packet) };
            if result == AGAIN || result == END_OF_STREAM {
                return Ok(drained);
            }
            check(result)?;
            unsafe {
                let packet = &*self.packet;
                if packet.data.is_null() || packet.size <= 0 {
                    native::av_packet_unref(self.packet);
                    return Err(ENCODE_ERROR);
                }
                frames.push(EncodeFrame {
                    data: slice::from_raw_parts(packet.data, packet.size as usize).to_vec(),
                    pts: packet.pts,
                    key: i32::from(packet.flags & native::AV_PKT_FLAG_KEY as i32 != 0),
                });
                native::av_packet_unref(self.packet);
            }
            drained = true;
        }
    }
}

impl Drop for NativeEncoder {
    fn drop(&mut self) {
        unsafe {
            native::av_packet_free(&mut self.packet);
            native::av_frame_free(&mut self.frame);
            native::av_frame_free(&mut self.hardware_frame);
            native::avcodec_free_context(&mut self.codec);
            #[cfg(windows)]
            hwcodec_free_ram_retirement(self.retirement);
        }
    }
}

fn valid_alignment(align: i32) -> bool {
    align == 0 || (align > 0 && (align as u32).is_power_of_two())
}

fn check(result: i32) -> Result<(), i32> {
    if result < 0 {
        Err(result)
    } else {
        Ok(())
    }
}

extern "C" {
    fn hwcodec_configure_ram_encoder(
        codec: *mut native::AVCodecContext,
        name: *const c_char,
        media_codec: *const c_char,
        kbs: i32,
        gop: i32,
        fps: i32,
        rate_control: i32,
        quality: i32,
        gpu: i32,
    ) -> bool;
}

#[cfg(windows)]
extern "C" {
    fn hwcodec_nvidia_adapter_index() -> i32;
    fn hwcodec_free_ram_retirement(state: *mut c_void);
    fn hwcodec_retire_ram_encoder(
        codec: *mut native::AVCodecContext,
        packet: *mut native::AVPacket,
        state: *mut *mut c_void,
        capture_device: *mut c_void,
        timeout_ms: i32,
    ) -> i32;
}

pub(super) fn image_layout(
    format: AVPixelFormat,
    width: usize,
    height: usize,
    align: usize,
) -> Result<(Vec<i32>, Vec<i32>, i32), ()> {
    let width = i32::try_from(width).map_err(|_| ())?;
    let height = i32::try_from(height).map_err(|_| ())?;
    let align = i32::try_from(align).map_err(|_| ())?;
    if width <= 0 || height <= 0 || !valid_alignment(align) {
        return Err(());
    }
    let layout = unsafe {
        let mut frame = native::av_frame_alloc();
        let image = frame.as_mut().ok_or(())?;
        image.format = format as i32;
        image.width = width;
        image.height = height;
        let layout = check(native::av_frame_get_buffer(frame, align))
            .and_then(|_| InputLayout::from_frame(&*frame));
        native::av_frame_free(&mut frame);
        layout.map_err(|_| ())?
    };
    Ok((
        layout.linesize.to_vec(),
        layout.offsets.to_vec(),
        layout.length,
    ))
}

pub struct Encoder {
    native: NativeEncoder,
    frames: Vec<EncodeFrame>,
    layout: InputLayout,
    output: OutputProgress,
    pub ctx: EncodeContext,
    pub linesize: Vec<i32>,
    pub offset: Vec<i32>,
    pub length: i32,
}

#[derive(Default)]
struct OutputProgress(u32);

impl OutputProgress {
    fn check(&mut self, frames_per_second: i32, produced_output: bool) -> Result<(), i32> {
        if produced_output {
            self.0 = 0;
            return Ok(());
        }
        // Base the limit on accepted frames so an idle desk does not age out the encoder.
        self.0 = self.0.saturating_add(1);
        if self.0 >= frames_per_second.clamp(3, 120) as u32 {
            return Err(-(native::ETIMEDOUT as i32));
        }
        Ok(())
    }
}

impl Encoder {
    pub fn new(ctx: EncodeContext) -> Result<Self, ()> {
        init_av_log();
        let (native, layout) = NativeEncoder::new(&ctx).map_err(|error| {
            log::debug!("Failed to initialize {} encoder: {error}", ctx.name);
        })?;
        Ok(Self {
            native,
            frames: Vec::new(),
            output: OutputProgress::default(),
            linesize: layout.linesize.to_vec(),
            offset: layout.offsets.to_vec(),
            length: layout.length,
            layout,
            ctx,
        })
    }

    pub fn encode(&mut self, data: &[u8], ms: i64) -> Result<&mut Vec<EncodeFrame>, i32> {
        self.frames.clear();
        self.native
            .encode(data, &self.layout, ms, &mut self.frames)?;
        self.output.check(self.ctx.fps, !self.frames.is_empty())?;
        Ok(&mut self.frames)
    }

    #[cfg(windows)]
    pub fn retire(&mut self, capture_device: *mut c_void, timeout_ms: i32) -> Result<(), i32> {
        self.native.retiring = true;
        check(unsafe {
            hwcodec_retire_ram_encoder(
                self.native.codec,
                self.native.packet,
                &mut self.native.retirement,
                capture_device,
                timeout_ms,
            )
        })
    }

    pub fn set_bitrate(&mut self, kbs: i32) -> Result<(), ()> {
        if kbs > 0 {
            let context = unsafe { &mut *self.native.codec };
            context.bit_rate = i64::from(kbs) * 1000;
            if self.ctx.name.contains("qsv") {
                context.rc_max_rate = context.bit_rate;
            }
        }
        Ok(())
    }

    pub fn format_from_name(name: String) -> Result<DataFormat, ()> {
        if name.contains("h264") {
            return Ok(H264);
        } else if name.contains("hevc") {
            return Ok(H265);
        } else if name.contains("vp8") {
            return Ok(VP8);
        } else if name.contains("vp9") {
            return Ok(VP9);
        } else if name.contains("av1") {
            return Ok(AV1);
        }
        Err(())
    }

    pub fn available_encoders(ctx: EncodeContext, _sdk: Option<String>) -> Vec<CodecInfo> {
        use log::debug;

        if !(cfg!(windows) || cfg!(target_os = "linux") || cfg!(target_os = "macos")) {
            return vec![];
        }
        let mut codecs: Vec<CodecInfo> = vec![];
        #[cfg(any(windows, target_os = "linux"))]
        {
            let contains = |_vendor: Driver, _format: DataFormat| {
                #[cfg(all(windows, feature = "vram"))]
                {
                    if let Some(_sdk) = _sdk.as_ref() {
                        if !_sdk.is_empty() {
                            if let Ok(available) =
                                crate::vram::Available::deserialize(_sdk.as_str())
                            {
                                return available.contains(true, _vendor, _format);
                            }
                        }
                    }
                }
                true
            };
            let (_nv, amf, _intel) = crate::common::supported_gpu(true);
            debug!(
                "GPU support detected - NV: {}, AMF: {}, Intel: {}",
                _nv, amf, _intel
            );

            #[cfg(windows)]
            if _intel && contains(Driver::MFX, H264) {
                codecs.push(CodecInfo {
                    name: "h264_qsv".to_owned(),
                    format: H264,
                    priority: Priority::Best as _,
                    ..Default::default()
                });
            }
            #[cfg(windows)]
            if _intel && contains(Driver::MFX, H265) {
                codecs.push(CodecInfo {
                    name: "hevc_qsv".to_owned(),
                    format: H265,
                    priority: Priority::Best as _,
                    ..Default::default()
                });
            }
            if _nv && contains(Driver::NV, H264) {
                codecs.push(CodecInfo {
                    name: "h264_nvenc".to_owned(),
                    format: H264,
                    priority: Priority::Best as _,
                    ..Default::default()
                });
            }
            if _nv && contains(Driver::NV, H265) {
                codecs.push(CodecInfo {
                    name: "hevc_nvenc".to_owned(),
                    format: H265,
                    priority: Priority::Best as _,
                    ..Default::default()
                });
            }
            if amf && contains(Driver::AMF, H264) {
                codecs.push(CodecInfo {
                    name: "h264_amf".to_owned(),
                    format: H264,
                    priority: Priority::Best as _,
                    ..Default::default()
                });
            }
            if amf {
                // sdk not use h265
                codecs.push(CodecInfo {
                    name: "hevc_amf".to_owned(),
                    format: H265,
                    priority: Priority::Best as _,
                    ..Default::default()
                });
            }
            #[cfg(target_os = "linux")]
            {
                codecs.push(CodecInfo {
                    name: "h264_vaapi".to_owned(),
                    format: H264,
                    priority: Priority::Good as _,
                    ..Default::default()
                });
                // remove because poor quality on one of my computer
                // codecs.push(CodecInfo {
                //     name: "hevc_vaapi".to_owned(),
                //     format: H265,
                //     priority: Priority::Good as _,
                //     ..Default::default()
                // });
            }
        }

        #[cfg(target_os = "macos")]
        {
            let (_h264, h265, _, _) = crate::common::get_video_toolbox_codec_support();
            // h264 encode failed too often, not AV_CODEC_CAP_HARDWARE
            // if h264 {
            //     codecs.push(CodecInfo {
            //         name: "h264_videotoolbox".to_owned(),
            //         format: H264,
            //         priority: Priority::Best as _,
            //         ..Default::default()
            //     });
            // }
            if h265 {
                codecs.push(CodecInfo {
                    name: "hevc_videotoolbox".to_owned(),
                    format: H265,
                    priority: Priority::Best as _,
                    ..Default::default()
                });
            }
        }

        // qsv doesn't support yuv420p
        codecs.retain(|c| {
            let ctx = ctx.clone();
            if ctx.pixfmt == AVPixelFormat::AV_PIX_FMT_YUV420P && c.name.contains("qsv") {
                return false;
            }
            return true;
        });

        let mut res = vec![];

        if let Ok(yuv) = Encoder::dummy_yuv(ctx.clone()) {
            for codec in codecs {
                // Skip if this format already exists in results
                if res
                    .iter()
                    .any(|existing: &CodecInfo| existing.format == codec.format)
                {
                    continue;
                }

                debug!("Testing encoder: {}", codec.name);

                let c = EncodeContext {
                    name: codec.name.clone(),
                    mc_name: codec.mc_name.clone(),
                    ..ctx
                };

                match Encoder::new(c) {
                    Ok(mut encoder) => {
                        debug!("Encoder {} created successfully", codec.name);
                        let mut passed = false;
                        let mut last_err: Option<i32> = None;

                        let max_attempts = if cfg!(all(target_os = "macos", target_arch = "x86_64"))
                        {
                            3
                        } else {
                            1
                        };
                        for attempt in 0..max_attempts {
                            let pts = (attempt as i64) * 33; // 33ms is an approximation for 30 FPS (1000 / 30)
                            let start = std::time::Instant::now();
                            match encoder.encode(&yuv, pts) {
                                Ok(frames) => {
                                    let elapsed = start.elapsed().as_millis();

                                    if frames.len() == 1 {
                                        if frames[0].key == 1 && elapsed < TEST_TIMEOUT_MS as _ {
                                            debug!(
                                                "Encoder {} test passed on attempt {}",
                                                codec.name,
                                                attempt + 1
                                            );
                                            res.push(codec.clone());
                                            passed = true;
                                            break;
                                        } else {
                                            debug!(
                                                "Encoder {} test failed on attempt {} - key: {}, timeout: {}ms",
                                                codec.name,
                                                attempt + 1,
                                                frames[0].key,
                                                elapsed
                                            );
                                        }
                                    } else {
                                        debug!(
                                            "Encoder {} test failed on attempt {} - wrong frame count: {}",
                                            codec.name,
                                            attempt + 1,
                                            frames.len()
                                        );
                                    }
                                }
                                Err(err) => {
                                    last_err = Some(err);
                                    debug!(
                                        "Encoder {} test attempt {} returned error: {}",
                                        codec.name,
                                        attempt + 1,
                                        err
                                    );
                                }
                            }
                        }

                        if !passed {
                            debug!(
                                "Encoder {} test failed after retries{}",
                                codec.name,
                                last_err
                                    .map(|e| format!(" (last err: {})", e))
                                    .unwrap_or_default()
                            );
                        }
                    }
                    Err(_) => {
                        debug!("Failed to create encoder {}", codec.name);
                    }
                }
            }
        } else {
            debug!("Failed to generate dummy YUV data");
        }

        res
    }

    fn dummy_yuv(ctx: EncodeContext) -> Result<Vec<u8>, ()> {
        let mut yuv = vec![];
        if let Ok((_, _, len)) = ffmpeg_linesize_offset_length(
            ctx.pixfmt,
            ctx.width as _,
            ctx.height as _,
            ctx.align as _,
        ) {
            yuv.resize(len as _, 0);
            return Ok(yuv);
        }

        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_input_uses_refcounted_frame_storage() {
        unsafe {
            let mut current = native::av_frame_alloc();
            let mut retained = native::av_frame_alloc();
            let frame = current.as_mut().expect("test frame");
            frame.width = 32;
            frame.height = 18;
            frame.format = AVPixelFormat::AV_PIX_FMT_NV12 as i32;
            assert_eq!(native::av_frame_get_buffer(current, 64), 0);
            let frame = &mut *current;
            let layout = InputLayout::from_frame(frame).unwrap();

            let mut pixels = vec![11; layout.length as usize];
            let original = frame.data[0];
            layout.copy_to(&pixels, frame).unwrap();
            assert_eq!(frame.data[0], original);
            assert_eq!(native::av_frame_ref(retained, current), 0);

            pixels.fill(22);
            assert_eq!(native::av_frame_make_writable(current), 0);
            layout.copy_to(&pixels, &mut *current).unwrap();
            drop(pixels);
            assert_eq!(*(*retained).data[0], 11);
            assert_eq!(*(*current).data[0], 22);
            native::av_frame_free(&mut current);
            native::av_frame_free(&mut retained);
        }
    }

    #[test]
    fn accepted_inputs_without_output_eventually_report_a_stalled_encoder() {
        let mut output = OutputProgress::default();
        assert_eq!(output.check(3, false), Ok(()));
        assert_eq!(output.check(3, false), Ok(()));
        assert_eq!(output.check(3, true), Ok(()));
        assert_eq!(output.check(3, false), Ok(()));
        assert_eq!(output.check(3, false), Ok(()));
        assert_eq!(output.check(3, false), Err(-(native::ETIMEDOUT as i32)));
        assert_eq!(output.check(3, true), Ok(()));
        assert_eq!(output.check(3, false), Ok(()));
    }
}
