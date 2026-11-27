//! Familiar's control-centre card: layout, drawing and hit-testing.

use std::time::{Duration, Instant};

use smithay::utils::{Logical, Point};

use crate::config::parse_hex_color;
use crate::renderer::hud::TrayQuad;
use crate::renderer::orb::Corner;
use crate::renderer::panel::{PanelInstance, PanelKind};
use crate::renderer::text::TextRun;
use crate::shell::module::{ModuleId, ModuleState};
use crate::state::SpatilState;
use crate::text::REGULAR;

pub const WIDTH: f32 = 340.0;
const PAD: f32 = 16.0;
const INSET: f32 = 12.0;
const RADIUS: f32 = 22.0;
const GAP: f32 = 10.0;
const SECTION_GAP: f32 = 14.0;
const TILE_H: f32 = 58.0;
const TILE_RADIUS: f32 = 14.0;
const CHIP: f32 = 30.0;
/// Space between an icon chip and the level ring around it.
const RING_GAP: f32 = 4.0;
const TILE_ICON: f32 = 18.0;
const DEST_H: f32 = 62.0;
const DEST_ICON: f32 = 20.0;
const TRAY_ICON: f32 = 20.0;
const TRAY_PITCH: f32 = 32.0;
const TRAY_LABEL_H: f32 = 16.0;
const NOTE_FOR: Duration = Duration::from_millis(2400);
const MEDIUM: u16 = 500;
const LIGHT: u16 = 300;

pub const STATUS: [ModuleId; 4] =
    [ModuleId::Network, ModuleId::Bluetooth, ModuleId::Audio, ModuleId::Battery];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Alerts,
    Calendar,
    Windows,
    Map,
}

impl Destination {
    pub const ALL: [Self; 4] = [Self::Alerts, Self::Calendar, Self::Windows, Self::Map];

