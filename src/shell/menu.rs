use smithay::utils::{Logical, Point, Rectangle};

use crate::renderer::panel::PanelInstance;
use crate::renderer::text::TextRun;
use crate::scene::animation::spring_step;
use crate::shell::tray::menu::{MenuNode, Toggle};
use crate::state::SpatilState;

const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 5.0;
const GUTTER: f32 = 18.0;
const SEPARATOR_H: f32 = 7.0;
const RADIUS: f32 = 8.0;
const MIN_WIDTH: f32 = 150.0;
const MAX_WIDTH: f32 = 360.0;
const GAP: f32 = 8.0;

const CHECK: &str = "✓";
const RADIO: &str = "•";
const ARROW: &str = "›";

pub struct OpenMenu {
    pub key: String,
    pub bus: String,
    pub path: String,
    pub root: MenuNode,
    pub trail: Vec<usize>,
    pub highlighted: Option<usize>,
    pub anchor: [f32; 4],
}

#[derive(Default)]
pub struct MenuRuntime {
    open: Option<OpenMenu>,
    rect: [f32; 4],
    rows: Vec<(usize, [f32; 4])>,
    alpha: f32,
    velocity: f32,
}

impl MenuRuntime {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn visible(&self) -> bool {
        self.open.is_some() && self.alpha > 0.01
    }
}

impl OpenMenu {
    /// The submenu currently drilled into, or the root.
    pub fn level(&self) -> &[MenuNode] {
        let mut current = &self.root.children;
        for index in &self.trail {
            let Some(node) = current.get(*index) else {
                break;
            };
            current = &node.children;
        }
        current
    }

    pub fn title(&self) -> Option<&str> {
        let mut node = &self.root;
        for index in &self.trail {
            node = node.children.get(*index)?;
        }
        (!self.trail.is_empty()).then_some(node.label.as_str())
    }
}

fn row_height(font_size: f32) -> f32 {
    crate::text::line_height(font_size).ceil() + PAD_Y * 2.0
}

/// Measures the panel and each row's box, in panel-local coordinates.
fn layout(
    items: &[MenuNode],
    font: &str,
    font_size: f32,
) -> (f32, f32, Vec<(usize, [f32; 4])>) {
    let row_h = row_height(font_size);
    let mut width: f32 = MIN_WIDTH;
    for node in items {
        if node.separator {
            continue;
        }
        let text_w = crate::text::measure(&node.label, font, font_size);
        width = width.max(text_w + GUTTER + PAD_X * 2.0 + GUTTER);
    }
    width = width.min(MAX_WIDTH);

    let mut rows = Vec::new();
    let mut y = PAD_Y;
    for (index, node) in items.iter().enumerate() {
        let h = if node.separator { SEPARATOR_H } else { row_h };
        rows.push((index, [0.0, y, width, h]));
        y += h;
    }
    (width, y + PAD_Y, rows)
}

/// Places the panel beside `anchor`, flipping and clamping to stay in `zone`.
fn place(
    anchor: [f32; 4],
    width: f32,
    height: f32,
    zone: Rectangle<i32, Logical>,
) -> [f32; 4] {
    let (zx, zy) = (zone.loc.x as f32, zone.loc.y as f32);
    let (zw, zh) = (zone.size.w as f32, zone.size.h as f32);

    let right_room = (zx + zw) - (anchor[0] + anchor[2]);
    let left_room = anchor[0] - zx;
    let x = if right_room >= width + GAP || right_room >= left_room {
        anchor[0] + anchor[2] + GAP
    } else {
        anchor[0] - GAP - width
    };

    let below_room = (zy + zh) - anchor[1];
    let y = if below_room >= height {
        anchor[1]
    } else {
        anchor[1] + anchor[3] - height
    };

    [
        x.clamp(zx, (zx + zw - width).max(zx)),
        y.clamp(zy, (zy + zh - height).max(zy)),
        width,
        height,
    ]
}

impl SpatilState {
    pub fn menu_is_open(&self) -> bool {
        self.menu.is_open()
    }

    pub fn open_menu(&mut self, key: String, bus: String, path: String, root: MenuNode, anchor: [f32; 4]) {
        if root.children.is_empty() {
            tracing::debug!(key, "tray menu is empty; not opening");
            return;
        }
        self.menu.open = Some(OpenMenu {
            key,
            bus,
            path,
            root,
            trail: Vec::new(),
            highlighted: None,
            anchor,
        });
    }

    pub fn close_menu(&mut self) -> bool {
        self.menu.rows.clear();
        self.menu.open.take().is_some()
    }

    pub fn tick_menu(&mut self, dt: f32) {
        let target = if self.menu.open.is_some() { 1.0 } else { 0.0 };
        let (alpha, velocity) =
            spring_step(self.menu.alpha, self.menu.velocity, target, 240.0, 27.0, dt);
        self.menu.alpha = alpha.clamp(0.0, 1.0);
        self.menu.velocity = velocity;

        let Some(open) = &self.menu.open else {
            return;
        };
        let font = self.config.shell.font.clone();
        let font_size = self.config.shell.font_size;
        let (width, height, rows) = layout(open.level(), &font, font_size);
        let rect = place(open.anchor, width, height, self.orb_zone());

        self.menu.rect = rect;
        self.menu.rows = rows
            .into_iter()
            .map(|(index, r)| (index, [rect[0] + r[0], rect[1] + r[1], r[2], r[3]]))
            .collect();
    }

