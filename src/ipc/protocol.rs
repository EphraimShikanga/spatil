use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct SpatilEvent {
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u32>,
}

impl SpatilEvent {
    pub fn new(event: impl Into<String>, window_id: u32) -> Self {
        Self {
            event: event.into(),
            window_id: Some(window_id),
        }
    }
}

/// A command from a script. Deliberately loose (name + optional target + float
/// args) so one malformed field is logged and ignored, not fatal to the socket.
#[derive(Debug, Clone, Deserialize)]
pub struct SpatilCommand {
    pub command: String,
    #[serde(default)]
    pub window_id: Option<u32>,
    #[serde(default)]
    pub args: Vec<f32>,
}

/// `$XDG_RUNTIME_DIR/<name>.sock`, falling back to `/tmp` only if the per-user
/// runtime dir is unset.
pub fn socket_path(socket_name: &str) -> PathBuf {
    let file = format!("{socket_name}.sock");
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join(file),
        _ => {
            tracing::warn!("XDG_RUNTIME_DIR unset; using /tmp for the IPC socket (less secure)");
            PathBuf::from("/tmp").join(file)
        }
    }
}
