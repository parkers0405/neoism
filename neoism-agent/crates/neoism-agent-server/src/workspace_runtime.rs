use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const IDLE_TTL: Duration = Duration::from_secs(60 * 60);
const CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct WorkspaceRuntime {
    pub(crate) root: PathBuf,
    services: neoism_agent_service_api::AgentServices,
    lifecycle: RwLock<Arc<WorkspaceLifecycle>>,
    snapshot: RwLock<Arc<neoism_agent_plugin_api::RegistrySnapshot>>,
    signature: RwLock<Vec<u8>>,
}

/// Optional plugin state for one canonical workspace and plugin generation.
///
/// Keeping these cells on the workspace runtime is important: application
/// kernel construction must not start plugin workers or allocate process maps,
/// and dropping/replacing a generation gives teardown one unambiguous owner.
#[derive(Default)]
pub(crate) struct WorkspaceLifecycle {
    mcp: OnceLock<Arc<crate::mcp::McpRuntimeManager>>,
    lsp: OnceLock<crate::lsp::LspRuntime>,
    pty: OnceLock<Arc<crate::pty::PtyWorkspaceRuntime>>,
    background: OnceLock<Arc<crate::background_job::BackgroundWorkspaceRuntime>>,
    subagents: OnceLock<Arc<crate::plugins::subagents::SubagentWorkspaceRuntime>>,
    semantic_client: OnceLock<Option<crate::semantic::EmbeddingsClient>>,
    semantic: tokio::sync::Mutex<Option<crate::semantic::SemanticIndexerHandle>>,
    workflow_enabled: std::sync::atomic::AtomicBool,
}

impl WorkspaceRuntime {
    pub(crate) fn snapshot(&self) -> Arc<neoism_agent_plugin_api::RegistrySnapshot> {
        self.snapshot.read().expect("workspace snapshot lock poisoned").clone()
    }

    fn lifecycle(&self) -> Arc<WorkspaceLifecycle> {
        self.lifecycle.read().expect("workspace lifecycle lock poisoned").clone()
    }

    pub(crate) fn mcp(&self) -> Arc<crate::mcp::McpRuntimeManager> {
        self.lifecycle().mcp.get_or_init(Default::default).clone()
    }

    #[cfg(test)]
    pub(crate) fn mcp_is_allocated(&self) -> bool {
        self.lifecycle().mcp.get().is_some()
    }

    pub(crate) fn mcp_if_allocated(&self) -> Option<Arc<crate::mcp::McpRuntimeManager>> {
        self.lifecycle().mcp.get().cloned()
    }

    pub(crate) fn lsp(&self) -> crate::lsp::LspRuntime {
        self.lifecycle().lsp.get_or_init(|| crate::lsp::LspRuntime::new(self.services.clone())).clone()
    }

    pub(crate) fn lsp_if_allocated(&self) -> Option<crate::lsp::LspRuntime> {
        self.lifecycle().lsp.get().cloned()
    }

    pub(crate) fn pty(&self) -> Arc<crate::pty::PtyWorkspaceRuntime> {
        self.lifecycle().pty.get_or_init(Default::default).clone()
    }

    pub(crate) fn pty_if_allocated(&self) -> Option<Arc<crate::pty::PtyWorkspaceRuntime>> {
        self.lifecycle().pty.get().cloned()
    }

    pub(crate) fn background(&self) -> Arc<crate::background_job::BackgroundWorkspaceRuntime> {
        self.lifecycle().background.get_or_init(Default::default).clone()
    }

    pub(crate) fn background_if_allocated(&self) -> Option<Arc<crate::background_job::BackgroundWorkspaceRuntime>> {
        self.lifecycle().background.get().cloned()
    }

    pub(crate) fn subagents(&self) -> Arc<crate::plugins::subagents::SubagentWorkspaceRuntime> {
        self.lifecycle().subagents.get_or_init(Default::default).clone()
    }

