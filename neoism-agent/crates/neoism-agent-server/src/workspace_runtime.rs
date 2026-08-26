use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex, RwLock, Weak};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures::FutureExt;
use tokio::sync::Mutex;

tokio::task_local! {
    static ACTIVE_PLUGIN_GENERATION: PluginGenerationLease;
}

const IDLE_TTL: Duration = Duration::from_secs(60 * 60);
const CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const PLUGIN_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(10);

async fn bounded_plugin_lifecycle<F>(operation: &str, future: F) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError>
where
    F: Future<Output = Result<(), neoism_agent_plugin_api::PluginRuntimeError>>,
{
    bounded_plugin_lifecycle_with_timeout(operation, PLUGIN_LIFECYCLE_TIMEOUT, future).await
}

async fn bounded_plugin_lifecycle_with_timeout<F>(operation: &str, timeout: Duration, future: F) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError>
where
    F: Future<Output = Result<(), neoism_agent_plugin_api::PluginRuntimeError>>,
{
    match tokio::time::timeout(timeout, std::panic::AssertUnwindSafe(future).catch_unwind()).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(neoism_agent_plugin_api::PluginRuntimeError::new(format!("plugin {operation} panicked"))),
        Err(_) => Err(neoism_agent_plugin_api::PluginRuntimeError::new(format!("plugin {operation} timed out"))),
    }
}

pub(crate) fn managed_plugin_factory(
    inner: Box<dyn neoism_agent_plugin_api::PluginFactory>,
    lifecycle: Arc<WorkspaceLifecycle>,
    root: PathBuf,
) -> Box<dyn neoism_agent_plugin_api::PluginFactory> {
    let descriptor = inner.descriptor();
    Box::new(ManagedPluginFactory { inner, descriptor, lifecycle, root })
}

struct ManagedPluginFactory {
    inner: Box<dyn neoism_agent_plugin_api::PluginFactory>,
    descriptor: neoism_agent_plugin_api::PluginDescriptor,
    lifecycle: Arc<WorkspaceLifecycle>,
    root: PathBuf,
}

impl neoism_agent_plugin_api::PluginFactory for ManagedPluginFactory {
    fn descriptor(&self) -> neoism_agent_plugin_api::PluginDescriptor { self.descriptor.clone() }

    fn create<'a>(&'a self, context: neoism_agent_plugin_api::PluginContext) -> neoism_agent_plugin_api::PluginFuture<'a, Box<dyn neoism_agent_plugin_api::PluginInstance>> {
        Box::pin(async move {
            let plugin_id = self.descriptor.manifest.id.clone();
            let inner = self.inner.create(context).await?;
            Ok(Box::new(ManagedPluginInstance {
                inner,
                plugin_id,
                lifecycle: self.lifecycle.clone(),
                root: self.root.clone(),
                shutdown: AtomicU8::new(MANAGED_OPEN),
            }) as Box<dyn neoism_agent_plugin_api::PluginInstance>)
        })
    }
}

struct ManagedPluginInstance {
    inner: Box<dyn neoism_agent_plugin_api::PluginInstance>,
    plugin_id: String,
    lifecycle: Arc<WorkspaceLifecycle>,
    root: PathBuf,
    shutdown: AtomicU8,
}

impl neoism_agent_plugin_api::PluginInstance for ManagedPluginInstance {
    fn start<'a>(&'a self) -> neoism_agent_plugin_api::PluginFuture<'a, ()> { self.inner.start() }
    fn readiness(&self) -> neoism_agent_plugin_api::PluginReadiness { self.inner.readiness() }
    fn contributions(&self) -> neoism_agent_plugin_api::PluginContributions { self.inner.contributions() }
    fn shutdown<'a>(&'a self) -> neoism_agent_plugin_api::PluginFuture<'a, ()> {
        Box::pin(async move {
            match self.shutdown.compare_exchange(MANAGED_OPEN, MANAGED_RUNNING, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {}
                Err(MANAGED_CLOSED) => return Ok(()),
                Err(_) => return Err(neoism_agent_plugin_api::PluginRuntimeError::new("managed plugin shutdown is already in progress")),
            }
            self.lifecycle.close();
            let mut attempt = ManagedShutdownAttempt { state: &self.shutdown, complete: false };
            self.inner.shutdown().await?;
            self.lifecycle.shutdown_plugin(&self.plugin_id, &self.root).await?;
            self.shutdown.store(MANAGED_CLOSED, Ordering::Release);
            attempt.complete = true;
            Ok(())
        })
    }
}

const MANAGED_OPEN: u8 = 0;
const MANAGED_RUNNING: u8 = 1;
const MANAGED_CLOSED: u8 = 2;
struct ManagedShutdownAttempt<'a> { state: &'a AtomicU8, complete: bool }
impl Drop for ManagedShutdownAttempt<'_> {
    fn drop(&mut self) { if !self.complete { self.state.store(MANAGED_OPEN, Ordering::Release); } }
}

pub(crate) struct WorkspaceRuntime {
    pub(crate) root: PathBuf,
    services: neoism_agent_service_api::AgentServices,
    generation: PluginGenerationSlot,
    signature: RwLock<Vec<u8>>,
    reload: Mutex<()>,
    next_generation: AtomicU64,
    closed: AtomicBool,
}

pub(crate) struct PluginGeneration {
    snapshot: Arc<neoism_agent_plugin_api::RegistrySnapshot>,
    config: Arc<neoism_agent_core::AgentConfigDocument>,
    lifecycle: Arc<WorkspaceLifecycle>,
    services: neoism_agent_service_api::AgentServices,
    root: PathBuf,
    installed: Arc<neoism_agent_plugin_api::InstalledPlugins>,
    lease_count: AtomicUsize,
    lease_released: tokio::sync::Notify,
    retiring: AtomicBool,
    websocket_cancel: tokio::sync::watch::Sender<bool>,
}

#[derive(Clone)]
pub(crate) struct PluginGenerationLease {
    inner: Arc<PluginGeneration>,
    _token: Arc<GenerationLeaseToken>,
}

pub(crate) struct LeasedResource<T> {
    resource: Arc<T>,
    _generation: PluginGenerationLease,
}

impl<T> Deref for LeasedResource<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { &self.resource }
}

struct GenerationLeaseToken { generation: Arc<PluginGeneration> }
impl Drop for GenerationLeaseToken {
    fn drop(&mut self) {
        self.generation.lease_count.fetch_sub(1, Ordering::AcqRel);
        self.generation.lease_released.notify_waiters();
    }
}


impl Deref for PluginGenerationLease {
    type Target = neoism_agent_plugin_api::RegistrySnapshot;

    fn deref(&self) -> &Self::Target {
        &self.inner.snapshot
    }
}

impl AsRef<neoism_agent_plugin_api::RegistrySnapshot> for PluginGenerationLease {
    fn as_ref(&self) -> &neoism_agent_plugin_api::RegistrySnapshot {
        self
    }
}

impl PluginGenerationLease {
    fn try_new(inner: Arc<PluginGeneration>) -> Option<Self> {
        if inner.retiring.load(Ordering::Acquire) { return None; }
        inner.lease_count.fetch_add(1, Ordering::AcqRel);
        if inner.retiring.load(Ordering::Acquire) {
            inner.lease_count.fetch_sub(1, Ordering::AcqRel);
            inner.lease_released.notify_waiters();
            return None;
        }
        let token = Arc::new(GenerationLeaseToken { generation: inner.clone() });
        Some(Self { inner, _token: token })
    }

    fn new(inner: Arc<PluginGeneration>) -> Self {
        inner.lease_count.fetch_add(1, Ordering::AcqRel);
        let token = Arc::new(GenerationLeaseToken { generation: inner.clone() });
        Self { inner, _token: token }
    }
    pub(crate) fn config(&self) -> &neoism_agent_core::AgentConfigDocument {
        &self.inner.config
    }

    pub(crate) fn mcp(&self) -> Result<Arc<crate::mcp::McpRuntimeManager>, neoism_agent_plugin_api::PluginRuntimeError> {
        self.inner
            .state_with_shutdown(neoism_agent_builtins::plugin::mcp::ID, Default::default, |runtime: Arc<crate::mcp::McpRuntimeManager>, root| async move {
                runtime.shutdown_workspace(root.to_string_lossy().as_ref()).await;
            })
    }

    pub(crate) fn lsp(&self) -> Result<crate::lsp::LspRuntime, neoism_agent_plugin_api::PluginRuntimeError> {
        Ok((*self.inner.state_with_shutdown(
            neoism_agent_builtins::plugin::lsp::ID,
            || {
                crate::lsp::LspRuntime::new_with_config(
                    self.inner.services.clone(),
                    self.inner.config.clone(),
                )
            },
            |runtime: Arc<crate::lsp::LspRuntime>, root| async move { runtime.shutdown_root(&root); },
        )?)
        .clone())
    }

    pub(crate) fn background(&self) -> Result<Arc<crate::background_job::BackgroundWorkspaceRuntime>, neoism_agent_plugin_api::PluginRuntimeError> {
        self.inner
            .state_with_shutdown(neoism_agent_builtins::plugin::workspace_tools::ID, Default::default, |runtime: Arc<crate::background_job::BackgroundWorkspaceRuntime>, _| async move { runtime.cancel_and_clear().await; })
    }

