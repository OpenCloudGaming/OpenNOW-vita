
use crate::streaming::video::VideoDecodeWorker;
use h264_reader::annexb::AnnexBReader;
use h264_reader::nal::sps::SeqParameterSet;
use h264_reader::nal::{Nal, RefNal, UnitType};
use h264_reader::push::NalInterest;
use rtc::rtp::Packet;
use rtc::rtp::codec::h264::H264Packet;
use rtc::rtp::packetizer::Depacketizer;

const MAX_H264_ACCESS_UNIT_BYTES: usize = 2 * 1024 * 1024;
const VIDEO_RTP_CLOCK_RATE: u32 = 90_000;
const LOW_FPS_DAMAGE_LIMIT: u8 = 8;
const HIGH_FPS_DAMAGE_LIMIT: u8 = 3;
const MIN_REORDER_GRACE_US: u64 = 2_000;
const MAX_REORDER_GRACE_US: u64 = 12_000;

#[derive(Default)]
pub struct VideoSampleStats {
    pub dropped: u32,
    pub source_frame_duration_us: Option<u64>,
    pub encoded_resolution: Option<(u32, u32)>,
    pub jitter_ms: f32,
    pub reorder_rescued: u32,
    pub reorder_expired: u32,
}

pub(crate) trait VideoAuSink {
    fn submit_access_unit(&self, data: Vec<u8>, source_frame_duration_us: Option<u64>) -> bool;
    fn begin_resync(&self);
}

impl VideoAuSink for VideoDecodeWorker {
    fn submit_access_unit(&self, data: Vec<u8>, source_frame_duration_us: Option<u64>) -> bool {
        VideoDecodeWorker::submit_access_unit(self, data, source_frame_duration_us)
    }

    fn begin_resync(&self) {
        VideoDecodeWorker::begin_resync(self);
    }
}

// jitter estimator, rfc 3550 formula
pub struct JitterEstimator {
    jitter_us: f64,
    last_arrival_us: Option<u64>,
    last_rtp_timestamp: Option<u32>,
}

impl Default for JitterEstimator {
    fn default() -> Self {
        Self {
            jitter_us: 0.0,
            last_arrival_us: None,
            last_rtp_timestamp: None,
        }
    }
}

impl JitterEstimator {
    pub fn update(&mut self, arrival_us: u64, rtp_timestamp: u32) -> f32 {
        if let (Some(last_arrival), Some(last_rtp)) =
            (self.last_arrival_us, self.last_rtp_timestamp)
        {
            let arrival_diff_us = arrival_us.saturating_sub(last_arrival) as f64;
            let rtp_diff_us = (rtp_timestamp.wrapping_sub(last_rtp) as f64 * 1_000_000.0)
                / f64::from(VIDEO_RTP_CLOCK_RATE);
            let transit_diff_us = (arrival_diff_us - rtp_diff_us).abs();
            // RFC 3550 EWMA smoother: J = J + (|D| - J) / 16
            self.jitter_us += (transit_diff_us - self.jitter_us) / 16.0;
        }
        self.last_arrival_us = Some(arrival_us);
        self.last_rtp_timestamp = Some(rtp_timestamp);
        (self.jitter_us / 1000.0) as f32
    }

    pub fn current_jitter_ms(&self) -> f32 {
        (self.jitter_us / 1000.0) as f32
    }
}

pub(crate) fn reorder_grace_us_from_jitter(jitter_ms: f32) -> u64 {
    let twice_jitter_us = (jitter_ms.max(0.0) * 2.0 * 1000.0) as u64;
    twice_jitter_us.clamp(MIN_REORDER_GRACE_US, MAX_REORDER_GRACE_US)
}

pub struct VideoRtp {
    depacketizer: H264Packet,
    pending: Option<PendingVideoFrame>,
    reorder_hold: Option<PendingVideoFrame>,
    reorder_hold_started_at_us: u64,
    reorder_hold_grace_us: u64,
    reorder_hold_expected_sequence: Option<u16>,
    parked_au: Option<AssembledVideoFrame>,
    assemble_order: Vec<usize>,
    assemble_buf: Vec<u8>,
    next_sequence: Option<u16>,
    last_frame_timestamp: Option<u32>,
    source_frame_duration_us: Option<u64>,
    damage_score: u8,
    decode_width: u32,
    decode_height: u32,
    stream_too_large: bool,
    waiting_for_keyframe: bool,
    jitter_estimator: JitterEstimator,
}

