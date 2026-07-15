use std::time::Duration;

use cgmath::Vector3;
use hecs::{Entity, World};

use crate::config::Direction;
use crate::renderer::pipeline::Camera;
use crate::scene::animation::Tween;
use crate::scene::components::{Billboard, Parent, Transform, WaylandSurface};
use crate::scene::mode::billboard_rotation;

pub fn strata_window_placement(window_id: u32) -> Vector3<f32> {
    const COLS: u32 = 3;
    const SPACING_X: f32 = 0.9;
    const ROW_UP: f32 = 0.5;
    const ROW_BACK: f32 = 0.6;
    let index = window_id.saturating_sub(1);
    let col = (index % COLS) as f32;
    let row = (index / COLS) as f32;
    let x = (col - (COLS as f32 - 1.0) / 2.0) * SPACING_X;
    Vector3::new(x, row * ROW_UP, -row * ROW_BACK)
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

pub fn update_map_visibility(
    world: &mut World,
    has_ipc_clients: bool,
    map_timeout: Duration,
) -> Vec<Entity> {
    let mut revealed = Vec::new();
    for (entity, surface) in world.query_mut::<&mut WaylandSurface>() {
        if surface.visible {
            continue;
        }
        let Some(committed_at) = surface.committed_at else {
            continue;
        };
        if !has_ipc_clients || committed_at.elapsed() >= map_timeout {
            surface.visible = true;
            revealed.push(entity);
        }
    }
    revealed
}

/// The visible window nearest `from` in `direction`, judged on the view plane.
pub fn focus_in_direction(
    world: &World,
    camera: &Camera,
    from: Entity,
    direction: Direction,
) -> Option<Entity> {
    let Ok(origin) = world.get::<&Transform>(from).map(|t| t.translation) else {
        return None;
    };
    let view = camera.build_view_matrix();
    let project = |p: Vector3<f32>| {
        let v = view * p.extend(1.0);
        (v.x, v.y)
    };

    let candidates: Vec<_> = world
        .query::<(&Transform, &WaylandSurface)>()
        .iter()
        .filter(|(entity, (_, surface))| {
            *entity != from && surface.visible && !surface.closing
        })
        .map(|(entity, (transform, _))| (entity, project(transform.translation)))
        .collect();

    pick_in_direction(project(origin), &candidates, direction)
}

fn pick_in_direction(
    origin: (f32, f32),
    candidates: &[(Entity, (f32, f32))],
    direction: Direction,
) -> Option<Entity> {
    /// How many times more a unit sideways costs than a unit ahead.
    const OFF_AXIS_PENALTY: f32 = 2.0;
    /// Roughly a 70-degree cone: anything wider is not in that direction at all.
    const MIN_ALIGNMENT: f32 = 0.3;

    let (ox, oy) = origin;
    let (dx, dy) = direction.screen_step();

    let mut forward: Option<(f32, Entity)> = None;
    let mut wrapped: Option<(f32, Entity)> = None;
    for &(entity, (x, y)) in candidates {
        let (rx, ry) = (x - ox, y - oy);
        let distance = (rx * rx + ry * ry).sqrt();
        if distance < 1e-4 {
            continue;
        }
        // The axis is a unit vector, so the dot is the signed distance along it and
        // the cross magnitude the distance across
        let along = rx * dx + ry * dy;
        let across = (rx * dy - ry * dx).abs();
        let alignment = along / distance;
        if alignment > MIN_ALIGNMENT {
            let score = along + across * OFF_AXIS_PENALTY;
            if forward.is_none_or(|(best, _)| score < best) {
                forward = Some((score, entity));
            }
        } else if alignment < -MIN_ALIGNMENT && wrapped.is_none_or(|(best, _)| -along > best) {
            wrapped = Some((-along, entity));
        }
    }

    forward.or(wrapped).map(|(_, entity)| entity)
}

pub fn update_world_matrices(world: &mut World, camera: &Camera) {
    let facing = billboard_rotation(camera.eye, camera.target);

    // First pass: Compute local matrices
    for (_entity, (transform, billboard, tween)) in
        world.query_mut::<(&mut Transform, Option<&Billboard>, Option<&Tween>)>()
    {
        if billboard.is_some() && tween.is_none() {
            transform.rotation = facing;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> (World, Vec<Entity>) {
        let mut world = World::new();
        // origin, right, far-right, up, left
        let positions = [(0.0, 0.0), (1.0, 0.0), (5.0, 0.0), (0.0, 1.0), (-1.0, 0.0)];
        let entities = positions.iter().map(|_| world.spawn(())).collect();
        (world, entities)
    }

    fn at(entities: &[Entity], coords: &[(f32, f32)]) -> Vec<(Entity, (f32, f32))> {
        entities[1..].iter().copied().zip(coords[1..].iter().copied()).collect()
    }

    const COORDS: [(f32, f32); 5] =
        [(0.0, 0.0), (1.0, 0.0), (5.0, 0.0), (0.0, 1.0), (-1.0, 0.0)];

    #[test]
    fn picks_the_nearest_window_on_the_requested_axis() {
        let (_world, e) = scene();
        let candidates = at(&e, &COORDS);
        assert_eq!(pick_in_direction(COORDS[0], &candidates, Direction::Right), Some(e[1]));
        assert_eq!(pick_in_direction(COORDS[0], &candidates, Direction::Up), Some(e[3]));
        assert_eq!(pick_in_direction(COORDS[0], &candidates, Direction::Left), Some(e[4]));
    }

    #[test]
    fn straight_ahead_beats_nearer_but_off_axis() {
        let (mut world, _) = scene();
        let ahead = world.spawn(());
        let sideways = world.spawn(());
        // `sideways` is closer in a straight line but almost 45 degrees off.
        let candidates = [(ahead, (3.0, 0.0)), (sideways, (1.4, 1.4))];
        assert_eq!(pick_in_direction((0.0, 0.0), &candidates, Direction::Right), Some(ahead));
    }

    #[test]
    fn wraps_around_when_nothing_lies_that_way() {
        let (_world, e) = scene();
        // Everything is to the right of the far-right window, so going right from
        // it wraps to the furthest window the other way rather than dead-ending.
        let candidates: Vec<_> = [0, 1, 3, 4]
            .iter()
            .map(|&i| (e[i], COORDS[i]))
            .collect();
        assert_eq!(pick_in_direction(COORDS[2], &candidates, Direction::Right), Some(e[4]));
    }

    #[test]
    fn an_empty_scene_has_nothing_to_focus() {
        assert_eq!(pick_in_direction((0.0, 0.0), &[], Direction::Left), None);
    }
}
