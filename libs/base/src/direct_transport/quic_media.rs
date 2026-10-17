//! Versioned, bounded framing for encoded video on QUIC DATAGRAMs.
//!
//! This module does not establish decoder reference continuity. Callers must detect
//! missing whole frame IDs, reorder completed frames if needed, and request a keyframe
//! over reliable control after loss or rejection. Never decode a dependent frame just
//! because reassembly succeeded. Use a fresh assembler for each authenticated session.
use std::{
    collections::{BTreeMap, VecDeque},
    convert::TryInto,
    fmt,
    time::{Duration, Instant},
};

pub const HEADER_SIZE: usize = 26;
pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;
pub const MAX_FRAGMENTS: usize = 8192;
pub const MAX_PENDING_FRAMES: usize = 8;
// Cleanup ceiling for incomplete assemblies, not a send/playout latency target.
pub const FRAME_TTL: Duration = Duration::from_millis(500);
const MAGIC: &[u8; 4] = b"XQMV";
const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameMetadata {
    pub frame_id: u64,
    pub display_id: u32,
    pub keyframe: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaError {
    InvalidTtl,
    DatagramTooSmall,
    FrameTooLarge,
    TooManyFragments,
    InvalidEnvelope,
    ConflictingFragment,
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid QUIC media datagram: {:?}", self)
    }
}
impl std::error::Error for MediaError {}

pub fn fragment_frame(
    metadata: FrameMetadata,
    payload: &[u8],
    max_datagram_size: usize,
) -> Result<Vec<Vec<u8>>, MediaError> {
    if payload.is_empty() || payload.len() > MAX_FRAME_SIZE {
        return Err(MediaError::FrameTooLarge);
    }
    let capacity = max_datagram_size
        .checked_sub(HEADER_SIZE)
        .filter(|&size| size > 0)
        .ok_or(MediaError::DatagramTooSmall)?;
    let count = 1 + (payload.len() - 1) / capacity;
    if count > MAX_FRAGMENTS {
        return Err(MediaError::TooManyFragments);
    }
    let mut datagrams = Vec::with_capacity(count);
    for (index, chunk) in payload.chunks(capacity).enumerate() {
        let mut datagram = Vec::with_capacity(HEADER_SIZE + chunk.len());
        datagram.extend_from_slice(MAGIC);
        datagram.push(VERSION);
        datagram.push(u8::from(metadata.keyframe));
        datagram.extend_from_slice(&metadata.display_id.to_be_bytes());
        datagram.extend_from_slice(&metadata.frame_id.to_be_bytes());
        datagram.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        datagram.extend_from_slice(&(index as u16).to_be_bytes());
        datagram.extend_from_slice(&(count as u16).to_be_bytes());
        datagram.extend_from_slice(chunk);
        datagrams.push(datagram);
    }
    Ok(datagrams)
}