struct PendingVideoFrame {
    timestamp: u32,
    packets: Vec<Packet>,
}

struct AssembledVideoFrame {
    data: Vec<u8>,
    timestamp: u32,
    marker_sequence: u16,
}

enum FrameAssembly {
    Pending,
    Complete { data: Vec<u8>, marker_sequence: u16 },
    Invalid,
}

impl PendingVideoFrame {
    fn new(packet: Packet) -> Self {
        Self {
            timestamp: packet.header.timestamp,
            packets: vec![packet],
        }
    }

    fn insert(&mut self, packet: Packet) {
        if !self
            .packets
            .iter()
            .any(|existing| existing.header.sequence_number == packet.header.sequence_number)
        {
            self.packets.push(packet);
        }
    }

    fn marker_sequence(&self) -> Option<u16> {
        self.packets
            .iter()
            .find(|packet| packet.header.marker)
            .map(|packet| packet.header.sequence_number)
    }

    fn assemble(
        &self,
        depacketizer: &mut H264Packet,
        expected_sequence: Option<u16>,
        order: &mut Vec<usize>,
        buf: &mut Vec<u8>,
    ) -> FrameAssembly {
        let Some(marker_sequence) = self.marker_sequence() else {
            return FrameAssembly::Pending;
        };

        order.clear();
        if self.packets.len() == 1 {
            order.push(0);
        } else {
            order.extend(0..self.packets.len());
            order.sort_unstable_by_key(|&index| {
                std::cmp::Reverse(
                    marker_sequence.wrapping_sub(self.packets[index].header.sequence_number),
                )
            });
        }

        let Some(&first_index) = order.first() else {
            return FrameAssembly::Pending;
        };
        let first = &self.packets[first_index];
        if expected_sequence.is_some_and(|expected| first.header.sequence_number != expected)
            || !depacketizer.is_partition_head(&first.payload)
        {
            return FrameAssembly::Pending;
        }
        if order.windows(2).any(|pair| {
            self.packets[pair[1]].header.sequence_number
                != self.packets[pair[0]]
                    .header
                    .sequence_number
                    .wrapping_add(1)
        }) {
            return FrameAssembly::Pending;
        }

        *depacketizer = H264Packet::default();
        let payload_bytes: usize = order
            .iter()
            .map(|&index| self.packets[index].payload.len())
            .sum();
        buf.clear();
        buf.reserve(payload_bytes);
        for &index in order.iter() {
            let Ok(nalu) = depacketizer.depacketize(&self.packets[index].payload) else {
                *depacketizer = H264Packet::default();
                buf.clear();
                return FrameAssembly::Invalid;
            };
            buf.extend_from_slice(&nalu);
            if buf.len() > MAX_H264_ACCESS_UNIT_BYTES {
                *depacketizer = H264Packet::default();
                buf.clear();
                return FrameAssembly::Invalid;
            }
        }
        *depacketizer = H264Packet::default();
        FrameAssembly::Complete {
            data: std::mem::take(buf),
            marker_sequence,
        }
    }
}

impl VideoRtp {
    pub fn new(decode_width: u32, decode_height: u32) -> Self {
        Self {
            depacketizer: H264Packet::default(),
            pending: None,
            reorder_hold: None,
            reorder_hold_started_at_us: 0,
            reorder_hold_grace_us: MIN_REORDER_GRACE_US,
            reorder_hold_expected_sequence: None,
            parked_au: None,
            assemble_order: Vec::new(),
            assemble_buf: Vec::new(),
            next_sequence: None,
            last_frame_timestamp: None,
            source_frame_duration_us: None,
            damage_score: 0,
            decode_width,
            decode_height,
            stream_too_large: false,
            waiting_for_keyframe: false,
            jitter_estimator: JitterEstimator::default(),
        }
    }

    pub fn waiting_for_keyframe(&self) -> bool {
        self.waiting_for_keyframe
    }

    pub fn current_jitter_ms(&self) -> f32 {
        self.jitter_estimator.current_jitter_ms()
    }

