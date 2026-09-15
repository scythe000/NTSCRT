//! GPU side of the pipeline: wgpu device, downscale kernels, shader chain.

pub mod chain;
pub mod downscaler;
pub mod grade;
pub mod pipeline;

pub use chain::ShaderChain;
pub use downscaler::{Downscaler, WORK_FORMAT};
pub use pipeline::{Pipeline, RenderRequest};
