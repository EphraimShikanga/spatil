pub mod bootstrap;
pub mod keybindings;
mod loader;
pub mod watcher;

pub use bootstrap::bootstrap_config_dir;
pub use keybindings::{Action, Direction, KeyCombo, Modifier};
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
    pub output: Output,
    pub keyboard: Keyboard,
    pub windows: Windows,
    pub scene_graph: SceneGraph,
    pub animation: Animation,
    pub input: Input,
    pub autostart: Autostart,
    pub keybindings: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Input {
    pub world_modifier: String,
    pub orbit_speed: f32,
    pub pan_speed: f32,
    pub zoom_speed: f32,
    pub drag_depth_speed: f32,
    pub move_step: f32,
    pub invert_scroll_zoom: bool,
    pub focus_border_enabled: bool,
    pub focus_border_color: String,
    /// World units, not pixels.
    pub focus_border_width: f32,
    pub backdrop_dim: f32,
    pub backdrop_dim_color: String,
    pub present_fill: f32,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            world_modifier: "Super".to_string(),
            orbit_speed: 0.005,
            pan_speed: 0.002,
            zoom_speed: 0.1,
            drag_depth_speed: 0.05,
            move_step: 0.25,
            invert_scroll_zoom: false,
            focus_border_enabled: true,
            focus_border_color: "#4A90E2".to_string(),
            focus_border_width: 0.004,
            backdrop_dim: 0.55,
            backdrop_dim_color: "#000000".to_string(),
            present_fill: 0.8,
        }
    }
}