    pub(crate) fn subagents(&self) -> Result<Arc<crate::plugins::subagents::SubagentWorkspaceRuntime>, neoism_agent_plugin_api::PluginRuntimeError> {
        self.inner
            .state_with_shutdown(neoism_agent_builtins::plugin::subagents::ID, Default::default, |runtime: Arc<crate::plugins::subagents::SubagentWorkspaceRuntime>, _| async move { runtime.teardown().await; })
    }

    pub(crate) fn pty(&self) -> Result<Arc<crate::pty::PtyWorkspaceRuntime>, neoism_agent_plugin_api::PluginRuntimeError> {
        self.inner
            .state_with_shutdown(neoism_agent_builtins::plugin::pty::ID, Default::default, |runtime: Arc<crate::pty::PtyWorkspaceRuntime>, _| async move { runtime.shutdown().await; })
    }

    pub(crate) fn pty_if_allocated(&self) -> Option<Arc<crate::pty::PtyWorkspaceRuntime>> {
        self.inner
            .lifecycle
            .state_if_allocated(neoism_agent_builtins::plugin::pty::ID)
    }

    pub(crate) fn websocket_cancellation(&self) -> tokio::sync::watch::Receiver<bool> {
        self.inner.websocket_cancel.subscribe()
    }

    fn belongs_to(&self, directory: &str) -> bool {
        self.inner.root == canonical_location(directory)
    }

    pub(crate) fn set_workflow_enabled(&self, enabled: bool, state: crate::state::AppState) {
        if !enabled { return; }
        let state = Arc::downgrade(&state.inner);
        let Ok(workflow) = self.inner
            .state_with_shutdown(neoism_agent_builtins::plugin::workflows::ID, WorkflowLifecycle::default, move |runtime: Arc<WorkflowLifecycle>, root| { let state = state.clone(); async move {
                runtime.workflow_enabled.store(false, Ordering::SeqCst);
                if let Some(inner) = state.upgrade() { crate::workflow::workspace_disabled(&crate::state::AppState { inner }, &root).await; }
            } })
        else { return; };
        workflow.workflow_enabled.store(true, Ordering::SeqCst);
    }

    pub(crate) async fn enable_semantic(&self, state: crate::state::AppState) {
        let Ok(semantic) = self
            .inner
            .state_with_shutdown(neoism_agent_builtins::plugin::semantic::ID, SemanticLifecycle::default, |runtime: Arc<SemanticLifecycle>, _| async move { if let Some(indexer) = runtime.indexer.lock().await.take() { indexer.shutdown().await; } })
        else { return; };
        let auth = if let Some(provider_id) = crate::semantic::EmbeddingsClient::configured_provider_id() {
            state.inner.provider_service.auth(&provider_id).await.ok().flatten()
        } else {
            None
        };
        let client = {
            let mut client = semantic.client.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            client
                .get_or_insert_with(|| crate::semantic::EmbeddingsClient::from_env(auth))
                .clone()
        };
        let mut indexer = semantic.indexer.lock().await;
        if indexer.is_none() {
            *indexer = crate::semantic::spawn_indexer(state, self.inner.root.clone(), client);
        }
    }

    pub(crate) fn semantic_client(&self) -> Option<crate::semantic::EmbeddingsClient> {
        let semantic = self
            .inner
            .state_with_shutdown(neoism_agent_builtins::plugin::semantic::ID, SemanticLifecycle::default, |runtime: Arc<SemanticLifecycle>, _| async move { if let Some(indexer) = runtime.indexer.lock().await.take() { indexer.shutdown().await; } }).ok()?;
        let client = semantic
            .client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .flatten();
        client
    }

    #[cfg(test)]
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

pub(crate) fn active_generation(directory: &str) -> Option<PluginGenerationLease> {
    ACTIVE_PLUGIN_GENERATION
        .try_with(|generation| generation.belongs_to(directory).then(|| generation.clone()))
        .ok()
        .flatten()
}

pub(crate) fn closed_snapshot() -> PluginGenerationLease {
    PluginGenerationLease::new(PluginGeneration::empty(Arc::new(
        neoism_agent_plugin_api::RegistrySnapshot::closed(),
    )))
}

pub(crate) async fn scope_generation<F: std::future::Future>(
    generation: PluginGenerationLease,
    future: F,
) -> F::Output {
    ACTIVE_PLUGIN_GENERATION.scope(generation, future).await
}

impl PluginGeneration {
    fn state_with_shutdown<T, F, Fut>(&self, plugin_id: &str, create: impl FnOnce() -> T, shutdown: F) -> Result<Arc<T>, neoism_agent_plugin_api::PluginRuntimeError>
    where
        T: Send + Sync + 'static,
        F: Fn(Arc<T>, PathBuf) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.snapshot.ensure_active()?;
        if !self.snapshot.manifests.iter().any(|manifest| manifest.id == plugin_id) {
            return Err(neoism_agent_plugin_api::PluginRuntimeError::new(format!("plugin `{plugin_id}` is not installed")));
        }
        self.lifecycle.state_with_shutdown(plugin_id, create, shutdown)
    }

    async fn shutdown(&self) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        self.lifecycle.close();
        self.installed.shutdown().await
    }

    async fn pre_drain(&self, timeout: Duration) -> Vec<String> {
        let mut errors = Vec::new();
        let _ = self.websocket_cancel.send(true);
        self.lifecycle.close();
        for plugin_id in [
            neoism_agent_builtins::plugin::pty::ID,
            neoism_agent_builtins::plugin::workspace_tools::ID,
            neoism_agent_builtins::plugin::subagents::ID,
        ] {
            if let Err(error) = bounded_plugin_lifecycle_with_timeout("pre-drain", timeout, self.lifecycle.shutdown_plugin(plugin_id, &self.root)).await {
                tracing::warn!(%error, %plugin_id, "plugin pre-drain failed");
                errors.push(format!("{plugin_id} pre-drain: {error}"));
            }
        }
        errors
    }

    async fn wait_until_unleased(&self) {
        loop {
            let released = self.lease_released.notified();
            if self.lease_count.load(Ordering::Acquire) == 0 { break; }
            released.await;
        }
    }

    fn build(
        snapshot: Arc<neoism_agent_plugin_api::RegistrySnapshot>,
        installed: Arc<neoism_agent_plugin_api::InstalledPlugins>,
        config: Arc<neoism_agent_core::AgentConfigDocument>,
        lifecycle: Arc<WorkspaceLifecycle>,
        services: neoism_agent_service_api::AgentServices,
        root: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            snapshot,
            config,
            lifecycle,
            services,
            root,
            installed,
            lease_count: AtomicUsize::new(0),
            lease_released: tokio::sync::Notify::new(),
            retiring: AtomicBool::new(false),
            websocket_cancel: tokio::sync::watch::channel(false).0,
        })
    }

    fn empty(snapshot: Arc<neoism_agent_plugin_api::RegistrySnapshot>) -> Arc<Self> {
        let installed = Arc::new(neoism_agent_plugin_api::InstalledPlugins::empty(snapshot.clone()));
        Self::build(
            snapshot,
            installed,
            Arc::new(neoism_agent_core::AgentConfigDocument::default()),
            Arc::new(WorkspaceLifecycle::default()),
            crate::standard_services(),
            PathBuf::new(),
        )
    }

    fn workspace(
        installed: neoism_agent_plugin_api::InstalledPlugins,
        config: Arc<neoism_agent_core::AgentConfigDocument>,
        lifecycle: Arc<WorkspaceLifecycle>,
        state: &crate::state::AppState,
        root: PathBuf,
    ) -> Arc<Self> {
        let snapshot = installed.snapshot();
        Self::build(snapshot, Arc::new(installed), config, lifecycle, state.services().clone(), root)
    }
}

struct PluginGenerationSlot {
    published: RwLock<Arc<PluginGeneration>>,
    retired: StdMutex<Vec<Weak<PluginGeneration>>>,
    retirements: StdMutex<Vec<RetirementTask>>,
    quarantine: Arc<StdMutex<Vec<GenerationQuarantine>>>,
}

struct RetirementTask {
    generation: Arc<PluginGeneration>,
    task: tokio::task::JoinHandle<()>,
}

