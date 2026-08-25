use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const IDLE_TTL: Duration = Duration::from_secs(60 * 60);
const CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct WorkspaceRuntime {
    pub(crate) root: PathBuf,
    services: neoism_agent_service_api::AgentServices,
    lifecycle: RwLock<Arc<WorkspaceLifecycle>>,
    generation: PluginGenerationSlot,
    signature: RwLock<Vec<u8>>,
}

/// An opaque, exactly-once retirement hook for plugin-owned resources.
///
/// The server deliberately does not know the resource type. Cloning this
/// handle is a lease: replacement removes the generation from publication,
/// while retirement waits until all in-flight leases have been dropped.
pub(crate) struct PluginLifecycleHandle {
    teardown: StdMutex<Option<Box<dyn FnOnce() + Send + 'static>>>,
}

impl PluginLifecycleHandle {
    pub(crate) fn new(teardown: impl FnOnce() + Send + 'static) -> Self {
        Self { teardown: StdMutex::new(Some(Box::new(teardown))) }
    }

    pub(crate) fn shutdown(&self) {
        if let Some(teardown) = self.teardown.lock().expect("plugin lifecycle lock poisoned").take() {
            teardown();
        }
    }
}

impl Drop for PluginLifecycleHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One immutable, ready-to-serve plugin generation.
pub(crate) struct PluginLifecycleRegistry {
    handles: HashMap<String, Arc<PluginLifecycleHandle>>,
}

impl PluginLifecycleRegistry {
    fn len(&self) -> usize {
        self.handles.len()
    }

    #[cfg(test)]
    pub(crate) fn get(&self, plugin_id: &str) -> Option<Arc<PluginLifecycleHandle>> {
        self.handles.get(plugin_id).cloned()
    }
}

pub(crate) struct PluginGeneration {
    snapshot: Arc<neoism_agent_plugin_api::RegistrySnapshot>,
    lifecycles: PluginLifecycleRegistry,
}

impl PluginGeneration {
    /// Build privately. A failed readiness check drops all candidate handles;
    /// callers cannot publish a partially-ready generation.
    pub(crate) fn build(
        snapshot: Arc<neoism_agent_plugin_api::RegistrySnapshot>,
        configure: impl FnOnce(&mut PluginGenerationBuilder) -> Result<(), String>,
    ) -> Result<Arc<Self>, String> {
        let mut builder = PluginGenerationBuilder::default();
        configure(&mut builder)?;
        Ok(Arc::new(Self {
            snapshot,
            lifecycles: PluginLifecycleRegistry { handles: builder.lifecycles },
        }))
    }

    fn empty(snapshot: Arc<neoism_agent_plugin_api::RegistrySnapshot>) -> Arc<Self> {
        let plugin_ids = snapshot
            .manifests
            .iter()
            .map(|manifest| manifest.id.clone())
            .collect::<Vec<_>>();
        Self::build(snapshot, |builder| {
            for plugin_id in plugin_ids {
                builder.register(plugin_id, || Ok(()), || {})?;
            }
            Ok(())
        })
        .expect("installed plugin generation is ready")
    }

    pub(crate) fn snapshot(&self) -> Arc<neoism_agent_plugin_api::RegistrySnapshot> {
        self.snapshot.clone()
    }

    #[cfg(test)]
    pub(crate) fn lifecycle(&self, plugin_id: &str) -> Option<Arc<PluginLifecycleHandle>> {
        self.lifecycles.get(plugin_id)
    }
}

#[derive(Default)]
pub(crate) struct PluginGenerationBuilder {
    lifecycles: HashMap<String, Arc<PluginLifecycleHandle>>,
}

impl PluginGenerationBuilder {
    pub(crate) fn register(
        &mut self,
        plugin_id: impl Into<String>,
        ready: impl FnOnce() -> Result<(), String>,
        teardown: impl FnOnce() + Send + 'static,
    ) -> Result<(), String> {
        let plugin_id = plugin_id.into();
        let handle = Arc::new(PluginLifecycleHandle::new(teardown));
        ready()?;
        if self.lifecycles.contains_key(&plugin_id) {
            return Err(format!("duplicate plugin lifecycle: {plugin_id}"));
        }
        self.lifecycles.insert(plugin_id, handle);
        Ok(())
    }
}

struct PluginGenerationSlot {
    published: RwLock<Arc<PluginGeneration>>,
}

impl PluginGenerationSlot {
    fn new(generation: Arc<PluginGeneration>) -> Self {
        Self { published: RwLock::new(generation) }
    }

