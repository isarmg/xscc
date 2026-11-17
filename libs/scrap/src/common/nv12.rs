//! Copy the visible NV12 planes while the MediaCodec output buffer is still owned.
pub(super) fn copy_planes(
    y: &[u8],
    uv: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    destination: &mut Vec<u8>,
) -> Option<()> {
    let chroma_row = width.checked_add(1)?.checked_div(2)?.checked_mul(2)?;
    let chroma_height = height.checked_add(1)?.checked_div(2)?;
    let y_len = width.checked_mul(height)?;
    let uv_len = chroma_row.checked_mul(chroma_height)?;
    let required =
        |rows: usize, row: usize| rows.checked_sub(1)?.checked_mul(stride)?.checked_add(row);
    if width == 0
        || height == 0
        || stride < chroma_row
        || y.len() < required(height, width)?
        || uv.len() < required(chroma_height, chroma_row)?
    {
        return None;
    }
    destination.resize(y_len.checked_add(uv_len)?, 0);
    for (source, output) in y
        .chunks(stride)
        .zip(destination[..y_len].chunks_exact_mut(width))
    {
        output.copy_from_slice(&source[..width]);
    }
    for (source, output) in uv
        .chunks(stride)
        .zip(destination[y_len..].chunks_exact_mut(chroma_row))
    {
        output.copy_from_slice(&source[..chroma_row]);
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cropped_padded_odd_frames_keep_both_planes_without_rgb_expansion() {
        let y = [1, 2, 3, 90, 91, 92, 4, 5, 6, 93, 94, 95, 7, 8, 9];
        let uv = [10, 11, 12, 13, 96, 97, 14, 15, 16, 17];
        let mut pixels = Vec::new();
        copy_planes(&y, &uv, 6, 3, 3, &mut pixels).unwrap();
        assert_eq!(
            pixels,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17]
        );
        assert!(copy_planes(&y[..14], &uv, 6, 3, 3, &mut pixels).is_none());
        assert!(copy_planes(&y, &uv[..9], 6, 3, 3, &mut pixels).is_none());
    }

    #[test]
    fn full_hd_transfers_only_the_decoder_planes() {
        let mut pixels = Vec::new();
        let source = vec![128; 1920 * 1080 * 3 / 2];
        let (y, uv) = source.split_at(1920 * 1080);
        copy_planes(y, uv, 1920, 1920, 1080, &mut pixels).unwrap();
        assert_eq!(pixels.len(), 3_110_400);
        assert_eq!(pixels, source);
    }
}
