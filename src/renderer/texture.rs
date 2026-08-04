use cgmath::SquareMatrix;
use wgpu::util::DeviceExt;

use crate::renderer::pipeline::QuadUniform;

/// A committed surface's texture plus its per-quad uniform buffer and bind
/// groups (color pass at `@group(0)`, shadow pass at `@group(1)`).
pub struct TexturedQuad {
    pub texture: wgpu::Texture,
    pub bind_group: wgpu::BindGroup,
    pub shadow_bind_group: wgpu::BindGroup,
    pub uniform_buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    pub uv_scale: [f32; 2],
    pub has_alpha: bool,
}

impl TexturedQuad {
    pub fn from_bytes(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bind_group_layout: &wgpu::BindGroupLayout,
        shadow_quad_layout: &wgpu::BindGroupLayout,
        width: u32,
        height: u32,
        stride: u32,
        bytes: &[u8],
        has_alpha: bool,
    ) -> Self {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shm surface texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Wayland's little-endian Argb8888/Xrgb8888 SHM formats are
            // byte-order BGRA in memory -- a direct match, no swizzle needed.
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let bytes_per_pixel = 4;
        let packed_stride = width * bytes_per_pixel;
        let packed_bytes: std::borrow::Cow<[u8]> = if stride == packed_stride {
            std::borrow::Cow::Borrowed(bytes)
        } else {
            let mut packed = Vec::with_capacity((packed_stride * height) as usize);
            for row in 0..height {
                let start = (row * stride) as usize;
                let end = start + packed_stride as usize;
                packed.extend_from_slice(&bytes[start..end]);
            }
            std::borrow::Cow::Owned(packed)
        };

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &packed_bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(packed_stride),
                rows_per_image: Some(height),
            },
            size,
        );

        Self::assemble(
            device,
            bind_group_layout,
            shadow_quad_layout,
            texture,
            width,
            height,
            [1.0, 1.0],
            has_alpha,
        )
    }

    /// Wraps an already-created texture (the zero-copy dmabuf path); no upload.
    pub fn from_wgpu_texture(
        device: &wgpu::Device,
        bind_group_layout: &wgpu::BindGroupLayout,
        shadow_quad_layout: &wgpu::BindGroupLayout,
        texture: wgpu::Texture,
        width: u32,
        height: u32,
        uv_scale: [f32; 2],
        has_alpha: bool,
    ) -> Self {
        Self::assemble(
            device,
            bind_group_layout,
            shadow_quad_layout,
            texture,
            width,
            height,
            uv_scale,
            has_alpha,
        )
    }

    fn assemble(
        device: &wgpu::Device,
        bind_group_layout: &wgpu::BindGroupLayout,
        shadow_quad_layout: &wgpu::BindGroupLayout,
        texture: wgpu::Texture,
        width: u32,
        height: u32,
        uv_scale: [f32; 2],
        has_alpha: bool,
    ) -> Self {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("surface sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quad uniform buffer"),
            contents: bytemuck::cast_slice(&[QuadUniform {
                mvp: cgmath::Matrix4::identity().into(),
                model: cgmath::Matrix4::identity().into(),
                uv_scale,
                alpha: 1.0,
                border_width: 0.0,
                tint: [0.0; 4],
                border_color: [0.0; 4],
                surface_alpha: if has_alpha { 1.0 } else { 0.0 },
                _pad: [0.0; 3],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad bind group"),
            layout: bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad shadow bind group"),
            layout: shadow_quad_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            texture,
            bind_group,
            shadow_bind_group,
            uniform_buffer,
            width,
            height,
            uv_scale,
            has_alpha,
        }
    }
}
