use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path as FsPath, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{
    DateTime, Datelike, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
};
use chrono_tz::Tz;
use croner::{errors::CronError, Cron};
use neoism_agent_core::{
    event_type, CreateSessionRequest, EventPayload, Id, IdKind, ModelRef,
    PermissionAction, PermissionRule, PromptPart, PromptRequest,
};
use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::state::AppState;
use crate::workflow_action::{self, DirectAction, ValidatedDirectAction};
use crate::{now_millis, resolve_directory};

const PREVIEW_COUNT: usize = 10;
const WATCH_DEBOUNCE: Duration = Duration::from_millis(150);
const CLOCK_SAFETY_WAKE: Duration = Duration::from_secs(60 * 60);
const SCHEDULED_SHELL: &str = "scheduledShell";
const SCHEDULED_COMPOSE: &str = "scheduledCompose";
const MANUAL_COMPOSE: &str = "manualCompose";

fn scheduled_direct_trigger(trigger: &str) -> bool {
    matches!(trigger, SCHEDULED_SHELL | SCHEDULED_COMPOSE)
}

#[path = "workflow_mcp_events.rs"]
mod mcp_event_trigger;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkflowDefinition {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) active: bool,
    pub(crate) schedule: WorkflowSchedule,
    pub(crate) prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) action: Option<DirectAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) skill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) permissions: Option<BTreeMap<String, WorkflowPermission>>,
    #[serde(default)]
    pub(crate) retry: WorkflowRetryPolicy,
    #[serde(default)]
    pub(crate) concurrency: WorkflowConcurrencyPolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkflowRetryPolicy {
    #[serde(default = "one")]
    pub(crate) max_attempts: u32,
    #[serde(default)]
    pub(crate) backoff: WorkflowBackoff,
    #[serde(default)]
    pub(crate) initial_delay_ms: u64,
    #[serde(default)]
    pub(crate) max_delay_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) retryable_errors: Vec<String>,
}

