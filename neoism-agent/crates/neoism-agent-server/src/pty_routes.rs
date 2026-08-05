use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
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
    let ptys = state.inner.ptys.read().await;
    Json(pty::list_ptys(&*ptys))
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
    let info = pty::create_pty_info(
        request,
        resolve_directory(query.directory, &headers),
        shell,
        now_millis(),
    );
    let mut ptys = state.inner.ptys.write().await;
    let info = pty::insert_pty(&mut *ptys, info);
    drop(ptys);
    state.publish(EventPayload::new(
        event_type::PTY_CREATED,
        json!({ "id": info.id.clone(), "ptyID": info.id.clone(), "info": info.clone() }),
    ));
    Ok(Json(info))
}

pub(crate) async fn pty_get(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
) -> Result<Json<PtyInfo>, ApiError> {
    let ptys = state.inner.ptys.read().await;
    pty::get_pty(&*ptys, &pty_id)
        .map(Json)
        .map_err(|_| ApiError::not_found("PTY session not found"))
}

pub(crate) async fn pty_update(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
    Json(request): Json<pty::PtyUpdateRequest>,
) -> Result<Json<PtyInfo>, ApiError> {
    let size = request.size;
    let updated = {
        let mut ptys = state.inner.ptys.write().await;
        pty::update_pty(&mut *ptys, &pty_id, request)
            .map_err(|_| ApiError::not_found("PTY session not found"))?
    };
    if let Some(size) = size {
        pty::resize_pty_process(&pty_id, size).await;
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
    let mut ptys = state.inner.ptys.write().await;
    let removed = pty::remove_pty(&mut *ptys, &pty_id)
        .map_err(|_| ApiError::not_found("PTY session not found"))?;
    drop(ptys);
    pty::stop_pty_process(&pty_id).await;
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
        let ptys = state.inner.ptys.read().await;
        pty::get_pty(&*ptys, &pty_id)
            .map_err(|_| ApiError::not_found("PTY session not found"))?;
    }
    let now = now_millis();
    let mut tokens = state.inner.pty_connect_tokens.write().await;
    tokens.prune_expired(now);
    Ok(Json(tokens.issue(pty_id, now)))
}

pub(crate) async fn pty_connect(
    State(state): State<AppState>,
    Path(pty_id): Path<String>,
    Query(query): Query<PtyConnectQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let info = {
        let ptys = state.inner.ptys.read().await;
        pty::get_pty(&*ptys, &pty_id)
            .map_err(|_| ApiError::not_found("PTY session not found"))?
    };
    let ticket = query
        .ticket
        .as_deref()
        .ok_or_else(|| ApiError::forbidden("PTY connect ticket is required"))?;
    state
        .inner
        .pty_connect_tokens
        .write()
        .await
        .validate(&pty_id, ticket, now_millis())
        .map_err(|_| ApiError::forbidden("invalid PTY connect ticket"))?;

    let cursor = query.cursor;
    let publish_state = state.clone();
    Ok(ws
        .on_upgrade(move |socket| async move {
            pty::serve_websocket(info, cursor, socket, move |id, exit_status| {
                publish_state.publish(EventPayload::new(
                    event_type::PTY_EXITED,
                    json!({ "id": id, "ptyID": id, "exitStatus": exit_status }),
                ));
            })
            .await;
        })
        .into_response())
}
