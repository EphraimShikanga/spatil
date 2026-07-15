use cgmath::{Vector3, Zero};

use crate::scene::animation::{Physics, spring_step};

const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 4.0;
/// Short of 90, where the orbit basis degenerates against the world up-axis.
const PITCH_LIMIT: f32 = 85.0;

#[derive(Debug, Clone, Copy)]
pub struct CameraController {
    pub orbit_yaw: f32,
    pub orbit_pitch: f32,
    pub pan: Vector3<f32>,
    pub zoom: f32,
    current: View,
    velocity: View,
}

#[derive(Debug, Clone, Copy)]
struct View {
    yaw: f32,
    pitch: f32,
    pan: Vector3<f32>,
    zoom: f32,
}

impl Default for CameraController {
    fn default() -> Self {
        let rest = View { yaw: 0.0, pitch: 0.0, pan: Vector3::zero(), zoom: 1.0 };
        Self {
            orbit_yaw: 0.0,
            orbit_pitch: 0.0,
            pan: Vector3::zero(),
            zoom: 1.0,
            current: rest,
            velocity: View { zoom: 0.0, ..rest },
        }
    }
}

impl CameraController {
    pub fn orbit(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.orbit_yaw += yaw_delta;
        self.orbit_pitch = (self.orbit_pitch + pitch_delta).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    pub fn pan_by(&mut self, delta: Vector3<f32>) {
        self.pan += delta;
    }

    pub fn look_at(&mut self, point: Vector3<f32>) {
        self.pan = point;
    }

    pub fn zoom_toward(&mut self, pivot: Vector3<f32>, steps: f32, speed: f32) {
        let requested = self.zoom * (1.0 - speed).powf(steps);
        let clamped = requested.clamp(ZOOM_MIN, ZOOM_MAX);
        let factor = clamped / self.zoom;
        self.zoom = clamped;
        self.pan = self.pan * factor + pivot * (1.0 - factor);

        // Once the distance bottoms out, the refused factor becomes travel rather
        // than a wall
        let residual = requested / clamped;
        if residual < 1.0 {
            self.pan = self.pan * residual + pivot * (1.0 - residual);
        }
    }

    pub fn zoom_by(&mut self, steps: f32, speed: f32) {
        self.zoom_toward(self.pan, steps, speed);
    }

    pub fn reset(&mut self) {
        self.orbit_yaw = 0.0;
        self.orbit_pitch = 0.0;
        self.pan = Vector3::zero();
        self.zoom = 1.0;
    }

    pub fn tick(&mut self, dt_ms: f32, physics: Physics) {
        let dt = (dt_ms / 1000.0).min(0.05);
        if dt <= 0.0 {
            return;
        }
        let (k, d) = (physics.spring_stiffness, physics.spring_damping);
        let step = |pos: &mut f32, vel: &mut f32, anchor: f32| {
            let (p, v) = spring_step(*pos, *vel, anchor, k, d, dt);
            *pos = p;
            *vel = v;
        };
        step(&mut self.current.yaw, &mut self.velocity.yaw, self.orbit_yaw);
        step(&mut self.current.pitch, &mut self.velocity.pitch, self.orbit_pitch);
        step(&mut self.current.zoom, &mut self.velocity.zoom, self.zoom);
        for axis in 0..3 {
            step(&mut self.current.pan[axis], &mut self.velocity.pan[axis], self.pan[axis]);
        }
    }

    pub fn smoothed(&self) -> (f32, f32, Vector3<f32>, f32) {
        (
            self.current.yaw,
            self.current.pitch,
            self.current.pan,
            self.current.zoom.clamp(ZOOM_MIN, ZOOM_MAX),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgmath::InnerSpace;

    fn physics() -> Physics {
        Physics { damping: 0.02, spring_stiffness: 220.0, spring_damping: 22.0 }
    }

    fn settle(c: &mut CameraController) {
        for _ in 0..600 {
            c.tick(16.0, physics());
        }
    }

    #[test]
    fn smoothing_converges_on_the_requested_view() {
        let mut c = CameraController::default();
        c.orbit(30.0, 10.0);
        c.pan_by(Vector3::new(0.5, 0.0, 0.0));
        // The request is instant; the view it frames with has to catch up.
        assert_eq!(c.smoothed().0, 0.0);

        settle(&mut c);
        let (yaw, pitch, pan, zoom) = c.smoothed();
        assert!((yaw - 30.0).abs() < 0.01, "yaw {yaw}");
        assert!((pitch - 10.0).abs() < 0.01, "pitch {pitch}");
        assert!((pan.x - 0.5).abs() < 0.01, "pan.x {}", pan.x);
        assert!((zoom - 1.0).abs() < 0.01, "zoom {zoom}");
    }

    #[test]
    fn zoom_is_geometric_clamped_and_reversible() {
        let mut c = CameraController::default();
        c.zoom_by(1.0, 0.1);
        assert!((c.zoom - 0.9).abs() < 1e-5, "one notch in: {}", c.zoom);
        c.zoom_by(-1.0, 0.1);
        assert!((c.zoom - 1.0).abs() < 1e-5, "back out: {}", c.zoom);

        for _ in 0..500 {
            c.zoom_by(1.0, 0.1);
        }
        assert_eq!(c.zoom, ZOOM_MIN);
        for _ in 0..1000 {
            c.zoom_by(-1.0, 0.1);
        }
        assert_eq!(c.zoom, ZOOM_MAX);
    }

    /// Where the eye ends up for a given controller state, mirroring how
    /// `scene::mode::frame_camera` builds it: target + direction × distance × zoom.
    fn eye(c: &CameraController, base_distance: f32, direction: Vector3<f32>) -> Vector3<f32> {
        c.pan + direction * (base_distance * c.zoom)
    }

    #[test]
    fn zooming_toward_a_pivot_keeps_that_pivot_fixed_on_screen() {
        let direction = Vector3::new(0.0, 0.0, 1.0);
        let base = 3.0;
        let pivot = Vector3::new(1.0, 0.5, 0.0);

        let mut c = CameraController::default();
        let before = eye(&c, base, direction);

        c.zoom_toward(pivot, 1.0, 0.1);
        let after = eye(&c, base, direction);

        // The defining property: eye_new == pivot + (eye_old - pivot) * factor.
        let factor = c.zoom;
        let expected = pivot + (before - pivot) * factor;
        assert!(
            (after - expected).magnitude() < 1e-5,
            "eye {after:?} should scale about the pivot to {expected:?}"
        );
        // And it actually travelled toward the pivot rather than straight in.
        assert!(c.pan.x > 0.0 && c.pan.y > 0.0, "pan should chase the pivot, got {:?}", c.pan);
    }

    #[test]
    fn zooming_toward_a_pivot_converges_on_it() {
        let mut c = CameraController::default();
        let pivot = Vector3::new(2.0, 0.0, -1.0);
        for _ in 0..200 {
            c.zoom_toward(pivot, 1.0, 0.1);
        }
        assert_eq!(c.zoom, ZOOM_MIN, "zoom should bottom out");
        assert!(
            (c.pan - pivot).magnitude() < 0.05,
            "scrolling in at a point should arrive at it, stopped at {:?}",
            c.pan
        );
    }

    #[test]
    fn scrolling_out_from_a_flown_to_point_keeps_looking_at_it() {
        let mut c = CameraController::default();
        let pivot = Vector3::new(2.0, 0.0, -1.0);
        for _ in 0..200 {
            c.zoom_toward(pivot, 1.0, 0.1);
        }
        // Having arrived, backing off must pull the distance out again without
        // abandoning the thing you flew to.
        for _ in 0..20 {
            c.zoom_toward(pivot, -1.0, 0.1);
        }
        assert!(c.zoom > 0.9, "distance should recover, zoom {}", c.zoom);
        assert!(
            (c.pan - pivot).magnitude() < 0.05,
            "backing off should keep the pivot framed, looking at {:?}",
            c.pan
        );
    }

    #[test]
    fn zooming_out_never_dollies_backwards_through_the_far_clamp() {
        let mut c = CameraController::default();
        let pivot = Vector3::new(5.0, 0.0, 0.0);
        for _ in 0..200 {
            c.zoom_toward(pivot, -1.0, 0.1);
        }
        assert_eq!(c.zoom, ZOOM_MAX);
        // Past the far clamp the residual is discarded, so the camera parks rather
        // than being flung geometrically into the void.
        assert!(
            c.pan.magnitude() < 100.0,
            "camera ran away to {:?} on repeated zoom-out",
            c.pan
        );
    }

    #[test]
    fn zooming_by_keyboard_pivots_on_the_view_centre() {
        let mut c = CameraController::default();
        c.pan_by(Vector3::new(1.0, 2.0, 3.0));
        let before = c.pan;
        c.zoom_by(1.0, 0.1);
        assert!((c.pan - before).magnitude() < 1e-6, "keyboard zoom must not pan");
        assert!((c.zoom - 0.9).abs() < 1e-5);
    }

    #[test]
    fn look_at_sends_the_camera_to_a_coordinate() {
        let mut c = CameraController::default();
        c.orbit(30.0, 10.0);
        c.look_at(Vector3::new(4.0, -1.0, 2.0));
        settle(&mut c);
        let (yaw, _, pan, _) = c.smoothed();
        assert!((pan - Vector3::new(4.0, -1.0, 2.0)).magnitude() < 0.01, "pan {pan:?}");
        // Going somewhere must not silently re-aim the camera.
        assert!((yaw - 30.0).abs() < 0.01, "look_at should keep the angles, yaw {yaw}");
    }

    #[test]
    fn pitch_is_clamped_short_of_the_poles() {
        let mut c = CameraController::default();
        c.orbit(0.0, 1000.0);
        assert_eq!(c.orbit_pitch, PITCH_LIMIT);
        c.orbit(0.0, -10_000.0);
        assert_eq!(c.orbit_pitch, -PITCH_LIMIT);
    }

    #[test]
    fn reset_eases_back_to_the_configured_framing() {
        let mut c = CameraController::default();
        c.orbit(45.0, 20.0);
        c.zoom_by(3.0, 0.1);
        settle(&mut c);

        c.reset();
        settle(&mut c);
        let (yaw, pitch, pan, zoom) = c.smoothed();
        assert!(yaw.abs() < 0.01 && pitch.abs() < 0.01);
        assert!(pan.magnitude() < 0.01);
        assert!((zoom - 1.0).abs() < 0.01);
    }
}
