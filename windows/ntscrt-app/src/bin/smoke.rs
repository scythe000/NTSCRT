//! Headless pipeline verifier — the Windows counterpart of the macOS
//! `crt-smoke` target (`Sources/CrtSmoke/main.swift`).
//!
//! Runs an image through NTSC → downscale → CRT shader and writes a PNG,
//! with no window and no GUI dependencies. Useful for checking that a build
//! renders correctly on a given adapter, and for byte-comparing output
//! between revisions.
//!
//! ```text
//! ntscrt-smoke <input> <output.png> [--shader royale] [--downscale 320]
//!              [--method area] [--height 960] [--no-ntsc] [--snap]
//!              [--ntsc-preset preset.json] [--frame N] [--list-shaders]
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use ntscrt_app::image_io::SourceImage;
use ntscrt_app::{HeadlessRenderer, RenderSettings};
use ntscrt_core::DownscaleMethod;

const USAGE: &str = "\
ntscrt-smoke — headless NTSCRT pipeline verifier

USAGE:
    ntscrt-smoke <input-image> <output.png> [options]
    ntscrt-smoke --list-shaders

OPTIONS:
    --shader <id>         CRT preset id (default: royale). --list-shaders to see them.
    --downscale <px>      Retro width the shader sees (default: 320). 'off' disables.
    --method <name>       nearest | nearest+ | bilinear | bicubic | lanczos | area
    --height <px>         Output height (default: 960).
    --snap                Snap output onto the scanline grid instead of supersampling.
    --no-ntsc             Skip the NTSC/VHS signal stage.
    --ntsc-preset <file>  ntsc-rs preset JSON (interchangeable with the ntsc-rs app).
    --frame <n>           Frame index for the deterministic RNG (default: 0).
";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--list-shaders") {
        for p in ntscrt_app::presets::ALL {
            let found = ntscrt_app::presets::resolve(p).is_some();
            println!("{:<14} {:<22} {}", p.id, p.display_name,
                     if found { "ok" } else { "MISSING" });
        }
        return Ok(());
    }
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return Ok(());
    }

    let mut positional: Vec<String> = Vec::new();
    let mut settings = RenderSettings::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let mut value = |what: &str| -> Result<String, String> {
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("{what} needs a value"))
        };
        match arg.as_str() {
            "--shader" => settings.shader_id = value("--shader")?,
            "--height" => settings.output_height = value("--height")?.parse()?,
            "--frame" => settings.frame_count = value("--frame")?.parse()?,
            "--snap" => settings.snap_to_scanline_grid = true,
            "--no-ntsc" => settings.ntsc_enabled = false,
            "--downscale" => {
                let v = value("--downscale")?;
                settings.downscale_width =
                    if v == "off" { None } else { Some(v.parse()?) };
            }
            "--method" => {
                let v = value("--method")?;
                settings.downscale_method = DownscaleMethod::from_str(&v)
                    .map_err(|_| format!("unknown downscale method '{v}'"))?;
            }
            "--ntsc-preset" => {
                let p = value("--ntsc-preset")?;
                settings.ntsc_preset_json = Some(std::fs::read_to_string(p)?);
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option '{other}'\n\n{USAGE}").into());
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    if positional.len() != 2 {
        return Err(format!("expected <input> and <output.png>\n\n{USAGE}").into());
    }
    let input = PathBuf::from(&positional[0]);
    let output = PathBuf::from(&positional[1]);

    let source = SourceImage::load(&input)
        .map_err(|e| format!("could not read {}: {e}", input.display()))?;

    let mut renderer = HeadlessRenderer::new()?;
    let info = renderer.adapter_info();
    println!("adapter: {} ({:?})", info.name, info.backend);
    println!("source:  {}x{}", source.width, source.height);

    let started = std::time::Instant::now();
    let (w, h) = renderer.render_to_png(&source, &settings, &output)?;
    println!(
        "wrote:   {} ({w}x{h}) in {:.2}s",
        output.display(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}
