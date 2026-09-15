//! Application state and the eframe shell, ported from
//! `Sources/CrtApp/AppState.swift` and `Views/ContentView.swift`.
//!
//! The macOS app owns an `MTKView` that redraws on demand; here the preview
//! is rendered into an offscreen wgpu texture which egui then draws as an
//! image. That keeps the pipeline identical to the export path — the preview
//! is the same render at a different size, not a separate code path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;
use ntscrt_core::{DownscaleMethod, DownscaleSpec, NtscStage, Rotation, ScanlineGrid};

use crate::app_video::VideoState;
use crate::gpu::{chain::ShaderParamMeta, Pipeline, RenderRequest, ShaderChain, WORK_FORMAT};
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
    /// Applied to the source before anything else. See `ntscrt_core::rotation`
    /// for why it has to come first.
    pub rotation: Rotation,
    /// The loaded still as it came off disk, before rotation. None while a
    /// video is loaded — there the frames arrive rotated from the producer.
    ///
    /// `source` is always pipeline-ready: rotation has already been applied,
    /// so nothing downstream has to know rotation exists.
    pub(crate) original_source: Option<crate::image_io::SourceImage>,
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
    /// Off bypasses the CRT shader: the preview and exports show the chain
    /// input (the NTSC stage and downscale) upscaled with nearest sampling,
    /// so the signal stage can be judged on its own.
    pub shader_enabled: bool,
    pub shader_id: String,
    /// Current value per parameter name.
    pub shader_params: HashMap<String, f32>,
    /// Declared metadata (label, range, step) in shader declaration order.
    pub shader_param_meta: Vec<ShaderParamMeta>,
    chain: Option<ShaderChain>,
    /// Parameter values of shaders switched away from, so coming back to
    /// one restores what was dialled in rather than its defaults.
    saved_shader_params: HashMap<String, HashMap<String, f32>>,

    // ---- video ----
    /// The clip, when the source is a video rather than a still. Playback,
    /// the frame cache and the transport bar all live behind this — see
    /// `app_video`.
    pub(crate) video: Option<VideoState>,

    // ---- view ----
    pub animate: bool,
    pub compare: bool,
    /// Split position as a fraction of preview width, 0..1.
    pub compare_split: f32,
    /// Display magnification of the preview; 1 fits the panel.
    pub zoom: f32,
    /// Where the zoomed image sits, as an offset of its centre from the
    /// panel's centre in points. Only meaningful when it is larger than
    /// the panel.
    pub pan: egui::Vec2,
    /// Lock the displayed image to whole-pixel multiples of the chain input
    /// so every scanline is the same height on screen.
    pub integer_scale: bool,
    pub frame_count: usize,

    // ---- export ----
    /// Requested size of the output's longer side; the other follows the
    /// source aspect. See [`NtscrtApp::export_output_size`].
    pub export_long_edge: u32,
    pub snap_to_scanline_grid: bool,
    /// Movie export settings. Ignored when the source is a still and
    /// `export_still_frames` is None — that case writes a PNG.
    pub export_job: crate::video::ExportJob,
    /// A movie export running on its own thread, if any.
    pub export_task: Option<ExportTask>,
    /// Name of the preset currently loaded, so the Preset menu can tick it.
    /// Cleared as soon as any setting it controls is edited — a tick that
    /// survives edits would claim the preset is still what you are seeing.
    pub active_preset: Option<String>,
    /// Frames of VHS motion to write when exporting a *still* as video.
    /// None keeps the still export a PNG, as it has always been.
    pub export_still_frames: Option<u32>,

    // ---- gpu ----
    pub(crate) pipeline: Pipeline,
    preview_texture: Option<(u32, u32, wgpu::Texture, egui::TextureId)>,
    pipeline_cache: bool,

    /// Untouched source uploaded for the compare split, keyed by
    /// `source_version` so it is re-uploaded only when the image changes.
    pub(crate) source_texture: Option<(i64, egui::TextureHandle)>,

    /// Keyframe animation for the loaded preset, if it has any.
    pub timeline: Option<ntscrt_core::Timeline>,
    /// Playhead position, 0..1 along the timeline.
    pub playhead: f64,
    /// Previewing the keyframe animation on a still (a video uses playback).
    pub timeline_playing: bool,
    /// Whether the timeline bar is shown.
    pub timeline_open: bool,
    /// Wall-clock pacing for the timeline preview and Animate, so neither
    /// runs at the display's refresh rate. See `pacer`.
    timeline_pacer: crate::pacer::Pacer,
    animate_pacer: crate::pacer::Pacer,

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
            rotation: Rotation::None,
            original_source: None,
            source_version: 0,
            downscale_enabled: true,
            downscale_width: 320,
            downscale_preset: "VGA (320px)".to_string(),
            downscale_method: DownscaleMethod::Area,
            ntsc_enabled: true,
            ntsc: NtscStage::house(),
            video: None,
            shader_enabled: true,
            // The macOS build opens on CRT Glow (Gaussian) too.
            shader_id: "glow_gauss".to_string(),
            shader_params: HashMap::new(),
            shader_param_meta: Vec::new(),
            chain: None,
            saved_shader_params: HashMap::new(),
            animate: false,
            compare: false,
            compare_split: 0.5,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            integer_scale: true,
            frame_count: 0,
            export_long_edge: 1920,
            snap_to_scanline_grid: false,
            export_job: crate::video::ExportJob::default(),
            export_task: None,
            active_preset: None,
            export_still_frames: None,
            pipeline,
            preview_texture: None,
            pipeline_cache,
            source_texture: None,
            timeline: None,
            playhead: 0.0,
            timeline_playing: false,
            timeline_open: false,
            timeline_pacer: Default::default(),
            animate_pacer: Default::default(),
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
            let (sw, sh) = self.source_size();
            DownscaleSpec::for_width(self.downscale_width, sw, sh, self.downscale_method)
        })
    }

    /// Size of what the pipeline consumes, i.e. after rotation. The
    /// downscale's derived height, the output aspect and the scanline grid
    /// all key off this rather than the file's own dimensions.
    pub fn source_size(&self) -> (u32, u32) {
        (self.source.width, self.source.height)
    }

    /// Rebuild `source` from the unrotated original. Stills only — video
    /// frames arrive already rotated.
    fn rebuild_rotation(&mut self) {
        let Some(original) = self.original_source.as_ref() else { return };
        if self.rotation == Rotation::None {
            self.source = original.clone();
            return;
        }
        let (pixels, width, height) = ntscrt_core::rotate_rgba(
            &original.pixels,
            original.width,
            original.height,
            self.rotation,
        );
        self.source = crate::image_io::SourceImage { width, height, pixels };
    }

    /// Rotate a freshly decoded video frame, which arrives unrotated from a
    /// seek or the first-frame fetch.
    pub(crate) fn rotate_decoded(
        &self,
        img: crate::image_io::SourceImage,
    ) -> crate::image_io::SourceImage {
        if self.rotation == Rotation::None {
            return img;
        }
        let (pixels, width, height) =
            ntscrt_core::rotate_rgba(&img.pixels, img.width, img.height, self.rotation);
        crate::image_io::SourceImage { width, height, pixels }
    }

    /// Turn the source a quarter turn clockwise.
    pub fn rotate_cw(&mut self) {
        self.set_rotation(self.rotation.next_cw());
    }

    pub fn set_rotation(&mut self, rotation: Rotation) {
        if self.rotation == rotation {
            return;
        }
        self.rotation = rotation;
        self.rebuild_rotation();
        // A video's current frame is still the old orientation; re-fetch it
        // so the preview turns immediately rather than at the next frame.
        self.refresh_rotated_video_frame();
        // The source pixels the NTSC stage sees have changed shape, so its
        // cached clean copy and every cached frame are stale.
        self.source_version += 1;
        self.ntsc.invalidate();
        self.mark_chain_input_edited();
    }

    /// What the shader actually sees — the gate rules need this.
    pub fn chain_input_size(&self) -> (u32, u32) {
        let (sw, sh) = self.source_size();
        ScanlineGrid::chain_input_size(sw, sh, self.downscale_spec().as_ref())
    }

    /// A setting the loaded preset controls has changed, so the menu tick
    /// no longer describes what is on screen.
    pub fn leave_preset(&mut self) {
        self.active_preset = None;
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn load_source(&mut self, path: PathBuf) {
        // Videos take the whole other path: decoder, playback, frame cache.
        if crate::video::is_video_path(&path) {
            self.load_video(path);
            return;
        }
        self.video = None;
        match SourceImage::load(&path) {
            Ok(img) => {
                self.status = Some(format!(
                    "Loaded {} ({}x{})",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    img.width,
                    img.height
                ));
                self.original_source = Some(img);
                self.rebuild_rotation();
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
                let saved = self.saved_shader_params.remove(&self.shader_id);
                self.shader_params.clear();
                self.shader_param_meta = chain.parameters().to_vec();
                for p in &self.shader_param_meta {
                    // Start from the house default when the app has one,
                    // else what the chain holds (the .slangp may override
                    // the shader's declared value), else the declaration —
                    // then restore what the user last dialled in here.
                    let v = saved
                        .as_ref()
                        .and_then(|s| s.get(&p.name).copied())
                        .unwrap_or_else(|| self.shader_default(&p.name, p.initial, Some(&chain)));
                    chain.set_parameter(&p.name, v);
                    self.shader_params.insert(p.name.clone(), v);
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

    /// Switch to another bundled shader, remembering this one's parameter
    /// values so switching back restores them. A no-op for the current id.
    pub fn select_shader(&mut self, id: &str, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.shader_id == id {
            return;
        }
        if !self.shader_params.is_empty() {
            self.saved_shader_params
                .insert(self.shader_id.clone(), self.shader_params.clone());
        }
        self.shader_id = id.to_string();
        self.reload_chain(device, queue);
    }

    /// The value a parameter opens on for the current shader: the house
    /// override when there is one, else what the loaded chain holds (the
    /// .slangp may set it), else the shader's declaration.
    fn shader_default(&self, name: &str, declared: f32, chain: Option<&ShaderChain>) -> f32 {
        crate::presets::house_shader_default(&self.shader_id, name)
            .or_else(|| chain.and_then(|c| c.parameter(name)))
            .unwrap_or(declared)
    }

    /// Put every parameter of the current shader back to its default.
    pub fn reset_shader_params(&mut self) {
        // The chain's values are the *current* ones by now, so the fallback
        // has to be the declaration, not the chain.
        let values: Vec<(String, f32)> = self
            .shader_param_meta
            .iter()
            .map(|p| (p.name.clone(), self.shader_default(&p.name, p.initial, None)))
            .collect();
        for (name, v) in values {
            self.set_shader_param(&name, v);
        }
        self.leave_preset();
        self.auto_key_if_parked();
    }

    /// Put the whole signal stage back to the house look.
    pub fn reset_ntsc(&mut self) {
        if let Err(e) = self.ntsc.reset_to_house_defaults() {
            self.error = Some(format!("Reset failed: {e}"));
            return;
        }
        self.mark_chain_input_edited();
        self.auto_key_if_parked();
    }

    pub fn set_shader_param(&mut self, name: &str, value: f32) {
        if let Some(chain) = &self.chain {
            chain.set_parameter(name, value);
        }
        self.shader_params.insert(name.to_string(), value);
        self.dirty = true;
    }

    /// Turn the CRT shader on or off. Cached video frames stay valid: they
    /// hold the chain *input*, which the shader only reads.
    pub fn set_shader_enabled(&mut self, on: bool) {
        if self.shader_enabled != on {
            self.shader_enabled = on;
            self.leave_preset();
            self.mark_dirty();
        }
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
        let shader_enabled = self.shader_enabled;
        let frame_count = self.frame_count;
        let source_version = self.source_version;
        let id = self.preview_texture.as_ref().unwrap().3;

        if !(self.dirty || self.animate) {
            return Some((id, size));
        }

        // Destructuring splits the borrow: `pipeline`, `ntsc`, `chain`,
        // `source`, `video` and `preview_texture` are disjoint fields, so
        // each can be borrowed independently.
        let Self { pipeline, ntsc, chain, source, preview_texture, video, .. } = self;
        let Some(chain) = chain.as_mut() else { return Some((id, size)) };

        // Video supplies the chain input in two cheaper shapes than "run
        // everything on the decoded frame":
        //
        //  - a cache hit is finished — NTSC and downscale both done — and
        //    goes straight to the shader chain;
        //  - the playback producer's output has the NTSC stage baked in
        //    already, so only the downscale is left.
        //
        // Anything else (a still, or a video frame nothing has touched yet)
        // is the ordinary full path.
        let prepared = video.as_ref().and_then(|v| v.cached_chain_input.as_ref());
        let processed = video.as_ref().and_then(|v| v.processed_source.as_deref());
        let (pixels, ntsc_enabled) = match processed {
            Some(pixels) => (pixels, false),
            None => (source.pixels.as_slice(), ntsc_enabled),
        };

        let view = preview_texture
            .as_ref()
            .unwrap()
            .2
            .create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ntscrt.preview.encode"),
        });
        let request = RenderRequest {
            source: pixels,
            source_size: (source.width, source.height),
            downscale,
            ntsc_enabled,
            shader_enabled,
            frame_count,
            source_version,
            prepared_chain_input: prepared,
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

        // A playing video drives its own frame index off the wall clock, so
        // Animate only applies to stills. Neither may advance once per
        // repaint — that would tie their speed to the display's refresh
        // rate — so both are paced against the clock, like the macOS app.
        let now = std::time::Instant::now();
        if self.is_playing() {
            self.consume_playback_frame();
            ctx.request_repaint();
        } else if self.animate {
            // Leave Animate on for the real experience: tape noise, jitter
            // and interlacing only move when the frame index advances. NTSC
            // is a 30 fps format, and the stage is CPU work at full source
            // resolution, so that is where Animate is capped.
            let fps = if self.ntsc_enabled { 30.0 } else { 60.0 };
            if self.animate_pacer.frames_due(now, fps) > 0 {
                self.frame_count = self.frame_count.wrapping_add(1);
            }
            ctx.request_repaint_after(self.animate_pacer.until_next(now));
        } else {
            self.animate_pacer.reset();
        }
        // Previewing a keyframe animation advances the playhead itself, at
        // the timeline's own frame rate.
        if self.timeline_playing {
            let (_, fps) = self.effective_timeline();
            let due = self.timeline_pacer.frames_due(now, fps);
            if due > 0 {
                self.tick_timeline_preview(due);
            }
            ctx.request_repaint_after(self.timeline_pacer.until_next(now));
        } else {
            self.timeline_pacer.reset();
        }

        // An export publishes progress from its own thread, so the window
        // has to keep repainting to show it moving.
        self.poll_export();
        if self.export_task.is_some() {
            ctx.request_repaint();
        }

        let render_state = frame.wgpu_render_state().cloned();

        crate::ui::top_bar(self, ui, render_state.as_ref());
        crate::ui::sidebar(self, ui, render_state.as_ref());
        crate::ui::status_bar(self, ui);
        crate::ui::timeline(self, ui);
        crate::ui::transport_bar(self, ui);
        crate::ui::preview(self, ui, render_state.as_ref());

        if let Some(rs) = render_state.as_ref() {
            // The chain input the preview just produced is worth keeping for
            // the RAM preview; and while paused, keep filling the cache in
            // the background so the next play-through needs no CPU work.
            if self.is_playing() {
                self.cache_rendered_chain_input(&rs.device, &rs.queue);
            } else if self.video.is_some() {
                self.tick_prerender(&rs.device, &rs.queue);
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }

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

    /// Capture the current configuration as a preset.
    ///
    /// `timeline` is carried over from whatever was last loaded so keyframes
    /// this build cannot play are not destroyed by a save.
    pub(crate) fn to_preset(&self) -> crate::app_preset::AppPreset {
        use crate::app_preset::*;
        AppPreset {
            version: 1,
            downscale: DownscaleSection {
                enabled: self.downscale_enabled,
                method: self.downscale_method.raw_value().to_string(),
                preset: self.downscale_preset.clone(),
                width: self.downscale_width,
            },
            ntsc: NtscSection {
                enabled: self.ntsc_enabled,
                settings: self
                    .ntsc
                    .settings_json()
                    .ok()
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or(serde_json::Value::Null),
            },
            shader: ShaderSection {
                enabled: self.shader_enabled,
                preset: self.shader_id.clone(),
                params: self.shader_params.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            },
            view: ViewSection {
                animate: self.animate,
                compare: self.compare,
                integer_scale: self.integer_scale,
            },
            rotation: self.rotation,
            timeline: self.timeline.clone(),
        }
    }

    /// Apply a loaded preset. Returns a human-readable note when something
    /// in the preset could not be applied, so the UI can say so rather than
    /// leaving the user to spot it.
    pub(crate) fn apply_preset(
        &mut self,
        preset: crate::app_preset::AppPreset,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<String> {
        use std::str::FromStr;
        let mut notes: Vec<String> = Vec::new();

        self.downscale_enabled = preset.downscale.enabled;
        self.downscale_width = preset.downscale.width.clamp(16, 4096);
        self.downscale_preset = preset.downscale.preset.clone();
        match DownscaleMethod::from_str(&preset.downscale.method) {
            Ok(m) => self.downscale_method = m,
            Err(()) => notes.push(format!(
                "unknown downscale method '{}', kept {}",
                preset.downscale.method,
                self.downscale_method.display_name()
            )),
        }

        self.ntsc_enabled = preset.ntsc.enabled;
        if !preset.ntsc.settings.is_null() {
            let json = preset.ntsc.settings.to_string();
            if let Err(e) = self.ntsc.set_settings_json(&json) {
                notes.push(format!("NTSC settings not applied: {e}"));
            }
        }

        // Swap the shader before pushing parameters — the chain owns them,
        // and a chain for the wrong preset would reject the names.
        self.shader_enabled = preset.shader.enabled;
        if crate::presets::find(&preset.shader.preset).is_some() {
            self.select_shader(&preset.shader.preset, device, queue);
            let mut unknown = 0usize;
            for (name, value) in &preset.shader.params {
                if self.shader_params.contains_key(name) {
                    self.set_shader_param(name, *value);
                } else {
                    unknown += 1;
                }
            }
            if unknown > 0 {
                notes.push(format!(
                    "{unknown} shader parameter(s) in the preset aren't in this shader build"
                ));
            }
        } else {
            notes.push(format!("unknown shader '{}'", preset.shader.preset));
        }

        // Every preset animates on load, whatever its own `view.animate`
        // says. These looks are built out of tape noise, jitter and
        // tracking error — frozen on one frame a preset shows you a still
        // that happens to be noisy, not the effect it is actually for. Four
        // of the bundled presets store `animate: false`; the flag is still
        // round-tripped on save, it just doesn't decide what you see.
        self.animate = true;
        self.compare = preset.view.compare;
        self.integer_scale = preset.view.integer_scale;
        self.set_rotation(preset.rotation);

        let keyed = preset.has_keyframes();
        if keyed {
            let n = preset.timeline.as_ref().map(|t| t.keys.len()).unwrap_or(0);
            notes.push(format!("{n} keyframes"));
        }
        self.timeline = preset.timeline;
        self.timeline_playing = false;
        // A preset carrying keyframes opens the timeline, whether or not it
        // was open when the preset was saved — otherwise the animation is
        // loaded but invisible, and the preset looks like it did nothing.
        // On a still it also starts previewing, since the animation *is*
        // the look; a video's transport owns playback.
        if keyed {
            self.timeline_open = true;
            if self.video.is_none() {
                self.timeline_playing = true;
            }
        }
        self.scrub_timeline(0.0);

        self.mark_dirty();
        (!notes.is_empty()).then(|| notes.join(" \u{2014} "))
    }

    /// Whether Export writes a movie rather than a PNG: a loaded video
    /// always does, and a still does when asked for VHS-motion frames.
    pub(crate) fn exports_video(&self) -> bool {
        self.video.is_some() || self.export_still_frames.is_some()
    }

    /// Write whatever the Export button says it will — a movie or a PNG —
    /// to `dest`.
    pub fn export_from_ui(&mut self, dest: PathBuf) {
        if self.exports_video() {
            self.export_video(dest);
        } else {
            self.export_png(dest);
        }
    }

    /// Start rendering the whole source and encoding it, on its own thread.
    ///
    /// Export is minutes of work at 4K and every frame goes through the CPU
    /// signal stage, so running it inline would freeze the window for the
    /// duration. The thread gets its own headless device — the same reason
    /// the still export has always had one — and reports back through
    /// [`ExportTask`], which the UI polls once a frame.
    pub(crate) fn export_video(&mut self, dest: PathBuf) {
        if self.export_task.is_some() {
            self.error = Some("An export is already running".into());
            return;
        }
        let Some(source_path) = self.source_path.clone() else {
            self.error = Some("Nothing loaded to export".into());
            return;
        };
        let settings = self.render_settings();
        let job = crate::video::ExportJob { dest: dest.clone(), ..self.export_job };
        let still = self
            .export_still_frames
            .filter(|_| self.video.is_none())
            .map(|n| (n, 24.0));

        let shared = Arc::new(Mutex::new(ExportProgress::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_shared = Arc::clone(&shared);
        let worker_cancel = Arc::clone(&cancel);

        let handle = std::thread::Builder::new()
            .name("ntscrt.export".into())
            .spawn(move || {
                let result = crate::HeadlessRenderer::new().and_then(|mut r| {
                    crate::video::export(
                        &mut r,
                        &settings,
                        &job,
                        &source_path,
                        still,
                        &mut |done, total| {
                            if let Ok(mut p) = worker_shared.lock() {
                                p.done = done;
                                p.total = total;
                            }
                            !worker_cancel.load(Ordering::Relaxed)
                        },
                    )
                });
                if let Ok(mut p) = worker_shared.lock() {
                    p.finished = Some(result.map_err(|e| e.to_string()));
                }
            });

        match handle {
            Ok(handle) => {
                self.status = Some(format!("Exporting {}...", dest.display()));
                self.error = None;
                self.export_task = Some(ExportTask { progress: shared, cancel, handle, dest });
            }
            Err(e) => self.error = Some(format!("Could not start export: {e}")),
        }
    }

    /// Ask a running export to stop at the next frame.
    pub(crate) fn cancel_export(&mut self) {
        if let Some(task) = &self.export_task {
            task.cancel.store(true, Ordering::Relaxed);
            self.status = Some("Cancelling export...".into());
        }
    }

    /// Fold a finished export back into the UI. Called once a frame.
    fn poll_export(&mut self) {
        let Some(task) = &self.export_task else { return };
        let finished = task.progress.lock().ok().and_then(|p| p.finished.clone());
        let Some(result) = finished else { return };

        // The worker has already stored its result, so this joins at once.
        let task = self.export_task.take().unwrap();
        let _ = task.handle.join();

        match result {
            Ok(s) => {
                self.status = Some(format!(
                    "Exported {} ({}x{}, {} frames{}, {:.1} MB)",
                    task.dest.display(),
                    s.width,
                    s.height,
                    s.frames,
                    match s.audio {
                        crate::video::AudioMode::None => "",
                        crate::video::AudioMode::Copied => ", audio copied",
                        crate::video::AudioMode::Reencoded => ", audio re-encoded to AAC",
                    },
                    s.bytes as f64 / (1024.0 * 1024.0)
                ));
                self.error = None;
            }
            // Cancelling is something the user asked for, not a failure.
            Err(e) if e.contains("cancelled") => {
                self.status = Some("Export cancelled".into());
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Export failed: {e}")),
        }
    }

    /// The size the export will actually have — see [`export_output_size`].
    pub fn export_output_size(&self) -> (u32, u32) {
        export_output_size(
            self.export_long_edge,
            self.chain_input_size(),
            self.snap_to_scanline_grid,
            self.exports_video(),
        )
    }

    /// The current configuration as render settings. Shared by every export
    /// path so a still and a movie cannot drift apart.
    pub(crate) fn render_settings(&self) -> crate::RenderSettings {
        crate::RenderSettings {
            downscale_width: self.downscale_enabled.then_some(self.downscale_width),
            downscale_method: self.downscale_method,
            ntsc_enabled: self.ntsc_enabled,
            shader_enabled: self.shader_enabled,
            rotation: self.rotation,
            timeline: self.timeline.clone(),
            ntsc_preset_json: self.ntsc.settings_json().ok(),
            shader_id: self.shader_id.clone(),
            shader_params: self.shader_params.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            output_height: self.export_output_size().1,
            snap_to_scanline_grid: self.snap_to_scanline_grid,
            frame_count: self.frame_count,
        }
    }

    pub(crate) fn export_png(&mut self, dest: PathBuf) {
        let settings = self.render_settings();
        // A separate headless device keeps the export off the presenting
        // device, so a long render cannot stall the UI's swapchain.
        match crate::HeadlessRenderer::new()
                        // The original, not `effective_source()`: `settings.rotation`
            // makes the renderer do the turn, and doing both would rotate
            // twice.
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

/// What a running export publishes for the UI to read.
#[derive(Default)]
pub struct ExportProgress {
    pub done: u32,
    pub total: u32,
    /// Set once, when the worker stops. `Err` carries the message as a
    /// string because the error is not `Send`.
    pub finished: Option<Result<crate::video::ExportSummary, String>>,
}

/// A movie export in flight.
pub struct ExportTask {
    pub progress: Arc<Mutex<ExportProgress>>,
    cancel: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
    dest: PathBuf,
}

impl ExportTask {
    /// `(done, total)` as of the last completed frame.
    pub fn counts(&self) -> (u32, u32) {
        self.progress
            .lock()
            .map(|p| (p.done, p.total))
            .unwrap_or((0, 0))
    }

    pub fn fraction(&self) -> f32 {
        let (done, total) = self.counts();
        if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        }
    }

    pub fn is_cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// eframe hands out `RenderState` behind an `Arc`-ish clone; alias it so the
/// UI modules don't need the egui_wgpu path.
pub type RenderState = egui_wgpu::RenderState;

/// Output dimensions for an export, from the requested long edge and the
/// chain input the shader sees. The long edge is the side the user asked
/// for and the other follows the chain's aspect, as `ExportPopover` does on
/// the Mac; the width is then derived from the height the same way the
/// renderer does it, so the number shown is the number written. With
/// `snap` on the size is rounded onto the scanline grid instead, and movie
/// encoders need `even` dimensions.
pub fn export_output_size(long_edge: u32, chain: (u32, u32), snap: bool, even: bool) -> (u32, u32) {
    let (cw, ch) = (chain.0.max(1) as f64, chain.1.max(1) as f64);
    let long_edge = long_edge.max(16) as f64;
    let height = if cw >= ch { (long_edge * ch / cw).round().max(16.0) } else { long_edge };
    let height = height as u32;
    let (mut w, mut h) = if snap {
        ntscrt_core::ScanlineGrid::snapped_size(chain.0, chain.1, height)
    } else {
        ((height as f64 * cw / ch).round().max(1.0) as u32, height)
    };
    if even {
        w &= !1;
        h &= !1;
    }
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::export_output_size;

    #[test]
    fn long_edge_is_the_wide_side_of_a_landscape_source() {
        assert_eq!(export_output_size(1920, (320, 240), false, false), (1920, 1440));
        assert_eq!(export_output_size(1920, (640, 360), false, false), (1920, 1080));
    }

    #[test]
    fn long_edge_is_the_tall_side_of_a_portrait_source() {
        assert_eq!(export_output_size(1920, (240, 320), false, false), (1440, 1920));
    }

    #[test]
    fn movies_get_even_dimensions() {
        // 1366x768 → 1079 tall → 1919 wide before evening.
        let (w, h) = export_output_size(1920, (1366, 768), false, true);
        assert_eq!((w % 2, h % 2), (0, 0));
        assert_eq!((w, h), (1918, 1078));
    }

    #[test]
    fn snapping_lands_on_a_whole_number_of_rows_per_line() {
        let (w, h) = export_output_size(1000, (320, 240), true, false);
        assert_eq!(h % 240, 0, "{h} rows is not a multiple of 240 lines");
        assert_eq!(w, h * 320 / 240);
    }

    #[test]
    fn a_degenerate_chain_does_not_divide_by_zero() {
        let (w, h) = export_output_size(1920, (0, 0), false, false);
        assert!(w >= 1 && h >= 16);
    }
}
