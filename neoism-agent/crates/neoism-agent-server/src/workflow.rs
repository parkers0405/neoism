use std::collections::{BTreeMap, BTreeSet};
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
use crate::{now_millis, resolve_directory, InstanceQuery};

const PREVIEW_COUNT: usize = 10;
const WATCH_DEBOUNCE: Duration = Duration::from_millis(150);
const CLOCK_SAFETY_WAKE: Duration = Duration::from_secs(60 * 60);

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
    pub(crate) directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) skill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) permissions: Option<BTreeMap<String, WorkflowPermission>>,
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
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowSource {
    definition: WorkflowDefinition,
    source_path: String,
    source_hash: String,
}

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

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WorkflowHistoryQuery {
    directory: Option<String>,
    limit: Option<usize>,
}

pub(crate) fn spawn_scheduler(state: AppState) {
    let weak_state = std::sync::Arc::downgrade(&state.inner);
    let workflow_notify = state.inner.workflow_notify.clone();
    tokio::spawn(async move {
        let (watch_tx, mut watch_rx) = tokio::sync::mpsc::unbounded_channel();
        if let Some(inner) = weak_state.upgrade() {
            if let Err(error) = recover_unfinished_runs(&AppState { inner }).await {
                tracing::warn!(%error, "failed to recover workflow runs");
            }
        }
        let mut watcher: Option<notify::RecommendedWatcher>;
        loop {
            let Some(inner) = weak_state.upgrade() else {
                return;
            };
            let state = AppState { inner };
            if state.inner.workflow_workspaces.read().await.is_empty() {
                state.inner.workflow_scheduler_started.store(false, std::sync::atomic::Ordering::SeqCst);
                // Close the race where participation was added while the flag
                // still said the old scheduler was alive.
                if !state.inner.workflow_workspaces.read().await.is_empty()
                    && state.inner.workflow_scheduler_started.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok()
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
            match install_workflow_watches(state.services(), &workspaces, watch_tx.clone()) {
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

fn workflow_watch_paths(services: &neoism_agent_service_api::AgentServices, workspaces: &[String]) -> BTreeMap<PathBuf, RecursiveMode> {
    let mut paths = BTreeMap::new();
    for workspace in workspaces {
        let roots = crate::config::roots(services, workspace);
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
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let workspace = workspace_root(resolve_directory(query.directory, &headers))?;
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
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(
        json!({ "workflows": workflows, "diagnostics": catalog.diagnostics }),
    ))
}

pub(crate) async fn workflow_get(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workspace_root(resolve_directory(query.directory, &headers))?;
    track_workspace(&state, &workspace).await;
    let source = find_source(state.services(), &workspace, &workflow_id).await?;
    let projection = state
        .inner
        .store
        .get_workflow(&activation_id(&workspace, &workflow_id))
        .await?;
    Ok(Json(json!({
        "definition": source.definition,
        "sourcePath": source.source_path,
        "sourceHash": source.source_hash,
        "active": projection.as_ref().map(|item| item.active).unwrap_or(source.definition.active),
        "activationID": projection.as_ref().map(|item| item.activation_id.as_str()),
        "lastScheduledAt": projection.and_then(|item| item.last_scheduled_at),
    })))
}

pub(crate) async fn workflow_activate(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workspace_root(resolve_directory(query.directory, &headers))?;
    track_workspace(&state, &workspace).await;
    let source = find_source(state.services(), &workspace, &workflow_id).await?;
    let id = activation_id(&workspace, &workflow_id);
    let existing = state.inner.store.get_workflow(&id).await?;
    let now = now_millis();
    let projection = WorkflowProjection {
        activation_id: id,
        workflow_id,
        workspace_root: workspace,
        source_path: source.source_path,
        source_hash: source.source_hash,
        definition: source.definition,
        active: true,
        activated_at: existing
            .as_ref()
            .map(|item| item.activated_at)
            .unwrap_or(now),
        last_scheduled_at: existing.and_then(|item| item.last_scheduled_at),
        updated: now,
    };
    state.inner.store.upsert_workflow(&projection).await?;
    publish_workflow(&state, &projection);
    state.inner.workflow_notify.notify_one();
    Ok(Json(json!(projection)))
}

pub(crate) async fn workflow_pause(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workspace_root(resolve_directory(query.directory, &headers))?;
    track_workspace(&state, &workspace).await;
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
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workspace_root(resolve_directory(query.directory, &headers))?;
    track_workspace(&state, &workspace).await;
    let source = find_source(state.services(), &workspace, &workflow_id).await?;
    let projection = ensure_projection(&state, workspace, source, false).await?;
    let run = new_run(&projection, now_millis(), "manual");
    if !state.inner.store.claim_workflow_run(&run).await? {
        return Err(ApiError::conflict(
            "Workflow already has a queued or running run",
        ));
    }
    publish_run(&state, &run);
    tokio::spawn(execute_run(state.clone(), projection, run.clone()));
    Ok(Json(json!(run)))
}

pub(crate) async fn workflow_preview(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Path(workflow_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = workspace_root(resolve_directory(query.directory, &headers))?;
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
    let workspace = workspace_root(resolve_directory(query.directory, &headers))?;
    track_workspace(&state, &workspace).await;
    let id = activation_id(&workspace, &workflow_id);
    if state.inner.store.get_workflow(&id).await?.is_none() {
        return Err(ApiError::not_found("Workflow has no run history"));
    }
    let runs = state
        .inner
        .store
        .list_workflow_runs(&id, query.limit.unwrap_or(50))
        .await?;
    Ok(Json(json!({ "runs": runs })))
}

async fn discover_async(services: neoism_agent_service_api::AgentServices, workspace: String) -> Result<WorkflowCatalog, ApiError> {
    tokio::task::spawn_blocking(move || discover(&services, &workspace))
        .await
        .map_err(|error| {
            ApiError::internal(format!("workflow discovery task failed: {error}"))
        })?
        .map_err(ApiError::from)
}

async fn track_workspace(state: &AppState, workspace: &str) {
    let snapshot = state.plugin_snapshot(workspace).await;
    if !snapshot.manifests.iter().any(|plugin| plugin.id == neoism_agent_builtins::plugin::workflows::ID) {
        return;
    }
    workspace_enabled(state, crate::workspace_runtime::canonical_location(workspace)).await;
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
        if state.inner.workflow_scheduler_started
            .compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst)
            .is_ok()
        {
            spawn_scheduler(state.clone());
        }
        state.inner.workflow_notify.notify_one();
    }
}

pub(crate) async fn workspace_disabled(state: &AppState, workspace: &FsPath) {
    state.inner.workflow_workspaces.write().await.remove(workspace.to_string_lossy().as_ref());
    state.inner.workflow_notify.notify_waiters();
}

async fn reconcile_workspaces(state: &AppState) -> anyhow::Result<()> {
    let _reconcile = state.inner.workflow_reconcile.lock().await;
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
        let catalog = tokio::task::spawn_blocking(move || discover(&services, &scan_workspace))
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
                    projection.active = source.definition.active;
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

fn discover(services: &neoism_agent_service_api::AgentServices, workspace: &str) -> anyhow::Result<WorkflowCatalog> {
    let mut catalog = WorkflowCatalog::default();
    for root in crate::config::roots(services, workspace) {
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
        source_path: path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string(),
        source_hash: format!("{:x}", Sha256::digest(raw.as_bytes())),
    })
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
    if definition.prompt.trim().is_empty() {
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
    let schedule = &definition.schedule;
    if schedule.interval == 0 {
        bail!("schedule.interval must be at least 1");
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
        other => bail!("unsupported schedule.frequency `{other}`; use hourly, daily, weekly, monthly, or a date"),
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

fn preview_slots(
    schedule: &WorkflowSchedule,
    after: u64,
    count: usize,
) -> Result<Vec<u64>, ApiError> {
    if is_one_time(schedule) {
        let slot = one_time_slot(schedule)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        return Ok((slot > after).then_some(slot).into_iter().collect());
    }
    let mut slots = Vec::with_capacity(count);
    let mut cursor = after;
    for _ in 0..count {
        cursor = next_slot(schedule, cursor)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        slots.push(cursor);
    }
    Ok(slots)
}

fn next_slot(schedule: &WorkflowSchedule, after: u64) -> anyhow::Result<u64> {
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
    Ok(earliest)
}

fn is_one_time(schedule: &WorkflowSchedule) -> bool {
    schedule.date.is_some() || schedule.at.is_some()
}

async fn schedule_due_workflows(state: &AppState) -> anyhow::Result<()> {
    reconcile_workspaces(state).await?;
    for mut projection in state.inner.store.list_active_workflows().await? {
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
            projection.definition = source.definition;
            projection.source_hash = source.source_hash;
            projection.updated = now_millis();
            state.inner.store.upsert_workflow(&projection).await?;
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
        let run = new_run(&projection, slot, "scheduled");
        let claimed = state.inner.store.claim_scheduled_workflow_run(&run).await?;
        if claimed {
            publish_run(state, &run);
            tokio::spawn(execute_run(state.clone(), projection, run));
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
    WorkflowRun {
        id: format!("wfr_{}_{}", created, crate::slug()),
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
    }
}

async fn execute_run(
    state: AppState,
    projection: WorkflowProjection,
    mut run: WorkflowRun,
) {
    let result = execute_run_inner(&state, &projection, &mut run).await;
    match result {
        Ok(()) => {
            run.status = "completed".to_string();
            run.finished_at = Some(now_millis());
            if let Err(error) = state
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
                tracing::error!(%error, run_id = %run.id, "failed to finish workflow run");
            }
        }
        Err(error) => {
            let message = error.to_string();
            run.status = "failed".to_string();
            run.error = Some(message.clone());
            run.finished_at = Some(now_millis());
            if let Err(store_error) = state
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
                tracing::error!(error = %store_error, run_id = %run.id, "failed to record workflow failure");
            }
        }
    }
    publish_run(&state, &run);
}

async fn execute_run_inner(
    state: &AppState,
    projection: &WorkflowProjection,
    run: &mut WorkflowRun,
) -> Result<(), ApiError> {
    let mut prompt = projection.definition.prompt.clone();
    if let Some(skill_name) = projection.definition.skill.as_deref() {
        let skills = crate::skill::load_async(state.services(), &projection.workspace_root).await?;
        let skill = skills
            .into_iter()
            .find(|skill| {
                skill.info.name == skill_name
                    || skill.info.path.as_deref() == Some(skill_name)
            })
            .ok_or_else(|| {
                ApiError::bad_request(format!(
                    "workflow skill `{skill_name}` was not found"
                ))
            })?;
        prompt = format!("{}\n\n{}", skill.content.trim(), prompt);
    }
    let mut extra = BTreeMap::new();
    extra.insert("workflowRunID".to_string(), json!(run.id));
    extra.insert("workflowID".to_string(), json!(run.workflow_id));
    extra.insert("workflowActivationID".to_string(), json!(run.activation_id));
    extra.insert("workflowScheduledAt".to_string(), json!(run.scheduled_at));
    extra.insert("workflowTrigger".to_string(), json!(run.trigger));
    let execution_directory = workflow_execution_directory(state.services(), projection)?;
    if !crate::plugins::enabled(state.services(), &execution_directory, neoism_agent_builtins::plugin::workflows::ID) {
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
        },
        extra,
    )
    .await?;
    state
        .inner
        .store
        .update_workflow_run(&run.id, "running", Some(session.id.as_str()), None, false)
        .await?;
    run.status = "running".to_string();
    run.started_at = Some(now_millis());
    run.session_id = Some(session.id.to_string());
    publish_run(state, run);
    let message_key = format!("{}:{}", run.activation_id, run.scheduled_at);
    let digest = format!("{:x}", Sha256::digest(message_key.as_bytes()));
    let message_id =
        Id::parse(IdKind::Message, format!("msg_workflow_{}", &digest[..24]))
            .map_err(|error| ApiError::internal(error.to_string()))?;
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

async fn recover_unfinished_runs(state: &AppState) -> anyhow::Result<()> {
    for mut run in state.inner.store.list_unfinished_workflow_runs().await? {
        run.status = "interrupted".to_string();
        run.error =
            Some("server restarted before the workflow run completed".to_string());
        run.finished_at = Some(now_millis());
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
    let default_directory = if workflow_source_is_global(services, &projection.workspace_root, &projection.source_path) {
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

fn workflow_source_is_global(services: &neoism_agent_service_api::AgentServices, workspace: &str, source_path: &str) -> bool {
    let source = FsPath::new(source_path);
    let workspace = PathBuf::from(workspace);
    crate::config::roots(services, workspace.to_string_lossy().as_ref()).into_iter().any(|root| !root.starts_with(&workspace) && source.starts_with(root))
}

fn activation_id(workspace: &str, workflow_id: &str) -> String {
    let digest = Sha256::digest(format!("{workspace}\0{workflow_id}").as_bytes());
    format!("wfa_{:x}", digest)
}

fn publish_workflow(state: &AppState, projection: &WorkflowProjection) {
    state.publish(EventPayload::new(
        event_type::WORKFLOW_UPDATED,
        json!({
            "aggregateID": projection.activation_id,
            "workflow": projection,
        }),
    ));
}

fn publish_run(state: &AppState, run: &WorkflowRun) {
    state.publish(EventPayload::new(
        event_type::WORKFLOW_RUN_UPDATED,
        json!({
            "aggregateID": run.activation_id,
            "run": run,
        }),
    ));
}

#[cfg(test)]
mod tests {
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
            directory: None,
            schedule: schedule("daily"),
            skill: None,
            agent: None,
            model: None,
            permissions: None,
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
            directory: None,
            skill: None,
            agent: None,
            model: None,
            permissions: None,
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
                directory: Some("packages/app".to_string()),
                skill: None,
                agent: None,
                model: None,
                permissions: None,
            },
            active: true,
            activated_at: 0,
            last_scheduled_at: None,
            updated: 0,
        };
        assert_eq!(
            workflow_execution_directory(&crate::standard_services(), &projection).unwrap(),
            nested.canonicalize().unwrap().display().to_string()
        );
        let external = std::env::temp_dir().join(format!(
            "neoism-workflow-external-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&external).unwrap();
        projection.definition.directory = Some(external.display().to_string());
        assert_eq!(
            workflow_execution_directory(&crate::standard_services(), &projection).unwrap(),
            external.canonicalize().unwrap().display().to_string()
        );
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(external);
    }

    #[test]
    fn global_workflow_defaults_to_os_home_directory() {
        let home = dirs::home_dir().expect("home directory");
        let services = crate::standard_services();
        let global_root = crate::config::roots(&services, std::env::temp_dir().to_string_lossy().as_ref()).remove(0);
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
                directory: None,
                skill: None,
                agent: None,
                model: None,
                permissions: None,
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
        let workspace = root.display().to_string();
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
                directory: None,
                skill: None,
                agent: None,
                model: None,
                permissions: None,
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
        assert!(store.claim_scheduled_workflow_run(&first).await.unwrap());

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
        assert!(!store.claim_scheduled_workflow_run(&retry).await.unwrap());
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
    fn watcher_covers_existing_and_not_yet_created_workflow_roots() {
        let unique = Id::ascending(IdKind::Event).to_string();
        let root = std::env::temp_dir().join(format!("neoism-workflow-watch-{unique}"));
        let workflow_root = root.join(".agent/workflows");
        std::fs::create_dir_all(&workflow_root).unwrap();
        let workspace = root.display().to_string();
        let paths = workflow_watch_paths(&crate::standard_services(), std::slice::from_ref(&workspace));
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
        let watcher =
            install_workflow_watches(&crate::standard_services(), &[root.display().to_string()], event_tx).unwrap();

        std::fs::create_dir(root.join(".agent/workflows")).unwrap();
        tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .expect("filesystem watcher did not report workflow directory creation")
            .expect("filesystem watcher channel closed");

        drop(watcher);
        let _ = std::fs::remove_dir_all(root);
    }
}
