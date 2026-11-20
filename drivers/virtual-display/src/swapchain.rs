pub(crate) fn drain(mut stopped: impl FnMut() -> bool, mut process_frame: impl FnMut() -> bool) {
    while !stopped() && process_frame() {}
}

#[cfg(test)]
mod tests {
    use super::drain;
    use std::cell::Cell;

    #[test]
    fn continuously_ready_frames_do_not_starve_cancellation() {
        let frames = Cell::new(0);
        drain(
            || frames.get() == 128,
            || {
                frames.set(frames.get() + 1);
                true
            },
        );
        assert_eq!(frames.get(), 128);
    }
}
