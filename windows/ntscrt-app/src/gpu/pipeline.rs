//! Frame orchestration, ported from `Sources/CrtCore/Pipeline.swift` and the
//! `SupersampledPass` half of `Sources/CrtCore/ScanlineGrid.swift`.
//!
//! Signal order, unchanged from the macOS build:
//!
//! ```text
//! source → NTSC/VHS degradation (full res, CPU) → downscale → grade → CRT shader → output
//! ```
//!
//! The grade is this build's own addition (`ntscrt_core::grade`): a colour
//! pass on the chain input, downstream of everything the frame cache keeps.
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
use super::grade::GradePass;

/// Everything needed to render one frame.
pub struct RenderRequest<'a> {
    /// Full-resolution source pixels, RGBA8, tightly packed.
    pub source: &'a [u8],
    pub source_size: (u32, u32),
    /// Retro resolution the shader sees. None renders the source directly.
    pub downscale: Option<DownscaleSpec>,
    /// None leaves the signal clean (the NTSC panel switched off).
    pub ntsc_enabled: bool,
    /// Off bypasses the CRT shader: the chain input is scaled to the output
    /// with nearest sampling instead, showing the signal stage on its own.
    pub shader_enabled: bool,
    /// Colour grade, packed for the GPU (`Grade::uniform`). None skips the
    /// pass — the caller checks `Grade::is_identity`, so a preset from
    /// before the stage costs nothing.
    pub grade: Option<[f32; ntscrt_core::GRADE_UNIFORM_LEN]>,
    /// Drives ntsc-rs's deterministic RNG and the shaders' animation uniforms.
    pub frame_count: usize,
    /// Identifies the source contents so the NTSC stage can skip re-copying
    /// unchanged pixels. See `NtscStage::process`.
    pub source_version: i64,
    pub output_size: (u32, u32),
    /// A chain input that is already finished — the NTSC stage *and* the
    /// downscale have run. Set by the frame cache on a hit (see
    /// `video::ChainInputCache`), in which case both stages are skipped and
    /// `source`, `downscale` and `ntsc_enabled` are ignored.
    ///
    /// Note this is not how the playback producer's output arrives: that has
    /// only the NTSC stage baked in and still needs downscaling, so it comes
    /// through `source` with `ntsc_enabled` off.
    pub prepared_chain_input: Option<&'a wgpu::Texture>,
}

pub struct Pipeline {
    downscaler: Downscaler,
    grade: GradePass,
    /// Staging buffer for the NTSC stage, reused across frames.
    ntsc_buf: Vec<u8>,
    /// Texture the chain reads: either the uploaded source or the downscale
    /// target, depending on whether the downscale stage is on.
    chain_input: Option<(u32, u32, wgpu::Texture)>,
    /// Full-resolution upload target when the downscale stage is enabled.
    upload: Option<(u32, u32, wgpu::Texture)>,
    /// Oversized render target for the supersampled scanline pass.
    supersample: Option<(u32, u32, wgpu::Texture)>,
    /// Whatever fed the shader chain last. The frame cache copies out of it
    /// after a render, which is how a played frame gets into the RAM preview
    /// without being recomputed. Textures are handles, so this is a refcount
    /// bump, not a copy.
    last_chain_input: Option<wgpu::Texture>,
}

