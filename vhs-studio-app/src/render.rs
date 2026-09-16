//! Headless renderer: owns a wgpu device with no window attached.
//!
//! This is what the CLI verifier and the still-export path use. The GUI
//! borrows eframe's device instead and drives [`gpu::Pipeline`] directly, so
//! both paths run identical kernels and identical shader chains.

use std::path::Path;

use vhs_studio_core::{DownscaleMethod, DownscaleSpec, NtscStage, ScanlineGrid};

use crate::gpu::{Pipeline, RenderRequest, ShaderChain, WORK_FORMAT};
use crate::image_io::{padded_bytes_per_row, unpad_rows, SourceImage};

/// One render's worth of configuration.
pub struct RenderSettings {
    /// Retro width the CRT shader sees; None disables the downscale stage.
    pub downscale_width: Option<u32>,
    pub downscale_method: DownscaleMethod,
    pub ntsc_enabled: bool,
    /// Off writes the chain input itself, nearest-scaled to the output.
    pub shader_enabled: bool,
    /// Applied to the source before the signal stage. See
    /// `vhs_studio_core::rotation`.
    pub rotation: vhs_studio_core::Rotation,
    /// Keyframe animation. When present and non-empty, the NTSC settings and
    /// shader parameters are evaluated per frame instead of being fixed.
    pub timeline: Option<vhs_studio_core::Timeline>,
    /// ntsc-rs preset JSON. None uses the house look the app opens on
    /// (`NtscStage::house`).
    pub ntsc_preset_json: Option<String>,
    pub shader_id: String,
    /// Shader parameter overrides applied after the chain loads. Names the
    /// shader doesn't expose are ignored — a preset written against a
    /// different shader build shouldn't fail the whole render.
    pub shader_params: Vec<(String, f32)>,
    /// Colour grade on the chain input. Default is off.
    pub grade: vhs_studio_core::Grade,
    pub output_height: u32,
    /// Round the output onto the scanline grid instead of supersampling.
    pub snap_to_scanline_grid: bool,
    pub frame_count: usize,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            downscale_width: Some(320),
            downscale_method: DownscaleMethod::Area,
            ntsc_enabled: true,
            shader_enabled: true,
            rotation: vhs_studio_core::Rotation::None,
            timeline: None,
            ntsc_preset_json: None,
            shader_id: "royale".to_string(),
            shader_params: Vec::new(),
            grade: vhs_studio_core::Grade::default(),
            output_height: 960,
            snap_to_scanline_grid: false,
            frame_count: 0,
        }
    }
}

/// A loaded chain and its render target, reused across a run of frames.
///
/// Created by [`HeadlessRenderer::begin_sequence`]; it borrows nothing, so
/// the renderer stays free to be used alongside it.
pub struct FrameSequence {
    chain: ShaderChain,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    /// What the frames actually come out at — this is what an encoder must
    /// be told, and it differs from the requested height under `--snap`.
    pub output_size: (u32, u32),
    downscale: Option<DownscaleSpec>,
    ntsc_enabled: bool,
    shader_enabled: bool,
    /// Current grade; the evaluator rewrites it per frame when keyed.
    grade: vhs_studio_core::Grade,
    /// Built once from the timeline plus the chain's own parameter bounds,
    /// then read per frame.
    evaluator: Option<vhs_studio_core::TimelineEvaluator>,
    /// Total frames the animation spans, for turning a frame index into a
    /// position along the timeline.
    timeline_frames: u32,
}

impl FrameSequence {
    /// The chain, for a caller that adjusts parameters between frames.
    pub fn chain(&self) -> &ShaderChain {
        &self.chain
    }

    /// Whether this sequence animates.
    pub fn is_animated(&self) -> bool {
        self.evaluator.is_some()
    }

    /// What a playback producer needs to bake the animation into the frames
    /// it processes: clip frame `i` gets the ntsc-rs settings at
    /// `timeline_position(i, timeline_frames)`, the mapping `encode_frame`
    /// uses for the shader and grade. None when nothing animates.
    pub fn per_frame_ntsc_json(&self) -> Option<crate::video::playback::PerFrameJson> {
        let ev = self.evaluator.clone()?;
        let total = self.timeline_frames;
        Some(std::sync::Arc::new(move |frame: usize| Some(ev.ntsc_json(timeline_position(frame, total)))))
    }

    /// Frames the animation spans. 0 when there is none.
    pub fn timeline_frames(&self) -> u32 {
        if self.evaluator.is_some() { self.timeline_frames } else { 0 }
    }

