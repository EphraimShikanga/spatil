use std::time::{Instant, SystemTime, UNIX_EPOCH};

use smithay::desktop::layer_map_for_output;
use smithay::utils::{Logical, Point, Rectangle};

use crate::renderer::orb::{Corner, OrbFrame};
use crate::scene::animation::spring_step;
use crate::state::SpatilState;

#[derive(Default, Clone, Copy)]
struct Eased {
    value: f32,
    velocity: f32,
}

impl Eased {
    fn tick(&mut self, target: f32, stiffness: f32, damping: f32, dt: f32) {
        let (value, velocity) =
            spring_step(self.value, self.velocity, target, stiffness, damping, dt);
        self.value = value.clamp(-0.2, 1.2);
        self.velocity = velocity;
    }
}

#[derive(Default)]
pub struct OrbRuntime {
    open: Eased,
    hover: Eased,
    ambient: Eased,
    expanded: bool,
    hovered: bool,
}

fn seconds_of_day() -> f32 {
    let Ok(since_epoch) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return 0.0;
    };
    let secs = since_epoch.as_secs();
    let offset = local_utc_offset_seconds();
    let local = (secs as i64 + offset).rem_euclid(86_400) as f32;
    local + since_epoch.subsec_nanos() as f32 / 1e9
}

fn local_utc_offset_seconds() -> i64 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff
}

fn corner_rect(
    zone: Rectangle<i32, Logical>,
    corner: Corner,
    size: f32,
    margin: f32,
) -> Option<Rectangle<i32, Logical>> {
    if zone.size.w <= 0 || zone.size.h <= 0 {
        return None;
    }
    let size = size.round().max(1.0) as i32;
    let margin = margin.round().max(0.0) as i32;
    if size + margin * 2 > zone.size.w || size + margin * 2 > zone.size.h {
        return None;
    }

    let (left, top) = (zone.loc.x + margin, zone.loc.y + margin);
    let right = zone.loc.x + zone.size.w - margin - size;
    let bottom = zone.loc.y + zone.size.h - margin - size;
    let loc = match corner {
        Corner::TopLeft => (left, top),
        Corner::TopRight => (right, top),
        Corner::BottomLeft => (left, bottom),
        Corner::BottomRight => (right, bottom),
    };
    Some(Rectangle::new(loc.into(), (size, size).into()))
}

impl SpatilState {
    pub fn note_input(&mut self) {
        self.last_input = Instant::now();
    }

    pub fn tick_orb(&mut self, dt_ms: f32) {
        let params = &self.wgpu_state.orb;
        if !params.enabled {
            self.wgpu_state.orb_frame = None;
            return;
        }

        let idle = self.last_input.elapsed().as_secs_f32() * 1000.0 >= params.idle_ms;
        let (stiffness, damping) = (
            self.config.animation.spring_stiffness,
            self.config.animation.spring_damping,
        );
        let dt = dt_ms / 1000.0;

        let (hovered, expanded) = (self.orb.hovered, self.orb.expanded);
        self.orb.hover.tick(hovered as u8 as f32, stiffness, damping, dt);
        self.orb.open.tick(expanded as u8 as f32, stiffness, damping, dt);
        let ambient_target = (idle && !hovered && !expanded) as u8 as f32;
        self.orb.ambient.tick(ambient_target, stiffness, damping, dt);

        let Some(rect) = self.orb_rect() else {
            self.wgpu_state.orb_frame = None;
            return;
        };
        self.wgpu_state.orb_frame = Some(OrbFrame {
            rect: [
                rect.loc.x as f32,
                rect.loc.y as f32,
                rect.size.w as f32,
                rect.size.h as f32,
            ],
            seconds_of_day: seconds_of_day(),
            open: self.orb.open.value.clamp(0.0, 1.0),
            hover: self.orb.hover.value.clamp(0.0, 1.0),
            ambient: self.orb.ambient.value.clamp(0.0, 1.0),
        });
    }

    pub fn orb_rect(&self) -> Option<Rectangle<i32, Logical>> {
        let params = &self.wgpu_state.orb;
        if !params.enabled {
            return None;
        }
        let zone = layer_map_for_output(&self.output).non_exclusive_zone();
        corner_rect(zone, params.corner, params.size, params.margin)
    }

