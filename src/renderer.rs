pub mod grid;
pub mod particles;
pub mod pipeline;
pub mod shadow;
pub mod texture;

use std::collections::HashMap;
use std::sync::Arc;

use cgmath::{InnerSpace, Matrix4, Point3, SquareMatrix, Vector3};
use wgpu::{Device, Queue, Surface, SurfaceConfiguration};
use winit::window::Window;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer as _, Fourcc, Modifier};
use smithay::reexports::wayland_server::backend::ObjectId;

use crate::config::{Aether, Display, SceneGraph, SpatilConfig, parse_hex_color};
use crate::scene::WORLD_SCALE;
use crate::scene::components::{Transform, WaylandSurface};
use crate::scene::animation::SceneTransition;
use crate::scene::mode::{self, AetherView, CameraFraming, SceneMode};
use crate::scene::systems::update_world_matrices;

use grid::{GridPipeline, GridUniform};
use particles::{ParticlePipeline, ParticleUniform};
use pipeline::{Camera, OPENGL_TO_WGPU_MATRIX, QuadPipeline, QuadUniform};
use shadow::{LightUniform, ShadowMap, ShadowPipeline};
use texture::TexturedQuad;

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

fn glass_tint(aether: &Aether) -> [f32; 4] {
    let rgb = parse_hex_color(&aether.glass_tint);
    [rgb[0], rgb[1], rgb[2], aether.glass_tint_strength]
}

fn particle_color(aether: &Aether) -> [f32; 4] {
    let c = parse_hex_color(&aether.particle_color);
    [c[0], c[1], c[2], aether.particle_opacity]
}

fn floor_half_extent(display: &Display) -> [f32; 2] {
    [
        display.width as f32 / WORLD_SCALE / 2.0,
        display.height as f32 / WORLD_SCALE / 2.0,
    ]
}

/// Orthographic light view-projection, sized to cover the floor plus a margin
/// for windows hovering above and beyond its edges.
fn light_view_projection(light_position: [f32; 3], floor_half: [f32; 2]) -> Matrix4<f32> {
    let eye = Point3::new(light_position[0], light_position[1], light_position[2]);
    let target = Point3::new(0.0, 0.0, 0.0);
    let view = Matrix4::look_at_rh(eye, target, Vector3::unit_y());
    let radius = floor_half[0].max(floor_half[1]) + 3.0;
    let proj = cgmath::ortho(-radius, radius, -radius, radius, 0.1, 30.0);
    OPENGL_TO_WGPU_MATRIX * proj * view
}

pub struct WgpuState {
    pub surface: Surface<'static>,
    pub device: Device,
    pub queue: Queue,
    pub config: SurfaceConfiguration,
    pub depth_texture_view: wgpu::TextureView,
    pub quad_pipeline: QuadPipeline,
    pub grid_pipeline: GridPipeline,
    pub particle_pipeline: ParticlePipeline,
    pub particle_instances: wgpu::Buffer,
    pub particle_count: u32,
    pub shadow_pipeline: ShadowPipeline,
    pub shadow_map: ShadowMap,
    pub camera: Camera,
    pub scene_mode: SceneMode,
    pub scene_transition: SceneTransition,
    pub pointer_ndc: [f32; 2],
    pub grid_color: [f32; 4],
    pub void_color: [f32; 4],
    pub axis_color: [f32; 4],
    pub aether_void_color: [f32; 4],
    pub aether_particle_color: [f32; 4],
    pub aether_glass_tint: [f32; 4],
    pub aether_glass_alpha: f32,
    pub aether_particle_size: f32,
    pub aether_fog_density: f32,
    pub aether_camera_distance: f32,
    pub aether_field: [f32; 3],
    pub aether_parallax_enabled: bool,
    pub aether_parallax_strength: f32,
    pub floor_half: [f32; 2],
    pub light_position: [f32; 3],
    pub grid_spacing: f32,
    pub grid_extent: f32,
    pub grid_level: f32,
    pub grid_fog_density: f32,
    pub strata_camera_distance: f32,
    pub strata_camera_elevation: f32,
    pub strata_camera_orbit: f32,
    pub enable_shadows: bool,
    pub quads: Vec<(ObjectId, TexturedQuad)>,
    pub dmabuf_textures: HashMap<ObjectId, (wgpu::Texture, u32, u32, [f32; 2])>,
    pub main_device_id: Option<u64>,
    pub dmabuf_modifiers: Vec<Modifier>,
    needs_reconfigure: bool,
}

