mod bus;
mod icon;

use std::sync::mpsc::{Sender as StdSender, channel};

use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::channel::{Event, channel as calloop_channel};

use smithay::utils::{Logical, Point};

use crate::input::drag::{BTN_MIDDLE, BTN_RIGHT};
use crate::renderer::hud::TrayQuad;
use crate::renderer::orb::Corner;
use crate::state::SpatilState;

const ICON: f32 = 26.0;
const GAP: f32 = 10.0;
const RING_INSET: f32 = 20.0;
const MAX_RING_GROWTH: f32 = 2.6;
const STAGGER: f32 = 0.07;
const QUARTER: f32 = std::f32::consts::FRAC_PI_2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraySlot {
    pub centre: (f32, f32),
    pub size: f32,
}

fn arc_start(corner: Corner) -> f32 {
    match corner {
        Corner::TopLeft => 0.0,
        Corner::TopRight => QUARTER,
        Corner::BottomRight => std::f32::consts::PI,
        Corner::BottomLeft => 3.0 * QUARTER,
    }
}

pub fn tray_slots(
    corner: Corner,
    orb_centre: (f32, f32),
    orb_radius: f32,
    count: usize,
) -> Vec<TraySlot> {
    if count == 0 {
        return Vec::new();
    }
    let base = orb_radius + ICON * 0.5 + RING_INSET;
    let pitch = ICON + GAP;
    let needed = count as f32 * pitch / QUARTER;
    let radius = base.max(needed).min(base * MAX_RING_GROWTH);

    let step = pitch / radius;
    let fits = (((QUARTER / step).floor() as usize) + 1).max(1);
    let shown = count.min(fits);
    if shown < count {
        tracing::warn!("tray arc holds {shown} of {count} items; the rest are not shown");
    }

    let span = (shown as f32 - 1.0) * step;
    let mid = arc_start(corner) + QUARTER * 0.5;
    let first = mid - span * 0.5;
    (0..shown)
        .map(|i| {
            let angle = first + i as f32 * step;
            TraySlot {
                centre: (
                    orb_centre.0 + angle.cos() * radius,
                    orb_centre.1 + angle.sin() * radius,
                ),
                size: ICON,
            }
        })
        .collect()
}

fn stagger(open: f32, index: usize) -> f32 {
    let lo = (index as f32 * STAGGER).min(0.6);
    ((open - lo) / (1.0 - lo).max(0.001)).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayRole {
    Unavailable,
    Owner,
    Guest,
}

impl TrayRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Owner => "owner",
            Self::Guest => "guest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    Passive,
    Active,
    NeedsAttention,
}

impl TrayStatus {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "passive" => Self::Passive,
            "needsattention" => Self::NeedsAttention,
            _ => Self::Active,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Active => "active",
            Self::NeedsAttention => "needs_attention",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrayPixmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TrayItem {
    pub key: String,
    pub bus: String,
    pub owner: Option<String>,
    pub path: String,
    pub id: String,
    pub title: String,
    pub status: TrayStatus,
    pub icon_name: String,
    pub pixmap: Option<TrayPixmap>,
    pub menu: Option<String>,
}

impl TrayItem {
    pub fn visible(&self) -> bool {
        self.status != TrayStatus::Passive
    }

    pub fn answers_to(&self, name: &str) -> bool {
        self.bus == name || self.owner.as_deref() == Some(name)
    }

    fn pixels(&self) -> Option<&[u8]> {
        self.pixmap.as_ref().map(|p| p.bgra.as_slice())
    }
}

pub enum TrayEvent {
    Unavailable,
    Role(TrayRole),
    Discovered { bus: String, path: String },
    Changed(String),
    OwnerLost(String),
    Updated(Box<TrayItem>),
    Removed(String),
}

pub enum TrayRequest {
    Refresh { bus: String, path: String },
    Activate { bus: String, path: String, x: i32, y: i32 },
    SecondaryActivate { bus: String, path: String, x: i32, y: i32 },
    ContextMenu { bus: String, path: String, x: i32, y: i32 },
    Scroll { bus: String, path: String, delta: i32 },
}

impl TrayRequest {
    pub fn address(&self) -> (&str, &str) {
        match self {
            Self::Refresh { bus, path }
            | Self::Activate { bus, path, .. }
            | Self::SecondaryActivate { bus, path, .. }
            | Self::ContextMenu { bus, path, .. }
            | Self::Scroll { bus, path, .. } => (bus, path),
        }
    }
}

#[derive(Default)]
pub struct Tray {
    pub role: TrayRole,
    items: Vec<TrayItem>,
    requests: Option<StdSender<TrayRequest>>,
    stale_icons: Vec<String>,
}

impl Default for TrayRole {
    fn default() -> Self {
        Self::Unavailable
    }
}

impl Tray {
    pub fn attach(&mut self, requests: Option<StdSender<TrayRequest>>) {
        self.requests = requests;
    }

