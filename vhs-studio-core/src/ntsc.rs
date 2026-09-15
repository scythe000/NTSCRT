//! CPU signal-degradation stage (ntsc-rs).
//!
//! Ported from `Sources/CrtCore/NtscStage.swift` plus the C ABI in
//! `Vendor/ntscrs-capi/src/lib.rs`, which this replaces: on Windows the app
//! and the effect are both Rust, so the settings type is used directly
//! instead of being marshalled through JSON across an FFI boundary.
//!
//! The stage keeps the *clean* (pre-effect) pixels of the last frame it was
//! handed, keyed by a caller-supplied version. The effect is destructive and
//! animating a still re-runs it every frame against an unchanged image, so
//! reusing the clean copy saves a full readback per frame — the same
//! optimisation the Swift version makes, minus the GPU round trip (on
//! Windows the caller owns the staging buffer).

use ntsc_rs::settings::standard::NtscEffectFullSettings;
#[cfg(test)]
use ntsc_rs::settings::standard::FilterType;
use ntsc_rs::settings::SettingsList;
use ntsc_rs::yiq_fielding::{Bgrx, BlitInfo, DeinterlaceMode, Rgbx, YiqView};
use ntsc_rs::NtscEffect;

/// Byte order of the 4-byte pixels handed to [`NtscStage::process`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    Bgra8,
}

#[derive(Debug)]
pub enum NtscError {
    /// `row_bytes` was smaller than `width * 4`, or a dimension was zero.
    BadBuffer,
    /// The effect panicked, or the preset JSON did not parse.
    Process(String),
}

impl std::fmt::Display for NtscError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NtscError::BadBuffer => write!(f, "ntsc: buffer dimensions or stride invalid"),
            NtscError::Process(m) => write!(f, "ntsc: {m}"),
        }
    }
}

impl std::error::Error for NtscError {}

pub struct NtscStage {
    settings: NtscEffectFullSettings,
    /// Clean pixels of the last input, and the caller's id for those
    /// contents. See [`NtscStage::process`].
    clean: Vec<u8>,
    clean_version: Option<i64>,
}

impl Default for NtscStage {
    fn default() -> Self {
        Self::new()
    }
}

/// The app's house VHS look, overlaid on ntsc-rs's library defaults — the
/// same dialled-in values as `AppState.appNtscDefaults` in the macOS build,
/// so both apps open on the same picture and Reset returns to it. Keys are
/// ntsc-rs setting ids, as in preset JSON.
///
/// `head_switching_offset` must stay below `head_switching_height` or the
/// switch band leaves the frame and the effect goes dead. `bandwidth_scale`
/// and `vertical_scale` at 0.3 land the whole stage lighter — the raw look
/// was too aggressive out of the box — and `scale_with_video_size` keeps
/// artifact sizes tracking the input, which at 1080p+ would otherwise be
/// proportionally tiny.
pub const HOUSE_DEFAULTS_JSON: &str = r#"{
    "filter_type": 1,
    "composite_preemphasis": 1.106,
    "composite_noise_intensity": 0.204,
    "composite_noise_frequency": 0.8576,
    "composite_noise_detail": 2,
    "snow_intensity": 0,
    "video_scanline_phase_shift_offset": 3,
    "luma_smear": 0.6692,
    "head_switching_height": 8,
    "head_switching_offset": 3,
    "head_switching_horizontal_shift": 41.57,
    "head_switching_mid_line_jitter": 0.181,
    "tracking_noise_height": 63,
    "ringing_power": 5.674,
    "ringing_scale": 4.935,
    "luma_noise_intensity": 0.153,
    "chroma_noise_intensity": 0.201,
    "chroma_noise_frequency": 0.0777,
    "chroma_phase_error": 0.016,
    "chroma_phase_noise_intensity": 0.029,
    "chroma_delay_horizontal": 2.667,
    "chroma_delay_vertical": 2,
    "vhs_chroma_loss": 0.124,
    "scale_with_video_size": true,
    "bandwidth_scale": 0.3,
    "vertical_scale": 0.3
}"#;

impl NtscStage {
    /// ntsc-rs's own defaults, untouched.
    pub fn new() -> Self {
        Self {
            settings: NtscEffectFullSettings::default(),
            clean: Vec::new(),
            clean_version: None,
        }
    }