    pub fn label(self) -> &'static str {
        match self {
            Self::Alerts => "Alerts",
            Self::Calendar => "Calendar",
            Self::Windows => "Windows",
            Self::Map => "Map",
        }
    }

    fn unbuilt(self) -> &'static str {
        match self {
            Self::Alerts => "Notifications aren't built yet",
            Self::Calendar => "The calendar isn't built yet",
            Self::Windows => "The window overview isn't built yet",
            Self::Map => "The world map isn't built yet",
        }
    }

    fn icon_names(self) -> &'static [&'static str] {
        match self {
            Self::Alerts => &[
                "preferences-system-notifications-symbolic",
                "notifications-symbolic",
                "notification-symbolic",
            ],
            Self::Calendar => &["x-office-calendar-symbolic", "office-calendar-symbolic"],
            Self::Windows => &["view-grid-symbolic", "view-app-grid-symbolic", "window-new-symbolic"],
            Self::Map => &["find-location-symbolic", "mark-location-symbolic", "map-symbolic"],
        }
    }

    pub fn key(self) -> String {
        format!("dest:{}", self.label().to_ascii_lowercase())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CardTarget {
    Module(ModuleId),
    Destination(Destination),
}

#[derive(Default)]
pub struct CardRuntime {
    rect: Option<[f32; 4]>,
    destinations: Vec<(Destination, [f32; 4])>,
    hovered: Option<CardTarget>,
    note: Option<(Destination, Instant)>,
    icons_resolved: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CardLayout {
    pub rect: [f32; 4],
    pub header: [f32; 4],
    pub tiles: [[f32; 4]; 4],
    pub destinations: [[f32; 4]; 4],
    pub tray_label: Option<[f32; 4]>,
    pub tray: Vec<[f32; 4]>,
}

/// Sections stack outward from the orb, so the card is mirrored per corner; the content
/// inside a section is never mirrored, so Wi-Fi is always top-left and text always reads.
pub fn layout(corner: Corner, orb: [f32; 4], tray_count: usize) -> CardLayout {
    let right = matches!(corner, Corner::TopRight | Corner::BottomRight);
    let bottom = matches!(corner, Corner::BottomLeft | Corner::BottomRight);

    let header_h = orb[3] + INSET * 2.0;
    let tiles_h = TILE_H * 2.0 + GAP;
    let tray_h = TRAY_LABEL_H + 6.0 + TRAY_ICON + 6.0;
    let mut height = header_h + SECTION_GAP + tiles_h + SECTION_GAP + DEST_H + PAD;
    if tray_count > 0 {
        height += SECTION_GAP + tray_h;
    }

    let x = if right { orb[0] + orb[2] + INSET - WIDTH } else { orb[0] - INSET };
    let y = if bottom { orb[1] + orb[3] + INSET - height } else { orb[1] - INSET };
    let rect = [x, y, WIDTH, height];
    let band = |offset: f32, h: f32| if bottom { y + height - offset - h } else { y + offset };

    let header = if right {
        [x + PAD, band(0.0, header_h), orb[0] - 14.0 - (x + PAD), header_h]
    } else {
        let left = orb[0] + orb[2] + 14.0;
        [left, band(0.0, header_h), x + WIDTH - PAD - left, header_h]
    };

    let inner = WIDTH - PAD * 2.0;
    let tile_w = (inner - GAP) * 0.5;
    let tiles_y = band(header_h + SECTION_GAP, tiles_h);
    let tiles = std::array::from_fn(|i| {
        let (col, row) = ((i % 2) as f32, (i / 2) as f32);
        [x + PAD + col * (tile_w + GAP), tiles_y + row * (TILE_H + GAP), tile_w, TILE_H]
    });

    let dest_w = (inner - GAP * 3.0) / 4.0;
    let dest_y = band(header_h + SECTION_GAP + tiles_h + SECTION_GAP, DEST_H);
    let destinations =
        std::array::from_fn(|i| [x + PAD + i as f32 * (dest_w + GAP), dest_y, dest_w, DEST_H]);

    let (tray_label, tray) = if tray_count > 0 {
        let tray_y = band(header_h + SECTION_GAP + tiles_h + SECTION_GAP + DEST_H + SECTION_GAP, tray_h);
        let per_row = ((inner + TRAY_PITCH - TRAY_ICON) / TRAY_PITCH).floor().max(1.0) as usize;
        let icons = (0..tray_count.min(per_row))
            .map(|i| {
                let ix = x + PAD + i as f32 * TRAY_PITCH;
                [ix, tray_y + TRAY_LABEL_H + 6.0, TRAY_ICON, TRAY_ICON]
            })
            .collect();
        (Some([x + PAD, tray_y, inner, TRAY_LABEL_H]), icons)
    } else {
        (None, Vec::new())
    };

    CardLayout { rect, header, tiles, destinations, tray_label, tray }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn inside(rect: [f32; 4], point: Point<f64, Logical>) -> bool {
    let (x, y) = (point.x as f32, point.y as f32);
    x >= rect[0] && x < rect[0] + rect[2] && y >= rect[1] && y < rect[1] + rect[3]
}

fn to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Lightens toward white by `t` in perceptual (sRGB) terms; the HUD blends in linear light,
/// where the same step would look several times stronger.
fn lighten(linear: [f32; 4], t: f32) -> [f32; 4] {
    let f = |c: f32| to_linear(lerp(to_srgb(c), 1.0, t));
    [f(linear[0]), f(linear[1]), f(linear[2]), linear[3]]
}

fn with_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] *= alpha;
    color
}

fn local_date() -> Option<(i32, i32, i32)> {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    // SAFETY: localtime_r writes only into `tm`, which outlives the call.
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return None;
    }
    Some((tm.tm_wday, tm.tm_mday, tm.tm_mon))
}

fn date_line() -> String {
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] = [
        "January", "February", "March", "April", "May", "June", "July", "August", "September",
        "October", "November", "December",
    ];
    let Some((wday, mday, mon)) = local_date() else {
        return String::new();
    };
    let day = DAYS.get(wday as usize).copied().unwrap_or("");
    let month = MONTHS.get(mon as usize).copied().unwrap_or("");
    format!("{day} {mday} {month}")
}

