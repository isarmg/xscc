use super::{Pixfmt, TraitPixelBuffer};

/// Borrowed pixels, independent of how they were acquired.
pub struct PixelBuffer<'a> {
    data: &'a [u8],
    pixfmt: Pixfmt,
    width: usize,
    height: usize,
    stride: Vec<usize>,
}

impl<'a> PixelBuffer<'a> {
    pub fn new(data: &'a [u8], pixfmt: Pixfmt, width: usize, height: usize) -> Self {
        let stride0 = data.len() / height;
        let mut stride = Vec::new();
        stride.push(stride0);
        Self {
            data,
            pixfmt,
            width,
            height,
            stride,
        }
    }
}

impl<'a> TraitPixelBuffer for PixelBuffer<'a> {
    fn data(&self) -> &[u8] {
        self.data
    }

    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn stride(&self) -> Vec<usize> {
        self.stride.clone()
    }

    fn pixfmt(&self) -> crate::Pixfmt {
        self.pixfmt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_pixels_keep_padded_rows_without_capture() {
        let bytes = [42; 32];
        let pixels = PixelBuffer::new(&bytes, Pixfmt::BGRA, 3, 2);
        assert_eq!(pixels.data(), &bytes);
        assert_eq!(pixels.data().as_ptr(), bytes.as_ptr());
        assert_eq!((pixels.width(), pixels.height()), (3, 2));
        assert_eq!(pixels.stride(), vec![16]);
        assert_eq!(pixels.pixfmt(), Pixfmt::BGRA);
    }
}