    pub fn reorder_deadline_us(&self) -> Option<u64> {
        self.reorder_hold
            .as_ref()
            .map(|_| self.reorder_hold_started_at_us.saturating_add(self.reorder_hold_grace_us))
    }

    pub fn receive(
        &mut self,
        worker: &VideoDecodeWorker,
        packet: Packet,
        keyframe_requested: &mut bool,
        arrival_us: u64,
    ) -> VideoSampleStats {
        self.receive_into(worker, packet, keyframe_requested, arrival_us)
    }

    pub(crate) fn receive_into<S: VideoAuSink>(
        &mut self,
        sink: &S,
        packet: Packet,
        keyframe_requested: &mut bool,
        arrival_us: u64,
    ) -> VideoSampleStats {
        let mut stats = VideoSampleStats::default();
        stats.jitter_ms = self.jitter_estimator.update(arrival_us, packet.header.timestamp);
        self.expire_reorder_grace(sink, keyframe_requested, arrival_us, &mut stats);

        if packet.payload.is_empty() {
            if self.next_sequence == Some(packet.header.sequence_number) {
                self.next_sequence = Some(packet.header.sequence_number.wrapping_add(1));
            }
            return stats;
        }

        let packet_timestamp = packet.header.timestamp;

        if self
            .reorder_hold
            .as_ref()
            .is_some_and(|hold| hold.timestamp == packet_timestamp)
        {
            if let Some(hold) = &mut self.reorder_hold {
                hold.insert(packet);
            }
            self.try_complete_reorder_hold(sink, keyframe_requested, &mut stats);
            return stats;
        }

        let belongs_to_pending = self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.timestamp == packet_timestamp);
        if !belongs_to_pending && (self.reorder_hold.is_some() || self.parked_au.is_some()) {
            let reference_ts = self
                .pending
                .as_ref()
                .map(|pending| pending.timestamp)
                .or_else(|| self.parked_au.as_ref().map(|ready| ready.timestamp))
                .or_else(|| self.reorder_hold.as_ref().map(|hold| hold.timestamp));
            if reference_ts.is_some_and(|ts| !timestamp_is_newer(packet_timestamp, ts)) {
                return stats;
            }
            if self.reorder_hold.is_some() {
                self.reorder_hold_started_at_us = arrival_us.saturating_sub(self.reorder_hold_grace_us);
                self.expire_reorder_grace(sink, keyframe_requested, arrival_us, &mut stats);
            } else if let Some(ready) = self.parked_au.take() {
                self.process_assembled(ready, sink, keyframe_requested, &mut stats);
            }
        }

        if let Some(pending) = &self.pending
            && pending.timestamp != packet_timestamp
        {
            if !timestamp_is_newer(packet_timestamp, pending.timestamp) {
                return stats;
            }
            if let Some(incomplete) = self.pending.take() {
                let grace = reorder_grace_us_from_jitter(self.jitter_estimator.current_jitter_ms());
                self.reorder_hold_expected_sequence = self.next_sequence;
                self.reorder_hold_grace_us = grace;
                self.reorder_hold_started_at_us = arrival_us;
                self.next_sequence = Some(packet.header.sequence_number);
                self.reorder_hold = Some(incomplete);
            }
        }

        if self.pending.is_none() {
            if self
                .last_frame_timestamp
                .is_some_and(|last| !timestamp_is_newer(packet_timestamp, last))
            {
                return stats;
            }
            self.pending = Some(PendingVideoFrame::new(packet));
        } else if let Some(pending) = &mut self.pending {
            pending.insert(packet);
        }

        let Some(pending) = self.pending.take() else {
            return stats;
        };
        let assembly = pending.assemble(
            &mut self.depacketizer,
            self.next_sequence,
            &mut self.assemble_order,
            &mut self.assemble_buf,
        );
        let (data, marker_sequence) = match assembly {
            FrameAssembly::Pending => {
                self.pending = Some(pending);
                return stats;
            }
            FrameAssembly::Invalid => {
                self.next_sequence = None;
                *keyframe_requested = true;
                self.record_damage(sink);
                stats.dropped = stats.dropped.saturating_add(1);
                return stats;
            }
            FrameAssembly::Complete {
                data,
                marker_sequence,
            } => (data, marker_sequence),
        };
        let completed = pending;

