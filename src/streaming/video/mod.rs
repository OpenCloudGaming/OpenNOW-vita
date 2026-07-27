// Adapted from green-vita (MPL-2.0, https://github.com/Day-OS/green-vita)
// src/streaming/video/mod.rs - the decoder/render-thread synchronization for the direct
// video-texture path, plus the hardware decoder pieces in the submodules.
// See THIRD_PARTY_NOTICES.md.

mod decoder;
#[cfg(target_os = "vita")]
mod memory;
mod worker;

#[cfg(target_os = "vita")]
pub use memory::reserve_decoder_cdram;
pub use worker::VideoDecodeWorker;

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// How many reference frames the Vita's hardware decoder is initialized to retain.
///
/// This is a *contract with the server*, not a local tuning knob: `gfn::sdp` sends the same value
/// as `a=video.maxNumReferenceFrames`, so GFN's encoder never emits a P-frame referencing a
/// picture the decoder has already dropped. Lowering it without lowering the SDP attribute (as
/// this originally did with `1`) makes `sceAvcdecDecode` silently consume frames and return no
/// picture on real hardware. 4 matches OpenNOW, the working GFN reference client.
///
/// Each extra reference costs roughly one 720p YUV420 frame of CDRAM (~1.4 MB); the startup
/// reservation in `memory.rs` starts at 48 MB, so there is ample headroom.
pub const AVCDEC_NUM_REF_FRAMES: u32 = 4;

/// Pixel format negotiated between the render thread (which knows what SDL texture formats
/// the platform supports) and the decoder (which asks sceAvcdec for matching output).
/// Bgr565 is the default: it is the decode contract proven on real Vita hardware. Iyuv exists
/// only as a fallback for Vita3K's HLE AVCDEC, whose YUV420 output path is emulator-only - on
/// real hardware it produced black frames, so the surface tries Bgr565 first and records its
/// choice here for the decoder to follow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoPixelFormat {
    Bgr565,
    Iyuv,
}

// Let a short render hitch absorb at most two source-frame intervals. Together with the frame
// already pending for presentation, this caps the microbuffer at three frames.
//
// Sized against the 60 fps profile in `cloudmatch::StreamSettings::for_vita` (~16.7ms each):
// leaving this at the old 30 fps figure (67ms) would let a hitch stack up four frames of
// latency instead of two, turning jitter into steady input lag.
const MAX_PENDING_TEXTURE_WAIT: Duration = Duration::from_millis(34);

/// One SDL streaming texture's writable memory, registered by the shell (`shell::surface`).
/// The pointer is stored as an integer so the platform-specific unsafe boundary stays in the
/// code that registers and consumes the textures.
#[derive(Clone, Copy)]
pub struct VideoTextureTarget {
    pub ptr: usize,
    pub pitch: u32,
    pub capacity: u32,
}

struct DirectVideoOutputState {
    targets: Option<[VideoTextureTarget; 2]>,
    displayed: Option<usize>,
    pending: Option<(usize, u64)>,
    next_generation: u64,
}

/// Synchronizes the frame-producer thread with the two SDL/GXM textures owned by the render
/// thread. The producer writes pixels straight into the texture memory - nothing video-sized
/// is ever allocated per frame, which is what keeps the Vita inside its VRAM budget.
pub struct DirectVideoOutput {
    state: Mutex<DirectVideoOutputState>,
    frame_displayed: Condvar,
    pub decoder_ready: AtomicBool,
    /// 0 = not yet registered, 1 = Bgr565, 2 = Iyuv. Set by the render thread together with
    /// `set_targets`; read by the decode thread on every frame.
    pixel_format: AtomicU8,
    /// Set by the decode thread when Bgr565 keeps decoding "successfully" but the output is
    /// suspiciously blank - Vita3K's HLE AVCDEC silently zeroes RGB565 output instead of
    /// erroring (see `worker::BLANK_FRAME_FALLBACK_STREAK`), so a decode error is not a
    /// reliable signal there. The render thread polls this once per frame and, if set, forces
    /// the Iyuv fallback instead of retrying Bgr565 (which real hardware needs and proved
    /// correct, so this must never fire spuriously there).
    format_fallback_requested: AtomicBool,
    pub width: u32,
    pub height: u32,
}