    /// Stretch the animation over `frames` frames instead of the timeline's
    /// own duration × rate. A clip's keyframes are proportional to the clip
    /// (the app maps frame `i` of a video to `t = i / (total - 1)`), so an
    /// export of one spans the clip's frame count, not the still length the
    /// timeline stores.
    pub fn set_timeline_frames(&mut self, frames: u32) {
        self.timeline_frames = frames.max(1);
    }

    /// Whether the CPU signal stage runs for the frames that follow.
    ///
    /// Playback turns it off, because its background producer has already
    /// applied the stage to the pixels it hands over, and back on for the
    /// occasional frame that has to be processed here after all.
    pub fn set_ntsc_enabled(&mut self, enabled: bool) {
        self.ntsc_enabled = enabled;
    }
}

pub struct HeadlessRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_info: wgpu::AdapterInfo,
    pipeline: Pipeline,
    ntsc: NtscStage,
    /// Whether the device was created with PIPELINE_CACHE.
    pipeline_cache: bool,
}

impl HeadlessRenderer {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // D3D12 first (the native Windows path), Vulkan as the fallback for
        // adapters where it behaves better.
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12 | wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(desc);

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))?;
        let adapter_info = adapter.get_info();

        // librashader caches compiled pipelines when asked, which needs
        // PIPELINE_CACHE on the device. Not every adapter/backend exposes it,
        // so take it when offered and tell the chain to skip the cache when
        // it isn't — requesting the cache without the feature is a fatal
        // wgpu validation error, not a soft fallback.
        let pipeline_cache = adapter.features().contains(wgpu::Features::PIPELINE_CACHE);
        let required_features = if pipeline_cache {
            wgpu::Features::PIPELINE_CACHE
        } else {
            wgpu::Features::empty()
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vhs-studio.headless"),
            required_features,
            // crt-royale is the heaviest preset in the bundle and needs more
            // than the downlevel defaults allow.
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))?;

        let pipeline = Pipeline::new(&device);
        Ok(Self { device, queue, adapter_info, pipeline, ntsc: NtscStage::house(), pipeline_cache })
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Load the shader chain and size the target once, for a run of frames.
    ///
    /// Video renders thousands of frames with one configuration, and loading
    /// the chain per frame would dominate the export (crt-royale is a
    /// ten-pass preset). Stills go through the same path with a run of one,
    /// so there is no second rendering path to keep in step.
    pub fn begin_sequence(
        &mut self,
        settings: &RenderSettings,
        source_size: (u32, u32),
    ) -> Result<FrameSequence, Box<dyn std::error::Error>> {
        match &settings.ntsc_preset_json {
            Some(json) => self.ntsc.set_settings_json(json)?,
            None => self.ntsc.reset_to_house_defaults()?,
        }

        let entry = crate::presets::find(&settings.shader_id)
            .ok_or_else(|| format!("unknown shader preset '{}'", settings.shader_id))?;
        let preset_path = crate::presets::resolve(entry).ok_or_else(|| {
            format!(
                "shader '{}' not found. Set VHS_STUDIO_SHADERS to a slang-shaders checkout, \
                 or run: git submodule update --init Vendor/slang-shaders",
                entry.relative_path
            )
        })?;
        let chain = ShaderChain::load(
            &preset_path,
            &self.device,
            &self.queue,
            Some(self.adapter_info.clone()),
            self.pipeline_cache,
        )?;

        // House defaults first, then the caller's overrides — the same
        // layering the app applies, so the CLI matches what the app shows.
        if let Some((_, house)) = crate::presets::HOUSE_SHADER_DEFAULTS
            .iter()
            .find(|(id, _)| *id == settings.shader_id)
        {
            for (name, value) in *house {
                chain.set_parameter(name, *value);
            }
        }
        for (name, value) in &settings.shader_params {
            chain.set_parameter(name, *value);
        }

        let (sw, sh) = source_size;
        let downscale = settings
            .downscale_width
            .map(|w| DownscaleSpec::for_width(w, sw, sh, settings.downscale_method));
        let chain_size = ScanlineGrid::chain_input_size(sw, sh, downscale.as_ref());

        // Output width follows the chain input's aspect ratio, then either
        // snaps onto the scanline grid or keeps the requested height.
        let output_size = if settings.snap_to_scanline_grid {
            ScanlineGrid::snapped_size(chain_size.0, chain_size.1, settings.output_height)
        } else {
            let w = (settings.output_height as f64 * chain_size.0 as f64 / chain_size.1 as f64)
                .round()
                .max(1.0) as u32;
            (w, settings.output_height.max(1))
        };

        let output = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vhs-studio.output"),
            size: wgpu::Extent3d {
                width: output_size.0,
                height: output_size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORK_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output.create_view(&Default::default());

        // The evaluator needs the chain's declared parameter bounds, so it
        // can only be built once the chain is loaded — an interpolated value
        // has to land on a legal step, not halfway between two mask modes.
        let (evaluator, timeline_frames) = match settings.timeline.as_ref() {
            Some(tl) if tl.has_keys() => {
                let meta: std::collections::BTreeMap<String, vhs_studio_core::ShaderMeta> = chain
                    .parameters()
                    .iter()
                    .map(|p| {
                        (
                            p.name.clone(),
                            vhs_studio_core::ShaderMeta {
                                minimum: p.minimum,
                                maximum: p.maximum,
                                step: p.step,
                            },
                        )
                    })
                    .collect();
                (
                    vhs_studio_core::TimelineEvaluator::new(tl, meta, vhs_studio_core::timeline::ntsc_interp_table()),
                    tl.frame_count(),
                )
            }
            _ => (None, 0),
        };

        Ok(FrameSequence {
            chain,
            output,
            output_view,
            output_size,
            downscale,
            ntsc_enabled: settings.ntsc_enabled,
            shader_enabled: settings.shader_enabled,
            grade: settings.grade.clone(),
            evaluator,
            timeline_frames,
        })
    }

    /// Render one frame of `sequence` into its target, without reading back.
    ///
    /// `source_version` lets the NTSC stage reuse its clean copy across
    /// frames of an unchanging image (a still animating), and must be None
    /// for video, where every frame is different pixels.
    pub fn encode_frame(
        &mut self,
        sequence: &mut FrameSequence,
        source: &[u8],
        source_size: (u32, u32),
        frame_count: usize,
        source_version: Option<i64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // An animated sequence re-dials the whole effect before every frame.
        if let Some(ev) = &sequence.evaluator {
            let t = timeline_position(frame_count, sequence.timeline_frames);
            self.ntsc.set_settings_json(&ev.ntsc_json(t))?;
            for (name, value) in ev.shader_params(t) {
                sequence.chain.set_parameter(&name, value);
            }
            for (name, value) in ev.grade_values(t) {
                sequence.grade.set(&name, value);
            }
        }
        let grade = (!sequence.grade.is_identity()).then(|| sequence.grade.uniform());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("vhs-studio.render") });

        self.pipeline.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &mut self.ntsc,
            &mut sequence.chain,
            &RenderRequest {
                source,
                source_size,
                downscale: sequence.downscale,
                ntsc_enabled: sequence.ntsc_enabled,
                shader_enabled: sequence.shader_enabled,
                grade,
                frame_count,
                // No version means "these are new pixels": the stage snapshots
                // them rather than restoring the previous frame's.
                source_version: source_version.unwrap_or(i64::MIN),
                prepared_chain_input: None,
                output_size: sequence.output_size,
            },
            &sequence.output_view,
            WORK_FORMAT,
        )?;
        self.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    /// Render one frame from a chain input the frame cache already holds —
    /// the NTSC stage and the downscale are both skipped.
    pub fn encode_frame_from_chain_input(
        &mut self,
        sequence: &mut FrameSequence,
        chain_input: &wgpu::Texture,
        frame_count: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let grade = (!sequence.grade.is_identity()).then(|| sequence.grade.uniform());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vhs-studio.render.cached"),
        });
        self.pipeline.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &mut self.ntsc,
            &mut sequence.chain,
            &RenderRequest {
                // Ignored when a prepared chain input is given, but the source
                // size still has to be non-degenerate for the early-out.
                source: &[],
                source_size: (chain_input.width(), chain_input.height()),
                downscale: None,
                ntsc_enabled: false,
                shader_enabled: sequence.shader_enabled,
                grade,
                frame_count,
                source_version: 0,
                prepared_chain_input: Some(chain_input),
                output_size: sequence.output_size,
            },
            &sequence.output_view,
            WORK_FORMAT,
        )?;
        self.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    /// Take the last render's chain input for the frame cache to keep.
    pub fn take_chain_input_copy(&mut self) -> Option<(wgpu::Texture, (u32, u32))> {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vhs-studio.cache_fill"),
        });
        let copied = self.pipeline.copy_last_chain_input(&self.device, &mut encoder)?;
        self.queue.submit(Some(encoder.finish()));
        Some(copied)
    }

    /// Copy a rendered frame back to the CPU as tightly packed RGBA8.
    pub fn read_back(
        &mut self,
        sequence: &FrameSequence,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let (out_w, out_h) = sequence.output_size;
        let padded = padded_bytes_per_row(out_w);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vhs-studio.readback"),
            size: (padded * out_h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vhs-studio.readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &sequence.output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(out_h),
                },
            },
            wgpu::Extent3d { width: out_w, height: out_h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;

        let pixels = {
            let view = slice.get_mapped_range()?;
            unpad_rows(&view, padded, out_w, out_h)
        };
        buffer.unmap();
        Ok(pixels)
    }

    /// Run `source` through the full pipeline and return RGBA8 pixels plus
    /// the size actually produced (which differs from the request when
    /// `snap_to_scanline_grid` is on).
    pub fn render(
        &mut self,
        source: &SourceImage,
        settings: &RenderSettings,
    ) -> Result<(Vec<u8>, u32, u32), Box<dyn std::error::Error>> {
        // Rotation comes first, before the signal stage — see
        // `vhs_studio_core::rotation` for why the order is not negotiable.
        let rotated = (settings.rotation != vhs_studio_core::Rotation::None).then(|| {
            let (pixels, width, height) = vhs_studio_core::rotate_rgba(
                &source.pixels,
                source.width,
                source.height,
                settings.rotation,
            );
            SourceImage { width, height, pixels }
        });
        let source = rotated.as_ref().unwrap_or(source);

        let mut sequence = self.begin_sequence(settings, source.size())?;
        self.encode_frame(
            &mut sequence,
            &source.pixels,
            source.size(),
            settings.frame_count,
            Some(0),
        )?;
        let pixels = self.read_back(&sequence)?;
        let (w, h) = sequence.output_size;
        Ok((pixels, w, h))
    }

    /// Load a shader and report the parameters it declares, in order.
    ///
    /// Used by the verifier's `--list-params` to confirm a preset's metadata
    /// reaches the UI (labels, ranges and steps drive the CRT panel's
    /// controls, and the descriptions feed the hyllian gate rule).
    pub fn shader_parameters(
        &mut self,
        shader_id: &str,
    ) -> Result<Vec<crate::gpu::chain::ShaderParamMeta>, Box<dyn std::error::Error>> {
        let entry = crate::presets::find(shader_id)
            .ok_or_else(|| format!("unknown shader preset '{shader_id}'"))?;
        let path = crate::presets::resolve(entry)
            .ok_or_else(|| format!("shader '{}' not found", entry.relative_path))?;
        let chain = ShaderChain::load(
            &path,
            &self.device,
            &self.queue,
            Some(self.adapter_info.clone()),
            self.pipeline_cache,
        )?;
        Ok(chain.parameters().to_vec())
    }

    /// Render `source` and write the result to `dest` as a PNG.
    pub fn render_to_png(
        &mut self,
        source: &SourceImage,
        settings: &RenderSettings,
        dest: &Path,
    ) -> Result<(u32, u32), Box<dyn std::error::Error>> {
        let (pixels, w, h) = self.render(source, settings)?;
        crate::image_io::save_png(dest, &pixels, w, h)?;
        Ok((w, h))
    }
}

