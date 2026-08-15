// Direct-texture video path adapted from green-vita (MPL-2.0,
// https://github.com/Day-OS/green-vita) src/shell/surface.rs. See THIRD_PARTY_NOTICES.md.

use crate::gfn::peer::PeerEngine;
use crate::shell::egui_painter::{PaintStats, SdlEguiPainter};
use crate::streaming::video::{
    DirectVideoOutput, VIDEO_TEXTURE_COUNT, VideoPixelFormat, VideoTextureTarget,
};
use anyhow::{Context, Result};
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{Canvas, Texture};
use sdl2::video::Window;
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[derive(Default, Clone, Copy)]
pub struct FramePaintStats {
    pub texture_apply_secs: f64,
    pub geometry_secs: f64,
    pub present_secs: f64,
    pub draw_calls: u32,
    pub textures_uploaded: u32,
    pub vertices_drawn: u32,
}

pub const WIDTH: u32 = 960;
pub const HEIGHT: u32 = 544;

/// Owns the SDL window/canvas, the egui painter, and the two streaming BGR565 video textures that
/// the frame producer (`gfn::peer`) writes into directly - the video never round-trips through
/// egui, which is what keeps the Vita inside its VRAM budget.
pub struct VitaSurface {
    canvas: Canvas<Window>,
    video_textures: Option<[Texture; VIDEO_TEXTURE_COUNT]>,
    displayed_video_texture: Option<usize>,
    direct_video_output: Option<Arc<DirectVideoOutput>>,
    video_width: u32,
    video_height: u32,
    last_frame_id: u64,
    egui_painter: SdlEguiPainter,
    /// Set once the decode thread reports Bgr565 is producing blank frames (Vita3K's HLE AVCDEC
    /// gap - see `streaming::video::worker::BLANK_FRAME_FALLBACK_STREAK`).
    force_iyuv: bool,
}

impl VitaSurface {
    pub fn new(video: &sdl2::VideoSubsystem) -> Result<Self> {
        sdl2::hint::set("SDL_RENDER_SCALE_QUALITY", "1");

        let window = video
            .window("OpenNOW Vita", WIDTH, HEIGHT)
            .position_centered()
            .build()
            .context("failed to create SDL Vita window")?;
        let mut canvas = window
            .into_canvas()
            .accelerated()
            .build()
            .map_err(anyhow::Error::msg)
            .context("failed to create SDL Vita renderer")?;
        canvas
            .set_logical_size(WIDTH, HEIGHT)
            .map_err(anyhow::Error::msg)
            .context("failed to set Vita logical render size")?;

        Ok(Self {
            canvas,
            video_textures: None,
            displayed_video_texture: None,
            direct_video_output: None,
            video_width: 0,
            video_height: 0,
            last_frame_id: 0,
            egui_painter: SdlEguiPainter::default(),
            force_iyuv: false,
        })
    }

    /// Where the video quad lands on screen - accounts for letterboxing, see `fit_rect`.
    fn video_rect(&self) -> sdl2::rect::Rect {
        Self::fit_rect(self.video_width, self.video_height, WIDTH, HEIGHT)
    }

    /// Registers/releases the direct video textures as streaming starts/stops and flips to the
    /// most recently published frame.
    pub fn sync_video_frame(
        &mut self,
        streaming: Option<&PeerEngine>,
        latest_video: Option<&(u64, crate::streaming::video::DecodedFrame)>,
    ) -> Result<()> {
        let Some(streaming) = streaming else {
            self.detach_direct_video_output();
            return Ok(());
        };
        if !self.force_iyuv
            && let Some(output) = &self.direct_video_output
            && output.take_format_fallback_request()
        {
            self.force_iyuv = true;
            self.detach_direct_video_output();
        }
        self.ensure_direct_video_output(streaming)?;

        let Some((frame_id, frame)) = latest_video else {
            return Ok(());
        };
        if *frame_id == self.last_frame_id {
            return Ok(());
        }
        let index = frame.texture_index;
        if index >= VIDEO_TEXTURE_COUNT {
            anyhow::bail!("frame producer returned invalid direct texture index {index}");
        }
        if let Some(output) = &self.direct_video_output {
            output.mark_displayed(index, frame.generation);
        }
        self.displayed_video_texture = Some(index);
        self.last_frame_id = *frame_id;
        Ok(())
    }