        if self.reorder_hold.is_some() {
            self.parked_au = Some(AssembledVideoFrame {
                data,
                timestamp: completed.timestamp,
                marker_sequence,
            });
            return stats;
        }

        self.process_assembled(
            AssembledVideoFrame {
                data,
                timestamp: completed.timestamp,
                marker_sequence,
            },
            sink,
            keyframe_requested,
            &mut stats,
        );
        stats
    }

    pub fn expire_reorder_grace_if_due<S: VideoAuSink>(
        &mut self,
        sink: &S,
        keyframe_requested: &mut bool,
        now_us: u64,
    ) -> VideoSampleStats {
        let mut stats = VideoSampleStats::default();
        self.expire_reorder_grace(sink, keyframe_requested, now_us, &mut stats);
        stats
    }

    fn expire_reorder_grace<S: VideoAuSink>(
        &mut self,
        sink: &S,
        keyframe_requested: &mut bool,
        now_us: u64,
        stats: &mut VideoSampleStats,
    ) {
        if self.reorder_hold.is_none()
            || now_us.saturating_sub(self.reorder_hold_started_at_us) < self.reorder_hold_grace_us
        {
            return;
        }
        let Some(_hold) = self.reorder_hold.take() else {
            return;
        };
        stats.reorder_expired = stats.reorder_expired.saturating_add(1);
        stats.dropped = stats.dropped.saturating_add(1);
        *keyframe_requested = true;
        self.record_damage(sink);
        self.reorder_hold_started_at_us = 0;
        self.reorder_hold_expected_sequence = None;
        self.depacketizer = H264Packet::default();
        if let Some(ready) = self.parked_au.take() {
            self.process_assembled(ready, sink, keyframe_requested, stats);
        }
    }

    fn try_complete_reorder_hold<S: VideoAuSink>(
        &mut self,
        sink: &S,
        keyframe_requested: &mut bool,
        stats: &mut VideoSampleStats,
    ) {
        let Some(hold) = self.reorder_hold.take() else {
            return;
        };
        let assembly = hold.assemble(
            &mut self.depacketizer,
            self.reorder_hold_expected_sequence,
            &mut self.assemble_order,
            &mut self.assemble_buf,
        );
        let FrameAssembly::Complete {
            data,
            marker_sequence,
        } = assembly
        else {
            self.reorder_hold = Some(hold);
            return;
        };
        let completed = hold;
        self.reorder_hold_started_at_us = 0;
        self.reorder_hold_expected_sequence = None;
        self.depacketizer = H264Packet::default();
        stats.reorder_rescued = stats.reorder_rescued.saturating_add(1);
        self.process_assembled(
            AssembledVideoFrame {
                data,
                timestamp: completed.timestamp,
                marker_sequence,
            },
            sink,
            keyframe_requested,
            stats,
        );
        if let Some(ready) = self.parked_au.take() {
            self.process_assembled(ready, sink, keyframe_requested, stats);
        }
    }

    fn process_assembled<S: VideoAuSink>(
        &mut self,
        assembled: AssembledVideoFrame,
        sink: &S,
        keyframe_requested: &mut bool,
        stats: &mut VideoSampleStats,
    ) {
        let AssembledVideoFrame {
            data,
            timestamp,
            marker_sequence,
        } = assembled;
        self.next_sequence = Some(marker_sequence.wrapping_add(1));
        stats.source_frame_duration_us = self.last_frame_timestamp.map(|previous| {
            u64::from(timestamp.wrapping_sub(previous)) * 1_000_000
                / u64::from(VIDEO_RTP_CLOCK_RATE)
        });
        if let Some(duration) = stats.source_frame_duration_us {
            self.source_frame_duration_us = Some(
                self.source_frame_duration_us
                    .map(|average| (average * 7 + duration) / 8)
                    .unwrap_or(duration),
            );
        }
        self.last_frame_timestamp = Some(timestamp);

        let unit = inspect_h264_access_unit(&data);
        stats.encoded_resolution = unit.resolution;
        let sample_too_large = unit
            .resolution
            .is_some_and(|(width, height)| width > self.decode_width || height > self.decode_height);
        if sample_too_large {
            eprintln!(
                "Dropping H264 access unit larger than decoder: {:?} > {}x{}",
                unit.resolution, self.decode_width, self.decode_height
            );
            self.stream_too_large = true;
            *keyframe_requested = true;
            if !self.waiting_for_keyframe {
                sink.begin_resync();
            }
            self.waiting_for_keyframe = true;
            stats.dropped = stats.dropped.saturating_add(1);
            return;
        }
        if self.stream_too_large {
            if unit.resolution.is_none() || !unit.has_idr {
                *keyframe_requested = true;
                self.waiting_for_keyframe = true;
                stats.dropped = stats.dropped.saturating_add(1);
                return;
            }
            self.stream_too_large = false;
        }
        if self.waiting_for_keyframe {
            if !unit.has_idr {
                *keyframe_requested = true;
                stats.dropped = stats.dropped.saturating_add(1);
                return;
            }
            self.waiting_for_keyframe = false;
            self.damage_score = 0;
        } else {
            self.damage_score = self.damage_score.saturating_sub(1);
        }

        if !sink.submit_access_unit(data, self.source_frame_duration_us) {
            stats.dropped = stats.dropped.saturating_add(1);
        }
    }

    fn record_damage<S: VideoAuSink>(&mut self, sink: &S) {
        if self.waiting_for_keyframe {
            return;
        }

        self.damage_score = self.damage_score.saturating_add(1);
        let source_fps = self
            .source_frame_duration_us
            .filter(|duration| *duration > 0)
            .map(|duration| 1_000_000 / duration)
            .unwrap_or(30);
        let damage_limit = if source_fps <= 30 {
            LOW_FPS_DAMAGE_LIMIT
        } else if source_fps >= 60 {
            HIGH_FPS_DAMAGE_LIMIT
        } else {
            LOW_FPS_DAMAGE_LIMIT
                - (((source_fps - 30) * u64::from(LOW_FPS_DAMAGE_LIMIT - HIGH_FPS_DAMAGE_LIMIT)
                    + 29)
                    / 30) as u8
        };
        if self.damage_score < damage_limit {
            return;
        }

        sink.begin_resync();
        self.waiting_for_keyframe = true;
        self.damage_score = 0;
    }
}

