use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

pub(crate) enum Stage {
    Acquire,
    Upload,
    Render,
    Finish,
    Submit,
    Present,
}

struct Timing {
    since: Instant,
    frames: u32,
    totals: [Duration; 6],
    maxima: [Duration; 6],
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            since: Instant::now(),
            frames: 0,
            totals: [Duration::ZERO; 6],
            maxima: [Duration::ZERO; 6],
        }
    }
}

thread_local! {
    static TIMING: RefCell<Timing> = RefCell::new(Timing::default());
}

pub(crate) fn record(stage: Stage, started: Instant) {
    let elapsed = started.elapsed();
    TIMING.with_borrow_mut(|timing| {
        let index = stage as usize;
        timing.totals[index] += elapsed;
        timing.maxima[index] = timing.maxima[index].max(elapsed);
    });
}

pub(crate) fn finish_frame() {
    TIMING.with_borrow_mut(|timing| {
        timing.frames += 1;
        if timing.since.elapsed() < Duration::from_secs(1) {
            return;
        }
        let avg = timing.totals.map(|t| t.as_secs_f64() * 1000.0 / f64::from(timing.frames));
        let max = timing.maxima.map(|t| t.as_secs_f64() * 1000.0);
        log::info!("android_gpu_pipeline frames={} acquire_ms={:.3} upload_ms={:.3} render_ms={:.3} finish_ms={:.3} submit_ms={:.3} present_ms={:.3} max_ms={:?}",
            timing.frames, avg[0], avg[1], avg[2], avg[3], avg[4], avg[5], max);
        *timing = Timing::default();
    });
}
