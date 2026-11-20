use hbb_common::{bail, ResultType};
use std::{io, mem};
use winapi::{
    shared::windef::{HBITMAP, HDC},
    um::wingdi::{GetDIBits, BITMAPV5HEADER, BI_BITFIELDS, DIB_RGB_COLORS},
};

pub(super) fn apply_monochrome_mask(
    pixels: &mut [u8],
    mask: &[u8],
    width: usize,
    height: usize,
    stride: usize,
) -> bool {
    let (and_mask, xor_mask) = mask.split_at(stride * height);
    let mut outline = false;
    for (y, row) in pixels.chunks_exact_mut(width * 4).enumerate() {
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let byte = y * stride + x / 8;
            let bit = 1 << (7 - x % 8);
            let xor = xor_mask[byte] & bit != 0;
            if and_mask[byte] & bit == 0 {
                let shade = if xor { 255 } else { 0 };
                pixel.copy_from_slice(&[shade, shade, shade, 255]);
            } else if xor {
                // The cursor protocol cannot represent XOR. Contrast a black pixel with an outline.
                pixel.copy_from_slice(&[0, 0, 0, 255]);
                outline = true;
            }
        }
    }
    outline
}

pub(super) fn draw_outline(pixels: &[u8], width: usize, height: usize) -> ResultType<Vec<u8>> {
    let stride = (width + 2) * 4;
    let length = stride
        .checked_mul(height + 2)
        .ok_or_else(|| io::Error::other("Cursor outline size overflow"))?;
    let mut result = Vec::new();
    result.try_reserve_exact(length)?;
    result.resize(length, 0);

    for (y, row) in pixels.chunks_exact(width * 4).enumerate() {
        for (x, pixel) in row.chunks_exact(4).enumerate() {
            if pixel[3] != 0 {
                let offset = y * stride + x * 4;
                for line in 0..3 {
                    let start = offset + line * stride;
                    result[start..start + 12].fill(255);
                }
            }
        }
    }
    for (y, row) in pixels.chunks_exact(width * 4).enumerate() {
        for (x, pixel) in row.chunks_exact(4).enumerate() {
            if pixel[3] != 0 {
                let start = (y + 1) * stride + (x + 1) * 4;
                result[start..start + 4].copy_from_slice(pixel);
            }
        }
    }
    Ok(result)
}

pub(super) fn read_rgba(
    dc: HDC,
    bitmap: HBITMAP,
    width: i32,
    height: i32,
    output: &mut [u8],
) -> ResultType<()> {
    let mut header = BITMAPV5HEADER {
        bV5Size: mem::size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: width,
        bV5Height: -height,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5RedMask: 0x0000_00ff,
        bV5GreenMask: 0x0000_ff00,
        bV5BlueMask: 0x00ff_0000,
        bV5AlphaMask: 0xff00_0000,
        ..unsafe { mem::zeroed() }
    };

    let copied = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            output.as_mut_ptr().cast(),
            (&mut header as *mut BITMAPV5HEADER).cast(),
            DIB_RGB_COLORS,
        )
    };
    if copied != height {
        bail!(
            "Failed to read cursor bitmap: {} ({copied}/{height} rows)",
            io::Error::last_os_error()
        );
    }

    let mut offsets = [0; 4];
    let mut used = [false; 4];
    for (channel, mask) in [header.bV5RedMask, header.bV5GreenMask, header.bV5BlueMask]
        .into_iter()
        .enumerate()
    {
        let offset = (mask.trailing_zeros() / 8) as usize;
        if offset >= used.len() || mask != 0xff << (offset * 8) || used[offset] {
            bail!("Unsupported cursor bitmap channel masks");
        }
        offsets[channel] = offset;
        used[offset] = true;
    }
    offsets[3] = used
        .into_iter()
        .position(|value| !value)
        .ok_or_else(|| io::Error::other("Missing cursor bitmap alpha channel"))?;

    for pixel in output.chunks_exact_mut(4) {
        let rgba = offsets.map(|offset| pixel[offset]);
        pixel.copy_from_slice(&rgba);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{apply_monochrome_mask, draw_outline};

    #[test]
    fn decodes_black_white_transparent_and_xor_cursor_pixels() {
        let mut pixels = [0; 16];
        assert!(apply_monochrome_mask(
            &mut pixels,
            &[0b0011_0000, 0b0101_0000],
            4,
            1,
            1,
        ));
        assert_eq!(
            pixels,
            [0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 255]
        );
        let outlined = draw_outline(&pixels, 4, 1).unwrap();
        assert_eq!(&outlined[4..8], &[255; 4]);
        assert_eq!(&outlined[28..32], &[0, 0, 0, 255]);
    }
}
