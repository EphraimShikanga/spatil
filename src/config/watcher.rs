use super::SpatilConfig;

/// Watches the config directory (recursively, so included `.toml` files are
/// covered) and pushes a freshly parsed [`SpatilConfig`] through `tx` on any
/// change to a `.toml` file. Watching the directory (not a file) survives editor
/// save-via-rename. A mid-edit parse error keeps the current config rather than
/// reverting to defaults (via [`SpatilConfig::try_load`]). The returned watcher
/// must be kept alive; `None` disables hot-reload without stopping the compositor.
pub fn spawn_config_watcher(
    tx: smithay::reexports::calloop::channel::Sender<SpatilConfig>,
) -> Option<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};

    let dir = SpatilConfig::config_dir()?;
    if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!("could not create {} ({err}); hot-reload disabled", dir.display());
        return None;
    }

    let mut watcher =
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                if !matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) {
                    return;
                }
                if !event.paths.iter().any(|p| p.extension().is_some_and(|e| e == "toml")) {
                    return;
                }
                match SpatilConfig::try_load() {
                    Ok(config) => {
                        if tx.send(config).is_err() {
                            tracing::debug!("config reload channel closed; watcher idle");
                        }
                    }
                    Err(err) => {
                        tracing::warn!("config reload failed ({err}); keeping current config");
                    }
                }
            }
            Err(err) => tracing::warn!("config watcher error: {err}"),
        }) {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!("failed to create config watcher: {err}; hot-reload disabled");
                return None;
            }
        };

    if let Err(err) = watcher.watch(&dir, RecursiveMode::Recursive) {
        tracing::warn!("failed to watch {} ({err}); hot-reload disabled", dir.display());
        return None;
    }
    tracing::info!("watching {} for live config changes", dir.display());
    Some(watcher)
}
