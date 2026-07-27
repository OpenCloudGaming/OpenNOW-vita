// Adapted from green-vita (MPL-2.0, https://github.com/Day-OS/green-vita)
// src/streaming/video/worker.rs - dedicated decode thread pulling H.264 access units from a
// bounded queue and publishing decoded frames through DirectVideoOutput. Metrics and the
// adaptive queue sizing were dropped in this port; results publish straight into the
// `(frame id, DecodedFrame)` slot the shell polls. See THIRD_PARTY_NOTICES.md.

use super::decoder::HwVideoDecoder;
use super::{
    DecodedFrame, DecoderConfig, DirectVideoOutput, VideoMetrics, VideoPixelFormat,
    VideoTextureTarget,
};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, select_biased, unbounded};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// Two compressed frames of queue (~33ms at the 60 fps profile). Anything deeper converts
// network jitter into steady-state latency; when the queue is full we drop instead. A deeper
// queue would absorb bursty high-fps streams, but latency is the thing we're short of here,
// not throughput.
const MAX_PENDING_ACCESS_UNITS: usize = 2;

// Vita3K's HLE AVCDEC doesn't error on Bgr565 output - it just leaves the buffer zeroed (see
// `shell::surface::ensure_direct_video_output`'s comment). A single black frame can happen on
// real hardware too (e.g. a black loading screen in the streamed game), so only treat a long
// run of *consecutive* blank "successful" decodes as the emulator signal, not one frame.
const BLANK_FRAME_FALLBACK_STREAK: u32 = 30;
// How many leading bytes of the output buffer to sample - cheap, and real video content is
// never actually all-zero across this many bytes in the frame's top-left corner.
const BLANK_FRAME_SAMPLE_BYTES: usize = 512;

/// Best-effort check for "the decoder said it produced a frame, but the buffer is all zero" -
/// see `BLANK_FRAME_FALLBACK_STREAK`.
fn output_looks_blank(target: VideoTextureTarget) -> bool {
    let len = (target.capacity as usize).min(BLANK_FRAME_SAMPLE_BYTES);
    if len == 0 {
        return false;
    }
    // SAFETY: `target` was just written into by `HwVideoDecoder::decode` on this same thread;
    // `len` is bounded by the texture's own reported capacity.
    let sample = unsafe { std::slice::from_raw_parts(target.ptr as *const u8, len) };
    sample.iter().all(|&byte| byte == 0)
}

struct QueuedAccessUnit {
    data: Vec<u8>,
    generation: u64,
}

enum DecoderCommand {
    Reset,
    Stop,
}

pub struct VideoDecodeWorker {
    access_units: Sender<QueuedAccessUnit>,
    commands: Sender<DecoderCommand>,
    generation: Arc<AtomicU64>,
    metrics: Arc<VideoMetrics>,
}

impl VideoDecodeWorker {
    /// Spawns the decode thread. Decoded frames land in `latest_frame` (with a monotonically
    /// increasing id) for `shell::surface::sync_video_frame` to pick up.
    pub fn spawn(
        config: DecoderConfig,
        direct_output: Arc<DirectVideoOutput>,
        latest_frame: Arc<Mutex<Option<(u64, DecodedFrame)>>>,
    ) -> Result<Self> {
        let decoder =
            HwVideoDecoder::new(config).context("failed to create hardware H264 decoder")?;
        direct_output.decoder_ready.store(true, Ordering::Release);
        let (access_units, worker_access_units) = bounded(MAX_PENDING_ACCESS_UNITS);
        let (commands, worker_commands) = unbounded();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = Arc::clone(&generation);
        let metrics = Arc::new(VideoMetrics::default());
        // The decoder built above counts as the session's first (and ideally only) build.
        metrics.decoder_rebuilds.fetch_add(1, Ordering::Relaxed);
        let worker_metrics = Arc::clone(&metrics);

        std::thread::Builder::new()
            .name("jade-vita-video-decode".to_owned())
            .spawn(move || {
                #[cfg(target_os = "vita")]
                pin_decoder_thread();
                run_decode_loop(
                    worker_access_units,
                    worker_commands,
                    worker_generation,
                    latest_frame,
                    decoder,
                    config,
                    direct_output,
                    worker_metrics,
                )
            })
            .context("failed to spawn video decode worker")?;

        Ok(Self {
            access_units,
            commands,
            generation,
            metrics,
        })
    }

