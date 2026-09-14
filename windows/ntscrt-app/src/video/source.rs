//! Video decoding, ported from `Sources/CrtCore/VideoSource.swift`.
//!
//! Same two modes as the Swift original, for the same reasons:
//!
//!   - **Random access** ([`VideoSource::frame_at_index`]) for scrubbing,
//!     which jumps to an arbitrary time and decodes one frame.
//!   - **Sequential** ([`VideoSource::sequential_reader`]) for playback and
//!     export, which streams frames in order. The Swift comment records the
//!     measurement behind the split — decoding successive frames by seeking
//!     to each one re-decodes from the preceding keyframe every time (47 ms
//!     vs 2.8 ms a frame on a 1176x1764 h264 clip) — and the same holds here,
//!     since that is a property of inter-frame codecs, not of the decoder.
//!
//! Frames arrive as tightly packed RGBA8, the shape [`SourceImage`] already
//! holds, so everything downstream of here is the still pipeline unchanged.
//!
//! One thing the port drops rather than reproduces: the macOS build's
//! `needsPreferredTransform` fallback, where a rotated track has to go through
//! the much slower image-generator path because the sequential reader hands
//! back raw, un-rotated pixel buffers. ffmpeg applies the display matrix on
//! both paths, so rotated clips play at full speed here.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::image_io::SourceImage;

use super::ffmpeg::{base_args, ffmpeg_path, ffprobe_path, probe_args, Process};

/// Container extensions the file picker offers.
pub const VIDEO_EXTENSIONS: &[&str] =
    &["mp4", "mov", "m4v", "avi", "mkv", "webm", "wmv", "mpg", "mpeg"];

/// True when `path` looks like a video rather than a still.
pub fn is_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| e.eq_ignore_ascii_case(v)))
}

/// What ffprobe tells us about a clip.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    /// Frame size *after* rotation, i.e. what comes out of the decoder.
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub duration_seconds: f64,
    /// `duration * frame_rate`, as the macOS build computes it.
    pub total_frames: usize,
    pub has_audio: bool,
    pub video_codec: String,
}

impl VideoInfo {
    /// Time of frame `index` in seconds — the seek target for random access.
    pub fn time_for_frame(&self, index: usize) -> f64 {
        index as f64 / self.frame_rate.max(1.0)
    }

