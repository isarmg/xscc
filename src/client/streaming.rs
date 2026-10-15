pub(super) fn target_fps(
    enabled: bool,
    target: &str,
    quality: &str,
    custom: &str,
) -> i32 {
    let fps = if enabled {
        match target.parse::<i32>() {
            Ok(fps @ (60 | 90 | 120)) => fps,
            _ => 60,
        }
    } else if quality == "custom" {
        custom
            .parse::<i32>()
            .ok()
            .filter(|fps| (1..=120).contains(fps))
            .unwrap_or(30)
    } else {
        30
    };
    fps
}

#[cfg(test)]
mod tests {
    use super::target_fps;

    #[test]
    fn streaming_target_is_independent_of_image_quality() {
        for quality in ["balanced", "best", "custom"] {
            assert_eq!(target_fps(true, "90", quality, "24"), 90);
        }
        assert_eq!(target_fps(true, "invalid", "best", "24"), 60);
        assert_eq!(target_fps(true, "120", "best", "24"), 120);
    }

    #[test]
    fn disabling_restores_the_original_fps_preference() {
        assert_eq!(target_fps(false, "120", "custom", "24"), 24);
        assert_eq!(target_fps(false, "120", "balanced", "24"), 30);
        assert_eq!(target_fps(false, "120", "custom", "999"), 30);
    }

}