impl Pipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            downscaler: Downscaler::new(device),
            grade: GradePass::new(device),
            ntsc_buf: Vec::new(),
            chain_input: None,
            upload: None,
            supersample: None,
            last_chain_input: None,
        }
    }

    /// The texture that fed the shader chain on the last successful render.
    ///
    /// This is the chain input the frame cache stores: NTSC applied and
    /// downscaled, the small representation the RAM preview keeps per frame.
    pub fn last_chain_input(&self) -> Option<&wgpu::Texture> {
        self.last_chain_input.as_ref()
    }

    /// Copy the last render's chain input into a texture of its own, which is
    /// what the frame cache keeps.
    ///
    /// The copy is necessary: the pipeline's own chain-input texture is
    /// reused by the very next frame, so handing the cache that handle would
    /// give it a slot whose contents keep changing. Encodes into `encoder`;
    /// the caller submits.
    pub fn copy_last_chain_input(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Option<(wgpu::Texture, (u32, u32))> {
        let source = self.last_chain_input.as_ref()?;
        let (width, height) = (source.width(), source.height());
        let copy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ntscrt.cached_chain_input"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORK_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &copy,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        Some((copy, (width, height)))
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

        // A cache hit arrives with both CPU stages already done, so the whole
        // NTSC/upload/downscale half below is skipped and the chain reads the
        // stored texture directly — that is the entire point of the RAM
        // preview.
        // Textures are refcounted handles, so the clones here are bookkeeping,
        // not copies — and taking one ends the borrow of `self` that
        // `obtain` holds, leaving the downscaler free to be used below.
        let (chain_input, chain_size) = match request.prepared_chain_input {
            Some(tex) => (tex.clone(), (tex.width(), tex.height())),
            None => {
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
                let size = ScanlineGrid::chain_input_size(sw, sh, request.downscale.as_ref());
                let tex: wgpu::Texture = match request.downscale {
                    Some(spec) => {
                        // Upload at full res, then run the kernel into the chain input.
                        let upload =
                            Self::obtain(device, &mut self.upload, sw, sh, "ntscrt.upload", false);
                        Self::write(queue, upload, pixels, sw, sh);
                        let up_view = upload.create_view(&Default::default());

                        let dst = Self::obtain(
                            device, &mut self.chain_input, spec.width, spec.height,
                            "ntscrt.chain_input", false,
                        );
                        let dst_view = dst.create_view(&Default::default());
                        self.downscaler.encode(
                            device, encoder,
                            &up_view, (sw, sh),
                            &dst_view, (spec.width, spec.height),
                            spec.method,
                        );
                        dst.clone()
                    }
                    None => {
                        let tex = Self::obtain(
                            device, &mut self.chain_input, sw, sh, "ntscrt.chain_input", false,
                        );
                        Self::write(queue, tex, pixels, sw, sh);
                        tex.clone()
                    }
                };
                (tex, size)
            }
        };
        // The cache keeps the *ungraded* chain input: the grade is cheap to
        // rerun and is the one stage a user dials while a video plays.
        self.last_chain_input = Some(chain_input.clone());

        // ---- colour grade ----
        let chain_input = match request.grade.as_ref() {
            Some(uniform) => self.grade.encode(device, encoder, &chain_input, uniform),
            None => chain_input,
        };
        let chain_input = &chain_input;

        if !request.shader_enabled {
            // Shader off: show the chain input as it is, each retro pixel a
            // hard block, which is what the Mac's blit does too. The output
            // target is always [`WORK_FORMAT`] with storage usage — the
            // supersample path below writes to it the same way.
            let view = chain_input.create_view(&Default::default());
            self.downscaler.encode(
                device, encoder,
                &view, chain_size,
                output_view, request.output_size,
                ntscrt_core::DownscaleMethod::Nearest,
            );
            return Ok(());
        }

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

    /// Upload `pixels` and run the downscale into a texture of its own,
    /// stopping short of the shader chain.
    ///
    /// This is the pre-render path: while a video sits paused, frames are
    /// decoded and processed in the background and turned into chain inputs
    /// for the cache, with nothing drawn. `pixels` already has the NTSC stage
    /// applied (the producer did it), so only the downscale is left.
    pub fn encode_chain_input_copy(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        pixels: &[u8],
        size: (u32, u32),
        downscale: Option<DownscaleSpec>,
    ) -> Option<(wgpu::Texture, (u32, u32))> {
        let (sw, sh) = size;
        if sw == 0 || sh == 0 || pixels.len() < (sw as usize * sh as usize * 4) {
            return None;
        }
        let out_size = ScanlineGrid::chain_input_size(sw, sh, downscale.as_ref());
        let copy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ntscrt.cached_chain_input"),
            size: wgpu::Extent3d {
                width: out_size.0,
                height: out_size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORK_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });

        match downscale {
            Some(spec) => {
                let upload = Self::obtain(device, &mut self.upload, sw, sh, "ntscrt.upload", false);
                Self::write(queue, upload, pixels, sw, sh);
                let up_view = upload.create_view(&Default::default());
                let dst_view = copy.create_view(&Default::default());
                self.downscaler.encode(
                    device, encoder,
                    &up_view, (sw, sh),
                    &dst_view, out_size,
                    spec.method,
                );
            }
            // With the stage off the chain input is the frame itself, so
            // there is nothing to run — just put the pixels where they go.
            None => Self::write(queue, &copy, pixels, sw, sh),
        }
        Some((copy, out_size))
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
            // COPY_SRC throughout: the frame cache copies the chain input out
            // after a render (see `copy_last_chain_input`).
            let mut usage = wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC;
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
