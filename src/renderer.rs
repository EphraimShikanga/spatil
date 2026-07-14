pub mod dmabuf;
pub mod grid;
pub mod particles;
pub mod pipeline;
pub mod scene_params;
pub mod shadow;
pub mod texture;

use std::collections::HashMap;
use std::sync::Arc;

use cgmath::{InnerSpace, Matrix4, Point3, SquareMatrix, Vector3};
use wgpu::{Device, Queue};
use winit::window::Window;

use smithay::backend::allocator::Modifier;
use smithay::reexports::wayland_server::backend::ObjectId;

use crate::config::{Display, SceneGraph, SpatilConfig};
use crate::scene::animation::SceneTransition;
use crate::scene::components::{Transform, WaylandSurface};
use crate::scene::mode::{self, AetherView, SceneMode};
use crate::scene::systems::update_world_matrices;

use grid::{GridPipeline, GridUniform};
use particles::{ParticlePipeline, ParticleUniform};
use pipeline::{Camera, OPENGL_TO_WGPU_MATRIX, QuadPipeline, QuadUniform};
use scene_params::{AetherParams, StrataParams};
use shadow::{LightUniform, ShadowMap, ShadowPipeline};
use texture::TexturedQuad;

pub use dmabuf::{query_dmabuf_modifiers, query_render_modifiers};
pub(crate) use dmabuf::query_render_node_dev_t;

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

fn light_view_projection(light_position: [f32; 3], floor_half: [f32; 2]) -> Matrix4<f32> {
    let eye = Point3::new(light_position[0], light_position[1], light_position[2]);
    let target = Point3::new(0.0, 0.0, 0.0);
    let view = Matrix4::look_at_rh(eye, target, Vector3::unit_y());
    let radius = floor_half[0].max(floor_half[1]) + 3.0;
    let proj = cgmath::ortho(-radius, radius, -radius, radius, 0.1, 30.0);
    OPENGL_TO_WGPU_MATRIX * proj * view
}

fn create_depth_view(device: &Device, width: u32, height: u32) -> wgpu::TextureView {
    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Depth Texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    depth_texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Vulkan-only: zero-copy dmabuf import (`texture_from_dmabuf_fd`) exists only on the
/// wgpu Vulkan HAL.
pub(crate) fn new_vulkan_instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
}

pub(crate) async fn request_device(adapter: &wgpu::Adapter) -> (Device, Queue) {
    let required_features = if adapter
        .features()
        .contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
    {
        wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF
    } else {
        tracing::warn!(
            "adapter does not support VULKAN_EXTERNAL_MEMORY_DMA_BUF -- dmabuf import will be unavailable"
        );
        wgpu::Features::empty()
    };
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features,
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            ..Default::default()
        })
        .await
        .unwrap()
}

pub struct WgpuState {
    pub device: Device,
    pub queue: Queue,
    pub width: u32,
    pub height: u32,
    pub depth_texture_view: wgpu::TextureView,
    pub quad_pipeline: QuadPipeline,
    pub grid_pipeline: GridPipeline,
    pub particle_pipeline: ParticlePipeline,
    pub particle_instances: wgpu::Buffer,
    pub shadow_pipeline: ShadowPipeline,
    pub shadow_map: ShadowMap,
    pub camera: Camera,
    pub scene_mode: SceneMode,
    pub scene_transition: SceneTransition,
    pub pointer_ndc: [f32; 2],
    pub strata: StrataParams,
    pub aether: AetherParams,
    pub quads: Vec<(ObjectId, TexturedQuad)>,
    pub dmabuf_textures: HashMap<ObjectId, (wgpu::Texture, u32, u32, [f32; 2])>,
    pub main_device_id: Option<u64>,
    pub dmabuf_modifiers: Vec<Modifier>,
}

impl WgpuState {
    /// Winit backend: builds the instance + swapchain surface and hands the
    /// configured `wgpu::Surface` back so the backend owns acquire/present.
    pub async fn new_winit(
        window: Arc<Window>,
        config: &SpatilConfig,
    ) -> (Self, wgpu::Surface<'static>, wgpu::SurfaceConfiguration) {
        let size = window.inner_size();
        let instance = new_vulkan_instance();
        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .expect(
                "no Vulkan adapter available -- hardware-accelerated dmabuf import requires Vulkan",
            );

        let (device, queue) = request_device(&adapter).await;

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let state = Self::from_device(
            device,
            queue,
            &adapter,
            surface_format,
            surface_config.width,
            surface_config.height,
            config,
        );
        (state, surface, surface_config)
    }

