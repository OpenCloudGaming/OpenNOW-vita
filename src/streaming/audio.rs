// Adapted from green-vita (MPL-2.0, https://github.com/Day-OS/green-vita)
// src/streaming/audio.rs - Opus decode + SDL audio queue playback. See THIRD_PARTY_NOTICES.md.
//
// The jitter buffer, RED recovery and gain stage below follow OpenNOW-Switch's `AudioPipeline`
// (app/src/stream/audio/), which solves the same two NVIDIA-specific problems this port has:
// NVST audio arrives out of order often enough to matter on a 2.4 GHz radio, and it arrives far
// quieter than a local GameStream host.

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use std::collections::VecDeque;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::{Duration, Instant};

pub const AUDIO_SAMPLE_RATE: i32 = 48_000;
const AUDIO_CHANNELS: usize = 2;

const AUDIO_BYTES_PER_SECOND: u32 = AUDIO_SAMPLE_RATE as u32 * AUDIO_CHANNELS as u32 * 2;
/// Ceiling that also walks the queue back down to the operating point.
///
/// Back to the value from `fed4671`, the build that sounded right. The device clock and the
/// server's audio clock differ, so the queue drifts upward on its own and something has to pull it
/// back: at green-vita's 240 ms it was measured sitting at 134 ms indefinitely. That commit
/// corrected the drift by clearing the whole queue here; this drops a single 10 ms frame instead,
/// which holds the same latency bound without the audible cut a full clear causes.
const MAX_QUEUED_AUDIO_BYTES: u32 = AUDIO_BYTES_PER_SECOND * 100 / 1_000;
/// Lower than it used to be (was 40 ms) because the jitter buffer now absorbs reordering upstream;
/// stacking both full-size buffers would just add latency for no extra smoothing.
/// The operating point: the queue settles here, so this *is* the audio latency.
///
/// The value from `fed4671`, the build that sounded right on hardware. It has to cover network
/// jitter on its own, because the jitter buffer upstream reorders but keeps no cushion - it drains
/// to empty on every pass.
const AUDIO_START_BUFFER_BYTES: u32 = AUDIO_BYTES_PER_SECOND * 40 / 1_000;

/// How long the queue must stay empty before playback is treated as genuinely over rather than
/// momentarily starved.
const STREAM_IDLE_PAUSE: Duration = Duration::from_millis(250);
/// Hard reset, kept well above the ceiling so it only fires when something is genuinely wrong -
/// ordinary drift is handled by dropping single frames at `MAX_QUEUED_AUDIO_BYTES`.
const AUDIO_RESYNC_BYTES: u32 = AUDIO_BYTES_PER_SECOND * 150 / 1_000;
const MAX_OPUS_FRAME_SAMPLES_PER_CHANNEL: usize = 5_760;
/// Hand-off to the decode thread, in 10 ms packets. The value from `fed4671`.
///
/// The worker drains this to empty on every pass, so packets do not accumulate here the way they
/// would in a queue feeding a real-time consumer.
const MAX_PENDING_OPUS_PACKETS: usize = 6;

/// NVIDIA advertises `ptime=10`, so one packet is 10 ms - 480 samples per channel.
const AUDIO_FRAME_SAMPLES_PER_CHANNEL: i32 = 480;
/// `a=rtpmap:63 red/48000/2` - RFC 2198 redundant coding, which NVST uses for audio FEC.
pub const RED_PAYLOAD_TYPE: u8 = 63;
/// Any non-RED type; only the comparison against `RED_PAYLOAD_TYPE` matters to the parser.
const OPUS_PAYLOAD_TYPE: u8 = 111;

/// How much audio the jitter buffer holds before releasing the first packet. Clamped the same way
/// OpenNOW-Switch's `AudioLatencyPolicy` clamps it, and for the same reason: under 30 ms stops
/// absorbing anything, over 100 ms is audible lag.
const TARGET_BUFFER_MS: u32 = 40;
const MAX_JITTER_PACKETS: usize = 12;
/// A gap this long means the transport stalled rather than a packet being late; the sequence
/// numbers on the far side of it are not worth waiting for.
const EPOCH_RESET_GAP: Duration = Duration::from_millis(500);

/// Peak ceiling the limiter aims for, leaving a little room below i16::MAX so that the rounding in
/// the gain multiply cannot clip.
const LIMITER_PEAK_CEILING: f32 = 30_000.0;
/// Fast attack, slow release: dropping the gain instantly prevents clipping, while easing it back
/// up keeps the level from audibly pumping between loud and quiet passages.
const GAIN_RELEASE_COEFFICIENT: f32 = 0.08;
/// Frames (per channel) of silence before the gain ramp starts, then the total ramp length. The
/// stream's first packets often decode to garbage; fading in hides that instead of clicking.
const FADE_SILENCE_FRAMES: u32 = 2_400;
const FADE_TOTAL_FRAMES: u32 = 12_000;

const OPUS_OK: i32 = 0;

/// One RTP audio packet, with the header fields the jitter buffer needs to order and repair the
/// stream. The old pipeline passed the payload alone, which made both impossible.
#[derive(Debug, Clone)]
pub struct AudioPacket {
    pub payload: Bytes,
    pub sequence: u16,
    pub payload_type: u8,
}

