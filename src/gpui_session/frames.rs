//! Bounded decoded-frame admission, independent of native codec and window libraries.
use crossbeam_queue::ArrayQueue;
use std::time::{Duration, Instant};

pub(super) fn clear<T>(queue: &ArrayQueue<T>) {
    while queue.pop().is_some() {}
}

pub(super) fn take_fresh<T>(
    queue: &ArrayQueue<T>,
    now: Instant,
    budget: Duration,
    ready_at: impl Fn(&T) -> Instant,
    mut dropped: impl FnMut(),
) -> Option<T> {
    loop {
        let frame = queue.pop()?;
        if now.saturating_duration_since(ready_at(&frame)) > budget && !queue.is_empty() {
            dropped();
            continue;
        }
        // A static desktop still needs its final image, regardless of its age.
        return Some(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_spare_frame_preserves_bursts_across_display_ticks() {
        let simulate = |capacity| {
            let queue = ArrayQueue::new(capacity);
            let start = Instant::now();
            let mut arrivals = [1, 2, 17, 18, 33, 34].into_iter().peekable();
            let mut shown = Vec::new();
            for tick in (8..=48).step_by(8) {
                while arrivals.peek().is_some_and(|arrival| *arrival <= tick) {
                    let arrival = arrivals.next().unwrap();
                    queue.force_push((arrival, start + Duration::from_millis(arrival)));
                }
                if let Some((id, _)) = take_fresh(
                    &queue,
                    start + Duration::from_millis(tick),
                    Duration::from_millis(16),
                    |frame| frame.1,
                    || panic!("fresh burst frame must not expire"),
                ) {
                    shown.push(id);
                }
            }
            shown
        };
        assert_eq!(simulate(1), [2, 18, 34]);
        assert_eq!(simulate(2), [1, 2, 17, 18, 33, 34]);
    }

    #[test]
    fn overloaded_or_idle_streams_stay_bounded_and_reset_drops_every_frame() {
        let queue = ArrayQueue::new(2);
        let start = Instant::now();
        assert!(queue.force_push((1, start)).is_none());
        assert!(queue
            .force_push((2, start + Duration::from_millis(1)))
            .is_none());
        assert_eq!(
            queue
                .force_push((3, start + Duration::from_millis(49)))
                .unwrap()
                .0,
            1
        );
        let mut dropped = 0;
        let next = take_fresh(
            &queue,
            start + Duration::from_millis(50),
            Duration::from_millis(16),
            |frame| frame.1,
            || dropped += 1,
        )
        .unwrap();
        assert_eq!(next.0, 3);
        assert_eq!(dropped, 1);
        queue.force_push(next);
        assert_eq!(
            take_fresh(
                &queue,
                start + Duration::from_secs(5),
                Duration::from_millis(16),
                |frame| frame.1,
                || panic!("last static image must survive"),
            )
            .unwrap()
            .0,
            3
        );
        queue.force_push((4, start));
        queue.force_push((5, start));
        clear(&queue);
        assert!(queue.is_empty());
    }
}