    /// Shared render-state construction from an already-created device/queue
    /// (winit swapchain or headless DRM adapter).
    pub(crate) fn from_device(
        device: Device,
        queue: Queue,
        adapter: &wgpu::Adapter,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        config: &SpatilConfig,
    ) -> Self {
        let depth_texture_view = create_depth_view(&device, width, height);

        let scene = &config.scene_graph;
        let scene_mode = scene.scene_mode();
        let strata = StrataParams::from_config(&scene.strata, &config.display);
        let aether = AetherParams::from_config(&scene.aether);

        let quad_pipeline = QuadPipeline::new(&device, format);
        let shadow_pipeline = ShadowPipeline::new(&device);
        let shadow_map = ShadowMap::new(&device, scene.strata.shadow_map_size);
        let grid_pipeline = GridPipeline::new(&device, format, &shadow_map);
        let particle_pipeline = ParticlePipeline::new(&device, format);
        let particle_positions = particles::generate_particles(
            aether.particle_count,
            aether.field[0],
            aether.field[1],
            aether.field[2],
        );
        let particle_instances = particles::instance_buffer(&device, &particle_positions);
        let mut camera = Camera::new(width as f32 / height as f32);
        mode::frame_camera(scene_mode, &mut camera, &strata.camera_framing());

        let main_device_id = query_render_node_dev_t(adapter);
        let dmabuf_modifiers = query_dmabuf_modifiers(adapter);
        match main_device_id {
            Some(dev) => tracing::info!(
                "adapter DRM render node: dev_t {}:{}",
                (dev >> 8) & 0xfff,
                dev & 0xff | ((dev >> 12) & !0xff)
            ),
            None => tracing::warn!(
                "could not query adapter's DRM render node (VK_EXT_physical_device_drm); \
                 dmabuf v4 feedback unavailable, falling back to v3"
            ),
        }

        Self {
            device,
            queue,
            width,
            height,
            depth_texture_view,
            quad_pipeline,
            grid_pipeline,
            particle_pipeline,
            particle_instances,
            shadow_pipeline,
            shadow_map,
            camera,
            scene_mode,
            scene_transition: SceneTransition::idle(),
            pointer_ndc: [0.0, 0.0],
            strata,
            aether,
            quads: Vec::new(),
            dmabuf_textures: HashMap::new(),
            main_device_id,
            dmabuf_modifiers,
        }
    }

    pub fn apply_scene_config(&mut self, scene: &SceneGraph, display: &Display) {
        self.scene_mode = scene.scene_mode();
        self.strata = StrataParams::from_config(&scene.strata, display);

        // Regenerate the point cloud only when its shape changed, so unrelated
        // edits don't reshuffle the stars.
        let aether = AetherParams::from_config(&scene.aether);
        if self.aether.particle_count != aether.particle_count || self.aether.field != aether.field {
            let positions = particles::generate_particles(
                aether.particle_count,
                aether.field[0],
                aether.field[1],
                aether.field[2],
            );
            self.particle_instances = particles::instance_buffer(&self.device, &positions);
        }
        self.aether = aether;

        mode::frame_camera(self.scene_mode, &mut self.camera, &self.strata.camera_framing());
    }

