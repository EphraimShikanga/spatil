//! The banner that morphs out of the closed orb while Familiar listens, thinks, plans, works and answers.

use std::hash::{DefaultHasher, Hash, Hasher};

use smithay::utils::{Logical, Point};

use crate::config::parse_hex_color;
use crate::renderer::orb::Corner;
use crate::renderer::panel::PanelInstance;
use crate::renderer::text::TextRun;
use crate::scene::animation::spring_step;
use crate::shell::agent::{AgentEvent, AgentPhase};
use crate::shell::card::{inside, with_alpha};
use crate::shell::orb::Eased;
use crate::state::SpatilState;
use crate::text::REGULAR;

const INSET: f32 = 12.0;
const PILL: [f32; 3] = [352.0, 76.0, 38.0];
const ANSWER_W: f32 = 380.0;
const PLAN_W: f32 = 380.0;
const PLAN_R: f32 = 22.0;
const PLAN_PAD: f32 = 18.0;
const FAR_PAD: f32 = 22.0;
const ORB_PAD: f32 = 84.0;
const ANSWER_ORB_PAD: f32 = 80.0;
const BUTTONS_ORB_PAD: f32 = 66.0;
const APPEAR_DELAY_MS: f32 = 160.0;
const APPEAR_MS: f32 = 260.0;
const APPEAR_RISE: f32 = 4.0;
/// CSS `line-height: normal` for Noto Sans: ascent plus descent, in ems.
const NORMAL: f32 = 1.362;
const MEDIUM: u16 = 500;
const SEMIBOLD: u16 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Listening,
    Thinking,
    Plan,
    Working,
    Answering,
}

