use cgmath::{InnerSpace, Matrix4, SquareMatrix, Vector3, Vector4};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};

use crate::scene::components::{Transform, WaylandSurface};
use crate::state::SpatilState;

/// The closest visible window under `cursor_pos` (host-window pixels), with the
/// surface-local hit point in logical coordinates.
pub fn raycast_surface(
    state: &SpatilState,
    cursor_pos: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let width = state.wgpu_state.config.width as f32;
    let height = state.wgpu_state.config.height as f32;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    // Cursor pixels → NDC. Screen Y grows downward, clip Y upward, so flip Y.
    let ndc_x = (cursor_pos.x as f32 / width) * 2.0 - 1.0;
    let ndc_y = 1.0 - (cursor_pos.y as f32 / height) * 2.0;

    let inv_vp = state.wgpu_state.camera.build_inverse_view_projection_matrix()?;

    // World-space ray: unproject the near (z=0) and far (z=1) clip points.
    let near = unproject(&inv_vp, ndc_x, ndc_y, 0.0)?;
    let far = unproject(&inv_vp, ndc_x, ndc_y, 1.0)?;
    let ray_origin = near;
    let ray_dir = (far - near).normalize();

    let mut best: Option<(f32, WlSurface, Point<f64, Logical>)> = None;

    for (_entity, (transform, surface)) in state.world.query::<(&Transform, &WaylandSurface)>().iter()
    {
        if !surface.visible {
            continue;
        }
        let Some((_, quad)) = state.wgpu_state.quads.iter().find(|(id, _)| *id == surface.id) else {
            continue;
        };
        let Some(inv_model) = transform.world_matrix.invert() else {
            continue;
        };

        // Ray into the quad's local space (unit quad in the z=0 plane); the
        // affine world matrix keeps origin w=1, direction w=0.
        let local_origin = (inv_model * ray_origin.extend(1.0)).truncate();
        let local_dir = (inv_model * ray_dir.extend(0.0)).truncate();
        if local_dir.z.abs() < 1e-6 {
            continue;
        }
        let t = -local_origin.z / local_dir.z;
        if t <= 0.0 {
            continue;
        }
        let hit = local_origin + local_dir * t;
        if hit.x < -0.5 || hit.x > 0.5 || hit.y < -0.5 || hit.y > 0.5 {
            continue;
        }

        // Front-most window wins.
        let world_hit = (transform.world_matrix * hit.extend(1.0)).truncate();
        let distance = (world_hit - ray_origin).magnitude();
        if best.as_ref().is_some_and(|(d, _, _)| *d <= distance) {
            continue;
        }

        // Local [-0.5, 0.5] → UV → surface-local pixels (v flips; output scale 1).
        let u = (hit.x + 0.5) as f64;
        let v = (0.5 - hit.y) as f64;
        let local_point =
            Point::<f64, Logical>::from((u * quad.width as f64, v * quad.height as f64));

        let Some(wl_surface) = state
            .toplevels
            .iter()
            .find(|t| t.wl_surface().id() == surface.id)
            .map(|t| t.wl_surface().clone())
        else {
            continue;
        };

        best = Some((distance, wl_surface, local_point));
    }

    best.map(|(_, surface, point)| (surface, point))
}

fn unproject(inv_vp: &Matrix4<f32>, x: f32, y: f32, z: f32) -> Option<Vector3<f32>> {
    let p = inv_vp * Vector4::new(x, y, z, 1.0);
    if p.w.abs() < 1e-6 {
        return None;
    }
    Some(p.truncate() / p.w)
}
