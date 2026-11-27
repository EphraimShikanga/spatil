//! The now-playing crown on the orb and the strip it draws out into on hover.

use std::sync::mpsc::Sender;

use smithay::utils::{Logical, Point};

use crate::config::parse_hex_color;
use crate::renderer::orb::Corner;
use crate::renderer::text::TextRun;
use crate::scene::animation::spring_step;
use crate::shell::card::{inside, with_alpha};
use crate::shell::mpris::{MprisRequest, NowPlaying, Playback};
use crate::shell::orb::Eased;
use crate::shell::tray::TrayPixmap;
use crate::state::SpatilState;
use crate::text::REGULAR;

const STRIP_W: f32 = 304.0;
const STRIP_H: f32 = 44.0;
const BUTTON: f32 = 28.0;
const ART: f32 = 32.0;
const VOLUME_STEP: f64 = 0.05;
/// CSS `line-height: normal` for Noto Sans, in ems.
const NORMAL: f32 = 1.362;

/// Everything the orb shader draws for the crown and its strip, in pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CrownLook {
    pub crown: [f32; 4],
    pub crown_art: [f32; 4],
    /// alpha, art present, crown radius, art radius
    pub crown_style: [f32; 4],
    pub strip: [f32; 4],
    /// alpha, drop shadow and top highlight, playing, strip radius
    pub strip_style: [f32; 4],
    pub strip_color: [f32; 4],
    pub strip_art: [f32; 4],
    /// pause/play centre, next centre
    pub glyphs: [f32; 4],
    /// x, y, width, played fraction
    pub line: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrownGeometry {
    pub crown: [f32; 4],
    pub crown_art: [f32; 4],
    pub strip: [f32; 4],
    /// The strip's fixed end, at the orb; it grows away from here.
    pub anchor: f32,
    pub grows_left: bool,
    pub art: [f32; 4],
    pub pause: [f32; 4],
    pub next: [f32; 4],
    pub text: [f32; 2],
    pub line: [f32; 3],
    pub in_card: bool,
}

/// Crown.dc.html: the crown sits on the side facing the screen; the strip runs from the orb's
/// centre toward the screen (closed), or over the card's foot up to the orb (card open).
pub fn geometry(corner: Corner, orb: [f32; 4], card: Option<[f32; 4]>) -> CrownGeometry {
    let grows_left = matches!(corner, Corner::TopRight | Corner::BottomRight);
    let s = orb[2] / 56.0;
    let (cx, cy) = (orb[0] + orb[2] * 0.5, orb[1] + orb[3] * 0.5);
    let crown_x = if grows_left { cx - 36.0 * s } else { cx + 25.0 * s };
    let crown = [crown_x, cy - 8.0 * s, 11.0 * s, 16.0 * s];
    let art_x = if grows_left { crown[0] + 2.0 * s } else { crown[0] + crown[2] - 8.0 * s };
    let crown_art = [art_x, cy - 5.0 * s, 6.0 * s, 10.0 * s];
    let y = cy - STRIP_H * 0.5;

    // (x0, x1) of the full strip, and the content box's padding on its left and right.
    let (x0, x1, pad_left, pad_right, line_left, line_right, anchor) = match card {
        None if grows_left => (cx - STRIP_W, cx, STRIP_W - 36.0 - 260.0, 36.0, 22.0, 36.0, cx),
        None => (cx, cx + STRIP_W, 36.0, STRIP_W - 36.0 - 260.0, 36.0, 22.0, cx),
        Some(card) if grows_left => {
            let (x0, x1) = (card[0] + 12.0, orb[0] + 6.0);
            (x0, x1, 6.0, 24.0, 22.0, 24.0, x1)
        }
        Some(card) => {
            let (x0, x1) = (orb[0] + orb[2] - 6.0, card[0] + card[2] - 12.0);
            (x0, x1, 24.0, 6.0, 24.0, 22.0, x0)
        }
    };
    let (left, right) = (x0 + pad_left, x1 - pad_right);
    let button = |x: f32| [x, cy - BUTTON * 0.5, BUTTON, BUTTON];
    let next = button(right - BUTTON);
    let pause = button(right - BUTTON * 2.0 - 10.0);
    let art = [left, cy - ART * 0.5, ART, ART];
    let text_x = left + ART + 10.0;
    CrownGeometry {
        crown,
        crown_art,
        strip: [x0, y, x1 - x0, STRIP_H],
        anchor,
        grows_left,
        art,
        pause,
        next,
        text: [text_x, pause[0] - 10.0 - text_x],
        line: [x0 + line_left, y + STRIP_H - 2.0, (x1 - line_right) - (x0 + line_left)],
        in_card: card.is_some(),
    }
}

