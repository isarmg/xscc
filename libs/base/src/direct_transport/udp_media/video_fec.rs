use super::{FrameDescriptor, FramePlan, MediaError, MediaKind, DEFAULT_MTU, FRAGMENT_OVERHEAD};
use bytes::Bytes;

pub(super) const SHARD_BYTES: usize = DEFAULT_MTU - FRAGMENT_OVERHEAD;
const DATA_PER_PARITY: u16 = 5;

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub(super) groups: u16,
    data: u16,
    tail: usize,
}

impl Layout {
    pub(super) fn new(descriptor: FrameDescriptor) -> Option<Self> {
        let size = descriptor.total_size as usize;
        let data = descriptor.fragment_count;
        (descriptor.metadata.kind == MediaKind::Video
            && data >= DATA_PER_PARITY
            && usize::from(data) == size.div_ceil(SHARD_BYTES))
        .then(|| Self {
            groups: data / DATA_PER_PARITY,
            data,
            tail: size - (usize::from(data) - 1) * SHARD_BYTES,
        })
    }

    fn members(self, group: u16) -> impl Iterator<Item = u16> {
        // Distribute adjacent datagram losses across distinct parity groups.
        (group..self.data).step_by(usize::from(self.groups))
    }

    fn data_size(self, index: u16) -> usize {
        if index + 1 == self.data {
            self.tail
        } else {
            SHARD_BYTES
        }
    }

    pub(super) fn encode(self, plan: &FramePlan, group: u16) -> Result<Vec<u8>, MediaError> {
        if group >= self.groups {
            return Err(MediaError::InvalidEnvelope);
        }
        let mut parity = vec![0; SHARD_BYTES];
        for index in self.members(group) {
            xor(&mut parity, plan.fragment(index)?);
        }
        Ok(parity)
    }

    pub(super) fn recover(
        self,
        group: u16,
        parity: &Bytes,
        data: &[Option<Bytes>],
    ) -> Option<(usize, Bytes)> {
        let mut missing = None;
        for index in self.members(group) {
            match &data[usize::from(index)] {
                None if missing.is_none() => missing = Some(index),
                Some(bytes) if bytes.len() == self.data_size(index) => {}
                _ => return None,
            }
        }
        let index = missing?;
        let mut recovered = parity.to_vec();
        for index in self.members(group) {
            if let Some(bytes) = &data[usize::from(index)] {
                xor(&mut recovered, bytes);
            }
        }
        recovered.truncate(self.data_size(index));
        Some((usize::from(index), Bytes::from(recovered)))
    }
}

fn xor(into: &mut [u8], source: &[u8]) {
    for (byte, next) in into.iter_mut().zip(source) {
        *byte ^= *next;
    }
}

pub(super) fn reserve_wire_budget(bps: u64) -> u64 {
    bps.saturating_mul(u64::from(DATA_PER_PARITY)) / (u64::from(DATA_PER_PARITY) + 1)
}
