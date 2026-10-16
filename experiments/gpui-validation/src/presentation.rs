use gpui::{RenderImage, Window};
use librustdesk::gpui_session::Frame;
use std::sync::Arc;

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

    pub fn update(
        &mut self,
        frame: Option<Frame>,
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
        if !packed_bgra(frame.width, frame.height, frame.stride, frame.bgra.len()) {
            return Err("远程画面像素布局无效。".into());
        }
        let extent = (frame.width, frame.height);
        // GPUI RenderImage consumes BGRA bytes despite the image crate's container name.
        let pixels = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
            .ok_or_else(|| "远程画面尺寸无效。".to_owned())?;
        let next = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
        if let Some(previous) = self.image.replace(next) {
            window
                .drop_image(previous)
                .map_err(|error| error.to_string())?;
        }
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
