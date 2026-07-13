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

use crate::scene::mode::SceneMode;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SpatilConfig {
    pub general: General,
    pub display: Display,
    pub keyboard: Keyboard,
    pub windows: Windows,
    pub scene_graph: SceneGraph,
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
pub struct SceneGraph {
    pub mode: String,
    pub strata: Strata,
    pub aether: Aether,
}

impl SceneGraph {
    pub fn scene_mode(&self) -> SceneMode {
        if self.mode.is_empty() {
            SceneMode::default()
        } else {
            SceneMode::from_config(&self.mode)
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Strata {
    pub enable_shadows: bool,
    pub camera_pitch: f32,
    pub window_pitch: f32,
    pub window_hover_height: f32,
    pub light_position: [f32; 3],
    pub grid_color: String,
    pub void_color: String,
    pub axis_color: String,
    pub grid_spacing: f32,
    pub grid_depth: f32,
    pub grid_width: f32,
    pub shadow_map_size: u32,
}

impl Default for Strata {
    fn default() -> Self {
        Self {
            enable_shadows: true,
            camera_pitch: -15.0,
            window_pitch: 30.0,
            window_hover_height: 0.5,
            light_position: [-5.0, 10.0, 5.0],
            grid_color: "#1E2D2F".to_string(),
            void_color: "#0A0A0C".to_string(),
            axis_color: "#4A6E72".to_string(),
            grid_spacing: 0.05,
            grid_depth: 1.5,
            grid_width: 1.25,
            shadow_map_size: 2048,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Aether {
    pub void_color: String,
    pub dot_color: String,
    pub dot_spacing: f32,
    pub dot_radius: f32,
    pub parallax_strength: f32,
    pub glass_alpha: f32,
    pub glass_tint: String,
    pub glass_tint_strength: f32,
    pub z_layer_gap: f32,
}

impl Default for Aether {
    fn default() -> Self {
        Self {
            void_color: "#0F0F11".to_string(),
            dot_color: "#D1D1D6".to_string(),
            dot_spacing: 0.08,
            dot_radius: 0.06,
            parallax_strength: 0.5,
            glass_alpha: 0.85,
            glass_tint: "#12141A".to_string(),
            glass_tint_strength: 0.25,
            z_layer_gap: 0.35,
        }
    }
}

/// Parses `#RRGGBB` / `#RRGGBBAA` into linear-space RGBA; invalid input yields
/// opaque black. The bytes are treated as sRGB and decoded for the sRGB swapchain.
pub fn parse_hex_color(hex: &str) -> [f32; 4] {
    fn srgb_to_linear(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    let h = hex.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    let (r, g, b, a) = match h.len() {
        6 => (byte(0), byte(2), byte(4), Some(1.0)),
        8 => (byte(0), byte(2), byte(4), byte(6)),
        _ => (None, None, None, None),
    };
    match (r, g, b, a) {
        (Some(r), Some(g), Some(b), Some(a)) => {
            [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
        }
        _ => {
            tracing::warn!("invalid hex color {hex:?}; falling back to black");
            [0.0, 0.0, 0.0, 1.0]
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Autostart {
    /// Commands run once at startup via `sh -c`
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
            scene_graph: SceneGraph::default(),
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
    fn parse_hex_color_decodes_and_linearizes() {
        // Pure black/white survive sRGB→linear exactly.
        assert_eq!(parse_hex_color("#000000"), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(parse_hex_color("#ffffff"), [1.0, 1.0, 1.0, 1.0]);
        // 8-digit form carries a straight (non-linearized) alpha.
        assert_eq!(parse_hex_color("#00000080")[3], 128.0 / 255.0);
        // A mid-gray must decode darker in linear space than its sRGB fraction.
        let g = parse_hex_color("#808080")[0];
        assert!(g > 0.0 && g < 0.5, "expected linearized gray, got {g}");
        // Garbage degrades to opaque black, never panics.
        assert_eq!(parse_hex_color("nope"), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn scene_mode_parses_case_insensitively_and_degrades() {
        assert_eq!("strata".parse(), Ok(SceneMode::Strata));
        assert_eq!("AETHER".parse(), Ok(SceneMode::Aether));
        assert_eq!(SceneMode::from_config("aether"), SceneMode::Aether);
        // Unknown / empty degrade to the default (Strata).
        assert_eq!(SceneMode::from_config("nope"), SceneMode::Strata);
        assert_eq!(SceneGraph::default().scene_mode(), SceneMode::Strata);
    }

    #[test]
    fn nested_scene_graph_selects_mode_and_fills_defaults() {
        let cfg: SpatilConfig = toml::from_str(
            "[scene_graph]\nmode = \"aether\"\n\
             [scene_graph.aether]\nz_layer_gap = 1.0\n",
        )
        .unwrap();
        assert_eq!(cfg.scene_graph.scene_mode(), SceneMode::Aether);
        assert_eq!(cfg.scene_graph.aether.z_layer_gap, 1.0);
        // Untouched sub-table keeps its defaults.
        assert_eq!(cfg.scene_graph.strata.camera_pitch, -15.0);
        assert_eq!(cfg.scene_graph.aether.void_color, "#0F0F11");
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
