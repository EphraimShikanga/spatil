use std::time::{Duration, Instant};

use smithay::utils::{Logical, Rectangle};

use crate::renderer::panel::PanelInstance;
use crate::renderer::text::TextRun;
use crate::scene::animation::spring_step;
use crate::shell::module::ModuleId;
use crate::state::SpatilState;

pub const DWELL: Duration = Duration::from_millis(450);
const PAD_X: f32 = 9.0;
const PAD_Y: f32 = 5.0;
const GAP: f32 = 10.0;
const RADIUS: f32 = 6.0;
const MAX_WIDTH: f32 = 320.0;

#[derive(Default)]
pub struct TooltipRuntime {
    hovered: Option<(ModuleId, Instant)>,
    shown: Option<ModuleId>,
    text: String,
    rect: [f32; 4],
    alpha: f32,
    velocity: f32,
}

impl TooltipRuntime {
    pub fn visible(&self) -> bool {
        self.alpha > 0.01 && !self.text.is_empty()
    }
}

/// Places the tooltip beside `anchor`, kept inside `zone`.
fn place(anchor: [f32; 4], width: f32, height: f32, zone: Rectangle<i32, Logical>) -> [f32; 4] {
    let (zx, zy) = (zone.loc.x as f32, zone.loc.y as f32);
    let (zw, zh) = (zone.size.w as f32, zone.size.h as f32);
    let centre_y = anchor[1] + anchor[3] * 0.5 - height * 0.5;

    // Prefer the side with more room, so a corner-anchored orb reads inward.
    let left_room = anchor[0] - zx;
    let right_room = (zx + zw) - (anchor[0] + anchor[2]);
    let x = if right_room >= width + GAP || right_room >= left_room {
        anchor[0] + anchor[2] + GAP
    } else {
        anchor[0] - GAP - width
    };

    [
        x.clamp(zx, (zx + zw - width).max(zx)),
        centre_y.clamp(zy, (zy + zh - height).max(zy)),
        width,
        height,
    ]
}

impl SpatilState {
    pub fn set_hovered_module(&mut self, module: Option<ModuleId>) {
        let current = self.tooltip.hovered.as_ref().map(|(id, _)| id);
        if current == module.as_ref() {
            return;
        }
        self.tooltip.hovered = module.map(|id| (id, Instant::now()));
    }

    pub fn tick_tooltip(&mut self, dt: f32) {
        let font_size = self.config.shell.font_size;
        let due = self
            .tooltip
            .hovered
            .as_ref()
            .filter(|(_, since)| since.elapsed() >= DWELL)
            .map(|(id, _)| id.clone());

        if self.tooltip.shown != due {
            self.tooltip.shown = due.clone();
            self.tooltip.text.clear();
            if let Some(id) = &due {
                self.tooltip.text = self.module_tooltip(id);
            }
        }

        let target = if self.tooltip.text.is_empty() { 0.0 } else { 1.0 };
        let (alpha, velocity) = spring_step(
            self.tooltip.alpha,
            self.tooltip.velocity,
            target,
            220.0,
            26.0,
            dt,
        );
        self.tooltip.alpha = alpha.clamp(0.0, 1.0);
        self.tooltip.velocity = velocity;

        if !self.tooltip.visible() {
            return;
        }
        let Some(id) = self.tooltip.shown.clone() else {
            return;
        };
        let Some(anchor) = self.module_rect(&id) else {
            return;
        };

        let font = self.config.shell.font.clone();
        let (fitted, width) = crate::text::fit(
            &self.tooltip.text,
            &font,
            font_size,
            MAX_WIDTH - PAD_X * 2.0,
        );
        if fitted != self.tooltip.text {
            self.tooltip.text = fitted;
        }
        let height = crate::text::line_height(font_size).ceil() + PAD_Y * 2.0;
        self.tooltip.rect = place(anchor, width + PAD_X * 2.0, height, self.orb_zone());
    }

    pub fn text_report(&self) -> serde_json::Value {
        let (glyphs, atlas_glyphs, atlas_resets, fonts_ready) = self.wgpu_state.text_report();
        serde_json::json!({
            "fonts_ready": fonts_ready,
            "font": self.config.shell.font,
            "font_size": self.config.shell.font_size,
            "clock": if self.wgpu_state.orb.digits { "digital" } else { "analog" },
            "glyphs": glyphs,
            "atlas_glyphs": atlas_glyphs,
            "atlas_resets": atlas_resets,
            "runs": self.wgpu_state.text_runs.len(),
            "dwell_ms": DWELL.as_millis() as u64,
            "hovered": self.tooltip.hovered.as_ref().map(|(id, _)| id.key()),
            "tooltip": {
                "visible": self.tooltip.visible(),
                "text": self.tooltip.text,
                "rect": self.tooltip.rect,
                "alpha": self.tooltip.alpha,
                "module": self.tooltip.shown.as_ref().map(|id| id.key()),
            },
            "modules": self.shell_modules()
                .iter()
                .map(|m| serde_json::json!({
                    "id": m.id.key(),
                    "label": m.label,
                    "tooltip": m.tooltip,
                }))
                .collect::<Vec<_>>(),
        })
    }

