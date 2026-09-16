//! Movie export, ported from `Sources/CrtCore/Mp4Exporter.swift` and
//! `GifExporter.swift`.
//!
//! The macOS build writes through AVFoundation. Here frames go out the same
//! way they come in: rendered to RGBA8, then piped to ffmpeg as rawvideo.
//! That keeps one encoder path for every format and means the codec list is
//! whatever the installed ffmpeg supports.
//!
//! Exports are deterministic. Every frame is rendered — never sampled from
//! the playback cache, never dropped under load — and the frame index seeds
//! the signal stage's RNG, so the same settings and the same frame produce
//! the same pixels on every run.
//!
//! A clip's audio comes along: the source file is handed to ffmpeg as a
//! second input and its first audio track mapped alongside the picture. The
//! track is **copied** when the output container can hold its codec, so the
//! sound is untouched and the audio side of the export is free; when it
//! can't (PCM or Vorbis into MP4, say), it is re-encoded to AAC as the macOS
//! exporter always does. Looped exports loop the audio with the picture.
//! Stills and GIFs have no audio.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::render::{HeadlessRenderer, RenderSettings};
use crate::video::ffmpeg;
use crate::video::source::VideoSource;

/// What the export writes.
///
/// GIF sits alongside the codecs in the picker but takes a different path:
/// it has its own width and frame rate, and needs a palette pass. See
/// [`GifSettings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    H264,
    Hevc,
    ProRes422,
    ProRes422HQ,
    Gif,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 5] = [
        ExportFormat::H264,
        ExportFormat::Hevc,
        ExportFormat::ProRes422,
        ExportFormat::ProRes422HQ,
        ExportFormat::Gif,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            ExportFormat::H264 => "H.264",
            ExportFormat::Hevc => "HEVC",
            ExportFormat::ProRes422 => "ProRes 422",
            ExportFormat::ProRes422HQ => "ProRes 422 HQ",
            ExportFormat::Gif => "GIF",
        }
    }

    pub fn is_gif(self) -> bool {
        matches!(self, ExportFormat::Gif)
    }

    /// ProRes goes in a QuickTime container, as on macOS.
    pub fn is_prores(self) -> bool {
        matches!(self, ExportFormat::ProRes422 | ExportFormat::ProRes422HQ)
    }

    pub fn file_extension(self) -> &'static str {
        match self {
            ExportFormat::Gif => "gif",
            _ if self.is_prores() => "mov",
            _ => "mp4",
        }
    }

    /// Whether a source audio track in `codec` (ffprobe's name) can be
    /// stream-copied into this format's container.
    ///
    /// These are the codecs the container specification allows *and* that
    /// common players open. MP4 takes the MPEG family plus Dolby; QuickTime
    /// takes those and raw PCM, which MP4 does not (ffmpeg's `ipcm` boxes are
    /// legal but few players read them). Anything else — Vorbis, FLAC, an
    /// unnamed codec — is re-encoded, which is never wrong, only lossy.
    pub fn can_copy_audio(self, codec: Option<&str>) -> bool {
        let Some(codec) = codec else { return false };
        const MPEG_AND_DOLBY: &[&str] = &["aac", "mp3", "mp2", "ac3", "eac3", "alac"];
        // The PCM layouts QuickTime has tags for; camera and NLE output is
        // one of these when it is PCM at all.
        const QUICKTIME_PCM: &[&str] = &[
            "pcm_s16le", "pcm_s16be", "pcm_s24le", "pcm_s24be", "pcm_s32le", "pcm_s32be",
            "pcm_f32le", "pcm_f32be", "pcm_f64le", "pcm_f64be", "pcm_u8", "pcm_alaw", "pcm_mulaw",
        ];
        match self {
            ExportFormat::Gif => false,
            ExportFormat::H264 | ExportFormat::Hevc => MPEG_AND_DOLBY.contains(&codec),
            ExportFormat::ProRes422 | ExportFormat::ProRes422HQ => {
                MPEG_AND_DOLBY.contains(&codec) || QUICKTIME_PCM.contains(&codec)
            }
        }
    }

    /// Short name for the export button, matching `ExportFormat.buttonName`.
    pub fn button_name(self) -> &'static str {
        match self {
            ExportFormat::Gif => "GIF",
            _ if self.is_prores() => "MOV",
            _ => "MP4",
        }
    }
}

