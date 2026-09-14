//! Video: decoding, playback, and the movie export paths.
//!
//! The macOS build gets all of this from AVFoundation. Windows has no
//! equivalent covering the same codecs, so decode and encode go through
//! ffmpeg — see [`ffmpeg`] for why it is driven as a subprocess rather than
//! linked. Everything above that boundary is the still pipeline unchanged:
//! frames arrive as tightly packed RGBA8, exactly the shape
//! [`crate::image_io::SourceImage`] holds.

pub mod ffmpeg;
pub mod source;

pub use source::{is_video_path, SequentialReader, VideoInfo, VideoSource, VIDEO_EXTENSIONS};
