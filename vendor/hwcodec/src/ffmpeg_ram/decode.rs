#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use super::Priority;
use crate::common::TEST_TIMEOUT_MS;
use crate::ffmpeg::{init_av_log, AVHWDeviceType::*};

use crate::{
    common::DataFormat::*,
    ffmpeg::{AVHWDeviceType, AVPixelFormat},
    ffmpeg_ram::{
        ffmpeg_ram_decode, ffmpeg_ram_free_decoder, ffmpeg_ram_new_decoder, CodecInfo,
        AV_NUM_DATA_POINTERS,
    },
};
use log::error;
use std::{
    ffi::{c_void, CString},
    os::raw::c_int,
    slice::from_raw_parts,
    time::Instant,
    vec,
};

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
        self.reusable.clear();
        self.reusable.append(&mut self.frames);
    }
}

pub struct Decoder {
    codec: *mut c_void,
    frames: *mut DecodeOutput,
    pub ctx: DecodeContext,
}

unsafe impl Send for Decoder {}
unsafe impl Sync for Decoder {}

impl Decoder {
    pub fn new(ctx: DecodeContext) -> Result<Self, ()> {
        init_av_log();
        unsafe {
            let codec = ffmpeg_ram_new_decoder(
                CString::new(ctx.name.as_str()).map_err(|_| ())?.as_ptr(),
                ctx.device_type as _,
                ctx.thread_count,
                Some(Decoder::callback),
            );

            if codec.is_null() {
                return Err(());
            }

            Ok(Decoder {
                codec,
                frames: Box::into_raw(Box::new(DecodeOutput::default())),
                ctx,
            })
        }
    }

    pub fn decode(&mut self, packet: &[u8]) -> Result<&mut Vec<DecodeFrame>, i32> {
        unsafe {
            (&mut *self.frames).begin_packet();
            let ret = ffmpeg_ram_decode(
                self.codec,
                packet.as_ptr(),
                packet.len() as c_int,
                self.frames as *const _ as *const c_void,
            );

            if ret < 0 {
                Err(ret)
            } else {
                Ok(&mut (*self.frames).frames)
            }
        }
    }

    unsafe extern "C" fn callback(
        obj: *const c_void,
        width: c_int,
        height: c_int,
        pixfmt: c_int,
        linesizes: *mut c_int,
        datas: *mut *mut u8,
        key: c_int,
    ) {
        let output = &mut *(obj as *mut DecodeOutput);
        let datas = from_raw_parts(datas, AV_NUM_DATA_POINTERS as _);
        let linesizes = from_raw_parts(linesizes, AV_NUM_DATA_POINTERS as _);
        let (format, planes) = if pixfmt == AVPixelFormat::AV_PIX_FMT_YUV420P as c_int {
            (AVPixelFormat::AV_PIX_FMT_YUV420P, 3)
        } else if pixfmt == AVPixelFormat::AV_PIX_FMT_NV12 as c_int {
            (AVPixelFormat::AV_PIX_FMT_NV12, 2)
        } else {
            error!("unsupported pixfmt {}", pixfmt);
            return;
        };

        if width <= 0 || height <= 0 {
            return;
        }
        let mut lengths = [0; 3];
        for plane in 0..planes {
            let rows = if plane == 0 {
                height as usize
            } else {
                (height as usize).div_ceil(2)
            };
            let Ok(stride) = usize::try_from(linesizes[plane]) else {
                return;
            };
            let Some(length) = stride.checked_mul(rows) else {
                return;
            };
            if length == 0 || datas[plane].is_null() {
                return;
            }
            lengths[plane] = length;
        }

        let mut frame = output
            .reusable
            .pop()
            .filter(|frame| {
                frame.pixfmt == format
                    && frame.width == width
                    && frame.height == height
                    && frame.linesize.as_slice() == &linesizes[..planes]
            })
            .unwrap_or_else(|| DecodeFrame {
                pixfmt: format,
                width,
                height,
                data: Vec::new(),
                linesize: Vec::new(),
                key: key != 0,
            });
        frame.pixfmt = format;
        frame.width = width;
        frame.height = height;
        frame.key = key != 0;
        frame.data.resize_with(planes, Vec::new);
        frame.linesize.clear();
        frame.linesize.extend_from_slice(&linesizes[..planes]);
        for plane in 0..planes {
            let bytes = from_raw_parts(datas[plane], lengths[plane]);
            frame.data[plane].clear();
            frame.data[plane].extend_from_slice(bytes);
        }
        output.frames.push(frame);
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
                        Ok(_) => {
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

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ffmpeg_ram_free_decoder(self.codec);
            self.codec = std::ptr::null_mut();
            let _ = Box::from_raw(self.frames);
        }
    }
}

#[cfg(test)]
mod decode_output_tests {
    use super::*;

    fn emit(output: &mut DecodeOutput, width: i32, height: i32, format: AVPixelFormat, value: u8) {
        let planes = if format == AVPixelFormat::AV_PIX_FMT_NV12 {
            2
        } else {
            3
        };
        let mut data = vec![vec![value; (width * height) as usize]];
        for _ in 1..planes {
            data.push(vec![value + 1; (width * ((height + 1) / 2)) as usize]);
        }
        let mut pointers = [std::ptr::null_mut(); AV_NUM_DATA_POINTERS as usize];
        let mut strides = [0; AV_NUM_DATA_POINTERS as usize];
        for index in 0..planes {
            pointers[index] = data[index].as_mut_ptr();
            strides[index] = width;
        }
        unsafe {
            Decoder::callback(
                output as *mut _ as _,
                width,
                height,
                format as _,
                strides.as_mut_ptr(),
                pointers.as_mut_ptr(),
                0,
            );
        }
    }

    #[test]
    fn nv12_output_reuses_plane_allocations_without_retaining_old_pixels() {
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
        assert!(frame.data[0].iter().all(|v| *v == 20));
        assert!(frame.data[1].iter().all(|v| *v == 21));
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
            assert_eq!(
                frame.data.iter().map(Vec::len).collect::<Vec<_>>(),
                [216, 120, 120]
            );
            assert!(frame.data[0].iter().all(|v| *v == value));
            assert!(frame.data[1].iter().all(|v| *v == value + 1));
        }
    }

    #[test]
    fn packet_without_decoded_output_exposes_no_stale_frame() {
        let mut output = DecodeOutput::default();
        emit(&mut output, 16, 8, AVPixelFormat::AV_PIX_FMT_NV12, 1);
        output.begin_packet();
        assert!(output.frames.is_empty());
        output.begin_packet();
        emit(&mut output, 16, 8, AVPixelFormat::AV_PIX_FMT_NV12, 2);
        assert_eq!(output.frames.len(), 1);
        assert!(output.frames[0].data[0].iter().all(|v| *v == 2));
    }
}
