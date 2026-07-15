use std::str::FromStr;

use cgmath::{
    Deg, EuclideanSpace, InnerSpace, Matrix3, One, Point3, Quaternion, Rotation3, Vector3,
};

use crate::config::SceneGraph;
use crate::renderer::pipeline::Camera;
use crate::scene::components::Transform;
use crate::scene::systems::{aether_window_placement, strata_window_placement};

// Baseline Aether camera distance for the static framing path; the per-frame
// path uses the configured distance.
const AETHER_CAMERA_DISTANCE: f32 = 2.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SceneMode {
    #[default]
    Strata,
    Aether,
}

impl FromStr for SceneMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "strata" => Ok(Self::Strata),
            "aether" => Ok(Self::Aether),
            _ => Err(()),
        }
    }
}

impl SceneMode {
    pub fn from_config(s: &str) -> Self {
        s.parse().unwrap_or_else(|()| {
            tracing::warn!("unknown scene mode {s:?}; defaulting to strata");
            Self::Strata
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strata => "strata",
            Self::Aether => "aether",
        }
    }
}

pub struct CameraFraming {
    pub distance: f32,
    pub elevation: f32,
    pub orbit: f32,
    pub target: Vector3<f32>,
}

pub fn frame_camera(mode: SceneMode, camera: &mut Camera, framing: &CameraFraming) {
    let target = Point3::from_vec(framing.target);
    match mode {
        SceneMode::Strata => {
            let elev = framing.elevation.to_radians();
            let orbit = framing.orbit.to_radians();
            let d = framing.distance;
            camera.eye = target
                + Vector3::new(
                    d * orbit.sin() * elev.cos(),
                    d * elev.sin(),
                    d * orbit.cos() * elev.cos(),
                );
        }
        SceneMode::Aether => {
            camera.eye = target + Vector3::new(0.0, 0.0, AETHER_CAMERA_DISTANCE);
        }
    }
    camera.target = target;
    camera.up = Vector3::unit_y();
}

pub struct AetherView {
    pub distance: f32,
    pub pointer_ndc: [f32; 2],
    pub parallax_strength: f32,
    pub target: Vector3<f32>,
    pub orbit: f32,
    pub elevation: f32,
}

pub fn frame_aether(camera: &mut Camera, view: &AetherView) {
    let target = Point3::from_vec(view.target);
    let elev = view.elevation.to_radians();
    let orbit = view.orbit.to_radians();
    let d = view.distance;
    // Parallax shifts the eye in the camera's own plane, so it survives an orbit.
    let forward = Vector3::new(
        -orbit.sin() * elev.cos(),
        -elev.sin(),
        -orbit.cos() * elev.cos(),
    );
    let right = forward.cross(Vector3::unit_y()).normalize();
    let up = right.cross(forward);
    let px = view.pointer_ndc[0] * view.parallax_strength;
    let py = view.pointer_ndc[1] * view.parallax_strength;

    camera.eye = target - forward * d + right * px + up * py;
    camera.target = target;
    camera.up = Vector3::unit_y();
}

pub fn billboard_rotation(eye: Point3<f32>, target: Point3<f32>) -> Quaternion<f32> {
    let back = eye - target;
    if back.magnitude2() < 1e-8 {
        return Quaternion::one();
    }
    let back = back.normalize();
    let mut right = Vector3::unit_y().cross(back);
    if right.magnitude2() < 1e-8 {
        // Looking straight down the world up-axis: the cross product degenerates,
        // so pick a perpendicular deterministically.
        right = Vector3::unit_x();
    }
    let right = right.normalize();
    let up = back.cross(right);
    Quaternion::from(Matrix3::from_cols(right, up, back))
}

pub fn fit_distance(fovy_degrees: f32, height: f32, fill: f32) -> f32 {
    let half_fov = (fovy_degrees * 0.5).to_radians();
    height / (2.0 * fill.clamp(0.05, 1.0) * half_fov.tan())
}

pub fn default_transform(
    mode: SceneMode,
    window_id: u32,
    scene: &SceneGraph,
    facing: Quaternion<f32>,
) -> Transform {
    match mode {
        SceneMode::Strata => {
            let strata = &scene.strata;
            let base = Vector3::new(0.0, strata.window_base_y, strata.window_base_z);
            Transform {
                translation: base + strata_window_placement(window_id),
                // Post-multiplied: the recline happens in the window's own space, so
                // it leans away from the camera rather than from world -Z.
                rotation: facing * Quaternion::from_angle_x(Deg(-strata.window_pitch)),
                ..Transform::default()
            }
        }
        SceneMode::Aether => Transform {
            translation: aether_window_placement(window_id, scene.aether.z_layer_gap),
            rotation: facing,
            ..Transform::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgmath::Zero;

    fn camera() -> Camera {
        Camera::new(16.0 / 9.0)
    }

    #[test]
    fn pan_moves_the_eye_and_the_look_at_target_together() {
        let mut c = camera();
        let framing = |target| CameraFraming { distance: 3.0, elevation: 0.0, orbit: 0.0, target };

        frame_camera(SceneMode::Strata, &mut c, &framing(Vector3::zero()));
        let (eye, target) = (c.eye, c.target);

        let pan = Vector3::new(1.0, 2.0, 0.0);
        frame_camera(SceneMode::Strata, &mut c, &framing(pan));
        assert_eq!(c.target, target + pan);
        assert_eq!(c.eye, eye + pan);
    }

    #[test]
    fn orbit_swings_the_eye_around_the_target_at_a_fixed_distance() {
        let mut c = camera();
        frame_camera(
            SceneMode::Strata,
            &mut c,
            &CameraFraming { distance: 3.0, elevation: 0.0, orbit: 90.0, target: Vector3::zero() },
        );
        // A quarter turn puts the camera on +X, still 3 units out.
        assert!((c.eye.x - 3.0).abs() < 1e-4, "eye.x {}", c.eye.x);
        assert!(c.eye.z.abs() < 1e-4, "eye.z {}", c.eye.z);
        assert!((c.eye.to_vec().magnitude() - 3.0).abs() < 1e-4);
    }

    #[test]
    fn the_straight_on_aether_camera_billboards_to_identity() {
        // The pose Aether panes already hold, so billboarding changes nothing until
        // the view actually orbits.
        let mut c = camera();
        frame_aether(
            &mut c,
            &AetherView {
                distance: 2.5,
                pointer_ndc: [0.0, 0.0],
                parallax_strength: 0.0,
                target: Vector3::zero(),
                orbit: 0.0,
                elevation: 0.0,
            },
        );
        let rotation = billboard_rotation(c.eye, c.target);
        assert!(
            (rotation * Vector3::unit_z() - Vector3::unit_z()).magnitude() < 1e-5,
            "expected identity, got {rotation:?}"
        );
    }

    #[test]
    fn a_billboard_lies_flat_against_the_view_plane_from_any_angle() {
        for (eye, target) in [
            (Point3::new(3.0, 0.0, 0.0), Point3::new(0.0, 0.0, 0.0)),
            (Point3::new(-2.0, 1.5, -2.0), Point3::new(0.0, 0.0, 0.0)),
            (Point3::new(1.0, 1.0, 1.0), Point3::new(0.5, 0.0, -0.5)),
        ] {
            let rotation = billboard_rotation(eye, target);
            // The quad's local +Z (its face normal) must oppose the view direction.
            let normal = rotation * Vector3::unit_z();
            let back = (eye - target).normalize();
            assert!(
                (normal - back).magnitude() < 1e-4,
                "normal {normal:?} should face the camera along {back:?}"
            );
            // And it must stay upright: no roll about the view axis.
            let up = rotation * Vector3::unit_y();
            assert!(up.y > 0.0, "billboard rolled over: up {up:?}");
        }
    }

    #[test]
    fn a_billboard_degenerates_gracefully_looking_straight_down() {
        let rotation = billboard_rotation(Point3::new(0.0, 5.0, 0.0), Point3::new(0.0, 0.0, 0.0));
        let normal = rotation * Vector3::unit_z();
        assert!((normal - Vector3::unit_y()).magnitude() < 1e-4, "normal {normal:?}");
    }

    #[test]
    fn a_window_opens_facing_the_camera_from_any_angle() {
        let scene = SceneGraph::default();
        for orbit in [0.0, 90.0, 200.0] {
            let mut c = camera();
            frame_camera(
                SceneMode::Strata,
                &mut c,
                &CameraFraming { distance: 3.0, elevation: 0.0, orbit, target: Vector3::zero() },
            );
            let facing = billboard_rotation(c.eye, c.target);
            let t = default_transform(SceneMode::Strata, 1, &scene, facing);

            // It reclines by window_pitch, so the angle to the view direction is
            // that pitch whatever the orbit -- never facing away.
            let normal = t.rotation * Vector3::unit_z();
            let to_camera = (c.eye.to_vec() - t.translation).normalize();
            let angle = normal.dot(to_camera).acos().to_degrees();
            assert!(
                (angle - scene.strata.window_pitch).abs() < 15.0,
                "orbit {orbit}: window is {angle} deg off the camera, expected ~{}",
                scene.strata.window_pitch
            );
        }
    }

    #[test]
    fn the_unorbited_camera_reproduces_the_original_spawn_pose() {
        // Navigation must not have moved the furniture: with the default view,
        // `facing` is identity and the pose is the pre-phase-3 one exactly.
        let scene = SceneGraph::default();
        let t = default_transform(SceneMode::Strata, 1, &scene, Quaternion::one());
        let expected = Quaternion::from_angle_x(Deg(-scene.strata.window_pitch));
        assert!((t.rotation - expected).magnitude() < 1e-6, "got {:?}", t.rotation);

        let t = default_transform(SceneMode::Aether, 1, &scene, Quaternion::one());
        assert!((t.rotation - Quaternion::one()).magnitude() < 1e-6);
    }

    #[test]
    fn fit_distance_frames_the_requested_fraction_of_the_view() {
        let (fovy, height) = (45.0, 1.08);
        let d = fit_distance(fovy, height, 1.0);
        // At d the viewport is exactly as tall as the quad.
        let viewport = 2.0 * d * (fovy * 0.5f32).to_radians().tan();
        assert!((viewport - height).abs() < 1e-4, "viewport {viewport} vs height {height}");

        // Half the fill needs twice the distance.
        assert!((fit_distance(fovy, height, 0.5) - 2.0 * d).abs() < 1e-4);
    }
}
