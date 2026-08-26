use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use neoism_agent_core::{event_type, EventPayload, PtyInfo, ShellItem};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;
use crate::{now_millis, pty, resolve_directory, InstanceQuery};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct PtyConnectQuery {
    pub(crate) ticket: Option<String>,
    pub(crate) cursor: Option<i64>,
}

pub(crate) async fn pty_shells() -> Json<Vec<ShellItem>> {
    Json(pty::discover_shells())
}

pub(crate) async fn pty_list(State(state): State<AppState>) -> Json<Vec<PtyInfo>> {
    let mut out = Vec::new();
    for runtime in state.inner.workspace_runtimes.runtimes().await {
        if let Some(ptys) = runtime.pty_if_allocated() {
            out.extend(pty::list_ptys(&*ptys.infos.read().await));
        }
    }
    Json(out)
}

pub(crate) async fn pty_create(
    State(state): State<AppState>,
    Query(query): Query<InstanceQuery>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<PtyInfo>, ApiError> {
    let request = serde_json::from_value::<pty::PtyCreateRequest>(body)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let shell = pty::discover_shells()
        .into_iter()
        .find(|shell| shell.acceptable)
        .map(|shell| shell.path)
        .unwrap_or_else(pty::fallback_shell);
    let directory = resolve_directory(query.directory, &headers);
    let runtime = state.workspace_runtime(&directory).await.map_err(ApiError::gone)?;
    let pty_runtime = runtime.pty().map_err(|error| ApiError::gone(error.to_string()))?;
    let mut info = pty::create_pty_info(
        request,
        runtime.root.to_string_lossy().into_owned(),
        shell,
        now_millis(),
    );
    if let Some(program) = info.command.first_mut() {
        let resolved = resolve_pty_command(state.services(), program, &info.cwd)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
        *program = resolved.to_string_lossy().into_owned();
    }
    let mut ptys = pty_runtime.infos.write().await;
    let info = pty::insert_pty(&mut *ptys, info);
    drop(ptys);
    state.publish(EventPayload::new(
        event_type::PTY_CREATED,
        json!({ "id": info.id.clone(), "ptyID": info.id.clone(), "info": info.clone() }),
    ));
    Ok(Json(info))
}

fn resolve_pty_command(
    services: &neoism_agent_service_api::AgentServices,
    program: &str,
    cwd: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let requested = crate::executable::in_directory(program, std::path::Path::new(cwd));
    crate::executable::resolve_command(
        services,
        requested,
        neoism_agent_service_api::ExecutablePurpose::Other("pty-command".to_string()),
        "PTY command",
    )
}

#[cfg(test)]
mod executable_tests {
    use super::*;
    use crate::executable::test_support::FakeExecutableService;
    use std::sync::Arc;

    #[test]
    fn pty_command_honors_injected_path_and_reports_missing_executable() {
        let injected = std::path::PathBuf::from("/injected/pty-shell");
        let mut services = crate::standard_services();
        services.executables = Arc::new(FakeExecutableService::with("pty-shell", &injected));
        assert_eq!(resolve_pty_command(&services, "pty-shell", ".").unwrap(), injected);

        services.executables = Arc::new(FakeExecutableService::default());
        let error = resolve_pty_command(&services, "pty-shell", ".").unwrap_err().to_string();
        assert!(error.contains("PTY command executable `pty-shell` is unavailable"));
        assert!(error.contains("install it"));
    }
}

pub(crate) async fn pty_get(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
) -> Result<Json<PtyInfo>, ApiError> {
    let runtime = find_pty_runtime(&state, &pty_id).await.ok_or_else(|| ApiError::not_found("PTY session not found"))?;
    let ptys = runtime.infos.read().await;
    pty::get_pty(&*ptys, &pty_id)
        .map(Json)
        .map_err(|_| ApiError::not_found("PTY session not found"))
}

pub(crate) async fn pty_update(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
    Json(request): Json<pty::PtyUpdateRequest>,
) -> Result<Json<PtyInfo>, ApiError> {
    let runtime = find_pty_runtime(&state, &pty_id)
        .await
        .ok_or_else(|| ApiError::not_found("PTY session not found"))?;
    let size = request.size;
    let updated = {
        let mut ptys = runtime.infos.write().await;
        pty::update_pty(&mut *ptys, &pty_id, request)
            .map_err(|_| ApiError::not_found("PTY session not found"))?
    };
    if let Some(size) = size {
        runtime.processes.resize(&pty_id, size).await;
    }
    state.publish(EventPayload::new(
        event_type::PTY_UPDATED,
        json!({ "id": updated.id.clone(), "ptyID": updated.id.clone(), "info": updated.clone() }),
    ));
    Ok(Json(updated))
}

pub(crate) async fn pty_remove(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
) -> Result<Json<bool>, ApiError> {
    let runtime = find_pty_runtime(&state, &pty_id).await.ok_or_else(|| ApiError::not_found("PTY session not found"))?;
    let mut ptys = runtime.infos.write().await;
    let removed = pty::remove_pty(&mut *ptys, &pty_id)
        .map_err(|_| ApiError::not_found("PTY session not found"))?;
    drop(ptys);
    runtime.processes.stop(&pty_id).await;
    state.publish(EventPayload::new(
        event_type::PTY_DELETED,
        json!({ "id": removed.id.clone(), "ptyID": removed.id.clone(), "info": removed.clone() }),
    ));
    Ok(Json(true))
}

pub(crate) async fn pty_connect_token(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<pty::PtyConnectToken>, ApiError> {
    if headers
        .get("x-opencode-ticket")
        .and_then(|value| value.to_str().ok())
        != Some("1")
    {
        return Err(ApiError::forbidden("PTY connect ticket header is required"));
    }
    {
        let runtime = find_pty_runtime(&state, &pty_id).await.ok_or_else(|| ApiError::not_found("PTY session not found"))?;
        let ptys = runtime.infos.read().await;
        pty::get_pty(&*ptys, &pty_id)
            .map_err(|_| ApiError::not_found("PTY session not found"))?;
    }
    let now = now_millis();
    let runtime = find_pty_runtime(&state, &pty_id).await.ok_or_else(|| ApiError::not_found("PTY session not found"))?;
    let mut tokens = runtime.tokens.write().await;
    tokens.prune_expired(now);
    Ok(Json(tokens.issue(pty_id, now)))
}

pub(crate) async fn prepare_connection_with_runtime(
    state: AppState,
    runtime: std::sync::Arc<pty::PtyWorkspaceRuntime>,
    pty_id: String,
    query: PtyConnectQuery,
) -> Result<std::sync::Arc<dyn neoism_agent_plugin_api::WebSocketSession>, ApiError> {
    let info = {
        let ptys = runtime.infos.read().await;
        pty::get_pty(&*ptys, &pty_id)
            .map_err(|_| ApiError::not_found("PTY session not found"))?
    };
    let ticket = query
        .ticket
        .as_deref()
        .ok_or_else(|| ApiError::forbidden("PTY connect ticket is required"))?;
    runtime
        .tokens
        .write()
        .await
        .validate(&pty_id, ticket, now_millis())
        .map_err(|_| ApiError::forbidden("invalid PTY connect ticket"))?;

    let cursor = query.cursor;
    let processes = runtime.processes.clone();
    let publish_state = std::sync::Arc::downgrade(&state.inner);
    Ok(std::sync::Arc::new(PtySocketSession { processes, info, cursor, publish_state }))
}

struct PtySocketSession {
    processes: std::sync::Arc<pty::PtyProcessRegistry>,
    info: PtyInfo,
    cursor: Option<i64>,
    publish_state: std::sync::Weak<crate::state::InnerState>,
}

impl neoism_agent_plugin_api::WebSocketSession for PtySocketSession {
    fn run<'a>(&'a self, socket: Box<dyn neoism_agent_plugin_api::PluginWebSocket>) -> neoism_agent_plugin_api::PluginFuture<'a, ()> {
        Box::pin(async move {
            let publish_state = self.publish_state.clone();
            pty::serve_websocket(self.processes.clone(), self.info.clone(), self.cursor, socket, move |id, exit_status| {
                let Some(inner) = publish_state.upgrade() else {
                    return;
                };
                AppState { inner }.publish(EventPayload::new(
                    event_type::PTY_EXITED,
                    json!({ "id": id, "ptyID": id, "exitStatus": exit_status }),
                ));
            }).await;
            Ok(())
        })
    }
}

struct LeasedPtyRuntime {
    _workspace: std::sync::Arc<crate::workspace_runtime::WorkspaceRuntime>,
    _generation: crate::workspace_runtime::PluginGenerationLease,
    runtime: std::sync::Arc<pty::PtyWorkspaceRuntime>,
}

impl std::ops::Deref for LeasedPtyRuntime {
    type Target = pty::PtyWorkspaceRuntime;

    fn deref(&self) -> &Self::Target { &self.runtime }
}

async fn find_pty_runtime(state: &AppState, pty_id: &str) -> Option<LeasedPtyRuntime> {
    for runtime in state.inner.workspace_runtimes.runtimes().await {
        let generation = runtime.snapshot();
        let Some(ptys) = generation.pty_if_allocated() else { continue; };
        if ptys.infos.read().await.contains_key(pty_id) {
            return Some(LeasedPtyRuntime { _workspace: runtime, _generation: generation, runtime: ptys });
        }
    }
    None
}
