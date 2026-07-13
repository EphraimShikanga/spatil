pub mod bootstrap;
pub mod keybindings;
pub mod watcher;

pub use bootstrap::bootstrap_config_dir;
pub use keybindings::{Action, KeyCombo};
pub use watcher::spawn_config_watcher;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SpatilConfig {
    pub general: General,
    pub display: Display,
    pub keyboard: Keyboard,
    pub windows: Windows,
    pub autostart: Autostart,
    pub keybindings: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct General {
    /// Base name; resolved to `$XDG_RUNTIME_DIR/<socket_name>.sock`.
    pub socket_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    /// mHz (60000 = 60 Hz), matching `wl_output`.
    pub refresh_rate: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Keyboard {
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    pub xkb_layout: String,
    pub xkb_model: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Windows {
    pub default_width: i32,
    pub default_height: i32,
    /// Map-postponement budget (ms) before a hidden window is revealed anyway.
    pub map_timeout_ms: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Autostart {
    /// Commands run once at startup via `sh -c`, with the working directory set
    /// to the config dir (so `hooks/foo.py` resolves there). Fire-and-forget.
    pub exec: Vec<String>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            socket_name: "spatil-0".to_string(),
        }
    }
}

impl Default for Display {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            refresh_rate: 60_000,
        }
    }
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            repeat_rate: 25,
            repeat_delay: 200,
            xkb_layout: "us".to_string(),
            xkb_model: "pc105".to_string(),
        }
    }
}

impl Default for Windows {
    fn default() -> Self {
        Self {
            default_width: 800,
            default_height: 600,
            map_timeout_ms: 80,
        }
    }
}

impl Default for SpatilConfig {
    fn default() -> Self {
        Self {
            general: General::default(),
            display: Display::default(),
            keyboard: Keyboard::default(),
            windows: Windows::default(),
            autostart: Autostart::default(),
            keybindings: default_keybindings(),
        }
    }
}

fn default_keybindings() -> HashMap<String, String> {
    HashMap::from([
        ("Super+Return".to_string(), "exec alacritty".to_string()),
        ("Super+q".to_string(), "close_window".to_string()),
        ("Super+Shift+q".to_string(), "quit".to_string()),
    ])
}

impl SpatilConfig {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("spatil").join("spatil.toml"))
    }

    pub fn config_dir() -> Option<PathBuf> {
        Self::config_path().and_then(|p| p.parent().map(Path::to_path_buf))
    }

    /// Reads and parses the config file; any missing/unreadable/malformed file
    /// falls back to [`Default`] with a logged warning rather than failing.
    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            tracing::warn!("could not determine a config directory; using built-in defaults");
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<SpatilConfig>(&contents) {
                Ok(config) => {
                    tracing::info!("loaded configuration from {}", path.display());
                    config
                }
                Err(err) => {
                    tracing::warn!("failed to parse {} ({err}); using defaults", path.display());
                    Self::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("no config file at {}; using defaults", path.display());
                Self::default()
            }
            Err(err) => {
                tracing::warn!("failed to read {} ({err}); using defaults", path.display());
                Self::default()
            }
        }
    }

    /// Compiles the raw `keybindings` map into matchable combos, skipping (with a
    /// warning) any entry whose combo or action fails to parse.
    pub fn compile_keybindings(&self) -> HashMap<KeyCombo, Action> {
        let mut out = HashMap::with_capacity(self.keybindings.len());
        for (combo_str, action_str) in &self.keybindings {
            let Some(combo) = KeyCombo::parse(combo_str) else {
                tracing::warn!("ignoring keybinding with unknown key combo: {combo_str:?}");
                continue;
            };
            let Some(action) = Action::parse(action_str) else {
                tracing::warn!("ignoring keybinding {combo_str:?} with unknown action: {action_str:?}");
                continue;
            };
            out.insert(combo, action);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_toml_fills_defaults() {
        let cfg: SpatilConfig = toml::from_str("[display]\nwidth = 2560\n").unwrap();
        assert_eq!(cfg.display.width, 2560);
        assert_eq!(cfg.display.height, 1080);
        assert_eq!(cfg.keyboard.repeat_rate, 25);
        assert_eq!(cfg.general.socket_name, "spatil-0");
    }

    #[test]
    fn compiles_keybindings_skipping_bad_entries() {
        let cfg: SpatilConfig = toml::from_str(
            "[keybindings]\n\"Super+Return\" = \"exec alacritty\"\n\"Bogus+zzz\" = \"quit\"\n",
        )
        .unwrap();
        let compiled = cfg.compile_keybindings();
        assert_eq!(compiled.len(), 1);
        assert!(compiled.values().any(|a| matches!(a, Action::Exec(_))));
    }
}
