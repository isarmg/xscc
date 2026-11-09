#!/usr/bin/env python3
"""Run actual MediaCodec delivery method bodies against a fake NDK boundary.

CPU regressions only: the native Android codec, conversion backend and physical
presentation still require native compilation and device acceptance. The decoder
control flow below is extracted from production, not reimplemented by the test.
"""
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / 'libs/scrap/src/common/mediacodec.rs'
METHODS = ('decode', 'decode_with_callback', 'has_pending_output',
           'take_decoded_output_count', 'poll_output', 'poll_output_with_callback')
TEST_NAMES = {
    'every_output_before_and_after_input_is_released_then_published_without_overwrite',
    'backpressure_keeps_input_borrowed_and_drains_each_output_before_retrying',
    'format_notifications_empty_buffers_and_release_errors_never_publish_fake_outputs',
}


def production_method(source, name):
    marker = '    pub fn ' + name + '('
    if source.count(marker) != 1:
        raise ValueError('Missing or ambiguous production method: ' + name)
    start = source.index(marker)
    opening = source.index('{', start)
    depth, end = 1, opening + 1
    while depth and end < len(source):
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    if depth:
        raise ValueError('Unterminated production method: ' + name)
    return source[start:end]


PREFIX = r"""
use std::{cell::RefCell, collections::VecDeque, rc::Rc, time::{Duration, Instant}};
#[derive(Debug)] struct Status(i32);
#[derive(Debug)] enum NdkMediaError { UnknownResult(Status), Release, Queue }
#[derive(Debug)] enum Error { Ndk(NdkMediaError), Pixels, Message(String) }
impl From<NdkMediaError> for Error { fn from(error: NdkMediaError) -> Self { Self::Ndk(error) } }
type ResultType<T> = Result<T, Error>;
macro_rules! bail { ($($arg:tt)*) => { return Err(Error::Message(format!($($arg)*))) }; }
#[derive(Default)] struct ImageRgb { raw: Vec<u8> }
struct Format;
struct Input { bytes: Vec<u8> }
impl Input { fn buffer_mut(&mut self) -> &mut [u8] { &mut self.bytes } }
struct Output { value: u8, empty: bool }
impl Output {
    fn format(&self) -> Format { Format }
    fn buffer(&self) -> &[u8] { if self.empty { &[] } else { std::slice::from_ref(&self.value) } }
}
enum Event { Frame(u8), Empty, Changed, BuffersChanged, Again }
struct FakeCodec {
    events: RefCell<VecDeque<Event>>, after_queue: RefCell<Vec<Event>>,
    on_miss: RefCell<VecDeque<Vec<Event>>>, inputs: RefCell<VecDeque<bool>>,
    queued: RefCell<Vec<Vec<u8>>>, trace: Rc<RefCell<Vec<String>>>, release_fails: bool, queue_fails: bool,
}
impl Default for FakeCodec {
    fn default() -> Self { Self { events: Default::default(), after_queue: Default::default(),
        on_miss: Default::default(), inputs: Default::default(), queued: Default::default(),
        trace: Default::default(), release_fails: false, queue_fails: false } }
}
impl FakeCodec {
    fn dequeue_output_buffer(&self, _: Duration) -> Result<Option<Output>, NdkMediaError> {
        match self.events.borrow_mut().pop_front().unwrap_or(Event::Again) {
            Event::Frame(value) => Ok(Some(Output {value, empty:false})),
            Event::Empty => Ok(Some(Output {value:0, empty:true})),
            Event::Changed => Err(NdkMediaError::UnknownResult(Status(-2))),
            Event::BuffersChanged => Err(NdkMediaError::UnknownResult(Status(-3))),
            Event::Again => Ok(None),
        }
    }
    fn release_output_buffer(&self, output: Output, render: bool) -> Result<(), NdkMediaError> {
        assert!(!render);
        self.trace.borrow_mut().push(format!("release:{}", output.value));
        if self.release_fails { Err(NdkMediaError::Release) } else { Ok(()) }
    }
    fn dequeue_input_buffer(&self, _: Duration) -> Result<Option<Input>, NdkMediaError> {
        if self.inputs.borrow_mut().pop_front().unwrap_or(true) { return Ok(Some(Input {bytes:vec![0;16]})); }
        self.trace.borrow_mut().push("input-backpressure".into());
        self.events.borrow_mut().extend(self.on_miss.borrow_mut().pop_front().unwrap_or_default());
        Ok(None)
    }
    fn queue_input_buffer(&self, input: Input, _: usize, size: usize, _: u64, _: u32) -> Result<(), NdkMediaError> {
        if self.queue_fails { return Err(NdkMediaError::Queue); }
        self.queued.borrow_mut().push(input.bytes[..size].to_vec());
        self.trace.borrow_mut().push("queue-input".into());
        self.events.borrow_mut().extend(self.after_queue.borrow_mut().drain(..));
        Ok(())
    }
}
fn convert_output(_: &Format, bytes: &[u8], rgb: &mut ImageRgb) -> ResultType<()> {
    if bytes.is_empty() { return Err(Error::Pixels); }
    rgb.raw.clear(); rgb.raw.extend_from_slice(bytes); Ok(())
}
struct MediaCodecDecoder { decoder:FakeCodec, configuration:Option<(i32,i32,i32)>,
    pending_output:usize, decoded_outputs:usize }
impl Default for MediaCodecDecoder {
    fn default() -> Self { Self { decoder:FakeCodec::default(), configuration:Some((1,1,120)), pending_output:0, decoded_outputs:0 } }
}
impl MediaCodecDecoder {
"""