#[repr(C)]
struct OpusDecoderState {
    _private: [u8; 0],
}

#[link(name = "opus", kind = "static")]
unsafe extern "C" {
    fn opus_decoder_create(
        sample_rate: i32,
        channels: i32,
        error: *mut i32,
    ) -> *mut OpusDecoderState;
    fn opus_decode(
        decoder: *mut OpusDecoderState,
        data: *const u8,
        length: i32,
        pcm: *mut i16,
        frame_size: i32,
        decode_fec: i32,
    ) -> i32;
    fn opus_decoder_destroy(decoder: *mut OpusDecoderState);
}

struct NativeOpusDecoder {
    state: NonNull<OpusDecoderState>,
}

unsafe impl Send for NativeOpusDecoder {}

impl NativeOpusDecoder {
    fn new() -> Result<Self> {
        let mut error = OPUS_OK;
        // SAFETY: libopus initializes and exclusively owns the returned opaque decoder state.
        let state =
            unsafe { opus_decoder_create(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as i32, &mut error) };
        if error != OPUS_OK {
            if !state.is_null() {
                // SAFETY: a non-null state returned by libopus must be released with this function.
                unsafe { opus_decoder_destroy(state) };
            }
            bail!("libopus failed to create a decoder: error {error}");
        }
        let state = NonNull::new(state).context("libopus returned a null decoder")?;
        Ok(Self { state })
    }

    fn decode(&mut self, packet: &[u8], pcm: &mut [i16]) -> Result<usize> {
        let packet_len = i32::try_from(packet.len()).context("Opus packet is too large")?;
        // SAFETY: `state` is a live decoder, and `pcm` has room for the maximum Opus frame.
        let decoded = unsafe {
            opus_decode(
                self.state.as_ptr(),
                packet.as_ptr(),
                packet_len,
                pcm.as_mut_ptr(),
                MAX_OPUS_FRAME_SAMPLES_PER_CHANNEL as i32,
                0,
            )
        };
        if decoded < OPUS_OK {
            bail!("libopus decode error {decoded}");
        }
        Ok(decoded as usize)
    }

    /// Packet loss concealment: libopus interpolates a replacement frame when handed a null buffer,
    /// which sounds considerably better than the silence a naive gap-filler would insert. This is
    /// the same call upstream moonlight-common-c makes for its missing-packet placeholders.
    fn conceal(&mut self, pcm: &mut [i16]) -> Result<usize> {
        // SAFETY: libopus documents a null/zero-length input as the PLC path; `pcm` is sized for
        // one frame at `AUDIO_FRAME_SAMPLES_PER_CHANNEL`.
        let decoded = unsafe {
            opus_decode(
                self.state.as_ptr(),
                std::ptr::null(),
                0,
                pcm.as_mut_ptr(),
                AUDIO_FRAME_SAMPLES_PER_CHANNEL,
                0,
            )
        };
        if decoded < OPUS_OK {
            bail!("libopus concealment error {decoded}");
        }
        Ok(decoded as usize)
    }
}

impl Drop for NativeOpusDecoder {
    fn drop(&mut self) {
        // SAFETY: this is the sole owner and the state has not previously been destroyed.
        unsafe { opus_decoder_destroy(self.state.as_ptr()) };
    }
}

/// The primary (most recent) Opus payload carried by an RTP packet, unwrapped from its RED
/// envelope if it has one.
fn red_primary(data: &[u8], payload_type: u8) -> Option<&[u8]> {
    if data.is_empty() {
        return None;
    }
    if payload_type != RED_PAYLOAD_TYPE {
        return Some(data);
    }

    // RED block headers are 4 bytes each while the top bit is set, then a 1-byte header for the
    // primary block. The redundant payloads sit between the headers and the primary one.
    let mut header = 0usize;
    let mut redundant_bytes = 0usize;
    while header < data.len() && data[header] & 0x80 != 0 {
        if header + 4 > data.len() {
            return None;
        }
        redundant_bytes += usize::from(data[header + 2] & 0x03) << 8 | usize::from(data[header + 3]);
        header += 4;
    }
    if header >= data.len() {
        return None;
    }
    header += 1;
    if header + redundant_bytes >= data.len() {
        return None;
    }
    Some(&data[header + redundant_bytes..])
}

/// The first redundant payload in a RED packet - a copy of an *earlier* packet's audio. This is
/// what makes recovery possible: when packet N is lost, packet N+1 usually still carries it.
fn red_first_redundant(data: &[u8]) -> Option<&[u8]> {
    if data.len() < 5 || data[0] & 0x80 == 0 {
        return None;
    }
    let first_length = usize::from(data[2] & 0x03) << 8 | usize::from(data[3]);
    let mut header = 0usize;
    while header < data.len() && data[header] & 0x80 != 0 {
        if header + 4 > data.len() {
            return None;
        }
        header += 4;
    }
    if header >= data.len() || first_length == 0 {
        return None;
    }
    header += 1;
    if header + first_length > data.len() {
        return None;
    }
    Some(&data[header..header + first_length])
}

