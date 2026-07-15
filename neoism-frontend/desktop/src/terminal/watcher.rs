use crate::event::{EventListener, RioEvent};
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::time::Duration;

const POLLING_TIMEOUT: Duration = Duration::from_secs(2);

fn config_watcher_event_kind(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Any
            | EventKind::Create(_)
            | EventKind::Modify(_)
            | EventKind::Remove(_)
            | EventKind::Other
    )
}

fn config_update_paths_match(config_file_path: &Path, paths: &[PathBuf]) -> bool {
    if paths.is_empty() {
        return true;
    }

    paths.iter().any(|path| {
        path == config_file_path
            || path.file_name() == Some(std::ffi::OsStr::new("config.json"))
            || path.file_name() == Some(std::ffi::OsStr::new("config.toml"))
    })
}

pub fn configuration_file_updates<
    P: AsRef<Path> + std::marker::Send + 'static,
    T: EventListener + std::marker::Send + 'static,
>(
    path: P,
    event_proxy: T,
) -> notify::Result<()> {
    let config_dir = path.as_ref().to_path_buf();
    // JSON is the primary config; the filename filter below also accepts
    // a legacy config.toml so either format hot-reloads on save.
    let config_file_path = config_dir.join("config.json");

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();

        // Keep notify/inotify setup off the launch path. The first config
        // change can wait for this thread; the first window should not.
        let mut watcher = match RecommendedWatcher::new(
            tx,
            Config::default().with_poll_interval(POLLING_TIMEOUT),
        ) {
            Ok(watcher) => watcher,
            Err(err_message) => {
                tracing::warn!("unable to create config watcher: {err_message:?}");
                return;
            }
        };

        // Watch the config directory for config.toml creates/replaces, but
        // filter events below so app state files in the same directory do not
        // trigger full config reloads.
        if let Err(err_message) = watcher.watch(&config_dir, RecursiveMode::NonRecursive)
        {
            tracing::warn!("unable to watch config directory {err_message:?}");
        };
        tracing::info!(
            target: "neoism::config_watcher",
            config_dir = %config_dir.display(),
            config_file = %config_file_path.display(),
            "watching config directory"
        );

        for res in rx {
            match res {
                Ok(event) => {
                    if config_watcher_event_kind(&event.kind) {
                        if !config_update_paths_match(&config_file_path, &event.paths) {
                            tracing::debug!(
                                target: "neoism::config_watcher",
                                kind = ?event.kind,
                                paths = ?event.paths,
                                "ignored non-config file change in config directory"
                            );
                            continue;
                        }

                        tracing::info!(
                            target: "neoism::config_watcher",
                            kind = ?event.kind,
                            paths = ?event.paths,
                            "config file changed; scheduling config reload"
                        );
                        event_proxy.send_event(
                            RioEvent::PrepareUpdateConfig,
                            neoism_backend::event::WindowId::from(0),
                        );
                    }
                }
                Err(err_message) => {
                    tracing::error!("unable to watch config directory: {err_message:?}")
                }
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::config_update_paths_match;
    use std::path::PathBuf;

    #[test]
    fn config_watcher_ignores_terminal_history_file() {
        let config_file = PathBuf::from("/tmp/neoism/config.toml");
        let paths = vec![PathBuf::from("/tmp/neoism/terminal-history")];

        assert!(!config_update_paths_match(&config_file, &paths));
    }

    #[test]
    fn config_watcher_accepts_config_toml_file() {
        let config_file = PathBuf::from("/tmp/neoism/config.toml");
        let paths = vec![config_file.clone()];

        assert!(config_update_paths_match(&config_file, &paths));
    }
}
