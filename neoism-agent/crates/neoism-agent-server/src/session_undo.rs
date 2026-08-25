use std::collections::BTreeSet;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{
    event_type, EventPayload, MessageId, MessageInfo, MessageWithParts, Part,
    SessionInfo, SessionUndoTree,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::snapshot;
use crate::state::AppState;
use crate::{ensure_session, message_id_of, now_millis, part_id_of};

#[path = "session_undo_tree.rs"]
mod session_undo_tree;

use session_undo_tree::{
    build_session_undo_tree, decode_persisted_revert, PersistedRevert,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RevertRequest {
    pub message_id: Option<MessageId>,
    pub part_id: Option<neoism_agent_core::PartId>,
}

pub(crate) async fn session_undo_tree(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionUndoTree>, ApiError> {
    let info = ensure_session(&state, &session_id).await?;
    let messages = state.inner.store.list_messages(&session_id).await?;
    Ok(Json(build_session_undo_tree(&info, messages)?))
}

pub(crate) async fn session_revert(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<RevertRequest>,
) -> Result<Json<SessionInfo>, ApiError> {
    let message_id = body
        .message_id
        .ok_or_else(|| ApiError::bad_request("messageId is required"))?
        .to_string();
    let part_id = body.part_id.map(|id| id.to_string());
    Ok(Json(
        revert_session(&state, session_id, message_id, part_id).await?,
    ))
}

async fn revert_session(
    state: &AppState,
    session_id: String,
    message_id: String,
    part_id: Option<String>,
) -> Result<SessionInfo, ApiError> {
    if state.inner.runs.read().await.contains_key(&session_id) {
        return Err(ApiError::conflict("Session is already running"));
    }
    let mut info = ensure_session(state, &session_id).await?;
    let previous_revert = decode_persisted_revert(info.extra.get("revert"))?;
    let mut messages = state.inner.store.list_messages(&session_id).await?;
    let existing_message_ids =
        messages.iter().map(message_id_of).collect::<BTreeSet<_>>();
    if let Some(revert) = &previous_revert {
        if !revert.parts.is_empty() {
            if let Some(message) = messages
                .iter_mut()
                .find(|message| message_id_of(message) == revert.message_id)
            {
                message.parts.extend(revert.parts.clone());
            }
        }
        messages.extend(revert.messages.clone());
    }
    let reconstructed_messages = messages.clone();
    let mut found = false;
    let mut removing = false;
    let mut removed_messages = Vec::new();
    let mut removed_parts = Vec::new();
    let mut target_update = None;

    for mut message in messages {
        let current_id = message_id_of(&message);
        if removing {
            removed_messages.push(message);
            continue;
        }
        if current_id != message_id {
            continue;
        }
        found = true;
        if let Some(part_id) = part_id.as_deref() {
            let Some(index) = message
                .parts
                .iter()
                .position(|part| part_id_of(part) == part_id)
            else {
                return Err(ApiError::not_found("Part not found"));
            };
            removed_parts = message.parts.split_off(index);
            target_update = Some(message);
        } else {
            removed_messages.push(message);
        }
        removing = true;
    }

    if !found {
        return Err(ApiError::not_found("Message not found"));
    }

    let previous_snapshots = previous_revert
        .as_ref()
        .map(|revert| {
            snapshot::collect_from_revert_items(&revert.messages, &revert.parts)
        })
        .unwrap_or_default();
    let snapshots =
        snapshot::collect_from_revert_items(&removed_messages, &removed_parts);
    if !previous_snapshots.is_empty() {
        apply_file_snapshots(
            &info.directory,
            &previous_snapshots,
            snapshot::SnapshotDirection::Unrevert,
        )?;
    }
    if let Err(error) = apply_file_snapshots(
        &info.directory,
        &snapshots,
        snapshot::SnapshotDirection::Revert,
    ) {
        if !previous_snapshots.is_empty() {
            let _ = snapshot::apply(
                &info.directory,
                &previous_snapshots,
                snapshot::SnapshotDirection::Revert,
            );
        }
        return Err(error);
    }

    let target_update_id = target_update.as_ref().map(message_id_of);
    if let Some(message) = target_update.as_ref() {
        let updated = state
            .inner
            .store
            .update_message(&session_id, message)
            .await?;
        if !updated {
            state
                .inner
                .store
                .append_message(&session_id, message)
                .await?;
        }
        state.publish(EventPayload::new(
            event_type::MESSAGE_UPDATED,
            json!({ "sessionID": session_id, "info": message.info }),
        ));
        for part in &removed_parts {
            state.publish(EventPayload::new(
                event_type::MESSAGE_PART_REMOVED,
                json!({ "sessionID": session_id, "messageID": message_id, "partID": part_id_of(part) }),
            ));
        }
    }

    let removed_message_ids = removed_messages
        .iter()
        .map(message_id_of)
        .collect::<BTreeSet<_>>();
    for message in &reconstructed_messages {
        let restored_id = message_id_of(message);
        if existing_message_ids.contains(&restored_id)
            || removed_message_ids.contains(&restored_id)
            || target_update_id.as_deref() == Some(restored_id.as_str())
        {
            continue;
        }
        state
            .inner
            .store
            .append_message(&session_id, message)
            .await?;
        state.publish(EventPayload::new(
            event_type::MESSAGE_UPDATED,
            json!({ "sessionID": session_id, "info": message.info }),
        ));
    }

    for message in &removed_messages {
        let removed_id = message_id_of(message);
        let deleted = state
            .inner
            .store
            .delete_message(&session_id, &removed_id)
            .await?;
        if deleted {
            state.publish(EventPayload::new(
                event_type::MESSAGE_REMOVED,
                json!({ "sessionID": session_id, "messageID": removed_id }),
            ));
        }
    }

    info.time.updated = now_millis();
    info.extra.insert(
        "revert".to_string(),
        json!({
            "messageID": message_id,
            "partID": part_id,
            "time": info.time.updated,
            "messages": removed_messages,
            "parts": removed_parts,
        }),
    );
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session_id, "info": info }),
    ));
    Ok(info)
}