impl Kind {
    fn of(phase: AgentPhase) -> Option<Self> {
        Some(match phase {
            AgentPhase::Listening => Self::Listening,
            AgentPhase::Thinking => Self::Thinking,
            AgentPhase::WaitingForYou => Self::Plan,
            AgentPhase::Working { .. } => Self::Working,
            AgentPhase::Answering => Self::Answering,
            AgentPhase::Idle | AgentPhase::Done => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerButton {
    DoIt,
    Change,
    Undo,
}

impl BannerButton {
    fn event(self) -> AgentEvent {
        match self {
            Self::DoIt => AgentEvent::DoIt,
            Self::Change => AgentEvent::Change,
            Self::Undo => AgentEvent::Undo,
        }
    }
}

#[derive(Clone, Default)]
struct PlanText {
    said: Vec<String>,
    steps: Vec<Vec<String>>,
    height: f32,
}

#[derive(Default)]
pub struct BannerRuntime {
    open: Eased,
    size: [f32; 3],
    velocity: [f32; 3],
    target: [f32; 3],
    shown: Option<Kind>,
    content_ms: f32,
    rect: Option<[f32; 4]>,
    buttons: Vec<(BannerButton, [f32; 4])>,
    plan: Option<(u64, PlanText)>,
}

struct Ink {
    text: [f32; 4],
    label: [f32; 4],
    said: [f32; 4],
    ring: [f32; 4],
    button: [f32; 4],
    on_primary: [f32; 4],
    surface: [f32; 4],
}

impl Ink {
    fn new(body: [f32; 4]) -> Self {
        Self {
            text: parse_hex_color("#F2F4F8"),
            label: parse_hex_color("#9AA3AF"),
            said: parse_hex_color("#C9CFD8"),
            ring: parse_hex_color("#5F6672"),
            button: parse_hex_color("#2B3037"),
            on_primary: parse_hex_color("#0F1216"),
            // The orb shader's card surface.
            surface: [body[0] * 1.3, body[1] * 1.3, body[2] * 1.3, 1.0],
        }
    }
}

/// The banner's rect for a size, its near corner `INSET` outside the orb's, as the card's is.
pub fn anchor(corner: Corner, orb: [f32; 4], w: f32, h: f32) -> [f32; 4] {
    let right = matches!(corner, Corner::TopRight | Corner::BottomRight);
    let bottom = matches!(corner, Corner::BottomLeft | Corner::BottomRight);
    let x = if right { orb[0] + orb[2] + INSET - w } else { orb[0] - INSET };
    let y = if bottom { orb[1] + orb[3] + INSET - h } else { orb[1] - INSET };
    [x, y, w, h]
}

/// The text column of a pill: `far` from the end away from the orb, `near` from the orb's end.
fn pill_column(corner: Corner, rect: [f32; 4], near: f32) -> (f32, f32) {
    let right = matches!(corner, Corner::TopRight | Corner::BottomRight);
    let (left_pad, right_pad) = if right { (FAR_PAD, near) } else { (near, FAR_PAD) };
    (rect[0] + left_pad, rect[2] - left_pad - right_pad)
}

/// Where a line of `size` text sits inside a CSS line box of `line` height starting at `top`.
fn line_y(top: f32, line: f32, size: f32) -> f32 {
    top + (line - NORMAL * size) * 0.5
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

fn plan_text(font: &str, said: &str, steps: &[String]) -> PlanText {
    let inner = PLAN_W - PLAN_PAD * 2.0;
    let said = crate::text::wrap(said, font, 12.5, REGULAR, inner, usize::MAX);
    let steps: Vec<Vec<String>> = steps
        .iter()
        .map(|s| crate::text::wrap(s, font, 12.5, REGULAR, inner - 28.0, usize::MAX))
        .collect();
    let said_h = NORMAL * 11.0 + 3.0 + said.len().max(1) as f32 * 12.5 * 1.5;
    let steps_h: f32 = steps.iter().map(|l| (l.len().max(1) as f32 * 12.5 * 1.45).max(18.0)).sum::<f32>()
        + 9.0 * steps.len().saturating_sub(1) as f32;
    let mut height = PLAN_PAD + said_h + 14.0 + 36.0 + PLAN_PAD;
    if !steps.is_empty() {
        height += steps_h + 14.0;
    }
    PlanText { said, steps, height }
}

impl SpatilState {
    fn banner_kind(&self) -> Option<Kind> {
        if self.orb_expanded() || self.card_rect().is_some() {
            return None;
        }
        Kind::of(self.agent.phase())
    }

    fn banner_plan(&mut self) -> PlanText {
        let mut hasher = DefaultHasher::new();
        (&self.agent.transcript, &self.agent.plan, &self.config.shell.font).hash(&mut hasher);
        let key = hasher.finish();
        if let Some((cached, text)) = &self.banner.plan
            && *cached == key
        {
            return text.clone();
        }
        let text = plan_text(&self.config.shell.font, &self.agent.transcript, &self.agent.plan);
        self.banner.plan = Some((key, text.clone()));
        text
    }

    pub fn tick_banner(&mut self, dt_ms: f32) {
        let kind = self.banner_kind();
        let target = match kind {
            Some(Kind::Plan) => [PLAN_W, self.banner_plan().height, PLAN_R],
            Some(Kind::Answering) => [ANSWER_W, PILL[1], PILL[2]],
            Some(_) => PILL,
            None => self.banner.target,
        };
        let (stiffness, damping) = (
            self.config.animation.spring_stiffness,
            self.config.animation.spring_damping,
        );
        let dt = dt_ms / 1000.0;
        let banner = &mut self.banner;
        banner.target = target;
        banner.open.tick(kind.is_some() as u8 as f32, stiffness, damping, dt);
        // Fresh out of the orb, the banner grows straight to its shape rather than via the last one.
        if kind.is_some() && banner.open.value < 0.02 {
            banner.size = target;
            banner.velocity = [0.0; 3];
        } else if kind.is_some() {
            for i in 0..3 {
                let (v, vel) = spring_step(banner.size[i], banner.velocity[i], target[i], stiffness, damping, dt);
                banner.size[i] = v;
                banner.velocity[i] = vel;
            }
        }
        if banner.shown != kind {
            banner.shown = kind;
            banner.content_ms = 0.0;
        } else {
            banner.content_ms += dt_ms;
        }
    }

    pub fn banner_rect(&self) -> Option<[f32; 4]> {
        self.banner.rect
    }

    pub fn banner_contains(&self, point: Point<f64, Logical>) -> bool {
        self.banner.rect.is_some_and(|rect| inside(rect, point))
    }

    /// A press on the banner: its buttons answer, the rest is swallowed; the orb keeps its own.
    pub fn banner_press(&mut self, point: Point<f64, Logical>) -> bool {
        if !self.banner_contains(point) || self.orb_contains(point) {
            return false;
        }
        let hit = self.banner.buttons.iter().find(|(_, r)| inside(*r, point)).map(|(b, _)| *b);
        if let Some(button) = hit {
            self.agent.apply(button.event());
        }
        true
    }

    pub fn banner_button(&self, button: BannerButton) -> Option<[f32; 4]> {
        self.banner.buttons.iter().find(|(b, _)| *b == button).map(|(_, r)| *r)
    }

    pub fn banner_report(&self) -> serde_json::Value {
        serde_json::json!({
            "rect": self.banner.rect,
            "open": self.banner.open.value,
            "buttons": self.banner.buttons.iter().map(|(b, r)| (format!("{b:?}"), *r)).collect::<Vec<_>>(),
        })
    }

    /// Draws the banner surface (through the orb shader's card) and its content into the HUD base layer.
    pub fn draw_banner(&mut self) {
        self.banner.rect = None;
        self.banner.buttons.clear();
        if self.card_rect().is_some() {
            return;
        }
        let t = ease(self.banner.open.value);
        let Some(orb) = self.orb_rect() else {
            return;
        };
        if t <= 0.002 {
            return;
        }
        let orb = [orb.loc.x as f32, orb.loc.y as f32, orb.size.w as f32, orb.size.h as f32];
        let corner = self.orb_corner();
        let [w, h, r] = self.banner.size;
        let shaped = anchor(corner, orb, w, h);
        let morph: [f32; 4] = std::array::from_fn(|i| lerp(orb[i], shaped[i], t));
        if let Some(frame) = self.wgpu_state.orb_frame.as_mut() {
            frame.card = morph;
            frame.card_radius = lerp(orb[2] * 0.5, r, t);
            frame.blend = 96.0 * t * (1.0 - t);
            frame.open = frame.open.max(t);
        }
        self.banner.rect = Some(morph);

        let Some(kind) = self.banner.shown else {
            return;
        };
        let [tw, th, _] = self.banner.target;
        let covered = (1.0 - ((w - tw).abs() + (h - th).abs()) / 24.0).clamp(0.0, 1.0);
        let appear = ease_out((self.banner.content_ms - APPEAR_DELAY_MS) / APPEAR_MS);
        let alpha = appear * ease((t - 0.88) / 0.12) * covered;
        if alpha <= 0.01 {
            return;
        }
        let rise = APPEAR_RISE * (1.0 - appear);
        let rect = anchor(corner, orb, tw, th);
        let ink = Ink::new(self.wgpu_state.orb.body_color);
        let font = self.config.shell.font.clone();
        let mut panels = std::mem::take(&mut self.wgpu_state.panels);
        let mut runs = std::mem::take(&mut self.wgpu_state.text_runs);
        let mut buttons = Vec::new();
        let paint = Paint { font: &font, alpha };

        match kind {
            Kind::Listening | Kind::Thinking => {
                let listening = kind == Kind::Listening;
                let label = if listening { "Listening · let go to send" } else { "Thinking" };
                let (x, width) = pill_column(corner, rect, ORB_PAD);
                let top = rect[1] + (rect[3] - (NORMAL * 11.0 + 3.0 + NORMAL * 13.5)) * 0.5 + rise;
                paint.line(&mut runs, label, x, top, 11.0, REGULAR, ink.label);
                let caret = if listening { 3.5 } else { 0.0 };
                let (tail, tail_w) =
                    crate::text::fit_tail(&self.agent.transcript, &font, 13.5, REGULAR, width - caret);
                let text_top = top + NORMAL * 11.0 + 3.0;
                let text_x = x + width - caret - tail_w;
                let color = if listening { ink.text } else { ink.label };
                paint.line(&mut runs, &tail, text_x, text_top, 13.5, REGULAR, color);
                if listening {
                    let baseline = text_top + 1.069 * 13.5;
                    let caret_rect = [x + width - 1.5, baseline + 2.0 - 15.0, 1.5, 15.0];
                    panels.push(PanelInstance::fill(caret_rect, 0.0, with_alpha(ink.text, alpha)));
                }
            }
            Kind::Working => {
                let (step, total) = match self.agent.phase() {
                    AgentPhase::Working { step, total } => (step, total),
                    _ => (0, 0),
                };
                let n = (step + 1).min(total.max(1));
                let label = format!("Working · step {n} of {}", total.max(1));
                let text = self
                    .agent
                    .plan
                    .get(step.min(total.saturating_sub(1)) as usize)
                    .cloned()
                    .unwrap_or_default();
                let (x, width) = pill_column(corner, rect, ORB_PAD);
                let top = rect[1] + (rect[3] - (NORMAL * 11.0 + 3.0 + NORMAL * 13.5)) * 0.5 + rise;
                paint.line(&mut runs, &label, x, top, 11.0, REGULAR, ink.label);
                let (text, _) = crate::text::fit(&text, &font, 13.5, REGULAR, width);
                paint.line(&mut runs, &text, x, top + NORMAL * 11.0 + 3.0, 13.5, REGULAR, ink.text);
            }
            Kind::Answering => {
                let (x, width) = pill_column(corner, rect, ANSWER_ORB_PAD);
                let undo_w = crate::text::measure("Undo", &font, 12.0, MEDIUM) + 24.0;
                let undo = [x + width - undo_w, rect[1] + (rect[3] - 32.0) * 0.5 + rise, undo_w, 32.0];
                paint.button(&mut panels, &mut runs, undo, 10.0, "Undo", 12.0, MEDIUM, ink.button, ink.text);
                buttons.push((BannerButton::Undo, undo));

                let text_w = width - undo_w - 10.0;
                let lines = crate::text::wrap(&self.agent.answer, &font, 13.0, REGULAR, text_w, 2);
                let line = 13.0 * 1.4;
                let block = NORMAL * 11.0 + 3.0 + line * lines.len().max(1) as f32;
                let top = rect[1] + (rect[3] - block) * 0.5 + rise;
                paint.line(&mut runs, "Familiar", x, top, 11.0, REGULAR, ink.label);
                for (i, l) in lines.iter().enumerate() {
                    let line_top = top + NORMAL * 11.0 + 3.0 + i as f32 * line;
                    paint.line(&mut runs, l, x, line_y(line_top, line, 13.0), 13.0, REGULAR, ink.text);
                }
            }
            Kind::Plan => {
                let plan = self.banner_plan();
                let rects = plan_layout(corner, rect, &plan, &font);
                paint.line(&mut runs, "You said", rects.said[0], rects.said[1] + rise, 11.0, REGULAR, ink.label);
                let said_line = 12.5 * 1.5;
                for (i, l) in plan.said.iter().enumerate() {
                    let top = rects.said[1] + NORMAL * 11.0 + 3.0 + i as f32 * said_line + rise;
                    paint.line(&mut runs, l, rects.said[0], line_y(top, said_line, 12.5), 12.5, REGULAR, ink.said);
                }
                let step_line = 12.5 * 1.45;
                for (i, (lines, at)) in plan.steps.iter().zip(&rects.steps).enumerate() {
                    let circle = [at[0], at[1] + rise, 18.0, 18.0];
                    panels.push(PanelInstance::fill(circle, 9.0, with_alpha(ink.ring, alpha)));
                    let hole = [circle[0] + 1.5, circle[1] + 1.5, 15.0, 15.0];
                    panels.push(PanelInstance::fill(hole, 7.5, with_alpha(ink.surface, alpha)));
                    let n = (i + 1).to_string();
                    let n_w = crate::text::measure(&n, &font, 10.5, SEMIBOLD);
                    let n_y = line_y(circle[1], 18.0, 10.5);
                    paint.line(&mut runs, &n, circle[0] + (18.0 - n_w) * 0.5, n_y, 10.5, SEMIBOLD, ink.said);
                    for (j, l) in lines.iter().enumerate() {
                        let top = at[1] + j as f32 * step_line + rise;
                        paint.line(&mut runs, l, at[0] + 28.0, line_y(top, step_line, 12.5), 12.5, REGULAR, ink.text);
                    }
                }
                let [do_it, change, hint] = rects.buttons;
                let lift = |r: [f32; 4]| [r[0], r[1] + rise, r[2], r[3]];
                paint.button(&mut panels, &mut runs, lift(do_it), 11.0, "Do it", 12.5, SEMIBOLD, ink.text, ink.on_primary);
                paint.button(&mut panels, &mut runs, lift(change), 11.0, "Change", 12.5, MEDIUM, ink.button, ink.text);
                let hint_y = line_y(hint[1] + rise, hint[3], 11.0);
                paint.line(&mut runs, "or say \"yes\"", hint[0], hint_y, 11.0, REGULAR, ink.label);
                buttons.push((BannerButton::DoIt, do_it));
                buttons.push((BannerButton::Change, change));
            }
        }

        self.wgpu_state.panels = panels;
        self.wgpu_state.text_runs = runs;
        self.banner.buttons = buttons;
    }
}

struct PlanRects {
    said: [f32; 2],
    steps: Vec<[f32; 2]>,
    /// Do it, Change, and the "or say yes" hint.
    buttons: [[f32; 4]; 3],
}

/// Sections stack outward from the orb, as the card's do: for a top corner the buttons come first.
fn plan_layout(corner: Corner, rect: [f32; 4], plan: &PlanText, font: &str) -> PlanRects {
    let right = matches!(corner, Corner::TopRight | Corner::BottomRight);
    let bottom = matches!(corner, Corner::BottomLeft | Corner::BottomRight);
    let x = rect[0] + PLAN_PAD;
    let said_h = NORMAL * 11.0 + 3.0 + plan.said.len().max(1) as f32 * 12.5 * 1.5;
    let step_hs: Vec<f32> =
        plan.steps.iter().map(|l| (l.len().max(1) as f32 * 12.5 * 1.45).max(18.0)).collect();
    let steps_h = step_hs.iter().sum::<f32>() + 9.0 * step_hs.len().saturating_sub(1) as f32;

    let mut sections: Vec<(&str, f32)> = vec![("said", said_h)];
    if !plan.steps.is_empty() {
        sections.push(("steps", steps_h));
    }
    sections.push(("buttons", 36.0));
    if !bottom {
        sections.reverse();
    }

    let mut y = rect[1] + PLAN_PAD;
    let (mut said, mut steps_top, mut buttons_top) = ([x, y], y, y);
    for (name, h) in sections {
        match name {
            "said" => said = [x, y],
            "steps" => steps_top = y,
            _ => buttons_top = y,
        }
        y += h + 14.0;
    }
    let mut steps = Vec::new();
    let mut sy = steps_top;
    for h in step_hs {
        steps.push([x, sy]);
        sy += h + 9.0;
    }

    let do_w = crate::text::measure("Do it", font, 12.5, SEMIBOLD) + 36.0;
    let change_w = crate::text::measure("Change", font, 12.5, MEDIUM) + 28.0;
    let hint_w = crate::text::measure("or say \"yes\"", font, 11.0, REGULAR);
    let row_x = if right { x } else { x + BUTTONS_ORB_PAD };
    let do_it = [row_x, buttons_top, do_w, 36.0];
    let change = [row_x + do_w + 8.0, buttons_top, change_w, 36.0];
    let hint = [change[0] + change_w + 8.0, buttons_top, hint_w, 36.0];
    PlanRects { said, steps, buttons: [do_it, change, hint] }
}

struct Paint<'a> {
    font: &'a str,
    alpha: f32,
}

impl Paint<'_> {
    #[allow(clippy::too_many_arguments)]
    fn line(&self, runs: &mut Vec<TextRun>, text: &str, x: f32, y: f32, size: f32, weight: u16, color: [f32; 4]) {
        if text.is_empty() {
            return;
        }
        runs.push(TextRun { text: text.to_string(), x, y, size, weight, color: with_alpha(color, self.alpha) });
    }

    #[allow(clippy::too_many_arguments)]
    fn button(
        &self,
        panels: &mut Vec<PanelInstance>,
        runs: &mut Vec<TextRun>,
        rect: [f32; 4],
        radius: f32,
        label: &str,
        size: f32,
        weight: u16,
        fill: [f32; 4],
        text: [f32; 4],
    ) {
        panels.push(PanelInstance::fill(rect, radius, with_alpha(fill, self.alpha)));
        let w = crate::text::measure(label, self.font, size, weight);
        let y = line_y(rect[1], rect[3], size);
        self.line(runs, label, rect[0] + (rect[2] - w) * 0.5, y, size, weight, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORB: f32 = 56.0;

    fn orb_at(corner: Corner) -> [f32; 4] {
        let (w, h, m) = (1920.0, 1080.0, 20.0);
        match corner {
            Corner::TopLeft => [m, m, ORB, ORB],
            Corner::TopRight => [w - m - ORB, m, ORB, ORB],
            Corner::BottomLeft => [m, h - m - ORB, ORB, ORB],
            Corner::BottomRight => [w - m - ORB, h - m - ORB, ORB, ORB],
        }
    }

    #[test]
    fn the_pill_holds_the_orb_in_its_near_end_for_every_corner() {
        for corner in [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight] {
            let orb = orb_at(corner);
            let pill = anchor(corner, orb, PILL[0], PILL[1]);
            assert!(pill[0] <= orb[0] && orb[0] + ORB <= pill[0] + pill[2], "{corner:?}");
            assert!(pill[1] <= orb[1] && orb[1] + ORB <= pill[1] + pill[3], "{corner:?}");
            let near_x = if matches!(corner, Corner::TopRight | Corner::BottomRight) {
                pill[0] + pill[2] - (orb[0] + ORB)
            } else {
                orb[0] - pill[0]
            };
            assert_eq!(near_x, INSET, "{corner:?}");
        }
    }

    #[test]
    fn pill_text_keeps_clear_of_the_orb() {
        for corner in [Corner::TopLeft, Corner::BottomRight] {
            let orb = orb_at(corner);
            let pill = anchor(corner, orb, PILL[0], PILL[1]);
            let (x, w) = pill_column(corner, pill, ORB_PAD);
            assert!(x + w <= orb[0] || x >= orb[0] + ORB, "{corner:?}: text runs under the orb");
            assert!((w - (PILL[0] - FAR_PAD - ORB_PAD)).abs() < 0.01);
        }
    }

    #[test]
    fn plan_sections_stack_outward_from_the_orb() {
        let plan = PlanText {
            said: vec!["a".into(), "b".into()],
            steps: vec![vec!["one".into()], vec!["two".into()]],
            height: 0.0,
        };
        let bottom = plan_layout(Corner::BottomRight, [0.0, 0.0, PLAN_W, 300.0], &plan, "sans-serif");
        assert!(bottom.said[1] < bottom.steps[0][1] && bottom.steps[1][1] < bottom.buttons[0][1]);
        let top = plan_layout(Corner::TopRight, [0.0, 0.0, PLAN_W, 300.0], &plan, "sans-serif");
        assert!(top.buttons[0][1] < top.steps[0][1] && top.steps[1][1] < top.said[1]);
    }

    #[test]
    fn the_buttons_row_leaves_room_for_the_orb() {
        let plan = PlanText { said: vec!["a".into()], steps: vec![], height: 0.0 };
        let left = plan_layout(Corner::BottomLeft, [0.0, 0.0, PLAN_W, 200.0], &plan, "sans-serif");
        assert!(left.buttons[0][0] >= PLAN_PAD + BUTTONS_ORB_PAD, "buttons start past the orb");
        let right = plan_layout(Corner::BottomRight, [0.0, 0.0, PLAN_W, 200.0], &plan, "sans-serif");
        assert_eq!(right.buttons[0][0], PLAN_PAD);
    }
}
