//! Headless renderer: owns a wgpu device with no window attached.
//!
//! This is what the CLI verifier and the still-export path use. The GUI
//! borrows eframe's device instead and drives [`gpu::Pipeline`] directly, so
//! both paths run identical kernels and identical shader chains.

use std::path::Path;

use ntscrt_core::{DownscaleMethod, DownscaleSpec, NtscStage, ScanlineGrid};

use crate::gpu::{Pipeline, RenderRequest, ShaderChain, WORK_FORMAT};
use crate::image_io::{padded_bytes_per_row, unpad_rows, SourceImage};

/// One render's worth of configuration.
pub struct RenderSettings {
    /// Retro width the CRT shader sees; None disables the downscale stage.
    pub downscale_width: Option<u32>,
    pub downscale_method: DownscaleMethod,
    pub ntsc_enabled: bool,
    /// ntsc-rs preset JSON. None keeps ntsc-rs defaults.
    pub ntsc_preset_json: Option<String>,
    pub shader_id: String,
    /// Shader parameter overrides applied after the chain loads. Names the
    /// shader doesn't expose are ignored — a preset written against a
    /// different shader build shouldn't fail the whole render.
    pub shader_params: Vec<(String, f32)>,
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
            ntsc_preset_json: None,
            shader_id: "royale".to_string(),
            shader_params: Vec::new(),
            output_height: 960,
            snap_to_scanline_grid: false,
            frame_count: 0,
        }
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
            label: Some("ntscrt.headless"),
            required_features,
            // crt-royale is the heaviest preset in the bundle and needs more
            // than the downlevel defaults allow.
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))?;

        let pipeline = Pipeline::new(&device);
        Ok(Self { device, queue, adapter_info, pipeline, ntsc: NtscStage::new(), pipeline_cache })
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    /// Run `source` through the full pipeline and return RGBA8 pixels plus
    /// the size actually produced (which differs from the request when
    /// `snap_to_scanline_grid` is on).
    pub fn render(
        &mut self,
        source: &SourceImage,
        settings: &RenderSettings,
    ) -> Result<(Vec<u8>, u32, u32), Box<dyn std::error::Error>> {
        if let Some(json) = &settings.ntsc_preset_json {
            self.ntsc.set_settings_json(json)?;
        }

        let entry = crate::presets::find(&settings.shader_id)
            .ok_or_else(|| format!("unknown shader preset '{}'", settings.shader_id))?;
        let preset_path = crate::presets::resolve(entry).ok_or_else(|| {
            format!(
                "shader '{}' not found. Set NTSCRT_SHADERS to a slang-shaders checkout, \
                 or run: git submodule update --init Vendor/slang-shaders",
                entry.relative_path
            )
        })?;
        let mut chain = ShaderChain::load(
            &preset_path,
            &self.device,
            &self.queue,
            Some(self.adapter_info.clone()),
            self.pipeline_cache,
        )?;

        for (name, value) in &settings.shader_params {
            chain.set_parameter(name, *value);
        }

        let downscale = settings.downscale_width.map(|w| {
            DownscaleSpec::for_width(w, source.width, source.height, settings.downscale_method)
        });
        let chain_size = ScanlineGrid::chain_input_size(source.width, source.height, downscale.as_ref());

        // Output width follows the chain input's aspect ratio, then either
        // snaps onto the scanline grid or keeps the requested height.
        let (out_w, out_h) = if settings.snap_to_scanline_grid {
            ScanlineGrid::snapped_size(chain_size.0, chain_size.1, settings.output_height)
        } else {
            let w = (settings.output_height as f64 * chain_size.0 as f64 / chain_size.1 as f64)
                .round()
                .max(1.0) as u32;
            (w, settings.output_height.max(1))
        };

        let output = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ntscrt.output"),
            size: wgpu::Extent3d { width: out_w, height: out_h, depth_or_array_layers: 1 },
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

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ntscrt.render") });

        self.pipeline.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &mut self.ntsc,
            &mut chain,
            &RenderRequest {
                source: &source.pixels,
                source_size: (source.width, source.height),
                downscale,
                ntsc_enabled: settings.ntsc_enabled,
                frame_count: settings.frame_count,
                source_version: 0,
                output_size: (out_w, out_h),
            },
            &output_view,
            WORK_FORMAT,
        )?;

        // ---- readback ----
        let padded = padded_bytes_per_row(out_w);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ntscrt.readback"),
            size: (padded * out_h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output,
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
        Ok((pixels, out_w, out_h))
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
