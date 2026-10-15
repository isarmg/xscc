use hbb_common::{anyhow::Error, bail, log, ResultType};
use ndk::media::{
    media_codec::{MediaCodec, MediaCodecDirection, MediaFormat},
    NdkMediaError,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
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
    size: Option<(i32, i32)>,
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
                    size: None,
                });
            }
        }
        None
    }

    pub fn set_video_size(&mut self, width: i32, height: i32) -> ResultType<()> {
        if width <= 0 || height <= 0 {
            bail!("Invalid MediaCodec input dimensions: {width}x{height}");
        }
        if self.size == Some((width, height)) {
            return Ok(());
        }
        if self.size.take().is_some() {
            self.decoder.stop()?;
        }
        let format = MediaFormat::new();
        format.set_str("mime", self.mime);
        format.set_i32("width", width);
        format.set_i32("height", height);
        format.set_i32("color-format", COLOR_YUV420_SEMIPLANAR);
        self.decoder
            .configure(&format, None, MediaCodecDirection::Decoder)?;
        self.decoder.start()?;
        self.size = Some((width, height));
        Ok(())
    }

    pub fn decode(&mut self, data: &[u8], rgb: &mut ImageRgb) -> ResultType<bool> {
        if self.size.is_none() {
            bail!("MediaCodec input dimensions are not configured");
        }
        let decoder = &self.decoder;
        let Some(mut input) = decoder.dequeue_input_buffer(Duration::from_millis(100))? else {
            bail!("No MediaCodec input buffer available");
        };
        if data.len() > input.buffer_mut().len() {
            decoder.queue_input_buffer(input, 0, 0, 0, 0)?;
            bail!("Encoded frame exceeds the MediaCodec input buffer");
        }
        input.buffer_mut()[..data.len()].copy_from_slice(data);
        decoder.queue_input_buffer(input, 0, data.len(), 0, 0)?;

        let mut decoded = false;
        loop {
            let output = match decoder.dequeue_output_buffer(if decoded {
                Duration::ZERO
            } else {
                Duration::from_millis(100)
            }) {
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
            converted?;
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