TESTS = r"""
}
#[test]
fn every_output_before_and_after_input_is_released_then_published_without_overwrite() {
    let mut decoder = MediaCodecDecoder::default();
    decoder.decoder.events.borrow_mut().extend([Event::Changed, Event::Frame(1), Event::Again]);
    decoder.decoder.after_queue.borrow_mut().extend([Event::BuffersChanged, Event::Frame(2), Event::Frame(3)]);
    let trace = decoder.decoder.trace.clone();
    let mut images = Vec::new();
    let mut rgb = ImageRgb::default();
    assert!(decoder.decode_with_callback(&[7,8,9], &mut rgb, &mut |image| {
        let value = image.raw[0];
        assert_eq!(trace.borrow().last().unwrap(), &format!("release:{value}"));
        trace.borrow_mut().push(format!("publish:{value}"));
        images.push(std::mem::take(&mut image.raw));
    }).unwrap());
    assert_eq!(images, vec![vec![1],vec![2],vec![3]]);
    assert_eq!(*decoder.decoder.queued.borrow(), vec![vec![7,8,9]]);
    assert_eq!(*trace.borrow(), ["release:1", "publish:1", "queue-input", "release:2", "publish:2", "release:3", "publish:3"]);
    assert_eq!(decoder.take_decoded_output_count(), 3);
    assert!(!decoder.has_pending_output());
    decoder.decoder.events.borrow_mut().extend([Event::Frame(4),Event::Frame(5)]);
    assert!(decoder.poll_output(&mut rgb).unwrap());
    assert_eq!(rgb.raw, vec![5]); // Non-callback callers retain the legacy final RGB.
}
#[test]
fn backpressure_keeps_input_borrowed_and_drains_each_output_before_retrying() {
    let mut decoder = MediaCodecDecoder::default();
    decoder.decoder.events.borrow_mut().push_back(Event::Again);
    decoder.decoder.inputs.borrow_mut().extend([false,false,true]);
    decoder.decoder.on_miss.borrow_mut().push_back(vec![Event::Changed,Event::Frame(6),Event::Again]);
    let mut rgb = ImageRgb::default();
    let mut published = 0;
    assert!(decoder.decode_with_callback(&[42], &mut rgb, &mut |_| published += 1).unwrap());
    assert_eq!(published,1);
    assert_eq!(*decoder.decoder.queued.borrow(), vec![vec![42]]);
    assert_eq!(decoder.take_decoded_output_count(),1);
    // A failed later input cannot undo an actual output already drained before it.
    decoder.decoder.queue_fails = true;
    decoder.decoder.events.borrow_mut().extend([Event::Frame(8), Event::Again]);
    assert!(decoder.decode_with_callback(&[43], &mut rgb, &mut |_| published += 1).is_err());
    assert_eq!(published,2);
    assert_eq!(decoder.take_decoded_output_count(),1);
    assert_eq!(*decoder.decoder.queued.borrow(), vec![vec![42]]);
}
#[test]
fn format_notifications_empty_buffers_and_release_errors_never_publish_fake_outputs() {
    let mut decoder = MediaCodecDecoder::default();
    let mut rgb = ImageRgb::default();
    let mut published = 0;
    decoder.decoder.events.borrow_mut().extend([Event::Changed,Event::BuffersChanged]);
    assert!(!decoder.poll_output_with_callback(&mut rgb, &mut |_| published += 1).unwrap());
    assert_eq!(decoder.take_decoded_output_count(),0);
    decoder.decoder.events.borrow_mut().push_back(Event::Empty);
    decoder.pending_output=1;
    assert!(decoder.poll_output_with_callback(&mut rgb, &mut |_| published += 1).is_err());
    assert_eq!(published,0);
    assert_eq!(decoder.decoder.trace.borrow().last().unwrap(), "release:0");
    assert_eq!(decoder.take_decoded_output_count(),0);
    assert!(!decoder.has_pending_output());
    decoder.decoder.release_fails=true;
    decoder.decoder.events.borrow_mut().push_back(Event::Frame(7));
    assert!(decoder.poll_output_with_callback(&mut rgb, &mut |_| published += 1).is_err());
    assert_eq!(published,0);
    assert_eq!(decoder.take_decoded_output_count(),0);
    assert_eq!(decoder.decoder.trace.borrow().last().unwrap(), "release:7");
}
"""


def main():
    rustc = shutil.which('rustc')
    if not rustc:
        raise SystemExit('Requires an installed Rust compiler; nothing is downloaded.')
    source = SOURCE.read_text(encoding='utf-8')
    methods = '\n\n'.join(production_method(source, name) for name in METHODS)
    with tempfile.TemporaryDirectory(prefix='gpui-android-each-output-') as directory:
        harness = Path(directory) / 'actual-mediacodec-delivery.rs'
        executable = Path(directory) / ('delivery-tests.exe' if os.name == 'nt' else 'delivery-tests')
        harness.write_text('#![allow(dead_code)]\n' + PREFIX + methods + TESTS, encoding='utf-8')
        subprocess.run([rustc, '--edition', '2024', '--test', str(harness), '-o', str(executable)], check=True)
        listing = subprocess.check_output([str(executable), '--list'], text=True)
        selected = set(re.findall(r'^([^\n]+): test$', listing, re.M))
        if selected != TEST_NAMES:
            raise SystemExit('MediaCodec per-output regression selection differs: ' + repr(selected))
        result = subprocess.run([str(executable), '--nocapture'], check=True, capture_output=True, text=True)
        print(result.stdout, end='')
        if not re.search(r'^test result: ok\. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;', result.stdout, re.M):
            raise SystemExit('All three MediaCodec per-output regressions must actually pass.')
    print('Production delivery methods SHA256: ' + hashlib.sha256(methods.encode()).hexdigest())


if __name__ == '__main__':
    main()
