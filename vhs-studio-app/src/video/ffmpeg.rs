//! Locating and launching ffmpeg, and the plumbing shared by every module
//! that spawns it.
//!
//! The macOS build decodes and encodes through AVFoundation, which is part of
//! the OS. Windows has no equivalent that covers the same codecs, so video I/O
//! goes through ffmpeg. It is driven as a **subprocess over pipes** rather
//! than by linking libav*:
//!
//! - the README's build requirements stay "Rust + MSVC" — linking libav needs
//!   an ffmpeg development build, `pkg-config` and a C toolchain wired to it,
//!   none of which a `cargo build` can assume on Windows;
//! - the pipeline only ever wants whole RGBA frames, which is exactly what
//!   `-f rawvideo -pix_fmt rgba` hands over, so the binding would buy nothing;
//! - ffmpeg ships as a standalone executable everywhere, so it can sit
//!   beside ours in the zip (see `package.ps1`) or come from PATH.
//!
//! Lookup order, per tool: the `VHS_STUDIO_FFMPEG` / `VHS_STUDIO_FFPROBE` override;
//! then a copy **beside our own executable**, which is what the packaged zip
//! ships (a pinned build, so decoding and encoding behave the same on every
//! machine and HEIC works regardless of what else is installed); then the
//! bare name, i.e. whatever PATH has, for source-tree runs.
//!
//! The cost is one process per decode/encode session and a pipe copy per
//! frame. At the frame sizes this app works with, the NTSC stage is still two
//! orders of magnitude more expensive (see `PlaybackPipeline`).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// Set `VHS_STUDIO_FFMPEG` / `VHS_STUDIO_FFPROBE` to a full executable path to
/// override the lookup — the counterpart of `VHS_STUDIO_SHADERS`.
pub fn ffmpeg_path() -> PathBuf {
    tool_path("VHS_STUDIO_FFMPEG", "ffmpeg")
}

pub fn ffprobe_path() -> PathBuf {
    tool_path("VHS_STUDIO_FFPROBE", "ffprobe")
}

/// Where a tool comes from, for the status line and the packaging check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    /// `VHS_STUDIO_FFMPEG` / `VHS_STUDIO_FFPROBE`.
    Override,
    /// Beside our own executable — the copy the zip ships.
    Bundled,
    /// The bare name, resolved by the OS through PATH.
    Path,
}

impl ToolSource {
    pub fn describe(self) -> &'static str {
        match self {
            ToolSource::Override => "from VHS_STUDIO_FFMPEG/VHS_STUDIO_FFPROBE",
            ToolSource::Bundled => "bundled beside the app",
            ToolSource::Path => "from PATH",
        }
    }
}

/// The path a tool will be launched by and why. `default` is the bare name
/// (`ffmpeg`); the executable suffix is added where the OS needs one.
pub fn resolve_tool(var: &str, default: &str) -> (PathBuf, ToolSource) {
    if let Ok(p) = std::env::var(var) {
        if !p.trim().is_empty() {
            return (PathBuf::from(p), ToolSource::Override);
        }
    }
    if let Some(p) = beside_executable(default) {
        return (p, ToolSource::Bundled);
    }
    (PathBuf::from(default), ToolSource::Path)
}

fn tool_path(var: &str, default: &str) -> PathBuf {
    resolve_tool(var, default).0
}

/// `<dir of our exe>/<name>[.exe]`, when such a file exists.
fn beside_executable(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let candidate = dir.join(file);
    candidate.is_file().then_some(candidate)
}

