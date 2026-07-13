pub mod bootstrap;
pub mod keybindings;
mod loader;
pub mod watcher;

pub use bootstrap::bootstrap_config_dir;
pub use keybindings::{Action, KeyCombo};
pub use watcher::spawn_config_watcher;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml::Value;

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

    /// Loads the config for startup: like [`Self::try_load`] but never fails,
    /// falling back to [`Default`] (with a warning) on any error.
    pub fn load() -> Self {
        Self::try_load().unwrap_or_else(|err| {
            tracing::warn!("{err}; using built-in defaults");
            Self::default()
        })
    }

    /// Reads, resolves `include`s, and parses the config. `Ok(Default)` when no
    /// file exists yet (first run); `Err` on a read/parse/merge failure — the
    /// hot-reload path uses this to keep the last-good config instead of
    /// reverting to defaults on a mid-edit typo.
    pub fn try_load() -> Result<Self, String> {
        let Some(path) = Self::config_path() else {
            return Ok(Self::default());
        };
        match std::fs::metadata(&path) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!("no config file at {}; using defaults", path.display());
                return Ok(Self::default());
            }
            Err(err) => return Err(format!("cannot access {} ({err})", path.display())),
        }

        let table = loader::load_with_includes(&path)?;
        let config: SpatilConfig = Value::Table(table)
            .try_into()
            .map_err(|err| format!("invalid config: {err}"))?;
        tracing::info!("loaded configuration from {}", path.display());
        Ok(config)
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
    fn includes_override_the_including_file() {
        let dir = std::env::temp_dir().join(format!("spatil-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("base.toml");
        std::fs::write(
            &base,
            "include = [\"over.toml\"]\n\
             [display]\nwidth = 1000\nheight = 1000\n\
             [keybindings]\n\"Super+q\" = \"close_window\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("over.toml"),
            "[display]\nheight = 2000\n\
             [keybindings]\n\"Super+q\" = \"quit\"\n\"Super+e\" = \"exec foo\"\n",
        )
        .unwrap();

        let table = loader::load_with_includes(&base).unwrap();
        let cfg: SpatilConfig = Value::Table(table).try_into().unwrap();

        assert_eq!(cfg.display.width, 1000);
        assert_eq!(cfg.display.height, 2000);
        assert_eq!(cfg.keybindings["Super+q"], "quit");
        assert_eq!(cfg.keybindings["Super+e"], "exec foo");

        std::fs::remove_dir_all(&dir).ok();
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
