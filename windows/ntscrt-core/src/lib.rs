//! Platform-neutral half of NTSCRT, ported from the macOS `CrtCore` target.
//!
//! Nothing in here touches a GPU or a windowing system: it is the signal
//! stage (ntsc-rs), the resampling/scanline math the renderer needs, and the
//! preset model. The Windows front end (`ntscrt-app`) supplies wgpu and egui
//! on top.
//!
//! Where the macOS build reached ntsc-rs through an Objective-C bridge over a
//! C ABI (`Vendor/ntscrs-capi`), this crate calls the same Rust library
//! directly — same effect, same preset JSON, one less boundary.

pub mod downscale;
pub mod ntsc;
pub mod rotation;
pub mod scanline;
pub mod settings_ui;

pub use downscale::{DownscaleMethod, DownscaleSpec};
pub use ntsc::{NtscStage, PixelFormat};
pub use rotation::{rotate_rgba, rotate_rgba_into, Rotation};
pub use scanline::ScanlineGrid;
pub use settings_ui::{descriptors, AnySetting, SettingDescriptor, SettingKind};