fn query_render_node_dev_t(adapter: &wgpu::Adapter) -> Option<u64> {
    // SAFETY: read-only property queries against the adapter's physical
    // device; the `as_hal` guard is dropped before returning.
    unsafe {
        let hal_adapter = adapter.as_hal::<wgpu_hal::api::Vulkan>()?;
        let shared = hal_adapter.shared_instance();
        // `get_physical_device_properties2` is core in Vulkan 1.1+.
        if shared.instance_api_version() < ash::vk::API_VERSION_1_1 {
            return None;
        }
        let instance = shared.raw_instance();
        let phys = hal_adapter.raw_physical_device();

        // Only chain the DRM-properties struct if the extension is actually
        // present -- querying an unsupported extension struct is invalid.
        let ext_supported = instance
            .enumerate_device_extension_properties(phys)
            .ok()?
            .iter()
            .any(|ext| {
                ext.extension_name_as_c_str()
                    .is_ok_and(|name| name == ash::ext::physical_device_drm::NAME)
            });
        if !ext_supported {
            return None;
        }

        let mut drm_props = ash::vk::PhysicalDeviceDrmPropertiesEXT::default();
        let mut props2 = ash::vk::PhysicalDeviceProperties2::default().push_next(&mut drm_props);
        instance.get_physical_device_properties2(phys, &mut props2);

        if drm_props.has_render == 0 {
            return None;
        }
        Some(libc::makedev(
            drm_props.render_major as libc::c_uint,
            drm_props.render_minor as libc::c_uint,
        ))
    }
}

pub enum RenderOutcome {
    Rendered,
    Skipped,
    Lost,
}


