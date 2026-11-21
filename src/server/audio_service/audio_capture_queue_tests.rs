use super::{super::create_format_msg, *};
use crate::{
    audio_resampler::allocation_tests::assert_no_allocations,
    server::{ConnInner, Service},
};
use base::message_proto::{message, misc};
use hbb_common::tokio;
use std::{sync::mpsc, time::Duration};

const PACKET_SAMPLES: usize = 4;
const TEST_TIMEOUT: Duration = Duration::from_secs(2);

struct WorkerPause {
    entered: mpsc::Sender<()>,
    resume: mpsc::Receiver<()>,
}

fn paused_worker(receiver: CapturePcmReceiver, pause: WorkerPause) -> CapturePcmReceiver {
    let consumed = {
        let mut buffers = receiver.handoff.buffers.lock().unwrap();
        let consumed = buffers.ready.pop_front().unwrap().1;
        pause.entered.send(()).unwrap();
        pause.resume.recv().unwrap();
        consumed
    };
    receiver.recycle(consumed);
    receiver
}

fn assert_callback_progress() {
    let (mut sender, receiver) =
        new_pcm_handoff(CAPTURE_PCM_QUEUE_PACKETS, PACKET_SAMPLES).unwrap();
    for sequence in 0..CAPTURE_PCM_QUEUE_PACKETS {
        sender.submit(&[sequence as f32; PACKET_SAMPLES]);
    }
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let pause = WorkerPause {
        entered: entered_tx,
        resume: resume_rx,
    };
    let worker = std::thread::spawn(move || paused_worker(receiver, pause));
    sender.set_wake_thread(worker.thread().clone()).unwrap();
    entered_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    let (completed_tx, completed_rx) = mpsc::channel();
    let callback = std::thread::spawn(move || {
        let packet = [CAPTURE_PCM_QUEUE_PACKETS as f32; PACKET_SAMPLES];
        assert_no_allocations(|| sender.submit(&packet));
        completed_tx.send(()).unwrap();
        sender
    });
    let completed = completed_rx.recv_timeout(TEST_TIMEOUT);
    // Release and join both threads before asserting progress, including on failure.
    resume_tx.send(()).unwrap();
    let receiver = worker.join().unwrap();
    let mut sender = callback.join().unwrap();
    assert!(completed.is_ok(), "callback waited for the encoder worker");
    assert_eq!(
        receiver.take_loss(),
        CapturePcmLoss {
            dropped: 1,
            contention_dropped: 1,
            ..Default::default()
        }
    );
    assert_packets_after_contention(&mut sender, &receiver);
}

fn assert_packets_after_contention(sender: &mut CapturePcmSender, receiver: &CapturePcmReceiver) {
    let next_sequence = CAPTURE_PCM_QUEUE_PACKETS + 1;
    sender.submit(&[next_sequence as f32; PACKET_SAMPLES]);
    for expected in (1..CAPTURE_PCM_QUEUE_PACKETS).chain(std::iter::once(next_sequence)) {
        let (sequence, packet) = receiver.pop_packet().unwrap();
        assert_eq!(sequence, expected);
        assert_eq!(packet, [expected as f32; PACKET_SAMPLES]);
        receiver.recycle(packet);
    }
    assert!(receiver.is_empty());
    assert!(receiver.take_loss().is_empty());
    assert_pool_restored(receiver, CAPTURE_PCM_QUEUE_PACKETS);
}

fn assert_pool_restored(receiver: &CapturePcmReceiver, capacity: usize) {
    let buffers = receiver.handoff.buffers.lock().unwrap();
    assert_eq!(buffers.available.len(), capacity);
    assert!(buffers
        .available
        .iter()
        .all(|buffer| buffer.capacity() >= PACKET_SAMPLES));
}

#[test]
fn callback_finishes_while_worker_releases_a_ready_packet() {
    assert_callback_progress();
}

#[tokio::test]
async fn capture_replacement_sends_format_before_new_audio() {
    let service = GenericService::new("audio".into(), true);
    let (first, mut existing) = tokio::sync::mpsc::unbounded_channel();
    service.on_subscribe(ConnInner::new(1, Some(first), None));
    service.publish_audio_format(&Arc::new(create_format_msg(48_000, 2)), true);
    existing.try_recv().unwrap();

    let config = CaptureEncoderConfig {
        sample_rate: 24_000,
        encode_channel: Channels::Mono,
        max_packet_samples: 240,
    };
    let (mut sender, mut worker) = start_capture_encoder(config, service.clone()).unwrap();
    sender.submit(&[0.25; 240]);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), existing.recv())
            .await
            .is_err()
    );

    let (later, mut joining) = tokio::sync::mpsc::unbounded_channel();
    service.on_subscribe(ConnInner::new(2, Some(later), None));
    service.publish_audio_format(&Arc::new(create_format_msg(24_000, 1)), true);
    worker.start();

    for peer in [&mut existing, &mut joining] {
        let (_, format) = peer.try_recv().unwrap();
        assert!(matches!(&format.union,
            Some(message::Union::Misc(misc)) if matches!(&misc.union,
                Some(misc::Union::AudioFormat(format)) if format.channels == 1 && format.sample_rate == 24_000)));
        let (_, packet) = tokio::time::timeout(TEST_TIMEOUT, peer.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(&packet.union, Some(message::Union::AudioFrame(frame)) if !frame.data.is_empty())
        );
    }
}
