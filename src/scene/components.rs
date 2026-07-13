use std::time::Instant;

use cgmath::{Matrix4, Quaternion, Vector3, SquareMatrix, Zero, One};
use hecs::Entity;
use smithay::reexports::wayland_server::backend::ObjectId;

pub struct Transform {
    pub translation: Vector3<f32>,
    pub rotation: Quaternion<f32>,
    pub scale: Vector3<f32>,
    pub world_matrix: Matrix4<f32>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vector3::zero(),
            rotation: Quaternion::one(),
            scale: Vector3::new(1.0, 1.0, 1.0),
            world_matrix: Matrix4::identity(),
        }
    }
}

pub struct Velocity {
    pub linear: Vector3<f32>,
    pub angular: Vector3<f32>,
}

pub struct WaylandSurface {
    pub id: ObjectId,
    pub window_id: u32,
    pub visible: bool,
    pub committed_at: Option<Instant>,
}

impl WaylandSurface {
    pub fn new(id: ObjectId, window_id: u32) -> Self {
        Self {
            id,
            window_id,
            visible: false,
            committed_at: None,
        }
    }
}

pub struct Parent {
    pub entity: Entity,
}