    fn ensure_direct_video_output(&mut self, streaming: &PeerEngine) -> Result<()> {
        let output = streaming.direct_video_output();
        let output_is_current = self
            .direct_video_output
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &output));
        if !output_is_current && self.direct_video_output.is_some() {
            self.detach_direct_video_output();
        }
        if !output.decoder_ready.load(Ordering::Acquire) {
            return Ok(());
        }
        if output_is_current && self.video_textures.is_some() {
            return Ok(());
        }

        self.detach_direct_video_output();
        let (width, height) = (output.width, output.height);
        let force_iyuv = self.force_iyuv;
        let mut format = VideoPixelFormat::Bgr565;
        let create_targets = |pixel_format: PixelFormatEnum| -> Result<[Texture; VIDEO_TEXTURE_COUNT]> {
            let create_one = || {
                self.canvas
                    .create_texture_streaming(pixel_format, width, height)
                    .map_err(anyhow::Error::msg)
                    .with_context(|| format!("failed to create {pixel_format:?} video texture"))
            };
            Ok([create_one()?, create_one()?, create_one()?])
        };
        let want_32_bit = crate::gfn::stream_prefs::color_depth()
            == crate::gfn::stream_prefs::ColorDepth::ThirtyTwoBit;
        let mut textures = if force_iyuv {
            format = VideoPixelFormat::Iyuv;
            create_targets(PixelFormatEnum::IYUV)?
        } else {
            let thirty_two = want_32_bit
                .then(|| create_targets(PixelFormatEnum::ABGR8888))
                .transpose()
                .unwrap_or_else(|error| {
                    eprintln!("ABGR8888 video textures unavailable ({error:#}); using BGR565");
                    None
                });
            match thirty_two {
                Some(textures) => {
                    format = VideoPixelFormat::Rgba8888;
                    textures
                }
                None => match create_targets(PixelFormatEnum::BGR565) {
                    Ok(textures) => textures,
                    Err(error) => {
                        eprintln!("BGR565 video textures unavailable ({error:#}); using IYUV");
                        format = VideoPixelFormat::Iyuv;
                        create_targets(PixelFormatEnum::IYUV)?
                    }
                },
            }
        };
        let record_targets =
            |textures: &mut [Texture; VIDEO_TEXTURE_COUNT]| -> Result<[VideoTextureTarget; VIDEO_TEXTURE_COUNT]> {
                let mut targets = [VideoTextureTarget {
                    ptr: 0,
                    pitch: 0,
                    capacity: 0,
                }; VIDEO_TEXTURE_COUNT];
                for (index, texture) in textures.iter_mut().enumerate() {
                    texture
                        .with_lock(None, |pixels, pitch| {
                            targets[index] = VideoTextureTarget {
                                ptr: pixels.as_mut_ptr() as usize,
                                pitch: pitch as u32,
                                capacity: pixels.len().min(u32::MAX as usize) as u32,
                            };
                        })
                        .map_err(anyhow::Error::msg)
                        .context("failed to lock direct SDL video texture")?;
                }
                Ok(targets)
            };

        let mut targets = record_targets(&mut textures)?;
        if !force_iyuv
            && format == VideoPixelFormat::Iyuv
            && targets.iter().any(|target| target.pitch != width)
        {
            eprintln!(
                "IYUV texture pitch {} != width {width}; using BGR565",
                targets[0].pitch
            );
            format = VideoPixelFormat::Bgr565;
            textures = create_targets(PixelFormatEnum::BGR565)?;
            targets = record_targets(&mut textures)?;
        }
        output.set_pixel_format(format);
        output.set_targets(targets);
        self.video_textures = Some(textures);
        self.displayed_video_texture = None;
        self.direct_video_output = Some(output);
        self.video_width = width;
        self.video_height = height;
        self.last_frame_id = 0;
        Ok(())
    }

    fn detach_direct_video_output(&mut self) {
        if let Some(output) = self.direct_video_output.take() {
            output.clear_targets();
        }
        self.video_textures = None;
        self.displayed_video_texture = None;
        self.video_width = 0;
        self.video_height = 0;
        self.last_frame_id = 0;
    }

    pub fn draw_scene(&mut self, show_video: bool) -> Result<()> {
        self.canvas.set_draw_color(sdl2::pixels::Color::BLACK);
        self.canvas.clear();

        if show_video
            && let Some(index) = self.displayed_video_texture
            && let Some(texture) = self
                .video_textures
                .as_ref()
                .map(|textures| &textures[index])
        {
            let destination = self.video_rect();
            self.canvas
                .copy(texture, None, destination)
                .map_err(anyhow::Error::msg)
                .context("failed to draw direct video frame")?;
        }

        Ok(())
    }

    pub fn paint_egui(
        &mut self,
        pixels_per_point: f32,
        primitives: &[egui::ClippedPrimitive],
        textures_delta: &egui::TexturesDelta,
    ) -> Result<FramePaintStats> {
        let PaintStats {
            texture_apply_secs,
            geometry_secs,
            draw_calls,
            textures_uploaded,
            vertices_drawn,
        } = self.egui_painter.paint(
            &mut self.canvas,
            [WIDTH, HEIGHT],
            pixels_per_point,
            primitives,
            textures_delta,
        )?;
        let present_started_at = std::time::Instant::now();
        self.canvas.present();
        let present_secs = present_started_at.elapsed().as_secs_f64();
        Ok(FramePaintStats {
            texture_apply_secs,
            geometry_secs,
            present_secs,
            draw_calls,
            textures_uploaded,
            vertices_drawn,
        })
    }

    fn fit_rect(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> sdl2::rect::Rect {
        if src_w == 0 || src_h == 0 {
            return sdl2::rect::Rect::new(0, 0, dst_w, dst_h);
        }
        let src_aspect = src_w as f32 / src_h as f32;
        let dst_aspect = dst_w as f32 / dst_h as f32;
        if src_aspect > dst_aspect {
            let height = (dst_w as f32 / src_aspect).round() as u32;
            let y = ((dst_h - height) / 2) as i32;
            sdl2::rect::Rect::new(0, y, dst_w, height)
        } else {
            let width = (dst_h as f32 * src_aspect).round() as u32;
            let x = ((dst_w - width) / 2) as i32;
            sdl2::rect::Rect::new(x, 0, width, dst_h)
        }
    }
}
