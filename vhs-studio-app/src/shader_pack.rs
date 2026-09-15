//! The shader pack: the seven CRT presets and every file they pull in, as
//! one compressed blob embedded in the executable.
//!
//! The slang-shaders submodule is ~5,000 files / 77 MB, of which the app uses
//! about a hundred. Shipping the whole tree beside the exe was the safe
//! choice while nothing computed which hundred; this module computes it.
//! `build.rs` walks the presets (`#reference`, `shaderN =`, the `textures`
//! list) and their sources (`#include`, recursively), packs the files it
//! finds into a single deflate stream, and the app embeds that with
//! `include_bytes!`. At run time librashader still wants real files — it
//! resolves includes and textures on disk — so the pack is unpacked once into
//! a per-user cache directory named after its content hash, and that directory
//! is handed to librashader as the shader root. A second launch finds the
//! directory and does nothing; a new build with different shaders gets a new
//! hash and a new directory. See `presets::shaders_root`.
//!
//! This file is compiled twice: as part of the crate, and by `build.rs`
//! through `#[path]`. It therefore uses only std and flate2.
//!
//! Format (`VHSPACK1` then one deflate stream of):
//!
//! ```text
//! u32 count
//! per file: u16 path length, path bytes (UTF-8, '/' separators, relative
//!           to the shader root), u32 data length, data
//! ```
//!
//! Compressing all files as one stream, rather than one per file as zip does,
//! is what makes a hundred small text files compress well — they share most
//! of their vocabulary.

use std::collections::BTreeSet;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

pub const MAGIC: &[u8; 8] = b"VHSPACK1";

/// The seven presets, relative to the slang-shaders root. Kept here rather
/// than taken from `presets::ALL` because `build.rs` compiles this file on
/// its own; `presets.rs` has a test pinning the two lists together.
pub const PRESETS: &[&str] = &[
    "crt/crt-aperture.slangp",
    "crt/crt-easymode.slangp",
    "crt/crtglow_gauss.slangp",
    "crt/crtglow_lanczos.slangp",
    "crt/crt-hyllian.slangp",
    "crt/crt-royale.slangp",
    "crt/crtsim.slangp",
];

/// Every file under `root` that the given presets need, as sorted
/// root-relative paths with `/` separators. Errors name the first file a
/// preset or source refers to that does not exist, so a broken include
/// fails the build instead of the first render.
pub fn closure(root: &Path, presets: &[&str]) -> Result<Vec<String>, String> {
    let mut files = BTreeSet::new();
    for preset in presets {
        walk_preset(root, Path::new(preset), &mut files, 0)?;
    }
    Ok(files.into_iter().collect())
}

fn walk_preset(root: &Path, rel: &Path, files: &mut BTreeSet<String>, depth: usize) -> Result<(), String> {
    if depth > 16 {
        return Err(format!("{}: presets reference each other too deeply", rel.display()));
    }
    let key = add(root, rel, files)?;
    if key.is_none() {
        return Ok(());
    }
    let text = read(root, rel)?;
    let dir = rel.parent().unwrap_or(Path::new(""));

    // The preset's texture names come from the `textures` line; each is then
    // a key whose value is the image path. Flags like `Name_linear = true`
    // are different keys and never match.
    let mut texture_names: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = strip_comment(line).trim();
        if let Some((k, v)) = key_value(line) {
            if k == "textures" {
                texture_names.extend(v.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
            }
        }
    }

    for line in text.lines() {
        let line = strip_comment(line).trim();
        if let Some(rest) = line.strip_prefix("#reference") {
            let target = unquote(rest.trim());
            if !target.is_empty() {
                walk_preset(root, &join(dir, target), files, depth + 1)?;
            }
            continue;
        }
        let Some((k, v)) = key_value(line) else { continue };
        let is_pass = k.strip_prefix("shader").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if is_pass {
            walk_source(root, &join(dir, v), files, 0)?;
        } else if texture_names.iter().any(|t| t == k) {
            add(root, &join(dir, v), files)?;
        }
    }
    Ok(())
}