struct Palette {
    primary: [f32; 4],
    secondary: [f32; 4],
    tile: [f32; 4],
    tile_hover: [f32; 4],
    chip_off: [f32; 4],
    accent: [f32; 4],
    amber: [f32; 4],
}

impl Palette {
    fn new(body: [f32; 4], accent: [f32; 4]) -> Self {
        let body = [body[0], body[1], body[2], 1.0];
        Self {
            primary: parse_hex_color("#F2F4F8"),
            secondary: parse_hex_color("#9AA3AF"),
            tile: lighten(body, 0.05),
            tile_hover: lighten(body, 0.1),
            chip_off: lighten(body, 0.14),
            accent,
            amber: parse_hex_color("#F5A524"),
        }
    }
}

impl SpatilState {
    pub fn card_destination_rect(&self, dest: Destination) -> Option<[f32; 4]> {
        self.card.destinations.iter().find(|(d, _)| *d == dest).map(|(_, r)| *r)
    }

    pub fn card_rect(&self) -> Option<[f32; 4]> {
        self.card.rect
    }

    pub fn card_contains(&self, point: Point<f64, Logical>) -> bool {
        self.card.rect.is_some_and(|rect| inside(rect, point))
    }

    fn card_target_at(&self, point: Point<f64, Logical>) -> Option<CardTarget> {
        if let Some(id) = self.module_at(point) {
            return Some(CardTarget::Module(id));
        }
        self.card
            .destinations
            .iter()
            .find(|(_, rect)| inside(*rect, point))
            .map(|(dest, _)| CardTarget::Destination(*dest))
    }

    pub fn card_hover(&mut self, point: Point<f64, Logical>) {
        self.card.hovered = self.card_target_at(point);
    }

    /// A press inside the open card: destinations answer, the rest is swallowed so the
    /// scene behind never sees a click that landed on the card.
    pub fn card_press(&mut self, point: Point<f64, Logical>) -> bool {
        if !self.card_contains(point) {
            return false;
        }
        if let Some(CardTarget::Destination(dest)) = self.card_target_at(point) {
            tracing::info!("familiar: {} pressed; {}", dest.label(), dest.unbuilt());
            self.card.note = Some((dest, Instant::now()));
        }
        true
    }

    fn resolve_destination_icons(&mut self) {
        if self.card.icons_resolved {
            return;
        }
        self.card.icons_resolved = true;
        for dest in Destination::ALL {
            let icon = dest.icon_names().iter().find_map(|name| crate::shell::icon_source(name));
            match icon {
                Some(icon) => {
                    self.wgpu_state.upload_tray_icon(dest.key(), icon.width, icon.height, &icon.bgra)
                }
                None => tracing::debug!("no icon found for the {} destination", dest.label()),
            }
        }
    }