    pub(crate) fn subagents_if_allocated(&self) -> Option<Arc<crate::plugins::subagents::SubagentWorkspaceRuntime>> {
        self.lifecycle().subagents.get().cloned()
    }

    pub(crate) fn set_workflow_enabled(&self, enabled: bool) {
        self.lifecycle().workflow_enabled.store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) async fn teardown(&self, state: &crate::state::AppState) {
        let lifecycle = self.lifecycle();
        if let Some(mcp) = lifecycle.mcp.get() {
            mcp.shutdown_workspace(self.root.to_string_lossy().as_ref()).await;
        }
        if let Some(lsp) = lifecycle.lsp.get() {
            lsp.shutdown_root(&self.root);
        }
        if let Some(pty) = lifecycle.pty.get() {
            pty.shutdown().await;
        }
        if let Some(background) = lifecycle.background.get() {
            background.cancel_and_clear().await;
        }
        if let Some(subagents) = lifecycle.subagents.get() {
            subagents.teardown(state, &self.root).await;
        }
        if let Some(indexer) = lifecycle.semantic.lock().await.take() {
            indexer.shutdown().await;
        }
        self.set_workflow_enabled(false);
        crate::workflow::workspace_disabled(state, &self.root).await;
    }

    pub(crate) async fn replace_generation(&self, state: &crate::state::AppState) {
        self.teardown(state).await;
        *self.lifecycle.write().expect("workspace lifecycle lock poisoned") = Arc::new(WorkspaceLifecycle::default());
    }

    pub(crate) async fn enable_semantic(&self, state: crate::state::AppState) {
        let lifecycle = self.lifecycle();
        let client = lifecycle.semantic_client.get_or_init(|| crate::semantic::EmbeddingsClient::from_env(&state.inner.auth_store)).clone();
        let mut indexer = lifecycle.semantic.lock().await;
        if indexer.is_none() {
            *indexer = crate::semantic::spawn_indexer(state, self.root.clone(), client);
        }
    }

    pub(crate) fn semantic_client(&self, state: &crate::state::AppState) -> Option<crate::semantic::EmbeddingsClient> {
        self.lifecycle().semantic_client.get_or_init(|| crate::semantic::EmbeddingsClient::from_env(&state.inner.auth_store)).clone()
    }
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
        state: &crate::state::AppState,
    ) -> (Arc<WorkspaceRuntime>, Vec<Arc<WorkspaceRuntime>>) {
        let services = state.services();
        let root = canonical_location(directory);
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        let stale = entries.iter().filter_map(|(root, entry)| {
            (Arc::strong_count(&entry.runtime) == 1
                && now.duration_since(entry.last_used) >= IDLE_TTL)
                .then(|| root.clone())
        }).collect::<Vec<_>>();
        let evicted = stale.into_iter().filter_map(|root| entries.remove(&root).map(|entry| entry.runtime)).collect::<Vec<_>>();
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
                refresh_plugins(&runtime, state);
            }
            return (runtime, evicted);
        }

        let signature = config_signature(services, &root);
        let host = crate::plugins::build_host(state, &root.to_string_lossy())
            .expect("built-in workspace plugin registration must be valid");
        let runtime = Arc::new(WorkspaceRuntime {
            root: root.clone(),
            services: services.clone(),
            lifecycle: RwLock::new(Arc::new(WorkspaceLifecycle::default())),
            snapshot: RwLock::new(host.snapshot()),
            signature: RwLock::new(signature),
        });
        entries.insert(
            root,
            RuntimeEntry {
                runtime: runtime.clone(),
                last_used: now,
                last_config_refresh: now,
            },
        );
        (runtime, evicted)
    }

    pub(crate) async fn runtimes(&self) -> Vec<Arc<WorkspaceRuntime>> {
        self.entries.lock().await.values().map(|entry| entry.runtime.clone()).collect()
    }

    pub(crate) fn loaded(&self, directory: &str) -> Option<Arc<WorkspaceRuntime>> {
        self.entries.try_lock().ok()?.get(&canonical_location(directory)).map(|entry| entry.runtime.clone())
    }

    #[cfg(test)]
    pub(crate) async fn evict(&self, directory: &str) -> Option<Arc<WorkspaceRuntime>> {
        self.entries.lock().await.remove(&canonical_location(directory)).map(|entry| entry.runtime)
    }
}