/// What the jitter buffer decided to do with the next slot in the sequence.
enum Release {
    /// Decode this payload. The payload type travels with it because it decides whether the bytes
    /// are RED-wrapped: assuming either way mangles the other.
    Payload { payload: Bytes, payload_type: u8 },
    /// The packet is gone and could not be recovered - run concealment for its slot.
    Conceal,
}

/// Reorders RTP audio by sequence number, recovers losses from RED redundancy, and reports
/// unrecoverable gaps so the decoder can conceal them.
struct JitterBuffer {
    packets: VecDeque<AudioPacket>,
    expected_sequence: u16,
    have_expected: bool,
    primed: bool,
    prime_packets: usize,
    initial_hold: Duration,
    resync_hold: Duration,
    hold_until: Option<Instant>,
    last_arrival: Option<Instant>,
    /// Losses repaired from RED redundancy, and gaps that had to be concealed. Plain counters
    /// rather than atomics so this stays pure logic the tests can drive directly; the worker
    /// publishes them.
    recovered_count: u64,
    concealed_count: u64,
    late_drops: u64,
}

impl JitterBuffer {
    fn new(target_buffer_ms: u32) -> Self {
        let target = target_buffer_ms.clamp(30, 100);
        Self {
            packets: VecDeque::with_capacity(MAX_JITTER_PACKETS),
            expected_sequence: 0,
            have_expected: false,
            primed: false,
            // One packet is 10 ms, so the target buffer converts straight to a packet count.
            prime_packets: (target / 10).max(1) as usize,
            initial_hold: Duration::from_millis(u64::from(target)),
            resync_hold: Duration::from_millis(u64::from(target / 2).max(30)),
            hold_until: None,
            last_arrival: None,
            recovered_count: 0,
            concealed_count: 0,
            late_drops: 0,
        }
    }

    fn reset(&mut self, hold: Duration, now: Instant) {
        self.packets.clear();
        self.primed = false;
        self.have_expected = false;
        self.hold_until = Some(now + hold);
    }

    fn push(&mut self, packet: AudioPacket) {
        let now = Instant::now();
        let stalled = self
            .last_arrival
            .is_some_and(|previous| now.duration_since(previous) > EPOCH_RESET_GAP);
        self.last_arrival = Some(now);

        // A huge forward jump means the sender restarted its sequence rather than that we are
        // missing thousands of packets, so waiting for the gap to fill would stall forever.
        let jumped = self.have_expected
            && packet.sequence.wrapping_sub(self.expected_sequence) as i16
                > (MAX_JITTER_PACKETS * 4) as i16;
        if stalled || jumped {
            self.reset(self.resync_hold, now);
        }

        if self.hold_until.is_none() && !self.primed {
            self.hold_until = Some(now + self.initial_hold);
        }

        // Already played past this one - decoding it now would put it out of order.
        if self.have_expected
            && (packet.sequence.wrapping_sub(self.expected_sequence) as i16) < 0
        {
            self.late_drops += 1;
            return;
        }
        if self.packets.iter().any(|held| held.sequence == packet.sequence) {
            return;
        }
        if self.packets.len() >= MAX_JITTER_PACKETS {
            self.packets.pop_front();
        }
        let position = self
            .packets
            .iter()
            .position(|held| (packet.sequence.wrapping_sub(held.sequence) as i16) < 0)
            .unwrap_or(self.packets.len());
        self.packets.insert(position, packet);
    }

    fn next(&mut self) -> Option<Release> {
        if self.hold_until.is_some_and(|until| Instant::now() < until) {
            return None;
        }
        if !self.primed {
            if self.packets.len() < self.prime_packets {
                return None;
            }
            self.primed = true;
            self.hold_until = None;
            self.expected_sequence = self.packets.front()?.sequence;
            self.have_expected = true;
        }
        if self.packets.is_empty() {
            return None;
        }

        if let Some(index) = self
            .packets
            .iter()
            .position(|held| held.sequence == self.expected_sequence)
        {
            let packet = self.packets.remove(index)?;
            self.expected_sequence = self.expected_sequence.wrapping_add(1);
            return Some(Release::Payload {
                payload: packet.payload,
                payload_type: packet.payload_type,
            });
        }

        // The expected packet is missing. If the next one is RED, it carries a copy of exactly
        // this payload, so the loss is repairable without waiting for a retransmit.
        let successor = self.expected_sequence.wrapping_add(1);
        if let Some(next) = self
            .packets
            .iter()
            .find(|held| held.sequence == successor && held.payload_type == RED_PAYLOAD_TYPE)
            && let Some(recovered) = red_first_redundant(&next.payload)
        {
            let recovered = Bytes::copy_from_slice(recovered);
            self.expected_sequence = self.expected_sequence.wrapping_add(1);
            self.recovered_count += 1;
            return Some(Release::Payload {
                payload: recovered,
                // Already unwrapped from its RED envelope - it is bare Opus now, and running it
                // through the RED parser again would eat the front of the frame.
                payload_type: OPUS_PAYLOAD_TYPE,
            });
        }

        self.expected_sequence = self.expected_sequence.wrapping_add(1);
        self.concealed_count += 1;
        Some(Release::Conceal)
    }
}

