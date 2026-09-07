use std::collections::{BTreeMap, HashMap};
use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Extension;
use axum::Json;
use neoism_agent_builtins::plugin::vcs;
use neoism_agent_core::{
    event_type, CreateSessionRequest, EventPayload, Id, IdKind, MessageId, MessageInfo,
    MessageWithParts, Part, SessionInfo, TimeInfo, TodoInfo, VcsFileDiff,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;
use crate::{
    message_id_of, model_ref_from_config_with_variant, now_millis, project,
    resolve_directory, slug, InstanceQuery,
};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SessionListQuery {
    pub directory: Option<String>,
    pub path: Option<String>,
    pub roots: Option<String>,
    pub start: Option<u64>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SessionUpdateRequest {
    title: Option<String>,
    agent: Option<String>,
    permission: Option<Vec<neoism_agent_core::PermissionRule>>,
    model: Option<neoism_agent_core::ModelRef>,
    directory: Option<String>,
    time: Option<SessionUpdateTime>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct SessionDirectoryQuery {
    query: Option<String>,
    limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SessionUpdateTime {
    archived: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ForkSessionRequest {
    message_id: Option<MessageId>,
}

pub(crate) async fn session_create(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    body: Option<Json<CreateSessionRequest>>,
) -> Result<Json<SessionInfo>, ApiError> {
    let mut request = body.map(|Json(body)| body).unwrap_or(CreateSessionRequest {
        parent_id: None,
        title: None,
        agent: None,
        model: None,
        permission: None,
        workspace_id: None,
    });
    let directory = resolve_directory(query.directory, &headers);
    let mut extra = BTreeMap::new();
    if let Some(Extension(claims)) = claims {
        if !crate::caller::allows_directory(&claims, &directory) {
            return Err(ApiError::forbidden(
                "The caller is not authorized for this directory",
            ));
        }
        if let Some(limit) = claims.max_sessions {
            let count = state
                .inner
                .store
                .list_sessions()
                .await?
                .iter()
                .filter(|session| {
                    crate::caller::session_tenant(session) == claims.tenant_id
                })
                .count();
            if count >= limit {
                return Err(ApiError::too_many_requests("Session quota exceeded"));
            }
        }
        extra.insert(
            crate::caller::TENANT_EXTRA_KEY.to_string(),
            Value::String(claims.tenant_id.clone()),
        );
        if let Some(parent_id) = request.parent_id.as_ref() {
            let parent = state
                .inner
                .store
                .get_session(parent_id.as_str())
                .await?
                .ok_or_else(|| ApiError::not_found("Parent session not found"))?;
            if !crate::caller::allows_session(&claims, &parent) {
                return Err(ApiError::forbidden(
                    "Parent session is outside the caller's workspace",
                ));
            }
        }
        bind_authenticated_workspace(&mut request, &claims)?;
    }
    Ok(Json(
        create_session_in_directory(&state, &directory, request, extra).await?,
    ))
}

fn bind_authenticated_workspace(
    request: &mut CreateSessionRequest,
    claims: &crate::caller::CallerClaims,
) -> Result<(), ApiError> {
    let Some(workspace_id) = claims.workspace_id.as_deref() else {
        return Ok(());
    };
    if request
        .workspace_id
        .as_ref()
        .is_some_and(|requested| requested != workspace_id)
    {
        return Err(ApiError::forbidden(
            "Requested workspace does not match the authenticated workspace",
        ));
    }
    request.workspace_id = Some(workspace_id.to_string());
    Ok(())
}

pub(crate) async fn create_session_in_directory(
    state: &AppState,
    directory: &str,
    mut request: CreateSessionRequest,
    mut extra: BTreeMap<String, Value>,
) -> Result<SessionInfo, ApiError> {
    let now = now_millis();
    let id = neoism_agent_core::new_session_id();
    let directory = crate::windows_process::canonicalize_path(FsPath::new(directory))
        .map_err(|error| {
            ApiError::bad_request(format!(
                "workflow directory is not accessible: {error}"
            ))
        })?;
    if !directory.is_dir() {
        return Err(ApiError::bad_request(
            "workflow directory is not a directory",
        ));
    }
    let project_context = project::discover(state.services(), directory);
    let directory = project_context.directory.clone();
    let snapshot = state.plugin_snapshot(&directory).await;
    let loaded_config = snapshot.config();
    let agents = crate::plugins::agent_catalog(&snapshot, &directory)?;
    let is_child = request.parent_id.is_some();
    if let Some(parent_id) = request.parent_id.as_ref() {
        let parent = state
            .inner
            .store
            .get_session(parent_id.as_str())
            .await?
            .ok_or_else(|| ApiError::not_found("Parent session not found"))?;
        let parent_tenant = crate::caller::session_tenant(&parent);
        let local_continuation = request.workspace_id.is_none()
            && extra
                .get(crate::caller::TENANT_EXTRA_KEY)
                .and_then(Value::as_str)
                .unwrap_or("local")
                == "local"
            && parent
                .extra
                .get(crate::caller::HOST_LOCAL_ACCESS_KEY)
                .and_then(Value::as_bool)
                == Some(true);
        if local_continuation {
            request.workspace_id = parent.workspace_id.clone();
            extra.insert(
                crate::caller::TENANT_EXTRA_KEY.into(),
                Value::String(parent_tenant.into()),
            );
        }
        if let Some(tenant) = extra
            .get(crate::caller::TENANT_EXTRA_KEY)
            .and_then(Value::as_str)
        {
            if tenant != parent_tenant {
                return Err(ApiError::forbidden(
                    "Parent session belongs to another tenant",
                ));
            }
        } else if parent_tenant != "local" {
            extra.insert(
                crate::caller::TENANT_EXTRA_KEY.to_string(),
                Value::String(parent_tenant.to_string()),
            );
        }
    }
    let info = SessionInfo {
        id: id.clone(),
        slug: slug(),
        project_id: project_context.info.id,
        workspace_id: request.workspace_id,
        directory,
        path: project_context.path,
        parent_id: request.parent_id,
        title: request
            .title
            .unwrap_or_else(|| neoism_agent_core::default_session_title(is_child, now)),
        agent: Some(
            request
                .agent
                .unwrap_or_else(|| agents.default_agent().to_string()),
        ),
        model: request.model.or_else(|| {
            loaded_config.model.as_deref().and_then(|model| {
                model_ref_from_config_with_variant(model, loaded_config.variant.clone())
            })
        }),
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: now,
            updated: now,
            compacting: None,
            archived: None,
        },
        permission: request.permission,
        extra,
    };

    state.inner.store.insert_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_CREATED,
        json!({ "sessionID": id, "info": info }),
    ));
    Ok(info)
}

pub(crate) async fn session_get(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionInfo>, ApiError> {
    let info = state
        .inner
        .store
        .get_session(&session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    Ok(Json(info))
}

pub(crate) async fn session_delete(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<bool>, ApiError> {
    let deleted_session = state.inner.store.get_session(&session_id).await?;
    let execution_root = match deleted_session.as_ref() {
        Some(session) => {
            Some(crate::execution_activity::root_session_id(&state, session).await)
        }
        None => None,
    };
    crate::interaction::cancel_session_interactions(&state, &session_id).await;
    if !state.inner.store.delete_session(&session_id).await? {
        return Err(ApiError::not_found("Session not found"));
    }
    state.inner.statuses.write().await.remove(&session_id);
    state.publish(EventPayload::new(
        event_type::SESSION_DELETED,
        json!({ "sessionID": session_id }),
    ));
    if let Some(root) = execution_root.filter(|root| root != &session_id) {
        crate::execution_activity::finish_if_quiescent(&state, &root).await;
    }
    Ok(Json(true))
}

pub(crate) async fn session_update(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(update): Json<SessionUpdateRequest>,
) -> Result<Json<SessionInfo>, ApiError> {
    let mut info = state
        .inner
        .store
        .get_session(&session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if let Some(title) = update.title {
        info.title = title;
    }
    if let Some(agent) = update.agent {
        info.agent = Some(agent);
    }
    if let Some(permission) = update.permission {
        info.permission = Some(permission);
    }
    if let Some(model) = update.model {
        info.model = Some(model);
    }
    if let Some(directory) = update.directory {
        if state
            .inner
            .session_coordinator
            .active_run(&session_id)
            .await
            .is_some()
        {
            return Err(ApiError::conflict(
                "cannot change directory while the session is running",
            ));
        }
        let project_context =
            resolve_session_directory(state.services(), &info.directory, &directory)?;
        if claims.as_ref().is_some_and(|Extension(claims)| {
            !crate::caller::allows_directory(claims, &project_context.directory)
        }) {
            return Err(ApiError::forbidden(
                "The caller is not authorized for this directory",
            ));
        }
        info.directory = project_context.directory;
        info.project_id = project_context.info.id;
        info.path = project_context.path;
        crate::context_epoch::reconcile(&state, &mut info).await?;
    }
    if let Some(time) = update.time {
        if let Some(archived) = time.archived {
            info.time.archived = Some(archived);
        }
    }
    info.time.updated = now_millis();
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session_id, "info": info }),
    ));
    Ok(Json(info))
}

pub(crate) async fn session_directory_options(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionDirectoryQuery>,
) -> Result<Json<Vec<String>>, ApiError> {
    let info = state
        .inner
        .store
        .get_session(&session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    let plugins = state.plugin_snapshot(&info.directory).await;
    if !crate::plugins::enabled(
        &plugins,
        neoism_agent_builtins::plugin::workspace_tools::ID,
    ) {
        return Err(ApiError::not_found(
            "workspace filesystem tools are disabled",
        ));
    }
    let current = PathBuf::from(info.directory);
    let search_root = directory_search_root(&current);
    let needle = query.query.unwrap_or_default();
    let limit = query.limit.unwrap_or(256).clamp(1, 1_000);
    let search = state.services().workspace_search.clone();
    let options = tokio::task::spawn_blocking(move || {
        workspace_directory_options(
            search.as_ref(),
            &search_root,
            &current,
            &needle,
            limit,
        )
    })
    .await
    .map_err(|error| ApiError::internal(format!("directory search failed: {error}")))??;
    Ok(Json(options))
}

fn resolve_session_directory(
    services: &neoism_agent_service_api::AgentServices,
    current: &str,
    requested: &str,
) -> Result<project::ProjectContext, ApiError> {
    let requested = requested.trim();
    let requested = requested
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            requested
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(requested)
        .trim();
    if requested.is_empty() {
        return Err(ApiError::bad_request("usage: /cd <directory>"));
    }
    let expanded = expand_home_path(requested)?;
    let candidate = if expanded.is_absolute() {
        expanded
    } else {
        PathBuf::from(current).join(expanded)
    };
    let canonical =
        crate::windows_process::canonicalize_path(&candidate).map_err(|error| {
            ApiError::bad_request(format!(
                "directory {} is not accessible: {error}",
                candidate.display()
            ))
        })?;
    if !canonical.is_dir() {
        return Err(ApiError::bad_request(format!(
            "{} is not a directory",
            canonical.display()
        )));
    }
    Ok(project::discover(services, canonical))
}

fn expand_home_path(path: &str) -> Result<PathBuf, ApiError> {
    if path == "~" {
        return home_directory()
            .ok_or_else(|| ApiError::bad_request("cannot resolve home directory"));
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return home_directory()
            .map(|home| home.join(rest))
            .ok_or_else(|| ApiError::bad_request("cannot resolve home directory"));
    }
    if path.starts_with('~') {
        return Err(ApiError::bad_request(
            "only ~ and ~/... home paths are supported",
        ));
    }
    Ok(PathBuf::from(path))
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
}

fn directory_search_root(current: &FsPath) -> PathBuf {
    current
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(current)
        .to_path_buf()
}

fn workspace_directory_options(
    search: &dyn neoism_agent_service_api::WorkspaceSearchService,
    root: &FsPath,
    current: &FsPath,
    query: &str,
    limit: usize,
) -> Result<Vec<String>, ApiError> {
    let mut options = Vec::new();
    push_directory_option(&mut options, current);
    if let Some(parent) = current.parent() {
        push_directory_option(&mut options, parent);
    }
    if let Some(home) = home_directory() {
        push_directory_option(&mut options, &home);
    }
    let remaining = limit.saturating_sub(options.len());
    if remaining == 0 {
        return Ok(options);
    }
    let result = search
        .search_directories(&neoism_agent_service_api::DirectorySearchRequest {
            root: root.to_path_buf(),
            query: query.to_string(),
            offset: 0,
            limit: remaining,
            control: neoism_agent_service_api::WorkspaceSearchRequestControl::default(),
        })
        .map_err(|error| ApiError::internal(error.to_string()))?;
    for relative in result.paths {
        let relative = relative.trim_end_matches(['/', '\\']);
        if !relative.is_empty() {
            push_directory_option(&mut options, &root.join(relative));
        }
        if options.len() >= limit {
            break;
        }
    }
    Ok(options)
}

fn push_directory_option(options: &mut Vec<String>, path: &FsPath) {
    let path = crate::windows_process::canonicalize_path_lossy(path)
        .to_string_lossy()
        .to_string();
    if !path.is_empty() && !options.iter().any(|existing| existing == &path) {
        options.push(path);
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetPinRequest {
    /// Desired pin state. When omitted the flag is toggled relative to its
    /// current value (so a bare POST flips it).
    pub(crate) pinned: Option<bool>,
}

/// Set, clear, or toggle a session's pinned flag.
///
/// Sibling of the goal routes: the flag lives in [`SessionInfo::extra`] and is
/// persisted into the session store (its `info_json`) so it survives reloads
/// and shows up in the session list / cross-device sync. Returns the updated
/// [`SessionInfo`].
pub(crate) async fn session_set_pin(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    body: Option<Json<SetPinRequest>>,
) -> Result<Json<SessionInfo>, ApiError> {
    let request = body.map(|Json(body)| body).unwrap_or_default();
    let mut info = state
        .inner
        .store
        .get_session(&session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    let next = request.pinned.unwrap_or(!info.pinned());
    info.set_pinned(next);
    info.time.updated = now_millis();
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session_id, "info": info }),
    ));
    Ok(Json(info))
}

pub(crate) async fn session_todo_list(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<TodoInfo>>, ApiError> {
    crate::ensure_session(&state, &session_id).await?;
    Ok(Json(
        state
            .inner
            .todos
            .read()
            .await
            .get(&session_id)
            .cloned()
            .unwrap_or_default(),
    ))
}

pub(crate) async fn session_fork(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    body: Option<Json<ForkSessionRequest>>,
) -> Result<Json<SessionInfo>, ApiError> {
    let parent = crate::ensure_session(&state, &session_id).await?;
    let now = now_millis();
    let child_id = neoism_agent_core::new_session_id();
    let child = SessionInfo {
        id: child_id.clone(),
        slug: slug(),
        project_id: parent.project_id,
        workspace_id: parent.workspace_id,
        directory: parent.directory,
        path: parent.path,
        parent_id: Some(parent.id),
        title: format!("Fork - {}", parent.title),
        agent: parent.agent,
        model: parent.model,
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: now,
            updated: now,
            compacting: None,
            archived: None,
        },
        permission: parent.permission,
        extra: parent.extra,
    };
    state.inner.store.insert_session(&child).await?;
    let cutoff = body.and_then(|Json(body)| body.message_id.map(|id| id.to_string()));
    for message in state.inner.store.list_messages(&session_id).await? {
        let original_id = message_id_of(&message);
        let retargeted = retarget_message(message, &child_id);
        state
            .inner
            .store
            .append_message(child_id.as_str(), &retargeted)
            .await?;
        if cutoff.as_deref() == Some(original_id.as_str()) {
            break;
        }
    }
    state.publish(EventPayload::new(
        event_type::SESSION_CREATED,
        json!({ "sessionID": child_id, "info": child }),
    ));
    Ok(Json(child))
}

pub(crate) async fn session_status(
    State(state): State<AppState>,
) -> Json<HashMap<String, Value>> {
    let statuses = state.inner.statuses.read().await.clone();
    let runs = state.inner.session_coordinator.active_runs().await;
    let mut out = HashMap::new();
    for (session_id, status) in statuses {
        let mut value = json!(status);
        if let Some(run) = runs.get(&session_id) {
            value["runID"] = json!(run.id);
            value["startedAt"] = json!(run.started_at);
        }
        if let Ok(Some(session)) = state.inner.store.get_session(&session_id).await {
            if let Some(parent_id) = session.parent_id.as_ref() {
                value["parentSessionID"] = json!(parent_id);
                value["sourceSessionID"] = json!(session.id.to_string());
                value["sourceTitle"] = json!(session.title);
                if let Some(agent) = session.agent.as_ref() {
                    value["sourceAgent"] = json!(agent);
                }
            }
        }
        out.insert(session_id, value);
    }
    Json(out)
}

pub(crate) async fn session_diff(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<VcsFileDiff>>, ApiError> {
    let info = crate::ensure_session(&state, &session_id).await?;
    Ok(Json(vcs::diff(state.services(), &info.directory)))
}

fn retarget_message(
    mut message: MessageWithParts,
    session_id: &neoism_agent_core::SessionId,
) -> MessageWithParts {
    let next_message_id = Id::ascending(IdKind::Message);
    match &mut message.info {
        MessageInfo::User(info) => {
            info.id = next_message_id.clone();
            info.session_id = session_id.clone();
        }
        MessageInfo::Assistant(info) => {
            info.id = next_message_id.clone();
            info.session_id = session_id.clone();
        }
    }
    for part in &mut message.parts {
        match part {
            Part::Text(part) => {
                part.id = Id::ascending(IdKind::Part);
                part.session_id = session_id.clone();
                part.message_id = next_message_id.clone();
            }
            Part::Compaction(part) => {
                part.id = Id::ascending(IdKind::Part);
                part.session_id = session_id.clone();
                part.message_id = next_message_id.clone();
            }
            Part::Agent(part) => {
                part.id = Id::ascending(IdKind::Part);
                part.session_id = session_id.clone();
                part.message_id = next_message_id.clone();
            }
            Part::Subtask(part) => {
                part.id = Id::ascending(IdKind::Part);
                part.session_id = session_id.clone();
                part.message_id = next_message_id.clone();
            }
            Part::Reasoning(part) => {
                part.id = Id::ascending(IdKind::Part);
                part.session_id = session_id.clone();
                part.message_id = next_message_id.clone();
            }
            Part::Tool(part) => {
                part.id = Id::ascending(IdKind::Part);
                part.session_id = session_id.clone();
                part.message_id = next_message_id.clone();
            }
            Part::StepStart(part) => {
                part.id = Id::ascending(IdKind::Part);
                part.session_id = session_id.clone();
                part.message_id = next_message_id.clone();
            }
            Part::StepFinish(part) => {
                part.id = Id::ascending(IdKind::Part);
                part.session_id = session_id.clone();
                part.message_id = next_message_id.clone();
            }
            Part::File(part) => {
                part.id = Id::ascending(IdKind::Part);
                part.session_id = session_id.clone();
                part.message_id = next_message_id.clone();
            }
        }
    }
    message
}

#[cfg(test)]
mod directory_tests {
    use super::*;

    #[test]
    fn session_directory_resolves_relative_and_quoted_paths() {
        let root = std::env::temp_dir().join(format!(
            "neoism-session-cd-{}",
            Id::ascending(IdKind::Event)
        ));
        let current = root.join("from");
        let target = root.join("to with spaces");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&target).unwrap();

        let resolved = resolve_session_directory(
            &crate::standard_services(),
            current.to_string_lossy().as_ref(),
            "'../to with spaces'",
        )
        .unwrap();

        assert_eq!(
            PathBuf::from(resolved.directory),
            target.canonicalize().unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn session_directory_expands_home_and_rejects_named_users() {
        if let Some(home) = home_directory() {
            assert_eq!(
                expand_home_path("~/projects").unwrap(),
                home.join("projects")
            );
        }
        assert!(expand_home_path("~someone/projects").is_err());
    }

    #[test]
    fn authenticated_workspace_is_authoritative_for_session_creation() {
        let workspace_id = "3c3f94da-6409-463d-a132-e03f1a0920d4".to_string();
        let claims = crate::caller::CallerClaims {
            subject: "actor".into(),
            workspace_id: Some(workspace_id.to_string()),
            tenant_id: "tenant".into(),
            directory_prefixes: Vec::new(),
            hosted: false,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
        };
        let mut request = CreateSessionRequest {
            parent_id: None,
            title: None,
            agent: None,
            model: None,
            permission: None,
            workspace_id: None,
        };
        bind_authenticated_workspace(&mut request, &claims).unwrap();
        assert_eq!(request.workspace_id.as_ref(), Some(&workspace_id));

        request.workspace_id = Some("other-workspace".to_string());
        assert!(bind_authenticated_workspace(&mut request, &claims).is_err());
    }
}