    pub fn orb_contains(&self, point: Point<f64, Logical>) -> bool {
        let Some(rect) = self.orb_rect() else {
            return false;
        };
        let radius = rect.size.w as f64 * 0.5;
        let cx = rect.loc.x as f64 + radius;
        let cy = rect.loc.y as f64 + radius;
        let (dx, dy) = (point.x - cx, point.y - cy);
        dx * dx + dy * dy <= radius * radius
    }

    pub fn set_orb_hover(&mut self, hovered: bool) {
        self.orb.hovered = hovered;
    }

    pub fn orb_report(&self) -> serde_json::Value {
        let params = &self.wgpu_state.orb;
        let rect = self.orb_rect();
        serde_json::json!({
            "enabled": params.enabled,
            "visible": self.wgpu_state.orb_frame.is_some(),
            "rect": rect.map(|r| [r.loc.x, r.loc.y, r.size.w, r.size.h]),
            "idle_ms": params.idle_ms,
            "idle_for_ms": self.last_input.elapsed().as_millis() as u64,
            "hovered": self.orb.hovered,
            "expanded": self.orb.expanded,
            "hover": self.orb.hover.value,
            "open": self.orb.open.value,
            "ambient": self.orb.ambient.value,
            "seconds_of_day": seconds_of_day(),
        })
    }

    pub fn orb_press(&mut self, point: Point<f64, Logical>) -> bool {
        if !self.orb_contains(point) {
            self.orb.expanded = false;
            return false;
        }
        self.orb.expanded = !self.orb.expanded;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn each_corner_insets_from_its_own_two_edges() {
        let screen = zone(0, 0, 1920, 1080);
        let at = |c| corner_rect(screen, c, 56.0, 20.0).expect("fits");

        assert_eq!(at(Corner::TopLeft).loc, (20, 20).into());
        assert_eq!(at(Corner::TopRight).loc, (1920 - 20 - 56, 20).into());
        assert_eq!(at(Corner::BottomLeft).loc, (20, 1080 - 20 - 56).into());
        assert_eq!(
            at(Corner::BottomRight).loc,
            (1920 - 20 - 56, 1080 - 20 - 56).into()
        );
        assert_eq!(at(Corner::TopLeft).size, (56, 56).into());
    }

    #[test]
    fn an_exclusive_zone_pushes_the_orb_out_of_a_bar() {
        let free = zone(0, 40, 1920, 1040);
        let top = corner_rect(free, Corner::TopRight, 56.0, 20.0).expect("fits");
        assert_eq!(top.loc.y, 60, "the orb must clear the bar, not overlap it");

        let bottom = corner_rect(free, Corner::BottomRight, 56.0, 20.0).expect("fits");
        assert_eq!(bottom.loc.y, 40 + 1040 - 20 - 56);
    }

    #[test]
    fn an_area_too_small_for_the_orb_yields_nothing() {
        assert!(corner_rect(zone(0, 0, 80, 800), Corner::TopLeft, 56.0, 20.0).is_none());
        assert!(corner_rect(zone(0, 0, 800, 80), Corner::TopLeft, 56.0, 20.0).is_none());
        assert!(corner_rect(zone(0, 0, 0, 0), Corner::TopLeft, 56.0, 20.0).is_none());
    }

    #[test]
    fn corner_names_parse_case_and_separator_insensitively() {
        assert_eq!(Corner::parse("bottom_right"), Some(Corner::BottomRight));
        assert_eq!(Corner::parse("Bottom-Right"), Some(Corner::BottomRight));
        assert_eq!(Corner::parse(" TOPLEFT "), Some(Corner::TopLeft));
        assert_eq!(Corner::parse("middle"), None);
    }

    #[test]
    fn a_spring_scalar_converges_on_its_target() {
        let mut eased = Eased::default();
        for _ in 0..600 {
            eased.tick(1.0, 220.0, 22.0, 1.0 / 60.0);
        }
        assert!((eased.value - 1.0).abs() < 0.01, "settled at {}", eased.value);

        for _ in 0..600 {
            eased.tick(0.0, 220.0, 22.0, 1.0 / 60.0);
        }
        assert!(eased.value.abs() < 0.01, "settled at {}", eased.value);
    }
}
