//! Root-owner assistant identity. Runtime storage is not a project/workspace.
use std::collections::BTreeMap;
use std::sync::Arc;
use neoism_agent_core::{event_type, EventPayload, SessionInfo, TimeInfo};
use serde_json::{json, Value};
use crate::{caller, error::ApiError, state::AppState};

pub(crate) const OWNER: &str = "local-operator";

pub(crate) fn root_operator(claims: &caller::CallerClaims) -> bool {
    claims.is_system_workflow() || (!claims.hosted && claims.worker.is_none()
        && claims.resolved.is_none() && claims.workspace_id.is_none()
        && claims.tenant_id == "local" && claims.subject == OWNER
        && claims.directory_prefixes.is_empty())
}

pub(crate) fn legacy_owned(info: &SessionInfo) -> bool {
    let tenant = caller::session_tenant(info);
    let creator = info.extra.get(caller::CREATED_BY_EXTRA_KEY).and_then(Value::as_str);
    if tenant == "local" { return creator.is_none() || creator == Some(OWNER); }
    creator == Some(OWNER)
        && info.workspace_id.as_ref().is_some_and(|id| tenant == format!("workspace:{id}"))
        && info.extra.get(caller::HOST_LOCAL_ACCESS_KEY).and_then(Value::as_bool) == Some(true)
}

pub(crate) async fn session(state: &AppState, claims: Option<&caller::CallerClaims>) -> Result<SessionInfo, ApiError> {
    if state.services().hosted || state.services().workspace_worker.is_some()
        || claims.is_some_and(|claims| !root_operator(claims)) {
        return Err(ApiError::forbidden("Neo requires the root operator capability; workspace credentials cannot access Neo"));
    }
    let key = json!(["local", OWNER]).to_string();
    let lock = state.inner.subtask_parent_locks.lock().await
        .entry(format!("neo-root:{key}")).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone();
    let _guard = lock.lock().await;
    if let Some(info) = state.inner.store.neo_session(&key).await? { return Ok(info); }
    // Satisfy the runtime path contract without discovering a project.
    let runtime = state.inner.artifact_root.join("neo-runtime");
    tokio::fs::create_dir_all(&runtime).await.map_err(|e| ApiError::internal(e.to_string()))?;
    let runtime = crate::windows_process::canonicalize_path(&runtime).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut candidates = state.inner.store.list_sessions().await?;
    candidates.retain(|info| info.is_neo() && !info.extra.contains_key("neoOwnerKey") && legacy_owned(info));
    candidates.sort_by(|a, b| (a.time.created, a.id.as_str()).cmp(&(b.time.created, b.id.as_str())));
    let adopted = !candidates.is_empty();
    let now = crate::now_millis();
    let mut info = candidates.into_iter().next().unwrap_or_else(|| SessionInfo {
        id: neoism_agent_core::new_session_id(), slug: crate::slug(), project_id: String::new(), workspace_id: None,
        directory: String::new(), path: None, parent_id: None, title: "Neo".into(), agent: Some("neo".into()),
        model: None, permission: None, version: env!("CARGO_PKG_VERSION").into(),
        time: TimeInfo { created: now, updated: now, compacting: None, archived: None }, extra: BTreeMap::new(),
    });
    // Preserve transcript, model, permissions, task associations and stable ID.
    info.directory = runtime.to_string_lossy().into_owned();
    info.project_id.clear(); info.workspace_id = None; info.path = None; info.parent_id = None;
    info.agent = Some("neo".into()); info.title = "Neo".into(); info.time.archived = None;
    for field in ["neoWorkspace", caller::HOST_LOCAL_ACCESS_KEY, caller::DIRECTORY_PREFIXES_EXTRA_KEY, caller::QUOTAS_EXTRA_KEY,
        crate::execution_activity::EXECUTION_ID_KEY, crate::execution_activity::EXECUTION_ROOT_KEY] { info.extra.remove(field); }
    info.extra.insert("sessionRole".into(), json!("neo"));
    info.extra.insert("sessionScope".into(), json!("root"));
    info.extra.insert("neoOwnerKey".into(), json!(key));
    info.extra.insert("neoOwner".into(), json!(OWNER));
    info.extra.insert(caller::TENANT_EXTRA_KEY.into(), json!("local"));
    info.extra.insert(caller::CREATED_BY_EXTRA_KEY.into(), json!(OWNER));
    info.extra.insert(caller::EXECUTION_POLICY_EXTRA_KEY.into(), json!({"mode":"native-local"}));
    let saved = if adopted { state.inner.store.update_session(&info).await } else { state.inner.store.insert_session(&info).await };
    if let Err(error) = saved {
        if let Some(existing) = state.inner.store.neo_session(&key).await? { return Ok(existing); }
        return Err(error.into());
    }
    state.publish(EventPayload::new(if adopted { event_type::SESSION_UPDATED } else { event_type::SESSION_CREATED }, json!({"sessionID":info.id,"info":info})));
    Ok(info)
}

