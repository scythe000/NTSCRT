//! Headless pipeline verifier — the Windows counterpart of the macOS
//! `crt-smoke` target (`Sources/CrtSmoke/main.swift`).
//!
//! Runs an image or a video frame through NTSC → downscale → CRT shader and
//! writes a PNG, with no window and no GUI dependencies. Useful for checking
//! that a build renders correctly on a given adapter, and for byte-comparing
//! output between revisions.
//!
//! ```text
//! vhs-studio-smoke <input> <output.png> [--shader royale] [--downscale 320]
//!              [--method area] [--height 960] [--no-ntsc] [--no-shader] [--snap]
//!              [--ntsc-preset preset.json] [--frame N] [--list-shaders]
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use vhs_studio_app::image_io::SourceImage;
use vhs_studio_app::video::{is_video_path, VideoSource};
use vhs_studio_app::{HeadlessRenderer, RenderSettings};
use vhs_studio_core::DownscaleMethod;

const USAGE: &str = "\
vhs-studio-smoke — headless VHS-Studio pipeline verifier

USAGE:
    vhs-studio-smoke <input-image|video> <output.png> [options]
    vhs-studio-smoke --list-shaders
    vhs-studio-smoke --list-params [shader-id]
    vhs-studio-smoke --list-grade
    vhs-studio-smoke --ffmpeg
    vhs-studio-smoke --version
    vhs-studio-smoke --list-presets
    vhs-studio-smoke --timeline <preset> [--watch <ntsc-setting>]
    vhs-studio-smoke --video-info <file>

OPTIONS:
    --preset <name|file>  Load a full app preset (downscale + NTSC + shader).
                          Bundled name or path; later flags still override it.
                          Repeatable: a colour preset such as Neon stacks its
                          grade on the preset before it, as in the app's menu.
    --shader <id>         CRT preset id (default: royale). --list-shaders to see them.
    --downscale <px>      Retro width the shader sees (default: 320). 'off' disables.
    --method <name>       nearest | nearest+ | bilinear | bicubic | lanczos | area
    --height <px>         Output height (default: 960).
    --snap                Snap output onto the scanline grid instead of supersampling.
    --rotate <deg>        Rotate the source before the pipeline: 0, 90, 180, 270.
    --no-ntsc             Skip the NTSC/VHS signal stage.
    --no-shader           Skip the CRT shader: write the chain input, nearest-scaled.
    --grade name=value    Set a colour-grade control (and turn the stage on); repeatable.
                          --list-grade for the names, ranges and defaults.
    --ntsc-preset <file>  ntsc-rs preset JSON (interchangeable with the ntsc-rs app).
    --frame <n>           Frame index: the deterministic RNG's seed, and which
                          frame is decoded when the input is a video (default: 0).

VIDEO:
    --video-info <file>   Probe a clip and report size, rate and frame count.
    --playback <n>        Play <n> frames of a video input headlessly, through
                          the real producer, schedule and frame cache, and
                          report throughput, drops and cache hits.
    --export <file>       Encode the whole source to <file>. Every frame is
                          rendered; nothing is sampled from the cache.
    --format <name>       h264 | hevc | prores422 | prores422hq | gif
    --quality <name>      standard | high | veryhigh | maximum (default high)
    --loop <n>            Repeat the content <n> times in the file (not GIF).
    --gif-width <px>      GIF width (default 480).
    --gif-fps <n>         GIF rate: 6, 12, 24 or 30 (default 12).
    --still-frames <n>    Export a still as video: <n> frames of VHS motion.
    --cancel-after <n>    Cancel the export after <n> frames. For checking the
                          cancel path leaves no partial file behind.
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

/// One line summarising a clip, printed by every path that opens one.
fn describe(video: &VideoSource) -> String {
    let i = &video.info;
    format!(
        "video:   {}x{} {:.3} fps, {} frames ({:.2}s), {}",
        i.width, i.height, i.frame_rate, i.total_frames, i.duration_seconds, i.video_codec
    )
}

