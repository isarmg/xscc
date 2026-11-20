use crate::ffmpeg::native;
use std::{
    ffi::{c_char, CString},
    ptr,
    time::Instant,
};

#[derive(Debug, Clone, PartialEq)]
pub struct MuxContext {
    pub filename: String,
    pub width: usize,
    pub height: usize,
    pub is265: bool,
    pub framerate: usize,
}

pub struct Muxer {
    output: *mut native::AVFormatContext,
    stream: *mut native::AVStream,
    packet: *mut native::AVPacket,
    minimum_duration: i64,
    start: Instant,
    start_ms: Option<i64>,
    last_pts: i64,
    trailer_result: Option<i32>,
    pub ctx: MuxContext,
}

// The format context and output file are owned exclusively by the muxer.
unsafe impl Send for Muxer {}
unsafe impl Sync for Muxer {}

impl Muxer {
    pub fn new(ctx: MuxContext) -> Result<Self, ()> {
        let filename = CString::new(ctx.filename.as_str()).map_err(|_| ())?;
        let width = i32::try_from(ctx.width).map_err(|_| ())?;
        let height = i32::try_from(ctx.height).map_err(|_| ())?;
        if width <= 0 || height <= 0 || ctx.framerate == 0 {
            return Err(());
        }
        let mut muxer = Self {
            output: ptr::null_mut(),
            stream: ptr::null_mut(),
            packet: ptr::null_mut(),
            minimum_duration: (1000 / ctx.framerate).max(1) as i64,
            start: Instant::now(),
            start_ms: None,
            last_pts: 0,
            trailer_result: None,
            ctx,
        };
        muxer
            .initialize(filename.as_ptr(), width, height)
            .map_err(|error| {
                log::debug!("FFmpeg mux setup failed: {error}");
            })?;
        Ok(muxer)
    }

    fn initialize(&mut self, filename: *const c_char, width: i32, height: i32) -> Result<(), i32> {
        unsafe {
            check(native::avformat_alloc_output_context2(
                &mut self.output,
                ptr::null(),
                ptr::null(),
                filename,
            ))?;
            if self.output.is_null() {
                return Err(-1);
            }
            self.stream = native::avformat_new_stream(self.output, ptr::null());
            let stream = self.stream.as_mut().ok_or(-1)?;
            let parameters = stream.codecpar.as_mut().ok_or(-1)?;
            stream.id = stream.index;
            parameters.codec_id = if self.ctx.is265 {
                native::AVCodecID_AV_CODEC_ID_HEVC
            } else {
                native::AVCodecID_AV_CODEC_ID_H264
            };
            parameters.codec_type = native::AVMediaType_AVMEDIA_TYPE_VIDEO;
            parameters.width = width;
            parameters.height = height;

            let output = &mut *self.output;
            if (*output.oformat).flags & native::AVFMT_NOFILE as i32 == 0 {
                check(native::avio_open(
                    &mut output.pb,
                    filename,
                    native::AVIO_FLAG_WRITE as i32,
                ))?;
            }
            self.packet = native::av_packet_alloc();
            if self.packet.is_null() {
                return Err(-1);
            }
            check(native::avformat_write_header(self.output, ptr::null_mut()))
        }
    }

    pub fn write_video(&mut self, data: &[u8], key: bool) -> Result<(), i32> {
        if self.trailer_result.is_some() || data.is_empty() {
            return Err(-1);
        }
        if self.start_ms.is_none() && !key {
            return Err(-2);
        }

        let length = i32::try_from(data.len()).map_err(|_| -1)?;
        let now = i64::try_from(self.start.elapsed().as_millis()).map_err(|_| -1)?;
        let start_ms = self.start_ms.unwrap_or(now);
        let mut pts = now - start_ms;
        if self.start_ms.is_some() && pts <= self.last_pts {
            pts = self.last_pts.checked_add(self.minimum_duration).ok_or(-1)?;
        }
        let duration = pts - self.last_pts;
        unsafe {
            check(native::av_new_packet(self.packet, length))?;
            let packet = &mut *self.packet;
            ptr::copy_nonoverlapping(data.as_ptr(), packet.data, data.len());
            packet.pts = pts;
            packet.dts = pts;
            packet.duration = if duration > 0 {
                duration
            } else {
                self.minimum_duration
            };
            packet.stream_index = (*self.stream).index;
            if key {
                packet.flags |= native::AV_PKT_FLAG_KEY as i32;
            }
            native::av_packet_rescale_ts(
                self.packet,
                native::AVRational { num: 1, den: 1000 },
                (*self.stream).time_base,
            );
            let result = native::av_write_frame(self.output, self.packet);
            native::av_packet_unref(self.packet);
            check(result)?;
        }
        self.start_ms = Some(start_ms);
        self.last_pts = pts;
        Ok(())
    }

    pub fn write_tail(&mut self) -> Result<(), i32> {
        let result = *self
            .trailer_result
            .get_or_insert_with(|| unsafe { native::av_write_trailer(self.output) });
        check(result)
    }
}

impl Drop for Muxer {
    fn drop(&mut self) {
        unsafe {
            native::av_packet_free(&mut self.packet);
            if let Some(output) = self.output.as_mut() {
                if !output.pb.is_null()
                    && (*output.oformat).flags & native::AVFMT_NOFILE as i32 == 0
                {
                    native::avio_closep(&mut output.pb);
                }
                native::avformat_free_context(self.output);
            }
        }
    }
}

fn check(result: i32) -> Result<(), i32> {
    if result < 0 {
        Err(result)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::SystemTime};

    #[test]
    fn writes_and_finalizes_an_mp4_only_once() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("xscc-mux-{}-{stamp}.mp4", std::process::id()));
        let mut muxer = Muxer::new(MuxContext {
            filename: path.to_str().unwrap().to_owned(),
            width: 1280,
            height: 720,
            is265: false,
            framerate: 30,
        })
        .unwrap();
        let frame = crate::common::DATA_H264_720P;
        assert_eq!(muxer.write_video(frame, false), Err(-2));
        muxer.write_video(frame, true).unwrap();
        muxer.write_tail().unwrap();
        muxer.write_tail().unwrap();
        assert!(muxer.write_video(frame, true).is_err());
        drop(muxer);
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        assert_eq!(bytes.windows(4).filter(|item| *item == b"moov").count(), 1);
    }
}
