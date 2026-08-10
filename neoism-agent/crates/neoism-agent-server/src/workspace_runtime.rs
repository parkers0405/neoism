use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::plugin::PluginRegistry;

const IDLE_TTL: Duration = Duration::from_secs(60 * 60);
const CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct WorkspaceRuntime {
    pub(crate) root: PathBuf,
    pub(crate) plugins: PluginRegistry,
}

#[derive(Default)]
pub(crate) struct WorkspaceRuntimeRegistry {
    entries: Mutex<HashMap<PathBuf, RuntimeEntry>>,
}

struct RuntimeEntry {
    runtime: Arc<WorkspaceRuntime>,
    last_used: Instant,
    last_config_refresh: Instant,
}

impl WorkspaceRuntimeRegistry {
    pub(crate) async fn acquire(
        &self,
        directory: &str,
        base_plugins: &PluginRegistry,
    ) -> Arc<WorkspaceRuntime> {
        let root = canonical_location(directory);
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        entries.retain(|_, entry| {
            Arc::strong_count(&entry.runtime) > 1
                || now.duration_since(entry.last_used) < IDLE_TTL
        });
        if let Some(entry) = entries.get_mut(&root) {
            entry.last_used = now;
            let refresh =
                now.duration_since(entry.last_config_refresh) >= CONFIG_REFRESH_INTERVAL;
            if refresh {
                entry.last_config_refresh = now;
            }
            let runtime = entry.runtime.clone();
            drop(entries);
            if refresh {
                refresh_plugins(&runtime);
            }
            return runtime;
        }

        let plugins = base_plugins.fork();
        if let Ok(loaded) = crate::config::load(&root.to_string_lossy()) {
            plugins.register_configured_plugins(&loaded.info, &root.to_string_lossy());
        }
        let runtime = Arc::new(WorkspaceRuntime {
            root: root.clone(),
            plugins,
        });
        entries.insert(
            root,
            RuntimeEntry {
                runtime: runtime.clone(),
                last_used: now,
                last_config_refresh: now,
            },
        );
        runtime
    }
}

fn refresh_plugins(runtime: &WorkspaceRuntime) {
    if let Ok(loaded) = crate::config::load(&runtime.root.to_string_lossy()) {
        runtime
            .plugins
            .register_configured_plugins(&loaded.info, &runtime.root.to_string_lossy());
    }
}

fn canonical_location(directory: &str) -> PathBuf {
    let path = Path::new(directory);
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn canonical_paths_share_one_workspace_and_other_roots_do_not() {
        let root = std::env::temp_dir().join(format!(
            "neoism-workspace-runtime-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let registry = WorkspaceRuntimeRegistry::default();
        let plugins = PluginRegistry::default();
        let first = registry.acquire(&root.to_string_lossy(), &plugins).await;
        let alias = registry
            .acquire(&root.join(".").to_string_lossy(), &plugins)
            .await;
        let second = registry.acquire(&other.to_string_lossy(), &plugins).await;
        assert!(Arc::ptr_eq(&first, &alias));
        assert!(!Arc::ptr_eq(&first, &second));
        let _ = std::fs::remove_dir_all(root);
    }
}
