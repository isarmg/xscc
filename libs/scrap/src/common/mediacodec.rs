use hbb_common::{
    anyhow::{Context, Error},
    bail, log, ResultType,
};
use ndk::media::{
    media_codec::{MediaCodec, MediaCodecDirection, MediaFormat},
    NdkMediaError,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use crate::{
    android::ffi::get_decoder_infos, CodecFormat, ImageFormat, ImageRgb, NV12ToABGR, NV12ToARGB,
};

const H264_MIME_TYPE: &str = "video/avc";
const H265_MIME_TYPE: &str = "video/hevc";
const COLOR_YUV420_SEMIPLANAR: i32 = 21;

pub static H264_DECODER_SUPPORT: AtomicBool = AtomicBool::new(false);
pub static H265_DECODER_SUPPORT: AtomicBool = AtomicBool::new(false);

pub struct MediaCodecDecoder {
    decoder: MediaCodec,
    mime: &'static str,
    configuration: Option<(i32, i32, i32)>,
    pending_output: usize,
    decoded_outputs: usize,
}

impl MediaCodecDecoder {
    pub fn new(format: CodecFormat) -> Option<MediaCodecDecoder> {
        let mime = match format {
            CodecFormat::H264 => H264_MIME_TYPE,
            CodecFormat::H265 => H265_MIME_TYPE,
            _ => return None,
        };
        // Only names reported as hardware decoders by Android are eligible.
        for info in get_decoder_infos(mime) {
            // Planar/flexible/vendor layouts need per-plane metadata, unavailable here.
            if !info.color_formats.contains(&COLOR_YUV420_SEMIPLANAR) {
                continue;
            }
            if let Some(decoder) = MediaCodec::from_codec_name(&info.name) {
                return Some(Self {
                    decoder,
                    mime,
                    configuration: None,
                    pending_output: 0,
                    decoded_outputs: 0,
                });
            }
        }
        None
    }

    pub fn set_video_size(&mut self, width: i32, height: i32, fps: i32) -> ResultType<()> {
        if width <= 0 || height <= 0 {
            bail!("Invalid MediaCodec input dimensions: {width}x{height}");
        }
        if !(1..=base::session_protocol::MAX_SESSION_FPS as i32).contains(&fps) {
            bail!("Invalid MediaCodec frame rate: {fps}");
        }
        if let Some((old_width, old_height, old_fps)) = self.configuration {
            if (old_width, old_height) == (width, height) {
                if old_fps != fps {
                    // Keep parameter sets and queued frames when only the rate changes.
                    let parameters = MediaFormat::new();
                    parameters.set_f32("operating-rate", fps as f32);
                    self.decoder.set_parameters(parameters).with_context(|| {
                        format!("Update MediaCodec operating rate to {fps} fps")
                    })?;
                    self.configuration = Some((width, height, fps));
                }
                return Ok(());
            }
        }
        if self.configuration.take().is_some() {
            self.decoder.stop()?;
        }
        self.pending_output = 0;
        self.decoded_outputs = 0;
        let format = MediaFormat::new();
        format.set_str("mime", self.mime);
        format.set_i32("width", width);
        format.set_i32("height", height);
        format.set_i32("color-format", COLOR_YUV420_SEMIPLANAR);
        format.set_i32("frame-rate", fps);
        format.set_f32("operating-rate", fps as f32);
        format.set_i32("priority", 0);
        format.set_i32("low-latency", 1);
        self.decoder
            .configure(&format, None, MediaCodecDirection::Decoder)
            .with_context(|| {
                format!("Configure realtime MediaCodec {width}x{height} at {fps} fps")
            })?;
        self.decoder.start()?;
        self.configuration = Some((width, height, fps));
        Ok(())
    }

    pub fn decode(&mut self, data: &[u8], rgb: &mut ImageRgb) -> ResultType<bool> {
        if self.configuration.is_none() {
            bail!("MediaCodec input dimensions are not configured");
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        let mut decoded = self.poll_output(rgb)?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            // Keep this packet borrowed until it is queued; input backpressure is not consumption.
            let queued = if let Some(mut input) = self
                .decoder
                .dequeue_input_buffer(remaining.min(Duration::from_millis(1)))?
            {
                if data.len() > input.buffer_mut().len() {
                    self.decoder.queue_input_buffer(input, 0, 0, 0, 0)?;
                    bail!("Encoded frame exceeds the MediaCodec input buffer");
                }
                input.buffer_mut()[..data.len()].copy_from_slice(data);
                self.decoder
                    .queue_input_buffer(input, 0, data.len(), 0, 0)?;
                true
            } else {
                false
            };
            if queued {
                self.pending_output += 1;
                decoded |= self.poll_output(rgb)?;
                return Ok(decoded);
            }
            if remaining.is_zero() {
                bail!("No MediaCodec input buffer available");
            }
            decoded |= self.poll_output(rgb)?;
        }
    }

    pub fn has_pending_output(&self) -> bool {
        self.pending_output > 0
    }

    pub fn take_decoded_output_count(&mut self) -> usize {
        std::mem::take(&mut self.decoded_outputs)
    }

    pub fn poll_output(&mut self, rgb: &mut ImageRgb) -> ResultType<bool> {
        let decoder = &self.decoder;
        let mut decoded = false;
        loop {
            let output = match decoder.dequeue_output_buffer(Duration::ZERO) {
                Ok(Some(output)) => output,
                Ok(None) => return Ok(decoded),
                // ndk 0.7 returns these normal format/buffer notifications as errors.
                Err(NdkMediaError::UnknownResult(status)) if status.0 == -2 || status.0 == -3 => {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let converted = convert_output(&output.format(), output.buffer(), rgb);
            decoder.release_output_buffer(output, false)?;
            self.pending_output = self.pending_output.saturating_sub(1);
            converted?;
            self.decoded_outputs = self.decoded_outputs.saturating_add(1);
            decoded = true;
        }
    }
}

fn convert_output(format: &MediaFormat, buffer: &[u8], rgb: &mut ImageRgb) -> ResultType<()> {
    let get = |key| {
        format
            .i32(key)
            .ok_or_else(|| Error::msg(format!("MediaCodec output is missing {key}")))
    };
    let width = get("width")?;
    let height = get("height")?;
    let stride = get("stride")?;
    let slice_height = get("slice-height")?;
    let color = get("color-format")?;
    if width <= 0 || height <= 0 || stride < width || slice_height < height {
        bail!("Invalid MediaCodec output dimensions or stride");
    }
    if color != COLOR_YUV420_SEMIPLANAR {
        bail!("Unsupported MediaCodec output color format: {color}");
    }
    let left = format.i32("crop-left").unwrap_or(0);
    let top = format.i32("crop-top").unwrap_or(0);
    let right = format.i32("crop-right").unwrap_or(width - 1);
    let bottom = format.i32("crop-bottom").unwrap_or(height - 1);
    if left < 0
        || top < 0
        || right < left
        || bottom < top
        || right >= width
        || bottom >= height
        || left % 2 != 0
        || top % 2 != 0
    {
        bail!("Unsupported MediaCodec output crop");
    }
    let (stride, slice_height) = (stride as usize, slice_height as usize);
    let chroma_height = (slice_height + 1) / 2;
    let y_len = stride
        .checked_mul(slice_height)
        .ok_or_else(|| Error::msg("Y plane overflow"))?;
    let uv_len = stride
        .checked_mul(chroma_height)
        .ok_or_else(|| Error::msg("UV plane overflow"))?;
    let required = y_len
        .checked_add(uv_len)
        .ok_or_else(|| Error::msg("MediaCodec output size overflow"))?;
    if buffer.len() < required || stride % 2 != 0 {
        bail!("Unsupported or truncated MediaCodec output layout");
    }
    let w = (right - left + 1) as usize;
    let h = (bottom - top + 1) as usize;
    let align = rgb.align().max(1);
    if !align.is_power_of_two() {
        bail!("Invalid RGB alignment");
    }
    let row_bytes = w
        .checked_mul(4)
        .and_then(|row| row.checked_add(align - 1))
        .map(|row| row & !(align - 1))
        .filter(|row| *row <= i32::MAX as usize)
        .ok_or_else(|| Error::msg("RGB stride overflow"))?;
    let len = row_bytes
        .checked_mul(h)
        .ok_or_else(|| Error::msg("RGB size overflow"))?;
    let (left, top) = (left as usize, top as usize);
    let y = &buffer[top * stride + left..];
    let uv_offset = y_len + (top / 2) * stride + left;
    rgb.raw.resize(len, 0);
    let convert = match rgb.fmt() {
        ImageFormat::ARGB => NV12ToARGB,
        ImageFormat::ABGR => NV12ToABGR,
        _ => bail!("Unsupported RGB format"),
    };
    let result = unsafe {
        convert(
            y.as_ptr(),
            stride as _,
            buffer[uv_offset..].as_ptr(),
            stride as _,
            rgb.raw.as_mut_ptr(),
            row_bytes as _,
            w as _,
            h as _,
        )
    };
    if result != 0 {
        bail!("MediaCodec output conversion failed: {result}");
    }
    rgb.w = w;
    rgb.h = h;
    Ok(())
}

pub fn check_mediacodec() {
    H264_DECODER_SUPPORT.store(
        MediaCodecDecoder::new(CodecFormat::H264).is_some(),
        Ordering::SeqCst,
    );
    H265_DECODER_SUPPORT.store(
        MediaCodecDecoder::new(CodecFormat::H265).is_some(),
        Ordering::SeqCst,
    );
    log::debug!("Android hardware decoder inventory checked");
}