impl Input {
    pub fn modifier(&self) -> Modifier {
        Modifier::parse(&self.world_modifier).unwrap_or_else(|| {
            tracing::warn!(
                "unknown world_modifier {:?}; falling back to Super",
                self.world_modifier
            );
            Modifier::Logo
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Output {
    pub connector: String,
    pub mode: String,
    pub scale: i32,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            connector: "auto".to_string(),
            mode: "auto".to_string(),
            scale: 1,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Animation {
    pub enabled: bool,
    pub switch_ms: f32,
    pub open_ms: f32,
    pub close_ms: f32,
    pub present_ms: f32,
    pub damping: f32,
    pub spring_stiffness: f32,
    pub spring_damping: f32,
}

impl Default for Animation {
    fn default() -> Self {
        Self {
            enabled: true,
            switch_ms: 500.0,
            open_ms: 250.0,
            close_ms: 220.0,
            present_ms: 280.0,
            damping: 0.02,
            spring_stiffness: 220.0,
            spring_damping: 22.0,
        }
    }
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
    pub camera_distance: f32,
    pub camera_elevation: f32,
    pub camera_orbit: f32,
    pub window_pitch: f32,
    pub window_base_y: f32,
    pub window_base_z: f32,
    pub light_position: [f32; 3],
    pub grid_color: String,
    pub void_color: String,
    pub axis_color: String,
    pub grid_level: f32,
    pub grid_extent: f32,
    pub grid_spacing: f32,
    pub fog_density: f32,
    pub shadow_map_size: u32,
}

impl Default for Strata {
    fn default() -> Self {
        Self {
            enable_shadows: true,
            camera_distance: 3.0,
            camera_elevation: 0.0,
            camera_orbit: 0.0,
            window_pitch: 45.0,
            window_base_y: -0.2,
            window_base_z: -0.5,
            light_position: [1.0, 2.0, 1.0],
            grid_color: "#2A2A30".to_string(),
            void_color: "#0A0A0C".to_string(),
            axis_color: "#2A2A30".to_string(),
            grid_level: -0.6,
            grid_extent: 5.0,
            grid_spacing: 0.2,
            fog_density: 0.15,
            shadow_map_size: 2048,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Aether {
    pub void_color: String,
    pub particle_color: String,
    pub particle_count: u32,
    pub particle_size: f32,
    pub particle_opacity: f32,
    pub field_width: f32,
    pub field_height: f32,
    pub field_depth: f32,
    pub fog_density: f32,
    pub camera_distance: f32,
    pub parallax_enabled: bool,
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
            particle_color: "#D1D1D6".to_string(),
            particle_count: 2000,
            particle_size: 0.015,
            particle_opacity: 0.8,
            field_width: 5.0,
            field_height: 3.0,
            field_depth: 8.0,
            fog_density: 0.15,
            camera_distance: 2.5,
            parallax_enabled: true,
            parallax_strength: 0.25,
            glass_alpha: 0.95,
            glass_tint: "#12141A".to_string(),
            glass_tint_strength: 0.1,
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
            output: Output::default(),
            keyboard: Keyboard::default(),
            windows: Windows::default(),
            scene_graph: SceneGraph::default(),
            animation: Animation::default(),
            input: Input::default(),
            autostart: Autostart::default(),
            keybindings: default_keybindings(),
        }
    }
}

fn default_keybindings() -> HashMap<String, String> {
    HashMap::from([
        ("Super+Return".to_string(), "exec kitty".to_string()),
        ("Super+q".to_string(), "close_window".to_string()),
        ("Super+Shift+q".to_string(), "quit".to_string()),
        ("Super+Tab".to_string(), "toggle_scene".to_string()),
        ("Super+Left".to_string(), "focus left".to_string()),
        ("Super+Right".to_string(), "focus right".to_string()),
        ("Super+Up".to_string(), "focus up".to_string()),
        ("Super+Down".to_string(), "focus down".to_string()),
        ("Super+Shift+Left".to_string(), "move_window left".to_string()),
        ("Super+Shift+Right".to_string(), "move_window right".to_string()),
        ("Super+Shift+Up".to_string(), "move_window up".to_string()),
        ("Super+Shift+Down".to_string(), "move_window down".to_string()),
        ("Super+space".to_string(), "present".to_string()),
        ("Super+f".to_string(), "fullscreen".to_string()),
        ("Super+Shift+h".to_string(), "pan left".to_string()),
        ("Super+Shift+l".to_string(), "pan right".to_string()),
        ("Super+Shift+k".to_string(), "pan up".to_string()),
        ("Super+Shift+j".to_string(), "pan down".to_string()),
        ("Super+bracketleft".to_string(), "orbit left".to_string()),
        ("Super+bracketright".to_string(), "orbit right".to_string()),
        ("Super+Prior".to_string(), "tilt up".to_string()),
        ("Super+Next".to_string(), "tilt down".to_string()),
        ("Super+equal".to_string(), "zoom in".to_string()),
        ("Super+minus".to_string(), "zoom out".to_string()),
        ("Super+0".to_string(), "reset_view".to_string()),
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
        assert_eq!(cfg.scene_graph.strata.camera_distance, 3.0);
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

    /// The shipped example is both our documentation and what first-run writes, so
    /// a key that drifts from its default silently lies to every new user.
    #[test]
    fn the_shipped_example_config_matches_the_built_in_defaults() {
        let example: SpatilConfig =
            toml::from_str(include_str!("../assets/spatil.toml")).expect("assets/spatil.toml");
        let defaults = SpatilConfig::default();

        assert_eq!(example.input.world_modifier, defaults.input.world_modifier);
        assert_eq!(example.input.orbit_speed, defaults.input.orbit_speed);
        assert_eq!(example.input.pan_speed, defaults.input.pan_speed);
        assert_eq!(example.input.zoom_speed, defaults.input.zoom_speed);
        assert_eq!(example.input.drag_depth_speed, defaults.input.drag_depth_speed);
        assert_eq!(example.input.move_step, defaults.input.move_step);
        assert_eq!(example.input.focus_border_color, defaults.input.focus_border_color);
        assert_eq!(example.input.focus_border_width, defaults.input.focus_border_width);
        assert_eq!(example.input.backdrop_dim, defaults.input.backdrop_dim);
        assert_eq!(example.input.present_fill, defaults.input.present_fill);
        assert_eq!(example.animation.present_ms, defaults.animation.present_ms);
        assert_eq!(example.animation.damping, defaults.animation.damping);
        assert_eq!(example.animation.spring_stiffness, defaults.animation.spring_stiffness);
        assert_eq!(example.animation.spring_damping, defaults.animation.spring_damping);

        // A subset, not equality: the example may demo extra bindings. What must not
        // happen is a default going undocumented, or documented differently.
        for (combo, action) in &defaults.keybindings {
            assert_eq!(
                example.keybindings.get(combo),
                Some(action),
                "default binding {combo:?} = {action:?} is missing or wrong in assets/spatil.toml"
            );
        }
    }

    #[test]
    fn every_documented_action_and_modifier_actually_parses() {
        let example: SpatilConfig =
            toml::from_str(include_str!("../assets/spatil.toml")).expect("assets/spatil.toml");
        let compiled = example.compile_keybindings();
        assert_eq!(
            compiled.len(),
            example.keybindings.len(),
            "a shipped keybinding failed to compile"
        );
        assert_eq!(example.input.modifier(), Modifier::Logo);
    }
}