/// Whether both tools can be launched, with the version string when they can.
///
/// Video is the one part of this build with an external runtime dependency,
/// so the failure has to be legible: the app and the verifier both report
/// this rather than letting a spawn failure surface as "file not found".
pub fn probe_tools() -> Result<String, String> {
    let version = |path: PathBuf, name: &str| -> Result<String, String> {
        let out = command(&path)
            .arg("-version")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                format!(
                    "{name} could not be run ({e}). Put {name}.exe beside vhs-studio.exe (the \
                     release zip ships it), install ffmpeg on PATH, or set VHS_STUDIO_{} to \
                     the executable.",
                    name.to_uppercase()
                )
            })?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .next()
            .unwrap_or("unknown version")
            .trim()
            .to_string())
    };
    let reported = version(ffmpeg_path(), "ffmpeg")?;
    // ffprobe is checked too — they install together, but a PATH can have one
    // without the other, and the failure should name which is missing.
    version(ffprobe_path(), "ffprobe")?;
    Ok(reported)
}

/// A `Command` for one of the tools, with the platform flags every launch
/// needs. On Windows that is CREATE_NO_WINDOW: the GUI build is a
/// windows-subsystem binary, and without it every spawn — including the
/// `-version` probe when a video is opened — flashes a console.
fn command(program: &PathBuf) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// A running ffmpeg, with its stderr drained in the background.
///
/// The drain thread matters: ffmpeg writes to stderr unprompted, and a full
/// stderr pipe blocks the process mid-frame, which would deadlock against our
/// read of stdout. Draining also means the text is available to put in the
/// error when a session ends badly, instead of just "broken pipe".
pub struct Process {
    pub child: Child,
    stderr: Arc<Mutex<String>>,
}

impl Process {
    /// Spawn `args`, with stdout piped when `capture_stdout` (decoding) and
    /// stdin piped when `capture_stdin` (encoding).
    pub fn spawn(
        program: &PathBuf,
        args: &[String],
        capture_stdout: bool,
        capture_stdin: bool,
    ) -> Result<Self, String> {
        let mut cmd = command(program);
        cmd.args(args)
            .stdin(if capture_stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(if capture_stdout { Stdio::piped() } else { Stdio::null() })
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "could not run {} ({e}). Install ffmpeg and put it on PATH, \
                 or set VHS_STUDIO_FFMPEG to the executable.",
                program.display()
            )
        })?;

        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(mut pipe) = child.stderr.take() {
            let sink = Arc::clone(&stderr);
            std::thread::Builder::new()
                .name("vhs-studio.ffmpeg.stderr".into())
                .spawn(move || {
                    let mut buf = Vec::new();
                    let _ = pipe.read_to_end(&mut buf);
                    if let Ok(mut s) = sink.lock() {
                        s.push_str(&String::from_utf8_lossy(&buf));
                    }
                })
                .ok();
        }
        Ok(Self { child, stderr })
    }

    /// Whatever ffmpeg has said so far, trimmed to the last few lines — the
    /// useful part of a failure is always at the end.
    pub fn diagnostics(&self) -> String {
        let Ok(text) = self.stderr.lock() else {
            return String::new();
        };
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = lines.len().saturating_sub(4);
        lines[tail..].join("; ")
    }

    /// Wait for exit, turning a non-zero status into an error carrying the
    /// diagnostics.
    pub fn finish(mut self, what: &str) -> Result<(), String> {
        let status = self
            .child
            .wait()
            .map_err(|e| format!("{what}: waiting for ffmpeg failed ({e})"))?;
        if status.success() {
            return Ok(());
        }
        let diag = self.diagnostics();
        Err(if diag.is_empty() {
            format!("{what}: ffmpeg exited with {status}")
        } else {
            format!("{what}: ffmpeg exited with {status} — {diag}")
        })
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Sessions are routinely abandoned (a seek during playback, a
        // cancelled export), and an ffmpeg left writing into a pipe nobody
        // reads would sit there forever.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Arguments every ffmpeg invocation wants: never touch the terminal, never
/// read our stdin, and only speak up about real problems.
pub fn base_args() -> Vec<String> {
    ["-hide_banner", "-nostdin", "-loglevel", "error"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The same for ffprobe, which has no `-nostdin` (it never reads stdin) and
/// rejects the option outright rather than ignoring it.
pub fn probe_args() -> Vec<String> {
    ["-hide_banner", "-loglevel", "error"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
