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
}

impl WgpuState {
    pub async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                    ..Default::default()
                }
            )
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

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output
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
        output.present();

        Ok(())
    }
}
