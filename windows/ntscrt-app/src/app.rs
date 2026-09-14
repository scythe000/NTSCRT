//! Application state and the eframe shell, ported from
//! `Sources/CrtApp/AppState.swift` and `Views/ContentView.swift`.
//!
//! The macOS app owns an `MTKView` that redraws on demand; here the preview
//! is rendered into an offscreen wgpu texture which egui then draws as an
//! image. That keeps the pipeline identical to the export path — the preview
//! is the same render at a different size, not a separate code path.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;
use ntscrt_core::{DownscaleMethod, DownscaleSpec, NtscStage, ScanlineGrid};

use crate::gpu::{Pipeline, RenderRequest, ShaderChain, WORK_FORMAT};
use crate::image_io::SourceImage;

/// Console presets pick a horizontal resolution; the vertical always follows
/// the source's aspect ratio, so any input shape works. Matches
/// `Views/DownscalePanel.swift`.
pub const DOWNSCALE_PRESETS: &[(&str, u32)] = &[
    ("SNES (256px)", 256),
    ("NES (256px)", 256),
    ("VGA (320px)", 320),
    ("Arcade (384px)", 384),
    ("VGA\u{00B2} (640px)", 640),
];

pub struct NtscrtApp {
    // ---- source ----
    pub source: SourceImage,
    pub source_path: Option<PathBuf>,
    /// Bumped whenever the source pixels change, so the NTSC stage knows to
    /// re-read them instead of reusing its cached clean copy.
    pub source_version: i64,

    // ---- downscale ----
    pub downscale_enabled: bool,
    pub downscale_width: u32,
    pub downscale_preset: String,
    pub downscale_method: DownscaleMethod,

    // ---- ntsc ----
    pub ntsc_enabled: bool,
    pub ntsc: NtscStage,

    // ---- shader ----
    pub shader_id: String,
    pub shader_params: HashMap<String, f32>,
    /// Descriptions keyed by parameter name, for the hyllian "*" gate rule.
    pub shader_param_descriptions: HashMap<String, String>,
    chain: Option<ShaderChain>,

    // ---- view ----
    pub animate: bool,
    pub compare: bool,
    /// Split position as a fraction of preview width, 0..1.
    pub compare_split: f32,
    pub zoom: f32,
    pub frame_count: usize,

    // ---- export ----
    pub export_height: u32,
    pub snap_to_scanline_grid: bool,

    // ---- gpu ----
    pipeline: Pipeline,
    preview_texture: Option<(u32, u32, wgpu::Texture, egui::TextureId)>,
    pipeline_cache: bool,

    /// Untouched source uploaded for the compare split, keyed by
    /// `source_version` so it is re-uploaded only when the image changes.
    pub(crate) source_texture: Option<(i64, egui::TextureHandle)>,

    pub status: Option<String>,
    pub error: Option<String>,
    /// Set when settings change; clears once a frame has been rendered.
    dirty: bool,
}

