//! Bundled shader catalog and asset locations.
//!
//! Ported from `Sources/CrtCore/Presets.swift` and `Sources/CrtApp/Paths.swift`.
//! The macOS build resolves these inside the .app bundle; on Windows we look
//! next to the executable first (how the app ships), then walk up to the repo
//! checkout so `cargo run` works from a source tree.

use std::path::{Path, PathBuf};

/// One of the seven CRT shaders bundled with the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetEntry {
    pub id: &'static str,
    pub display_name: &'static str,
    /// Path relative to the slang-shaders root, e.g. "crt/crt-aperture.slangp".
    pub relative_path: &'static str,
}

pub const ALL: &[PresetEntry] = &[
    PresetEntry { id: "aperture",     display_name: "CRT Aperture",        relative_path: "crt/crt-aperture.slangp" },
    PresetEntry { id: "easymode",     display_name: "CRT Easymode",        relative_path: "crt/crt-easymode.slangp" },
    PresetEntry { id: "glow_gauss",   display_name: "CRT Glow (Gaussian)", relative_path: "crt/crtglow_gauss.slangp" },
    PresetEntry { id: "glow_lanczos", display_name: "CRT Glow (Lanczos)",  relative_path: "crt/crtglow_lanczos.slangp" },
    PresetEntry { id: "hyllian",      display_name: "CRT Hyllian",         relative_path: "crt/crt-hyllian.slangp" },
    PresetEntry { id: "royale",       display_name: "CRT Royale",          relative_path: "crt/crt-royale.slangp" },
    PresetEntry { id: "sim",          display_name: "CRT Sim",             relative_path: "crt/crtsim.slangp" },
];

pub fn find(id: &str) -> Option<&'static PresetEntry> {
    ALL.iter().find(|p| p.id == id)
}

/// House tweaks to a shader's declared parameter defaults, per shader id —
/// the same values as `AppState.appShaderDefaults` in the macOS build. These
/// are what a freshly selected shader opens on and what its Reset returns to.
pub const HOUSE_SHADER_DEFAULTS: &[(&str, &[(&str, f32)])] = &[
    ("glow_gauss",   &[("BOOST", 1.1), ("GLOW_ROLLOFF", 2.4), ("BLOOM_STRENGTH", 0.1)]),
    ("glow_lanczos", &[("BOOST", 1.1), ("GLOW_ROLLOFF", 2.4), ("BLOOM_STRENGTH", 0.1)]),
];

/// The house default for one parameter of one shader, if the app overrides
/// what the shader declares.
pub fn house_shader_default(shader_id: &str, param: &str) -> Option<f32> {
    HOUSE_SHADER_DEFAULTS
        .iter()
        .find(|(id, _)| *id == shader_id)
        .and_then(|(_, params)| params.iter().find(|(name, _)| *name == param))
        .map(|(_, v)| *v)
}

/// Root of the slang-shaders tree.
///
/// `NTSCRT_SHADERS` overrides everything (the counterpart of the macOS
/// build's `CRT_SHADERS`), then `shaders/` beside the executable, then the
/// `Vendor/slang-shaders` submodule found by walking up from the exe or the
/// current directory.
pub fn shaders_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NTSCRT_SHADERS") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }

    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join("shaders");
            if beside.is_dir() {
                return Some(beside);
            }
            starts.push(dir.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }

    for start in starts {
        if let Some(found) = walk_up_for(&start, Path::new("Vendor/slang-shaders")) {
            return Some(found);
        }
    }
    None
}

/// Directory holding the bundled `*.json` app presets (`presets/` in the repo).
pub fn app_presets_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NTSCRT_PRESETS") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join("presets");
            if beside.is_dir() {
                return Some(beside);
            }
            if let Some(found) = walk_up_for(dir, Path::new("presets")) {
                return Some(found);
            }
        }
    }
    std::env::current_dir()
        .ok()
        .and_then(|cwd| walk_up_for(&cwd, Path::new("presets")))
}

/// Walk up from `start` looking for `relative`, stopping at the filesystem
/// root. Bounded so a deep path can't spin.
fn walk_up_for(start: &Path, relative: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    for _ in 0..12 {
        let d = dir?;
        let candidate = d.join(relative);
        if candidate.is_dir() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Absolute path to a shader preset, or None when the shader tree is missing.
pub fn resolve(entry: &PresetEntry) -> Option<PathBuf> {
    let root = shaders_root()?;
    let path = root.join(entry.relative_path);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_matches_the_swift_preset_list() {
        assert_eq!(ALL.len(), 7);
        for e in ALL {
            assert!(e.relative_path.starts_with("crt/"), "{}", e.relative_path);
            assert!(e.relative_path.ends_with(".slangp"), "{}", e.relative_path);
            assert_eq!(find(e.id), Some(e));
        }
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = ALL.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "duplicate preset id");
    }

    #[test]
    fn unknown_id_is_none() {
        assert!(find("does-not-exist").is_none());
    }

    #[test]
    fn walk_up_finds_a_parent_directory() {
        let tmp = std::env::temp_dir();
        let nested = tmp.join("ntscrt-walkup-test/a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        let target = tmp.join("ntscrt-walkup-test/presets");
        std::fs::create_dir_all(&target).unwrap();

        let found = walk_up_for(&nested, Path::new("presets"));
        assert_eq!(found.map(|p| p.canonicalize().unwrap()),
                   Some(target.canonicalize().unwrap()));

        let _ = std::fs::remove_dir_all(tmp.join("ntscrt-walkup-test"));
    }

    #[test]
    fn walk_up_gives_up_on_a_missing_directory() {
        let start = std::env::temp_dir();
        assert!(walk_up_for(&start, Path::new("definitely-not-here-ntscrt")).is_none());
    }
}
