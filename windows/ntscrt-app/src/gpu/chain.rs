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

use librashader::presets::ShaderPreset;
use librashader::runtime::wgpu::{FilterChain, FilterChainOptions, WgpuOutputView};
use librashader::runtime::{FilterChainParameters, Size, Viewport};
// Not re-exported through the librashader facade in 0.12; cargo unifies the
// version with the one librashader itself depends on.
use librashader_common::shader_features::ShaderFeatures;

pub struct ShaderChain {
    chain: FilterChain,
    /// Parameter names the loaded preset actually exposes, in preset order.
    parameter_names: Vec<String>,
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

        // Parse the preset separately so the parameter list can be read in
        // declaration order — the CRT panel presents controls in the order
        // the shader author wrote them, as the Swift ShaderPanel does.
        let preset = ShaderPreset::try_parse(path, features)?;
        let parameter_names = preset.parameters.iter().map(|p| p.name.to_string()).collect();

        let options = FilterChainOptions {
            enable_cache,
            adapter_info,
            ..Default::default()
        };
        let chain = FilterChain::load_from_preset(preset, device, queue, Some(&options))?;

        Ok(Self { chain, parameter_names, preset_path: path.to_path_buf() })
    }

    pub fn preset_path(&self) -> &Path {
        &self.preset_path
    }

    pub fn parameter_names(&self) -> &[String] {
        &self.parameter_names
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