impl NtscrtApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .expect("ntscrt requires the wgpu backend");
        let pipeline = Pipeline::new(&render_state.device);
        let pipeline_cache = render_state
            .device
            .features()
            .contains(wgpu::Features::PIPELINE_CACHE);

        let mut app = Self {
            source: SourceImage::solid(320, 240, [16, 16, 24, 255]),
            source_path: None,
            source_version: 0,
            downscale_enabled: true,
            downscale_width: 320,
            downscale_preset: "VGA (320px)".to_string(),
            downscale_method: DownscaleMethod::Area,
            ntsc_enabled: true,
            ntsc: NtscStage::new(),
            shader_id: "royale".to_string(),
            shader_params: HashMap::new(),
            shader_param_descriptions: HashMap::new(),
            chain: None,
            animate: false,
            compare: false,
            compare_split: 0.5,
            zoom: 1.0,
            frame_count: 0,
            export_height: 960,
            snap_to_scanline_grid: false,
            pipeline,
            preview_texture: None,
            pipeline_cache,
            source_texture: None,
            status: None,
            error: None,
            dirty: true,
        };
        app.reload_chain(&render_state.device, &render_state.queue);
        app
    }

    /// Downscale spec for the current source, or None when the stage is off.
    pub fn downscale_spec(&self) -> Option<DownscaleSpec> {
        self.downscale_enabled.then(|| {
            DownscaleSpec::for_width(
                self.downscale_width,
                self.source.width,
                self.source.height,
                self.downscale_method,
            )
        })
    }

    /// What the shader actually sees — the gate rules need this.
    pub fn chain_input_size(&self) -> (u32, u32) {
        ScanlineGrid::chain_input_size(
            self.source.width,
            self.source.height,
            self.downscale_spec().as_ref(),
        )
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn load_source(&mut self, path: PathBuf) {
        match SourceImage::load(&path) {
            Ok(img) => {
                self.status = Some(format!(
                    "Loaded {} ({}x{})",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    img.width,
                    img.height
                ));
                self.source = img;
                self.source_path = Some(path);
                self.source_version += 1;
                self.ntsc.invalidate();
                self.error = None;
                self.dirty = true;
            }
            Err(e) => self.error = Some(format!("Could not open {}: {e}", path.display())),
        }
    }

    pub fn reload_chain(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(entry) = crate::presets::find(&self.shader_id) else {
            self.error = Some(format!("Unknown shader '{}'", self.shader_id));
            return;
        };
        let Some(path) = crate::presets::resolve(entry) else {
            self.error = Some(format!(
                "Shader '{}' not found. Set NTSCRT_SHADERS to a slang-shaders checkout, \
                 or run: git submodule update --init Vendor/slang-shaders",
                entry.relative_path
            ));
            self.chain = None;
            return;
        };
        match ShaderChain::load(&path, device, queue, None, self.pipeline_cache) {
            Ok(chain) => {
                self.shader_params.clear();
                self.shader_param_descriptions.clear();
                for name in chain.parameter_names() {
                    if let Some(v) = chain.parameter(name) {
                        self.shader_params.insert(name.clone(), v);
                    }
                }
                self.chain = Some(chain);
                self.error = None;
                self.dirty = true;
            }
            Err(e) => {
                self.error = Some(format!("Could not load shader '{}': {e}", entry.display_name));
                self.chain = None;
            }
        }
    }

    pub fn set_shader_param(&mut self, name: &str, value: f32) {
        if let Some(chain) = &self.chain {
            chain.set_parameter(name, value);
        }
        self.shader_params.insert(name.to_string(), value);
        self.dirty = true;
    }

    pub fn has_chain(&self) -> bool {
        self.chain.is_some()
    }

    /// Render the preview into the offscreen texture, returning its egui id
    /// and pixel size.
    fn render_preview(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        size: (u32, u32),
    ) -> Option<(egui::TextureId, (u32, u32))> {
        if self.chain.is_none() {
            return None;
        }
        if size.0 == 0 || size.1 == 0 {
            return None;
        }

        let device = &render_state.device;
        let queue = &render_state.queue;

        // (Re)allocate the offscreen target when the size changes.
        let needs_alloc = !matches!(&self.preview_texture, Some((w, h, _, _)) if *w == size.0 && *h == size.1);
        if needs_alloc {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ntscrt.preview"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORK_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let mut renderer = render_state.renderer.write();
            let id = match self.preview_texture.take() {
                Some((_, _, _, old_id)) => {
                    renderer.update_egui_texture_from_wgpu_texture(
                        device, &view, wgpu::FilterMode::Nearest, old_id,
                    );
                    old_id
                }
                None => renderer.register_native_texture(device, &view, wgpu::FilterMode::Nearest),
            };
            self.preview_texture = Some((size.0, size.1, texture, id));
            self.dirty = true;
        }

        // Everything the request needs is read up front: the render below
        // borrows several fields of `self` at once, so no method taking
        // `&self` (like `downscale_spec`) may be called inside it.
        let downscale = self.downscale_spec();
        let ntsc_enabled = self.ntsc_enabled;
        let frame_count = self.frame_count;
        let source_version = self.source_version;
        let id = self.preview_texture.as_ref().unwrap().3;

        if !(self.dirty || self.animate) {
            return Some((id, size));
        }

        // Destructuring splits the borrow: `pipeline`, `ntsc`, `chain`,
        // `source` and `preview_texture` are disjoint fields, so each can be
        // borrowed independently.
        let Self { pipeline, ntsc, chain, source, preview_texture, .. } = self;
        let Some(chain) = chain.as_mut() else { return Some((id, size)) };

        let view = preview_texture
            .as_ref()
            .unwrap()
            .2
            .create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ntscrt.preview.encode"),
        });
        let request = RenderRequest {
            source: &source.pixels,
            source_size: (source.width, source.height),
            downscale,
            ntsc_enabled,
            frame_count,
            source_version,
            output_size: size,
        };
        let result = pipeline.render(
            device, queue, &mut encoder, ntsc, chain, &request, &view, WORK_FORMAT,
        );

        match result {
            Ok(()) => {
                queue.submit(Some(encoder.finish()));
                self.dirty = false;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Render failed: {e}")),
        }
        Some((id, size))
    }

    /// Preview render size: the CRT pass always renders at a whole multiple
    /// of the chain input (at least 6 rows per line — the same supersampling
    /// exports use) and is then fitted to the window, so what you see never
    /// depends on how big the window is.
    fn preview_render_size(&self, available: egui::Vec2) -> (u32, u32) {
        let (cw, ch) = self.chain_input_size();
        if cw == 0 || ch == 0 {
            return (1, 1);
        }
        // Enough rows per source line to resolve the scanline profile.
        let mut k = 6u32;
        // Don't exceed what the window can show by much, and stay inside
        // sane texture sizes.
        let target_h = (available.y.max(1.0) * self.zoom) as u32;
        while k > 1 && (ch * k > target_h.saturating_mul(2) || ch * k > 4096) {
            k -= 1;
        }
        ((cw * k).max(1), (ch * k).max(1))
    }
}