impl CrownGeometry {
    /// The strip drawn out by `t` of its full width, from the orb end.
    pub fn strip_at(&self, t: f32) -> [f32; 4] {
        let w = self.strip[2] * t.clamp(0.0, 1.0);
        let x = if self.grows_left { self.anchor - w } else { self.anchor };
        [x, self.strip[1], w, self.strip[3]]
    }

    /// How far the strip must be drawn out before a point at `x` is under it.
    fn reach(&self, x: f32) -> f32 {
        (self.anchor - x).abs() / self.strip[2].max(1.0)
    }
}

#[derive(Default)]
pub struct CrownRuntime {
    now: Option<NowPlaying>,
    requests: Option<Sender<MprisRequest>>,
    shown: Eased,
    hovered: bool,
    strip: f32,
    strip_velocity: f32,
    strip_alpha: Eased,
    art_url: Option<String>,
    has_art: bool,
    geometry: Option<CrownGeometry>,
}

impl SpatilState {
    pub fn attach_mpris(&mut self, requests: Option<Sender<MprisRequest>>) {
        self.crown.requests = requests;
    }

    pub fn set_now_playing(&mut self, now: Option<NowPlaying>) {
        let url = now.as_ref().map(|n| n.track.art_url.clone()).filter(|u| !u.is_empty());
        if url != self.crown.art_url {
            let art = url.as_deref().and_then(crate::shell::mpris::load_art);
            self.set_crown_art(art.as_ref());
            self.crown.art_url = url;
        }
        self.crown.now = now;
    }

    pub fn set_crown_art(&mut self, art: Option<&TrayPixmap>) {
        self.crown.has_art = art.is_some();
        self.wgpu_state.set_orb_art(art);
    }

    fn request(&self, make: impl FnOnce(String) -> MprisRequest) {
        let (Some(now), Some(requests)) = (&self.crown.now, &self.crown.requests) else {
            return;
        };
        let _ = requests.send(make(now.bus.clone()));
    }

    fn crown_strip_rect(&self) -> Option<[f32; 4]> {
        let geometry = self.crown.geometry?;
        (self.crown.strip > 0.05).then(|| geometry.strip_at(self.crown.strip))
    }

    pub fn crown_contains(&self, point: Point<f64, Logical>) -> bool {
        let Some(geometry) = self.crown.geometry else {
            return false;
        };
        if self.crown.shown.value < 0.5 {
            return false;
        }
        inside(geometry.crown, point) || self.crown_strip_rect().is_some_and(|r| inside(r, point))
    }

    pub fn crown_hover(&mut self, point: Point<f64, Logical>) {
        self.crown.hovered = self.crown_contains(point) && !self.orb_contains(point);
    }

