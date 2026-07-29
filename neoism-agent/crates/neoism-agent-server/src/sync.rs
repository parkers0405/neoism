use std::collections::BTreeMap;

use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, MessageInfo, MessageWithParts, Part,
    PermissionRequestInfo, PromptRequest, PtyInfo, QuestionRequestInfo, SessionInfo,
    SessionStatus, TodoInfo,
};
use serde_json::{json, Map, Value};

use crate::state::{AppState, PersistedEvent};
use crate::{message_id_of, part_id_of};

pub(crate) struct ReplayEvent {
    pub(crate) aggregate_id: String,
    pub(crate) seq: i64,
    pub(crate) payload: EventPayload,
}

pub(crate) fn aggregate_id(payload: &EventPayload) -> String {
    payload
        .properties
        .get("aggregateID")
        .or_else(|| payload.properties.get("aggregateId"))
        .or_else(|| payload.properties.get("sessionID"))
        .or_else(|| payload.properties.get("sessionId"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| payload.id.to_string())
}

pub(crate) fn opencode_history_rows(
    events: Vec<PersistedEvent>,
    known_sequences: &BTreeMap<String, i64>,
) -> Vec<Value> {
    let mut rows = Vec::new();
    for event in events {
        let aggregate_id = event.aggregate_id.clone();
        let seq = event.aggregate_seq;
        if known_sequences
            .get(&aggregate_id)
            .is_some_and(|known| seq <= *known)
        {
            continue;
        }
        rows.push(json!({
            "id": event.payload.id,
            "aggregate_id": aggregate_id,
            "aggregateID": aggregate_id,
            "seq": seq,
            "type": event.payload.kind,
            "data": event.payload.properties,
            "ownerID": event.owner_id,
        }));
    }
    rows
}

pub(crate) fn parse_known_sequences(body: &Value) -> Option<BTreeMap<String, i64>> {
    let object = body.as_object()?;
    if object.keys().any(|key| {
        matches!(
            key.as_str(),
            "since" | "cursor" | "limit" | "sessionID" | "sessionId"
        )
    }) {
        return None;
    }
    let mut sequences = BTreeMap::new();
    for (key, value) in object {
        let seq = value.as_i64()?;
        if seq < 0 {
            return None;
        }
        sequences.insert(key.clone(), seq);
    }
    Some(sequences)
}

pub(crate) fn replay_event_payload(event: &Value) -> anyhow::Result<ReplayEvent> {
    let aggregate_id = required_str(event, "aggregateID")
        .or_else(|_| required_str(event, "aggregate_id"))?
        .to_string();
    let seq = event
        .get("seq")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("sync replay event seq is required"))?;
    if seq < 0 {
        anyhow::bail!("sync replay event seq must be non-negative");
    }
    let kind = required_str(event, "type")?.to_string();
    let id = event
        .get("id")
        .and_then(Value::as_str)
        .and_then(|value| Id::parse(IdKind::Event, value).ok())
        .unwrap_or_else(|| Id::ascending(IdKind::Event));
    let mut data = event
        .get("data")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("sync replay event data is required"))?;
    if let Value::Object(ref mut object) = data {
        ensure_aggregate_fields(object, &aggregate_id);
    }
    Ok(ReplayEvent {
        aggregate_id,
        seq,
        payload: EventPayload {
            id,
            kind,
            properties: data,
        },
    })
}

pub(crate) async fn replay_all(
    state: &AppState,
    events: Vec<ReplayEvent>,
    owner_id: Option<&str>,
    publish: bool,
) -> anyhow::Result<Option<String>> {
    let Some(source) = events.first().map(|event| event.aggregate_id.clone()) else {
        return Ok(None);
    };
    if events.iter().any(|event| event.aggregate_id != source) {
        anyhow::bail!("Replay events must belong to the same session");
    }
    let start = events[0].seq;
    for (index, event) in events.iter().enumerate() {
        let expected = start + index as i64;
        if event.seq != expected {
            anyhow::bail!(
                "Replay sequence mismatch at index {index}: expected {expected}, got {}",
                event.seq
            );
        }
    }
    for event in events {
        replay_one(state, event, owner_id, publish).await?;
    }
    Ok(Some(source))
}

