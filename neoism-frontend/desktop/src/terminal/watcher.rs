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

fn config_update_paths_match(
    config_dir: &Path,
    omarchy_current_dir: Option<&Path>,
    paths: &[PathBuf],
) -> bool {
    if paths.is_empty() {
        return true;
    }

    paths.iter().any(|path| {
        path.as_path() == config_dir.join("config.json")
            || path.file_name() == Some(std::ffi::OsStr::new("config.json"))
            || path.starts_with(config_dir.join("ide-themes"))
            || path.starts_with(config_dir.join("packs"))
            || omarchy_current_dir.is_some_and(|dir| {
                path.as_path() == dir.join("theme.name")
                    || path.starts_with(dir.join("theme"))
            })
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
    let config_file_path = config_dir.join("config.json");
    let omarchy_current_dir = neoism_backend::config::mashup::omarchy_current_dir()
        .filter(|path| path.is_dir());

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

        // Runtime themes and packs live below the config directory, so watch
        // recursively and filter unrelated app-state events below.
        if let Err(err_message) = watcher.watch(&config_dir, RecursiveMode::Recursive) {
            tracing::warn!("unable to watch config directory {err_message:?}");
        };
        if let Some(dir) = omarchy_current_dir.as_deref() {
            if let Err(err_message) = watcher.watch(dir, RecursiveMode::Recursive) {
                tracing::warn!(
                    "unable to watch Omarchy theme directory {}: {err_message:?}",
                    dir.display()
                );
            }
        }
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
                        if !config_update_paths_match(
                            &config_dir,
                            omarchy_current_dir.as_deref(),
                            &event.paths,
                        ) {
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
        let config_dir = PathBuf::from("/tmp/neoism");
        let paths = vec![PathBuf::from("/tmp/neoism/terminal-history")];

        assert!(!config_update_paths_match(&config_dir, None, &paths));
    }

    #[test]
    fn config_watcher_accepts_config_json_file() {
        let config_dir = PathBuf::from("/tmp/neoism");
        let paths = vec![config_dir.join("config.json")];

        assert!(config_update_paths_match(&config_dir, None, &paths));
    }

    #[test]
    fn config_watcher_accepts_runtime_and_omarchy_themes() {
        let config_dir = PathBuf::from("/tmp/neoism");
        let omarchy_dir = PathBuf::from("/tmp/omarchy/current");

        assert!(config_update_paths_match(
            &config_dir,
            Some(&omarchy_dir),
            &[config_dir.join("ide-themes/custom.json")],
        ));
        assert!(config_update_paths_match(
            &config_dir,
            Some(&omarchy_dir),
            &[omarchy_dir.join("theme/colors.toml")],
        ));
        assert!(config_update_paths_match(
            &config_dir,
            Some(&omarchy_dir),
            &[omarchy_dir.join("theme")],
        ));
    }
}