impl DirectVideoOutput {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            state: Mutex::new(DirectVideoOutputState {
                targets: None,
                displayed: None,
                pending: None,
                next_generation: 0,
            }),
            frame_displayed: Condvar::new(),
            decoder_ready: AtomicBool::new(false),
            pixel_format: AtomicU8::new(0),
            format_fallback_requested: AtomicBool::new(false),
            width,
            height,
        }
    }

    /// Called by the decode thread once it's confident Bgr565 isn't actually working here
    /// (see `worker::BLANK_FRAME_FALLBACK_STREAK`).
    pub fn request_format_fallback(&self) {
        self.format_fallback_requested.store(true, Ordering::Release);
    }

    /// Polled once per frame by the render thread; clears the flag on read.
    pub fn take_format_fallback_request(&self) -> bool {
        self.format_fallback_requested.swap(false, Ordering::AcqRel)
    }

    pub fn set_pixel_format(&self, format: VideoPixelFormat) {
        let value = match format {
            VideoPixelFormat::Bgr565 => 1,
            VideoPixelFormat::Iyuv => 2,
        };
        self.pixel_format.store(value, Ordering::Release);
    }

    pub fn pixel_format(&self) -> Option<VideoPixelFormat> {
        match self.pixel_format.load(Ordering::Acquire) {
            1 => Some(VideoPixelFormat::Bgr565),
            2 => Some(VideoPixelFormat::Iyuv),
            _ => None,
        }
    }

    pub fn set_targets(&self, targets: [VideoTextureTarget; 2]) {
        if let Ok(mut state) = self.state.lock() {
            state.targets = Some(targets);
            state.displayed = None;
            state.pending = None;
        }
    }

    pub fn clear_targets(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.targets = None;
            state.displayed = None;
            state.pending = None;
        }
        self.frame_displayed.notify_all();
    }

    pub fn mark_displayed(&self, index: usize, generation: u64) {
        let mut cleared_pending = false;
        if let Ok(mut state) = self.state.lock() {
            state.displayed = Some(index);
            if state.pending == Some((index, generation)) {
                state.pending = None;
                cleared_pending = true;
            }
        }
        if cleared_pending {
            self.frame_displayed.notify_one();
        }
    }

    /// Blocks (bounded by `MAX_PENDING_TEXTURE_WAIT`) until a texture is free to write into.
    /// Must be called from a dedicated OS thread, never from the tokio/UI thread.
    ///
    /// `stalls` counts the times the wait expired with the frame still undisplayed - that is the
    /// signature of decode being gated by presentation rather than by the decoder itself.
    pub fn lock_decode_target(
        &self,
        stalls: &AtomicU64,
    ) -> Option<DirectVideoTargetGuard<'_>> {
        let mut state = self.state.lock().ok()?;
        if state.pending.is_some() {
            let (waited_state, timeout) = self
                .frame_displayed
                .wait_timeout_while(state, MAX_PENDING_TEXTURE_WAIT, |state| {
                    state.targets.is_some() && state.pending.is_some()
                })
                .ok()?;
            if timeout.timed_out() {
                stalls.fetch_add(1, Ordering::Relaxed);
            }
            state = waited_state;
        }
        let targets = state.targets?;
        let index = state
            .pending
            .map(|(index, _)| index)
            .unwrap_or_else(|| state.displayed.map_or(0, |displayed| 1 - displayed));
        Some(DirectVideoTargetGuard {
            state,
            target: targets[index],
            index,
        })
    }
}

pub struct DirectVideoTargetGuard<'a> {
    state: MutexGuard<'a, DirectVideoOutputState>,
    target: VideoTextureTarget,
    index: usize,
}

impl DirectVideoTargetGuard<'_> {
    pub fn target(&self) -> VideoTextureTarget {
        self.target
    }

    pub fn publish(mut self) -> (usize, u64) {
        self.state.next_generation = self.state.next_generation.wrapping_add(1);
        let generation = self.state.next_generation;
        self.state.pending = Some((self.index, generation));
        (self.index, generation)
    }
}

/// Counters for every place a frame can be lost between RTP and the screen. Ported in spirit
/// Without these the only visible symptom of a
/// stalled pipeline is "the video is slow", with no way to tell *which* stage lost the frame.
///
/// All plain `Relaxed` atomics: they are diagnostics, so a torn read across a tick boundary is
/// irrelevant and they must never add synchronization to the decode path.
#[derive(Default)]
pub struct VideoMetrics {
    /// Access units accepted into the decode queue.
    pub submitted: AtomicU64,
    /// Access units rejected because the queue was already full - i.e. the decoder is the
    /// bottleneck and we chose to drop rather than buffer latency.
    pub queue_full: AtomicU64,
    /// Calls into `HwVideoDecoder::decode`, and their cumulative wall time.
    pub decode_calls: AtomicU64,
    pub decode_us: AtomicU64,
    /// Decoder consumed the access unit but produced no picture (needs more data).
    pub no_frame: AtomicU64,
    /// Decode returned an error (or panicked); each one forces a decoder rebuild.
    pub decode_errors: AtomicU64,
    /// Hardware decoder created from scratch (`sceVideodecInitLibrary` + CDRAM). Expensive;
    /// should be ~1 per session, never per second.
    pub decoder_rebuilds: AtomicU64,
    /// `lock_decode_target` gave up waiting for the render thread to free a texture, so the
    /// previous undisplayed frame was overwritten. High => presentation is gating decode.
    pub target_stalls: AtomicU64,
}

#[derive(Clone, Copy)]
pub struct DecodedFrame {
    pub texture_index: usize,
    pub generation: u64,
}

#[derive(Clone, Copy)]
pub struct DecoderConfig {
    pub decode_width: u32,
    pub decode_height: u32,
    pub output_width: u32,
    pub output_height: u32,
}