/// Bitrate tiers, ported from `ExportQuality`.
///
/// Scanline detail is brutal on lossy codecs, so these run well above typical
/// camera-footage rates — the macOS UI tells you to use High or better.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportQuality {
    Standard,
    High,
    VeryHigh,
    Maximum,
}

impl ExportQuality {
    pub const ALL: [ExportQuality; 4] = [
        ExportQuality::Standard,
        ExportQuality::High,
        ExportQuality::VeryHigh,
        ExportQuality::Maximum,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            ExportQuality::Standard => "Standard",
            ExportQuality::High => "High",
            ExportQuality::VeryHigh => "Very high",
            ExportQuality::Maximum => "Maximum",
        }
    }

    pub fn bits_per_pixel(self) -> f64 {
        match self {
            ExportQuality::Standard => 0.12,
            ExportQuality::High => 0.25,
            ExportQuality::VeryHigh => 0.5,
            ExportQuality::Maximum => 1.0,
        }
    }

    /// Target average bitrate, with the same 2 Mbps floor as the macOS build.
    pub fn bitrate(self, width: u32, height: u32, fps: f64) -> u64 {
        let computed = width as f64 * height as f64 * fps * self.bits_per_pixel();
        (computed as u64).max(2_000_000)
    }
}

/// GIF's own size and rate, which do not follow the video settings.
///
/// 256 colours and run-length compression against full-frame analog noise
/// means files run large — measured at roughly 0.65-0.95 bytes per pixel per
/// frame, so the estimate below uses 0.8.
#[derive(Debug, Clone, Copy)]
pub struct GifSettings {
    pub width: u32,
    pub fps: u32,
}

impl Default for GifSettings {
    fn default() -> Self {
        Self { width: 480, fps: 12 }
    }
}

impl GifSettings {
    /// The rates the picker offers. 60 is absent deliberately: GIF cannot
    /// reliably go past 50.
    pub const RATES: [u32; 4] = [6, 12, 24, 30];

    /// GIF stores frame delays in whole hundredths of a second, so the
    /// achievable rates sit on that grid: 12 fps really plays at 12.5, 24 at
    /// 25, 30 at 33.3. The floor of 2 (50 fps) is deliberate — many decoders
    /// treat a delay of 0 or 1 as "as fast as possible".
    pub fn delay_centiseconds(fps: u32) -> u32 {
        let d = (100.0 / fps.max(1) as f64).round() as u32;
        d.max(2)
    }

    pub fn true_fps(fps: u32) -> f64 {
        100.0 / Self::delay_centiseconds(fps) as f64
    }

    /// Rough output size. Measured: 0.94 bytes/px/frame at 320px wide, 0.78
    /// at 480, 0.66 at 640 — 0.8 is the middle of the range the app works in.
    pub fn estimated_bytes(width: u32, height: u32, frames: u32) -> u64 {
        (0.8 * width as f64 * height as f64 * frames as f64) as u64
    }

    /// The macOS panel warns past this, because most platforms reject more.
    pub const WARN_BYTES: u64 = 10 * 1024 * 1024;
}

/// Everything an export needs beyond the render settings.
pub struct ExportJob {
    pub format: ExportFormat,
    pub quality: ExportQuality,
    pub gif: GifSettings,
    /// Repeat the content in the exported file. 3 turns a 6-second clip into
    /// an 18-second file that plays through three times — for places that
    /// don't loop video on playback. GIFs already loop forever, so this does
    /// not apply to them.
    pub loop_count: u32,
    pub dest: PathBuf,
}

impl Default for ExportJob {
    fn default() -> Self {
        Self {
            format: ExportFormat::H264,
            quality: ExportQuality::High,
            gif: GifSettings::default(),
            loop_count: 1,
            dest: PathBuf::from("out.mp4"),
        }
    }
}

/// Progress, so a caller can drive a bar: called with `(done, total)` after
/// every frame.
///
/// Returning `false` cancels — the encoder is torn down and the partial file
/// removed. An export is minutes of work at 4K, so it has to be abandonable
/// without killing the app.
pub type Progress<'a> = &'a mut dyn FnMut(u32, u32) -> bool;