    pub fn menu_at(&self, point: Point<f64, Logical>) -> Option<usize> {
        if !self.menu.visible() {
            return None;
        }
        self.menu.rows.iter().find_map(|(index, rect)| {
            let inside = point.x >= rect[0] as f64
                && point.x <= (rect[0] + rect[2]) as f64
                && point.y >= rect[1] as f64
                && point.y <= (rect[1] + rect[3]) as f64;
            inside.then_some(*index)
        })
    }

    pub fn menu_contains(&self, point: Point<f64, Logical>) -> bool {
        if !self.menu.visible() {
            return false;
        }
        let r = self.menu.rect;
        point.x >= r[0] as f64
            && point.x <= (r[0] + r[2]) as f64
            && point.y >= r[1] as f64
            && point.y <= (r[1] + r[3]) as f64
    }

    pub fn menu_hover(&mut self, point: Point<f64, Logical>) {
        let index = self.menu_at(point);
        if let Some(open) = &mut self.menu.open {
            open.highlighted = index;
        }
    }

    /// Activates the highlighted row: descends into a submenu, or fires the item.
    pub fn menu_activate(&mut self, index: usize) {
        let Some(open) = &self.menu.open else {
            return;
        };
        let Some(node) = open.level().get(index) else {
            return;
        };
        if !node.selectable() {
            return;
        }
        if node.submenu && !node.children.is_empty() {
            if let Some(open) = &mut self.menu.open {
                open.trail.push(index);
                open.highlighted = None;
            }
            return;
        }

        let (bus, path, id) = (open.bus.clone(), open.path.clone(), node.id);
        self.close_menu();
        self.close_orb();
        self.send_menu_event(bus, path, id);
    }

    pub fn menu_back(&mut self) -> bool {
        let Some(open) = &mut self.menu.open else {
            return false;
        };
        if open.trail.pop().is_some() {
            open.highlighted = None;
            return true;
        }
        false
    }

