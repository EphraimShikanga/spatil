use smithay::input::keyboard::{Keysym, ModifiersState, xkb};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Exec(String),
    Hook(String),
    CloseWindow,
    Quit,
    ToggleScene,
    ChangeVt(i32),
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(cmd) = s.strip_prefix("exec ") {
            let cmd = cmd.trim();
            return (!cmd.is_empty()).then(|| Action::Exec(cmd.to_string()));
        }
        if let Some(spec) = s.strip_prefix("hook ") {
            let spec = spec.trim();
            return (!spec.is_empty()).then(|| Action::Hook(spec.to_string()));
        }
        if let Some(vt) = s.strip_prefix("change_vt ") {
            return vt.trim().parse().ok().map(Action::ChangeVt);
        }
        match s {
            "close_window" | "close" => Some(Action::CloseWindow),
            "quit" | "exit" => Some(Action::Quit),
            "toggle_scene" => Some(Action::ToggleScene),
            _ => None,
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
            match m.to_ascii_lowercase().as_str() {
                "super" | "logo" | "win" | "mod" | "mod4" => logo = true,
                "ctrl" | "control" => ctrl = true,
                "alt" | "mod1" | "meta" => alt = true,
                "shift" => shift = true,
                other => {
                    tracing::warn!("unknown modifier {other:?} in keybinding {s:?}");
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
}