/// Render `source` through the pipeline and encode it.
///
/// `still_frames` applies when the source is an image rather than a clip:
/// tape noise, jitter and interlacing animate on their own, so a still can
/// export video ("VHS motion" in the macOS build).
pub fn export(
    renderer: &mut HeadlessRenderer,
    settings: &RenderSettings,
    job: &ExportJob,
    source: &Path,
    still_frames: Option<(u32, f64)>,
    progress: Progress<'_>,
) -> Result<ExportSummary, Box<dyn std::error::Error>> {
    ffmpeg::probe_tools()?;

    // Decide the frame supply: a clip's own frames, or a still repeated.
    let (video, still, source_size, frame_count, fps) = match still_frames {
        Some((frames, fps)) => {
            let img = crate::image_io::SourceImage::load(source)?;
            let size = settings.rotation.output_size(img.width, img.height);
            (None, Some(img), size, frames, fps)
        }
        None => {
            let v = VideoSource::open(source)?;
            let (rw, rh) = v.size();
            let size = settings.rotation.output_size(rw, rh);
            let n = v.info.total_frames as u32;
            let fps = v.info.frame_rate;
            (Some(v), None, size, n, fps)
        }
    };
    if frame_count == 0 {
        return Err("source has no frames".into());
    }
    // The decoder hands over unrotated frames; `source_size` is post-rotation.
    let unrotated_size = if settings.rotation.swaps_axes() {
        (source_size.1, source_size.0)
    } else {
        source_size
    };

    let mut sequence = renderer.begin_sequence(settings, source_size)?;
    let (out_w, out_h) = sequence.output_size;

    // yuv420p needs even dimensions, and GIF is happier with them too. The
    // scanline grid already prefers even multiples, so this rarely bites.
    let (enc_w, enc_h) = (out_w & !1, out_h & !1);
    if enc_w == 0 || enc_h == 0 {
        return Err(format!("output size {out_w}x{out_h} is too small to encode").into());
    }

    let out_fps = if job.format.is_gif() {
        GifSettings::true_fps(job.gif.fps)
    } else {
        fps
    };

    // GIFs already loop forever on their own, so the loop field does not
    // apply to them.
    let loops = if job.format.is_gif() { 1 } else { job.loop_count.max(1) };
    let total = frame_count * loops;

    // Audio rides along only when there is a clip with a track to take it
    // from and a container that can hold it.
    let audio = match &video {
        Some(v) if v.info.has_audio && !job.format.is_gif() => Some(AudioTrack {
            path: source,
            loops,
            seconds: total as f64 / out_fps.max(f64::EPSILON),
            copy: job.format.can_copy_audio(v.info.audio_codec.as_deref()),
        }),
        _ => None,
    };
    let audio_mode = match &audio {
        None => AudioMode::None,
        Some(a) if a.copy => AudioMode::Copied,
        Some(_) => AudioMode::Reencoded,
    };

    let args = encode_args(job, enc_w, enc_h, out_fps, audio);
    let mut proc = ffmpeg::Process::spawn(&ffmpeg::ffmpeg_path(), &args, false, true)?;
    let mut stdin = proc
        .child
        .stdin
        .take()
        .ok_or("could not open a pipe to ffmpeg")?;

    let mut written = 0u32;
    let mut cancelled = false;
    let mut reader = video.as_ref().map(|v| v.sequential_reader(0)).transpose()?;

    'outer: for _ in 0..loops {
        // Each loop pass rewinds the decoder; the still path has nothing to
        // rewind.
        if let Some(v) = video.as_ref() {
            reader = Some(v.sequential_reader(0)?);
        }
        for _ in 0..frame_count {
            let decoded: Vec<u8> = match (&mut reader, &still) {
                (Some(r), _) => match r.next_image() {
                    Some(img) => img.pixels,
                    // The container's frame count can overcount; stopping
                    // early is correct, not an error.
                    None => break 'outer,
                },
                (None, Some(img)) => img.pixels.clone(),
                _ => return Err("no frame source".into()),
            };
            // Rotate before the pipeline, as everywhere else.
            let pixels = if settings.rotation == vhs_studio_core::Rotation::None {
                decoded
            } else {
                let (w, h) = unrotated_size;
                vhs_studio_core::rotate_rgba(&decoded, w, h, settings.rotation).0
            };

            // `written` rather than `index` seeds the signal stage, so a
            // looped export keeps animating instead of repeating its noise.
            renderer.encode_frame(
                &mut sequence,
                &pixels,
                source_size,
                written as usize,
                Some(written as i64),
            )?;
            let frame = renderer.read_back(&sequence)?;

            // Crop to the even encode size if the render was odd.
            if (enc_w, enc_h) == (out_w, out_h) {
                stdin.write_all(&frame)?;
            } else {
                let row = (out_w * 4) as usize;
                for y in 0..enc_h as usize {
                    let start = y * row;
                    stdin.write_all(&frame[start..start + (enc_w * 4) as usize])?;
                }
            }

            written += 1;
            if !progress(written, total) {
                cancelled = true;
                break 'outer;
            }
        }
    }

    drop(stdin);

    if cancelled {
        // The partial file is worse than nothing, so it goes — and there is
        // no point letting ffmpeg finish the rest of the audio first.
        let _ = proc.child.kill();
        let _ = proc.child.wait();
        let _ = std::fs::remove_file(&job.dest);
        return Err(ExportError::Cancelled.into());
    }

    let status = proc.child.wait()?;
    if !status.success() {
        return Err(format!("ffmpeg failed: {}", proc.diagnostics()).into());
    }

    let bytes = std::fs::metadata(&job.dest).map(|m| m.len()).unwrap_or(0);
    Ok(ExportSummary {
        frames: written,
        width: enc_w,
        height: enc_h,
        fps: out_fps,
        bytes,
        audio: audio_mode,
    })
}

