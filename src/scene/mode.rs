//! Scene-mode seam: what varies between Strata and Aether.

use std::str::FromStr;

use cgmath::{Deg, One, Point3, Quaternion, Rotation3, Vector3};

use crate::config::SceneGraph;
use crate::renderer::pipeline::Camera;
use crate::scene::components::Transform;
use crate::scene::systems::{aether_window_placement, default_window_placement};

// Phase-0 placeholder framing distance; phase 1 derives it from the window
// cluster bounds.
const AETHER_CAMERA_DISTANCE: f32 = 3.0;

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
    pub camera_pitch: f32,
    pub floor_half: [f32; 2],
}

pub fn frame_camera(mode: SceneMode, camera: &mut Camera, framing: &CameraFraming) {
    match mode {
        SceneMode::Strata => camera.retarget(framing.camera_pitch, framing.floor_half),
        SceneMode::Aether => {
            camera.eye = Point3::new(0.0, 0.0, AETHER_CAMERA_DISTANCE);
            camera.target = Point3::new(0.0, 0.0, 0.0);
            camera.up = Vector3::unit_y();
        }
    }
}

pub fn default_transform(mode: SceneMode, window_id: u32, scene: &SceneGraph) -> Transform {
    match mode {
        SceneMode::Strata => {
            let strata = &scene.strata;
            Transform {
                translation: default_window_placement(window_id, strata.window_hover_height),
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
