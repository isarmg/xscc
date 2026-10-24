//! Checked packed-row conversion. Tight BGRA transfers require no pixel copy.
pub(super) struct Layout {
    pub row: usize,
    stride: usize,
    height: usize,
}

impl Layout {
    pub fn new(width: usize, height: usize, align: usize, len: usize) -> Option<Self> {
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return None;
        }
        let row = width.checked_mul(4)?;
        let align = align.max(1);
        let stride = row
            .checked_add(align - 1)?
            .checked_div(align)?
            .checked_mul(align)?;
        if len < stride.checked_mul(height)? {
            return None;
        }
        Some(Self {
            row,
            stride,
            height,
        })
    }

    /// `new` validates the input length. Compaction never reads rows already overwritten.
    pub fn pack(&self, bytes: &mut Vec<u8>, swap_red_blue: bool) {
        if self.row != self.stride {
            for y in 1..self.height {
                let source = y * self.stride;
                bytes.copy_within(source..source + self.row, y * self.row);
            }
        }
        bytes.truncate(self.row * self.height);
        if swap_red_blue {
            for pixel in bytes.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tight_bgra_keeps_allocation_and_bytes() {
        let mut bytes = vec![1, 2, 3, 255, 4, 5, 6, 255];
        let pointer = bytes.as_ptr();
        Layout::new(2, 1, 1, bytes.len())
            .unwrap()
            .pack(&mut bytes, false);
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes, [1, 2, 3, 255, 4, 5, 6, 255]);
    }
    #[test]
    fn padded_rgba_compacts_overlapping_rows_and_converts_channels() {
        let mut bytes = vec![
            1, 2, 3, 4, 9, 9, 9, 9, 5, 6, 7, 8, 9, 9, 9, 9, 10, 11, 12, 13, 9, 9, 9, 9,
        ];
        let pointer = bytes.as_ptr();
        Layout::new(1, 3, 8, bytes.len())
            .unwrap()
            .pack(&mut bytes, true);
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes, [3, 2, 1, 4, 7, 6, 5, 8, 12, 11, 10, 13]);
    }
    #[test]
    fn rejects_truncated_oversized_and_overflowing_layouts() {
        for (w, h, a, len) in [
            (0, 1, 1, 4),
            (1, 0, 1, 4),
            (1, 2, 8, 15),
            (16385, 1, 1, usize::MAX),
            (1, 1, usize::MAX, usize::MAX),
        ] {
            assert!(Layout::new(w, h, a, len).is_none());
        }
        assert!(Layout::new(1, 1, 0, 4).is_some());
    }
}
