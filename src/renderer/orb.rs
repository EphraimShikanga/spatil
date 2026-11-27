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
    /// Dial angle (clockwise from twelve) pointing from this corner into the screen.
    pub fn facing(self) -> f32 {
        let eighth = std::f32::consts::FRAC_PI_4;
        match self {
            Self::TopLeft => 3.0 * eighth,
            Self::TopRight => 5.0 * eighth,
            Self::BottomRight => 7.0 * eighth,
            Self::BottomLeft => eighth,
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "top_left" | "topleft" => Some(Self::TopLeft),
            "top_right" | "topright" => Some(Self::TopRight),
            "bottom_left" | "bottomleft" => Some(Self::BottomLeft),
            "bottom_right" | "bottomright" => Some(Self::BottomRight),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
        }
    }
}

/// The ambient second hand's tip, 24 of the face's 28 design units.
pub const SECOND_HAND: f32 = 24.0 / 28.0;

pub struct OrbParams {
    pub enabled: bool,
    pub corner: Corner,
    pub size: f32,
    pub margin: f32,
    pub idle_ms: f32,
    pub digits: bool,
    pub body_color: [f32; 4],
    pub accent_color: [f32; 4],
    pub palette: OrbPalette,
}

/// The design's fixed colours (`dev/familiar-design/HANDOFF.md` tokens), in linear light.
pub struct OrbPalette {
    pub ink: [f32; 4],
    pub hand_ink: [f32; 4],
    pub dark: [f32; 4],
    pub amber: [f32; 4],
    pub quiet: [f32; 4],
    pub deep: [f32; 4],
    pub face_hover: [f32; 4],
    pub face_ambient: [f32; 4],
    pub hand_ambient: [f32; 4],
}

impl OrbPalette {
    /// The hover and ambient faces are the design's hexes for its own `#12161C`, scaled with
    /// whatever `familiar_color` is set to.
    fn new(body: [f32; 4]) -> Self {
        let rest = parse_hex_color("#12161C");
        let follow = |hex: &str| {
            let c = parse_hex_color(hex);
            std::array::from_fn(|i| if i == 3 { 1.0 } else { c[i] * body[i] / rest[i].max(1e-4) })
        };
        Self {
            face_hover: follow("#1A1F27"),
            face_ambient: follow("#0C0E12"),
            hand_ambient: parse_hex_color("#D7E3F2"),
            ink: parse_hex_color("#F2F4F8"),
            hand_ink: parse_hex_color("#F3F5F8"),
            dark: parse_hex_color("#0B0D10"),
            amber: parse_hex_color("#F5A524"),
            quiet: parse_hex_color("#9AA3AF"),
            deep: parse_hex_color("#0F1216"),
        }
    }
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
            digits: shell.familiar_clock.eq_ignore_ascii_case("digital"),
            body_color: parse_hex_color(&shell.familiar_color),
            accent_color: parse_hex_color(&shell.familiar_accent),
            palette: OrbPalette::new(parse_hex_color(&shell.familiar_color)),
        }
    }
}

