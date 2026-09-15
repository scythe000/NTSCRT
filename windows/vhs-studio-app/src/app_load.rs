//! Opening a file without freezing the window.
//!
//! Opening a video is four subprocesses in a row — `ffmpeg -version`,
//! `ffprobe -version`, the probe, and an ffmpeg decode of the first frame —
//! and opening a large still is a full decode; on Windows each spawn alone
//! is tens of milliseconds and the decode of a 4K frame far more. Done
//! inline, as it first was, the window stopped painting for the duration
//! and the user saw a hang.
//!
//! So the work runs on a thread and the result comes back through a
//! channel that `ui()` polls once a frame. Everything that touches the app
//! (`apply_loaded_*`) still happens on the UI thread; the thread only
//! produces plain data. Opening another file while one is in flight simply
//! replaces the task — the old thread finishes into a dropped receiver.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::Instant;

use crate::app::VhsStudioApp;
use crate::image_io::SourceImage;
use crate::video::VideoSource;

/// What the loader thread hands back.
pub enum Loaded {
    Image(SourceImage),
    /// The opened clip and its first frame, decoded.
    Video { video: VideoSource, first: SourceImage },
}

/// A file being opened on a worker thread.
pub struct LoadTask {
    pub path: PathBuf,
    pub started: Instant,
    receiver: Receiver<Result<Loaded, String>>,
}

impl LoadTask {
    /// The file's name, for the indicator.
    pub fn file_name(&self) -> String {
        self.path.file_name().unwrap_or_default().to_string_lossy().to_string()
    }
}

/// Everything that can run off the UI thread: the whole decode.
fn load_off_thread(path: &Path) -> Result<Loaded, String> {
    if crate::video::is_video_path(path) {
        // ffmpeg is the one external runtime dependency in this build, so
        // say so plainly rather than letting a spawn failure surface as a
        // decode error on every file the user tries.
        crate::video::ffmpeg::probe_tools()?;
        let video = VideoSource::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
        let first = video
            .frame_at_index(0)
            .map_err(|e| format!("Could not decode the first frame: {e}"))?;
        Ok(Loaded::Video { video, first })
    } else {
        SourceImage::load(path)
            .map(Loaded::Image)
            .map_err(|e| format!("Could not open {}: {e}", path.display()))
    }
}

impl VhsStudioApp {
    /// Open a file as the source. Returns at once; the picture changes when
    /// the decode finishes (see [`Self::poll_load`]). Until then the
    /// current source stays on screen and the toolbar shows what is being
    /// opened.
    pub fn load_source(&mut self, path: PathBuf) {
        let (sender, receiver) = channel();
        let worker_path = path.clone();
        let spawned = std::thread::Builder::new()
            .name("vhs-studio.load".into())
            .spawn(move || {
                let result = load_off_thread(&worker_path);
                // A dropped receiver means the user opened something else
                // meanwhile; nothing to do with the result.
                let _ = sender.send(result);
            });
        match spawned {
            Ok(_) => {
                self.status = Some(format!(
                    "Opening {}\u{2026}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                self.error = None;
                self.load_task = Some(LoadTask { path, started: Instant::now(), receiver });
            }
            Err(e) => self.error = Some(format!("Could not start opening {}: {e}", path.display())),
        }
    }

    pub fn is_loading(&self) -> bool {
        self.load_task.is_some()
    }

    /// Fold a finished load back into the app. Called once a frame.
    pub(crate) fn poll_load(&mut self) {
        let Some(task) = &self.load_task else { return };
        let result = match task.receiver.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the loader stopped without a result".to_string()),
        };
        let task = self.load_task.take().unwrap();
        match result {
            Ok(Loaded::Image(img)) => self.apply_loaded_image(task.path, img),
            Ok(Loaded::Video { video, first }) => self.apply_loaded_video(task.path, video, first),
            Err(e) => {
                self.error = Some(e);
                // The "Opening…" status would otherwise sit under the error.
                self.status = None;
            }
        }
    }

    fn apply_loaded_image(&mut self, path: PathBuf, img: SourceImage) {
        // Dropping the video state stops its producer thread.
        self.video = None;
        self.status = Some(format!(
            "Loaded {} ({}x{})",
            path.file_name().unwrap_or_default().to_string_lossy(),
            img.width,
            img.height
        ));
        self.original_source = Some(img);
        self.rebuild_rotation();
        self.source_path = Some(path);
        self.source_version += 1;
        self.ntsc.invalidate();
        self.error = None;
        self.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_still_decodes_off_thread_and_arrives_through_the_channel() {
        let dir = std::env::temp_dir().join(format!("vhs-studio-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tiny.png");
        crate::image_io::save_png(&path, &[255, 0, 0, 255, 0, 255, 0, 255], 2, 1).unwrap();

        // The same shape `load_source` uses: worker thread, channel, poll.
        let (sender, receiver) = channel();
        let worker_path = path.clone();
        std::thread::spawn(move || {
            let _ = sender.send(load_off_thread(&worker_path));
        });
        match receiver.recv_timeout(std::time::Duration::from_secs(10)).unwrap() {
            Ok(Loaded::Image(img)) => assert_eq!((img.width, img.height), (2, 1)),
            Ok(Loaded::Video { .. }) => panic!("a .png is not a video"),
            Err(e) => panic!("{e}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_reports_its_path() {
        let path = Path::new("/definitely/not/here.png");
        let err = load_off_thread(path).err().expect("must fail");
        assert!(err.contains("Could not open"), "{err}");
        assert!(err.contains("here.png"), "{err}");
    }

    #[test]
    fn a_missing_video_fails_before_any_decode() {
        let path = Path::new("/definitely/not/here.mp4");
        let err = load_off_thread(path).err().expect("must fail");
        // Either ffmpeg is missing or the file is; both are stated plainly.
        assert!(err.contains("ffmpeg") || err.contains("here.mp4"), "{err}");
    }
}
