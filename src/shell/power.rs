use std::sync::Arc;

use futures_util::stream::StreamExt;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::channel::{Event, Sender, channel};
use zbus::message::Type as MessageType;
use zbus::{Connection, MatchRule, MessageStream, proxy};

use super::icon_source;
use super::module::{ModuleId, ModuleState};
use crate::state::SpatilState;

const SERVICE: &str = "org.freedesktop.UPower";
const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const LOW: f64 = 15.0;

#[derive(Debug, Clone, PartialEq)]
pub struct BatteryStatus {
    pub present: bool,
    pub percentage: f64,
    pub charging: bool,
    pub full: bool,
    pub upower_icon: String,
}

impl BatteryStatus {
    pub fn absent() -> Self {
        Self {
            present: false,
            percentage: 0.0,
            charging: false,
            full: false,
            upower_icon: String::new(),
        }
    }

    pub fn icon_name(&self) -> String {
        if !self.upower_icon.is_empty() && self.upower_icon != "battery-missing-symbolic" {
            return self.upower_icon.clone();
        }
        let step = ((self.percentage / 10.0).round() as i64 * 10).clamp(0, 100);
        match (self.charging, step) {
            (true, 100) => "battery-full-charged-symbolic".to_string(),
            (true, _) => format!("battery-level-{step}-charging-symbolic"),
            (false, 100) => "battery-full-symbolic".to_string(),
            (false, _) => format!("battery-level-{step}-symbolic"),
        }
    }

    pub fn level(&self) -> Option<f32> {
        self.present.then(|| (self.percentage / 100.0).clamp(0.0, 1.0) as f32)
    }

    pub fn urgent(&self) -> bool {
        self.present && !self.charging && !self.full && self.percentage <= LOW
    }

    pub fn summary(&self) -> String {
        if !self.present {
            return "No battery".to_string();
        }
        let percent = self.percentage.round() as i64;
        if self.full {
            format!("Battery {percent}% (full)")
        } else if self.charging {
            format!("Battery {percent}% (charging)")
        } else {
            format!("Battery {percent}%")
        }
    }
}

#[proxy(
    interface = "org.freedesktop.UPower.Device",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower/devices/DisplayDevice"
)]
trait Device {
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;
}

async fn read_status(connection: &Connection) -> BatteryStatus {
    let Ok(device) = DeviceProxy::new(connection).await else {
        return BatteryStatus::absent();
    };
    let present = device.is_present().await.unwrap_or(false);
    if !present {
        return BatteryStatus::absent();
    }
    let state = device.state().await.unwrap_or(0);
    BatteryStatus {
        present,
        percentage: device.percentage().await.unwrap_or(0.0),
        charging: matches!(state, 1 | 5),
        full: state == 4,
        upower_icon: device.icon_name().await.unwrap_or_default(),
    }
}

pub fn init_power(loop_handle: &LoopHandle<'static, SpatilState>) {
    let (tx, rx) = channel::<ModuleState>();
    let inserted = loop_handle.insert_source(rx, |event, _, state: &mut SpatilState| {
        if let Event::Msg(module) = event {
            state.battery = Some(module);
        }
    });
    if let Err(err) = inserted {
        tracing::warn!("could not insert the battery event source: {err}");
        return;
    }
    if let Err(err) = std::thread::Builder::new()
        .name("spatil-power".into())
        .spawn(move || run(tx))
    {
        tracing::warn!("could not start the battery thread: {err}");
    }
}

fn run(tx: Sender<ModuleState>) {
    pollster::block_on(async move {
        let Ok(connection) = Connection::system().await else {
            tracing::info!("no system bus; the battery module is unavailable");
            return;
        };

        let mut last: Option<BatteryStatus> = None;
        publish(&connection, &tx, &mut last).await;

        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(SERVICE)
            .and_then(|b| b.path(DISPLAY_DEVICE))
            .map(|b| b.build());
        let Ok(rule) = rule else { return };
        let Ok(mut changes) = MessageStream::for_match_rule(rule, &connection, None).await else {
            tracing::warn!("cannot watch UPower; battery will not update");
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
    last: &mut Option<BatteryStatus>,
) {
    let status = read_status(connection).await;
    if last.as_ref() == Some(&status) {
        return;
    }
    if !status.present {
        *last = Some(status);
        return;
    }
    tracing::info!("power: {}", status.summary());

    let mut module = ModuleState::new(ModuleId::Battery);
    let name = status.icon_name();
    module.icon = icon_source(&name)
        .or_else(|| icon_source("battery-full-symbolic"))
        .map(Arc::new);
    module.level = status.level();
    module.active = !status.urgent();
    module.urgent = status.urgent();
    module.label = status.summary();
    module.tooltip = status.summary();
    let _ = tx.send(module);
    *last = Some(status);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn battery(percentage: f64, charging: bool) -> BatteryStatus {
        BatteryStatus {
            present: true,
            percentage,
            charging,
            full: false,
            upower_icon: String::new(),
        }
    }

    #[test]
    fn upower_names_the_icon_when_it_offers_one() {
        let mut status = battery(63.0, false);
        status.upower_icon = "battery-level-60-symbolic".to_string();
        assert_eq!(status.icon_name(), "battery-level-60-symbolic");
    }

    #[test]
    fn without_upowers_name_the_level_is_rounded_to_a_stock_icon() {
        assert_eq!(battery(63.0, false).icon_name(), "battery-level-60-symbolic");
        assert_eq!(battery(97.0, false).icon_name(), "battery-full-symbolic");
        assert_eq!(battery(63.0, true).icon_name(), "battery-level-60-charging-symbolic");
        assert_eq!(battery(0.0, false).icon_name(), "battery-level-0-symbolic");
    }

    #[test]
    fn only_a_draining_battery_is_urgent() {
        assert!(battery(9.0, false).urgent());
        assert!(!battery(9.0, true).urgent(), "charging is not an emergency");
        assert!(!battery(80.0, false).urgent());
        assert!(!BatteryStatus::absent().urgent());
    }

    #[test]
    fn a_desktop_without_a_battery_reports_no_level() {
        assert_eq!(BatteryStatus::absent().level(), None);
        assert_eq!(BatteryStatus::absent().summary(), "No battery");
        assert_eq!(battery(50.0, false).level(), Some(0.5));
    }

    #[test]
    fn the_summary_distinguishes_charging_from_full() {
        assert_eq!(battery(63.0, false).summary(), "Battery 63%");
        assert_eq!(battery(63.0, true).summary(), "Battery 63% (charging)");
        let full = BatteryStatus { full: true, ..battery(100.0, true) };
        assert_eq!(full.summary(), "Battery 100% (full)");
    }
}