impl WgpuState {
    pub async fn new(window: Arc<Window>, config: &SpatilConfig) -> Self {
        let size = window.inner_size();

        // Forced to Vulkan (not `Backends::PRIMARY`): dmabuf zero-copy import
        // (`wgpu_hal::vulkan::Device::texture_from_dmabuf_fd`) only exists on
        // the Vulkan HAL backend. `display` is left unset -- wgpu's docs note
        // it's unused on Vulkan (only GLES needs it for presentation).
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

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

        // Only request the dmabuf-import feature if this adapter actually
        // supports it; otherwise Spatil still runs, just without the
        // `zwp_linux_dmabuf_v1` global (SHM-only).
        let adapter_features = adapter.features();
        let dmabuf_import_supported =
            adapter_features.contains(wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF);
        let required_features = if dmabuf_import_supported {
            wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF
        } else {
            tracing::warn!(
                "adapter does not support VULKAN_EXTERNAL_MEMORY_DMA_BUF -- dmabuf import will be unavailable"
            );
            wgpu::Features::empty()
        };

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features,
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                ..Default::default()
            })
            .await
            .unwrap();

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

        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Depth Texture"),
            size: wgpu::Extent3d {
                width: surface_config.width.max(1),
                height: surface_config.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_texture_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let scene = &config.scene_graph;
        let strata = &scene.strata;
        let scene_mode = scene.scene_mode();
        let floor_half = floor_half_extent(&config.display);

        let quad_pipeline = QuadPipeline::new(&device, surface_config.format);
        let shadow_pipeline = ShadowPipeline::new(&device);
        let shadow_map = ShadowMap::new(&device, strata.shadow_map_size);
        let grid_pipeline = GridPipeline::new(&device, surface_config.format, &shadow_map);
        let particle_pipeline = ParticlePipeline::new(&device, surface_config.format);
        let aether = &scene.aether;
        let particle_positions = particles::generate_particles(
            aether.particle_count,
            aether.field_width,
            aether.field_height,
            aether.field_depth,
        );
        let particle_instances = particles::instance_buffer(&device, &particle_positions);
        let mut camera =
            Camera::new(surface_config.width as f32 / surface_config.height as f32);
        mode::frame_camera(
            scene_mode,
            &mut camera,
            &CameraFraming {
                distance: strata.camera_distance,
                elevation: strata.camera_elevation,
                orbit: strata.camera_orbit,
            },
        );

        let render_node = query_render_node_dev_t(&adapter);
        let dmabuf_modifiers = query_dmabuf_modifiers(&adapter);

        let main_device_id = render_node;
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
            surface,
            device,
            queue,
            config: surface_config,
            depth_texture_view,
            quad_pipeline,
            grid_pipeline,
            particle_pipeline,
            particle_instances,
            particle_count: aether.particle_count,
            shadow_pipeline,
            shadow_map,
            camera,
            scene_mode,
            scene_transition: SceneTransition::idle(),
            pointer_ndc: [0.0, 0.0],
            grid_color: parse_hex_color(&strata.grid_color),
            void_color: parse_hex_color(&strata.void_color),
            axis_color: parse_hex_color(&strata.axis_color),
            aether_void_color: parse_hex_color(&aether.void_color),
            aether_particle_color: particle_color(aether),
            aether_glass_tint: glass_tint(aether),
            aether_glass_alpha: aether.glass_alpha,
            aether_particle_size: aether.particle_size,
            aether_fog_density: aether.fog_density,
            aether_camera_distance: aether.camera_distance,
            aether_field: [aether.field_width, aether.field_height, aether.field_depth],
            aether_parallax_enabled: aether.parallax_enabled,
            aether_parallax_strength: aether.parallax_strength,
            floor_half,
            light_position: strata.light_position,
            grid_spacing: strata.grid_spacing,
            grid_extent: strata.grid_extent,
            grid_level: strata.grid_level,
            grid_fog_density: strata.fog_density,
            strata_camera_distance: strata.camera_distance,
            strata_camera_elevation: strata.camera_elevation,
            strata_camera_orbit: strata.camera_orbit,
            enable_shadows: strata.enable_shadows,
            quads: Vec::new(),
            dmabuf_textures: HashMap::new(),
            main_device_id,
            dmabuf_modifiers,
            needs_reconfigure: false,
        }
    }

    pub fn apply_scene_config(&mut self, scene: &SceneGraph, display: &Display) {
        let strata = &scene.strata;
        self.scene_mode = scene.scene_mode();
        self.grid_color = parse_hex_color(&strata.grid_color);
        self.void_color = parse_hex_color(&strata.void_color);
        self.axis_color = parse_hex_color(&strata.axis_color);
        let aether = &scene.aether;
        self.aether_void_color = parse_hex_color(&aether.void_color);
        self.aether_particle_color = particle_color(aether);
        self.aether_glass_tint = glass_tint(aether);
        self.aether_glass_alpha = aether.glass_alpha;
        self.aether_particle_size = aether.particle_size;
        self.aether_fog_density = aether.fog_density;
        self.aether_camera_distance = aether.camera_distance;
        self.aether_parallax_enabled = aether.parallax_enabled;
        self.aether_parallax_strength = aether.parallax_strength;

        // Regenerate the point cloud only when its shape changed, so unrelated
        // edits don't reshuffle the stars.
        let field = [aether.field_width, aether.field_height, aether.field_depth];
        if self.particle_count != aether.particle_count || self.aether_field != field {
            let positions = particles::generate_particles(
                aether.particle_count,
                aether.field_width,
                aether.field_height,
                aether.field_depth,
            );
            self.particle_instances = particles::instance_buffer(&self.device, &positions);
            self.particle_count = aether.particle_count;
            self.aether_field = field;
        }

        self.floor_half = floor_half_extent(display);
        self.light_position = strata.light_position;
        self.grid_spacing = strata.grid_spacing;
        self.grid_extent = strata.grid_extent;
        self.grid_level = strata.grid_level;
        self.grid_fog_density = strata.fog_density;
        self.strata_camera_distance = strata.camera_distance;
        self.strata_camera_elevation = strata.camera_elevation;
        self.strata_camera_orbit = strata.camera_orbit;
        self.enable_shadows = strata.enable_shadows;
        mode::frame_camera(
            self.scene_mode,
            &mut self.camera,
            &CameraFraming {
                distance: strata.camera_distance,
                elevation: strata.camera_elevation,
                orbit: strata.camera_orbit,
            },
        );
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);

            let depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Depth Texture"),
                size: wgpu::Extent3d {
                    width: self.config.width.max(1),
                    height: self.config.height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth_texture_view =
                depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

            // Both modes only need the projection aspect updated on resize; the
            // eye/target are set by the mode framing (fixed for Strata, per-frame
            // for Aether).
            self.camera
                .set_aspect(new_size.width as f32 / new_size.height as f32);
        }
    }

    /// Uploads (or replaces) the texture for a committed surface, identified
    /// by its Wayland object id.
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

    /// Imports a single-plane, linear-modifier dmabuf as a zero-copy
    /// `wgpu::Texture` via the Vulkan HAL. Returns the texture without caching
    /// it -- the dmabuf handler decides how to key it (by `wl_buffer` id, once
    /// the buffer object exists).
    pub fn import_dmabuf(
        &self,
        dmabuf: &Dmabuf,
    ) -> Result<(wgpu::Texture, u32, u32, [f32; 2]), String> {
        if dmabuf.num_planes() != 1 {
            return Err(format!(
                "unsupported plane count {} (single-plane only)",
                dmabuf.num_planes()
            ));
        }
        let format = dmabuf.format();
        let texture_format = match format.code {
            // Use Srgb format so textureSample automatically performs sRGB-to-linear conversion.
            // When written to an Srgb swapchain, wgpu will convert it back. This is the only
            // way to guarantee correct linear blending and avoid the double-gamma "grey wash".
            Fourcc::Argb8888 | Fourcc::Xrgb8888 => wgpu::TextureFormat::Bgra8UnormSrgb,
            other => return Err(format!("unsupported fourcc {other:?}")),
        };

        tracing::info!(
            fourcc = %format.code,
            modifier = ?format.modifier,
            width = dmabuf.width(),
            height = dmabuf.height(),
            stride = dmabuf.strides().next().unwrap_or(0),
            offset = dmabuf.offsets().next().unwrap_or(0),
            y_inverted = dmabuf.y_inverted(),
            wgpu_format = ?texture_format,
            "attempting dmabuf import"
        );

        // `texture_from_dmabuf_fd` consumes an OwnedFd (Vulkan takes ownership
        // on success, closes it on failure). Smithay's Dmabuf only hands out
        // BorrowedFds it still owns, so dup first -- passing the original
        // would end in a double-close.
        let fd = dmabuf
            .handles()
            .next()
            .ok_or_else(|| "dmabuf has no plane fd".to_string())?
            .try_clone_to_owned()
            .map_err(|e| format!("failed to dup dmabuf fd: {e}"))?;
        let stride = u64::from(dmabuf.strides().next().unwrap_or(0));
        let offset = u64::from(dmabuf.offsets().next().unwrap_or(0));

        let client_width = dmabuf.width();
        let client_height = dmabuf.height();

        let bytes_per_pixel = 4; // Bgra8Unorm
        let texture_width = if format.modifier == Modifier::Linear && stride > 0 {
            (stride as u32) / bytes_per_pixel
        } else {
            client_width
        };

        let uv_scale_x = client_width as f32 / texture_width as f32;

        let size = wgpu::Extent3d {
            width: texture_width,
            height: client_height,
            depth_or_array_layers: 1,
        };

        if dmabuf.y_inverted() {
            tracing::warn!("dmabuf is y-inverted; content will render flipped (not yet handled)");
        }

        let hal_desc = wgpu_hal::TextureDescriptor {
            label: Some("dmabuf imported texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: vec![],
        };

        // SAFETY: the instance is created with `Backends::VULKAN`, so the HAL
        // downcast is to the right backend; `fd` is a freshly dup'd OwnedFd
        // whose ownership transfers to Vulkan; modifier/stride/offset are the
        // client-provided plane metadata Smithay validated at params time.
        let hal_texture = unsafe {
            let hal_device = self
                .device
                .as_hal::<wgpu_hal::api::Vulkan>()
                .ok_or_else(|| "device is not backed by Vulkan".to_string())?;
            hal_device
                .texture_from_dmabuf_fd(fd, &hal_desc, format.modifier.into(), stride, offset)
                .map_err(|e| format!("Vulkan dmabuf import failed: {e:?}"))?
        };

        let desc = wgpu::TextureDescriptor {
            label: Some("dmabuf imported texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        // SAFETY: `hal_texture` was created from this device's own Vulkan
        // handle and matches `desc`. The imported dmabuf already contains
        // initialized client-rendered pixels, so do not wrap it as
        // `UNINITIALIZED` -- that tells wgpu the contents may be discarded as
        // junk. `RESOURCE` is the closest wgpu-side state for our first use:
        // sampled read in the fragment shader.
        let texture = unsafe {
            self.device
                .create_texture_from_hal::<wgpu_hal::api::Vulkan>(
                    hal_texture,
                    &desc,
                    wgpu::TextureUses::RESOURCE,
                )
        };
        Ok((texture, client_width, client_height, [uv_scale_x, 1.0]))
    }

    /// Attaches a previously imported dmabuf texture (keyed by `wl_buffer` id)
    /// to a surface's quad. Returns `false` if no imported texture exists for
    /// that buffer.
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

    /// Drops an imported dmabuf texture once its `wl_buffer` is destroyed. Any
    /// quad still displaying it keeps its own clone alive (wgpu resources are
    /// internally refcounted), so this only releases the cache entry.
    pub fn remove_dmabuf_texture(&mut self, buffer_id: &ObjectId) {
        self.dmabuf_textures.remove(buffer_id);
    }

    /// Removes a surface's texture, e.g. once its toplevel is destroyed.
    pub fn remove_texture(&mut self, id: &ObjectId) {
        self.quads.retain(|(qid, _)| qid != id);
    }

    /// Inserts or replaces the quad for a surface.
    fn set_quad(&mut self, id: ObjectId, quad: TexturedQuad) {
        if let Some(existing) = self.quads.iter_mut().find(|(qid, _)| *qid == id) {
            existing.1 = quad;
        } else {
            self.quads.push((id, quad));
        }
    }

    fn frame_for(&self, camera: &mut Camera, mode: SceneMode) {
        match mode {
            SceneMode::Strata => mode::frame_camera(
                SceneMode::Strata,
                camera,
                &CameraFraming {
                    distance: self.strata_camera_distance,
                    elevation: self.strata_camera_elevation,
                    orbit: self.strata_camera_orbit,
                },
            ),
            SceneMode::Aether => {
                let parallax = if self.aether_parallax_enabled {
                    self.aether_parallax_strength
                } else {
                    0.0
                };
                mode::frame_aether(
                    camera,
                    &AetherView {
                        distance: self.aether_camera_distance,
                        pointer_ndc: self.pointer_ndc,
                        parallax_strength: parallax,
                    },
                );
            }
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

    /// Acquires and draws a frame. Returns `true` if the caller should
    /// reconfigure the surface (e.g. after resize/output-change) before the
    /// next frame; the surface is already reconfigured internally for the
    /// `Outdated` case, so this is only for the `Lost` case, which needs the
    /// full width/height passed back in via `resize()`.
    pub fn render(&mut self, world: &mut hecs::World) -> RenderOutcome {
        update_world_matrices(world);

        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                // Still valid to draw to this frame; reconfigure right after
                // presenting so the next frame is optimal.
                self.needs_reconfigure = true;
                t
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                // Docs: "Call Surface::configure() and try again."
                self.surface.configure(&self.device, &self.config);
                return RenderOutcome::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => {
                return RenderOutcome::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return RenderOutcome::Lost;
            }
        };
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

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
        let light_view_proj = light_view_projection(self.light_position, self.floor_half);

        let clear = lerp4(self.void_color, self.aether_void_color, presence);
        let window_alpha = 1.0 + (self.aether_glass_alpha - 1.0) * presence;
        let window_tint = lerp4([0.0; 4], self.aether_glass_tint, presence);

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
        if draw_grid && self.enable_shadows {
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
                    grid_color: self.grid_color,
                    void_color: self.void_color,
                    axis_color: self.axis_color,
                    grid_extent: self.grid_extent,
                    grid_level: self.grid_level,
                    grid_spacing: self.grid_spacing,
                    fog_density: self.grid_fog_density,
                    enable_shadows: if self.enable_shadows { 1.0 } else { 0.0 },
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
                    bg_color: self.aether_void_color,
                    color: self.aether_particle_color,
                    size: self.aether_particle_size,
                    fog_density: self.aether_fog_density,
                    field_alpha,
                    _pad: 0.0,
                }]),
            );
        }

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
                render_pass.draw(0..6, 0..self.particle_count);
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
        self.queue.present(surface_texture);

        if self.needs_reconfigure {
            self.needs_reconfigure = false;
            self.surface.configure(&self.device, &self.config);
        }

        RenderOutcome::Rendered
    }
}

