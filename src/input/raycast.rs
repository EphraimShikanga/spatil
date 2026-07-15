use cgmath::{InnerSpace, Matrix4, SquareMatrix, Vector3, Vector4};
use hecs::Entity;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};

use crate::scene::components::{Transform, WaylandSurface};
use crate::state::SpatilState;

pub struct Ray {
    pub origin: Vector3<f32>,
    pub direction: Vector3<f32>,
}

impl Ray {
    pub fn plane_hit(&self, point: Vector3<f32>, normal: Vector3<f32>) -> Option<Vector3<f32>> {
        let denominator = self.direction.dot(normal);
        if denominator.abs() < 1e-6 {
            return None;
        }
        let t = (point - self.origin).dot(normal) / denominator;
        (t > 0.0).then(|| self.origin + self.direction * t)
    }
}

pub struct SurfaceHit {
    pub entity: Entity,
    pub surface: WlSurface,
    pub world_hit: Vector3<f32>,
    pub local_point: Point<f64, Logical>,
}

/// `cursor_pos` is in host-window pixels.
pub fn cursor_ray(state: &SpatilState, cursor_pos: Point<f64, Logical>) -> Option<Ray> {
    let width = state.wgpu_state.width as f32;
    let height = state.wgpu_state.height as f32;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    // Cursor pixels -> NDC. Screen Y grows downward, clip Y upward, so flip Y.
    let ndc_x = (cursor_pos.x as f32 / width) * 2.0 - 1.0;
    let ndc_y = 1.0 - (cursor_pos.y as f32 / height) * 2.0;

    let inv_vp = state.wgpu_state.camera.build_inverse_view_projection_matrix()?;

    // Unproject the near (z=0) and far (z=1) clip points.
    let near = unproject(&inv_vp, ndc_x, ndc_y, 0.0)?;
    let far = unproject(&inv_vp, ndc_x, ndc_y, 1.0)?;
    Some(Ray { origin: near, direction: (far - near).normalize() })
}

pub fn raycast_entity(state: &SpatilState, cursor_pos: Point<f64, Logical>) -> Option<SurfaceHit> {
    let ray = cursor_ray(state, cursor_pos)?;
    let mut best: Option<(f32, SurfaceHit)> = None;

    for (entity, (transform, surface)) in state.world.query::<(&Transform, &WaylandSurface)>().iter()
    {
        if !surface.visible || surface.closing {
            continue;
        }
        let Some((_, quad)) = state.wgpu_state.quads.iter().find(|(id, _)| *id == surface.id) else {
            continue;
        };
        let Some(inv_model) = transform.world_matrix.invert() else {
            continue;
        };

        let local_origin = (inv_model * ray.origin.extend(1.0)).truncate();
        let local_dir = (inv_model * ray.direction.extend(0.0)).truncate();
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
        let distance = (world_hit - ray.origin).magnitude();
        if best.as_ref().is_some_and(|(d, _)| *d <= distance) {
            continue;
        }

        // Local [-0.5, 0.5] -> UV -> surface-local pixels, v flips; output scale 1.
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

        best = Some((
            distance,
            SurfaceHit { entity, surface: wl_surface, world_hit, local_point },
        ));
    }

    best.map(|(_, hit)| hit)
}

pub fn raycast_surface(
    state: &SpatilState,
    cursor_pos: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    raycast_entity(state, cursor_pos).map(|hit| (hit.surface, hit.local_point))
}

fn unproject(inv_vp: &Matrix4<f32>, x: f32, y: f32, z: f32) -> Option<Vector3<f32>> {
    let p = inv_vp * Vector4::new(x, y, z, 1.0);
    if p.w.abs() < 1e-6 {
        return None;
    }
    Some(p.truncate() / p.w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_meets_a_screen_parallel_plane_where_it_should() {
        // Looking down -Z at a plane 2 units out, normal facing the camera.
        let ray = Ray { origin: Vector3::new(0.0, 0.0, 5.0), direction: -Vector3::unit_z() };
        let hit = ray
            .plane_hit(Vector3::new(0.0, 0.0, 3.0), Vector3::unit_z())
            .expect("a head-on ray must hit");
        assert!((hit - Vector3::new(0.0, 0.0, 3.0)).magnitude() < 1e-5, "{hit:?}");
    }

    #[test]
    fn an_off_axis_ray_lands_on_the_plane_not_the_plane_point() {
        let ray = Ray {
            origin: Vector3::new(0.0, 0.0, 5.0),
            direction: Vector3::new(1.0, 1.0, -2.0).normalize(),
        };
        let hit = ray
            .plane_hit(Vector3::new(0.0, 0.0, 3.0), Vector3::unit_z())
            .expect("hit expected");
        // Two units of travel in Z means one unit each in X and Y.
        assert!((hit.z - 3.0).abs() < 1e-5, "z {}", hit.z);
        assert!((hit.x - 1.0).abs() < 1e-5, "x {}", hit.x);
        assert!((hit.y - 1.0).abs() < 1e-5, "y {}", hit.y);
    }

    #[test]
    fn a_parallel_or_behind_ray_misses() {
        // Parallel to the plane: no intersection at all.
        let parallel = Ray { origin: Vector3::new(0.0, 0.0, 5.0), direction: Vector3::unit_x() };
        assert!(parallel.plane_hit(Vector3::new(0.0, 0.0, 3.0), Vector3::unit_z()).is_none());

        // Plane behind the camera: rejected rather than reported at negative t.
        let behind = Ray { origin: Vector3::new(0.0, 0.0, 5.0), direction: -Vector3::unit_z() };
        assert!(behind.plane_hit(Vector3::new(0.0, 0.0, 9.0), Vector3::unit_z()).is_none());
    }
}
