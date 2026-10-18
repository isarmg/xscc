/// All dimensions are logical pixels from the display's usable work area.
pub fn window_sizes(work: [f32; 2], host: bool) -> ([f32; 2], [f32; 2]) {
    // Leave room for native window decorations as well as a visible desktop margin.
    let available = [(work[0] - 48.).max(1.), (work[1] - 48.).max(1.)];
    let desired: [f32; 2] = if host { [720., 680.] } else { [1080., 720.] };
    let initial = [desired[0].min(available[0]), desired[1].min(available[1])];
    let minimum = [360_f32.min(initial[0]), 320_f32.min(initial[1])];
    (initial, minimum)
}

pub fn card_columns(width: f32) -> u16 {
    ((width + 12.) / 332.).floor().clamp(1., 3.) as u16
}

pub fn peer_status(online: Option<bool>) -> &'static str {
    match online {
        Some(true) => "在线",
        Some(false) => "离线",
        None => "状态未知",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_and_scaled_work_areas_bound_both_initial_and_minimum_sizes() {
        for work in [[1366., 728.], [853., 432.], [320., 240.]] {
            for host in [true, false] {
                let (initial, minimum) = window_sizes(work, host);
                for axis in 0..2 {
                    assert!(initial[axis] + 48. <= work[axis]);
                    assert!(minimum[axis] <= initial[axis]);
                }
            }
        }
    }

    #[test]
    fn cards_reflow_and_unknown_status_never_claims_online() {
        assert_eq!(card_columns(300.), 1);
        assert_eq!(card_columns(660.), 2);
        assert_eq!(card_columns(1024.), 3);
        assert_eq!(peer_status(None), "状态未知");
        assert_eq!(peer_status(Some(false)), "离线");
        assert_eq!(peer_status(Some(true)), "在线");
    }
}