    /// Bytes one decoded RGBA8 frame occupies.
    pub fn frame_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

/// Parse ffprobe's `r_frame_rate`, which is a rational: "24/1", "30000/1001".
fn parse_rational(text: &str) -> Option<f64> {
    let (num, den) = text.split_once('/')?;
    let num: f64 = num.trim().parse().ok()?;
    let den: f64 = den.trim().parse().ok()?;
    (den != 0.0 && num > 0.0).then(|| num / den)
}

/// Rotation in degrees from a stream's side data or its legacy tag,
/// normalised to 0/90/180/270. ffmpeg autorotates on decode, so this only
/// says whether the decoded frame has its axes swapped relative to the coded
/// size.
fn rotation_degrees(stream: &serde_json::Value) -> i64 {
    let from_side_data = stream
        .get("side_data_list")
        .and_then(|l| l.as_array())
        .and_then(|list| {
            list.iter()
                .find_map(|d| d.get("rotation").and_then(|r| r.as_f64()))
        });
    let from_tags = stream
        .get("tags")
        .and_then(|t| t.get("rotate"))
        .and_then(|r| r.as_str())
        .and_then(|r| r.parse::<f64>().ok());
    let raw = from_side_data.or(from_tags).unwrap_or(0.0);
    // Side data reports quarter turns as negatives; only the axis swap matters.
    ((raw.round() as i64 % 360) + 360) % 360
}

/// Build [`VideoInfo`] from ffprobe's JSON. Split out from the spawn so the
/// parsing rules are testable without ffmpeg present.
fn parse_probe(json: &str) -> Result<VideoInfo, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("could not read ffprobe output: {e}"))?;
    let streams = root
        .get("streams")
        .and_then(|s| s.as_array())
        .ok_or("ffprobe reported no streams")?;

    let is_type = |s: &serde_json::Value, want: &str| {
        s.get("codec_type").and_then(|t| t.as_str()) == Some(want)
    };
    let video = streams
        .iter()
        .find(|s| is_type(s, "video"))
        .ok_or("no video track in this file")?;
    let has_audio = streams.iter().any(|s| is_type(s, "audio"));

    // ffprobe quotes some numbers and not others depending on the field.
    let num = |v: &serde_json::Value, key: &str| -> Option<f64> {
        v.get(key).and_then(|x| match x {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        })
    };

    let coded_w = num(video, "width").unwrap_or(0.0).round() as u32;
    let coded_h = num(video, "height").unwrap_or(0.0).round() as u32;
    if coded_w == 0 || coded_h == 0 {
        return Err("video track reports a zero frame size".into());
    }
    let (width, height) = match rotation_degrees(video) {
        90 | 270 => (coded_h, coded_w),
        _ => (coded_w, coded_h),
    };

    let frame_rate = video
        .get("r_frame_rate")
        .and_then(|r| r.as_str())
        .and_then(parse_rational)
        // Same fallback as the Swift, which substitutes 30 for a track
        // reporting no nominal rate.
        .unwrap_or(30.0);

    // Stream duration is missing on some containers; the format's covers it.
    let duration_seconds = num(video, "duration")
        .or_else(|| root.get("format").and_then(|f| num(f, "duration")))
        .filter(|d| *d > 0.0)
        // Nothing said how long it is: some containers carry a frame count
        // instead, which is the same fact at a different rate.
        .or_else(|| num(video, "nb_frames").map(|n| n / frame_rate))
        .unwrap_or(0.0);

    // max(1, round(duration * rate)) — the macOS expression exactly.
    let total_frames = ((duration_seconds * frame_rate).round() as i64).max(1) as usize;

    Ok(VideoInfo {
        width,
        height,
        frame_rate,
        duration_seconds,
        total_frames,
        has_audio,
        video_codec: video
            .get("codec_name")
            .and_then(|c| c.as_str())
            .unwrap_or("unknown")
            .to_string(),
    })
}

/// A video file, opened and probed.
///
/// It holds no decoder, only what ffprobe reported, so it is cheap to clone
/// and several readers over one file coexist — the player's and an export's,
/// for instance.
#[derive(Debug, Clone)]
pub struct VideoSource {
    pub path: PathBuf,
    pub info: VideoInfo,
}

impl VideoSource {
    pub fn open(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Err(format!("{} does not exist", path.display()));
        }
        let mut args = probe_args();
        args.extend(
            [
                "-show_entries",
                "stream=codec_type,codec_name,width,height,r_frame_rate,duration,nb_frames:stream_side_data=rotation:stream_tags=rotate:format=duration",
                "-of",
                "json",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        args.push(path.to_string_lossy().to_string());

        let mut proc = Process::spawn(&ffprobe_path(), &args, true, false)?;
        let mut json = String::new();
        if let Some(out) = proc.child.stdout.as_mut() {
            out.read_to_string(&mut json)
                .map_err(|e| format!("could not read ffprobe output: {e}"))?;
        }
        proc.finish("probing the file")?;

        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let info = parse_probe(&json).map_err(|e| format!("{name}: {e}"))?;
        Ok(Self { path: path.to_path_buf(), info })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.info.width, self.info.height)
    }