    /// Lays out and draws the card into the HUD's base layer for this frame's `open`.
    pub fn draw_card(&mut self, modules: &[ModuleState], open: f32, warn: bool) {
        self.module_slots.clear();
        self.card.destinations.clear();
        self.wgpu_state.tray_layout.clear();
        if let Some(frame) = self.wgpu_state.orb_frame.as_mut() {
            frame.warn = warn as u8 as f32;
        }

        let Some(orb) = self.orb_rect() else {
            self.card.rect = None;
            return;
        };
        let orb = [orb.loc.x as f32, orb.loc.y as f32, orb.size.w as f32, orb.size.h as f32];
        if open <= 0.002 {
            self.card.rect = None;
            self.card.hovered = None;
            return;
        }
        self.resolve_destination_icons();

        let tray: Vec<&ModuleState> = modules
            .iter()
            .filter(|m| matches!(m.id, ModuleId::Tray(_)) && m.drawable())
            .collect();
        let card = layout(self.orb_corner(), orb, tray.len());

        let t = ease(open);
        let morph = std::array::from_fn(|i| lerp(orb[i], card.rect[i], t));
        if let Some(frame) = self.wgpu_state.orb_frame.as_mut() {
            frame.card = morph;
            frame.card_radius = lerp(orb[2] * 0.5, RADIUS, t);
            frame.blend = 96.0 * t * (1.0 - t);
        }
        self.card.rect = Some(morph);

        let alpha = ease((open - 0.55) / 0.45);
        if alpha <= 0.01 {
            return;
        }

        let palette = Palette::new(self.wgpu_state.orb.body_color, self.wgpu_state.orb.accent_color);
        let font = self.config.shell.font.clone();
        let mut panels = std::mem::take(&mut self.wgpu_state.panels);
        let mut runs = std::mem::take(&mut self.wgpu_state.text_runs);
        let mut icons = Vec::new();

        self.draw_header(&card, &font, &palette, alpha, &mut runs);

        for (id, rect) in STATUS.iter().zip(card.tiles) {
            let module = modules.iter().find(|m| &m.id == id);
            let hovered = self.card.hovered == Some(CardTarget::Module(id.clone()));
            draw_tile(id, module, rect, hovered, &font, &palette, alpha, &mut panels, &mut runs, &mut icons);
            if module.is_some() {
                self.module_slots.push((id.clone(), rect));
            }
        }

        for (dest, rect) in Destination::ALL.into_iter().zip(card.destinations) {
            let hovered = self.card.hovered == Some(CardTarget::Destination(dest));
            let fill = if hovered { palette.tile_hover } else { palette.tile };
            panels.push(PanelInstance::fill(rect, TILE_RADIUS, with_alpha(fill, alpha)));
            icons.push(TrayQuad {
                key: dest.key(),
                rect: [
                    rect[0] + (rect[2] - DEST_ICON) * 0.5,
                    rect[1] + 12.0,
                    DEST_ICON,
                    DEST_ICON,
                ],
                alpha,
            });
            let size = 11.5;
            let width = crate::text::measure(dest.label(), &font, size, MEDIUM);
            runs.push(TextRun {
                text: dest.label().to_string(),
                x: rect[0] + (rect[2] - width) * 0.5,
                y: rect[1] + 38.0,
                size,
                weight: MEDIUM,
                color: with_alpha(palette.secondary, alpha),
            });
            self.card.destinations.push((dest, rect));
        }

        if let Some(label) = card.tray_label {
            runs.push(TextRun {
                text: "Background".to_string(),
                x: label[0],
                y: label[1],
                size: 11.0,
                weight: MEDIUM,
                color: with_alpha(palette.secondary, alpha),
            });
        }
        for (module, rect) in tray.iter().zip(&card.tray) {
            icons.push(TrayQuad { key: module.id.key(), rect: *rect, alpha });
            let hit = [rect[0] - 6.0, rect[1] - 6.0, rect[2] + 12.0, rect[3] + 12.0];
            self.module_slots.push((module.id.clone(), hit));
        }

        self.wgpu_state.panels = panels;
        self.wgpu_state.text_runs = runs;
        self.wgpu_state.tray_layout = icons;
    }

