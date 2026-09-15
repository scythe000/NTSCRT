//! NTSCRT for Windows.
//!
//! The pipeline and asset plumbing live here so both the GUI (`ntscrt`) and
//! the headless verifier (`ntscrt-smoke`, the counterpart of the macOS
//! `crt-smoke` target) drive exactly the same code.

pub mod app;
pub mod app_preset;
pub mod app_timeline;
pub mod app_video;
pub mod gpu;
pub mod image_io;
pub mod pacer;
pub mod param_gates;
pub mod presets;
pub mod render;
pub mod ui;
pub mod video;

pub use render::{FrameSequence, HeadlessRenderer, RenderSettings};