/// Resolve an explicitly selected project, never the orchestrator's runtime cwd.
pub(crate) async fn delegate(state: &AppState, parent: &SessionInfo, input: &Value) -> Result<SessionInfo, ApiError> {
    if !parent.is_neo() || parent.extra.get("sessionScope").and_then(Value::as_str) != Some("root")
        || caller::session_tenant(parent) != "local" || state.services().workspace_worker.is_some() || state.services().hosted {
        return Err(ApiError::forbidden("Delegation requires the root owner Neo"));
    }
    let requested_workspace = input.get("workspace_id").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
    let workspace = if let Some(id) = requested_workspace {
        Some(state.services().workspace_management.as_ref()
            .get_workspace(id).map_err(|e| ApiError::bad_request(e.to_string()))?
            .ok_or_else(|| ApiError::not_found("Target workspace not found"))?)
    } else { None };
    let requested_directory = input.get("directory").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from).or_else(|| workspace.as_ref().map(|w| w.root.clone()));
    let target = if let Some(path) = requested_directory {
        if !path.is_absolute() { return Err(ApiError::bad_request("Neo task directory must be absolute")); }
        let path = crate::windows_process::canonicalize_path(&path).map_err(|e| ApiError::bad_request(e.to_string()))?;
        if !path.is_dir() { return Err(ApiError::bad_request("Neo task target must be a directory")); }
        if let Some(workspace) = workspace.as_ref() {
            let root = crate::windows_process::canonicalize_path(&workspace.root).map_err(|e| ApiError::bad_request(e.to_string()))?;
            if !path.starts_with(root) { return Err(ApiError::forbidden("Directory is outside the selected workspace")); }
        }
        Some(path)
    } else { None };
    if let Some(id) = input.get("session_id").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        let child = state.inner.store.get_session(id).await?.ok_or_else(|| ApiError::not_found("Delegated session not found"))?;
        if child.parent_id.is_some() || child.extra.get("neoSessionId").and_then(Value::as_str) != Some(parent.id.as_str()) {
            return Err(ApiError::forbidden("Session was not delegated by this Neo"));
        }
        if target.as_ref().is_some_and(|path| path.to_string_lossy() != child.directory)
            || requested_workspace.is_some_and(|id| child.extra.get("neoTargetWorkspaceId").and_then(Value::as_str) != Some(id)) {
            return Err(ApiError::bad_request("Follow-up context does not match the delegated session"));
        }
        return Ok(child);
    }
    let target = target.ok_or_else(|| ApiError::bad_request("neo_task requires an explicit directory or workspace_id for new work"))?;
    if target.starts_with(&parent.directory) { return Err(ApiError::bad_request("Neo's internal runtime directory is not a delegation target")); }
    let directory = target.to_string_lossy().into_owned();
    let mut extra = BTreeMap::new();
    extra.insert("neoSessionId".into(), json!(parent.id));
    extra.insert(caller::CREATED_BY_EXTRA_KEY.into(), json!(OWNER));
    extra.insert(caller::EXECUTION_POLICY_EXTRA_KEY.into(), json!({"mode":"native-local"}));
    extra.insert(caller::DIRECTORY_PREFIXES_EXTRA_KEY.into(), json!([directory]));
    let workspace_id = if let Some(id) = requested_workspace {
        extra.insert("neoTargetWorkspaceId".into(), json!(id));
        let namespace = state.inner.store.associate_host_directory(&directory, id).await?;
        extra.insert(caller::TENANT_EXTRA_KEY.into(), json!(format!("workspace:{namespace}")));
        Some(namespace)
    } else { None };
    crate::session_routes::create_session_in_directory(state, &directory, neoism_agent_core::CreateSessionRequest {
        parent_id: None, title: input.get("description").and_then(Value::as_str).map(str::to_owned),
        agent: Some("build".into()), model: None, permission: parent.permission.clone(), workspace_id,
        external_provider: None, external_options: None,
    }, extra).await
}