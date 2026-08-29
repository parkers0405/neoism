use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context;
use neoism_agent_core::{
    ArtifactInfo, AuditEntry, EventPayload, Id, IdKind, MessageInfo, MessageWithParts,
    PermissionRequestInfo, PermissionRule, PromptRequest, QuestionRequestInfo,
    SessionInfo, SessionStatus, TodoInfo,
};
use neoism_agent_service_api::AgentServices;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify, RwLock};
use turso::Value as SqlValue;

pub(crate) const EVENT_BROADCAST_CAPACITY: usize = 16_384;

#[derive(Clone)]
pub struct AppState {
    pub(crate) inner: Arc<InnerState>,
}

pub(crate) struct InnerState {
    pub(crate) services: AgentServices,
    pub(crate) store: SessionStore,
    pub(crate) artifact_root: PathBuf,
    pub(crate) provider_service: Arc<dyn neoism_agent_plugin_api::ProviderService>,
    pub(crate) caller_policy: crate::caller::CallerPolicy,
    pub(crate) utilities: Arc<crate::utility_runtime::UtilityRuntime>,
    pub(crate) workspace_runtimes: crate::workspace_runtime::WorkspaceRuntimeRegistry,
    pub(crate) workspace_plugin_generations:
        Mutex<HashMap<PathBuf, (u64, BTreeSet<String>)>>,
    pub(crate) statuses: RwLock<HashMap<String, SessionStatus>>,
    pub(crate) session_coordinator: crate::session_coordinator::SessionCoordinator,
    /// Keyed completion-state mutation locks. The map lock is held only long
    /// enough to clone a child's lock; callers never await storage while
    /// holding it and never acquire the same child lock recursively.
    pub(crate) subtask_completion_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Serializes each parent's list/check/enqueue reconciliation so sibling
    /// completions cannot race through queue dedupe.
    pub(crate) subtask_parent_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(crate) execution_activity_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(crate) execution_owner_id: String,
    execution_lease_control: Arc<ExecutionLeaseControl>,
    execution_lease_handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(crate) permissions: RwLock<HashMap<String, PermissionRequestInfo>>,
    pub(crate) permission_waiters: RwLock<HashMap<String, PermissionPending>>,
    pub(crate) permission_approvals: RwLock<HashMap<String, Vec<PermissionRule>>>,
    pub(crate) questions: RwLock<HashMap<String, QuestionRequestInfo>>,
    pub(crate) question_waiters: RwLock<HashMap<String, QuestionPending>>,
    pub(crate) todos: RwLock<HashMap<String, Vec<TodoInfo>>>,
    pub(crate) workflow_workspaces: RwLock<BTreeSet<String>>,
    pub(crate) workflow_reconcile: Mutex<()>,
    pub(crate) workflow_notify: Arc<Notify>,
    pub(crate) workflow_scheduler_started: AtomicBool,
    events: broadcast::Sender<EventPayload>,
    event_writer: mpsc::UnboundedSender<EventPayload>,
    /// Wire-monotone sequence allocator for broadcast events, seeded from
    /// the durable log's high-water mark so live and replayed events share
    /// one cursor space.
    event_sequence: std::sync::atomic::AtomicU64,
    /// Serializes stamp+broadcast so subscribers observe sequences in
    /// broadcast order.
    event_order: std::sync::Mutex<()>,
}

struct ExecutionLeaseControl {
    stopping: AtomicBool,
    notify: Notify,
}

impl Drop for InnerState {
    fn drop(&mut self) {
        if let Ok(handle) = self.execution_lease_handle.get_mut() {
            if let Some(handle) = handle.take() {
                handle.abort();
            }
        }
    }
}

pub(crate) struct PermissionPending {
    pub(crate) request: PermissionRequestInfo,
    pub(crate) sender: oneshot::Sender<Result<Vec<PermissionRule>, String>>,
}