/// Play `frames` frames of `video` with no window, driving exactly what the
/// app drives: the background producer, the wall-clock schedule, and the
/// RAM-preview frame cache.
///
/// This is the video counterpart of rendering a still and looking at it — it
/// is the only way to see, on a machine with no display, whether playback
/// keeps up and whether the cache is doing its job on the second pass.
fn run_playback(
    video: &VideoSource,
    settings: &RenderSettings,
    frames: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    use vhs_studio_app::video::cache::{ChainInputCache, Stamp};
    use vhs_studio_app::video::playback::{Config, PlaybackPipeline};

    let mut renderer = HeadlessRenderer::new()?;
    let info = renderer.adapter_info();
    println!("adapter: {} ({:?})", info.name, info.backend);

    let mut sequence = renderer.begin_sequence(settings, video.size())?;
    println!(
        "output:  {}x{}",
        sequence.output_size.0, sequence.output_size.1
    );

    // One generation for the whole run: nothing edits settings mid-playback
    // here, so every frame the producer makes stays valid.
    const GENERATION: u64 = 1;
    let mut cache: ChainInputCache<wgpu::Texture> = ChainInputCache::new();
    let stamp = Stamp {
        generation: GENERATION,
        downscale: settings.downscale_width.map(|w| {
            vhs_studio_core::DownscaleSpec::for_width(
                w,
                video.info.width,
                video.info.height,
                settings.downscale_method,
            )
        }),
    };

    let config = Config::new(
        settings.ntsc_enabled,
        settings.ntsc_preset_json.clone(),
        GENERATION,
        settings.rotation,
    );
    config.set_cache_probe(Some(cache.probe()));
    let pipeline = PlaybackPipeline::start(
        video.clone(),
        0,
        config,
        PlaybackPipeline::DEFAULT_QUEUE_DEPTH,
    )?;

    let fps = video.info.frame_rate.max(1.0);
    let mut displayed = 0usize;
    let mut dropped = 0usize;
    let mut cache_hits = 0usize;

    // Prime: the clock starts when the first frame exists, so the schedule
    // cannot run ahead of a producer that hasn't begun.
    let start = std::time::Instant::now();
    while !pipeline.has_output() {
        if start.elapsed().as_secs_f64() > 10.0 {
            return Err("the producer emitted no frames within 10s".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let clock_start = std::time::Instant::now();
    let schedule_base = pipeline.first_queued_index().unwrap_or(0);

    while displayed < frames {
        let schedule = schedule_base + (clock_start.elapsed().as_secs_f64() * fps) as usize;
        pipeline.set_target_absolute_index(schedule);
        let (output, just_dropped) = pipeline.take_ready(schedule, GENERATION);
        dropped += just_dropped;
        let Some(output) = output else {
            // Nothing due yet. The app is woken by egui's repaint clock; here
            // a short sleep stands in for it, comfortably finer than a frame
            // budget at any sane rate.
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        };

        // The producer skips the NTSC stage for frames the cache holds; serve
        // those from the cache and skip the downscale too.
        let cached = (output.processed.is_none() && settings.ntsc_enabled)
            .then(|| cache.lookup(output.frame_index, stamp))
            .flatten();
        match cached {
            Some(chain_input) => {
                cache_hits += 1;
                renderer.encode_frame_from_chain_input(
                    &mut sequence,
                    chain_input,
                    output.frame_index,
                )?;
            }
            None => {
                // Normally the producer's output already has the signal stage
                // baked in, leaving only the downscale and the chain. The
                // fallback covers the frame it skipped for a cache entry that
                // then turned out not to be usable.
                let pixels = output.processed.as_deref().unwrap_or(&output.clean);
                sequence.set_ntsc_enabled(settings.ntsc_enabled && output.processed.is_none());
                renderer.encode_frame(
                    &mut sequence,
                    pixels,
                    output.size,
                    output.frame_index,
                    None,
                )?;
                if let Some((copy, size)) = renderer.take_chain_input_copy() {
                    if cache.has_room(ChainInputCache::<wgpu::Texture>::byte_count(size.0, size.1)) {
                        cache.insert(output.frame_index, copy, size, stamp);
                    }
                }
            }
        }
        displayed += 1;
    }

    let seconds = clock_start.elapsed().as_secs_f64().max(1e-6);
    println!(
        "played:  {displayed} frames in {seconds:.2}s = {:.1} fps (clip is {:.1} fps)",
        displayed as f64 / seconds,
        fps
    );
    println!("dropped: {dropped}");
    println!(
        "cache:   {} hits, {} frames held ({} MB of {} MB)",
        cache_hits,
        cache.len(),
        cache.bytes() >> 20,
        cache.capacity() >> 20
    );
    println!("queue:   {}", pipeline.take_stats_line());
    Ok(())
}

/// Report what ffmpeg is available and what it says about a clip — the video
/// counterpart of `--list-shaders`, and the first thing to run when video
/// misbehaves on a machine.
fn print_video_info(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match vhs_studio_app::video::ffmpeg::probe_tools() {
        Ok(version) => {
            let (_, source) = vhs_studio_app::video::ffmpeg::resolve_tool("VHS_STUDIO_FFMPEG", "ffmpeg");
            println!("ffmpeg:  {version} ({})", source.describe());
        }
        Err(e) => return Err(e.into()),
    }
    let video = VideoSource::open(path)?;
    let i = &video.info;
    println!("file:    {}", video.path.display());
    println!("codec:   {}", i.video_codec);
    println!("size:    {}x{}", i.width, i.height);
    println!("rate:    {:.4} fps", i.frame_rate);
    println!("length:  {:.3}s, {} frames", i.duration_seconds, i.total_frames);
    println!(
        "audio:   {}",
        match (&i.has_audio, &i.audio_codec) {
            (false, _) => "none".to_string(),
            (true, Some(c)) => c.clone(),
            (true, None) => "yes (codec unknown)".to_string(),
        }
    );
    Ok(())
}

/// Accept either a path to a preset file or the name of a bundled one.
fn resolve_preset(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let direct = PathBuf::from(name);
    if direct.is_file() {
        return Ok(direct);
    }
    let bundled = vhs_studio_app::app_preset::bundled();
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
        for p in vhs_studio_app::presets::ALL {
            let found = vhs_studio_app::presets::resolve(p).is_some();
            println!("{:<14} {:<22} {}", p.id, p.display_name,
                     if found { "ok" } else { "MISSING" });
        }
        return Ok(());
    }
    // Evaluate a preset's timeline and print how a setting moves. This is
    // how you check an animation is actually being applied rather than
    // inferring it from frames that differ only by the signal stage's own
    // frame-seeded noise.
    if let Some(i) = args.iter().position(|a| a == "--timeline") {
        let name = args.get(i + 1).ok_or("--timeline needs a preset")?;
        let watch = args
            .iter()
            .position(|a| a == "--watch")
            .and_then(|j| args.get(j + 1))
            .cloned()
            .unwrap_or_else(|| "vhs_edge_wave".to_string());

        let path = resolve_preset(name)?;
        let preset = vhs_studio_app::app_preset::AppPreset::load(&path)?;
        let Some(tl) = preset.timeline.filter(|t| t.has_keys()) else {
            return Err(format!("'{name}' has no keyframes").into());
        };
        let ev = vhs_studio_core::TimelineEvaluator::new(
            &tl,
            Default::default(),
            vhs_studio_core::timeline::ntsc_interp_table(),
        )
        .ok_or("could not build an evaluator")?;

        println!(
            "{name}: {} keyframes over {}s at {} fps ({} frames)",
            tl.keys.len(),
            tl.duration,
            tl.fps,
            tl.frame_count()
        );
        println!("static value of {watch}: {}", preset.ntsc.settings.get(&watch)
            .map(|v| v.to_string()).unwrap_or_else(|| "-".into()));
        println!("{:<7} {:<8} {}", "frame", "t", watch);
        let total = tl.frame_count();
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for f in 0..total {
            let t = tl.t_for_frame(f);
            let v = ev.ntsc_values(t).get(&watch).and_then(|v| v.as_f64());
            if let Some(v) = v {
                lo = lo.min(v);
                hi = hi.max(v);
                if f % (total / 12).max(1) == 0 || f == total - 1 {
                    println!("{f:<7} {t:<8.4} {v:.4}");
                }
            }
        }
        if lo.is_finite() {
            println!("range: {lo:.4} .. {hi:.4}");
        }
        return Ok(());
    }

    if args.iter().any(|a| a == "--list-presets") {
        let found = vhs_studio_app::app_preset::bundled();
        if found.is_empty() {
            return Err("no bundled presets found. Set VHS_STUDIO_PRESETS to the presets/ \
                        directory, or run from the repository."
                .into());
        }
        println!("{} bundled presets", found.len());
        for (name, path) in &found {
            match vhs_studio_app::app_preset::AppPreset::load(path) {
                Ok(p) => println!(
                    "  {:<24} shader={:<14} downscale={}px {:<10} ntsc={:<5} {}",
                    name,
                    p.shader.preset,
                    p.downscale.width,
                    p.downscale.method,
                    p.ntsc.enabled,
                    if p.is_colour_layer() {
                        "[colour, stacks]"
                    } else if p.has_keyframes() {
                        "[has keyframes]"
                    } else {
                        ""
                    }
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
    if args.first().is_some_and(|a| a == "--version" || a == "-V") {
        // The same text as the app's About box, GPU left out (no context
        // here) and ffmpeg included, since that is what a bug report needs.
        print!("{}", vhs_studio_app::about::BUILD.report(None, Some(&vhs_studio_app::about::describe_ffmpeg())));
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--ffmpeg") {
        // Which ffmpeg/ffprobe the app will launch, and from where. The
        // packaging script runs this from the staged folder to prove the
        // bundled copy is the one found, not something on PATH.
        use vhs_studio_app::video::ffmpeg::{probe_tools, resolve_tool};
        for (var, name) in [("VHS_STUDIO_FFMPEG", "ffmpeg"), ("VHS_STUDIO_FFPROBE", "ffprobe")] {
            let (path, source) = resolve_tool(var, name);
            println!("{name:8} {} ({})", path.display(), source.describe());
        }
        let version = probe_tools()?;
        println!("version  {version}");
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--list-grade") {
        println!("colour grade: {} controls", vhs_studio_core::GRADE_PARAMS.len());
        for p in vhs_studio_core::GRADE_PARAMS {
            println!("  {:<20} {:>7} .. {:<7} = {:<7} {}", p.name, p.minimum, p.maximum, p.default, p.label);
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
    let mut playback_frames: Option<usize> = None;
    let mut export_dest: Option<PathBuf> = None;
    let mut job = vhs_studio_app::video::ExportJob::default();
    let mut still_frames: Option<u32> = None;
    let mut cancel_after: Option<u32> = None;
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
            "--rotate" => {
                let deg: i32 = value("--rotate")?.parse()?;
                settings.rotation = vhs_studio_core::Rotation::try_from(deg)?;
            }
            "--playback" => playback_frames = Some(value("--playback")?.parse()?),
            "--export" => export_dest = Some(PathBuf::from(value("--export")?)),
            "--loop" => job.loop_count = value("--loop")?.parse()?,
            "--gif-width" => job.gif.width = value("--gif-width")?.parse()?,
            "--gif-fps" => job.gif.fps = value("--gif-fps")?.parse()?,
            "--still-frames" => still_frames = Some(value("--still-frames")?.parse()?),
            "--cancel-after" => cancel_after = Some(value("--cancel-after")?.parse()?),
            "--format" => {
                let v = value("--format")?;
                job.format = match v.to_ascii_lowercase().as_str() {
                    "h264" => vhs_studio_app::video::ExportFormat::H264,
                    "hevc" => vhs_studio_app::video::ExportFormat::Hevc,
                    "prores422" => vhs_studio_app::video::ExportFormat::ProRes422,
                    "prores422hq" => vhs_studio_app::video::ExportFormat::ProRes422HQ,
                    "gif" => vhs_studio_app::video::ExportFormat::Gif,
                    _ => return Err(format!("unknown format '{v}'").into()),
                };
            }
            "--quality" => {
                let v = value("--quality")?;
                job.quality = match v.to_ascii_lowercase().replace(' ', "").as_str() {
                    "standard" => vhs_studio_app::video::ExportQuality::Standard,
                    "high" => vhs_studio_app::video::ExportQuality::High,
                    "veryhigh" => vhs_studio_app::video::ExportQuality::VeryHigh,
                    "maximum" => vhs_studio_app::video::ExportQuality::Maximum,
                    _ => return Err(format!("unknown quality '{v}'").into()),
                };
            }
            "--no-ntsc" => settings.ntsc_enabled = false,
            "--no-shader" => settings.shader_enabled = false,
            "--grade" => {
                let v = value("--grade")?;
                let (name, val) = v
                    .split_once('=')
                    .ok_or_else(|| format!("--grade wants name=value, got '{v}'"))?;
                if vhs_studio_core::grade_param(name).is_none() {
                    return Err(format!("unknown grade control '{name}' (see --list-grade)").into());
                }
                settings.grade.enabled = true;
                settings.grade.set(name, val.parse()?);
            }
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
                let p = vhs_studio_app::app_preset::AppPreset::load(&path)?;
                // A colour preset is only its grade; it stacks on whatever
                // --preset came before it, as it does in the app's menu.
                if p.is_colour_layer() {
                    println!("colour preset '{name}' stacked (grade only)");
                    settings.grade = p.grade.clone();
                    i += 1;
                    continue;
                }
                settings.downscale_width = p.downscale.enabled.then_some(p.downscale.width);
                settings.downscale_method = DownscaleMethod::from_str(&p.downscale.method)
                    .map_err(|_| format!("preset has unknown method '{}'", p.downscale.method))?;
                settings.ntsc_enabled = p.ntsc.enabled;
                settings.shader_enabled = p.shader.enabled;
                if !p.ntsc.settings.is_null() {
                    settings.ntsc_preset_json = Some(p.ntsc.settings.to_string());
                }
                settings.shader_id = p.shader.preset.clone();
                if let Some(tl) = &p.timeline {
                    if tl.has_keys() {
                        println!(
                            "timeline: {} keyframes over {}s at {} fps",
                            tl.keys.len(),
                            tl.duration,
                            tl.fps
                        );
                    }
                }
                settings.timeline = p.timeline.clone();
                settings.grade = p.grade.clone();
                preset_params = p.shader.params.into_iter().collect();
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option '{other}'\n\n{USAGE}").into());
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    settings.shader_params = std::mem::take(&mut preset_params);

    // Export names its own destination, so it takes only an input positionally.
    if let Some(dest) = export_dest {
        let input = positional
            .first()
            .map(PathBuf::from)
            .ok_or_else(|| format!("--export needs an input

{USAGE}"))?;
        job.dest = dest;

        // A still exports video only if asked how many frames to make: the
        // signal stage animates on its own, so there is nothing to infer.
        let still = if is_video_path(&input) {
            None
        } else {
            let frames = still_frames.ok_or(
                "exporting a still as video needs --still-frames <n>",
            )?;
            Some((frames, 24.0))
        };

        let mut renderer = HeadlessRenderer::new()?;
        println!("adapter: {}", renderer.adapter_info().name);
        if job.format.is_gif() {
            println!(
                "gif:     {}px at {} fps (plays at {:.1})",
                job.gif.width,
                job.gif.fps,
                vhs_studio_app::video::GifSettings::true_fps(job.gif.fps)
            );
        }

        let started = std::time::Instant::now();
        let mut last = 0u32;
        let summary = vhs_studio_app::video::export(
            &mut renderer,
            &settings,
            &job,
            &input,
            still,
            &mut |done, total| {
                // One line per 10% so a long export shows progress without
                // scrolling the terminal.
                if total > 0 && done * 10 / total > last {
                    last = done * 10 / total;
                    println!("  {}%  ({done}/{total} frames)", last * 10);
                }
                match cancel_after {
                    Some(n) if done >= n => {
                        println!("  cancelling after {done} frames");
                        false
                    }
                    _ => true,
                }
            },
        )?;

        println!(
            "wrote:   {} ({}x{}, {} frames at {:.2} fps{}, {:.1} MB) in {:.1}s",
            job.dest.display(),
            summary.width,
            summary.height,
            summary.frames,
            summary.fps,
            match summary.audio {
                vhs_studio_app::video::AudioMode::None => ", silent",
                vhs_studio_app::video::AudioMode::Copied => ", audio copied",
                vhs_studio_app::video::AudioMode::Reencoded => ", audio re-encoded to AAC",
            },
            summary.bytes as f64 / (1024.0 * 1024.0),
            started.elapsed().as_secs_f32()
        );
        return Ok(());
    }

    // Playback writes no file, so it takes the input on its own.
    if let Some(frames) = playback_frames {
        let input = positional
            .first()
            .map(PathBuf::from)
            .ok_or_else(|| format!("--playback needs a video input\n\n{USAGE}"))?;
        if !is_video_path(&input) {
            return Err(format!("--playback needs a video file, not {}", input.display()).into());
        }
        let video = VideoSource::open(&input)?;
        println!("{}", describe(&video));
        return run_playback(&video, &settings, frames);
    }

    if positional.len() != 2 {
        return Err(format!("expected <input> and <output.png>\n\n{USAGE}").into());
    }
    let input = PathBuf::from(&positional[0]);
    let output = PathBuf::from(&positional[1]);

    // A video input renders one frame: `--frame` picks it, and the same index
    // seeds the signal stage's RNG, so the still you get back is the frame
    // the player would show at that position.
    let source = if is_video_path(&input) {
        let video = VideoSource::open(&input)?;
        println!("{}", describe(&video));
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