/// The source's audio, to be muxed into the export.
struct AudioTrack<'a> {
    path: &'a Path,
    /// How many times the clip plays in the file; the audio loops with it.
    loops: u32,
    /// The picture's total length. The audio is cut to it so a track that
    /// runs past the last frame cannot lengthen the file.
    seconds: f64,
    /// Stream-copy the track rather than re-encode it.
    copy: bool,
}

/// What happened to the source's audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    /// The file has no audio: a still, a GIF, or a silent clip.
    None,
    /// The source track was copied untouched.
    Copied,
    /// The source track was re-encoded to AAC because the container could
    /// not hold its codec.
    Reencoded,
}

/// Cancellation is not a failure, but it has to travel as one so the whole
/// call unwinds. Callers match on it to stay quiet rather than reporting an
/// error the user caused deliberately.
#[derive(Debug)]
pub enum ExportError {
    Cancelled,
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Cancelled => write!(f, "export cancelled"),
        }
    }
}

impl std::error::Error for ExportError {}

#[derive(Debug, Clone, Copy)]
pub struct ExportSummary {
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub bytes: u64,
    pub audio: AudioMode,
}

/// ffmpeg arguments for one export.
///
/// Input is always rawvideo RGBA at the encode size; what differs is the
/// codec and, for GIF, the palette filter. With `audio`, the source file is
/// a second input whose first audio track is mapped alongside the picture.
fn encode_args(
    job: &ExportJob,
    w: u32,
    h: u32,
    fps: f64,
    audio: Option<AudioTrack<'_>>,
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-s".into(),
        format!("{w}x{h}"),
        "-r".into(),
        format!("{fps}"),
        "-i".into(),
        "pipe:0".into(),
    ];

    if let Some(audio) = &audio {
        // `-stream_loop` applies to the input that follows it, and counts
        // repeats, not plays.
        if audio.loops > 1 {
            a.push("-stream_loop".into());
            a.push(format!("{}", audio.loops - 1));
        }
        a.push("-i".into());
        a.push(audio.path.to_string_lossy().to_string());
        // Explicit maps: left to itself ffmpeg would take the "best" video
        // stream across both inputs, which is the source, not the render.
        a.extend([
            "-map".into(), "0:v:0".into(),
            "-map".into(), "1:a:0".into(),
        ]);
    }

    match job.format {
        ExportFormat::Gif => {
            // One pass: split the stream, build a palette from the whole clip,
            // then map through it. Analog noise is full-frame, so a global
            // palette beats a per-frame one and dithering keeps the mask from
            // banding.
            let scale = if job.gif.width > 0 && job.gif.width != w {
                format!("scale={}:-2:flags=lanczos,", job.gif.width)
            } else {
                String::new()
            };
            a.push("-vf".into());
            a.push(format!(
                "{scale}split[s0][s1];[s0]palettegen=max_colors=256[p];\
                 [s1][p]paletteuse=dither=bayer:bayer_scale=3"
            ));
            a.push("-loop".into());
            a.push("0".into());
        }
        ExportFormat::ProRes422 | ExportFormat::ProRes422HQ => {
            let profile = if job.format == ExportFormat::ProRes422HQ { "3" } else { "2" };
            a.extend([
                "-c:v".into(), "prores_ks".into(),
                "-profile:v".into(), profile.into(),
                "-pix_fmt".into(), "yuv422p10le".into(),
            ]);
        }
        ExportFormat::H264 | ExportFormat::Hevc => {
            let codec = if job.format == ExportFormat::Hevc { "libx265" } else { "libx264" };
            let bitrate = job.quality.bitrate(w, h, fps);
            a.extend([
                "-c:v".into(), codec.into(),
                "-pix_fmt".into(), "yuv420p".into(),
                "-b:v".into(), format!("{bitrate}"),
                "-preset".into(), "slow".into(),
            ]);
        }
    }

    if let Some(audio) = &audio {
        if audio.copy {
            a.extend(["-c:a".into(), "copy".into()]);
        } else {
            // The macOS exporter's settings: AAC, 44.1 kHz, stereo, 128 kbps.
            a.extend([
                "-c:a".into(), "aac".into(),
                "-b:a".into(), "128k".into(),
                "-ar".into(), "44100".into(),
                "-ac".into(), "2".into(),
            ]);
        }
        a.extend(["-t".into(), format!("{:.6}", audio.seconds)]);
    }

    a.push(job.dest.to_string_lossy().to_string());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_and_containers_match_the_swift_table() {
        assert_eq!(ExportFormat::H264.file_extension(), "mp4");
        assert_eq!(ExportFormat::Hevc.file_extension(), "mp4");
        assert_eq!(ExportFormat::ProRes422.file_extension(), "mov");
        assert_eq!(ExportFormat::ProRes422HQ.file_extension(), "mov");
        assert_eq!(ExportFormat::Gif.file_extension(), "gif");

        assert_eq!(ExportFormat::H264.button_name(), "MP4");
        assert_eq!(ExportFormat::ProRes422.button_name(), "MOV");
        assert_eq!(ExportFormat::Gif.button_name(), "GIF");

        assert!(ExportFormat::ProRes422HQ.is_prores());
        assert!(!ExportFormat::Hevc.is_prores());
        assert!(ExportFormat::Gif.is_gif());
    }

    #[test]
    fn quality_tiers_match_the_swift_values() {
        assert_eq!(ExportQuality::Standard.bits_per_pixel(), 0.12);
        assert_eq!(ExportQuality::High.bits_per_pixel(), 0.25);
        assert_eq!(ExportQuality::VeryHigh.bits_per_pixel(), 0.5);
        assert_eq!(ExportQuality::Maximum.bits_per_pixel(), 1.0);
    }

    #[test]
    fn bitrate_scales_with_pixels_and_honours_the_floor() {
        // 1920x1080 at 30fps, High: well above the floor.
        let b = ExportQuality::High.bitrate(1920, 1080, 30.0);
        assert_eq!(b, (1920.0 * 1080.0 * 30.0 * 0.25) as u64);
        // A tiny frame still gets the 2 Mbps minimum.
        assert_eq!(ExportQuality::Standard.bitrate(64, 48, 12.0), 2_000_000);
    }

    #[test]
    fn gif_delays_land_on_the_centisecond_grid() {
        // The documented rates: 12 plays at 12.5, 24 at 25, 30 at 33.3.
        assert_eq!(GifSettings::delay_centiseconds(12), 8);
        assert!((GifSettings::true_fps(12) - 12.5).abs() < 1e-9);
        assert_eq!(GifSettings::delay_centiseconds(24), 4);
        assert!((GifSettings::true_fps(24) - 25.0).abs() < 1e-9);
        assert_eq!(GifSettings::delay_centiseconds(30), 3);
        assert!((GifSettings::true_fps(30) - 33.333333).abs() < 1e-4);
        assert_eq!(GifSettings::delay_centiseconds(6), 17);
    }

    #[test]
    fn gif_delay_never_drops_below_the_floor() {
        // Past 50 fps the grid runs out; the floor keeps decoders sane.
        for fps in [50, 60, 100, 1000] {
            assert_eq!(GifSettings::delay_centiseconds(fps), 2, "fps {fps}");
        }
        // A zero rate must not divide by zero. It clamps to 1 fps — a
        // one-second delay — which is what the Swift does too.
        assert_eq!(GifSettings::delay_centiseconds(0), 100);
    }

    #[test]
    fn gif_size_estimate_matches_the_documented_example() {
        // The README: a 5-second 480px GIF at 12 fps lands near 8 MB.
        let frames = 5 * 12;
        let bytes = GifSettings::estimated_bytes(480, 360, frames);
        let mb = bytes as f64 / (1024.0 * 1024.0);
        assert!((7.0..9.5).contains(&mb), "expected ~8 MB, got {mb:.1} MB");
        assert!(bytes > GifSettings::WARN_BYTES / 2);
    }

    #[test]
    fn gif_offers_only_rates_it_can_hit() {
        assert_eq!(GifSettings::RATES, [6, 12, 24, 30]);
        // 60 is deliberately absent - GIF cannot reliably go past 50.
        assert!(!GifSettings::RATES.contains(&60));
    }

    fn args_for(format: ExportFormat) -> Vec<String> {
        let job = ExportJob { format, dest: PathBuf::from("o"), ..Default::default() };
        encode_args(&job, 640, 480, 24.0, None)
    }

    fn args_with_audio(format: ExportFormat, loops: u32) -> Vec<String> {
        args_with_audio_mode(format, loops, false)
    }

    fn args_with_audio_mode(format: ExportFormat, loops: u32, copy: bool) -> Vec<String> {
        let job = ExportJob { format, loop_count: loops, dest: PathBuf::from("o"), ..Default::default() };
        let audio = AudioTrack { path: Path::new("clip.mp4"), loops, seconds: 2.5 * loops as f64, copy };
        encode_args(&job, 640, 480, 24.0, Some(audio))
    }

    fn after<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
        a.iter().position(|x| x == flag).map(|i| a[i + 1].as_str())
    }

    #[test]
    fn without_audio_there_is_a_single_input_and_no_maps() {
        for f in ExportFormat::ALL {
            let a = args_for(f);
            assert_eq!(a.iter().filter(|x| *x == "-i").count(), 1, "{f:?}");
            assert!(!a.contains(&"-map".to_string()), "{f:?}");
            assert!(!a.contains(&"-c:a".to_string()), "{f:?}");
        }
    }

    #[test]
    fn audio_adds_the_source_as_a_second_input_and_maps_both_explicitly() {
        let a = args_with_audio(ExportFormat::H264, 1);
        let inputs: Vec<&str> = a
            .iter()
            .enumerate()
            .filter(|(_, x)| *x == "-i")
            .map(|(i, _)| a[i + 1].as_str())
            .collect();
        assert_eq!(inputs, ["pipe:0", "clip.mp4"]);
        let maps: Vec<&str> = a
            .iter()
            .enumerate()
            .filter(|(_, x)| *x == "-map")
            .map(|(i, _)| a[i + 1].as_str())
            .collect();
        // Picture from the pipe, sound from the file — never the file's video.
        assert_eq!(maps, ["0:v:0", "1:a:0"]);
        // Re-encoded to the macOS exporter's AAC settings.
        assert_eq!(after(&a, "-c:a"), Some("aac"));
        assert_eq!(after(&a, "-b:a"), Some("128k"));
        assert_eq!(after(&a, "-ar"), Some("44100"));
        assert_eq!(after(&a, "-ac"), Some("2"));
        // And cut to the picture's length.
        assert_eq!(after(&a, "-t"), Some("2.500000"));
        // A single play does not loop the input.
        assert!(!a.contains(&"-stream_loop".to_string()));
        assert_eq!(a.last().unwrap(), "o");
    }

    #[test]
    fn looped_audio_repeats_the_source_input_with_the_picture() {
        let a = args_with_audio(ExportFormat::ProRes422, 3);
        // `-stream_loop` counts repeats and must precede the input it loops.
        let sl = a.iter().position(|x| x == "-stream_loop").expect("stream_loop");
        assert_eq!(a[sl + 1], "2");
        assert_eq!(a[sl + 2], "-i");
        assert_eq!(a[sl + 3], "clip.mp4");
        assert_eq!(after(&a, "-t"), Some("7.500000"));
        // The video encoder is untouched by the audio options.
        assert!(a.contains(&"prores_ks".to_string()));
    }

    #[test]
    fn a_copied_track_carries_no_encoder_settings_but_is_still_cut_to_length() {
        let a = args_with_audio_mode(ExportFormat::H264, 2, true);
        assert_eq!(after(&a, "-c:a"), Some("copy"));
        for flag in ["-b:a", "-ar", "-ac"] {
            assert!(!a.contains(&flag.to_string()), "{flag} must not accompany a copy");
        }
        assert_eq!(after(&a, "-t"), Some("5.000000"));
        assert_eq!(after(&a, "-stream_loop"), Some("1"));
    }

    #[test]
    fn audio_is_copied_only_when_the_container_can_hold_it() {
        use ExportFormat::*;
        // The MPEG family goes into both containers untouched.
        for codec in ["aac", "mp3", "ac3", "eac3", "alac"] {
            assert!(H264.can_copy_audio(Some(codec)), "{codec} into mp4");
            assert!(Hevc.can_copy_audio(Some(codec)), "{codec} into mp4");
            assert!(ProRes422.can_copy_audio(Some(codec)), "{codec} into mov");
        }
        // PCM is a QuickTime thing: fine in .mov, re-encoded for .mp4.
        assert!(ProRes422HQ.can_copy_audio(Some("pcm_s16le")));
        assert!(ProRes422.can_copy_audio(Some("pcm_s24le")));
        assert!(!H264.can_copy_audio(Some("pcm_s16le")));
        // Vorbis, FLAC and Opus are re-encoded rather than risk a file that
        // some players refuse.
        for codec in ["vorbis", "flac", "opus"] {
            assert!(!H264.can_copy_audio(Some(codec)), "{codec}");
            assert!(!ProRes422.can_copy_audio(Some(codec)), "{codec}");
        }
        // No codec name means no basis for copying.
        assert!(!H264.can_copy_audio(None));
        // GIF has no audio at all.
        assert!(!Gif.can_copy_audio(Some("aac")));
    }

    #[test]
    fn encoder_selection_matches_the_format() {
        assert!(args_for(ExportFormat::H264).contains(&"libx264".to_string()));
        assert!(args_for(ExportFormat::Hevc).contains(&"libx265".to_string()));
        assert!(args_for(ExportFormat::ProRes422).contains(&"prores_ks".to_string()));
        // 422 is profile 2, HQ is profile 3.
        let hq = args_for(ExportFormat::ProRes422HQ);
        let i = hq.iter().position(|a| a == "-profile:v").unwrap();
        assert_eq!(hq[i + 1], "3");
    }

    #[test]
    fn gif_args_build_a_palette_and_loop_forever() {
        let a = args_for(ExportFormat::Gif);
        let vf = a.iter().find(|s| s.contains("palettegen")).expect("palette pass");
        assert!(vf.contains("max_colors=256"));
        assert!(vf.contains("paletteuse"));
        let i = a.iter().position(|x| x == "-loop").unwrap();
        assert_eq!(a[i + 1], "0");
        // GIF never carries a video bitrate.
        assert!(!a.contains(&"-b:v".to_string()));
    }

    #[test]
    fn input_is_always_rawvideo_rgba_at_the_encode_size() {
        for f in ExportFormat::ALL {
            let a = args_for(f);
            assert!(a.contains(&"rawvideo".to_string()), "{f:?}");
            assert!(a.contains(&"rgba".to_string()), "{f:?}");
            assert!(a.contains(&"640x480".to_string()), "{f:?}");
            assert_eq!(a.last().unwrap(), "o", "{f:?} must end with the destination");
        }
    }
}
