pub mod backdrop;
pub mod cursor;
pub mod dmabuf;
pub mod gauge;
pub mod grid;
pub mod hud;
pub mod orb;
pub mod panel;
pub mod particles;
pub mod pipeline;
pub mod scene_params;
pub mod shadow;
pub mod text;
pub mod texture;

use std::collections::HashMap;
use std::sync::Arc;

use cgmath::{EuclideanSpace, InnerSpace, Matrix4, Point3, SquareMatrix, Vector3};
use wgpu::{Device, Queue};
use winit::window::Window;

use smithay::backend::allocator::Modifier;
use smithay::reexports::wayland_server::backend::ObjectId;

use crate::config::{Display, Input, SceneGraph, Shell, SpatilConfig};
use crate::scene::animation::SceneTransition;
use crate::scene::camera_controller::CameraController;
use crate::scene::components::{Popup, Presentation, Transform, WaylandSurface, WindowState};
use crate::scene::mode::{self, AetherView, SceneMode};
use crate::scene::systems::update_world_matrices;

use backdrop::{BackdropPipeline, BackdropUniform};
use gauge::{GaugeInstance, GaugePipeline, GaugeUniform};
use grid::{GridPipeline, GridUniform};
use cursor::{CursorFrame, CursorPipeline, CursorUniform};
use hud::{HudPipeline, HudQuad, TrayQuad, hud_mvp};
use orb::{OrbFrame, OrbParams, OrbPipeline, OrbUniform};
use panel::{PanelInstance, PanelPipeline, PanelUniform};
use particles::{ParticlePipeline, ParticleUniform};
use pipeline::{Camera, OPENGL_TO_WGPU_MATRIX, QuadPipeline, QuadUniform};
use scene_params::{AetherParams, InteractionParams, StrataParams};
use shadow::{LightUniform, ShadowMap, ShadowPipeline};
use text::{GlyphInstance, TextPipeline, TextRun, TextUniform};
use texture::TexturedQuad;

pub use dmabuf::{query_dmabuf_modifiers, query_render_modifiers};
pub(crate) use dmabuf::query_render_node_dev_t;