/// Gain above unity with a peak limiter, plus a startup fade.
///
/// A flat multiply would clip the loud passages at the factors this needs (GFN's level is quiet
/// enough to want 12x), so the requested gain is really a ceiling: the limiter backs it off to
/// whatever keeps the current frame's peak under `LIMITER_PEAK_CEILING`.
struct GainStage {
    requested: f32,
    applied: f32,
    fade_frames: u32,
}

impl GainStage {
    fn new(percent: u16) -> Self {
        Self {
            requested: f32::from(percent) / 100.0,
            // Starts silent so the fade ramps up from nothing rather than clicking in.
            applied: 0.0,
            fade_frames: 0,
        }
    }

    fn reset(&mut self) {
        self.applied = 0.0;
        self.fade_frames = 0;
    }

    fn apply(&mut self, pcm: &mut [i16]) {
        if pcm.is_empty() {
            return;
        }
        if self.requested <= 1.0 && self.fade_frames >= FADE_TOTAL_FRAMES {
            return;
        }

        let peak = pcm
            .iter()
            .map(|sample| i32::from(*sample).abs())
            .max()
            .unwrap_or(0);
        let safe_gain = if peak > 0 && self.requested > 1.0 {
            self.requested.min(LIMITER_PEAK_CEILING / peak as f32)
        } else {
            self.requested
        };
        if safe_gain < self.applied {
            self.applied = safe_gain;
        } else {
            self.applied += (safe_gain - self.applied) * GAIN_RELEASE_COEFFICIENT;
        }
        self.applied = self.applied.clamp(0.0, self.requested);

        for (index, sample) in pcm.iter_mut().enumerate() {
            let fade = if self.fade_frames < FADE_TOTAL_FRAMES {
                let frame = self.fade_frames + (index / AUDIO_CHANNELS) as u32;
                if frame < FADE_SILENCE_FRAMES {
                    0.0
                } else {
                    ((frame - FADE_SILENCE_FRAMES) as f32
                        / (FADE_TOTAL_FRAMES - FADE_SILENCE_FRAMES) as f32)
                        .min(1.0)
                }
            } else {
                1.0
            };
            let amplified = (f32::from(*sample) * self.applied * fade).round();
            *sample = amplified.clamp(-32_768.0, 32_767.0) as i16;
        }

        self.fade_frames = (self.fade_frames + (pcm.len() / AUDIO_CHANNELS) as u32)
            .min(FADE_TOTAL_FRAMES);
    }
}


/// Live audio counters, for the on-screen readout.
///
/// Every audio problem so far was diagnosed by guesswork because none of this was measurable. The
/// number that matters most is `underruns`: it distinguishes "the queue ran dry" (a pacing
/// problem, ours) from "packets never arrived" (a network problem, not ours).
#[derive(Default)]
struct AudioStats {
    packets_rx: AtomicU64,
    decoded: AtomicU64,
    decode_errors: AtomicU64,
    concealed: AtomicU64,
    red_recovered: AtomicU64,
    late_drops: AtomicU64,
    /// Dropped because the SDL queue was full.
    queue_drops: AtomicU64,
    /// Dropped at the hand-off to the decode thread. Distinct from `queue_drops`: this one means
    /// the decoder fell behind, that one means playback did.
    handoff_drops: AtomicU64,
    /// Times the SDL queue hit empty while playback was running.
    underruns: AtomicU64,
    /// Current depth of the SDL queue, in milliseconds of audio.
    queued_ms: AtomicU32,
    applied_gain_x100: AtomicU32,
}

static STATS: AudioStats = AudioStats {
    packets_rx: AtomicU64::new(0),
    decoded: AtomicU64::new(0),
    decode_errors: AtomicU64::new(0),
    concealed: AtomicU64::new(0),
    red_recovered: AtomicU64::new(0),
    late_drops: AtomicU64::new(0),
    queue_drops: AtomicU64::new(0),
    handoff_drops: AtomicU64::new(0),
    underruns: AtomicU64::new(0),
    queued_ms: AtomicU32::new(0),
    applied_gain_x100: AtomicU32::new(0),
};

fn reset_stats() {
    STATS.packets_rx.store(0, Ordering::Relaxed);
    STATS.decoded.store(0, Ordering::Relaxed);
    STATS.decode_errors.store(0, Ordering::Relaxed);
    STATS.concealed.store(0, Ordering::Relaxed);
    STATS.red_recovered.store(0, Ordering::Relaxed);
    STATS.late_drops.store(0, Ordering::Relaxed);
    STATS.queue_drops.store(0, Ordering::Relaxed);
    STATS.handoff_drops.store(0, Ordering::Relaxed);
    STATS.underruns.store(0, Ordering::Relaxed);
    STATS.queued_ms.store(0, Ordering::Relaxed);
    STATS.applied_gain_x100.store(0, Ordering::Relaxed);
}