struct GenerationQuarantine {
    generation: Arc<PluginGeneration>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl PluginGenerationSlot {
    #[cfg(test)]
    fn new(generation: Arc<PluginGeneration>) -> Self {
        Self::with_quarantine(generation, Arc::new(StdMutex::new(Vec::new())))
    }

    fn with_quarantine(generation: Arc<PluginGeneration>, quarantine: Arc<StdMutex<Vec<GenerationQuarantine>>>) -> Self {
        Self {
            published: RwLock::new(generation),
            retired: StdMutex::new(Vec::new()),
            retirements: StdMutex::new(Vec::new()),
            quarantine,
        }
    }

    fn load(&self) -> Arc<PluginGeneration> {
        self.published.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn lease(&self, generation: u64) -> Option<PluginGenerationLease> {
        let published = self
            .published
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if published.snapshot.generation == generation {
            return PluginGenerationLease::try_new(published.clone());
        }
        drop(published);
        self.retired
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(Weak::upgrade)
            .find(|candidate| candidate.snapshot.generation == generation)
            .and_then(PluginGenerationLease::try_new)
    }

    /// The only publication point. The fully-built candidate becomes visible
    /// in one write; the old generation retires when its final Arc lease ends.
    fn publish(&self, candidate: Arc<PluginGeneration>) {
        tracing::debug!(
            generation = candidate.snapshot.generation,
            plugins = candidate.installed.len(),
            "publishing ready plugin generation"
        );
        let retired = {
            let mut published = self.published.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            published.retiring.store(true, Ordering::Release);
            std::mem::replace(&mut *published, candidate)
        };
        self.retired
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Arc::downgrade(&retired));
        let quarantine = self.quarantine.clone();
        let task_generation = retired.clone();
        let retirement = tokio::spawn(async move {
            task_generation.wait_until_unleased().await;
            let mut result = None;
            for _ in 0..3 {
                match bounded_plugin_lifecycle("retirement shutdown", task_generation.shutdown()).await {
                    Ok(()) => { result = None; break; }
                    Err(error) => {
                        tracing::warn!(%error, "retired plugin generation shutdown failed; retrying");
                        result = Some(error);
                    }
                }
            }
            if let Some(error) = result {
                tracing::error!(%error, "retired plugin generation moved to cleanup quarantine");
                quarantine.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(GenerationQuarantine { generation: task_generation, task: None });
            }
        });
        self.retirements.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(RetirementTask { generation: retired, task: retirement });
    }

    fn close_current(&self) -> Arc<PluginGeneration> {
        let published = self.published.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        published.retiring.store(true, Ordering::Release);
        published.clone()
    }

    fn retiring_generations(&self) -> Vec<Arc<PluginGeneration>> {
        let mut generations = self.retirements
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|retirement| retirement.generation.clone())
            .collect::<Vec<_>>();
        for generation in self.retired
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(Weak::upgrade)
        {
            if !generations.iter().any(|existing| Arc::ptr_eq(existing, &generation)) {
                generations.push(generation);
            }
        }
        generations
    }

    fn has_leases(&self) -> bool {
        let current_leased = self.published.read().unwrap_or_else(std::sync::PoisonError::into_inner).lease_count.load(Ordering::Acquire) > 0;
        let mut retired = self
            .retired
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        retired.retain(|generation| generation.strong_count() > 0);
        let retired_leased = retired.iter().filter_map(Weak::upgrade).any(|generation| generation.lease_count.load(Ordering::Acquire) > 0);
        let active_retirements = self.retirements.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().any(|retirement| !retirement.task.is_finished());
        current_leased || retired_leased || active_retirements
    }

    #[cfg(test)]
    async fn drain_retirements(&self) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        self.drain_retirements_with_timeout(PLUGIN_LIFECYCLE_TIMEOUT).await
    }

    async fn drain_retirements_with_timeout(&self, timeout: Duration) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        let tasks = std::mem::take(&mut *self.retirements.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let deadline = Instant::now() + timeout;
        let mut errors = Vec::new();
        for mut retirement in tasks {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                errors.push(format!("generation {} retirement task drain timed out", retirement.generation.snapshot.generation));
                self.quarantine.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(GenerationQuarantine { generation: retirement.generation, task: Some(retirement.task) });
                continue;
            };
            match tokio::time::timeout(remaining, &mut retirement.task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    errors.push(format!("generation {} retirement task failed: {error}", retirement.generation.snapshot.generation));
                    self.quarantine.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(GenerationQuarantine { generation: retirement.generation, task: None });
                }
                Err(_) => {
                    errors.push(format!("generation {} retirement task drain timed out", retirement.generation.snapshot.generation));
                    self.quarantine.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(GenerationQuarantine { generation: retirement.generation, task: Some(retirement.task) });
                }
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(neoism_agent_plugin_api::PluginRuntimeError::new(errors.join("; "))) }
    }
}

/// Optional plugin state for one canonical workspace and plugin generation.
///
/// Keeping these cells on the workspace runtime is important: application
/// kernel construction must not start plugin workers or allocate process maps,
/// and dropping/replacing a generation gives teardown one unambiguous owner.
pub(crate) struct WorkspaceLifecycle {
    states: StdMutex<HashMap<String, WorkspaceResource>>,
    closed: AtomicBool,
}

type ErasedResource = Arc<dyn Any + Send + Sync>;
type ResourceShutdown = Arc<dyn Fn(ErasedResource, PathBuf) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

#[derive(Clone)]
struct WorkspaceResource {
    value: ErasedResource,
    shutdown: ResourceShutdown,
}

impl Default for WorkspaceLifecycle {
    fn default() -> Self {
        Self { states: StdMutex::new(HashMap::new()), closed: AtomicBool::new(false) }
    }
}

impl WorkspaceLifecycle {
    fn close(&self) {
        let _states = self.states.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.closed.store(true, Ordering::Release);
    }

    fn state_with_shutdown<T, F, Fut>(&self, plugin_id: &str, create: impl FnOnce() -> T, shutdown: F) -> Result<Arc<T>, neoism_agent_plugin_api::PluginRuntimeError>
    where
        T: Send + Sync + 'static,
        F: Fn(Arc<T>, PathBuf) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.closed.load(Ordering::Acquire) { return Err(neoism_agent_plugin_api::PluginRuntimeError::new("plugin generation is shut down")); }
        let mut states = self.states.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.closed.load(Ordering::Acquire) { return Err(neoism_agent_plugin_api::PluginRuntimeError::new("plugin generation is shut down")); }
        let state = states.entry(plugin_id.to_string()).or_insert_with(|| {
            let value: ErasedResource = Arc::new(create());
            let shutdown: ResourceShutdown = Arc::new(move |value, root| {
                match value.downcast::<T>() {
                    Ok(value) => Box::pin(shutdown(value, root)),
                    Err(_) => Box::pin(async {}),
                }
            });
            WorkspaceResource { value, shutdown }
        }).value.clone();
        drop(states);
        state.downcast::<T>().map_err(|_| neoism_agent_plugin_api::PluginRuntimeError::new(format!("plugin state type mismatch for `{plugin_id}`")))
    }

    fn state_if_allocated<T: Send + Sync + 'static>(&self, plugin_id: &str) -> Option<Arc<T>> {
        let states = self.states.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.closed.load(Ordering::Acquire) { return None; }
        let state = states.get(plugin_id)?.value.clone();
        state.downcast::<T>().ok()
    }

    async fn shutdown_plugin(&self, plugin_id: &str, root: &Path) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        let resource = self.states.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(plugin_id).cloned();
        if let Some(resource) = resource {
            (resource.shutdown)(resource.value.clone(), root.to_path_buf()).await;
            let mut states = self.states.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if states.get(plugin_id).is_some_and(|current| Arc::ptr_eq(&current.value, &resource.value)) { states.remove(plugin_id); }
        }
        Ok(())
    }
}

#[derive(Default)]
struct SemanticLifecycle {
    client: StdMutex<Option<Option<crate::semantic::EmbeddingsClient>>>,
    indexer: tokio::sync::Mutex<Option<crate::semantic::SemanticIndexerHandle>>,
}

#[derive(Default)]
struct WorkflowLifecycle {
    workflow_enabled: std::sync::atomic::AtomicBool,
}

impl WorkspaceRuntime {
    pub(crate) fn snapshot(&self) -> PluginGenerationLease {
        if let Some(active) = active_generation(&self.root.to_string_lossy()) {
            return active;
        }
        self.published_snapshot()
    }

    pub(crate) fn published_snapshot(&self) -> PluginGenerationLease {
        if !self.closed.load(Ordering::Acquire) {
            if let Some(lease) = PluginGenerationLease::try_new(self.generation.load()) {
                return lease;
            }
        }
        closed_snapshot()
    }

    pub(crate) fn lease_generation(&self, generation: u64) -> Option<PluginGenerationLease> {
        self.generation.lease(generation)
    }

    pub(crate) fn mcp(&self) -> Result<LeasedResource<crate::mcp::McpRuntimeManager>, neoism_agent_plugin_api::PluginRuntimeError> {
        let generation = self.snapshot();
        let resource = generation.inner.state_with_shutdown(neoism_agent_builtins::plugin::mcp::ID, Default::default, |runtime: Arc<crate::mcp::McpRuntimeManager>, root| async move { runtime.shutdown_workspace(root.to_string_lossy().as_ref()).await; })?;
        Ok(LeasedResource { resource, _generation: generation })
    }

    #[cfg(test)]
    pub(crate) fn mcp_is_allocated(&self) -> bool {
        let generation = self.snapshot();
        generation.inner.lifecycle.state_if_allocated::<crate::mcp::McpRuntimeManager>(neoism_agent_builtins::plugin::mcp::ID).is_some()
    }

    #[cfg(test)]
    pub(crate) fn mcp_if_allocated(&self) -> Option<LeasedResource<crate::mcp::McpRuntimeManager>> {
        let generation = self.snapshot();
        let resource = generation.inner.lifecycle.state_if_allocated(neoism_agent_builtins::plugin::mcp::ID)?;
        Some(LeasedResource { resource, _generation: generation })
    }

    pub(crate) fn lsp(&self) -> Result<LeasedResource<crate::lsp::LspRuntime>, neoism_agent_plugin_api::PluginRuntimeError> {
        let generation = self.snapshot();
        let resource = generation.inner.state_with_shutdown(neoism_agent_builtins::plugin::lsp::ID, || {
            crate::lsp::LspRuntime::new_with_config(
                self.services.clone(),
                generation.inner.config.clone(),
            )
        }, |runtime: Arc<crate::lsp::LspRuntime>, root| async move { runtime.shutdown_root(&root); })?;
        Ok(LeasedResource { resource, _generation: generation })
    }

