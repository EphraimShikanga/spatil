//! CSS timing functions and transitions, for the motions the design specifies exactly.

pub type Curve = [f32; 4];

pub const EASE: Curve = [0.25, 0.1, 0.25, 1.0];
pub const EASE_IN_OUT: Curve = [0.42, 0.0, 0.58, 1.0];
pub const EASE_OUT: Curve = [0.0, 0.0, 0.58, 1.0];

/// `cubic-bezier(x1, y1, x2, y2)` evaluated at time `t`.
pub fn cubic_bezier(curve: Curve, t: f32) -> f32 {
    let [x1, y1, x2, y2] = curve;
    let t = t.clamp(0.0, 1.0);
    let bez = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..28 {
        let mid = (lo + hi) * 0.5;
        if bez(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) * 0.5)
}

/// A CSS `transition` on one value: retargeting starts from wherever it is now.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tween {
    from: f32,
    to: f32,
    elapsed_ms: f32,
    duration_ms: f32,
    curve: Curve,
}

impl Tween {
    pub fn new(value: f32, duration_ms: f32, curve: Curve) -> Self {
        Self { from: value, to: value, elapsed_ms: duration_ms, duration_ms, curve }
    }

    pub fn set(&mut self, target: f32) {
        if (target - self.to).abs() > f32::EPSILON {
            self.from = self.value();
            self.to = target;
            self.elapsed_ms = 0.0;
        }
    }

    pub fn tick(&mut self, dt_ms: f32) {
        self.elapsed_ms = (self.elapsed_ms + dt_ms).min(self.duration_ms);
    }

    pub fn progress(&self) -> f32 {
        if self.duration_ms <= 0.0 { 1.0 } else { self.elapsed_ms / self.duration_ms }
    }

    pub fn value(&self) -> f32 {
        self.from + (self.to - self.from) * cubic_bezier(self.curve, self.progress())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_run_from_zero_to_one() {
        for curve in [EASE, EASE_IN_OUT, EASE_OUT, [0.2, 0.9, 0.25, 1.0]] {
            assert!(cubic_bezier(curve, 0.0).abs() < 1e-3);
            assert!((cubic_bezier(curve, 1.0) - 1.0).abs() < 1e-3);
        }
        assert!((cubic_bezier(EASE_IN_OUT, 0.5) - 0.5).abs() < 1e-3, "ease-in-out is symmetric");
    }

    #[test]
    fn a_retargeted_tween_starts_from_where_it_is() {
        let mut t = Tween::new(0.0, 400.0, EASE);
        t.set(1.0);
        t.tick(200.0);
        let mid = t.value();
        assert!(mid > 0.3 && mid < 1.0, "{mid}");
        t.set(0.0);
        assert!((t.value() - mid).abs() < 1e-6, "no jump on retarget");
        t.tick(400.0);
        assert_eq!(t.value(), 0.0);
    }
}
