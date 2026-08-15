// Adapted from green-vita (MPL-2.0, https://github.com/Day-OS/green-vita)
// src/streaming/video/worker.rs - dedicated decode thread pulling H.264 access units from a

use super::decoder::HwVideoDecoder;
use super::{
    DecodedFrame, DecoderConfig, DirectVideoOutput, VideoMetrics, VideoPixelFormat,
    VideoTextureTarget,
};
use anyhow::{Context, Result};
use crossbeam_channel::{
    Receiver, Sender, TryRecvError, TrySendError, after, bounded, never, select, unbounded,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const AU_QUEUE_CAP: usize = 6;
const AU_QUEUE_FLOOR: usize = 1;
const PICTURE_POLL_INTERVAL: Duration = Duration::from_millis(1);

const BLANK_FRAME_FALLBACK_STREAK: u32 = 30;
const BLANK_FRAME_SAMPLE_BYTES: usize = 512;

fn output_looks_blank(target: VideoTextureTarget) -> bool {
    let len = (target.capacity as usize).min(BLANK_FRAME_SAMPLE_BYTES);
    if len == 0 {
        return false;
    }
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

#[derive(Default)]
struct DecoderOutputState {
    latest_published_pts: u64,
    blank_streak: u32,
    blank_check_active: bool,
    picture_pending: bool,
}

pub struct VideoDecodeWorker {
    access_units: Sender<QueuedAccessUnit>,
    drop_oldest: Receiver<QueuedAccessUnit>,
    commands: Sender<DecoderCommand>,
    generation: Arc<AtomicU64>,
    metrics: Arc<VideoMetrics>,
    preferred_fps: u32,
}

impl VideoDecodeWorker {
    /// Spawns the decode thread.
    pub fn spawn(
        config: DecoderConfig,
        direct_output: Arc<DirectVideoOutput>,
        latest_frame: Arc<Mutex<Option<(u64, DecodedFrame)>>>,
    ) -> Result<Self> {
        let preferred_fps = crate::gfn::stream_prefs::fps_value();
        let decoder =
            HwVideoDecoder::new(config).context("failed to create hardware H264 decoder")?;
        direct_output.decoder_ready.store(true, Ordering::Release);
        let (access_units, worker_access_units) = bounded(AU_QUEUE_CAP);
        let drop_oldest = worker_access_units.clone();
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
            drop_oldest,
            commands,
            generation,
            metrics,
            preferred_fps,
        })
    }

    /// Live pipeline counters, for the on-screen diagnostic readout in `gfn::peer`.
    pub fn metrics(&self) -> Arc<VideoMetrics> {
        Arc::clone(&self.metrics)
    }

    pub fn submit_access_unit(
        &self,
        data: Vec<u8>,
        source_frame_duration_us: Option<u64>,
    ) -> bool {
        let source_fps = source_frame_duration_us
            .filter(|duration| *duration > 0)
            .map(|duration| 1_000_000 / duration)
            .unwrap_or(u64::from(self.preferred_fps));
        let extra_capacity = source_fps
            .saturating_sub(30)
            .min(25)
            .saturating_mul((AU_QUEUE_CAP - AU_QUEUE_FLOOR) as u64)
            .saturating_add(12)
            / 25;
        let pending_limit = AU_QUEUE_FLOOR + extra_capacity as usize;

        let access_unit = QueuedAccessUnit {
            data,
            generation: self.generation.load(Ordering::Acquire),
        };

        if self.access_units.len() >= pending_limit {
            self.metrics.queue_full.fetch_add(1, Ordering::Relaxed);
            match self.drop_oldest.try_recv() {
                Ok(_) | Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return false,
            }
        }

        match self.access_units.try_send(access_unit) {
            Ok(()) => {
                self.metrics.submitted.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(access_unit)) => {
                self.metrics.queue_full.fetch_add(1, Ordering::Relaxed);
                let _ = self.drop_oldest.try_recv();
                match self.access_units.try_send(access_unit) {
                    Ok(()) => {
                        self.metrics.submitted.fetch_add(1, Ordering::Relaxed);
                        true
                    }
                    Err(_) => false,
                }
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Drops queued frames and **recreates the hardware decoder**.
    pub fn reset_decoder(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.commands.send(DecoderCommand::Reset);
    }

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
    let mut output_state = DecoderOutputState {
        blank_check_active: true,
        ..DecoderOutputState::default()
    };

    loop {
        let picture_poll = if output_state.picture_pending {
            after(PICTURE_POLL_INTERVAL)
        } else {
            never()
        };

        select! {
            recv(commands) -> command => match command {
                Ok(DecoderCommand::Reset) => {
                    decoder = None;
                    output_state = DecoderOutputState {
                        blank_check_active: true,
                        ..DecoderOutputState::default()
                    };
                    continue;
                }
                Ok(DecoderCommand::Stop) | Err(_) => break,
            },
            recv(access_units) -> access_unit => {
                let Ok(access_unit) = access_unit else { break };
                submit_queued_access_unit(
                    &mut decoder,
                    config,
                    &generation,
                    &latest_frame,
                    &mut frame_id,
                    access_unit,
                    &direct_output,
                    &mut output_state,
                    &metrics,
                );
            },
            recv(picture_poll) -> _ => {
                drain_picture(
                    &mut decoder,
                    &latest_frame,
                    &mut frame_id,
                    &direct_output,
                    &mut output_state,
                    &metrics,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn submit_queued_access_unit(
    decoder: &mut Option<HwVideoDecoder>,
    config: DecoderConfig,
    generation: &AtomicU64,
    latest_frame: &Mutex<Option<(u64, DecodedFrame)>>,
    frame_id: &mut u64,
    access_unit: QueuedAccessUnit,
    direct_output: &DirectVideoOutput,
    output_state: &mut DecoderOutputState,
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

    let submit_result = catch_unwind(AssertUnwindSafe(|| {
        decoder
            .as_mut()
            .expect("decoder recreated above")
            .submit_access_unit(&access_unit.data)
    }));
    metrics.decode_calls.fetch_add(1, Ordering::Relaxed);

    match submit_result {
        Ok(Ok(())) => {
            output_state.picture_pending = true;
        }
        Ok(Err(error)) => {
            eprintln!("H264 AU submit error, recreating decoder: {error:#}");
            metrics.decode_errors.fetch_add(1, Ordering::Relaxed);
            *decoder = None;
            output_state.picture_pending = false;
            return;
        }
        Err(_) => {
            eprintln!("H264 decoder panicked on submit; recreating on next frame");
            metrics.decode_errors.fetch_add(1, Ordering::Relaxed);
            *decoder = None;
            output_state.picture_pending = false;
            return;
        }
    }

    if access_unit.generation != generation.load(Ordering::Acquire) {
        return;
    }

    drain_picture(
        decoder,
        latest_frame,
        frame_id,
        direct_output,
        output_state,
        metrics,
    );
}

fn drain_picture(
    decoder: &mut Option<HwVideoDecoder>,
    latest_frame: &Mutex<Option<(u64, DecodedFrame)>>,
    frame_id: &mut u64,
    direct_output: &DirectVideoOutput,
    output_state: &mut DecoderOutputState,
    metrics: &VideoMetrics,
) {
    let Some(decoder_instance) = decoder.as_mut() else {
        output_state.picture_pending = false;
        return;
    };
    if decoder_instance.submitted_sequence() == 0 {
        output_state.picture_pending = false;
        return;
    }

    let Some(pixel_format) = direct_output.pixel_format() else {
        return;
    };

    let Some(direct_target) = direct_output.try_lock_decode_target() else {
        metrics.target_stalls.fetch_add(1, Ordering::Relaxed);
        output_state.picture_pending = true;
        return;
    };
    metrics.target_wait_calls.fetch_add(1, Ordering::Relaxed);

    let picture_result = catch_unwind(AssertUnwindSafe(|| {
        decoder_instance.get_picture(direct_target.target(), pixel_format)
    }));

    match picture_result {
        Ok(Ok(Some(returned_pts))) => {
            if output_state.blank_check_active
                && matches!(
                    pixel_format,
                    VideoPixelFormat::Bgr565 | VideoPixelFormat::Rgba8888
                )
            {
                if output_looks_blank(direct_target.target()) {
                    output_state.blank_streak += 1;
                    if output_state.blank_streak >= BLANK_FRAME_FALLBACK_STREAK {
                        eprintln!(
                            "{pixel_format:?} decoded {BLANK_FRAME_FALLBACK_STREAK} frames in a \
                             row with blank output (Vita3K-style HLE gap); requesting Iyuv fallback"
                        );
                        direct_output.request_format_fallback();
                        output_state.blank_streak = 0;
                        output_state.blank_check_active = false;
                    }
                } else {
                    output_state.blank_streak = 0;
                    output_state.blank_check_active = false;
                }
            }

            if returned_pts <= output_state.latest_published_pts
                && output_state.latest_published_pts != 0
            {
                drop(direct_target);
                metrics.no_frame.fetch_add(1, Ordering::Relaxed);
                output_state.picture_pending = true;
                return;
            }
            output_state.latest_published_pts = returned_pts;
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
            output_state.picture_pending = true;
        }
        Ok(Ok(None)) => {
            metrics.no_frame.fetch_add(1, Ordering::Relaxed);
            output_state.picture_pending = false;
        }
        Ok(Err(error)) => {
            eprintln!("H264 get_picture error, recreating decoder: {error:#}");
            metrics.decode_errors.fetch_add(1, Ordering::Relaxed);
            *decoder = None;
            output_state.picture_pending = false;
        }
        Err(_) => {
            eprintln!("H264 decoder panicked on get_picture; recreating on next frame");
            metrics.decode_errors.fetch_add(1, Ordering::Relaxed);
            *decoder = None;
            output_state.picture_pending = false;
        }
    }
}