/// One line of audio diagnostics for the streaming overlay.
pub fn stats_line() -> String {
    format!(
        "aud q:{}ms und:{} rx:{} dec:{} plc:{} red:{} late:{} drop:{}/{} err:{} gain:{:.2}x",
        STATS.queued_ms.load(Ordering::Relaxed),
        STATS.underruns.load(Ordering::Relaxed),
        STATS.packets_rx.load(Ordering::Relaxed),
        STATS.decoded.load(Ordering::Relaxed),
        STATS.concealed.load(Ordering::Relaxed),
        STATS.red_recovered.load(Ordering::Relaxed),
        STATS.late_drops.load(Ordering::Relaxed),
        STATS.queue_drops.load(Ordering::Relaxed),
        STATS.handoff_drops.load(Ordering::Relaxed),
        STATS.decode_errors.load(Ordering::Relaxed),
        f64::from(STATS.applied_gain_x100.load(Ordering::Relaxed)) / 100.0,
    )
}


/// The live decode worker's inbox, published so the peer thread can hand packets straight over.
///
/// Audio used to reach the worker via the render loop: the peer parked packets in a `Vec` and the
/// shell drained it once per *video* frame. At 30 fps that delivered 33 ms of audio in a burst
/// every 33 ms, so the device queue swung between overflowing and running dry - measured as 75
/// underruns and 282 drops in the same session. Handing over on arrival keeps the 10 ms packet
/// cadence the network already provides.
static WORKER_INBOX: Mutex<Option<SyncSender<AudioPacket>>> = Mutex::new(None);

/// Hands one freshly-received RTP audio packet to the decode thread. Safe to call from any thread,
/// and a no-op when no session is running.
pub fn submit_packet(packet: AudioPacket) {
    let Ok(inbox) = WORKER_INBOX.lock() else {
        return;
    };
    let Some(sender) = inbox.as_ref() else {
        return;
    };
    STATS.packets_rx.fetch_add(1, Ordering::Relaxed);
    if sender.try_send(packet).is_err() {
        STATS.handoff_drops.fetch_add(1, Ordering::Relaxed);
    }
}

fn publish_inbox(sender: Option<SyncSender<AudioPacket>>) {
    if let Ok(mut inbox) = WORKER_INBOX.lock() {
        *inbox = sender;
    }
}

/// The SDL playback device, owned by the decode thread rather than the render loop.
///
/// `sdl2::audio::AudioQueue` cannot be used here: it is not `Send` (it holds an `AudioSubsystem`)
/// and it keeps its device id private. `SDL_QueueAudio` *is* thread-safe in C and the device id is
/// a plain integer, so the device is opened through the raw bindings and the id handed to the
/// worker.
#[derive(Clone, Copy)]
struct AudioDevice {
    id: sdl2::sys::SDL_AudioDeviceID,
}

// SAFETY: the only thing carried across threads is an integer device id. Every SDL call made with
// it (`SDL_QueueAudio`, `SDL_GetQueuedAudioSize`, `SDL_ClearQueuedAudio`, `SDL_PauseAudioDevice`)
// is documented thread-safe; SDL locks the device internally.
unsafe impl Send for AudioDevice {}

impl AudioDevice {
    fn open() -> Result<Self> {
        let desired = sdl2::sys::SDL_AudioSpec {
            freq: AUDIO_SAMPLE_RATE,
            format: sdl2::sys::AUDIO_S16LSB as sdl2::sys::SDL_AudioFormat,
            channels: AUDIO_CHANNELS as u8,
            silence: 0,
            // green-vita's value. A smaller buffer was tried to cut the device's own granularity,
            // but with an 80 ms queue in front of it there is nothing left for it to fix, and the
            // extra wakeups are not worth spending on a machine this small.
            samples: 1024,
            padding: 0,
            size: 0,
            // No callback: this is a queued device, fed by `SDL_QueueAudio`.
            callback: None,
            userdata: std::ptr::null_mut(),
        };
        let mut obtained = desired;
        // SAFETY: both specs are valid for the duration of the call, and the audio subsystem is
        // already initialized by the caller.
        let id = unsafe {
            sdl2::sys::SDL_OpenAudioDevice(
                std::ptr::null(),
                0,
                &desired,
                &mut obtained,
                0,
            )
        };
        if id == 0 {
            bail!("SDL could not open an audio device");
        }
        if obtained.freq != AUDIO_SAMPLE_RATE || obtained.channels != AUDIO_CHANNELS as u8 {
            eprintln!(
                "SDL audio opened as {} Hz / {} channel(s), requested {} Hz / {} channel(s)",
                obtained.freq, obtained.channels, AUDIO_SAMPLE_RATE, AUDIO_CHANNELS
            );
        }
        Ok(Self { id })
    }

    /// Bytes currently waiting to be played.
    fn queued_bytes(&self) -> u32 {
        // SAFETY: `id` names a device this type opened and has not closed.
        unsafe { sdl2::sys::SDL_GetQueuedAudioSize(self.id) }
    }

    fn queue(&self, samples: &[i16]) {
        // SAFETY: the pointer and length describe `samples`, which outlives the call.
        let result = unsafe {
            sdl2::sys::SDL_QueueAudio(
                self.id,
                samples.as_ptr().cast(),
                std::mem::size_of_val(samples) as u32,
            )
        };
        if result != 0 {
            eprintln!("Failed to queue SDL audio");
        }
    }

