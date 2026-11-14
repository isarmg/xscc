//! Authenticated UDP media primitives. TCP owns login, permission and format epochs.
//! A SID lookup is not authentication; path validation must precede sending media.
use bytes::Bytes;
use hbb_common::sodiumoxide::{
    self,
    crypto::{aead::xchacha20poly1305_ietf as aead, generichash},
};
use serde_derive::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    convert::{TryFrom, TryInto},
    fmt,
    time::{Duration, Instant},
};

pub const VERSION: u8 = 1;
pub const DEFAULT_MTU: usize = 1200;
pub const MAX_DATAGRAM_SIZE: usize = 65_507;
pub const MAX_VIDEO_FRAME: usize = 8 * 1024 * 1024;
pub const MAX_AUDIO_FRAME: usize = 64 * 1024;
pub const MAX_FRAGMENTS: usize = 8192;
pub const MAX_ACK_RANGES: usize = 64;
pub const REPLAY_WINDOW: u64 = 8192;
pub const REPORT_INTERVAL: Duration = Duration::from_millis(20);
pub const DELTA_TTL: Duration = Duration::from_millis(100);
pub const AUDIO_TTL: Duration = Duration::from_millis(40);
const MAGIC: &[u8; 4] = b"XSMU";
const PREFIX: usize = 30;
const FRAGMENT_HEADER: usize = 30;
pub const FRAGMENT_OVERHEAD: usize = PREFIX + aead::TAGBYTES + FRAGMENT_HEADER;
const REPLAY_BUCKETS: usize = REPLAY_WINDOW as usize / 64 + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    ControllerToHost,
    HostToController,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MediaKind {
    Video,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameMetadata {
    pub kind: MediaKind,
    pub display_id: u32,
    pub epoch: u64,
    pub frame_id: u64,
    pub keyframe: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameDescriptor {
    pub metadata: FrameMetadata,
    pub total_size: u32,
    pub fragment_count: u16,
}
impl FrameDescriptor {
    pub fn validate(&self) -> Result<(), MediaError> {
        let (size, fragments) = match self.metadata.kind {
            MediaKind::Video => (MAX_VIDEO_FRAME, MAX_FRAGMENTS),
            MediaKind::Audio => {
                if self.metadata.keyframe || self.metadata.display_id != 0 {
                    return Err(MediaError::InvalidEnvelope);
                }
                (MAX_AUDIO_FRAME, 64)
            }
        };
        if self.total_size == 0
            || self.total_size as usize > size
            || self.fragment_count == 0
            || self.fragment_count as usize > fragments
            || u32::from(self.fragment_count) > self.total_size
        {
            return Err(MediaError::InvalidEnvelope);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameNotice {
    Sent(FrameDescriptor),
    Abandoned(FrameMetadata),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyframeNack {
    pub descriptor: FrameDescriptor,
    /// Bit i asks for fragment i; unused high bits must be zero.
    pub missing: Vec<u8>,
}
impl KeyframeNack {
    pub fn validate(&self) -> Result<(), MediaError> {
        self.descriptor.validate()?;
        let count = self.descriptor.fragment_count as usize;
        if !self.descriptor.metadata.keyframe
            || self.missing.len() != count.div_ceil(8)
            || (!count.is_multiple_of(8) && self.missing[count / 8] >> (count % 8) != 0)
        {
            return Err(MediaError::InvalidFeedback);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyframeComplete {
    pub metadata: FrameMetadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckRange {
    pub first: u64,
    pub last: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveReport {
    pub largest: u64,
    pub ack_delay_us: u32,
    /// Inclusive, descending, disjoint ranges. Only media fragments elicit ACKs.
    pub ranges: Vec<AckRange>,
}
impl ReceiveReport {
    pub fn validate(&self) -> Result<(), MediaError> {
        if self.ranges.is_empty()
            || self.ranges.len() > MAX_ACK_RANGES
            || self.ranges[0].last != self.largest
            || self.ack_delay_us > 250_000
        {
            return Err(MediaError::InvalidFeedback);
        }
        let mut preceding = None;
        for range in &self.ranges {
            if range.first > range.last || preceding.is_some_and(|first: u64| range.last >= first) {
                return Err(MediaError::InvalidFeedback);
            }
            preceding = Some(range.first);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeKind {
    Probe,
    Challenge,
    Response,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaError {
    Crypto,
    InvalidEnvelope,
    InvalidFeedback,
    WrongSession,
    Replay,
    SequenceExhausted,
    DatagramTooSmall,
    DatagramTooLarge,
    FrameTooLarge,
    TooManyFragments,
    ConflictingFragment,
    Capacity,
    WrongEpoch,
    Expired,
}
impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UDP media: {self:?}")
    }
}
impl std::error::Error for MediaError {}

#[derive(Clone)]
pub struct FramePlan {
    descriptor: FrameDescriptor,
    payload: Bytes,
    capacity: usize,
}
impl FramePlan {
    pub fn new(metadata: FrameMetadata, payload: Bytes, mtu: usize) -> Result<Self, MediaError> {
        if mtu > MAX_DATAGRAM_SIZE {
            return Err(MediaError::DatagramTooLarge);
        }
        let capacity = mtu
            .checked_sub(FRAGMENT_OVERHEAD)
            .filter(|value| *value > 0)
            .ok_or(MediaError::DatagramTooSmall)?;
        if payload.is_empty()
            || payload.len()
                > match metadata.kind {
                    MediaKind::Video => MAX_VIDEO_FRAME,
                    MediaKind::Audio => MAX_AUDIO_FRAME,
                }
        {
            return Err(MediaError::FrameTooLarge);
        }
        let count = payload.len().div_ceil(capacity);
        let fragment_count = u16::try_from(count).map_err(|_| MediaError::TooManyFragments)?;
        let descriptor = FrameDescriptor {
            metadata,
            total_size: payload.len() as u32,
            fragment_count,
        };
        descriptor.validate()?;
        Ok(Self {
            descriptor,
            payload,
            capacity,
        })
    }
    pub fn descriptor(&self) -> FrameDescriptor {
        self.descriptor
    }
    pub fn payload(&self) -> &Bytes {
        &self.payload
    }
    pub fn fragment_count(&self) -> u16 {
        self.descriptor.fragment_count
    }
    pub fn packet_len(&self, index: u16) -> Result<usize, MediaError> {
        Ok(FRAGMENT_OVERHEAD + self.fragment(index)?.len())
    }
    fn fragment(&self, index: u16) -> Result<&[u8], MediaError> {
        if index >= self.fragment_count() {
            return Err(MediaError::InvalidEnvelope);
        }
        let start = usize::from(index) * self.capacity;
        Ok(&self.payload[start..self.payload.len().min(start + self.capacity)])
    }
}

pub struct SealedDatagram {
    pub sequence: u64,
    pub bytes: Vec<u8>,
    pub ack_eliciting: bool,
}

#[derive(Debug)]
pub struct Fragment {
    pub descriptor: FrameDescriptor,
    pub index: u16,
    pub payload: Bytes,
}

#[derive(Debug)]
pub enum AuthenticatedPacket {
    Probe { kind: ProbeKind, token: [u8; 16] },
    Fragment(Fragment),
    Report(ReceiveReport),
}

fn key(seed: &[u8; 32], sid: SessionId, direction: Direction) -> Result<aead::Key, MediaError> {
    sodiumoxide::init().map_err(|_| MediaError::Crypto)?;
    let mut context = b"xscc:udp-media:xchacha20poly1305:v1\0".to_vec();
    context.extend_from_slice(&sid.0);
    context.extend_from_slice(match direction {
        Direction::ControllerToHost => b"controller-to-host",
        Direction::HostToController => b"host-to-controller",
    });
    let digest = generichash::hash(&context, Some(aead::KEYBYTES), Some(seed))
        .map_err(|_| MediaError::Crypto)?;
    aead::Key::from_slice(digest.as_ref()).ok_or(MediaError::Crypto)
}
fn nonce(sid: SessionId, sequence: u64) -> aead::Nonce {
    let mut value = [0; aead::NONCEBYTES];
    value[..16].copy_from_slice(&sid.0);
    value[16..].copy_from_slice(&sequence.to_be_bytes());
    aead::Nonce(value)
}
pub fn peek_session_id(packet: &[u8]) -> Result<SessionId, MediaError> {
    if packet.len() < PREFIX + aead::TAGBYTES
        || packet.len() > MAX_DATAGRAM_SIZE
        || &packet[..4] != MAGIC
        || packet[4] != VERSION
        || !(1..=5).contains(&packet[5])
    {
        return Err(MediaError::InvalidEnvelope);
    }
    let mut value = [0; 16];
    value.copy_from_slice(&packet[6..22]);
    Ok(SessionId(value))
}
/// Untrusted source-address admission hint only; `open` must still authenticate it.
pub fn peek_probe_kind(packet: &[u8]) -> Result<Option<ProbeKind>, MediaError> {
    peek_session_id(packet)?;
    Ok(match packet[5] {
        2 => Some(ProbeKind::Probe),
        3 => Some(ProbeKind::Challenge),
        4 => Some(ProbeKind::Response),
        _ => None,
    })
}

pub struct DatagramSender {
    sid: SessionId,
    key: aead::Key,
    next: u64,
}
impl DatagramSender {
    pub fn new(seed: &[u8; 32], sid: SessionId, direction: Direction) -> Result<Self, MediaError> {
        Ok(Self {
            sid,
            key: key(seed, sid, direction)?,
            next: 0,
        })
    }
    fn seal(&mut self, kind: u8, body: &[u8]) -> Result<SealedDatagram, MediaError> {
        if PREFIX + aead::TAGBYTES + body.len() > MAX_DATAGRAM_SIZE {
            return Err(MediaError::DatagramTooLarge);
        }
        let sequence = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or(MediaError::SequenceExhausted)?;
        let mut bytes = Vec::with_capacity(PREFIX + aead::TAGBYTES + body.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&[VERSION, kind]);
        bytes.extend_from_slice(&self.sid.0);
        bytes.extend_from_slice(&sequence.to_be_bytes());
        let encrypted = aead::seal(body, Some(&bytes), &nonce(self.sid, sequence), &self.key);
        bytes.extend_from_slice(&encrypted);
        Ok(SealedDatagram {
            sequence,
            bytes,
            ack_eliciting: kind == 1,
        })
    }
    pub fn seal_probe(
        &mut self,
        kind: ProbeKind,
        token: [u8; 16],
    ) -> Result<SealedDatagram, MediaError> {
        self.seal(
            match kind {
                ProbeKind::Probe => 2,
                ProbeKind::Challenge => 3,
                ProbeKind::Response => 4,
            },
            &token,
        )
    }
    pub fn seal_fragment(
        &mut self,
        frame: &FramePlan,
        index: u16,
    ) -> Result<SealedDatagram, MediaError> {
        let descriptor = frame.descriptor;
        let metadata = descriptor.metadata;
        let payload = frame.fragment(index)?;
        let mut body = Vec::with_capacity(FRAGMENT_HEADER + payload.len());
        body.push(match metadata.kind {
            MediaKind::Video => 1,
            MediaKind::Audio => 2,
        });
        body.push(u8::from(metadata.keyframe));
        body.extend_from_slice(&metadata.display_id.to_be_bytes());
        body.extend_from_slice(&metadata.epoch.to_be_bytes());
        body.extend_from_slice(&metadata.frame_id.to_be_bytes());
        body.extend_from_slice(&descriptor.total_size.to_be_bytes());
        body.extend_from_slice(&index.to_be_bytes());
        body.extend_from_slice(&descriptor.fragment_count.to_be_bytes());
        body.extend_from_slice(payload);
        self.seal(1, &body)
    }
    pub fn seal_report(&mut self, report: &ReceiveReport) -> Result<SealedDatagram, MediaError> {
        report.validate()?;
        let mut body = Vec::with_capacity(13 + report.ranges.len() * 16);
        body.extend_from_slice(&report.largest.to_be_bytes());
        body.extend_from_slice(&report.ack_delay_us.to_be_bytes());
        body.push(report.ranges.len() as u8);
        for range in &report.ranges {
            body.extend_from_slice(&range.first.to_be_bytes());
            body.extend_from_slice(&range.last.to_be_bytes());
        }
        self.seal(5, &body)
    }
}

struct ReplayWindow {
    highest: Option<u64>,
    epochs: [u64; REPLAY_BUCKETS],
    bits: [u64; REPLAY_BUCKETS],
}
impl Default for ReplayWindow {
    fn default() -> Self {
        Self {
            highest: None,
            epochs: [u64::MAX; REPLAY_BUCKETS],
            bits: [0; REPLAY_BUCKETS],
        }
    }
}
impl ReplayWindow {
    fn check(&self, sequence: u64) -> Result<(), MediaError> {
        if self
            .highest
            .is_some_and(|highest| highest.saturating_sub(sequence) >= REPLAY_WINDOW)
        {
            return Err(MediaError::Replay);
        }
        let epoch = sequence / 64;
        let slot = epoch as usize % REPLAY_BUCKETS;
        if self.epochs[slot] == epoch && self.bits[slot] & (1 << (sequence % 64)) != 0 {
            return Err(MediaError::Replay);
        }
        Ok(())
    }
    fn commit(&mut self, sequence: u64) {
        self.highest = Some(
            self.highest
                .map_or(sequence, |highest| highest.max(sequence)),
        );
        let epoch = sequence / 64;
        // One extra bucket preserves both partial ends of the sliding window.
        let slot = epoch as usize % REPLAY_BUCKETS;
        if self.epochs[slot] != epoch {
            self.epochs[slot] = epoch;
            self.bits[slot] = 0;
        }
        self.bits[slot] |= 1 << (sequence % 64);
    }
}

pub struct DatagramReceiver {
    sid: SessionId,
    key: aead::Key,
    replay: ReplayWindow,
    ranges: Vec<AckRange>,
    largest_at: Option<Instant>,
    last_report: Option<Instant>,
    unreported: usize,
}
impl DatagramReceiver {
    pub fn new(seed: &[u8; 32], sid: SessionId, direction: Direction) -> Result<Self, MediaError> {
        Ok(Self {
            sid,
            key: key(seed, sid, direction)?,
            replay: ReplayWindow::default(),
            ranges: Vec::new(),
            largest_at: None,
            last_report: None,
            unreported: 0,
        })
    }
    pub fn open(&mut self, bytes: &[u8], now: Instant) -> Result<AuthenticatedPacket, MediaError> {
        if peek_session_id(bytes)? != self.sid {
            return Err(MediaError::WrongSession);
        }
        let sequence = u64::from_be_bytes(
            bytes[22..30]
                .try_into()
                .map_err(|_| MediaError::InvalidEnvelope)?,
        );
        if sequence == u64::MAX {
            return Err(MediaError::SequenceExhausted);
        }
        self.replay.check(sequence)?;
        let body = aead::open(
            &bytes[PREFIX..],
            Some(&bytes[..PREFIX]),
            &nonce(self.sid, sequence),
            &self.key,
        )
        .map_err(|_| MediaError::Crypto)?;
        let packet = match bytes[5] {
            1 => AuthenticatedPacket::Fragment(decode_fragment(body)?),
            kind @ 2..=4 if body.len() == 16 => {
                let mut token = [0; 16];
                token.copy_from_slice(&body);
                AuthenticatedPacket::Probe {
                    kind: match kind {
                        2 => ProbeKind::Probe,
                        3 => ProbeKind::Challenge,
                        _ => ProbeKind::Response,
                    },
                    token,
                }
            }
            5 => AuthenticatedPacket::Report(decode_report(&body)?),
            _ => return Err(MediaError::InvalidEnvelope),
        };
        self.replay.commit(sequence);
        if matches!(packet, AuthenticatedPacket::Fragment(_)) {
            self.received(sequence, now);
        }
        Ok(packet)
    }
    fn received(&mut self, sequence: u64, now: Instant) {
        if self
            .ranges
            .first()
            .is_none_or(|range| sequence > range.last)
        {
            self.largest_at = Some(now);
        }
        self.ranges.push(AckRange {
            first: sequence,
            last: sequence,
        });
        self.ranges
            .sort_unstable_by_key(|range| std::cmp::Reverse(range.last));
        let mut merged: Vec<AckRange> = Vec::with_capacity(MAX_ACK_RANGES + 1);
        for range in self.ranges.drain(..) {
            if let Some(previous) = merged.last_mut() {
                if range.last.saturating_add(1) >= previous.first {
                    previous.first = previous.first.min(range.first);
                    continue;
                }
            }
            merged.push(range);
        }
        merged.truncate(MAX_ACK_RANGES);
        self.ranges = merged;
        self.unreported = self.unreported.saturating_add(1);
    }
    pub fn take_report(&mut self, now: Instant) -> Option<ReceiveReport> {
        let largest_at = self.largest_at?;
        if self.unreported == 0
            || (self.unreported < 32
                && now.saturating_duration_since(self.last_report.unwrap_or(largest_at))
                    < REPORT_INTERVAL)
        {
            return None;
        }
        self.last_report = Some(now);
        self.unreported = 0;
        Some(ReceiveReport {
            largest: self.ranges[0].last,
            ack_delay_us: now
                .saturating_duration_since(largest_at)
                .as_micros()
                .min(250_000) as u32,
            ranges: self.ranges.clone(),
        })
    }
}

fn decode_fragment(body: Vec<u8>) -> Result<Fragment, MediaError> {
    if body.len() <= FRAGMENT_HEADER || body[1] > 1 {
        return Err(MediaError::InvalidEnvelope);
    }
    let u32_at = |start| {
        u32::from_be_bytes(
            body[start..start + 4]
                .try_into()
                .expect("invariant: checked fragment header"),
        )
    };
    let u64_at = |start| {
        u64::from_be_bytes(
            body[start..start + 8]
                .try_into()
                .expect("invariant: checked fragment header"),
        )
    };
    let descriptor = FrameDescriptor {
        metadata: FrameMetadata {
            kind: match body[0] {
                1 => MediaKind::Video,
                2 => MediaKind::Audio,
                _ => return Err(MediaError::InvalidEnvelope),
            },
            keyframe: body[1] == 1,
            display_id: u32_at(2),
            epoch: u64_at(6),
            frame_id: u64_at(14),
        },
        total_size: u32_at(22),
        fragment_count: u16::from_be_bytes([body[28], body[29]]),
    };
    descriptor.validate()?;
    let index = u16::from_be_bytes([body[26], body[27]]);
    if index >= descriptor.fragment_count
        || body.len() - FRAGMENT_HEADER > descriptor.total_size as usize
    {
        return Err(MediaError::InvalidEnvelope);
    }
    Ok(Fragment {
        descriptor,
        index,
        payload: Bytes::from(body).slice(FRAGMENT_HEADER..),
    })
}
fn decode_report(body: &[u8]) -> Result<ReceiveReport, MediaError> {
    if body.len() < 13 {
        return Err(MediaError::InvalidFeedback);
    }
    let count = body[12] as usize;
    if count > MAX_ACK_RANGES || body.len() != 13 + count * 16 {
        return Err(MediaError::InvalidFeedback);
    }
    let mut ranges = Vec::with_capacity(count);
    for chunk in body[13..].as_chunks::<16>().0 {
        ranges.push(AckRange {
            first: u64::from_be_bytes(chunk[..8].try_into().expect("invariant: fixed ACK chunk")),
            last: u64::from_be_bytes(chunk[8..].try_into().expect("invariant: fixed ACK chunk")),
        });
    }
    let report = ReceiveReport {
        largest: u64::from_be_bytes(
            body[..8]
                .try_into()
                .expect("invariant: checked report header"),
        ),
        ack_delay_us: u32::from_be_bytes(
            body[8..12]
                .try_into()
                .expect("invariant: checked report header"),
        ),
        ranges,
    };
    report.validate()?;
    Ok(report)
}

type StreamKey = (MediaKind, u32);
type FrameKey = (MediaKind, u32, u64, u64);
fn stream_key(metadata: FrameMetadata) -> StreamKey {
    (metadata.kind, metadata.display_id)
}
fn frame_key(metadata: FrameMetadata) -> FrameKey {
    (
        metadata.kind,
        metadata.display_id,
        metadata.epoch,
        metadata.frame_id,
    )
}

/// Two transfer durations plus recovery time, bounded independently of network claims.
pub fn keyframe_deadline(size: usize, effective_bps: u64) -> Duration {
    let millis =
        (size as u128 * 8 * 2000 / u128::from(effective_bps.max(1)) + 2000).clamp(10_000, 120_000);
    Duration::from_millis(millis as u64)
}

/// Reserve repair/audio headroom inside the user's unchanged wire ceiling.
pub fn video_payload_bitrate_limit_kbps(wire_limit_kbps: u32) -> Result<u32, MediaError> {
    crate::session_protocol::validate_bitrate_limit(wire_limit_kbps)
        .map_err(|_| MediaError::InvalidFeedback)?;
    Ok(
        (u64::from(wire_limit_kbps) * (DEFAULT_MTU - FRAGMENT_OVERHEAD) as u64 * 90
            / (DEFAULT_MTU as u64 * 100)) as u32,
    )
}

/// A codec drain can contain old deltas followed by an independently decodable IDR.
/// Keep that boundary first so the authenticated key flag and repair cache are truthful.
/// The original message is borrowed unchanged unless a delta prefix actually exists.
pub fn video_from_keyframe_boundary(
    message: &crate::message_proto::Message,
) -> Option<crate::message_proto::Message> {
    use crate::message_proto::{message, video_frame};
    let Some(message::Union::VideoFrame(video)) = &message.union else {
        return None;
    };
    let frames = match &video.union {
        Some(
            video_frame::Union::Vp8s(frames)
            | video_frame::Union::Vp9s(frames)
            | video_frame::Union::H264s(frames)
            | video_frame::Union::H265s(frames)
            | video_frame::Union::Av1s(frames),
        ) => frames,
        _ => return None,
    };
    let prefix = frames.frames.iter().position(|frame| frame.key)?;
    if prefix == 0 {
        return None;
    }
    let mut message = message.clone();
    let Some(message::Union::VideoFrame(video)) = &mut message.union else {
        unreachable!();
    };
    let frames = match &mut video.union {
        Some(
            video_frame::Union::Vp8s(frames)
            | video_frame::Union::Vp9s(frames)
            | video_frame::Union::H264s(frames)
            | video_frame::Union::H265s(frames)
            | video_frame::Union::Av1s(frames),
        ) => frames,
        _ => unreachable!(),
    };
    frames.frames.drain(..prefix);
    Some(message)
}

const VIDEO_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const KEYFRAME_STALL_TIMEOUT: Duration = Duration::from_secs(1);

struct PendingRecoveryKeyframe {
    descriptor: FrameDescriptor,
    started: Instant,
    last_progress: Instant,
    received_bytes: usize,
}
struct RecoveryStream {
    epoch: u64,
    last_refresh: Instant,
    wanted: Option<FrameMetadata>,
    pending: Option<PendingRecoveryKeyframe>,
    delivered_frame: Option<u64>,
    retired_keyframe: Option<u64>,
}
#[derive(Default)]
pub struct RecoveryTick {
    pub abandon: Vec<FrameMetadata>,
    pub refresh: Vec<FrameMetadata>,
}

/// One gate shared by reassembly and application delivery; no lock is held across I/O.
/// A receiving/repairing keyframe suppresses duplicate refresh requests, but only real
/// new payload bytes extend its one-second stall guard. The total transfer also has
/// a separate size/rate deadline, so duplicate fragments cannot hold a stream forever.
pub struct VideoRecovery {
    streams: BTreeMap<u32, RecoveryStream>,
    effective_bps: u64,
}
impl Default for VideoRecovery {
    fn default() -> Self {
        Self {
            streams: BTreeMap::new(),
            effective_bps: 1_000_000_000,
        }
    }
}
impl VideoRecovery {
    pub fn set_effective_rate(&mut self, bps: u64) -> Result<(), MediaError> {
        if bps == 0 || bps > 1_000_000_000 {
            return Err(MediaError::InvalidFeedback);
        }
        self.effective_bps = bps;
        Ok(())
    }
    pub fn set_epoch(&mut self, display: u32, epoch: u64, now: Instant) -> Result<(), MediaError> {
        if self
            .streams
            .get(&display)
            .is_some_and(|stream| stream.epoch >= epoch)
        {
            return Ok(());
        }
        if !self.streams.contains_key(&display) && self.streams.len() >= 64 {
            return Err(MediaError::Capacity);
        }
        self.streams.insert(
            display,
            RecoveryStream {
                epoch,
                last_refresh: now,
                pending: None,
                delivered_frame: None,
                retired_keyframe: None,
                // Also recover a wholly lost first/static frame when no UDP event follows.
                wanted: Some(FrameMetadata {
                    kind: MediaKind::Video,
                    display_id: display,
                    epoch,
                    frame_id: 0,
                    keyframe: false,
                }),
            },
        );
        Ok(())
    }
    /// Return the superseded/rejected assembly that the caller can release locally.
    pub fn keyframe_progress(
        &mut self,
        descriptor: FrameDescriptor,
        received_bytes: usize,
        now: Instant,
    ) -> Result<Option<FrameMetadata>, MediaError> {
        descriptor.validate()?;
        let metadata = descriptor.metadata;
        if metadata.kind != MediaKind::Video
            || !metadata.keyframe
            || received_bytes > descriptor.total_size as usize
        {
            return Err(MediaError::InvalidEnvelope);
        }
        let Some(stream) = self.streams.get_mut(&metadata.display_id) else {
            return Ok(None);
        };
        if stream.epoch != metadata.epoch
            || stream
                .retired_keyframe
                .is_some_and(|last| metadata.frame_id <= last)
            || stream
                .delivered_frame
                .is_some_and(|last| metadata.frame_id <= last)
            || stream
                .pending
                .as_ref()
                .is_some_and(|pending| pending.descriptor.metadata.frame_id > metadata.frame_id)
        {
            return Ok(Some(metadata));
        }
        if let Some(pending) = stream
            .pending
            .as_mut()
            .filter(|pending| pending.descriptor.metadata == metadata)
        {
            if pending.descriptor != descriptor {
                return Err(MediaError::ConflictingFragment);
            }
            if received_bytes > pending.received_bytes {
                pending.received_bytes = received_bytes;
                pending.last_progress = now;
            }
            return Ok(None);
        }
        let replaced = stream
            .pending
            .take()
            .map(|pending| pending.descriptor.metadata);
        stream.pending = Some(PendingRecoveryKeyframe {
            descriptor,
            started: now,
            last_progress: now,
            received_bytes,
        });
        Ok(replaced)
    }
    pub fn keyframe_abandoned(&mut self, metadata: FrameMetadata) {
        if metadata.kind != MediaKind::Video || !metadata.keyframe {
            return;
        }
        if let Some(stream) = self
            .streams
            .get_mut(&metadata.display_id)
            .filter(|stream| stream.epoch == metadata.epoch)
        {
            stream.retired_keyframe =
                Some(stream.retired_keyframe.unwrap_or(0).max(metadata.frame_id));
            if stream
                .pending
                .as_ref()
                .is_some_and(|pending| pending.descriptor.metadata == metadata)
            {
                stream.pending = None;
            }
        }
    }
    pub fn frame_delivered(&mut self, metadata: FrameMetadata) {
        if metadata.kind != MediaKind::Video {
            return;
        }
        let Some(stream) = self
            .streams
            .get_mut(&metadata.display_id)
            .filter(|stream| stream.epoch == metadata.epoch)
        else {
            return;
        };
        stream.delivered_frame = Some(stream.delivered_frame.unwrap_or(0).max(metadata.frame_id));
        if stream
            .pending
            .as_ref()
            .is_some_and(|pending| pending.descriptor.metadata.frame_id <= metadata.frame_id)
        {
            stream.pending = None;
        }
        if stream
            .wanted
            .is_some_and(|wanted| wanted.frame_id <= metadata.frame_id)
        {
            stream.wanted = None;
        }
    }
    pub fn request(
        &mut self,
        metadata: FrameMetadata,
        failed_keyframe: bool,
        now: Instant,
    ) -> bool {
        if metadata.kind != MediaKind::Video {
            return false;
        }
        let Some(stream) = self
            .streams
            .get_mut(&metadata.display_id)
            .filter(|stream| stream.epoch == metadata.epoch)
        else {
            return false;
        };
        if stream
            .delivered_frame
            .is_some_and(|last| metadata.frame_id <= last)
        {
            return false;
        }
        if stream
            .wanted
            .is_none_or(|wanted| wanted.frame_id < metadata.frame_id)
        {
            stream.wanted = Some(metadata);
        }
        if failed_keyframe {
            stream.retired_keyframe =
                Some(stream.retired_keyframe.unwrap_or(0).max(metadata.frame_id));
            if stream
                .pending
                .as_ref()
                .is_some_and(|pending| pending.descriptor.metadata == metadata)
            {
                stream.pending = None;
            }
        }
        if stream.pending.is_some()
            || now.duration_since(stream.last_refresh) < VIDEO_REFRESH_INTERVAL
        {
            return false;
        }
        stream.last_refresh = now;
        true
    }
    pub fn tick(&mut self, now: Instant) -> RecoveryTick {
        let mut tick = RecoveryTick::default();
        let effective_bps = self.effective_bps;
        for stream in self.streams.values_mut() {
            let expired = stream.pending.as_ref().is_some_and(|pending| {
                now.duration_since(pending.last_progress) >= KEYFRAME_STALL_TIMEOUT
                    || now.duration_since(pending.started)
                        >= keyframe_deadline(pending.descriptor.total_size as usize, effective_bps)
            });
            if expired {
                let metadata = stream.pending.take().unwrap().descriptor.metadata;
                stream.retired_keyframe =
                    Some(stream.retired_keyframe.unwrap_or(0).max(metadata.frame_id));
                tick.abandon.push(metadata);
                if stream
                    .wanted
                    .is_none_or(|wanted| wanted.frame_id < metadata.frame_id)
                {
                    stream.wanted = Some(metadata);
                }
            }
            if stream.pending.is_none()
                && now.duration_since(stream.last_refresh) >= VIDEO_REFRESH_INTERVAL
            {
                if let Some(metadata) = stream.wanted {
                    stream.last_refresh = now;
                    tick.refresh.push(metadata);
                }
            }
        }
        tick
    }
}

#[derive(Debug)]
pub struct CompletedFrame {
    pub metadata: FrameMetadata,
    pub payload: Bytes,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovery {
    RefreshVideo(FrameMetadata),
    KeyframeNack(KeyframeNack),
    KeyframeComplete(KeyframeComplete),
}
#[derive(Debug, Default)]
pub struct ReceiveOutcome {
    pub frame: Option<CompletedFrame>,
    pub recovery: Vec<Recovery>,
}

struct PendingFrame {
    descriptor: FrameDescriptor,
    deadline: Instant,
    fragments: Vec<Option<Bytes>>,
    received: usize,
    remaining: usize,
    sent_complete: bool,
    next_nack: Instant,
    nack_rounds: u8,
}

pub struct Reassembler {
    epochs: BTreeMap<StreamKey, u64>,
    pending: BTreeMap<FrameKey, PendingFrame>,
    notices: BTreeMap<StreamKey, (FrameDescriptor, Instant)>,
    completed: VecDeque<FrameKey>,
    audio_last: Option<(u64, u64)>,
    effective_bps: u64,
}
impl Default for Reassembler {
    fn default() -> Self {
        Self {
            epochs: BTreeMap::new(),
            pending: BTreeMap::new(),
            notices: BTreeMap::new(),
            completed: VecDeque::new(),
            audio_last: None,
            effective_bps: 1_000_000,
        }
    }
}
impl Reassembler {
    pub fn received_bytes(&self, metadata: FrameMetadata) -> Option<usize> {
        self.pending
            .get(&frame_key(metadata))
            .map(|frame| frame.received)
    }
    /// Release a stalled/superseded assembly without changing its ordered TCP epoch.
    pub fn discard(&mut self, metadata: FrameMetadata) {
        self.pending.remove(&frame_key(metadata));
        if self
            .notices
            .get(&stream_key(metadata))
            .is_some_and(|(notice, _)| notice.metadata == metadata)
        {
            self.notices.remove(&stream_key(metadata));
        }
    }
    pub fn set_effective_rate(&mut self, bits_per_second: u64) -> Result<(), MediaError> {
        if bits_per_second == 0 {
            return Err(MediaError::InvalidFeedback);
        }
        self.effective_bps = bits_per_second;
        Ok(())
    }
    pub fn set_epoch(
        &mut self,
        kind: MediaKind,
        display: u32,
        epoch: u64,
    ) -> Result<(), MediaError> {
        let key = (kind, display);
        if (kind == MediaKind::Audio && display != 0)
            || (!self.epochs.contains_key(&key) && self.epochs.len() >= 65)
        {
            return Err(MediaError::Capacity);
        }
        if self.epochs.get(&key) == Some(&epoch) {
            return Ok(());
        }
        if kind == MediaKind::Audio {
            self.audio_last = None;
        }
        self.pending
            .retain(|key, _| (key.0, key.1) != (kind, display));
        self.completed
            .retain(|key| (key.0, key.1) != (kind, display));
        self.notices.remove(&key);
        self.epochs.insert(key, epoch);
        Ok(())
    }
    fn epoch(&mut self, metadata: FrameMetadata) -> Result<(), MediaError> {
        let stream = stream_key(metadata);
        if !self.epochs.contains_key(&stream) {
            if metadata.epoch != 0 {
                return Err(MediaError::WrongEpoch);
            }
            self.set_epoch(metadata.kind, metadata.display_id, 0)?;
        }
        if self.epochs.get(&stream) != Some(&metadata.epoch) {
            return Err(MediaError::WrongEpoch);
        }
        Ok(())
    }
    fn insert(&mut self, descriptor: FrameDescriptor, now: Instant) -> Result<(), MediaError> {
        descriptor.validate()?;
        self.epoch(descriptor.metadata)?;
        let key = frame_key(descriptor.metadata);
        if self.pending.contains_key(&key) {
            return Ok(());
        }
        let kind = descriptor.metadata.kind;
        let (count, reserved) = self
            .pending
            .values()
            .filter(|frame| frame.descriptor.metadata.kind == kind)
            .fold((0, 0usize), |(count, reserved), frame| {
                (count + 1, reserved + frame.descriptor.total_size as usize)
            });
        let (max_count, max_bytes) = match kind {
            MediaKind::Video => (8, 16 * 1024 * 1024),
            MediaKind::Audio => (4, 256 * 1024),
        };
        if count >= max_count || reserved + descriptor.total_size as usize > max_bytes {
            return Err(MediaError::Capacity);
        }
        let ttl = match (kind, descriptor.metadata.keyframe) {
            (MediaKind::Audio, _) => AUDIO_TTL,
            // The reliable Sent notice starts the rate-based recovery deadline.
            // An unfinished initial transfer must not inherit a short guessed rate.
            (_, true) => Duration::from_secs(120),
            _ => DELTA_TTL,
        };
        self.pending.insert(
            key,
            PendingFrame {
                descriptor,
                deadline: now + ttl,
                fragments: vec![None; descriptor.fragment_count as usize],
                received: 0,
                remaining: descriptor.fragment_count as usize,
                sent_complete: false,
                next_nack: now + REPORT_INTERVAL,
                nack_rounds: 0,
            },
        );
        Ok(())
    }
    pub fn note_notice(
        &mut self,
        notice: FrameNotice,
        now: Instant,
    ) -> Result<ReceiveOutcome, MediaError> {
        let mut outcome = ReceiveOutcome::default();
        match notice {
            FrameNotice::Abandoned(metadata) => {
                self.epoch(metadata)?;
                self.pending.remove(&frame_key(metadata));
                if self
                    .notices
                    .get(&stream_key(metadata))
                    .is_some_and(|(notice, _)| notice.metadata == metadata)
                {
                    self.notices.remove(&stream_key(metadata));
                }
                if metadata.kind == MediaKind::Video
                    && !self.completed.contains(&frame_key(metadata))
                {
                    outcome.recovery.push(Recovery::RefreshVideo(metadata));
                }
            }
            FrameNotice::Sent(descriptor) => {
                descriptor.validate()?;
                self.epoch(descriptor.metadata)?;
                let metadata = descriptor.metadata;
                let key = frame_key(metadata);
                if self.completed.contains(&key) {
                    if metadata.keyframe {
                        outcome
                            .recovery
                            .push(Recovery::KeyframeComplete(KeyframeComplete { metadata }));
                    }
                    return Ok(outcome);
                }
                if metadata.kind == MediaKind::Audio {
                    return Ok(outcome);
                }
                if metadata.keyframe {
                    self.insert(descriptor, now)?;
                    let frame = self
                        .pending
                        .get_mut(&key)
                        .expect("invariant: inserted keyframe");
                    if frame.descriptor != descriptor {
                        return Err(MediaError::ConflictingFragment);
                    }
                    // Start recovery after initial transmission, not while future fragments are unsent.
                    if !frame.sent_complete {
                        frame.sent_complete = true;
                        frame.next_nack = now + REPORT_INTERVAL;
                        frame.deadline = now
                            + keyframe_deadline(descriptor.total_size as usize, self.effective_bps);
                    }
                } else {
                    let stream = stream_key(metadata);
                    if self
                        .notices
                        .get(&stream)
                        .is_none_or(|(previous, _)| previous.metadata.frame_id < metadata.frame_id)
                    {
                        self.notices.insert(stream, (descriptor, now + DELTA_TTL));
                    }
                }
            }
        }
        Ok(outcome)
    }
    pub fn push(&mut self, fragment: Fragment, now: Instant) -> Result<ReceiveOutcome, MediaError> {
        let mut outcome = ReceiveOutcome::default();
        let descriptor = fragment.descriptor;
        descriptor.validate()?;
        self.epoch(descriptor.metadata)?;
        if fragment.index >= descriptor.fragment_count
            || fragment.payload.is_empty()
            || fragment.payload.len() > descriptor.total_size as usize
        {
            return Err(MediaError::InvalidEnvelope);
        }
        let key = frame_key(descriptor.metadata);
        if self.completed.contains(&key) {
            return Ok(outcome);
        }
        if descriptor.metadata.kind == MediaKind::Audio
            && self.audio_last.is_some_and(|(epoch, last)| {
                epoch == descriptor.metadata.epoch && descriptor.metadata.frame_id <= last
            })
        {
            return Ok(outcome);
        }
        self.insert(descriptor, now)?;
        let frame = self
            .pending
            .get_mut(&key)
            .expect("invariant: inserted fragment assembly");
        if frame.descriptor != descriptor {
            return Err(MediaError::ConflictingFragment);
        }
        let slot = &mut frame.fragments[fragment.index as usize];
        if let Some(previous) = slot {
            if previous != &fragment.payload {
                return Err(MediaError::ConflictingFragment);
            }
            return Ok(outcome);
        }
        if frame.received + fragment.payload.len() > descriptor.total_size as usize {
            return Err(MediaError::InvalidEnvelope);
        }
        frame.received += fragment.payload.len();
        frame.remaining -= 1;
        *slot = Some(fragment.payload);
        if frame.remaining != 0 {
            return Ok(outcome);
        }
        let frame = self
            .pending
            .remove(&key)
            .expect("invariant: complete assembly exists");
        if frame.received != descriptor.total_size as usize {
            return Err(MediaError::InvalidEnvelope);
        }
        let mut payload = Vec::with_capacity(frame.received);
        for fragment in frame.fragments.into_iter().flatten() {
            payload.extend_from_slice(&fragment);
        }
        if self.completed.len() == 128 {
            self.completed.pop_front();
        }
        self.completed.push_back(key);
        if descriptor.metadata.kind == MediaKind::Audio {
            self.audio_last = Some((descriptor.metadata.epoch, descriptor.metadata.frame_id));
            self.pending.retain(|_, pending| {
                pending.descriptor.metadata.kind != MediaKind::Audio
                    || pending.descriptor.metadata.frame_id > descriptor.metadata.frame_id
            });
        }
        if self
            .notices
            .get(&stream_key(descriptor.metadata))
            .is_some_and(|(notice, _)| notice.metadata == descriptor.metadata)
        {
            self.notices.remove(&stream_key(descriptor.metadata));
        }
        if descriptor.metadata.keyframe {
            outcome
                .recovery
                .push(Recovery::KeyframeComplete(KeyframeComplete {
                    metadata: descriptor.metadata,
                }));
        }
        outcome.frame = Some(CompletedFrame {
            metadata: descriptor.metadata,
            payload: payload.into(),
        });
        Ok(outcome)
    }
    /// Poll periodically even without packets; this also detects a wholly lost last frame.
    pub fn expire(&mut self, now: Instant) -> ReceiveOutcome {
        let mut outcome = ReceiveOutcome::default();
        self.notices.retain(|_, (descriptor, deadline)| {
            if now < *deadline {
                return true;
            }
            outcome
                .recovery
                .push(Recovery::RefreshVideo(descriptor.metadata));
            false
        });
        self.pending.retain(|_, frame| {
            if now >= frame.deadline {
                if frame.descriptor.metadata.kind == MediaKind::Video {
                    outcome
                        .recovery
                        .push(Recovery::RefreshVideo(frame.descriptor.metadata));
                }
                return false;
            }
            if frame.descriptor.metadata.keyframe
                && frame.sent_complete
                && frame.nack_rounds < 3
                && now >= frame.next_nack
            {
                let mut missing = vec![0; frame.fragments.len().div_ceil(8)];
                for (index, fragment) in frame.fragments.iter().enumerate() {
                    if fragment.is_none() {
                        missing[index / 8] |= 1 << (index % 8);
                    }
                }
                frame.nack_rounds += 1;
                frame.next_nack = now + Duration::from_millis(250);
                outcome.recovery.push(Recovery::KeyframeNack(KeyframeNack {
                    descriptor: frame.descriptor,
                    missing,
                }));
            }
            true
        });
        outcome
    }
}

struct CachedKeyframe {
    plan: FramePlan,
    inserted: Instant,
    deadline: Instant,
    retries: Vec<u8>,
    initial_sent: bool,
}
#[derive(Default)]
pub struct KeyframeCache {
    frames: BTreeMap<FrameKey, CachedKeyframe>,
    bytes: usize,
}
pub struct RetransmissionPlan {
    pub frame: FramePlan,
    pub indices: Vec<u16>,
}
impl KeyframeCache {
    pub fn insert(
        &mut self,
        plan: FramePlan,
        now: Instant,
    ) -> Result<Vec<FrameMetadata>, MediaError> {
        if !plan.descriptor.metadata.keyframe {
            return Err(MediaError::InvalidEnvelope);
        }
        let key = frame_key(plan.descriptor.metadata);
        if self.frames.contains_key(&key) {
            return Err(MediaError::ConflictingFragment);
        }
        let mut evicted = self.expire(now);
        while self.frames.len() >= 2 || self.bytes + plan.payload.len() > 16 * 1024 * 1024 {
            let oldest = self
                .frames
                .iter()
                .min_by_key(|(_, frame)| frame.inserted)
                .map(|(key, _)| *key)
                .expect("invariant: capacity pressure has a cached frame");
            let frame = self
                .frames
                .remove(&oldest)
                .expect("invariant: selected cached frame exists");
            self.bytes -= frame.plan.payload.len();
            evicted.push(frame.plan.descriptor.metadata);
        }
        self.bytes += plan.payload.len();
        let frame = CachedKeyframe {
            deadline: now + Duration::from_secs(120),
            retries: vec![0; plan.fragment_count() as usize],
            plan,
            inserted: now,
            initial_sent: false,
        };
        self.frames.insert(key, frame);
        Ok(evicted)
    }
    pub fn initial_sent(
        &mut self,
        metadata: FrameMetadata,
        now: Instant,
        effective_bps: u64,
    ) -> Result<(), MediaError> {
        let frame = self
            .frames
            .get_mut(&frame_key(metadata))
            .ok_or(MediaError::Expired)?;
        if !frame.initial_sent {
            frame.initial_sent = true;
            frame.deadline = now + keyframe_deadline(frame.plan.payload.len(), effective_bps);
        }
        Ok(())
    }
    pub fn retransmit(
        &mut self,
        nack: &KeyframeNack,
        now: Instant,
    ) -> Result<RetransmissionPlan, MediaError> {
        nack.validate()?;
        let frame = self
            .frames
            .get_mut(&frame_key(nack.descriptor.metadata))
            .ok_or(MediaError::Expired)?;
        if frame.plan.descriptor != nack.descriptor {
            return Err(MediaError::InvalidFeedback);
        }
        if now >= frame.deadline || !frame.initial_sent {
            return Err(MediaError::Expired);
        }
        let mut indices = Vec::new();
        for (index, retries) in frame.retries.iter_mut().enumerate() {
            if nack.missing[index / 8] & (1 << (index % 8)) != 0 && *retries < 2 {
                *retries += 1;
                indices.push(index as u16);
            }
        }
        Ok(RetransmissionPlan {
            frame: frame.plan.clone(),
            indices,
        })
    }
    pub fn complete(&mut self, complete: KeyframeComplete) {
        if !complete.metadata.keyframe {
            return;
        }
        if let Some(frame) = self.frames.remove(&frame_key(complete.metadata)) {
            self.bytes -= frame.plan.payload.len();
        }
    }
    pub fn expire(&mut self, now: Instant) -> Vec<FrameMetadata> {
        let mut expired = Vec::new();
        let bytes = &mut self.bytes;
        self.frames.retain(|_, frame| {
            if now < frame.deadline {
                return true;
            }
            *bytes -= frame.plan.payload.len();
            expired.push(frame.plan.descriptor.metadata);
            false
        });
        expired
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendDecision {
    Send,
    Wait(Duration),
    Expired,
    FeedbackRequired,
}
struct Flight {
    sent: Instant,
    bytes: usize,
}

/// Metadata-only Reno window. ACK reports are authenticated UDP packets, not ACK-of-ACKs.
pub struct Pacer {
    mtu: usize,
    limit_bps: u64,
    window: usize,
    threshold: usize,
    inflight_bytes: usize,
    flights: BTreeMap<u64, Flight>,
    highest_sent: Option<u64>,
    srtt: Duration,
    last_feedback: Instant,
    last_activity: Instant,
    last_loss: Option<Instant>,
    tokens: f64,
    refilled: Instant,
}
impl Pacer {
    pub fn new(mtu: usize, limit_bps: u64, now: Instant) -> Result<Self, MediaError> {
        if !(FRAGMENT_OVERHEAD + 1..=MAX_DATAGRAM_SIZE).contains(&mtu)
            || !(1..=1_000_000_000).contains(&limit_bps)
        {
            return Err(MediaError::InvalidFeedback);
        }
        Ok(Self {
            mtu,
            limit_bps,
            window: 10 * mtu,
            threshold: 64 * 1024 * 1024,
            inflight_bytes: 0,
            flights: BTreeMap::new(),
            highest_sent: None,
            srtt: Duration::from_millis(100),
            last_feedback: now,
            last_activity: now,
            last_loss: None,
            tokens: (10 * mtu) as f64,
            refilled: now,
        })
    }
    pub fn set_bitrate_limit(&mut self, bits_per_second: u64) -> Result<(), MediaError> {
        if !(1..=1_000_000_000).contains(&bits_per_second) {
            return Err(MediaError::InvalidFeedback);
        }
        self.limit_bps = bits_per_second;
        Ok(())
    }
    pub fn effective_bitrate(&self) -> u64 {
        let window = self.window.saturating_sub(2 * self.mtu).max(2 * self.mtu);
        ((window as f64 * 8.0 / self.srtt.as_secs_f64().max(0.000_001)).min(self.limit_bps as f64))
            as u64
    }
    fn feedback_timeout(&self) -> Duration {
        (self.srtt * 4).clamp(Duration::from_millis(500), Duration::from_secs(5))
    }
    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.effective_bitrate() as f64 / 8.0)
            .min(self.window.min(16 * self.mtu) as f64);
        self.refilled = now;
    }
    /// A waiting reliable TCP writer has precedence; writing into its local buffer is not an ACK.
    pub fn decide(
        &mut self,
        bytes: usize,
        deadline: Instant,
        now: Instant,
        reliable_pending: bool,
    ) -> SendDecision {
        if now >= deadline {
            return SendDecision::Expired;
        }
        if bytes == 0 || bytes > self.mtu {
            return SendDecision::Expired;
        }
        if self.flights.is_empty()
            && now.saturating_duration_since(self.last_activity) >= self.feedback_timeout()
        {
            self.window = self.window.min(10 * self.mtu);
            self.last_activity = now;
            self.last_feedback = now;
        }
        self.refill(now);
        if now.saturating_duration_since(self.last_feedback) >= self.feedback_timeout() {
            return SendDecision::FeedbackRequired;
        }
        if reliable_pending {
            return SendDecision::Wait(Duration::from_millis(1));
        }
        let usable = self.window.saturating_sub(2 * self.mtu).max(2 * self.mtu);
        if self.flights.len() >= 65_536 || self.inflight_bytes + bytes > usable {
            return SendDecision::Wait(self.srtt.min(REPORT_INTERVAL));
        }
        if self.tokens < bytes as f64 {
            return SendDecision::Wait(Duration::from_secs_f64(
                ((bytes as f64 - self.tokens) * 8.0 / self.effective_bitrate().max(1) as f64)
                    .max(0.000_001),
            ));
        }
        SendDecision::Send
    }
    /// Call only after a successful socket send of an ACK-eliciting fragment.
    pub fn on_sent(&mut self, sequence: u64, bytes: usize, now: Instant) -> Result<(), MediaError> {
        self.refill(now);
        let usable = self.window.saturating_sub(2 * self.mtu).max(2 * self.mtu);
        if bytes == 0
            || bytes > self.mtu
            || self.highest_sent.is_some_and(|highest| sequence <= highest)
            || self.flights.len() >= 65_536
            || self.inflight_bytes + bytes > usable
            || self.tokens + 0.001 < bytes as f64
        {
            return Err(MediaError::InvalidFeedback);
        }
        self.tokens = (self.tokens - bytes as f64).max(0.0);
        self.flights.insert(sequence, Flight { sent: now, bytes });
        self.highest_sent = Some(sequence);
        self.inflight_bytes += bytes;
        self.last_activity = now;
        Ok(())
    }
    pub fn on_report(&mut self, report: &ReceiveReport, now: Instant) -> Result<(), MediaError> {
        report.validate()?;
        if self
            .highest_sent
            .is_none_or(|highest| report.largest > highest)
        {
            return Err(MediaError::InvalidFeedback);
        }
        self.refill(now);
        let mut acknowledged = 0usize;
        let mut sample = None;
        for range in &report.ranges {
            for (sequence, flight) in self
                .flights
                .extract_if(range.first..=range.last, |_, _| true)
            {
                acknowledged += flight.bytes;
                self.inflight_bytes -= flight.bytes;
                if sequence == report.largest {
                    let elapsed = now.saturating_duration_since(flight.sent);
                    sample = Some(
                        elapsed
                            .saturating_sub(Duration::from_micros(u64::from(report.ack_delay_us)))
                            .max(Duration::from_micros(1)),
                    );
                }
            }
        }
        if let Some(sample) = sample {
            self.srtt = (self.srtt * 7 + sample) / 8;
        }
        self.last_feedback = now;
        let loss_age = self.feedback_timeout();
        let srtt = self.srtt;
        let mut had_loss = false;
        for (_, flight) in self.flights.extract_if(.., |&sequence, flight| {
            let age = now.saturating_duration_since(flight.sent);
            age >= loss_age
                || (sequence.saturating_add(3) <= report.largest && age >= srtt.mul_f64(1.125))
        }) {
            had_loss = true;
            self.inflight_bytes -= flight.bytes;
        }
        if had_loss
            && self
                .last_loss
                .is_none_or(|at| now.saturating_duration_since(at) >= self.srtt)
        {
            self.window = (self.window / 2).max(2 * self.mtu);
            self.threshold = self.window;
            self.last_loss = Some(now);
        } else if !had_loss && acknowledged > 0 {
            let increase = if self.window < self.threshold {
                acknowledged
            } else {
                (self.mtu * acknowledged / self.window).max(1)
            };
            self.window = (self.window + increase).min(64 * 1024 * 1024);
        }
        self.tokens = self.tokens.min(self.window.min(16 * self.mtu) as f64);
        Ok(())
    }
    /// Only a newly authenticated path probe round trip may restart a feedback-stalled path.
    pub fn on_probe_response(&mut self, rtt: Duration, now: Instant) -> Result<(), MediaError> {
        if rtt.is_zero() || rtt > Duration::from_secs(30) {
            return Err(MediaError::InvalidFeedback);
        }
        self.srtt = rtt;
        self.window = self.window.min(10 * self.mtu).max(2 * self.mtu);
        self.flights.clear();
        self.inflight_bytes = 0;
        self.last_feedback = now;
        self.last_activity = now;
        self.refilled = now;
        self.tokens = self.window.min(16 * self.mtu) as f64;
        Ok(())
    }
    pub fn inflight_bytes(&self) -> usize {
        self.inflight_bytes
    }
    pub fn congestion_window(&self) -> usize {
        self.window
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{net::UdpSocket, time::timeout};

    fn metadata(id: u64, keyframe: bool) -> FrameMetadata {
        FrameMetadata {
            kind: MediaKind::Video,
            display_id: 0,
            epoch: 0,
            frame_id: id,
            keyframe,
        }
    }
    #[test]
    fn recovery_mixed_codec_batch_starts_at_real_keyframe_and_preserves_order() {
        use crate::message_proto::{
            message, video_frame, EncodedVideoFrame, EncodedVideoFrames, Message, VideoFrame,
        };
        use protobuf::Message as _;
        for codec in 0..5 {
            let frames = EncodedVideoFrames {
                frames: [false, false, true, false, true]
                    .iter()
                    .enumerate()
                    .map(|(index, key)| EncodedVideoFrame {
                        key: *key,
                        data: Bytes::from(vec![index as u8]),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            };
            let mut video = VideoFrame::new();
            video.display = 7;
            video.union = Some(match codec {
                0 => video_frame::Union::Vp8s(frames),
                1 => video_frame::Union::Vp9s(frames),
                2 => video_frame::Union::H264s(frames),
                3 => video_frame::Union::H265s(frames),
                _ => video_frame::Union::Av1s(frames),
            });
            let mut original = Message::new();
            original.set_video_frame(video);
            let original_bytes = original.write_to_bytes().unwrap();
            let trimmed = video_from_keyframe_boundary(&original).unwrap();
            // Exercise the actual wire serializer, not only a synthetic list operation.
            let decoded = Message::parse_from_bytes(&trimmed.write_to_bytes().unwrap()).unwrap();
            let Some(message::Union::VideoFrame(video)) = decoded.union else {
                panic!("video");
            };
            assert_eq!(video.display, 7);
            let frames = match video.union.unwrap() {
                video_frame::Union::Vp8s(frames)
                | video_frame::Union::Vp9s(frames)
                | video_frame::Union::H264s(frames)
                | video_frame::Union::H265s(frames)
                | video_frame::Union::Av1s(frames) => frames,
                _ => panic!("encoded frames"),
            };
            assert_eq!(
                frames
                    .frames
                    .iter()
                    .map(|frame| (frame.key, frame.data[0]))
                    .collect::<Vec<_>>(),
                vec![(true, 2), (false, 3), (true, 4)]
            );
            assert_eq!(original.write_to_bytes().unwrap(), original_bytes);
            assert!(video_from_keyframe_boundary(&trimmed).is_none());
        }
        assert_eq!(video_payload_bitrate_limit_kbps(64).unwrap(), 53);
        assert_eq!(video_payload_bitrate_limit_kbps(999).unwrap(), 842);
        assert_eq!(
            video_payload_bitrate_limit_kbps(63),
            Err(MediaError::InvalidFeedback)
        );
        assert_eq!(video_payload_bitrate_limit_kbps(1_000).unwrap(), 843);
        assert_eq!(video_payload_bitrate_limit_kbps(8_000).unwrap(), 6_744);
        assert_eq!(
            video_payload_bitrate_limit_kbps(1_000_000).unwrap(),
            843_000
        );
        assert_eq!(
            video_payload_bitrate_limit_kbps(0),
            Err(MediaError::InvalidFeedback)
        );
        assert_eq!(
            video_payload_bitrate_limit_kbps(1_000_001),
            Err(MediaError::InvalidFeedback)
        );
    }

    #[test]
    fn recovery_shared_gate_coalesces_loss_and_retries_without_incoming_events() {
        use std::sync::{Arc, Mutex};
        let now = Instant::now();
        let gate = Arc::new(Mutex::new(VideoRecovery::default()));
        gate.lock().unwrap().set_epoch(0, 0, now).unwrap();
        // Delivery and UDP-worker loss paths race at exactly the same time.
        let results = std::thread::scope(|scope| {
            let left = gate.clone();
            let right = gate.clone();
            let left = scope.spawn(move || {
                left.lock().unwrap().request(
                    metadata(1, false),
                    false,
                    now + Duration::from_millis(250),
                )
            });
            let right = scope.spawn(move || {
                right.lock().unwrap().request(
                    metadata(2, false),
                    false,
                    now + Duration::from_millis(250),
                )
            });
            [left.join().unwrap(), right.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|requested| **requested).count(), 1);
        let descriptor = FrameDescriptor {
            metadata: metadata(3, true),
            total_size: 2_000,
            fragment_count: 2,
        };
        let mut gate = gate.lock().unwrap();
        gate.keyframe_progress(descriptor, 1_000, now + Duration::from_millis(260))
            .unwrap();
        assert!(!gate.request(metadata(4, false), false, now + Duration::from_millis(600)));
        // A duplicate fragment / repeated Sent does not extend the progress deadline.
        gate.keyframe_progress(descriptor, 1_000, now + Duration::from_millis(1_000))
            .unwrap();
        let tick = gate.tick(now + Duration::from_millis(1_260));
        assert_eq!(tick.abandon, vec![descriptor.metadata]);
        assert_eq!(tick.refresh, vec![metadata(4, false)]);
        assert!(gate
            .tick(now + Duration::from_millis(1_400))
            .refresh
            .is_empty());
        // There is no new packet, delta, notice, or application event here.
        assert_eq!(
            gate.tick(now + Duration::from_millis(1_510)).refresh,
            vec![metadata(4, false)]
        );
        let new_key = FrameDescriptor {
            metadata: metadata(5, true),
            ..descriptor
        };
        gate.keyframe_progress(new_key, 2_000, now + Duration::from_millis(1_520))
            .unwrap();
        gate.frame_delivered(new_key.metadata);
        assert!(gate.request(
            metadata(6, false),
            false,
            now + Duration::from_millis(1_800)
        ));
        // A late but now contiguous delta resolves its prior loss without needing an IDR.
        gate.frame_delivered(metadata(6, false));
        assert!(!gate.request(
            metadata(6, false),
            false,
            now + Duration::from_millis(1_900)
        ));
        assert!(gate.tick(now + Duration::from_secs(2)).refresh.is_empty());
        // An old epoch's Abandoned/refresh cannot cancel a new epoch's in-flight key.
        gate.set_epoch(0, 1, now + Duration::from_secs(2)).unwrap();
        let epoch_key = FrameDescriptor {
            metadata: FrameMetadata {
                epoch: 1,
                ..metadata(6, true)
            },
            ..descriptor
        };
        gate.keyframe_progress(epoch_key, 1_000, now + Duration::from_millis(2_010))
            .unwrap();
        gate.keyframe_abandoned(new_key.metadata);
        assert!(!gate.request(metadata(9, false), true, now + Duration::from_millis(2_400)));
        assert!(gate
            .tick(now + Duration::from_millis(2_400))
            .refresh
            .is_empty());
        // Same-epoch repeated control does not reset progress or forget a valid delivery.
        gate.set_epoch(0, 1, now + Duration::from_millis(2_450))
            .unwrap();
        // An older queued worker command cannot roll back the already-published TCP epoch.
        gate.set_epoch(0, 0, now + Duration::from_millis(2_450))
            .unwrap();
        assert!(gate
            .tick(now + Duration::from_millis(2_500))
            .refresh
            .is_empty());
        gate.frame_delivered(epoch_key.metadata);
        assert!(gate.tick(now + Duration::from_secs(4)).refresh.is_empty());
    }

    #[test]
    fn recovery_real_progress_has_a_total_budget_and_releases_only_matching_assembly() {
        let now = Instant::now();
        let mut gate = VideoRecovery::default();
        gate.set_epoch(0, 0, now).unwrap();
        let large = FrameDescriptor {
            metadata: metadata(1, true),
            total_size: MAX_VIDEO_FRAME as u32,
            fragment_count: MAX_FRAGMENTS as u16,
        };
        gate.keyframe_progress(large, 1, now).unwrap();
        for step in 1..=11 {
            let time = now + Duration::from_millis(step * 900);
            gate.keyframe_progress(large, step as usize + 1, time)
                .unwrap();
            assert!(gate.tick(time).abandon.is_empty());
        }
        // Even continual real progress cannot extend the independently bounded deadline.
        let tick = gate.tick(now + Duration::from_secs(10));
        assert_eq!(tick.abandon, vec![large.metadata]);
        assert_eq!(tick.refresh, vec![large.metadata]);
        // Preserve legitimate low-rate transfer time; idle time is still bounded by 1s.
        gate.set_effective_rate(1_000_000).unwrap();
        assert_eq!(
            gate.keyframe_progress(large, 100, now + Duration::from_millis(10_500))
                .unwrap(),
            Some(large.metadata)
        );
        let large = FrameDescriptor {
            metadata: metadata(2, true),
            ..large
        };
        gate.keyframe_progress(large, 1, now + Duration::from_secs(11))
            .unwrap();
        for step in 1..=12 {
            let time = now + Duration::from_secs(11) + Duration::from_millis(step * 900);
            gate.keyframe_progress(large, step as usize + 1, time)
                .unwrap();
            assert!(gate.tick(time).abandon.is_empty());
        }
        assert_eq!(
            gate.tick(now + Duration::from_millis(22_800)).abandon,
            vec![large.metadata]
        );
        let mut assembler = Reassembler::default();
        let plan = FramePlan::new(metadata(7, true), vec![7; 2_000].into(), DEFAULT_MTU).unwrap();
        let first = Fragment {
            descriptor: plan.descriptor(),
            index: 0,
            payload: plan.fragment(0).unwrap().to_vec().into(),
        };
        assert!(assembler.push(first, now).unwrap().frame.is_none());
        assert_eq!(
            assembler.received_bytes(plan.descriptor().metadata),
            Some(DEFAULT_MTU - FRAGMENT_OVERHEAD)
        );
        assembler.discard(plan.descriptor().metadata);
        assert_eq!(assembler.received_bytes(plan.descriptor().metadata), None);
        // A new key on the unchanged epoch still assembles and completes normally.
        let replacement =
            FramePlan::new(metadata(8, true), vec![8; 500].into(), DEFAULT_MTU).unwrap();
        let completed = assembler
            .push(
                Fragment {
                    descriptor: replacement.descriptor(),
                    index: 0,
                    payload: replacement.payload().clone(),
                },
                now,
            )
            .unwrap()
            .frame
            .unwrap();
        assert_eq!(completed.metadata.epoch, 0);
        assert_eq!(completed.metadata.frame_id, 8);
        gate.keyframe_progress(replacement.descriptor(), 1, now + Duration::from_secs(24))
            .unwrap();
        gate.keyframe_abandoned(replacement.descriptor().metadata);
        assert!(gate.request(
            replacement.descriptor().metadata,
            true,
            now + Duration::from_secs(24)
        ));
    }

    async fn socket() -> UdpSocket {
        UdpSocket::bind("127.0.0.1:0").await.unwrap()
    }
    async fn received(socket: &UdpSocket) -> Vec<u8> {
        let mut buffer = vec![0; MAX_DATAGRAM_SIZE];
        let count = timeout(Duration::from_secs(2), socket.recv(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        buffer.truncate(count);
        buffer
    }
    async fn deliver(from: &UdpSocket, to: &UdpSocket, packet: &SealedDatagram) -> Vec<u8> {
        from.send_to(&packet.bytes, to.local_addr().unwrap())
            .await
            .unwrap();
        received(to).await
    }
    async fn fragment(
        from: &UdpSocket,
        to: &UdpSocket,
        sender: &mut DatagramSender,
        receiver: &mut DatagramReceiver,
        plan: &FramePlan,
        index: u16,
        now: Instant,
    ) -> Fragment {
        let packet = sender.seal_fragment(plan, index).unwrap();
        let raw = deliver(from, to, &packet).await;
        let AuthenticatedPacket::Fragment(fragment) = receiver.open(&raw, now).unwrap() else {
            panic!("fragment expected")
        };
        fragment
    }

    #[tokio::test]
    async fn authenticated_udp_path_direction_tampering_and_replay() {
        let client = socket().await;
        let host = socket().await;
        let sid = SessionId([9; 16]);
        let seed = [7; 32];
        let now = Instant::now();
        let mut client_tx = DatagramSender::new(&seed, sid, Direction::ControllerToHost).unwrap();
        let mut host_rx = DatagramReceiver::new(&seed, sid, Direction::ControllerToHost).unwrap();
        let mut host_tx = DatagramSender::new(&seed, sid, Direction::HostToController).unwrap();
        let mut client_rx = DatagramReceiver::new(&seed, sid, Direction::HostToController).unwrap();
        let probe = client_tx.seal_probe(ProbeKind::Probe, [11; 16]).unwrap();
        assert_eq!(peek_session_id(&probe.bytes).unwrap(), sid);
        assert_eq!(
            peek_probe_kind(&probe.bytes).unwrap(),
            Some(ProbeKind::Probe)
        );
        let raw = deliver(&client, &host, &probe).await;
        assert!(
            matches!(host_rx.open(&raw, now).unwrap(), AuthenticatedPacket::Probe { kind: ProbeKind::Probe, token } if token == [11; 16])
        );
        assert_eq!(host_rx.open(&raw, now).unwrap_err(), MediaError::Replay);
        let challenge = host_tx.seal_probe(ProbeKind::Challenge, [12; 16]).unwrap();
        let raw = deliver(&host, &client, &challenge).await;
        assert!(
            matches!(client_rx.open(&raw, now).unwrap(), AuthenticatedPacket::Probe { kind: ProbeKind::Challenge, token } if token == [12; 16])
        );
        let mut wrong_direction =
            DatagramReceiver::new(&seed, sid, Direction::ControllerToHost).unwrap();
        assert_eq!(
            wrong_direction.open(&raw, now).unwrap_err(),
            MediaError::Crypto
        );
        let response = client_tx.seal_probe(ProbeKind::Response, [12; 16]).unwrap();
        let mut forged = response.bytes.clone();
        forged[22..30].copy_from_slice(&1_000_000u64.to_be_bytes());
        assert_eq!(host_rx.open(&forged, now).unwrap_err(), MediaError::Crypto);
        let raw = deliver(&client, &host, &response).await;
        assert!(
            matches!(host_rx.open(&raw, now).unwrap(), AuthenticatedPacket::Probe { kind: ProbeKind::Response, token } if token == [12; 16])
        );
        assert!(host_rx.take_report(now + REPORT_INTERVAL).is_none());
        for index in 0..REPLAY_WINDOW {
            let packet = client_tx
                .seal_probe(ProbeKind::Probe, [(index % 256) as u8; 16])
                .unwrap();
            host_rx.open(&packet.bytes, now).unwrap();
        }
        assert_eq!(
            host_rx.open(&response.bytes, now).unwrap_err(),
            MediaError::Replay
        );
        let mut other_session =
            DatagramReceiver::new(&seed, SessionId([10; 16]), Direction::ControllerToHost).unwrap();
        assert_eq!(
            other_session.open(&response.bytes, now).unwrap_err(),
            MediaError::WrongSession
        );
    }

    #[tokio::test]
    async fn udp_fragment_loss_tail_notice_and_audio_epochs_are_independent() {
        let tx = socket().await;
        let rx = socket().await;
        let relay = socket().await;
        let sid = SessionId([2; 16]);
        let now = Instant::now();
        let mut sender = DatagramSender::new(&[3; 32], sid, Direction::HostToController).unwrap();
        let mut receiver =
            DatagramReceiver::new(&[3; 32], sid, Direction::HostToController).unwrap();
        let mut assembler = Reassembler::default();
        let plan =
            FramePlan::new(metadata(0, false), Bytes::from(vec![23; 4000]), DEFAULT_MTU).unwrap();
        let mut completed = None;
        for index in (0..plan.fragment_count()).rev() {
            completed = assembler
                .push(
                    fragment(&tx, &rx, &mut sender, &mut receiver, &plan, index, now).await,
                    now,
                )
                .unwrap()
                .frame
                .or(completed);
        }
        assert_eq!(completed.unwrap().payload, plan.payload().clone());
        let missing =
            FramePlan::new(metadata(1, false), Bytes::from(vec![17; 4000]), DEFAULT_MTU).unwrap();
        for index in 0..missing.fragment_count() {
            let packet = sender.seal_fragment(&missing, index).unwrap();
            let raw = deliver(&tx, &relay, &packet).await;
            if index == 1 {
                continue;
            } // Actual UDP proxy discards the encrypted datagram.
            relay.send_to(&raw, rx.local_addr().unwrap()).await.unwrap();
            let AuthenticatedPacket::Fragment(fragment) =
                receiver.open(&received(&rx).await, now).unwrap()
            else {
                panic!("fragment expected")
            };
            assembler.push(fragment, now).unwrap();
        }
        assert!(assembler
            .expire(now + DELTA_TTL)
            .recovery
            .contains(&Recovery::RefreshVideo(metadata(1, false))));
        let tail = FramePlan::new(
            metadata(2, false),
            Bytes::from_static(b"last static update"),
            DEFAULT_MTU,
        )
        .unwrap();
        assembler
            .note_notice(FrameNotice::Sent(tail.descriptor()), now)
            .unwrap();
        assert!(assembler
            .expire(now + DELTA_TTL)
            .recovery
            .contains(&Recovery::RefreshVideo(metadata(2, false))));
        assembler.set_epoch(MediaKind::Video, 0, 1).unwrap();
        let stale = fragment(&tx, &rx, &mut sender, &mut receiver, &tail, 0, now).await;
        assert!(matches!(
            assembler.push(stale, now),
            Err(MediaError::WrongEpoch)
        ));
        let future = FramePlan::new(
            FrameMetadata {
                epoch: 2,
                frame_id: 3,
                keyframe: true,
                ..metadata(3, true)
            },
            Bytes::from_static(b"new format keyframe"),
            DEFAULT_MTU,
        )
        .unwrap();
        assert!(matches!(
            assembler.push(
                fragment(&tx, &rx, &mut sender, &mut receiver, &future, 0, now).await,
                now
            ),
            Err(MediaError::WrongEpoch)
        ));
        assembler.set_epoch(MediaKind::Video, 0, 2).unwrap();
        assert_eq!(
            assembler
                .push(
                    fragment(&tx, &rx, &mut sender, &mut receiver, &future, 0, now).await,
                    now
                )
                .unwrap()
                .frame
                .unwrap()
                .payload,
            future.payload().clone()
        );
        let audio_meta = FrameMetadata {
            kind: MediaKind::Audio,
            epoch: 0,
            display_id: 0,
            frame_id: 9,
            keyframe: false,
        };
        let audio =
            FramePlan::new(audio_meta, Bytes::from_static(b"opus packet"), DEFAULT_MTU).unwrap();
        let frame = assembler
            .push(
                fragment(&tx, &rx, &mut sender, &mut receiver, &audio, 0, now).await,
                now,
            )
            .unwrap()
            .frame
            .unwrap();
        assert_eq!(frame.payload, audio.payload().clone());
        let old_audio = FramePlan::new(
            FrameMetadata {
                frame_id: 8,
                ..audio_meta
            },
            Bytes::from_static(b"older opus"),
            DEFAULT_MTU,
        )
        .unwrap();
        assert!(assembler
            .push(
                fragment(&tx, &rx, &mut sender, &mut receiver, &old_audio, 0, now).await,
                now
            )
            .unwrap()
            .frame
            .is_none());
        assembler.set_epoch(MediaKind::Audio, 0, 1).unwrap();
        assert!(matches!(
            assembler.push(
                fragment(&tx, &rx, &mut sender, &mut receiver, &audio, 0, now).await,
                now
            ),
            Err(MediaError::WrongEpoch)
        ));
    }

    #[tokio::test]
    async fn udp_keyframe_nack_reseals_only_missing_fragments_with_bounded_retries() {
        let tx = socket().await;
        let rx = socket().await;
        let relay = socket().await;
        let sid = SessionId([4; 16]);
        let now = Instant::now();
        let mut sender = DatagramSender::new(&[5; 32], sid, Direction::HostToController).unwrap();
        let mut receiver =
            DatagramReceiver::new(&[5; 32], sid, Direction::HostToController).unwrap();
        let plan =
            FramePlan::new(metadata(7, true), Bytes::from(vec![31; 4000]), DEFAULT_MTU).unwrap();
        let mut cache = KeyframeCache::default();
        cache.insert(plan.clone(), now).unwrap();
        let mut assembler = Reassembler::default();
        for index in 0..plan.fragment_count() {
            let packet = sender.seal_fragment(&plan, index).unwrap();
            let raw = deliver(&tx, &relay, &packet).await;
            if index == 1 {
                continue;
            }
            relay.send_to(&raw, rx.local_addr().unwrap()).await.unwrap();
            let AuthenticatedPacket::Fragment(fragment) =
                receiver.open(&received(&rx).await, now).unwrap()
            else {
                panic!("fragment expected")
            };
            assembler.push(fragment, now).unwrap();
        }
        cache
            .initial_sent(plan.descriptor().metadata, now, 1_000_000)
            .unwrap();
        assembler
            .note_notice(FrameNotice::Sent(plan.descriptor()), now)
            .unwrap();
        let Recovery::KeyframeNack(nack) =
            assembler.expire(now + REPORT_INTERVAL).recovery.remove(0)
        else {
            panic!("NACK expected")
        };
        let retry = cache.retransmit(&nack, now).unwrap();
        assert_eq!(retry.indices, [1]);
        assert_eq!(cache.retransmit(&nack, now).unwrap().indices, [1]);
        assert!(cache.retransmit(&nack, now).unwrap().indices.is_empty());
        let frame = assembler
            .push(
                fragment(
                    &tx,
                    &rx,
                    &mut sender,
                    &mut receiver,
                    &retry.frame,
                    retry.indices[0],
                    now,
                )
                .await,
                now,
            )
            .unwrap();
        assert_eq!(frame.frame.unwrap().payload, plan.payload().clone());
        assert_eq!(
            frame.recovery,
            [Recovery::KeyframeComplete(KeyframeComplete {
                metadata: metadata(7, true)
            })]
        );
        cache.complete(KeyframeComplete {
            metadata: metadata(7, true),
        });
        assert!(cache.retransmit(&nack, now).is_err());
        assert!(keyframe_deadline(MAX_VIDEO_FRAME, 1_000_000) > Duration::from_secs(60));
        let whole = FramePlan::new(
            metadata(8, true),
            Bytes::from_static(b"whole keyframe lost"),
            DEFAULT_MTU,
        )
        .unwrap();
        assembler
            .note_notice(FrameNotice::Sent(whole.descriptor()), now)
            .unwrap();
        assert!(matches!(
            assembler.expire(now + REPORT_INTERVAL).recovery.as_slice(),
            [Recovery::KeyframeNack(_)]
        ));
        let mut quota = Reassembler::default();
        for id in 20..28 {
            quota
                .note_notice(
                    FrameNotice::Sent(FrameDescriptor {
                        metadata: metadata(id, true),
                        ..plan.descriptor()
                    }),
                    now,
                )
                .unwrap();
        }
        let next = FrameNotice::Sent(FrameDescriptor {
            metadata: metadata(28, true),
            ..plan.descriptor()
        });
        assert!(matches!(
            quota.note_notice(next, now),
            Err(MediaError::Capacity)
        ));
        assert_eq!(
            quota
                .note_notice(FrameNotice::Abandoned(metadata(20, true)), now)
                .unwrap()
                .recovery,
            [Recovery::RefreshVideo(metadata(20, true))]
        );
        quota.note_notice(next, now).unwrap();
    }

    #[test]
    fn disjoint_ack_ranges_release_credit_once_and_preserve_loss_response() {
        let now = Instant::now();
        let mut pacer = Pacer::new(DEFAULT_MTU, 1_000_000_000, now).unwrap();
        let initial = pacer.congestion_window();
        for sequence in 0..8 {
            pacer.on_sent(sequence, DEFAULT_MTU, now).unwrap();
        }
        let report = ReceiveReport {
            largest: 7,
            ack_delay_us: 0,
            ranges: vec![
                AckRange { first: 6, last: 7 },
                AckRange { first: 0, last: 3 },
            ],
        };
        pacer
            .on_report(&report, now + Duration::from_millis(25))
            .unwrap();
        assert_eq!(pacer.inflight_bytes(), 2 * DEFAULT_MTU);
        let grown = pacer.congestion_window();
        assert_eq!(grown, initial + 6 * DEFAULT_MTU);
        pacer
            .on_report(&report, now + Duration::from_millis(25))
            .unwrap();
        assert_eq!(pacer.inflight_bytes(), 2 * DEFAULT_MTU);
        assert_eq!(pacer.congestion_window(), grown);
        pacer
            .on_report(&report, now + Duration::from_millis(200))
            .unwrap();
        assert_eq!(pacer.inflight_bytes(), DEFAULT_MTU);
        assert_eq!(pacer.congestion_window(), grown / 2);
    }

    #[tokio::test]
    async fn udp_authenticated_ack_feedback_drives_slow_start_loss_and_pause() {
        let tx = socket().await;
        let rx = socket().await;
        let relay = socket().await;
        let sid = SessionId([6; 16]);
        let now = Instant::now();
        let mut sender = DatagramSender::new(&[7; 32], sid, Direction::HostToController).unwrap();
        let mut receiver =
            DatagramReceiver::new(&[7; 32], sid, Direction::HostToController).unwrap();
        let mut feedback_tx =
            DatagramSender::new(&[7; 32], sid, Direction::ControllerToHost).unwrap();
        let mut feedback_rx =
            DatagramReceiver::new(&[7; 32], sid, Direction::ControllerToHost).unwrap();
        let plan =
            FramePlan::new(metadata(0, false), Bytes::from(vec![2; 8400]), DEFAULT_MTU).unwrap();
        let mut pacer = Pacer::new(DEFAULT_MTU, 1_000_000_000, now).unwrap();
        let initial = pacer.congestion_window();
        for index in 0..plan.fragment_count() {
            let packet = sender.seal_fragment(&plan, index).unwrap();
            assert_eq!(
                pacer.decide(packet.bytes.len(), now + Duration::from_secs(5), now, false),
                SendDecision::Send
            );
            let raw = deliver(&tx, &relay, &packet).await;
            pacer
                .on_sent(packet.sequence, packet.bytes.len(), now)
                .unwrap();
            if index == 1 {
                continue;
            }
            relay.send_to(&raw, rx.local_addr().unwrap()).await.unwrap();
            receiver.open(&received(&rx).await, now).unwrap();
        }
        let report = receiver.take_report(now + REPORT_INTERVAL).unwrap();
        assert!(report.ranges.len() <= MAX_ACK_RANGES);
        let packet = feedback_tx.seal_report(&report).unwrap();
        assert!(!packet.ack_eliciting);
        let AuthenticatedPacket::Report(report) = feedback_rx
            .open(&deliver(&rx, &tx, &packet).await, now)
            .unwrap()
        else {
            panic!("report expected")
        };
        assert!(feedback_rx.take_report(now + REPORT_INTERVAL).is_none());
        pacer
            .on_report(&report, now + Duration::from_millis(25))
            .unwrap();
        assert!(pacer.congestion_window() > initial);
        let grown = pacer.congestion_window();
        pacer
            .on_report(&report, now + Duration::from_millis(200))
            .unwrap();
        assert!(pacer.congestion_window() < grown);
        let packet = sender.seal_fragment(&plan, 0).unwrap();
        let _ = deliver(&tx, &relay, &packet).await; // A later media datagram is lost after the real send.
        pacer
            .on_sent(
                packet.sequence,
                packet.bytes.len(),
                now + Duration::from_millis(300),
            )
            .unwrap();
        assert_eq!(
            pacer.decide(
                100,
                now + Duration::from_secs(4),
                now + Duration::from_secs(2),
                false
            ),
            SendDecision::FeedbackRequired
        );
        pacer
            .on_probe_response(Duration::from_millis(5), now + Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            pacer.decide(
                100,
                now + Duration::from_secs(4),
                now + Duration::from_secs(2),
                false
            ),
            SendDecision::Send
        );
        assert_eq!(
            pacer.decide(
                100,
                now + Duration::from_secs(4),
                now + Duration::from_secs(2),
                true
            ),
            SendDecision::Wait(Duration::from_millis(1))
        );
        assert_eq!(pacer.decide(100, now, now, false), SendDecision::Expired);
    }
}