/// Where along the animation frame `frame` of a run of `total` falls,
/// 0..=1. Keyframe times are proportional, so frame 0 is t=0 and the last
/// frame is t=1 — the same mapping the app's preview and playback producer
/// use (`frame / (total - 1)`). Indices past `total` wrap, so a looped
/// export replays the animation each pass.
fn timeline_position(frame: usize, total: u32) -> f64 {
    let total = total.max(1);
    if total <= 1 {
        return 0.0;
    }
    (frame % total as usize) as f64 / (total - 1) as f64
}

#[cfg(test)]
mod tests {
    use super::timeline_position;

    #[test]
    fn frames_span_the_animation_end_to_end() {
        assert_eq!(timeline_position(0, 24), 0.0);
        assert_eq!(timeline_position(23, 24), 1.0);
        assert!((timeline_position(12, 25) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn a_looped_run_replays_from_the_start() {
        assert_eq!(timeline_position(24, 24), 0.0);
        assert_eq!(timeline_position(47, 24), 1.0);
    }

    #[test]
    fn a_single_frame_sits_at_the_start() {
        assert_eq!(timeline_position(0, 1), 0.0);
        assert_eq!(timeline_position(5, 1), 0.0);
        assert_eq!(timeline_position(5, 0), 0.0);
    }
}