    fn set_paused(&self, paused: bool) {
        // SAFETY: as above.
        unsafe { sdl2::sys::SDL_PauseAudioDevice(self.id, i32::from(paused)) };
    }

    fn clear(&self) {
        // SAFETY: as above.
        unsafe { sdl2::sys::SDL_ClearQueuedAudio(self.id) };
    }

    fn close(&self) {
        // SAFETY: as above; called once, from `AudioRenderer::drop`.
        unsafe { sdl2::sys::SDL_CloseAudioDevice(self.id) };
    }
}

/// Owns the SDL playback device and a dedicated Opus decode thread.
///
/// The decode thread queues PCM into the device itself. It used to hand PCM back here to be pumped
/// once per video frame, which coupled audio to the frame rate: at 30 fps the queue was refilled
/// every 33 ms while the device drained it continuously, so it alternated between overflowing on
/// refill and running dry before the next one.
pub struct AudioRenderer {
    /// Kept alive so SDL does not shut the audio subsystem down under the device.
    _audio: sdl2::AudioSubsystem,
    device: AudioDevice,
}

impl AudioRenderer {
    pub fn new(audio: &sdl2::AudioSubsystem) -> Result<Self> {
        let device = AudioDevice::open()?;
        publish_inbox(Some(spawn_decode_worker(device)?));

        Ok(Self {
            _audio: audio.clone(),
            device,
        })
    }
}

impl Drop for AudioRenderer {
    fn drop(&mut self) {
        publish_inbox(None);
        self.device.set_paused(true);
        self.device.close();
    }
}

