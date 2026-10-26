use axum::extract::{Path, Query, State};
use axum::Json;
use neoism_agent_core::{event_type, EventPayload, MessageWithParts, Part, ToolState};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;
use crate::{now_millis, part_id_of};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct MessageListQuery {
    pub limit: Option<usize>,
    pub order: Option<String>,
    pub slim: Option<bool>,
    pub cursor: Option<String>,
}

pub(crate) async fn message_list(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<MessageListQuery>,
) -> Result<Json<Vec<MessageWithParts>>, ApiError> {
    let started = crate::perf::now();
    state
        .inner
        .store
        .get_session(&session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    // Page in SQL (cursor + limit) instead of loading the whole transcript and
    // slicing in memory — "load older" on a huge session no longer re-reads
    // every message each request.
    let order_desc = query.order.as_deref() == Some("desc");
    let mut messages = state
        .inner
        .store
        .list_messages_page(
            &session_id,
            query.cursor.as_deref(),
            query.limit,
            order_desc,
        )
        .await?;
    if query.slim == Some(true) {
        for message in &mut messages {
            slim_message(message);
        }
    }
    if crate::perf::enabled() {
        let part_count: usize = messages.iter().map(|message| message.parts.len()).sum();
        let json_bytes = serde_json::to_string(&messages).map(|text| text.len()).ok();
        tracing::info!(
            target: "neoism_agent::perf",
            session_id,
            limit = query.limit,
            order = query.order.as_deref(),
            cursor = query.cursor.as_deref(),
            messages = messages.len(),
            part_count,
            json_bytes,
            elapsed_ms = crate::perf::elapsed_ms(started),
            "session message list"
        );
    }
    Ok(Json(messages))
}

fn slim_message(message: &mut MessageWithParts) {
    if let neoism_agent_core::MessageInfo::Assistant(assistant) = &mut message.info {
        assistant.provider_replay = None;
    }
    for part in &mut message.parts {
        match part {
            Part::Tool(part) => {
                part.metadata = slim_metadata(part.metadata.take());
                match &mut part.state {
                    ToolState::Completed { metadata, .. } => {
                        *metadata = slim_metadata_value(std::mem::take(metadata));
                    }
                    ToolState::Pending { .. }
                    | ToolState::Running { .. }
                    | ToolState::Error { .. } => {}
                }
            }
            Part::Reasoning(part) => {
                part.metadata = slim_metadata(part.metadata.take());
            }
            _ => {}
        }
    }
}

fn slim_metadata(metadata: Option<Value>) -> Option<Value> {
    let Some(Value::Object(mut metadata)) = metadata else {
        return metadata;
    };
    if metadata.remove("snapshots").is_some() {
        metadata.insert("snapshotsOmitted".to_string(), Value::Bool(true));
    }
    if let Some(Value::Array(entries)) = metadata.get_mut("diagnostics") {
        for entry in entries {
            slim_diagnostics_entry(entry);
        }
    }
    // Opaque provider replay state; display clients never read it, and it
    // can be most of a reasoning-heavy page.
    if let Some(Value::Object(openai)) = metadata.get_mut("openai") {
        openai.remove("encryptedContent");
    }
    Some(Value::Object(metadata))
}

/// Display clients render `{ message, severity, range.start }` per LSP
/// diagnostic. Drop the bulky remainder (rustc's full `data.rendered`
/// output, related information, code descriptions), which dominated
/// history pages for edit-heavy sessions.
fn slim_diagnostics_entry(entry: &mut Value) {
    let Some(Value::Array(items)) = entry.get_mut("diagnostics") else {
        return;
    };
    for item in items {
        if let Value::Object(item) = item {
            item.retain(|key, _| {
                matches!(key.as_str(), "message" | "severity" | "range" | "source" | "code")
            });
        }
    }
}

fn slim_metadata_value(metadata: Value) -> Value {
    slim_metadata(Some(metadata)).unwrap_or(Value::Null)
}

pub(crate) async fn message_get(
    State(state): State<AppState>,
    Path((session_id, message_id)): Path<(String, String)>,
) -> Result<Json<MessageWithParts>, ApiError> {
    let message = state
        .inner
        .store
        .get_message(&session_id, &message_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Message not found"))?;
    Ok(Json(message))
}

pub(crate) async fn message_delete(
    State(state): State<AppState>,
    Path((session_id, message_id)): Path<(String, String)>,
) -> Result<Json<bool>, ApiError> {
    if !state
        .inner
        .store
        .delete_message(&session_id, &message_id)
        .await?
    {
        return Err(ApiError::not_found("Message not found"));
    }
    state.publish(EventPayload::new(
        event_type::MESSAGE_REMOVED,
        json!({ "sessionID": session_id, "messageID": message_id }),
    ));
    Ok(Json(true))
}

pub(crate) async fn part_delete(
    State(state): State<AppState>,
    Path((session_id, message_id, part_id)): Path<(String, String, String)>,
) -> Result<Json<bool>, ApiError> {
    let mut message = state
        .inner
        .store
        .get_message(&session_id, &message_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Message not found"))?;
    let before = message.parts.len();
    message.parts.retain(|part| part_id_of(part) != part_id);
    if message.parts.len() == before {
        return Err(ApiError::not_found("Part not found"));
    }
    if let neoism_agent_core::MessageInfo::Assistant(assistant) = &mut message.info {
        assistant.provider_replay = None;
    }
    state
        .inner
        .store
        .update_message(&session_id, &message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_PART_REMOVED,
        json!({ "sessionID": session_id, "messageID": message_id, "partID": part_id }),
    ));
    Ok(Json(true))
}

pub(crate) async fn part_update(
    State(state): State<AppState>,
    Path((session_id, message_id, part_id)): Path<(String, String, String)>,
    Json(part): Json<Part>,
) -> Result<Json<Part>, ApiError> {
    if part_id_of(&part) != part_id {
        return Err(ApiError::bad_request("part id does not match route"));
    }
    let mut message = state
        .inner
        .store
        .get_message(&session_id, &message_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Message not found"))?;
    let Some(existing) = message
        .parts
        .iter_mut()
        .find(|existing| part_id_of(existing) == part_id)
    else {
        return Err(ApiError::not_found("Part not found"));
    };
    *existing = part.clone();
    if let neoism_agent_core::MessageInfo::Assistant(assistant) = &mut message.info {
        assistant.provider_replay = None;
    }
    state
        .inner
        .store
        .update_message(&session_id, &message)
        .await?;
    state.publish(EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({ "sessionID": session_id, "part": part, "time": now_millis() }),
    ));
    Ok(Json(part))
}

#[cfg(test)]
mod slim_tests {
    use super::*;

    #[test]
    fn slim_metadata_keeps_displayed_diagnostic_fields_only() {
        let metadata = json!({
            "snapshots": [1, 2],
            "diff": "keep",
            "diagnostics": [{
                "path": "src/lib.rs",
                "source": "rust-analyzer",
                "diagnostics": [{
                    "message": "mismatched types",
                    "severity": "error",
                    "range": { "start": { "line": 3, "character": 4 } },
                    "code": "E0308",
                    "data": { "rendered": "error[E0308]: ... long rustc output" },
                    "related_information": [{ "path": "src/x.rs", "message": "here" }],
                    "codeDescription": { "href": "https://doc.rust-lang.org" }
                }]
            }],
            "openai": { "itemId": "rs_1", "encryptedContent": "opaque" }
        });
        let slim = slim_metadata_value(metadata);
        assert_eq!(slim["snapshotsOmitted"], json!(true));
        assert_eq!(slim["diff"], json!("keep"));
        assert_eq!(slim["diagnostics"][0]["path"], json!("src/lib.rs"));
        assert_eq!(
            slim["diagnostics"][0]["diagnostics"][0],
            json!({
                "message": "mismatched types",
                "severity": "error",
                "range": { "start": { "line": 3, "character": 4 } },
                "code": "E0308"
            })
        );
        assert_eq!(slim["openai"], json!({ "itemId": "rs_1" }));
    }
}
