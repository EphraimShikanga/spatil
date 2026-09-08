use std::time::{Instant, SystemTime, UNIX_EPOCH};

use smithay::desktop::layer_map_for_output;
use smithay::utils::{Logical, Point, Rectangle};

use crate::config::SessionState;
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
const TAP_SLOP: f64 = 6.0;
const SETTLED: f32 = 0.5;

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ramp(v: f32, lo: f32, hi: f32) -> f32 {
    ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
}

struct OrbDrag {
    grab: (f32, f32),
    press_at: Point<f64, Logical>,
    moved: bool,
}

#[derive(Default)]
pub struct OrbRuntime {
    open: Eased,
    hover: Eased,
    ambient: Eased,
    expanded: bool,
    hovered: bool,
    corner: Option<Corner>,
    drag: Option<OrbDrag>,
    pos: Option<(f32, f32)>,
    pos_velocity: (f32, f32),
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

fn nearest_corner(centre: (f32, f32), zone: Rectangle<i32, Logical>) -> Corner {
    let mid_x = zone.loc.x as f32 + zone.size.w as f32 * 0.5;
    let mid_y = zone.loc.y as f32 + zone.size.h as f32 * 0.5;
    match (centre.0 < mid_x, centre.1 < mid_y) {
        (true, true) => Corner::TopLeft,
        (false, true) => Corner::TopRight,
        (true, false) => Corner::BottomLeft,
        (false, false) => Corner::BottomRight,
    }
}

fn clamp_orb(pos: (f32, f32), size: f32, zone: Rectangle<i32, Logical>) -> (f32, f32) {
    let max_x = (zone.loc.x + zone.size.w) as f32 - size;
    let max_y = (zone.loc.y + zone.size.h) as f32 - size;
    (
        pos.0.clamp(zone.loc.x as f32, max_x.max(zone.loc.x as f32)),
        pos.1.clamp(zone.loc.y as f32, max_y.max(zone.loc.y as f32)),
    )
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

        self.settle_orb(dt);
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

    fn settle_orb(&mut self, dt: f32) {
        if self.orb.drag.is_some() {
            return;
        }
        let (Some(pos), Some(resting)) = (self.orb.pos, self.orb_resting_rect()) else {
            return;
        };
        let target = (resting.loc.x as f32, resting.loc.y as f32);
        let (stiffness, damping) = (
            self.config.animation.spring_stiffness,
            self.config.animation.spring_damping,
        );
        let (x, vx) = spring_step(pos.0, self.orb.pos_velocity.0, target.0, stiffness, damping, dt);
        let (y, vy) = spring_step(pos.1, self.orb.pos_velocity.1, target.1, stiffness, damping, dt);

        let settled = (x - target.0).abs() < SETTLED
            && (y - target.1).abs() < SETTLED
            && vx.abs() < SETTLED * 10.0
            && vy.abs() < SETTLED * 10.0;
        if settled {
            self.orb.pos = None;
            self.orb.pos_velocity = (0.0, 0.0);
        } else {
            self.orb.pos = Some((x, y));
            self.orb.pos_velocity = (vx, vy);
        }
    }

    fn clear_orb(&mut self) {
        self.wgpu_state.orb_frame = None;
        self.wgpu_state.cursor_frame = None;
        self.cursor_hidden = false;
    }

    pub fn orb_corner(&self) -> Corner {
        self.orb.corner.unwrap_or(self.wgpu_state.orb.corner)
    }

    pub fn orb_zone(&self) -> Rectangle<i32, Logical> {
        layer_map_for_output(&self.output).non_exclusive_zone()
    }

    fn orb_resting_rect(&self) -> Option<Rectangle<i32, Logical>> {
        let params = &self.wgpu_state.orb;
        if !params.enabled {
            return None;
        }
        corner_rect(self.orb_zone(), self.orb_corner(), params.size, params.margin)
    }

    pub fn orb_rect(&self) -> Option<Rectangle<i32, Logical>> {
        let resting = self.orb_resting_rect()?;
        let Some(pos) = self.orb.pos else {
            return Some(resting);
        };
        Some(Rectangle::new(
            (pos.0.round() as i32, pos.1.round() as i32).into(),
            resting.size,
        ))
    }

    pub fn set_orb_corner(&mut self, corner: Corner) {
        if self.orb_corner() == corner && self.orb.corner.is_some() {
            return;
        }
        self.orb.corner = Some(corner);
        let mut state = SessionState::load();
        state.shell.familiar_corner = Some(corner.as_str().to_string());
        state.save();
        tracing::info!("familiar moved to {}", corner.as_str());
    }

    pub fn restore_orb_corner(&mut self, state: &SessionState) {
        let Some(name) = state.shell.familiar_corner.as_deref() else {
            return;
        };
        match Corner::parse(name) {
            Some(corner) => self.orb.corner = Some(corner),
            None => tracing::warn!("ignoring unknown saved familiar_corner {name:?}"),
        }
    }

    pub fn forget_orb_corner(&mut self) {
        if self.orb.corner.take().is_some() {
            let mut state = SessionState::load();
            state.shell.familiar_corner = None;
            state.save();
        }
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

    pub fn close_orb(&mut self) {
        self.orb.expanded = false;
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
            "corner": self.orb_corner().as_str(),
            "corner_pinned": self.orb.corner.is_some(),
            "dragging": self.orb.drag.is_some(),
            "settling": self.orb.pos.is_some() && self.orb.drag.is_none(),
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
        let Some(rect) = self.orb_rect() else {
            return false;
        };
        self.orb.drag = Some(OrbDrag {
            grab: (
                rect.loc.x as f32 - point.x as f32,
                rect.loc.y as f32 - point.y as f32,
            ),
            press_at: point,
            moved: false,
        });
        true
    }

    pub fn orb_dragging(&self) -> bool {
        self.orb.drag.is_some()
    }

    pub fn orb_drag_motion(&mut self, point: Point<f64, Logical>) -> bool {
        let Some(drag) = &self.orb.drag else {
            return false;
        };
        let (dx, dy) = (point.x - drag.press_at.x, point.y - drag.press_at.y);
        let moved = drag.moved || dx * dx + dy * dy > TAP_SLOP * TAP_SLOP;
        let target = (point.x as f32 + drag.grab.0, point.y as f32 + drag.grab.1);
        if !moved {
            return true;
        }

        let size = self.wgpu_state.orb.size;
        let zone = self.orb_zone();
        if let Some(drag) = &mut self.orb.drag {
            drag.moved = true;
        }
        self.orb.expanded = false;
        self.orb.pos = Some(clamp_orb(target, size, zone));
        self.orb.pos_velocity = (0.0, 0.0);
        true
    }

    pub fn orb_release(&mut self) -> bool {
        let Some(drag) = self.orb.drag.take() else {
            return false;
        };
        if !drag.moved {
            self.orb.expanded = !self.orb.expanded;
            return true;
        }
        if let Some(pos) = self.orb.pos {
            let half = self.wgpu_state.orb.size * 0.5;
            let centre = (pos.0 + half, pos.1 + half);
            let corner = nearest_corner(centre, self.orb_zone());
            self.set_orb_corner(corner);
        }
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
    fn a_dragged_orb_snaps_to_whichever_corner_it_is_nearest() {
        let screen = zone(0, 0, 1920, 1080);
        assert_eq!(nearest_corner((10.0, 10.0), screen), Corner::TopLeft);
        assert_eq!(nearest_corner((1900.0, 10.0), screen), Corner::TopRight);
        assert_eq!(nearest_corner((10.0, 1070.0), screen), Corner::BottomLeft);
        assert_eq!(nearest_corner((1900.0, 1070.0), screen), Corner::BottomRight);
        assert_eq!(
            nearest_corner((959.0, 539.0), screen),
            Corner::TopLeft,
            "just inside both midlines stays in the top-left quadrant",
        );
    }

    #[test]
    fn snapping_is_measured_against_the_free_area_not_the_screen() {
        let free = zone(0, 400, 1920, 680);
        assert_eq!(nearest_corner((100.0, 500.0), free), Corner::TopLeft);
        assert_eq!(nearest_corner((100.0, 1000.0), free), Corner::BottomLeft);
    }

    #[test]
    fn a_dragged_orb_cannot_leave_its_zone() {
        let free = zone(0, 40, 1920, 1040);
        assert_eq!(clamp_orb((-500.0, -500.0), 56.0, free), (0.0, 40.0));
        assert_eq!(
            clamp_orb((9000.0, 9000.0), 56.0, free),
            (1920.0 - 56.0, 40.0 + 1040.0 - 56.0),
        );
        assert_eq!(clamp_orb((300.0, 300.0), 56.0, free), (300.0, 300.0));
    }

    #[test]
    fn a_zone_smaller_than_the_orb_still_yields_a_position_inside_it() {
        let tiny = zone(10, 10, 20, 20);
        let (x, y) = clamp_orb((-100.0, -100.0), 56.0, tiny);
        assert_eq!((x, y), (10.0, 10.0), "clamping must not invert its own bounds");
    }

    #[test]
    fn corner_names_round_trip_through_their_saved_form() {
        for corner in [
            Corner::TopLeft,
            Corner::TopRight,
            Corner::BottomLeft,
            Corner::BottomRight,
        ] {
            assert_eq!(Corner::parse(corner.as_str()), Some(corner));
        }
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
