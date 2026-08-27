use std::sync::Arc;

use super::tray::TrayPixmap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleId {
    Network,
    Bluetooth,
    Battery,
    Audio,
    Tray(String),
}

impl ModuleId {
    pub fn key(&self) -> String {
        match self {
            Self::Network => "network".to_string(),
            Self::Bluetooth => "bluetooth".to_string(),
            Self::Battery => "battery".to_string(),
            Self::Audio => "audio".to_string(),
            Self::Tray(key) => format!("tray:{key}"),
        }
    }

    pub fn is_tray(&self) -> bool {
        matches!(self, Self::Tray(_))
    }

    pub fn rank(&self) -> u8 {
        match self {
            Self::Network => 0,
            Self::Bluetooth => 1,
            Self::Audio => 2,
            Self::Battery => 3,
            Self::Tray(_) => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleAction {
    Activate,
    Secondary,
    Context,
    ScrollUp,
    ScrollDown,
}

#[derive(Debug, Clone)]
pub struct ModuleState {
    pub id: ModuleId,
    pub icon: Option<Arc<TrayPixmap>>,
    pub level: Option<f32>,
    pub active: bool,
    pub urgent: bool,
    pub label: String,
    pub tooltip: String,
}

impl ModuleState {
    pub fn new(id: ModuleId) -> Self {
        Self {
            id,
            icon: None,
            level: None,
            active: true,
            urgent: false,
            label: String::new(),
            tooltip: String::new(),
        }
    }

    pub fn drawable(&self) -> bool {
        self.icon.is_some()
    }
}

pub fn order(modules: &mut [ModuleState]) {
    modules.sort_by(|a, b| {
        a.id.rank()
            .cmp(&b.id.rank())
            .then_with(|| a.id.key().cmp(&b.id.key()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: ModuleId) -> ModuleState {
        ModuleState::new(id)
    }

    #[test]
    fn keys_are_distinct_across_kinds() {
        let keys = [
            ModuleId::Network.key(),
            ModuleId::Bluetooth.key(),
            ModuleId::Battery.key(),
            ModuleId::Audio.key(),
            ModuleId::Tray("network".to_string()).key(),
        ];
        let unique = keys.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), keys.len(), "a tray item must not collide with a module: {keys:?}");
    }

    #[test]
    fn native_modules_lead_and_tray_items_follow() {
        let mut modules = vec![
            m(ModuleId::Tray("zzz".to_string())),
            m(ModuleId::Battery),
            m(ModuleId::Tray("aaa".to_string())),
            m(ModuleId::Network),
        ];
        order(&mut modules);
        let ids: Vec<_> = modules.iter().map(|m| m.id.clone()).collect();
        assert_eq!(
            ids,
            vec![
                ModuleId::Network,
                ModuleId::Battery,
                ModuleId::Tray("aaa".to_string()),
                ModuleId::Tray("zzz".to_string()),
            ],
        );
    }

    #[test]
    fn ordering_is_stable_as_tray_items_come_and_go() {
        let mut with = vec![
            m(ModuleId::Tray("b".to_string())),
            m(ModuleId::Network),
            m(ModuleId::Tray("a".to_string())),
        ];
        order(&mut with);
        let before: Vec<_> = with.iter().map(|m| m.id.key()).collect();

        with.retain(|m| m.id != ModuleId::Tray("a".to_string()));
        order(&mut with);
        let after: Vec<_> = with.iter().map(|m| m.id.key()).collect();

        assert_eq!(before[0], "network");
        assert_eq!(after[0], "network", "a departing item must not move the others");
        assert_eq!(after[1], "tray:b");
    }

    #[test]
    fn a_module_without_an_icon_is_not_drawable() {
        let mut module = m(ModuleId::Network);
        assert!(!module.drawable());
        module.icon = Some(Arc::new(TrayPixmap { width: 1, height: 1, bgra: vec![0; 4] }));
        assert!(module.drawable());
    }
}
