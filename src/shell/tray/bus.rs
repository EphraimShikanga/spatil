use std::sync::mpsc::Receiver;

use futures_util::stream::StreamExt;
use smithay::reexports::calloop::channel::Sender;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedObjectPath;
use zbus::message::Type as MessageType;
use zbus::{Connection, MatchRule, MessageStream, interface, proxy};

use super::menu::{DBusMenuProxy, MAX_DEPTH, MenuNode, WANTED_PROPERTIES, parse};
use super::{TrayEvent, TrayItem, TrayPixmap, TrayRequest, TrayRole, TrayStatus};

pub const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
pub const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";
const PROTOCOL_VERSION: i32 = 0;
const MAX_ICON_EDGE: i32 = 128;

#[proxy(interface = "org.kde.StatusNotifierItem", assume_defaults = false)]
trait Item {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn title(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn icon_pixmap(&self) -> zbus::Result<Vec<(i32, i32, Vec<u8>)>>;
    #[zbus(property)]
    fn menu(&self) -> zbus::Result<OwnedObjectPath>;
    #[zbus(property)]
    fn icon_theme_path(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn tool_tip(&self) -> zbus::Result<(String, Vec<(i32, i32, Vec<u8>)>, String, String)>;

    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;
    fn secondary_activate(&self, x: i32, y: i32) -> zbus::Result<()>;
    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;
    fn scroll(&self, delta: i32, orientation: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_icon(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn new_title(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn new_status(&self, status: &str) -> zbus::Result<()>;
}

#[proxy(
    interface = "org.kde.StatusNotifierWatcher",
    default_service = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher"
)]
trait Watcher {
    fn register_status_notifier_host(&self, service: &str) -> zbus::Result<()>;
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::Result<Vec<String>>;
    #[zbus(signal)]
    fn status_notifier_item_registered(&self, service: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    fn status_notifier_item_unregistered(&self, service: &str) -> zbus::Result<()>;
}

fn split_service(service: &str, sender: Option<&str>) -> Option<(String, String)> {
    if let Some(path) = service.strip_prefix('/') {
        return sender.map(|bus| (bus.to_string(), format!("/{path}")));
    }
    match service.split_once('/') {
        Some((bus, path)) => Some((bus.to_string(), format!("/{path}"))),
        None => Some((service.to_string(), ITEM_PATH.to_string())),
    }
}

fn argb_be_to_bgra(mut data: Vec<u8>) -> Vec<u8> {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 3);
        px.swap(1, 2);
    }
    data
}

fn best_pixmap(mut candidates: Vec<(i32, i32, Vec<u8>)>) -> Option<TrayPixmap> {
    candidates.retain(|(w, h, data)| {
        *w > 0 && *h > 0 && data.len() as i64 >= (*w as i64) * (*h as i64) * 4
    });
    let pick = candidates
        .iter()
        .enumerate()
        .filter(|(_, (w, h, _))| *w <= MAX_ICON_EDGE && *h <= MAX_ICON_EDGE)
        .max_by_key(|(_, (w, h, _))| *w * *h)
        .map(|(i, _)| i)
        .or_else(|| {
            candidates
                .iter()
                .enumerate()
                .min_by_key(|(_, (w, h, _))| *w * *h)
                .map(|(i, _)| i)
        })?;
    let (width, height, data) = candidates.swap_remove(pick);
    Some(TrayPixmap {
        width: width as u32,
        height: height as u32,
        bgra: argb_be_to_bgra(data),
    })
}

struct WatcherService {
    items: Vec<String>,
    hosts: Vec<String>,
    events: Sender<TrayEvent>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl WatcherService {
    async fn register_status_notifier_item(
        &mut self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let sender = header.sender().map(|s| s.to_string());
        let Some((bus, path)) = split_service(service, sender.as_deref()) else {
            tracing::warn!("ignoring unusable tray registration {service:?}");
            return;
        };
        let key = format!("{bus}{path}");
        if self.items.contains(&key) {
            return;
        }
        self.items.push(key.clone());
        let _ = self.events.send(TrayEvent::Discovered { bus, path });
        let _ = Self::status_notifier_item_registered(&emitter, &key).await;
    }

    async fn register_status_notifier_host(&mut self, service: &str) {
        if !self.hosts.iter().any(|h| h == service) {
            self.hosts.push(service.to_string());
            tracing::info!("tray host registered: {service}");
        }
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        PROTOCOL_VERSION
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

pub fn run(events: Sender<TrayEvent>, requests: Receiver<TrayRequest>) {
    let events_for_requests = events.clone();
    pollster::block_on(async move {
        let connection = match Connection::session().await {
            Ok(connection) => connection,
            Err(err) => {
                tracing::info!("no session bus ({err}); system tray unavailable");
                let _ = events.send(TrayEvent::Unavailable);
                return;
            }
        };

        let role = match claim_watcher(&connection, events.clone()).await {
            Ok(role) => role,
            Err(err) => {
                tracing::warn!("tray watcher setup failed: {err}");
                let _ = events.send(TrayEvent::Unavailable);
                return;
            }
        };
        let _ = events.send(TrayEvent::Role(role));

        if let Err(err) = register_host(&connection).await {
            tracing::warn!("could not register as a tray host: {err}");
        }

        spawn_request_worker(connection.clone(), requests, events_for_requests);

        if let Err(err) = watch_items(&connection, events.clone()).await {
            tracing::warn!("tray item watch ended: {err}");
        }
    });
}

async fn claim_watcher(connection: &Connection, events: Sender<TrayEvent>) -> zbus::Result<TrayRole> {
    let service = WatcherService {
        items: Vec::new(),
        hosts: Vec::new(),
        events,
    };
    connection.object_server().at(WATCHER_PATH, service).await?;

    let reply = connection
        .request_name_with_flags(
            WATCHER_NAME,
            RequestNameFlags::AllowReplacement | RequestNameFlags::DoNotQueue,
        )
        .await;

    match reply {
        Ok(RequestNameReply::PrimaryOwner) => {
            tracing::info!("owning {WATCHER_NAME}: Spatil is this session's tray registry");
            Ok(TrayRole::Owner)
        }
        Ok(other) => {
            tracing::info!(
                "{WATCHER_NAME} is already owned ({other:?}); consuming it as a guest host"
            );
            let _ = connection.object_server().remove::<WatcherService, _>(WATCHER_PATH).await;
            Ok(TrayRole::Guest)
        }
        Err(err) => {
            tracing::info!("could not take {WATCHER_NAME} ({err}); consuming it as a guest host");
            let _ = connection.object_server().remove::<WatcherService, _>(WATCHER_PATH).await;
            Ok(TrayRole::Guest)
        }
    }
}

async fn register_host(connection: &Connection) -> zbus::Result<()> {
    let unique = connection
        .unique_name()
        .map(|n| n.to_string())
        .unwrap_or_default();
    let host = format!("org.kde.StatusNotifierHost-spatil-{}", std::process::id());
    let _ = DBusProxy::new(connection)
        .await?
        .request_name(
            host.as_str().try_into().map_err(zbus::Error::from)?,
            RequestNameFlags::DoNotQueue.into(),
        )
        .await;

    let watcher = WatcherProxy::new(connection).await?;
    watcher
        .register_status_notifier_host(if unique.is_empty() { &host } else { &unique })
        .await
}

async fn watch_items(connection: &Connection, events: Sender<TrayEvent>) -> zbus::Result<()> {
    let watcher = WatcherProxy::new(connection).await?;

    if let Ok(existing) = watcher.registered_status_notifier_items().await {
        for service in existing {
            if let Some((bus, path)) = split_service(&service, None) {
                let _ = events.send(TrayEvent::Discovered { bus, path });
            }
        }
    }

    let item_signals = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface("org.kde.StatusNotifierItem")?
        .build();
    let mut changes = MessageStream::for_match_rule(item_signals, connection, None).await?;

    let mut registered = watcher.receive_status_notifier_item_registered().await?;
    let mut unregistered = watcher.receive_status_notifier_item_unregistered().await?;
    let mut owner_changed = DBusProxy::new(connection)
        .await?
        .receive_name_owner_changed()
        .await?;

    loop {
        futures_util::select! {
            signal = registered.next() => {
                let Some(signal) = signal else { break };
                if let Ok(args) = signal.args()
                    && let Some((bus, path)) = split_service(args.service, None)
                {
                    let _ = events.send(TrayEvent::Discovered { bus, path });
                }
            }
            signal = unregistered.next() => {
                let Some(signal) = signal else { break };
                if let Ok(args) = signal.args() {
                    let _ = events.send(TrayEvent::Removed(args.service.to_string()));
                }
            }
            message = changes.next() => {
                let Some(Ok(message)) = message else { break };
                let sender = message.header().sender().map(|s| s.to_string());
                if let Some(sender) = sender {
                    let _ = events.send(TrayEvent::Changed(sender));
                }
            }
            signal = owner_changed.next() => {
                let Some(signal) = signal else { break };
                let Ok(args) = signal.args() else { continue };
                if args.new_owner.is_some() {
                    continue;
                }
                let gone = args.name.to_string();
                forget_owner(connection, &gone).await;
                let _ = events.send(TrayEvent::OwnerLost(gone));
            }
            complete => break,
        }
    }
    Ok(())
}

async fn forget_owner(connection: &Connection, gone: &str) {
    let Ok(iface) = connection
        .object_server()
        .interface::<_, WatcherService>(WATCHER_PATH)
        .await
    else {
        return;
    };
    let dropped: Vec<String> = {
        let mut service = iface.get_mut().await;
        let before = service.items.len();
        let dropped = service
            .items
            .iter()
            .filter(|key| key.starts_with(gone))
            .cloned()
            .collect();
        service.items.retain(|key| !key.starts_with(gone));
        if service.items.len() == before {
            return;
        }
        dropped
    };
    for key in dropped {
        let _ = WatcherService::status_notifier_item_unregistered(iface.signal_emitter(), &key)
            .await;
    }
}

pub async fn read_item(
    connection: &Connection,
    bus: &str,
    path: &str,
) -> zbus::Result<TrayItem> {
    let proxy = ItemProxy::builder(connection)
        .destination(bus.to_string())?
        .path(path.to_string())?
        .build()
        .await?;

    let status = proxy
        .status()
        .await
        .ok()
        .map(|s| TrayStatus::parse(&s))
        .unwrap_or(TrayStatus::Active);

    let icon_name = proxy.icon_name().await.unwrap_or_default();
    let pixmap = match proxy.icon_pixmap().await.ok().and_then(best_pixmap) {
        Some(pixmap) => Some(pixmap),
        None => {
            let theme_path = proxy.icon_theme_path().await.ok().filter(|p| !p.is_empty());
            super::icon::resolve(&icon_name, theme_path.as_deref()).and_then(|found| {
                let mut decoded = super::icon::decode(&found)?;
                if super::icon::is_symbolic(&found) {
                    super::icon::tint(&mut decoded, crate::shell::SYMBOLIC_TINT);
                }
                Some(decoded)
            })
        }
    };
    if pixmap.is_none() && !icon_name.is_empty() {
        tracing::info!("no drawable icon for {icon_name:?}; the item is tracked but not shown");
    }

    let owner = match DBusProxy::new(connection).await {
        Ok(dbus) => match bus.try_into() {
            Ok(name) => dbus.get_name_owner(name).await.ok().map(|n| n.to_string()),
            Err(_) => None,
        },
        Err(_) => None,
    };

    Ok(TrayItem {
        key: format!("{bus}{path}"),
        bus: bus.to_string(),
        owner,
        path: path.to_string(),
        id: proxy.id().await.unwrap_or_default(),
        title: proxy.title().await.unwrap_or_default(),
        status,
        icon_name,
        pixmap,
        menu: proxy.menu().await.ok().map(|p| p.to_string()),
        tooltip: tooltip_text(proxy.tool_tip().await.ok()),
    })
}

/// SNI ToolTip is `(icon_name, icon_pixmap, title, description)`; the two text
/// fields are joined since either may be the only one an application sets.
fn tooltip_text(raw: Option<(String, Vec<(i32, i32, Vec<u8>)>, String, String)>) -> String {
    let Some((_, _, title, description)) = raw else {
        return String::new();
    };
    match (title.trim(), description.trim()) {
        ("", "") => String::new(),
        ("", body) => body.to_string(),
        (head, "") => head.to_string(),
        (head, body) if head == body => head.to_string(),
        (head, body) => format!("{head} — {body}"),
    }
}

async fn menu_proxy<'a>(
    connection: &Connection,
    bus: &str,
    path: &str,
) -> zbus::Result<DBusMenuProxy<'a>> {
    DBusMenuProxy::builder(connection)
        .destination(bus.to_string())?
        .path(path.to_string())?
        .build()
        .await
}

pub async fn probe_menu(connection: &Connection, bus: &str, path: &str) -> zbus::Result<MenuNode> {
    fetch_menu(connection, bus, path).await
}

async fn fetch_menu(connection: &Connection, bus: &str, path: &str) -> zbus::Result<MenuNode> {
    let proxy = menu_proxy(connection, bus, path).await?;
    // Advisory: applications populate lazily off it, but many never implement it.
    let _ = proxy.about_to_show(0).await;
    let (_, raw) = proxy
        .get_layout(0, MAX_DEPTH as i32, WANTED_PROPERTIES)
        .await?;
    Ok(parse(&raw))
}

async fn send_menu_event(
    connection: &Connection,
    bus: &str,
    path: &str,
    id: i32,
) -> zbus::Result<()> {
    let proxy = menu_proxy(connection, bus, path).await?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    proxy
        .event(id, "clicked", &zbus::zvariant::Value::from(0i32), timestamp)
        .await
}

fn spawn_request_worker(
    connection: Connection,
    requests: Receiver<TrayRequest>,
    events: Sender<TrayEvent>,
) {
    std::thread::Builder::new()
        .name("spatil-tray-rpc".into())
        .spawn(move || {
            while let Ok(request) = requests.recv() {
                pollster::block_on(dispatch_request(&connection, &request, &events));
            }
        })
        .map(|_| ())
        .unwrap_or_else(|err| tracing::warn!("could not start the tray request worker: {err}"));
}

async fn dispatch_request(
    connection: &Connection,
    request: &TrayRequest,
    events: &Sender<TrayEvent>,
) {
    let (bus, path) = request.address();

    match request {
        TrayRequest::FetchMenu { key, .. } => {
            match fetch_menu(connection, bus, path).await {
                Ok(root) => {
                    let _ = events.send(TrayEvent::Menu {
                        key: key.clone(),
                        root: Box::new(root),
                    });
                }
                Err(err) => {
                    tracing::debug!("tray item {bus}{path} has no usable menu: {err}");
                    let _ = events.send(TrayEvent::Menu {
                        key: key.clone(),
                        root: Box::new(MenuNode::default()),
                    });
                }
            }
            return;
        }
        TrayRequest::MenuEvent { id, .. } => {
            if let Err(err) = send_menu_event(connection, bus, path, *id).await {
                tracing::warn!("tray menu {bus}{path} rejected item {id}: {err}");
            }
            return;
        }
        _ => {}
    }

    if let TrayRequest::Refresh { .. } = request {
        match read_item(connection, bus, path).await {
            Ok(item) => {
                let _ = events.send(TrayEvent::Updated(Box::new(item)));
            }
            Err(err) => {
                tracing::debug!("dropping unreadable tray item {bus}{path}: {err}");
                let _ = events.send(TrayEvent::Removed(format!("{bus}{path}")));
            }
        }
        return;
    }

    let proxy = match ItemProxy::builder(connection)
        .destination(bus.to_string())
        .and_then(|b| b.path(path.to_string()))
    {
        Ok(builder) => match builder.build().await {
            Ok(proxy) => proxy,
            Err(err) => {
                tracing::warn!("could not reach tray item {bus}{path}: {err}");
                return;
            }
        },
        Err(err) => {
            tracing::warn!("bad tray address {bus}{path}: {err}");
            return;
        }
    };

    let result = match request {
        TrayRequest::Refresh { .. } => Ok(()),
        TrayRequest::Activate { x, y, .. } => proxy.activate(*x, *y).await,
        TrayRequest::SecondaryActivate { x, y, .. } => proxy.secondary_activate(*x, *y).await,
        TrayRequest::ContextMenu { x, y, .. } => proxy.context_menu(*x, *y).await,
        TrayRequest::Scroll { delta, .. } => proxy.scroll(*delta, "vertical").await,
        TrayRequest::FetchMenu { .. } | TrayRequest::MenuEvent { .. } => Ok(()),
    };
    if let Err(err) = result {
        tracing::debug!("tray item {bus}{path} rejected the call: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_path_registration_falls_back_to_the_sender() {
        assert_eq!(
            split_service("/StatusNotifierItem", Some(":1.42")),
            Some((":1.42".to_string(), "/StatusNotifierItem".to_string())),
        );
        assert_eq!(split_service("/StatusNotifierItem", None), None);
    }

    #[test]
    fn a_bus_name_registration_uses_the_conventional_path() {
        assert_eq!(
            split_service("org.kde.StatusNotifierItem-1-1", None),
            Some((
                "org.kde.StatusNotifierItem-1-1".to_string(),
                "/StatusNotifierItem".to_string(),
            )),
        );
    }

    #[test]
    fn a_combined_registration_splits_at_the_path() {
        assert_eq!(
            split_service("org.example.Tray/Custom/Path", None),
            Some((
                "org.example.Tray".to_string(),
                "/Custom/Path".to_string(),
            )),
        );
    }

    #[test]
    fn pixmaps_are_byte_swapped_into_texture_order() {
        assert_eq!(argb_be_to_bgra(vec![0xFF, 0xFF, 0x00, 0x00]), vec![
            0x00, 0x00, 0xFF, 0xFF
        ]);
    }

    #[test]
    fn the_largest_sensible_pixmap_wins() {
        let px = |w: i32, h: i32| (w, h, vec![0u8; (w * h * 4) as usize]);
        let best = best_pixmap(vec![px(16, 16), px(64, 64), px(22, 22)]).expect("a pixmap");
        assert_eq!((best.width, best.height), (64, 64));
    }

    #[test]
    fn a_tooltip_joins_its_title_and_description() {
        let raw = |t: &str, d: &str| {
            Some((String::new(), Vec::new(), t.to_string(), d.to_string()))
        };
        assert_eq!(tooltip_text(raw("Network", "Connected to Wi-Fi")), "Network — Connected to Wi-Fi");
        assert_eq!(tooltip_text(raw("Network", "")), "Network");
        assert_eq!(tooltip_text(raw("", "Connected")), "Connected");
        assert_eq!(tooltip_text(raw("Same", "Same")), "Same");
        assert_eq!(tooltip_text(raw("  ", "  ")), "");
    }

    #[test]
    fn an_item_with_no_tooltip_property_yields_an_empty_string() {
        assert_eq!(tooltip_text(None), "");
    }

    #[test]
    fn an_oversized_pixmap_is_still_better_than_none() {
        let px = |w: i32, h: i32| (w, h, vec![0u8; (w * h * 4) as usize]);
        let best = best_pixmap(vec![px(512, 512)]).expect("a pixmap");
        assert_eq!((best.width, best.height), (512, 512));
    }

    #[test]
    fn a_truncated_pixmap_is_rejected_rather_than_read_past_its_end() {
        assert!(best_pixmap(vec![(64, 64, vec![0u8; 16])]).is_none());
        assert!(best_pixmap(vec![(0, 0, Vec::new())]).is_none());
    }
}