fn walk_source(root: &Path, rel: &Path, files: &mut BTreeSet<String>, depth: usize) -> Result<(), String> {
    if depth > 32 {
        return Err(format!("{}: includes nest too deeply", rel.display()));
    }
    if add(root, rel, files)?.is_none() {
        return Ok(());
    }
    let text = read(root, rel)?;
    let dir = rel.parent().unwrap_or(Path::new(""));
    for line in text.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("#include") {
            let rest = rest.trim();
            if let Some(inner) = rest.strip_prefix('"').and_then(|r| r.split('"').next()) {
                walk_source(root, &join(dir, inner), files, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// Record `rel` as needed. Returns the normalised key when this is the first
/// time it is seen, None when it was already there (so callers stop
/// recursing), and an error when the file is missing or escapes the root.
fn add(root: &Path, rel: &Path, files: &mut BTreeSet<String>) -> Result<Option<String>, String> {
    let key = normalise(rel).ok_or_else(|| format!("{}: path leaves the shader root", rel.display()))?;
    if !root.join(&key).is_file() {
        return Err(format!("{key}: referenced by a shader but not in the tree"));
    }
    Ok(files.insert(key.clone()).then_some(key))
}

fn read(root: &Path, rel: &Path) -> Result<String, String> {
    let key = normalise(rel).unwrap_or_default();
    std::fs::read_to_string(root.join(&key)).map_err(|e| format!("{key}: {e}"))
}

fn join(dir: &Path, target: &str) -> PathBuf {
    dir.join(target.replace('\\', "/"))
}

/// Collapse `.` and `..` lexically and use `/` throughout, so the same file
/// reached by two routes gets one entry and the key is valid on any host.
fn normalise(rel: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?.to_string()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

fn strip_comment(line: &str) -> &str {
    // `#reference` is a directive, not a comment; anything else after `#` is.
    if line.trim_start().starts_with("#reference") {
        return line;
    }
    line.split('#').next().unwrap_or("")
}

fn key_value(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once('=')?;
    Some((k.trim(), unquote(v.trim())))
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim()
}

/// Read the listed files from `root` and write the pack.
pub fn pack(root: &Path, files: &[String]) -> io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    raw.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for rel in files {
        let data = std::fs::read(root.join(rel))?;
        let name = rel.as_bytes();
        if name.len() > u16::MAX as usize {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{rel}: path too long")));
        }
        raw.extend_from_slice(&(name.len() as u16).to_le_bytes());
        raw.extend_from_slice(name);
        raw.extend_from_slice(&(data.len() as u32).to_le_bytes());
        raw.extend_from_slice(&data);
    }
    let mut out = MAGIC.to_vec();
    let mut enc = flate2::write::DeflateEncoder::new(&mut out, flate2::Compression::best());
    enc.write_all(&raw)?;
    enc.finish()?;
    Ok(out)
}

/// A pack with nothing in it — what a build without the submodule embeds.
pub fn empty() -> Vec<u8> {
    pack(Path::new("."), &[]).expect("an empty pack cannot fail")
}

/// The files in a pack, as (path, contents).
pub fn unpack(bytes: &[u8]) -> io::Result<Vec<(String, Vec<u8>)>> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, format!("shader pack: {m}"));
    let body = bytes.strip_prefix(&MAGIC[..]).ok_or_else(|| bad("wrong magic"))?;
    let mut raw = Vec::new();
    flate2::read::DeflateDecoder::new(body).read_to_end(&mut raw)?;

    let mut at = 0usize;
    let mut take = |n: usize| -> io::Result<&[u8]> {
        let s = raw.get(at..at + n).ok_or_else(|| bad("truncated"))?;
        at += n;
        Ok(s)
    };
    let count = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
    let mut files = Vec::with_capacity(count);
    for _ in 0..count {
        let n = u16::from_le_bytes(take(2)?.try_into().unwrap()) as usize;
        let name = std::str::from_utf8(take(n)?).map_err(|_| bad("path is not UTF-8"))?.to_string();
        if normalise(Path::new(&name)).as_deref() != Some(name.as_str()) || name.is_empty() {
            return Err(bad(&format!("unsafe path {name:?}")));
        }
        let len = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
        files.push((name, take(len)?.to_vec()));
    }
    Ok(files)
}

/// Write a pack's files under `dir`, creating directories as needed.
pub fn unpack_to(bytes: &[u8], dir: &Path) -> io::Result<usize> {
    let files = unpack(bytes)?;
    for (name, data) in &files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, data)?;
    }
    Ok(files.len())
}