    pub(crate) fn lsp_if_allocated(&self) -> Option<LeasedResource<crate::lsp::LspRuntime>> {
        let generation = self.snapshot();
        let resource = generation.inner.lifecycle.state_if_allocated::<crate::lsp::LspRuntime>(neoism_agent_builtins::plugin::lsp::ID)?;
        Some(LeasedResource { resource, _generation: generation })
    }

    pub(crate) fn pty(&self) -> Result<LeasedResource<crate::pty::PtyWorkspaceRuntime>, neoism_agent_plugin_api::PluginRuntimeError> {
        let generation = self.snapshot();
        let resource = generation.inner.state_with_shutdown(neoism_agent_builtins::plugin::pty::ID, Default::default, |runtime: Arc<crate::pty::PtyWorkspaceRuntime>, _| async move { runtime.shutdown().await; })?;
        Ok(LeasedResource { resource, _generation: generation })
    }

    pub(crate) fn pty_if_allocated(&self) -> Option<LeasedResource<crate::pty::PtyWorkspaceRuntime>> {
        let generation = self.snapshot();
        let resource = generation.inner.lifecycle.state_if_allocated(neoism_agent_builtins::plugin::pty::ID)?;
        Some(LeasedResource { resource, _generation: generation })
    }

    pub(crate) fn background_if_allocated(&self) -> Option<LeasedResource<crate::background_job::BackgroundWorkspaceRuntime>> {
        let generation = self.snapshot();
        let resource = generation.inner.lifecycle.state_if_allocated(neoism_agent_builtins::plugin::workspace_tools::ID)?;
        Some(LeasedResource { resource, _generation: generation })
    }

    pub(crate) fn subagents(&self) -> Result<LeasedResource<crate::plugins::subagents::SubagentWorkspaceRuntime>, neoism_agent_plugin_api::PluginRuntimeError> {
        let generation = self.snapshot();
        let resource = generation.inner.state_with_shutdown(neoism_agent_builtins::plugin::subagents::ID, Default::default, |runtime: Arc<crate::plugins::subagents::SubagentWorkspaceRuntime>, _| async move { runtime.teardown().await; })?;
        Ok(LeasedResource { resource, _generation: generation })
    }

    pub(crate) fn subagents_if_allocated(&self) -> Option<LeasedResource<crate::plugins::subagents::SubagentWorkspaceRuntime>> {
        let generation = self.snapshot();
        let resource = generation.inner.lifecycle.state_if_allocated(neoism_agent_builtins::plugin::subagents::ID)?;
        Some(LeasedResource { resource, _generation: generation })
    }

    pub(crate) async fn teardown(&self, _state: &crate::state::AppState) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        self.teardown_with_timeout(PLUGIN_LIFECYCLE_TIMEOUT).await
    }

    async fn teardown_with_timeout(&self, timeout: Duration) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        self.closed.store(true, Ordering::Release);
        let _reload = self.reload.lock().await;
        let generation = self.generation.close_current();
        let mut errors = Vec::new();
        for retired in self.generation.retiring_generations() {
            errors.extend(retired.pre_drain(timeout).await.into_iter().map(|error| {
                format!("retired generation {} {error}", retired.snapshot.generation)
            }));
        }
        errors.extend(generation.pre_drain(timeout).await);
        if tokio::time::timeout(timeout, generation.wait_until_unleased()).await.is_err() {
            errors.push(format!("generation {} lease drain timed out", generation.snapshot.generation));
        }
        let mut shutdown_error = None;
        for _ in 0..3 {
            match bounded_plugin_lifecycle_with_timeout("workspace shutdown", timeout, generation.shutdown()).await {
                Ok(()) => { shutdown_error = None; break; }
                Err(error) => shutdown_error = Some(error),
            }
        }
        if let Some(error) = shutdown_error { errors.push(error.to_string()); }
        if let Err(error) = self.generation.drain_retirements_with_timeout(timeout).await {
            errors.push(error.to_string());
        }
        if errors.is_empty() { Ok(()) } else { Err(neoism_agent_plugin_api::PluginRuntimeError::new(errors.join("; "))) }
    }

}

pub(crate) struct WorkspaceRuntimeRegistry {
    entries: Mutex<HashMap<PathBuf, RuntimeEntry>>,
    failed_shutdowns: Mutex<Vec<Arc<WorkspaceRuntime>>>,
    generation_quarantine: Arc<StdMutex<Vec<GenerationQuarantine>>>,
    plugin_quarantine: Mutex<Vec<Arc<neoism_agent_plugin_api::InstalledPlugins>>>,
    closed: AtomicBool,
}

impl Default for WorkspaceRuntimeRegistry {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            failed_shutdowns: Mutex::new(Vec::new()),
            generation_quarantine: Arc::new(StdMutex::new(Vec::new())),
            plugin_quarantine: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        }
    }
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
    ) -> Result<(Arc<WorkspaceRuntime>, Vec<Arc<WorkspaceRuntime>>), String> {
        if self.closed.load(Ordering::Acquire) { return Err("workspace runtime registry is shut down".into()); }
        let services = state.services();
        let root = canonical_location(directory);
        let now = Instant::now();
        let entries = self.entries.lock().await;
        if self.closed.load(Ordering::Acquire) { return Err("workspace runtime registry is shut down".into()); }
        let stale = entries.iter().filter_map(|(root, entry)| {
            (Arc::strong_count(&entry.runtime) == 1
                && !entry.runtime.generation.has_leases()
                && now.duration_since(entry.last_used) >= IDLE_TTL)
                .then(|| (root.clone(), entry.runtime.clone()))
        }).collect::<Vec<_>>();
        drop(entries);
        let mut evicted = Vec::new();
        for (stale_root, stale_runtime) in stale {
            match stale_runtime.teardown(state).await {
                Ok(()) => {
                    let mut current = self.entries.lock().await;
                    if current.get(&stale_root).is_some_and(|entry| Arc::ptr_eq(&entry.runtime, &stale_runtime)) {
                        current.remove(&stale_root);
                        evicted.push(stale_runtime);
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, root = %stale_root.display(), "idle workspace cleanup failed; retaining runtime for retry");
                }
            }
        }
        let mut entries = self.entries.lock().await;
        if self.closed.load(Ordering::Acquire) { return Err("workspace runtime registry is shut down".into()); }
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
                if let Err(error) = refresh_plugins(&runtime, state).await {
                    if self.closed.load(Ordering::Acquire) || runtime.closed.load(Ordering::Acquire) {
                        return Err(error);
                    }
                    tracing::warn!(%error, root = %runtime.root.display(), "workspace plugin refresh failed; retaining current generation");
                }
            }
            if self.closed.load(Ordering::Acquire) || runtime.closed.load(Ordering::Acquire) {
                return Err("workspace runtime registry is shut down".into());
            }
            return Ok((runtime, evicted));
        }
        drop(entries);

        let signature = config_signature(services, &root).unwrap_or_default();
        let build = crate::plugins::build_host(state, &root.to_string_lossy()).await
            .or_else(|error| {
                tracing::error!(%error, root = %root.display(), "initial workspace configuration rejected; using safe defaults");
                Err(error)
            });
        let build = match build {
            Ok(build) => build,
            Err(_) => crate::plugins::build_default_host(state, &root.to_string_lossy())
                .await
                .map_err(|error| format!("default built-in workspace plugin registration failed: {error}"))?,
        };
        let generation = PluginGeneration::workspace(build.installed, build.config, build.lifecycle, state, root.clone());
        let next_generation = generation.snapshot.generation.saturating_add(1);
        let runtime = Arc::new(WorkspaceRuntime {
            root: root.clone(),
            services: services.clone(),
            generation: PluginGenerationSlot::with_quarantine(generation, self.generation_quarantine.clone()),
            signature: RwLock::new(signature),
            reload: Mutex::new(()),
            next_generation: AtomicU64::new(next_generation),
            closed: AtomicBool::new(false),
        });
        let mut entries = self.entries.lock().await;
        if self.closed.load(Ordering::Acquire) {
            drop(entries);
            let _ = runtime.teardown(state).await;
            for stale in evicted { let _ = stale.teardown(state).await; }
            return Err("workspace runtime registry is shut down".into());
        }
        if let Some(existing) = entries.get_mut(&root) {
            existing.last_used = now;
            let existing = existing.runtime.clone();
            drop(entries);
            let _ = runtime.teardown(state).await;
            return Ok((existing, evicted));
        }
        entries.insert(
            root,
            RuntimeEntry {
                runtime: runtime.clone(),
                last_used: now,
                last_config_refresh: now,
            },
        );
        Ok((runtime, evicted))
    }

    pub(crate) async fn runtimes(&self) -> Vec<Arc<WorkspaceRuntime>> {
        self.entries.lock().await.values().map(|entry| entry.runtime.clone()).collect()
    }

    pub(crate) async fn loaded(&self, directory: &str) -> Option<Arc<WorkspaceRuntime>> {
        self.entries.lock().await.get(&canonical_location(directory)).map(|entry| entry.runtime.clone())
    }

    #[cfg(test)]
    pub(crate) async fn evict(&self, directory: &str) -> Option<Arc<WorkspaceRuntime>> {
        self.entries.lock().await.remove(&canonical_location(directory)).map(|entry| entry.runtime)
    }

    pub(crate) async fn close(&self) -> Vec<Arc<WorkspaceRuntime>> {
        self.closed.store(true, Ordering::Release);
        let mut entries = self.entries.lock().await;
        for entry in entries.values() {
            entry.runtime.closed.store(true, Ordering::Release);
        }
        let mut runtimes = entries.drain().map(|(_, entry)| entry.runtime).collect::<Vec<_>>();
        drop(entries);
        runtimes.extend(self.failed_shutdowns.lock().await.drain(..));
        runtimes
    }

    pub(crate) async fn retain_failed_shutdown(&self, runtime: Arc<WorkspaceRuntime>) {
        self.failed_shutdowns.lock().await.push(runtime);
    }

    pub(crate) async fn retain_plugin_quarantine(&self, installed: neoism_agent_plugin_api::InstalledPlugins) {
        self.plugin_quarantine.lock().await.push(Arc::new(installed));
    }

    fn retain_generation_quarantine(&self, generation: Arc<PluginGeneration>) {
        self.generation_quarantine.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(GenerationQuarantine { generation, task: None });
    }

    pub(crate) async fn retry_quarantines(&self) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        let generations = std::mem::take(&mut *self.generation_quarantine.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let mut retained_generations = Vec::new();
        let mut errors = Vec::new();
        for mut quarantine in generations {
            if let Some(mut task) = quarantine.task.take() {
                match tokio::time::timeout(PLUGIN_LIFECYCLE_TIMEOUT, &mut task).await {
                    Ok(Ok(())) => continue,
                    Ok(Err(error)) => errors.push(format!("quarantined retirement task failed: {error}")),
                    Err(_) => {
                        errors.push(format!("generation {} quarantined retirement task timed out", quarantine.generation.snapshot.generation));
                        quarantine.task = Some(task);
                        retained_generations.push(quarantine);
                        continue;
                    }
                }
            }
            let mut last_error = None;
            for _ in 0..3 {
                match bounded_plugin_lifecycle("quarantined generation shutdown", quarantine.generation.shutdown()).await {
                    Ok(()) => { last_error = None; break; }
                    Err(error) => last_error = Some(error),
                }
            }
            if let Some(error) = last_error {
                errors.push(error.to_string());
                retained_generations.push(quarantine);
            }
        }
        self.generation_quarantine.lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend(retained_generations);

        let plugins = std::mem::take(&mut *self.plugin_quarantine.lock().await);
        let mut retained_plugins = Vec::new();
        for installed in plugins {
            let mut last_error = None;
            for _ in 0..3 {
                match bounded_plugin_lifecycle("quarantined install shutdown", installed.shutdown()).await {
                    Ok(()) => { last_error = None; break; }
                    Err(error) => last_error = Some(error),
                }
            }
            if let Some(error) = last_error {
                errors.push(error.to_string());
                retained_plugins.push(installed);
            }
        }
        self.plugin_quarantine.lock().await.extend(retained_plugins);
        if errors.is_empty() { Ok(()) } else { Err(neoism_agent_plugin_api::PluginRuntimeError::new(errors.join("; "))) }
    }
}