fn refresh_plugins(runtime: &WorkspaceRuntime, state: &crate::state::AppState) {
    let services = state.services();
    let signature = config_signature(services, &runtime.root);
    {
        let current = runtime.signature.read().expect("workspace signature lock poisoned");
        if *current == signature {
            return;
        }
    }
    match crate::plugins::build_host(state, &runtime.root.to_string_lossy()) {
        Ok(host) => {
            let old_generation = runtime.snapshot().generation;
            let mut next = (*host.snapshot()).clone();
            next.generation = old_generation.saturating_add(1);
            *runtime.snapshot.write().expect("workspace snapshot lock poisoned") = Arc::new(next);
            *runtime.signature.write().expect("workspace signature lock poisoned") = signature;
        }
        Err(error) => tracing::error!(%error, root = %runtime.root.display(), "workspace plugin generation rejected"),
    }
}

fn config_signature(
    services: &neoism_agent_service_api::AgentServices,
    root: &Path,
) -> Vec<u8> {
    crate::config::load(services, &root.to_string_lossy())
        .ok()
        .and_then(|loaded| serde_json::to_vec(&loaded.info).ok())
        .unwrap_or_default()
}

pub(crate) fn canonical_location(directory: &str) -> PathBuf {
    let path = Path::new(directory);
    crate::windows_process::canonicalize_path(path).unwrap_or_else(|_| {
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
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let registry = WorkspaceRuntimeRegistry::default();
        let (first, _) = registry.acquire(&root.to_string_lossy(), &state).await;
        let (alias, _) = registry
            .acquire(&root.join(".").to_string_lossy(), &state)
            .await;
        let (second, _) = registry.acquire(&other.to_string_lossy(), &state).await;
        assert!(Arc::ptr_eq(&first, &alias));
        assert!(!Arc::ptr_eq(&first, &second));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn alias_eviction_tears_down_only_that_workspace_and_drops_generation() {
        let root = std::env::temp_dir().join(format!(
            "neoism-workspace-lifecycle-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let first = state.workspace_runtime(root.to_string_lossy().as_ref()).await;
        let second = state.workspace_runtime(other.to_string_lossy().as_ref()).await;
        let first_pty = first.pty();
        let second_pty = second.pty();
        let first_info = crate::pty::create_pty_info(Default::default(), first.root.to_string_lossy().into_owned(), crate::pty::fallback_shell(), crate::now_millis());
        let second_info = crate::pty::create_pty_info(Default::default(), second.root.to_string_lossy().into_owned(), crate::pty::fallback_shell(), crate::now_millis());
        first_pty.infos.write().await.insert(first_info.id.clone(), first_info);
        second_pty.infos.write().await.insert(second_info.id.clone(), second_info);

        let alias = root.join(".");
        let evicted = state.inner.workspace_runtimes.evict(alias.to_string_lossy().as_ref()).await.unwrap();
        evicted.teardown(&state).await;
        state.inner.workspace_plugin_generations.lock().await.remove(&evicted.root);

        assert!(first_pty.infos.read().await.is_empty());
        assert_eq!(second_pty.infos.read().await.len(), 1);
        assert!(!state.inner.workspace_plugin_generations.lock().await.contains_key(&first.root));
        assert!(state.inner.workspace_plugin_generations.lock().await.contains_key(&second.root));
        state.shutdown().await;
        let _ = std::fs::remove_dir_all(root);
    }
}
