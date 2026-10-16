#[cfg(windows)]
mod windows_probe;

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    return windows_probe::run();
    #[cfg(not(windows))]
    anyhow::bail!("This D3D11 probe must run on Windows; no GPU test was performed")
}

#[cfg(any(windows, test))]
fn pattern(width: usize, height: usize, frame: u8) -> Vec<u8> {
    (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| [x as u8 ^ frame, y as u8 ^ frame, frame, 255]))
        .collect()
}

#[cfg(any(windows, test))]
fn verify_rows(
    actual: &[u8],
    pitch: usize,
    expected: &[u8],
    width: usize,
    height: usize,
) -> anyhow::Result<()> {
    anyhow::ensure!(pitch >= width * 4, "row pitch is smaller than a BGRA row");
    anyhow::ensure!(width > 0 && height > 0, "empty frame");
    anyhow::ensure!(
        actual.len() >= pitch * (height - 1) + width * 4,
        "mapped buffer is too small"
    );
    anyhow::ensure!(
        expected.len() == width * height * 4,
        "invalid expected buffer"
    );
    for y in 0..height {
        let row = &actual[y * pitch..y * pitch + width * 4];
        anyhow::ensure!(
            row == &expected[y * width * 4..(y + 1) * width * 4],
            "pixel mismatch at row {y}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn checks_bgra_and_padded_rows() {
        let expected = super::pattern(2, 2, 7);
        assert_eq!(&expected[..8], &[7, 7, 7, 255, 6, 7, 7, 255]);
        let mut padded = vec![0; 24];
        padded[..8].copy_from_slice(&expected[..8]);
        padded[12..20].copy_from_slice(&expected[8..]);
        assert!(super::verify_rows(&padded[..20], 12, &expected, 2, 2).is_ok());
        padded[15] ^= 1;
        assert!(super::verify_rows(&padded, 12, &expected, 2, 2).is_err());
    }

    #[test]
    fn rejects_invalid_pitch_and_length() {
        let expected = super::pattern(2, 2, 0);
        assert!(super::verify_rows(&expected, 4, &expected, 2, 2).is_err());
        assert!(super::verify_rows(&expected[..15], 8, &expected, 2, 2).is_err());
    }
}