    pub fn items(&self) -> &[TrayItem] {
        &self.items
    }

    pub fn visible_items(&self) -> impl Iterator<Item = &TrayItem> {
        self.items.iter().filter(|item| item.visible())
    }

    pub fn request(&self, request: TrayRequest) {
        let Some(tx) = &self.requests else {
            return;
        };
        if tx.send(request).is_err() {
            tracing::debug!("tray request channel closed");
        }
    }

    fn apply(&mut self, event: TrayEvent) -> Option<TrayRequest> {
        match event {
            TrayEvent::Unavailable => {
                self.role = TrayRole::Unavailable;
                self.items.clear();
                None
            }
            TrayEvent::Role(role) => {
                self.role = role;
                None
            }
            TrayEvent::Discovered { bus, path } => {
                let key = format!("{bus}{path}");
                (!self.items.iter().any(|item| item.key == key))
                    .then_some(TrayRequest::Refresh { bus, path })
            }
            TrayEvent::Changed(sender) => self
                .items
                .iter()
                .find(|item| item.answers_to(&sender))
                .map(|item| TrayRequest::Refresh {
                    bus: item.bus.clone(),
                    path: item.path.clone(),
                }),
            TrayEvent::OwnerLost(name) => {
                self.items.retain(|item| !item.answers_to(&name));
                None
            }
            TrayEvent::Updated(item) => {
                match self.items.iter_mut().find(|existing| existing.key == item.key) {
                    Some(existing) => {
                        if existing.pixels() != item.pixels() {
                            self.stale_icons.push(item.key.clone());
                        }
                        *existing = *item;
                    }
                    None => {
                        tracing::info!(
                            "tray item appeared: {} ({})",
                            if item.title.is_empty() { &item.id } else { &item.title },
                            item.key
                        );
                        self.items.push(*item);
                    }
                }
                None
            }
            TrayEvent::Removed(key) => {
                self.items.retain(|item| item.key != key);
                None
            }
        }
    }
}

pub fn init_tray(loop_handle: &LoopHandle<'static, SpatilState>) -> Option<StdSender<TrayRequest>> {
    let (event_tx, event_rx) = calloop_channel::<TrayEvent>();
    let (request_tx, request_rx) = channel::<TrayRequest>();

    let inserted = loop_handle.insert_source(event_rx, |event, _, state: &mut SpatilState| {
        if let Event::Msg(event) = event
            && let Some(followup) = state.tray.apply(event)
        {
            state.tray.request(followup);
        }
    });
    if let Err(err) = inserted {
        tracing::warn!("could not insert the tray event source: {err}; system tray disabled");
        return None;
    }

    let spawned = std::thread::Builder::new()
        .name("spatil-tray".into())
        .spawn(move || bus::run(event_tx, request_rx));
    match spawned {
        Ok(_) => Some(request_tx),
        Err(err) => {
            tracing::warn!("could not start the tray thread: {err}; system tray disabled");
            None
        }
    }
}

impl SpatilState {
    pub fn tick_tray(&mut self) {
        for key in std::mem::take(&mut self.tray.stale_icons) {
            self.wgpu_state.remove_tray_icon(&key);
        }

        if !self.wgpu_state.tray_icons.is_empty() {
            let live: Vec<String> = self.tray.items().iter().map(|i| i.key.clone()).collect();
            self.wgpu_state
                .tray_icons
                .retain(|key, _| live.iter().any(|k| k == key));
        }

        let open = self
            .wgpu_state
            .orb_frame
            .as_ref()
            .map_or(0.0, |frame| frame.open);
        if open <= 0.002 {
            self.wgpu_state.tray_layout.clear();
            return;
        }
        let Some(rect) = self.orb_rect() else {
            self.wgpu_state.tray_layout.clear();
            return;
        };

        let pending: Vec<(String, u32, u32, Vec<u8>)> = self
            .tray
            .visible_items()
            .filter(|item| !self.wgpu_state.has_tray_icon(&item.key))
            .filter_map(|item| {
                item.pixmap
                    .as_ref()
                    .map(|p| (item.key.clone(), p.width, p.height, p.bgra.clone()))
            })
            .collect();
        for (key, width, height, bgra) in pending {
            self.wgpu_state.upload_tray_icon(key, width, height, &bgra);
        }

        let drawable: Vec<String> = self
            .tray
            .visible_items()
            .filter(|item| self.wgpu_state.has_tray_icon(&item.key))
            .map(|item| item.key.clone())
            .collect();

        let radius = rect.size.w as f32 * 0.5;
        let centre = (rect.loc.x as f32 + radius, rect.loc.y as f32 + radius);
        let slots = tray_slots(self.orb_corner(), centre, radius, drawable.len());

        self.wgpu_state.tray_layout = drawable
            .into_iter()
            .zip(slots)
            .enumerate()
            .filter_map(|(i, (key, slot))| {
                let reveal = stagger(open, i);
                (reveal > 0.002).then(|| {
                    let size = slot.size * (0.6 + 0.4 * reveal);
                    TrayQuad {
                        key,
                        rect: [
                            slot.centre.0 - size * 0.5,
                            slot.centre.1 - size * 0.5,
                            size,
                            size,
                        ],
                        alpha: reveal,
                    }
                })
            })
            .collect();
    }

