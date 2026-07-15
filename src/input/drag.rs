use std::time::Instant;

use cgmath::{EuclideanSpace, InnerSpace, Vector3, Zero};
use hecs::Entity;
use smithay::utils::{Logical, Point};

use crate::config::{Direction, Modifier};
use crate::input::raycast::{self, raycast_entity};
use crate::scene::animation::{self, AnimSnapshot, Easing, OnComplete};
use crate::scene::components::{Spring, Transform, Velocity};
use crate::state::SpatilState;

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

/// How far toward the camera a grabbed window lifts, in world units.
const HOVER_LIFT: f32 = 0.06;

/// Anything faster is a stalled frame, not intent.
const MAX_THROW_SPEED: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    Plane,
    Depth,
}

pub struct Drag {
    pub kind: DragKind,
    pub entity: Entity,
    grab_offset: Vector3<f32>,
    plane_normal: Vector3<f32>,
    plane_point: Vector3<f32>,
    depth: f32,
    last_position: Vector3<f32>,
    last_cursor: Point<f64, Logical>,
    last_move: Instant,
    velocity: Vector3<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKind {
    Orbit,
    Pan,
}

pub struct Navigate {
    kind: NavKind,
    last_cursor: Point<f64, Logical>,
}

impl SpatilState {
    fn modifier_held(&self, modifier: Modifier) -> bool {
        self.seat
            .get_keyboard()
            .is_some_and(|keyboard| modifier.held(&keyboard.modifier_state()))
    }

    pub fn world_modifier_held(&self) -> bool {
        self.modifier_held(self.config.input.modifier())
    }

    pub fn world_is_modal(&self) -> bool {
        self.modal_window().is_some()
    }

    /// False when the press isn't ours, so the caller forwards it to the client.
    pub fn begin_drag(&mut self, button: u32, cursor: Point<f64, Logical>) -> bool {
        if !self.world_modifier_held() || self.world_is_modal() {
            return false;
        }

        if button == BTN_MIDDLE {
            let kind = if self.modifier_held(Modifier::Shift) {
                NavKind::Pan
            } else {
                NavKind::Orbit
            };
            self.navigate = Some(Navigate { kind, last_cursor: cursor });
            return true;
        }

        let kind = match button {
            BTN_LEFT => DragKind::Plane,
            BTN_RIGHT => DragKind::Depth,
            _ => return false,
        };
        let Some(hit) = raycast_entity(self, cursor) else {
            return false;
        };
        let Ok(position) = self.world.get::<&Transform>(hit.entity).map(|t| t.translation) else {
            return false;
        };

        let plane_normal = -self.wgpu_state.camera.forward();
        self.world.remove_one::<Velocity>(hit.entity).ok();
        self.world.remove_one::<Spring>(hit.entity).ok();

        self.drag = Some(Drag {
            kind,
            entity: hit.entity,
            grab_offset: position - hit.world_hit,
            plane_normal,
            plane_point: position,
            depth: 0.0,
            last_position: position,
            last_cursor: cursor,
            last_move: Instant::now(),
            velocity: Vector3::zero(),
        });
        self.lift_dragged_window();
        self.focus_entity(hit.entity);
        true
    }

    pub fn update_drag(&mut self, cursor: Point<f64, Logical>) -> bool {
        if let Some(nav) = &mut self.navigate {
            let (kind, delta) = (nav.kind, cursor - nav.last_cursor);
            nav.last_cursor = cursor;
            self.navigate_by(kind, delta.x as f32, delta.y as f32);
            return true;
        }

        let Some(drag) = &self.drag else {
            return false;
        };
        if drag.kind == DragKind::Depth {
            let dy = (cursor - drag.last_cursor).y as f32;
            let speed = self.config.input.drag_depth_speed;
            self.push_drag_depth(-dy * speed);
            if let Some(drag) = &mut self.drag {
                drag.last_cursor = cursor;
            }
            return true;
        }

        let (plane_point, plane_normal) = (drag.plane_point, drag.plane_normal);
        let Some(ray) = raycast::cursor_ray(self, cursor) else {
            return true;
        };
        let Some(hit) = ray.plane_hit(plane_point, plane_normal) else {
            return true;
        };

        let (entity, target) = {
            let drag = self.drag.as_mut().expect("checked above");
            let target = hit + drag.grab_offset + drag.plane_normal * (drag.depth + HOVER_LIFT);
            let now = Instant::now();
            let dt = now.duration_since(drag.last_move).as_secs_f32();
            if dt > 1e-4 {
                drag.velocity = (target - drag.last_position) / dt;
                drag.last_move = now;
            }
            drag.last_position = target;
            drag.last_cursor = cursor;
            (drag.entity, target)
        };

        if let Ok(transform) = self.world.query_one_mut::<&mut Transform>(entity) {
            transform.translation = target;
        }
        true
    }

