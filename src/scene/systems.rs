use std::collections::HashMap;
use std::time::Duration;

use cgmath::{Matrix4, Quaternion, Vector3};
use hecs::{Entity, World};

use crate::config::Direction;
use crate::renderer::pipeline::Camera;
use crate::scene::WORLD_SCALE;
use crate::scene::animation::Tween;
use crate::scene::components::{Billboard, Parent, Popup, Transform, WaylandSurface};
use crate::scene::mode::billboard_rotation;

const POPUP_LIFT: f32 = 0.02;

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
        .query::<(&Transform, &WaylandSurface, Option<&Popup>)>()
        .iter()
        .filter(|(entity, (_, surface, popup))| {
            *entity != from && surface.visible && !surface.closing && popup.is_none()
        })
        .map(|(entity, (transform, _, _))| (entity, project(transform.translation)))
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

fn local_matrix(transform: &Transform) -> Matrix4<f32> {
    let translation = Matrix4::from_translation(transform.translation);
    let rotation = Matrix4::from(transform.rotation);
    let scale = Matrix4::from_nonuniform_scale(
        transform.scale.x,
        transform.scale.y,
        transform.scale.z,
    );
    translation * rotation * scale
}

fn popup_pose(
    root_translation: Vector3<f32>,
    root_rotation: Quaternion<f32>,
    root_scale: Vector3<f32>,
    root_size: (u32, u32),
    size: (u32, u32),
    offset: (i32, i32),
    depth: u32,
) -> Option<(Vector3<f32>, Vector3<f32>)> {
    if root_size.0 == 0 || root_size.1 == 0 || size.0 == 0 || size.1 == 0 {
        return None;
    }
    let (root_w, root_h) = (root_size.0 as f32, root_size.1 as f32);
    let fx = (offset.0 as f32 + size.0 as f32 / 2.0) / root_w - 0.5;
    let fy = 0.5 - (offset.1 as f32 + size.1 as f32 / 2.0) / root_h;
    let magnification = root_scale.x * WORLD_SCALE / root_w;
    let local = Vector3::new(
        fx * root_scale.x,
        fy * root_scale.y,
        (depth + 1) as f32 * POPUP_LIFT,
    );
    Some((
        root_translation + root_rotation * local,
        Vector3::new(
            size.0 as f32 / WORLD_SCALE * magnification,
            size.1 as f32 / WORLD_SCALE * magnification,
            1.0,
        ),
    ))
}