impl Default for WorkflowRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff: WorkflowBackoff::Fixed,
            initial_delay_ms: 0,
            max_delay_ms: 0,
            retryable_errors: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WorkflowBackoff {
    #[default]
    Fixed,
    Exponential,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkflowConcurrencyPolicy {
    #[serde(default)]
    pub(crate) mode: WorkflowConcurrencyMode,
    #[serde(default = "one")]
    pub(crate) max_running: u32,
}

impl Default for WorkflowConcurrencyPolicy {
    fn default() -> Self {
        Self {
            mode: WorkflowConcurrencyMode::Forbid,
            max_running: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WorkflowConcurrencyMode {
    #[default]
    Forbid,
    Replace,
    Allow,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkflowDefinitionPatch {
    id: Option<String>,
    name: Option<String>,
    active: Option<bool>,
    schedule: Option<WorkflowSchedule>,
    prompt: Option<String>,
    directory: Option<Option<String>>,
    skill: Option<Option<String>>,
    agent: Option<Option<String>>,
    model: Option<Option<ModelRef>>,
    permissions: Option<Option<BTreeMap<String, WorkflowPermission>>>,
    retry: Option<WorkflowRetryPolicy>,
    concurrency: Option<WorkflowConcurrencyPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkflowSchedule {
    #[serde(default = "once")]
    pub(crate) frequency: String,
    #[serde(default = "one")]
    pub(crate) interval: u32,
    #[serde(default = "utc")]
    pub(crate) timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) minute: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) time: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) weekdays: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) month_day: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cron: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum WorkflowPermission {
    Action(PermissionAction),
    Rules(WorkflowPermissionRules),
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowPermissionRules {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<PermissionAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ask: Vec<String>,
}

impl WorkflowDefinition {
    fn permission_rules(&self) -> Option<Vec<PermissionRule>> {
        let permissions = self.permissions.as_ref()?;
        let mut rules = Vec::new();
        for (permission, setting) in permissions {
            match setting {
                WorkflowPermission::Action(action) => rules.push(PermissionRule {
                    permission: permission.clone(),
                    pattern: "*".to_string(),
                    action: *action,
                }),
                WorkflowPermission::Rules(setting) => {
                    if let Some(action) = setting.default {
                        rules.push(PermissionRule {
                            permission: permission.clone(),
                            pattern: "*".to_string(),
                            action,
                        });
                    }
                    for (action, patterns) in [
                        (PermissionAction::Ask, &setting.ask),
                        (PermissionAction::Allow, &setting.allow),
                        (PermissionAction::Deny, &setting.deny),
                    ] {
                        rules.extend(patterns.iter().map(|pattern| PermissionRule {
                            permission: permission.clone(),
                            pattern: pattern.clone(),
                            action,
                        }));
                    }
                }
            }
        }
        Some(rules)
    }

    fn effective_permission_rules(&self) -> Option<Vec<PermissionRule>> {
        self.permission_rules()
    }
}

fn one() -> u32 {
    1
}
fn utc() -> String {
    "UTC".to_string()
}
fn once() -> String {
    "once".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowProjection {
    pub(crate) activation_id: String,
    pub(crate) workflow_id: String,
    pub(crate) workspace_root: String,
    pub(crate) source_path: String,
    pub(crate) source_hash: String,
    pub(crate) definition: WorkflowDefinition,
    pub(crate) active: bool,
    pub(crate) activated_at: u64,
    pub(crate) last_scheduled_at: Option<u64>,
    pub(crate) updated: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowRun {
    pub(crate) id: String,
    pub(crate) activation_id: String,
    pub(crate) workflow_id: String,
    pub(crate) scheduled_at: u64,
    pub(crate) started_at: Option<u64>,
    pub(crate) finished_at: Option<u64>,
    pub(crate) session_id: Option<String>,
    pub(crate) status: String,
    pub(crate) trigger: String,
    pub(crate) error: Option<String>,
    pub(crate) created: u64,
    pub(crate) attempt: u32,
    pub(crate) root_run_id: String,
    pub(crate) retry_of: Option<String>,
    pub(crate) next_attempt_at: Option<u64>,
    pub(crate) lease_owner: Option<String>,
    pub(crate) lease_expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) shell_result: Option<WorkflowShellResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) compose_result: Option<WorkflowComposeResult>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowComposeResult {
    pub(crate) value: Value,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowShellResult {
    pub(crate) exit_code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowSource {
    pub(crate) definition: WorkflowDefinition,
    pub(crate) source_path: String,
    pub(crate) source_hash: String,
    #[serde(skip)]
    pub(crate) local_origin: Option<String>,
}

pub(crate) const RUNE_SCHEDULED_PROMPT: &str = "rune-scheduled";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowDiagnostic {
    source_path: String,
    message: String,
}

#[derive(Default)]
struct WorkflowCatalog {
    workflows: BTreeMap<String, WorkflowSource>,
    diagnostics: Vec<WorkflowDiagnostic>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WorkflowScope {
    Installation,
    #[default]
    Workspace,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct WorkflowQuery {
    pub directory: Option<String>,
    #[serde(default)]
    pub scope: WorkflowScope,
}

// A persisted, scope-qualified identity, never an execution directory. This keeps
// global activations/history distinct from all workspace activations of the same id.
const INSTALLATION_CONTEXT_PREFIX: &str =
    neoism_agent_service_api::INSTALLATION_CONTEXT_PREFIX;

pub(crate) fn installation_context(
    services: &neoism_agent_service_api::AgentServices,
) -> Result<String, ApiError> {
    let snapshot = services
        .config
        .snapshot(&neoism_agent_service_api::ConfigSnapshotRequest::installation())
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let root = snapshot
        .discovery_roots
        .into_iter()
        .find(|root| {
            root.scope == neoism_agent_service_api::ConfigDiscoveryScope::Installation
        })
        .ok_or_else(|| {
            ApiError::bad_request("No installation discovery root is configured")
        })?;
    if !root.path.is_absolute() {
        return Err(ApiError::bad_request(
            "Installation discovery root must be absolute",
        ));
    }
    Ok(format!(
        "{INSTALLATION_CONTEXT_PREFIX}{}",
        root.path.display()
    ))
}

fn workflow_context(
    state: &AppState,
    directory: Option<String>,
    scope: WorkflowScope,
    headers: &HeaderMap,
) -> Result<String, ApiError> {
    match scope {
        WorkflowScope::Installation => installation_context(state.services()),
        WorkflowScope::Workspace => workspace_root(resolve_directory(directory, headers)),
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WorkflowHistoryQuery {
    directory: Option<String>,
    #[serde(default)]
    scope: WorkflowScope,
    limit: Option<usize>,
}

pub(crate) fn agent_tool_available(
    state: &AppState,
    snapshot: &crate::workspace_runtime::PluginGenerationLease,
    execution: &neoism_agent_service_api::ExecutionPolicy,
) -> bool {
    !state.services().hosted
        && crate::caller::native_execution_allowed(execution)
        && snapshot
            .manifests
            .iter()
            .any(|plugin| plugin.id == neoism_agent_builtins::plugin::workflows::ID)
}

pub(crate) async fn agent_tool(
    state: &AppState,
    directory: &str,
    permissions: &[PermissionRule],
    input: Value,
) -> Result<crate::tool::ToolExecutionResult, String> {
    #[derive(Deserialize)]
    #[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
    enum Request {
        List,
        Get { id: String },
        Preview { id: String },
        History { id: String },
        Activate { id: String, revision: String },
        Pause { id: String },
        Run { id: String, revision: String },
        Draft { definition: Box<WorkflowDefinition> },
    }

    crate::ensure_tool_permission(permissions, "workflow", "*")?;
    let request: Request = serde_json::from_value(input).map_err(|error| error.to_string())?;
    let query = WorkflowQuery {
        directory: Some(directory.to_owned()),
        scope: WorkflowScope::Workspace,
    };
    let headers = HeaderMap::new();
    let result = match request {
        Request::List => workflow_list(State(state.clone()), Query(query), headers).await,
        Request::Get { id } => {
            workflow_get(State(state.clone()), Query(query), headers, Path(id)).await
        }
        Request::Preview { id } => {
            workflow_preview(State(state.clone()), Query(query), headers, Path(id)).await
        }
        Request::History { id } => {
            let history = WorkflowHistoryQuery {
                directory: query.directory,
                scope: query.scope,
                limit: Some(20),
            };
            workflow_history(State(state.clone()), Query(history), headers, Path(id)).await
        }
        Request::Pause { id } => {
            crate::ensure_tool_permission(permissions, "workflow_pause", &id)?;
            workflow_pause(State(state.clone()), Query(query), headers, Path(id)).await
        }
        request @ (Request::Activate { .. } | Request::Run { .. }) => {
            let (id, revision, activate) = match request {
                Request::Activate { id, revision } => (id, revision, true),
                Request::Run { id, revision } => (id, revision, false),
                _ => unreachable!(),
            };
            let workspace = workspace_root(directory.to_owned()).map_err(|e| e.to_string())?;
            let source = find_source(state.services(), &workspace, &id)
                .await
                .map_err(|e| e.to_string())?;
            if revision != format!("sha256:{}", source.source_hash) {
                return Err("Workflow changed; inspect its current definition before proceeding".into());
            }
            let operation = if activate { "workflow_activate" } else { "workflow_run" };
            crate::ensure_tool_permission(permissions, operation, &format!("{id}@{revision}"))?;
            track_workspace(state, &workspace).await;
            let _guard = state.inner.workflow_reconcile.lock().await;
            if activate {
                activate_source(state, workspace, id, source, None).await
            } else {
                run_source(state, workspace, source).await
            }
        }
        Request::Draft { definition } => {
            if definition.active {
                return Err("workflow draft must be paused (active: false)".into());
            }
            validate_definition(&definition).map_err(|error| error.to_string())?;
            let workspace = canonical_definition_context(FsPath::new(directory))
                .map_err(|error| error.to_string())?;
            let path = managed_workflow_path(&workspace, &definition.id)
                .map_err(|error| error.to_string())?;
            crate::ensure_tool_permission(permissions, "edit", &path.to_string_lossy())?;
            let directory = workspace.to_string_lossy();
            let guard = state.inner.management_lock.lock().await;
            let catalog = discover_async(state.services().clone(), directory.to_string())
                .await
                .map_err(|error| error.to_string())?;
            if catalog.workflows.contains_key(&definition.id) {
                return Err("A workflow with this ID already exists".into());
            }
            let bytes = workflow_markdown(&definition).map_err(|error| error.to_string())?;
            atomic_workflow_write_with_mode(&workspace, &path, &bytes, true)
                .map_err(|error| error.to_string())?;
            drop(guard);
            Ok(Json(json!({ "id": definition.id, "sourcePath": path, "active": false })))
        }
    }
    .map_err(|error| error.to_string())?;
    Ok(crate::tool::ToolExecutionResult {
        title: "Scheduled workflows".into(),
        output: result.0.to_string(),
        metadata: None,
    })
}

pub(crate) fn spawn_scheduler(state: AppState) {
    let weak_state = std::sync::Arc::downgrade(&state.inner);
    let workflow_notify = state.inner.workflow_notify.clone();
    let local = !state.services().hosted;
    tokio::spawn(async move {
        let (watch_tx, mut watch_rx) = tokio::sync::mpsc::unbounded_channel();
        if let Some(inner) = weak_state.upgrade() {
            if let Err(error) = recover_unfinished_runs(&AppState { inner }).await {
                tracing::warn!(%error, "failed to recover workflow runs");
            }
        }
        let mut event_worker = tokio::task::JoinSet::new();
        if local {
            mcp_event_trigger::spawn(&mut event_worker, weak_state.clone());
        }
        let mut watcher: Option<notify::RecommendedWatcher>;
        loop {
            let Some(inner) = weak_state.upgrade() else {
                return;
            };
            let state = AppState { inner };
            if state.inner.workflow_workspaces.read().await.is_empty() {
                state
                    .inner
                    .workflow_scheduler_started
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                // Close the race where participation was added while the flag
                // still said the old scheduler was alive.
                if !state.inner.workflow_workspaces.read().await.is_empty()
                    && state
                        .inner
                        .workflow_scheduler_started
                        .compare_exchange(
                            false,
                            true,
                            std::sync::atomic::Ordering::SeqCst,
                            std::sync::atomic::Ordering::SeqCst,
                        )
                        .is_ok()
                {
                    spawn_scheduler(state);
                }
                return;
            }
            if let Err(error) = schedule_due_workflows(&state).await {
                tracing::warn!(%error, "workflow scheduler pass failed");
            }
            let workspaces = state
                .inner
                .workflow_workspaces
                .read()
                .await
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            match install_workflow_watches(
                state.services(),
                &workspaces,
                watch_tx.clone(),
            ) {
                Ok(replacement) => watcher = Some(replacement),
                Err(error) => {
                    watcher = None;
                    tracing::warn!(%error, "failed to install workflow filesystem watches");
                }
            }
            let _watcher_guard = watcher.as_ref();
            let sleep = next_scheduler_due(&state)
                .await
                .ok()
                .flatten()
                .map(|due| Duration::from_millis(due.saturating_sub(now_millis())))
                .unwrap_or(CLOCK_SAFETY_WAKE)
                .min(CLOCK_SAFETY_WAKE);
            drop(state);
            tokio::select! {
                _ = workflow_notify.notified() => {},
                event = watch_rx.recv() => {
                    if event.is_none() {
                        tracing::warn!("workflow filesystem event channel closed");
                        return;
                    }
                    tokio::time::sleep(WATCH_DEBOUNCE).await;
                    while watch_rx.try_recv().is_ok() {}
                },
                _ = tokio::time::sleep(sleep) => {},
            }
        }
    });
}

fn install_workflow_watches(
    services: &neoism_agent_service_api::AgentServices,
    workspaces: &[String],
    event_tx: tokio::sync::mpsc::UnboundedSender<()>,
) -> anyhow::Result<notify::RecommendedWatcher> {
    let paths = workflow_watch_paths(services, workspaces);
    if paths.is_empty() {
        bail!("no enabled workflow workspace paths to watch");
    }
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            match event {
                Ok(event) if event.need_rescan() || !event.paths.is_empty() => {
                    let _ = event_tx.send(());
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "workflow filesystem watcher error");
                    let _ = event_tx.send(());
                }
            }
        })?;
    for (path, mode) in paths {
        if let Err(error) = watcher.watch(&path, mode) {
            tracing::warn!(path = %path.display(), %error, "failed to watch workflow path");
        }
    }
    Ok(watcher)
}

fn workflow_watch_paths(
    services: &neoism_agent_service_api::AgentServices,
    workspaces: &[String],
) -> BTreeMap<PathBuf, RecursiveMode> {
    let mut paths = BTreeMap::new();
    for workspace in workspaces {
        let roots = workflow_discovery_roots(services, workspace);
        for root in roots {
            if root.is_dir() {
                insert_watch(&mut paths, root.clone(), RecursiveMode::NonRecursive);
                for name in ["workflow", "workflows"] {
                    let workflow_root = root.join(name);
                    if workflow_root.is_dir() {
                        insert_watch(&mut paths, workflow_root, RecursiveMode::Recursive);
                    }
                }
            } else if let Some(parent) = nearest_existing_parent(&root) {
                insert_watch(&mut paths, parent, RecursiveMode::NonRecursive);
            }
        }
    }
    paths
}

fn insert_watch(
    paths: &mut BTreeMap<PathBuf, RecursiveMode>,
    path: PathBuf,
    mode: RecursiveMode,
) {
    let path = path.canonicalize().unwrap_or(path);
    paths
        .entry(path)
        .and_modify(|existing| {
            if mode == RecursiveMode::Recursive {
                *existing = RecursiveMode::Recursive;
            }
        })
        .or_insert(mode);
}

fn nearest_existing_parent(path: &FsPath) -> Option<PathBuf> {
    let mut candidate = path.to_path_buf();
    while !candidate.is_dir() {
        if !candidate.pop() {
            return None;
        }
    }
    Some(candidate)
}

pub(crate) async fn workflow_list(
    State(state): State<AppState>,
    Query(query): Query<WorkflowQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let workspace = workflow_context(&state, query.directory, query.scope, &headers)?;
    track_workspace(&state, &workspace).await;
    let catalog = discover_async(state.services().clone(), workspace.clone()).await?;
    let persisted = state.inner.store.list_workflows().await?;
    let workflows = catalog
        .workflows
        .values()
        .map(|source| {
            let projection = persisted.iter().find(|item| {
                item.workspace_root == workspace
                    && item.workflow_id == source.definition.id
            });
            json!({
                "definition": source.definition,
                "sourcePath": source.source_path,
                "sourceHash": source.source_hash,
                "active": projection.map(|item| item.active).unwrap_or(source.definition.active),
                "activationID": projection.map(|item| item.activation_id.as_str()),
                "lastScheduledAt": projection.and_then(|item| item.last_scheduled_at),
                "writable": workflow_source_is_managed(&workspace, source),
                "revision": workflow_source_is_managed(&workspace, source).then(|| format!("sha256:{}", source.source_hash)),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(
        json!({ "workflows": workflows, "diagnostics": catalog.diagnostics }),
    ))
}

pub(crate) async fn workflow_get(
    State(state): State<AppState>,
    Query(query): Query<WorkflowQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workflow_context(&state, query.directory, query.scope, &headers)?;
    track_workspace(&state, &workspace).await;
    let source = find_source(state.services(), &workspace, &workflow_id).await?;
    let projection = state
        .inner
        .store
        .get_workflow(&activation_id(&workspace, &workflow_id))
        .await?;
    let writable = workflow_source_is_managed(&workspace, &source);
    Ok(Json(json!({
        "definition": source.definition,
        "sourcePath": source.source_path,
        "sourceHash": source.source_hash,
        "active": projection.as_ref().map(|item| item.active).unwrap_or(source.definition.active),
        "activationID": projection.as_ref().map(|item| item.activation_id.as_str()),
        "lastScheduledAt": projection.and_then(|item| item.last_scheduled_at),
        "writable": writable,
        "revision": writable.then(|| format!("sha256:{}", source.source_hash)),
    })))
}

fn workflow_source_is_managed(workspace: &str, source: &WorkflowSource) -> bool {
    let expected = managed_workflow_root(FsPath::new(workspace))
        .join("workflows")
        .join(format!("{}.md", source.definition.id));
    FsPath::new(&source.source_path) == expected.canonicalize().unwrap_or(expected)
}

pub(crate) async fn workflow_activate(
    State(state): State<AppState>,
    Query(query): Query<WorkflowQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workflow_context(&state, query.directory, query.scope, &headers)?;
    track_workspace(&state, &workspace).await;
    let _guard = state.inner.workflow_reconcile.lock().await;
    let source = find_source(state.services(), &workspace, &workflow_id).await?;
    let action = if source.definition.action.is_some() {
        let action = admit_operator_action(&state, &headers, &workspace, &source)?;
        Some(action.approval_identity().to_owned())
    } else {
        None
    };
    activate_source(&state, workspace, workflow_id, source, action.as_deref()).await
}

async fn activate_source(
    state: &AppState,
    workspace: String,
    workflow_id: String,
    source: WorkflowSource,
    action_identity: Option<&str>,
) -> Result<Json<Value>, ApiError> {
    if action_identity.is_none() {
        reject_direct_workflow(&source.definition)?;
    }
    let id = activation_id(&workspace, &workflow_id);
    let existing = state.inner.store.get_workflow(&id).await?;
    let now = now_millis();
    let activated_at = existing.as_ref()
        .filter(|item| {
            action_identity.is_none()
                || (item.active && item.source_hash == source.source_hash && item.source_path == source.source_path)
        })
        .map_or(now, |item| item.activated_at);
    let projection = WorkflowProjection {
        activation_id: id,
        workflow_id,
        workspace_root: workspace,
        source_path: source.source_path,
        source_hash: source.source_hash,
        definition: source.definition,
        active: true,
        activated_at,
        last_scheduled_at: existing.and_then(|item| item.last_scheduled_at),
        updated: now,
    };
    if let Some(identity) = action_identity {
        state.inner.store.activate_scheduled_workflow_action(&projection, identity).await?;
    } else {
        state.inner.store.upsert_workflow(&projection).await?;
    }
    publish_workflow(state, &projection);
    state.inner.workflow_notify.notify_one();
    Ok(Json(json!(projection)))
}

pub(crate) async fn workflow_pause(
    State(state): State<AppState>,
    Query(query): Query<WorkflowQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workflow_context(&state, query.directory, query.scope, &headers)?;
    track_workspace(&state, &workspace).await;
    let _guard = state.inner.workflow_reconcile.lock().await;
    let id = activation_id(&workspace, &workflow_id);
    if !state
        .inner
        .store
        .set_workflow_active(&id, false, now_millis())
        .await?
    {
        return Err(ApiError::not_found("Workflow is not activated"));
    }
    let projection = state
        .inner
        .store
        .get_workflow(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("Workflow not found"))?;
    publish_workflow(&state, &projection);
    state.inner.workflow_notify.notify_one();
    Ok(Json(json!(projection)))
}

pub(crate) async fn workflow_run_now(
    State(state): State<AppState>,
    Query(query): Query<WorkflowQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workflow_context(&state, query.directory, query.scope, &headers)?;
    track_workspace(&state, &workspace).await;
    let guard = state.inner.workflow_reconcile.lock().await;
    let source = find_source(state.services(), &workspace, &workflow_id).await?;
    if source.definition.action.is_some() {
        let action = admit_operator_action(&state, &headers, &workspace, &source)?;
        let trigger = if matches!(action.action(), DirectAction::Compose { .. }) { MANUAL_COMPOSE } else { "manual" };
        let approval_identity = action.approval_identity().to_owned();
        let projection = ensure_projection(&state, workspace, source, false).await?;
        let run = claim_manual_run_with_trigger(&state, &projection, trigger).await?;
        drop(guard);
        tokio::spawn(execute_shell_run(
            state, projection, run.clone(), approval_identity, check_operator_action_approval,
        ));
        return Ok(Json(json!(run)));
    }
    run_source(&state, workspace, source).await
}

async fn run_source(
    state: &AppState,
    workspace: String,
    source: WorkflowSource,
) -> Result<Json<Value>, ApiError> {
    reject_direct_workflow(&source.definition)?;
    let projection = ensure_projection(state, workspace, source, false).await?;
    let run = claim_manual_run(state, &projection).await?;
    tokio::spawn(execute_run(state.clone(), projection, run.clone()));
    Ok(Json(json!(run)))
}

async fn claim_manual_run(state: &AppState, projection: &WorkflowProjection) -> Result<WorkflowRun, ApiError> {
    claim_manual_run_with_trigger(state, projection, "manual").await
}

pub(crate) async fn claim_manual_run_with_trigger(state: &AppState, projection: &WorkflowProjection, trigger: &str) -> Result<WorkflowRun, ApiError> {
    let mut run = new_run(projection, now_millis(), trigger);
    assign_run_lease(state, &mut run);
    if !state
        .inner
        .store
        .claim_workflow_run(&run, &projection.definition.concurrency)
        .await?
    {
        return Err(ApiError::conflict(
            "Workflow already has a queued or running run",
        ));
    }
    publish_run(state, &run);
    Ok(run)
}

pub(crate) async fn workflow_preview(
    State(state): State<AppState>,
    Query(query): Query<WorkflowQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workflow_context(&state, query.directory, query.scope, &headers)?;
    track_workspace(&state, &workspace).await;
    let source = find_source(state.services(), &workspace, &workflow_id).await?;
    let slots = preview_slots(&source.definition.schedule, now_millis(), PREVIEW_COUNT)?;
    let tz = resolve_timezone(&source.definition.schedule.timezone)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let slots = slots
        .into_iter()
        .map(|timestamp| {
            let local = Utc
                .timestamp_millis_opt(timestamp as i64)
                .single()
                .unwrap()
                .with_timezone(&tz);
            json!({ "scheduledAt": timestamp, "local": local.to_rfc3339() })
        })
        .collect::<Vec<_>>();
    Ok(Json(
        json!({ "definition": source.definition, "sourcePath": source.source_path, "upcoming": slots }),
    ))
}

pub(crate) async fn workflow_history(
    State(state): State<AppState>,
    Query(query): Query<WorkflowHistoryQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workflow_context(&state, query.directory, query.scope, &headers)?;
    track_workspace(&state, &workspace).await;
    let id = activation_id(&workspace, &workflow_id);
    if state.inner.store.get_workflow(&id).await?.is_none() {
        find_source(state.services(), &workspace, &workflow_id).await?;
        return Ok(Json(json!({ "runs": [] })));
    }
    let mut runs = state
        .inner
        .store
        .list_workflow_runs(&id, query.limit.unwrap_or(50))
        .await?;
    if !is_shell_operator(&state, &headers) {
        for run in &mut runs {
            run.shell_result = None;
            run.compose_result = None;
        }
    }
    Ok(Json(json!({ "runs": runs })))
}

pub(crate) async fn workflow_run_get(
    State(state): State<AppState>,
    Query(query): Query<WorkflowQuery>,
    headers: HeaderMap,
    Path((workflow_id, run_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workflow_context(&state, query.directory, query.scope, &headers)?;
    let mut run = state
        .inner
        .store
        .get_workflow_run(&run_id)
        .await?
        .filter(|run| run.activation_id == activation_id(&workspace, &workflow_id))
        .ok_or_else(|| ApiError::not_found("Workflow run not found"))?;
    if !is_shell_operator(&state, &headers) {
        run.shell_result = None;
        run.compose_result = None;
    }
    Ok(Json(json!(run)))
}

pub(crate) async fn workflow_run_retry(
    State(state): State<AppState>,
    Query(query): Query<WorkflowQuery>,
    headers: HeaderMap,
    Path((workflow_id, run_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workflow_context(&state, query.directory, query.scope, &headers)?;
    track_workspace(&state, &workspace).await;
    let activation = activation_id(&workspace, &workflow_id);
    let previous = state
        .inner
        .store
        .get_workflow_run(&run_id)
        .await?
        .filter(|run| run.activation_id == activation)
        .ok_or_else(|| ApiError::not_found("Workflow run not found"))?;
    if previous.trigger == "mcpEvent" {
        return Err(ApiError::conflict("MCP event submissions cannot be retried"));
    }
    if !matches!(
        previous.status.as_str(),
        "failed" | "interrupted" | "replaced"
    ) {
        return Err(ApiError::conflict(
            "Only failed, interrupted, or replaced workflow runs can be retried",
        ));
    }
    let projection = state
        .inner
        .store
        .get_workflow(&activation)
        .await?
        .ok_or_else(|| ApiError::not_found("Workflow activation not found"))?;
    reject_direct_workflow(&projection.definition)?;
    if scheduled_direct_trigger(&previous.trigger) || previous.trigger == MANUAL_COMPOSE {
        return Err(ApiError::conflict("Direct action runs cannot be retried"));
    }
    let mut run = new_retry_run(&previous, now_millis(), "manual-retry");
    assign_run_lease(&state, &mut run);
    if !state
        .inner
        .store
        .claim_workflow_run(&run, &projection.definition.concurrency)
        .await?
    {
        return Err(ApiError::conflict(
            "Workflow concurrency limit has been reached",
        ));
    }
    publish_run(&state, &run);
    tokio::spawn(execute_run(state.clone(), projection, run.clone()));
    Ok(Json(json!(run)))
}

fn admin_response(
    status: u16,
    body: Value,
) -> Result<
    neoism_agent_plugin_api::RouteResponse,
    neoism_agent_plugin_api::PluginRuntimeError,
> {
    Ok(neoism_agent_plugin_api::RouteResponse::json(status, body))
}

fn admin_error(
    status: u16,
    code: &str,
    message: impl Into<String>,
    details: Value,
) -> Result<
    neoism_agent_plugin_api::RouteResponse,
    neoism_agent_plugin_api::PluginRuntimeError,
> {
    admin_response(
        status,
        json!({ "code": code, "message": message.into(), "retryable": false, "details": details }),
    )
}

fn authorize_admin(
    state: &AppState,
    actor: Option<&str>,
) -> Option<
    Result<
        neoism_agent_plugin_api::RouteResponse,
        neoism_agent_plugin_api::PluginRuntimeError,
    >,
> {
    if !state.management_enabled() {
        Some(admin_error(
            404,
            "management.disabled",
            "Management API is disabled",
            json!({}),
        ))
    } else if actor.is_none() {
        Some(admin_error(401, "management.authentication_required", "Workflow definition mutations require authenticated local or daemon credentials", json!({})))
    } else {
        None
    }
}

fn managed_workflow_root(workspace: &FsPath) -> PathBuf {
    match workspace
        .to_string_lossy()
        .strip_prefix(INSTALLATION_CONTEXT_PREFIX)
    {
        Some(root) => PathBuf::from(root),
        None => workspace.join(".agent"),
    }
}

fn canonical_definition_context(workspace: &FsPath) -> std::io::Result<PathBuf> {
    if workspace
        .to_string_lossy()
        .starts_with(INSTALLATION_CONTEXT_PREFIX)
    {
        Ok(workspace.to_path_buf())
    } else {
        workspace.canonicalize()
    }
}

fn managed_workflow_path(workspace: &FsPath, id: &str) -> anyhow::Result<PathBuf> {
    if id.is_empty()
        || id.len() > 80
        || !id.as_bytes()[0].is_ascii_lowercase()
        || !id.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        bail!("id must be a 1-80 character lowercase workflow slug");
    }
    let root = managed_workflow_root(workspace);
    for candidate in [root.as_path(), root.join("workflows").as_path()] {
        if candidate.exists() && fs::symlink_metadata(candidate)?.file_type().is_symlink()
        {
            bail!("managed workflow paths may not contain symlinks");
        }
    }
    Ok(root.join("workflows").join(format!("{id}.md")))
}

fn workflow_revision(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn current_revision(path: &FsPath) -> anyhow::Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(workflow_revision(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn requested_revision(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    query.map(str::to_string).or_else(|| {
        headers
            .get("if-match")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim_matches('"').to_string())
    })
}

fn check_workflow_revision(
    current: Option<&str>,
    expected: Option<&str>,
    creating: bool,
) -> std::result::Result<(), (u16, Value)> {
    match (current, expected, creating) {
        (Some(current), _, true) => Err((409, json!({ "currentRevision": current }))),
        (None, _, false) => Err((404, json!({}))),
        (Some(current), Some(expected), false)
            if expected != "*" && expected != current =>
        {
            Err((412, json!({ "currentRevision": current })))
        }
        _ => Ok(()),
    }
}

fn workflow_markdown(definition: &WorkflowDefinition) -> anyhow::Result<Vec<u8>> {
    let mut value = serde_json::to_value(definition)?;
    let prompt = value
        .as_object_mut()
        .and_then(|map| map.remove("prompt"))
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    let yaml = serde_yaml::to_string(&value)?;
    Ok(format!(
        "---\n{}---\n{}{}",
        yaml.trim_start_matches("---\n"),
        prompt,
        if prompt.ends_with('\n') { "" } else { "\n" }
    )
    .into_bytes())
}

fn atomic_workflow_write(
    workspace: &FsPath,
    path: &FsPath,
    bytes: &[u8],
) -> anyhow::Result<()> {
    atomic_workflow_write_with_mode(workspace, path, bytes, false)
}

fn atomic_workflow_write_with_mode(
    workspace: &FsPath,
    path: &FsPath,
    bytes: &[u8],
    create_only: bool,
) -> anyhow::Result<()> {
    let directory = path.parent().context("workflow path has no parent")?;
    fs::create_dir_all(directory)?;
    for candidate in [managed_workflow_root(workspace), directory.to_path_buf()] {
        let metadata = fs::symlink_metadata(&candidate)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("managed workflow path is not a real directory");
        }
    }
    let temp = directory.join(format!(
        ".neoism-workflow-{}-{}.tmp",
        std::process::id(),
        now_millis()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    drop(file);
    let publish = if create_only {
        // Publish atomically without replacing a file created outside the API lock.
        fs::hard_link(&temp, path)
    } else {
        fs::rename(&temp, path)
    };
    if create_only || publish.is_err() {
        let _ = fs::remove_file(&temp);
    }
    publish?;
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

async fn refresh_after_definition_change(
    state: &AppState,
    workspace: &str,
) -> anyhow::Result<()> {
    track_workspace(state, workspace).await;
    let runtime = state
        .workspace_runtime(workspace)
        .await
        .map_err(anyhow::Error::msg)?;
    let _ = crate::workspace_runtime::refresh_plugins(&runtime, state).await;
    reconcile_workspaces(state).await?;
    state.inner.workflow_notify.notify_waiters();
    Ok(())
}

fn workflow_view(source: WorkflowSource, active: bool) -> Value {
    json!({
        "definition": source.definition, "sourcePath": source.source_path, "sourceHash": source.source_hash,
        "revision": format!("sha256:{}", source.source_hash), "writable": true, "active": active,
        "activationID": Value::Null, "lastScheduledAt": Value::Null,
    })
}

pub(crate) async fn workflow_create(
    state: &AppState,
    workspace: Option<&FsPath>,
    actor: Option<&str>,
    headers: &HeaderMap,
    body: Value,
) -> Result<
    neoism_agent_plugin_api::RouteResponse,
    neoism_agent_plugin_api::PluginRuntimeError,
> {
    if let Some(response) = authorize_admin(state, actor) {
        return response;
    }
    let definition: WorkflowDefinition = match serde_json::from_value(body) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                400,
                "management.invalid_request",
                error.to_string(),
                json!({}),
            )
        }
    };
    if let Err(error) = validate_definition(&definition) {
        return admin_error(
            400,
            "management.invalid_request",
            error.to_string(),
            json!({}),
        );
    }
    let Some(workspace) = workspace else {
        return admin_error(
            400,
            "management.invalid_request",
            "Workspace is required",
            json!({}),
        );
    };
    let workspace = match canonical_definition_context(workspace) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                400,
                "management.invalid_request",
                error.to_string(),
                json!({}),
            )
        }
    };
    let path = match managed_workflow_path(&workspace, &definition.id) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                400,
                "management.invalid_request",
                error.to_string(),
                json!({}),
            )
        }
    };
    let directory = workspace.to_string_lossy().into_owned();
    let _guard = state.inner.management_lock.lock().await;
    let current = current_revision(&path).ok().flatten();
    if let Err((status, details)) = check_workflow_revision(
        current.as_deref(),
        requested_revision(headers, None).as_deref(),
        true,
    ) {
        return admin_error(
            status,
            "management.revision_conflict",
            "Workflow revision does not match",
            details,
        );
    }
    if find_source(state.services(), &directory, &definition.id)
        .await
        .is_ok()
    {
        return admin_error(
            403,
            "management.read_only",
            "A built-in or discovered workflow with this id cannot be overwritten",
            json!({}),
        );
    }
    let bytes = match workflow_markdown(&definition) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                400,
                "management.invalid_request",
                error.to_string(),
                json!({}),
            )
        }
    };
    if let Err(error) = atomic_workflow_write(&workspace, &path, &bytes) {
        return admin_error(
            500,
            "management.storage_error",
            error.to_string(),
            json!({}),
        );
    }
    drop(_guard);
    if let Err(error) = refresh_after_definition_change(state, &directory).await {
        return admin_error(
            500,
            "management.storage_error",
            error.to_string(),
            json!({}),
        );
    }
    let source = parse_source(&path).expect("newly validated workflow must parse");
    admin_response(201, workflow_view(source, definition.active))
}

pub(crate) async fn workflow_update(
    state: &AppState,
    workspace: Option<&FsPath>,
    actor: Option<&str>,
    headers: &HeaderMap,
    id: &str,
    body: Value,
    patch: bool,
) -> Result<
    neoism_agent_plugin_api::RouteResponse,
    neoism_agent_plugin_api::PluginRuntimeError,
> {
    if let Some(response) = authorize_admin(state, actor) {
        return response;
    }
    let Some(workspace) = workspace else {
        return admin_error(
            400,
            "management.invalid_request",
            "Workspace is required",
            json!({}),
        );
    };
    let workspace = match canonical_definition_context(workspace) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                400,
                "management.invalid_request",
                error.to_string(),
                json!({}),
            )
        }
    };
    let path = match managed_workflow_path(&workspace, id) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                400,
                "management.invalid_request",
                error.to_string(),
                json!({}),
            )
        }
    };
    let directory = workspace.to_string_lossy().into_owned();
    let _guard = state.inner.management_lock.lock().await;
    let current = match current_revision(&path) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                500,
                "management.storage_error",
                error.to_string(),
                json!({}),
            )
        }
    };
    if current.is_none() && find_source(state.services(), &directory, id).await.is_ok() {
        return admin_error(
            403,
            "management.read_only",
            "Built-in and discovered workflows cannot be overwritten",
            json!({}),
        );
    }
    if let Err((status, details)) = check_workflow_revision(
        current.as_deref(),
        requested_revision(headers, None).as_deref(),
        false,
    ) {
        return admin_error(
            status,
            "management.revision_conflict",
            "Workflow revision does not match",
            details,
        );
    }
    let definition = if patch {
        let source = match parse_source(&path) {
            Ok(value) => value,
            Err(error) => {
                return admin_error(
                    400,
                    "management.invalid_request",
                    error.to_string(),
                    json!({}),
                )
            }
        };
        let patch: WorkflowDefinitionPatch = match serde_json::from_value(body) {
            Ok(value) => value,
            Err(error) => {
                return admin_error(
                    400,
                    "management.invalid_request",
                    error.to_string(),
                    json!({}),
                )
            }
        };
        let mut value = source.definition;
        if let Some(id) = patch.id {
            value.id = id;
        }
        if let Some(name) = patch.name {
            value.name = name;
        }
        if let Some(active) = patch.active {
            value.active = active;
        }
        if let Some(schedule) = patch.schedule {
            value.schedule = schedule;
        }
        if let Some(prompt) = patch.prompt {
            value.prompt = prompt;
        }
        if let Some(directory) = patch.directory {
            value.directory = directory;
        }
        if let Some(skill) = patch.skill {
            value.skill = skill;
        }
        if let Some(agent) = patch.agent {
            value.agent = agent;
        }
        if let Some(model) = patch.model {
            value.model = model;
        }
        if let Some(permissions) = patch.permissions {
            value.permissions = permissions;
        }
        if let Some(retry) = patch.retry {
            value.retry = retry;
        }
        if let Some(concurrency) = patch.concurrency {
            value.concurrency = concurrency;
        }
        value
    } else {
        match serde_json::from_value(body) {
            Ok(value) => value,
            Err(error) => {
                return admin_error(
                    400,
                    "management.invalid_request",
                    error.to_string(),
                    json!({}),
                )
            }
        }
    };
    if definition.id != id {
        return admin_error(
            400,
            "management.invalid_request",
            "Workflow id cannot be changed",
            json!({}),
        );
    }
    if let Err(error) = validate_definition(&definition) {
        return admin_error(
            400,
            "management.invalid_request",
            error.to_string(),
            json!({}),
        );
    }
    let bytes = match workflow_markdown(&definition) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                400,
                "management.invalid_request",
                error.to_string(),
                json!({}),
            )
        }
    };
    if let Err(error) = atomic_workflow_write(&workspace, &path, &bytes) {
        return admin_error(
            500,
            "management.storage_error",
            error.to_string(),
            json!({}),
        );
    }
    drop(_guard);
    if let Err(error) = refresh_after_definition_change(state, &directory).await {
        return admin_error(
            500,
            "management.storage_error",
            error.to_string(),
            json!({}),
        );
    }
    let source = parse_source(&path).expect("updated workflow must parse");
    admin_response(200, workflow_view(source, definition.active))
}

pub(crate) async fn workflow_delete(
    state: &AppState,
    workspace: Option<&FsPath>,
    actor: Option<&str>,
    headers: &HeaderMap,
    id: &str,
    query: &BTreeMap<String, Vec<String>>,
) -> Result<
    neoism_agent_plugin_api::RouteResponse,
    neoism_agent_plugin_api::PluginRuntimeError,