    pub fn end_drag(&mut self) -> bool {
        if self.navigate.take().is_some() {
            return true;
        }
        let Some(drag) = self.drag.take() else {
            return false;
        };

        let anchor = drag.last_position - drag.plane_normal * HOVER_LIFT;
        let speed = drag.velocity.magnitude();
        let throw = if speed > MAX_THROW_SPEED {
            drag.velocity * (MAX_THROW_SPEED / speed)
        } else {
            drag.velocity
        };
        // The axis lock is what stops the spring cancelling the throw
        let _ = self.world.insert(
            drag.entity,
            (
                Velocity::linear(throw),
                Spring {
                    anchor,
                    axis: Some(drag.plane_normal),
                    stiffness: self.config.animation.spring_stiffness,
                    damping: self.config.animation.spring_damping,
                },
            ),
        );
        if !self.config.animation.enabled {
            let _ = self.world.remove_one::<Velocity>(drag.entity);
            let _ = self.world.remove_one::<Spring>(drag.entity);
            if let Ok(transform) = self.world.query_one_mut::<&mut Transform>(drag.entity) {
                transform.translation = anchor;
            }
        }
        self.remember_placement(drag.entity);
        true
    }

    /// Signed wheel notches: depth during a drag, zoom otherwise.
    pub fn wheel_depth_or_zoom(&mut self, steps: f32) -> bool {
        if !self.world_modifier_held() || self.world_is_modal() {
            return false;
        }
        let invert = if self.config.input.invert_scroll_zoom { -1.0 } else { 1.0 };
        if self.drag.is_some() {
            let speed = self.config.input.drag_depth_speed;
            self.push_drag_depth(steps * invert * speed);
        } else {
            let speed = self.config.input.zoom_speed;
            let pivot = self.cursor_pivot();
            self.wgpu_state
                .camera_controller
                .zoom_toward(pivot, steps * invert, speed);
        }
        true
    }

    /// The world point under the cursor: a window if the ray strikes one, else the
    /// plane through the current look-at
    fn cursor_pivot(&self) -> Vector3<f32> {
        let camera = &self.wgpu_state.camera;
        let fallback = camera.target.to_vec();
        if let Some(hit) = raycast_entity(self, self.pointer_location) {
            return hit.world_hit;
        }
        raycast::cursor_ray(self, self.pointer_location)
            .and_then(|ray| ray.plane_hit(fallback, -camera.forward()))
            .unwrap_or(fallback)
    }

    fn push_drag_depth(&mut self, amount: f32) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        drag.depth += amount;
        let target = drag.last_position + drag.plane_normal * amount;
        drag.last_position = target;
        let entity = drag.entity;
        if let Ok(transform) = self.world.query_one_mut::<&mut Transform>(entity) {
            transform.translation = target;
        }
    }

    fn lift_dragged_window(&mut self) {
        let Some(drag) = &self.drag else {
            return;
        };
        let (entity, lift) = (drag.entity, drag.plane_normal * HOVER_LIFT);
        if let Ok(transform) = self.world.query_one_mut::<&mut Transform>(entity) {
            transform.translation += lift;
        }
    }

    /// Both gestures work in both modes; both "grab the world" (the scene tracks
    /// the cursor).
    fn navigate_by(&mut self, kind: NavKind, dx: f32, dy: f32) {
        match kind {
            NavKind::Orbit => {
                // Speeds are radians per pixel; the framing takes degrees.
                let speed = self.config.input.orbit_speed.to_degrees();
                self.wgpu_state.camera_controller.orbit(-dx * speed, dy * speed);
            }
            NavKind::Pan => {
                let speed = self.config.input.pan_speed;
                self.pan_camera(-dx * speed, dy * speed);
            }
        }
    }

    fn pan_camera(&mut self, right_amount: f32, up_amount: f32) {
        let (right, up) = self.wgpu_state.camera.screen_axes();
        let delta = right * right_amount + up * up_amount;
        self.wgpu_state.camera_controller.pan_by(delta);
    }

    pub fn pan_view(&mut self, direction: Direction) {
        let (sx, sy) = direction.screen_step();
        let step = self.config.input.move_step;
        self.pan_camera(sx * step, sy * step);
    }

    pub fn move_focused_window(&mut self, direction: Direction) {
        if self.world_is_modal() {
            return;
        }
        let Some(entity) = self.focused_entity() else {
            return;
        };
        let (right, up) = self.wgpu_state.camera.screen_axes();
        let (sx, sy) = direction.screen_step();
        let step = (right * sx + up * sy) * self.config.input.move_step;

        let Ok(start) = self.world.get::<&Transform>(entity).map(|t| AnimSnapshot::of(&t)) else {
            return;
        };
        let mut end = start;
        end.translation = start.translation + step;

        if self.config.animation.enabled {
            let duration = self.config.animation.open_ms;
            animation::tween_entity(
                &mut self.world,
                entity,
                start,
                end,
                duration,
                Easing::EaseOutCubic,
                OnComplete::Remove,
            );
        } else if let Ok(transform) = self.world.query_one_mut::<&mut Transform>(entity) {
            end.apply_to(transform);
        }
        self.remember_placement(entity);
    }

    fn remember_placement(&mut self, entity: Entity) {
        let mode = self.config.scene_graph.scene_mode();
        self.remember_placement_in(entity, mode);
    }
}
