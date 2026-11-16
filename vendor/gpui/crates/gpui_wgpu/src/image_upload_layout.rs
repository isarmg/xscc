#[derive(Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) row_bytes: u32,
    pub(crate) padded_row: u32,
    pub(crate) size: u64,
}

pub(crate) fn layout(width: u32, height: u32) -> Option<Layout> {
    if width == 0 || height == 0 {
        return None;
    }
    let row_bytes = width.checked_mul(4)?;
    let padded_row = row_bytes.checked_add(255)? / 256 * 256;
    Some(Layout {
        row_bytes,
        padded_row,
        size: u64::from(padded_row).checked_mul(u64::from(height))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_and_custom_widths_have_valid_gpu_copy_rows() {
        assert_eq!(
            layout(1920, 1080),
            Some(Layout {
                row_bytes: 7680,
                padded_row: 7680,
                size: 8_294_400
            })
        );
        assert_eq!(
            layout(641, 480),
            Some(Layout {
                row_bytes: 2564,
                padded_row: 2816,
                size: 1_351_680
            })
        );
        assert_eq!(layout(3840, 2160).unwrap().size, 33_177_600);
    }

    #[test]
    fn empty_and_overflowing_copy_rows_are_rejected() {
        assert!(layout(0, 1080).is_none());
        assert!(layout(1920, 0).is_none());
        assert!(layout(u32::MAX, 1).is_none());
        assert!(layout(u32::MAX / 4, 1).is_none());
    }
}