    fn load(&self) -> Arc<PluginGeneration> {
        self.published.read().expect("plugin generation lock poisoned").clone()
    }

    /// The only publication point. The fully-built candidate becomes visible
    /// in one write; the old generation retires when its final Arc lease ends.
    fn publish(&self, candidate: Arc<PluginGeneration>) {
        tracing::debug!(
            generation = candidate.snapshot.generation,
            plugins = candidate.lifecycles.len(),
            "publishing ready plugin generation"
        );
        *self.published.write().expect("plugin generation lock poisoned") = candidate;
    }
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
    pub(crate) fn plugin_generation(&self) -> Arc<PluginGeneration> {
        self.generation.load()
    }

    pub(crate) fn snapshot(&self) -> Arc<neoism_agent_plugin_api::RegistrySnapshot> {
        self.plugin_generation().snapshot()
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
            generation: PluginGenerationSlot::new(PluginGeneration::empty(host.snapshot())),
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
            let candidate = PluginGeneration::empty(Arc::new(next));
            runtime.generation.publish(candidate);
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_generation(
        generation: u64,
        plugin_id: &str,
        teardown_count: Arc<AtomicUsize>,
    ) -> Arc<PluginGeneration> {
        let mut snapshot = neoism_agent_plugin_api::RegistrySnapshot::empty();
        snapshot.generation = generation;
        PluginGeneration::build(Arc::new(snapshot), |builder| {
            builder.register(
                plugin_id,
                || Ok(()),
                move || {
                    teardown_count.fetch_add(1, Ordering::SeqCst);
                },
            )
        })
        .unwrap()
    }

    #[test]
    fn failed_readiness_is_never_publishable_and_tears_down_candidate() {
        let teardowns = Arc::new(AtomicUsize::new(0));
        let count = teardowns.clone();
        let result = PluginGeneration::build(
            Arc::new(neoism_agent_plugin_api::RegistrySnapshot::empty()),
            |builder| {
                builder.register(
                    "test.not-ready",
                    || Err("not ready".to_string()),
                    move || {
                        count.fetch_add(1, Ordering::SeqCst);
                    },
                )
            },
        );
        assert!(result.is_err());
        assert_eq!(teardowns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn replacement_publishes_complete_generation_atomically() {
        let old_count = Arc::new(AtomicUsize::new(0));
        let new_count = Arc::new(AtomicUsize::new(0));
        let slot = PluginGenerationSlot::new(test_generation(1, "test.old", old_count.clone()));
        let candidate = test_generation(2, "test.new", new_count.clone());

        slot.publish(candidate);
        let current = slot.load();
        assert_eq!(current.snapshot.generation, 2);
        assert!(current.lifecycle("test.new").is_some());
        assert!(current.lifecycle("test.old").is_none());
        assert_eq!(old_count.load(Ordering::SeqCst), 1);
        assert_eq!(new_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn replacement_retains_old_generation_for_in_flight_arc_users() {
        let old_count = Arc::new(AtomicUsize::new(0));
        let slot = PluginGenerationSlot::new(test_generation(1, "test.plugin", old_count.clone()));
        let in_flight = slot.load();

        slot.publish(PluginGeneration::empty(Arc::new(
            neoism_agent_plugin_api::RegistrySnapshot::empty(),
        )));
        assert_eq!(old_count.load(Ordering::SeqCst), 0);
        drop(in_flight);
        assert_eq!(old_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn plugin_generations_are_isolated_per_workspace_slot() {
        let first_count = Arc::new(AtomicUsize::new(0));
        let second_count = Arc::new(AtomicUsize::new(0));
        let first = PluginGenerationSlot::new(test_generation(1, "test.plugin", first_count.clone()));
        let second = PluginGenerationSlot::new(test_generation(1, "test.plugin", second_count.clone()));

        first.publish(PluginGeneration::empty(Arc::new(
            neoism_agent_plugin_api::RegistrySnapshot::empty(),
        )));
        assert_eq!(first_count.load(Ordering::SeqCst), 1);
        assert_eq!(second_count.load(Ordering::SeqCst), 0);
        assert!(second.load().lifecycle("test.plugin").is_some());
    }

    #[test]
    fn lifecycle_shutdown_runs_exactly_once() {
        let count = Arc::new(AtomicUsize::new(0));
        let teardown_count = count.clone();
        let handle = PluginLifecycleHandle::new(move || {
            teardown_count.fetch_add(1, Ordering::SeqCst);
        });
        handle.shutdown();
        handle.shutdown();
        drop(handle);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

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
