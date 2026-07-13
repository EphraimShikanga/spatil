use super::SpatilConfig;

/// Watches the config directory and pushes a freshly parsed [`SpatilConfig`]
/// through `tx` on every change touching `spatil.toml`. Watches the directory
/// (not the file) so it survives editor save-via-rename. The returned watcher
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
    let config_path = SpatilConfig::config_path()?;

    let mut watcher =
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                if !matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) {
                    return;
                }
                if event.paths.iter().any(|p| p.file_name() == config_path.file_name())
                    && tx.send(SpatilConfig::load()).is_err()
                {
                    tracing::debug!("config reload channel closed; watcher idle");
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

    if let Err(err) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
        tracing::warn!("failed to watch {} ({err}); hot-reload disabled", dir.display());
        return None;
    }
    tracing::info!("watching {} for live config changes", dir.display());
    Some(watcher)
}
