//! wgpu side of the downscale stage, ported from
//! `Sources/CrtCore/Downscaler.swift`.
//!
//! Pipelines are created once and cached by kernel; the separable filters
//! share two entry points specialised on `FILTER_KIND` instead of the Metal
//! version's macro-stamped kernel pair per filter. The intermediate
//! (dstW x srcH) scratch texture is reused while dimensions match.

use std::collections::HashMap;

use vhs_studio_core::DownscaleMethod;
use wgpu::util::DeviceExt;

/// Storage-texture format for every intermediate in the chain. rgba8unorm is
/// the only 8-bit format guaranteed writable as a storage texture in core
/// wgpu, and it matches the BGRA/RGBA8 the NTSC stage round-trips through.
pub const WORK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Dims {
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
}

/// Which entry point / specialisation a method maps to.
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
enum Kernel {
    Nearest,
    Area,
    SeparableH(u32),
    SeparableV(u32),
}

fn filter_kind(method: DownscaleMethod) -> u32 {
    match method {
        DownscaleMethod::NearestAA => 0,
        DownscaleMethod::Bilinear => 1,
        DownscaleMethod::Bicubic => 2,
        DownscaleMethod::Lanczos => 3,
        // Not separable; never reaches the specialised entry points.
        DownscaleMethod::Nearest | DownscaleMethod::Area => 1,
    }
}

pub struct Downscaler {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<Kernel, wgpu::ComputePipeline>,
    scratch: Option<(u32, u32, wgpu::Texture)>,
}

impl Downscaler {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("downscale"),
            source: wgpu::ShaderSource::Wgsl(include_str!("downscale.wgsl").into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("downscale.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: WORK_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("downscale.pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        Self {
            module,
            layout,
            pipeline_layout,
            pipelines: HashMap::new(),
            scratch: None,
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device, kernel: Kernel) -> &wgpu::ComputePipeline {
        self.pipelines.entry(kernel).or_insert_with(|| {
            let (entry, kind) = match kernel {
                Kernel::Nearest => ("downscale_nearest", None),
                Kernel::Area => ("downscale_area", None),
                Kernel::SeparableH(k) => ("downscale_separable_h", Some(k)),
                Kernel::SeparableV(k) => ("downscale_separable_v", Some(k)),
            };
            let specialised = [("FILTER_KIND", kind.unwrap_or(0) as f64)];
            let constants: &[(&str, f64)] = if kind.is_some() { &specialised } else { &[] };
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&self.pipeline_layout),
                module: &self.module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    zero_initialize_workgroup_memory: false,
                },
                cache: None,
            })
        })
    }

    /// Encode `source` -> `destination` with the chosen kernel.
    ///
    /// `src_size` and `dst_size` are passed explicitly because the separable
    /// passes need the *original* source dimensions in both halves, not the
    /// scratch texture's.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        src_size: (u32, u32),
        destination: &wgpu::TextureView,
        dst_size: (u32, u32),
        method: DownscaleMethod,
    ) {
        let dims = Dims { sw: src_size.0, sh: src_size.1, dw: dst_size.0, dh: dst_size.1 };

        if !method.is_separable() {
            let kernel = if matches!(method, DownscaleMethod::Area) { Kernel::Area } else { Kernel::Nearest };
            self.dispatch(device, encoder, kernel, source, destination, dims, dst_size);
            return;
        }

        // Separable: horizontal into scratch (dstW x srcH), then vertical.
        let kind = filter_kind(method);
        let scratch_view = {
            let tex = self.scratch_texture(device, dst_size.0, src_size.1);
            tex.create_view(&wgpu::TextureViewDescriptor::default())
        };

        self.dispatch(
            device, encoder, Kernel::SeparableH(kind),
            source, &scratch_view, dims, (dst_size.0, src_size.1),
        );
        self.dispatch(
            device, encoder, Kernel::SeparableV(kind),
            &scratch_view, destination, dims, dst_size,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        kernel: Kernel,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        dims: Dims,
        grid: (u32, u32),
    ) {
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("downscale.dims"),
            contents: bytemuck::bytes_of(&dims),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("downscale.bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(dst) },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
            ],
        });

        // Borrow the pipeline before starting the pass so `self` is free.
        let pipeline = self.pipeline(device, kernel).clone();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("downscale.pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(grid.0.div_ceil(8), grid.1.div_ceil(8), 1);
    }

    fn scratch_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) -> &wgpu::Texture {
        let matches = matches!(&self.scratch, Some((w, h, _)) if *w == width && *h == height);
        if !matches {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("downscale.scratch"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORK_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            });
            self.scratch = Some((width, height, tex));
        }
        &self.scratch.as_ref().unwrap().2
    }
}
