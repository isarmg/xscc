use gpui::{RenderImage, Window};
use std::sync::Arc;
use xscc_session_model::FrameBuffer;

/// Owns only UI-thread image resources. Native decoding never touches GPUI.
#[derive(Default)]
pub struct VideoPresentation {
    image: Option<Arc<RenderImage>>,
    #[cfg(target_os = "ios")]
    surface: Option<gpui::SurfaceSource>,
    invalidated: bool,
}

impl VideoPresentation {
    pub fn image(&self) -> Option<&Arc<RenderImage>> {
        self.image.as_ref()
    }

    pub fn has_frame(&self) -> bool {
        #[cfg(target_os = "ios")]
        if self.surface.is_some() {
            return true;
        }
        self.image.is_some()
    }

    #[cfg(target_os = "ios")]
    pub fn surface(&self) -> Option<&gpui::SurfaceSource> {
        self.surface.as_ref()
    }

    #[cfg(target_os = "ios")]
    pub fn update_surface(
        &mut self,
        surface: gpui::SurfaceSource,
        window: &mut Window,
    ) -> Result<(), String> {
        self.invalidated = false;
        if let Some(image) = self.image.take() {
            window
                .drop_image(image)
                .map_err(|error| error.to_string())?;
        }
        self.surface = Some(surface);
        Ok(())
    }

    pub fn invalidate(&mut self) {
        self.invalidated = true;
    }

    pub fn update<F: FrameBuffer>(
        &mut self,
        frame: Option<F>,
        window: &mut Window,
    ) -> Result<Option<(u32, u32)>, String> {
        if self.invalidated {
            self.invalidated = false;
            #[cfg(target_os = "ios")]
            {
                self.surface = None;
            }
            if let Some(image) = self.image.take() {
                window
                    .drop_image(image)
                    .map_err(|error| error.to_string())?;
            }
        }
        let Some(frame) = frame else { return Ok(None) };
        #[cfg(target_os = "ios")]
        {
            self.surface = None;
        }
        let (width, height) = frame.extent();
        #[cfg(any(target_os = "android", target_os = "linux"))]
        let nv12 = frame.is_nv12();
        #[cfg(not(any(target_os = "android", target_os = "linux")))]
        let nv12 = false;
        if (nv12 && frame.stride() != width as usize)
            || (!nv12 && !packed_bgra(width, height, frame.stride(), frame.as_ref().len()))
        {
            return Err("远程画面像素布局无效。".into());
        }
        let extent = (width, height);
        let submitted = frame.submission();
        let next = Arc::new(
            if nv12 {
                RenderImage::from_nv12(width, height, frame)
            } else {
                RenderImage::from_bgra(width, height, frame)
            }
            .map_err(|error| error.to_string())?,
        );
        if let Some(previous) = self.image.take() {
            window
                .replace_image(previous, &next)
                .map_err(|error| error.to_string())?;
        }
        self.image = Some(next);
        submitted();
        Ok(Some(extent))
    }
}

fn packed_bgra(width: u32, height: u32, stride: usize, len: usize) -> bool {
    width > 0
        && height > 0
        && (width as usize).checked_mul(4) == Some(stride)
        && stride.checked_mul(height as usize) == Some(len)
}

/// Limit streaming work to a reported display rate; an unknown rate retains
/// the remote request. Native variable-refresh displays report their maximum.
pub fn stream_fps(remote_hz: u32, display_interval: Option<std::time::Duration>) -> u32 {
    display_interval
        .filter(|interval| !interval.is_zero())
        .map(|interval| interval.as_secs_f64().recip().round() as u32)
        .filter(|hertz| *hertz > 0)
        .map_or(remote_hz, |limit| remote_hz.min(limit))
}

#[cfg(test)]
mod tests {
    use super::{packed_bgra, stream_fps};

    #[test]
    fn stream_fps_uses_reported_display_limits_and_preserves_unknown_rates() {
        use std::time::Duration;

        for (remote, hertz, expected) in [
            (90, 120., 90),
            (120, 120., 120),
            (144, 120., 120),
            (240, 120., 120),
            (240, 240., 240),
            (120, 59.94, 60),
        ] {
            assert_eq!(
                stream_fps(remote, Some(Duration::from_secs_f64(1. / hertz))),
                expected
            );
        }
        assert_eq!(stream_fps(144, None), 144);
        assert_eq!(stream_fps(144, Some(Duration::ZERO)), 144);
        assert_eq!(stream_fps(0, Some(Duration::from_secs_f64(1. / 120.))), 0);
    }

    #[test]
    fn accepts_only_complete_tightly_packed_bgra() {
        assert!(packed_bgra(2, 3, 8, 24));
        assert!(!packed_bgra(2, 3, 12, 36));
        assert!(!packed_bgra(2, 3, 8, 23));
        assert!(!packed_bgra(0, 0, 0, 0));
        assert!(!packed_bgra(u32::MAX, u32::MAX, usize::MAX, 0));
    }
}
