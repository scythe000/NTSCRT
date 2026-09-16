//! Bundled shader catalog and asset locations.
//!
//! Ported from `Sources/CrtCore/Presets.swift` and `Sources/CrtApp/Paths.swift`.
//! The macOS build resolves these inside the .app bundle. Here the shaders
//! the seven presets need are embedded in the executable as one compressed
//! pack (see `shader_pack.rs`) and unpacked once into a per-user cache;
//! a `shaders/` directory beside the executable or `VHS_STUDIO_SHADERS`
//! overrides that, and a source-tree build without the submodule falls back
//! to walking up to `Vendor/slang-shaders`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

/// The shader pack `build.rs` produced: the presets' file closure, or an
/// empty pack when the build had no slang-shaders checkout.
pub const SHADER_PACK: &[u8] = include_bytes!(env!("VHS_STUDIO_SHADER_PACK_FILE"));
/// Content hash of `SHADER_PACK`; names its cache directory.
pub const SHADER_PACK_ID: &str = env!("VHS_STUDIO_SHADER_PACK_ID");

/// Where the shader root came from — for `vhs-studio-smoke --list-shaders`
/// and the About box, and so the packaging script can prove the staged
/// executable uses its own pack rather than a tree it happened to find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadersSource {
    /// `VHS_STUDIO_SHADERS`.
    Override,
    /// A `shaders/` directory beside the executable.
    BesideExecutable,
    /// The embedded pack, unpacked to the cache directory.
    Embedded { files: usize },
    /// `Vendor/slang-shaders` found by walking up from the executable or cwd.
    SourceTree,
}

impl ShadersSource {
    pub fn describe(&self) -> String {
        match self {
            Self::Override => "VHS_STUDIO_SHADERS".into(),
            Self::BesideExecutable => "shaders/ beside the executable".into(),
            Self::Embedded { files } => format!("embedded pack ({files} files, unpacked to the cache)"),
            Self::SourceTree => "Vendor/slang-shaders source tree".into(),
        }
    }
}

/// Root of the slang-shaders tree.
///
/// `VHS_STUDIO_SHADERS` overrides everything (the counterpart of the macOS
/// build's `CRT_SHADERS`), then `shaders/` beside the executable, then the
/// pack embedded in the executable (unpacked once per build into the user's
/// cache directory), then the `Vendor/slang-shaders` submodule found by
/// walking up from the exe or the current directory.
pub fn shaders_root() -> Option<PathBuf> {
    shaders_root_and_source().map(|(p, _)| p)
}

/// `shaders_root` plus where it came from.
pub fn shaders_root_and_source() -> Option<(PathBuf, ShadersSource)> {
    if let Ok(p) = std::env::var("VHS_STUDIO_SHADERS") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some((p, ShadersSource::Override));
        }
    }

    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join("shaders");
            if beside.is_dir() {
                return Some((beside, ShadersSource::BesideExecutable));
            }
            starts.push(dir.to_path_buf());
        }
    }

    if let Some((dir, files)) = embedded_shaders() {
        return Some((dir.clone(), ShadersSource::Embedded { files: *files }));
    }

    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    for start in starts {
        if let Some(found) = walk_up_for(&start, Path::new("Vendor/slang-shaders")) {
            return Some((found, ShadersSource::SourceTree));
        }
    }
    None
}

/// The embedded pack, unpacked: its directory and file count. None when the
/// pack is empty (a build without the submodule) or the cache directory
/// cannot be written. Done once per process; the directory is named after
/// the pack's content hash, so a later launch of the same build finds it
/// already there and a different build never sees stale files.
fn embedded_shaders() -> Option<&'static (PathBuf, usize)> {
    static UNPACKED: OnceLock<Option<(PathBuf, usize)>> = OnceLock::new();
    UNPACKED
        .get_or_init(|| match unpack_embedded() {
            Ok(v) => v,
            Err(e) => {
                log::warn!("shader pack: {e}; looking for a shader tree on disk instead");
                None
            }
        })
        .as_ref()
}

