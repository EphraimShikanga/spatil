use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use cosmic_text::CacheKey;
use wgpu::util::DeviceExt;

use crate::text::GlyphImage;

pub const ATLAS_SIZE: u32 = 1024;
const GLYPH_PAD: u32 = 1;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct TextUniform {
    pub screen: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct GlyphInstance {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub color: [f32; 4],
}

impl GlyphInstance {
    const ATTRIBS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GlyphInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextRun {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: [f32; 4],
}

#[derive(Copy, Clone, Debug)]
pub struct AtlasEntry {
    pub uv: [f32; 4],
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub color: bool,
}

#[derive(Copy, Clone, Debug)]
struct Shelf {
    y: u32,
    height: u32,
    next_x: u32,
}

pub struct GlyphAtlas {
    entries: HashMap<CacheKey, AtlasEntry>,
    shelves: Vec<Shelf>,
    used_height: u32,
    resets: u64,
}

impl GlyphAtlas {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            shelves: Vec::new(),
            used_height: 0,
            resets: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn resets(&self) -> u64 {
        self.resets
    }

    pub fn get(&self, key: &CacheKey) -> Option<&AtlasEntry> {
        self.entries.get(key)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.shelves.clear();
        self.used_height = 0;
    }

    /// Shelf-packs a `width`x`height` box, returning its top-left in atlas pixels.
    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let (w, h) = (width + GLYPH_PAD, height + GLYPH_PAD);
        if w > ATLAS_SIZE || h > ATLAS_SIZE {
            return None;
        }
        for shelf in &mut self.shelves {
            // A much taller shelf would waste its whole band on a short glyph.
            if shelf.height >= h && shelf.height <= h * 2 && shelf.next_x + w <= ATLAS_SIZE {
                let x = shelf.next_x;
                shelf.next_x += w;
                return Some((x, shelf.y));
            }
        }
        if self.used_height + h > ATLAS_SIZE {
            return None;
        }
        let shelf = Shelf {
            y: self.used_height,
            height: h,
            next_x: w,
        };
        self.used_height += h;
        self.shelves.push(shelf);
        Some((0, shelf.y))
    }
}

pub struct TextPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub uniform_buffer: wgpu::Buffer,
    pub mask_bind_group: wgpu::BindGroup,
    pub color_bind_group: wgpu::BindGroup,
    pub instances: wgpu::Buffer,
    pub capacity: usize,
    pub atlas: GlyphAtlas,
    texture: wgpu::Texture,
}

impl TextPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/text.wgsl"));

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("text pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(GlyphInstance::layout())],
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

        // One texture, two views. Coverage must be sampled raw, so the atlas is
        // linear; emoji bitmaps are sRGB-encoded and need decoding on sample.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ],
        });
        let mask_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("glyph atlas mask view"),
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        let color_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("glyph atlas colour view"),
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph atlas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("text uniform buffer"),
            size: std::mem::size_of::<TextUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = |label, view| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        let mask_bind_group = bind_group("text mask bind group", &mask_view);
        let color_bind_group = bind_group("text colour bind group", &color_view);

        let capacity = 64;
        let instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("glyph instance buffer"),
            contents: bytemuck::cast_slice(&vec![GlyphInstance::zeroed(); capacity]),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            pipeline,
            uniform_buffer,
            mask_bind_group,
            color_bind_group,
            instances,
            capacity,
            atlas: GlyphAtlas::new(),
            texture,
        }
    }

    /// Uploads a rasterized glyph, resetting the atlas once it can no longer fit one.
    pub fn insert(&mut self, queue: &wgpu::Queue, key: CacheKey, image: &GlyphImage) -> bool {
        let origin = match self.atlas.allocate(image.width, image.height) {
            Some(origin) => origin,
            None => {
                self.atlas.clear();
                self.atlas.resets += 1;
                tracing::debug!("glyph atlas full; cleared and repacking");
                match self.atlas.allocate(image.width, image.height) {
                    Some(origin) => origin,
                    None => {
                        tracing::warn!(
                            width = image.width,
                            height = image.height,
                            "glyph does not fit an empty atlas; skipping"
                        );
                        return false;
                    }
                }
            }
        };

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin.0,
                    y: origin.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: Some(image.height),
            },
            wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
        );

        let scale = 1.0 / ATLAS_SIZE as f32;
        self.atlas.entries.insert(
            key,
            AtlasEntry {
                uv: [
                    origin.0 as f32 * scale,
                    origin.1 as f32 * scale,
                    (origin.0 + image.width) as f32 * scale,
                    (origin.1 + image.height) as f32 * scale,
                ],
                width: image.width,
                height: image.height,
                left: image.left,
                top: image.top,
                color: image.color,
            },
        );
        true
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[GlyphInstance]) {
        if data.is_empty() {
            return;
        }
        if data.len() > self.capacity {
            self.capacity = data.len().next_power_of_two();
            self.instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("glyph instance buffer"),
                contents: bytemuck::cast_slice(&vec![GlyphInstance::zeroed(); self.capacity]),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(data));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas() -> GlyphAtlas {
        GlyphAtlas::new()
    }

    #[test]
    fn boxes_pack_left_to_right_along_one_shelf() {
        let mut a = atlas();
        let first = a.allocate(10, 20).expect("first fits");
        let second = a.allocate(10, 20).expect("second fits");
        assert_eq!(first, (0, 0));
        assert_eq!(second.1, 0, "same shelf");
        assert!(second.0 > first.0, "advanced rightwards");
    }

    #[test]
    fn a_full_row_opens_a_new_shelf_below() {
        let mut a = atlas();
        let first = a.allocate(ATLAS_SIZE - 1, 20).expect("fills the row");
        let second = a.allocate(ATLAS_SIZE - 1, 20).expect("next shelf");
        assert_eq!(first.1, 0);
        assert!(second.1 >= 20, "dropped below the first shelf");
    }

    #[test]
    fn a_short_box_does_not_waste_a_tall_shelf() {
        let mut a = atlas();
        a.allocate(10, 100).expect("tall shelf");
        let short = a.allocate(10, 4).expect("short box");
        assert_ne!(short.1, 0, "opened its own shelf rather than reusing the tall one");
    }

    #[test]
    fn allocation_fails_once_the_atlas_is_exhausted() {
        let mut a = atlas();
        while a.allocate(ATLAS_SIZE - 1, 64).is_some() {}
        assert!(a.allocate(ATLAS_SIZE - 1, 64).is_none());
    }

    #[test]
    fn a_box_larger_than_the_atlas_is_refused_outright() {
        let mut a = atlas();
        assert!(a.allocate(ATLAS_SIZE + 1, 10).is_none());
        assert!(a.allocate(10, ATLAS_SIZE + 1).is_none());
    }

    #[test]
    fn clearing_returns_the_whole_atlas() {
        let mut a = atlas();
        while a.allocate(ATLAS_SIZE - 1, 64).is_some() {}
        a.clear();
        assert_eq!(a.allocate(10, 20), Some((0, 0)));
        assert_eq!(a.len(), 0);
    }

    #[test]
    fn padding_keeps_neighbours_from_touching() {
        let mut a = atlas();
        let first = a.allocate(10, 20).expect("first");
        let second = a.allocate(10, 20).expect("second");
        assert!(
            second.0 >= first.0 + 10 + GLYPH_PAD,
            "adjacent glyphs would bleed under linear filtering"
        );
    }
}
