//! App presets: the entire configuration as a JSON file.
//!
//! Same `"version": 1` format the macOS build reads and writes (see
//! `Sources/CrtApp/AppState.swift`), so the 17 files in `presets/` load here
//! and anything saved here opens on a Mac.
//!
//! ```text
//! { downscale, ntsc, shader, grade, view, timeline, version }
//! ```
//!
//! `grade` is this build's colour-grade stage (`ntscrt_core::grade`), which
//! the macOS app does not have: it ignores the key, and a file without it
//! loads here with the stage off.
//!
//! `ntsc.settings` is ntsc-rs's own preset object, carrying its own version —
//! it passes straight through to the signal stage untouched, which is what
//! keeps presets interchangeable with the ntsc-rs desktop app.
//!
//! The `timeline` section carries keyframe animation — see
//! `ntscrt_core::timeline` for the model and how it interpolates.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AppPreset {
    pub version: u32,
    pub downscale: DownscaleSection,
    pub ntsc: NtscSection,
    pub shader: ShaderSection,
    #[serde(default)]
    pub view: ViewSection,
    /// Source rotation in degrees. A Windows addition, so it defaults to 0
    /// for the macOS presets and for anything written by that build; Swift's
    /// decoder ignores keys it doesn't know, so presets still move both ways.
    #[serde(default)]
    pub rotation: ntscrt_core::Rotation,
    /// Keyframe animation. Absent in a preset that has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<ntscrt_core::Timeline>,
    /// Colour grade. Absent in presets from before the stage, and in
    /// anything the macOS build writes; both load with the stage off.
    #[serde(default, skip_serializing_if = "grade_is_default")]
    pub grade: ntscrt_core::Grade,
}

