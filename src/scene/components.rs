use std::time::Instant;

use cgmath::{Matrix4, Quaternion, Vector3, SquareMatrix, Zero, One};
use hecs::Entity;
use smithay::reexports::wayland_server::backend::ObjectId;

use crate::scene::animation::AnimSnapshot;
use crate::scene::mode::SceneMode;

pub struct Transform {
    pub translation: Vector3<f32>,
    pub rotation: Quaternion<f32>,
    pub scale: Vector3<f32>,
    pub opacity: f32,
    pub world_matrix: Matrix4<f32>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vector3::zero(),
            rotation: Quaternion::one(),
            scale: Vector3::new(1.0, 1.0, 1.0),
            opacity: 1.0,
            world_matrix: Matrix4::identity(),
        }
    }
}

/// World units per second.
pub struct Velocity {
    pub linear: Vector3<f32>,
}

impl Velocity {
    pub fn linear(linear: Vector3<f32>) -> Self {
        Self { linear }
    }
}

pub struct Spring {
    pub anchor: Vector3<f32>,
    pub axis: Option<Vector3<f32>>,
    pub stiffness: f32,
    pub damping: f32,
}

pub struct Billboard;

#[derive(Default)]
pub struct Placement {
    pub strata: Option<AnimSnapshot>,
    pub aether: Option<AnimSnapshot>,
}

impl Placement {
    pub fn get(&self, mode: SceneMode) -> Option<&AnimSnapshot> {
        match mode {
            SceneMode::Strata => self.strata.as_ref(),
            SceneMode::Aether => self.aether.as_ref(),
        }
    }

    pub fn set(&mut self, mode: SceneMode, snapshot: AnimSnapshot) {
        match mode {
            SceneMode::Strata => self.strata = Some(snapshot),
            SceneMode::Aether => self.aether = Some(snapshot),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Ambient,
    Presented,
    Fullscreen,
}

pub struct Presentation {
    pub state: WindowState,
    pub ambient: AnimSnapshot,
    pub ambient_size: (i32, i32),
}

pub struct WaylandSurface {
    pub id: ObjectId,
    pub window_id: u32,
    pub visible: bool,
    pub closing: bool,
    pub committed_at: Option<Instant>,
}

impl WaylandSurface {
    pub fn new(id: ObjectId, window_id: u32) -> Self {
        Self {
            id,
            window_id,
            visible: false,
            closing: false,
            committed_at: None,
        }
    }
}

pub struct Parent {
    pub entity: Entity,
}
