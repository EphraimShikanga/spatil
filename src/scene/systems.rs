use std::time::Duration;

use cgmath::Vector3;
use hecs::World;
use crate::scene::components::{Transform, Parent, WaylandSurface};

pub fn default_window_placement(window_id: u32, hover_height: f32) -> Vector3<f32> {
    const COLS: u32 = 3;
    const SPACING: f32 = 0.9;
    let index = window_id.saturating_sub(1);
    let col = (index % COLS) as f32;
    let row = (index / COLS) as f32;
    let x = (col - (COLS as f32 - 1.0) / 2.0) * SPACING;
    let z = -row * SPACING;
    Vector3::new(x, hover_height, z)
}

pub fn aether_window_placement(window_id: u32, z_layer_gap: f32) -> Vector3<f32> {
    const COLS: u32 = 3;
    const SPACING: f32 = 0.9;
    let index = window_id.saturating_sub(1);
    let col = (index % COLS) as f32;
    let row = (index / COLS) as f32;
    let x = (col - (COLS as f32 - 1.0) / 2.0) * SPACING;
    let y = -row * SPACING;
    let z = -(index as f32) * z_layer_gap;
    Vector3::new(x, y, z)
}

/// Reveals committed-but-hidden windows once map-postponement lapses: either no
/// script is connected to place them, or the timeout since first commit expired.
/// IPC commands reveal windows directly; this is the automatic fallback path.
pub fn update_map_visibility(world: &mut World, has_ipc_clients: bool, map_timeout: Duration) {
    for (_entity, surface) in world.query_mut::<&mut WaylandSurface>() {
        if surface.visible {
            continue;
        }
        let Some(committed_at) = surface.committed_at else {
            continue;
        };
        if !has_ipc_clients || committed_at.elapsed() >= map_timeout {
            surface.visible = true;
        }
    }
}

pub fn update_world_matrices(world: &mut World) {
    // First pass: Compute local matrices
    for (_entity, transform) in world.query_mut::<&mut Transform>() {
        let translation = cgmath::Matrix4::from_translation(transform.translation);
        let rotation = cgmath::Matrix4::from(transform.rotation);
        let scale = cgmath::Matrix4::from_nonuniform_scale(
            transform.scale.x,
            transform.scale.y,
            transform.scale.z,
        );
        transform.world_matrix = translation * rotation * scale;
    }

    // Second pass: Apply parent transforms
    // Collect updates first to avoid multiple mutable borrows
    let mut updates = Vec::new();
    for (entity, (transform, parent)) in world.query::<(&Transform, &Parent)>().iter() {
        if let Ok(parent_transform) = world.get::<&Transform>(parent.entity) {
            // Child's world matrix = Parent's world matrix * Child's local matrix
            let new_world_matrix = parent_transform.world_matrix * transform.world_matrix;
            updates.push((entity, new_world_matrix));
        }
    }

    // Apply updates
    for (entity, new_world_matrix) in updates {
        if let Ok(transform) = world.query_one_mut::<&mut Transform>(entity) {
            transform.world_matrix = new_world_matrix;
        }
    }
}
