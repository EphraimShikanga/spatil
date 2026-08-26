use std::time::{Instant, SystemTime, UNIX_EPOCH};

use smithay::desktop::layer_map_for_output;
use smithay::utils::{Logical, Point, Rectangle};

use crate::renderer::cursor::CursorFrame;
use crate::renderer::orb::{Corner, OrbFrame, SECOND_HAND};
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

const TAU: f32 = std::f32::consts::TAU;
const CURSOR_SIZE: f32 = 13.0;
const HANDOFF: f32 = 0.72;

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ramp(v: f32, lo: f32, hi: f32) -> f32 {
    ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    pub cursor_pos: (f32, f32),
    pub arrow_alpha: f32,
    pub arrow_angle: f32,
    pub arrow_shrink: f32,
    pub hand: f32,
    pub hide_real_cursor: bool,
}

fn flight_pose(
    ambient: f32,
    pointer: (f32, f32),
    orb_centre: (f32, f32),
    orb_radius: f32,
    second_angle: f32,
) -> Flight {
    let a = ambient.clamp(0.0, 1.0);
    let travel = ease(ramp(a, 0.0, HANDOFF));
    let hand = ease(ramp(a, HANDOFF, 1.0));

    let reach = SECOND_HAND * orb_radius;
    let tip = (
        orb_centre.0 + second_angle.sin() * reach,
        orb_centre.1 - second_angle.cos() * reach,
    );

    let shortest = if second_angle > std::f32::consts::PI {
        second_angle - TAU
    } else {
        second_angle
    };

    Flight {
        cursor_pos: (
            pointer.0 + (tip.0 - pointer.0) * travel,
            pointer.1 + (tip.1 - pointer.1) * travel,
        ),
        arrow_alpha: 1.0 - hand,
        arrow_angle: shortest * travel,
        arrow_shrink: 1.0 - 0.45 * hand,
        hand,
        hide_real_cursor: a > 0.001,
    }
}

impl SpatilState {
    pub fn note_input(&mut self) {
        self.last_input = Instant::now();
    }

    pub fn tick_orb(&mut self, dt_ms: f32) {
        let params = &self.wgpu_state.orb;
        if !params.enabled {
            self.clear_orb();
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
            self.clear_orb();
            return;
        };

        let secs = seconds_of_day();
        let ambient = self.orb.ambient.value.clamp(0.0, 1.0);
        let radius = rect.size.w as f32 * 0.5;
        let flight = flight_pose(
            ambient,
            (
                self.pointer_location.x as f32,
                self.pointer_location.y as f32,
            ),
            (rect.loc.x as f32 + radius, rect.loc.y as f32 + radius),
            radius,
            TAU * (secs / 60.0).fract(),
        );

        self.wgpu_state.orb_frame = Some(OrbFrame {
            rect: [
                rect.loc.x as f32,
                rect.loc.y as f32,
                rect.size.w as f32,
                rect.size.h as f32,
            ],
            seconds_of_day: secs,
            open: self.orb.open.value.clamp(0.0, 1.0),
            hover: self.orb.hover.value.clamp(0.0, 1.0),
            ambient,
            hand: flight.hand,
        });

        self.cursor_hidden = flight.hide_real_cursor;
        self.wgpu_state.cursor_frame = (flight.arrow_alpha > 0.002
            && flight.hide_real_cursor)
            .then(|| CursorFrame {
                rect: [
                    flight.cursor_pos.0 - CURSOR_SIZE,
                    flight.cursor_pos.1 - CURSOR_SIZE,
                    CURSOR_SIZE * 2.0,
                    CURSOR_SIZE * 2.0,
                ],
                alpha: flight.arrow_alpha,
                angle: flight.arrow_angle,
                shrink: flight.arrow_shrink,
            });
    }

    fn clear_orb(&mut self) {
        self.wgpu_state.orb_frame = None;
        self.wgpu_state.cursor_frame = None;
        self.cursor_hidden = false;
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
            "hand": self.wgpu_state.orb_frame.as_ref().map_or(0.0, |f| f.hand),
            "cursor_hidden": self.cursor_hidden,
            "cursor_flying": self.wgpu_state.cursor_frame.is_some(),
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

    fn pose(ambient: f32) -> Flight {
        flight_pose(
            ambient,
            (100.0, 100.0),
            (1000.0, 900.0),
            28.0,
            std::f32::consts::FRAC_PI_2,
        )
    }

    #[test]
    fn an_active_session_leaves_the_real_cursor_alone() {
        let f = pose(0.0);
        assert!(!f.hide_real_cursor, "the real cursor must stay visible when active");
        assert_eq!(f.hand, 0.0, "no second hand while the user is active");
        assert_eq!(f.cursor_pos, (100.0, 100.0), "the drawn arrow starts at the pointer");
    }

    #[test]
    fn going_idle_flies_the_cursor_to_the_second_hand_tip() {
        let start = pose(0.02);
        assert!(start.hide_real_cursor, "the real cursor hides as soon as the flight starts");
        let d0 = (start.cursor_pos.0 - 100.0).hypot(start.cursor_pos.1 - 100.0);
        assert!(d0 < 20.0, "the flight begins at the pointer, moved {d0}");

        let mid = pose(0.4);
        assert!(
            mid.cursor_pos.0 > start.cursor_pos.0 && mid.cursor_pos.1 > start.cursor_pos.1,
            "the arrow travels toward the orb",
        );

        let end = pose(1.0);
        let tip = (1000.0 + 0.86 * 28.0, 900.0);
        assert!((end.cursor_pos.0 - tip.0).abs() < 0.01, "{:?}", end.cursor_pos);
        assert!((end.cursor_pos.1 - tip.1).abs() < 0.01, "{:?}", end.cursor_pos);
        assert!(end.hand > 0.99, "hand {}", end.hand);
        assert!(end.arrow_alpha < 0.01, "the arrow must be gone once the hand is formed");
    }

    #[test]
    fn the_arrow_and_the_hand_never_both_show_fully() {
        for i in 0..=20 {
            let f = pose(i as f32 / 20.0);
            let total = f.arrow_alpha + f.hand;
            assert!((total - 1.0).abs() < 0.001, "at {i}: arrow+hand = {total}");
        }
    }

    #[test]
    fn the_arrow_turns_the_short_way_round() {
        let late = flight_pose(1.0, (0.0, 0.0), (500.0, 500.0), 28.0, TAU * 55.0 / 60.0);
        assert!(late.arrow_angle < 0.0, "angle {}", late.arrow_angle);
        assert!(late.arrow_angle.abs() < std::f32::consts::PI, "angle {}", late.arrow_angle);
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