async fn replay_one(
    state: &AppState,
    event: ReplayEvent,
    owner_id: Option<&str>,
    publish: bool,
) -> anyhow::Result<()> {
    let latest = state
        .inner
        .store
        .aggregate_sequence(&event.aggregate_id)
        .await?;
    let latest_seq = latest.as_ref().map(|row| row.seq).unwrap_or(-1);
    if event.seq <= latest_seq {
        return Ok(());
    }
    if let Some(existing_owner) = latest.as_ref().and_then(|row| row.owner_id.as_deref())
    {
        if Some(existing_owner) != owner_id {
            return Ok(());
        }
    }
    let expected = latest_seq + 1;
    if event.seq != expected {
        anyhow::bail!(
            "Sequence mismatch for aggregate {}: expected {}, got {}",
            event.aggregate_id,
            expected,
            event.seq
        );
    }
    project_event(state, &event).await?;
    if publish {
        state
            .publish_persisted_with_owner(event.payload, owner_id)
            .await?;
    } else {
        state
            .inner
            .store
            .append_event_with_owner(&event.payload, owner_id)
            .await?;
    }
    Ok(())
}

async fn project_event(state: &AppState, event: &ReplayEvent) -> anyhow::Result<()> {
    match event.payload.kind.as_str() {
        event_type::SESSION_CREATED | event_type::SESSION_UPDATED => {
            if let Some(info) = event
                .payload
                .properties
                .get("info")
                .cloned()
                .map(serde_json::from_value::<SessionInfo>)
                .transpose()?
            {
                if state
                    .inner
                    .store
                    .get_session(info.id.as_str())
                    .await?
                    .is_some()
                {
                    state.inner.store.update_session(&info).await?;
                } else {
                    state.inner.store.insert_session(&info).await?;
                }
            }
        }
        event_type::SESSION_DELETED => {
            state
                .inner
                .store
                .delete_session(&event.aggregate_id)
                .await?;
        }
        event_type::SESSION_COMPACTED => {
            project_session_compacted(state, &event.payload).await?
        }
        event_type::SESSION_QUEUE_UPDATED => {
            project_session_queue_updated(state, &event.payload).await?
        }
        event_type::PTY_CREATED | event_type::PTY_UPDATED => {
            project_pty_upserted(state, &event.payload).await?
        }
        event_type::PTY_DELETED | event_type::PTY_EXITED => {
            project_pty_removed(state, &event.payload).await?
        }
        event_type::MESSAGE_UPDATED => {
            project_message_updated(state, &event.payload).await?
        }
        event_type::MESSAGE_REMOVED => {
            let Some((session_id, message_id)) =
                payload_session_and_message(&event.payload)
            else {
                return Ok(());
            };
            state
                .inner
                .store
                .delete_message(&session_id, &message_id)
                .await?;
        }
        event_type::MESSAGE_PART_UPDATED => {
            project_part_updated(state, &event.payload).await?
        }
        event_type::MESSAGE_PART_REMOVED => {
            project_part_removed(state, &event.payload).await?
        }
        event_type::SESSION_STATUS => {
            project_session_status(state, &event.payload).await?
        }
        event_type::PERMISSION_ASKED => {
            project_permission_asked(state, &event.payload).await?
        }
        event_type::PERMISSION_REPLIED => {
            if let Some(request_id) = payload_request_id(&event.payload) {
                state.inner.permissions.write().await.remove(&request_id);
            }
        }
        event_type::QUESTION_ASKED => {
            project_question_asked(state, &event.payload).await?
        }
        event_type::QUESTION_REPLIED | event_type::QUESTION_REJECTED => {
            if let Some(request_id) = payload_request_id(&event.payload) {
                state.inner.questions.write().await.remove(&request_id);
            }
        }
        event_type::TODO_UPDATED => {
            let Some(session_id) = payload_session_id(&event.payload) else {
                return Ok(());
            };
            let todos = event
                .payload
                .properties
                .get("todos")
                .cloned()
                .map(serde_json::from_value::<Vec<TodoInfo>>)
                .transpose()?
                .unwrap_or_default();
            state.inner.todos.write().await.insert(session_id, todos);
        }
        _ => {}
    }
    Ok(())
}

