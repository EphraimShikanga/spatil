use cgmath::Matrix4;
use smithay::reexports::wayland_server::backend::ObjectId;

use super::pipeline::Vertex;

pub struct HudQuad {
    pub id: ObjectId,
    pub rect: [f32; 4],
}

pub struct TrayQuad {
    pub key: String,
    pub rect: [f32; 4],
    pub alpha: f32,
}

#[rustfmt::skip]
pub fn hud_mvp(rect: [f32; 4], screen_w: f32, screen_h: f32) -> Matrix4<f32> {
    let [x, y, w, h] = rect;
    let (cx, cy) = (x + w * 0.5, y + h * 0.5);
    Matrix4::new(
        2.0 * w / screen_w,          0.0,                         0.0, 0.0,
        0.0,                         2.0 * h / screen_h,          0.0, 0.0,
        0.0,                         0.0,                         0.0, 0.0,
        2.0 * cx / screen_w - 1.0,   1.0 - 2.0 * cy / screen_h,   0.0, 1.0,
    )
}

pub struct HudPipeline {
    pub pipeline: wgpu::RenderPipeline,
}

impl HudPipeline {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        quad_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/quad.wgsl"));

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hud pipeline layout"),
            bind_group_layouts: &[Some(quad_bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(Vertex::layout())],
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

        Self { pipeline }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgmath::{SquareMatrix, Vector4};

    fn project(mvp: Matrix4<f32>, local: [f32; 2]) -> (f32, f32) {
        let p = mvp * Vector4::new(local[0], local[1], 0.0, 1.0);
        (p.x, p.y)
    }

    #[test]
    fn a_full_screen_rect_fills_clip_space() {
        let mvp = hud_mvp([0.0, 0.0, 1920.0, 1080.0], 1920.0, 1080.0);
        let (x, y) = project(mvp, [-0.5, 0.5]);
        assert!((x + 1.0).abs() < 1e-5 && (y - 1.0).abs() < 1e-5, "{x} {y}");
        let (x, y) = project(mvp, [0.5, -0.5]);
        assert!((x - 1.0).abs() < 1e-5 && (y + 1.0).abs() < 1e-5, "{x} {y}");
    }

    #[test]
    fn a_top_anchored_bar_lands_at_the_top_of_the_screen() {
        let mvp = hud_mvp([0.0, 0.0, 1920.0, 40.0], 1920.0, 1080.0);
        let (_, top) = project(mvp, [0.0, 0.5]);
        let (_, bottom) = project(mvp, [0.0, -0.5]);
        assert!((top - 1.0).abs() < 1e-5, "top {top}");
        assert!((bottom - (1.0 - 2.0 * 40.0 / 1080.0)).abs() < 1e-5, "bottom {bottom}");
        assert!(bottom < top, "screen Y must grow downward");
    }

    #[test]
    fn a_corner_rect_stays_in_its_corner() {
        let (sw, sh) = (800.0, 600.0);
        let mvp = hud_mvp([sw - 64.0, sh - 64.0, 48.0, 48.0], sw, sh);
        let (right, bottom) = project(mvp, [0.5, -0.5]);
        assert!((right - (1.0 - 2.0 * 16.0 / sw)).abs() < 1e-5, "right {right}");
        assert!((bottom - (-1.0 + 2.0 * 16.0 / sh)).abs() < 1e-5, "bottom {bottom}");
    }

    #[test]
    fn the_projection_is_pure_translation_and_scale() {
        let mvp = hud_mvp([100.0, 50.0, 200.0, 80.0], 1280.0, 720.0);
        assert_eq!(mvp.x.y, 0.0);
        assert_eq!(mvp.y.x, 0.0);
        assert!(mvp.determinant().abs() < 1e-9, "z column is intentionally flat");
    }
}
