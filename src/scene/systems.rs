use hecs::World;
use crate::scene::components::{Transform, Parent};

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
