//! NTSCRT for Windows.
//!
//! The pipeline and asset plumbing live here so both the GUI (`ntscrt`) and
//! the headless verifier (`ntscrt-smoke`, the counterpart of the macOS
//! `crt-smoke` target) drive exactly the same code.

pub mod gpu;
pub mod image_io;
pub mod presets;
pub mod render;

pub use render::{HeadlessRenderer, RenderSettings};
