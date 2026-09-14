//! Headless pipeline verifier — the Windows counterpart of the macOS
//! `crt-smoke` target (`Sources/CrtSmoke/main.swift`).
//!
//! Runs an image or a video frame through NTSC → downscale → CRT shader and
//! writes a PNG, with no window and no GUI dependencies. Useful for checking
//! that a build renders correctly on a given adapter, and for byte-comparing
//! output between revisions.
//!
//! ```text
//! ntscrt-smoke <input> <output.png> [--shader royale] [--downscale 320]
//!              [--method area] [--height 960] [--no-ntsc] [--snap]
//!              [--ntsc-preset preset.json] [--frame N] [--list-shaders]
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use ntscrt_app::image_io::SourceImage;
use ntscrt_app::video::{is_video_path, VideoSource};
use ntscrt_app::{HeadlessRenderer, RenderSettings};
use ntscrt_core::DownscaleMethod;

const USAGE: &str = "\
ntscrt-smoke — headless NTSCRT pipeline verifier

USAGE:
    ntscrt-smoke <input-image|video> <output.png> [options]
    ntscrt-smoke --list-shaders
    ntscrt-smoke --list-params [shader-id]
    ntscrt-smoke --list-presets
    ntscrt-smoke --video-info <file>

OPTIONS:
    --preset <name|file>  Load a full app preset (downscale + NTSC + shader).
                          Bundled name or path; later flags still override it.
    --shader <id>         CRT preset id (default: royale). --list-shaders to see them.
    --downscale <px>      Retro width the shader sees (default: 320). 'off' disables.
    --method <name>       nearest | nearest+ | bilinear | bicubic | lanczos | area
    --height <px>         Output height (default: 960).
    --snap                Snap output onto the scanline grid instead of supersampling.
    --no-ntsc             Skip the NTSC/VHS signal stage.
    --ntsc-preset <file>  ntsc-rs preset JSON (interchangeable with the ntsc-rs app).
    --frame <n>           Frame index: the deterministic RNG's seed, and which
                          frame is decoded when the input is a video (default: 0).

VIDEO:
    --video-info <file>   Probe a clip and report size, rate and frame count.
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

/// Report what ffmpeg is available and what it says about a clip — the video
/// counterpart of `--list-shaders`, and the first thing to run when video
/// misbehaves on a machine.
fn print_video_info(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match ntscrt_app::video::ffmpeg::probe_tools() {
        Ok(version) => println!("ffmpeg:  {version}"),
        Err(e) => return Err(e.into()),
    }
    let video = VideoSource::open(path)?;
    let i = &video.info;
    println!("file:    {}", video.path.display());
    println!("codec:   {}", i.video_codec);
    println!("size:    {}x{}", i.width, i.height);
    println!("rate:    {:.4} fps", i.frame_rate);
    println!("length:  {:.3}s, {} frames", i.duration_seconds, i.total_frames);
    println!("audio:   {}", if i.has_audio { "yes" } else { "none" });
    Ok(())
}

/// Accept either a path to a preset file or the name of a bundled one.
fn resolve_preset(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let direct = PathBuf::from(name);
    if direct.is_file() {
        return Ok(direct);
    }
    let bundled = ntscrt_app::app_preset::bundled();
    bundled
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, p)| p.clone())
        .ok_or_else(|| {
            let names: Vec<&str> = bundled.iter().map(|(n, _)| n.as_str()).collect();
            format!("no preset '{name}'. Bundled: {}", names.join(", ")).into()
        })
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
    if args.iter().any(|a| a == "--list-presets") {
        let found = ntscrt_app::app_preset::bundled();
        if found.is_empty() {
            return Err("no bundled presets found. Set NTSCRT_PRESETS to the presets/ \
                        directory, or run from the repository."
                .into());
        }
        println!("{} bundled presets", found.len());
        for (name, path) in &found {
            match ntscrt_app::app_preset::AppPreset::load(path) {
                Ok(p) => println!(
                    "  {:<24} shader={:<14} downscale={}px {:<10} ntsc={:<5} {}",
                    name,
                    p.shader.preset,
                    p.downscale.width,
                    p.downscale.method,
                    p.ntsc.enabled,
                    if p.has_keyframes() { "[has keyframes]" } else { "" }
                ),
                Err(e) => println!("  {name:<24} FAILED: {e}"),
            }
        }
        return Ok(());
    }

    if let Some(i) = args.iter().position(|a| a == "--video-info") {
        let path = args.get(i + 1).ok_or("--video-info needs a file")?;
        print_video_info(Path::new(path))?;
        return Ok(());
    }

    if let Some(i) = args.iter().position(|a| a == "--list-params") {
        let shader = args.get(i + 1).map(String::as_str).unwrap_or("royale");
        let mut renderer = HeadlessRenderer::new()?;
        let params = renderer.shader_parameters(shader)?;
        println!("{shader}: {} parameters", params.len());
        for p in &params {
            let kind = if p.is_toggle() {
                "toggle".to_string()
            } else if p.has_usable_range() {
                format!("{} .. {} step {}", p.minimum, p.maximum, p.step)
            } else {
                "unbounded".to_string()
            };
            println!("  {:<34} {:<28} = {:<10} {}", p.name, kind, p.initial, p.description);
        }
        return Ok(());
    }
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return Ok(());
    }

    let mut positional: Vec<String> = Vec::new();
    let mut settings = RenderSettings::default();
    let mut preset_params: Vec<(String, f32)> = Vec::new();
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
            // A full app preset drives downscale, NTSC and shader at once.
            // Any flag given after it still wins, so a preset can be used as
            // a starting point.
            "--preset" => {
                let name = value("--preset")?;
                let path = resolve_preset(&name)?;
                let p = ntscrt_app::app_preset::AppPreset::load(&path)?;
                settings.downscale_width = p.downscale.enabled.then_some(p.downscale.width);
                settings.downscale_method = DownscaleMethod::from_str(&p.downscale.method)
                    .map_err(|_| format!("preset has unknown method '{}'", p.downscale.method))?;
                settings.ntsc_enabled = p.ntsc.enabled;
                if !p.ntsc.settings.is_null() {
                    settings.ntsc_preset_json = Some(p.ntsc.settings.to_string());
                }
                settings.shader_id = p.shader.preset.clone();
                if p.has_keyframes() {
                    eprintln!("note: '{name}' carries keyframes; this build has no timeline");
                }
                preset_params = p.shader.params.into_iter().collect();
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

    settings.shader_params = preset_params;

    // A video input renders one frame: `--frame` picks it, and the same index
    // seeds the signal stage's RNG, so the still you get back is the frame
    // the player would show at that position.
    let source = if is_video_path(&input) {
        let video = VideoSource::open(&input)?;
        println!(
            "video:   {}x{} {:.3} fps, {} frames ({:.2}s), {}",
            video.info.width,
            video.info.height,
            video.info.frame_rate,
            video.info.total_frames,
            video.info.duration_seconds,
            video.info.video_codec
        );
        video.frame_at_index(settings.frame_count)?
    } else {
        SourceImage::load(&input).map_err(|e| format!("could not read {}: {e}", input.display()))?
    };

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
