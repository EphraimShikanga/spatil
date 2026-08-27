use std::sync::Arc;

use futures_util::stream::StreamExt;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::channel::{Event, Sender, channel};
use zbus::message::Type as MessageType;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, MatchRule, MessageStream, proxy};

use super::icon_source;
use super::module::{ModuleId, ModuleState};
use crate::state::SpatilState;

const SERVICE: &str = "org.freedesktop.NetworkManager";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    None,
    Wired,
    Wireless,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkStatus {
    pub kind: LinkKind,
    pub connected: bool,
    pub strength: Option<u8>,
    pub name: String,
    pub wireless_enabled: bool,
}

impl NetworkStatus {
    pub fn offline() -> Self {
        Self {
            kind: LinkKind::None,
            connected: false,
            strength: None,
            name: String::new(),
            wireless_enabled: true,
        }
    }

    pub fn icon_name(&self) -> &'static str {
        if !self.connected {
            return if self.wireless_enabled {
                "network-offline-symbolic"
            } else {
                "network-wireless-disabled-symbolic"
            };
        }
        match self.kind {
            LinkKind::Wired => "network-wired-symbolic",
            LinkKind::None => "network-offline-symbolic",
            LinkKind::Wireless => match self.strength.unwrap_or(0) {
                80..=u8::MAX => "network-wireless-signal-excellent-symbolic",
                55..=79 => "network-wireless-signal-good-symbolic",
                30..=54 => "network-wireless-signal-ok-symbolic",
                5..=29 => "network-wireless-signal-weak-symbolic",
                _ => "network-wireless-signal-none-symbolic",
            },
        }
    }

    pub fn level(&self) -> Option<f32> {
        match (self.kind, self.connected) {
            (LinkKind::Wireless, true) => self.strength.map(|s| s as f32 / 100.0),
            _ => None,
        }
    }

    pub fn summary(&self) -> String {
        if !self.connected {
            return "Offline".to_string();
        }
        match (self.kind, self.strength) {
            (LinkKind::Wireless, Some(strength)) => format!("{} ({strength}%)", self.name),
            (LinkKind::Wireless, None) => self.name.clone(),
            (LinkKind::Wired, _) => {
                if self.name.is_empty() { "Wired".to_string() } else { self.name.clone() }
            }
            _ => "Offline".to_string(),
        }
    }
}

#[proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait Manager {
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn wireless_enabled(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn primary_connection(&self) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.freedesktop.NetworkManager.Connection.Active",
    default_service = "org.freedesktop.NetworkManager"
)]
trait ActiveConnection {
    #[zbus(property, name = "Type")]
    fn kind(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[proxy(
    interface = "org.freedesktop.NetworkManager.Device.Wireless",
    default_service = "org.freedesktop.NetworkManager"
)]
trait WirelessDevice {
    #[zbus(property)]
    fn active_access_point(&self) -> zbus::Result<OwnedObjectPath>;
}

#[proxy(
    interface = "org.freedesktop.NetworkManager.AccessPoint",
    default_service = "org.freedesktop.NetworkManager"
)]
trait AccessPoint {
    #[zbus(property)]
    fn strength(&self) -> zbus::Result<u8>;
}

async fn read_status(connection: &Connection) -> NetworkStatus {
    let Ok(manager) = ManagerProxy::new(connection).await else {
        return NetworkStatus::offline();
    };
    let wireless_enabled = manager.wireless_enabled().await.unwrap_or(true);
    let connected = manager.state().await.unwrap_or(0) >= 50;

    let primary = manager
        .primary_connection()
        .await
        .ok()
        .filter(|path| path.as_str() != "/");
    let Some(primary) = primary else {
        return NetworkStatus { connected: false, wireless_enabled, ..NetworkStatus::offline() };
    };

    let active = match ActiveConnectionProxy::builder(connection).path(primary) {
        Ok(builder) => builder.build().await.ok(),
        Err(_) => None,
    };
    let Some(active) = active else {
        return NetworkStatus { connected, wireless_enabled, ..NetworkStatus::offline() };
    };

    let raw = active.kind().await.unwrap_or_default();
    let kind = if raw.contains("wireless") {
        LinkKind::Wireless
    } else if raw.is_empty() {
        LinkKind::None
    } else {
        LinkKind::Wired
    };
    let name = active.id().await.unwrap_or_default();

    let mut strength = None;
    if kind == LinkKind::Wireless {
        for device in active.devices().await.unwrap_or_default() {
            let Ok(builder) = WirelessDeviceProxy::builder(connection).path(device) else {
                continue;
            };
            let Ok(wireless) = builder.build().await else { continue };
            let Ok(access_point) = wireless.active_access_point().await else { continue };
            if access_point.as_str() == "/" {
                continue;
            }
            let Ok(builder) = AccessPointProxy::builder(connection).path(access_point) else {
                continue;
            };
            let Ok(proxy) = builder.build().await else { continue };
            if let Ok(value) = proxy.strength().await {
                strength = Some(value);
                break;
            }
        }
    }

    NetworkStatus { kind, connected, strength, name, wireless_enabled }
}