fn unpack_embedded() -> std::io::Result<Option<(PathBuf, usize)>> {
    let files = crate::shader_pack::unpack(SHADER_PACK)?;
    if files.is_empty() {
        return Ok(None);
    }
    let dir = shader_cache_dir().join(SHADER_PACK_ID);
    if dir.join(".complete").is_file() {
        return Ok(Some((dir, files.len())));
    }
    // Unpack beside the final name and rename into place, so a crash or a
    // second instance racing this one never leaves a half-written tree that
    // later launches would take for a complete one.
    let staging = shader_cache_dir().join(format!("{SHADER_PACK_ID}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    for (name, data) in &files {
        let path = staging.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, data)?;
    }
    std::fs::write(staging.join(".complete"), format!("{} files\n", files.len()))?;
    match std::fs::rename(&staging, &dir) {
        Ok(()) => {}
        // Someone else finished first; theirs is identical.
        Err(_) if dir.join(".complete").is_file() => {
            let _ = std::fs::remove_dir_all(&staging);
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    }
    log::info!("shader pack: unpacked {} files to {}", files.len(), dir.display());
    Ok(Some((dir, files.len())))
}

/// Per-user cache for unpacked shader packs: `%LOCALAPPDATA%\VHS-Studio\shaders`
/// on Windows, `$XDG_CACHE_HOME/vhs-studio/shaders` (or `~/.cache/...`)
/// elsewhere, the temp directory when neither is known. `VHS_STUDIO_CACHE`
/// overrides the parent.
pub fn shader_cache_dir() -> PathBuf {
    cache_root().join("shaders")
}

fn cache_root() -> PathBuf {
    if let Some(p) = std::env::var_os("VHS_STUDIO_CACHE") {
        return PathBuf::from(p);
    }
    if cfg!(windows) {
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(p).join("VHS-Studio");
        }
    } else if let Some(p) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(p).join("vhs-studio");
    } else if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("vhs-studio");
    }
    std::env::temp_dir().join("vhs-studio")
}

/// Directory holding the bundled `*.json` app presets (`presets/` in the repo).
pub fn app_presets_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VHS_STUDIO_PRESETS") {
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
    fn the_pack_lists_the_same_presets_as_the_catalog() {
        let mut ours: Vec<&str> = ALL.iter().map(|e| e.relative_path).collect();
        let mut packed: Vec<&str> = crate::shader_pack::PRESETS.to_vec();
        ours.sort_unstable();
        packed.sort_unstable();
        assert_eq!(ours, packed, "shader_pack::PRESETS must name exactly the bundled presets");
    }

    #[test]
    fn the_embedded_pack_parses_and_carries_every_preset() {
        let files = crate::shader_pack::unpack(SHADER_PACK).unwrap();
        assert_eq!(SHADER_PACK_ID, crate::shader_pack::content_id(SHADER_PACK));
        if files.is_empty() {
            return; // built without the submodule; nothing to check
        }
        for e in ALL {
            assert!(files.iter().any(|(n, _)| n == e.relative_path), "{} not in the pack", e.relative_path);
        }
        // Everything the presets resolve through the pack exists on disk.
        let (root, source) = shaders_root_and_source().unwrap();
        if let ShadersSource::Embedded { files: n } = source {
            assert_eq!(n, files.len());
            assert!(root.join(".complete").is_file());
            for e in ALL {
                assert!(resolve(e).is_some(), "{} did not resolve", e.id);
            }
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
        let nested = tmp.join("vhs-studio-walkup-test/a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        let target = tmp.join("vhs-studio-walkup-test/presets");
        std::fs::create_dir_all(&target).unwrap();

        let found = walk_up_for(&nested, Path::new("presets"));
        assert_eq!(found.map(|p| p.canonicalize().unwrap()),
                   Some(target.canonicalize().unwrap()));

        let _ = std::fs::remove_dir_all(tmp.join("vhs-studio-walkup-test"));
    }

    #[test]
    fn walk_up_gives_up_on_a_missing_directory() {
        let start = std::env::temp_dir();
        assert!(walk_up_for(&start, Path::new("definitely-not-here-vhs-studio")).is_none());
    }
}