    /// Live pipeline counters, for the on-screen diagnostic readout in `gfn::peer`.
    pub fn metrics(&self) -> Arc<VideoMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Queues one Annex-B access unit; drops it (returning `false`) when the decoder is
    /// falling behind, which is preferable to buffering latency.
    pub fn submit_access_unit(&self, data: Vec<u8>) -> bool {
        let access_unit = QueuedAccessUnit {
            data,
            generation: self.generation.load(Ordering::Acquire),
        };
        match self.access_units.try_send(access_unit) {
            Ok(()) => {
                self.metrics.submitted.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(_)) => {
                self.metrics.queue_full.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Drops queued frames and **recreates the hardware decoder**. Reserved for genuine stream
    /// discontinuities (resolution change, track restart) and unrecoverable decode errors.
    ///
    /// Recreating means `sceVideodecTermLibrary` + `sceVideodecInitLibrary` + a fresh CDRAM
    /// allocation, which costs far more than a frame interval - never call this for ordinary
    /// packet loss. Use [`Self::begin_resync`] for that.
    pub fn reset_decoder(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.commands.send(DecoderCommand::Reset);
    }

    /// Discards everything currently queued but keeps the hardware decoder alive - the correct
    /// response to packet damage.
    ///
    /// Bumping the generation is enough: `decode_queued_access_unit` drops any access unit whose
    /// generation no longer matches, so in-flight and queued units are abandoned without
    /// touching the decoder itself. The caller then waits for the next keyframe.
    ///
    /// This distinction is load-bearing. Using `reset_decoder` here (as this port originally did)
    /// tore down and reinitialized the hardware decoder every time `DAMAGE_RESYNC_THRESHOLD` was
    /// reached - a few times per second on a lossy link - which collapsed throughput to ~8 fps
    /// while RTP was still assembling a full 60 access units per second.
    pub fn begin_resync(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for VideoDecodeWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(DecoderCommand::Stop);
    }
}

#[cfg(target_os = "vita")]
fn pin_decoder_thread() {
    let thread_id = unsafe { vitasdk_sys::sceKernelGetThreadId() };
    let result = unsafe {
        vitasdk_sys::sceKernelChangeThreadCpuAffinityMask(
            thread_id,
            vitasdk_sys::SCE_KERNEL_CPU_MASK_USER_2 as i32,
        )
    };
    if result < 0 {
        eprintln!("Failed to pin video decoder thread to user CPU 2: {result:#x}");
    }
}

#[allow(clippy::too_many_arguments)]
fn run_decode_loop(
    access_units: Receiver<QueuedAccessUnit>,
    commands: Receiver<DecoderCommand>,
    generation: Arc<AtomicU64>,
    latest_frame: Arc<Mutex<Option<(u64, DecodedFrame)>>>,
    initial_decoder: HwVideoDecoder,
    config: DecoderConfig,
    direct_output: Arc<DirectVideoOutput>,
    metrics: Arc<VideoMetrics>,
) {
    let mut decoder = Some(initial_decoder);
    let mut frame_id: u64 = 0;
    let mut blank_streak: u32 = 0;

    loop {
        select_biased! {
            recv(commands) -> command => match command {
                Ok(DecoderCommand::Reset) => {
                    decoder = None;
                    continue;
                }
                Ok(DecoderCommand::Stop) | Err(_) => break,
            },
            recv(access_units) -> access_unit => {
                let Ok(access_unit) = access_unit else { break };
                decode_queued_access_unit(
                    &mut decoder,
                    config,
                    &generation,
                    &latest_frame,
                    &mut frame_id,
                    access_unit,
                    &direct_output,
                    &mut blank_streak,
                    &metrics,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_queued_access_unit(
    decoder: &mut Option<HwVideoDecoder>,
    config: DecoderConfig,
    generation: &AtomicU64,
    latest_frame: &Mutex<Option<(u64, DecodedFrame)>>,
    frame_id: &mut u64,
    access_unit: QueuedAccessUnit,
    direct_output: &DirectVideoOutput,
    blank_streak: &mut u32,
    metrics: &VideoMetrics,
) {
    if access_unit.generation != generation.load(Ordering::Acquire) {
        return;
    }

    if decoder.is_none() {
        metrics.decoder_rebuilds.fetch_add(1, Ordering::Relaxed);
        match HwVideoDecoder::new(config) {
            Ok(new_decoder) => *decoder = Some(new_decoder),
            Err(error) => {
                eprintln!("failed to recreate H264 decoder: {error:#}");
                return;
            }
        }
    }

    // The renderer registers the texture pixel format together with the targets; without
    // either there is nowhere (and no way) to decode.
    let Some(pixel_format) = direct_output.pixel_format() else {
        return;
    };
    let Some(direct_target) = direct_output.lock_decode_target(&metrics.target_stalls) else {
        return;
    };
    // Contain an unexpected decoder panic inside its worker thread.
    let decode_started_at = std::time::Instant::now();
    let decode_result = catch_unwind(AssertUnwindSafe(|| {
        decoder
            .as_mut()
            .expect("decoder recreated above")
            .decode(&access_unit.data, direct_target.target(), pixel_format)
    }));
    metrics.decode_calls.fetch_add(1, Ordering::Relaxed);
    metrics
        .decode_us
        .fetch_add(decode_started_at.elapsed().as_micros() as u64, Ordering::Relaxed);
    if access_unit.generation != generation.load(Ordering::Acquire) {
        return;
    }

    match decode_result {
        Ok(Ok(true)) => {
            if pixel_format == VideoPixelFormat::Bgr565 {
                if output_looks_blank(direct_target.target()) {
                    *blank_streak += 1;
                    if *blank_streak >= BLANK_FRAME_FALLBACK_STREAK {
                        eprintln!(
                            "Bgr565 decoded {BLANK_FRAME_FALLBACK_STREAK} frames in a row with \
                             blank output (Vita3K-style HLE gap); requesting Iyuv fallback"
                        );
                        direct_output.request_format_fallback();
                        *blank_streak = 0;
                    }
                } else {
                    *blank_streak = 0;
                }
            }
            let (texture_index, generation) = direct_target.publish();
            *frame_id += 1;
            if let Ok(mut slot) = latest_frame.lock() {
                *slot = Some((
                    *frame_id,
                    DecodedFrame {
                        texture_index,
                        generation,
                    },
                ));
            }
        }
        Ok(Ok(false)) => {
            metrics.no_frame.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Err(error)) => {
            eprintln!("H264 decode error, recreating decoder: {error:#}");
            metrics.decode_errors.fetch_add(1, Ordering::Relaxed);
            *decoder = None;
        }
        Err(_) => {
            eprintln!("H264 decoder panicked; recreating decoder on next frame");
            metrics.decode_errors.fetch_add(1, Ordering::Relaxed);
            *decoder = None;
        }
    }
}