fn spawn_decode_worker(device: AudioDevice) -> Result<SyncSender<AudioPacket>> {
    let (packets_tx, packets_rx) = sync_channel::<AudioPacket>(MAX_PENDING_OPUS_PACKETS);

    let mut decoder = NativeOpusDecoder::new().context("failed to create Opus decoder")?;
    let mut gain = GainStage::new(crate::gfn::stream_prefs::audio_boost().percent());
    reset_stats();

    std::thread::Builder::new()
        .name("opennow-vita-audio-decode".to_owned())
        .spawn(move || {
            crate::thread_affinity::pin_current_thread(
                crate::thread_affinity::VitaCore::Media,
                "audio decode",
            );
            let mut jitter = JitterBuffer::new(TARGET_BUFFER_MS);
            let mut started = false;
            let mut empty_since: Option<Instant> = None;
            let mut decode_buf = vec![0i16; MAX_OPUS_FRAME_SAMPLES_PER_CHANNEL * AUDIO_CHANNELS];
            // Long enough to notice a stalled stream, short enough that the jitter buffer's hold
            // deadline is honoured with roughly one packet of slack.
            let poll_interval = Duration::from_millis(12);

            loop {
                match packets_rx.recv_timeout(poll_interval) {
                    Ok(packet) => jitter.push(packet),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
                // Anything else already waiting goes in before we release, so a burst is
                // reordered as a group rather than one packet per poll.
                while let Ok(packet) = packets_rx.try_recv() {
                    jitter.push(packet);
                }

                let queued = device.queued_bytes();
                STATS
                    .queued_ms
                    .store(queued / (AUDIO_BYTES_PER_SECOND / 1_000), Ordering::Relaxed);
                if queued == 0 {
                    if started && empty_since.is_none() {
                        // Counted once per dry spell, so the number reads as "times the audio ran
                        // out" rather than "polls spent waiting".
                        STATS.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    let since = *empty_since.get_or_insert_with(Instant::now);
                    if started && since.elapsed() >= STREAM_IDLE_PAUSE {
                        device.set_paused(true);
                        started = false;
                    }
                } else {
                    empty_since = None;
                }

                while let Some(release) = jitter.next() {
                    let samples_per_channel = match release {
                        Release::Payload {
                            payload,
                            payload_type,
                        } => {
                            match red_primary(&payload, payload_type)
                                .filter(|primary| !primary.is_empty())
                            {
                                Some(primary) => match decoder.decode(primary, &mut decode_buf) {
                                    Ok(samples) => samples,
                                    Err(error) => {
                                        eprintln!("Failed to decode Opus audio packet: {error}");
                                        STATS.decode_errors.fetch_add(1, Ordering::Relaxed);
                                        decoder.conceal(&mut decode_buf).unwrap_or(0)
                                    }
                                },
                                None => decoder.conceal(&mut decode_buf).unwrap_or(0),
                            }
                        }
                        Release::Conceal => match decoder.conceal(&mut decode_buf) {
                            Ok(samples) => samples,
                            Err(error) => {
                                eprintln!("Opus concealment failed: {error}");
                                0
                            }
                        },
                    };
                    if samples_per_channel == 0 {
                        continue;
                    }

                    let sample_count = samples_per_channel * AUDIO_CHANNELS;
                    let frame = &mut decode_buf[..sample_count];
                    gain.apply(frame);
                    STATS.decoded.fetch_add(1, Ordering::Relaxed);
                    STATS
                        .applied_gain_x100
                        .store((gain.applied * 100.0) as u32, Ordering::Relaxed);
                    // Republished wholesale rather than incremented: the jitter buffer owns these
                    // as running totals, so a store keeps the two in step without double counting.
                    STATS
                        .red_recovered
                        .store(jitter.recovered_count, Ordering::Relaxed);
                    STATS.concealed.store(jitter.concealed_count, Ordering::Relaxed);
                    STATS.late_drops.store(jitter.late_drops, Ordering::Relaxed);

                    let queued = device.queued_bytes();
                    let frame_bytes = std::mem::size_of_val(frame) as u32;
                    if queued >= AUDIO_RESYNC_BYTES {
                        // Genuinely drifting: nothing short of a reset brings latency back.
                        device.set_paused(true);
                        device.clear();
                        started = false;
                    } else if queued.saturating_add(frame_bytes) > MAX_QUEUED_AUDIO_BYTES {
                        // A burst. Dropping this frame keeps playback continuous, where clearing
                        // would throw away good buffered audio and force a re-buffer.
                        STATS.queue_drops.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    device.queue(frame);
                    if !started && device.queued_bytes() >= AUDIO_START_BUFFER_BYTES {
                        device.set_paused(false);
                        started = true;
                    }
                }
            }
            gain.reset();
        })
        .context("failed to spawn audio decode worker")?;

    Ok(packets_tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A RED packet: one 4-byte redundant block header (top bit set) describing a 3-byte redundant
    /// payload, then the 1-byte primary header, then both payloads.
    fn red_packet() -> Vec<u8> {
        let mut packet = vec![0x80 | 111, 0x00, 0x00, 0x03];
        packet.push(111);
        packet.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        packet.extend_from_slice(&[0x11, 0x22]);
        packet
    }

    #[test]
    fn red_primary_skips_redundant_blocks() {
        let packet = red_packet();
        assert_eq!(
            red_primary(&packet, RED_PAYLOAD_TYPE),
            Some(&[0x11u8, 0x22][..])
        );
    }

    #[test]
    fn red_primary_passes_plain_opus_through() {
        let packet = [0x11u8, 0x22, 0x33];
        assert_eq!(red_primary(&packet, 111), Some(&packet[..]));
    }

    #[test]
    fn red_first_redundant_recovers_previous_payload() {
        let packet = red_packet();
        assert_eq!(
            red_first_redundant(&packet),
            Some(&[0xAAu8, 0xBB, 0xCC][..])
        );
    }

    #[test]
    fn red_helpers_reject_truncated_packets() {
        assert_eq!(red_primary(&[0x80, 0x00], RED_PAYLOAD_TYPE), None);
        assert_eq!(red_first_redundant(&[0x80, 0x00]), None);
        assert_eq!(red_first_redundant(&[0x01, 0x02, 0x03, 0x04, 0x05]), None);
    }

    fn packet(sequence: u16, payload: &[u8], payload_type: u8) -> AudioPacket {
        AudioPacket {
            payload: Bytes::copy_from_slice(payload),
            sequence,
            payload_type,
        }
    }

    fn primed_buffer() -> JitterBuffer {
        // A 30 ms target primes on 3 packets, keeping these tests short.
        JitterBuffer::new(30)
    }

    fn release_payload(release: Option<Release>) -> Option<Bytes> {
        match release {
            Some(Release::Payload { payload, .. }) => Some(payload),
            _ => None,
        }
    }

    fn release_type(release: Option<Release>) -> Option<u8> {
        match release {
            Some(Release::Payload { payload_type, .. }) => Some(payload_type),
            _ => None,
        }
    }

    #[test]
    fn buffer_holds_until_primed() {
        let mut buffer = primed_buffer();
        buffer.push(packet(1, &[1], 111));
        buffer.hold_until = None;
        assert!(buffer.next().is_none(), "released before priming");
        buffer.push(packet(2, &[2], 111));
        buffer.push(packet(3, &[3], 111));
        buffer.hold_until = None;
        assert_eq!(release_payload(buffer.next()).as_deref(), Some(&[1u8][..]));
    }

    #[test]
    fn buffer_reorders_by_sequence() {
        let mut buffer = primed_buffer();
        buffer.push(packet(3, &[3], 111));
        buffer.push(packet(1, &[1], 111));
        buffer.push(packet(2, &[2], 111));
        buffer.hold_until = None;

        let released: Vec<u8> = std::iter::from_fn(|| release_payload(buffer.next()))
            .map(|payload| payload[0])
            .collect();
        assert_eq!(released, vec![1, 2, 3]);
    }

    #[test]
    fn buffer_recovers_loss_from_red_redundancy() {
        let mut buffer = primed_buffer();
        buffer.push(packet(10, &[10], 111));
        // 11 never arrives; 12 is RED and carries a copy of it.
        buffer.push(packet(12, &red_packet(), RED_PAYLOAD_TYPE));
        buffer.push(packet(13, &[13], 111));
        buffer.hold_until = None;

        assert_eq!(release_payload(buffer.next()).as_deref(), Some(&[10u8][..]));
        assert_eq!(
            release_payload(buffer.next()).as_deref(),
            Some(&[0xAAu8, 0xBB, 0xCC][..]),
            "lost packet was not recovered from RED redundancy"
        );
    }

    /// Regression: the payload type has to survive the jitter buffer. Releasing it without one
    /// forced the caller to guess, and guessing RED for a plain Opus packet made the parser walk
    /// headers that were not there and hand back a mangled frame - every packet, all session.
    #[test]
    fn released_packets_keep_their_payload_type() {
        let mut buffer = primed_buffer();
        buffer.push(packet(1, &[1], 111));
        buffer.push(packet(2, &red_packet(), RED_PAYLOAD_TYPE));
        buffer.push(packet(3, &[3], 111));
        buffer.hold_until = None;

        assert_eq!(release_type(buffer.next()), Some(111), "plain Opus stayed plain");
        assert_eq!(
            release_type(buffer.next()),
            Some(RED_PAYLOAD_TYPE),
            "a RED packet must be reported as RED so its envelope gets unwrapped"
        );
    }

    /// A payload recovered *out of* a RED envelope is already bare Opus. Re-parsing it as RED
    /// would eat the front of the frame.
    #[test]
    fn red_recovered_payloads_are_reported_as_plain_opus() {
        let mut buffer = primed_buffer();
        buffer.push(packet(10, &[10], 111));
        buffer.push(packet(12, &red_packet(), RED_PAYLOAD_TYPE));
        buffer.push(packet(13, &[13], 111));
        buffer.hold_until = None;

        buffer.next();
        let recovered = buffer.next();
        assert_eq!(
            release_type(recovered),
            Some(OPUS_PAYLOAD_TYPE),
            "recovered redundancy must not be run through the RED parser twice"
        );
    }

    /// The end-to-end shape of the bug: what the decoder actually receives for each packet type.
    #[test]
    fn plain_opus_survives_the_red_unwrap_step() {
        let opus = [0x11u8, 0x22, 0x33];
        assert_eq!(
            red_primary(&opus, 111),
            Some(&opus[..]),
            "plain Opus must pass through untouched"
        );
        assert_ne!(
            red_primary(&opus, RED_PAYLOAD_TYPE),
            Some(&opus[..]),
            "guessing RED for plain Opus corrupts it - this is what made audio unintelligible"
        );
    }

    #[test]
    fn buffer_conceals_unrecoverable_gap() {
        let mut buffer = primed_buffer();
        buffer.push(packet(20, &[20], 111));
        // 21 is lost and 22 is plain Opus, so there is no redundant copy to fall back on.
        buffer.push(packet(22, &[22], 111));
        buffer.push(packet(23, &[23], 111));
        buffer.hold_until = None;

        assert_eq!(release_payload(buffer.next()).as_deref(), Some(&[20u8][..]));
        assert!(
            matches!(buffer.next(), Some(Release::Conceal)),
            "gap should have asked for concealment"
        );
        assert_eq!(release_payload(buffer.next()).as_deref(), Some(&[22u8][..]));
    }

    #[test]
    fn buffer_drops_packets_already_played() {
        let mut buffer = primed_buffer();
        buffer.push(packet(5, &[5], 111));
        buffer.push(packet(6, &[6], 111));
        buffer.push(packet(7, &[7], 111));
        buffer.hold_until = None;
        assert_eq!(release_payload(buffer.next()).as_deref(), Some(&[5u8][..]));

        buffer.push(packet(4, &[4], 111));
        assert_eq!(
            release_payload(buffer.next()).as_deref(),
            Some(&[6u8][..]),
            "a late packet was played out of order"
        );
    }

    #[test]
    fn gain_limits_peaks_instead_of_clipping() {
        let mut gain = GainStage::new(1_200);
        gain.fade_frames = FADE_TOTAL_FRAMES;
        // Loud input: a flat 12x would wrap well past i16::MAX.
        let mut pcm = vec![20_000i16; AUDIO_CHANNELS * 8];
        gain.apply(&mut pcm);
        assert!(
            pcm.iter().all(|sample| sample.abs() <= 32_767),
            "limiter let the signal clip"
        );
        assert!(
            gain.applied < 12.0,
            "limiter should have backed the gain off, got {}",
            gain.applied
        );
    }

    #[test]
    fn gain_amplifies_quiet_audio() {
        let mut gain = GainStage::new(1_200);
        gain.fade_frames = FADE_TOTAL_FRAMES;
        // Quiet input, run for a while so the slow release reaches full gain.
        let mut last_peak = 0;
        for _ in 0..200 {
            let mut pcm = vec![200i16; AUDIO_CHANNELS * 8];
            gain.apply(&mut pcm);
            last_peak = pcm.iter().map(|sample| sample.abs()).max().unwrap_or(0);
        }
        assert!(
            last_peak > 1_000,
            "quiet audio was not amplified, peak {last_peak}"
        );
    }

    #[test]
    fn unity_gain_leaves_samples_untouched() {
        let mut gain = GainStage::new(100);
        gain.fade_frames = FADE_TOTAL_FRAMES;
        let mut pcm = vec![1_234i16; AUDIO_CHANNELS * 4];
        gain.apply(&mut pcm);
        assert!(pcm.iter().all(|sample| *sample == 1_234));
    }
}