#[derive(Debug, PartialEq, Eq)]
pub struct CompletedFrame {
    pub metadata: FrameMetadata,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryReason {
    Expired,
    Capacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryNeeded {
    pub metadata: FrameMetadata,
    pub reason: RecoveryReason,
}

#[derive(Debug, Default)]
pub struct ReceiveOutcome {
    pub frame: Option<CompletedFrame>,
    pub recovery: Vec<RecoveryNeeded>,
}

struct PendingFrame {
    metadata: FrameMetadata,
    started: Instant,
    total: usize,
    received: usize,
    remaining: usize,
    fragments: Vec<Option<Vec<u8>>>,
}

pub struct Reassembler {
    ttl: Duration,
    pending: BTreeMap<(u32, u64), PendingFrame>,
    // A bounded replay window; callers still reject older/stale frame IDs.
    completed: VecDeque<(u32, u64)>,
}

impl Default for Reassembler {
    fn default() -> Self {
        Self {
            ttl: FRAME_TTL,
            pending: BTreeMap::new(),
            completed: VecDeque::new(),
        }
    }
}

impl Reassembler {
    /// Select a shorter incomplete-frame cleanup deadline for the application.
    /// Completed frames are returned immediately; this never schedules playback.
    pub fn with_ttl(ttl: Duration) -> Result<Self, MediaError> {
        if ttl.is_zero() || ttl > FRAME_TTL {
            return Err(MediaError::InvalidTtl);
        }
        Ok(Self {
            ttl,
            ..Self::default()
        })
    }
    /// Call periodically even when no datagrams arrive. Duplicates do not renew TTL.
    pub fn expire(&mut self, now: Instant) -> Vec<RecoveryNeeded> {
        let mut recovery = Vec::new();
        let ttl = self.ttl;
        self.pending.retain(|_, frame| {
            let keep = now.saturating_duration_since(frame.started) < ttl;
            if !keep {
                recovery.push(RecoveryNeeded {
                    metadata: frame.metadata,
                    reason: RecoveryReason::Expired,
                });
            }
            keep
        });
        recovery
    }

    /// Invalid/conflicting packets return an error; the caller must recover the
    /// decoder reference chain, rather than silently dropping a possible P-frame.
    pub fn push(&mut self, datagram: &[u8], now: Instant) -> Result<ReceiveOutcome, MediaError> {
        if datagram.len() <= HEADER_SIZE
            || &datagram[..4] != MAGIC
            || datagram[4] != VERSION
            || datagram[5] > 1
        {
            return Err(MediaError::InvalidEnvelope);
        }
        let read_u32 = |range: std::ops::Range<usize>| -> Result<u32, MediaError> {
            Ok(u32::from_be_bytes(
                datagram[range]
                    .try_into()
                    .map_err(|_| MediaError::InvalidEnvelope)?,
            ))
        };
        let metadata = FrameMetadata {
            display_id: read_u32(6..10)?,
            frame_id: u64::from_be_bytes(
                datagram[10..18]
                    .try_into()
                    .map_err(|_| MediaError::InvalidEnvelope)?,
            ),
            keyframe: datagram[5] == 1,
        };
        let total = read_u32(18..22)? as usize;
        let index = u16::from_be_bytes([datagram[22], datagram[23]]) as usize;
        let count = u16::from_be_bytes([datagram[24], datagram[25]]) as usize;
        let payload = &datagram[HEADER_SIZE..];
        if total == 0
            || total > MAX_FRAME_SIZE
            || count == 0
            || count > MAX_FRAGMENTS
            || count > total
            || index >= count
            || payload.len() > total
        {
            return Err(MediaError::InvalidEnvelope);
        }
        let mut outcome = ReceiveOutcome {
            frame: None,
            recovery: self.expire(now),
        };
        let key = (metadata.display_id, metadata.frame_id);
        if self.completed.contains(&key) {
            return Ok(outcome);
        }
        if !self.pending.contains_key(&key) {
            if self.pending.len() == MAX_PENDING_FRAMES {
                outcome.recovery.push(RecoveryNeeded {
                    metadata,
                    reason: RecoveryReason::Capacity,
                });
                return Ok(outcome);
            }
            self.pending.insert(
                key,
                PendingFrame {
                    metadata,
                    started: now,
                    total,
                    received: 0,
                    remaining: count,
                    fragments: vec![None; count],
                },
            );
        }
        let frame = self
            .pending
            .get_mut(&key)
            .ok_or(MediaError::InvalidEnvelope)?;
        if frame.metadata != metadata || frame.total != total || frame.fragments.len() != count {
            return Err(MediaError::ConflictingFragment);
        }
        if let Some(existing) = &frame.fragments[index] {
            return if existing == payload {
                Ok(outcome)
            } else {
                Err(MediaError::ConflictingFragment)
            };
        }
        if payload.len() > frame.total - frame.received {
            return Err(MediaError::InvalidEnvelope);
        }
        frame.received += payload.len();
        frame.remaining -= 1;
        frame.fragments[index] = Some(payload.to_vec());
        if frame.remaining == 0 {
            let frame = self
                .pending
                .remove(&key)
                .ok_or(MediaError::InvalidEnvelope)?;
            if frame.received != frame.total {
                return Err(MediaError::InvalidEnvelope);
            }
            let mut payload = Vec::with_capacity(frame.total);
            for fragment in frame.fragments.into_iter().flatten() {
                payload.extend_from_slice(&fragment);
            }
            if self.completed.len() == 64 {
                self.completed.pop_front();
            }
            self.completed.push_back(key);
            outcome.frame = Some(CompletedFrame { metadata, payload });
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(frame_id: u64) -> FrameMetadata {
        FrameMetadata {
            frame_id,
            display_id: 2,
            keyframe: frame_id == 0,
        }
    }

    #[test]
    fn mtu_fragmentation_reassembles_out_of_order_and_ignores_duplicates() {
        let payload = vec![23; 4000];
        let fragments = fragment_frame(metadata(0), &payload, 1200).unwrap();
        assert!(fragments.iter().all(|part| part.len() <= 1200));
        let mut assembler = Reassembler::default();
        let now = Instant::now();
        assert!(assembler.push(&fragments[2], now).unwrap().frame.is_none());
        assert!(assembler.push(&fragments[2], now).unwrap().frame.is_none());
        for index in [3, 1] {
            assert!(assembler
                .push(&fragments[index], now)
                .unwrap()
                .frame
                .is_none());
        }
        assert_eq!(
            assembler.push(&fragments[0], now).unwrap().frame,
            Some(CompletedFrame {
                metadata: metadata(0),
                payload
            })
        );
        for fragment in &fragments {
            assert!(assembler.push(fragment, now).unwrap().frame.is_none());
        }
    }

    #[test]
    fn bounded_assembly_expires_without_extending_ttl_on_duplicates() {
        let mut assembler = Reassembler::default();
        let now = Instant::now();
        for id in 0..MAX_PENDING_FRAMES as u64 {
            let fragments = fragment_frame(metadata(id), &[1; 10], HEADER_SIZE + 5).unwrap();
            assembler.push(&fragments[0], now).unwrap();
            assembler.push(&fragments[0], now + FRAME_TTL / 2).unwrap();
        }
        let extra = fragment_frame(metadata(99), &[1; 10], HEADER_SIZE + 5).unwrap();
        assert_eq!(
            assembler.push(&extra[0], now).unwrap().recovery,
            vec![RecoveryNeeded {
                metadata: metadata(99),
                reason: RecoveryReason::Capacity
            }]
        );
        let expired = assembler.expire(now + FRAME_TTL);
        assert_eq!(expired.len(), MAX_PENDING_FRAMES);
        assert!(expired
            .iter()
            .all(|event| event.reason == RecoveryReason::Expired));
        assert!(assembler.expire(now + FRAME_TTL).is_empty());
        let mut short = Reassembler::with_ttl(Duration::from_millis(20)).unwrap();
        short.push(&extra[0], now).unwrap();
        assert_eq!(short.expire(now + Duration::from_millis(20)).len(), 1);
        assert!(Reassembler::with_ttl(Duration::ZERO).is_err());
    }

    #[test]
    fn rejects_invalid_limits_envelopes_and_conflicting_fragments() {
        assert_eq!(
            fragment_frame(metadata(0), &[1], HEADER_SIZE),
            Err(MediaError::DatagramTooSmall)
        );
        assert_eq!(
            fragment_frame(metadata(0), &vec![1; MAX_FRAGMENTS + 1], HEADER_SIZE + 1),
            Err(MediaError::TooManyFragments)
        );
        let mut packet = fragment_frame(metadata(0), &[1; 10], HEADER_SIZE + 5)
            .unwrap()
            .remove(0);
        let mut assembler = Reassembler::default();
        let now = Instant::now();
        for length in 0..=HEADER_SIZE {
            assert!(assembler.push(&packet[..length], now).is_err());
        }
        assembler.push(&packet, now).unwrap();
        packet[HEADER_SIZE] = 2;
        assert_eq!(
            assembler.push(&packet, now).unwrap_err(),
            MediaError::ConflictingFragment
        );
        packet[18..22].copy_from_slice(&((MAX_FRAME_SIZE + 1) as u32).to_be_bytes());
        assert!(assembler.push(&packet, now).is_err());
        packet[4] = 2;
        assert!(assembler.push(&packet, now).is_err());
    }
}