    pub fn tray_item_at(&self, point: Point<f64, Logical>) -> Option<String> {
        self.wgpu_state.tray_layout.iter().find_map(|quad| {
            let half = quad.rect[2] as f64 * 0.5;
            let cx = quad.rect[0] as f64 + half;
            let cy = quad.rect[1] as f64 + half;
            let (dx, dy) = (point.x - cx, point.y - cy);
            (dx * dx + dy * dy <= half * half).then(|| quad.key.clone())
        })
    }

    fn tray_address(&self, key: &str) -> Option<(String, String)> {
        self.tray
            .items()
            .iter()
            .find(|item| item.key == key)
            .map(|item| (item.bus.clone(), item.path.clone()))
    }

    pub fn tray_press(&mut self, point: Point<f64, Logical>, button: u32) -> bool {
        let Some(key) = self.tray_item_at(point) else {
            return false;
        };
        let Some((bus, path)) = self.tray_address(&key) else {
            return false;
        };
        let (x, y) = (point.x as i32, point.y as i32);
        let request = match button {
            BTN_RIGHT => TrayRequest::ContextMenu { bus, path, x, y },
            BTN_MIDDLE => TrayRequest::SecondaryActivate { bus, path, x, y },
            _ => TrayRequest::Activate { bus, path, x, y },
        };
        self.tray.request(request);
        true
    }

    pub fn tray_activate_index(&mut self, index: usize) -> bool {
        let Some((bus, path)) = self
            .tray
            .visible_items()
            .nth(index)
            .map(|item| (item.bus.clone(), item.path.clone()))
        else {
            return false;
        };
        self.tray.request(TrayRequest::Activate { bus, path, x: 0, y: 0 });
        true
    }

