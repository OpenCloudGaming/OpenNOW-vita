//! H.264 RTP reassembly with loss-aware recovery, ported from green-vita's
//! `api/streaming/rtc/rtp.rs` (MPL-2.0, https://github.com/Day-OS/green-vita). See
//! THIRD_PARTY_NOTICES.md.
//!
//! Unlike the naive "extend the access unit as packets arrive, flush on the marker bit"
//! approach, this buffers every packet of a frame (keyed by RTP timestamp) and only
//! depacketizes once the whole run is present *and* sequence-contiguous starting from the
//! packet right after the previous frame's marker. That catches reordering, not just gaps -
//! and gaps that do slip through still get flagged as damage so a resync can be forced before
//! AVCDEC spends cycles decoding data it has no valid reference for.

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

#[derive(Default)]
pub struct VideoSampleStats {
    pub dropped: u32,
    pub source_frame_duration_us: Option<u64>,
    pub encoded_resolution: Option<(u32, u32)>,
    pub jitter_ms: f32,
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
        if let (Some(last_arrival), Some(last_rtp)) = (self.last_arrival_us, self.last_rtp_timestamp) {
            let arrival_diff_us = arrival_us.saturating_sub(last_arrival) as f64;
            let rtp_diff_us = (rtp_timestamp.wrapping_sub(last_rtp) as f64 * 1_000_000.0) / u64::from(VIDEO_RTP_CLOCK_RATE) as f64;
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

pub struct VideoRtp {
    depacketizer: H264Packet,
    pending: Option<PendingVideoFrame>,
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
    ) -> FrameAssembly {
        let Some(marker_sequence) = self.marker_sequence() else {
            return FrameAssembly::Pending;
        };
        let mut packets = self.packets.iter().collect::<Vec<_>>();
        packets.sort_unstable_by_key(|packet| {
            std::cmp::Reverse(marker_sequence.wrapping_sub(packet.header.sequence_number))
        });
        let Some(first) = packets.first() else {
            return FrameAssembly::Pending;
        };
        if expected_sequence.is_some_and(|expected| first.header.sequence_number != expected)
            || !depacketizer.is_partition_head(&first.payload)
        {
            return FrameAssembly::Pending;
        }
        if packets.windows(2).any(|pair| {
            pair[1].header.sequence_number != pair[0].header.sequence_number.wrapping_add(1)
        }) {
            return FrameAssembly::Pending;
        }

        *depacketizer = H264Packet::default();
        let mut data = Vec::new();
        for packet in packets {
            let Ok(nalu) = depacketizer.depacketize(&packet.payload) else {
                *depacketizer = H264Packet::default();
                return FrameAssembly::Invalid;
            };
            data.extend_from_slice(&nalu);
            if data.len() > MAX_H264_ACCESS_UNIT_BYTES {
                *depacketizer = H264Packet::default();
                return FrameAssembly::Invalid;
            }
        }
        *depacketizer = H264Packet::default();
        FrameAssembly::Complete {
            data,
            marker_sequence,
        }
    }
}

impl VideoRtp {
    pub fn new(decode_width: u32, decode_height: u32) -> Self {
        Self {
            depacketizer: H264Packet::default(),
            pending: None,
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

    pub fn receive(
        &mut self,
        worker: &VideoDecodeWorker,
        packet: Packet,
        keyframe_requested: &mut bool,
        arrival_us: u64,
    ) -> VideoSampleStats {
        let mut stats = VideoSampleStats::default();
        stats.jitter_ms = self.jitter_estimator.update(arrival_us, packet.header.timestamp);
        let mut frame_was_damaged = false;
        if packet.payload.is_empty() {
            if self.next_sequence == Some(packet.header.sequence_number) {
                self.next_sequence = Some(packet.header.sequence_number.wrapping_add(1));
            }
            return stats;
        }

        let packet_timestamp = packet.header.timestamp;
        if let Some(pending) = &self.pending
            && pending.timestamp != packet_timestamp
        {
            if !timestamp_is_newer(packet_timestamp, pending.timestamp) {
                return stats;
            }
            if let Some(incomplete) = self.pending.take() {
                self.next_sequence = incomplete
                    .marker_sequence()
                    .map(|sequence| sequence.wrapping_add(1));
                self.depacketizer = H264Packet::default();
                *keyframe_requested = true;
                self.record_damage(worker);
                frame_was_damaged = true;
                stats.dropped = stats.dropped.saturating_add(1);
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

        let assembly = self
            .pending
            .as_ref()
            .map(|pending| pending.assemble(&mut self.depacketizer, self.next_sequence));
        let Some(assembly) = assembly else {
            return stats;
        };
        let (data, marker_sequence) = match assembly {
            FrameAssembly::Pending => return stats,
            FrameAssembly::Invalid => {
                self.pending = None;
                self.next_sequence = None;
                *keyframe_requested = true;
                self.record_damage(worker);
                stats.dropped = stats.dropped.saturating_add(1);
                return stats;
            }
            FrameAssembly::Complete {
                data,
                marker_sequence,
            } => (data, marker_sequence),
        };
        let completed = self.pending.take().expect("assembled pending video frame");
        self.next_sequence = Some(marker_sequence.wrapping_add(1));
        stats.source_frame_duration_us = self.last_frame_timestamp.map(|previous| {
            u64::from(completed.timestamp.wrapping_sub(previous)) * 1_000_000
                / u64::from(VIDEO_RTP_CLOCK_RATE)
        });
        if let Some(duration) = stats.source_frame_duration_us {
            self.source_frame_duration_us = Some(
                self.source_frame_duration_us
                    .map(|average| (average * 7 + duration) / 8)
                    .unwrap_or(duration),
            );
        }
        self.last_frame_timestamp = Some(completed.timestamp);

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
                worker.begin_resync();
            }
            self.waiting_for_keyframe = true;
            stats.dropped = stats.dropped.saturating_add(1);
            return stats;
        }
        if self.stream_too_large {
            if unit.resolution.is_none() || !unit.has_idr {
                *keyframe_requested = true;
                self.waiting_for_keyframe = true;
                stats.dropped = stats.dropped.saturating_add(1);
                return stats;
            }
            self.stream_too_large = false;
        }
        if self.waiting_for_keyframe {
            if !unit.has_idr {
                *keyframe_requested = true;
                stats.dropped = stats.dropped.saturating_add(1);
                return stats;
            }
            self.waiting_for_keyframe = false;
            self.damage_score = 0;
        } else if !frame_was_damaged {
            self.damage_score = self.damage_score.saturating_sub(1);
        }

        // A damaged access unit is missing macroblocks the decoder cannot reconstruct. Handing it
        // over anyway does not just corrupt this frame: every following P-frame predicts from it,
        // so a smear in one region outlives the packet loss by seconds and stays anchored to
        // whatever was moving there. Holding the previous frame instead keeps the damage from
        // entering the reference chain at all - the visible cost is one stale frame rather than a
        // patch of the picture that stops updating.
        // Note this deliberately does *not* ask for a keyframe. Doing so per damaged frame turned
        // a steady trickle of loss into a keyframe storm: an IDR costs several times a P-frame,
        // so on a link already dropping packets the repair traffic crowds out the content and
        // causes the next loss. `record_damage` above owns that escalation and only spends a
        // keyframe once damage has actually accumulated past its fps-scaled threshold.
        if frame_was_damaged {
            stats.dropped = stats.dropped.saturating_add(1);
            return stats;
        }

        if !worker.submit_access_unit(data) {
            eprintln!("Video decoder queue is full; continuing while requesting a keyframe");
            *keyframe_requested = true;
            stats.dropped = stats.dropped.saturating_add(1);
        }
        stats
    }

    fn record_damage(&mut self, worker: &VideoDecodeWorker) {
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

        worker.begin_resync();
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
