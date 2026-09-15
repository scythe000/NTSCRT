//! Windows resources (the app icon and the application manifest), the
//! build description the About box shows, and the shader pack.
//!
//! The shader pack is the part of the slang-shaders submodule the seven CRT
//! presets actually use — about a hundred files of five thousand — packed
//! into one compressed blob (`shader_pack.rs`, shared with the crate through
//! `#[path]`) that the app embeds. A checkout without the submodule builds
//! an empty pack and the app falls back to looking for a tree on disk.
//!
//! The icon's source of truth is `Assets/icon-source.png`, the same 1024px
//! render the macOS `.icns` is made from, so the two builds cannot drift
//! apart. It is turned into a multi-size `.ico` here at build time rather
//! than committed, and embedded with `winresource` when the target is
//! Windows. The `.ico` is written on every host so the conversion itself is
//! exercised by the Linux verification build; only the embedding needs
//! `rc.exe`, which the MSVC toolchain provides.

use std::path::{Path, PathBuf};

#[path = "src/shader_pack.rs"]
#[allow(dead_code)]
mod shader_pack;

const ICON_SOURCE: &str = "../Assets/icon-source.png";
const SHADERS_SOURCE: &str = "../Vendor/slang-shaders";

/// Sizes Explorer and the taskbar actually pick from. 256 is the ceiling the
/// ICO format allows per entry.
const ICON_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

/// Per-monitor v2 DPI awareness, so the window is crisp on mixed-DPI
/// desktops and Windows does not bitmap-stretch it; long paths so a deep
/// shader tree cannot trip MAX_PATH.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON_SOURCE}");

    describe_build();

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    write_shader_pack(Path::new(SHADERS_SOURCE), &out_dir.join("shaders.pack"));
    let ico = out_dir.join("vhs-studio.ico");
    let source = Path::new(ICON_SOURCE);

    // A checkout without the asset (or a broken one) must still build; the
    // icon is a nicety, the binary is not.
    let have_icon = match write_ico(source, &ico) {
        Ok(()) => true,
        Err(e) => {
            println!("cargo:warning=no app icon: {e}");
            false
        }
    };

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        if have_icon {
            res.set_icon(&ico.to_string_lossy());
        }
        res.set_manifest(MANIFEST)
            .set("ProductName", "VHS-Studio")
            .set("FileDescription", "VHS-Studio — NTSC/VHS signal emulation and CRT shaders")
            .set("LegalCopyright", "MIT OR ISC OR Apache-2.0");
        if let Err(e) = res.compile() {
            // Without rc.exe (a bare rustup install with no Windows SDK) the
            // binary still links; it just gets the default icon.
            println!("cargo:warning=Windows resources not embedded: {e}");
        }
    }
}

/// Pack the presets' closure from `source` into `dest` and tell the crate
/// where it is. `VHS_STUDIO_SHADER_PACK` overrides the source tree, for
/// building against a different slang-shaders checkout.
fn write_shader_pack(source: &Path, dest: &Path) {
    println!("cargo:rerun-if-env-changed=VHS_STUDIO_SHADER_PACK");
    let override_root = std::env::var_os("VHS_STUDIO_SHADER_PACK").map(PathBuf::from);
    let root = override_root.as_deref().unwrap_or(source);

    let bytes = if root.join("crt").is_dir() {
        match shader_pack::closure(root, shader_pack::PRESETS) {
            Ok(files) => {
                for f in &files {
                    println!("cargo:rerun-if-changed={}", root.join(f).display());
                }
                // Also when a preset gains a file: the presets themselves
                // are in the list, so an edited .slangp re-runs the walk.
                shader_pack::pack(root, &files).expect("packing the shader closure")
            }
            Err(e) => panic!("shader pack: {e} (in {})", root.display()),
        }
    } else {
        println!(
            "cargo:warning=no shader pack: {} is not a slang-shaders checkout \
             (run: git submodule update --init --depth 1 Vendor/slang-shaders); \
             the app will look for a tree on disk instead",
            root.display()
        );
        shader_pack::empty()
    };
    std::fs::write(dest, &bytes).expect("writing the shader pack");
    println!("cargo:rustc-env=VHS_STUDIO_SHADER_PACK_FILE={}", dest.display());
    println!("cargo:rustc-env=VHS_STUDIO_SHADER_PACK_ID={}", shader_pack::content_id(&bytes));
}

fn write_ico(source: &Path, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use image::imageops::FilterType;

    let img = image::open(source)
        .map_err(|e| format!("{}: {e}", source.display()))?
        .into_rgba8();

    // PNG-compressed entries: Vista and later read them, and at 256px a raw
    // BMP entry alone would be a quarter megabyte in the executable.
    let mut frames = Vec::with_capacity(ICON_SIZES.len());
    for &size in &ICON_SIZES {
        let scaled = image::imageops::resize(&img, size, size, FilterType::Lanczos3);
        frames.push(IcoFrame::as_png(
            scaled.as_raw(),
            size,
            size,
            image::ExtendedColorType::Rgba8,
        )?);
    }

    let file = std::fs::File::create(dest)?;
    IcoEncoder::new(std::io::BufWriter::new(file)).encode_images(&frames)?;
    Ok(())
}

/// Hand the About box what `CARGO_PKG_VERSION` alone can't tell the user:
/// which commit this is, whether the tree was clean, when it was built, and
/// the versions of the four libraries that decide what the picture looks
/// like. Everything is best-effort — a tarball build without git still
/// compiles, it just says "unknown".
fn describe_build() {
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    // Rebuild when the checked-out commit changes. HEAD names the branch;
    // the branch file moves on every commit.
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={git_dir}/{head}");
        }
    }
    let hash = git(&["rev-parse", "--short=9", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    println!("cargo:rustc-env=VHS_STUDIO_GIT_HASH={hash}{}", if dirty { "-dirty" } else { "" });

    // Date only: a build is identified by its commit, the date just orients
    // whoever reads the box. SOURCE_DATE_EPOCH keeps reproducible builds so.
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
    println!("cargo:rustc-env=VHS_STUDIO_BUILD_DATE={}", ymd_from_unix(secs));

    println!(
        "cargo:rustc-env=VHS_STUDIO_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown".into())
    );

    // Library versions from Cargo.lock, one directory up (the workspace).
    println!("cargo:rerun-if-changed=../Cargo.lock");
    let lock = std::fs::read_to_string("../Cargo.lock").unwrap_or_default();
    for (name, var) in [
        ("ntsc-rs", "VHS_STUDIO_DEP_NTSC_RS"),
        ("librashader", "VHS_STUDIO_DEP_LIBRASHADER"),
        ("wgpu", "VHS_STUDIO_DEP_WGPU"),
        ("egui", "VHS_STUDIO_DEP_EGUI"),
    ] {
        let version = lock_version(&lock, name).unwrap_or_else(|| "unknown".into());
        println!("cargo:rustc-env={var}={version}");
    }
}

/// The `version` of package `name` in Cargo.lock text.
fn lock_version(lock: &str, name: &str) -> Option<String> {
    let needle = format!("name = \"{name}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == needle {
            return lines
                .next()
                .and_then(|l| l.trim().strip_prefix("version = \""))
                .and_then(|v| v.strip_suffix('"'))
                .map(str::to_string);
        }
    }
    None
}

/// YYYY-MM-DD for a Unix timestamp; the civil-from-days algorithm, so the
/// build script needs no date crate.
fn ymd_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