    /// How much of the card's time the drawn-out strip covers, 0..1.
    pub fn crown_covers_time(&self) -> f32 {
        match self.crown.geometry {
            Some(g) if g.in_card => self.crown.strip_alpha.value.clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    pub fn crown_press(&mut self, point: Point<f64, Logical>) -> bool {
        if !self.crown_contains(point) || self.orb_contains(point) {
            return false;
        }
        let Some(geometry) = self.crown.geometry else {
            return false;
        };
        let strip_open = self.crown.strip > 0.6;
        if strip_open && inside(geometry.art, point) {
            tracing::info!("familiar: fly to the playing window (placeholder; not built yet)");
        } else if strip_open && inside(geometry.next, point) {
            self.request(MprisRequest::Next);
        } else if inside(geometry.crown, point) || (strip_open && inside(geometry.pause, point)) {
            self.request(MprisRequest::PlayPause);
        }
        true
    }

    pub fn crown_scroll(&mut self, point: Point<f64, Logical>, steps: f32) -> bool {
        if !self.crown_contains(point) {
            return false;
        }
        self.request(|bus| MprisRequest::Volume(bus, VOLUME_STEP * steps as f64));
        true
    }

    pub fn tick_crown(&mut self, dt_ms: f32) {
        let (stiffness, damping) = (
            self.config.animation.spring_stiffness,
            self.config.animation.spring_damping,
        );
        let dt = dt_ms / 1000.0;
        let card_settled = self.orb.open_value() < 0.02 || self.orb.open_value() > 0.98;
        let out = self.crown.hovered && self.crown.now.is_some() && self.banner_rect().is_none() && card_settled;
        let crown = &mut self.crown;
        crown.shown.tick(crown.now.is_some() as u8 as f32, stiffness, damping, dt);
        crown.strip_alpha.tick(out as u8 as f32, stiffness, damping, dt);
        let (v, vel) = spring_step(crown.strip, crown.strip_velocity, out as u8 as f32, stiffness, damping, dt);
        crown.strip = v.clamp(0.0, 1.05);
        crown.strip_velocity = vel;
    }

    pub fn crown_rect(&self) -> Option<[f32; 4]> {
        self.crown.geometry.map(|g| g.crown)
    }

    pub fn crown_report(&self) -> serde_json::Value {
        serde_json::json!({
            "now_playing": self.crown.now.as_ref().map(|n| n.report()),
            "has_art": self.crown.has_art,
            "hovered": self.crown.hovered,
            "strip": self.crown.strip,
            "crown_rect": self.crown.geometry.map(|g| g.crown),
        })
    }

    /// Lays out the crown and strip for the orb shader, and the strip's title and artist.
    pub fn draw_crown(&mut self) {
        let Some(orb) = self.orb_rect() else {
            self.crown.geometry = None;
            return;
        };
        let orb = [orb.loc.x as f32, orb.loc.y as f32, orb.size.w as f32, orb.size.h as f32];
        let card = self.card_rect().filter(|_| self.orb.open_value() > 0.98);
        let geometry = geometry(self.orb_corner(), orb, card);
        self.crown.geometry = Some(geometry);

        let shown = self.crown.shown.value.clamp(0.0, 1.0);
        let Some(now) = self.crown.now.clone() else {
            if let Some(frame) = self.wgpu_state.orb_frame.as_mut() {
                frame.crown.crown_style[0] = 0.0;
                frame.crown.strip_style[0] = 0.0;
            }
            return;
        };
        let s = orb[2] / 56.0;
        let strip_alpha = self.crown.strip_alpha.value.clamp(0.0, 1.0);
        let strip = geometry.strip_at(self.crown.strip);
        let playing = now.playback == Playback::Playing;
        let (strip_color, raised) = if geometry.in_card {
            (parse_hex_color("#22272F"), 0.0)
        } else {
            (parse_hex_color("#171B22"), 1.0)
        };
        let centre = |r: [f32; 4]| [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5];
        let [pcx, pcy] = centre(geometry.pause);
        let [ncx, ncy] = centre(geometry.next);
        let look = CrownLook {
            crown: geometry.crown,
            crown_art: geometry.crown_art,
            crown_style: [shown, self.crown.has_art as u8 as f32, 4.0 * s, 2.0 * s],
            strip,
            strip_style: [strip_alpha, raised, playing as u8 as f32, STRIP_H * 0.5],
            strip_color,
            strip_art: geometry.art,
            glyphs: [pcx, pcy, ncx, ncy],
            line: [geometry.line[0], geometry.line[1], geometry.line[2], now.progress()],
        };
        if let Some(frame) = self.wgpu_state.orb_frame.as_mut() {
            frame.crown = look;
        }

        let text_alpha = strip_alpha * ((self.crown.strip - geometry.reach(geometry.text[0])) / 0.06).clamp(0.0, 1.0);
        if text_alpha <= 0.01 {
            return;
        }
        let font = self.config.shell.font.clone();
        let [x, width] = geometry.text;
        let top = geometry.art[1] + (ART - NORMAL * 12.5 - NORMAL * 11.0) * 0.5;
        let (title, _) = crate::text::fit(&now.track.title, &font, 12.5, 500, width);
        let (artist, _) = crate::text::fit(&now.track.artist, &font, 11.0, REGULAR, width);
        let runs = &mut self.wgpu_state.text_runs;
        runs.push(TextRun {
            text: title,
            x,
            y: top,
            size: 12.5,
            weight: 500,
            color: with_alpha(parse_hex_color("#F2F4F8"), text_alpha),
        });
        runs.push(TextRun {
            text: artist,
            x,
            y: top + NORMAL * 12.5,
            size: 11.0,
            weight: REGULAR,
            color: with_alpha(parse_hex_color("#9AA3AF"), text_alpha),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORB: [f32; 4] = [1844.0, 1004.0, 56.0, 56.0];

    #[test]
    fn the_crown_is_11_by_16_and_tucks_3px_under_the_rim() {
        let g = geometry(Corner::BottomRight, ORB, None);
        assert_eq!((g.crown[2], g.crown[3]), (11.0, 16.0));
        assert_eq!(g.crown[0] + g.crown[2], ORB[0] + 3.0, "Crown.dc.html: right 77 vs the orb at 80");
        assert_eq!(g.crown[1] + 8.0, ORB[1] + 28.0, "centred on the orb's midline");
    }

    #[test]
    fn the_crown_faces_the_screen_in_every_corner() {
        let left = geometry(Corner::BottomLeft, [20.0, 1004.0, 56.0, 56.0], None);
        assert_eq!(left.crown[0], 20.0 + 56.0 - 3.0, "a left orb wears it on its right");
        assert!(!left.grows_left);
        assert!(geometry(Corner::TopRight, ORB, None).grows_left);
    }

    #[test]
    fn the_closed_strip_runs_304px_from_the_orb_centre() {
        let g = geometry(Corner::BottomRight, ORB, None);
        assert_eq!(g.strip, [ORB[0] + 28.0 - 304.0, ORB[1] + 28.0 - 22.0, 304.0, 44.0]);
        assert_eq!(g.next[0] + BUTTON, ORB[0] + 28.0 - 36.0, "content ends 36px short of the orb end");
        assert_eq!(g.art[0], g.strip[0] + 8.0, "the 260px content box leaves 8px at the far end");
        assert_eq!(g.line[0], g.strip[0] + 22.0);
        assert_eq!(g.strip_at(0.0)[2], 0.0);
        assert_eq!(g.strip_at(0.5)[0] + g.strip_at(0.5)[2], g.anchor, "it grows away from the orb");
    }

    #[test]
    fn content_is_never_mirrored() {
        for corner in [Corner::BottomLeft, Corner::BottomRight] {
            let g = geometry(corner, ORB, None);
            assert!(g.art[0] < g.text[0] && g.text[0] + g.text[1] < g.pause[0] && g.pause[0] < g.next[0]);
        }
    }

    #[test]
    fn with_the_card_open_the_strip_covers_the_foot_up_to_the_orb() {
        let card = [ORB[0] + 68.0 - 340.0, 600.0, 340.0, 472.0];
        let g = geometry(Corner::BottomRight, ORB, Some(card));
        assert_eq!(g.strip[0], card[0] + 12.0);
        assert_eq!(g.strip[0] + g.strip[2], ORB[0] + 6.0);
        assert_eq!(g.art[0], g.strip[0] + 6.0);
    }
}
