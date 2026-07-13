use bytemuck::{Pod, Zeroable};
use cgmath::{Matrix4, SquareMatrix};
use wgpu::util::DeviceExt;

use super::pipeline::Vertex;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct LightUniform {
    pub view_proj: [[f32; 4]; 4],
}

pub struct ShadowMap {
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

impl ShadowMap {
    pub fn new(device: &wgpu::Device, size: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Comparison sampler + Linear = free 2×2 PCF.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        Self { view, sampler }
    }
}

pub struct ShadowPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub light_uniform_buffer: wgpu::Buffer,
    pub light_bind_group: wgpu::BindGroup,
    pub quad_bind_group_layout: wgpu::BindGroupLayout,
}

impl ShadowPipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/shadow.wgsl"));

        let light_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shadow light bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let quad_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shadow quad bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow pipeline layout"),
            bind_group_layouts: &[Some(&light_bind_group_layout), Some(&quad_bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(Vertex::layout())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            // Depth-only: no fragment stage, no color targets.
            fragment: None,
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let light_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shadow light uniform buffer"),
            contents: bytemuck::cast_slice(&[LightUniform {
                view_proj: Matrix4::identity().into(),
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let light_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow light bind group"),
            layout: &light_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            light_uniform_buffer,
            light_bind_group,
            quad_bind_group_layout,
        }
    }
}
