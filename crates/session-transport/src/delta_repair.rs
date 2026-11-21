use crate::Transmission;
use base::direct_transport::udp_media::{
    FrameMetadata, FramePlan, MediaError, Pacer, ReceiveReport,
};
use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant},
};

const MAX_TRACKED_FRAGMENTS: usize = 128;
const MAX_PENDING_REPAIRS: usize = 16;
const MAX_REPAIR_FRAME_SIZE: u32 = 64 * 1024;

pub(super) struct DeltaFragment {
    pub(super) plan: FramePlan,
    pub(super) index: u16,
    pub(super) deadline: Instant,
}

#[derive(Default)]
pub(super) struct DeltaRepairs {
    sent: BTreeMap<u64, DeltaFragment>,
    queued: VecDeque<(u64, DeltaFragment)>,
}

impl DeltaRepairs {
    pub(super) fn track(
        &mut self,
        sequence: u64,
        frame: &Transmission,
        index: u16,
        now: Instant,
    ) {
        // Feedback can return after the receiver's keyframe request budget.
        // Repair is still useful until the original transmission expires.
        let deadline = frame.deadline;
        let plan = &frame.plan;
        if deadline <= now || plan.descriptor().total_size > MAX_REPAIR_FRAME_SIZE {
            return;
        }
        if self.sent.len() >= MAX_TRACKED_FRAGMENTS {
            self.sent.pop_first();
        }
        self.sent.insert(
            sequence,
            DeltaFragment {
                plan: plan.clone(),
                index,
                deadline,
            },
        );
    }

    pub(super) fn on_report(
        &mut self,
        pacer: &mut Pacer,
        report: &ReceiveReport,
        now: Instant,
    ) -> Result<(), MediaError> {
        pacer.on_report_with_losses(report, now, |sequence| {
            if let Some(fragment) = self.sent.remove(&sequence) {
                if fragment.deadline > now && self.queued.len() < MAX_PENDING_REPAIRS {
                    self.queued.push_back((sequence, fragment));
                }
            }
        })?;
        for range in &report.ranges {
            for _ in self.sent.extract_if(range.first..=range.last, |_, _| true) {}
        }
        self.queued.retain(|(sequence, fragment)| {
            fragment.deadline > now
                && !report
                    .ranges
                    .iter()
                    .any(|range| (range.first..=range.last).contains(sequence))
        });
        Ok(())
    }

    pub(super) fn expire(&mut self, now: Instant) {
        self.sent.retain(|_, fragment| fragment.deadline > now);
        self.queued.retain(|(_, fragment)| fragment.deadline > now);
    }

    pub(super) fn retain(&mut self, keep: impl Fn(FrameMetadata) -> bool) {
        self.sent
            .retain(|_, fragment| keep(fragment.plan.descriptor().metadata));
        self.queued
            .retain(|(_, fragment)| keep(fragment.plan.descriptor().metadata));
    }

    pub(super) fn take(&mut self, now: Instant) -> Option<DeltaFragment> {
        while let Some((_, fragment)) = self.queued.pop_front() {
            if fragment.deadline > now {
                return Some(fragment);
            }
        }
        None
    }

    pub(super) fn has_queued(&self) -> bool {
        !self.queued.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transmission;
    use base::direct_transport::udp_media::{
        AuthenticatedPacket, DatagramReceiver, DatagramSender, Direction, MediaKind, Reassembler,
        SessionId, DEFAULT_MTU,
    };

    #[test]
    fn retransmitting_the_missing_delta_fragment_completes_the_original_frame() {
        let sid = SessionId([9; 16]);
        let seed = [13; 32];
        let mut sender = DatagramSender::new(&seed, sid, Direction::HostToController).unwrap();
        let mut receiver = DatagramReceiver::new(&seed, sid, Direction::HostToController).unwrap();
        let mut assembler = Reassembler::default();
        let metadata = FrameMetadata {
            kind: MediaKind::Video,
            display_id: 1,
            epoch: 0,
            frame_id: 1,
            keyframe: false,
        };
        let plan = FramePlan::new(metadata, vec![1; DEFAULT_MTU].into(), DEFAULT_MTU).unwrap();
        let later = FramePlan::new(
            FrameMetadata {
                frame_id: 2,
                ..metadata
            },
            vec![2; 2 * DEFAULT_MTU].into(),
            DEFAULT_MTU,
        )
        .unwrap();
        let now = Instant::now();
        let mut pacer = Pacer::new(DEFAULT_MTU, 8_000_000, now).unwrap();
        pacer
            .on_probe_response(Duration::from_millis(4), now)
            .unwrap();
        let mut repairs = DeltaRepairs::default();
        for original in [&plan, &later] {
            let frame = transmission(original.clone(), true, vec![], 8_000_000, now);
            for index in 0..original.fragment_count() {
                let packet = sender.seal_fragment(original, index).unwrap();
                pacer
                    .on_sent(packet.sequence, packet.bytes.len(), now)
                    .unwrap();
                repairs.track(packet.sequence, &frame, index, now);
                if packet.sequence == 0 {
                    continue;
                }
                let arrived = now + Duration::from_millis(3);
                let AuthenticatedPacket::Fragment(fragment) =
                    receiver.open(&packet.bytes, arrived).unwrap()
                else {
                    panic!("expected a video fragment");
                };
                assembler.push(fragment, arrived).unwrap();
            }
        }
        let feedback_at = now + Duration::from_millis(70);
        let report = receiver.take_report(feedback_at).unwrap();
        repairs.on_report(&mut pacer, &report, feedback_at).unwrap();
        let retry = repairs
            .take(feedback_at)
            .expect("repair for the missing fragment");
        assert_eq!(retry.plan.descriptor().metadata, metadata);
        assert_eq!(retry.index, 0);
        assert!(repairs.take(feedback_at).is_none());
        // The repair goes through the same window and authenticated datagram path.
        let size = retry.plan.packet_len(retry.index).unwrap();
        assert_eq!(
            pacer.decide(size, retry.deadline, feedback_at, false),
            base::direct_transport::udp_media::SendDecision::Send
        );
        let packet = sender.seal_fragment(&retry.plan, retry.index).unwrap();
        pacer.on_sent(packet.sequence, size, feedback_at).unwrap();
        let arrived = feedback_at + Duration::from_millis(3);
        let AuthenticatedPacket::Fragment(fragment) =
            receiver.open(&packet.bytes, arrived).unwrap()
        else {
            panic!("expected a repaired video fragment");
        };
        let repaired = assembler.push(fragment, arrived).unwrap().frame.unwrap();
        assert_eq!(repaired.payload, *plan.payload());
    }
}
