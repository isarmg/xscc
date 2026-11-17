//! Copy visible YUV planes while the decoder still owns its output buffer.
#[cfg(any(target_os = "linux", test))]
pub(super) fn copy_yuv420(
    planes: &[Vec<u8>],
    strides: &[i32],
    width: usize,
    height: usize,
    planar: bool,
    destination: &mut Vec<u8>,
) -> Option<()> {
    use std::convert::TryFrom;

    if !(1..=16384).contains(&width) || !(1..=16384).contains(&height) {
        return None;
    }
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    let chroma_row = chroma_width * 2;
    let plane = |index: usize, row: usize, rows: usize| {
        let stride = usize::try_from(*strides.get(index)?).ok()?;
        let bytes = planes.get(index)?.as_slice();
        let required = (rows - 1).checked_mul(stride)?.checked_add(row)?;
        (stride >= row && bytes.len() >= required).then_some((bytes, stride))
    };
    let (y, y_stride) = plane(0, width, height)?;
    let (u, u_stride) = plane(
        1,
        if planar { chroma_width } else { chroma_row },
        chroma_height,
    )?;
    let v = if planar {
        Some(plane(2, chroma_width, chroma_height)?)
    } else {
        None
    };
    let y_len = width * height;
    destination.resize(y_len + chroma_row * chroma_height, 0);
    let (destination_y, destination_uv) = destination.split_at_mut(y_len);
    for (source, output) in y.chunks(y_stride).zip(destination_y.chunks_exact_mut(width)) {
        output.copy_from_slice(&source[..width]);
    }
    if let Some((v, v_stride)) = v {
        for ((u, v), output) in u
            .chunks(u_stride)
            .zip(v.chunks(v_stride))
            .zip(destination_uv.chunks_exact_mut(chroma_row))
        {
            for ((output, u), v) in output
                .chunks_exact_mut(2)
                .zip(&u[..chroma_width])
                .zip(&v[..chroma_width])
            {
                output.copy_from_slice(&[*u, *v]);
            }
        }
    } else {
        for (source, output) in u
            .chunks(u_stride)
            .zip(destination_uv.chunks_exact_mut(chroma_row))
        {
            output.copy_from_slice(&source[..chroma_row]);
        }
    }
    Some(())
}

#[cfg(any(all(target_os = "android", feature = "mediacodec"), test))]
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
    fn decoder_nv12_and_i420_pack_identical_visible_pixels_with_independent_strides() {
        let y = vec![1, 2, 3, 90, 91, 4, 5, 6, 92, 93, 7, 8, 9];
        let cases = [
            (
                false,
                vec![y.clone(), vec![10, 11, 12, 13, 96, 97, 14, 15, 16, 17]],
                vec![5, 6],
            ),
            (
                true,
                vec![y, vec![10, 12, 98, 99, 14, 16], vec![11, 13, 99, 15, 17]],
                vec![5, 4, 3],
            ),
        ];
        for (planar, planes, strides) in cases {
            let mut pixels = Vec::new();
            copy_yuv420(&planes, &strides, 3, 3, planar, &mut pixels).unwrap();
            assert_eq!(
                pixels,
                [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17]
            );
        }
    }

    #[test]
    fn invalid_decoder_planes_do_not_publish_partial_pixels() {
        let planes = vec![vec![0; 9], vec![0; 8]];
        let mut pixels = vec![42];
        for (strides, width, height, planar) in [
            (vec![-3, 4], 3, 3, false),
            (vec![2, 4], 3, 3, false),
            (vec![3, 4], 0, 3, false),
            (vec![3, 4], 3, 0, false),
            (vec![3, 4], 16385, 3, false),
            (vec![3, 4], 3, 3, true),
            (vec![3, 5], 3, 3, false),
        ] {
            assert!(copy_yuv420(&planes, &strides, width, height, planar, &mut pixels).is_none());
            assert_eq!(pixels, [42]);
        }
    }

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
