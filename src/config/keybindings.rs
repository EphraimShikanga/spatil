use std::str::FromStr;

use smithay::input::keyboard::{Keysym, ModifiersState, xkb};

use crate::renderer::orb::Corner;
use crate::scene::mode::SceneMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl FromStr for Direction {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            _ => Err(()),
        }
    }
}

impl Direction {
    /// X grows right, Y grows up.
    pub fn screen_step(self) -> (f32, f32) {
        match self {
            Self::Left => (-1.0, 0.0),
            Self::Right => (1.0, 0.0),
            Self::Up => (0.0, 1.0),
            Self::Down => (0.0, -1.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adjust {
    Increase,
    Decrease,
}

impl Adjust {
    pub fn sign(self) -> f32 {
        match self {
            Self::Increase => 1.0,
            Self::Decrease => -1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Exec(String),
    Hook(String),
    CloseWindow,
    Quit,
    ToggleScene,
    SetScene(SceneMode),
    ChangeVt(i32),
    Focus(Direction),
    MoveWindow(Direction),
    Pan(Direction),
    Present,
    Dismiss,
    Fullscreen,
    Zoom(Adjust),
    ResetView,
    Orbit(Direction),
    Tilt(Adjust),
    OrbCorner(Corner),
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some((verb, arg)) = s.split_once(char::is_whitespace) {
            let arg = arg.trim();
            if arg.is_empty() {
                return None;
            }
            return match verb {
                "exec" => Some(Action::Exec(arg.to_string())),
                "hook" => Some(Action::Hook(arg.to_string())),
                "change_vt" => arg.parse().ok().map(Action::ChangeVt),
                "set_scene" => arg.parse().ok().map(Action::SetScene),
                "focus" => arg.parse().ok().map(Action::Focus),
                "move_window" => arg.parse().ok().map(Action::MoveWindow),
                "pan" => arg.parse().ok().map(Action::Pan),
                "orbit" => arg.parse().ok().map(Action::Orbit),
                "orb" => Corner::parse(arg).map(Action::OrbCorner),
                "zoom" => match arg {
                    "in" => Some(Action::Zoom(Adjust::Increase)),
                    "out" => Some(Action::Zoom(Adjust::Decrease)),
                    _ => None,
                },
                "tilt" => match arg {
                    "up" => Some(Action::Tilt(Adjust::Increase)),
                    "down" => Some(Action::Tilt(Adjust::Decrease)),
                    _ => None,
                },
                _ => None,
            };
        }
        match s {
            "close_window" | "close" => Some(Action::CloseWindow),
            "quit" | "exit" => Some(Action::Quit),
            "toggle_scene" => Some(Action::ToggleScene),
            "present" => Some(Action::Present),
            "dismiss" => Some(Action::Dismiss),
            "fullscreen" => Some(Action::Fullscreen),
            "reset_view" => Some(Action::ResetView),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Logo,
}

impl Modifier {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "super" | "logo" | "win" | "mod" | "mod4" => Some(Self::Logo),
            "ctrl" | "control" => Some(Self::Ctrl),
            "alt" | "mod1" | "meta" => Some(Self::Alt),
            "shift" => Some(Self::Shift),
            _ => None,
        }
    }

    pub fn held(self, mods: &ModifiersState) -> bool {
        match self {
            Self::Ctrl => mods.ctrl,
            Self::Alt => mods.alt,
            Self::Shift => mods.shift,
            Self::Logo => mods.logo,
        }
    }
}

/// A normalized modifier + keysym chord used as the keybinding lookup key.
/// Uppercase ASCII letters fold to lowercase so a parsed config string and a
/// live keypress (with Shift tracked separately) agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
    pub sym: u32,
}

impl KeyCombo {
    /// Parses `"Super+Shift+Return"`: the last `+`-token is the key, the rest are
    /// modifiers. Case-insensitive. `None` if a name is unrecognized.
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut logo = false;

        let parts: Vec<&str> = s.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
        let (key, mods) = parts.split_last()?;

        for m in mods {
            match Modifier::parse(m) {
                Some(Modifier::Logo) => logo = true,
                Some(Modifier::Ctrl) => ctrl = true,
                Some(Modifier::Alt) => alt = true,
                Some(Modifier::Shift) => shift = true,
                None => {
                    tracing::warn!("unknown modifier {m:?} in keybinding {s:?}");
                    return None;
                }
            }
        }

        let sym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
        if sym.raw() == 0 {
            return None;
        }

        Some(Self { ctrl, alt, shift, logo, sym: sym.raw() }.normalized())
    }

    /// Builds a combo from live keyboard state (`sym` = `handle.modified_sym()`).
    pub fn from_state(mods: &ModifiersState, sym: Keysym) -> Self {
        Self {
            ctrl: mods.ctrl,
            alt: mods.alt,
            shift: mods.shift,
            logo: mods.logo,
            sym: sym.raw(),
        }
        .normalized()
    }

    fn normalized(mut self) -> Self {
        if (0x41..=0x5a).contains(&self.sym) {
            self.sym += 0x20;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::input::keyboard::keysyms;

    #[test]
    fn parses_modifiers_and_named_key() {
        let combo = KeyCombo::parse("Super+Shift+Return").unwrap();
        assert!(combo.logo && combo.shift && !combo.ctrl && !combo.alt);
        assert_eq!(combo.sym, keysyms::KEY_Return);
    }

    #[test]
    fn modifier_aliases_and_case_insensitive() {
        let a = KeyCombo::parse("mod4+ctrl+q").unwrap();
        let b = KeyCombo::parse("SUPER+Control+Q").unwrap();
        assert_eq!(a, b);
        assert!(a.logo && a.ctrl);
    }

    #[test]
    fn uppercase_letters_fold_to_lowercase() {
        let combo = KeyCombo::parse("Super+Q").unwrap();
        assert_eq!(combo.sym, keysyms::KEY_q);
        assert!(!combo.shift);
    }

    #[test]
    fn rejects_unknown_key_and_modifier() {
        assert!(KeyCombo::parse("Super+Nonsense123Key").is_none());
        assert!(KeyCombo::parse("Hyper+q").is_none());
    }

    #[test]
    fn action_parsing() {
        assert_eq!(
            Action::parse("exec alacritty -e vim"),
            Some(Action::Exec("alacritty -e vim".to_string()))
        );
        assert_eq!(Action::parse("close_window"), Some(Action::CloseWindow));
        assert_eq!(Action::parse("quit"), Some(Action::Quit));
        assert_eq!(Action::parse("change_vt 2"), Some(Action::ChangeVt(2)));
        assert_eq!(Action::parse("change_vt x"), None);
        assert_eq!(
            Action::parse("hook place_next.py --cols 4"),
            Some(Action::Hook("place_next.py --cols 4".to_string()))
        );
        assert_eq!(Action::parse("exec   "), None);
        assert_eq!(Action::parse("hook  "), None);
        assert_eq!(Action::parse("frobnicate"), None);
    }

    #[test]
    fn world_interaction_actions_parse() {
        assert_eq!(Action::parse("focus left"), Some(Action::Focus(Direction::Left)));
        assert_eq!(
            Action::parse("move_window down"),
            Some(Action::MoveWindow(Direction::Down))
        );
        assert_eq!(Action::parse("orbit right"), Some(Action::Orbit(Direction::Right)));
        assert_eq!(Action::parse("zoom in"), Some(Action::Zoom(Adjust::Increase)));
        assert_eq!(Action::parse("tilt down"), Some(Action::Tilt(Adjust::Decrease)));
        assert_eq!(Action::parse("present"), Some(Action::Present));
        assert_eq!(Action::parse("dismiss"), Some(Action::Dismiss));
        assert_eq!(Action::parse("fullscreen"), Some(Action::Fullscreen));
        assert_eq!(Action::parse("reset_view"), Some(Action::ResetView));
        assert_eq!(
            Action::parse("set_scene aether"),
            Some(Action::SetScene(SceneMode::Aether))
        );
        // Unknown arguments are rejected rather than silently defaulted.
        assert_eq!(Action::parse("focus sideways"), None);
        assert_eq!(Action::parse("zoom sideways"), None);
        assert_eq!(Action::parse("set_scene nope"), None);
    }

    #[test]
    fn modifier_aliases_resolve() {
        assert_eq!(Modifier::parse("Mod4"), Some(Modifier::Logo));
        assert_eq!(Modifier::parse("control"), Some(Modifier::Ctrl));
        assert_eq!(Modifier::parse("hyper"), None);

        let mut mods = ModifiersState::default();
        mods.logo = true;
        assert!(Modifier::Logo.held(&mods));
        assert!(!Modifier::Alt.held(&mods));
    }
}
