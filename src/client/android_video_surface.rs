use super::VideoHandler;
use base::message_proto::{video_frame, VideoFrame};
use scrap::{mediacodec::VideoSurface, CodecFormat};
use std::sync::Arc;

impl VideoHandler {
    pub(super) fn select_android_output(
        &mut self,
        frame: &mut VideoFrame,
        format: CodecFormat,
        surface: Option<Arc<VideoSurface>>,
    ) -> bool {
        let active = self.decoder.video_surface();
        if active.is_none() && surface.is_none() {
            return true;
        }
        if self.decoder.format() == format
            && matches!((active, &surface), (Some(a), Some(b)) if Arc::ptr_eq(a, b))
        {
            return true;
        }

        let frames = match &mut frame.union {
            Some(video_frame::Union::H264s(frames) | video_frame::Union::H265s(frames)) => {
                &mut frames.frames
            }
            _ => {
                if let Some(active) = active {
                    active.invalidate();
                }
                return true;
            }
        };
        let Some(keyframe) = frames.iter().position(|frame| frame.key) else {
            // Keep displaying existing byte-buffer output until the requested keyframe.
            return active.is_none();
        };
        frames.drain(..keyframe);
        self.reset(Some(format));
        if let Some(surface) = surface {
            self.decoder.use_video_surface(surface);
        }
        true
    }

    pub(super) fn discard_failed_android_surface(&mut self) -> bool {
        if let Some(surface) = self.decoder.video_surface() {
            surface.invalidate();
            self.reset(None);
            true
        } else {
            false
        }
    }
}