fn timestamp_is_newer(candidate: u32, reference: u32) -> bool {
    let distance = candidate.wrapping_sub(reference);
    distance != 0 && distance < (1 << 31)
}

struct AccessUnitInfo {
    has_idr: bool,
    resolution: Option<(u32, u32)>,
}

fn inspect_h264_access_unit(data: &[u8]) -> AccessUnitInfo {
    let mut info = AccessUnitInfo {
        has_idr: false,
        resolution: None,
    };
    let mut reader = AnnexBReader::accumulate(|nal: RefNal<'_>| {
        let Ok(header) = nal.header() else {
            return NalInterest::Ignore;
        };
        match header.nal_unit_type() {
            UnitType::SliceLayerWithoutPartitioningIdr => {
                info.has_idr = true;
                NalInterest::Ignore
            }
            UnitType::SeqParameterSet => {
                if nal.is_complete() {
                    info.resolution = SeqParameterSet::from_bits(nal.rbsp_bits())
                        .and_then(|sps| sps.pixel_dimensions())
                        .ok();
                }
                NalInterest::Buffer
            }
            _ => NalInterest::Ignore,
        }
    });
    reader.push(data);
    reader.reset();
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FakeSink {
        submitted: RefCell<Vec<Vec<u8>>>,
        resyncs: AtomicU64,
        accept: RefCell<bool>,
    }

    impl FakeSink {
        fn new() -> Self {
            Self {
                submitted: RefCell::new(Vec::new()),
                resyncs: AtomicU64::new(0),
                accept: RefCell::new(true),
            }
        }
    }

    impl VideoAuSink for FakeSink {
        fn submit_access_unit(
            &self,
            data: Vec<u8>,
            _source_frame_duration_us: Option<u64>,
        ) -> bool {
            if !*self.accept.borrow() {
                return false;
            }
            self.submitted.borrow_mut().push(data);
            true
        }

        fn begin_resync(&self) {
            self.resyncs.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn single_nal_payload() -> Bytes {
        Bytes::from_static(&[0x41, 0x9a, 0x00])
    }

    fn pkt(seq: u16, ts: u32, marker: bool) -> Packet {
        Packet {
            header: rtc::rtp::Header {
                version: 2,
                padding: false,
                extension: false,
                marker,
                payload_type: 96,
                sequence_number: seq,
                timestamp: ts,
                ssrc: 1,
                csrc: Vec::new(),
                extension_profile: 0,
                extensions: Vec::new(),
                extensions_padding: 0,
            },
            payload: single_nal_payload(),
        }
    }

    #[test]
    fn reorder_grace_clamps_to_2_12_ms() {
        assert_eq!(reorder_grace_us_from_jitter(0.0), MIN_REORDER_GRACE_US);
        assert_eq!(reorder_grace_us_from_jitter(0.5), MIN_REORDER_GRACE_US);
        assert_eq!(reorder_grace_us_from_jitter(3.0), 6_000);
        assert_eq!(reorder_grace_us_from_jitter(100.0), MAX_REORDER_GRACE_US);
    }

    #[test]
    fn reorder_hold_rescues_late_packet_before_deadline() {
        let sink = FakeSink::new();
        let mut rtp = VideoRtp::new(960, 544);
        let mut pli = false;

        rtp.receive_into(&sink, pkt(1, 90000, false), &mut pli, 1_000);
        rtp.receive_into(&sink, pkt(3, 180000, true), &mut pli, 1_500);
        assert!(rtp.reorder_hold.is_some());
        assert!(rtp.reorder_deadline_us().is_some());
        assert_eq!(sink.submitted.borrow().len(), 0);

        let stats = rtp.receive_into(&sink, pkt(2, 90000, true), &mut pli, 2_000);
        assert_eq!(stats.reorder_rescued, 1);
        assert!(rtp.reorder_hold.is_none());
        assert_eq!(sink.submitted.borrow().len(), 2);
    }

    #[test]
    fn reorder_hold_expires_after_grace_and_flushes_ready() {
        let sink = FakeSink::new();
        let mut rtp = VideoRtp::new(960, 544);
        let mut pli = false;

        rtp.receive_into(&sink, pkt(1, 90000, false), &mut pli, 1_000);
        rtp.receive_into(&sink, pkt(3, 180000, true), &mut pli, 1_200);
        assert!(rtp.parked_au.is_some());

        let deadline = rtp.reorder_deadline_us().expect("reorder hold active");
        let stats = rtp.expire_reorder_grace_if_due(&sink, &mut pli, deadline);
        assert_eq!(stats.reorder_expired, 1);
        assert!(rtp.reorder_hold.is_none());
        assert_eq!(sink.submitted.borrow().len(), 1);
        assert!(pli);
    }

    #[test]
    fn third_timestamp_force_expires_before_parked_overwrite() {
        let sink = FakeSink::new();
        let mut rtp = VideoRtp::new(960, 544);
        let mut pli = false;

        rtp.receive_into(&sink, pkt(1, 90000, false), &mut pli, 1_000);
        rtp.receive_into(&sink, pkt(3, 180000, true), &mut pli, 1_200);
        assert!(rtp.reorder_hold.is_some());
        assert!(rtp.parked_au.is_some());
        assert!(rtp.pending.is_none());
        let parked_ts_before = rtp.parked_au.as_ref().unwrap().timestamp;
        assert_eq!(parked_ts_before, 180000);

        rtp.receive_into(&sink, pkt(5, 270000, false), &mut pli, 1_400);
        assert!(
            sink.submitted.borrow().iter().any(|au| !au.is_empty()),
            "parked AU B must be flushed on third-TS force expire"
        );
        assert!(
            rtp.parked_au
                .as_ref()
                .is_none_or(|parked| parked.timestamp != parked_ts_before),
            "parked B must not remain after third timestamp"
        );
    }

    #[test]
    fn queue_full_does_not_request_keyframe() {
        let sink = FakeSink::new();
        *sink.accept.borrow_mut() = false;
        let mut rtp = VideoRtp::new(960, 544);
        let mut pli = false;
        let stats = rtp.receive_into(&sink, pkt(1, 90000, true), &mut pli, 1_000);
        assert_eq!(stats.dropped, 1);
        assert!(!pli);
    }
}