pub fn query_dmabuf_modifiers(adapter: &wgpu::Adapter) -> Vec<Modifier> {
    unsafe {
        let Some(hal_adapter) = adapter.as_hal::<wgpu_hal::api::Vulkan>() else {
            return vec![Modifier::Linear];
        };
        let shared = hal_adapter.shared_instance();
        let instance = shared.raw_instance();
        let phys = hal_adapter.raw_physical_device();

        if shared.instance_api_version() < ash::vk::API_VERSION_1_1 {
            return vec![Modifier::Linear];
        }

        let mut modifier_list = ash::vk::DrmFormatModifierPropertiesListEXT::default();
        {
            let mut format_props2 =
                ash::vk::FormatProperties2::default().push_next(&mut modifier_list);
            instance.get_physical_device_format_properties2(
                phys,
                ash::vk::Format::B8G8R8A8_UNORM,
                &mut format_props2,
            );
        }

        if modifier_list.drm_format_modifier_count == 0 {
            return vec![Modifier::Linear];
        }

        let mut modifiers = Vec::with_capacity(modifier_list.drm_format_modifier_count as usize);
        let mut props = vec![
            ash::vk::DrmFormatModifierPropertiesEXT::default();
            modifier_list.drm_format_modifier_count as usize
        ];
        modifier_list.p_drm_format_modifier_properties = props.as_mut_ptr();

        {
            let mut format_props2 =
                ash::vk::FormatProperties2::default().push_next(&mut modifier_list);
            instance.get_physical_device_format_properties2(
                phys,
                ash::vk::Format::B8G8R8A8_UNORM,
                &mut format_props2,
            );
        }

        for prop in props {
            if prop
                .drm_format_modifier_tiling_features
                .contains(ash::vk::FormatFeatureFlags::SAMPLED_IMAGE)
            {
                modifiers.push(Modifier::from(prop.drm_format_modifier));
            }
        }

        if modifiers.is_empty() {
            modifiers.push(Modifier::Linear);
        }
        modifiers
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
