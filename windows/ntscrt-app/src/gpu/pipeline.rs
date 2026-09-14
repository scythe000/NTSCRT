//! Frame orchestration, ported from `Sources/CrtCore/Pipeline.swift` and the
//! `SupersampledPass` half of `Sources/CrtCore/ScanlineGrid.swift`.
//!
//! Signal order, unchanged from the macOS build:
//!
//! ```text
//! source → NTSC/VHS degradation (full res, CPU) → downscale → CRT shader → output
//! ```
//!
//! The NTSC stage runs on the CPU at the source's full resolution — matching
//! how ntsc-rs is used standalone, so its `scale_settings` can size artifacts
//! against the real input — and the degraded signal is then downscaled into
//! the shader.
//!
//! One structural difference from the Swift: there, the NTSC round trip is
//! GPU→CPU→GPU because the source lives in a Metal texture. Here the source
//! is already a CPU buffer owned by the app (stills are decoded into RAM),
//! so the degraded pixels are uploaded once and the readback disappears.

use ntscrt_core::{DownscaleSpec, NtscStage, PixelFormat, ScanlineGrid};

use super::chain::ShaderChain;
use super::downscaler::{Downscaler, WORK_FORMAT};

/// Everything needed to render one frame.
pub struct RenderRequest<'a> {
    /// Full-resolution source pixels, RGBA8, tightly packed.
    pub source: &'a [u8],
    pub source_size: (u32, u32),
    /// Retro resolution the shader sees. None renders the source directly.
    pub downscale: Option<DownscaleSpec>,
    /// None leaves the signal clean (the NTSC panel switched off).
    pub ntsc_enabled: bool,
    /// Drives ntsc-rs's deterministic RNG and the shaders' animation uniforms.
    pub frame_count: usize,
    /// Identifies the source contents so the NTSC stage can skip re-copying
    /// unchanged pixels. See `NtscStage::process`.
    pub source_version: i64,
    pub output_size: (u32, u32),
}

pub struct Pipeline {
    downscaler: Downscaler,
    /// Staging buffer for the NTSC stage, reused across frames.
    ntsc_buf: Vec<u8>,
    /// Texture the chain reads: either the uploaded source or the downscale
    /// target, depending on whether the downscale stage is on.
    chain_input: Option<(u32, u32, wgpu::Texture)>,
    /// Full-resolution upload target when the downscale stage is enabled.
    upload: Option<(u32, u32, wgpu::Texture)>,
    /// Oversized render target for the supersampled scanline pass.
    supersample: Option<(u32, u32, wgpu::Texture)>,
}

impl Pipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            downscaler: Downscaler::new(device),
            ntsc_buf: Vec::new(),
            chain_input: None,
            upload: None,
            supersample: None,
        }
    }

    /// Render one frame into `output_view`.
    ///
    /// `output_format` must match the view; egui's offscreen target and the
    /// export target use the same [`WORK_FORMAT`], but the surface may differ.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        ntsc: &mut NtscStage,
        chain: &mut ShaderChain,
        request: &RenderRequest<'_>,
        output_view: &wgpu::TextureView,
        output_format: wgpu::TextureFormat,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (sw, sh) = request.source_size;
        if sw == 0 || sh == 0 {
            return Ok(());
        }

        // ---- NTSC stage (CPU, full resolution) ----
        let row_bytes = sw * 4;
        let needed = (row_bytes * sh) as usize;
        let pixels: &[u8] = if request.ntsc_enabled {
            if self.ntsc_buf.len() != needed {
                self.ntsc_buf.resize(needed, 0);
            }
            self.ntsc_buf.copy_from_slice(&request.source[..needed]);
            ntsc.process(
                &mut self.ntsc_buf,
                PixelFormat::Rgba8,
                sw,
                sh,
                row_bytes,
                request.frame_count as i64,
                Some(request.source_version),
            )?;
            &self.ntsc_buf
        } else {
            &request.source[..needed]
        };

        // ---- upload + downscale ----
        let chain_size = ScanlineGrid::chain_input_size(sw, sh, request.downscale.as_ref());

        let chain_input = match request.downscale {
            Some(spec) => {
                // Upload at full res, then run the kernel into the chain input.
                let upload = Self::obtain(device, &mut self.upload, sw, sh, "ntscrt.upload", false);
                Self::write(queue, upload, pixels, sw, sh);
                let up_view = upload.create_view(&Default::default());

                let dst = Self::obtain(
                    device, &mut self.chain_input, spec.width, spec.height, "ntscrt.chain_input", false,
                );
                let dst_view = dst.create_view(&Default::default());
                self.downscaler.encode(
                    device, encoder,
                    &up_view, (sw, sh),
                    &dst_view, (spec.width, spec.height),
                    spec.method,
                );
                dst
            }
            None => {
                let tex = Self::obtain(device, &mut self.chain_input, sw, sh, "ntscrt.chain_input", false);
                Self::write(queue, tex, pixels, sw, sh);
                tex
            }
        };

        // ---- CRT shader ----
        // CRT shaders draw scanlines in *output* pixels. When the output
        // height isn't a whole multiple of the chain input height the phase
        // drifts down the frame and the scanlines band. Render at a whole
        // multiple and integrate down when that applies.
        let k = ScanlineGrid::supersample_factor(chain_size.1, request.output_size.1);
        let super_size = (chain_size.0 * k, chain_size.1 * k);
        let use_supersample = k > 1
            && super_size.0 > request.output_size.0
            && super_size.1 > request.output_size.1;

        if use_supersample {
            let big = Self::obtain(
                device, &mut self.supersample, super_size.0, super_size.1, "ntscrt.supersample", true,
            );
            let big_view = big.create_view(&Default::default());
            chain.render(encoder, chain_input, &big_view, super_size, WORK_FORMAT, request.frame_count)?;
            self.downscaler.encode(
                device, encoder,
                &big_view, super_size,
                output_view, request.output_size,
                ntscrt_core::DownscaleMethod::Area,
            );
        } else {
            chain.render(
                encoder, chain_input, output_view,
                request.output_size, output_format, request.frame_count,
            )?;
        }
        Ok(())
    }

    /// Get or recreate a cached texture at the requested size.
    ///
    /// `render_target` textures are written by the shader chain and read by
    /// the area integrator, so they need RENDER_ATTACHMENT; the rest are
    /// written by compute or by queue upload.
    fn obtain<'t>(
        device: &wgpu::Device,
        slot: &'t mut Option<(u32, u32, wgpu::Texture)>,
        width: u32,
        height: u32,
        label: &str,
        render_target: bool,
    ) -> &'t wgpu::Texture {
        let matches = matches!(slot, Some((w, h, _)) if *w == width && *h == height);
        if !matches {
            let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
            if render_target {
                usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
            } else {
                usage |= wgpu::TextureUsages::STORAGE_BINDING;
            }
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORK_FORMAT,
                usage,
                view_formats: &[],
            });
            *slot = Some((width, height, tex));
        }
        &slot.as_ref().unwrap().2
    }

    fn write(queue: &wgpu::Queue, tex: &wgpu::Texture, pixels: &[u8], width: u32, height: u32) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }
}