async fn project_session_compacted(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some(info) = payload
        .properties
        .get("info")
        .cloned()
        .map(serde_json::from_value::<SessionInfo>)
        .transpose()?
    else {
        return Ok(());
    };
    if state
        .inner
        .store
        .get_session(info.id.as_str())
        .await?
        .is_some()
    {
        state.inner.store.update_session(&info).await?;
    } else {
        state.inner.store.insert_session(&info).await?;
    }
    Ok(())
}

async fn project_session_queue_updated(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some(session_id) = payload_session_id(payload) else {
        return Ok(());
    };
    let action = payload
        .properties
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("snapshot");
    match action {
        "enqueue" => {
            if let Some(request) = payload
                .properties
                .get("request")
                .cloned()
                .map(serde_json::from_value::<PromptRequest>)
                .transpose()?
            {
                state
                    .inner
                    .store
                    .enqueue_prompt(&session_id, &request)
                    .await?;
            }
        }
        "pop" | "dequeue" => {
            let _ = state.inner.store.pop_queued_prompt(&session_id).await?;
        }
        "clear" => {
            let _ = state.inner.store.clear_queued_prompts(&session_id).await?;
        }
        "snapshot" => {
            let _ = state.inner.store.clear_queued_prompts(&session_id).await?;
            if let Some(requests) =
                payload.properties.get("requests").and_then(Value::as_array)
            {
                for request in requests {
                    let request =
                        serde_json::from_value::<PromptRequest>(request.clone())?;
                    state
                        .inner
                        .store
                        .enqueue_prompt(&session_id, &request)
                        .await?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

async fn project_pty_upserted(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some(info) = payload
        .properties
        .get("info")
        .cloned()
        .map(serde_json::from_value::<PtyInfo>)
        .transpose()?
    else {
        return Ok(());
    };
    state.inner.ptys.write().await.insert(info.id.clone(), info);
    Ok(())
}

async fn project_pty_removed(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some(pty_id) = payload
        .properties
        .get("ptyID")
        .or_else(|| payload.properties.get("ptyId"))
        .or_else(|| payload.properties.get("id"))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    state.inner.ptys.write().await.remove(pty_id);
    Ok(())
}

async fn project_session_status(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some(session_id) = payload_session_id(payload) else {
        return Ok(());
    };
    let Some(status) = payload
        .properties
        .get("status")
        .cloned()
        .map(serde_json::from_value::<SessionStatus>)
        .transpose()?
    else {
        return Ok(());
    };
    match status {
        SessionStatus::Idle => {
            state.inner.statuses.write().await.remove(&session_id);
        }
        SessionStatus::Busy { .. } | SessionStatus::Retry { .. } => {
            state
                .inner
                .statuses
                .write()
                .await
                .insert(session_id, status);
        }
    }
    Ok(())
}

async fn project_permission_asked(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some(request) = payload_entity::<PermissionRequestInfo>(payload)? else {
        return Ok(());
    };
    state
        .inner
        .permissions
        .write()
        .await
        .insert(request.id.clone(), request);
    Ok(())
}

async fn project_question_asked(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some(request) = payload_entity::<QuestionRequestInfo>(payload)? else {
        return Ok(());
    };
    state
        .inner
        .questions
        .write()
        .await
        .insert(request.id.clone(), request);
    Ok(())
}

async fn project_message_updated(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some(session_id) = payload_session_id(payload) else {
        return Ok(());
    };
    let Some(info) = payload
        .properties
        .get("info")
        .cloned()
        .map(serde_json::from_value::<MessageInfo>)
        .transpose()?
    else {
        return Ok(());
    };
    let mut message = state
        .inner
        .store
        .get_message(&session_id, message_id_for_info(&info).as_str())
        .await?
        .unwrap_or_else(|| MessageWithParts {
            info: info.clone(),
            parts: Vec::new(),
        });
    message.info = info;
    if state
        .inner
        .store
        .get_message(&session_id, message_id_of(&message).as_str())
        .await?
        .is_some()
    {
        state
            .inner
            .store
            .update_message(&session_id, &message)
            .await?;
    } else {
        state
            .inner
            .store
            .append_message(&session_id, &message)
            .await?;
    }
    Ok(())
}

async fn project_part_updated(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some(part) = payload
        .properties
        .get("part")
        .cloned()
        .map(serde_json::from_value::<Part>)
        .transpose()?
    else {
        return Ok(());
    };
    let session_id = part_session_id(&part);
    let message_id = part_message_id(&part);
    let Some(mut message) = state
        .inner
        .store
        .get_message(&session_id, &message_id)
        .await?
    else {
        return Ok(());
    };
    let part_id = part_id_of(&part);
    match message
        .parts
        .iter_mut()
        .find(|existing| part_id_of(existing) == part_id)
    {
        Some(existing) => *existing = part,
        None => message.parts.push(part),
    }
    state
        .inner
        .store
        .update_message(&session_id, &message)
        .await?;
    Ok(())
}

async fn project_part_removed(
    state: &AppState,
    payload: &EventPayload,
) -> anyhow::Result<()> {
    let Some((session_id, message_id)) = payload_session_and_message(payload) else {
        return Ok(());
    };
    let Some(part_id) = payload
        .properties
        .get("partID")
        .or_else(|| payload.properties.get("partId"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        return Ok(());
    };
    let Some(mut message) = state
        .inner
        .store
        .get_message(&session_id, &message_id)
        .await?
    else {
        return Ok(());
    };
    message.parts.retain(|part| part_id_of(part) != part_id);
    state
        .inner
        .store
        .update_message(&session_id, &message)
        .await?;
    Ok(())
}

fn payload_session_id(payload: &EventPayload) -> Option<String> {
    payload
        .properties
        .get("sessionID")
        .or_else(|| payload.properties.get("sessionId"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn payload_session_and_message(payload: &EventPayload) -> Option<(String, String)> {
    let session_id = payload_session_id(payload)?;
    let message_id = payload
        .properties
        .get("messageID")
        .or_else(|| payload.properties.get("messageId"))
        .and_then(Value::as_str)?
        .to_string();
    Some((session_id, message_id))
}

fn payload_request_id(payload: &EventPayload) -> Option<String> {
    payload
        .properties
        .get("requestID")
        .or_else(|| payload.properties.get("requestId"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            payload
                .properties
                .get("info")
                .and_then(|info| info.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}

fn payload_entity<T>(payload: &EventPayload) -> anyhow::Result<Option<T>>
where
    T: serde::de::DeserializeOwned,
{
    let value = payload
        .properties
        .get("info")
        .cloned()
        .unwrap_or_else(|| payload.properties.clone());
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value).map(Some).map_err(Into::into)
}

fn message_id_for_info(info: &MessageInfo) -> String {
    match info {
        MessageInfo::User(info) => info.id.to_string(),
        MessageInfo::Assistant(info) => info.id.to_string(),
    }
}

fn part_session_id(part: &Part) -> String {
    match part {
        Part::Text(part) => part.session_id.to_string(),
        Part::Compaction(part) => part.session_id.to_string(),
        Part::Agent(part) => part.session_id.to_string(),
        Part::Subtask(part) => part.session_id.to_string(),
        Part::Reasoning(part) => part.session_id.to_string(),
        Part::Tool(part) => part.session_id.to_string(),
        Part::StepStart(part) => part.session_id.to_string(),
        Part::StepFinish(part) => part.session_id.to_string(),
        Part::File(part) => part.session_id.to_string(),
    }
}

fn part_message_id(part: &Part) -> String {
    match part {
        Part::Text(part) => part.message_id.to_string(),
        Part::Compaction(part) => part.message_id.to_string(),
        Part::Agent(part) => part.message_id.to_string(),
        Part::Subtask(part) => part.message_id.to_string(),
        Part::Reasoning(part) => part.message_id.to_string(),
        Part::Tool(part) => part.message_id.to_string(),
        Part::StepStart(part) => part.message_id.to_string(),
        Part::StepFinish(part) => part.message_id.to_string(),
        Part::File(part) => part.message_id.to_string(),
    }
}

fn required_str<'a>(value: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("sync replay event {key} is required"))
}

fn ensure_aggregate_fields(object: &mut Map<String, Value>, aggregate_id: &str) {
    object
        .entry("aggregateID".to_string())
        .or_insert_with(|| Value::String(aggregate_id.to_string()));
    if aggregate_id.starts_with("ses_") {
        object
            .entry("sessionID".to_string())
            .or_insert_with(|| Value::String(aggregate_id.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{event_type, PromptPart, SessionQueueStatus, TimeInfo};

    #[test]
    fn opencode_history_uses_aggregate_sequences() {
        let session_id = "ses_test";
        let payload = EventPayload::new(
            event_type::SESSION_STATUS,
            json!({ "sessionID": session_id, "status": { "type": "idle" } }),
        );
        let rows = opencode_history_rows(
            vec![PersistedEvent {
                seq: 1,
                aggregate_id: session_id.to_string(),
                aggregate_seq: 0,
                owner_id: None,
                payload,
            }],
            &BTreeMap::from([(session_id.to_string(), -1)]),
        );
        assert_eq!(rows[0]["aggregate_id"], session_id);
        assert_eq!(rows[0]["seq"], 0);
        assert_eq!(rows[0]["data"]["sessionID"], session_id);
    }

    #[test]
    fn known_sequences_filter_old_aggregate_events() {
        let session_id = "ses_test";
        let payload = EventPayload::new(
            event_type::SESSION_STATUS,
            json!({ "sessionID": session_id, "status": { "type": "idle" } }),
        );
        let rows = opencode_history_rows(
            vec![PersistedEvent {
                seq: 1,
                aggregate_id: session_id.to_string(),
                aggregate_seq: 0,
                owner_id: None,
                payload,
            }],
            &BTreeMap::from([(session_id.to_string(), 0)]),
        );
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn replay_projects_status_permission_and_question_state() {
        let db = std::env::temp_dir().join(format!(
            "neoism-agent-sync-live-state-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db.clone()).await.unwrap();
        let session_id = "ses_sync_live";
        let permission_id = "perm_sync";
        let question_id = "que_sync";

        let permission = PermissionRequestInfo {
            id: permission_id.to_string(),
            session_id: session_id.to_string(),
            message_id: "msg_sync".to_string(),
            title: "Allow edit?".to_string(),
            permission: "edit".to_string(),
            patterns: vec!["TASK.md".to_string()],
            always: vec!["TASK.md".to_string()],
            tool: None,
            metadata: None,
        };
        let question = QuestionRequestInfo {
            id: question_id.to_string(),
            session_id: session_id.to_string(),
            message_id: "msg_sync".to_string(),
            questions: vec![json!({ "question": "Proceed?" })],
        };

        replay_all(
            &state,
            vec![
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 0,
                    payload: EventPayload::new(
                        event_type::SESSION_STATUS,
                        json!({
                            "sessionID": session_id,
                            "status": {
                                "type": "busy",
                                "queue": { "count": 1, "preview": "next turn" }
                            }
                        }),
                    ),
                },
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 1,
                    payload: EventPayload::new(event_type::PERMISSION_ASKED, json!(permission)),
                },
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 2,
                    payload: EventPayload::new(event_type::QUESTION_ASKED, json!(question)),
                },
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 3,
                    payload: EventPayload::new(
                        event_type::PERMISSION_REPLIED,
                        json!({ "sessionID": session_id, "requestID": permission_id, "reply": "once" }),
                    ),
                },
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 4,
                    payload: EventPayload::new(
                        event_type::QUESTION_REJECTED,
                        json!({ "sessionID": session_id, "requestID": question_id }),
                    ),
                },
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 5,
                    payload: EventPayload::new(
                        event_type::SESSION_STATUS,
                        json!({ "sessionID": session_id, "status": { "type": "idle" } }),
                    ),
                },
            ],
            Some("owner"),
            false,
        )
        .await
        .unwrap();

        assert!(!state
            .inner
            .permissions
            .read()
            .await
            .contains_key(permission_id));
        assert!(!state.inner.questions.read().await.contains_key(question_id));
        assert!(!state.inner.statuses.read().await.contains_key(session_id));

        let row = state
            .inner
            .store
            .aggregate_sequence(session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.seq, 5);
        assert_eq!(row.owner_id.as_deref(), Some("owner"));

        let _ = std::fs::remove_file(db);
    }

    #[tokio::test]
    async fn replay_projects_queue_compaction_and_pty_state() {
        let db = std::env::temp_dir().join(format!(
            "neoism-agent-sync-projector-extra-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db.clone()).await.unwrap();
        let session_id = "ses_sync_extra";
        let pty_id = "pty_sync_extra";
        let mut session = SessionInfo {
            id: Id::parse(IdKind::Session, session_id).unwrap(),
            slug: "sync-extra".to_string(),
            project_id: "proj_sync".to_string(),
            workspace_id: None,
            directory: "/tmp".to_string(),
            path: None,
            parent_id: None,
            title: "Sync extra".to_string(),
            agent: None,
            model: None,
            version: "0.1.0".to_string(),
            time: TimeInfo {
                created: 1,
                updated: 1,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: BTreeMap::new(),
        };
        let request = PromptRequest {
            message_id: None,
            model: None,
            agent: None,
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text {
                text: "queued".to_string(),
            }],
        };
        let pty = PtyInfo {
            id: pty_id.to_string(),
            command: vec!["/bin/sh".to_string()],
            cwd: "/tmp".to_string(),
            title: "shell".to_string(),
            time: 2,
        };
        session.extra.insert(
            "summary".to_string(),
            json!({ "text": "compacted", "messageCount": 0 }),
        );

        replay_all(
            &state,
            vec![
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 0,
                    payload: EventPayload::new(
                        event_type::SESSION_CREATED,
                        json!({ "sessionID": session_id, "info": session.clone() }),
                    ),
                },
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 1,
                    payload: EventPayload::new(
                        event_type::SESSION_QUEUE_UPDATED,
                        json!({ "sessionID": session_id, "action": "enqueue", "request": request }),
                    ),
                },
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 2,
                    payload: EventPayload::new(
                        event_type::SESSION_COMPACTED,
                        json!({ "sessionID": session_id, "info": session, "summary": { "text": "compacted" } }),
                    ),
                },
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 3,
                    payload: EventPayload::new(
                        event_type::PTY_CREATED,
                        json!({ "aggregateID": session_id, "id": pty_id, "ptyID": pty_id, "info": pty }),
                    ),
                },
                ReplayEvent {
                    aggregate_id: session_id.to_string(),
                    seq: 4,
                    payload: EventPayload::new(
                        event_type::PTY_EXITED,
                        json!({ "aggregateID": session_id, "id": pty_id, "ptyID": pty_id, "exitStatus": 0 }),
                    ),
                },
            ],
            Some("owner"),
            false,
        )
        .await
        .unwrap();

        assert_eq!(
            state
                .inner
                .store
                .queued_prompt_count(session_id)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            state
                .inner
                .store
                .get_session(session_id)
                .await
                .unwrap()
                .unwrap()
                .extra["summary"]["text"],
            "compacted"
        );
        assert!(!state.inner.ptys.read().await.contains_key(pty_id));

        let _ = std::fs::remove_file(db);
    }

    #[test]
    fn session_status_payload_round_trips_busy_queue_shape() {
        let payload = EventPayload::new(
            event_type::SESSION_STATUS,
            json!({
                "sessionID": "ses_status",
                "status": {
                    "type": "busy",
                    "queue": { "count": 2, "preview": "queued" }
                }
            }),
        );
        let status: SessionStatus =
            serde_json::from_value(payload.properties["status"].clone())
                .expect("session status");
        match status {
            SessionStatus::Busy {
                queue:
                    Some(SessionQueueStatus {
                        count,
                        preview: Some(preview),
                    }),
            } => {
                assert_eq!(count, 2);
                assert_eq!(preview, "queued");
            }
            other => panic!("expected busy queue, got {other:?}"),
        }
    }
}