    fn module_tooltip(&self, id: &ModuleId) -> String {
        self.shell_modules()
            .into_iter()
            .find(|module| &module.id == id)
            .map(|module| {
                let text = if module.tooltip.is_empty() {
                    module.label
                } else {
                    module.tooltip
                };
                crate::text::sanitize(&text)
            })
            .unwrap_or_default()
    }

    fn module_rect(&self, id: &ModuleId) -> Option<[f32; 4]> {
        self.module_slots
            .iter()
            .find(|(slot, _)| slot == id)
            .map(|(_, rect)| *rect)
    }

    pub fn publish_hud_text(&mut self) {
        self.wgpu_state.panels.clear();
        self.wgpu_state.text_runs.clear();
        if let Some(run) = self.clock_digits() {
            self.wgpu_state.text_runs.push(run);
        }
        if let Some((panel, run)) = self.tooltip_draw() {
            self.wgpu_state.panels.push(panel);
            self.wgpu_state.text_runs.push(run);
        }
        let (mut panels, mut runs) = (
            std::mem::take(&mut self.wgpu_state.panels),
            std::mem::take(&mut self.wgpu_state.text_runs),
        );
        self.menu_draw(&mut panels, &mut runs);
        self.wgpu_state.panels = panels;
        self.wgpu_state.text_runs = runs;
    }

    fn clock_digits(&self) -> Option<TextRun> {
        if !self.wgpu_state.orb.digits {
            return None;
        }
        let frame = self.wgpu_state.orb_frame.as_ref()?;
        let secs = frame.seconds_of_day as u32;
        let text = format!("{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60);

        // Scaled off the orb so the face stays legible at any familiar_size.
        let size = (frame.rect[2] * 0.26).clamp(8.0, 40.0);
        let font = &self.config.shell.font;
        let width = crate::text::measure(&text, font, size);
        let mut color = self.wgpu_state.orb.accent_color;
        color[3] = 1.0 - frame.ambient * 0.45;
        Some(TextRun {
            x: frame.rect[0] + (frame.rect[2] - width) * 0.5,
            y: frame.rect[1] + (frame.rect[3] - crate::text::line_height(size)) * 0.5,
            text,
            size,
            color,
        })
    }

    pub fn tooltip_draw(&self) -> Option<(PanelInstance, TextRun)> {
        if !self.tooltip.visible() {
            return None;
        }
        let rect = self.tooltip.rect;
        let panel = PanelInstance::new(rect, RADIUS, self.tooltip.alpha, 0.0);
        let mut color = crate::config::parse_hex_color("#F2F4F8");
        color[3] = self.tooltip.alpha;
        let run = TextRun {
            text: self.tooltip.text.clone(),
            x: rect[0] + PAD_X,
            y: rect[1] + PAD_Y,
            size: self.config.shell.font_size,
            color,
        };
        Some((panel, run))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (w, h).into())
    }

    #[test]
    fn a_tooltip_sits_to_the_right_when_there_is_room() {
        let placed = place([100.0, 100.0, 26.0, 26.0], 80.0, 24.0, zone(800, 600));
        assert!(placed[0] >= 126.0 + GAP - 0.01);
    }

    #[test]
    fn a_tooltip_flips_left_rather_than_overflow_the_zone() {
        let placed = place([760.0, 300.0, 26.0, 26.0], 120.0, 24.0, zone(800, 600));
        assert!(placed[0] < 760.0, "flipped to the roomier side");
        assert!(placed[0] >= 0.0);
        assert!(placed[0] + placed[2] <= 800.0);
    }

    #[test]
    fn a_tooltip_is_centred_on_its_anchor() {
        let placed = place([100.0, 200.0, 26.0, 26.0], 80.0, 24.0, zone(800, 600));
        let anchor_centre = 200.0 + 13.0;
        assert!((placed[1] + 12.0 - anchor_centre).abs() < 0.01);
    }

    #[test]
    fn a_tooltip_never_leaves_the_zone_vertically() {
        let top = place([100.0, 0.0, 26.0, 26.0], 80.0, 40.0, zone(800, 600));
        assert!(top[1] >= 0.0);
        let bottom = place([100.0, 590.0, 26.0, 26.0], 80.0, 40.0, zone(800, 600));
        assert!(bottom[1] + bottom[3] <= 600.0);
    }

    #[test]
    fn a_zone_smaller_than_the_tooltip_still_yields_a_sane_rect() {
        let placed = place([10.0, 10.0, 26.0, 26.0], 400.0, 200.0, zone(100, 100));
        assert!(placed[0] >= 0.0 && placed[1] >= 0.0);
    }
}
