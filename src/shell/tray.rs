mod bus;
pub mod icon;
pub mod menu;

use std::sync::mpsc::{Sender as StdSender, channel};

use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::channel::{Event, channel as calloop_channel};

use smithay::utils::{Logical, Point};

use crate::input::drag::{BTN_MIDDLE, BTN_RIGHT};
use crate::shell::module::{self, ModuleAction, ModuleId, ModuleState};
use crate::shell::tray::menu::MenuNode;
use crate::state::SpatilState;

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
    pub tooltip: String,
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
    Menu { key: String, root: Box<MenuNode> },
}

pub enum TrayRequest {
    Refresh { bus: String, path: String },
    Activate { bus: String, path: String, x: i32, y: i32 },
    SecondaryActivate { bus: String, path: String, x: i32, y: i32 },
    ContextMenu { bus: String, path: String, x: i32, y: i32 },
    Scroll { bus: String, path: String, delta: i32 },
    FetchMenu { bus: String, path: String, key: String },
    MenuEvent { bus: String, path: String, id: i32 },
}

impl TrayRequest {
    pub fn address(&self) -> (&str, &str) {
        match self {
            Self::Refresh { bus, path }
            | Self::Activate { bus, path, .. }
            | Self::SecondaryActivate { bus, path, .. }
            | Self::ContextMenu { bus, path, .. }
            | Self::Scroll { bus, path, .. }
            | Self::FetchMenu { bus, path, .. }
            | Self::MenuEvent { bus, path, .. } => (bus, path),
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
            // Routed to SpatilState before it reaches here; see `init_tray`.
            TrayEvent::Menu { .. } => None,
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
        let Event::Msg(event) = event else {
            return;
        };
        if let TrayEvent::Menu { key, root } = event {
            state.present_tray_menu(key, *root);
            return;
        }
        if let Some(followup) = state.tray.apply(event) {
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
    pub fn shell_modules(&self) -> Vec<ModuleState> {
        let mut modules: Vec<ModuleState> = [
            self.network.as_ref(),
            self.bluetooth.as_ref(),
            self.audio.as_ref(),
            self.battery.as_ref(),
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
        modules.extend(
            self.tray
            .visible_items()
            .map(|item| {
                let mut module = ModuleState::new(ModuleId::Tray(item.key.clone()));
                module.icon = item.pixmap.clone().map(std::sync::Arc::new);
                module.label = item.title.clone();
                module.tooltip = if item.tooltip.is_empty() {
                    item.title.clone()
                } else {
                    item.tooltip.clone()
                };
                module.urgent = item.status == TrayStatus::NeedsAttention;
                module
            }),
        );
        module::order(&mut modules);
        modules
    }

    pub fn tick_modules(&mut self) {
        self.wgpu_state.panels.clear();
        self.wgpu_state.text_runs.clear();
        for key in std::mem::take(&mut self.tray.stale_icons) {
            self.wgpu_state.remove_tray_icon(&ModuleId::Tray(key).key());
        }

        let open = self
            .wgpu_state
            .orb_frame
            .as_ref()
            .map_or(0.0, |frame| frame.open);
        let modules = self.shell_modules();

        if !self.wgpu_state.tray_icons.is_empty() {
            let live: Vec<String> = modules.iter().map(|m| m.id.key()).collect();
            self.wgpu_state
                .tray_icons
                .retain(|key, _| key.starts_with("dest:") || live.iter().any(|k| k == key));
        }

        for module in modules.iter().filter(|m| m.drawable()) {
            let key = module.id.key();
            if self.wgpu_state.has_tray_icon(&key) {
                continue;
            }
            let Some(icon) = &module.icon else { continue };
            let (w, h, bgra) = (icon.width, icon.height, icon.bgra.clone());
            self.wgpu_state.upload_tray_icon(key, w, h, &bgra);
        }

        let warn = modules.iter().any(|m| m.urgent && !matches!(m.id, ModuleId::Tray(_)));
        self.draw_card(&modules, open, warn);
        self.draw_banner();
    }

    pub fn module_at(&self, point: Point<f64, Logical>) -> Option<ModuleId> {
        let (x, y) = (point.x as f32, point.y as f32);
        self.module_slots
            .iter()
            .find(|(_, r)| x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3])
            .map(|(id, _)| id.clone())
    }

    fn tray_address(&self, key: &str) -> Option<(String, String)> {
        self.tray
            .items()
            .iter()
            .find(|item| item.key == key)
            .map(|item| (item.bus.clone(), item.path.clone()))
    }

    pub fn module_action(
        &mut self,
        id: &ModuleId,
        action: ModuleAction,
        point: Point<f64, Logical>,
    ) -> bool {
        let ModuleId::Tray(key) = id else {
            if action == ModuleAction::Activate || action == ModuleAction::Context {
                match crate::shell::launch::resolve(id, &self.config.shell) {
                    Some(command) => {
                        if crate::shell::launch::spawn(&command) {
                            self.expect_centred_window();
                            self.close_orb();
                        }
                    }
                    None => tracing::warn!("{}", crate::shell::launch::describe_missing(id)),
                }
            }
            return true;
        };
        let Some((bus, path)) = self.tray_address(key) else {
            return false;
        };
        let (x, y) = (point.x as i32, point.y as i32);
        let request = match action {
            ModuleAction::Activate => TrayRequest::Activate { bus, path, x, y },
            ModuleAction::Secondary => TrayRequest::SecondaryActivate { bus, path, x, y },
            // Prefer drawing the menu ourselves; ContextMenu asks the application
            // to draw its own, which it cannot do usefully over a 3D compositor.
            ModuleAction::Context => match self.tray_menu_path(key) {
                Some(menu_path) => {
                    self.pending_menu = self.module_slots
                        .iter()
                        .find(|(slot, _)| slot == id)
                        .map(|(_, rect)| *rect);
                    TrayRequest::FetchMenu { bus, path: menu_path, key: key.clone() }
                }
                None => TrayRequest::ContextMenu { bus, path, x, y },
            },
            ModuleAction::ScrollUp => TrayRequest::Scroll { bus, path, delta: -1 },
            ModuleAction::ScrollDown => TrayRequest::Scroll { bus, path, delta: 1 },
        };
        self.tray.request(request);
        true
    }

    fn tray_menu_path(&self, key: &str) -> Option<String> {
        self.tray
            .visible_items()
            .find(|item| item.key == key)
            .and_then(|item| item.menu.clone())
            .filter(|path| path != "/")
    }

    pub fn present_tray_menu(&mut self, key: String, root: MenuNode) {
        let anchor = self
            .pending_menu
            .take()
            .or_else(|| {
                self.module_slots
                    .iter()
                    .find(|(slot, _)| slot.key() == key)
                    .map(|(_, rect)| *rect)
            })
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let Some((bus, _)) = self.tray_address(&key) else {
            return;
        };
        let Some(path) = self.tray_menu_path(&key) else {
            return;
        };
        self.open_menu(key, bus, path, root, anchor);
    }

    pub fn send_menu_event(&mut self, bus: String, path: String, id: i32) {
        self.tray.request(TrayRequest::MenuEvent { bus, path, id });
    }

    pub fn module_press(&mut self, point: Point<f64, Logical>, button: u32) -> bool {
        let Some(id) = self.module_at(point) else {
            return false;
        };
        let action = match button {
            BTN_RIGHT => ModuleAction::Context,
            BTN_MIDDLE => ModuleAction::Secondary,
            _ => ModuleAction::Activate,
        };
        self.module_action(&id, action, point)
    }

    pub fn module_scroll(&mut self, point: Point<f64, Logical>, delta: i32) -> bool {
        let Some(id) = self.module_at(point) else {
            return false;
        };
        let action = if delta < 0 { ModuleAction::ScrollUp } else { ModuleAction::ScrollDown };
        self.module_action(&id, action, point)
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
}

fn print_menu(node: &MenuNode, depth: usize) {
    let indent = "  ".repeat(depth);
    for child in &node.children {
        if child.separator {
            println!("{indent}---");
            continue;
        }
        let mut marks = Vec::new();
        if child.submenu {
            marks.push("submenu".to_string());
        }
        if let Some(toggle) = child.toggle {
            marks.push(format!("{toggle:?}"));
        }
        if !child.enabled {
            marks.push("disabled".to_string());
        }
        let suffix = if marks.is_empty() {
            String::new()
        } else {
            format!("  [{}]", marks.join(", "))
        };
        println!("{indent}{:?} (id {}){suffix}", child.label, child.id);
        print_menu(child, depth + 1);
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
            TrayEvent::Menu { key, root } => {
                println!("PROBE: menu for {key}");
                print_menu(root, 1);
            }
            TrayEvent::Updated(item) => {
                println!(
                    "PROBE: item id={} title={:?} status={} tooltip={:?} pixmap={:?} menu={:?}",
                    item.id,
                    item.title,
                    item.status.as_str(),
                    item.tooltip,
                    item.pixmap.as_ref().map(|p| (p.width, p.height)),
                    item.menu,
                );
                if let Some(menu_path) = item.menu.clone().filter(|p| p != "/") {
                    let connection = pollster::block_on(zbus::Connection::session());
                    if let Ok(connection) = connection {
                        match pollster::block_on(bus::probe_menu(&connection, &item.bus, &menu_path))
                        {
                            Ok(root) => {
                                println!("PROBE: menu {}{menu_path}", item.bus);
                                print_menu(&root, 1);
                            }
                            Err(err) => println!("PROBE: menu unavailable: {err}"),
                        }
                    }
                }
            }
        }
        if let Some(followup) = tray.apply(event) {
            let (bus, path) = followup.address();
            let (bus, path) = (bus.to_string(), path.to_string());
            let connection = pollster::block_on(zbus::Connection::session());
            if let Ok(connection) = connection
                && let Ok(item) = pollster::block_on(bus::read_item(&connection, &bus, &path))
            {
                println!(
                    "PROBE: item id={} title={:?} status={} tooltip={:?} pixmap={:?} menu={:?}",
                    item.id,
                    item.title,
                    item.status.as_str(),
                    item.tooltip,
                    item.pixmap.as_ref().map(|p| (p.width, p.height)),
                    item.menu,
                );
                if let Some(menu_path) = item.menu.clone().filter(|p| p != "/") {
                    match pollster::block_on(bus::probe_menu(&connection, &item.bus, &menu_path)) {
                        Ok(root) => {
                            println!("PROBE: menu {}{menu_path}", item.bus);
                            print_menu(&root, 1);
                        }
                        Err(err) => println!("PROBE: menu unavailable: {err}"),
                    }
                }
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
            tooltip: String::new(),
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
    fn status_strings_parse_case_insensitively_and_default_to_active() {
        assert_eq!(TrayStatus::parse("Passive"), TrayStatus::Passive);
        assert_eq!(TrayStatus::parse("NeedsAttention"), TrayStatus::NeedsAttention);
        assert_eq!(TrayStatus::parse("Active"), TrayStatus::Active);
        assert_eq!(TrayStatus::parse("nonsense"), TrayStatus::Active);
    }
}