> {
    if let Some(response) = authorize_admin(state, actor) {
        return response;
    }
    let Some(workspace) = workspace else {
        return admin_error(
            400,
            "management.invalid_request",
            "Workspace is required",
            json!({}),
        );
    };
    let workspace = match canonical_definition_context(workspace) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                400,
                "management.invalid_request",
                error.to_string(),
                json!({}),
            )
        }
    };
    let path = match managed_workflow_path(&workspace, id) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                400,
                "management.invalid_request",
                error.to_string(),
                json!({}),
            )
        }
    };
    let directory = workspace.to_string_lossy().into_owned();
    let _guard = state.inner.management_lock.lock().await;
    let current = match current_revision(&path) {
        Ok(value) => value,
        Err(error) => {
            return admin_error(
                500,
                "management.storage_error",
                error.to_string(),
                json!({}),
            )
        }
    };
    if current.is_none() && find_source(state.services(), &directory, id).await.is_ok() {
        return admin_error(
            403,
            "management.read_only",
            "Built-in and discovered workflows cannot be deleted",
            json!({}),
        );
    }
    let expected = query
        .get("expectedRevision")
        .and_then(|values| values.first())
        .map(String::as_str);
    if let Err((status, details)) = check_workflow_revision(
        current.as_deref(),
        requested_revision(headers, expected).as_deref(),
        false,
    ) {
        return admin_error(
            status,
            "management.revision_conflict",
            "Workflow revision does not match",
            details,
        );
    }
    let dispatch_guard = state.inner.workflow_reconcile.lock().await;
    if let Err(error) = fs::remove_file(&path) {
        return admin_error(
            500,
            "management.storage_error",
            error.to_string(),
            json!({}),
        );
    }
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        let _ = fs::File::open(parent).and_then(|file| file.sync_all());
    }
    let activation = activation_id(&directory, id);
    let _ = state
        .inner
        .store
        .set_workflow_active(&activation, false, now_millis())
        .await;
    drop(dispatch_guard);
    drop(_guard);
    if let Err(error) = refresh_after_definition_change(state, &directory).await {
        return admin_error(
            500,
            "management.storage_error",
            error.to_string(),
            json!({}),
        );
    }
    admin_response(204, Value::Null)
}

async fn discover_async(
    services: neoism_agent_service_api::AgentServices,
    workspace: String,
) -> Result<WorkflowCatalog, ApiError> {
    tokio::task::spawn_blocking(move || discover(&services, &workspace))
        .await
        .map_err(|error| {
            ApiError::internal(format!("workflow discovery task failed: {error}"))
        })?
        .map_err(ApiError::from)
}

async fn track_workspace(state: &AppState, workspace: &str) {
    let snapshot = state.plugin_snapshot(workspace).await;
    if !snapshot
        .manifests
        .iter()
        .any(|plugin| plugin.id == neoism_agent_builtins::plugin::workflows::ID)
    {
        return;
    }
    workspace_enabled(
        state,
        crate::workspace_runtime::canonical_location(workspace),
    )
    .await;
}

pub(crate) async fn workspace_enabled(state: &AppState, workspace: PathBuf) {
    let workspace = workspace.to_string_lossy().into_owned();
    if state
        .inner
        .workflow_workspaces
        .write()
        .await
        .insert(workspace.to_string())
    {
        if state
            .inner
            .workflow_scheduler_started
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            spawn_scheduler(state.clone());
        }
        state.inner.workflow_notify.notify_one();
    }
}

pub(crate) async fn workspace_disabled(state: &AppState, workspace: &FsPath) {
    state
        .inner
        .workflow_workspaces
        .write()
        .await
        .remove(workspace.to_string_lossy().as_ref());
    state.inner.workflow_notify.notify_waiters();
}

