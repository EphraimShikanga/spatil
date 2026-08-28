use std::collections::HashMap;
use std::sync::Arc;

use futures_util::stream::StreamExt;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::channel::{Event, Sender, channel};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, MatchRule, MessageStream, proxy};

use super::icon_source;
use super::module::{ModuleId, ModuleState};
use crate::state::SpatilState;

const SERVICE: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BluetoothStatus {
    pub available: bool,
    pub powered: bool,
    pub connected: Vec<String>,
}

impl BluetoothStatus {
    pub fn unavailable() -> Self {
        Self { available: false, powered: false, connected: Vec::new() }
    }

    pub fn icon_name(&self) -> &'static str {
        if !self.powered {
            "bluetooth-disabled-symbolic"
        } else if self.connected.is_empty() {
            "bluetooth-symbolic"
        } else {
            "bluetooth-active-symbolic"
        }
    }

    pub fn summary(&self) -> String {
        if !self.available {
            return "No bluetooth adapter".to_string();
        }
        if !self.powered {
            return "Bluetooth off".to_string();
        }
        match self.connected.len() {
            0 => "Bluetooth on".to_string(),
            1 => format!("Bluetooth: {}", self.connected[0]),
            n => format!("Bluetooth: {n} devices"),
        }
    }
}

#[proxy(
    interface = "org.freedesktop.DBus.ObjectManager",
    default_service = "org.bluez",
    default_path = "/"
)]
trait Objects {
    fn get_managed_objects(
        &self,
    ) -> zbus::Result<HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>>;
}

fn flag(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
    props.get(key).and_then(|v| bool::try_from(v.clone()).ok()).unwrap_or(false)
}

fn text(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props.get(key).and_then(|v| String::try_from(v.clone()).ok())
}

async fn read_status(connection: &Connection) -> BluetoothStatus {
    let Ok(objects) = ObjectsProxy::new(connection).await else {
        return BluetoothStatus::unavailable();
    };
    let Ok(managed) = objects.get_managed_objects().await else {
        return BluetoothStatus::unavailable();
    };

    let mut status = BluetoothStatus::unavailable();
    for interfaces in managed.values() {
        if let Some(adapter) = interfaces.get(ADAPTER) {
            status.available = true;
            status.powered |= flag(adapter, "Powered");
        }
        if let Some(device) = interfaces.get(DEVICE)
            && flag(device, "Connected")
        {
            let name = text(device, "Alias")
                .or_else(|| text(device, "Name"))
                .unwrap_or_else(|| "device".to_string());
            status.connected.push(name);
        }
    }
    status.connected.sort();
    status
}

pub fn init_bluetooth(loop_handle: &LoopHandle<'static, SpatilState>) {
    let (tx, rx) = channel::<ModuleState>();
    let inserted = loop_handle.insert_source(rx, |event, _, state: &mut SpatilState| {
        if let Event::Msg(module) = event {
            state.bluetooth = Some(module);
        }
    });
    if let Err(err) = inserted {
        tracing::warn!("could not insert the bluetooth event source: {err}");
        return;
    }
    if let Err(err) = std::thread::Builder::new()
        .name("spatil-bluetooth".into())
        .spawn(move || run(tx))
    {
        tracing::warn!("could not start the bluetooth thread: {err}");
    }
}

fn run(tx: Sender<ModuleState>) {
    pollster::block_on(async move {
        let Ok(connection) = Connection::system().await else {
            tracing::info!("no system bus; the bluetooth module is unavailable");
            return;
        };

        let mut last: Option<BluetoothStatus> = None;
        publish(&connection, &tx, &mut last).await;

        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(SERVICE)
            .map(|b| b.build());
        let Ok(rule) = rule else { return };
        let Ok(mut changes) = MessageStream::for_match_rule(rule, &connection, None).await else {
            tracing::warn!("cannot watch BlueZ; bluetooth will not update");
            return;
        };
        while changes.next().await.is_some() {
            publish(&connection, &tx, &mut last).await;
        }
    });
}

async fn publish(
    connection: &Connection,
    tx: &Sender<ModuleState>,
    last: &mut Option<BluetoothStatus>,
) {
    let status = read_status(connection).await;
    if last.as_ref() == Some(&status) {
        return;
    }
    if !status.available {
        *last = Some(status);
        return;
    }
    tracing::info!("bluetooth: {}", status.summary());

    let mut module = ModuleState::new(ModuleId::Bluetooth);
    module.icon = icon_source(status.icon_name()).map(Arc::new);
    module.active = status.powered;
    module.label = status.summary();
    module.tooltip = status.summary();
    let _ = tx.send(module);
    *last = Some(status);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn powered(connected: &[&str]) -> BluetoothStatus {
        BluetoothStatus {
            available: true,
            powered: true,
            connected: connected.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn the_icon_distinguishes_off_idle_and_in_use() {
        assert_eq!(
            BluetoothStatus { powered: false, ..powered(&[]) }.icon_name(),
            "bluetooth-disabled-symbolic",
        );
        assert_eq!(powered(&[]).icon_name(), "bluetooth-symbolic");
        assert_eq!(powered(&["Headphones"]).icon_name(), "bluetooth-active-symbolic");
    }

    #[test]
    fn the_summary_names_a_single_device_and_counts_several() {
        assert_eq!(powered(&["WH-1000XM4"]).summary(), "Bluetooth: WH-1000XM4");
        assert_eq!(powered(&["A", "B"]).summary(), "Bluetooth: 2 devices");
        assert_eq!(powered(&[]).summary(), "Bluetooth on");
        assert_eq!(
            BluetoothStatus { powered: false, ..powered(&[]) }.summary(),
            "Bluetooth off",
        );
    }

    #[test]
    fn a_machine_with_no_adapter_is_distinct_from_one_switched_off() {
        let none = BluetoothStatus::unavailable();
        assert!(!none.available);
        assert_eq!(none.summary(), "No bluetooth adapter");
    }

    #[test]
    fn every_state_names_an_icon_that_exists_in_a_real_theme() {
        for status in [
            powered(&["x"]),
            powered(&[]),
            BluetoothStatus { powered: false, ..powered(&[]) },
        ] {
            let name = status.icon_name();
            if super::super::icon_source(name).is_none() {
                eprintln!("no icon theme provides {name}; skipping");
                return;
            }
        }
    }
}
