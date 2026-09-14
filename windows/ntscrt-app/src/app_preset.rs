//! App presets: the entire configuration as a JSON file.
//!
//! Same `"version": 1` format the macOS build reads and writes (see
//! `Sources/CrtApp/AppState.swift`), so the 17 files in `presets/` load here
//! and anything saved here opens on a Mac.
//!
//! ```text
//! { downscale, ntsc, shader, view, timeline, version }
//! ```
//!
//! `ntsc.settings` is ntsc-rs's own preset object, carrying its own version —
//! it passes straight through to the signal stage untouched, which is what
//! keeps presets interchangeable with the ntsc-rs desktop app.
//!
//! The `timeline` section belongs to keyframe animation, which this build
//! does not implement. It is deliberately preserved verbatim rather than
//! dropped: loading a preset that carries keyframes and saving it again must
//! not silently destroy someone's animation.

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
    /// Opaque: read back out exactly as it came in. See the module note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<serde_json::Value>,
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

    /// Whether this preset carries keyframes this build cannot play back.
    /// The UI says so on load rather than letting the animation go missing
    /// without explanation.
    pub fn has_keyframes(&self) -> bool {
        self.timeline
            .as_ref()
            .and_then(|t| t.get("keys"))
            .and_then(|k| k.as_array())
            .is_some_and(|k| !k.is_empty())
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
            timeline: Some(serde_json::json!({
                "duration": 2, "enabled": false, "fps": 30,
                "keys": [{ "t": 0.0 }]
            })),
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
        assert_eq!(
            again.timeline.as_ref().unwrap()["keys"].as_array().unwrap().len(),
            1
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_preset_without_keyframes_reports_none() {
        let mut p = sample();
        p.timeline = Some(serde_json::json!({ "keys": [] }));
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
