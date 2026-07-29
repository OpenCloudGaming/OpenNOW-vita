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

/// Roughly a quarter-second of slack at the stream's frame rate, matching the sizing OpenNOW's
/// Switch client settles on (`clamp((fps + 14) / 15, 2, 4)`; 4 at 60 fps).
///
/// This was 2, which is under one 33 ms hiccup of headroom. That looks like the latency-conscious
/// choice, but dropping an access unit is not a cheap way to shed load: the decoder loses
/// reference frames, the stream has to resync, and the PLI that follows makes the server drop
/// quality. Trading a couple of frames of buffer for not converting ordinary scheduling jitter
/// into a full resync is the better deal on hardware this tight.
const MAX_PENDING_ACCESS_UNITS: usize = 4;

const BLANK_FRAME_FALLBACK_STREAK: u32 = 30;
const BLANK_FRAME_SAMPLE_BYTES: usize = 512;

/// Best-effort check for "the decoder said it produced a frame, but the buffer is all zero" - see
/// `BLANK_FRAME_FALLBACK_STREAK`.
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
    /// Spawns the decode thread.
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
        metrics.decoder_rebuilds.fetch_add(1, Ordering::Relaxed);
        let worker_metrics = Arc::clone(&metrics);

        std::thread::Builder::new()
            .name("opennow-vita-video-decode".to_owned())
            .spawn(move || {
                crate::thread_affinity::pin_current_thread(
                    crate::thread_affinity::VitaCore::Media,
                    "video decode",
                );
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

    /// Queues one Annex-B access unit; drops it (returning `false`) when the decoder is falling
    /// behind, which is preferable to buffering latency.
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

    /// Drops queued frames and **recreates the hardware decoder**.
    pub fn reset_decoder(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.commands.send(DecoderCommand::Reset);
    }

    /// Discards everything currently queued but keeps the hardware decoder alive - the correct
    /// response to packet damage.
    pub fn begin_resync(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for VideoDecodeWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(DecoderCommand::Stop);
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

    let Some(pixel_format) = direct_output.pixel_format() else {
        return;
    };
    // Timed around the whole call rather than inside it, so plain mutex contention against the
    // render thread's `mark_displayed` counts too - that is the other half of the same stall.
    let wait_started_at = std::time::Instant::now();
    let direct_target = direct_output.lock_decode_target(&metrics.target_stalls);
    metrics.target_wait_us.fetch_add(
        wait_started_at.elapsed().as_micros() as u64,
        Ordering::Relaxed,
    );
    metrics.target_wait_calls.fetch_add(1, Ordering::Relaxed);
    let Some(direct_target) = direct_target else {
        return;
    };
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