    pub fn tray_scroll(&mut self, point: Point<f64, Logical>, delta: i32) -> bool {
        let Some(key) = self.tray_item_at(point) else {
            return false;
        };
        let Some((bus, path)) = self.tray_address(&key) else {
            return false;
        };
        self.tray.request(TrayRequest::Scroll { bus, path, delta });
        true
    }
}

pub fn probe() {
    use std::time::{Duration, Instant};

    let (event_tx, event_rx) = calloop_channel::<TrayEvent>();
    let (_request_tx, request_rx) = channel::<TrayRequest>();
    let (report_tx, report_rx) = channel::<TrayEvent>();

    std::thread::spawn(move || bus::run(event_tx, request_rx));
    std::thread::spawn(move || {
        let mut event_loop = smithay::reexports::calloop::EventLoop::<Vec<TrayEvent>>::try_new()
            .expect("probe event loop");
        event_loop
            .handle()
            .insert_source(event_rx, |event, _, out: &mut Vec<TrayEvent>| {
                if let Event::Msg(msg) = event {
                    out.push(msg);
                }
            })
            .expect("probe source");
        let mut drained = Vec::new();
        loop {
            if event_loop
                .dispatch(Duration::from_millis(200), &mut drained)
                .is_err()
            {
                break;
            }
            for event in drained.drain(..) {
                if report_tx.send(event).is_err() {
                    return;
                }
            }
        }
    });

    let mut tray = Tray::default();
    let deadline = Instant::now() + Duration::from_secs(20);
    println!("PROBE: watching the session bus for tray activity");
    while Instant::now() < deadline {
        let Ok(event) = report_rx.recv_timeout(Duration::from_millis(500)) else {
            continue;
        };
        match &event {
            TrayEvent::Role(role) => println!("PROBE: role={}", role.as_str()),
            TrayEvent::Unavailable => println!("PROBE: role=unavailable"),
            TrayEvent::Discovered { bus, path } => println!("PROBE: discovered {bus}{path}"),
            TrayEvent::Removed(key) => println!("PROBE: removed {key}"),
            TrayEvent::Changed(sender) => println!("PROBE: changed {sender}"),
            TrayEvent::OwnerLost(name) => println!("PROBE: owner gone {name}"),
            TrayEvent::Updated(item) => println!(
                "PROBE: item id={} title={:?} status={} pixmap={:?} menu={}",
                item.id,
                item.title,
                item.status.as_str(),
                item.pixmap.as_ref().map(|p| (p.width, p.height)),
                item.menu.is_some(),
            ),
        }
        if let Some(followup) = tray.apply(event) {
            let (bus, path) = followup.address();
            let (bus, path) = (bus.to_string(), path.to_string());
            let connection = pollster::block_on(zbus::Connection::session());
            if let Ok(connection) = connection
                && let Ok(item) = pollster::block_on(bus::read_item(&connection, &bus, &path))
            {
                println!(
                    "PROBE: item id={} title={:?} status={} pixmap={:?} menu={}",
                    item.id,
                    item.title,
                    item.status.as_str(),
                    item.pixmap.as_ref().map(|p| (p.width, p.height)),
                    item.menu.is_some(),
                );
                tray.apply(TrayEvent::Updated(Box::new(item)));
            }
        }
        println!("PROBE: tracking {} item(s)", tray.items().len());
    }
    println!("PROBE: done");
}

impl SpatilState {
    pub fn tray_report(&self) -> serde_json::Value {
        serde_json::json!({
            "role": self.tray.role.as_str(),
            "count": self.tray.items().len(),
            "visible": self.tray.visible_items().count(),
            "items": self
                .tray
                .items()
                .iter()
                .map(|item| {
                    serde_json::json!({
                        "key": item.key,
                        "id": item.id,
                        "title": item.title,
                        "status": item.status.as_str(),
                        "icon_name": item.icon_name,
                        "pixmap": item.pixmap.as_ref().map(|p| [p.width, p.height]),
                        "has_menu": item.menu.is_some(),
                    })
                })
                .collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, status: TrayStatus) -> TrayItem {
        TrayItem {
            key: key.to_string(),
            bus: ":1.1".to_string(),
            owner: None,
            path: "/StatusNotifierItem".to_string(),
            id: "demo".to_string(),
            title: "Demo".to_string(),
            status,
            icon_name: "demo".to_string(),
            pixmap: None,
            menu: None,
        }
    }

    #[test]
    fn a_discovery_asks_for_the_item_to_be_read() {
        let mut tray = Tray::default();
        let followup = tray.apply(TrayEvent::Discovered {
            bus: ":1.7".to_string(),
            path: "/StatusNotifierItem".to_string(),
        });
        assert!(matches!(followup, Some(TrayRequest::Refresh { .. })));
        assert_eq!(tray.items().len(), 0, "discovery alone adds nothing");
    }

    #[test]
    fn an_update_adds_then_replaces_rather_than_duplicating() {
        let mut tray = Tray::default();
        tray.apply(TrayEvent::Updated(Box::new(item("a", TrayStatus::Active))));
        tray.apply(TrayEvent::Updated(Box::new(item("a", TrayStatus::NeedsAttention))));
        assert_eq!(tray.items().len(), 1);
        assert_eq!(tray.items()[0].status, TrayStatus::NeedsAttention);
    }

    #[test]
    fn a_changed_icon_invalidates_the_cached_texture() {
        let mut tray = Tray::default();
        let mut first = item("a", TrayStatus::Active);
        first.pixmap = Some(TrayPixmap { width: 1, height: 1, bgra: vec![1, 2, 3, 4] });
        tray.apply(TrayEvent::Updated(Box::new(first.clone())));
        assert!(tray.stale_icons.is_empty(), "a first upload is not stale");

        let mut same = first.clone();
        same.title = "renamed".to_string();
        tray.apply(TrayEvent::Updated(Box::new(same)));
        assert!(tray.stale_icons.is_empty(), "an unchanged icon must not re-upload");

        let mut repainted = first.clone();
        repainted.pixmap = Some(TrayPixmap { width: 1, height: 1, bgra: vec![9, 9, 9, 9] });
        tray.apply(TrayEvent::Updated(Box::new(repainted)));
        assert_eq!(tray.stale_icons, vec!["a".to_string()]);
    }

    #[test]
    fn a_change_signal_refreshes_by_unique_name_or_well_known_name() {
        let mut tray = Tray::default();
        let mut tracked = item("k", TrayStatus::Active);
        tracked.bus = "org.kde.StatusNotifierItem-9-1".to_string();
        tracked.owner = Some(":1.4".to_string());
        tray.apply(TrayEvent::Updated(Box::new(tracked)));

        assert!(
            tray.apply(TrayEvent::Changed(":1.4".to_string())).is_some(),
            "a signal arrives from the unique name, not the well-known one",
        );
        assert!(
            tray.apply(TrayEvent::Changed("org.kde.StatusNotifierItem-9-1".to_string())).is_some()
        );
        assert!(tray.apply(TrayEvent::Changed(":1.99".to_string())).is_none());
    }

    #[test]
    fn passive_items_are_tracked_but_not_shown() {
        let mut tray = Tray::default();
        tray.apply(TrayEvent::Updated(Box::new(item("a", TrayStatus::Passive))));
        tray.apply(TrayEvent::Updated(Box::new(item("b", TrayStatus::Active))));
        assert_eq!(tray.items().len(), 2);
        assert_eq!(tray.visible_items().count(), 1);
    }

    #[test]
    fn a_repeat_discovery_does_not_re_read_a_known_item() {
        let mut tray = Tray::default();
        let mut known = item("x", TrayStatus::Active);
        known.bus = ":1.9".to_string();
        known.key = ":1.9/StatusNotifierItem".to_string();
        tray.apply(TrayEvent::Updated(Box::new(known)));

        let followup = tray.apply(TrayEvent::Discovered {
            bus: ":1.9".to_string(),
            path: "/StatusNotifierItem".to_string(),
        });
        assert!(followup.is_none(), "we hear our own registration signal too");
    }

    #[test]
    fn an_exiting_application_takes_its_items_with_it() {
        let mut tray = Tray::default();
        let mut a = item("a", TrayStatus::Active);
        a.bus = ":1.5".to_string();
        let mut b = item("b", TrayStatus::Active);
        b.bus = ":1.6".to_string();
        tray.apply(TrayEvent::Updated(Box::new(a)));
        tray.apply(TrayEvent::Updated(Box::new(b)));

        tray.apply(TrayEvent::OwnerLost(":1.5".to_string()));
        assert_eq!(tray.items().len(), 1);
        assert_eq!(tray.items()[0].bus, ":1.6");
    }

    #[test]
    fn removal_drops_only_the_named_item() {
        let mut tray = Tray::default();
        tray.apply(TrayEvent::Updated(Box::new(item("a", TrayStatus::Active))));
        tray.apply(TrayEvent::Updated(Box::new(item("b", TrayStatus::Active))));
        tray.apply(TrayEvent::Removed("a".to_string()));
        assert_eq!(tray.items().len(), 1);
        assert_eq!(tray.items()[0].key, "b");
    }

    #[test]
    fn losing_the_bus_clears_everything() {
        let mut tray = Tray::default();
        tray.apply(TrayEvent::Role(TrayRole::Owner));
        tray.apply(TrayEvent::Updated(Box::new(item("a", TrayStatus::Active))));
        tray.apply(TrayEvent::Unavailable);
        assert_eq!(tray.role, TrayRole::Unavailable);
        assert!(tray.items().is_empty());
    }

    #[test]
    fn an_empty_tray_has_no_slots() {
        assert!(tray_slots(Corner::BottomRight, (100.0, 100.0), 28.0, 0).is_empty());
    }

    #[test]
    fn the_arc_opens_into_the_screen_from_each_corner() {
        let centre = (1000.0, 800.0);
        let one = |corner| tray_slots(corner, centre, 28.0, 1)[0].centre;

        let br = one(Corner::BottomRight);
        assert!(br.0 < centre.0 && br.1 < centre.1, "bottom-right fans up-left: {br:?}");
        let bl = one(Corner::BottomLeft);
        assert!(bl.0 > centre.0 && bl.1 < centre.1, "bottom-left fans up-right: {bl:?}");
        let tl = one(Corner::TopLeft);
        assert!(tl.0 > centre.0 && tl.1 > centre.1, "top-left fans down-right: {tl:?}");
        let tr = one(Corner::TopRight);
        assert!(tr.0 < centre.0 && tr.1 > centre.1, "top-right fans down-left: {tr:?}");
    }

    #[test]
    fn items_sit_on_a_circle_and_do_not_overlap() {
        let centre = (1000.0, 800.0);
        let slots = tray_slots(Corner::BottomRight, centre, 28.0, 5);
        assert_eq!(slots.len(), 5);

        let radius = |s: &TraySlot| {
            ((s.centre.0 - centre.0).powi(2) + (s.centre.1 - centre.1).powi(2)).sqrt()
        };
        let first = radius(&slots[0]);
        for slot in &slots {
            assert!((radius(slot) - first).abs() < 0.01, "all slots share one radius");
        }
        for pair in slots.windows(2) {
            let gap = ((pair[0].centre.0 - pair[1].centre.0).powi(2)
                + (pair[0].centre.1 - pair[1].centre.1).powi(2))
            .sqrt();
            assert!(gap >= ICON, "neighbours must not overlap, gap was {gap}");
        }
    }

    #[test]
    fn a_crowded_tray_grows_its_ring_rather_than_bunching_up() {
        let centre = (1000.0, 800.0);
        let radius = |n| {
            let s = &tray_slots(Corner::BottomRight, centre, 28.0, n)[0];
            ((s.centre.0 - centre.0).powi(2) + (s.centre.1 - centre.1).powi(2)).sqrt()
        };
        assert!(radius(12) > radius(2), "more items push the arc outward");
    }

    #[test]
    fn the_reveal_staggers_but_always_finishes() {
        assert_eq!(stagger(0.0, 0), 0.0);
        assert_eq!(stagger(1.0, 0), 1.0);
        assert_eq!(stagger(1.0, 7), 1.0, "every icon is fully out when open");
        assert!(
            stagger(0.5, 0) > stagger(0.5, 4),
            "earlier icons lead the later ones",
        );
    }

    #[test]
    fn status_strings_parse_case_insensitively_and_default_to_active() {
        assert_eq!(TrayStatus::parse("Passive"), TrayStatus::Passive);
        assert_eq!(TrayStatus::parse("NeedsAttention"), TrayStatus::NeedsAttention);
        assert_eq!(TrayStatus::parse("Active"), TrayStatus::Active);
        assert_eq!(TrayStatus::parse("nonsense"), TrayStatus::Active);
    }
}