pub(crate) async fn session_unrevert(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionInfo>, ApiError> {
    Ok(Json(unrevert_session(&state, session_id).await?))
}

async fn unrevert_session(
    state: &AppState,
    session_id: String,
) -> Result<SessionInfo, ApiError> {
    if state.inner.runs.read().await.contains_key(&session_id) {
        return Err(ApiError::conflict("Session is already running"));
    }
    let mut info = ensure_session(state, &session_id).await?;
    let Some(revert) = info.extra.get("revert").cloned() else {
        return Ok(info);
    };

    let parts: Vec<Part> = revert
        .get("parts")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| ApiError::internal(error.to_string()))?
        .unwrap_or_default();
    let removed_messages: Vec<MessageWithParts> = revert
        .get("messages")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| ApiError::internal(error.to_string()))?
        .unwrap_or_default();
    let snapshots = snapshot::collect_from_revert_items(&removed_messages, &parts);
    apply_file_snapshots(
        &info.directory,
        &snapshots,
        snapshot::SnapshotDirection::Unrevert,
    )?;

    info.extra.remove("revert");

    if let Some(message_id) = revert.get("messageID").and_then(Value::as_str) {
        if !parts.is_empty() {
            if let Some(mut message) = state
                .inner
                .store
                .get_message(&session_id, message_id)
                .await?
            {
                message.parts.extend(parts);
                state
                    .inner
                    .store
                    .update_message(&session_id, &message)
                    .await?;
                state.publish(EventPayload::new(
                    event_type::MESSAGE_UPDATED,
                    json!({ "sessionID": session_id, "info": message.info }),
                ));
            }
        }
    }

    for message in removed_messages {
        state
            .inner
            .store
            .append_message(&session_id, &message)
            .await?;
        state.publish(EventPayload::new(
            event_type::MESSAGE_UPDATED,
            json!({ "sessionID": session_id, "info": message.info }),
        ));
    }

    info.time.updated = now_millis();
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session_id, "info": info }),
    ));
    Ok(info)
}

/// `/undo` slash command: revert one step. Tolerates an empty request body —
/// the desktop/daemon clients POST with no JSON body, which would otherwise be
/// rejected by axum's `Json` extractor with `415 Unsupported Media Type`.
///
/// Mirrors opencode's TUI `undo`: with no explicit `messageID`, step back to the
/// most recent user message *before* the current revert marker (so repeated
/// undos walk backward through the conversation).
pub(crate) async fn session_undo(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    body: Bytes,
) -> Result<Json<SessionInfo>, ApiError> {
    let request = parse_optional_revert_body(&body)?;
    let info = ensure_session(&state, &session_id).await?;

    if let Some(message_id) = request.message_id {
        let part_id = request.part_id.map(|id| id.to_string());
        return Ok(Json(
            revert_session(&state, session_id, message_id.to_string(), part_id).await?,
        ));
    }

    let revert = decode_persisted_revert(info.extra.get("revert"))?;
    let messages = state.inner.store.list_messages(&session_id).await?;
    let reconstructed = reconstruct_messages(revert.as_ref(), messages);
    let marker = revert.as_ref().map(|revert| revert.message_id.as_str());

    match last_user_message_before(&reconstructed, marker) {
        Some(message_id) => Ok(Json(
            revert_session(&state, session_id, message_id, None).await?,
        )),
        // Nothing earlier to undo — return the current session unchanged.
        None => Ok(Json(info)),
    }
}

