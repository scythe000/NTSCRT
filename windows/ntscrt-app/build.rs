//! Windows resources: the app icon and the application manifest.
//!
//! The icon's source of truth is `Assets/icon-source.png`, the same 1024px
//! render the macOS `.icns` is made from, so the two builds cannot drift
//! apart. It is turned into a multi-size `.ico` here at build time rather
//! than committed, and embedded with `winresource` when the target is
//! Windows. The `.ico` is written on every host so the conversion itself is
//! exercised by the Linux verification build; only the embedding needs
//! `rc.exe`, which the MSVC toolchain provides.

use std::path::{Path, PathBuf};

const ICON_SOURCE: &str = "../../Assets/icon-source.png";

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

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out_dir.join("ntscrt.ico");
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
            .set("ProductName", "NTSCRT")
            .set("FileDescription", "NTSCRT — NTSC/VHS signal emulation and CRT shaders")
            .set("LegalCopyright", "MIT OR ISC OR Apache-2.0");
        if let Err(e) = res.compile() {
            // Without rc.exe (a bare rustup install with no Windows SDK) the
            // binary still links; it just gets the default icon.
            println!("cargo:warning=Windows resources not embedded: {e}");
        }
    }
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