    /// Decode the single frame at `index` — the scrubbing path.
    ///
    /// Seeking before `-i` makes ffmpeg seek the input rather than decode and
    /// discard everything up to the target, which is what keeps a scrub
    /// responsive on a long clip.
    pub fn frame_at_index(&self, index: usize) -> Result<SourceImage, String> {
        let index = index.min(self.info.total_frames.saturating_sub(1));
        let mut args = base_args();
        args.push("-ss".to_string());
        args.push(format!("{:.6}", self.info.time_for_frame(index)));
        args.push("-i".to_string());
        args.push(self.path.to_string_lossy().to_string());
        args.push("-frames:v".to_string());
        args.push("1".to_string());
        args.extend(raw_output_args());

        let mut proc = Process::spawn(&ffmpeg_path(), &args, true, false)?;
        let mut pixels = vec![0u8; self.info.frame_bytes()];
        let read = proc
            .child
            .stdout
            .as_mut()
            .ok_or("ffmpeg produced no output pipe")?
            .read_exact(&mut pixels);
        if read.is_err() {
            let diag = proc.diagnostics();
            return Err(format!(
                "could not decode frame {index}{}",
                if diag.is_empty() { String::new() } else { format!(" \u{2014} {diag}") }
            ));
        }
        // Dropping the process kills it: a one-frame request deliberately
        // leaves ffmpeg mid-stream rather than waiting for it to finish.
        Ok(SourceImage { width: self.info.width, height: self.info.height, pixels })
    }

    /// Stream frames in order from `start_frame` — the playback and export path.
    pub fn sequential_reader(&self, start_frame: usize) -> Result<SequentialReader, String> {
        let mut args = base_args();
        if start_frame > 0 {
            args.push("-ss".to_string());
            args.push(format!("{:.6}", self.info.time_for_frame(start_frame)));
        }
        args.push("-i".to_string());
        args.push(self.path.to_string_lossy().to_string());
        args.extend(raw_output_args());

        let proc = Process::spawn(&ffmpeg_path(), &args, true, false)?;
        Ok(SequentialReader {
            proc,
            frame_bytes: self.info.frame_bytes(),
            width: self.info.width,
            height: self.info.height,
            buffer: vec![0u8; self.info.frame_bytes()],
        })
    }
}