    /// Resizes the render targets (winit resize / DRM mode-set): reallocates depth
    /// and updates the projection aspect. The backend reconfigures its own swapchain.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.width = width;
            self.height = height;
            self.depth_texture_view = create_depth_view(&self.device, width, height);
            self.camera.set_aspect(width as f32 / height as f32);
        }
    }

    pub fn upload_shm_texture(
        &mut self,
        id: ObjectId,
        width: u32,
        height: u32,
        stride: u32,
        bytes: &[u8],
    ) {
        let quad = TexturedQuad::from_bytes(
            &self.device,
            &self.queue,
            &self.quad_pipeline.bind_group_layout,
            &self.shadow_pipeline.quad_bind_group_layout,
            width,
            height,
            stride,
            bytes,
        );
        self.set_quad(id, quad);
    }

    /// Attaches a previously imported dmabuf texture (keyed by `wl_buffer` id) to a
    /// surface's quad. Returns `false` if no imported texture exists for that buffer.
    pub fn attach_dmabuf_texture(&mut self, surface_id: ObjectId, buffer_id: &ObjectId) -> bool {
        let Some((texture, width, height, uv_scale)) = self.dmabuf_textures.get(buffer_id) else {
            return false;
        };
        let quad = TexturedQuad::from_wgpu_texture(
            &self.device,
            &self.quad_pipeline.bind_group_layout,
            &self.shadow_pipeline.quad_bind_group_layout,
            texture.clone(),
            *width,
            *height,
            *uv_scale,
        );
        self.set_quad(surface_id, quad);
        true
    }

    pub fn remove_dmabuf_texture(&mut self, buffer_id: &ObjectId) {
        self.dmabuf_textures.remove(buffer_id);
    }

    pub fn remove_texture(&mut self, id: &ObjectId) {
        self.quads.retain(|(qid, _)| qid != id);
    }

    fn set_quad(&mut self, id: ObjectId, quad: TexturedQuad) {
        if let Some(existing) = self.quads.iter_mut().find(|(qid, _)| *qid == id) {
            existing.1 = quad;
        } else {
            self.quads.push((id, quad));
        }
    }

    fn frame_for(&self, camera: &mut Camera, mode: SceneMode) {
        match mode {
            SceneMode::Strata => {
                mode::frame_camera(SceneMode::Strata, camera, &self.strata.camera_framing())
            }
            SceneMode::Aether => mode::frame_aether(
                camera,
                &AetherView {
                    distance: self.aether.camera_distance,
                    pointer_ndc: self.pointer_ndc,
                    parallax_strength: self.aether.parallax(),
                },
            ),
        }
    }

    fn scene_presence(&self) -> f32 {
        let base = |m: SceneMode| if matches!(m, SceneMode::Aether) { 1.0 } else { 0.0 };
        if self.scene_transition.active {
            let a = base(self.scene_transition.from);
            let b = base(self.scene_transition.to);
            a + (b - a) * self.scene_transition.eased()
        } else {
            base(self.scene_mode)
        }
    }

    /// Draws one frame into `view` (a winit swapchain image or a DRM scanout texture)
    /// and submits it. The backend owns acquire/present; this never touches a `Surface`.
    pub fn render(&mut self, world: &mut hecs::World, view: &wgpu::TextureView) {
        update_world_matrices(world);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        let presence = self.scene_presence();
        let field_alpha = presence;
        let grid_alpha = 1.0 - presence;
        let draw_particles = field_alpha > 0.001;
        let draw_grid = grid_alpha > 0.001;

        if self.scene_transition.active {
            let p = self.scene_transition.eased();
            let (from, to) = (self.scene_transition.from, self.scene_transition.to);
            let mut c0 = self.camera;
            self.frame_for(&mut c0, from);
            let mut c1 = self.camera;
            self.frame_for(&mut c1, to);
            self.camera.eye = c0.eye + (c1.eye - c0.eye) * p;
            self.camera.target = c0.target + (c1.target - c0.target) * p;
            self.camera.up = Vector3::unit_y();
        } else {
            let mut c = self.camera;
            self.frame_for(&mut c, self.scene_mode);
            self.camera = c;
        }

        let view_proj = self.camera.build_view_projection_matrix();
        let inv_view_proj = self
            .camera
            .build_inverse_view_projection_matrix()
            .unwrap_or_else(|| Matrix4::identity());
        let light_view_proj = light_view_projection(self.strata.light_position, self.strata.floor_half);

        let clear = lerp4(self.strata.void_color, self.aether.void_color, presence);
        let window_alpha = 1.0 + (self.aether.glass_alpha - 1.0) * presence;
        let window_tint = lerp4([0.0; 4], self.aether.glass_tint, presence);

        // Each quad writes its own uniform buffer so both the shadow and color
        // passes read final values without a mid-pass rewrite.
        let mut draw_list: Vec<(usize, f32)> = Vec::new();
        for (_id, (transform, wayland_surface)) in
            world.query::<(&Transform, &WaylandSurface)>().iter()
        {
            if !wayland_surface.visible {
                continue;
            }
            if let Some(idx) = self
                .quads
                .iter()
                .position(|(qid, _)| *qid == wayland_surface.id)
            {
                let model = transform.world_matrix;
                let quad = &self.quads[idx].1;
                self.queue.write_buffer(
                    &quad.uniform_buffer,
                    0,
                    bytemuck::cast_slice(&[QuadUniform {
                        mvp: (view_proj * model).into(),
                        model: model.into(),
                        uv_scale: quad.uv_scale,
                        alpha: window_alpha * transform.opacity,
                        _pad: 0.0,
                        tint: window_tint,
                    }]),
                );
                draw_list.push((idx, model.w.z));
            }
        }
        // Back-to-front so translucent windows composite correctly.
        if presence > 0.001 {
            draw_list.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        }

        // Runs even with no windows, so the map is cleared to 1.0 (unoccluded)
        // every frame rather than sampled stale by the grid.
        if draw_grid && self.strata.enable_shadows {
            self.queue.write_buffer(
                &self.shadow_pipeline.light_uniform_buffer,
                0,
                bytemuck::cast_slice(&[LightUniform {
                    view_proj: light_view_proj.into(),
                }]),
            );

            let mut shadow_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Shadow Pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_map.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            shadow_pass.set_pipeline(&self.shadow_pipeline.pipeline);
            shadow_pass.set_bind_group(0, &self.shadow_pipeline.light_bind_group, &[]);
            shadow_pass.set_vertex_buffer(0, self.quad_pipeline.vertex_buffer.slice(..));
            shadow_pass.set_index_buffer(
                self.quad_pipeline.index_buffer.slice(..),
                wgpu::IndexFormat::Uint16,
            );
            for &(idx, _) in &draw_list {
                let quad = &self.quads[idx].1;
                shadow_pass.set_bind_group(1, &quad.shadow_bind_group, &[]);
                shadow_pass.draw_indexed(0..self.quad_pipeline.num_indices, 0, 0..1);
            }
        }

        if draw_grid {
            self.queue.write_buffer(
                &self.grid_pipeline.uniform_buffer,
                0,
                bytemuck::cast_slice(&[GridUniform {
                    inv_view_proj: inv_view_proj.into(),
                    light_view_proj: light_view_proj.into(),
                    camera_eye: [self.camera.eye.x, self.camera.eye.y, self.camera.eye.z, 1.0],
                    grid_color: self.strata.grid_color,
                    void_color: self.strata.void_color,
                    axis_color: self.strata.axis_color,
                    grid_extent: self.strata.grid_extent,
                    grid_level: self.strata.grid_level,
                    grid_spacing: self.strata.grid_spacing,
                    fog_density: self.strata.fog_density,
                    enable_shadows: if self.strata.enable_shadows { 1.0 } else { 0.0 },
                    opacity: grid_alpha,
                    _pad: [0.0; 2],
                }]),
            );
        }

        if draw_particles {
            let forward = (self.camera.target - self.camera.eye).normalize();
            let right = forward.cross(self.camera.up).normalize();
            let cam_up = right.cross(forward);
            self.queue.write_buffer(
                &self.particle_pipeline.uniform_buffer,
                0,
                bytemuck::cast_slice(&[ParticleUniform {
                    view_proj: view_proj.into(),
                    camera_right: [right.x, right.y, right.z, 0.0],
                    camera_up: [cam_up.x, cam_up.y, cam_up.z, 0.0],
                    camera_eye: [self.camera.eye.x, self.camera.eye.y, self.camera.eye.z, 1.0],
                    bg_color: self.aether.void_color,
                    color: self.aether.particle_color,
                    size: self.aether.particle_size,
                    fog_density: self.aether.fog_density,
                    field_alpha,
                    _pad: 0.0,
                }]),
            );
        }

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64,
                            g: clear[1] as f64,
                            b: clear[2] as f64,
                            a: clear[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            // Backdrop first, windows, then the floor last (fills where no window drew).
            if draw_particles {
                render_pass.set_pipeline(&self.particle_pipeline.pipeline);
                render_pass.set_bind_group(0, &self.particle_pipeline.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.particle_instances.slice(..));
                render_pass.draw(0..6, 0..self.aether.particle_count);
            }

            render_pass.set_pipeline(&self.quad_pipeline.pipeline);
            render_pass.set_vertex_buffer(0, self.quad_pipeline.vertex_buffer.slice(..));
            render_pass.set_index_buffer(
                self.quad_pipeline.index_buffer.slice(..),
                wgpu::IndexFormat::Uint16,
            );
            for &(idx, _) in &draw_list {
                let quad = &self.quads[idx].1;
                render_pass.set_bind_group(0, &quad.bind_group, &[]);
                render_pass.draw_indexed(0..self.quad_pipeline.num_indices, 0, 0..1);
            }

            if draw_grid {
                render_pass.set_pipeline(&self.grid_pipeline.pipeline);
                render_pass.set_bind_group(0, &self.grid_pipeline.uniform_bind_group, &[]);
                render_pass.set_bind_group(1, &self.grid_pipeline.shadow_bind_group, &[]);
                render_pass.draw(0..3, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod shader_tests {
    // GPU-free WGSL validation via naga, the same front-end wgpu uses at
    // pipeline creation, so a shader error fails tests instead of the compositor.
    fn validate(name: &str, src: &str) {
        let module = naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|e| panic!("{name}: WGSL parse error:\n{}", e.emit_to_string(src)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: WGSL validation error: {e:?}"));
    }

    #[test]
    fn shaders_are_valid() {
        validate("quad.wgsl", include_str!("shaders/quad.wgsl"));
        validate("grid.wgsl", include_str!("shaders/grid.wgsl"));
        validate("particles.wgsl", include_str!("shaders/particles.wgsl"));
        validate("shadow.wgsl", include_str!("shaders/shadow.wgsl"));
    }
}
