//! `--hud-snapshot <dir>`: renders the HUD offscreen through the real pipelines and writes PNGs.

use std::path::{Path, PathBuf};

use crate::config::SpatilConfig;
use crate::renderer::WgpuState;

pub const WIDTH: u32 = 900;
pub const HEIGHT: u32 = 600;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

pub struct Snapshotter {
    target: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row: u32,
    out_dir: PathBuf,
}

/// A renderer with no surface, sized for snapshots; the DRM path builds the same way.
pub fn headless(config: &SpatilConfig) -> Result<WgpuState, String> {
        let instance = super::new_vulkan_instance();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("no adapter: {e}"))?;
        let (device, queue) = pollster::block_on(super::request_device(&adapter));

        Ok(WgpuState::from_device(device, queue, &adapter, FORMAT, WIDTH, HEIGHT, config))
}

impl Snapshotter {
    pub fn new(wgpu: &WgpuState, out_dir: &Path) -> Result<Self, String> {
        let target = wgpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hud snapshot target"),
            size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let padded_row = (WIDTH * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = wgpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud snapshot readback"),
            size: (padded_row * HEIGHT) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
        Ok(Self { target, readback, padded_row, out_dir: out_dir.to_path_buf() })
    }

    pub fn capture(
        &self,
        gpu: &mut WgpuState,
        world: &mut hecs::World,
        name: &str,
    ) -> Result<PathBuf, String> {
        let view = self.target.create_view(&wgpu::TextureViewDescriptor::default());
        gpu.render(world, &view);

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("hud snapshot copy") });
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(HEIGHT),
                },
            },
            wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        );
        gpu.queue.submit(std::iter::once(encoder.finish()));

        let slice = self.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("poll: {e}"))?;

        let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
        {
            let data = slice.get_mapped_range().map_err(|e| format!("map: {e:?}"))?;
            for row in data.chunks(self.padded_row as usize) {
                for px in row[..(WIDTH * 4) as usize].chunks(4) {
                    rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
        }
        self.readback.unmap();

        let path = self.out_dir.join(format!("{name}.png"));
        let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), WIDTH, HEIGHT);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .and_then(|mut w| w.write_image_data(&rgba))
            .map_err(|e| e.to_string())?;
        Ok(path)
    }
}
