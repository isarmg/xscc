use crate::{DevicePixels, Pixels, Result, SharedString, Size, size};
use smallvec::SmallVec;

use image::{Delay, Frame};
use std::{
    borrow::Cow,
    fmt,
    hash::Hash,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

/// A source of assets for this app to use.
pub trait AssetSource: 'static + Send + Sync {
    /// Load the given asset from the source path.
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;

    /// List the assets at the given path.
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}

impl AssetSource for () {
    fn load(&self, _path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(None)
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

/// A unique identifier for the image cache
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(pub usize);

#[derive(PartialEq, Eq, Hash, Clone)]
#[expect(missing_docs)]
pub struct RenderImageParams {
    pub image_id: ImageId,
    pub frame_index: usize,
}

/// A cached and processed image, in BGRA format
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    data: RenderImageData,
}

enum RenderImageData {
    Frames(SmallVec<[Frame; 1]>),
    OwnedBgra {
        size: Size<DevicePixels>,
        pixels: Box<dyn AsRef<[u8]> + Send + Sync>,
    },
}

impl PartialEq for RenderImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for RenderImage {}

impl RenderImage {
    /// Create a new image from the given data.
    pub fn new(data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self {
            id: ImageId(NEXT_ID.fetch_add(1, SeqCst)),
            scale_factor: 1.0,
            data: RenderImageData::Frames(data.into()),
        }
    }

    /// Own a complete packed BGRA frame without copying its pixel allocation.
    /// The owner remains alive until the final image reference is released.
    /// Pixels use the same straight (unassociated) alpha as `new`; the renderer
    /// applies target compositing. Opaque video should provide alpha 255.
    /// This constructor neither premultiplies nor scans/converts the pixels.
    pub fn from_bgra(
        width: u32,
        height: u32,
        pixels: impl AsRef<[u8]> + Send + Sync + 'static,
    ) -> Result<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        anyhow::ensure!(
            width > 0
                && height > 0
                && width <= i32::MAX as u32
                && height <= i32::MAX as u32
                && expected == Some(pixels.as_ref().len()),
            "owned image requires complete packed BGRA pixels"
        );
        let mut image = Self::new(SmallVec::new());
        image.data = RenderImageData::OwnedBgra {
            size: size(width.into(), height.into()),
            pixels: Box::new(pixels),
        };
        Ok(image)
    }

    /// Convert this image into a byte slice.
    pub fn as_bytes(&self, frame_index: usize) -> Option<&[u8]> {
        match &self.data {
            RenderImageData::Frames(frames) => frames
                .get(frame_index)
                .map(|frame| frame.buffer().as_raw().as_slice()),
            RenderImageData::OwnedBgra { pixels, .. } => {
                (frame_index == 0).then(|| pixels.as_ref().as_ref())
            }
        }
    }

    /// Get the size of this image, in pixels.
    pub fn size(&self, frame_index: usize) -> Size<DevicePixels> {
        match &self.data {
            RenderImageData::Frames(frames) => frames
                .get(frame_index)
                .map(|frame| {
                    let (width, height) = frame.buffer().dimensions();
                    size(width.into(), height.into())
                })
                .unwrap_or_default(),
            RenderImageData::OwnedBgra { size, .. } => {
                if frame_index == 0 {
                    *size
                } else {
                    Size::default()
                }
            }
        }
    }

    /// Get the size of this image, in pixels for display, adjusted for the scale factor.
    pub(crate) fn render_size(&self, frame_index: usize) -> Size<Pixels> {
        self.size(frame_index)
            .map(|v| (v.0 as f32 / self.scale_factor).into())
    }

    /// Get the delay of this frame from the previous.
    pub fn delay(&self, frame_index: usize) -> Delay {
        match &self.data {
            RenderImageData::Frames(frames) => frames.get(frame_index).map(|frame| frame.delay()),
            RenderImageData::OwnedBgra { .. } => None,
        }
        .unwrap_or(Delay::from_numer_denom_ms(100, 1))
    }

    /// Get the number of frames for this image.
    pub fn frame_count(&self) -> usize {
        match &self.data {
            RenderImageData::Frames(frames) => frames.len(),
            RenderImageData::OwnedBgra { .. } => 1,
        }
    }
}

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageData")
            .field("id", &self.id)
            .field("size", &self.size(0))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;

    #[test]
    fn owned_bgra_keeps_original_allocation_until_last_image_reference() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Pixels {
            bytes: Vec<u8>,
            drops: Arc<AtomicUsize>,
        }
        impl AsRef<[u8]> for Pixels {
            fn as_ref(&self) -> &[u8] {
                &self.bytes
            }
        }
        impl Drop for Pixels {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        let bytes = vec![1, 2, 3, 255];
        let pointer = bytes.as_ptr();
        let image = Arc::new(
            RenderImage::from_bgra(
                1,
                1,
                Pixels {
                    bytes,
                    drops: drops.clone(),
                },
            )
            .unwrap(),
        );
        assert_eq!(image.as_bytes(0).unwrap().as_ptr(), pointer);
        assert_eq!(image.frame_count(), 1);
        assert_eq!(image.size(0), size(1.into(), 1.into()));
        assert!(image.as_bytes(1).is_none());
        let retained = image.clone();
        drop(image);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(retained);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(RenderImage::from_bgra(1, 2, vec![0; 4]).is_err());
        assert!(RenderImage::from_bgra(0, 0, Vec::<u8>::new()).is_err());
    }

    #[test]
    fn empty_render_image_does_not_panic() {
        let image = RenderImage::new(SmallVec::new());
        assert_eq!(image.frame_count(), 0);
        assert_eq!(image.size(0), Size::default());
        assert_eq!(image.as_bytes(0), None);
        assert_eq!(image.render_size(0), Size::default());
        assert_eq!(image.delay(0), Delay::from_numer_denom_ms(100, 1));
        let _ = format!("{image:?}");
    }
}
