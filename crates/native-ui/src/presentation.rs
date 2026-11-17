use gpui::{RenderImage, Window};
use std::sync::Arc;
use xscc_session_model::FrameBuffer;

/// Owns only UI-thread image resources. Native decoding never touches GPUI.
#[derive(Default)]
pub struct VideoPresentation {
    image: Option<Arc<RenderImage>>,
    invalidated: bool,
}

impl VideoPresentation {
    pub fn image(&self) -> Option<&Arc<RenderImage>> {
        self.image.as_ref()
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
            if let Some(image) = self.image.take() {
                window
                    .drop_image(image)
                    .map_err(|error| error.to_string())?;
            }
        }
        let Some(frame) = frame else { return Ok(None) };
        let (width, height) = frame.extent();
        #[cfg(target_os = "android")]
        let nv12 = frame.is_nv12();
        #[cfg(not(target_os = "android"))]
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

#[cfg(test)]
mod tests {
    use super::packed_bgra;

    #[test]
    fn accepts_only_complete_tightly_packed_bgra() {
        assert!(packed_bgra(2, 3, 8, 24));
        assert!(!packed_bgra(2, 3, 12, 36));
        assert!(!packed_bgra(2, 3, 8, 23));
        assert!(!packed_bgra(0, 0, 0, 0));
        assert!(!packed_bgra(u32::MAX, u32::MAX, usize::MAX, 0));
    }
}