pub struct OrbFrame {
    pub rect: [f32; 4],
    pub seconds_of_day: f32,
    pub open: f32,
    pub hover: f32,
    pub ambient: f32,
    pub hand: f32,
    pub dial: f32,
    /// The control-centre card in pixels; the orb's own rect while closed.
    pub card: [f32; 4],
    pub card_radius: f32,
    /// Smooth-union distance in pixels between the orb disc and the card.
    pub blend: f32,
    /// A native module in trouble: the amber arc at six.
    pub warn: f32,
    /// Dial angle pointing from the orb toward the screen, where the alert pip sits.
    pub facing: f32,
    pub agent: crate::shell::agent::AgentLook,
    pub crown: crate::shell::crown::CrownLook,
    /// Opacity of the card/banner surface; the banner fades in and out with it.
    pub surface_alpha: f32,
    /// The surface's `box-shadow`: alpha, y offset px, blur px.
    pub shadow: [f32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct OrbUniform {
    pub rect: [f32; 4],
    pub screen: [f32; 4],
    pub body_color: [f32; 4],
    pub accent_color: [f32; 4],
    pub card: [f32; 4],
    pub seconds_of_day: f32,
    pub open: f32,
    pub hover: f32,
    pub ambient: f32,
    pub hand: f32,
    pub dial: f32,
    pub alert: f32,
    pub warn: f32,
    pub shape: [f32; 4],
    pub agent_a: [f32; 4],
    pub agent_b: [f32; 4],
    pub agent_c: [f32; 4],
    pub marks: [[f32; 4]; 3],
    pub ink: [f32; 4],
    pub hand_ink: [f32; 4],
    pub dark: [f32; 4],
    pub amber: [f32; 4],
    pub quiet: [f32; 4],
    pub deep: [f32; 4],
    pub crown: [f32; 4],
    pub crown_art: [f32; 4],
    pub crown_style: [f32; 4],
    pub strip: [f32; 4],
    pub strip_style: [f32; 4],
    pub strip_color: [f32; 4],
    pub strip_art: [f32; 4],
    pub glyphs: [f32; 4],
    pub line: [f32; 4],
    pub surface: [f32; 4],
    pub face_hover: [f32; 4],
    pub face_ambient: [f32; 4],
    pub hand_ambient: [f32; 4],
}

impl OrbUniform {
    pub fn new(frame: &OrbFrame, params: &OrbParams, screen: [f32; 2]) -> Self {
        let a = &frame.agent;
        let palette = &params.palette;
        Self {
            rect: frame.rect,
            screen: [screen[0], screen[1], 0.0, 0.0],
            body_color: params.body_color,
            accent_color: params.accent_color,
            card: frame.card,
            seconds_of_day: frame.seconds_of_day,
            open: frame.open,
            hover: frame.hover,
            ambient: frame.ambient,
            hand: frame.hand,
            dial: frame.dial,
            alert: a.alert,
            warn: frame.warn,
            shape: [frame.card_radius, frame.blend, frame.facing, a.focus],
            agent_a: [a.fold, a.park, a.voice, a.tint],
            agent_b: [a.dim, a.step_alpha, a.step_angle, a.ring_hub],
            agent_c: [a.solid_hub, a.arc, a.progress, a.privacy],
            marks: std::array::from_fn(|i| std::array::from_fn(|j| a.marks[i * 4 + j])),
            ink: palette.ink,
            hand_ink: palette.hand_ink,
            dark: palette.dark,
            amber: palette.amber,
            quiet: palette.quiet,
            deep: palette.deep,
            crown: frame.crown.crown,
            crown_art: frame.crown.crown_art,
            crown_style: frame.crown.crown_style,
            strip: frame.crown.strip,
            strip_style: frame.crown.strip_style,
            strip_color: frame.crown.strip_color,
            strip_art: frame.crown.strip_art,
            glyphs: frame.crown.glyphs,
            line: frame.crown.line,
            surface: [frame.surface_alpha, frame.shadow[0], frame.shadow[1], frame.shadow[2]],
            face_hover: palette.face_hover,
            face_ambient: palette.face_ambient,
            hand_ambient: palette.hand_ambient,
        }
    }
}

pub struct OrbPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl OrbPipeline {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/orb.wgsl"));

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("orb uniform bind group layout"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("orb art sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank = art_texture(device, queue, 1, 1, &[0, 0, 0, 0]);
        let uniform_bind_group =
            bind_group(device, &uniform_bind_group_layout, &uniform_buffer, &blank, &sampler);

        Self {
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            layout: uniform_bind_group_layout,
            sampler,
        }
    }

    /// Replaces the album art the crown and strip sample; `None` leaves an empty slot.
    pub fn set_art(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, art: Option<(u32, u32, &[u8])>) {
        let texture = match art {
            Some((w, h, bgra)) if w > 0 && h > 0 && bgra.len() >= (w * h * 4) as usize => {
                art_texture(device, queue, w, h, bgra)
            }
            _ => art_texture(device, queue, 1, 1, &[0, 0, 0, 0]),
        };
        self.uniform_bind_group =
            bind_group(device, &self.layout, &self.uniform_buffer, &texture, &self.sampler);
    }
}

fn art_texture(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, bgra: &[u8]) -> wgpu::Texture {
    let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("orb art"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &bgra[..(width * height * 4) as usize],
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
        size,
    );
    texture
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    art: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let view = art.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("orb bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}
