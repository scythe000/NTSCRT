//! librashader filter chain, replacing the Objective-C bridge in
//! `Sources/CrtAppBridge/LibrashaderBridge.m`.
//!
//! The macOS build reaches librashader through its C ABI because the host is
//! Swift. Here the host is Rust, so the chain is used as a normal crate: no
//! dynamic library to ship, no header to keep in sync, and preset parameters
//! come back as typed values instead of C strings.
//!
//! Backend note: this uses librashader's wgpu runtime, so one chain covers
//! both the D3D12 and Vulkan adapters wgpu may pick. The macOS build pins
//! librashader to 76462c03 because later versions shifted crt-royale's
//! output; that pin is Metal-specific and does not carry over — cross-backend
//! bit-parity with the Mac renders is not a goal Windows can meet.

use std::path::Path;

use librashader::presets::{ShaderPreset, ShaderPresetPack};
use librashader::runtime::wgpu::{error::FilterChainError, FilterChain, FilterChainOptions, WgpuOutputView};
use librashader::runtime::{FilterChainParameters, Size, Viewport};
// Not re-exported through the librashader facade in 0.12; cargo unifies the
// version with the one librashader itself depends on.
use librashader_common::shader_features::ShaderFeatures;

/// One runtime-tweakable shader parameter, as the shader author declared it
/// with `#pragma parameter`.
///
/// The macOS `ShaderPanel` gets this from librashader's C API; here it comes
/// off the preset pack. Without the range the UI can only offer a bare number
/// field, and without the description the hyllian "*" gate rule (see
/// `param_gates`) has nothing to match on.
#[derive(Debug, Clone)]
pub struct ShaderParamMeta {
    pub name: String,
    /// Human-readable label from the shader source.
    pub description: String,
    pub initial: f32,
    pub minimum: f32,
    pub maximum: f32,
    pub step: f32,
}

impl ShaderParamMeta {
    /// True when the declared range is usable for a slider. Some shaders
    /// declare a degenerate range (min == max, or a zero/NaN step), and
    /// egui's slider panics or misbehaves on those.
    pub fn has_usable_range(&self) -> bool {
        self.minimum.is_finite()
            && self.maximum.is_finite()
            && self.maximum > self.minimum
    }

    /// Whether the shader declared this as an on/off switch: a 0..1 range
    /// stepping by 1. Those read far better as a checkbox than a slider.
    pub fn is_toggle(&self) -> bool {
        self.minimum == 0.0 && self.maximum == 1.0 && self.step == 1.0
    }
}

pub struct ShaderChain {
    chain: FilterChain,
    /// Parameters the loaded preset exposes, in the order the shader author
    /// declared them — the CRT panel presents controls in that order, as the
    /// Swift ShaderPanel does.
    parameters: Vec<ShaderParamMeta>,
    preset_path: std::path::PathBuf,
}

impl ShaderChain {
    /// Load a `.slangp` preset onto `device`/`queue`.
    ///
    /// `adapter_info` only names the shader cache index; without it separate
    /// adapters share one "wgpu" entry and clobber each other.
    ///
    /// `enable_cache` must only be true when the device was created with
    /// [`wgpu::Features::PIPELINE_CACHE`] — librashader creates a pipeline
    /// cache unconditionally when asked, and wgpu treats the missing feature
    /// as a fatal validation error rather than degrading.
    pub fn load(
        path: &Path,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        adapter_info: Option<wgpu::AdapterInfo>,
        enable_cache: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // ORIGINAL_ASPECT and FRAMETIME uniforms are what the CRT presets in
        // slang-shaders expect to be available; sensors are irrelevant here.
        let features = ShaderFeatures::ORIGINAL_ASPECT_UNIFORMS | ShaderFeatures::FRAMETIME_UNIFORMS;

        // Three steps rather than one `load_from_path`, because the parameter
        // metadata is only available in between: the preset fixes the
        // declaration order, the pack carries each parameter's description and
        // range from its `#pragma parameter` line, and the chain consumes the
        // pack. Going straight from path to chain would throw both away.
        let preset = ShaderPreset::try_parse(path, features)?;
        let order: Vec<String> = preset.parameters.iter().map(|p| p.name.to_string()).collect();

        let pack = ShaderPresetPack::load_from_preset::<FilterChainError>(preset)?;

        // Merge every pass's parameter table. A multi-pass preset can declare
        // the same parameter in more than one pass; first declaration wins,
        // which is the order the panel shows.
        let mut meta: std::collections::HashMap<String, ShaderParamMeta> =
            std::collections::HashMap::new();
        for pass in &pack.passes {
            for (name, p) in &pass.data.parameters {
                meta.entry(name.to_string()).or_insert_with(|| ShaderParamMeta {
                    name: p.id.to_string(),
                    description: p.description.clone(),
                    initial: p.initial,
                    minimum: p.minimum,
                    maximum: p.maximum,
                    step: p.step,
                });
            }
        }

        // Preset order first; anything a pass declares but the preset doesn't
        // list still gets a control, appended in name order so it is stable.
        let mut parameters: Vec<ShaderParamMeta> =
            order.iter().filter_map(|n| meta.remove(n)).collect();
        let mut leftover: Vec<ShaderParamMeta> = meta.into_values().collect();
        leftover.sort_by(|a, b| a.name.cmp(&b.name));
        parameters.extend(leftover);

        let options = FilterChainOptions {
            enable_cache,
            adapter_info,
            ..Default::default()
        };
        let chain = FilterChain::load_from_pack(pack, device, queue, Some(&options))?;

        Ok(Self { chain, parameters, preset_path: path.to_path_buf() })
    }

    pub fn preset_path(&self) -> &Path {
        &self.preset_path
    }

    /// Parameters in the order the shader author declared them.
    pub fn parameters(&self) -> &[ShaderParamMeta] {
        &self.parameters
    }

    pub fn parameter(&self, name: &str) -> Option<f32> {
        self.chain.parameters().parameter_value(name)
    }

    pub fn set_parameter(&self, name: &str, value: f32) {
        self.chain.parameters().set_parameter_value(name, value);
    }

    /// Encode the chain: `input` -> `output`, filling `output` entirely.
    ///
    /// `frame_count` drives the shaders' own animation uniforms; passing the
    /// same count with the same parameters reproduces the same pixels, which
    /// is what keeps exports deterministic.
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::Texture,
        output_view: &wgpu::TextureView,
        output_size: (u32, u32),
        output_format: wgpu::TextureFormat,
        frame_count: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let size = Size::new(output_size.0, output_size.1);
        let out = WgpuOutputView::new_from_raw(output_view, size, output_format);
        let viewport = Viewport { x: 0.0, y: 0.0, mvp: None, output: out, size };
        self.chain.frame(input, &viewport, encoder, frame_count, None)?;
        Ok(())
    }
}
