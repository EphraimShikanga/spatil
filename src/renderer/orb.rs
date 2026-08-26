use bytemuck::{Pod, Zeroable};

use crate::config::{Shell, parse_hex_color};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "top_left" | "topleft" => Some(Self::TopLeft),
            "top_right" | "topright" => Some(Self::TopRight),
            "bottom_left" | "bottomleft" => Some(Self::BottomLeft),
            "bottom_right" | "bottomright" => Some(Self::BottomRight),
            _ => None,
        }
    }
}

pub struct OrbParams {
    pub enabled: bool,
    pub corner: Corner,
    pub size: f32,
    pub margin: f32,
    pub idle_ms: f32,
    pub body_color: [f32; 4],
    pub accent_color: [f32; 4],
}

impl OrbParams {
    pub fn from_config(shell: &Shell) -> Self {
        let corner = Corner::parse(&shell.familiar_corner).unwrap_or_else(|| {
            tracing::warn!(
                "unknown familiar_corner {:?}; falling back to bottom_right",
                shell.familiar_corner
            );
            Corner::BottomRight
        });
        Self {
            enabled: shell.familiar_enabled,
            corner,
            size: shell.familiar_size.max(1.0),
            margin: shell.familiar_margin.max(0.0),
            idle_ms: shell.idle_ms as f32,
            body_color: parse_hex_color(&shell.familiar_color),
            accent_color: parse_hex_color(&shell.familiar_accent),
        }
    }
}

pub struct OrbFrame {
    pub rect: [f32; 4],
    pub seconds_of_day: f32,
    pub open: f32,
    pub hover: f32,
    pub ambient: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct OrbUniform {
    pub rect: [f32; 4],
    pub screen: [f32; 4],
    pub body_color: [f32; 4],
    pub accent_color: [f32; 4],
    pub seconds_of_day: f32,
    pub open: f32,
    pub hover: f32,
    pub ambient: f32,
}

pub struct OrbPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
}

impl OrbPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/orb.wgsl"));

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("orb uniform bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("orb pipeline layout"),
            bind_group_layouts: &[Some(&uniform_bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("orb pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("orb uniform buffer"),
            size: std::mem::size_of::<OrbUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("orb uniform bind group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        Self { pipeline, uniform_buffer, uniform_bind_group }
    }
}
