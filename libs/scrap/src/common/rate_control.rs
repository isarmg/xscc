// Existing codec-specific budgets are calibrated at 30 FPS. Streaming keeps
// that per-frame budget while QoS can still lower quality or frame rate.
pub(crate) fn bitrate_for_fps(at_30_fps: u32, fps: Option<u32>) -> u32 {
    let Some(fps) = fps else { return at_30_fps };
    // The hardware FFI converts kbit/s to bit/s using a signed 32-bit integer.
    const MAX_KBPS: u64 = i32::MAX as u64 / 1000;
    ((at_30_fps as u64 * fps as u64 / 30).max(1).min(MAX_KBPS)) as u32
}

pub(crate) fn frame_duration(fps: Option<u32>, timebase: u32) -> u64 {
    match fps {
        Some(fps) => ((timebase as u64 + fps as u64 / 2) / fps.max(1) as u64).max(1),
        None => (timebase / 1000) as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_preserves_the_codec_budget_per_frame() {
        // Resolution/quality/codec weights are applied by the backend first.
        for at_30 in [400, 670, 1388, 2777, 4500, 7500, 18000] {
            assert_eq!(bitrate_for_fps(at_30, None), at_30);
            for fps in [15, 30, 60, 90, 120] {
                let budget = bitrate_for_fps(at_30, Some(fps));
                assert!((budget as i64 * 30 - at_30 as i64 * fps as i64).abs() < 30);
            }
        }
        assert_eq!(bitrate_for_fps(u32::MAX, Some(120)), i32::MAX as u32 / 1000);
    }

    #[test]
    fn streaming_duration_uses_the_encoder_timebase_and_preserves_legacy() {
        assert_eq!(frame_duration(None, 1000), 1);
        for (fps, duration) in [(30, 33), (60, 17), (90, 11), (120, 8)] {
            assert_eq!(frame_duration(Some(fps), 1000), duration);
            assert_eq!(frame_duration(Some(fps), 90000), 90000 / fps as u64);
        }
    }
}
