//! wgpu side of the colour-grade stage. See `ntscrt_core::grade` for the
//! model and `grade.wgsl` for the math; this file only moves texels through
//! it. One compute pass, chain-input sized, so it costs next to nothing.

use ntscrt_core::GRADE_UNIFORM_LEN;
use wgpu::util::DeviceExt;

use super::downscaler::WORK_FORMAT;

pub struct GradePass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    /// Output texture, reused while the chain input keeps its size.
    target: Option<(u32, u32, wgpu::Texture)>,
}

impl GradePass {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grade"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grade.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grade.bgl"),
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
            label: Some("grade.pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("grade_main"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("grade_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { layout, pipeline, target: None }
    }

    /// Grade `source` into a texture of the same size and return it. The
    /// returned handle is the pass's own target, valid until the next call.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        uniform: &[f32; GRADE_UNIFORM_LEN],
    ) -> wgpu::Texture {
        let (width, height) = (source.width(), source.height());
        let target = self.target_texture(device, width, height).clone();

        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grade.uniform"),
            contents: bytemuck::cast_slice(uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let src_view = source.create_view(&Default::default());
        let dst_view = target.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grade.bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&dst_view) },
                wgpu::BindGroupEntry { binding: 2, resource: buffer.as_entire_binding() },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("grade.pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        target
    }

    fn target_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) -> &wgpu::Texture {
        let matches = matches!(&self.target, Some((w, h, _)) if *w == width && *h == height);
        if !matches {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ntscrt.graded"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORK_FORMAT,
                // Read by the chain (TEXTURE_BINDING), written here (STORAGE),
                // and copied out when a headless render reads the chain input.
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.target = Some((width, height, tex));
        }
        &self.target.as_ref().unwrap().2
    }
}