const POPUP_SORT_EPSILON: f32 = 1e-3;

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
    pub backdrop_pipeline: BackdropPipeline,
    pub hud_pipeline: HudPipeline,
    /// Layer-shell surfaces in screen pixels, back-to-front.
    pub hud_layout: Vec<HudQuad>,
    pub orb_pipeline: OrbPipeline,
    pub orb: OrbParams,
    pub orb_frame: Option<OrbFrame>,
    pub cursor_pipeline: CursorPipeline,
    pub cursor_frame: Option<CursorFrame>,
    pub tray_icons: HashMap<String, TexturedQuad>,
    pub tray_layout: Vec<TrayQuad>,
    pub gauge_pipeline: GaugePipeline,
    pub gauges: Vec<GaugeInstance>,
    pub panel_pipeline: PanelPipeline,
    pub panels: Vec<PanelInstance>,
    pub text_pipeline: TextPipeline,
    pub text_runs: Vec<TextRun>,
    pub font: String,
    text_glyphs: Vec<GlyphInstance>,
    text_mask_count: u32,
    pub camera: Camera,
    pub camera_controller: CameraController,
    pub scene_mode: SceneMode,
    pub scene_transition: SceneTransition,
    pub pointer_ndc: [f32; 2],
    pub parallax_frozen: bool,
    pub backdrop_dim: f32,
    pub focused_surface: Option<ObjectId>,
    pub strata: StrataParams,
    pub aether: AetherParams,
    pub interaction: InteractionParams,
    pub quads: Vec<(ObjectId, TexturedQuad)>,
    pub dmabuf_textures: HashMap<ObjectId, (wgpu::Texture, u32, u32, [f32; 2], bool)>,
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
        let interaction = InteractionParams::from_config(&config.input);

        let quad_pipeline = QuadPipeline::new(&device, format);
        let shadow_pipeline = ShadowPipeline::new(&device);
        let shadow_map = ShadowMap::new(&device, scene.strata.shadow_map_size);
        let grid_pipeline = GridPipeline::new(&device, format, &shadow_map);
        let particle_pipeline = ParticlePipeline::new(&device, format);
        let backdrop_pipeline = BackdropPipeline::new(&device, format);
        let hud_pipeline = HudPipeline::new(&device, format, &quad_pipeline.bind_group_layout);
        let orb_pipeline = OrbPipeline::new(&device, format);
        let cursor_pipeline = CursorPipeline::new(&device, format);
        let gauge_pipeline = GaugePipeline::new(&device, format);
        let text_pipeline = TextPipeline::new(&device, format);
        let panel_pipeline = PanelPipeline::new(&device, format);
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
            backdrop_pipeline,
            hud_pipeline,
            hud_layout: Vec::new(),
            orb_pipeline,
            orb: OrbParams::from_config(&config.shell),
            orb_frame: None,
            cursor_pipeline,
            cursor_frame: None,
            tray_icons: HashMap::new(),
            tray_layout: Vec::new(),
            gauge_pipeline,
            gauges: Vec::new(),
            panel_pipeline,
            panels: Vec::new(),
            text_pipeline,
            text_runs: Vec::new(),
            font: config.shell.font.clone(),
            text_glyphs: Vec::new(),
            text_mask_count: 0,
            camera,
            camera_controller: CameraController::default(),
            scene_mode,
            scene_transition: SceneTransition::idle(),
            pointer_ndc: [0.0, 0.0],
            parallax_frozen: false,
            backdrop_dim: 0.0,
            focused_surface: None,
            strata,
            aether,
            interaction,
            quads: Vec::new(),
            dmabuf_textures: HashMap::new(),
            main_device_id,
            dmabuf_modifiers,
        }
    }

    pub fn apply_scene_config(
        &mut self,
        scene: &SceneGraph,
        display: &Display,
        input: &Input,
        shell: &Shell,
    ) {
        self.scene_mode = scene.scene_mode();
        self.strata = StrataParams::from_config(&scene.strata, display);
        self.interaction = InteractionParams::from_config(input);
        self.orb = OrbParams::from_config(shell);

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

        let mut camera = self.camera;
        self.frame_for(&mut camera, self.scene_mode);
        self.camera = camera;
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
        has_alpha: bool,
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
            has_alpha,
        );
        self.set_quad(id, quad);
    }

    /// Attaches a previously imported dmabuf texture (keyed by `wl_buffer` id) to a
    /// surface's quad. Returns `false` if no imported texture exists for that buffer.
    pub fn attach_dmabuf_texture(&mut self, surface_id: ObjectId, buffer_id: &ObjectId) -> bool {
        let Some((texture, width, height, uv_scale, has_alpha)) =
            self.dmabuf_textures.get(buffer_id)
        else {
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
            *has_alpha,
        );
        self.set_quad(surface_id, quad);
        true
    }

    pub fn remove_dmabuf_texture(&mut self, buffer_id: &ObjectId) {
        self.dmabuf_textures.remove(buffer_id);
    }

    fn build_text_glyphs(&mut self) {
        self.text_glyphs.clear();
        self.text_mask_count = 0;
        if self.text_runs.is_empty() || !crate::text::fonts_ready() {
            return;
        }
        let runs = std::mem::take(&mut self.text_runs);
        for attempt in 0..2 {
            let resets = self.text_pipeline.atlas.resets();
            self.collect_glyphs(&runs);
            // A reset mid-build repacks the atlas under UVs already collected,
            // so the whole run has to be rebuilt against the cleared atlas.
            if self.text_pipeline.atlas.resets() == resets {
                break;
            }
            if attempt == 1 {
                tracing::warn!("text exceeds the glyph atlas in a single frame; dropping it");
                self.text_glyphs.clear();
                self.text_mask_count = 0;
            }
        }
        self.text_runs = runs;
    }

    fn collect_glyphs(&mut self, runs: &[TextRun]) {
        let font = std::mem::take(&mut self.font);
        let mut mask: Vec<GlyphInstance> = Vec::new();
        let mut colored: Vec<GlyphInstance> = Vec::new();
        for run in runs {
            if run.text.is_empty() || run.color[3] <= 0.0 {
                continue;
            }
            let shaped = crate::text::shape(&run.text, &font, run.size);
            let origin_x = run.x.round();
            let baseline = (run.y + shaped.ascent).round();
            for glyph in &shaped.glyphs {
                let Some(entry) = self.atlas_entry(glyph.key) else {
                    continue;
                };
                let instance = GlyphInstance {
                    rect: [
                        origin_x + glyph.x as f32 + entry.left as f32,
                        baseline + glyph.y as f32 - entry.top as f32,
                        entry.width as f32,
                        entry.height as f32,
                    ],
                    uv: entry.uv,
                    color: if entry.color {
                        [1.0, 1.0, 1.0, run.color[3]]
                    } else {
                        run.color
                    },
                };
                if entry.color {
                    colored.push(instance);
                } else {
                    mask.push(instance);
                }
            }
        }
        self.font = font;
        self.text_mask_count = mask.len() as u32;
        self.text_glyphs = mask;
        self.text_glyphs.extend(colored);
    }

    fn atlas_entry(&mut self, key: cosmic_text::CacheKey) -> Option<text::AtlasEntry> {
        if let Some(entry) = self.text_pipeline.atlas.get(&key) {
            return Some(*entry);
        }
        let image = crate::text::rasterize(key)?;
        self.text_pipeline.insert(&self.queue, key, &image);
        self.text_pipeline.atlas.get(&key).copied()
    }

    pub fn text_report(&self) -> (usize, usize, u64, bool) {
        (
            self.text_glyphs.len(),
            self.text_pipeline.atlas.len(),
            self.text_pipeline.atlas.resets(),
            crate::text::fonts_ready(),
        )
    }

    pub fn upload_tray_icon(&mut self, key: String, width: u32, height: u32, bgra: &[u8]) {
        let quad = TexturedQuad::from_bytes(
            &self.device,
            &self.queue,
            &self.quad_pipeline.bind_group_layout,
            &self.shadow_pipeline.quad_bind_group_layout,
            width,
            height,
            width * 4,
            bgra,
            true,
        );
        self.tray_icons.insert(key, quad);
    }

    pub fn remove_tray_icon(&mut self, key: &str) {
        self.tray_icons.remove(key);
    }

    pub fn has_tray_icon(&self, key: &str) -> bool {
        self.tray_icons.contains_key(key)
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
        let parallax = if self.parallax_frozen { 0.0 } else { self.aether.parallax() };
        self.frame_with(camera, mode, parallax);
    }

    fn frame_with(&self, camera: &mut Camera, mode: SceneMode, parallax: f32) {
        let (yaw, pitch, pan, zoom) = self.camera_controller.smoothed();
        match mode {
            SceneMode::Strata => {
                let mut framing = self.strata.camera_framing();
                framing.distance *= zoom;
                framing.orbit += yaw;
                framing.elevation += pitch;
                framing.target = pan;
                mode::frame_camera(SceneMode::Strata, camera, &framing)
            }
            SceneMode::Aether => mode::frame_aether(
                camera,
                &AetherView {
                    distance: self.aether.camera_distance * zoom,
                    pointer_ndc: self.pointer_ndc,
                    parallax_strength: parallax,
                    target: pan,
                    orbit: yaw,
                    elevation: pitch,
                },
            ),
        }
    }

    fn settled_camera(&self, mode: SceneMode) -> Camera {
        let mut camera = self.camera;
        self.frame_with(&mut camera, mode, 0.0);
        camera
    }

    pub fn facing_rotation(&self, mode: SceneMode) -> cgmath::Quaternion<f32> {
        let camera = self.settled_camera(mode);
        mode::billboard_rotation(camera.eye, camera.target)
    }

    pub fn fit_placement(&self, mode: SceneMode, height: f32, fill: f32) -> Vector3<f32> {
        let camera = self.settled_camera(mode);
        let distance = mode::fit_distance(camera.fovy, height, fill);
        camera.eye.to_vec() + camera.forward() * distance
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
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        let presence = self.scene_presence();

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

        // After the camera, because billboards are turned to face it.
        update_world_matrices(world, &self.camera);

        // A fullscreen window is the whole picture: the world behind is not dimmed,
        // it is not drawn.
        let raised: HashMap<hecs::Entity, WindowState> = world
            .query::<(&Presentation, &WaylandSurface)>()
            .iter()
            .filter(|(_, (p, s))| p.state != WindowState::Ambient && s.visible)
            .map(|(entity, (p, _))| (entity, p.state))
            .collect();
        let world_hidden = raised.values().any(|s| *s == WindowState::Fullscreen);
        let depth_key: HashMap<hecs::Entity, f32> = world
            .query::<&Transform>()
            .iter()
            .map(|(entity, transform)| (entity, transform.world_matrix.w.z))
            .collect();
        let field_alpha = if world_hidden { 0.0 } else { presence };
        let grid_alpha = if world_hidden { 0.0 } else { 1.0 - presence };
        let draw_particles = field_alpha > 0.001;
        let draw_grid = grid_alpha > 0.001;
        let dim = self.backdrop_dim * self.interaction.backdrop_dim_color[3];
        let draw_backdrop = dim > 0.001 && !world_hidden;

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
        let mut modal_list: Vec<(usize, f32)> = Vec::new();
        for (entity, (transform, wayland_surface, presentation, popup)) in world
            .query::<(
                &Transform,
                &WaylandSurface,
                Option<&Presentation>,
                Option<&Popup>,
            )>()
            .iter()
        {
            if !wayland_surface.visible {
                continue;
            }
            let state = match popup {
                Some(popup) => raised.get(&popup.root).copied().unwrap_or(WindowState::Ambient),
                None => presentation.map_or(WindowState::Ambient, |p| p.state),
            };
            let modal = state != WindowState::Ambient;
            if world_hidden && !modal {
                continue;
            }
            // Aether's glass would let the clear colour bleed through the pane.
            let alpha = if state == WindowState::Fullscreen { 1.0 } else { window_alpha };
            if let Some(idx) = self
                .quads
                .iter()
                .position(|(qid, _)| *qid == wayland_surface.id)
            {
                let model = transform.world_matrix;
                let focused = self
                    .focused_surface
                    .as_ref()
                    .is_some_and(|id| *id == wayland_surface.id);
                let quad = &self.quads[idx].1;
                self.queue.write_buffer(
                    &quad.uniform_buffer,
                    0,
                    bytemuck::cast_slice(&[QuadUniform {
                        mvp: (view_proj * model).into(),
                        model: model.into(),
                        uv_scale: quad.uv_scale,
                        alpha: alpha * transform.opacity,
                        border_width: if focused {
                            self.interaction.focus_border_width
                        } else {
                            0.0
                        },
                        tint: window_tint,
                        border_color: self.interaction.focus_border_color,
                        surface_alpha: if quad.has_alpha { 1.0 } else { 0.0 },
                        _pad: [0.0; 3],
                    }]),
                );
                let key = match popup {
                    Some(popup) => {
                        depth_key.get(&popup.root).copied().unwrap_or(model.w.z)
                            + (popup.depth + 1) as f32 * POPUP_SORT_EPSILON
                    }
                    None => depth_key.get(&entity).copied().unwrap_or(model.w.z),
                };
                if modal {
                    modal_list.push((idx, key));
                } else {
                    draw_list.push((idx, key));
                }
            }
        }
        let back_to_front =
            |a: &(usize, f32), b: &(usize, f32)| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal);
        draw_list.sort_by(back_to_front);
        modal_list.sort_by(back_to_front);

        let (screen_w, screen_h) = (self.width.max(1) as f32, self.height.max(1) as f32);
        self.build_text_glyphs();
        let mut hud_list: Vec<usize> = Vec::new();
        for entry in &self.hud_layout {
            let Some(idx) = self.quads.iter().position(|(qid, _)| *qid == entry.id) else {
                continue;
            };
            let quad = &self.quads[idx].1;
            let mvp = hud_mvp(entry.rect, screen_w, screen_h);
            self.queue.write_buffer(
                &quad.uniform_buffer,
                0,
                bytemuck::cast_slice(&[QuadUniform {
                    mvp: mvp.into(),
                    model: mvp.into(),
                    uv_scale: quad.uv_scale,
                    alpha: 1.0,
                    border_width: 0.0,
                    tint: [0.0; 4],
                    border_color: [0.0; 4],
                    surface_alpha: if quad.has_alpha { 1.0 } else { 0.0 },
                    _pad: [0.0; 3],
                }]),
            );
            hud_list.push(idx);
        }

        if let Some(frame) = &self.orb_frame {
            self.queue.write_buffer(
                &self.orb_pipeline.uniform_buffer,
                0,
                bytemuck::cast_slice(&[OrbUniform {
                    rect: frame.rect,
                    screen: [screen_w, screen_h, 0.0, 0.0],
                    body_color: self.orb.body_color,
                    accent_color: self.orb.accent_color,
                    seconds_of_day: frame.seconds_of_day,
                    open: frame.open,
                    hover: frame.hover,
                    ambient: frame.ambient,
                    hand: frame.hand,
                    dial: frame.dial,
                    _pad: [0.0; 2],
                }]),
            );
        }

        let mut tray_list: Vec<(&TexturedQuad, f32)> = Vec::new();
        for entry in &self.tray_layout {
            let Some(quad) = self.tray_icons.get(&entry.key) else {
                continue;
            };
            let mvp = hud_mvp(entry.rect, screen_w, screen_h);
            self.queue.write_buffer(
                &quad.uniform_buffer,
                0,
                bytemuck::cast_slice(&[QuadUniform {
                    mvp: mvp.into(),
                    model: mvp.into(),
                    uv_scale: quad.uv_scale,
                    alpha: entry.alpha,
                    border_width: 0.0,
                    tint: [0.0; 4],
                    border_color: [0.0; 4],
                    surface_alpha: 1.0,
                    _pad: [0.0; 3],
                }]),
            );
            tray_list.push((quad, entry.alpha));
        }

        if !self.gauges.is_empty() {
            let accent = self.orb.accent_color;
            self.queue.write_buffer(
                &self.gauge_pipeline.uniform_buffer,
                0,
                bytemuck::cast_slice(&[GaugeUniform {
                    screen: [screen_w, screen_h, 0.0, 0.0],
                    track: [accent[0], accent[1], accent[2], 0.18],
                    fill: accent,
                    urgent: [0.85, 0.24, 0.24, 1.0],
                }]),
            );
            let gauges = std::mem::take(&mut self.gauges);
            self.gauge_pipeline.upload(&self.device, &self.queue, &gauges);
            self.gauges = gauges;
        }

        if !self.panels.is_empty() {
            let accent = self.orb.accent_color;
            let body = self.orb.body_color;
            self.queue.write_buffer(
                &self.panel_pipeline.uniform_buffer,
                0,
                bytemuck::cast_slice(&[PanelUniform {
                    screen: [screen_w, screen_h, 0.0, 0.0],
                    fill: [body[0], body[1], body[2], 0.96],
                    border: [accent[0], accent[1], accent[2], 0.55],
                    accent: [accent[0], accent[1], accent[2], 0.30],
                }]),
            );
            let panels = std::mem::take(&mut self.panels);
            self.panel_pipeline.upload(&self.device, &self.queue, &panels);
            self.panels = panels;
        }

        if !self.text_glyphs.is_empty() {
            self.queue.write_buffer(
                &self.text_pipeline.uniform_buffer,
                0,
                bytemuck::cast_slice(&[TextUniform {
                    screen: [screen_w, screen_h, 0.0, 0.0],
                }]),
            );
            let glyphs = std::mem::take(&mut self.text_glyphs);
            self.text_pipeline.upload(&self.device, &self.queue, &glyphs);
            self.text_glyphs = glyphs;
        }

        if let Some(frame) = &self.cursor_frame {
            self.queue.write_buffer(
                &self.cursor_pipeline.uniform_buffer,
                0,
                bytemuck::cast_slice(&[CursorUniform {
                    rect: frame.rect,
                    screen: [screen_w, screen_h, 0.0, 0.0],
                    fill: [1.0, 1.0, 1.0, 1.0],
                    outline: [0.0, 0.0, 0.0, 1.0],
                    alpha: frame.alpha,
                    angle: frame.angle,
                    shrink: frame.shrink,
                    _pad: 0.0,
                }]),
            );
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

        if draw_backdrop {
            let [r, g, b, _] = self.interaction.backdrop_dim_color;
            self.queue.write_buffer(
                &self.backdrop_pipeline.uniform_buffer,
                0,
                bytemuck::cast_slice(&[BackdropUniform { color: [r, g, b, dim] }]),
            );
        }

        if draw_particles {
            let forward = self.camera.forward();
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

            // The world is complete; darken all of it, then the modal window draws
            // on top undimmed.
            if draw_backdrop {
                render_pass.set_pipeline(&self.backdrop_pipeline.pipeline);
                render_pass.set_bind_group(0, &self.backdrop_pipeline.uniform_bind_group, &[]);
                render_pass.draw(0..3, 0..1);
            }

            if !modal_list.is_empty() {
                render_pass.set_pipeline(&self.quad_pipeline.pipeline);
                render_pass.set_vertex_buffer(0, self.quad_pipeline.vertex_buffer.slice(..));
                render_pass.set_index_buffer(
                    self.quad_pipeline.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint16,
                );
                for &(idx, _) in &modal_list {
                    let quad = &self.quads[idx].1;
                    render_pass.set_bind_group(0, &quad.bind_group, &[]);
                    render_pass.draw_indexed(0..self.quad_pipeline.num_indices, 0, 0..1);
                }
            }

            if !hud_list.is_empty() {
                render_pass.set_pipeline(&self.hud_pipeline.pipeline);
                render_pass.set_vertex_buffer(0, self.quad_pipeline.vertex_buffer.slice(..));
                render_pass.set_index_buffer(
                    self.quad_pipeline.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint16,
                );
                for &idx in &hud_list {
                    let quad = &self.quads[idx].1;
                    render_pass.set_bind_group(0, &quad.bind_group, &[]);
                    render_pass.draw_indexed(0..self.quad_pipeline.num_indices, 0, 0..1);
                }
            }

            if self.orb_frame.is_some() {
                render_pass.set_pipeline(&self.orb_pipeline.pipeline);
                render_pass.set_bind_group(0, &self.orb_pipeline.uniform_bind_group, &[]);
                render_pass.draw(0..6, 0..1);
            }

            if !tray_list.is_empty() {
                render_pass.set_pipeline(&self.hud_pipeline.pipeline);
                render_pass.set_vertex_buffer(0, self.quad_pipeline.vertex_buffer.slice(..));
                render_pass.set_index_buffer(
                    self.quad_pipeline.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint16,
                );
                for (quad, _) in &tray_list {
                    render_pass.set_bind_group(0, &quad.bind_group, &[]);
                    render_pass.draw_indexed(0..self.quad_pipeline.num_indices, 0, 0..1);
                }
            }

            if !self.gauges.is_empty() {
                render_pass.set_pipeline(&self.gauge_pipeline.pipeline);
                render_pass.set_bind_group(0, &self.gauge_pipeline.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.gauge_pipeline.instances.slice(..));
                render_pass.draw(0..6, 0..self.gauges.len() as u32);
            }

            if !self.panels.is_empty() {
                render_pass.set_pipeline(&self.panel_pipeline.pipeline);
                render_pass.set_bind_group(0, &self.panel_pipeline.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, self.panel_pipeline.instances.slice(..));
                render_pass.draw(0..6, 0..self.panels.len() as u32);
            }

            if !self.text_glyphs.is_empty() {
                let mask = self.text_mask_count;
                let total = self.text_glyphs.len() as u32;
                render_pass.set_pipeline(&self.text_pipeline.pipeline);
                render_pass.set_vertex_buffer(0, self.text_pipeline.instances.slice(..));
                if mask > 0 {
                    render_pass.set_bind_group(0, &self.text_pipeline.mask_bind_group, &[]);
                    render_pass.draw(0..6, 0..mask);
                }
                if total > mask {
                    render_pass.set_bind_group(0, &self.text_pipeline.color_bind_group, &[]);
                    render_pass.draw(0..6, mask..total);
                }
            }

            if self.cursor_frame.is_some() {
                render_pass.set_pipeline(&self.cursor_pipeline.pipeline);
                render_pass.set_bind_group(0, &self.cursor_pipeline.uniform_bind_group, &[]);
                render_pass.draw(0..6, 0..1);
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
        validate("backdrop.wgsl", include_str!("shaders/backdrop.wgsl"));
        validate("grid.wgsl", include_str!("shaders/grid.wgsl"));
        validate("particles.wgsl", include_str!("shaders/particles.wgsl"));
        validate("shadow.wgsl", include_str!("shaders/shadow.wgsl"));
        validate("orb.wgsl", include_str!("shaders/orb.wgsl"));
        validate("cursor.wgsl", include_str!("shaders/cursor.wgsl"));
        validate("gauge.wgsl", include_str!("shaders/gauge.wgsl"));
        validate("text.wgsl", include_str!("shaders/text.wgsl"));
        validate("panel.wgsl", include_str!("shaders/panel.wgsl"));
    }
}