    /// The house look — what the app starts on and what Reset returns to.
    pub fn house() -> Self {
        let mut stage = Self::new();
        stage
            .reset_to_house_defaults()
            .expect("house defaults are a fixed, valid overlay on ntsc-rs defaults");
        stage
    }

    /// Replace every setting with the house look. The clean-frame cache is
    /// kept: it holds the *input*, which settings don't change.
    pub fn reset_to_house_defaults(&mut self) -> Result<(), NtscError> {
        // A partial preset can't be loaded directly — ntsc-rs fills missing
        // keys from its legacy values, not its defaults — so overlay onto a
        // full default JSON and load that.
        let defaults = Self::new().settings_json()?;
        let mut merged: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&defaults)
            .map_err(|e| NtscError::Process(format!("default settings JSON: {e}")))?;
        let house: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(HOUSE_DEFAULTS_JSON)
                .map_err(|e| NtscError::Process(format!("house defaults JSON: {e}")))?;
        for (k, v) in house {
            merged.insert(k, v);
        }
        let json = serde_json::Value::Object(merged).to_string();
        self.set_settings_json(&json)
    }

    /// Current settings as ntsc-rs preset JSON (`"version": 1`) — the same
    /// format the ntsc-rs desktop app reads and writes, so presets copy/paste
    /// between the two.
    pub fn settings_json(&self) -> Result<String, NtscError> {
        SettingsList::<NtscEffectFullSettings>::new()
            .to_json_string(&self.settings)
            .map_err(|e| NtscError::Process(format!("{e}")))
    }

    /// Replace settings from preset JSON.
    pub fn set_settings_json(&mut self, json: &str) -> Result<(), NtscError> {
        let parsed = SettingsList::<NtscEffectFullSettings>::new()
            .from_json(json)
            .map_err(|e| NtscError::Process(format!("{e}")))?;
        self.settings = parsed;
        Ok(())
    }

    pub fn settings(&self) -> &NtscEffectFullSettings {
        &self.settings
    }

    pub fn settings_mut(&mut self) -> &mut NtscEffectFullSettings {
        &mut self.settings
    }

    /// Apply the effect in place to `data`: `width * height` 4-byte pixels
    /// with `row_bytes` stride.
    ///
    /// `frame_index` drives field selection and the deterministic RNG — the
    /// same index and settings always produce the same pixels, which is what
    /// makes exports reproducible.
    ///
    /// `input_version` identifies the *contents* of `data`. While it stays
    /// the same across calls, the clean pixels captured on the first call are
    /// reused, so an animated still is degraded fresh each frame rather than
    /// compounding the previous frame's artifacts. Pass a frame index for
    /// video, a constant for a fixed still, or `None` to treat every call as
    /// new content.
    ///
    /// Note: ntsc-rs writes alpha as opaque; callers needing source alpha
    /// must save and restore it themselves.
    pub fn process(
        &mut self,
        data: &mut [u8],
        format: PixelFormat,
        width: u32,
        height: u32,
        row_bytes: u32,
        frame_index: i64,
        input_version: Option<i64>,
    ) -> Result<(), NtscError> {
        if width == 0 || height == 0 || row_bytes < width * 4 {
            return Err(NtscError::BadBuffer);
        }
        let needed = row_bytes as usize * height as usize;
        if data.len() < needed {
            return Err(NtscError::BadBuffer);
        }
        let buf = &mut data[..needed];

        // Restore the clean pixels when the caller says the input is
        // unchanged, otherwise snapshot what we were just handed.
        let cache_usable = input_version.is_some()
            && input_version == self.clean_version
            && self.clean.len() == needed;
        if cache_usable {
            buf.copy_from_slice(&self.clean);
        } else {
            self.clean.clear();
            self.clean.extend_from_slice(buf);
            self.clean_version = input_version;
        }

        let dims = (width as usize, height as usize);
        let frame = frame_index.max(0) as usize;
        let stride = row_bytes as usize;

        let effect: NtscEffect = (&self.settings).into();
        let field = effect.use_field.to_yiq_field(frame);
        let mut yiq_buf = vec![0f32; YiqView::buf_length_for(dims, field)];
        let mut view = YiqView::from_parts(&mut yiq_buf, dims, field);
        let blit = BlitInfo::from_full_frame(dims.0, dims.1, stride);

        match format {
            PixelFormat::Bgra8 => {
                view.set_from_strided_buffer::<Bgrx, u8, _>(buf, blit, ());
                effect.apply_effect_to_yiq(&mut view, frame, [1.0, 1.0]);
                view.write_to_strided_buffer::<Bgrx, u8, _>(buf, blit, DeinterlaceMode::Bob, ());
            }
            PixelFormat::Rgba8 => {
                view.set_from_strided_buffer::<Rgbx, u8, _>(buf, blit, ());
                effect.apply_effect_to_yiq(&mut view, frame, [1.0, 1.0]);
                view.write_to_strided_buffer::<Rgbx, u8, _>(buf, blit, DeinterlaceMode::Bob, ());
            }
        }
        Ok(())
    }

    /// Drop the cached clean frame — call when the source image changes and
    /// the caller has no meaningful version to pass.
    pub fn invalidate(&mut self) {
        self.clean.clear();
        self.clean_version = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey_frame(w: u32, h: u32) -> Vec<u8> {
        vec![128u8; (w * h * 4) as usize]
    }

    #[test]
    fn default_settings_round_trip_through_json() {
        let mut stage = NtscStage::new();
        let json = stage.settings_json().expect("serialize");
        assert!(json.contains("version"));
        stage.set_settings_json(&json).expect("deserialize");
    }

    #[test]
    fn house_defaults_change_the_look_and_every_key_is_a_real_setting() {
        let raw = NtscStage::new();
        let house = NtscStage::house();
        let s = house.settings();
        // Spot-check values that differ from ntsc-rs's own defaults.
        assert!((s.luma_smear - 0.6692).abs() < 1e-4);
        assert!((s.composite_sharpening - 1.106).abs() < 1e-4);
        assert!(matches!(s.filter_type, FilterType::Butterworth));
        let scale = s.scale.settings.clone();
        assert!((scale.horizontal_scale - 0.3).abs() < 1e-6);
        assert!((scale.vertical_scale - 0.3).abs() < 1e-6);
        assert!(scale.scale_with_video_size);
        assert_ne!(
            raw.settings_json().unwrap(),
            house.settings_json().unwrap(),
            "the house look must differ from the library defaults"
        );

        // Every house key must name a setting in this ntsc-rs, or it is
        // silently ignored and the look drifts from the macOS build's.
        let defaults: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&raw.settings_json().unwrap()).unwrap();
        let overlay: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(HOUSE_DEFAULTS_JSON).unwrap();
        for key in overlay.keys() {
            assert!(defaults.contains_key(key), "house default '{key}' is not an ntsc-rs setting");
        }
    }

    #[test]
    fn reset_returns_to_the_house_look_after_edits() {
        let mut stage = NtscStage::house();
        let before = stage.settings_json().unwrap();
        stage.settings_mut().luma_smear = 0.0;
        assert_ne!(stage.settings_json().unwrap(), before);
        stage.reset_to_house_defaults().unwrap();
        assert_eq!(stage.settings_json().unwrap(), before);
    }

    #[test]
    fn rejects_short_stride() {
        let mut stage = NtscStage::new();
        let mut buf = grey_frame(16, 16);
        let err = stage.process(&mut buf, PixelFormat::Rgba8, 16, 16, 16 * 4 - 1, 0, None);
        assert!(matches!(err, Err(NtscError::BadBuffer)));
    }

    #[test]
    fn processing_changes_pixels_and_is_deterministic() {
        let (w, h) = (64u32, 48u32);
        let mut stage = NtscStage::new();

        let mut a = grey_frame(w, h);
        stage.process(&mut a, PixelFormat::Rgba8, w, h, w * 4, 0, None).unwrap();
        assert_ne!(a, grey_frame(w, h), "effect should alter a flat frame");

        let mut b = grey_frame(w, h);
        stage.process(&mut b, PixelFormat::Rgba8, w, h, w * 4, 0, None).unwrap();
        assert_eq!(a, b, "same frame index must produce identical pixels");
    }

    #[test]
    fn clean_cache_prevents_compounding() {
        let (w, h) = (64u32, 48u32);
        let mut stage = NtscStage::new();

        // Same version twice: the second call must degrade the *clean*
        // frame again, not the already-degraded output of the first.
        let mut first = grey_frame(w, h);
        stage.process(&mut first, PixelFormat::Rgba8, w, h, w * 4, 7, Some(1)).unwrap();
        let mut second = first.clone();
        stage.process(&mut second, PixelFormat::Rgba8, w, h, w * 4, 7, Some(1)).unwrap();
        assert_eq!(first, second, "cached clean pixels should be reused");
    }
}