impl eframe::App for NtscrtApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Leave Animate on for the real experience: tape noise, jitter and
        // interlacing only move when the frame index advances.
        if self.animate {
            self.frame_count = self.frame_count.wrapping_add(1);
            ctx.request_repaint();
        }

        let render_state = frame.wgpu_render_state().cloned();

        crate::ui::top_bar(self, ui, render_state.as_ref());
        crate::ui::sidebar(self, ui, render_state.as_ref());
        crate::ui::status_bar(self, ui);
        crate::ui::preview(self, ui, render_state.as_ref());

        // Files dropped anywhere on the window load as the source, matching
        // the macOS Source panel's drag & drop.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect()
        });
        if let Some(path) = dropped.into_iter().next() {
            self.load_source(path);
        }
    }
}

impl NtscrtApp {
    /// Used by the preview panel; kept here so the texture cache stays private.
    pub(crate) fn preview_image(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        available: egui::Vec2,
    ) -> Option<(egui::TextureId, (u32, u32))> {
        let size = self.preview_render_size(available);
        self.render_preview(render_state, size)
    }

    /// Shared by the sidebar's Export button and the CLI path.
    pub(crate) fn export_png(&mut self, dest: PathBuf) {
        let settings = crate::RenderSettings {
            downscale_width: self.downscale_enabled.then_some(self.downscale_width),
            downscale_method: self.downscale_method,
            ntsc_enabled: self.ntsc_enabled,
            ntsc_preset_json: self.ntsc.settings_json().ok(),
            shader_id: self.shader_id.clone(),
            output_height: self.export_height,
            snap_to_scanline_grid: self.snap_to_scanline_grid,
            frame_count: self.frame_count,
        };
        // A separate headless device keeps the export off the presenting
        // device, so a long render cannot stall the UI's swapchain.
        match crate::HeadlessRenderer::new()
            .and_then(|mut r| r.render_to_png(&self.source, &settings, &dest))
        {
            Ok((w, h)) => {
                self.status = Some(format!("Exported {} ({w}x{h})", dest.display()));
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Export failed: {e}")),
        }
    }
}

/// eframe hands out `RenderState` behind an `Arc`-ish clone; alias it so the
/// UI modules don't need the egui_wgpu path.
pub type RenderState = egui_wgpu::RenderState;
