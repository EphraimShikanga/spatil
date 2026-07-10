pub mod pipeline;
pub mod texture;

use std::sync::Arc;
use wgpu::{Device, Queue, Surface, SurfaceConfiguration};
use winit::window::Window;

use smithay::reexports::wayland_server::backend::ObjectId;

use pipeline::{Camera, QuadPipeline, QuadUniform};
use texture::TexturedQuad;

pub struct WgpuState {
    pub surface: Surface<'static>,
    pub device: Device,
    pub queue: Queue,
    pub config: SurfaceConfiguration,
    pub quad_pipeline: QuadPipeline,
    pub camera: Camera,
    /// Live textured quads, one per committed toplevel surface, keyed by the
    /// surface's Wayland object id. A `Vec` rather than a `HashMap` so draw
    /// order (and therefore placeholder placement) is deterministic.
    pub quads: Vec<(ObjectId, TexturedQuad)>,
    /// Set when `get_current_texture()` returned `Suboptimal`; reconfigured
    /// right after presenting the current (still-valid) frame.
    needs_reconfigure: bool,
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
            .expect("no Vulkan adapter available -- hardware-accelerated dmabuf import requires Vulkan");

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

        let quad_pipeline = QuadPipeline::new(&device, config.format);
        let camera = Camera::new(config.width as f32 / config.height as f32);

        Self {
            surface,
            device,
            queue,
            config,
            quad_pipeline,
            camera,
            quads: Vec::new(),
            needs_reconfigure: false,
        }
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
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
        let quad = TexturedQuad::new(
            &self.device,
            &self.queue,
            &self.quad_pipeline.bind_group_layout,
            width,
            height,
            stride,
            bytes,
        );
        if let Some(existing) = self.quads.iter_mut().find(|(qid, _)| *qid == id) {
            existing.1 = quad;
        } else {
            self.quads.push((id, quad));
        }
    }

    /// Removes a surface's texture, e.g. once its toplevel is destroyed.
    pub fn remove_texture(&mut self, id: &ObjectId) {
        self.quads.retain(|(qid, _)| qid != id);
    }

    /// Acquires and draws a frame. Returns `true` if the caller should
    /// reconfigure the surface (e.g. after resize/output-change) before the
    /// next frame; the surface is already reconfigured internally for the
    /// `Outdated` case, so this is only for the `Lost` case, which needs the
    /// full width/height passed back in via `resize()`.
    pub fn render(&mut self) -> RenderOutcome {
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
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            if !self.quads.is_empty() {
                let view_proj = self.camera.build_view_projection_matrix();
                let total = self.quads.len();

                render_pass.set_pipeline(&self.quad_pipeline.pipeline);
                render_pass.set_vertex_buffer(0, self.quad_pipeline.vertex_buffer.slice(..));
                render_pass.set_index_buffer(
                    self.quad_pipeline.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint16,
                );

                for (index, (_, quad)) in self.quads.iter().enumerate() {
                    let model = quad.model_matrix(index, total);
                    let mvp: [[f32; 4]; 4] = (view_proj * model).into();
                    self.queue.write_buffer(
                        &quad.uniform_buffer,
                        0,
                        bytemuck::cast_slice(&[QuadUniform { mvp }]),
                    );
                    render_pass.set_bind_group(0, &quad.bind_group, &[]);
                    render_pass.draw_indexed(0..self.quad_pipeline.num_indices, 0, 0..1);
                }
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