/// Decode to tightly packed RGBA8 on stdout. `-an` because nothing in the
/// video path wants audio through us — the MP4 exporter muxes it from the
/// source file directly.
fn raw_output_args() -> Vec<String> {
    ["-an", "-sn", "-dn", "-f", "rawvideo", "-pix_fmt", "rgba", "-"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// An in-order frame stream. Dropping it kills the decoder.
pub struct SequentialReader {
    proc: Process,
    frame_bytes: usize,
    width: u32,
    height: u32,
    buffer: Vec<u8>,
}

impl SequentialReader {
    /// The next frame, or None at end of stream.
    ///
    /// The pixels live in a buffer owned by the reader and are overwritten by
    /// the following call, so a caller that keeps a frame copies it. That is
    /// the same contract as the Swift `Frame`, which holds its
    /// `CVPixelBuffer` only until the next one is consumed.
    pub fn next_frame(&mut self) -> Option<&[u8]> {
        let stdout = self.proc.child.stdout.as_mut()?;
        // A short read means the stream ended: a partial frame at the tail is
        // not a frame.
        match stdout.read_exact(&mut self.buffer[..self.frame_bytes]) {
            Ok(()) => Some(&self.buffer[..self.frame_bytes]),
            Err(_) => None,
        }
    }

    /// The next frame as an owned image, for callers that keep it.
    pub fn next_image(&mut self) -> Option<SourceImage> {
        let (width, height) = (self.width, self.height);
        self.next_frame()
            .map(|pixels| SourceImage { width, height, pixels: pixels.to_vec() })
    }

    /// Anything ffmpeg has complained about, for error messages.
    pub fn diagnostics(&self) -> String {
        self.proc.diagnostics()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rationals_parse_including_ntsc_rates() {
        assert_eq!(parse_rational("24/1"), Some(24.0));
        assert_eq!(parse_rational("30000/1001").map(|r| (r * 100.0).round()), Some(2997.0));
        // Degenerate forms ffprobe emits for streams with no real rate.
        assert_eq!(parse_rational("0/0"), None);
        assert_eq!(parse_rational("25"), None);
    }

    fn probe_json(extra_stream: &str, format_body: &str) -> String {
        format!(
            r#"{{ "streams": [
                 {{ "codec_type": "video", "codec_name": "h264",
                    "width": 1920, "height": 1080,
                    "r_frame_rate": "24/1" {extra_stream} }} ],
               "format": {{ {format_body} }} }}"#
        )
    }

    #[test]
    fn probe_reports_the_macos_frame_count() {
        let info = parse_probe(&probe_json(r#", "duration": "2.5""#, "")).unwrap();
        assert_eq!((info.width, info.height), (1920, 1080));
        assert_eq!(info.frame_rate, 24.0);
        // max(1, round(2.5 * 24)) = 60, the Swift expression exactly.
        assert_eq!(info.total_frames, 60);
        assert!(!info.has_audio);
        assert_eq!(info.video_codec, "h264");
    }

    #[test]
    fn a_missing_stream_duration_falls_back_to_the_container() {
        let info = parse_probe(&probe_json("", r#""duration": "4.0""#)).unwrap();
        assert_eq!(info.total_frames, 96);
    }

    #[test]
    fn with_no_duration_at_all_the_frame_count_stands_in() {
        let info = parse_probe(&probe_json(r#", "nb_frames": "120""#, "")).unwrap();
        assert_eq!(info.total_frames, 120);
    }

    #[test]
    fn a_clip_of_unknown_length_still_has_one_frame() {
        // Nothing to go on: the count must not come out zero, because every
        // consumer divides by it.
        let info = parse_probe(&probe_json("", "")).unwrap();
        assert_eq!(info.total_frames, 1);
    }

    #[test]
    fn quarter_turns_swap_the_decoded_axes() {
        for rotation in ["90", "-90", "270"] {
            let json = probe_json(
                &format!(r#", "side_data_list": [ {{ "rotation": {rotation} }} ]"#),
                "",
            );
            let info = parse_probe(&json).unwrap();
            assert_eq!((info.width, info.height), (1080, 1920), "rotation {rotation}");
        }
        // Half turns and upright clips keep the coded size.
        for rotation in ["180", "0"] {
            let json = probe_json(
                &format!(r#", "side_data_list": [ {{ "rotation": {rotation} }} ]"#),
                "",
            );
            let info = parse_probe(&json).unwrap();
            assert_eq!((info.width, info.height), (1920, 1080), "rotation {rotation}");
        }
    }

    #[test]
    fn a_legacy_rotate_tag_counts_too() {
        let json = probe_json(r#", "tags": { "rotate": "270" }"#, "");
        assert_eq!(parse_probe(&json).unwrap().width, 1080);
    }

    #[test]
    fn an_audio_track_is_noticed() {
        let json = r#"{ "streams": [
            { "codec_type": "audio", "codec_name": "aac" },
            { "codec_type": "video", "codec_name": "h264", "width": 640,
              "height": 480, "r_frame_rate": "25/1", "duration": "1.0" } ] }"#;
        let info = parse_probe(json).unwrap();
        assert!(info.has_audio);
        assert_eq!(info.total_frames, 25);
    }

    #[test]
    fn a_file_with_no_video_track_is_refused() {
        let json = r#"{ "streams": [ { "codec_type": "audio" } ] }"#;
        assert!(parse_probe(json).is_err());
        // And so is a video track with no usable size.
        let json = r#"{ "streams": [ { "codec_type": "video", "width": 0, "height": 0 } ] }"#;
        assert!(parse_probe(json).is_err());
    }

    #[test]
    fn frame_times_follow_the_rate() {
        let info = parse_probe(&probe_json(r#", "duration": "10.0""#, "")).unwrap();
        assert_eq!(info.time_for_frame(0), 0.0);
        assert_eq!(info.time_for_frame(24), 1.0);
        assert_eq!(info.frame_bytes(), 1920 * 1080 * 4);
    }

    #[test]
    fn video_extensions_are_recognised_case_insensitively() {
        assert!(is_video_path(Path::new("clip.MP4")));
        assert!(is_video_path(Path::new("/tmp/a.mkv")));
        assert!(!is_video_path(Path::new("still.png")));
        assert!(!is_video_path(Path::new("no-extension")));
    }
}
