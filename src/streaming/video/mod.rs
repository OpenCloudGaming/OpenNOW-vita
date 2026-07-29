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
pub const AVCDEC_NUM_REF_FRAMES: u32 = 4;

/// Pixel format negotiated between the render thread (which knows what SDL texture formats the
/// platform supports) and the decoder (which asks sceAvcdec for matching output).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoPixelFormat {
    Bgr565,
    Iyuv,
}

const MAX_PENDING_TEXTURE_WAIT: Duration = Duration::from_millis(34);

/// How many textures the decoder and the render thread pass between them.
///
/// Two is the intuitive answer - one on screen, one being drawn - but it makes the handoff
/// lock-step: with both slots occupied the decoder has nowhere to write and parks until the
/// shell's next iteration hands one back. Measured on hardware that was 14.5 ms of waiting per
/// 2.8 ms of decoding. The third texture is the slack that lets the decoder keep working while
/// one frame is displayed and another waits its turn; it costs ~1 MiB.
pub const VIDEO_TEXTURE_COUNT: usize = 3;

/// One SDL streaming texture's writable memory, registered by the shell (`shell::surface`).
#[derive(Clone, Copy)]
pub struct VideoTextureTarget {
    pub ptr: usize,
    pub pitch: u32,
    pub capacity: u32,
}

struct DirectVideoOutputState {
    targets: Option<[VideoTextureTarget; VIDEO_TEXTURE_COUNT]>,
    displayed: Option<usize>,
    /// Decoded and waiting to be shown. Newest-wins rather than a queue: an undisplayed frame that
    /// has already been superseded is worth less than the latency a queue would add.
    pending: Option<(usize, u64)>,
    /// Handed out to the decode thread and being written into right now.
    writing: Option<usize>,
    next_generation: u64,
}

impl DirectVideoOutputState {
    /// The first texture nobody is displaying, holding for display, or writing into.
    fn free_slot(&self) -> Option<usize> {
        (0..VIDEO_TEXTURE_COUNT).find(|index| {
            self.displayed != Some(*index)
                && self.pending.map(|(slot, _)| slot) != Some(*index)
                && self.writing != Some(*index)
        })
    }
}

/// Synchronizes the frame-producer thread with the two SDL/GXM textures owned by the render
/// thread.
pub struct DirectVideoOutput {
    state: Mutex<DirectVideoOutputState>,
    frame_displayed: Condvar,
    pub decoder_ready: AtomicBool,
    /// 0 = not yet registered, 1 = Bgr565, 2 = Iyuv.
    pixel_format: AtomicU8,
    /// Set by the decode thread when Bgr565 keeps decoding "successfully" but the output is
    /// suspiciously blank - Vita3K's HLE AVCDEC silently zeroes RGB565 output instead of erroring
    /// (see `worker::BLANK_FRAME_FALLBACK_STREAK`), so a decode error is not a reliable signal
    /// there.
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
                writing: None,
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

    /// Called by the decode thread once it's confident Bgr565 isn't actually working here (see
    /// `worker::BLANK_FRAME_FALLBACK_STREAK`).
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

    pub fn set_targets(&self, targets: [VideoTextureTarget; VIDEO_TEXTURE_COUNT]) {
        if let Ok(mut state) = self.state.lock() {
            state.targets = Some(targets);
            state.displayed = None;
            state.pending = None;
            state.writing = None;
        }
    }

    pub fn clear_targets(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.targets = None;
            state.displayed = None;
            state.pending = None;
            state.writing = None;
        }
        self.frame_displayed.notify_all();
    }

    pub fn mark_displayed(&self, index: usize, generation: u64) {
        if let Ok(mut state) = self.state.lock() {
            // Showing a new texture frees whichever one it replaces, so this always notifies -
            // not only when it happens to clear `pending`.
            state.displayed = Some(index);
            if state.pending == Some((index, generation)) {
                state.pending = None;
            }
        }
        self.frame_displayed.notify_one();
    }

    /// Blocks (bounded by `MAX_PENDING_TEXTURE_WAIT`) until a texture is free to write into.
    pub fn lock_decode_target(
        &self,
        stalls: &AtomicU64,
    ) -> Option<DirectVideoTargetGuard<'_>> {
        let mut state = self.state.lock().ok()?;
        if state.free_slot().is_none() {
            let (waited_state, timeout) = self
                .frame_displayed
                .wait_timeout_while(state, MAX_PENDING_TEXTURE_WAIT, |state| {
                    state.targets.is_some() && state.free_slot().is_none()
                })
                .ok()?;
            if timeout.timed_out() {
                stalls.fetch_add(1, Ordering::Relaxed);
            }
            state = waited_state;
        }
        let targets = state.targets?;
        // On timeout there is still no free slot, so fall back to overwriting the undisplayed
        // pending frame - dropping a decoded frame beats dropping an access unit, which would cost
        // reference frames and force a resync.
        let index = match state.free_slot() {
            Some(index) => index,
            None => state.pending.map(|(index, _)| index)?,
        };
        state.writing = Some(index);
        Some(DirectVideoTargetGuard {
            state,
            target: targets[index],
            index,
            published: false,
        })
    }
}

pub struct DirectVideoTargetGuard<'a> {
    state: MutexGuard<'a, DirectVideoOutputState>,
    target: VideoTextureTarget,
    index: usize,
    published: bool,
}

impl Drop for DirectVideoTargetGuard<'_> {
    fn drop(&mut self) {
        // The decode path abandons a guard without publishing whenever the access unit turns out
        // to be stale. Releasing the slot here keeps that from permanently retiring a texture.
        if !self.published {
            self.state.writing = None;
        }
    }
}

impl DirectVideoTargetGuard<'_> {
    pub fn target(&self) -> VideoTextureTarget {
        self.target
    }

    pub fn publish(mut self) -> (usize, u64) {
        self.state.next_generation = self.state.next_generation.wrapping_add(1);
        let generation = self.state.next_generation;
        self.state.pending = Some((self.index, generation));
        self.state.writing = None;
        self.published = true;
        (self.index, generation)
    }
}

/// Counters for every place a frame can be lost between RTP and the screen.
#[derive(Default)]
pub struct VideoMetrics {
    /// Access units accepted into the decode queue.
    pub submitted: AtomicU64,
    /// Access units rejected because the queue was already full - i.e.
    pub queue_full: AtomicU64,
    /// Calls into `HwVideoDecoder::decode`, and their cumulative wall time.
    pub decode_calls: AtomicU64,
    pub decode_us: AtomicU64,
    /// Decoder consumed the access unit but produced no picture (needs more data).
    pub no_frame: AtomicU64,
    /// Decode returned an error (or panicked); each one forces a decoder rebuild.
    pub decode_errors: AtomicU64,
    /// Hardware decoder created from scratch (`sceVideodecInitLibrary` + CDRAM).
    pub decoder_rebuilds: AtomicU64,
    /// `lock_decode_target` gave up waiting for the render thread to free a texture, so the
    /// previous undisplayed frame was overwritten.
    pub target_stalls: AtomicU64,
    /// Cumulative time the decode thread spent inside `lock_decode_target` waiting for the render
    /// thread to hand a texture back.
    ///
    /// `decode_us` deliberately excludes this, which makes the two easy to confuse: a `dec:` of
    /// 1 ms alongside a saturated queue reads as "the decoder is idle" when it actually means
    /// "the decoder is parked". Counted separately so the readout can tell those apart.
    pub target_wait_us: AtomicU64,
    pub target_wait_calls: AtomicU64,
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
