pub(super) fn target_fps(
    enabled: bool,
    target: &str,
    quality: &str,
    custom: &str,
) -> i32 {
    let fps = if enabled {
        target
            .parse::<i32>()
            .ok()
            .filter(|fps| (1..=base::session_protocol::MAX_SESSION_FPS as i32).contains(fps))
            .unwrap_or(60)
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
        for fps in [75, 144, 165, 240, 360, 1000] {
            assert_eq!(target_fps(true, &fps.to_string(), "best", "24"), fps);
        }
        assert_eq!(target_fps(true, "1001", "best", "24"), 60);
        assert_eq!(target_fps(true, "0", "best", "24"), 60);
    }

    #[test]
    fn disabling_restores_the_original_fps_preference() {
        assert_eq!(target_fps(false, "120", "custom", "24"), 24);
        assert_eq!(target_fps(false, "120", "balanced", "24"), 30);
        assert_eq!(target_fps(false, "120", "custom", "999"), 30);
    }

}
