//! Standalone CPU handoff benchmark (no codec, network, GPU, or physical presentation).
//! Run: rustc --edition 2021 -O tests/presentation_bench.rs -o /tmp/presentation-bench && /tmp/presentation-bench
#![allow(dead_code)]
#[path = "../src/gpui_session/metrics.rs"]
mod metrics;
#[path = "../src/gpui_session/pixels.rs"]
mod pixels;
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

fn percentiles(mut samples: Vec<u128>) -> (u128, u128) {
    samples.sort_unstable();
    (
        samples[samples.len() / 2],
        samples[(samples.len() * 95).div_ceil(100) - 1],
    )
}
fn measure(mut operation: impl FnMut()) -> (u128, u128) {
    for _ in 0..12 {
        operation();
    }
    let mut samples = Vec::with_capacity(240);
    for _ in 0..240 {
        let start = Instant::now();
        operation();
        samples.push(start.elapsed().as_nanos());
    }
    percentiles(samples)
}
fn main() {
    for (width, height) in [(1920, 1080), (3840, 2160)] {
        let len = width * height * 4;
        let source = vec![127u8; len];
        let row = width * 4;
        let old = measure(|| {
            let mut packed = Vec::with_capacity(len);
            for source in black_box(&source).chunks_exact(row).take(height) {
                packed.extend_from_slice(source);
            }
            black_box(packed);
        });
        let layout = pixels::Layout::new(width, height, 1, len).unwrap();
        let mut decoder = vec![127u8; len];
        // Simulate bounded warm-pool ownership. Real Frame recycling is separately tested.
        let mut pool = vec![vec![0u8; len], vec![0u8; len], vec![0u8; len]];
        let new = measure(|| {
            let mut replacement = pool.pop().unwrap();
            replacement.resize(decoder.len(), 0);
            let mut frame = std::mem::replace(&mut decoder, replacement);
            layout.pack(black_box(&mut frame), false);
            black_box(&frame);
            pool.push(frame);
        });
        println!("{width}x{height}: old row-copy+allocation p50/p95={} / {} ns; warm ownership handoff={} / {} ns; eliminated CPU copy={} bytes/frame",old.0,old.1,new.0,new.1,len);
    }
    let mut telemetry = metrics::Metrics::default();
    let overhead = measure(|| {
        telemetry.received(1);
        telemetry.decoded(Duration::from_millis(3));
        telemetry.submitted(Duration::from_millis(2));
        black_box(&telemetry);
    });
    println!(
        "pure telemetry received+decoded+submitted (excluding adapter locks) p50/p95={} / {} ns",
        overhead.0, overhead.1
    );
}
