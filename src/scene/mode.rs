use std::str::FromStr;

use cgmath::{Deg, One, Point3, Quaternion, Rotation3, Vector3};

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
}

pub struct CameraFraming {
    pub distance: f32,
    pub elevation: f32,
    pub orbit: f32,
}

pub fn frame_camera(mode: SceneMode, camera: &mut Camera, framing: &CameraFraming) {
    match mode {
        SceneMode::Strata => {
            let elev = framing.elevation.to_radians();
            let orbit = framing.orbit.to_radians();
            let d = framing.distance;
            camera.eye = Point3::new(
                d * orbit.sin() * elev.cos(),
                d * elev.sin(),
                d * orbit.cos() * elev.cos(),
            );
            camera.target = Point3::new(0.0, 0.0, 0.0);
            camera.up = Vector3::unit_y();
        }
        SceneMode::Aether => {
            camera.eye = Point3::new(0.0, 0.0, AETHER_CAMERA_DISTANCE);
            camera.target = Point3::new(0.0, 0.0, 0.0);
            camera.up = Vector3::unit_y();
        }
    }
}

pub struct AetherView {
    pub distance: f32,
    pub pointer_ndc: [f32; 2],
    pub parallax_strength: f32,
}

pub fn frame_aether(camera: &mut Camera, view: &AetherView) {
    let px = view.pointer_ndc[0] * view.parallax_strength;
    let py = view.pointer_ndc[1] * view.parallax_strength;
    camera.eye = Point3::new(px, py, view.distance);
    camera.target = Point3::new(0.0, 0.0, 0.0);
    camera.up = Vector3::unit_y();
}

pub fn default_transform(mode: SceneMode, window_id: u32, scene: &SceneGraph) -> Transform {
    match mode {
        SceneMode::Strata => {
            let strata = &scene.strata;
            let base = Vector3::new(0.0, strata.window_base_y, strata.window_base_z);
            Transform {
                translation: base + strata_window_placement(window_id),
                rotation: Quaternion::from_angle_x(Deg(-strata.window_pitch)),
                ..Transform::default()
            }
        }
        SceneMode::Aether => Transform {
            translation: aether_window_placement(window_id, scene.aether.z_layer_gap),
            rotation: Quaternion::one(),
            ..Transform::default()
        },
    }
}
