use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::SpatilConfig;

const EXAMPLE_CONFIG: &str = include_str!("../../assets/spatil.toml");
const EXAMPLE_HOOK: &str = include_str!("../../assets/hooks/auto_place.py");

/// Writes the example config and hook into the config directory on first run,
/// unless `SPATIL_NO_BOOTSTRAP` is set (for packagers who place files
/// themselves) or a `spatil.toml` already exists. Never overwrites user files;
/// failures are logged, not fatal — the compositor runs on built-in defaults
/// regardless.
pub fn bootstrap_config_dir() {
    if std::env::var_os("SPATIL_NO_BOOTSTRAP").is_some() {
        return;
    }
    let (Some(config_path), Some(config_dir)) =
        (SpatilConfig::config_path(), SpatilConfig::config_dir())
    else {
        return;
    };
    if config_path.exists() {
        return;
    }

    let hooks_dir = config_dir.join("hooks");
    if let Err(err) = std::fs::create_dir_all(&hooks_dir) {
        tracing::warn!("bootstrap: could not create {} ({err})", hooks_dir.display());
        return;
    }

    if let Err(err) = std::fs::write(&config_path, EXAMPLE_CONFIG) {
        tracing::warn!("bootstrap: could not write {} ({err})", config_path.display());
        return;
    }
    tracing::info!("wrote starter config to {}", config_path.display());

    let hook_path = hooks_dir.join("auto_place.py");
    if let Err(err) = write_executable(&hook_path, EXAMPLE_HOOK) {
        tracing::warn!("bootstrap: could not write {} ({err})", hook_path.display());
        return;
    }
    tracing::info!("wrote example hook to {}", hook_path.display());
}

fn write_executable(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}
