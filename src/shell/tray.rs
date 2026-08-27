mod bus;

use std::sync::mpsc::{Sender as StdSender, channel};

use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::channel::{Event, channel as calloop_channel};

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
}

pub enum TrayEvent {
    Unavailable,
    Role(TrayRole),
    Discovered { bus: String, path: String },
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
            TrayEvent::OwnerLost(name) => {
                self.items.retain(|item| item.bus != name);
                None
            }
            TrayEvent::Updated(item) => {
                match self.items.iter_mut().find(|existing| existing.key == item.key) {
                    Some(existing) => *existing = *item,
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