pub(crate) async fn refresh_plugins(runtime: &WorkspaceRuntime, state: &crate::state::AppState) -> Result<bool, String> {
    if runtime.closed.load(Ordering::Acquire) || state.inner.workspace_runtimes.closed.load(Ordering::Acquire) {
        return Err("workspace runtime is shut down".into());
    }
    let _reload = runtime.reload.lock().await;
    if runtime.closed.load(Ordering::Acquire) || state.inner.workspace_runtimes.closed.load(Ordering::Acquire) {
        return Err("workspace runtime is shut down".into());
    }
    let services = state.services();
    let signature = match config_signature(services, &runtime.root) {
        Ok(signature) => signature,
        Err(error) => {
            tracing::error!(%error, root = %runtime.root.display(), "workspace plugin configuration rejected; retaining last-known-good generation");
            return Err(error);
        }
    };
    {
        let current = runtime.signature.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current == signature {
            return Ok(false);
        }
    }
    match crate::plugins::build_host(state, &runtime.root.to_string_lossy()).await {
        Ok(build) => {
            let mut next = (*build.installed.snapshot()).clone();
            next.generation = runtime.next_generation.fetch_add(1, Ordering::SeqCst);
            // The installed set and the published snapshot must describe the same
            // generation. Host-local generation numbers are replaced by the
            // workspace's monotonic sequence before publication.
            let installed = build.installed.with_snapshot(Arc::new(next));
            let candidate = PluginGeneration::workspace(installed, build.config, build.lifecycle, state, runtime.root.clone());
            publish_candidate_if_open(runtime, state, candidate, signature).await
        }
        Err(error) => {
            tracing::error!(%error, root = %runtime.root.display(), "workspace plugin generation rejected");
            Err(error.to_string())
        }
    }
}

async fn publish_candidate_if_open(
    runtime: &WorkspaceRuntime,
    state: &crate::state::AppState,
    candidate: Arc<PluginGeneration>,
    signature: Vec<u8>,
) -> Result<bool, String> {
    if runtime.closed.load(Ordering::Acquire) || state.inner.workspace_runtimes.closed.load(Ordering::Acquire) {
        let mut cleanup_error = None;
        for _ in 0..3 {
            match bounded_plugin_lifecycle("unpublished candidate cleanup", candidate.shutdown()).await {
                Ok(()) => { cleanup_error = None; break; }
                Err(error) => cleanup_error = Some(error),
            }
        }
        return match cleanup_error {
            Some(error) => {
                state.inner.workspace_runtimes.retain_generation_quarantine(candidate);
                Err(format!("workspace runtime was shut down before plugin publication; candidate cleanup failed: {error}"))
            }
            None => Err("workspace runtime was shut down before plugin publication".to_string()),
        };
    }
    runtime.generation.publish(candidate);
    *runtime.signature.write().unwrap_or_else(std::sync::PoisonError::into_inner) = signature;
    Ok(true)
}

