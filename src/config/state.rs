use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionState {
    pub shell: ShellState,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ShellState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub familiar_corner: Option<String>,
}

impl SessionState {
    pub fn path() -> Option<PathBuf> {
        dirs::state_dir().map(|dir| dir.join("spatil").join("state.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                tracing::warn!("could not read {}: {err}", path.display());
                return Self::default();
            }
        };
        match toml::from_str(&raw) {
            Ok(state) => state,
            Err(err) => {
                tracing::warn!("ignoring malformed {}: {err}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            tracing::warn!("no state directory available; session state not saved");
            return;
        };
        let body = match toml::to_string(self) {
            Ok(body) => body,
            Err(err) => {
                tracing::warn!("could not serialize session state: {err}");
                return;
            }
        };
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            tracing::warn!("could not create {}: {err}", parent.display());
            return;
        }
        let header = "# Written by Spatil. Safe to delete: removing it restores the\n\
                      # corresponding values from spatil.toml.\n";
        if let Err(err) = std::fs::write(&path, format!("{header}{body}")) {
            tracing::warn!("could not write {}: {err}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_state_round_trips_and_omits_unset_fields() {
        let state = SessionState::default();
        let body = toml::to_string(&state).expect("serialize");
        assert!(
            !body.contains("familiar_corner"),
            "an unset override must not be written: {body:?}"
        );
        let back: SessionState = toml::from_str(&body).expect("parse");
        assert!(back.shell.familiar_corner.is_none());
    }

    #[test]
    fn a_set_corner_round_trips() {
        let mut state = SessionState::default();
        state.shell.familiar_corner = Some("top_left".to_string());
        let body = toml::to_string(&state).expect("serialize");
        let back: SessionState = toml::from_str(&body).expect("parse");
        assert_eq!(back.shell.familiar_corner.as_deref(), Some("top_left"));
    }

    #[test]
    fn an_unknown_key_is_tolerated_rather_than_fatal() {
        let back: SessionState =
            toml::from_str("[shell]\nfamiliar_corner = \"top_right\"\n").expect("parse");
        assert_eq!(back.shell.familiar_corner.as_deref(), Some("top_right"));
        assert!(toml::from_str::<SessionState>("").is_ok(), "an empty file is valid");
    }
}