pub(crate) struct QuestionPending {
    pub(crate) sender: oneshot::Sender<Result<Vec<Vec<String>>, String>>,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionRun {
    pub(crate) id: String,
    pub(crate) started_at: u64,
    pub(crate) cancel: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionListPage {
    pub(crate) items: Vec<SessionInfo>,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionListCursor {
    updated: u64,
    id: String,
}

impl SessionListCursor {
    pub(crate) fn decode(value: &str) -> anyhow::Result<Self> {
        let value = value
            .strip_prefix("v1:")
            .context("unsupported session cursor version")?;
        let (updated, id) = value
            .split_once(':')
            .context("invalid session cursor")?;
        anyhow::ensure!(!id.is_empty(), "invalid session cursor");
        Ok(Self {
            updated: updated.parse().context("invalid session cursor timestamp")?,
            id: id.to_string(),
        })
    }

    fn encode(updated: u64, id: &str) -> String {
        format!("v1:{updated}:{id}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExecutionSubtaskRegistration {
    Inserted,
    AlreadyPresent,
    Rejected,
}

#[derive(Clone)]
pub(crate) struct SessionStore {
    db: Db,
}

/// Turso database access shared by the store. Reads may run concurrently,
/// while writes pass through one process-local gate so ordinary in-process
    /// contention does not consume the store's bounded busy retry budget.
#[derive(Clone)]
struct Db {
    database: turso::Database,
    write_gate: Arc<Mutex<()>>,
}

/// A database row represented as column names plus libSQL values.
struct DbRow {
    columns: Arc<Vec<String>>,
    values: Vec<SqlValue>,
}

impl DbRow {
    fn index(&self, name: &str) -> anyhow::Result<usize> {
        self.columns
            .iter()
            .position(|column| column == name)
            .with_context(|| format!("row has no column named `{name}`"))
    }

    fn value(&self, name: &str) -> anyhow::Result<&SqlValue> {
        let index = self.index(name)?;
        self.values
            .get(index)
            .with_context(|| format!("row has no value for column `{name}`"))
    }

    fn get_str(&self, name: &str) -> anyhow::Result<String> {
        match self.value(name)? {
            SqlValue::Text(value) => Ok(value.clone()),
            other => anyhow::bail!("column `{name}` is not text: {other:?}"),
        }
    }

    fn get_opt_str(&self, name: &str) -> anyhow::Result<Option<String>> {
        match self.value(name)? {
            SqlValue::Text(value) => Ok(Some(value.clone())),
            SqlValue::Null => Ok(None),
            other => anyhow::bail!("column `{name}` is not text or null: {other:?}"),
        }
    }

    fn get_i64(&self, name: &str) -> anyhow::Result<i64> {
        match self.value(name)? {
            SqlValue::Integer(value) => Ok(*value),
            other => anyhow::bail!("column `{name}` is not an integer: {other:?}"),
        }
    }

    fn get_opt_i64(&self, name: &str) -> anyhow::Result<Option<i64>> {
        match self.value(name)? {
            SqlValue::Integer(value) => Ok(Some(*value)),
            SqlValue::Null => Ok(None),
            other => {
                anyhow::bail!("column `{name}` is not an integer or null: {other:?}")
            }
        }
    }

    fn get_f64(&self, name: &str) -> anyhow::Result<f64> {
        match self.value(name)? {
            SqlValue::Real(value) => Ok(*value),
            SqlValue::Integer(value) => Ok(*value as f64),
            other => anyhow::bail!("column `{name}` is not numeric: {other:?}"),
        }
    }

    fn i64_at(&self, index: usize) -> anyhow::Result<i64> {
        match self.values.get(index) {
            Some(SqlValue::Integer(value)) => Ok(*value),
            Some(other) => anyhow::bail!("column {index} is not an integer: {other:?}"),
            None => anyhow::bail!("row has no column {index}"),
        }
    }
}

fn text(value: impl Into<String>) -> SqlValue {
    SqlValue::Text(value.into())
}

fn opt_text(value: Option<String>) -> SqlValue {
    value.map(SqlValue::Text).unwrap_or(SqlValue::Null)
}

fn int(value: i64) -> SqlValue {
    SqlValue::Integer(value)
}

fn session_list_index_statement(info: &SessionInfo) -> anyhow::Result<(String, Vec<SqlValue>)> {
    let mut extra = BTreeMap::new();
    for key in [crate::caller::TENANT_EXTRA_KEY, "pinned"] {
        if let Some(value) = info.extra.get(key) {
            extra.insert(key.to_string(), value.clone());
        }
    }
    let summary = SessionInfo {
        id: info.id.clone(),
        slug: info.slug.clone(),
        project_id: info.project_id.clone(),
        workspace_id: info.workspace_id.clone(),
        directory: info.directory.clone(),
        path: info.path.clone(),
        parent_id: info.parent_id.clone(),
        title: info.title.clone(),
        agent: info.agent.clone(),
        model: info.model.clone(),
        version: info.version.clone(),
        time: info.time.clone(),
        permission: None,
        extra,
    };
    let summary_json = serde_json::to_string(&summary)?;
    Ok((
        r#"INSERT INTO session_list_index
           (session_id, directory, path, parent_id, title, updated, archived, tenant_id, workspace_id, summary_json)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(session_id) DO UPDATE SET
             directory = excluded.directory,
             path = excluded.path,
             parent_id = excluded.parent_id,
             title = excluded.title,
             updated = excluded.updated,
             archived = excluded.archived,
             tenant_id = excluded.tenant_id,
             workspace_id = excluded.workspace_id,
             summary_json = excluded.summary_json
           WHERE excluded.updated >= session_list_index.updated"#
            .to_string(),
        vec![
            text(info.id.to_string()),
            text(&info.directory),
            opt_text(info.path.clone()),
            opt_text(info.parent_id.as_ref().map(ToString::to_string)),
            text(&info.title),
            int(store_i64(info.time.updated)),
            info.time.archived.map(int).unwrap_or(SqlValue::Null),
            text(crate::caller::session_tenant(info)),
            opt_text(info.workspace_id.as_ref().map(ToString::to_string)),
            text(summary_json),
        ],
    ))
}

fn session_list_info_from_row(row: &DbRow) -> anyhow::Result<SessionInfo> {
    if let Some(summary) = row.get_opt_str("summary_json")?.filter(|value| !value.is_empty()) {
        return decode_json(summary);
    }
    // Legacy sidecars predate compact summary_json. Reconstruct the list
    // contract from indexed columns rather than reopening multi-megabyte
    // canonical conversation blobs on the latency-sensitive sidebar path.
    let session_id = row.get_str("session_id")?;
    let updated = row.get_i64("updated")?.max(0) as u64;
    let mut value = json!({
        "id": session_id,
        "slug": session_id,
        "projectId": "global",
        "directory": row.get_str("directory")?,
        "title": row.get_str("title")?,
        "version": env!("CARGO_PKG_VERSION"),
        "time": {
            "created": updated,
            "updated": updated,
            "archived": row.get_opt_i64("archived")?,
        },
        crate::caller::TENANT_EXTRA_KEY: row.get_str("tenant_id")?,
    });
    let object = value
        .as_object_mut()
        .expect("session list fallback is an object");
    for (key, value) in [
        ("path", row.get_opt_str("path")?),
        ("parentId", row.get_opt_str("parent_id")?),
        ("workspaceId", row.get_opt_str("workspace_id")?),
    ] {
        if let Some(value) = value {
            object.insert(key.to_string(), Value::String(value));
        }
    }
    decode_json(value.to_string())
}

fn event_statements(
    event: &EventPayload,
    aggregate_id: &str,
    session_id: Option<String>,
    owner_id: Option<&str>,
) -> anyhow::Result<Vec<(String, Vec<SqlValue>)>> {
    Ok(vec![
        (
            r#"
            INSERT INTO event_sequences (aggregate_id, seq, owner_id)
            VALUES (?, 0, ?)
            ON CONFLICT(aggregate_id) DO UPDATE SET
                seq = event_sequences.seq + 1,
                owner_id = COALESCE(event_sequences.owner_id, excluded.owner_id)
            "#
            .to_string(),
            vec![
                text(aggregate_id),
                opt_text(owner_id.map(ToOwned::to_owned)),
            ],
        ),
        (
            "INSERT INTO events (seq, event_id, kind, aggregate_id, aggregate_seq, owner_id, session_id, event_json, created) VALUES (?, ?, ?, ?, (SELECT seq FROM event_sequences WHERE aggregate_id = ?), ?, ?, ?, ?)".to_string(),
            vec![
                event.sequence.map(|seq| int(seq as i64)).unwrap_or(SqlValue::Null),
                text(event.id.to_string()),
                text(event.kind.clone()),
                text(aggregate_id),
                text(aggregate_id),
                opt_text(owner_id.map(ToOwned::to_owned)),
                opt_text(session_id),
                text(serde_json::to_string(event)?),
                int(store_i64(crate::now_millis())),
            ],
        ),
    ])
}

/// Turso returns `Busy` immediately, so concurrent writers need a bounded retry.
async fn turso_busy_retry<T, F, Fut>(mut op: F) -> Result<T, turso::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, turso::Error>>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut delay = std::time::Duration::from_millis(2);
    loop {
        match op().await {
            Err(error)
                if turso_error_is_busy(&error)
                    && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(std::time::Duration::from_millis(100));
            }
            other => return other,
        }
    }
}

fn turso_error_is_busy(error: &turso::Error) -> bool {
    matches!(error, turso::Error::Busy(_) | turso::Error::BusySnapshot(_)) || {
        let message = error.to_string().to_ascii_lowercase();
        message.contains("locked") || message.contains("busy")
    }
}

impl Db {
    async fn lock_writer(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.write_gate.clone().lock_owned().await
    }

    async fn execute(&self, sql: &str, params: Vec<SqlValue>) -> anyhow::Result<u64> {
        let _writer = self.lock_writer().await;
        Ok(turso_busy_retry(|| {
            let params = params.clone();
            async move {
                let conn = self.database.connect()?;
                conn.execute(sql, params).await
            }
        })
        .await?)
    }

    async fn fetch_all(
        &self,
        sql: &str,
        params: Vec<SqlValue>,
    ) -> anyhow::Result<Vec<DbRow>> {
        Ok(turso_busy_retry(|| {
            let params = params.clone();
            async move {
                let conn = self.database.connect()?;
                let mut rows = conn.query(sql, params).await?;
                let columns = Arc::new(rows.column_names());
                let mut out = Vec::new();
                while let Some(row) = rows.next().await? {
                    let values = (0..row.column_count())
                        .map(|index| row.get_value(index))
                        .collect::<Result<Vec<_>, _>>()?;
                    out.push(DbRow {
                        columns: columns.clone(),
                        values,
                    });
                }
                Ok(out)
            }
        })
        .await?)
    }

    async fn fetch_optional(
        &self,
        sql: &str,
        params: Vec<SqlValue>,
    ) -> anyhow::Result<Option<DbRow>> {
        Ok(self.fetch_all(sql, params).await?.into_iter().next())
    }

    async fn fetch_scalar_i64(
        &self,
        sql: &str,
        params: Vec<SqlValue>,
    ) -> anyhow::Result<i64> {
        self.fetch_optional(sql, params)
            .await?
            .context("query returned no rows")?
            .i64_at(0)
    }

    async fn execute_transaction_with_results(
        &self,
        statements: Vec<(String, Vec<SqlValue>)>,
    ) -> anyhow::Result<Vec<u64>> {
        let _writer = self.lock_writer().await;
        let results = turso_busy_retry(|| {
            let statements = statements.clone();
            async move {
                let mut connection = self.database.connect()?;
                let transaction = connection
                    .transaction_with_behavior(
                        turso::transaction::TransactionBehavior::Immediate,
                    )
                    .await?;
                let mut results = Vec::with_capacity(statements.len());
                for (sql, params) in statements {
                    results.push(transaction.execute(&sql, params).await?);
                }
                transaction.commit().await?;
                Ok(results)
            }
        })
        .await?;
        Ok(results)
    }

    async fn execute_transaction(
        &self,
        statements: Vec<(String, Vec<SqlValue>)>,
    ) -> anyhow::Result<()> {
        self.execute_transaction_with_results(statements).await?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PersistedEvent {
    pub(crate) seq: i64,
    pub(crate) created: i64,
    pub(crate) payload: EventPayload,
}

impl AppState {
    #[cfg(test)]
    pub(crate) fn execution_lease_running(&self) -> bool {
        self.inner
            .execution_lease_handle
            .lock()
            .expect("execution lease handle poisoned")
            .is_some()
    }
    pub async fn open_default(services: AgentServices) -> anyhow::Result<Self> {
        let store = SessionStore::open_default().await?;
        let artifact_root = PathBuf::from(crate::default_state_dir()).join("artifacts");
        Self::from_store(store, artifact_root, services).await
    }

    pub async fn open_database_with_services(
        path: impl Into<PathBuf>,
        services: AgentServices,
    ) -> anyhow::Result<Self> {
        let path = path.into();
        let artifact_root = path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("artifacts");
        let store = SessionStore::open(path).await?;
        Self::from_store(store, artifact_root, services).await
    }

    #[cfg(test)]
    pub(crate) async fn open_database(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        Self::open_database_with_services(path, crate::standard_services()).await
    }

    async fn from_store(
        store: SessionStore,
        artifact_root: PathBuf,
        services: AgentServices,
    ) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&artifact_root).await?;
        // Single ordered bus for every event (live deltas + committed edges).
        // Capacity must absorb a full streaming burst per subscriber. A
        // subscriber that exceeds it is disconnected so reconnect recovery
        // can reconcile the lost transient deltas instead of silently
        // continuing with a permanently incomplete timeline.
        let (events, _) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
        let (event_writer, mut event_reader) = mpsc::unbounded_channel();
        let provider_service: Arc<dyn neoism_agent_plugin_api::ProviderService> =
            Arc::new(neoism_agent_builtins::ProviderPlatform::from_env());
        let caller_policy = crate::caller::CallerPolicy::from_env();
        let utilities = crate::utility_runtime::UtilityRuntime::new(&services);
        let permission_approvals = store.list_permission_approvals().await?;
        let durable_permissions = store.list_pending_permissions().await?;
        let durable_questions = store.list_pending_questions().await?;
        store.interrupt_stale_runs().await?;
        let event_sequence_seed = store.latest_event_sequence().await?;
        let execution_owner_id = Id::ascending(IdKind::Event).to_string();
        let execution_lease_control = Arc::new(ExecutionLeaseControl {
            stopping: AtomicBool::new(false),
            notify: Notify::new(),
        });
        store
            .heartbeat_execution_owner(&execution_owner_id, crate::now_millis())
            .await?;
        let state = Self {
            inner: Arc::new(InnerState {
                services,
                store,
                artifact_root,
                provider_service,
                caller_policy,
                utilities,
                workspace_runtimes: Default::default(),
                workspace_plugin_generations: Mutex::new(HashMap::new()),
                statuses: RwLock::new(HashMap::new()),
                session_coordinator: Default::default(),
                subtask_completion_locks: Mutex::new(HashMap::new()),
                subtask_parent_locks: Mutex::new(HashMap::new()),
                execution_activity_locks: Mutex::new(HashMap::new()),
                execution_owner_id,
                execution_lease_control: execution_lease_control.clone(),
                execution_lease_handle: std::sync::Mutex::new(None),
                permissions: RwLock::new(
                    durable_permissions
                        .into_iter()
                        .map(|request| (request.id.clone(), request))
                        .collect(),
                ),
                permission_waiters: RwLock::new(HashMap::new()),
                permission_approvals: RwLock::new(permission_approvals),
                questions: RwLock::new(
                    durable_questions
                        .into_iter()
                        .map(|request| (request.id.clone(), request))
                        .collect(),
                ),
                question_waiters: RwLock::new(HashMap::new()),
                todos: RwLock::new(HashMap::new()),
                workflow_workspaces: RwLock::new(BTreeSet::new()),
                workflow_reconcile: Mutex::new(()),
                workflow_notify: Arc::new(Notify::new()),
                workflow_scheduler_started: AtomicBool::new(false),
                events,
                event_writer,
                event_sequence: std::sync::atomic::AtomicU64::new(event_sequence_seed),
                event_order: std::sync::Mutex::new(()),
            }),
        };
        let weak_state = Arc::downgrade(&state.inner);
        let lease_control = execution_lease_control;
        let lease_handle = tokio::spawn(async move {
            loop {
                if lease_control.stopping.load(Ordering::SeqCst) {
                    break;
                }
                let Some(inner) = weak_state.upgrade() else { break; };
                let now = crate::now_millis();
                let store = inner.store.clone();
                let owner_id = inner.execution_owner_id.clone();
                drop(inner);
                if let Err(error) = store
                    .heartbeat_execution_owner(&owner_id, now)
                    .await
                {
                    tracing::warn!(%error, "failed to heartbeat execution activity owner");
                }
                if let Err(error) = store
                    .reconcile_stale_execution_segments(now.saturating_sub(15_000))
                    .await
                {
                    tracing::warn!(%error, "failed to reconcile stale execution activity segments");
                }
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {},
                    _ = lease_control.notify.notified() => {},
                }
            }
        });
        *state.inner.execution_lease_handle.lock().expect("execution lease handle poisoned") = Some(lease_handle);
        let durable_store = state.inner.store.clone();
        let durable_state = state.clone();
        tokio::spawn(async move {
            // Persistence only. Delivery happens synchronously in `publish` so
            // subscribers receive every event in publish order — a committed
            // part snapshot must never lag behind live deltas published after
            // it (out-of-order reasoning/tool rows in streaming timelines).
            while let Some(event) = event_reader.recv().await {
                match durable_store.append_event(&event).await {
                    Ok(()) => {
                        for runtime in durable_state.inner.workspace_runtimes.runtimes().await {
                            crate::plugin::publish_event(&runtime.snapshot(), &event);
                        }
                    }
                    Err(error) => {
                        tracing::error!(event = %event.kind, %error, "failed to durably commit event");
                    }
                }
            }
        });
        crate::session_queue::resume_prompt_queues(state.clone()).await?;
        crate::session_actions::resume_pending_subtask_completions(&state).await;
        Ok(state)
    }

    pub fn services(&self) -> &AgentServices {
        &self.inner.services
    }

    pub(crate) fn start_session_list_backfill(&self) {
        let store = self.inner.store.clone();
        tokio::spawn(async move {
            // Yield the launch path completely: the HTTP serve loop and first
            // interactive requests get priority over legacy index hydration.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            loop {
                match store.backfill_session_list_index().await {
                    Ok(()) => break,
                    Err(error) => {
                        tracing::warn!(%error, "session list index backfill retrying");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }

    pub(crate) async fn plugin_snapshot(
        &self,
        directory: &str,
    ) -> crate::workspace_runtime::PluginGenerationLease {
        if let Some(generation) = crate::workspace_runtime::active_generation(directory) {
            return generation;
        }
        self.try_workspace_runtime(directory).await.map_or_else(
            |_| crate::workspace_runtime::closed_snapshot(),
            |runtime| runtime.snapshot(),
        )
    }

    pub(crate) async fn refreshed_plugin_snapshot(
        &self,
        directory: &str,
    ) -> crate::workspace_runtime::PluginGenerationLease {
        if let Some(generation) = crate::workspace_runtime::active_generation(directory) {
            return generation;
        }
        let runtime = match self.try_workspace_runtime(directory).await {
            Ok(runtime) => runtime,
            Err(_) => return crate::workspace_runtime::closed_snapshot(),
        };
        let _ = crate::workspace_runtime::refresh_plugins(&runtime, self).await;
        let snapshot = runtime.published_snapshot();
        self.reconcile_workspace_plugins(&runtime, &snapshot).await;
        snapshot
    }

    pub(crate) async fn workspace_runtime(&self, directory: &str) -> Result<Arc<crate::workspace_runtime::WorkspaceRuntime>, String> {
        self.try_workspace_runtime(directory).await
    }

    pub(crate) async fn try_workspace_runtime(&self, directory: &str) -> Result<Arc<crate::workspace_runtime::WorkspaceRuntime>, String> {
        let (runtime, evicted) = self.inner
            .workspace_runtimes
            .acquire(directory, self)
            .await?;
        for stale in evicted {
            self.inner.workspace_plugin_generations.lock().await.remove(&stale.root);
        }
        self.reconcile_semantic_service().await;
        let snapshot = runtime.published_snapshot();
        self.reconcile_workspace_plugins(&runtime, &snapshot).await;
        Ok(runtime)
    }

    pub(crate) async fn reconcile_workspace_plugins(
        &self,
        runtime: &Arc<crate::workspace_runtime::WorkspaceRuntime>,
        snapshot: &crate::workspace_runtime::PluginGenerationLease,
    ) {
        let enabled = snapshot
            .manifests
            .iter()
            .map(|manifest| manifest.id.clone())
            .collect::<BTreeSet<_>>();
        let mut generations = self.inner.workspace_plugin_generations.lock().await;
        if runtime.published_snapshot().generation != snapshot.generation {
            return;
        }
        if generations
            .get(&runtime.root)
            .is_some_and(|(generation, _)| *generation == snapshot.generation)
        {
            return;
        }
        generations.insert(runtime.root.clone(), (snapshot.generation, enabled.clone()));
        let workflow = enabled.contains(neoism_agent_builtins::plugin::workflows::ID);
        snapshot.set_workflow_enabled(workflow, self.clone());
        if workflow {
            crate::workflow::workspace_enabled(self, runtime.root.clone()).await;
        } else {
            crate::workflow::workspace_disabled(self, &runtime.root).await;
        }
        if enabled.contains(neoism_agent_builtins::plugin::semantic::ID) {
            snapshot.enable_semantic(self.clone()).await;
            if let Some(memory) = self.inner.services.memory.as_ref() {
                memory.set_semantic_index(Some(Arc::new(crate::semantic::AgentSemanticMemoryIndex::new(
                    self.inner.store.clone(), snapshot.semantic_client(),
                ))));
            }
        } else {
            let semantic_enabled = generations.values().any(|(_, plugins)| {
                plugins.contains(neoism_agent_builtins::plugin::semantic::ID)
            });
            if !semantic_enabled {
                if let Some(memory) = self.inner.services.memory.as_ref() {
                    memory.set_semantic_index(None);
                }
            }
        }
    }

    async fn reconcile_semantic_service(&self) {
        let enabled = self.inner.workspace_plugin_generations.lock().await.values()
            .any(|(_, plugins)| plugins.contains(neoism_agent_builtins::plugin::semantic::ID));
        if !enabled {
            if let Some(memory) = self.inner.services.memory.as_ref() {
                memory.set_semantic_index(None);
            }
        }
    }

    /// Stop state-owned subprocesses and background transport tasks.
    pub async fn shutdown(&self) -> Result<(), neoism_agent_plugin_api::PluginRuntimeError> {
        self.inner.execution_lease_control.stopping.store(true, Ordering::SeqCst);
        self.inner.execution_lease_control.notify.notify_waiters();
        let lease_handle = self
            .inner
            .execution_lease_handle
            .lock()
            .expect("execution lease handle poisoned")
            .take();
        if let Some(handle) = lease_handle {
            let _ = handle.await;
        }
        let mut errors = Vec::new();
        for runtime in self.inner.workspace_runtimes.close().await {
            if let Err(error) = runtime.teardown(self).await {
                tracing::error!(%error, root = %runtime.root.display(), "workspace runtime shutdown failed");
                errors.push(format!("{}: {error}", runtime.root.display()));
                self.inner.workspace_runtimes.retain_failed_shutdown(runtime).await;
            }
        }
        if let Err(error) = self.inner.workspace_runtimes.retry_quarantines().await {
            tracing::error!(%error, "plugin cleanup quarantine still contains live ownership");
            errors.push(format!("plugin cleanup quarantine: {error}"));
        }
        self.inner.workspace_plugin_generations.lock().await.clear();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(neoism_agent_plugin_api::PluginRuntimeError::new(errors.join("; ")))
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventPayload> {
        self.inner.events.subscribe()
    }

    /// Stamp the wire sequence and broadcast, atomically with respect to
    /// other broadcasts so subscribers see sequences in send order. Events
    /// that were already stamped (transactional commits allocate before the
    /// write) keep their sequence.
    fn stamp_and_broadcast(&self, event: &mut EventPayload) {
        let _order = self
            .inner
            .event_order
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if event.sequence.is_none() {
            event.sequence = Some(
                self.inner
                    .event_sequence
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1,
            );
        }
        let _ = self.inner.events.send(event.clone());
    }

    /// Allocate a wire sequence without broadcasting — for events persisted
    /// transactionally before their broadcast.
    pub(crate) fn allocate_event_sequence(&self, event: &mut EventPayload) {
        if event.sequence.is_none() {
            event.sequence = Some(
                self.inner
                    .event_sequence
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1,
            );
        }
    }

    pub(crate) fn publish(&self, event: EventPayload) {
        // Opencode-model bus: broadcast in PUBLISH order, persist alongside.
        // Routing delivery through the durable writer made committed events
        // (reasoning/tool part snapshots) lag behind live token deltas
        // published after them, so subscribers saw parts out of order.
        let mut event = event;
        self.stamp_and_broadcast(&mut event);
        if let Err(error) = self.inner.event_writer.send(event) {
            tracing::error!(event = %error.0.kind, "durable event writer stopped");
        }
    }

    /// Broadcast transient stream progress without appending it to the durable
    /// event log. The complete message is persisted at semantic boundaries;
    /// writing every provider delta here stalls the provider and amplifies the
    /// growing message into thousands of database writes.
    pub(crate) fn publish_live(&self, event: EventPayload) {
        let mut event = event;
        self.stamp_and_broadcast(&mut event);
    }

    #[cfg(test)]
    pub(crate) async fn publish_persisted(
        &self,
        event: EventPayload,
    ) -> anyhow::Result<()> {
        let mut event = event;
        self.allocate_event_sequence(&mut event);
        self.inner.store.append_event(&event).await?;
        for runtime in self.inner.workspace_runtimes.runtimes().await {
            crate::plugin::publish_event(&runtime.snapshot(), &event);
        }
        let _ = self.inner.events.send(event);
        Ok(())
    }

    pub(crate) fn publish_committed(&self, event: EventPayload) {
        let state = self.clone();
        let hook_event = event.clone();
        tokio::spawn(async move {
            for runtime in state.inner.workspace_runtimes.runtimes().await {
                crate::plugin::publish_event(&runtime.snapshot(), &hook_event);
            }
        });
        let mut event = event;
        self.stamp_and_broadcast(&mut event);
    }

    pub(crate) async fn update_session_with_event(
        &self,
        info: &SessionInfo,
        event: EventPayload,
    ) -> anyhow::Result<()> {
        let mut event = event;
        self.allocate_event_sequence(&mut event);
        self.inner
            .store
            .commit_projection_event(
                vec![
                    (
                        "UPDATE sessions SET info_json = ?, updated = ? WHERE id = ?"
                            .to_string(),
                        vec![
                            text(serde_json::to_string(info)?),
                            int(store_i64(info.time.updated)),
                            text(info.id.to_string()),
                        ],
                    ),
                    session_list_index_statement(info)?,
                ],
                &event,
                None,
            )
            .await?;
        self.publish_committed(event);
        Ok(())
    }

    pub(crate) async fn append_message_with_event(
        &self,
        session_id: &str,
        message: &MessageWithParts,
        event: EventPayload,
    ) -> anyhow::Result<()> {
        let mut event = event;
        self.allocate_event_sequence(&mut event);
        self.inner
            .store
            .commit_projection_event(
                vec![(
                    "INSERT INTO messages (id, session_id, message_json, created, position) VALUES (?, ?, ?, ?, (SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE session_id = ?))".to_string(),
                    vec![
                        text(message_id(message)),
                        text(session_id),
                        text(serde_json::to_string(message)?),
                        int(store_i64(message_created(message))),
                        text(session_id),
                    ],
                )],
                &event,
                None,
            )
            .await?;
        self.publish_committed(event);
        Ok(())
    }

    pub(crate) async fn update_message_with_event(
        &self,
        session_id: &str,
        message: &MessageWithParts,
        event: EventPayload,
    ) -> anyhow::Result<()> {
        let mut event = event;
        self.allocate_event_sequence(&mut event);
        self.inner
            .store
            .commit_projection_event(
                vec![
                    (
                        "UPDATE messages SET message_json = ? WHERE session_id = ? AND id = ?"
                            .to_string(),
                        vec![
                            text(serde_json::to_string(message)?),
                            text(session_id),
                            text(message_id(message)),
                        ],
                    ),
                    (
                        "DELETE FROM message_embeddings WHERE message_id = ?".to_string(),
                        vec![text(message_id(message))],
                    ),
                ],
                &event,
                None,
            )
            .await?;
        self.publish_committed(event);
        Ok(())
    }

    pub(crate) async fn put_context_epoch_with_event(
        &self,
        session_id: &str,
        epoch: &crate::context_epoch::ContextEpoch,
        event: EventPayload,
    ) -> anyhow::Result<()> {
        let mut event = event;
        self.allocate_event_sequence(&mut event);
        self.inner
            .store
            .commit_projection_event(
                vec![(
                    r#"
                    INSERT INTO session_context_epochs
                        (session_id, baseline_json, snapshot_json, generation, baseline_seq, updated)
                    VALUES (?, ?, ?, ?, ?, ?)
                    ON CONFLICT(session_id) DO UPDATE SET
                        snapshot_json = excluded.snapshot_json,
                        generation = excluded.generation,
                        updated = excluded.updated
                    "#
                    .to_string(),
                    vec![
                        text(session_id),
                        text(serde_json::to_string(&epoch.baseline)?),
                        text(serde_json::to_string(&epoch.snapshot)?),
                        int(store_i64(epoch.generation)),
                        int(store_i64(epoch.baseline_seq)),
                        int(store_i64(epoch.updated)),
                    ],
                )],
                &event,
                None,
            )
            .await?;
        self.publish_committed(event);
        Ok(())
    }

    pub(crate) async fn enqueue_prompt_with_event(
        &self,
        session_id: &str,
        request: &PromptRequest,
        delivery: &str,
        event: EventPayload,
    ) -> anyhow::Result<usize> {
        let mut event = event;
        self.allocate_event_sequence(&mut event);
        self.inner
            .store
            .commit_projection_event(
                vec![(
                    "INSERT INTO prompt_queue (id, session_id, position, request_json, created, delivery) VALUES (?, ?, (SELECT COALESCE(MAX(position), -1) + 1 FROM prompt_queue WHERE session_id = ?), ?, ?, ?)".to_string(),
                    vec![
                        text(Id::ascending(IdKind::Event).to_string()),
                        text(session_id),
                        text(session_id),
                        text(serde_json::to_string(request)?),
                        int(store_i64(crate::now_millis())),
                        text(delivery),
                    ],
                )],
                &event,
                None,
            )
            .await?;
        self.publish_committed(event);
        self.inner.store.queued_prompt_count(session_id).await
    }
}

impl SessionStore {
    #[cfg(test)]
    pub(crate) async fn lock_writer_for_test(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.db.write_gate.clone().lock_owned().await
    }
    async fn commit_projection_event(
        &self,
        mut projection: Vec<(String, Vec<SqlValue>)>,
        event: &EventPayload,
        owner_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let aggregate_id = crate::sync::aggregate_id(event);
        let session_id = event
            .properties
            .get("sessionID")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        projection.extend(event_statements(
            event,
            &aggregate_id,
            session_id,
            owner_id,
        )?);
        self.db.execute_transaction(projection).await
    }

    pub(crate) async fn open_default() -> anyhow::Result<Self> {
        let state_dir = PathBuf::from(crate::default_state_dir());
        std::fs::create_dir_all(&state_dir).with_context(|| {
            format!("failed to create state directory {}", state_dir.display())
        })?;
        Self::open(state_dir.join("agent.turso.db")).await
    }

    pub(crate) async fn open(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create database directory {}", parent.display())
            })?;
        }

        let path = path
            .to_str()
            .context("turso database path is not valid UTF-8")?;
        let database = turso::Builder::new_local(path)
            .build()
            .await
            .with_context(|| format!("failed to open turso database {path}"))?;
        let db = Db {
            database,
            write_gate: Arc::new(Mutex::new(())),
        };
        let store = Self { db };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                info_json TEXT NOT NULL,
                updated INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        // List metadata lives in a compact sidecar. Creating it is constant
        // time even for legacy databases whose session JSON is many gigabytes.
        self.db
            .execute_transaction(vec![
                (
                    r#"CREATE TABLE IF NOT EXISTS schema_migrations (
                        version INTEGER PRIMARY KEY,
                        name TEXT NOT NULL,
                        applied_at INTEGER NOT NULL
                    )"#
                    .to_string(),
                    Vec::new(),
                ),
                (
                    r#"CREATE TABLE IF NOT EXISTS data_migrations (
                        name TEXT PRIMARY KEY,
                        state TEXT NOT NULL,
                        cursor_updated INTEGER,
                        cursor_id TEXT,
                        rows_done INTEGER NOT NULL DEFAULT 0,
                        updated_at INTEGER NOT NULL,
                        error TEXT
                    )"#
                    .to_string(),
                    Vec::new(),
                ),
                (
                    r#"CREATE TABLE IF NOT EXISTS session_list_index (
                        session_id TEXT PRIMARY KEY,
                        directory TEXT NOT NULL,
                        path TEXT,
                        parent_id TEXT,
                        title TEXT NOT NULL,
                        updated INTEGER NOT NULL,
                        archived INTEGER,
                        tenant_id TEXT NOT NULL DEFAULT 'local',
                        workspace_id TEXT,
                        summary_json TEXT
                    )"#
                    .to_string(),
                    Vec::new(),
                ),
                (
                    "CREATE INDEX IF NOT EXISTS idx_session_list_roots_directory_updated ON session_list_index(directory, updated DESC, session_id DESC) WHERE parent_id IS NULL"
                        .to_string(),
                    Vec::new(),
                ),
                (
                    "CREATE INDEX IF NOT EXISTS idx_session_list_roots_updated ON session_list_index(updated DESC, session_id DESC) WHERE parent_id IS NULL"
                        .to_string(),
                    Vec::new(),
                ),
                (
                    "INSERT OR IGNORE INTO schema_migrations (version, name, applied_at) VALUES (1, 'session-list-index', ?)"
                        .to_string(),
                    vec![int(store_i64(crate::now_millis()))],
                ),
                (
                    "INSERT OR IGNORE INTO data_migrations (name, state, rows_done, updated_at) VALUES ('session-list-index-v1', 'pending', 0, ?)"
                        .to_string(),
                    vec![int(store_i64(crate::now_millis()))],
                ),
            ])
            .await?;
        if !self.schema_migration_applied(3).await? {
            if !self
                .table_has_column("session_list_index", "summary_json")
                .await?
            {
                self.db
                    .execute(
                        "ALTER TABLE session_list_index ADD COLUMN summary_json TEXT",
                        Vec::new(),
                    )
                    .await?;
            }
            self.db
                .execute(
                    "INSERT OR IGNORE INTO schema_migrations (version, name, applied_at) VALUES (3, 'session-list-summary', ?)",
                    vec![int(store_i64(crate::now_millis()))],
                )
                .await?;
        }
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS execution_session_activity (
                root_session_id TEXT NOT NULL,
                execution_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                completed_ms INTEGER NOT NULL,
                PRIMARY KEY (root_session_id, execution_id, session_id)
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS execution_activity (
                root_session_id TEXT PRIMARY KEY,
                execution_id TEXT NOT NULL,
                root_message_id TEXT NOT NULL,
                completed_ms INTEGER NOT NULL,
                revision INTEGER NOT NULL,
                family_revision INTEGER NOT NULL DEFAULT 0,
                finished INTEGER NOT NULL DEFAULT 0,
                updated INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS execution_activity_segments (
                segment_id TEXT PRIMARY KEY,
                root_session_id TEXT NOT NULL,
                execution_id TEXT NOT NULL,
                owner_instance_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                started_at INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        if !self.schema_migration_applied(2).await? {
            // Additive migrations for databases created by the first execution
            // timer implementation. Column checks replace ignored ALTER errors,
            // and the data copy runs only once instead of on every launch.
            if !self.table_has_column("execution_activity", "family_revision").await? {
                self.db
                    .execute(
                        "ALTER TABLE execution_activity ADD COLUMN family_revision INTEGER NOT NULL DEFAULT 0",
                        Vec::new(),
                    )
                    .await?;
            }
            for (column, definition) in [
                ("session_id", "TEXT NOT NULL DEFAULT ''"),
                ("execution_id", "TEXT NOT NULL DEFAULT ''"),
                ("owner_instance_id", "TEXT NOT NULL DEFAULT ''"),
            ] {
                if !self
                    .table_has_column("execution_activity_segments", column)
                    .await?
                {
                    self.db
                        .execute(
                            &format!(
                                "ALTER TABLE execution_activity_segments ADD COLUMN {column} {definition}"
                            ),
                            Vec::new(),
                        )
                        .await?;
                }
            }
            self.db
                .execute_transaction(vec![
                    (
                        "UPDATE execution_activity_segments SET execution_id = COALESCE((SELECT execution_id FROM execution_activity WHERE execution_activity.root_session_id = execution_activity_segments.root_session_id), '') WHERE execution_id = ''".to_string(),
                        Vec::new(),
                    ),
                    (
                        "UPDATE execution_activity_segments SET session_id = root_session_id WHERE session_id = ''".to_string(),
                        Vec::new(),
                    ),
                    (
                        "INSERT OR IGNORE INTO execution_session_activity (root_session_id, execution_id, session_id, completed_ms) SELECT root_session_id, execution_id, root_session_id, completed_ms FROM execution_activity".to_string(),
                        Vec::new(),
                    ),
                    (
                        "INSERT OR IGNORE INTO execution_session_activity (root_session_id, execution_id, session_id, completed_ms) SELECT root_session_id, execution_id, session_id, 0 FROM execution_activity_segments".to_string(),
                        Vec::new(),
                    ),
                    (
                        "INSERT INTO schema_migrations (version, name, applied_at) VALUES (2, 'execution-activity-v2', ?)".to_string(),
                        vec![int(store_i64(crate::now_millis()))],
                    ),
                ])
                .await?;
        }
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS execution_activity_owners (
                owner_instance_id TEXT PRIMARY KEY,
                heartbeat_at INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS execution_subtasks (
                execution_id TEXT NOT NULL,
                child_session_id TEXT NOT NULL,
                root_session_id TEXT NOT NULL,
                parent_session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                PRIMARY KEY (execution_id, child_session_id)
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS interaction_requests (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                session_id TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                state TEXT NOT NULL,
                response_json TEXT,
                created INTEGER NOT NULL,
                updated INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS artifacts (
                id TEXT PRIMARY KEY,
                filename TEXT NOT NULL,
                media_type TEXT NOT NULL,
                size INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                session_id TEXT,
                tenant_id TEXT NOT NULL DEFAULT 'local',
                created INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS audit_log (
                id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                subject TEXT,
                method TEXT NOT NULL,
                path TEXT NOT NULL,
                status INTEGER NOT NULL,
                created INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        // Existing stores predate actor-level audit identity.
        let _ = self
            .db
            .execute(
                "ALTER TABLE audit_log ADD COLUMN subject TEXT",
                Vec::new(),
            )
            .await;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS workflows (
                activation_id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                source_path TEXT NOT NULL,
                source_hash TEXT NOT NULL,
                definition_json TEXT NOT NULL,
                active INTEGER NOT NULL,
                activated_at INTEGER NOT NULL,
                last_scheduled_at INTEGER,
                updated INTEGER NOT NULL,
                UNIQUE(workflow_id, workspace_root)
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS workflow_runs (
                id TEXT PRIMARY KEY,
                activation_id TEXT NOT NULL,
                workflow_id TEXT NOT NULL,
                scheduled_at INTEGER NOT NULL,
                started_at INTEGER,
                finished_at INTEGER,
                session_id TEXT,
                status TEXT NOT NULL,
                trigger TEXT NOT NULL,
                error TEXT,
                created INTEGER NOT NULL,
                UNIQUE(activation_id, scheduled_at)
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_json TEXT NOT NULL,
                created INTEGER NOT NULL,
                position INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS permission_approvals (
                project_id TEXT PRIMARY KEY,
                rules_json TEXT NOT NULL,
                updated INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS prompt_queue (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                request_json TEXT NOT NULL,
                created INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                aggregate_id TEXT,
                aggregate_seq INTEGER,
                owner_id TEXT,
                session_id TEXT,
                event_json TEXT NOT NULL,
                created INTEGER NOT NULL
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS event_sequences (
                aggregate_id TEXT PRIMARY KEY,
                seq INTEGER NOT NULL,
                owner_id TEXT
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS session_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                created INTEGER NOT NULL,
                updated INTEGER NOT NULL,
                error_json TEXT,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS session_context_epochs (
                session_id TEXT PRIMARY KEY,
                baseline_json TEXT NOT NULL,
                snapshot_json TEXT NOT NULL,
                generation INTEGER NOT NULL,
                baseline_seq INTEGER NOT NULL,
                updated INTEGER NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.ensure_prompt_queue_columns().await?;
        self.db
            .execute(
                r#"
            CREATE INDEX IF NOT EXISTS idx_events_missing_aggregate
            ON events(seq)
            WHERE aggregate_id IS NULL OR aggregate_seq IS NULL
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_messages_session_position ON messages(session_id, position)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_prompt_queue_session_position ON prompt_queue(session_id, position)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_events_seq ON events(seq)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_events_event_id ON events(event_id)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_events_aggregate_seq ON events(aggregate_id, aggregate_seq)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_session_runs_session_updated ON session_runs(session_id, updated)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_workflows_active ON workflows(active, updated)",
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_workflow_runs_history ON workflow_runs(activation_id, scheduled_at DESC)",
                Vec::new(),
            )
            .await?;
        self.migrate_semantic().await?;
        self.migrate_memory_semantic().await?;
        Ok(())
    }

    /// Vector-embedding mirror of `messages` for semantic search. Rows
    /// with a NULL embedding are tombstones for messages with no searchable
    /// text, so the indexer doesn't retry them forever.
    async fn migrate_semantic(&self) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS message_embeddings (
                message_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                created INTEGER NOT NULL,
                model TEXT NOT NULL,
                embedding BLOB
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_message_embeddings_session ON message_embeddings(session_id)",
                Vec::new(),
            )
            .await?;
        Ok(())
    }

    pub(crate) fn semantic_search_supported(&self) -> bool {
        true
    }

    /// Messages that still need an embedding for `model` — new messages plus
    /// anything indexed under a different model (a model switch re-indexes).
    /// Sessions with an active run are skipped so streamed messages are only
    /// embedded once they've stopped changing.
    pub(crate) async fn messages_missing_embeddings(
        &self,
        model: &str,
        session_ids: &[String],
        limit: usize,
    ) -> anyhow::Result<Vec<PendingEmbedding>> {
        if session_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; session_ids.len()].join(", ");
        let sql = format!(
            "SELECT m.session_id, m.id, m.created, m.message_json \
             FROM messages m \
             LEFT JOIN message_embeddings e \
               ON e.message_id = m.id AND (e.model = ? OR e.model = 'none') \
             WHERE e.message_id IS NULL AND m.session_id IN ({placeholders}) \
               AND m.session_id NOT IN (SELECT session_id FROM session_runs WHERE status IN ('running', 'retry')) \
             ORDER BY m.created DESC LIMIT ?"
        );
        let mut params = vec![text(model)];
        params.extend(session_ids.iter().cloned().map(text));
        params.push(int(limit.clamp(1, 256) as i64));
        let rows = self
            .db
            .fetch_all(
                &sql,
                params,
            )
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PendingEmbedding {
                    session_id: row.get_str("session_id")?,
                    message_id: row.get_str("id")?,
                    created: row.get_i64("created")?,
                    message_json: row.get_str("message_json")?,
                })
            })
            .collect()
    }

    pub(crate) async fn upsert_message_embedding(
        &self,
        message_id: &str,
        session_id: &str,
        created: i64,
        model: &str,
        vector_json: &str,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            INSERT INTO message_embeddings (message_id, session_id, created, model, embedding)
            VALUES (?, ?, ?, ?, vector32(?))
            ON CONFLICT(message_id) DO UPDATE SET
                model = excluded.model,
                embedding = excluded.embedding
            "#,
                vec![
                    text(message_id),
                    text(session_id),
                    int(created),
                    text(model),
                    text(vector_json),
                ],
            )
            .await?;
        Ok(())
    }

    /// Mark a message as having nothing to embed (no searchable text or
    /// undecodable JSON) so the indexer stops picking it up.
    pub(crate) async fn tombstone_message_embedding(
        &self,
        message_id: &str,
        session_id: &str,
        created: i64,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            INSERT INTO message_embeddings (message_id, session_id, created, model, embedding)
            VALUES (?, ?, ?, 'none', NULL)
            ON CONFLICT(message_id) DO UPDATE SET model = 'none', embedding = NULL
            "#,
                vec![text(message_id), text(session_id), int(created)],
            )
            .await?;
        Ok(())
    }

    /// Rank indexed messages by cosine distance to an embedded query.
    /// Exact scan — fine at chat-history scale.
    pub(crate) async fn semantic_search(
        &self,
        query_vector_json: &str,
        model: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<SemanticSearchHit>> {
        let limit = limit.clamp(1, 50) as i64;
        let (sql, params) = match session_id {
            Some(session) => (
                "SELECT e.session_id, e.message_id, m.message_json, \
                 vector_distance_cos(e.embedding, vector32(?)) AS distance \
                 FROM message_embeddings e JOIN messages m ON m.id = e.message_id \
                 WHERE e.model = ? AND e.embedding IS NOT NULL AND e.session_id = ? \
                 ORDER BY distance ASC LIMIT ?",
                vec![
                    text(query_vector_json),
                    text(model),
                    text(session),
                    int(limit),
                ],
            ),
            None => (
                "SELECT e.session_id, e.message_id, m.message_json, \
                 vector_distance_cos(e.embedding, vector32(?)) AS distance \
                 FROM message_embeddings e JOIN messages m ON m.id = e.message_id \
                 WHERE e.model = ? AND e.embedding IS NOT NULL \
                 ORDER BY distance ASC LIMIT ?",
                vec![text(query_vector_json), text(model), int(limit)],
            ),
        };
        let rows = self.db.fetch_all(sql, params).await?;
        let mut hits = Vec::new();
        for row in rows {
            let json = row.get_str("message_json")?;
            let Ok(message) = serde_json::from_str::<MessageWithParts>(&json) else {
                continue;
            };
            let (role, created, content) = search_document(&message);
            let excerpt: String = content.replace('\n', " ").chars().take(200).collect();
            hits.push(SemanticSearchHit {
                session_id: row.get_str("session_id")?,
                message_id: row.get_str("message_id")?,
                role,
                created,
                excerpt,
                distance: row.get_f64("distance")?,
            });
        }
        Ok(hits)
    }

    /// Vector-embedding mirror of host-provided memory documents. Keyed by the
    /// adapter's stable document key;
    /// `content_hash` detects edits so recall re-embeds only changed files.
    /// Same turso-only gating as `message_embeddings`.
    async fn migrate_memory_semantic(&self) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            CREATE TABLE IF NOT EXISTS memory_embeddings (
                path TEXT PRIMARY KEY,
                root TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                model TEXT NOT NULL,
                updated INTEGER NOT NULL,
                embedding BLOB
            )
            "#,
                Vec::new(),
            )
            .await?;
        self.db
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_memory_embeddings_root ON memory_embeddings(root)",
                Vec::new(),
            )
            .await?;
        Ok(())
    }

    /// (path, content_hash) for every indexed memory file under `root` and
    /// `model` — recall diffs this against the files on disk to find what
    /// needs (re)embedding.
    pub(crate) async fn memory_embedding_hashes(
        &self,
        root: &str,
        model: &str,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT path, content_hash FROM memory_embeddings \
                 WHERE root = ? AND model = ? AND embedding IS NOT NULL",
                vec![text(root), text(model)],
            )
            .await?;
        rows.into_iter()
            .map(|row| Ok((row.get_str("path")?, row.get_str("content_hash")?)))
            .collect()
    }

    pub(crate) async fn upsert_memory_embedding(
        &self,
        path: &str,
        root: &str,
        content_hash: &str,
        model: &str,
        updated: i64,
        vector_json: &str,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            INSERT INTO memory_embeddings (path, root, content_hash, model, updated, embedding)
            VALUES (?, ?, ?, ?, ?, vector32(?))
            ON CONFLICT(path) DO UPDATE SET
                root = excluded.root,
                content_hash = excluded.content_hash,
                model = excluded.model,
                updated = excluded.updated,
                embedding = excluded.embedding
            "#,
                vec![
                    text(path),
                    text(root),
                    text(content_hash),
                    text(model),
                    int(updated),
                    text(vector_json),
                ],
            )
            .await?;
        Ok(())
    }

    /// Drop index rows for memory files that no longer exist on disk.
    pub(crate) async fn delete_memory_embedding(&self, path: &str) -> anyhow::Result<()> {
        self.db
            .execute(
                "DELETE FROM memory_embeddings WHERE path = ?",
                vec![text(path)],
            )
            .await?;
        Ok(())
    }

    /// Rank indexed memory files by cosine distance to an embedded query.
    /// Exact scan — memory stores are tens of files, not millions.
    pub(crate) async fn memory_semantic_search(
        &self,
        roots: &[String],
        query_vector_json: &str,
        model: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(String, f64)>> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; roots.len()].join(", ");
        let sql = format!(
            "SELECT path, vector_distance_cos(embedding, vector32(?)) AS distance \
             FROM memory_embeddings \
             WHERE model = ? AND embedding IS NOT NULL AND root IN ({placeholders}) \
             ORDER BY distance ASC LIMIT ?"
        );
        let mut params = vec![text(query_vector_json), text(model)];
        params.extend(roots.iter().map(|root| text(root)));
        params.push(int(limit.clamp(1, 100) as i64));
        let rows = self.db.fetch_all(&sql, params).await?;
        rows.into_iter()
            .map(|row| Ok((row.get_str("path")?, row.get_f64("distance")?)))
            .collect()
    }

    /// Search recent session transcripts by AND-matching query terms
    /// case-insensitively against each flattened message document.
    pub(crate) async fn search_messages(
        &self,
        query: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<MessageSearchHit>> {
        self.search_messages_like(query, session_id, limit.clamp(1, 50))
            .await
    }

    /// Prefilter in SQL on the longest term, then
    /// AND-matches every term against the same flattened searchable document
    /// mirror indexes. Bounded to the most recent candidates, so it trades
    /// recall on huge histories for predictable work.
    async fn search_messages_like(
        &self,
        query: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<MessageSearchHit>> {
        const SCAN_CAP: i64 = 500;
        let terms: Vec<&str> = query.split_whitespace().collect();
        let Some(seed) = terms.iter().max_by_key(|term| term.len()) else {
            return Ok(Vec::new());
        };
        let pattern = format!("%{}%", escape_like(seed));
        let (sql, params) = match session_id {
            Some(session) => (
                "SELECT session_id, id, message_json FROM messages \
                 WHERE message_json LIKE ? ESCAPE '\\' AND session_id = ? \
                 ORDER BY created DESC LIMIT ?",
                vec![text(pattern), text(session), int(SCAN_CAP)],
            ),
            None => (
                "SELECT session_id, id, message_json FROM messages \
                 WHERE message_json LIKE ? ESCAPE '\\' \
                 ORDER BY created DESC LIMIT ?",
                vec![text(pattern), int(SCAN_CAP)],
            ),
        };
        let rows = self.db.fetch_all(sql, params).await?;
        let mut hits = Vec::new();
        for row in rows {
            let json = row.get_str("message_json")?;
            let Ok(message) = serde_json::from_str::<MessageWithParts>(&json) else {
                continue;
            };
            let (role, created, content) = search_document(&message);
            let matches: Vec<usize> = terms
                .iter()
                .map(|term| find_ignore_ascii_case(&content, term))
                .collect::<Option<Vec<_>>>()
                .unwrap_or_default();
            let Some(&first) = matches.iter().min() else {
                continue;
            };
            let matched_term = terms[matches
                .iter()
                .position(|&start| start == first)
                .unwrap_or(0)];
            hits.push(MessageSearchHit {
                session_id: row.get_str("session_id")?,
                message_id: row.get_str("id")?,
                role,
                created,
                excerpt: like_excerpt(&content, first, matched_term.len()),
            });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    async fn ensure_prompt_queue_columns(&self) -> anyhow::Result<()> {
        if !self.table_has_column("prompt_queue", "delivery").await? {
            self.db
                .execute(
                    "ALTER TABLE prompt_queue ADD COLUMN delivery TEXT NOT NULL DEFAULT 'queue'",
                    Vec::new(),
                )
                .await?;
        }
        Ok(())
    }

    async fn table_has_column(&self, table: &str, column: &str) -> anyhow::Result<bool> {
        let rows = self
            .db
            .fetch_all(&format!("PRAGMA table_info({table})"), Vec::new())
            .await?;
        for row in rows {
            if row.get_str("name")? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn schema_migration_applied(&self, version: i64) -> anyhow::Result<bool> {
        Ok(self
            .db
            .fetch_optional(
                "SELECT version FROM schema_migrations WHERE version = ?",
                vec![int(version)],
            )
            .await?
            .is_some())
    }

    pub(crate) async fn list_sessions(&self) -> anyhow::Result<Vec<SessionInfo>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT info_json FROM sessions ORDER BY updated DESC",
                Vec::new(),
            )
            .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get_str("info_json")?))
            .collect()
    }

    pub(crate) async fn list_root_sessions_page(
        &self,
        directory: Option<&str>,
        path: Option<&str>,
        start: Option<u64>,
        search: Option<&str>,
        cursor: Option<&SessionListCursor>,
        limit: Option<usize>,
    ) -> anyhow::Result<SessionListPage> {
        let limit = limit.unwrap_or(50).clamp(1, 200);
        let mut clauses = vec!["i.parent_id IS NULL".to_string()];
        let mut params = Vec::new();
        if let Some(directory) = directory {
            clauses.push("i.directory = ?".to_string());
            params.push(text(directory));
        }
        if let Some(path) = path {
            clauses.push("i.path = ?".to_string());
            params.push(text(path));
        }
        if let Some(start) = start {
            clauses.push("i.updated >= ?".to_string());
            params.push(int(store_i64(start)));
        }
        if let Some(search) = search.filter(|value| !value.is_empty()) {
            clauses.push("LOWER(i.title) LIKE ?".to_string());
            params.push(text(format!("%{}%", search.to_lowercase())));
        }
        if let Some(cursor) = cursor {
            clauses.push("(i.updated < ? OR (i.updated = ? AND i.session_id < ?))".to_string());
            params.push(int(store_i64(cursor.updated)));
            params.push(int(store_i64(cursor.updated)));
            params.push(text(&cursor.id));
        }
        params.push(int((limit + 1) as i64));
        let rows = self
            .db
            .fetch_all(
                &format!(
                    "SELECT i.session_id, i.directory, i.path, i.parent_id, i.title, i.updated, i.archived, i.tenant_id, i.workspace_id, i.summary_json FROM session_list_index i WHERE {} ORDER BY i.updated DESC, i.session_id DESC LIMIT ?",
                    clauses.join(" AND ")
                ),
                params,
            )
            .await?;
        let has_more = rows.len() > limit;
        let mut items = Vec::with_capacity(rows.len().min(limit));
        let mut last_key = None;
        for row in rows.into_iter().take(limit) {
            last_key = Some((
                row.get_i64("updated")?.max(0) as u64,
                row.get_str("session_id")?,
            ));
            items.push(session_list_info_from_row(&row)?);
        }
        Ok(SessionListPage {
            next_cursor: has_more.then(|| {
                let (updated, id) = last_key.expect("a page with more rows has an item");
                SessionListCursor::encode(updated, &id)
            }),
            items,
        })
    }

    pub(crate) async fn session_list_index_ready(&self) -> anyhow::Result<bool> {
        Ok(self
            .db
            .fetch_optional(
                "SELECT state FROM data_migrations WHERE name = 'session-list-index-v1'",
                Vec::new(),
            )
            .await?
            .is_some_and(|row| row.get_str("state").is_ok_and(|state| state == "complete")))
    }

    /// Decode only sessions carrying a particular flattened `extra` key.
    /// Startup completion recovery uses this instead of parsing every stored
    /// session (whose context snapshots can be several megabytes each).
    pub(crate) async fn list_sessions_with_extra_key(
        &self,
        key: &str,
    ) -> anyhow::Result<Vec<SessionInfo>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT info_json FROM sessions WHERE info_json LIKE ? ORDER BY updated DESC",
                vec![text(format!("%\"{key}\"%"))],
            )
            .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get_str("info_json")?))
            .collect()
    }

    pub(crate) async fn insert_session(&self, info: &SessionInfo) -> anyhow::Result<()> {
        self.db
            .execute_transaction(vec![
                (
                    "INSERT INTO sessions (id, info_json, updated) VALUES (?, ?, ?)".to_string(),
                    vec![
                        text(info.id.to_string()),
                        text(serde_json::to_string(info)?),
                        int(store_i64(info.time.updated)),
                    ],
                ),
                session_list_index_statement(info)?,
            ])
            .await?;
        Ok(())
    }

    pub(crate) async fn upsert_workflow(
        &self,
        projection: &crate::workflow::WorkflowProjection,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"INSERT INTO workflows
                (activation_id, workflow_id, workspace_root, source_path, source_hash,
                 definition_json, active, activated_at, last_scheduled_at, updated)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(activation_id) DO UPDATE SET
                  source_path = excluded.source_path,
                  source_hash = excluded.source_hash,
                  definition_json = excluded.definition_json,
                  active = excluded.active,
                  updated = excluded.updated"#,
                vec![
                    text(&projection.activation_id),
                    text(&projection.workflow_id),
                    text(&projection.workspace_root),
                    text(&projection.source_path),
                    text(&projection.source_hash),
                    text(serde_json::to_string(&projection.definition)?),
                    int(i64::from(projection.active)),
                    int(store_i64(projection.activated_at)),
                    projection
                        .last_scheduled_at
                        .map(|value| int(store_i64(value)))
                        .unwrap_or(SqlValue::Null),
                    int(store_i64(projection.updated)),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn set_workflow_active(
        &self,
        activation_id: &str,
        active: bool,
        updated: u64,
    ) -> anyhow::Result<bool> {
        Ok(self
            .db
            .execute(
                "UPDATE workflows SET active = ?, updated = ? WHERE activation_id = ?",
                vec![
                    int(i64::from(active)),
                    int(store_i64(updated)),
                    text(activation_id),
                ],
            )
            .await?
            > 0)
    }

    pub(crate) async fn get_workflow(
        &self,
        activation_id: &str,
    ) -> anyhow::Result<Option<crate::workflow::WorkflowProjection>> {
        let row = self
            .db
            .fetch_optional(
                "SELECT * FROM workflows WHERE activation_id = ?",
                vec![text(activation_id)],
            )
            .await?;
        row.map(decode_workflow_projection).transpose()
    }

    pub(crate) async fn list_active_workflows(
        &self,
    ) -> anyhow::Result<Vec<crate::workflow::WorkflowProjection>> {
        self.db
            .fetch_all(
                "SELECT * FROM workflows WHERE active = 1 ORDER BY workflow_id",
                Vec::new(),
            )
            .await?
            .into_iter()
            .map(decode_workflow_projection)
            .collect()
    }

    pub(crate) async fn list_workflows(
        &self,
    ) -> anyhow::Result<Vec<crate::workflow::WorkflowProjection>> {
        self.db
            .fetch_all("SELECT * FROM workflows ORDER BY workflow_id", Vec::new())
            .await?
            .into_iter()
            .map(decode_workflow_projection)
            .collect()
    }

    pub(crate) async fn claim_workflow_run(
        &self,
        run: &crate::workflow::WorkflowRun,
    ) -> anyhow::Result<bool> {
        Ok(self
            .db
            .execute(
                r#"INSERT OR IGNORE INTO workflow_runs
                (id, activation_id, workflow_id, scheduled_at, started_at, finished_at,
                  session_id, status, trigger, error, created)
                SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
                WHERE NOT EXISTS (
                  SELECT 1 FROM workflow_runs
                  WHERE activation_id = ? AND status IN ('queued', 'running')
                )"#,
                vec![
                    text(&run.id),
                    text(&run.activation_id),
                    text(&run.workflow_id),
                    int(store_i64(run.scheduled_at)),
                    run.started_at
                        .map(|v| int(store_i64(v)))
                        .unwrap_or(SqlValue::Null),
                    run.finished_at
                        .map(|v| int(store_i64(v)))
                        .unwrap_or(SqlValue::Null),
                    opt_text(run.session_id.clone()),
                    text(&run.status),
                    text(&run.trigger),
                    opt_text(run.error.clone()),
                    int(store_i64(run.created)),
                    text(&run.activation_id),
                ],
            )
            .await?
            > 0)
    }

    /// Atomically records a scheduled occurrence and advances its durable
    /// cursor. A restart can therefore observe both writes or neither, never a
    /// queued occurrence with a stale cursor. The cursor advances even when an
    /// overlapping run suppresses the insert, preserving scheduler coalescing.
    pub(crate) async fn claim_scheduled_workflow_run(
        &self,
        run: &crate::workflow::WorkflowRun,
    ) -> anyhow::Result<bool> {
        let results = self
            .db
            .execute_transaction_with_results(vec![
                (
                    r#"INSERT OR IGNORE INTO workflow_runs
                    (id, activation_id, workflow_id, scheduled_at, started_at, finished_at,
                      session_id, status, trigger, error, created)
                    SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
                    WHERE NOT EXISTS (
                      SELECT 1 FROM workflow_runs
                      WHERE activation_id = ? AND status IN ('queued', 'running')
                    )"#
                    .to_string(),
                    vec![
                        text(&run.id),
                        text(&run.activation_id),
                        text(&run.workflow_id),
                        int(store_i64(run.scheduled_at)),
                        run.started_at
                            .map(|v| int(store_i64(v)))
                            .unwrap_or(SqlValue::Null),
                        run.finished_at
                            .map(|v| int(store_i64(v)))
                            .unwrap_or(SqlValue::Null),
                        opt_text(run.session_id.clone()),
                        text(&run.status),
                        text(&run.trigger),
                        opt_text(run.error.clone()),
                        int(store_i64(run.created)),
                        text(&run.activation_id),
                    ],
                ),
                (
                    r#"UPDATE workflows
                    SET last_scheduled_at = CASE
                          WHEN last_scheduled_at IS NULL OR last_scheduled_at < ? THEN ?
                          ELSE last_scheduled_at
                        END,
                        updated = ?
                    WHERE activation_id = ?"#
                        .to_string(),
                    vec![
                        int(store_i64(run.scheduled_at)),
                        int(store_i64(run.scheduled_at)),
                        int(store_i64(crate::now_millis())),
                        text(&run.activation_id),
                    ],
                ),
            ])
            .await?;
        Ok(results.first().copied().unwrap_or_default() > 0)
    }

    pub(crate) async fn update_workflow_run(
        &self,
        run_id: &str,
        status: &str,
        session_id: Option<&str>,
        error: Option<&str>,
        finished: bool,
    ) -> anyhow::Result<()> {
        let now = crate::now_millis();
        self.db
            .execute(
                "UPDATE workflow_runs SET status = ?, session_id = COALESCE(?, session_id), started_at = COALESCE(started_at, ?), finished_at = ?, error = ? WHERE id = ?",
                vec![
                    text(status),
                    session_id.map(text).unwrap_or(SqlValue::Null),
                    int(store_i64(now)),
                    finished.then(|| int(store_i64(now))).unwrap_or(SqlValue::Null),
                    error.map(text).unwrap_or(SqlValue::Null),
                    text(run_id),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn list_workflow_runs(
        &self,
        activation_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<crate::workflow::WorkflowRun>> {
        self.db
            .fetch_all(
                "SELECT * FROM workflow_runs WHERE activation_id = ? ORDER BY scheduled_at DESC LIMIT ?",
                vec![text(activation_id), int(limit.min(200) as i64)],
            )
            .await?
            .into_iter()
            .map(decode_workflow_run)
            .collect()
    }

    pub(crate) async fn list_unfinished_workflow_runs(
        &self,
    ) -> anyhow::Result<Vec<crate::workflow::WorkflowRun>> {
        self.db
            .fetch_all(
                "SELECT * FROM workflow_runs WHERE status IN ('queued', 'running') ORDER BY scheduled_at",
                Vec::new(),
            )
            .await?
            .into_iter()
            .map(decode_workflow_run)
            .collect()
    }

    pub(crate) async fn update_session(&self, info: &SessionInfo) -> anyhow::Result<()> {
        self.db
            .execute_transaction(vec![
                (
                    "UPDATE sessions SET info_json = ?, updated = ? WHERE id = ?".to_string(),
                    vec![
                        text(serde_json::to_string(info)?),
                        int(store_i64(info.time.updated)),
                        text(info.id.to_string()),
                    ],
                ),
                session_list_index_statement(info)?,
            ])
            .await?;
        Ok(())
    }

    pub(crate) async fn get_context_epoch(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<crate::context_epoch::ContextEpoch>> {
        let row = self
            .db
            .fetch_optional(
                "SELECT baseline_json, snapshot_json, generation, baseline_seq, updated FROM session_context_epochs WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        row.map(|row| {
            Ok(crate::context_epoch::ContextEpoch {
                baseline: decode_json(row.get_str("baseline_json")?)?,
                snapshot: decode_json(row.get_str("snapshot_json")?)?,
                generation: row.get_i64("generation")?.max(1) as u64,
                baseline_seq: row.get_i64("baseline_seq")?.max(0) as u64,
                updated: row.get_i64("updated")?.max(0) as u64,
            })
        })
        .transpose()
    }

    pub(crate) async fn message_sequence(&self, session_id: &str) -> anyhow::Result<u64> {
        Ok(self
            .db
            .fetch_scalar_i64(
                "SELECT COALESCE(MAX(position), 0) FROM messages WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?
            .max(0) as u64)
    }

    pub(crate) async fn get_session(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<SessionInfo>> {
        let row = self
            .db
            .fetch_optional(
                "SELECT info_json FROM sessions WHERE id = ?",
                vec![text(session_id)],
            )
            .await?;
        row.map(|row| decode_json(row.get_str("info_json")?))
            .transpose()
    }

    #[cfg(test)]
    pub(crate) async fn replace_execution_activity(
        &self,
        snapshot: &neoism_agent_core::ExecutionActivitySnapshot,
    ) -> anyhow::Result<()> {
        self.db
            .execute_transaction_with_results(vec![
                (
                    "DELETE FROM execution_activity_segments WHERE root_session_id = ?".to_string(),
                    vec![text(&snapshot.root_session_id)],
                ),
                (
                    r#"INSERT INTO execution_activity
                    (root_session_id, execution_id, root_message_id, completed_ms, revision, family_revision, finished, updated)
                    VALUES (?, ?, ?, ?, ?, 1, ?, ?)
                    ON CONFLICT(root_session_id) DO UPDATE SET
                      execution_id = excluded.execution_id,
                      root_message_id = excluded.root_message_id,
                      completed_ms = excluded.completed_ms,
                      revision = excluded.revision,
                      family_revision = execution_activity.family_revision + 1,
                      finished = excluded.finished,
                      updated = excluded.updated"#
                        .to_string(),
                    vec![
                        text(&snapshot.root_session_id),
                        text(&snapshot.execution_id),
                        text(&snapshot.root_message_id),
                        int(store_i64(snapshot.completed_ms)),
                        int(store_i64(snapshot.revision)),
                        int(i64::from(snapshot.finished)),
                        int(store_i64(crate::now_millis())),
                    ],
                ),
            ])
            .await?;
        Ok(())
    }

    pub(crate) async fn admit_execution_activity(
        &self,
        root_session_id: &str,
        execution_id: &str,
        root_message_id: &str,
        allowed_run_id: &str,
    ) -> anyhow::Result<Option<neoism_agent_core::ExecutionActivitySnapshot>> {
        let now = crate::now_millis();
        self.db.execute_transaction(vec![
            (
                r#"UPDATE execution_activity SET
                     execution_id = CASE WHEN root_message_id = ? THEN execution_id ELSE ? END,
                     root_message_id = ?,
                     completed_ms = CASE WHEN root_message_id = ? THEN completed_ms ELSE 0 END,
                     revision = CASE WHEN root_message_id = ? THEN revision ELSE revision + 1 END,
                     family_revision = CASE WHEN root_message_id = ? THEN family_revision ELSE family_revision + 1 END,
                     finished = CASE WHEN root_message_id = ? THEN finished ELSE 0 END,
                     updated = ?
                   WHERE root_session_id = ? AND (root_message_id = ? OR (
                     finished = 1
                     AND NOT EXISTS (SELECT 1 FROM execution_activity_segments s WHERE s.root_session_id = ? AND s.execution_id = execution_activity.execution_id)
                     AND NOT EXISTS (SELECT 1 FROM execution_subtasks t WHERE t.execution_id = execution_activity.execution_id AND t.status = 'outstanding')
                     AND NOT EXISTS (SELECT 1 FROM session_runs r WHERE r.status = 'running' AND r.id <> ? AND (r.session_id = ? OR r.session_id IN (SELECT child_session_id FROM execution_subtasks WHERE execution_id = execution_activity.execution_id)))
                     AND NOT EXISTS (SELECT 1 FROM prompt_queue q WHERE q.session_id = ? OR q.session_id IN (SELECT child_session_id FROM execution_subtasks WHERE execution_id = execution_activity.execution_id))
                   ))"#.to_string(),
                vec![text(root_message_id), text(execution_id), text(root_message_id), text(root_message_id), text(root_message_id), text(root_message_id), text(root_message_id), int(store_i64(now)), text(root_session_id), text(root_message_id), text(root_session_id), text(allowed_run_id), text(root_session_id), text(root_session_id)],
            ),
            (
                "INSERT OR IGNORE INTO execution_activity (root_session_id, execution_id, root_message_id, completed_ms, revision, family_revision, finished, updated) SELECT ?, ?, ?, 0, 1, 1, 0, ? WHERE NOT EXISTS (SELECT 1 FROM session_runs r WHERE r.session_id = ? AND r.status = 'running' AND r.id <> ?)".to_string(),
                vec![text(root_session_id), text(execution_id), text(root_message_id), int(store_i64(now)), text(root_session_id), text(allowed_run_id)],
            ),
            (
                "DELETE FROM execution_session_activity WHERE root_session_id = ? AND execution_id <> (SELECT execution_id FROM execution_activity WHERE root_session_id = ?)".to_string(),
                vec![text(root_session_id), text(root_session_id)],
            ),
            (
                "INSERT OR IGNORE INTO execution_session_activity (root_session_id, execution_id, session_id, completed_ms) SELECT root_session_id, execution_id, root_session_id, 0 FROM execution_activity WHERE root_session_id = ?".to_string(),
                vec![text(root_session_id)],
            ),
        ]).await?;
        let snapshot = self.get_execution_activity(root_session_id).await?;
        Ok(snapshot.filter(|snapshot| snapshot.root_message_id == root_message_id))
    }

    pub(crate) async fn get_execution_activity(
        &self,
        root_session_id: &str,
    ) -> anyhow::Result<Option<neoism_agent_core::ExecutionActivitySnapshot>> {
        let Some(row) = self
            .db
            .fetch_optional(
                "SELECT * FROM execution_activity WHERE root_session_id = ?",
                vec![text(root_session_id)],
            )
            .await?
        else {
            return Ok(None);
        };
        let segment_rows = self
            .db
            .fetch_all(
                "SELECT segment_id, session_id, started_at FROM execution_activity_segments WHERE root_session_id = ? AND execution_id = ?",
                vec![text(root_session_id), text(row.get_str("execution_id")?)],
            )
            .await?;
        let mut active_segments = std::collections::BTreeMap::new();
        let mut session_activities = std::collections::BTreeMap::new();
        for activity in self.db.fetch_all(
            "SELECT session_id, completed_ms FROM execution_session_activity WHERE root_session_id = ? AND execution_id = ?",
            vec![text(root_session_id), text(row.get_str("execution_id")?)],
        ).await? {
            session_activities.insert(
                activity.get_str("session_id")?,
                neoism_agent_core::ProviderActivitySnapshot {
                    completed_ms: activity.get_i64("completed_ms")?.max(0) as u64,
                    active_segments: std::collections::BTreeMap::new(),
                },
            );
        }
        for segment in segment_rows {
            let segment_id = segment.get_str("segment_id")?;
            let started_at = segment.get_i64("started_at")?.max(0) as u64;
            active_segments.insert(segment_id.clone(), started_at);
            session_activities
                .entry(segment.get_str("session_id")?)
                .or_insert_with(neoism_agent_core::ProviderActivitySnapshot::default)
                .active_segments
                .insert(segment_id, started_at);
        }
        Ok(Some(neoism_agent_core::ExecutionActivitySnapshot {
            execution_id: row.get_str("execution_id")?,
            root_session_id: row.get_str("root_session_id")?,
            root_message_id: row.get_str("root_message_id")?,
            completed_ms: row.get_i64("completed_ms")?.max(0) as u64,
            active_segments,
            session_activities,
            revision: row.get_i64("revision")?.max(0) as u64,
            finished: row.get_i64("finished")? != 0,
        }))
    }

    pub(crate) async fn insert_execution_segment(
        &self,
        root_session_id: &str,
        execution_id: &str,
        segment_id: &str,
        owner_instance_id: &str,
        session_id: &str,
        started_at: u64,
    ) -> anyhow::Result<bool> {
        let results = self.db
            .execute_transaction_with_results(vec![
                (
                    "INSERT OR IGNORE INTO execution_activity_segments (segment_id, root_session_id, execution_id, owner_instance_id, session_id, started_at) SELECT ?, ?, ?, ?, ?, ? WHERE EXISTS (SELECT 1 FROM execution_activity WHERE root_session_id = ? AND execution_id = ? AND finished = 0)".to_string(),
                    vec![text(segment_id), text(root_session_id), text(execution_id), text(owner_instance_id), text(session_id), int(store_i64(started_at)), text(root_session_id), text(execution_id)],
                ),
                (
                    "UPDATE execution_activity SET revision = revision + 1, updated = ? WHERE root_session_id = ? AND execution_id = ? AND changes() > 0".to_string(),
                    vec![int(store_i64(crate::now_millis())), text(root_session_id), text(execution_id)],
                ),
                (
                    "INSERT OR IGNORE INTO execution_session_activity (root_session_id, execution_id, session_id, completed_ms) SELECT ?, ?, ?, 0 WHERE EXISTS (SELECT 1 FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ?)".to_string(),
                    vec![text(root_session_id), text(execution_id), text(session_id), text(segment_id), text(root_session_id), text(execution_id)],
                ),
            ])
            .await?;
        Ok(results.first().copied().unwrap_or(0) > 0)
    }

    pub(crate) async fn finish_execution_segment(
        &self,
        root_session_id: &str,
        execution_id: &str,
        segment_id: &str,
        ended_at: u64,
    ) -> anyhow::Result<bool> {
        let results = self.db
            .execute_transaction_with_results(vec![
                (
                    "INSERT OR IGNORE INTO execution_session_activity (root_session_id, execution_id, session_id, completed_ms) SELECT root_session_id, execution_id, session_id, 0 FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ?".to_string(),
                    vec![text(segment_id), text(root_session_id), text(execution_id)],
                ),
                (
                    "UPDATE execution_session_activity SET completed_ms = completed_ms + MAX(0, ? - (SELECT started_at FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ?)) WHERE root_session_id = ? AND execution_id = ? AND session_id = (SELECT session_id FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ?) AND EXISTS (SELECT 1 FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ?)".to_string(),
                    vec![int(store_i64(ended_at)), text(segment_id), text(root_session_id), text(execution_id), text(root_session_id), text(execution_id), text(segment_id), text(root_session_id), text(execution_id), text(segment_id), text(root_session_id), text(execution_id)],
                ),
                (
                    "UPDATE execution_activity SET completed_ms = completed_ms + MAX(0, ? - (SELECT started_at FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ?)), revision = revision + 1, updated = ? WHERE root_session_id = ? AND execution_id = ? AND changes() > 0 AND EXISTS (SELECT 1 FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ?)".to_string(),
                    vec![int(store_i64(ended_at)), text(segment_id), text(root_session_id), text(execution_id), int(store_i64(ended_at)), text(root_session_id), text(execution_id), text(segment_id), text(root_session_id), text(execution_id)],
                ),
                (
                    "DELETE FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ? AND changes() > 0".to_string(),
                    vec![text(segment_id), text(root_session_id), text(execution_id)],
                ),
            ])
            .await?;
        Ok(results.get(1).copied().unwrap_or(0) > 0)
    }

    pub(crate) async fn mark_execution_finished(
        &self,
        root_session_id: &str,
        execution_id: &str,
    ) -> anyhow::Result<bool> {
        Ok(self
            .db
            .execute(
                r#"UPDATE execution_activity
                   SET finished = 1, revision = revision + 1, updated = ?
                   WHERE root_session_id = ? AND execution_id = ? AND finished = 0
                     AND NOT EXISTS (
                       SELECT 1 FROM execution_activity_segments
                       WHERE root_session_id = ? AND execution_id = ?
                     )
                     AND NOT EXISTS (
                       SELECT 1 FROM execution_subtasks
                       WHERE execution_id = ? AND status = 'outstanding'
                     )
                     AND NOT EXISTS (
                       SELECT 1 FROM session_runs
                       WHERE status = 'running' AND (
                         session_id = ? OR session_id IN (
                           SELECT child_session_id FROM execution_subtasks
                           WHERE execution_id = ?
                         )
                       )
                     )
                     AND NOT EXISTS (
                       SELECT 1 FROM prompt_queue
                       WHERE session_id = ? OR session_id IN (
                         SELECT child_session_id FROM execution_subtasks
                         WHERE execution_id = ?
                       )
                     )"#,
                vec![
                    int(store_i64(crate::now_millis())),
                    text(root_session_id),
                    text(execution_id),
                    text(root_session_id),
                    text(execution_id),
                    text(execution_id),
                    text(root_session_id),
                    text(execution_id),
                    text(root_session_id),
                    text(execution_id),
                ],
            )
            .await?
            > 0)
    }

    pub(crate) async fn register_execution_subtask(
        &self,
        execution_id: &str,
        root_session_id: &str,
        parent_session_id: &str,
        child_session_id: &str,
        started_at: u64,
    ) -> anyhow::Result<ExecutionSubtaskRegistration> {
        let results = self.db
            .execute_transaction_with_results(vec![
                (
                r#"INSERT OR IGNORE INTO execution_subtasks
                (execution_id, child_session_id, root_session_id, parent_session_id, status, started_at)
                SELECT ?, ?, ?, ?, 'outstanding', ?
                WHERE EXISTS (
                    SELECT 1 FROM execution_activity
                    WHERE root_session_id = ? AND execution_id = ? AND finished = 0
                )"#
                        .to_string(),
                vec![
                    text(execution_id),
                    text(child_session_id),
                    text(root_session_id),
                    text(parent_session_id),
                    int(store_i64(started_at)),
                    text(root_session_id),
                    text(execution_id),
                ]),
                (
                    "UPDATE execution_activity SET family_revision = family_revision + 1, updated = ? WHERE root_session_id = ? AND execution_id = ? AND changes() > 0".to_string(),
                    vec![int(store_i64(crate::now_millis())), text(root_session_id), text(execution_id)],
                ),
            ])
            .await?;
        if results.first().copied().unwrap_or(0) > 0 {
            return Ok(ExecutionSubtaskRegistration::Inserted);
        }
        if self
            .execution_subtask_status(execution_id, child_session_id)
            .await?
            .is_some()
        {
            Ok(ExecutionSubtaskRegistration::AlreadyPresent)
        } else {
            Ok(ExecutionSubtaskRegistration::Rejected)
        }
    }

    pub(crate) async fn execution_subtask_status(
        &self,
        execution_id: &str,
        child_session_id: &str,
    ) -> anyhow::Result<Option<String>> {
        self.db
            .fetch_optional(
                "SELECT status FROM execution_subtasks WHERE execution_id = ? AND child_session_id = ?",
                vec![text(execution_id), text(child_session_id)],
            )
            .await?
            .map(|row| row.get_str("status"))
            .transpose()
    }

    pub(crate) async fn finish_execution_subtask(
        &self,
        execution_id: &str,
        child_session_id: &str,
        status: &str,
    ) -> anyhow::Result<bool> {
        let results = self.db.execute_transaction_with_results(vec![
            (
                "UPDATE execution_subtasks SET status = ? WHERE execution_id = ? AND child_session_id = ? AND status = 'outstanding'".to_string(),
                vec![text(status), text(execution_id), text(child_session_id)],
            ),
            (
                "UPDATE execution_activity SET family_revision = family_revision + 1, updated = ? WHERE execution_id = ? AND changes() > 0".to_string(),
                vec![int(store_i64(crate::now_millis())), text(execution_id)],
            ),
        ]).await?;
        Ok(results.first().copied().unwrap_or(0) > 0)
    }

    pub(crate) async fn list_execution_subtasks(
        &self,
        execution_id: &str,
    ) -> anyhow::Result<Vec<neoism_agent_core::SubtaskLifecycleSnapshot>> {
        self.db
            .fetch_all(
                "SELECT child_session_id, parent_session_id, status, started_at FROM execution_subtasks WHERE execution_id = ? ORDER BY started_at ASC, child_session_id ASC",
                vec![text(execution_id)],
            )
            .await?
            .into_iter()
            .map(|row| {
                Ok(neoism_agent_core::SubtaskLifecycleSnapshot {
                    session_id: row.get_str("child_session_id")?,
                    parent_session_id: row.get_str("parent_session_id")?,
                    status: row.get_str("status")?,
                    started_at: Some(row.get_i64("started_at")?.max(0) as u64),
                })
            })
            .collect()
    }

    pub(crate) async fn heartbeat_execution_owner(
        &self,
        owner_instance_id: &str,
        heartbeat_at: u64,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                "INSERT INTO execution_activity_owners (owner_instance_id, heartbeat_at) VALUES (?, ?) ON CONFLICT(owner_instance_id) DO UPDATE SET heartbeat_at = excluded.heartbeat_at",
                vec![text(owner_instance_id), int(store_i64(heartbeat_at))],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn reconcile_stale_execution_segments(
        &self,
        stale_before: u64,
    ) -> anyhow::Result<usize> {
        let rows = self
            .db
            .fetch_all(
                r#"SELECT s.segment_id, s.root_session_id, s.execution_id,
                           s.owner_instance_id, s.session_id, s.started_at,
                          COALESCE(o.heartbeat_at, s.started_at) AS heartbeat_at
                   FROM execution_activity_segments s
                   LEFT JOIN execution_activity_owners o
                     ON o.owner_instance_id = s.owner_instance_id
                   WHERE o.owner_instance_id IS NULL OR o.heartbeat_at < ?"#,
                vec![int(store_i64(stale_before))],
            )
            .await?;
        let mut reconciled = 0;
        for row in rows {
            let segment = row.get_str("segment_id")?;
            let root = row.get_str("root_session_id")?;
            let execution = row.get_str("execution_id")?;
            let owner = row.get_str("owner_instance_id")?;
            let session = row.get_str("session_id")?;
            let started = row.get_i64("started_at")?.max(0) as u64;
            let ended = row.get_i64("heartbeat_at")?.max(0) as u64;
            let results = self
                .db
                .execute_transaction_with_results(vec![
                    (
                        "INSERT OR IGNORE INTO execution_session_activity (root_session_id, execution_id, session_id, completed_ms) SELECT root_session_id, execution_id, session_id, 0 FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ?".to_string(),
                        vec![text(&segment), text(&root), text(&execution)],
                    ),
                    (
                        "UPDATE execution_session_activity SET completed_ms = completed_ms + MAX(0, ? - ?) WHERE root_session_id = ? AND execution_id = ? AND session_id = ? AND EXISTS (SELECT 1 FROM execution_activity_segments s LEFT JOIN execution_activity_owners o ON o.owner_instance_id = s.owner_instance_id WHERE s.segment_id = ? AND s.root_session_id = ? AND s.execution_id = ? AND s.owner_instance_id = ? AND (o.owner_instance_id IS NULL OR o.heartbeat_at < ?))".to_string(),
                        vec![int(store_i64(ended.max(started))), int(store_i64(started)), text(&root), text(&execution), text(&session), text(&segment), text(&root), text(&execution), text(&owner), int(store_i64(stale_before))],
                    ),
                    (
                        "UPDATE execution_activity SET completed_ms = completed_ms + MAX(0, ? - ?), revision = revision + 1, updated = ? WHERE root_session_id = ? AND execution_id = ? AND changes() > 0 AND EXISTS (SELECT 1 FROM execution_activity_segments s LEFT JOIN execution_activity_owners o ON o.owner_instance_id = s.owner_instance_id WHERE s.segment_id = ? AND s.root_session_id = ? AND s.execution_id = ? AND s.owner_instance_id = ? AND (o.owner_instance_id IS NULL OR o.heartbeat_at < ?))".to_string(),
                        vec![int(store_i64(ended.max(started))), int(store_i64(started)), int(store_i64(ended.max(started))), text(&root), text(&execution), text(&segment), text(&root), text(&execution), text(&owner), int(store_i64(stale_before))],
                    ),
                    (
                        "DELETE FROM execution_activity_segments WHERE segment_id = ? AND root_session_id = ? AND execution_id = ? AND owner_instance_id = ? AND changes() > 0".to_string(),
                        vec![text(&segment), text(&root), text(&execution), text(&owner)],
                    ),
                ])
                .await?;
            reconciled += usize::from(results.get(1).copied().unwrap_or(0) > 0);
        }
        Ok(reconciled)
    }

    /// Read timer segments and branch lifecycle in one SQL statement so a
    /// reconnect cannot combine revisions from different database moments.
    pub(crate) async fn get_session_runtime_snapshot(
        &self,
        root_session_id: &str,
    ) -> anyhow::Result<neoism_agent_core::SessionRuntimeSnapshot> {
        let rows = self
            .db
            .fetch_all(
                r#"WITH current AS (
                       SELECT execution_id, root_session_id, root_message_id,
                              completed_ms, revision, family_revision, finished
                       FROM execution_activity WHERE root_session_id = ?
                   )
                   SELECT current.*, 0 AS row_order, 'execution' AS row_kind,
                          NULL AS item_id, NULL AS item_parent, NULL AS item_status,
                          NULL AS item_started_at, NULL AS item_completed_ms
                   FROM current
                   UNION ALL
                   SELECT current.*, 1, 'segment', seg.segment_id, seg.session_id, NULL,
                          seg.started_at, NULL
                   FROM current JOIN execution_activity_segments seg
                     ON seg.root_session_id = current.root_session_id
                    AND seg.execution_id = current.execution_id
                   UNION ALL
                   SELECT current.*, 2, 'activity', activity.session_id, NULL, NULL,
                          NULL, activity.completed_ms
                   FROM current JOIN execution_session_activity activity
                     ON activity.root_session_id = current.root_session_id
                    AND activity.execution_id = current.execution_id
                   UNION ALL
                   SELECT current.*, 3, 'task', task.child_session_id,
                          task.parent_session_id, task.status, task.started_at, NULL
                   FROM current JOIN execution_subtasks task
                     ON task.execution_id = current.execution_id
                   ORDER BY row_order, item_started_at, item_id"#,
                vec![text(root_session_id)],
            )
            .await?;
        let Some(first) = rows.first() else {
            return Ok(neoism_agent_core::SessionRuntimeSnapshot {
                root_session_id: root_session_id.to_string(),
                family_revision: 0,
                branches: Vec::new(),
                execution: None,
            });
        };
        let execution_id = first.get_str("execution_id")?;
        let mut active_segments = std::collections::BTreeMap::new();
        let mut session_activities = std::collections::BTreeMap::new();
        let mut branches = std::collections::BTreeMap::new();
        for row in &rows {
            match row.get_str("row_kind")?.as_str() {
                "segment" => {
                    let segment_id = row.get_str("item_id")?;
                    let started_at = row.get_opt_i64("item_started_at")?
                        .unwrap_or_default().max(0) as u64;
                    active_segments.insert(segment_id.clone(), started_at);
                    if let Some(session_id) = row.get_opt_str("item_parent")? {
                        session_activities
                            .entry(session_id)
                            .or_insert_with(neoism_agent_core::ProviderActivitySnapshot::default)
                            .active_segments
                            .insert(segment_id, started_at);
                    }
                }
                "activity" => {
                    let session_id = row.get_str("item_id")?;
                    session_activities
                        .entry(session_id)
                        .or_insert_with(neoism_agent_core::ProviderActivitySnapshot::default)
                        .completed_ms = row.get_opt_i64("item_completed_ms")?
                        .unwrap_or_default().max(0) as u64;
                }
                "task" => {
                    let child_session_id = row.get_str("item_id")?;
                    branches.entry(child_session_id.clone()).or_insert(
                    neoism_agent_core::SubtaskLifecycleSnapshot {
                        session_id: child_session_id,
                        parent_session_id: row
                            .get_opt_str("item_parent")?
                            .unwrap_or_default(),
                        status: row.get_opt_str("item_status")?.unwrap_or_default(),
                        started_at: row
                            .get_opt_i64("item_started_at")?
                            .map(|value| value.max(0) as u64),
                    },
                );
                }
                _ => {}
            }
        }
        Ok(neoism_agent_core::SessionRuntimeSnapshot {
            root_session_id: first.get_str("root_session_id")?,
            family_revision: first.get_i64("family_revision")?.max(0) as u64,
            branches: branches.into_values().collect(),
            execution: Some(neoism_agent_core::ExecutionActivitySnapshot {
                execution_id,
                root_session_id: first.get_str("root_session_id")?,
                root_message_id: first.get_str("root_message_id")?,
                completed_ms: first.get_i64("completed_ms")?.max(0) as u64,
                active_segments,
                session_activities,
                revision: first.get_i64("revision")?.max(0) as u64,
                finished: first.get_i64("finished")? != 0,
            }),
        })
    }

    pub(crate) async fn delete_session(&self, session_id: &str) -> anyhow::Result<bool> {
        let now = crate::now_millis();
        let mut statements = vec![
            (
                "UPDATE execution_session_activity SET completed_ms = completed_ms + COALESCE((SELECT SUM(MAX(0, ? - started_at)) FROM execution_activity_segments s WHERE s.session_id = ? AND s.root_session_id = execution_session_activity.root_session_id AND s.execution_id = execution_session_activity.execution_id), 0) WHERE session_id = ? AND EXISTS (SELECT 1 FROM execution_activity_segments s WHERE s.session_id = ? AND s.root_session_id = execution_session_activity.root_session_id AND s.execution_id = execution_session_activity.execution_id)".to_string(),
                vec![int(store_i64(now)), text(session_id), text(session_id), text(session_id)],
            ),
            (
                "UPDATE execution_activity SET completed_ms = completed_ms + COALESCE((SELECT SUM(MAX(0, ? - started_at)) FROM execution_activity_segments s WHERE s.session_id = ? AND s.root_session_id = execution_activity.root_session_id AND s.execution_id = execution_activity.execution_id), 0), revision = revision + 1, updated = ? WHERE EXISTS (SELECT 1 FROM execution_activity_segments s WHERE s.session_id = ? AND s.root_session_id = execution_activity.root_session_id AND s.execution_id = execution_activity.execution_id)".to_string(),
                vec![int(store_i64(now)), text(session_id), int(store_i64(now)), text(session_id)],
            ),
            ("DELETE FROM execution_activity_segments WHERE session_id = ?".to_string(), vec![text(session_id)]),
            ("DELETE FROM execution_session_activity WHERE session_id = ?".to_string(), vec![text(session_id)]),
            ("UPDATE execution_activity SET family_revision = family_revision + 1, updated = ? WHERE EXISTS (SELECT 1 FROM execution_subtasks t WHERE t.child_session_id = ? AND t.execution_id = execution_activity.execution_id)".to_string(), vec![int(store_i64(now)), text(session_id)]),
            ("DELETE FROM execution_subtasks WHERE child_session_id = ?".to_string(), vec![text(session_id)]),
            ("DELETE FROM execution_subtasks WHERE root_session_id = ?".to_string(), vec![text(session_id)]),
            ("DELETE FROM execution_activity_segments WHERE root_session_id = ?".to_string(), vec![text(session_id)]),
            ("DELETE FROM execution_session_activity WHERE root_session_id = ?".to_string(), vec![text(session_id)]),
            ("DELETE FROM execution_activity WHERE root_session_id = ?".to_string(), vec![text(session_id)]),
            ("DELETE FROM execution_activity_owners WHERE NOT EXISTS (SELECT 1 FROM execution_activity_segments s WHERE s.owner_instance_id = execution_activity_owners.owner_instance_id)".to_string(), Vec::new()),
        ];
        for table in ["messages", "prompt_queue", "session_runs", "session_context_epochs", "message_embeddings"] {
            statements.push((format!("DELETE FROM {table} WHERE session_id = ?"), vec![text(session_id)]));
        }
        statements.push(("DELETE FROM session_list_index WHERE session_id = ?".to_string(), vec![text(session_id)]));
        statements.push(("DELETE FROM sessions WHERE id = ?".to_string(), vec![text(session_id)]));
        let results = self.db.execute_transaction_with_results(statements).await?;
        Ok(results.last().copied().unwrap_or(0) > 0)
    }

    async fn backfill_session_list_index(&self) -> anyhow::Result<()> {
        const BATCH_SIZE: i64 = 16;
        loop {
            let migration = self
                .db
                .fetch_optional(
                    "SELECT state, cursor_updated, cursor_id FROM data_migrations WHERE name = 'session-list-index-v1'",
                    Vec::new(),
                )
                .await?;
            let Some(migration) = migration else { return Ok(()); };
            if migration.get_str("state")? == "complete" {
                return Ok(());
            }
            let cursor_updated = migration.get_opt_i64("cursor_updated")?;
            let cursor_id = migration.get_opt_str("cursor_id")?;
            let (sql, params) = match (cursor_updated, cursor_id.as_deref()) {
                (Some(updated), Some(id)) => (
                    "SELECT id, info_json, updated FROM sessions WHERE updated < ? OR (updated = ? AND id < ?) ORDER BY updated DESC, id DESC LIMIT ?",
                    vec![int(updated), int(updated), text(id), int(BATCH_SIZE)],
                ),
                _ => (
                    "SELECT id, info_json, updated FROM sessions ORDER BY updated DESC, id DESC LIMIT ?",
                    vec![int(BATCH_SIZE)],
                ),
            };
            let rows = self.db.fetch_all(sql, params).await?;
            if rows.is_empty() {
                self.db
                    .execute(
                        "UPDATE data_migrations SET state = 'complete', updated_at = ?, error = NULL WHERE name = 'session-list-index-v1'",
                        vec![int(store_i64(crate::now_millis()))],
                    )
                    .await?;
                return Ok(());
            }
            let batch_len = rows.len();
            let last_updated = rows.last().expect("non-empty batch").get_i64("updated")?;
            let last_id = rows.last().expect("non-empty batch").get_str("id")?;
            let payloads = rows
                .into_iter()
                .map(|row| Ok((row.get_str("id")?, row.get_str("info_json")?)))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let decoded = tokio::task::spawn_blocking(move || {
                payloads
                    .into_iter()
                    .map(|(id, raw)| (id, decode_json::<SessionInfo>(raw)))
                    .collect::<Vec<_>>()
            })
            .await
            .context("session list index decoder task failed")?;
            let mut statements = Vec::with_capacity(batch_len + 1);
            let mut skipped = 0usize;
            for (session_id, decoded) in decoded {
                match decoded {
                    Ok(info) => statements.push(session_list_index_statement(&info)?),
                    Err(error) => {
                        skipped += 1;
                        tracing::warn!(%session_id, %error, "skipping malformed legacy session during list-index backfill");
                    }
                }
            }
            statements.push((
                "UPDATE data_migrations SET state = 'running', cursor_updated = ?, cursor_id = ?, rows_done = rows_done + ?, updated_at = ?, error = ? WHERE name = 'session-list-index-v1'".to_string(),
                vec![
                    int(last_updated),
                    text(last_id),
                    int(batch_len as i64),
                    int(store_i64(crate::now_millis())),
                    (skipped > 0).then(|| text(format!("skipped {skipped} malformed rows"))).unwrap_or(SqlValue::Null),
                ],
            ));
            self.db.execute_transaction(statements).await?;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn list_messages(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Vec<MessageWithParts>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT message_json FROM messages WHERE session_id = ? ORDER BY position ASC, created ASC",
                vec![text(session_id)],
            )
            .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get_str("message_json")?))
            .collect()
    }

    /// Page through a session's messages without decoding the whole
    /// transcript. `cursor` is the id of a message (its info id) or of any
    /// part inside one; results are the `limit` messages immediately older
    /// than it (`order_desc`) or newer than it (ascending). The cursor and
    /// limit are pushed into SQL so "load older" reads only the page it
    /// needs instead of the entire session every time.
    pub(crate) async fn list_messages_page(
        &self,
        session_id: &str,
        cursor: Option<&str>,
        limit: Option<usize>,
        order_desc: bool,
    ) -> anyhow::Result<Vec<MessageWithParts>> {
        let cursor_position = match cursor.filter(|cursor| !cursor.is_empty()) {
            Some(cursor) => self.message_position(session_id, cursor).await?,
            None => None,
        };
        let direction = if order_desc { "DESC" } else { "ASC" };
        // For desc we want messages older than the cursor (lower position);
        // for asc, newer (higher position). A cursor we couldn't resolve
        // behaves as no cursor — same as the old in-memory path.
        let cursor_clause = match (cursor_position, order_desc) {
            (Some(_), true) => " AND position < ?",
            (Some(_), false) => " AND position > ?",
            (None, _) => "",
        };
        let limit_clause = match limit.filter(|limit| *limit > 0) {
            Some(limit) => format!(" LIMIT {}", limit as i64),
            None => String::new(),
        };
        let sql = format!(
            "SELECT message_json FROM messages WHERE session_id = ?{cursor_clause} \
             ORDER BY position {direction}, created {direction}{limit_clause}"
        );
        let mut params = vec![text(session_id)];
        if let Some(position) = cursor_position {
            params.push(int(position));
        }
        let rows = self.db.fetch_all(&sql, params).await?;
        rows.into_iter()
            .map(|row| decode_json(row.get_str("message_json")?))
            .collect()
    }

    /// Resolve a history cursor to its row `position`. Tries the message id
    /// (primary key) first, then falls back to matching a part id embedded in
    /// the message JSON — without decoding rows into structs.
    async fn message_position(
        &self,
        session_id: &str,
        cursor: &str,
    ) -> anyhow::Result<Option<i64>> {
        let by_message_id = self
            .db
            .fetch_optional(
                "SELECT position FROM messages WHERE session_id = ? AND id = ? LIMIT 1",
                vec![text(session_id), text(cursor)],
            )
            .await?;
        if let Some(row) = by_message_id {
            return Ok(Some(row.get_i64("position")?));
        }
        let pattern = format!("%\"id\":\"{}\"%", escape_like(cursor));
        let by_part_id = self
            .db
            .fetch_optional(
                "SELECT position FROM messages WHERE session_id = ? AND message_json LIKE ? ESCAPE '\\' \
                 ORDER BY position ASC LIMIT 1",
                vec![text(session_id), text(pattern)],
            )
            .await?;
        by_part_id.map(|row| row.get_i64("position")).transpose()
    }

    pub(crate) async fn append_message(
        &self,
        session_id: &str,
        message: &MessageWithParts,
    ) -> anyhow::Result<()> {
        let position = self
            .db
            .fetch_scalar_i64(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM messages WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        self.db
            .execute(
                "INSERT INTO messages (id, session_id, message_json, created, position) VALUES (?, ?, ?, ?, ?)",
                vec![
                    text(message_id(message)),
                    text(session_id),
                    text(serde_json::to_string(message)?),
                    int(store_i64(message_created(message))),
                    int(position),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn get_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> anyhow::Result<Option<MessageWithParts>> {
        let row = self
            .db
            .fetch_optional(
                "SELECT message_json FROM messages WHERE session_id = ? AND id = ?",
                vec![text(session_id), text(message_id)],
            )
            .await?;
        row.map(|row| decode_json(row.get_str("message_json")?))
            .transpose()
    }

    pub(crate) async fn update_message(
        &self,
        session_id: &str,
        message: &MessageWithParts,
    ) -> anyhow::Result<bool> {
        let affected = self
            .db
            .execute(
                "UPDATE messages SET message_json = ? WHERE session_id = ? AND id = ?",
                vec![
                    text(serde_json::to_string(message)?),
                    text(session_id),
                    text(message_id(message)),
                ],
            )
            .await?;
        if affected > 0 {
            // Drop the stale embedding so the semantic indexer re-embeds the
            // edited content once the session goes quiet.
            self.db
                .execute(
                    "DELETE FROM message_embeddings WHERE message_id = ?",
                    vec![text(message_id(message))],
                )
                .await?;
        }
        Ok(affected > 0)
    }

    pub(crate) async fn delete_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> anyhow::Result<bool> {
        let affected = self
            .db
            .execute(
                "DELETE FROM messages WHERE session_id = ? AND id = ?",
                vec![text(session_id), text(message_id)],
            )
            .await?;
        self.db
            .execute(
                "DELETE FROM message_embeddings WHERE message_id = ?",
                vec![text(message_id)],
            )
            .await?;
        Ok(affected > 0)
    }

    /// Remove every transcript message for a session. Used by session import to
    /// make re-importing a bundle idempotent (the prior transcript is replaced
    /// rather than appended to).
    pub(crate) async fn delete_session_messages(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let affected = self
            .db
            .execute(
                "DELETE FROM messages WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        self.db
            .execute(
                "DELETE FROM message_embeddings WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        Ok(affected as usize)
    }

    pub(crate) async fn list_permission_approvals(
        &self,
    ) -> anyhow::Result<HashMap<String, Vec<PermissionRule>>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT project_id, rules_json FROM permission_approvals",
                Vec::new(),
            )
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.get_str("project_id")?,
                    decode_json(row.get_str("rules_json")?)?,
                ))
            })
            .collect()
    }

    pub(crate) async fn save_permission_approvals(
        &self,
        project_id: &str,
        rules: &[PermissionRule],
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            INSERT INTO permission_approvals (project_id, rules_json, updated)
            VALUES (?, ?, ?)
            ON CONFLICT(project_id) DO UPDATE SET
                rules_json = excluded.rules_json,
                updated = excluded.updated
            "#,
                vec![
                    text(project_id),
                    text(serde_json::to_string(rules)?),
                    int(store_i64(crate::now_millis())),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn enqueue_prompt_with_delivery(
        &self,
        session_id: &str,
        request: &PromptRequest,
        delivery: &str,
    ) -> anyhow::Result<usize> {
        if !matches!(delivery, "steer" | "queue" | "continue") {
            anyhow::bail!("unsupported prompt delivery {delivery}");
        }
        let position = self
            .db
            .fetch_scalar_i64(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM prompt_queue WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        self.db
            .execute(
                "INSERT INTO prompt_queue (id, session_id, position, request_json, created, delivery) VALUES (?, ?, ?, ?, ?, ?)",
                vec![
                    text(Id::ascending(IdKind::Event).to_string()),
                    text(session_id),
                    int(position),
                    text(serde_json::to_string(request)?),
                    int(store_i64(crate::now_millis())),
                    text(delivery),
                ],
            )
            .await?;
        self.queued_prompt_count(session_id).await
    }

    pub(crate) async fn list_queued_prompt_entries(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Vec<(PromptRequest, String)>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT request_json, delivery FROM prompt_queue WHERE session_id = ? ORDER BY position ASC, created ASC",
                vec![text(session_id)],
            )
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    decode_json(row.get_str("request_json")?)?,
                    row.get_str("delivery")?,
                ))
            })
            .collect()
    }

    pub(crate) async fn queued_prompt_count(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let count = self
            .db
            .fetch_scalar_i64(
                "SELECT COUNT(*) FROM prompt_queue WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        Ok(count.max(0) as usize)
    }

    pub(crate) async fn user_queued_prompt_count(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let count = self
            .db
            .fetch_scalar_i64(
                "SELECT COUNT(*) FROM prompt_queue WHERE session_id = ? AND delivery != 'continue'",
                vec![text(session_id)],
            )
            .await?;
        Ok(count.max(0) as usize)
    }

    pub(crate) async fn pop_user_queued_prompt(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<PromptRequest>> {
        let Some(row) = self
            .db
            .fetch_optional(
                "SELECT id, request_json FROM prompt_queue WHERE session_id = ? AND delivery != 'continue' ORDER BY position ASC, created ASC LIMIT 1",
                vec![text(session_id)],
            )
            .await?
        else {
            return Ok(None);
        };
        let id = row.get_str("id")?;
        let request = decode_json(row.get_str("request_json")?)?;
        self.db
            .execute("DELETE FROM prompt_queue WHERE id = ?", vec![text(id)])
            .await?;
        Ok(Some(request))
    }

    pub(crate) async fn pop_active_continuation_prompt(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Option<PromptRequest>> {
        let Some(row) = self
            .db
            .fetch_optional(
                "SELECT id, request_json FROM prompt_queue WHERE session_id = ? AND delivery IN ('steer', 'continue') ORDER BY position ASC, created ASC LIMIT 1",
                vec![text(session_id)],
            )
            .await?
        else {
            return Ok(None);
        };
        let id = row.get_str("id")?;
        let request = decode_json(row.get_str("request_json")?)?;
        self.db
            .execute("DELETE FROM prompt_queue WHERE id = ?", vec![text(id)])
            .await?;
        Ok(Some(request))
    }

    pub(crate) async fn pop_queued_prompt_with_delivery(
        &self,
        session_id: &str,
        delivery: Option<&str>,
    ) -> anyhow::Result<Option<(PromptRequest, String)>> {
        let (sql, parameters) = if let Some(delivery) = delivery {
            (
                "SELECT id, request_json, delivery FROM prompt_queue WHERE session_id = ? AND delivery = ? ORDER BY position ASC, created ASC LIMIT 1",
                vec![text(session_id), text(delivery)],
            )
        } else {
            (
                "SELECT id, request_json, delivery FROM prompt_queue WHERE session_id = ? ORDER BY position ASC, created ASC LIMIT 1",
                vec![text(session_id)],
            )
        };
        let Some(row) = self.db.fetch_optional(sql, parameters).await? else {
            return Ok(None);
        };
        let id = row.get_str("id")?;
        let request = decode_json(row.get_str("request_json")?)?;
        let delivery = row.get_str("delivery")?;
        self.db
            .execute("DELETE FROM prompt_queue WHERE id = ?", vec![text(id)])
            .await?;
        Ok(Some((request, delivery)))
    }

    /// Put a popped prompt back at the FRONT of the queue, preserving its
    /// delivery tag. Used when a drain worker pops a prompt but a run
    /// claims the session before the prompt can be appended — the prompt
    /// must survive (durably) and go out first once the run finishes.
    pub(crate) async fn requeue_prompt_front_with_delivery(
        &self,
        session_id: &str,
        request: &PromptRequest,
        delivery: &str,
    ) -> anyhow::Result<usize> {
        if !matches!(delivery, "steer" | "queue" | "continue") {
            anyhow::bail!("unsupported prompt delivery {delivery}");
        }
        let position = self
            .db
            .fetch_scalar_i64(
                "SELECT COALESCE(MIN(position), 1) - 1 FROM prompt_queue WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        self.db
            .execute(
                "INSERT INTO prompt_queue (id, session_id, position, request_json, created, delivery) VALUES (?, ?, ?, ?, ?, ?)",
                vec![
                    text(Id::ascending(IdKind::Event).to_string()),
                    text(session_id),
                    int(position),
                    text(serde_json::to_string(request)?),
                    int(store_i64(crate::now_millis())),
                    text(delivery),
                ],
            )
            .await?;
        self.queued_prompt_count(session_id).await
    }

    pub(crate) async fn clear_queued_prompts(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let affected = self
            .db
            .execute(
                "DELETE FROM prompt_queue WHERE session_id = ?",
                vec![text(session_id)],
            )
            .await?;
        Ok(affected as usize)
    }

    pub(crate) async fn clear_user_queued_prompts(
        &self,
        session_id: &str,
    ) -> anyhow::Result<usize> {
        let affected = self
            .db
            .execute(
                "DELETE FROM prompt_queue WHERE session_id = ? AND delivery != 'continue'",
                vec![text(session_id)],
            )
            .await?;
        Ok(affected as usize)
    }

    pub(crate) async fn queued_session_ids(&self) -> anyhow::Result<Vec<String>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT DISTINCT session_id FROM prompt_queue ORDER BY session_id",
                Vec::new(),
            )
            .await?;
        rows.into_iter()
            .map(|row| row.get_str("session_id"))
            .collect()
    }

    pub(crate) async fn append_event(&self, event: &EventPayload) -> anyhow::Result<()> {
        self.append_event_with_owner(event, None).await
    }

    pub(crate) async fn append_event_with_owner(
        &self,
        event: &EventPayload,
        owner_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let aggregate_id = crate::sync::aggregate_id(event);
        let session_id = event
            .properties
            .get("sessionID")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.db
            .execute_transaction(event_statements(
                event,
                &aggregate_id,
                session_id,
                owner_id,
            )?)
            .await?;
        Ok(())
    }

    pub(crate) async fn list_events_after(
        &self,
        since: i64,
        limit: usize,
        session_id: Option<&str>,
    ) -> anyhow::Result<Vec<PersistedEvent>> {
        let limit = limit.clamp(1, 5_000) as i64;
        let rows = if let Some(session_id) = session_id {
            self.db
                .fetch_all(
                    "SELECT seq, created, event_json FROM events WHERE seq > ? AND session_id = ? ORDER BY seq ASC LIMIT ?",
                    vec![int(since), text(session_id), int(limit)],
                )
                .await?
        } else {
            self.db
                .fetch_all(
                    "SELECT seq, created, event_json FROM events WHERE seq > ? ORDER BY seq ASC LIMIT ?",
                    vec![int(since), int(limit)],
                )
                .await?
        };
        rows.into_iter()
            .map(|row| {
                Ok(PersistedEvent {
                    seq: row.get_i64("seq")?,
                    created: row.get_i64("created")?,
                    payload: decode_json(row.get_str("event_json")?)?,
                })
            })
            .collect()
    }

    pub(crate) async fn latest_event_sequence(&self) -> anyhow::Result<u64> {
        Ok(self
            .db
            .fetch_scalar_i64("SELECT COALESCE(MAX(seq), 0) FROM events", vec![])
            .await?
            .max(0) as u64)
    }

    pub(crate) async fn insert_artifact(
        &self,
        artifact: &ArtifactInfo,
        tenant_id: &str,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                "INSERT INTO artifacts (id, filename, media_type, size, sha256, session_id, tenant_id, created) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                vec![
                    text(&artifact.id),
                    text(&artifact.filename),
                    text(&artifact.media_type),
                    int(i64::try_from(artifact.size).unwrap_or(i64::MAX)),
                    text(&artifact.sha256),
                    opt_text(artifact.session_id.clone()),
                    text(tenant_id),
                    int(store_i64(artifact.created)),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn get_artifact(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<ArtifactInfo>> {
        self.db
            .fetch_optional(
                "SELECT id, filename, media_type, size, sha256, session_id, created FROM artifacts WHERE id = ?",
                vec![text(id)],
            )
            .await?
            .map(artifact_from_row)
            .transpose()
    }

    pub(crate) async fn list_artifacts(
        &self,
        session_id: Option<&str>,
        tenant_id: Option<&str>,
    ) -> anyhow::Result<Vec<ArtifactInfo>> {
        let (sql, params) = match (session_id, tenant_id) {
            (Some(session_id), Some(tenant_id)) => (
                "SELECT id, filename, media_type, size, sha256, session_id, created FROM artifacts WHERE session_id = ? AND tenant_id = ? ORDER BY created DESC",
                vec![text(session_id), text(tenant_id)],
            ),
            (Some(session_id), None) => (
                "SELECT id, filename, media_type, size, sha256, session_id, created FROM artifacts WHERE session_id = ? ORDER BY created DESC",
                vec![text(session_id)],
            ),
            (None, Some(tenant_id)) => (
                "SELECT id, filename, media_type, size, sha256, session_id, created FROM artifacts WHERE tenant_id = ? ORDER BY created DESC",
                vec![text(tenant_id)],
            ),
            (None, None) => (
                "SELECT id, filename, media_type, size, sha256, session_id, created FROM artifacts ORDER BY created DESC",
                Vec::new(),
            ),
        };
        let rows = self.db.fetch_all(sql, params).await?;
        rows.into_iter().map(artifact_from_row).collect()
    }

    pub(crate) async fn artifact_tenant(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<String>> {
        let row = self
            .db
            .fetch_optional(
                "SELECT tenant_id FROM artifacts WHERE id = ?",
                vec![text(id)],
            )
            .await?;
        row.map(|row| row.get_str("tenant_id")).transpose()
    }

    pub(crate) async fn delete_artifact(&self, id: &str) -> anyhow::Result<()> {
        self.db
            .execute("DELETE FROM artifacts WHERE id = ?", vec![text(id)])
            .await?;
        Ok(())
    }

    pub(crate) async fn append_audit(&self, entry: &AuditEntry) -> anyhow::Result<()> {
        self.db
            .execute(
                "INSERT INTO audit_log (id, tenant_id, subject, method, path, status, created) VALUES (?, ?, ?, ?, ?, ?, ?)",
                vec![
                    text(&entry.id),
                    text(&entry.tenant_id),
                    entry.subject.as_deref().map(text).unwrap_or(SqlValue::Null),
                    text(&entry.method),
                    text(&entry.path),
                    int(entry.status.into()),
                    int(store_i64(entry.created)),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn list_audit(
        &self,
        tenant_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<AuditEntry>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT id, tenant_id, subject, method, path, status, created FROM audit_log WHERE tenant_id = ? ORDER BY created DESC LIMIT ?",
                vec![text(tenant_id), int(limit.min(1000) as i64)],
            )
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(AuditEntry {
                    id: row.get_str("id")?,
                    tenant_id: row.get_str("tenant_id")?,
                    subject: row.get_opt_str("subject")?,
                    method: row.get_str("method")?,
                    path: row.get_str("path")?,
                    status: row.get_i64("status")?.clamp(0, u16::MAX as i64) as u16,
                    created: row.get_i64("created")?.max(0) as u64,
                })
            })
            .collect()
    }

    pub(crate) async fn save_permission_request(
        &self,
        request: &PermissionRequestInfo,
    ) -> anyhow::Result<()> {
        self.save_interaction_request(
            &request.id,
            "permission",
            &request.session_id,
            serde_json::to_string(request)?,
        )
        .await
    }

    pub(crate) async fn save_question_request(
        &self,
        request: &QuestionRequestInfo,
    ) -> anyhow::Result<()> {
        self.save_interaction_request(
            &request.id,
            "question",
            &request.session_id,
            serde_json::to_string(request)?,
        )
        .await
    }

    async fn save_interaction_request(
        &self,
        id: &str,
        kind: &str,
        session_id: &str,
        payload_json: String,
    ) -> anyhow::Result<()> {
        let now = store_i64(crate::now_millis());
        self.db
            .execute(
                r#"
                INSERT INTO interaction_requests (id, kind, session_id, payload_json, state, response_json, created, updated)
                VALUES (?, ?, ?, ?, 'pending', NULL, ?, ?)
                ON CONFLICT(id) DO NOTHING
                "#,
                vec![
                    text(id),
                    text(kind),
                    text(session_id),
                    text(payload_json),
                    int(now),
                    int(now),
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn resolve_interaction(
        &self,
        id: &str,
        state: &str,
        response: Value,
    ) -> anyhow::Result<bool> {
        let affected = self
            .db
            .execute(
                "UPDATE interaction_requests SET state = ?, response_json = ?, updated = ? WHERE id = ? AND state = 'pending'",
                vec![
                    text(state),
                    text(response.to_string()),
                    int(store_i64(crate::now_millis())),
                    text(id),
                ],
            )
            .await?;
        Ok(affected > 0)
    }

    pub(crate) async fn cancel_session_interactions(
        &self,
        session_id: &str,
    ) -> anyhow::Result<u64> {
        self.db
            .execute(
                "UPDATE interaction_requests SET state = 'cancelled', response_json = ?, updated = ? WHERE session_id = ? AND state = 'pending'",
                vec![
                    text(json!({ "reason": "session cancelled" }).to_string()),
                    int(store_i64(crate::now_millis())),
                    text(session_id),
                ],
            )
            .await
    }

    pub(crate) async fn interaction_session_id(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<String>> {
        let row = self
            .db
            .fetch_optional(
                "SELECT session_id FROM interaction_requests WHERE id = ?",
                vec![text(request_id)],
            )
            .await?;
        row.map(|row| row.get_str("session_id")).transpose()
    }

    pub(crate) async fn list_pending_permissions(
        &self,
    ) -> anyhow::Result<Vec<PermissionRequestInfo>> {
        self.list_pending_interactions("permission").await
    }

    pub(crate) async fn list_pending_questions(
        &self,
    ) -> anyhow::Result<Vec<QuestionRequestInfo>> {
        self.list_pending_interactions("question").await
    }

    async fn list_pending_interactions<T: DeserializeOwned>(
        &self,
        kind: &str,
    ) -> anyhow::Result<Vec<T>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT payload_json FROM interaction_requests WHERE kind = ? AND state = 'pending' ORDER BY created ASC",
                vec![text(kind)],
            )
            .await?;
        rows.into_iter()
            .map(|row| decode_json(row.get_str("payload_json")?))
            .collect()
    }

    pub(crate) async fn start_run(
        &self,
        run_id: &str,
        session_id: &str,
    ) -> anyhow::Result<()> {
        let now = store_i64(crate::now_millis());
        self.db
            .execute(
                r#"
            INSERT INTO session_runs (id, session_id, status, created, updated, error_json)
            VALUES (?, ?, 'running', ?, ?, NULL)
            ON CONFLICT(id) DO UPDATE SET
                status = 'running',
                updated = excluded.updated,
                error_json = NULL
            "#,
                vec![text(run_id), text(session_id), int(now), int(now)],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn finish_run(
        &self,
        run_id: &str,
        status: &str,
        error: Option<Value>,
    ) -> anyhow::Result<()> {
        self.db
            .execute(
                r#"
            UPDATE session_runs
            SET status = ?, updated = ?, error_json = ?
            WHERE id = ?
            "#,
                vec![
                    text(status),
                    int(store_i64(crate::now_millis())),
                    opt_text(error.map(|value| value.to_string())),
                    text(run_id),
                ],
            )
            .await?;
        Ok(())
    }

    /// Interrupt store run rows for sessions that provably have no live run.
    /// Callers must hold the family's execution keyed lock and have verified
    /// the coordinator (the sole in-memory authority) shows no active run for
    /// any of these sessions — run starts serialize on the same lock, so a
    /// 'running'/'retry' row here is a leak from an interrupted teardown.
    pub(crate) async fn interrupt_abandoned_runs(
        &self,
        session_ids: &[String],
    ) -> anyhow::Result<u64> {
        if session_ids.is_empty() {
            return Ok(0);
        }
        let placeholders = vec!["?"; session_ids.len()].join(", ");
        let mut params = vec![
            int(store_i64(crate::now_millis())),
            text(
                json!({ "message": "Run abandoned without teardown; reconciled at quiescence" })
                    .to_string(),
            ),
        ];
        params.extend(session_ids.iter().map(|session_id| text(session_id.clone())));
        let affected = self
            .db
            .execute(
                &format!(
                    "UPDATE session_runs \
                     SET status = 'interrupted', updated = ?, error_json = ? \
                     WHERE status IN ('running', 'retry') AND session_id IN ({placeholders})"
                ),
                params,
            )
            .await?;
        Ok(affected)
    }

    #[cfg(test)]
    pub(crate) async fn session_run_statuses(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT id, status FROM session_runs WHERE session_id = ? ORDER BY created",
                vec![text(session_id)],
            )
            .await?;
        rows.into_iter()
            .map(|row| Ok((row.get_str("id")?, row.get_str("status")?)))
            .collect()
    }

    pub(crate) async fn interrupt_stale_runs(&self) -> anyhow::Result<u64> {
        let affected = self
            .db
            .execute(
                r#"
            UPDATE session_runs
            SET status = 'interrupted',
                updated = ?,
                error_json = ?
            WHERE status IN ('running', 'retry')
            "#,
                vec![
                    int(store_i64(crate::now_millis())),
                    text(
                        json!({ "message": "Server restarted before run completed" })
                            .to_string(),
                    ),
                ],
            )
            .await?;
        Ok(affected)
    }

    #[cfg(test)]
    pub(crate) async fn close(&self) {}
}

fn artifact_from_row(row: DbRow) -> anyhow::Result<ArtifactInfo> {
    Ok(ArtifactInfo {
        id: row.get_str("id")?,
        filename: row.get_str("filename")?,
        media_type: row.get_str("media_type")?,
        size: row.get_i64("size")?.max(0) as u64,
        sha256: row.get_str("sha256")?,
        session_id: row.get_opt_str("session_id")?,
        created: row.get_i64("created")?.max(0) as u64,
        download_url: String::new(),
    })
}

fn decode_json<T: DeserializeOwned>(raw: String) -> anyhow::Result<T> {
    serde_json::from_str(&raw).context("failed to decode persisted JSON")
}

fn decode_workflow_projection(
    row: DbRow,
) -> anyhow::Result<crate::workflow::WorkflowProjection> {
    Ok(crate::workflow::WorkflowProjection {
        activation_id: row.get_str("activation_id")?,
        workflow_id: row.get_str("workflow_id")?,
        workspace_root: row.get_str("workspace_root")?,
        source_path: row.get_str("source_path")?,
        source_hash: row.get_str("source_hash")?,
        definition: decode_json(row.get_str("definition_json")?)?,
        active: row.get_i64("active")? != 0,
        activated_at: row.get_i64("activated_at")?.max(0) as u64,
        last_scheduled_at: row
            .get_opt_i64("last_scheduled_at")?
            .map(|v| v.max(0) as u64),
        updated: row.get_i64("updated")?.max(0) as u64,
    })
}

fn decode_workflow_run(row: DbRow) -> anyhow::Result<crate::workflow::WorkflowRun> {
    Ok(crate::workflow::WorkflowRun {
        id: row.get_str("id")?,
        activation_id: row.get_str("activation_id")?,
        workflow_id: row.get_str("workflow_id")?,
        scheduled_at: row.get_i64("scheduled_at")?.max(0) as u64,
        started_at: row.get_opt_i64("started_at")?.map(|v| v.max(0) as u64),
        finished_at: row.get_opt_i64("finished_at")?.map(|v| v.max(0) as u64),
        session_id: row.get_opt_str("session_id")?,
        status: row.get_str("status")?,
        trigger: row.get_str("trigger")?,
        error: row.get_opt_str("error")?,
        created: row.get_i64("created")?.max(0) as u64,
    })
}

fn message_id(message: &MessageWithParts) -> String {
    match &message.info {
        MessageInfo::User(message) => message.id.to_string(),
        MessageInfo::Assistant(message) => message.id.to_string(),
    }
}

fn message_created(message: &MessageWithParts) -> u64 {
    match &message.info {
        MessageInfo::User(message) => message.time.created,
        MessageInfo::Assistant(message) => message.time.created,
    }
}

fn store_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

/// One row of `search_messages` output: where the match lives plus an excerpt
/// with `>>`/`<<` markers around the first matched term.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct MessageSearchHit {
    pub(crate) session_id: String,
    pub(crate) message_id: String,
    pub(crate) role: String,
    pub(crate) created: u64,
    pub(crate) excerpt: String,
}

/// A message the semantic indexer still needs to embed.
#[derive(Debug, Clone)]
pub(crate) struct PendingEmbedding {
    pub(crate) session_id: String,
    pub(crate) message_id: String,
    pub(crate) created: i64,
    pub(crate) message_json: String,
}

/// One semantic search result; `distance` is cosine distance (lower = closer).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SemanticSearchHit {
    pub(crate) session_id: String,
    pub(crate) message_id: String,
    pub(crate) role: String,
    pub(crate) created: u64,
    pub(crate) excerpt: String,
    pub(crate) distance: f64,
}

/// Flatten a message into `(role, created, searchable text)` for transcript
/// search and the semantic embedding indexer. Parts are inspected as JSON so
/// new part variants degrade to "not indexed" instead of breaking
/// compilation or persistence.
pub(crate) fn search_document(message: &MessageWithParts) -> (String, u64, String) {
    let role = match &message.info {
        MessageInfo::User(_) => "user",
        MessageInfo::Assistant(_) => "assistant",
    };
    let created = message_created(message);
    let mut chunks: Vec<String> = Vec::new();
    for part in &message.parts {
        let Ok(value) = serde_json::to_value(part) else {
            continue;
        };
        if let Some(text) = value.get("text").and_then(|text| text.as_str()) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                chunks.push(trimmed.to_string());
            }
        }
        // Tool outputs live under state.output on completed tool parts.
        // Cap their contribution so one huge output can't bloat the index.
        if let Some(output) = value
            .get("state")
            .and_then(|state| state.get("output"))
            .and_then(|output| output.as_str())
        {
            let trimmed = output.trim();
            if !trimmed.is_empty() {
                chunks.push(trimmed.chars().take(2000).collect());
            }
        }
    }
    (role.to_string(), created, chunks.join("\n"))
}

/// Escape a value for use inside a SQL `LIKE` pattern (with `ESCAPE '\'`).
/// Ids contain `_`, which is a `LIKE` wildcard, so it must be escaped or
/// `prt_abc` would also match `prtXabc`.
fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// ASCII-case-insensitive substring search. A match can only start on a
/// UTF-8 char boundary (a valid needle never begins with a continuation
/// byte), so the returned byte offset is safe to slice with.
fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Build a `>>match<<` excerpt around a match for the search tool and UI.
fn like_excerpt(content: &str, start: usize, len: usize) -> String {
    const CONTEXT: usize = 90;
    let end = (start + len).min(content.len());
    let mut window_start = start.saturating_sub(CONTEXT);
    while !content.is_char_boundary(window_start) {
        window_start -= 1;
    }
    let mut window_end = (end + CONTEXT).min(content.len());
    while !content.is_char_boundary(window_end) {
        window_end += 1;
    }
    let mut excerpt = String::new();
    if window_start > 0 {
        excerpt.push_str(" ... ");
    }
    excerpt.push_str(&content[window_start..start]);
    excerpt.push_str(">>");
    excerpt.push_str(&content[start..end]);
    excerpt.push_str("<<");
    excerpt.push_str(&content[end..window_end]);
    if window_end < content.len() {
        excerpt.push_str(" ... ");
    }
    excerpt.replace('\n', " ")
}