fn config_signature(
    services: &neoism_agent_service_api::AgentServices,
    root: &Path,
) -> Result<Vec<u8>, String> {
    let directory = root.to_string_lossy();
    let (info, _) = neoism_agent_builtins::plugin::config::load(services, &directory)
        .map_err(|error| error.to_string())?;
    let mut signature =
        serde_json::to_vec(&info).map_err(|error| error.to_string())?;
    // A serve plugin's runnable entry appearing on disk (a finished background
    // npm install) must read as a config change: the next acquire then
    // rebuilds the generation with the plugin live instead of Degraded.
    for (id, plugin) in &info.plugins {
        if !plugin.enabled {
            continue;
        }
        if let Some(spec) = crate::plugin_host_process::serve_plugin_spec(
            id,
            &directory,
            &plugin.options,
        ) {
            signature.extend_from_slice(id.as_bytes());
            signature.push(
                crate::plugin_host_process::resolved_serve_entry(&spec).is_some() as u8,
            );
        }
    }
    Ok(signature)
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
    use std::sync::atomic::AtomicUsize;

    fn test_generation(generation: u64) -> Arc<PluginGeneration> {
        let mut snapshot = neoism_agent_plugin_api::RegistrySnapshot::empty();
        snapshot.generation = generation;
        let snapshot = Arc::new(snapshot);
        let installed = Arc::new(neoism_agent_plugin_api::InstalledPlugins::empty(snapshot.clone()));
        PluginGeneration::build(
            snapshot,
            installed,
            Arc::new(neoism_agent_core::AgentConfigDocument::default()),
            Arc::new(WorkspaceLifecycle::default()),
            crate::standard_services(),
            PathBuf::new(),
        )
    }

    #[tokio::test]
    async fn disabled_and_closed_generations_cannot_allocate_plugin_resources() {
        let generation = test_generation(1);
        assert!(generation.state_with_shutdown("dev.neoism.disabled", || 1usize, |_, _| async {}).is_err());
        assert!(generation.lifecycle.states.lock().unwrap().is_empty());
        generation.shutdown().await.unwrap();
        assert!(generation.state_with_shutdown("dev.neoism.disabled", || 2usize, |_, _| async {}).is_err());
        assert!(generation.lifecycle.states.lock().unwrap().is_empty());
    }

    struct RetryManagedInstance { calls: Arc<AtomicUsize> }
    impl neoism_agent_plugin_api::PluginInstance for RetryManagedInstance {
        fn readiness(&self) -> neoism_agent_plugin_api::PluginReadiness { neoism_agent_plugin_api::PluginReadiness::ready() }
        fn contributions(&self) -> neoism_agent_plugin_api::PluginContributions { Default::default() }
        fn shutdown<'a>(&'a self) -> neoism_agent_plugin_api::PluginFuture<'a, ()> {
            Box::pin(async move {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 { return Err(neoism_agent_plugin_api::PluginRuntimeError::new("retry")); }
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn managed_shutdown_failure_keeps_resource_for_retry() {
        let lifecycle = Arc::new(WorkspaceLifecycle::default());
        lifecycle.state_with_shutdown("dev.neoism.retry", || 7usize, |_, _| async {}).unwrap();
        let instance = ManagedPluginInstance {
            inner: Box::new(RetryManagedInstance { calls: Arc::new(AtomicUsize::new(0)) }),
            plugin_id: "dev.neoism.retry".into(),
            lifecycle: lifecycle.clone(),
            root: PathBuf::new(),
            shutdown: AtomicU8::new(MANAGED_OPEN),
        };
        assert!(neoism_agent_plugin_api::PluginInstance::shutdown(&instance).await.is_err());
        assert!(lifecycle.states.lock().unwrap().contains_key("dev.neoism.retry"));
        neoism_agent_plugin_api::PluginInstance::shutdown(&instance).await.unwrap();
        assert!(!lifecycle.states.lock().unwrap().contains_key("dev.neoism.retry"));
    }

    struct PendingManagedInstance { calls: Arc<AtomicUsize> }
    impl neoism_agent_plugin_api::PluginInstance for PendingManagedInstance {
        fn readiness(&self) -> neoism_agent_plugin_api::PluginReadiness { neoism_agent_plugin_api::PluginReadiness::ready() }
        fn contributions(&self) -> neoism_agent_plugin_api::PluginContributions { Default::default() }
        fn shutdown<'a>(&'a self) -> neoism_agent_plugin_api::PluginFuture<'a, ()> {
            Box::pin(async move {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 { std::future::pending::<()>().await; }
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn cancelled_managed_shutdown_resets_attempt_without_removing_resource() {
        let lifecycle = Arc::new(WorkspaceLifecycle::default());
        lifecycle.state_with_shutdown("dev.neoism.pending", || 7usize, |_, _| async {}).unwrap();
        let instance = ManagedPluginInstance {
            inner: Box::new(PendingManagedInstance { calls: Arc::new(AtomicUsize::new(0)) }),
            plugin_id: "dev.neoism.pending".into(), lifecycle: lifecycle.clone(), root: PathBuf::new(), shutdown: AtomicU8::new(MANAGED_OPEN),
        };
        assert!(tokio::time::timeout(Duration::from_millis(1), neoism_agent_plugin_api::PluginInstance::shutdown(&instance)).await.is_err());
        assert!(lifecycle.states.lock().unwrap().contains_key("dev.neoism.pending"));
        neoism_agent_plugin_api::PluginInstance::shutdown(&instance).await.unwrap();
        assert!(!lifecycle.states.lock().unwrap().contains_key("dev.neoism.pending"));
    }

    struct DescriptorCountingFactory(Arc<AtomicUsize>);
    impl neoism_agent_plugin_api::PluginFactory for DescriptorCountingFactory {
        fn descriptor(&self) -> neoism_agent_plugin_api::PluginDescriptor {
            self.0.fetch_add(1, Ordering::SeqCst);
            neoism_agent_plugin_api::PluginDescriptor {
                manifest: neoism_agent_plugin_api::PluginManifest { id: "dev.neoism.cached".into(), name: "cached".into(), version: "1".into(), internal: true, disableable: true, capabilities: Vec::new(), requires: Vec::new(), event_namespaces: Vec::new(), api_prefix: None, config: std::collections::BTreeMap::new() },
                scope: neoism_agent_plugin_api::PluginScope::Workspace,
                required_capabilities: Vec::new(),
                plugin_api_major: neoism_agent_plugin_api::PLUGIN_API_MAJOR,
            }
        }
        fn create<'a>(&'a self, _: neoism_agent_plugin_api::PluginContext) -> neoism_agent_plugin_api::PluginFuture<'a, Box<dyn neoism_agent_plugin_api::PluginInstance>> {
            Box::pin(async { Ok(Box::new(neoism_agent_plugin_api::StaticPluginInstance::new(Default::default())) as Box<dyn neoism_agent_plugin_api::PluginInstance>) })
        }
    }

    #[test]
    fn managed_factory_caches_its_descriptor() {
        let calls = Arc::new(AtomicUsize::new(0));
        let factory = managed_plugin_factory(Box::new(DescriptorCountingFactory(calls.clone())), Arc::new(WorkspaceLifecycle::default()), PathBuf::new());
        let _ = factory.descriptor();
        let _ = factory.descriptor();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn replacement_publishes_complete_generation_atomically() {
        let slot = PluginGenerationSlot::new(test_generation(1));
        let candidate = test_generation(2);

        slot.publish(candidate);
        let current = slot.load();
        assert_eq!(current.snapshot.generation, 2);
        slot.drain_retirements().await.unwrap();
    }

    #[tokio::test]
    async fn replacement_retains_old_generation_for_in_flight_arc_users() {
        let slot = PluginGenerationSlot::new(test_generation(1));
        let retired = slot.load();
        let in_flight = PluginGenerationLease::new(retired.clone());

        slot.publish(PluginGeneration::empty(Arc::new(
            neoism_agent_plugin_api::RegistrySnapshot::empty(),
        )));
        assert_eq!(in_flight.generation, 1);
        assert!(retired.snapshot.is_active());
        drop(in_flight);
        slot.drain_retirements().await.unwrap();
        assert!(!retired.snapshot.is_active());
    }

    #[tokio::test]
    async fn published_snapshot_lease_delays_generation_retirement() {
        let slot = PluginGenerationSlot::new(test_generation(1));
        let lease = PluginGenerationLease::new(slot.load());

        slot.publish(PluginGeneration::empty(Arc::new(
            neoism_agent_plugin_api::RegistrySnapshot::empty(),
        )));
        assert_eq!(lease.generation, 1);
        drop(lease);
        slot.drain_retirements().await.unwrap();
    }

    #[tokio::test]
    async fn refresh_keeps_old_generation_resources_open_until_its_lease_finishes() {
        let root = std::env::temp_dir().join(format!(
            "neoism-refresh-resource-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let lease = runtime.snapshot();
        runtime.generation.publish(test_generation(lease.generation + 1));

        assert!(lease.pty().is_ok());
        assert!(lease.background().is_ok());
        assert!(lease.subagents().is_ok());
        assert!(!*lease.websocket_cancellation().borrow());

        drop(lease);
        runtime.generation.drain_retirements().await.unwrap();
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn convenience_resource_accessor_holds_generation_lease_through_use() {
        let root = std::env::temp_dir().join(format!("neoism-resource-lease-race-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let pty = runtime.pty().unwrap();
        let next = runtime.published_snapshot().generation + 1;
        runtime.generation.publish(test_generation(next));
        tokio::task::yield_now().await;
        assert!(runtime.generation.retirements.lock().unwrap().iter().any(|retirement| !retirement.task.is_finished()));
        assert!(pty.infos.read().await.is_empty());
        drop(pty);
        runtime.generation.drain_retirements().await.unwrap();
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn retirement_gate_rejects_new_leases_and_clones_share_one_count() {
        let generation = test_generation(1);
        let lease = PluginGenerationLease::try_new(generation.clone()).unwrap();
        let clone = lease.clone();
        assert_eq!(generation.lease_count.load(Ordering::Acquire), 1);
        generation.retiring.store(true, Ordering::Release);
        assert!(PluginGenerationLease::try_new(generation.clone()).is_none());
        drop(lease);
        assert_eq!(generation.lease_count.load(Ordering::Acquire), 1);
        drop(clone);
        assert_eq!(generation.lease_count.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn plugin_generations_are_isolated_per_workspace_slot() {
        let first = PluginGenerationSlot::new(test_generation(1));
        let second = PluginGenerationSlot::new(test_generation(1));

        first.publish(PluginGeneration::empty(Arc::new(
            neoism_agent_plugin_api::RegistrySnapshot::empty(),
        )));
        assert_eq!(second.load().snapshot.generation, 1);
        first.drain_retirements().await.unwrap();
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
        let (first, _) = registry.acquire(&root.to_string_lossy(), &state).await.unwrap();
        let (alias, _) = registry
            .acquire(&root.join(".").to_string_lossy(), &state)
            .await.unwrap();
        let (second, _) = registry.acquire(&other.to_string_lossy(), &state).await.unwrap();
        assert!(Arc::ptr_eq(&first, &alias));
        assert!(!Arc::ptr_eq(&first, &second));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn invalid_reload_retains_last_known_good_generation() {
        let root = std::env::temp_dir().join(format!(
            "neoism-workspace-invalid-reload-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        std::fs::create_dir_all(root.join(".agent/skills/review")).unwrap();
        std::fs::write(
            root.join(".agent/skills/review/SKILL.md"),
            "---\nname: Review\ndescription: Review code\n---\nReview the current changes.",
        )
        .unwrap();
        std::fs::write(
            root.join(".agent/agent.json"),
            r#"{"dangerouslySkipPermissions":true,"agent":{"reviewer":{"description":"Reviews code"}}}"#,
        )
        .unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3"))
            .await
            .unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let generation = runtime.snapshot().generation;
        let manifests = runtime.snapshot().manifests.clone();
        assert!(runtime.snapshot().config().dangerously_skip_permissions);
        assert!(crate::plugins::agent_catalog(&runtime.snapshot(), root.to_string_lossy().as_ref())
            .unwrap()
            .get("reviewer")
            .is_some());
        assert!(crate::skill::resolve_from_snapshot(
            &runtime.snapshot(),
            root.to_string_lossy().as_ref(),
            "review",
        )
        .await
        .unwrap()
        .is_some());

        std::fs::write(root.join(".agent/agent.json"), "{").unwrap();
        assert!(refresh_plugins(&runtime, &state).await.is_err());

        assert_eq!(runtime.snapshot().generation, generation);
        assert_eq!(runtime.snapshot().manifests, manifests);
        assert!(runtime.snapshot().config().dangerously_skip_permissions);
        assert!(crate::plugins::agent_catalog(&runtime.snapshot(), root.to_string_lossy().as_ref())
            .unwrap()
            .get("reviewer")
            .is_some());
        assert!(crate::skill::resolve_from_snapshot(
            &runtime.snapshot(),
            root.to_string_lossy().as_ref(),
            "review",
        )
        .await
        .unwrap()
        .is_some());
        drop(runtime);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn concurrent_refreshes_serialize_and_publish_one_monotonic_generation() {
        let root = std::env::temp_dir().join(format!(
            "neoism-workspace-concurrent-refresh-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let before = runtime.published_snapshot().generation;
        std::fs::write(root.join(".agent/agent.json"), r#"{"dangerouslySkipPermissions":true}"#).unwrap();

        let (left, right) = tokio::join!(
            refresh_plugins(&runtime, &state),
            refresh_plugins(&runtime, &state),
        );
        let changed = [left.unwrap(), right.unwrap()];
        assert_eq!(changed.into_iter().filter(|changed| *changed).count(), 1);
        assert_eq!(runtime.published_snapshot().generation, before + 1);
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn generation_lease_prevents_idle_runtime_eviction() {
        let root = std::env::temp_dir().join(format!(
            "neoism-workspace-leased-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3"))
            .await
            .unwrap();
        let registry = WorkspaceRuntimeRegistry::default();
        let (runtime, _) = registry.acquire(&root.to_string_lossy(), &state).await.unwrap();
        let lease = runtime.snapshot();
        let mut next = neoism_agent_plugin_api::RegistrySnapshot::empty();
        next.generation = lease.generation + 1;
        runtime
            .generation
            .publish(PluginGeneration::empty(Arc::new(next)));
        let published_generation = runtime.published_snapshot().generation;
        let routed_generation = scope_generation(lease.clone(), async {
            let routed = runtime.snapshot().generation;
            let published = runtime.published_snapshot();
            state.reconcile_workspace_plugins(&runtime, &published).await;
            routed
        })
        .await;
        assert_eq!(routed_generation, lease.generation);
        assert_eq!(runtime.published_snapshot().generation, published_generation);
        assert_eq!(
            state
                .inner
                .workspace_plugin_generations
                .lock()
                .await
                .get(&canonical_location(root.to_string_lossy().as_ref()))
                .map(|(generation, _)| *generation),
            Some(published_generation),
        );
        drop(runtime);
        registry
            .entries
            .lock()
            .await
            .get_mut(&canonical_location(root.to_string_lossy().as_ref()))
            .unwrap()
            .last_used = Instant::now() - IDLE_TTL - Duration::from_secs(1);

        let (_, evicted) = registry.acquire(&other.to_string_lossy(), &state).await.unwrap();
        assert!(evicted.is_empty());

        drop(lease);
        registry.loaded(root.to_string_lossy().as_ref()).await.unwrap().generation.drain_retirements().await.unwrap();
        let third = other.join("third");
        std::fs::create_dir_all(&third).unwrap();
        let (_, evicted) = registry.acquire(&third.to_string_lossy(), &state).await.unwrap();
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].root, canonical_location(root.to_string_lossy().as_ref()));
        drop(state);
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
        let first = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let second = state.workspace_runtime(other.to_string_lossy().as_ref()).await.unwrap();
        let first_pty = first.pty().unwrap();
        let second_pty = second.pty().unwrap();
        let first_info = crate::pty::create_pty_info(Default::default(), first.root.to_string_lossy().into_owned(), crate::pty::fallback_shell(), crate::now_millis());
        let second_info = crate::pty::create_pty_info(Default::default(), second.root.to_string_lossy().into_owned(), crate::pty::fallback_shell(), crate::now_millis());
        first_pty.infos.write().await.insert(first_info.id.clone(), first_info);
        second_pty.infos.write().await.insert(second_info.id.clone(), second_info);
        drop(first_pty);

        let alias = root.join(".");
        let evicted = state.inner.workspace_runtimes.evict(alias.to_string_lossy().as_ref()).await.unwrap();
        evicted.teardown(&state).await.unwrap();
        state.inner.workspace_plugin_generations.lock().await.remove(&evicted.root);

        assert_eq!(second_pty.infos.read().await.len(), 1);
        assert!(!state.inner.workspace_plugin_generations.lock().await.contains_key(&first.root));
        assert!(state.inner.workspace_plugin_generations.lock().await.contains_key(&second.root));
        drop(second_pty);
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn shutdown_is_terminal_and_waits_for_generation_leases() {
        let root = std::env::temp_dir().join(format!(
            "neoism-workspace-shutdown-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let lease = runtime.snapshot();
        let shutdown_state = state.clone();
        let shutdown = tokio::spawn(async move { shutdown_state.shutdown().await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!shutdown.is_finished());
        assert!(state.inner.workspace_runtimes.acquire(root.to_string_lossy().as_ref(), &state).await.is_err());
        assert!(refresh_plugins(&runtime, &state).await.is_err());
        drop(lease);
        shutdown.await.unwrap().unwrap();
        assert!(state.inner.workspace_runtimes.acquire(root.to_string_lossy().as_ref(), &state).await.is_err());
        assert!(state.workspace_runtime(root.to_string_lossy().as_ref()).await.is_err());
        assert!(crate::agent_tool_registry::acquire_workspace_plugin_snapshot(&state, root.to_string_lossy().as_ref()).await.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    struct LeaseCycle {
        release: StdMutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    #[tokio::test]
    async fn terminal_pre_drain_breaks_a_lease_owning_pty_session_cycle() {
        let root = std::env::temp_dir().join(format!("neoism-pty-lease-cycle-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let lease = runtime.snapshot();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        lease.inner.state_with_shutdown(
            neoism_agent_builtins::plugin::pty::ID,
            || LeaseCycle { release: StdMutex::new(Some(release_tx)) },
            |cycle: Arc<LeaseCycle>, _| async move {
                if let Some(release) = cycle.release.lock().unwrap().take() { let _ = release.send(()); }
            },
        ).unwrap();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let session = tokio::spawn(scope_generation(lease, async move {
            let _ = ready_tx.send(());
            let _ = release_rx.await;
        }));
        ready_rx.await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), runtime.teardown(&state)).await.unwrap().unwrap();
        session.await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn app_teardown_predrains_retired_generation_terminal_lease_cycle() {
        let root = std::env::temp_dir().join(format!("neoism-retired-pty-lease-cycle-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let lease = runtime.snapshot();
        let old_generation = lease.generation;
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        lease.inner.state_with_shutdown(
            neoism_agent_builtins::plugin::pty::ID,
            || LeaseCycle { release: StdMutex::new(Some(release_tx)) },
            |cycle: Arc<LeaseCycle>, _| async move {
                if let Some(release) = cycle.release.lock().unwrap().take() { let _ = release.send(()); }
            },
        ).unwrap();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let session = tokio::spawn(scope_generation(lease, async move {
            let _ = ready_tx.send(());
            let _ = release_rx.await;
        }));
        ready_rx.await.unwrap();
        runtime.generation.publish(test_generation(old_generation + 1));
        tokio::task::yield_now().await;
        assert!(!session.is_finished(), "normal refresh pre-drained the retired generation");

        tokio::time::timeout(Duration::from_secs(2), state.shutdown()).await.unwrap().unwrap();
        session.await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn terminal_teardown_bounds_failed_pre_drain_and_retained_lease_wait() {
        let root = std::env::temp_dir().join(format!("neoism-terminal-drain-timeout-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let lease = runtime.snapshot();
        lease.inner.lifecycle.state_with_shutdown(
            neoism_agent_builtins::plugin::pty::ID,
            || 1usize,
            |_: Arc<usize>, _| async move { std::future::pending::<()>().await },
        ).unwrap();

        let error = tokio::time::timeout(
            Duration::from_secs(1),
            runtime.teardown_with_timeout(Duration::from_millis(10)),
        ).await.unwrap().unwrap_err();
        let message = error.to_string();
        assert!(message.contains("pre-drain"), "{message}");
        assert!(message.contains("lease drain timed out"), "{message}");
        assert!(message.contains("workspace shutdown timed out"), "{message}");
        drop(lease);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn timed_out_retirement_task_transfers_task_and_generation_to_registry_quarantine() {
        let root = std::env::temp_dir().join(format!("neoism-retirement-task-timeout-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let lease = runtime.snapshot();
        runtime.generation.publish(test_generation(lease.generation + 1));

        let error = runtime.teardown_with_timeout(Duration::from_millis(10)).await.unwrap_err();
        assert!(error.to_string().contains("retirement task drain timed out"));
        {
            let quarantine = state.inner.workspace_runtimes.generation_quarantine.lock().unwrap();
            assert_eq!(quarantine.len(), 1);
            assert!(quarantine[0].task.is_some());
        }
        drop(lease);
        state.inner.workspace_runtimes.retry_quarantines().await.unwrap();
        assert!(state.inner.workspace_runtimes.generation_quarantine.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn server_plugin_lifecycle_boundary_contains_panics_and_timeouts() {
        let timed_out = bounded_plugin_lifecycle_with_timeout(
            "test timeout",
            Duration::from_millis(1),
            std::future::pending::<Result<(), neoism_agent_plugin_api::PluginRuntimeError>>(),
        ).await.unwrap_err();
        assert!(timed_out.to_string().contains("timed out"));
        let panicked = bounded_plugin_lifecycle_with_timeout(
            "test panic",
            Duration::from_secs(1),
            async { panic!("plugin panic") },
        ).await.unwrap_err();
        assert!(panicked.to_string().contains("panicked"));
    }

    #[tokio::test]
    async fn close_wins_over_a_candidate_built_before_the_reload_gate_reopens() {
        let root = std::env::temp_dir().join(format!("neoism-close-publication-race-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
        let before = runtime.published_snapshot().generation;
        let candidate = test_generation(before + 1);
        let gate = runtime.reload.lock().await;
        let closing_runtime = runtime.clone();
        let closing_state = state.clone();
        let closing = tokio::spawn(async move { closing_runtime.teardown(&closing_state).await });
        while !runtime.closed.load(Ordering::Acquire) { tokio::task::yield_now().await; }
        assert!(publish_candidate_if_open(&runtime, &state, candidate.clone(), b"late".to_vec()).await.is_err());
        assert!(!candidate.snapshot.is_active());
        assert_eq!(runtime.generation.load().snapshot.generation, before);
        drop(gate);
        closing.await.unwrap().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn completed_retirements_are_reaped_and_do_not_block_idle_eviction() {
        let root = std::env::temp_dir().join(format!("neoism-retirement-reap-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        let other = root.with_extension("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let registry = WorkspaceRuntimeRegistry::default();
        let (runtime, _) = registry.acquire(root.to_string_lossy().as_ref(), &state).await.unwrap();
        runtime.generation.publish(test_generation(runtime.published_snapshot().generation + 1));
        while runtime.generation.retirements.lock().unwrap().iter().any(|retirement| !retirement.task.is_finished()) { tokio::task::yield_now().await; }
        assert!(!runtime.generation.has_leases());
        runtime.generation.drain_retirements().await.unwrap();
        assert!(runtime.generation.retirements.lock().unwrap().is_empty());
        runtime.generation.retired.lock().unwrap().retain(|generation| generation.strong_count() > 0);
        assert!(runtime.generation.retired.lock().unwrap().is_empty());
        registry.entries.lock().await.get_mut(&runtime.root).unwrap().last_used = Instant::now() - IDLE_TTL - Duration::from_secs(1);
        drop(runtime);
        let (_, evicted) = registry.acquire(other.to_string_lossy().as_ref(), &state).await.unwrap();
        assert_eq!(evicted.len(), 1);
        evicted[0].teardown(&state).await.unwrap();
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(other);
    }

    struct RetryRetirementFactory(Arc<AtomicUsize>);
    struct RetryRetirementInstance(Arc<AtomicUsize>);

    impl neoism_agent_plugin_api::PluginFactory for RetryRetirementFactory {
        fn descriptor(&self) -> neoism_agent_plugin_api::PluginDescriptor {
            neoism_agent_plugin_api::PluginDescriptor {
                manifest: neoism_agent_plugin_api::PluginManifest {
                    id: "dev.neoism.retirement-retry".into(), name: "retry".into(), version: "1".into(),
                    internal: true, disableable: true, capabilities: Vec::new(), requires: Vec::new(),
                    event_namespaces: Vec::new(), api_prefix: None, config: std::collections::BTreeMap::new(),
                },
                scope: neoism_agent_plugin_api::PluginScope::Workspace,
                required_capabilities: Vec::new(),
                plugin_api_major: neoism_agent_plugin_api::PLUGIN_API_MAJOR,
            }
        }

        fn create<'a>(&'a self, _: neoism_agent_plugin_api::PluginContext) -> neoism_agent_plugin_api::PluginFuture<'a, Box<dyn neoism_agent_plugin_api::PluginInstance>> {
            let calls = self.0.clone();
            Box::pin(async move { Ok(Box::new(RetryRetirementInstance(calls)) as Box<dyn neoism_agent_plugin_api::PluginInstance>) })
        }
    }

    impl neoism_agent_plugin_api::PluginInstance for RetryRetirementInstance {
        fn readiness(&self) -> neoism_agent_plugin_api::PluginReadiness { neoism_agent_plugin_api::PluginReadiness::ready() }
        fn contributions(&self) -> neoism_agent_plugin_api::PluginContributions { Default::default() }
        fn shutdown<'a>(&'a self) -> neoism_agent_plugin_api::PluginFuture<'a, ()> {
            Box::pin(async move {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(neoism_agent_plugin_api::PluginRuntimeError::new("permanent retirement failure"))
            })
        }
    }

    struct InstallQuarantineFactory(Arc<AtomicUsize>);
    struct InstallQuarantineInstance(Arc<AtomicUsize>);
    impl neoism_agent_plugin_api::PluginFactory for InstallQuarantineFactory {
        fn descriptor(&self) -> neoism_agent_plugin_api::PluginDescriptor {
            let mut descriptor = RetryRetirementFactory(self.0.clone()).descriptor();
            descriptor.manifest.id = "dev.neoism.install-quarantine".into();
            descriptor
        }
        fn create<'a>(&'a self, _: neoism_agent_plugin_api::PluginContext) -> neoism_agent_plugin_api::PluginFuture<'a, Box<dyn neoism_agent_plugin_api::PluginInstance>> {
            let calls = self.0.clone();
            Box::pin(async move { Ok(Box::new(InstallQuarantineInstance(calls)) as Box<dyn neoism_agent_plugin_api::PluginInstance>) })
        }
    }
    impl neoism_agent_plugin_api::PluginInstance for InstallQuarantineInstance {
        fn start<'a>(&'a self) -> neoism_agent_plugin_api::PluginFuture<'a, ()> {
            Box::pin(async { Err(neoism_agent_plugin_api::PluginRuntimeError::new("start failure")) })
        }
        fn readiness(&self) -> neoism_agent_plugin_api::PluginReadiness { neoism_agent_plugin_api::PluginReadiness::ready() }
        fn contributions(&self) -> neoism_agent_plugin_api::PluginContributions { Default::default() }
        fn shutdown<'a>(&'a self) -> neoism_agent_plugin_api::PluginFuture<'a, ()> {
            Box::pin(async move {
                if self.0.fetch_add(1, Ordering::SeqCst) < 3 {
                    Err(neoism_agent_plugin_api::PluginRuntimeError::new("cleanup retry"))
                } else {
                    Ok(())
                }
            })
        }
    }

    #[tokio::test]
    async fn registry_owns_failed_install_cleanup_until_a_later_retry_succeeds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let context = neoism_agent_plugin_api::PluginContext::new(
            neoism_agent_plugin_api::RuntimeScope::Workspace(neoism_agent_plugin_api::WorkspaceIdentity { id: "test".into(), root: PathBuf::from(".") }),
            neoism_agent_plugin_api::CapabilityGrants::default(),
        );
        let failure = match neoism_agent_plugin_api::PluginHost::default()
            .install(vec![Box::new(InstallQuarantineFactory(calls.clone()))], &[], context).await
        {
            Ok(_) => panic!("installation unexpectedly succeeded"),
            Err(failure) => failure,
        };
        let (_, quarantine) = failure.into_parts();
        let registry = WorkspaceRuntimeRegistry::default();
        registry.retain_plugin_quarantine(quarantine.unwrap()).await;
        assert_eq!(registry.plugin_quarantine.lock().await.len(), 1);
        registry.retry_quarantines().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert!(registry.plugin_quarantine.lock().await.is_empty());
    }

    #[tokio::test]
    async fn permanent_failed_retirement_is_quarantined_independently_of_idle_eviction() {
        let root = std::env::temp_dir().join(format!("neoism-retirement-retry-{}", neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
        let other = root.with_extension("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let state = crate::state::AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let registry = WorkspaceRuntimeRegistry::default();
        let (runtime, _) = registry.acquire(root.to_string_lossy().as_ref(), &state).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let context = neoism_agent_plugin_api::PluginContext::new(
            neoism_agent_plugin_api::RuntimeScope::Workspace(neoism_agent_plugin_api::WorkspaceIdentity {
                id: root.to_string_lossy().into_owned(), root: root.clone(),
            }),
            neoism_agent_plugin_api::CapabilityGrants::default(),
        );
        let installed = neoism_agent_plugin_api::PluginHost::default()
            .install(vec![Box::new(RetryRetirementFactory(calls.clone()))], &[], context)
            .await
            .unwrap();
        let failed_generation = PluginGeneration::workspace(
            installed,
            Arc::new(neoism_agent_core::AgentConfigDocument::default()),
            Arc::new(WorkspaceLifecycle::default()),
            &state,
            root.clone(),
        );
        let original = {
            let mut published = runtime.generation.published.write().unwrap();
            std::mem::replace(&mut *published, failed_generation)
        };
        original.shutdown().await.unwrap();
        runtime.generation.publish(test_generation(2));
        while runtime.generation.retirements.lock().unwrap().iter().any(|retirement| !retirement.task.is_finished()) {
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(runtime.generation.quarantine.lock().unwrap().len(), 1);
        assert!(!runtime.generation.has_leases());

        registry.entries.lock().await.get_mut(&runtime.root).unwrap().last_used = Instant::now() - IDLE_TTL - Duration::from_secs(1);
        drop(runtime);
        let (_, evicted) = registry.acquire(other.to_string_lossy().as_ref(), &state).await.unwrap();
        assert_eq!(evicted.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(registry.loaded(root.to_string_lossy().as_ref()).await.is_none());
        assert!(registry.retry_quarantines().await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 6);
        assert_eq!(registry.generation_quarantine.lock().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(other);
    }
}
