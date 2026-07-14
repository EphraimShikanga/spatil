//! Tween-based animation: eased `Transform`/opacity interpolation + the scene-switch timer.

use cgmath::{Quaternion, Vector3};
use hecs::{Entity, World};
use smithay::reexports::wayland_server::backend::ObjectId;

use crate::scene::components::{Transform, WaylandSurface};
use crate::scene::mode::SceneMode;

#[derive(Debug, Clone, Copy)]
pub enum Easing {
    Linear,
    EaseOutCubic,
    EaseInOutCubic,
    EaseOutExpo,
}

impl Easing {
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Easing::EaseOutExpo => {
                if t >= 1.0 {
                    1.0
                } else {
                    1.0 - 2.0_f32.powf(-10.0 * t)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AnimSnapshot {
    pub translation: Vector3<f32>,
    pub rotation: Quaternion<f32>,
    pub scale: Vector3<f32>,
    pub opacity: f32,
}

impl AnimSnapshot {
    pub fn of(t: &Transform) -> Self {
        Self {
            translation: t.translation,
            rotation: t.rotation,
            scale: t.scale,
            opacity: t.opacity,
        }
    }

    pub fn apply_to(&self, t: &mut Transform) {
        t.translation = self.translation;
        t.rotation = self.rotation;
        t.scale = self.scale;
        t.opacity = self.opacity;
    }

    fn lerp(&self, other: &Self, k: f32) -> Self {
        Self {
            translation: self.translation + (other.translation - self.translation) * k,
            rotation: self.rotation.slerp(other.rotation, k),
            scale: self.scale + (other.scale - self.scale) * k,
            opacity: self.opacity + (other.opacity - self.opacity) * k,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum OnComplete {
    Remove,
    Despawn,
}

pub struct Tween {
    pub start: AnimSnapshot,
    pub end: AnimSnapshot,
    pub elapsed: f32,
    pub duration_ms: f32,
    pub easing: Easing,
    pub on_complete: OnComplete,
}

impl Tween {
    pub fn new(start: AnimSnapshot, end: AnimSnapshot, duration_ms: f32, easing: Easing, on_complete: OnComplete) -> Self {
        Self { start, end, elapsed: 0.0, duration_ms: duration_ms.max(1.0), easing, on_complete }
    }
}

/// Returns the surface ids of windows despawned by a finished close animation,
/// so the caller can release their GPU textures.
pub fn update(world: &mut World, dt_ms: f32) -> Vec<ObjectId> {
    let mut remove = Vec::new();
    let mut despawn = Vec::new();
    for (entity, (transform, tween)) in world.query_mut::<(&mut Transform, &mut Tween)>() {
        tween.elapsed += dt_ms;
        let raw = (tween.elapsed / tween.duration_ms).clamp(0.0, 1.0);
        let k = tween.easing.apply(raw);
        tween.start.lerp(&tween.end, k).apply_to(transform);
        if raw >= 1.0 {
            match tween.on_complete {
                OnComplete::Remove => remove.push(entity),
                OnComplete::Despawn => despawn.push(entity),
            }
        }
    }

    for e in remove {
        let _ = world.remove_one::<Tween>(e);
    }
    let mut removed_ids = Vec::new();
    for e in despawn {
        if let Ok(surface) = world.get::<&WaylandSurface>(e) {
            removed_ids.push(surface.id.clone());
        }
        let _ = world.despawn(e);
    }
    removed_ids
}

pub fn open_start(mode: SceneMode, resting: &AnimSnapshot) -> AnimSnapshot {
    let mut s = *resting;
    s.opacity = 0.0;
    match mode {
        SceneMode::Aether => {
            s.scale = resting.scale * 0.8;
            s.translation.z -= 0.4;
        }
        SceneMode::Strata => {
            s.rotation = flat_on_floor();
        }
    }
    s
}

pub fn close_end(mode: SceneMode, resting: &AnimSnapshot) -> AnimSnapshot {
    let mut s = *resting;
    s.opacity = 0.0;
    match mode {
        SceneMode::Aether => {
            s.scale = resting.scale * 0.85;
            s.translation.z -= 0.6;
        }
        SceneMode::Strata => {
            s.scale = resting.scale * 0.9;
            s.rotation = flat_on_floor();
        }
    }
    s
}

fn flat_on_floor() -> Quaternion<f32> {
    use cgmath::{Deg, Rotation3};
    Quaternion::from_angle_x(Deg(-90.0))
}

pub struct SceneTransition {
    pub from: SceneMode,
    pub to: SceneMode,
    pub elapsed_ms: f32,
    pub duration_ms: f32,
    pub easing: Easing,
    pub active: bool,
}

impl SceneTransition {
    pub fn idle() -> Self {
        Self {
            from: SceneMode::Strata,
            to: SceneMode::Strata,
            elapsed_ms: 0.0,
            duration_ms: 1.0,
            easing: Easing::EaseOutExpo,
            active: false,
        }
    }

    pub fn start(&mut self, from: SceneMode, to: SceneMode, duration_ms: f32) {
        self.from = from;
        self.to = to;
        self.elapsed_ms = 0.0;
        self.duration_ms = duration_ms.max(1.0);
        self.easing = Easing::EaseOutExpo;
        self.active = true;
    }

    pub fn tick(&mut self, dt_ms: f32) {
        if self.active {
            self.elapsed_ms += dt_ms;
            if self.elapsed_ms >= self.duration_ms {
                self.active = false;
            }
        }
    }

    pub fn eased(&self) -> f32 {
        self.easing.apply(self.elapsed_ms / self.duration_ms)
    }
}

pub fn tween_entity(
    world: &mut World,
    entity: Entity,
    start: AnimSnapshot,
    end: AnimSnapshot,
    duration_ms: f32,
    easing: Easing,
    on_complete: OnComplete,
) {
    let _ = world.insert_one(entity, Tween::new(start, end, duration_ms, easing, on_complete));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints_and_monotonic() {
        for e in [Easing::Linear, Easing::EaseOutCubic, Easing::EaseInOutCubic, Easing::EaseOutExpo] {
            assert!((e.apply(0.0) - 0.0).abs() < 1e-4, "{e:?} at 0");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-4, "{e:?} at 1");
            // out-of-range clamps
            assert_eq!(e.apply(-1.0), e.apply(0.0));
            assert_eq!(e.apply(2.0), e.apply(1.0));
            // ease-out curves lead ahead of linear at the midpoint
        }
        assert!(Easing::EaseOutExpo.apply(0.5) > 0.5);
        assert!(Easing::EaseOutCubic.apply(0.5) > 0.5);
    }

    #[test]
    fn snapshot_lerp_interpolates_linearly() {
        let a = AnimSnapshot {
            translation: Vector3::new(0.0, 0.0, 0.0),
            rotation: Quaternion::new(1.0, 0.0, 0.0, 0.0),
            scale: Vector3::new(1.0, 1.0, 1.0),
            opacity: 0.0,
        };
        let mut b = a;
        b.translation = Vector3::new(2.0, 4.0, 6.0);
        b.opacity = 1.0;
        let m = a.lerp(&b, 0.5);
        assert_eq!(m.translation, Vector3::new(1.0, 2.0, 3.0));
        assert!((m.opacity - 0.5).abs() < 1e-6);
    }
}