fn pose_popups(world: &mut World) {
    let roots: HashMap<Entity, (Vector3<f32>, Quaternion<f32>, Vector3<f32>, (u32, u32), f32)> =
        world
            .query::<(&Transform, &WaylandSurface)>()
            .iter()
            .map(|(entity, (t, s))| {
                (entity, (t.translation, t.rotation, t.scale, s.size, t.opacity))
            })
            .collect();

    let popups: Vec<_> = world
        .query::<(&Popup, &WaylandSurface)>()
        .iter()
        .map(|(entity, (popup, surface))| {
            (entity, popup.root, popup.depth, popup.offset, surface.size)
        })
        .collect();

    for (entity, root, depth, offset, size) in popups {
        let Some(&(root_pos, root_rot, root_scale, root_size, root_opacity)) = roots.get(&root)
        else {
            continue;
        };
        let Some((translation, scale)) = popup_pose(
            root_pos, root_rot, root_scale, root_size, size, offset, depth,
        ) else {
            continue;
        };

        let Ok(transform) = world.query_one_mut::<&mut Transform>(entity) else {
            continue;
        };
        transform.translation = translation;
        transform.rotation = root_rot;
        transform.scale = scale;
        transform.opacity = root_opacity;
        let matrix = local_matrix(transform);
        transform.world_matrix = matrix;
    }
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
        let matrix = local_matrix(transform);
        transform.world_matrix = matrix;
    }

    pose_popups(world);

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
    use cgmath::{One, Rotation3};

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

    /// An 800x600 window whose quad is the buffer size in world units.
    fn root() -> (Vector3<f32>, Quaternion<f32>, Vector3<f32>, (u32, u32)) {
        (
            Vector3::new(0.0, 0.0, 0.0),
            Quaternion::one(),
            Vector3::new(0.8, 0.6, 1.0),
            (800, 600),
        )
    }

    fn pose(
        offset: (i32, i32),
        size: (u32, u32),
        depth: u32,
    ) -> (Vector3<f32>, Vector3<f32>) {
        let (translation, rotation, scale, root_size) = root();
        popup_pose(translation, rotation, scale, root_size, size, offset, depth)
            .expect("both sides have committed")
    }

    #[test]
    fn a_popup_lands_where_the_protocol_offset_puts_it_on_the_window() {
        // A 200x100 menu at the window's top-left corner: its centre sits a tenth of the
        // window in from the left edge and a twelfth down from the top.
        let (translation, scale) = pose((0, 0), (200, 100), 0);
        assert!((translation.x - (-0.3)).abs() < 1e-5, "{translation:?}");
        assert!((translation.y - 0.25).abs() < 1e-5, "{translation:?}");
        assert!((scale.x - 0.2).abs() < 1e-5, "{scale:?}");
        assert!((scale.y - 0.1).abs() < 1e-5, "{scale:?}");

        // Centred on the window: dead centre, whatever the popup's own size.
        let (centered, _) = pose((300, 250), (200, 100), 0);
        assert!(centered.x.abs() < 1e-5 && centered.y.abs() < 1e-5, "{centered:?}");
    }

    #[test]
    fn each_popup_level_stacks_further_toward_the_viewer() {
        let (menu, _) = pose((0, 0), (200, 100), 0);
        let (submenu, _) = pose((0, 0), (200, 100), 1);
        assert!(menu.z > 0.0, "a popup sits in front of its window, not in it");
        assert!(submenu.z > menu.z, "{submenu:?} should stack over {menu:?}");
    }

    #[test]
    fn a_popup_rides_the_window_through_a_tween() {
        // Mid open-animation the window is at half scale while its buffer is unchanged:
        // the popup must halve with it, and stay on the same spot of the pane.
        let (translation, rotation, scale, root_size) = root();
        let (full, full_scale) = pose((0, 0), (200, 100), 0);
        let (half, half_scale) = popup_pose(
            translation,
            rotation,
            scale / 2.0,
            root_size,
            (200, 100),
            (0, 0),
            0,
        )
        .expect("committed");
        assert!((half.x - full.x / 2.0).abs() < 1e-5, "{half:?} vs {full:?}");
        assert!((half.y - full.y / 2.0).abs() < 1e-5, "{half:?} vs {full:?}");
        assert!((half_scale.x - full_scale.x / 2.0).abs() < 1e-5);
        assert!((half_scale.y - full_scale.y / 2.0).abs() < 1e-5);
    }

    #[test]
    fn a_popup_turns_with_its_window() {
        // Yawed 90 degrees about Y, the pane's local +x becomes world -z, so an offset
        // that was to the left of the window is now behind it.
        let (translation, _, scale, root_size) = root();
        let rotation = Quaternion::from_angle_y(cgmath::Deg(90.0));
        let (posed, _) =
            popup_pose(translation, rotation, scale, root_size, (200, 100), (0, 0), 0)
                .expect("committed");
        assert!((posed.z - 0.3).abs() < 1e-5, "{posed:?}");
        assert!(posed.x.abs() < 0.1, "{posed:?}");
    }

    #[test]
    fn a_popup_has_no_pose_before_either_side_has_committed() {
        let (translation, rotation, scale, _) = root();
        assert!(
            popup_pose(translation, rotation, scale, (0, 0), (200, 100), (0, 0), 0).is_none()
        );
        assert!(
            popup_pose(translation, rotation, scale, (800, 600), (0, 0), (0, 0), 0).is_none()
        );
    }
}