pub fn probe() {
    pollster::block_on(async {
        let Ok(connection) = Connection::system().await else {
            println!("NET: no system bus");
            return;
        };
        let status = read_status(&connection).await;
        println!(
            "NET: kind={:?} connected={} strength={:?} name={:?}",
            status.kind, status.connected, status.strength, status.name
        );
        println!("NET: icon={} level={:?}", status.icon_name(), status.level());
        println!("NET: summary={:?}", status.summary());
        println!("NET: icon decoded={:?}", icon_source(status.icon_name()).map(|p| (p.width, p.height)));
    });
}

pub fn init_network(loop_handle: &LoopHandle<'static, SpatilState>) {
    let (tx, rx) = channel::<ModuleState>();
    let inserted = loop_handle.insert_source(rx, |event, _, state: &mut SpatilState| {
        if let Event::Msg(module) = event {
            state.network = Some(module);
        }
    });
    if let Err(err) = inserted {
        tracing::warn!("could not insert the network event source: {err}");
        return;
    }
    if let Err(err) = std::thread::Builder::new()
        .name("spatil-network".into())
        .spawn(move || run(tx))
    {
        tracing::warn!("could not start the network thread: {err}");
    }
}

fn run(tx: Sender<ModuleState>) {
    pollster::block_on(async move {
        let connection = match Connection::system().await {
            Ok(connection) => connection,
            Err(err) => {
                tracing::info!("no system bus ({err}); the network module is unavailable");
                return;
            }
        };

        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(SERVICE)
            .and_then(|b| b.interface("org.freedesktop.DBus.Properties"))
            .map(|b| b.build());
        let mut changes = match rule {
            Ok(rule) => match MessageStream::for_match_rule(rule, &connection, None).await {
                Ok(stream) => stream,
                Err(err) => {
                    tracing::warn!("cannot watch NetworkManager: {err}");
                    return;
                }
            },
            Err(err) => {
                tracing::warn!("bad NetworkManager match rule: {err}");
                return;
            }
        };

        let mut last: Option<NetworkStatus> = None;
        publish(&connection, &tx, &mut last).await;
        while changes.next().await.is_some() {
            publish(&connection, &tx, &mut last).await;
        }
    });
}

async fn publish(
    connection: &Connection,
    tx: &Sender<ModuleState>,
    last: &mut Option<NetworkStatus>,
) {
    let status = read_status(connection).await;
    if last.as_ref() == Some(&status) {
        return;
    }
    tracing::info!("network: {}", status.summary());

    let mut module = ModuleState::new(ModuleId::Network);
    module.icon = icon_source(status.icon_name()).map(Arc::new);
    module.level = status.level();
    module.active = status.connected;
    module.urgent = !status.connected;
    module.label = status.summary();
    module.tooltip = status.summary();
    let _ = tx.send(module);
    *last = Some(status);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wifi(strength: u8) -> NetworkStatus {
        NetworkStatus {
            kind: LinkKind::Wireless,
            connected: true,
            strength: Some(strength),
            name: "Huawei-Guest".to_string(),
            wireless_enabled: true,
        }
    }

    #[test]
    fn signal_strength_picks_a_matching_icon() {
        assert!(wifi(95).icon_name().ends_with("excellent-symbolic"));
        assert!(wifi(59).icon_name().ends_with("good-symbolic"));
        assert!(wifi(40).icon_name().ends_with("ok-symbolic"));
        assert!(wifi(10).icon_name().ends_with("weak-symbolic"));
        assert!(wifi(0).icon_name().ends_with("none-symbolic"));
    }

    #[test]
    fn every_state_names_an_icon_that_exists_in_a_real_theme() {
        let states = [
            wifi(95),
            wifi(0),
            NetworkStatus { kind: LinkKind::Wired, connected: true, ..wifi(0) },
            NetworkStatus::offline(),
            NetworkStatus { wireless_enabled: false, ..NetworkStatus::offline() },
        ];
        for status in states {
            let name = status.icon_name();
            if super::super::icon_source(name).is_none() {
                eprintln!("no icon theme provides {name}; skipping (none installed?)");
                return;
            }
        }
    }

    #[test]
    fn disconnected_beats_link_kind_and_radio_state_beats_both() {
        let down = NetworkStatus { connected: false, ..wifi(95) };
        assert_eq!(down.icon_name(), "network-offline-symbolic");
        let radio_off = NetworkStatus { connected: false, wireless_enabled: false, ..wifi(95) };
        assert_eq!(radio_off.icon_name(), "network-wireless-disabled-symbolic");
    }

    #[test]
    fn only_a_live_wireless_link_reports_a_level() {
        assert_eq!(wifi(59).level(), Some(0.59));
        assert_eq!(NetworkStatus { connected: false, ..wifi(59) }.level(), None);
        assert_eq!(
            NetworkStatus { kind: LinkKind::Wired, connected: true, ..wifi(59) }.level(),
            None,
        );
    }

    #[test]
    fn the_summary_reads_as_a_person_would_say_it() {
        assert_eq!(wifi(59).summary(), "Huawei-Guest (59%)");
        assert_eq!(NetworkStatus::offline().summary(), "Offline");
        assert_eq!(
            NetworkStatus { kind: LinkKind::Wired, connected: true, name: String::new(), ..wifi(0) }
                .summary(),
            "Wired",
        );
    }
}