/// `/redo` slash command: restore one step. Tolerates an empty request body for
/// the same reason as [`session_undo`].
///
/// Mirrors opencode's TUI `redo`: step forward to the next user message *after*
/// the current revert marker, or fully unrevert when there is none left.
pub(crate) async fn session_redo(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    body: Bytes,
) -> Result<Json<SessionInfo>, ApiError> {
    let request = parse_optional_revert_body(&body)?;
    let info = ensure_session(&state, &session_id).await?;

    let Some(revert) = decode_persisted_revert(info.extra.get("revert"))? else {
        // Nothing has been reverted — nothing to redo.
        return Ok(Json(info));
    };

    if let Some(message_id) = request.message_id {
        return Ok(Json(
            revert_session(&state, session_id, message_id.to_string(), None).await?,
        ));
    }

    let messages = state.inner.store.list_messages(&session_id).await?;
    let reconstructed = reconstruct_messages(Some(&revert), messages);

    match first_user_message_after(&reconstructed, &revert.message_id) {
        // Step the marker forward to the next user turn.
        Some(message_id) => Ok(Json(
            revert_session(&state, session_id, message_id, None).await?,
        )),
        // No later user turn remains — fully restore.
        None => Ok(Json(unrevert_session(&state, session_id).await?)),
    }
}

fn parse_optional_revert_body(body: &Bytes) -> Result<RevertRequest, ApiError> {
    if body.is_empty() {
        return Ok(RevertRequest {
            message_id: None,
            part_id: None,
        });
    }
    serde_json::from_slice(body)
        .map_err(|error| ApiError::bad_request(format!("invalid revert body: {error}")))
}

/// Merge the messages/parts stashed in the session's `revert` marker back into
/// the live message list, recreating the full pre-revert history so undo/redo
/// can locate steps that were already reverted out of the store.
fn reconstruct_messages(
    revert: Option<&PersistedRevert>,
    mut messages: Vec<MessageWithParts>,
) -> Vec<MessageWithParts> {
    if let Some(revert) = revert {
        if !revert.parts.is_empty() {
            if let Some(message) = messages
                .iter_mut()
                .find(|message| message_id_of(message) == revert.message_id)
            {
                message.parts.extend(revert.parts.clone());
            }
        }
        messages.extend(revert.messages.clone());
    }
    messages
}

/// Most recent user message strictly before `before` (or the most recent user
/// message overall when `before` is `None`). IDs are monotonic, so the
/// lexicographic maximum is the latest message.
fn last_user_message_before(
    messages: &[MessageWithParts],
    before: Option<&str>,
) -> Option<String> {
    messages
        .iter()
        .filter(|message| matches!(message.info, MessageInfo::User(_)))
        .map(message_id_of)
        .filter(|id| before.is_none_or(|before| id.as_str() < before))
        .max()
}

/// Earliest user message strictly after `after`.
fn first_user_message_after(
    messages: &[MessageWithParts],
    after: &str,
) -> Option<String> {
    messages
        .iter()
        .filter(|message| matches!(message.info, MessageInfo::User(_)))
        .map(message_id_of)
        .filter(|id| id.as_str() > after)
        .min()
}

fn apply_file_snapshots(
    directory: &str,
    snapshots: &[snapshot::FileSnapshot],
    direction: snapshot::SnapshotDirection,
) -> Result<(), ApiError> {
    match snapshot::apply(directory, snapshots, direction) {
        Ok(_) => Ok(()),
        Err(snapshot::SnapshotApplyError::Conflict(message)) => {
            Err(ApiError::conflict(message))
        }
        Err(snapshot::SnapshotApplyError::Io(error)) => {
            Err(ApiError::internal(error.to_string()))
        }
    }
}
