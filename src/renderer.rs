pub mod background;
pub mod pipeline;
pub mod texture;

use std::collections::HashMap;
use std::sync::Arc;
use wgpu::{Device, Queue, Surface, SurfaceConfiguration};
use winit::window::Window;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer as _, Fourcc, Modifier};
use smithay::reexports::wayland_server::backend::ObjectId;

use pipeline::{Camera, QuadPipeline, QuadUniform};
use texture::TexturedQuad;

use crate::scene::components::{Transform, WaylandSurface};
use crate::scene::systems::update_world_matrices;

pub struct WgpuState {
    pub surface: Surface<'static>,
    pub device: Device,
    pub queue: Queue,
    pub config: SurfaceConfiguration,
    pub depth_texture_view: wgpu::TextureView,
    pub quad_pipeline: QuadPipeline,
    pub background_pipeline: background::BackgroundPipeline,
    pub camera: Camera,
    /// Live textured quads, one per committed toplevel surface, keyed by the
    /// surface's Wayland object id. A `Vec` rather than a `HashMap` so draw
    /// order (and therefore placeholder placement) is deterministic.
    pub quads: Vec<(ObjectId, TexturedQuad)>,
    /// Zero-copy dmabuf textures, keyed by the `wl_buffer`'s object id. Import
    /// happens once per buffer (in `DmabufHandler::dmabuf_imported`), not per
    /// commit -- commits just look the texture up here and attach it to the
    /// committing surface's quad.
    pub dmabuf_textures: HashMap<ObjectId, (wgpu::Texture, u32, u32, [f32; 2])>,
    /// The DRM render node (`dev_t`) of the adapter's GPU, queried via
    /// `VK_EXT_physical_device_drm`. Mesa clients need this advertised through
    /// dmabuf v4 feedback (`main_device`) to pick the right GPU for their EGL
    /// context; without it their hardware path fails ("fd -1") and they fall
    /// back to software rendering. `None` if the driver can't report it.
    pub main_device_id: Option<u64>,
    pub dmabuf_modifiers: Vec<Modifier>,
    /// Set when `get_current_texture()` returned `Suboptimal`; reconfigured
    /// right after presenting the current (still-valid) frame.
    needs_reconfigure: bool,
}

/// Queries the DRM render node device id (`dev_t`) of the adapter's physical
/// device via `VK_EXT_physical_device_drm`. Advertised to clients as dmabuf
/// v4 feedback's `main_device` so Mesa knows which GPU to allocate on.
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

/// Result of a `WgpuState::render()` call, for the caller to act on.
pub enum RenderOutcome {
    /// A frame was drawn and presented.
    Rendered,
    /// No frame was presented this pass (occluded/timeout/outdated/etc.) --
    /// not an error, just try again next frame.
    Skipped,
    /// The surface was lost and needs a full `resize()` to recreate it.
    Lost,
}

impl WgpuState {
    pub async fn new(window: Arc<Window>) -> Self {
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

        let config = wgpu::SurfaceConfiguration {
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
        surface.configure(&device, &config);

        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Depth Texture"),
            size: wgpu::Extent3d {
                width: config.width.max(1),
                height: config.height.max(1),
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

        let quad_pipeline = QuadPipeline::new(&device, config.format);
        let background_pipeline = background::BackgroundPipeline::new(&device, config.format);
        let camera = Camera::new(config.width as f32 / config.height as f32);

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
            config,
            depth_texture_view,
            quad_pipeline,
            background_pipeline,
            camera,
            quads: Vec::new(),
            dmabuf_textures: HashMap::new(),
            main_device_id,
            dmabuf_modifiers,
            needs_reconfigure: false,
        }
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

            self.camera.aspect = new_size.width as f32 / new_size.height as f32;
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

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
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

            if !self.quads.is_empty() {
                // This is to just test the 3d effect by rotating the camera for now
                static START_TIME: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);
                let time = START_TIME.elapsed().as_secs_f32();
                self.camera.eye = cgmath::Point3::new(time.sin() * 5.0, 0.0, time.cos() * 5.0);
                
                let view_proj = self.camera.build_view_projection_matrix();

                render_pass.set_pipeline(&self.quad_pipeline.pipeline);
                render_pass.set_vertex_buffer(0, self.quad_pipeline.vertex_buffer.slice(..));
                render_pass.set_index_buffer(
                    self.quad_pipeline.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint16,
                );

                for (_id, (transform, wayland_surface)) in
                    world.query_mut::<(&Transform, &WaylandSurface)>()
                {
                    if let Some((_, quad)) = self
                        .quads
                        .iter()
                        .find(|(qid, _)| *qid == wayland_surface.id)
                    {
                        let model = transform.world_matrix;
                        let mvp: [[f32; 4]; 4] = (view_proj * model).into();
                        self.queue.write_buffer(
                            &quad.uniform_buffer,
                            0,
                            bytemuck::cast_slice(&[QuadUniform {
                                mvp,
                                uv_scale: quad.uv_scale,
                                _padding: [0.0; 2],
                            }]),
                        );
                        render_pass.set_bind_group(0, &quad.bind_group, &[]);
                        render_pass.draw_indexed(0..self.quad_pipeline.num_indices, 0, 0..1);
                    }
                }
            }

            // Draw background last so it fills in any empty space
            render_pass.set_pipeline(&self.background_pipeline.pipeline);
            render_pass.draw(0..3, 0..1);
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
