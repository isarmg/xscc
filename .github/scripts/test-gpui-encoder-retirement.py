#!/usr/bin/env python3
"""Compile the actual retirement owner module against CPU-only drop-counting stubs."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile

FIXTURE = r"""
extern crate self as hbb_common;
extern crate self as scrap;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static ENCODER_DROPS: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_DROPS: AtomicUsize = AtomicUsize::new(0);
static RAII_DROPS: AtomicUsize = AtomicUsize::new(0);
static GUARD_DROPS: AtomicUsize = AtomicUsize::new(0);
static STICKY: AtomicBool = AtomicBool::new(false);
static UNVERIFIED: AtomicBool = AtomicBool::new(false);
static TOPOLOGY: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub struct TestError(&'static str);
impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) }
}
impl std::error::Error for TestError {}
#[macro_export]
macro_rules! anyhow { ($message:expr) => { $crate::TestError($message) }; }
pub mod anyhow { pub use crate::anyhow; }
#[macro_export]
macro_rules! test_log { ($($args:tt)*) => { { let _ = format_args!($($args)*); } }; }
pub mod log { pub use crate::test_log as error; pub use crate::test_log as warn; pub use crate::test_log as info; }

pub mod codec {
    use super::*;
    pub struct Encoder { pub supported: bool, pub outcome: u8 }
    impl Encoder {
        pub fn retirement_supported(&self) -> bool { self.supported }
        pub fn retire_for_display_change(&mut self, _: *mut std::ffi::c_void,
                                          budget: std::time::Duration) -> Result<(), TestError> {
            assert_eq!(budget, std::time::Duration::from_millis(1500));
            if !self.supported || self.outcome == 1 { return Err(TestError("mock error")); }
            if self.outcome == 2 { panic!("mock retirement panic"); }
            Ok(())
        }
    }
    impl Drop for Encoder {
        fn drop(&mut self) { ENCODER_DROPS.fetch_add(1, Ordering::SeqCst); }
    }
}
pub struct CapturerInfo;
impl Drop for CapturerInfo {
    fn drop(&mut self) { CAPTURE_DROPS.fetch_add(1, Ordering::SeqCst); }
}
pub struct Raii;
impl Drop for Raii {
    fn drop(&mut self) { RAII_DROPS.fetch_add(1, Ordering::SeqCst); }
}
pub mod capture_lifecycle {
    use super::*;
    pub struct CaptureGuard<'a>(pub &'a ());
    impl Drop for CaptureGuard<'_> {
        fn drop(&mut self) { GUARD_DROPS.fetch_add(1, Ordering::SeqCst); }
    }
    pub fn change_pending() -> bool { TOPOLOGY.load(Ordering::SeqCst) }
    pub fn record_retirement_failure() { STICKY.store(true, Ordering::SeqCst); }
    pub fn record_unverified_retirement() { UNVERIFIED.store(true, Ordering::SeqCst); }
}
#[path = "@PRODUCTION@"]
mod encoder_retirement;

fn reset(topology: bool) {
    for value in [&ENCODER_DROPS, &CAPTURE_DROPS, &RAII_DROPS, &GUARD_DROPS] {
        value.store(0, Ordering::SeqCst);
    }
    STICKY.store(false, Ordering::SeqCst);
    UNVERIFIED.store(false, Ordering::SeqCst);
    TOPOLOGY.store(topology, Ordering::SeqCst);
}
fn one_owner(supported: bool, outcome: u8, device_known: bool) {
    let mut capture = encoder_retirement::CaptureOwner::new(
        CapturerInfo, Raii, capture_lifecycle::CaptureGuard(&()));
    {
        let (_, _, failed) = capture.parts();
        let encoder = codec::Encoder { supported, outcome };
        let _owner = encoder_retirement::EncoderOwner::new(
            encoder, device_known.then_some(std::ptr::null_mut()), failed);
    }
    drop(capture);
}
fn drops(expected: usize) {
    for value in [&ENCODER_DROPS, &CAPTURE_DROPS, &RAII_DROPS, &GUARD_DROPS] {
        assert_eq!(value.load(Ordering::SeqCst), expected);
    }
}

#[test]
fn successful_retirement_drops_all_existing_owners_once() {
    reset(false);
    one_owner(true, 0, true);
    drops(1);
    assert!(!STICKY.load(Ordering::SeqCst));
}
#[test]
fn errors_panics_and_unknown_devices_do_not_free_or_ack_idle() {
    for (outcome, known) in [(1, true), (2, true), (0, false)] {
        reset(true);
        one_owner(true, outcome, known);
        drops(0);
        assert!(STICKY.load(Ordering::SeqCst));
    }
}
#[test]
fn unsupported_backend_keeps_legacy_disconnect_but_refuses_topology() {
    reset(false);
    one_owner(false, 0, true);
    drops(1);
    assert!(!STICKY.load(Ordering::SeqCst));
    assert!(UNVERIFIED.load(Ordering::SeqCst));
    // This history is topology-only: another ordinary remote owner still runs.
    one_owner(true, 0, true);
    drops(2);
    assert!(!STICKY.load(Ordering::SeqCst));
    assert!(UNVERIFIED.load(Ordering::SeqCst));
    reset(true);
    one_owner(false, 0, true);
    drops(0);
    assert!(STICKY.load(Ordering::SeqCst));
}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--rustc', default='rustc')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    source_module = root / 'src/server/video_service/encoder_retirement.rs'
    escaped = str(source_module).replace('\\', '\\\\').replace('"', '\\"')
    temporary_root = Path(tempfile.gettempdir()).resolve()
    with tempfile.TemporaryDirectory(prefix='xscc-encoder-retirement-', dir=temporary_root) as directory:
        work = Path(directory).resolve()
        if work.parent != temporary_root:
            raise ValueError('Owner mock directory escaped the temporary root')
        source = work / 'test.rs'
        source.write_text(FIXTURE.replace('@PRODUCTION@', escaped))
        executable = work / ('test.exe' if os.name == 'nt' else 'test')
        subprocess.run(
            [args.rustc, '--edition=2021', '--test', str(source), '-o', str(executable)],
            check=True, timeout=120,
        )
        subprocess.run([str(executable), '--test-threads=1'], check=True, timeout=10)


if __name__ == '__main__':
    main()