async fn reconcile_workspaces(state: &AppState) -> anyhow::Result<()> {
    let _reconcile = state.inner.workflow_reconcile.lock().await;
    reconcile_rune_prompts(state, None).await?;
    let workspaces = state
        .inner
        .workflow_workspaces
        .read()
        .await
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    for workspace in workspaces {
        let scan_workspace = workspace.clone();
        let services = state.services().clone();
        let catalog =
            tokio::task::spawn_blocking(move || discover(&services, &scan_workspace))
                .await
                .context("workflow discovery task failed")??;
        for diagnostic in catalog.diagnostics {
            tracing::warn!(
                source_path = %diagnostic.source_path,
                message = %diagnostic.message,
                "invalid workflow source"
            );
        }
        for source in catalog.workflows.into_values() {
            let id = activation_id(&workspace, &source.definition.id);
            if state.inner.store.rune_prompt_pin(&id).await?.is_some() { continue; }
            let existing = state.inner.store.get_workflow(&id).await?;
            match existing {
                None if source.definition.active => {
                    let now = now_millis();
                    let projection = WorkflowProjection {
                        activation_id: id,
                        workflow_id: source.definition.id.clone(),
                        workspace_root: workspace.clone(),
                        source_path: source.source_path,
                        source_hash: source.source_hash,
                        definition: source.definition,
                        active: true,
                        activated_at: now,
                        last_scheduled_at: None,
                        updated: now,
                    };
                    state.inner.store.upsert_workflow(&projection).await?;
                    publish_workflow(state, &projection);
                }
                Some(mut projection)
                    if projection.source_hash != source.source_hash
                        || projection.source_path != source.source_path =>
                {
                    let was_active = projection.active;
                    projection.source_path = source.source_path;
                    projection.source_hash = source.source_hash;
                    projection.active = source.definition.active
                        && projection.definition.action.is_none()
                        && source.definition.action.is_none();
                    projection.definition = source.definition;
                    projection.updated = now_millis();
                    if projection.active && !was_active {
                        projection.activated_at = projection.updated;
                        projection.last_scheduled_at = None;
                    }
                    state.inner.store.upsert_workflow(&projection).await?;
                    publish_workflow(state, &projection);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

async fn find_source(
    services: &neoism_agent_service_api::AgentServices,
    workspace: &str,
    workflow_id: &str,
) -> Result<WorkflowSource, ApiError> {
    let catalog = discover_async(services.clone(), workspace.to_string()).await?;
    catalog.workflows.get(workflow_id).cloned().ok_or_else(|| {
        let detail = catalog
            .diagnostics
            .iter()
            .find(|item| item.source_path.contains(workflow_id))
            .map(|item| format!(": {}", item.message))
            .unwrap_or_default();
        ApiError::not_found(format!("Workflow `{workflow_id}` not found{detail}"))
    })
}

// Managed workflow writes retain their shipped .agent destination even when a
// host adapter discovers other resources under .neoism. Reads and watches must
// include that destination too; managed project definitions take precedence.
fn workflow_discovery_roots(
    services: &neoism_agent_service_api::AgentServices,
    workspace: &str,
) -> Vec<PathBuf> {
    if let Some(root) = workspace.strip_prefix(INSTALLATION_CONTEXT_PREFIX) {
        return vec![PathBuf::from(root)];
    }
    let mut roots = crate::config::roots(services, workspace);
    let managed = FsPath::new(workspace).join(".agent");
    if !roots.contains(&managed) {
        roots.push(managed);
    }
    roots
}

fn discover(
    services: &neoism_agent_service_api::AgentServices,
    workspace: &str,
) -> anyhow::Result<WorkflowCatalog> {
    let mut catalog = WorkflowCatalog::default();
    for root in workflow_discovery_roots(services, workspace) {
        for directory in [root.join("workflow"), root.join("workflows")] {
            for path in markdown_files(&directory)? {
                match parse_source(&path) {
                    Ok(source) => {
                        catalog
                            .workflows
                            .insert(source.definition.id.clone(), source);
                    }
                    Err(error) => catalog.diagnostics.push(WorkflowDiagnostic {
                        source_path: path.display().to_string(),
                        message: format!("{error:#}"),
                    }),
                }
            }
        }
    }
    Ok(catalog)
}

fn markdown_files(root: &FsPath) -> anyhow::Result<Vec<PathBuf>> {
    fn visit(path: &FsPath, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
        if !path.is_dir() {
            return Ok(());
        }
        for entry in std::fs::read_dir(path)? {
            let path = entry?.path();
            if path.is_dir() {
                visit(&path, files)?;
            } else if path.extension().and_then(|value| value.to_str()) == Some("md") {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn parse_source(path: &FsPath) -> anyhow::Result<WorkflowSource> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let source_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    parse_source_text(&source_path, &raw)
}

fn parse_source_text(path: &FsPath, raw: &str) -> anyhow::Result<WorkflowSource> {
    let normalized = raw.replace("\r\n", "\n");
    let mut lines = normalized.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        bail!("workflow must begin with YAML frontmatter");
    }
    let mut yaml = String::new();
    let mut found_end = false;
    let mut body = String::new();
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            found_end = true;
            body = lines.collect();
            break;
        }
        yaml.push_str(line);
    }
    if !found_end {
        bail!("workflow frontmatter is missing its closing `---`");
    }
    let prompt = body.trim().to_string();
    let yaml_value: serde_yaml::Value =
        serde_yaml::from_str(&yaml).context("invalid workflow YAML")?;
    let mut value = serde_json::to_value(yaml_value)?;
    let map = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("workflow frontmatter must be a mapping"))?;
    map.insert("prompt".to_string(), Value::String(prompt));
    let definition: WorkflowDefinition =
        serde_json::from_value(value).context("invalid workflow definition")?;
    validate_definition(&definition)?;
    Ok(WorkflowSource {
        definition,
        source_path: path.display().to_string(),
        source_hash: format!("{:x}", Sha256::digest(raw.as_bytes())),
        local_origin: None,
    })
}

/// A read-only catalog for the dedicated local daemon capability. Do not call
/// workspace tracking or the ordinary discovery path: they can import installation
/// resources and/or reconcile active workflows as a side effect.
pub(crate) async fn local_workspace_catalog(state: &AppState, root: PathBuf) -> Result<Value, ApiError> {
    #[cfg(not(unix))]
    {
        let _ = (state, root);
        Err(ApiError::forbidden("The local workflow catalog currently requires Unix"))
    }
    #[cfg(unix)]
    {
        let _guard = state.inner.workflow_reconcile.lock().await;
        let catalog = scan_local_workflows(state, root.clone()).await?;
        reconcile_rune_prompts(state, Some((&root, &catalog))).await?;
        let persisted = state.inner.store.list_workflows().await?;
        let workflows = catalog.workflows.into_values().filter(|source| source.definition.action.is_none()).map(|source| {
            let projection = persisted.iter().find(|item| item.workspace_root == root.to_string_lossy()
                && item.workflow_id == source.definition.id && item.definition.action.is_none()
                && item.source_path == source.source_path);
            json!({ "active": projection.map(|item| item.active).unwrap_or(source.definition.active),
                "activationID": projection.map(|item| item.activation_id.as_str()), "writable": false,
                "definition": source.definition, "sourcePath": source.source_path, "sourceHash": source.source_hash,
                "sourceOrigin": source.local_origin, "canActivate": rune_schedule_eligible(&source.definition) })
        }).collect::<Vec<_>>();
        Ok(json!({ "workflows": workflows, "diagnostics": catalog.diagnostics }))
    }
}

pub(crate) fn rune_schedule_eligible(definition: &WorkflowDefinition) -> bool {
    definition.action.is_none() && definition.schedule.frequency != "event"
        && definition.directory.is_none() && definition.skill.is_none() && definition.retry.max_attempts == 1
        && definition.concurrency.mode == WorkflowConcurrencyMode::Forbid
}

#[cfg(unix)]
fn rune_origin_matches(projection: &WorkflowProjection, origin: &str, source: &WorkflowSource) -> bool {
    rune_schedule_eligible(&source.definition) && source.definition.id == projection.workflow_id
        && source.source_path == projection.source_path && source.source_hash == projection.source_hash
        && source.local_origin.as_deref() == Some(origin)
}

async fn invalidate_rune_prompt(state: &AppState, projection: &WorkflowProjection, epoch: i64) -> anyhow::Result<()> {
    if state.inner.store.invalidate_rune_prompt(&projection.activation_id, epoch).await? {
        let mut paused = projection.clone();
        paused.active = false;
        publish_workflow(state, &paused);
        state.inner.workflow_notify.notify_one();
    }
    Ok(())
}

/// Called with workflow_reconcile held; retain an inactive pin on failure.
async fn reconcile_rune_prompts(state: &AppState, scanned: Option<(&FsPath, &WorkflowCatalog)>) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mut catalogs = BTreeMap::new();
        for projection in state.inner.store.list_active_workflows().await? {
            let root = FsPath::new(&projection.workspace_root);
            if scanned.is_some_and(|(scope, _)| scope != root) { continue; }
            let Some((epoch, origin)) = state.inner.store.rune_prompt_pin(&projection.activation_id).await? else { continue };
            let catalog = if let Some((_, catalog)) = scanned { Some(catalog) } else {
                if !catalogs.contains_key(root) {
                    catalogs.insert(root.to_path_buf(), scan_local_workflows(state, root.to_path_buf()).await.ok());
                }
                catalogs.get(root).and_then(Option::as_ref)
            };
            if !catalog.and_then(|catalog| catalog.workflows.get(&projection.workflow_id))
                .is_some_and(|source| rune_origin_matches(&projection, &origin, source)) {
                invalidate_rune_prompt(state, &projection, epoch).await?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (state, scanned);
    Ok(())
}

async fn rune_dispatch_allowed(state: &AppState, projection: &WorkflowProjection, expected_epoch: Option<i64>) -> anyhow::Result<bool> {
    let pin = state.inner.store.rune_prompt_pin(&projection.activation_id).await?;
    let Some(expected_epoch) = expected_epoch else { return Ok(pin.is_none()); };
    let Some((epoch, origin)) = pin.filter(|(epoch, _)| *epoch == expected_epoch) else { return Ok(false) };
    let Some(current) = state.inner.store.get_workflow(&projection.activation_id).await? else { return Ok(false) };
    if !current.active || current.workspace_root != projection.workspace_root || current.source_path != projection.source_path
        || current.source_hash != projection.source_hash || current.workflow_id != projection.workflow_id { return Ok(false); }
    #[cfg(unix)]
    if local_prompt_source(state, PathBuf::from(&current.workspace_root), &current.workflow_id).await
        .is_ok_and(|source| rune_origin_matches(&current, &origin, &source)) { return Ok(true); }
    #[cfg(not(unix))]
    let _ = origin;
    invalidate_rune_prompt(state, &current, epoch).await?;
    Ok(false)
}

#[cfg(unix)]
async fn scan_local_workflows(state: &AppState, root: PathBuf) -> Result<WorkflowCatalog, ApiError> {
    let config = state.services().config.clone();
    tokio::task::spawn_blocking(move || {
        let installation = config.snapshot(&neoism_agent_service_api::ConfigSnapshotRequest::installation())?;
        let excluded = installation.discovery_roots.into_iter()
            .filter(|r| r.scope == neoism_agent_service_api::ConfigDiscoveryScope::Installation)
            .map(|r| r.path.canonicalize().unwrap_or(r.path)).collect::<Vec<_>>();
        local_workflow_sources(&root, &excluded)
    }).await.map_err(|_| ApiError::internal("Workflow catalog scan failed"))?
        .map_err(|_| ApiError::internal("Could not read local workflow catalog"))
}

#[cfg(unix)]
pub(crate) async fn local_prompt_source(state: &AppState, root: PathBuf, id: &str) -> Result<WorkflowSource, ApiError> {
    scan_local_workflows(state, root).await?.workflows.remove(id)
        .filter(|source| source.definition.action.is_none())
        .ok_or_else(|| ApiError::not_found("Workspace prompt workflow not found"))
}

#[cfg(unix)]
fn local_workflow_sources(root: &FsPath, excluded: &[PathBuf]) -> anyhow::Result<WorkflowCatalog> {
    use std::{io::Read, os::unix::fs::MetadataExt};
    const MAX_SOURCES: usize = 256;
    const MAX_SOURCE_BYTES: u64 = 128 * 1024;
    const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;
    let mut catalog = WorkflowCatalog::default();
    // Only direct files in fixed project directories. No arbitrary config search
    // roots, ancestor traversal, symlinks, hard links, special files or cross-device reads.
    let workspace = open_shell_path(root, true)?.metadata()?;
    let mut count = 0;
    let mut bytes = 0;
    let mut duplicates = std::collections::HashSet::new();
    for config in [".neoism", ".agent"] {
        for name in ["workflow", "workflows"] {
            let directory = root.join(config).join(name);
            if excluded.iter().any(|path| directory.starts_with(path) || path.starts_with(&directory)) { continue; }
            let Ok(dir) = open_shell_path(&directory, true) else { continue };
            if dir.metadata()?.dev() != workspace.dev() { continue; }
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                if path.extension().and_then(|s| s.to_str()) != Some("md") { continue; }
                let Ok(file) = open_shell_path(&path, false) else { continue };
                let md = file.metadata()?;
                if !md.is_file() || md.nlink() != 1 || md.dev() != workspace.dev() { continue; }
                count += 1;
                if count > MAX_SOURCES { bail!("too many workspace workflow sources"); }
                let mut raw = String::new();
                if (&file).take(MAX_SOURCE_BYTES + 1).read_to_string(&mut raw).is_err() || raw.len() as u64 > MAX_SOURCE_BYTES {
                    catalog.diagnostics.push(WorkflowDiagnostic { source_path: path.display().to_string(), message: "Invalid or oversized workflow source".into() });
                    continue;
                }
                bytes += raw.len();
                if bytes > MAX_TOTAL_BYTES { bail!("workspace workflow sources exceed the catalog limit"); }
                match parse_source_text(&path, &raw) {
                    Ok(mut source) => {
                        let final_md = file.metadata()?;
                        let stamp = |m: &fs::Metadata| [m.dev(), m.ino(), m.ctime() as u64, m.ctime_nsec() as u64, m.mtime() as u64, m.mtime_nsec() as u64];
                        if stamp(&md) != stamp(&final_md) || final_md.nlink() != 1 { continue; }
                        let current = open_shell_path(&path, false).and_then(|file| file.metadata());
                        if !current.is_ok_and(|current| current.nlink() == 1 && stamp(&current) == stamp(&final_md)) { continue; }
                        let parent = dir.metadata()?;
                        let current_parent = open_shell_path(&directory, true).and_then(|dir| dir.metadata());
                        if !current_parent.is_ok_and(|current| (current.dev(), current.ino()) == (parent.dev(), parent.ino())) { continue; }
                        let config_dir = open_shell_path(&root.join(config), true)?.metadata()?;
                        let origin = format!("{:?}:{:?}", [workspace.dev(), workspace.ino(), config_dir.dev(), config_dir.ino(), parent.dev(), parent.ino()], stamp(&final_md));
                        source.local_origin = Some(format!("{:x}", Sha256::digest(origin.as_bytes())));
                        let id = source.definition.id.clone();
                        if duplicates.contains(&id) || catalog.workflows.insert(id.clone(), source).is_some() {
                            duplicates.insert(id.clone());
                            catalog.workflows.remove(&id);
                            catalog.diagnostics.push(WorkflowDiagnostic { source_path: path.display().to_string(), message: "Duplicate workflow ID in workspace".into() });
                        }
                    }
                    Err(error) => catalog.diagnostics.push(WorkflowDiagnostic { source_path: path.display().to_string(), message: error.to_string() }),
                }
            }
        }
    }
    Ok(catalog)
}

fn validate_definition(definition: &WorkflowDefinition) -> anyhow::Result<()> {
    if definition.id.is_empty()
        || !definition.id.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || (index > 0 && matches!(ch, '-' | '_' | '.'))
        })
    {
        bail!(
            "id must be a lowercase slug containing letters, numbers, `.`, `_`, or `-`"
        );
    }
    if definition.name.trim().is_empty() {
        bail!("name must not be empty");
    }
    if let Some(action) = &definition.action {
        action.validate_workflow_action()?;
        if !definition.prompt.trim().is_empty() || definition.active || definition.skill.is_some()
            || definition.agent.is_some() || definition.model.is_some() || definition.permissions.is_some()
            || definition.retry.max_attempts != 1 || definition.concurrency.mode != WorkflowConcurrencyMode::Forbid
        {
            bail!("Direct workflows require explicit activation and cannot have a prompt, agent options, permissions, retries, or non-forbid concurrency");
        }
    } else if definition.prompt.trim().is_empty() {
        bail!("Markdown prompt body must not be empty");
    }
    if let Some(directory) = definition.directory.as_deref() {
        if directory.trim().is_empty() {
            bail!("directory must not be empty");
        }
        let path = FsPath::new(directory);
        if !path.is_absolute()
            && !directory.starts_with('~')
            && path
                .components()
                .any(|component| component == Component::ParentDir)
        {
            bail!(
                "relative directory cannot contain `..`; use ~/... or an absolute path"
            );
        }
    }
    if let Some(permissions) = definition.permissions.as_ref() {
        for (permission, setting) in permissions {
            if permission.trim().is_empty() {
                bail!("permission names must not be empty");
            }
            if let WorkflowPermission::Rules(setting) = setting {
                if setting.default.is_none()
                    && setting.allow.is_empty()
                    && setting.deny.is_empty()
                    && setting.ask.is_empty()
                {
                    bail!("permissions.{permission} must define default, allow, deny, or ask");
                }
                if setting
                    .allow
                    .iter()
                    .chain(&setting.deny)
                    .chain(&setting.ask)
                    .any(|pattern| pattern.trim().is_empty())
                {
                    bail!("permissions.{permission} patterns must not be empty");
                }
            }
        }
    }
    if definition
        .effective_permission_rules()
        .as_deref()
        .into_iter()
        .flatten()
        .any(|rule| rule.action == PermissionAction::Ask)
    {
        bail!("scheduled workflows cannot use `ask` permissions; use explicit allow or deny rules");
    }
    if definition.retry.max_attempts == 0 {
        bail!("retry.maxAttempts must be at least 1");
    }
    if definition.retry.max_delay_ms > 0
        && definition.retry.max_delay_ms < definition.retry.initial_delay_ms
    {
        bail!("retry.maxDelayMs must be greater than or equal to retry.initialDelayMs");
    }
    if definition
        .retry
        .retryable_errors
        .iter()
        .any(|value| value.trim().is_empty())
    {
        bail!("retry.retryableErrors must not contain empty values");
    }
    if definition.concurrency.max_running == 0 {
        bail!("concurrency.maxRunning must be at least 1");
    }
    if definition.concurrency.mode != WorkflowConcurrencyMode::Allow
        && definition.concurrency.max_running != 1
    {
        bail!("concurrency.maxRunning must be 1 unless concurrency.mode is allow");
    }
    if definition.schedule.frequency == "event" && definition.action.is_some() {
        bail!("event-only schedules require a prompt workflow");
    }
    validate_schedule(&definition.schedule)
}

fn validate_schedule(schedule: &WorkflowSchedule) -> anyhow::Result<()> {
    if schedule.interval == 0 {
        bail!("schedule.interval must be at least 1");
    }
    if schedule.frequency == "event" {
        if schedule.interval != 1
            || schedule.timezone != "UTC"
            || schedule.at.is_some()
            || schedule.date.is_some()
            || schedule.time.is_some()
            || schedule.minute.is_some()
            || !schedule.weekdays.is_empty()
            || schedule.month_day.is_some()
            || schedule.cron.is_some()
        {
            bail!("event-only schedules do not accept clock settings");
        }
        return Ok(());
    }
    if schedule.frequency == "cron" {
        let (cron, tz) = parse_cron_schedule(schedule)?;
        // Reject expressions with no possible next occurrence (e.g. February 31).
        cron_next_slot(&cron, tz, now_millis())?;
        return Ok(());
    }
    if schedule.cron.is_some() {
        bail!("schedule.cron requires schedule.frequency `cron`");
    }
    resolve_timezone(&schedule.timezone)?;
    if let Some(at) = schedule.at.as_deref() {
        DateTime::parse_from_rfc3339(at).with_context(|| {
            format!("at `{at}` must be an RFC 3339 timestamp with an offset")
        })?;
        if schedule.frequency != "once"
            || schedule.interval != 1
            || schedule.date.is_some()
            || schedule.time.is_some()
            || schedule.minute.is_some()
            || !schedule.weekdays.is_empty()
            || schedule.month_day.is_some()
        {
            bail!("timestamp schedules only accept schedule.at");
        }
        return Ok(());
    }
    if let Some(date) = schedule.date.as_deref() {
        NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .with_context(|| format!("date `{date}` must use YYYY-MM-DD"))?;
        parse_time(schedule.time.as_deref().unwrap_or("00:00"))?;
        if schedule.frequency != "once" {
            bail!("one-time schedules cannot also set frequency");
        }
        if schedule.interval != 1
            || schedule.minute.is_some()
            || !schedule.weekdays.is_empty()
            || schedule.month_day.is_some()
        {
            bail!("one-time schedules only accept date, time, and timezone");
        }
        return Ok(());
    }
    match schedule.frequency.as_str() {
        "hourly" => {
            if schedule.minute.unwrap_or(0) > 59 { bail!("hourly schedule.minute must be between 0 and 59"); }
            if schedule.time.is_some() || !schedule.weekdays.is_empty() || schedule.month_day.is_some() { bail!("hourly schedules only accept minute"); }
        }
        "daily" => {
            parse_time(schedule.time.as_deref().unwrap_or("00:00"))?;
            if schedule.minute.is_some() || !schedule.weekdays.is_empty() || schedule.month_day.is_some() { bail!("daily schedules only accept time"); }
        }
        "weekly" => {
            parse_time(schedule.time.as_deref().unwrap_or("00:00"))?;
            if schedule.weekdays.is_empty() { bail!("weekly schedules require at least one weekday"); }
            let mut seen = BTreeSet::new();
            for weekday in &schedule.weekdays {
                let value = weekday_number(weekday)?;
                if !seen.insert(value) { bail!("weekly schedule contains duplicate weekday `{weekday}`"); }
            }
            if schedule.minute.is_some() || schedule.month_day.is_some() { bail!("weekly schedules only accept time and weekdays"); }
        }
        "monthly" => {
            parse_time(schedule.time.as_deref().unwrap_or("00:00"))?;
            if !(1..=31).contains(&schedule.month_day.unwrap_or(1)) { bail!("monthly schedule.monthDay must be between 1 and 31"); }
            if schedule.minute.is_some() || !schedule.weekdays.is_empty() { bail!("monthly schedules only accept time and monthDay"); }
        }
        "once" => bail!("one-time schedules require schedule.date"),
        other => bail!("unsupported schedule.frequency `{other}`; use hourly, daily, weekly, monthly, cron, or a date"),
    }
    Ok(())
}

fn parse_time(value: &str) -> anyhow::Result<NaiveTime> {
    let normalized = value.trim().to_ascii_uppercase();
    for format in ["%H:%M", "%H:%M:%S", "%I:%M %p", "%I:%M:%S %p"] {
        if let Ok(time) = NaiveTime::parse_from_str(&normalized, format) {
            return Ok(time);
        }
    }
    bail!("time `{value}` must use HH:MM, HH:MM:SS, h:MM AM/PM, or h:MM:SS AM/PM")
}

fn resolve_timezone(value: &str) -> anyhow::Result<Tz> {
    let timezone = if value.eq_ignore_ascii_case("local") {
        iana_time_zone::get_timezone()
            .context("could not determine the local timezone")?
    } else {
        value.to_string()
    };
    timezone
        .parse()
        .with_context(|| format!("unknown timezone `{value}`"))
}

fn weekday_number(value: &str) -> anyhow::Result<u32> {
    match value.to_ascii_lowercase().as_str() {
        "monday" | "mon" => Ok(0),
        "tuesday" | "tue" => Ok(1),
        "wednesday" | "wed" => Ok(2),
        "thursday" | "thu" => Ok(3),
        "friday" | "fri" => Ok(4),
        "saturday" | "sat" => Ok(5),
        "sunday" | "sun" => Ok(6),
        _ => bail!("unknown weekday `{value}`"),
    }
}

fn parse_cron_schedule(schedule: &WorkflowSchedule) -> anyhow::Result<(Cron, Tz)> {
    if schedule.interval != 1
        || schedule.minute.is_some()
        || schedule.time.is_some()
        || !schedule.weekdays.is_empty()
        || schedule.month_day.is_some()
        || schedule.date.is_some()
        || schedule.at.is_some()
    {
        bail!("cron schedules only accept schedule.cron and schedule.timezone, with interval 1");
    }
    let expression = schedule.cron.as_deref().context("missing schedule.cron")?;
    if expression.chars().take(257).count() > 256 || expression.split_whitespace().count() != 5 {
        bail!("schedule.cron must be five fields (minute hour day-of-month month day-of-week), at most 256 characters");
    }
    let cron = expression
        .parse::<Cron>()
        .map_err(|error| anyhow::anyhow!("invalid schedule.cron: {error}"))?;
    let tz = schedule.timezone.parse::<Tz>().with_context(|| {
        format!("cron schedules require an IANA timezone, not `{}`", schedule.timezone)
    })?;
    Ok((cron, tz))
}

fn cron_search_start(tz: Tz, millis: u64) -> anyhow::Result<DateTime<Tz>> {
    // Croner returns whole-second instants. Floor before searching so the
    // exclusive forward / inclusive backward comparisons also work for subsecond inputs.
    let utc = DateTime::from_timestamp((millis / 1_000) as i64, 0)
        .filter(|date| date.year() <= croner::YEAR_UPPER_LIMIT)
        .context("timestamp is outside the cron search range")?;
    Ok(utc.with_timezone(&tz))
}

fn cron_next_slot(cron: &Cron, tz: Tz, after: u64) -> anyhow::Result<u64> {
    // Croner jumps between matching fields and bounds iterations and calendar years.
    let next = cron
        .find_next_occurrence(&cron_search_start(tz, after)?, false)
        .map_err(|error| {
            anyhow::anyhow!("could not calculate the next schedule.cron time: {error}")
        })?;
    Ok(u64::try_from(next.timestamp_millis())?)
}

fn preview_slots(
    schedule: &WorkflowSchedule,
    after: u64,
    count: usize,
) -> Result<Vec<u64>, ApiError> {
    if schedule.frequency == "event" {
        return Ok(Vec::new());
    }
    let cron = if schedule.frequency == "cron" {
        Some(parse_cron_schedule(schedule).map_err(|error| ApiError::bad_request(error.to_string()))?)
    } else if is_one_time(schedule) {
        let slot = one_time_slot(schedule)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        return Ok((slot > after).then_some(slot).into_iter().collect());
    } else {
        None
    };
    let mut slots = Vec::with_capacity(count);
    let mut cursor = after;
    for _ in 0..count {
        cursor = match &cron {
            Some((cron, tz)) => cron_next_slot(cron, *tz, cursor),
            None => next_slot(schedule, cursor),
        }
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
        slots.push(cursor);
    }
    Ok(slots)
}

fn next_slot(schedule: &WorkflowSchedule, after: u64) -> anyhow::Result<u64> {
    if schedule.frequency == "cron" {
        let (cron, tz) = parse_cron_schedule(schedule)?;
        return cron_next_slot(&cron, tz, after);
    }
    let tz = resolve_timezone(&schedule.timezone)?;
    let after_utc = Utc
        .timestamp_millis_opt(after as i64)
        .single()
        .context("timestamp is outside supported range")?;
    let start_date = after_utc.with_timezone(&tz).date_naive();
    let interval = i64::from(schedule.interval);
    let max_days = match schedule.frequency.as_str() {
        "hourly" => interval.saturating_mul(2).saturating_add(4),
        "daily" => interval.saturating_add(3),
        "weekly" => interval.saturating_mul(7).saturating_add(8),
        "monthly" => interval.saturating_mul(32).saturating_add(40),
        _ => bail!("unsupported schedule frequency"),
    };
    for day_offset in 0..=max_days {
        let date = start_date
            .checked_add_days(chrono::Days::new(day_offset as u64))
            .context("schedule date overflow")?;
        match schedule.frequency.as_str() {
            "hourly" => {
                for hour in 0..24 {
                    let local_hour_index = i64::from(date.num_days_from_ce())
                        .saturating_mul(24)
                        + i64::from(hour);
                    if local_hour_index.rem_euclid(interval) != 0 {
                        continue;
                    }
                    let time =
                        NaiveTime::from_hms_opt(hour, schedule.minute.unwrap_or(0), 0)
                            .unwrap();
                    if let Some(value) = local_timestamp(tz, date.and_time(time)) {
                        if value > after {
                            return Ok(value);
                        }
                    }
                }
            }
            "daily" => {
                if i64::from(date.num_days_from_ce()).rem_euclid(interval) != 0 {
                    continue;
                }
                if let Some(value) = local_timestamp(
                    tz,
                    date.and_time(parse_time(
                        schedule.time.as_deref().unwrap_or("00:00"),
                    )?),
                ) {
                    if value > after {
                        return Ok(value);
                    }
                }
            }
            "weekly" => {
                let week = i64::from(date.num_days_from_ce()).div_euclid(7);
                if week.rem_euclid(interval) != 0 {
                    continue;
                }
                let weekday = date.weekday().num_days_from_monday();
                if !schedule
                    .weekdays
                    .iter()
                    .any(|item| weekday_number(item).ok() == Some(weekday))
                {
                    continue;
                }
                if let Some(value) = local_timestamp(
                    tz,
                    date.and_time(parse_time(
                        schedule.time.as_deref().unwrap_or("00:00"),
                    )?),
                ) {
                    if value > after {
                        return Ok(value);
                    }
                }
            }
            "monthly" => {
                let month_index =
                    i64::from(date.year()).saturating_mul(12) + i64::from(date.month0());
                if month_index.rem_euclid(interval) != 0 || date.day() != 1 {
                    continue;
                }
                let requested = schedule.month_day.unwrap_or(1);
                let last = last_day_of_month(date.year(), date.month());
                let target = NaiveDate::from_ymd_opt(
                    date.year(),
                    date.month(),
                    requested.min(last),
                )
                .unwrap();
                if let Some(value) = local_timestamp(
                    tz,
                    target.and_time(parse_time(
                        schedule.time.as_deref().unwrap_or("00:00"),
                    )?),
                ) {
                    if value > after {
                        return Ok(value);
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    bail!("could not calculate the next workflow time")
}

fn one_time_slot(schedule: &WorkflowSchedule) -> anyhow::Result<u64> {
    if let Some(at) = schedule.at.as_deref() {
        let timestamp = DateTime::parse_from_rfc3339(at)?.timestamp_millis();
        return u64::try_from(timestamp)
            .context("one-time timestamp must not be before 1970");
    }
    let tz = resolve_timezone(&schedule.timezone)?;
    let date = NaiveDate::parse_from_str(
        schedule
            .date
            .as_deref()
            .context("one-time schedule is missing date")?,
        "%Y-%m-%d",
    )?;
    let time = parse_time(schedule.time.as_deref().unwrap_or("00:00"))?;
    local_timestamp(tz, date.and_time(time))
        .context("one-time schedule is outside the supported timestamp range")
}

fn local_timestamp(tz: Tz, mut local: NaiveDateTime) -> Option<u64> {
    for _ in 0..=180 {
        match tz.from_local_datetime(&local) {
            LocalResult::Single(value) => {
                return u64::try_from(value.timestamp_millis()).ok()
            }
            LocalResult::Ambiguous(first, second) => {
                return u64::try_from(first.min(second).timestamp_millis()).ok()
            }
            LocalResult::None => {
                local = local.checked_add_signed(chrono::Duration::minutes(1))?
            }
        }
    }
    None
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .unwrap()
        .pred_opt()
        .unwrap()
        .day()
}

fn latest_due(
    schedule: &WorkflowSchedule,
    activated_at: u64,
    now: u64,
) -> anyhow::Result<Option<u64>> {
    if schedule.frequency == "event" {
        return Ok(None);
    }
    if schedule.frequency == "cron" {
        if activated_at > now {
            return Ok(None);
        }
        let (cron, tz) = parse_cron_schedule(schedule)?;
        // Look backward once, rather than replaying missed ticks after downtime.
        let previous = match cron.find_previous_occurrence(&cron_search_start(tz, now)?, true) {
            Ok(previous) => previous,
            Err(CronError::TimeSearchLimitExceeded) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        return Ok(u64::try_from(previous.timestamp_millis())
            .ok()
            .filter(|slot| *slot >= activated_at && *slot <= now));
    }
    if is_one_time(schedule) {
        let slot = one_time_slot(schedule)?;
        return Ok((slot >= activated_at && slot <= now).then_some(slot));
    }
    let interval = u64::from(schedule.interval);
    let horizon = match schedule.frequency.as_str() {
        "hourly" => interval.saturating_mul(3_600_000).saturating_add(7_200_000),
        "daily" => interval
            .saturating_mul(86_400_000)
            .saturating_add(172_800_000),
        "weekly" => interval
            .saturating_mul(604_800_000)
            .saturating_add(691_200_000),
        "monthly" => interval
            .saturating_mul(2_764_800_000)
            .saturating_add(3_456_000_000),
        _ => bail!("unsupported schedule frequency"),
    };
    let mut cursor = activated_at
        .saturating_sub(1)
        .max(now.saturating_sub(horizon));
    let mut latest = None;
    for _ in 0..128 {
        let next = next_slot(schedule, cursor)?;
        if next > now {
            break;
        }
        if next >= activated_at {
            latest = Some(next);
        }
        cursor = next;
    }
    Ok(latest)
}

async fn next_scheduler_due(state: &AppState) -> anyhow::Result<Option<u64>> {
    let now = now_millis();
    let mut earliest: Option<u64> = None;
    for projection in state.inner.store.list_active_workflows().await? {
        let schedule = &projection.definition.schedule;
        if schedule.frequency == "event" {
            continue;
        }
        let next = if is_one_time(schedule) {
            let slot = one_time_slot(schedule)?;
            let already_scheduled = projection
                .last_scheduled_at
                .map(|cursor| cursor >= slot)
                .unwrap_or(false);
            (!already_scheduled && slot >= projection.activated_at && slot > now)
                .then_some(slot)
        } else {
            let after = now.max(
                projection
                    .last_scheduled_at
                    .unwrap_or_else(|| projection.activated_at.saturating_sub(1)),
            );
            Some(next_slot(schedule, after)?)
        };
        if let Some(next) = next {
            earliest = Some(earliest.map(|current| current.min(next)).unwrap_or(next));
        }
    }
    if let Some(retry) = state.inner.store.next_workflow_retry_at().await? {
        earliest = Some(earliest.map(|current| current.min(retry)).unwrap_or(retry));
    }
    Ok(earliest)
}

fn is_one_time(schedule: &WorkflowSchedule) -> bool {
    schedule.date.is_some() || schedule.at.is_some()
}

async fn schedule_due_workflows(state: &AppState) -> anyhow::Result<()> {
    reconcile_workspaces(state).await?;
    schedule_due_retries(state).await?;
    let _guard = state.inner.workflow_reconcile.lock().await;
    for mut projection in state.inner.store.list_active_workflows().await? {
        let pin = state.inner.store.rune_prompt_pin(&projection.activation_id).await?;
        if pin.is_none() {
        if projection.definition.action.is_some() {
            if let Some((run, grant)) = claim_due_shell(state, &projection, now_millis()).await? {
                tokio::spawn(execute_scheduled_shell_run(
                    state.clone(), projection, run, grant, check_operator_action_approval,
                ));
            }
            continue;
        }
        let source = match tokio::task::spawn_blocking({
            let path = projection.source_path.clone();
            move || parse_source(FsPath::new(&path))
        })
        .await
        {
            Ok(Ok(source)) if source.definition.id == projection.workflow_id => source,
            Ok(Ok(_)) => {
                pause_invalid_source(state, &projection, "workflow id changed").await?;
                continue;
            }
            Ok(Err(error)) => {
                pause_invalid_source(state, &projection, &error.to_string()).await?;
                continue;
            }
            Err(error) => {
                pause_invalid_source(state, &projection, &error.to_string()).await?;
                continue;
            }
        };
        if source.source_hash != projection.source_hash {
            if source.definition.action.is_some() {
                pause_invalid_source(state, &projection, "direct shell workflows require explicit activation").await?;
                continue;
            }
            projection.definition = source.definition;
            projection.source_hash = source.source_hash;
            projection.updated = now_millis();
            state.inner.store.upsert_workflow(&projection).await?;
        }
        }
        let now = now_millis();
        let Some(slot) = latest_due(
            &projection.definition.schedule,
            projection.activated_at,
            now,
        )?
        else {
            continue;
        };
        if projection
            .last_scheduled_at
            .map(|cursor| slot <= cursor)
            .unwrap_or(false)
        {
            continue;
        }
        let mut run = new_run(&projection, slot, if pin.is_some() { RUNE_SCHEDULED_PROMPT } else { "scheduled" });
        assign_run_lease(state, &mut run);
        let epoch = pin.map(|(epoch, _)| epoch);
        let claimed = if let Some(epoch) = epoch {
            if !rune_dispatch_allowed(state, &projection, Some(epoch)).await? { continue; }
            state.inner.store.claim_scheduled_rune_prompt(&run, &projection.definition.concurrency, epoch).await?
        } else {
            state.inner.store.claim_scheduled_workflow_run(&run, &projection.definition.concurrency).await?
        };
        if claimed {
            publish_run(state, &run);
            tokio::spawn(execute_prompt_run(state.clone(), projection, run, epoch));
        }
    }
    Ok(())
}

type ScheduledActionGrant = (i64, String);

/// Called under the reconciliation lock so pause, activation and occurrence
/// claims have a single order within the scheduler's server instance.
async fn claim_due_shell(
    state: &AppState,
    projection: &WorkflowProjection,
    now: u64,
) -> anyhow::Result<Option<(WorkflowRun, ScheduledActionGrant)>> {
    let Some(grant) = state.inner.store.scheduled_workflow_action_grant(projection).await? else {
        pause_invalid_source(state, projection, "missing scheduled action activation").await?;
        return Ok(None);
    };
    if !shell_source_is_current(FsPath::new(&projection.source_path), &projection.source_hash) {
        pause_invalid_source(state, projection, "direct shell source changed").await?;
        return Ok(None);
    }
    let Some(slot) = latest_due(&projection.definition.schedule, projection.activated_at, now)? else {
        return Ok(None);
    };
    if projection.last_scheduled_at.is_some_and(|last| slot <= last) {
        return Ok(None);
    }
    let trigger = if matches!(projection.definition.action, Some(DirectAction::Compose { .. })) {
        SCHEDULED_COMPOSE
    } else {
        SCHEDULED_SHELL
    };
    let mut run = new_run(projection, slot, trigger);
    assign_run_lease(state, &mut run);
    if !state.inner.store.claim_scheduled_workflow_action(&run, &projection.definition.concurrency, grant.0).await? {
        return Ok(None);
    }
    publish_run(state, &run);
    Ok(Some((run, grant)))
}

async fn schedule_due_retries(state: &AppState) -> anyhow::Result<()> {
    let now = now_millis();
    for mut run in state.inner.store.list_due_workflow_retries(now).await? {
        let Some(projection) = state.inner.store.get_workflow(&run.activation_id).await?
        else {
            continue;
        };
        if !projection.active || projection.definition.action.is_some() {
            continue;
        }
        let lease_expires_at = now.saturating_add(5 * 60 * 1000);
        if state
            .inner
            .store
            .claim_workflow_retry(
                &run,
                &projection.definition.concurrency,
                &state.inner.execution_owner_id,
                lease_expires_at,
            )
            .await?
        {
            run.status = "queued".into();
            run.next_attempt_at = None;
            run.lease_owner = Some(state.inner.execution_owner_id.clone());
            run.lease_expires_at = Some(lease_expires_at);
            publish_run(state, &run);
            tokio::spawn(execute_run(state.clone(), projection, run));
        } else {
            state
                .inner
                .store
                .defer_workflow_retry(&run.id, now.saturating_add(1_000))
                .await?;
        }
    }
    Ok(())
}

async fn pause_invalid_source(
    state: &AppState,
    projection: &WorkflowProjection,
    error: &str,
) -> anyhow::Result<()> {
    state
        .inner
        .store
        .set_workflow_active(&projection.activation_id, false, now_millis())
        .await?;
    state.publish(EventPayload::new(
        event_type::WORKFLOW_UPDATED,
        json!({
            "aggregateID": projection.activation_id,
            "workflowID": projection.workflow_id,
            "active": false,
            "error": error,
        }),
    ));
    Ok(())
}

async fn ensure_projection(
    state: &AppState,
    workspace: String,
    source: WorkflowSource,
    active: bool,
) -> Result<WorkflowProjection, ApiError> {
            let id = activation_id(&workspace, &source.definition.id);
    if let Some(mut existing) = state.inner.store.get_workflow(&id).await? {
        if source.definition.action.is_some()
            && existing.source_hash == source.source_hash
            && existing.source_path == source.source_path
        {
            return Ok(existing);
        }
        if existing.definition.action.is_some() || source.definition.action.is_some() {
            existing.active = false;
        }
        existing.source_path = source.source_path;
        existing.source_hash = source.source_hash;
        existing.definition = source.definition;
        existing.updated = now_millis();
        state.inner.store.upsert_workflow(&existing).await?;
        return Ok(existing);
    }
    let now = now_millis();
    let projection = WorkflowProjection {
        activation_id: id,
        workflow_id: source.definition.id.clone(),
        workspace_root: workspace,
        source_path: source.source_path,
        source_hash: source.source_hash,
        definition: source.definition,
        active,
        activated_at: now,
        last_scheduled_at: None,
        updated: now,
    };
    state.inner.store.upsert_workflow(&projection).await?;
    Ok(projection)
}

fn new_run(
    projection: &WorkflowProjection,
    scheduled_at: u64,
    trigger: &str,
) -> WorkflowRun {
    let created = now_millis();
    let id = format!("wfr_{}_{}", created, Id::ascending(IdKind::Event));
    WorkflowRun {
        id: id.clone(),
        activation_id: projection.activation_id.clone(),
        workflow_id: projection.workflow_id.clone(),
        scheduled_at,
        started_at: None,
        finished_at: None,
        session_id: None,
        status: "queued".to_string(),
        trigger: trigger.to_string(),
        error: None,
        created,
        attempt: 1,
        root_run_id: id,
        retry_of: None,
        next_attempt_at: None,
        lease_owner: None,
        lease_expires_at: None,
        shell_result: None,
        compose_result: None,
    }
}

fn new_retry_run(previous: &WorkflowRun, _due: u64, trigger: &str) -> WorkflowRun {
    let created = now_millis();
    let id = format!("wfr_{}_{}", created, crate::slug());
    WorkflowRun {
        id,
        activation_id: previous.activation_id.clone(),
        workflow_id: previous.workflow_id.clone(),
        // `scheduled_at` is also the durable occurrence key. Keep retries
        // adjacent to their lineage while `next_attempt_at` carries the real
        // backoff deadline, avoiding collisions with future schedule slots.
        scheduled_at: previous.scheduled_at.saturating_add(1),
        started_at: None,
        finished_at: None,
        session_id: None,
        status: "queued".into(),
        trigger: trigger.into(),
        error: None,
        created,
        attempt: previous.attempt.saturating_add(1),
        root_run_id: previous.root_run_id.clone(),
        retry_of: Some(previous.id.clone()),
        next_attempt_at: None,
        lease_owner: None,
        lease_expires_at: None,
        shell_result: None,
        compose_result: None,
    }
}

fn retry_delay(policy: &WorkflowRetryPolicy, attempt: u32) -> u64 {
    let multiplier = match policy.backoff {
        WorkflowBackoff::Fixed => 1,
        WorkflowBackoff::Exponential => 1u64
            .checked_shl(attempt.saturating_sub(2).min(62))
            .unwrap_or(u64::MAX),
    };
    let delay = policy.initial_delay_ms.saturating_mul(multiplier);
    if policy.max_delay_ms == 0 {
        delay
    } else {
        delay.min(policy.max_delay_ms)
    }
}

fn error_is_retryable(policy: &WorkflowRetryPolicy, message: &str) -> bool {
    policy.retryable_errors.is_empty()
        || policy.retryable_errors.iter().any(|candidate| {
            message
                .to_ascii_lowercase()
                .contains(&candidate.to_ascii_lowercase())
        })
}

fn assign_run_lease(state: &AppState, run: &mut WorkflowRun) {
    run.lease_owner = Some(state.inner.execution_owner_id.clone());
    run.lease_expires_at = Some(now_millis().saturating_add(5 * 60 * 1000));
}

pub(crate) async fn execute_run(
    state: AppState,
    projection: WorkflowProjection,
    run: WorkflowRun,
) {
    execute_prompt_run(state, projection, run, None).await;
}

async fn execute_prompt_run(state: AppState, projection: WorkflowProjection, mut run: WorkflowRun, epoch: Option<i64>) {
    let result = execute_run_inner(&state, &projection, &mut run, epoch).await;
    let mut publish_terminal = true;
    match result {
        Ok(()) => {
            run.status = "completed".to_string();
            run.finished_at = Some(now_millis());
            match state
                .inner
                .store
                .update_workflow_run(
                    &run.id,
                    "completed",
                    run.session_id.as_deref(),
                    None,
                    true,
                )
                .await
            {
                Ok(updated) => publish_terminal = updated,
                Err(error) => {
                    tracing::error!(%error, run_id = %run.id, "failed to finish workflow run")
                }
            }
        }
        Err(error) => {
            let message = error.to_string();
            run.status = "failed".to_string();
            run.error = Some(message.clone());
            run.finished_at = Some(now_millis());
            if epoch.is_none() && projection.definition.action.is_none() && run.attempt < projection.definition.retry.max_attempts
                && error_is_retryable(&projection.definition.retry, &message)
            {
                let due = now_millis().saturating_add(retry_delay(
                    &projection.definition.retry,
                    run.attempt.saturating_add(1),
                ));
                let mut retry = new_retry_run(&run, due, "automatic-retry");
                retry.status = "retry_waiting".into();
                retry.next_attempt_at = Some(due);
                match state
                    .inner
                    .store
                    .schedule_workflow_retry(&run, &retry)
                    .await
                {
                    Ok(true) => {
                        publish_run(&state, &retry);
                        state.inner.workflow_notify.notify_one();
                    }
                    Ok(false) => {
                        publish_terminal = false;
                        tracing::warn!(run_id = %run.id, "workflow failure was already finalized");
                    }
                    Err(store_error) => {
                        tracing::error!(error = %store_error, run_id = %run.id, "failed to schedule workflow retry")
                    }
                }
            } else {
                match state
                    .inner
                    .store
                    .update_workflow_run(
                        &run.id,
                        "failed",
                        run.session_id.as_deref(),
                        Some(&message),
                        true,
                    )
                    .await
                {
                    Ok(updated) => publish_terminal = updated,
                    Err(store_error) => {
                        tracing::error!(error = %store_error, run_id = %run.id, "failed to record workflow failure")
                    }
                }
            }
        }
    }
    if publish_terminal {
        publish_run(&state, &run);
    }
}

async fn execute_run_inner(
    state: &AppState,
    projection: &WorkflowProjection,
    run: &mut WorkflowRun,
    epoch: Option<i64>,
) -> Result<(), ApiError> {
    reject_direct_workflow(&projection.definition)?;
    if epoch.is_some() {
        let _guard = state.inner.workflow_reconcile.lock().await;
        if !rune_dispatch_allowed(state, projection, epoch).await? {
            return Err(ApiError::conflict("Rune activation is no longer current"));
        }
    }
    let mut prompt = projection.definition.prompt.clone();
    if let Some(skill_name) = projection.definition.skill.as_deref() {
        let plugins = state.plugin_snapshot(&projection.workspace_root).await;
        let skill = crate::skill::resolve_from_snapshot(
            &plugins,
            &projection.workspace_root,
            skill_name,
        )
        .await?
        .ok_or_else(|| {
            ApiError::bad_request(format!("workflow skill `{skill_name}` was not found"))
        })?;
        prompt = format!("{}\n\n{}", skill.content.trim(), prompt);
    }
    let mut extra = BTreeMap::new();
    extra.insert("workflowRunID".to_string(), json!(run.id));
    extra.insert("workflowID".to_string(), json!(run.workflow_id));
    extra.insert("workflowActivationID".to_string(), json!(run.activation_id));
    extra.insert("workflowScheduledAt".to_string(), json!(run.scheduled_at));
    extra.insert("workflowTrigger".to_string(), json!(run.trigger));
    let execution_directory = if epoch.is_some() { projection.workspace_root.clone() }
        else { workflow_execution_directory(state.services(), projection)? };
    let plugins = state.plugin_snapshot(&execution_directory).await;
    if !crate::plugins::enabled(&plugins, neoism_agent_builtins::plugin::workflows::ID) {
        return Err(ApiError::not_found(
            "Workflow plugin is disabled for the workspace",
        ));
    }
    extra.insert("workflowDirectory".to_string(), json!(execution_directory));
    let session = crate::session_routes::create_session_in_directory(
        state,
        &execution_directory,
        CreateSessionRequest {
            parent_id: None,
            title: Some(projection.definition.name.clone()),
            agent: projection.definition.agent.clone(),
            model: projection.definition.model.clone(),
            permission: projection.definition.effective_permission_rules(),
            workspace_id: None,
            external_provider: None,
            external_options: None,
        },
        extra,
    )
    .await?;
    let started = state
        .inner
        .store
        .update_workflow_run(&run.id, "running", Some(session.id.as_str()), None, false)
        .await?;
    if !started {
        return Err(ApiError::conflict(
            "Workflow run was replaced before execution began",
        ));
    }
    run.status = "running".to_string();
    run.started_at = Some(now_millis());
    run.session_id = Some(session.id.to_string());
    publish_run(state, run);
    let message_key = format!("{}:{}", run.activation_id, run.scheduled_at);
    let digest = format!("{:x}", Sha256::digest(message_key.as_bytes()));
    let message_id =
        Id::parse(IdKind::Message, format!("msg_workflow_{}", &digest[..24]))
            .map_err(|error| ApiError::internal(error.to_string()))?;
    // Fence again at prompt admission after session creation. Serialize the check
    // and admission against Pause/Activate and ordinary reconciliation.
    let _dispatch = if run.trigger != "rune-manual" {
        let guard = state.inner.workflow_reconcile.lock().await;
        if !rune_dispatch_allowed(state, projection, epoch).await? {
            return Err(ApiError::conflict("Rune activation is no longer current"));
        }
        epoch.map(|_| guard)
    } else { None };
    crate::append_prompt(
        state,
        session.id.as_str(),
        PromptRequest {
            message_id: Some(message_id),
            model: None,
            agent: None,
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text { text: prompt }],
        },
        true,
    )
    .await?;
    Ok(())
}

fn reject_direct_workflow(definition: &WorkflowDefinition) -> Result<(), ApiError> {
    if definition.action.is_some() {
        return Err(ApiError::forbidden("Direct workflows require a local operator request"));
    }
    Ok(())
}

fn is_shell_operator(state: &AppState, headers: &HeaderMap) -> bool {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    state.inner.caller_policy.is_workflow_shell_operator(bearer)
}

fn admit_operator_action(
    state: &AppState,
    headers: &HeaderMap,
    workspace: &str,
    source: &WorkflowSource,
) -> Result<ValidatedDirectAction, ApiError> {
    if !is_shell_operator(state, headers) {
        return Err(ApiError::forbidden("Direct workflows require the local operator bearer credential"));
    }
    let revision = format!("sha256:{}", source.source_hash);
    if requested_revision(headers, None).as_deref() != Some(revision.as_str()) {
        return Err(ApiError::conflict("Direct workflows require If-Match with the current source revision"));
    }
    let action = prepare_workflow_shell(workspace, &source.definition, &source.source_path, &source.source_hash)?;
    check_operator_action_approval(&action)?;
    Ok(action)
}

fn shell_admission_error(error: anyhow::Error) -> ApiError {
    ApiError::bad_request(error.to_string())
}

fn prepare_workflow_shell(
    workspace: &str,
    definition: &WorkflowDefinition,
    source_path: &str,
    source_hash: &str,
) -> Result<ValidatedDirectAction, ApiError> {
    if !cfg!(unix) || workspace.starts_with(INSTALLATION_CONTEXT_PREFIX) {
        return Err(ApiError::forbidden("Direct workflows are only supported in local Unix workspaces"));
    }
    validate_definition(definition).map_err(shell_admission_error)?;
    let root = FsPath::new(workspace);
    let canonical_root = root.canonicalize().map_err(|e| shell_admission_error(e.into()))?;
    if root != canonical_root {
        return Err(ApiError::conflict("Direct workflow workspace changed"));
    }
    let expected = managed_workflow_path(root, &definition.id).map_err(shell_admission_error)?;
    if expected != FsPath::new(source_path) {
        return Err(ApiError::forbidden("Direct workflows must use their managed workspace source"));
    }
    if !shell_source_is_current(&expected, source_hash) {
        return Err(ApiError::conflict("Direct workflow source changed or is inaccessible"));
    }
    let cwd = root.join(definition.directory.as_deref().unwrap_or("."));
    let action = definition.action.clone().ok_or_else(|| ApiError::bad_request("Missing direct shell action"))?
        .validate(&cwd, &format!("sha256:{source_hash}"))
        .map_err(shell_admission_error)?;
    if !action.directory().starts_with(root) {
        return Err(ApiError::forbidden("Direct workflow cwd must be inside the workspace"));
    }
    Ok(action)
}

fn shell_source_is_current(path: &FsPath, source_hash: &str) -> bool {
    #[cfg(unix)]
    {
        use std::io::Read;
        const MAX_SOURCE_BYTES: u64 = 128 * 1024;
        let Ok(file) = open_shell_path(path, false) else { return false; };
        if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
            return false;
        }
        let mut bytes = Vec::new();
        file.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes).is_ok()
            && bytes.len() as u64 <= MAX_SOURCE_BYTES
            && format!("{:x}", Sha256::digest(&bytes)) == source_hash
    }
    #[cfg(not(unix))]
    {
        let _ = (path, source_hash);
        false
    }
}

/// Walk an absolute path without following any symlink, retaining the final
/// handle so the runner never resolves an approved cwd by name again.
#[cfg(unix)]
fn open_shell_path(path: &FsPath, directory: bool) -> std::io::Result<fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let mut components = path.components().peekable();
    if components.next() != Some(Component::RootDir) {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut parent = fs::File::open("/")?;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(std::io::ErrorKind::InvalidInput.into());
        };
        let name = CString::new(name.as_bytes())?;
        let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        if directory || components.peek().is_some() {
            flags |= libc::O_DIRECTORY;
        }
        // SAFETY: The parent fd and NUL-terminated component outlive openat.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd == -1 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: A successful openat returns a new owned file descriptor.
        parent = unsafe { fs::File::from_raw_fd(fd) };
    }
    Ok(parent)
}

fn check_operator_action_approval(action: &ValidatedDirectAction) -> Result<(), ApiError> {
    if matches!(action.action(), DirectAction::Compose { tool_ids, .. } if crate::tool_composition::metadata::is_metadata(tool_ids)) {
        return crate::tool_composition::metadata::check_operator(action).map_err(|error| {
            ApiError::forbidden(format!("{error}; approval identity: {}", action.approval_identity()))
        });
    }
    let shell = matches!(action.action(), DirectAction::Shell { .. });
    let configured = std::env::var(if shell { workflow_action::SHELL_APPROVALS_ENV } else { workflow_action::COMPOSE_APPROVALS_ENV }).ok();
    let docs = std::env::var(workflow_action::COMPOSE_DOCS_PERMISSION_ENV).ok();
    check_configured_action_approval(action, configured.as_deref(), docs.as_deref())
}

fn check_configured_action_approval(
    action: &ValidatedDirectAction,
    configured: Option<&str>,
    docs: Option<&str>,
) -> Result<(), ApiError> {
    let shell = matches!(action.action(), DirectAction::Shell { .. });
    let approval = if shell {
        workflow_action::require_shell_approval(action, configured)
    } else {
        workflow_action::require_compose_approval(action, configured)
    };
    approval.map_err(|error| {
        ApiError::forbidden(format!("{error}; approval identity: {}", action.approval_identity()))
    })?;
    if !shell && docs != Some("allow") {
        return Err(ApiError::forbidden("Direct compose requires explicit operator docs permission"));
    }
    Ok(())
}

async fn execute_shell_run(
    state: AppState,
    projection: WorkflowProjection,
    run: WorkflowRun,
    queued_identity: String,
    check_approval: impl FnOnce(&ValidatedDirectAction) -> Result<(), ApiError>,
) {
    execute_direct_shell_run(state, projection, run, queued_identity, None, check_approval).await;
}

async fn execute_scheduled_shell_run(
    state: AppState,
    projection: WorkflowProjection,
    run: WorkflowRun,
    (epoch, identity): ScheduledActionGrant,
    check_approval: impl FnOnce(&ValidatedDirectAction) -> Result<(), ApiError>,
) {
    execute_direct_shell_run(state, projection, run, identity, Some(epoch), check_approval).await;
}

async fn execute_direct_shell_run(
    state: AppState,
    projection: WorkflowProjection,
    mut run: WorkflowRun,
    queued_identity: String,
    scheduled_epoch: Option<i64>,
    check_approval: impl FnOnce(&ValidatedDirectAction) -> Result<(), ApiError>,
) {
    if scheduled_direct_trigger(&run.trigger) != scheduled_epoch.is_some() {
        return;
    }
    let guard = if scheduled_epoch.is_some() {
        Some(state.inner.workflow_reconcile.lock().await)
    } else {
        None
    };
    let start = state.inner.store.start_workflow_shell_run(&run).await;
    match start {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            tracing::error!(%error, run_id = %run.id, "failed to start workflow shell run");
            return;
        }
    }
    run.started_at = Some(now_millis());
    run.status = "running".into();
    publish_run(&state, &run);
    let result = async {
        if let Some(epoch) = scheduled_epoch {
            if state.services().hosted || state.services().workspace_worker.is_some() {
                bail!("Scheduled shell workflows require a local runtime");
            }
            let current = state.inner.store.scheduled_workflow_action_grant(&projection).await?;
            if !current.as_ref().is_some_and(|(current_epoch, identity)| *current_epoch == epoch && identity == &queued_identity) {
                bail!("Scheduled action activation was paused or changed");
            }
        }
        dispatch_workflow_action(&state, &projection, &queued_identity, check_approval, guard).await
    }.await;
    match result {
        Ok(DirectResult::Shell(result)) => {
            if result.exit_code != 0 {
                run.error = Some(format!("Shell exited with status {}", result.exit_code));
            }
            run.shell_result = Some(result);
        }
        Ok(DirectResult::Compose(result)) => run.compose_result = Some(result),
        Err(error) => run.error = Some(error.to_string()),
    }
    run.finished_at = Some(now_millis());
    run.status = if run.error.is_none() { "completed" } else { "failed" }.into();
    match state.inner.store.finish_workflow_shell_run(&run).await {
        Ok(true) => publish_run(&state, &run),
        Ok(false) => {}
        Err(error) => tracing::error!(%error, run_id = %run.id, "failed to finish workflow shell run"),
    }
}

enum DirectResult {
    Shell(WorkflowShellResult),
    Compose(WorkflowComposeResult),
}

async fn dispatch_workflow_compose(
    state: &AppState,
    projection: &WorkflowProjection,
    approval_identity: &str,
    check_approval: impl FnOnce(&ValidatedDirectAction) -> Result<(), ApiError>,
) -> anyhow::Result<WorkflowComposeResult> {
    // Do not initialize or reconcile workspace plugins from an unattended run.
    let runtime = state.inner.workspace_runtimes.loaded_for_tenant("local", &projection.workspace_root)
        .await.context("Direct compose requires a loaded local workspace catalog")?;
    // The runtime lookup can suspend. Revalidate bytes and cwd after it returns.
    let action = prepare_workflow_shell(
        &projection.workspace_root, &projection.definition, &projection.source_path, &projection.source_hash,
    ).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    action.require_approval(Some(approval_identity))?;
    check_approval(&action).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let DirectAction::Compose { code, tool_ids } = action.action() else {
        bail!("Not a fixed compose action");
    };
    let rules = [PermissionRule {
        permission: "docs".into(),
        pattern: "*".into(),
        action: PermissionAction::Allow,
    }];
    let result = if crate::tool_composition::metadata::is_metadata(tool_ids) {
        anyhow::ensure!(!state.services().hosted && state.services().workspace_worker.is_none(),
            "metadata composition requires a local runtime");
        crate::tool_composition::metadata::run(
            state, &projection.workspace_root, runtime.snapshot(), tool_ids, code, approval_identity,
        ).await?
    } else {
        crate::tool_composition::unattended::run(
            state, runtime.snapshot(), &rules, tool_ids, code, Default::default(),
        ).await.map_err(anyhow::Error::msg)?
    };
    anyhow::ensure!(result.ok, "Direct compose program failed");
    let fits = crate::tool_composition::serialized_size(&result.value, 16 * 1024).is_ok();
    Ok(WorkflowComposeResult {
        value: if fits { result.value } else { Value::Null },
        truncated: result.truncated || !fits,
    })
}

async fn dispatch_workflow_action(
    state: &AppState,
    projection: &WorkflowProjection,
    queued_identity: &str,
    check_approval: impl FnOnce(&ValidatedDirectAction) -> Result<(), ApiError>,
    dispatch_guard: Option<tokio::sync::MutexGuard<'_, ()>>,
) -> anyhow::Result<DirectResult> {
    #[cfg(not(unix))]
    {
        let _ = (state, projection, queued_identity, check_approval, dispatch_guard);
        bail!("Direct shell workflows require Unix");
    }
    #[cfg(unix)]
    {
        // Re-read the file and resolve cwd after queueing. The immutable, checked
        // command is passed to the runner without any trigger/template expansion.
        let action = prepare_workflow_shell(
            &projection.workspace_root,
            &projection.definition,
            &projection.source_path,
            &projection.source_hash,
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        action.require_approval(Some(queued_identity))?;
        if matches!(action.action(), DirectAction::Compose { .. }) {
            let result = dispatch_workflow_compose(state, projection, queued_identity, check_approval).await;
            drop(dispatch_guard);
            return result.map(DirectResult::Compose);
        }
        check_approval(&action).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let DirectAction::Shell { command } = action.action() else {
            bail!("Not a fixed shell action");
        };
        let directory = open_shell_path(action.directory(), true)
            .context("could not pin approved shell cwd")?;
        let runner = crate::local_process::run_in_pinned_directory(
            crate::local_process::ProcessSpec {
                executable: "/bin/sh".into(),
                args: vec!["-c".into(), command.clone()],
                cwd: None,
                env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
                stdin: None,
                timeout_ms: Some(30_000),
            },
            directory,
        );
        // The bounded runner spawns before its first suspension. Serialize
        // that dispatch point with pause, without locking for process lifetime.
        tokio::pin!(runner);
        let started = futures::poll!(&mut runner);
        drop(dispatch_guard);
        let captured = match started {
            std::task::Poll::Ready(result) => result?,
            std::task::Poll::Pending => runner.await?,
        };

        const HISTORY_STREAM_BYTES: usize = 4096;
        let truncated = captured.truncated
            || captured.stdout.len() > HISTORY_STREAM_BYTES
            || captured.stderr.len() > HISTORY_STREAM_BYTES;
        let text = |bytes: &[u8]| {
            String::from_utf8_lossy(&bytes[..bytes.len().min(HISTORY_STREAM_BYTES)]).into_owned()
        };
        Ok(DirectResult::Shell(WorkflowShellResult {
            exit_code: captured.status,
            stdout: text(&captured.stdout),
            stderr: text(&captured.stderr),
            truncated,
        }))
    }
}

async fn recover_unfinished_runs(state: &AppState) -> anyhow::Result<()> {
    for mut run in state.inner.store.list_unfinished_workflow_runs().await? {
        // Scheduler startup can race an operator's first manual invocation.
        // A live compose lease owned by this process is not restart work.
        if matches!(run.trigger.as_str(), MANUAL_COMPOSE | SCHEDULED_COMPOSE)
            && run.lease_owner.as_deref() == Some(state.inner.execution_owner_id.as_str())
            && run.lease_expires_at.is_some_and(|expiry| expiry > now_millis())
        {
            continue;
        }
        run.status = "interrupted".to_string();
        let message = "server restarted before the workflow run completed".to_string();
        run.error = Some(message.clone());
        run.finished_at = Some(now_millis());
        let projection = state.inner.store.get_workflow(&run.activation_id).await?;
        if let Some(projection) = projection.filter(|projection| {
            run.trigger != "mcpEvent" && !scheduled_direct_trigger(&run.trigger) && run.trigger != MANUAL_COMPOSE
                && projection.definition.action.is_none()
                && run.attempt < projection.definition.retry.max_attempts
                && error_is_retryable(&projection.definition.retry, &message)
        }) {
            let due = now_millis().saturating_add(retry_delay(
                &projection.definition.retry,
                run.attempt.saturating_add(1),
            ));
            let mut retry = new_retry_run(&run, due, "recovery-retry");
            retry.status = "retry_waiting".into();
            retry.next_attempt_at = Some(due);
            if state
                .inner
                .store
                .schedule_workflow_retry(&run, &retry)
                .await?
            {
                publish_run(state, &retry);
            }
        } else {
            state
                .inner
                .store
                .update_workflow_run(
                    &run.id,
                    "interrupted",
                    run.session_id.as_deref(),
                    run.error.as_deref(),
                    true,
                )
                .await?;
        }
        publish_run(state, &run);
    }
    Ok(())
}

fn workspace_root(directory: String) -> Result<String, ApiError> {
    let path = crate::windows_process::canonicalize_path(FsPath::new(&directory))
        .map_err(|error| {
            ApiError::bad_request(format!(
                "workflow directory is not accessible: {error}"
            ))
        })?;
    if !path.is_dir() {
        return Err(ApiError::bad_request(
            "workflow directory is not a directory",
        ));
    }
    Ok(path.display().to_string())
}

fn workflow_execution_directory(
    services: &neoism_agent_service_api::AgentServices,
    projection: &WorkflowProjection,
) -> Result<String, ApiError> {
    let default_directory = if workflow_source_is_global(
        services,
        &projection.workspace_root,
        &projection.source_path,
    ) {
        dirs::home_dir()
            .ok_or_else(|| ApiError::bad_request("home directory is unavailable"))?
    } else {
        PathBuf::from(&projection.workspace_root)
    };
    let root = crate::windows_process::canonicalize_path(&default_directory).map_err(
        |error| {
            ApiError::bad_request(format!(
                "workflow default directory is not accessible: {error}"
            ))
        },
    )?;
    let Some(directory) = projection.definition.directory.as_deref() else {
        return Ok(root.display().to_string());
    };
    let configured = if directory == "~" {
        dirs::home_dir()
            .ok_or_else(|| ApiError::bad_request("home directory is unavailable"))?
    } else if let Some(relative) = directory
        .strip_prefix("~/")
        .or_else(|| directory.strip_prefix("~\\"))
    {
        dirs::home_dir()
            .ok_or_else(|| ApiError::bad_request("home directory is unavailable"))?
            .join(relative)
    } else {
        let path = PathBuf::from(directory);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    };
    let candidate =
        crate::windows_process::canonicalize_path(&configured).map_err(|error| {
            ApiError::bad_request(format!(
                "workflow directory `{directory}` is not accessible: {error}"
            ))
        })?;
    if !candidate.is_dir() {
        return Err(ApiError::bad_request(format!(
            "workflow directory `{directory}` is not a directory"
        )));
    }
    Ok(candidate.display().to_string())
}

fn workflow_source_is_global(
    services: &neoism_agent_service_api::AgentServices,
    workspace: &str,
    source_path: &str,
) -> bool {
    if let Some(root) = workspace.strip_prefix(INSTALLATION_CONTEXT_PREFIX) {
        return FsPath::new(source_path).starts_with(root);
    }
    let source = FsPath::new(source_path);
    let workspace = PathBuf::from(workspace);
    crate::config::roots(services, workspace.to_string_lossy().as_ref())
        .into_iter()
        .any(|root| !root.starts_with(&workspace) && source.starts_with(root))
}

pub(crate) fn activation_id(workspace: &str, workflow_id: &str) -> String {
    let digest = Sha256::digest(format!("{workspace}\0{workflow_id}").as_bytes());
    format!("wfa_{:x}", digest)
}

pub(crate) fn publish_workflow(state: &AppState, projection: &WorkflowProjection) {
    state.publish(EventPayload::new(
        event_type::WORKFLOW_UPDATED,
        json!({
            "aggregateID": projection.activation_id,
            "workflow": projection,
        }),
    ));
}

fn publish_run(state: &AppState, run: &WorkflowRun) {
    // Output belongs in scoped run history, not in the workflow event stream.
    let mut run = run.clone();
    run.shell_result = None;
    run.compose_result = None;
    state.publish(EventPayload::new(
        event_type::WORKFLOW_RUN_UPDATED,
        json!({
            "aggregateID": run.activation_id,
            "run": run,
        }),
    ));
}

#[cfg(test)]
#[path = "workflow_global_tests.rs"]
mod global_tests;

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[tokio::test]
    async fn rune_pin_survives_restart_and_fences_replacement_and_queued_dispatch() {
        use super::*;
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("project");
        let sources = root.join(".agent/workflows");
        fs::create_dir_all(&sources).unwrap();
        let path = sources.join("pin.md");
        let text = "---\nid: pin\nname: Pinned\nactive: true\nschedule:\n  frequency: daily\n  time: '09:00'\n---\nA harmless fixture.\n";
        fs::write(&path, text).unwrap();
        let db = base.join("agent.db");
        let state = AppState::open_database(&db).await.unwrap();
        let source = local_prompt_source(&state, root.clone(), "pin").await.unwrap();
        let workspace = root.to_str().unwrap();
        let id = activation_id(workspace, "pin");
        let mut projection = WorkflowProjection {
            activation_id: id.clone(), workflow_id: "pin".into(), workspace_root: workspace.into(),
            source_path: source.source_path, source_hash: source.source_hash, definition: source.definition,
            active: true, activated_at: now_millis(), last_scheduled_at: None, updated: now_millis(),
        };
        let origin = source.local_origin.unwrap();
        state.inner.store.pin_rune_prompt(&projection, &origin).await.unwrap();
        let (epoch, _) = state.inner.store.rune_prompt_pin(&id).await.unwrap().unwrap();
        assert!(rune_dispatch_allowed(&state, &projection, Some(epoch)).await.unwrap());
        let run = new_run(&projection, now_millis(), RUNE_SCHEDULED_PROMPT);
        assert!(state.inner.store.claim_scheduled_rune_prompt(&run, &projection.definition.concurrency, epoch).await.unwrap());

        // A replacement with identical bytes must still invalidate consent.
        let replacement = base.join("replacement.md");
        fs::write(&replacement, text).unwrap();
        fs::rename(&replacement, &path).unwrap();
        execute_prompt_run(state.clone(), projection.clone(), run.clone(), Some(epoch)).await;
        let rejected = state.inner.store.get_workflow_run(&run.id).await.unwrap().unwrap();
        assert_eq!(rejected.status, "failed");
        assert_eq!(rejected.session_id, None);
        assert!(!state.inner.store.get_workflow(&id).await.unwrap().unwrap().active);
        drop(state);

        let restarted = AppState::open_database(&db).await.unwrap();
        assert_eq!(restarted.inner.store.rune_prompt_pin(&id).await.unwrap().unwrap().1, origin);
        assert!(restarted.inner.store.upsert_workflow(&projection).await.is_err());
        assert!(!restarted.inner.store.claim_scheduled_rune_prompt(&new_run(&projection, run.scheduled_at + 1, RUNE_SCHEDULED_PROMPT), &projection.definition.concurrency, epoch).await.unwrap());
        let current = local_prompt_source(&restarted, root.clone(), "pin").await.unwrap();
        restarted.inner.store.pin_rune_prompt(&projection, current.local_origin.as_deref().unwrap()).await.unwrap();
        let (fresh, _) = restarted.inner.store.rune_prompt_pin(&id).await.unwrap().unwrap();
        assert_ne!(fresh, epoch);
        assert!(!rune_dispatch_allowed(&restarted, &projection, Some(epoch)).await.unwrap());
        assert!(rune_dispatch_allowed(&restarted, &projection, Some(fresh)).await.unwrap());

        fs::write(&path, text.replace("harmless", "different harmless")).unwrap();
        reconcile_workspaces(&restarted).await.unwrap();
        assert!(!restarted.inner.store.get_workflow(&id).await.unwrap().unwrap().active);
        let changed = local_prompt_source(&restarted, root, "pin").await.unwrap();
        projection.source_hash = changed.source_hash;
        projection.definition = changed.definition;
        restarted.inner.store.pin_rune_prompt(&projection, changed.local_origin.as_deref().unwrap()).await.unwrap();
        assert!(restarted.inner.store.get_workflow(&id).await.unwrap().unwrap().active);
        drop(restarted);

        let moved = sources.join("moved.md");
        fs::rename(&path, &moved).unwrap();
        let restarted = AppState::open_database(&db).await.unwrap();
        assert!(restarted.inner.store.get_workflow(&id).await.unwrap().unwrap().active);
        reconcile_workspaces(&restarted).await.unwrap();
        assert!(!restarted.inner.store.get_workflow(&id).await.unwrap().unwrap().active);
    }
    use super::*;
    use crate::state::SessionStore;
    use chrono::Timelike;

    fn schedule(frequency: &str) -> WorkflowSchedule {
        WorkflowSchedule {
            frequency: frequency.to_string(),
            interval: 1,
            timezone: "UTC".to_string(),
            minute: None,
            time: None,
            weekdays: Vec::new(),
            month_day: None,
            date: None,
            at: None,
            cron: None,
        }
    }

    #[test]
    fn parses_strict_markdown_workflow() {
        let root =
            std::env::temp_dir().join(format!("neoism-workflow-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("daily.md");
        std::fs::write(&path, "---\nid: daily-review\nname: Daily review\nschedule:\n  frequency: daily\n  time: '09:30'\n---\nReview the workspace.\n").unwrap();
        let source = parse_source(&path).unwrap();
        assert_eq!(source.definition.id, "daily-review");
        assert_eq!(source.definition.prompt, "Review the workspace.");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    const SHELL_SOURCE: &str = "---\nid: shell-test\nname: Shell test\nschedule:\n  date: 2099-09-15\n  time: '09:30'\naction:\n  kind: shell\n  command: |\n    printf '%s\\n' \"$PWD\"\n    printf '%4100s' x\n    printf 'oops' >&2\n    exit 7\n---\n";

    #[cfg(unix)]
    fn shell_source(root: &FsPath) -> (PathBuf, String) {
        let workspace = root.canonicalize().unwrap();
        let path = workspace.join(".agent/workflows/shell-test.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, SHELL_SOURCE).unwrap();
        (path, workspace.to_str().unwrap().to_owned())
    }

    #[cfg(unix)]
    #[test]
    fn direct_workflow_file_cannot_activate_or_supply_approval() {
        let root = tempfile::tempdir().unwrap();
        let (path, _) = shell_source(root.path());
        let definition = parse_source(&path).unwrap().definition;
        assert!(reject_direct_workflow(&definition).is_err());
        assert!(parse_source(&path).unwrap().definition.prompt.is_empty());
        for field in ["active: true", "approval: approved", "permissions: {bash: allow}"] {
            fs::write(&path, SHELL_SOURCE.replacen("action:\n", &format!("{field}\naction:\n"), 1)).unwrap();
            assert!(parse_source(&path).is_err(), "accepted {field}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_shell_rechecks_stale_source_and_persists_bounded_result() {
        let root = tempfile::tempdir().unwrap();
        let (path, workspace) = shell_source(root.path());
        let state = AppState::open_database(root.path().join("state.sqlite3")).await.unwrap();
        let source = parse_source(&path).unwrap();
        let projection = ensure_projection(&state, workspace.clone(), source, false).await.unwrap();
        let action = prepare_workflow_shell(&workspace, &projection.definition, &projection.source_path, &projection.source_hash).unwrap();
        let identity = action.approval_identity().to_owned();
        assert!(workflow_action::require_shell_approval(&action, None).is_err());
        assert!(workflow_action::require_shell_approval(&action, Some(&format!("{identity},*"))).is_err());
        let run = claim_manual_run(&state, &projection).await.unwrap();
        fs::write(&path, format!("{SHELL_SOURCE}\n")).unwrap();
        execute_shell_run(state.clone(), projection.clone(), run.clone(), identity.clone(), |_| {
            panic!("stale source should fail before the grant check")
        }).await;
        let stale = state.inner.store.get_workflow_run(&run.id).await.unwrap().unwrap();
        assert_eq!(stale.status, "failed");
        assert!(stale.error.unwrap().contains("source changed"));
        assert!(stale.shell_result.is_none());

        fs::write(&path, SHELL_SOURCE).unwrap();
        // Use a distinct occurrence key even on clocks with coarse resolution.
        let mut run = new_run(&projection, run.scheduled_at + 1, "manual");
        assign_run_lease(&state, &mut run);
        assert!(state.inner.store.claim_workflow_run(&run, &projection.definition.concurrency).await.unwrap());
        execute_shell_run(state.clone(), projection, run.clone(), identity.clone(), |action| {
            workflow_action::require_shell_approval(action, Some(&identity)).map_err(shell_admission_error)
        }).await;
        let finished = state.inner.store.get_workflow_run(&run.id).await.unwrap().unwrap();
        assert_eq!(finished.status, "failed");
        assert!(finished.session_id.is_none());
        let result = finished.shell_result.unwrap();
        assert_eq!(result.exit_code, 7);
        assert!(result.stdout.starts_with(&format!("{workspace}\n")));
        assert_eq!(result.stdout.len(), 4096);
        assert_eq!(result.stderr, "oops");
        assert!(result.truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scheduled_shell_uses_durable_activation_and_rechecks_before_spawn() {
        let root = tempfile::tempdir().unwrap();
        let (path, workspace) = shell_source(root.path());
        let text = SHELL_SOURCE
            .replace("  date: 2099-09-15\n  time: '09:30'", "  frequency: cron\n  cron: '0 * * * *'")
            .replace("    exit 7", "    exit 0");
        fs::write(&path, &text).unwrap();
        let db = root.path().join("scheduled.sqlite3");
        let state = AppState::open_database(&db).await.unwrap();
        let id = activation_id(&workspace, "shell-test");
        let source = parse_source(&path).unwrap();
        let action = prepare_workflow_shell(&workspace, &source.definition, &source.source_path, &source.source_hash).unwrap();
        let identity = action.approval_identity().to_owned();
        let allow = |action: &ValidatedDirectAction| {
            workflow_action::require_shell_approval(action, Some(&identity)).map_err(shell_admission_error)
        };
        assert!(activate_source(&state, workspace.clone(), "shell-test".into(), parse_source(&path).unwrap(), None).await.is_err());
        let inactive = ensure_projection(&state, workspace.clone(), parse_source(&path).unwrap(), false).await.unwrap();
        assert!(state.inner.store.scheduled_workflow_action_grant(&inactive).await.unwrap().is_none());
        let activate = |state: AppState| {
            let workspace = workspace.clone();
            let source = parse_source(&path).unwrap();
            let identity = identity.clone();
            async move {
                let _ = activate_source(&state, workspace, "shell-test".into(), source, Some(&identity)).await.unwrap();
            }
        };
        allow(&action).unwrap();
        activate(state.clone()).await;
        drop(state);
        let state = AppState::open_database(&db).await.unwrap();
        let load = || state.inner.store.get_workflow(&id);
        let projection = load().await.unwrap().unwrap();
        let hour = 3_600_000;
        let slot = (projection.activated_at / hour + 1) * hour;
        let (run, grant) = claim_due_shell(&state, &projection, slot).await.unwrap().unwrap();
        execute_scheduled_shell_run(state.clone(), projection, run.clone(), grant, allow).await;
        let finished = state.inner.store.get_workflow_run(&run.id).await.unwrap().unwrap();
        assert_eq!(finished.status, "completed");
        assert_eq!(finished.shell_result.unwrap().exit_code, 0);
        let projection = load().await.unwrap().unwrap();
        assert!(claim_due_shell(&state, &projection, slot).await.unwrap().is_none());
        assert!(ensure_projection(&state, workspace.clone(), parse_source(&path).unwrap(), false).await.unwrap().active);

        let (run, grant) = claim_due_shell(&state, &projection, slot + hour).await.unwrap().unwrap();
        state.inner.store.set_workflow_active(&id, false, now_millis()).await.unwrap();
        assert!(state.inner.store.scheduled_workflow_action_grant(&projection).await.unwrap().is_none());
        // Even reactivating exactly the same revision cannot revive this queued run.
        activate(state.clone()).await;
        execute_scheduled_shell_run(state.clone(), projection, run.clone(), grant, |_| panic!("old activation must not dispatch")).await;
        let paused = state.inner.store.get_workflow_run(&run.id).await.unwrap().unwrap();
        assert!(paused.error.unwrap().contains("paused or changed"));
        assert!(paused.shell_result.is_none());

        let projection = load().await.unwrap().unwrap();
        let (run, grant) = claim_due_shell(&state, &projection, slot + 2 * hour).await.unwrap().unwrap();
        execute_scheduled_shell_run(state.clone(), projection, run.clone(), grant, |action| {
            workflow_action::require_shell_approval(action, None).map_err(shell_admission_error)
        }).await;
        let denied = state.inner.store.get_workflow_run(&run.id).await.unwrap().unwrap();
        assert!(denied.error.unwrap().contains("requires approval"));
        assert!(denied.shell_result.is_none());

        let projection = load().await.unwrap().unwrap();
        let (run, grant) = claim_due_shell(&state, &projection, slot + 3 * hour).await.unwrap().unwrap();
        fs::write(&path, format!("{text}\n")).unwrap();
        execute_scheduled_shell_run(state.clone(), projection.clone(), run.clone(), grant, |_| panic!("stale source must not dispatch")).await;
        let stale = state.inner.store.get_workflow_run(&run.id).await.unwrap().unwrap();
        assert!(stale.error.unwrap().contains("source changed"));
        assert!(stale.shell_result.is_none());
        assert!(claim_due_shell(&state, &projection, slot + 4 * hour).await.unwrap().is_none());
        assert!(!load().await.unwrap().unwrap().active);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn direct_compose_manual_and_scheduled_are_fenced() {
        use neoism_agent_service_api::{DocumentationPage, DocumentationPageSummary,
            DocumentationSearchHit, DocumentationService, ServiceError};

        struct Docs;
        impl DocumentationService for Docs {
            fn list(&self) -> Result<Vec<DocumentationPageSummary>, ServiceError> {
                Ok(vec![DocumentationPageSummary { path: "Start.md".into(), title: "Start".into() }])
            }
            fn search(&self, _: &str, _: usize) -> Result<Vec<DocumentationSearchHit>, ServiceError> {
                panic!("search is unavailable")
            }
            fn read(&self, _: &str) -> Result<DocumentationPage, ServiceError> {
                panic!("read is unavailable")
            }
        }

        let root = tempfile::tempdir().unwrap();
        let (path, workspace) = shell_source(root.path());
        let text = "---\nid: shell-test\nname: Compose test\nschedule:\n  frequency: cron\n  cron: '0 * * * *'\naction:\n  kind: compose\n  toolIds: [docs]\n  code: |\n    return await tools.neoism.docs({operation: 'list'});\n---\n";
        fs::write(&path, text.replace("toolIds: [docs]", "toolIds: [docs, bash]")).unwrap();
        assert!(parse_source(&path).is_err());
        fs::write(&path, text).unwrap();
        let state = AppState::open_database_with_services(root.path().join("compose.db"),
            crate::standard_services().with_documentation(std::sync::Arc::new(Docs))).await.unwrap();
        // Drive scheduler claims explicitly against fixed slots in this test.
        state.inner.workflow_scheduler_started.store(true, std::sync::atomic::Ordering::SeqCst);
        let source = parse_source(&path).unwrap();
        let action = prepare_workflow_shell(&workspace, &source.definition, &source.source_path, &source.source_hash).unwrap();
        let identity = action.approval_identity().to_owned();
        let allow = |action: &ValidatedDirectAction| check_configured_action_approval(action, Some(&identity), Some("allow"));
        allow(&action).unwrap();
        assert!(workflow_action::require_shell_approval(&action, Some(&identity)).is_err());
        assert!(check_configured_action_approval(&action, None, Some("allow")).is_err());
        let _ = activate_source(&state, workspace.clone(), "shell-test".into(), source, Some(&identity)).await.unwrap();
        let id = activation_id(&workspace, "shell-test");
        let projection = state.inner.store.get_workflow(&id).await.unwrap().unwrap();
        assert!(dispatch_workflow_compose(&state, &projection, &identity, allow).await.is_err());
        crate::agent_tool_registry::acquire_workspace_plugin_snapshot(&state, &workspace).await.unwrap();
        let run = claim_manual_run_with_trigger(&state, &projection, MANUAL_COMPOSE).await.unwrap();
        execute_shell_run(state.clone(), projection.clone(), run.clone(), identity.clone(), allow).await;
        let check_result = |id: String| {
            let state = state.clone();
            async move {
                let saved = state.inner.store.get_workflow_run(&id).await.unwrap().unwrap();
                assert_eq!(saved.status, "completed", "{:?}", saved.error);
                assert!(saved.shell_result.is_none());
                assert_eq!(saved.compose_result.unwrap().value["documents"][0]["path"], "Start.md");
            }
        };
        check_result(run.id).await;
        let hour = 3_600_000;
        let slot = (projection.activated_at / hour + 1) * hour;
        let (run, grant) = claim_due_shell(&state, &projection, slot).await.unwrap().unwrap();
        assert_eq!(run.trigger, SCHEDULED_COMPOSE);
        execute_scheduled_shell_run(state.clone(), projection.clone(), run.clone(), grant, allow).await;
        check_result(run.id).await;

        let (run, grant) = claim_due_shell(&state, &projection, slot + hour).await.unwrap().unwrap();
        execute_scheduled_shell_run(state.clone(), projection.clone(), run.clone(), grant, |action| {
            check_configured_action_approval(action, Some(&identity), Some("deny"))
        }).await;
        let denied = state.inner.store.get_workflow_run(&run.id).await.unwrap().unwrap();
        assert!(denied.error.unwrap().contains("docs permission"));
        assert!(denied.compose_result.is_none());

        let (run, grant) = claim_due_shell(&state, &projection, slot + 2 * hour).await.unwrap().unwrap();
        fs::write(&path, format!("{text}\n")).unwrap();
        execute_scheduled_shell_run(state.clone(), projection, run.clone(), grant, |_| panic!("stale source must not dispatch")).await;
        let stale = state.inner.store.get_workflow_run(&run.id).await.unwrap().unwrap();
        assert!(stale.error.unwrap().contains("source changed"));
        assert!(stale.compose_result.is_none());
    }

    #[tokio::test]
    async fn agent_workflow_draft_is_paused_and_cannot_overwrite_a_source() {
        let unique = Id::ascending(IdKind::Event).to_string();
        let root = std::env::temp_dir().join(format!("neoism-workflow-draft-{unique}"));
        fs::create_dir_all(&root).unwrap();
        let state = AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let workspace = root.to_str().unwrap();
        let permissions = [PermissionRule {
            permission: "*".into(),
            pattern: "*".into(),
            action: PermissionAction::Allow,
        }];
        let id = format!("draft-{}", unique.to_ascii_lowercase());
        let path = root.join(".agent/workflows").join(format!("{id}.md"));
        let mut input = json!({"action":"draft", "definition":{
            "id": id, "name":"Review", "prompt":"List changed files.", "active":true,
            "schedule":{"frequency":"daily", "time":"09:00"}
        }});
        assert!(agent_tool(&state, workspace, &permissions, input.clone()).await.is_err());
        assert!(!path.exists());

        input["definition"]["active"] = json!(false);
        assert!(agent_tool(&state, workspace, &permissions, input.clone()).await.is_ok());
        let source = parse_source(&path).unwrap();
        assert!(!source.definition.active);
        input["definition"]["prompt"] = json!("Replacement prompt.");
        assert!(agent_tool(&state, workspace, &permissions, input).await.is_err());
        assert_eq!(parse_source(&path).unwrap().definition.prompt, "List changed files.");
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn agent_workflow_lifecycle_requires_permission_and_current_revision() {
        let unique = Id::ascending(IdKind::Event).to_string();
        let root = std::env::temp_dir().join(format!("neoism-workflow-lifecycle-{unique}"));
        let workflow_dir = root.join(".agent/workflows");
        fs::create_dir_all(&workflow_dir).unwrap();
        let path = workflow_dir.join("approval-flow.md");
        let original = "---\nid: approval-flow\nname: Review\nschedule:\n  date: 2099-09-15\n  time: '09:30'\n---\nReview changed files.\n";
        fs::write(&path, original).unwrap();
        let state = AppState::open_database(root.join("state.sqlite3")).await.unwrap();
        let workspace = workspace_root(root.display().to_string()).unwrap();
        let allow = |permission: &str, pattern: String| PermissionRule {
            permission: permission.into(),
            pattern,
            action: PermissionAction::Allow,
        };
        let mut permissions = vec![allow("workflow", "*".into())];
        let revision = format!("sha256:{}", parse_source(&path).unwrap().source_hash);
        let input = json!({"action":"activate", "id":"approval-flow", "revision":revision});
        let err = agent_tool(&state, &workspace, &permissions, input.clone()).await.unwrap_err();
        assert_eq!(crate::permission_runtime::parse_permission_required_error(&err),
            Some(("workflow_activate".into(), vec![format!("approval-flow@{revision}")])));
        permissions.push(allow("workflow_activate", format!("approval-flow@{revision}")));
        fs::write(&path, format!("{original}Check formatting too.\n")).unwrap();
        let err = agent_tool(&state, &workspace, &permissions, input).await.unwrap_err();
        assert!(err.contains("Workflow changed"));
        let activation = activation_id(&workspace, "approval-flow");
        assert!(state.inner.store.get_workflow(&activation).await.unwrap().is_none());

        let revision = format!("sha256:{}", parse_source(&path).unwrap().source_hash);
        permissions.push(allow("workflow_activate", format!("approval-flow@{revision}")));
        permissions.push(allow("workflow_pause", "approval-flow".into()));
        agent_tool(&state, &workspace, &permissions,
            json!({"action":"activate", "id":"approval-flow", "revision":revision})).await.unwrap();
        assert!(state.inner.store.get_workflow(&activation).await.unwrap().unwrap().active);
        agent_tool(&state, &workspace, &permissions,
            json!({"action":"pause", "id":"approval-flow"})).await.unwrap();
        assert!(!state.inner.store.get_workflow(&activation).await.unwrap().unwrap().active);
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cron_round_trips_file_schedule_and_previews_local_weekdays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("review.md");
        fs::write(
            &path,
            "---\nid: review\nname: Review\nschedule:\n  frequency: cron\n  cron: '30 9 * * MON-FRI'\n  timezone: America/New_York\n---\nReview.\n",
        )
        .unwrap();
        let source = parse_source(&path).unwrap();
        fs::write(&path, workflow_markdown(&source.definition).unwrap()).unwrap();
        let schedule = parse_source(&path).unwrap().definition.schedule;
        assert_eq!(schedule.cron.as_deref(), Some("30 9 * * MON-FRI"));

        // At Friday's exact slot, preview must skip to Monday; DST changes over the weekend.
        let friday = utc_millis(2026, 3, 6, 14, 30);
        assert_eq!(
            preview_slots(&schedule, friday, 2).unwrap(),
            [utc_millis(2026, 3, 9, 13, 30), utc_millis(2026, 3, 10, 13, 30)]
        );
        assert_eq!(next_slot(&schedule, friday - 1).unwrap(), friday);
    }

    fn utc_millis(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .timestamp_millis() as u64
    }

    #[test]
    fn cron_due_coalesces_downtime_and_respects_activation_and_milliseconds() {
        let mut cron = schedule("cron");
        cron.cron = Some("*/5 * * * *".into());
        let old = utc_millis(2020, 1, 1, 0, 0);
        let slot = utc_millis(2026, 8, 27, 12, 5);
        assert_eq!(latest_due(&cron, old, slot + 987).unwrap(), Some(slot));
        assert_eq!(latest_due(&cron, slot, slot).unwrap(), Some(slot));
        assert_eq!(latest_due(&cron, slot + 1, slot + 987).unwrap(), None);
        assert_eq!(latest_due(&cron, slot, slot - 1).unwrap(), None);
        assert!(next_slot(&cron, u64::MAX).is_err());

        cron.cron = Some("0 0 29 FEB *".into());
        let past_leap = utc_millis(2096, 2, 29, 0, 0);
        let next_leap = utc_millis(2104, 2, 29, 0, 0);
        assert_eq!(next_slot(&cron, past_leap).unwrap(), next_leap);
        assert_eq!(latest_due(&cron, old, next_leap - 1).unwrap(), Some(past_leap));
    }

    #[test]
    fn cron_fixed_time_handles_dst_gap_and_fold() {
        let mut cron = schedule("cron");
        cron.timezone = "America/New_York".into();
        cron.cron = Some("30 2 * * *".into());
        let gap_end = utc_millis(2026, 3, 8, 7, 0);
        assert_eq!(next_slot(&cron, gap_end - 1).unwrap(), gap_end);
        assert_eq!(latest_due(&cron, gap_end, gap_end + 1).unwrap(), Some(gap_end));

        cron.cron = Some("30 1 * * *".into());
        let early = utc_millis(2026, 11, 1, 5, 30);
        let late = utc_millis(2026, 11, 1, 6, 30);
        assert_eq!(next_slot(&cron, early - 1).unwrap(), early);
        assert_eq!(next_slot(&cron, early).unwrap(), utc_millis(2026, 11, 2, 6, 30));
        assert_eq!(latest_due(&cron, early, late).unwrap(), Some(early));
    }

    #[test]
    fn rejects_invalid_cron_schedules() {
        let valid = json!({"frequency": "cron", "cron": "0 9 * * 1-5"});
        let parse = |value| serde_json::from_value::<WorkflowSchedule>(value).unwrap();
        let cron = parse(valid.clone());
        assert_eq!(cron.timezone, "UTC");
        assert_eq!(cron.interval, 1);
        validate_schedule(&cron).unwrap();
        assert!(serde_json::to_value(schedule("daily")).unwrap().get("cron").is_none());
        for (key, value) in [
            ("cron", json!(null)),
            ("cron", json!("0 0 9 * * 1-5")),
            ("cron", json!("*/0 * * * *")),
            ("cron", json!("0 0 31 FEB *")),
            ("cron", json!(format!("{}0 9 * * *", " ".repeat(256)))),
            ("timezone", json!("+05:00")),
            ("timezone", json!("local")),
            ("timezone", json!("America/Unknown")),
            ("interval", json!(2)),
            ("date", json!("2099-01-01")),
            ("at", json!("2099-01-01T00:00:00Z")),
            ("minute", json!(0)),
            ("time", json!("09:00")),
            ("weekdays", json!(["mon"])),
            ("monthDay", json!(1)),
            ("frequency", json!("daily")),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            assert!(validate_schedule(&parse(invalid.clone())).is_err(), "{invalid}");
        }
    }

    #[test]
    fn calculates_month_end_and_dst_slots() {
        let mut monthly = schedule("monthly");
        monthly.month_day = Some(31);
        monthly.time = Some("10:00".to_string());
        let after = Utc
            .with_ymd_and_hms(2027, 1, 31, 10, 0, 0)
            .unwrap()
            .timestamp_millis() as u64;
        let next = Utc
            .timestamp_millis_opt(next_slot(&monthly, after).unwrap() as i64)
            .unwrap();
        assert_eq!((next.year(), next.month(), next.day()), (2027, 2, 28));

        let mut daily = schedule("daily");
        daily.timezone = "America/New_York".to_string();
        daily.time = Some("02:30".to_string());
        let after = Utc
            .with_ymd_and_hms(2027, 3, 13, 8, 0, 0)
            .unwrap()
            .timestamp_millis() as u64;
        let next = Utc
            .timestamp_millis_opt(next_slot(&daily, after).unwrap() as i64)
            .unwrap()
            .with_timezone(&daily.timezone.parse::<Tz>().unwrap());
        assert_eq!((next.day(), next.hour(), next.minute()), (14, 3, 0));
    }

    #[test]
    fn rejects_frequency_specific_fields() {
        let mut definition = WorkflowDefinition {
            id: "test".to_string(),
            name: "Test".to_string(),
            active: false,
            prompt: "Run".to_string(),
            action: None,
            directory: None,
            schedule: schedule("daily"),
            skill: None,
            agent: None,
            model: None,
            permissions: None,
            retry: WorkflowRetryPolicy::default(),
            concurrency: WorkflowConcurrencyPolicy::default(),
        };
        definition.schedule.minute = Some(10);
        assert!(validate_definition(&definition)
            .unwrap_err()
            .to_string()
            .contains("only accept time"));
    }

    #[test]
    fn calculates_one_time_date() {
        let mut one_time = schedule("once");
        one_time.date = Some("2026-09-15".to_string());
        one_time.time = Some("09:30".to_string());
        one_time.timezone = "America/Los_Angeles".to_string();
        let slot = one_time_slot(&one_time).unwrap();
        let local = Utc
            .timestamp_millis_opt(slot as i64)
            .unwrap()
            .with_timezone(&one_time.timezone.parse::<Tz>().unwrap());
        assert_eq!((local.year(), local.month(), local.day()), (2026, 9, 15));
        assert_eq!((local.hour(), local.minute()), (9, 30));
        assert!(preview_slots(&one_time, slot, PREVIEW_COUNT)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parses_friendly_permissions_and_time_formats() {
        let root = std::env::temp_dir().join(format!(
            "neoism-workflow-permissions-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("notification.md");
        std::fs::write(
            &path,
            "---\nid: notification\nname: Notification\nactive: true\ndirectory: tools\nschedule:\n  date: 2026-09-15\n  time: 10:40 PM\n  timezone: America/Chicago\npermissions:\n  bash:\n    default: deny\n    allow: [notify-send*]\n  edit: deny\n---\nNotify me.\n",
        )
        .unwrap();
        let definition = parse_source(&path).unwrap().definition;
        assert_eq!(definition.directory.as_deref(), Some("tools"));
        let rules = definition.effective_permission_rules().unwrap();
        assert_eq!(
            crate::permission::evaluate("bash", "notify-send test", &rules).action,
            PermissionAction::Allow
        );
        assert_eq!(
            crate::permission::evaluate("bash", "rm file", &rules).action,
            PermissionAction::Deny
        );
        assert_eq!(
            crate::permission::evaluate("edit", "file", &rules).action,
            PermissionAction::Deny
        );
        assert_eq!(parse_time("22:40:30").unwrap().hour(), 22);
        assert_eq!(parse_time("10:40 pm").unwrap().hour(), 22);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn calculates_rfc3339_timestamp() {
        let mut one_time = schedule("once");
        one_time.at = Some("2026-08-23T22:40:00-05:00".to_string());
        validate_definition(&WorkflowDefinition {
            id: "timestamp".to_string(),
            name: "Timestamp".to_string(),
            active: true,
            schedule: one_time.clone(),
            prompt: "Run".to_string(),
            action: None,
            directory: None,
            skill: None,
            agent: None,
            model: None,
            permissions: None,
            retry: WorkflowRetryPolicy::default(),
            concurrency: WorkflowConcurrencyPolicy::default(),
        })
        .unwrap();
        assert_eq!(
            Utc.timestamp_millis_opt(one_time_slot(&one_time).unwrap() as i64)
                .unwrap()
                .to_rfc3339(),
            "2026-08-24T03:40:00+00:00"
        );
    }

    #[test]
    fn resolves_workflow_directory_inside_project_root() {
        let root = std::env::temp_dir().join(format!(
            "neoism-workflow-directory-{}",
            Id::ascending(IdKind::Event)
        ));
        let nested = root.join("packages/app");
        std::fs::create_dir_all(&nested).unwrap();
        let mut projection = WorkflowProjection {
            activation_id: "activation".to_string(),
            workflow_id: "directory".to_string(),
            workspace_root: root.display().to_string(),
            source_path: root.join("workflow.md").display().to_string(),
            source_hash: "hash".to_string(),
            definition: WorkflowDefinition {
                id: "directory".to_string(),
                name: "Directory".to_string(),
                active: true,
                schedule: schedule("daily"),
                prompt: "Run".to_string(),
                action: None,
                directory: Some("packages/app".to_string()),
                skill: None,
                agent: None,
                model: None,
                permissions: None,
                retry: WorkflowRetryPolicy::default(),
                concurrency: WorkflowConcurrencyPolicy::default(),
            },
            active: true,
            activated_at: 0,
            last_scheduled_at: None,
            updated: 0,
        };
        assert_eq!(
            workflow_execution_directory(&crate::standard_services(), &projection)
                .unwrap(),
            nested.canonicalize().unwrap().display().to_string()
        );
        let external = std::env::temp_dir().join(format!(
            "neoism-workflow-external-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&external).unwrap();
        projection.definition.directory = Some(external.display().to_string());
        assert_eq!(
            workflow_execution_directory(&crate::standard_services(), &projection)
                .unwrap(),
            external.canonicalize().unwrap().display().to_string()
        );
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(external);
    }

    #[test]
    fn global_workflow_defaults_to_os_home_directory() {
        let home = dirs::home_dir().expect("home directory");
        let services = crate::standard_services();
        let global_root = crate::config::roots(
            &services,
            std::env::temp_dir().to_string_lossy().as_ref(),
        )
        .remove(0);
        let projection = WorkflowProjection {
            activation_id: "activation".to_string(),
            workflow_id: "global".to_string(),
            workspace_root: std::env::temp_dir().display().to_string(),
            source_path: global_root
                .join("workflows/global.md")
                .display()
                .to_string(),
            source_hash: "hash".to_string(),
            definition: WorkflowDefinition {
                id: "global".to_string(),
                name: "Global".to_string(),
                active: true,
                schedule: schedule("daily"),
                prompt: "Run".to_string(),
                action: None,
                directory: None,
                skill: None,
                agent: None,
                model: None,
                permissions: None,
                retry: WorkflowRetryPolicy::default(),
                concurrency: WorkflowConcurrencyPolicy::default(),
            },
            active: true,
            activated_at: 0,
            last_scheduled_at: None,
            updated: 0,
        };
        assert_eq!(
            workflow_execution_directory(&services, &projection).unwrap(),
            home.canonicalize().unwrap().display().to_string()
        );
    }

    #[tokio::test]
    async fn active_frontmatter_hot_reloads_activation() {
        let unique = Id::ascending(IdKind::Event).to_string();
        let root = std::env::temp_dir().join(format!("neoism-workflow-hot-{unique}"));
        let workflow_dir = root.join(".agent/workflows");
        std::fs::create_dir_all(&workflow_dir).unwrap();
        let workflow_path = workflow_dir.join("once.md");
        let source = |active: bool| {
            format!(
                "---\nid: hot-reload\nname: Hot reload\nactive: {active}\nschedule:\n  date: 2099-09-15\n  time: '09:30'\n---\nRun once.\n"
            )
        };
        std::fs::write(&workflow_path, source(true)).unwrap();
        let database = root.join("state.sqlite3");
        let state = AppState::open_database(database).await.unwrap();
        let workspace = workspace_root(root.display().to_string()).unwrap();
        track_workspace(&state, &workspace).await;
        reconcile_workspaces(&state).await.unwrap();
        let id = activation_id(&workspace, "hot-reload");
        assert!(
            state
                .inner
                .store
                .get_workflow(&id)
                .await
                .unwrap()
                .unwrap()
                .active
        );

        std::fs::write(&workflow_path, source(false)).unwrap();
        reconcile_workspaces(&state).await.unwrap();
        assert!(
            !state
                .inner
                .store
                .get_workflow(&id)
                .await
                .unwrap()
                .unwrap()
                .active
        );
    }

    #[tokio::test]
    async fn scheduled_claim_survives_immediate_restart_without_duplicate_occurrence() {
        assert_scheduled_claim_survives_restart().await;
    }

    async fn assert_scheduled_claim_survives_restart() {
        let path = std::env::temp_dir().join(format!(
            "neoism-workflow-claim-{}.turso.db",
            Id::ascending(IdKind::Event),
        ));
        let projection = WorkflowProjection {
            activation_id: "restart-activation".to_string(),
            workflow_id: "restart-workflow".to_string(),
            workspace_root: std::env::temp_dir().display().to_string(),
            source_path: "restart-workflow.md".to_string(),
            source_hash: "hash".to_string(),
            definition: WorkflowDefinition {
                id: "restart-workflow".to_string(),
                name: "Restart workflow".to_string(),
                active: true,
                schedule: schedule("daily"),
                prompt: "Run once".to_string(),
                action: None,
                directory: None,
                skill: None,
                agent: None,
                model: None,
                permissions: None,
                retry: WorkflowRetryPolicy::default(),
                concurrency: WorkflowConcurrencyPolicy::default(),
            },
            active: true,
            activated_at: 1,
            last_scheduled_at: None,
            updated: 1,
        };
        let slot = 1_800_000_000_000;

        let store = SessionStore::open(path.clone()).await.unwrap();
        store.upsert_workflow(&projection).await.unwrap();
        let first = new_run(&projection, slot, "scheduled");
        assert!(store
            .claim_scheduled_workflow_run(&first, &projection.definition.concurrency)
            .await
            .unwrap());

        // Simulate a crash immediately after claiming, before execution can
        // move the queued run forward, then perform startup recovery.
        store.close().await;
        drop(store);
        let store = SessionStore::open(path.clone()).await.unwrap();
        assert_eq!(
            store
                .get_workflow(&projection.activation_id)
                .await
                .unwrap()
                .unwrap()
                .last_scheduled_at,
            Some(slot)
        );
        let unfinished = store.list_unfinished_workflow_runs().await.unwrap();
        assert_eq!(unfinished.len(), 1);
        store
            .update_workflow_run(
                &unfinished[0].id,
                "interrupted",
                unfinished[0].session_id.as_deref(),
                Some("simulated restart"),
                true,
            )
            .await
            .unwrap();

        let retry = new_run(&projection, slot, "scheduled");
        assert!(!store
            .claim_scheduled_workflow_run(&retry, &projection.definition.concurrency)
            .await
            .unwrap());
        let history = store
            .list_workflow_runs(&projection.activation_id, 10)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, first.id);
        assert_eq!(history[0].status, "interrupted");

        store.close().await;
        for candidate in [
            path.clone(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

    #[test]
    fn managed_write_is_discovered_and_watched_with_native_adapter_roots() {
        use neoism_agent_service_api::*;
        struct NativeRoots;
        impl ConfigSourceService for NativeRoots {
            fn snapshot(
                &self,
                request: &ConfigSnapshotRequest,
            ) -> Result<ConfigSnapshot, ServiceError> {
                let mut snapshot = crate::standard_services().config.snapshot(request)?;
                snapshot.discovery_roots = vec![ConfigDiscoveryRoot {
                    scope: ConfigDiscoveryScope::Workspace,
                    source_id: "native:project".into(),
                    path: snapshot.workspace.join(".neoism"),
                }];
                Ok(snapshot)
            }
            fn update<'a>(
                &'a self,
                _: &'a ConfigUpdateRequest,
            ) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
                Box::pin(async { Err(ServiceError::new("read-only test adapter")) })
            }
        }
        let root = std::env::temp_dir().join(format!(
            "neoism-native-workflow-{}",
            Id::ascending(IdKind::Event)
        ));
        let selected = root.join("selected");
        let other = root.join("default");
        fs::create_dir_all(selected.join(".neoism")).unwrap();
        fs::create_dir_all(&other).unwrap();
        let services =
            crate::standard_services().with_config(std::sync::Arc::new(NativeRoots));
        let workspace =
            workspace_root(selected.join("..").join("selected").display().to_string())
                .unwrap();
        let path =
            managed_workflow_path(FsPath::new(&workspace), "selected-flow").unwrap();
        atomic_workflow_write(FsPath::new(&workspace), &path, b"---\nid: selected-flow\nname: Selected flow\nschedule:\n  date: 2099-09-15\n  time: '09:30'\n---\nRun once.\n").unwrap();
        assert!(selected.join(".agent/workflows/selected-flow.md").is_file());
        assert!(!other.join(".agent").exists());
        let catalog = discover(&services, &workspace).unwrap();
        let source = catalog
            .workflows
            .get("selected-flow")
            .expect("managed workflow must be discoverable under native roots");
        assert!(workflow_source_is_managed(&workspace, source));
        assert_eq!(
            workflow_watch_paths(&services, std::slice::from_ref(&workspace))
                .get(&path.parent().unwrap().canonicalize().unwrap()),
            Some(&RecursiveMode::Recursive)
        );
        assert!(discover(&services, other.to_str().unwrap())
            .unwrap()
            .workflows
            .is_empty());
        assert!(managed_workflow_path(&selected, "../escape").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn watcher_covers_existing_and_not_yet_created_workflow_roots() {
        let unique = Id::ascending(IdKind::Event).to_string();
        let root = std::env::temp_dir().join(format!("neoism-workflow-watch-{unique}"));
        let workflow_root = root.join(".agent/workflows");
        std::fs::create_dir_all(&workflow_root).unwrap();
        let workspace = root.display().to_string();
        let paths = workflow_watch_paths(
            &crate::standard_services(),
            std::slice::from_ref(&workspace),
        );
        assert_eq!(
            paths.get(&workflow_root.canonicalize().unwrap()),
            Some(&RecursiveMode::Recursive)
        );

        std::fs::remove_dir_all(root.join(".agent")).unwrap();
        let paths = workflow_watch_paths(&crate::standard_services(), &[workspace]);
        assert_eq!(
            paths.get(&root.canonicalize().unwrap()),
            Some(&RecursiveMode::NonRecursive)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn watcher_emits_when_workflow_directory_is_created() {
        let unique = Id::ascending(IdKind::Event).to_string();
        let root = std::env::temp_dir().join(format!("neoism-workflow-event-{unique}"));
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher = install_workflow_watches(
            &crate::standard_services(),
            &[root.display().to_string()],
            event_tx,
        )
        .unwrap();

        std::fs::create_dir(root.join(".agent/workflows")).unwrap();
        tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .expect("filesystem watcher did not report workflow directory creation")
            .expect("filesystem watcher channel closed");

        drop(watcher);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn managed_definition_crud_is_revisioned_and_file_backed() {
        let root = std::env::temp_dir().join(format!(
            "neoism-workflow-admin-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = AppState::open_database_with_services_and_management(
            root.join("state.sqlite3"),
            crate::standard_services(),
            crate::ManagementPolicy::enabled(),
        )
        .await
        .unwrap();
        let definition = json!({
            "id": "managed-daily", "name": "Managed daily", "active": false,
            "schedule": { "frequency": "daily", "time": "09:00" },
            "prompt": "Review the workspace."
        });
        let unauthenticated = workflow_create(
            &state,
            Some(&root),
            None,
            &HeaderMap::new(),
            definition.clone(),
        )
        .await
        .unwrap();
        assert_eq!(unauthenticated.status, 401);
        let response = workflow_create(
            &state,
            Some(&root),
            Some("local-test"),
            &HeaderMap::new(),
            definition,
        )
        .await
        .unwrap();
        assert_eq!(response.status, 201);
        let revision = response.body["revision"].as_str().unwrap().to_string();
        let path = root.join(".agent/workflows/managed-daily.md");
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.starts_with("---\nactive: false\nconcurrency:"));
        assert!(first.ends_with("---\nReview the workspace.\n"));

        let mut stale = HeaderMap::new();
        stale.insert("if-match", "sha256:stale".parse().unwrap());
        let response = workflow_update(
            &state,
            Some(&root),
            Some("local-test"),
            &stale,
            "managed-daily",
            json!({ "name": "Changed" }),
            true,
        )
        .await
        .unwrap();
        assert_eq!(response.status, 412);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);

        let mut matching = HeaderMap::new();
        matching.insert("if-match", revision.parse().unwrap());
        let response = workflow_update(
            &state,
            Some(&root),
            Some("local-test"),
            &matching,
            "managed-daily",
            json!({ "name": "Changed" }),
            true,
        )
        .await
        .unwrap();
        assert_eq!(response.status, 200);
        let next_revision = response.body["revision"].as_str().unwrap().to_string();
        let mut query = BTreeMap::new();
        query.insert("expectedRevision".to_string(), vec![next_revision]);
        let response = workflow_delete(
            &state,
            Some(&root),
            Some("local-test"),
            &HeaderMap::new(),
            "managed-daily",
            &query,
        )
        .await
        .unwrap();
        assert_eq!(response.status, 204);
        assert!(!path.exists());

        let discovered = root.join(".agent/workflow");
        std::fs::create_dir_all(&discovered).unwrap();
        std::fs::write(discovered.join("readonly.md"), "---\nid: readonly\nname: Read only\nschedule:\n  frequency: daily\n---\nDo not overwrite.\n").unwrap();
        let response = workflow_create(
            &state,
            Some(&root),
            Some("local-test"),
            &HeaderMap::new(),
            json!({
                "id": "readonly", "name": "Replacement", "active": false,
                "schedule": { "frequency": "daily" }, "prompt": "Replace."
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status, 403);
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn mcp_events_admission_is_durable_and_fenced() {
        use crate::mcp_events::{Event, ListPage};
        use crate::state::WorkflowEventAdmission::{Admitted, Blocked, Duplicate};

        let path = std::env::temp_dir().join(format!(
            "neoism-workflow-events-{}.turso.db",
            Id::ascending(IdKind::Event)
        ));
        let mut projection = WorkflowProjection {
            activation_id: "event-activation".into(),
            workflow_id: "event-test".into(),
            workspace_root: std::env::temp_dir().display().to_string(),
            source_path: "event.md".into(),
            source_hash: "revision-1".into(),
            definition: serde_json::from_value(serde_json::json!({
                "id": "event-test", "name": "Event test", "active": true,
                "schedule": {"frequency": "daily"}, "prompt": "Summarize incoming data"
            })).unwrap(),
            active: true,
            activated_at: 1,
            last_scheduled_at: None,
            updated: 1,
        };
        let page = ListPage::decode(br#"{"events":[{"name":"issue.updated","delivery":["push"],"inputSchema":{},"payloadSchema":{}}]}"#).unwrap();
        let kind = &page.events[0];
        let digest = [5; 32];
        let mut event = Event {
            name: kind.name.clone(),
            event_id: "event-1".into(),
            timestamp: "2026-07-28T00:00:00Z".into(),
            data: serde_json::Map::new(),
            cursor: Some("first".into()),
        };
        let store = SessionStore::open(path.clone()).await.unwrap();
        store.upsert_workflow(&projection).await.unwrap();
        let mut checkpoint = store.workflow_event_checkpoint(&projection, "issues", kind, &digest).await.unwrap().unwrap();
        let mut stale = store.workflow_event_checkpoint(&projection, "issues", kind, &digest).await.unwrap().unwrap();
        let Admitted(run_id) = store.admit_workflow_event(&mut checkpoint, &event).await.unwrap() else {
            panic!("first event should be admitted")
        };
        event.cursor = Some("not-committed".into());
        assert_eq!(store.admit_workflow_event(&mut checkpoint, &event).await.unwrap(), Duplicate);
        assert_eq!(store.admit_workflow_event(&mut stale, &event).await.unwrap(), Blocked);
        event.event_id = "event-2".into();
        assert_eq!(store.admit_workflow_event(&mut checkpoint, &event).await.unwrap(), Blocked);
        event.name = "unrelated".into();
        assert!(store.admit_workflow_event(&mut checkpoint, &event).await.is_err());
        event.name = kind.name.clone();
        event.data.insert("huge".into(), serde_json::json!("x".repeat(16 * 1024)));
        assert!(store.admit_workflow_event(&mut checkpoint, &event).await.is_err());
        event.data.clear();
        assert_eq!(checkpoint.cursor.as_deref(), Some("first"));
        assert!(store.list_unfinished_workflow_runs().await.unwrap().is_empty());
        let other = WorkflowProjection {
            activation_id: "other-activation".into(),
            workspace_root: format!("{}-other", projection.workspace_root),
            ..projection.clone()
        };
        store.upsert_workflow(&other).await.unwrap();
        let mut independent = store.workflow_event_checkpoint(&other, "issues", kind, &digest).await.unwrap().unwrap();
        event.event_id = "event-1".into();
        event.cursor = None;
        assert!(matches!(store.admit_workflow_event(&mut independent, &event).await.unwrap(), Admitted(_)));
        assert!(independent.cursor.is_none());
        event.event_id = "event-2".into();
        drop(store);

        let store = SessionStore::open(path.clone()).await.unwrap();
        let mut checkpoint = store.workflow_event_checkpoint(&projection, "issues", kind, &digest).await.unwrap().unwrap();
        assert_eq!(checkpoint.cursor.as_deref(), Some("first"));
        assert!(store.workflow_event_checkpoint(&projection, "issues", kind, &[6; 32]).await.unwrap().unwrap().cursor.is_none());
        assert_eq!(store.get_workflow_run(&run_id).await.unwrap().unwrap().status, "event_pending");
        store.set_workflow_active(&projection.activation_id, false, 1).await.unwrap();
        assert!(store.workflow_event_checkpoint(&projection, "issues", kind, &digest).await.unwrap().is_none());
        store.set_workflow_active(&projection.activation_id, true, 1).await.unwrap();
        assert_eq!(store.admit_workflow_event(&mut checkpoint, &event).await.unwrap(), Blocked);
        projection.source_hash = "revision-2".into();
        store.upsert_workflow(&projection).await.unwrap();
        assert!(store.workflow_event_checkpoint(&projection, "issues", kind, &digest).await.unwrap().unwrap().cursor.is_none());
        assert_eq!(store.list_workflow_runs(&projection.activation_id, 10).await.unwrap().len(), 1);
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn durable_concurrency_and_retry_lineage_survive_reopen() {
        let path = std::env::temp_dir().join(format!(
            "neoism-workflow-policy-{}.turso.db",
            Id::ascending(IdKind::Event)
        ));
        let mut definition = WorkflowDefinition {
            id: "policy".into(),
            name: "Policy".into(),
            active: true,
            schedule: schedule("daily"),
            prompt: "Run".into(),
            action: None,
            directory: None,
            skill: None,
            agent: None,
            model: None,
            permissions: None,
            retry: WorkflowRetryPolicy {
                max_attempts: 3,
                initial_delay_ms: 10,
                ..Default::default()
            },
            concurrency: WorkflowConcurrencyPolicy {
                mode: WorkflowConcurrencyMode::Allow,
                max_running: 2,
            },
        };
        let projection = WorkflowProjection {
            activation_id: "policy-activation".into(),
            workflow_id: "policy".into(),
            workspace_root: std::env::temp_dir().display().to_string(),
            source_path: "policy.md".into(),
            source_hash: "hash".into(),
            definition: definition.clone(),
            active: true,
            activated_at: 1,
            last_scheduled_at: None,
            updated: 1,
        };
        let store = SessionStore::open(path.clone()).await.unwrap();
        store.upsert_workflow(&projection).await.unwrap();
        let first = new_run(&projection, 10, "manual");
        let second = new_run(&projection, 11, "manual");
        let third = new_run(&projection, 12, "manual");
        assert!(store
            .claim_workflow_run(&first, &definition.concurrency)
            .await
            .unwrap());
        assert!(store
            .claim_workflow_run(&second, &definition.concurrency)
            .await
            .unwrap());
        assert!(!store
            .claim_workflow_run(&third, &definition.concurrency)
            .await
            .unwrap());

        definition.concurrency = WorkflowConcurrencyPolicy {
            mode: WorkflowConcurrencyMode::Replace,
            max_running: 1,
        };
        let replacement = new_run(&projection, 13, "manual");
        assert!(store
            .claim_workflow_run(&replacement, &definition.concurrency)
            .await
            .unwrap());
        let history = store
            .list_workflow_runs(&projection.activation_id, 10)
            .await
            .unwrap();
        assert_eq!(
            history.iter().filter(|run| run.status == "queued").count(),
            1
        );
        assert_eq!(
            history
                .iter()
                .filter(|run| run.status == "replaced")
                .count(),
            2
        );
        let mut collision = new_run(&projection, 13, "manual");
        collision.id = format!("{}-collision", replacement.id);
        assert!(!store
            .claim_workflow_run(&collision, &definition.concurrency)
            .await
            .unwrap());
        assert_eq!(
            store
                .get_workflow_run(&replacement.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );

        let mut failed = replacement.clone();
        failed.status = "failed".into();
        failed.error = Some("transient".into());
        failed.finished_at = Some(20);
        let mut retry = new_retry_run(&failed, 30, "automatic-retry");
        retry.status = "retry_waiting".into();
        retry.next_attempt_at = Some(30);
        assert!(store
            .schedule_workflow_retry(&failed, &retry)
            .await
            .unwrap());
        store.close().await;
        drop(store);
        let store = SessionStore::open(path.clone()).await.unwrap();
        let due = store.list_due_workflow_retries(30).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].attempt, 2);
        assert_eq!(due[0].retry_of.as_deref(), Some(replacement.id.as_str()));
        assert_eq!(due[0].root_run_id, replacement.root_run_id);
        store.close().await;
        for candidate in [
            path.clone(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }
}