/// FNV-1a over the pack bytes: names the cache directory, so a different
/// pack never reuses a stale extraction. Not cryptographic; nothing here
/// needs it to be.
pub fn content_id(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vhs-studio-pack-{}-{}", std::process::id(), files.len()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, text) in files {
            let p = dir.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir
    }

    #[test]
    fn closure_follows_passes_includes_textures_and_references() {
        let root = tree(&[
            ("crt/a.slangp", "#reference \"../base/b.slangp\"\nshaders = 1\nshader0 = \"shaders/a.slang\"\nfilter_linear0 = true\n"),
            ("crt/shaders/a.slang", "#version 450\n#include \"../../include/common.inc\"\n// #include \"not/this.inc\"\n"),
            ("include/common.inc", "#include \"./deeper.inc\"\n"),
            ("include/deeper.inc", "float x;\n"),
            ("base/b.slangp", "shaders = 1\nshader0 = b.slang # trailing comment\ntextures = \"LUT;Mask\"\nLUT = lut.png\nLUT_linear = true\nMask = \"../include/mask.png\"\n"),
            ("base/b.slang", "void main(){}\n"),
            ("base/lut.png", "png"),
            ("include/mask.png", "png"),
            ("unused/file.slang", "never\n"),
        ]);
        // `not/this.inc` is inside a `//` comment; `#include` scanning is
        // line-prefix based, as it is in chain.rs, so it is not followed.
        let files = closure(&root, &["crt/a.slangp"]).unwrap();
        assert_eq!(
            files,
            [
                "base/b.slang",
                "base/b.slangp",
                "base/lut.png",
                "crt/a.slangp",
                "crt/shaders/a.slang",
                "include/common.inc",
                "include/deeper.inc",
                "include/mask.png",
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_include_is_an_error_naming_the_file() {
        let root = tree(&[
            ("crt/a.slangp", "shader0 = a.slang\n"),
            ("crt/a.slang", "#include \"gone.inc\"\n"),
        ]);
        let err = closure(&root, &["crt/a.slangp"]).unwrap_err();
        assert!(err.starts_with("crt/gone.inc:"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn paths_may_not_leave_the_root() {
        let root = tree(&[("crt/a.slangp", "shader0 = ../../a.slang\n")]);
        let err = closure(&root, &["crt/a.slangp"]).unwrap_err();
        assert!(err.contains("leaves the shader root"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pack_round_trips_and_is_content_addressed() {
        let root = tree(&[("crt/a.slangp", "shader0 = a.slang\n"), ("crt/a.slang", "void main(){}\n")]);
        let files = closure(&root, &["crt/a.slangp"]).unwrap();
        let bytes = pack(&root, &files).unwrap();
        assert!(bytes.starts_with(MAGIC));
        let again = pack(&root, &files).unwrap();
        assert_eq!(bytes, again, "packing is deterministic");

        let out = unpack(&bytes).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].0, "crt/a.slangp");
        assert_eq!(out[1].1, b"shader0 = a.slang\n");

        let dest = root.join("out");
        assert_eq!(unpack_to(&bytes, &dest).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(dest.join("crt/a.slang")).unwrap(), "void main(){}\n");

        assert_ne!(content_id(&bytes), content_id(&empty()));
        assert_eq!(unpack(&empty()).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pack_with_an_escaping_path_is_rejected() {
        // Build a body by hand with a `..` path and check unpack refuses it.
        let mut raw = Vec::new();
        raw.extend_from_slice(&1u32.to_le_bytes());
        let name = b"../evil";
        raw.extend_from_slice(&(name.len() as u16).to_le_bytes());
        raw.extend_from_slice(name);
        raw.extend_from_slice(&0u32.to_le_bytes());
        let mut bytes = MAGIC.to_vec();
        let mut enc = flate2::write::DeflateEncoder::new(&mut bytes, flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap();
        assert!(unpack(&bytes).unwrap_err().to_string().contains("unsafe path"));
    }
}
