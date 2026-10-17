/// Android density-independent pixels use 160 dpi as the baseline.
pub(crate) fn density(dpi: Option<u32>) -> f32 {
    dpi.filter(|dpi| *dpi > 0).unwrap_or(160) as f32 / 160.0
}

#[cfg(test)]
mod tests {
    use super::density;

    #[test]
    fn density_uses_android_logical_pixel_scale() {
        assert_eq!(density(Some(320)), 2.0);
        assert_eq!(density(Some(480)), 3.0);
        assert_eq!(density(Some(0)), 1.0);
        assert_eq!(density(None), 1.0);
    }
}