    fn draw_header(&self, card: &CardLayout, font: &str, palette: &Palette, alpha: f32, runs: &mut Vec<TextRun>) {
        let Some(frame) = self.wgpu_state.orb_frame.as_ref() else {
            return;
        };
        let secs = frame.seconds_of_day as u32;
        let time = format!("{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60);
        let (time_size, date_size) = (30.0, 12.5);
        let block = crate::text::line_height(time_size) + crate::text::line_height(date_size) - 4.0;
        let top = card.header[1] + (card.header[3] - block) * 0.5;
        runs.push(TextRun {
            text: time,
            x: card.header[0],
            y: top,
            size: time_size,
            weight: LIGHT,
            color: with_alpha(palette.primary, alpha),
        });
        let (date, _) = crate::text::fit(&date_line(), font, date_size, REGULAR, card.header[2]);
        runs.push(TextRun {
            text: date,
            x: card.header[0] + 1.0,
            y: top + crate::text::line_height(time_size) - 4.0,
            size: date_size,
            weight: REGULAR,
            color: with_alpha(palette.secondary, alpha),
        });
    }

    /// The inline answer to pressing a destination that is not built yet, drawn above the card.
    pub fn card_note_draw(&mut self) -> Option<(PanelInstance, TextRun)> {
        let (dest, since) = self.card.note?;
        let age = since.elapsed();
        if age > NOTE_FOR || self.card.rect.is_none() {
            self.card.note = None;
            return None;
        }
        let anchor = self.card.destinations.iter().find(|(d, _)| *d == dest)?.1;
        let fade = ease(age.as_secs_f32() / 0.15) * ease((NOTE_FOR - age).as_secs_f32() / 0.3);
        let text = dest.unbuilt();
        let size = self.config.shell.font_size;
        let width = crate::text::measure(text, &self.config.shell.font, size, REGULAR) + 20.0;
        let height = crate::text::line_height(size).ceil() + 12.0;
        let card = self.card.rect?;
        let [x, y, _, _] = crate::shell::tooltip::outside(anchor, card, width, height, self.orb_zone());
        let mut color = parse_hex_color("#F2F4F8");
        color[3] = fade;
        Some((
            PanelInstance::new([x, y, width, height], 8.0, fade, PanelKind::Surface),
            TextRun { text: text.to_string(), x: x + 10.0, y: y + 6.0, size, weight: REGULAR, color },
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_tile(
    id: &ModuleId,
    module: Option<&ModuleState>,
    rect: [f32; 4],
    hovered: bool,
    font: &str,
    palette: &Palette,
    alpha: f32,
    panels: &mut Vec<PanelInstance>,
    runs: &mut Vec<TextRun>,
    icons: &mut Vec<TrayQuad>,
) {
    let fill = if hovered && module.is_some() { palette.tile_hover } else { palette.tile };
    panels.push(PanelInstance::fill(rect, TILE_RADIUS, with_alpha(fill, alpha)));

    let chip = [rect[0] + 14.0, rect[1] + (rect[3] - CHIP) * 0.5, CHIP, CHIP];
    let chip_color = match module {
        Some(m) if m.urgent => palette.amber,
        Some(m) if m.active => palette.accent,
        _ => palette.chip_off,
    };
    panels.push(PanelInstance::fill(chip, CHIP * 0.5, with_alpha(chip_color, alpha)));
    if module.is_some_and(|m| m.drawable()) {
        icons.push(TrayQuad {
            key: id.key(),
            rect: [
                chip[0] + (CHIP - TILE_ICON) * 0.5,
                chip[1] + (CHIP - TILE_ICON) * 0.5,
                TILE_ICON,
                TILE_ICON,
            ],
            alpha,
        });
    }

    let text_x = chip[0] + CHIP + RING_GAP + 10.0;
    let text_w = rect[0] + rect[2] - 10.0 - text_x;
    let fallback_title = match id {
        ModuleId::Network => "Network",
        ModuleId::Bluetooth => "Bluetooth",
        ModuleId::Audio => "Sound",
        ModuleId::Battery => "Battery",
        ModuleId::Tray(_) => "",
    };
    let (title, detail) = match module {
        Some(m) if !m.title.is_empty() => (m.title.as_str(), m.detail.as_str()),
        Some(_) => (fallback_title, ""),
        None => (fallback_title, "Unavailable"),
    };
    let (title, _) = crate::text::fit(title, font, 13.0, MEDIUM, text_w);
    let (detail, _) = crate::text::fit(detail, font, 11.5, REGULAR, text_w);
    let text_top = rect[1] + (rect[3] - 31.0) * 0.5;
    runs.push(TextRun {
        text: title,
        x: text_x,
        y: text_top,
        size: 13.0,
        weight: MEDIUM,
        color: with_alpha(palette.primary, alpha),
    });
    runs.push(TextRun {
        text: detail,
        x: text_x,
        y: text_top + 17.0,
        size: 11.5,
        weight: REGULAR,
        color: with_alpha(palette.secondary, alpha),
    });

    if let Some(level) = module.and_then(|m| m.level) {
        let ring = [chip[0] - RING_GAP, chip[1] - RING_GAP, CHIP + RING_GAP * 2.0, CHIP + RING_GAP * 2.0];
        let lit = if module.is_some_and(|m| m.urgent) { palette.amber } else { palette.primary };
        panels.push(PanelInstance::ring(ring, level.clamp(0.0, 1.0), with_alpha(lit, alpha * 0.9)));
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

    fn contains(outer: [f32; 4], inner: [f32; 4]) -> bool {
        inner[0] >= outer[0] - 0.01
            && inner[1] >= outer[1] - 0.01
            && inner[0] + inner[2] <= outer[0] + outer[2] + 0.01
            && inner[1] + inner[3] <= outer[1] + outer[3] + 0.01
    }

    #[test]
    fn the_card_holds_the_orb_in_its_corner_for_every_corner() {
        for corner in [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight] {
            let orb = orb_at(corner);
            let card = layout(corner, orb, 0);
            assert!(contains(card.rect, orb), "{corner:?}: orb outside card");
            let near_x = if matches!(corner, Corner::TopRight | Corner::BottomRight) {
                card.rect[0] + card.rect[2] - (orb[0] + orb[2])
            } else {
                orb[0] - card.rect[0]
            };
            assert!((near_x - INSET).abs() < 0.01, "{corner:?}: inset {near_x}");
        }
    }

    #[test]
    fn everything_sits_inside_the_card_and_nothing_overlaps_the_orb() {
        for corner in [Corner::TopLeft, Corner::BottomRight] {
            let orb = orb_at(corner);
            let card = layout(corner, orb, 3);
            let mut parts: Vec<[f32; 4]> = card.tiles.to_vec();
            parts.extend(card.destinations);
            parts.extend(card.tray.iter().copied());
            for part in parts {
                assert!(contains(card.rect, part), "{corner:?}: {part:?} escapes the card");
                let clear = part[0] + part[2] <= orb[0]
                    || part[0] >= orb[0] + orb[2]
                    || part[1] + part[3] <= orb[1]
                    || part[1] >= orb[1] + orb[3];
                assert!(clear, "{corner:?}: {part:?} overlaps the orb");
            }
        }
    }

    #[test]
    fn sections_stack_away_from_the_orb() {
        let bottom = layout(Corner::BottomRight, orb_at(Corner::BottomRight), 1);
        assert!(bottom.tiles[0][1] < bottom.header[1], "tiles sit above a bottom orb");
        assert!(bottom.destinations[0][1] < bottom.tiles[0][1]);
        assert!(bottom.tray[0][1] < bottom.destinations[0][1]);

        let top = layout(Corner::TopLeft, orb_at(Corner::TopLeft), 1);
        assert!(top.tiles[0][1] > top.header[1], "tiles sit below a top orb");
        assert!(top.destinations[0][1] > top.tiles[0][1]);
    }

    #[test]
    fn content_is_not_mirrored_so_wifi_stays_top_left() {
        for corner in [Corner::TopLeft, Corner::BottomRight] {
            let card = layout(corner, orb_at(corner), 0);
            assert!(card.tiles[0][0] < card.tiles[1][0], "{corner:?}");
            assert!(card.tiles[0][1] < card.tiles[2][1], "{corner:?}");
        }
    }

    #[test]
    fn a_card_without_tray_items_has_no_background_row() {
        let card = layout(Corner::BottomRight, orb_at(Corner::BottomRight), 0);
        assert!(card.tray_label.is_none() && card.tray.is_empty());
        let with = layout(Corner::BottomRight, orb_at(Corner::BottomRight), 2);
        assert!(with.rect[3] > card.rect[3]);
    }

    #[test]
    fn lightening_is_perceptual_and_monotonic() {
        let body = parse_hex_color("#12161C");
        let a = lighten(body, 0.05);
        let b = lighten(body, 0.1);
        assert!(a[0] > body[0] && b[0] > a[0]);
        assert!(b[0] < 0.05, "a 10% perceptual lift must stay dark in linear terms: {}", b[0]);
    }
}