fn grade_is_default(g: &ntscrt_core::Grade) -> bool {
    *g == ntscrt_core::Grade::default()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DownscaleSection {
    pub enabled: bool,
    /// Matches the Swift `DownscaleMethod` raw values ("nearest+", etc.).
    pub method: String,
    /// Label of the console preset, or "Custom".
    pub preset: String,
    pub width: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NtscSection {
    pub enabled: bool,
    /// ntsc-rs preset JSON, verbatim.
    pub settings: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ShaderSection {
    pub enabled: bool,
    /// Shader preset id, e.g. "royale".
    pub preset: String,
    /// BTreeMap so saved files have a stable key order and diff cleanly.
    pub params: BTreeMap<String, f32>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewSection {
    #[serde(default)]
    pub animate: bool,
    #[serde(default)]
    pub compare: bool,
    #[serde(default)]
    pub integer_scale: bool,
}

impl Default for ViewSection {
    fn default() -> Self {
        Self { animate: false, compare: false, integer_scale: true }
    }
}

impl AppPreset {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let text = std::fs::read_to_string(path)?;
        let preset: AppPreset = serde_json::from_str(&text)?;
        if preset.version != 1 {
            return Err(format!(
                "preset version {} is newer than this build understands (expected 1)",
                preset.version
            )
            .into());
        }
        Ok(preset)
    }

    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Whether this preset carries keyframe animation.
    pub fn has_keyframes(&self) -> bool {
        self.timeline.as_ref().is_some_and(|t| t.has_keys())
    }
}

/// The bundled `presets/*.json`, by display name, sorted.
pub fn bundled() -> Vec<(String, std::path::PathBuf)> {
    let Some(dir) = crate::presets::app_presets_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, std::path::PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .filter_map(|p| {
            p.file_stem()
                .map(|s| (s.to_string_lossy().to_string(), p.clone()))
        })
        .collect();
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AppPreset {
        AppPreset {
            version: 1,
            downscale: DownscaleSection {
                enabled: true,
                method: "nearest+".into(),
                preset: "SNES (256px)".into(),
                width: 256,
            },
            ntsc: NtscSection {
                enabled: true,
                settings: serde_json::json!({ "version": 1, "chroma_noise": true }),
            },
            shader: ShaderSection {
                enabled: true,
                preset: "royale".into(),
                params: [("CURVATURE".to_string(), 1.0)].into_iter().collect(),
            },
            view: ViewSection::default(),
            rotation: ntscrt_core::Rotation::None,
            grade: ntscrt_core::Grade::default(),
            timeline: Some(ntscrt_core::Timeline {
                duration: 2.0,
                fps: 30.0,
                enabled: false,
                keys: vec![ntscrt_core::Keyframe {
                    t: 0.0,
                    easing: ntscrt_core::Easing::Linear,
                    shader: Default::default(),
                    ntsc: Default::default(),
                    grade: Default::default(),
                }],
            }),
        }
    }

    #[test]
    fn round_trips_through_disk() {
        let path = std::env::temp_dir().join("ntscrt-preset-test.json");
        let p = sample();
        p.save(&path).unwrap();

        let back = AppPreset::load(&path).unwrap();
        assert_eq!(back.version, 1);
        assert_eq!(back.downscale.width, 256);
        assert_eq!(back.downscale.method, "nearest+");
        assert_eq!(back.shader.preset, "royale");
        assert_eq!(back.shader.params.get("CURVATURE"), Some(&1.0));
        assert_eq!(back.ntsc.settings["chroma_noise"], serde_json::json!(true));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn keyframes_survive_a_load_and_save_cycle() {
        // This build cannot play keyframes, but must not destroy them.
        let path = std::env::temp_dir().join("ntscrt-preset-keys.json");
        sample().save(&path).unwrap();
        let loaded = AppPreset::load(&path).unwrap();
        assert!(loaded.has_keyframes());

        loaded.save(&path).unwrap();
        let again = AppPreset::load(&path).unwrap();
        assert!(again.has_keyframes(), "keyframes were dropped on re-save");
        assert_eq!(again.timeline.as_ref().unwrap().keys.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_preset_without_keyframes_reports_none() {
        let mut p = sample();
        p.timeline = Some(ntscrt_core::Timeline::default());
        assert!(!p.has_keyframes());
        p.timeline = None;
        assert!(!p.has_keyframes());
    }

    #[test]
    fn a_future_version_is_refused_rather_than_misread() {
        let path = std::env::temp_dir().join("ntscrt-preset-v99.json");
        let mut v = serde_json::to_value(sample()).unwrap();
        v["version"] = serde_json::json!(99);
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();

        let err = AppPreset::load(&path);
        assert!(err.is_err(), "a newer preset version should be refused");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn view_section_defaults_when_absent() {
        let json = r#"{
            "version": 1,
            "downscale": { "enabled": true, "method": "area", "preset": "Custom", "width": 320 },
            "ntsc": { "enabled": false, "settings": {} },
            "shader": { "enabled": true, "preset": "aperture", "params": {} }
        }"#;
        let p: AppPreset = serde_json::from_str(json).unwrap();
        assert!(!p.view.animate);
        assert!(p.view.integer_scale);
    }

    #[test]
    fn grade_round_trips_and_is_off_when_absent() {
        let mut p = sample();
        p.grade.enabled = true;
        p.grade.set("saturation", 0.0);
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"grade\""));
        let back: AppPreset = serde_json::from_str(&json).unwrap();
        assert!(back.grade.enabled);
        assert_eq!(back.grade.get("saturation"), 0.0);

        // A default grade is not written, so old-format readers see nothing new.
        let plain = serde_json::to_string(&sample()).unwrap();
        assert!(!plain.contains("\"grade\""));
        let back: AppPreset = serde_json::from_str(&plain).unwrap();
        assert!(!back.grade.enabled);
    }

    #[test]
    fn every_bundled_preset_parses() {
        let found = bundled();
        if found.is_empty() {
            // The presets/ directory isn't reachable from wherever the test
            // runner sits; nothing to assert.
            return;
        }
        for (name, path) in &found {
            match AppPreset::load(path) {
                Ok(p) => assert_eq!(p.version, 1, "{name} has an unexpected version"),
                Err(e) => panic!("bundled preset '{name}' failed to parse: {e}"),
            }
        }
    }
}