    /// Moves the highlight by `delta`, skipping separators and disabled rows.
    pub fn menu_step(&mut self, delta: isize) {
        let Some(open) = &self.menu.open else {
            return;
        };
        let items = open.level();
        let selectable: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, node)| node.selectable())
            .map(|(index, _)| index)
            .collect();
        if selectable.is_empty() {
            return;
        }
        let position = open
            .highlighted
            .and_then(|current| selectable.iter().position(|index| *index == current));
        let next = match position {
            Some(pos) => {
                let len = selectable.len() as isize;
                selectable[(((pos as isize + delta) % len + len) % len) as usize]
            }
            None if delta >= 0 => selectable[0],
            None => selectable[selectable.len() - 1],
        };
        if let Some(open) = &mut self.menu.open {
            open.highlighted = Some(next);
        }
    }

    /// Jumps the highlight to the first selectable row, or the last when `end`.
    pub fn menu_home(&mut self, end: bool) {
        if let Some(open) = &mut self.menu.open {
            open.highlighted = None;
        }
        self.menu_step(if end { -1 } else { 1 });
    }

    pub fn menu_highlighted(&self) -> Option<usize> {
        self.menu.open.as_ref().and_then(|open| open.highlighted)
    }

    pub fn menu_draw(&self, panels: &mut Vec<PanelInstance>, runs: &mut Vec<TextRun>) {
        if !self.menu.visible() {
            return;
        }
        let Some(open) = &self.menu.open else {
            return;
        };
        let items = open.level();
        let alpha = self.menu.alpha;
        let font_size = self.config.shell.font_size;

        panels.push(PanelInstance::new(self.menu.rect, RADIUS, alpha, 0.0));

        let mut text = crate::config::parse_hex_color("#F2F4F8");
        let mut dim = crate::config::parse_hex_color("#8A909B");

        for (index, rect) in &self.menu.rows {
            let Some(node) = items.get(*index) else {
                continue;
            };
            if node.separator {
                let line = [rect[0] + PAD_X, rect[1] + rect[3] * 0.5, rect[2] - PAD_X * 2.0, 1.0];
                panels.push(PanelInstance::new(line, 0.0, alpha * 0.35, 0.0));
                continue;
            }
            if open.highlighted == Some(*index) && node.enabled {
                panels.push(PanelInstance::new(*rect, RADIUS * 0.5, alpha, 1.0));
            }

            let color = if node.enabled {
                text[3] = alpha;
                text
            } else {
                dim[3] = alpha * 0.8;
                dim
            };
            let baseline_y = rect[1] + PAD_Y;

            if let Some(toggle) = node.toggle {
                let (mark, on) = match toggle {
                    Toggle::Checkmark(on) => (CHECK, on),
                    Toggle::Radio(on) => (RADIO, on),
                };
                if on {
                    runs.push(TextRun {
                        text: mark.to_string(),
                        x: rect[0] + PAD_X,
                        y: baseline_y,
                        size: font_size,
                        color,
                    });
                }
            }

            runs.push(TextRun {
                text: node.label.clone(),
                x: rect[0] + PAD_X + GUTTER,
                y: baseline_y,
                size: font_size,
                color,
            });

            if node.submenu {
                let arrow_w = crate::text::measure(ARROW, &self.config.shell.font, font_size);
                runs.push(TextRun {
                    text: ARROW.to_string(),
                    x: rect[0] + rect[2] - PAD_X - arrow_w,
                    y: baseline_y,
                    size: font_size,
                    color,
                });
            }
        }
    }

    pub fn menu_report(&self) -> serde_json::Value {
        let Some(open) = &self.menu.open else {
            return serde_json::json!({ "open": false });
        };
        serde_json::json!({
            "open": true,
            "key": open.key,
            "path": open.path,
            "depth": open.trail.len(),
            "title": open.title(),
            "highlighted": open.highlighted,
            "alpha": self.menu.alpha,
            "rect": self.menu.rect,
            "rows": open.level().iter().map(|node| serde_json::json!({
                "id": node.id,
                "label": node.label,
                "enabled": node.enabled,
                "separator": node.separator,
                "submenu": node.submenu,
                "checked": match node.toggle {
                    Some(Toggle::Checkmark(on)) | Some(Toggle::Radio(on)) => Some(on),
                    None => None,
                },
            })).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((0, 0).into(), (w, h).into())
    }

    fn item(id: i32, label: &str) -> MenuNode {
        MenuNode {
            id,
            label: label.to_string(),
            ..Default::default()
        }
    }

    fn separator() -> MenuNode {
        MenuNode {
            separator: true,
            ..Default::default()
        }
    }

    fn menu(items: Vec<MenuNode>) -> OpenMenu {
        OpenMenu {
            key: "k".into(),
            bus: ":1.1".into(),
            path: "/MenuBar".into(),
            root: MenuNode {
                children: items,
                ..Default::default()
            },
            trail: Vec::new(),
            highlighted: None,
            anchor: [0.0; 4],
        }
    }

    #[test]
    fn the_root_level_is_the_roots_children() {
        let open = menu(vec![item(1, "One"), item(2, "Two")]);
        assert_eq!(open.level().len(), 2);
        assert!(open.title().is_none(), "the root has no breadcrumb");
    }

    #[test]
    fn drilling_into_a_submenu_changes_the_level_and_the_title() {
        let mut child = item(10, "Networks");
        child.children = vec![item(11, "Home"), item(12, "Cafe")];
        child.submenu = true;
        let mut open = menu(vec![item(1, "One"), child]);

        open.trail.push(1);
        assert_eq!(open.level().len(), 2);
        assert_eq!(open.title(), Some("Networks"));
        assert_eq!(open.level()[0].label, "Home");
    }

    #[test]
    fn a_trail_pointing_at_nothing_does_not_panic() {
        let mut open = menu(vec![item(1, "One")]);
        open.trail.push(99);
        let _ = open.level();
        assert!(open.title().is_none());
    }

    #[test]
    fn every_row_gets_a_box_and_separators_are_shorter() {
        let items = vec![item(1, "One"), separator(), item(2, "Two")];
        let (width, height, rows) = layout(&items, "sans-serif", 12.0);
        assert_eq!(rows.len(), 3);
        assert!(width >= MIN_WIDTH && width <= MAX_WIDTH);
        assert!(rows[1].1[3] < rows[0].1[3], "the separator row is thinner");
        let total: f32 = rows.iter().map(|(_, r)| r[3]).sum();
        assert!(height >= total, "the panel encloses every row");
    }

    #[test]
    fn rows_stack_downwards_without_overlapping() {
        let items = vec![item(1, "One"), item(2, "Two"), item(3, "Three")];
        let (_, _, rows) = layout(&items, "sans-serif", 12.0);
        for pair in rows.windows(2) {
            let (a, b) = (pair[0].1, pair[1].1);
            assert!((a[1] + a[3] - b[1]).abs() < 0.01, "{a:?} then {b:?}");
        }
    }

    #[test]
    fn a_menu_opens_to_the_right_when_there_is_room() {
        let placed = place([100.0, 100.0, 26.0, 26.0], 180.0, 120.0, zone(1920, 1080));
        assert!(placed[0] >= 126.0);
    }

    #[test]
    fn a_menu_flips_left_and_up_rather_than_leave_the_zone() {
        let placed = place([1880.0, 1040.0, 26.0, 26.0], 200.0, 300.0, zone(1920, 1080));
        assert!(placed[0] + placed[2] <= 1920.0);
        assert!(placed[1] + placed[3] <= 1080.0);
        assert!(placed[0] >= 0.0 && placed[1] >= 0.0);
    }

    #[test]
    fn a_menu_taller_than_the_zone_is_still_pinned_inside_it() {
        let placed = place([10.0, 10.0, 26.0, 26.0], 200.0, 5000.0, zone(800, 600));
        assert!(placed[0] >= 0.0 && placed[1] >= 0.0);
    }
}
