use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::Ordering;

use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, MessageId, MessageInfo, MessageWithParts, Part,
    PermissionAction, PermissionRule, PromptPart, PromptRequest, SessionInfo, TimeInfo,
    UserModel,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::command_routes::{expand_command_template, find_command};
use crate::error::ApiError;
use crate::state::AppState;
use crate::{
    append_prompt, ensure_session, model_ref_from_config, model_ref_from_user_model,
    now_millis, publish_idle_if_no_run, slug, user_model_from_model_ref,
};

const SUBTASK_COMPLETION_SYSTEM_MARKER: &str =
    "Agent runtime notification: background subagent completion.";
const SUBTASK_RESULT_INLINE_CHARS: usize = 32_000;
const SUBTASK_COMPLETION_EXTRA_KEY: &str = "subtaskCompletion";
const SUBTASK_COMPLETIONS_EXTRA_KEY: &str = "subtaskCompletions";
const SUBTASK_PERSISTENCE_VERSION_KEY: &str = "subtaskPersistenceVersion";
const SUBTASK_PERSISTENCE_VERSION: u64 = 3;

pub(crate) fn recorded_subtask_terminal_status(
    child: &SessionInfo,
    started_at: u64,
) -> Option<&'static str> {
    fn terminal_status(record: &Value, started_at: u64) -> Option<&'static str> {
        if record.get("completedAt").and_then(Value::as_u64)? < started_at {
            return None;
        }
        match record.get("status").and_then(Value::as_str)? {
            "completed" | "complete" | "done" => Some("completed"),
            "failed" | "error" | "errored" | "stopped" | "aborted" | "cancelled" => {
                Some("failed")
            }
            _ => None,
        }
    }

    child
        .extra
        .get(SUBTASK_COMPLETIONS_EXTRA_KEY)
        .and_then(Value::as_array)
        .and_then(|records| {
            records
                .iter()
                .rev()
                .find_map(|record| terminal_status(record, started_at))
        })
        .or_else(|| {
            child
                .extra
                .get(SUBTASK_COMPLETION_EXTRA_KEY)
                .and_then(|record| terminal_status(record, started_at))
        })
}

/// Set on a child when a continue-prompt is QUEUED onto it (the task tool's
/// child-already-running branch). The queued prompt runs through the generic
/// queue worker — no spawn wrapper exists to publish the completion — so the
/// child's next true-idle point publishes it instead.
const SUBTASK_NOTIFY_ON_IDLE_KEY: &str = "subtaskNotifyOnIdle";

#[derive(Clone, Debug)]
struct PendingSubtaskCompletion {
    child: SessionInfo,
    message_id: MessageId,
    status: String,
    text: String,
    completed_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionCommandRequest {
    pub message_id: Option<MessageId>,
    pub model: Option<UserModel>,
    pub agent: Option<String>,
    pub command: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionShellRequest {
    pub message_id: Option<MessageId>,
    pub model: Option<UserModel>,
    pub agent: Option<String>,
    pub command: String,
}

pub(crate) async fn abort_session_run(state: &AppState, session_id: &str) -> bool {
    let aborted = abort_session_run_impl(state, session_id, true).await;
    if aborted {
        let _ = crate::execution_activity::finish_subtask_for_child(
            state,
            session_id,
            "cancelled",
        )
        .await;
    }
    aborted
}

pub(crate) async fn clear_subtask_completion_for_teardown(state: &AppState, session_id: &str) {
    if let Ok(Some(mut child)) = state.inner.store.get_session(session_id).await {
        if child.extra.remove(SUBTASK_NOTIFY_ON_IDLE_KEY).is_some() {
            if let Err(error) = state.inner.store.update_session(&child).await {
                tracing::warn!(%error, session_id, "failed to clear subtask completion during workspace teardown");
            }
        }
    }
}

async fn abort_session_run_impl(state: &AppState, session_id: &str, reconcile_completion: bool) -> bool {
    let cancelled = state.inner.session_coordinator.abort_run(session_id).await;
    if let Some(cancelled) = cancelled.as_ref() {
        cancelled.cancel.store(true, Ordering::SeqCst);
    }
    let was_busy = state
        .inner
        .statuses
        .write()
        .await
        .remove(session_id)
        .is_some();
    publish_idle_if_no_run(state, session_id).await;

    let permission_ids = {
        let permissions = state.inner.permissions.read().await;
        permissions
            .iter()
            .filter(|(_, request)| request.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };
    if !permission_ids.is_empty() {
        let mut permissions = state.inner.permissions.write().await;
        let mut waiters = state.inner.permission_waiters.write().await;
        for id in permission_ids {
            let removed = permissions.remove(&id);
            if let Some(pending) = waiters.remove(&id) {
                let _ = pending.sender.send(Err("Session aborted".to_string()));
            }
            state.publish(EventPayload::new(
                event_type::PERMISSION_REPLIED,
                json!({ "requestID": id, "reply": "reject", "info": removed }),
            ));
        }
    }

    let question_ids = {
        let questions = state.inner.questions.read().await;
        questions
            .iter()
            .filter(|(_, request)| request.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };
    if !question_ids.is_empty() {
        let mut questions = state.inner.questions.write().await;
        let mut waiters = state.inner.question_waiters.write().await;
        for id in question_ids {
            let removed = questions.remove(&id);
            if let Some(pending) = waiters.remove(&id) {
                let _ = pending.sender.send(Err("Session aborted".to_string()));
            }
            state.publish(EventPayload::new(
                event_type::QUESTION_REJECTED,
                json!({ "requestID": id, "info": removed }),
            ));
        }
    }

    if reconcile_completion {
        reconcile_parent_subtask_completions_for_child(state, session_id).await;
    }

    cancelled.is_some() || was_busy
}

pub(crate) async fn create_subtask_session(
    state: &AppState,
    parent: &SessionInfo,
    command: &str,
    description: &str,
    agent: &str,
    model: Option<UserModel>,
) -> Result<SessionInfo, ApiError> {
    let snapshot = state.plugin_snapshot(&parent.directory).await;
    let agents = crate::plugins::agent_catalog(&snapshot, &parent.directory)?;
    let agent_info = agents.get(agent).ok_or_else(|| {
        let available = agents
            .list()
            .into_iter()
            .filter(|agent| agent.mode == "subagent")
            .map(|agent| agent.name)
            .collect::<Vec<_>>()
            .join(", ");
        ApiError::bad_request(format!(
            "unknown agent {agent}; available subagents: {available}"
        ))
    })?;
    let now = now_millis();
    let child_id = neoism_agent_core::new_session_id();
    let title = if description.trim().is_empty() {
        format!("Task: {command}")
    } else {
        format!("{} (@{} subagent)", description.trim(), agent_info.name)
    };
    let child = SessionInfo {
        id: child_id.clone(),
        slug: slug(),
        project_id: parent.project_id.clone(),
        workspace_id: parent.workspace_id.clone(),
        directory: parent.directory.clone(),
        path: parent.path.clone(),
        parent_id: Some(parent.id.clone()),
        title,
        agent: Some(agent_info.name.clone()),
        model: model.as_ref().map(model_ref_from_user_model),
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: now,
            updated: now,
            compacting: None,
            archived: None,
        },
        permission: Some(subtask_permission(parent, &agent_info)),
        extra: [
            crate::execution_activity::EXECUTION_ID_KEY,
            crate::execution_activity::EXECUTION_ROOT_KEY,
            crate::caller::TENANT_EXTRA_KEY,
        ]
        .into_iter()
        .filter_map(|key| parent.extra.get(key).cloned().map(|value| (key.to_string(), value)))
        .collect(),
    };
    state.inner.store.insert_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_CREATED,
        json!({ "sessionID": child_id, "info": child }),
    ));
    Ok(child)
}

pub(crate) async fn append_child_subtask_prompt(
    state: &AppState,
    child_id: &str,
    generation: MessageId,
    prompt: &str,
    agent: String,
    model: Option<UserModel>,
) -> Result<MessageWithParts, ApiError> {
    Box::pin(append_prompt(
        state,
        child_id,
        PromptRequest {
            message_id: Some(generation),
            model,
            agent: Some(agent),
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text {
                text: prompt.to_string(),
            }],
        },
        true,
    ))
    .await
}

pub(crate) fn spawn_background_subtask_prompt(
    state: AppState,
    child_id: String,
    generation: MessageId,
    prompt: String,
    agent: String,
    model: Option<UserModel>,
    _plugin_generation: Option<crate::workspace_runtime::PluginGenerationLease>,
    admission: crate::execution_activity::SubtaskAdmissionGuard,
) {
    tokio::spawn(async move {
        let plugin_generation = _plugin_generation;
        match append_child_subtask_prompt(
            &state,
            &child_id,
            generation.clone(),
            &prompt,
            agent,
            model,
        )
        .await
        {
            Ok(message) => {
                let result = last_text_part(&message).unwrap_or_default();
                publish_background_subtask_finished(
                    &state,
                    &child_id,
                    &generation,
                    "completed",
                    &result,
                )
                .await;
                admission.complete("completed").await;
            }
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(
                    session_id = %child_id,
                    error = %message,
                    "background subtask failed"
                );
                publish_background_subtask_finished(
                    &state,
                    &child_id,
                    &generation,
                    "error",
                    &message,
                )
                .await;
                admission.complete("failed").await;
            }
        }
        drop(plugin_generation);
    });
}

pub(crate) async fn publish_background_subtask_finished(
    state: &AppState,
    child_id: &str,
    generation: &MessageId,
    status: &str,
    text: &str,
) {
    // Completion delivery is durable session state, not optional UI runtime
    // state. In particular, the embedded Neoism server may finish a child
    // after the workspace/plugin tracker generation that launched it has been
    // retired or before a replacement tracker is allocated. Requiring the
    // in-memory tracker here strands the result even though the child still
    // carries the exact generation it owes its parent. The durable generation
    // check in `mark_subtask_completion_pending` is the authority and also
    // preserves exactly-once delivery.
    if subtask_has_active_work(state, child_id).await {
        return;
    }
    let inline_result = subtask_result_inline(text);
    let mutation_lock =
        subtask_keyed_lock(&state.inner.subtask_completion_locks, child_id).await;
    let mutation = {
        let _guard = mutation_lock.lock().await;
        match mark_subtask_completion_pending(
            state,
            child_id,
            generation,
            status,
            &inline_result,
        )
        .await
        {
            Ok(mutation) => mutation,
            Err(error) => {
                tracing::warn!(session_id = %child_id, %error, "failed to persist pending subtask completion");
                None
            }
        }
    };
    let completion = mutation.and_then(|(child, _created)| {
        child
            .parent_id
            .as_ref()
            .map(ToString::to_string)
            .map(|parent_id| (child, parent_id))
    });

    // Admit the parent continuation while this branch is still outstanding.
    // Otherwise the last child can make the family quiescent first, and a
    // freshly-finished execution then rejects the continuation because its
    // own queue row counts as active work. The pending outbox record remains
    // authoritative if admission fails and is retried by reconciliation.
    if let Some((child, parent_id)) = completion.as_ref() {
        if let Err(error) =
            enqueue_parent_subtask_completion_prompts_if_ready(state, parent_id).await
        {
            tracing::warn!(
                session_id = %child.id,
                parent_id = %parent_id,
                error = %error,
                "failed to notify parent session about completed subtask"
            );
        }
    }

    // Execution lifecycle is authoritative and must not depend on whether the
    // optional UI notification runtime is loaded or still tracks this child.
    let lifecycle = match crate::execution_activity::finish_subtask_for_child(
        state, child_id, status,
    )
    .await
    {
        Ok(lifecycle) => lifecycle,
        Err(error) => {
            tracing::warn!(session_id = %child_id, %error, "failed to terminalize subtask lifecycle");
            return;
        }
    };
    let Some((child, parent_id)) = completion else {
        return;
    };
    {
        let mut payload = json!({
            "sessionID": parent_id.clone(),
            "parentSessionID": parent_id.clone(),
            "childSessionID": child.id.to_string(),
            "taskID": child.id.to_string(),
            "status": status,
            "title": child.title.clone(),
            "result": inline_result,
        });
        if let Some(agent) = child.agent.as_ref() {
            payload["agent"] = json!(agent);
            payload["sourceAgent"] = json!(agent);
        }
        if let Some((root_session_id, execution_id, family_revision)) = lifecycle.as_ref() {
            payload["rootSessionID"] = json!(root_session_id);
            payload["executionID"] = json!(execution_id);
            payload["familyRevision"] = json!(family_revision);
        }
        state.publish(EventPayload::new(
            event_type::SESSION_SUBTASK_COMPLETED,
            payload,
        ));
    }
}

async fn mark_subtask_completion_pending(
    state: &AppState,
    child_id: &str,
    generation: &MessageId,
    status: &str,
    text: &str,
) -> Result<Option<(SessionInfo, bool)>, ApiError> {
    let Some(mut child) = state.inner.store.get_session(child_id).await? else {
        return Ok(None);
    };
    if child.parent_id.is_none() {
        return Ok(None);
    }
    if owed_completion_generation(&child) != Some(generation.clone()) {
        return Ok(None);
    }
    let repaired = if child
        .extra
        .get(SUBTASK_COMPLETIONS_EXTRA_KEY)
        .is_some_and(Value::is_array)
    {
        false
    } else {
        child
            .extra
            .insert(SUBTASK_COMPLETIONS_EXTRA_KEY.to_string(), json!([]));
        true
    };
    let existing_pending = child.extra[SUBTASK_COMPLETIONS_EXTRA_KEY]
        .as_array()
        .and_then(|records| {
            records.iter().find(|record| {
                record.get("generation").and_then(Value::as_str)
                    == Some(generation.as_str())
            })
        })
        .map(|record| record.get("pending").and_then(Value::as_bool) == Some(true));
    if let Some(pending) = existing_pending {
        clear_owed_generation_if_matching(&mut child, generation);
        if repaired || child.extra.get(SUBTASK_NOTIFY_ON_IDLE_KEY).is_none() {
            child.time.updated = now_millis().max(child.time.updated);
            state.inner.store.update_session(&child).await?;
            state.publish(EventPayload::new(
                event_type::SESSION_UPDATED,
                json!({ "sessionID": child.id.to_string(), "info": child.clone() }),
            ));
        }
        return Ok(pending.then_some((child, false)));
    }
    let completed_at = now_millis();
    let message_id = Id::ascending(IdKind::Message);
    let completion = json!({
        "id": message_id.to_string(),
        "generation": generation.to_string(),
        "pending": true,
        "status": status,
        "result": text,
        "completedAt": completed_at,
    });
    let Some(records) = child
        .extra
        .get_mut(SUBTASK_COMPLETIONS_EXTRA_KEY)
        .and_then(Value::as_array_mut)
    else {
        return Err(ApiError::internal(
            "failed to normalize subtask completion metadata",
        ));
    };
    records.push(completion);
    child.extra.insert(
        SUBTASK_PERSISTENCE_VERSION_KEY.to_string(),
        json!(SUBTASK_PERSISTENCE_VERSION),
    );
    clear_owed_generation_if_matching(&mut child, generation);
    child.time.updated = completed_at;
    state.inner.store.update_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": child.id.to_string(), "info": child.clone() }),
    ));
    Ok(Some((child, true)))
}

fn migrate_completion_records(child: &mut SessionInfo) {
    let original = child
        .extra
        .remove(SUBTASK_COMPLETIONS_EXTRA_KEY)
        .unwrap_or_else(|| json!([]));
    let mut candidates = Vec::new();
    collect_recoverable_completion_records(original, &mut candidates);
    let mut normalized = Vec::new();
    let mut ids = BTreeSet::new();
    let mut generations = BTreeSet::new();
    for mut record in candidates {
        let Some(map) = record.as_object_mut() else {
            continue;
        };
        let id = map
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| Id::parse(IdKind::Message, id.to_string()).ok())
            .unwrap_or_else(|| Id::ascending(IdKind::Message));
        map.insert("id".to_string(), json!(id.to_string()));
        let id_key = id.to_string();
        let generation = map
            .get("generation")
            .and_then(Value::as_str)
            .and_then(|value| Id::parse(IdKind::Message, value.to_string()).ok())
            .map(|value| value.to_string());
        if !ids.insert(id_key)
            || generation
                .as_ref()
                .is_some_and(|generation| !generations.insert(generation.clone()))
        {
            continue;
        }
        normalized.push(record);
    }
    child.extra.insert(
        SUBTASK_COMPLETIONS_EXTRA_KEY.to_string(),
        Value::Array(normalized),
    );
}

fn collect_recoverable_completion_records(value: Value, records: &mut Vec<Value>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_recoverable_completion_records(item, records);
            }
        }
        Value::Object(mut map) => {
            if let Some(nested) =
                map.remove("records").or_else(|| map.remove("completions"))
            {
                collect_recoverable_completion_records(nested, records);
            } else if map.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "id" | "generation" | "pending" | "status" | "result" | "completedAt"
                )
            }) {
                records.push(Value::Object(map));
            }
        }
        Value::String(text) => {
            if let Ok(decoded) = serde_json::from_str::<Value>(&text) {
                collect_recoverable_completion_records(decoded, records);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

async fn subtask_keyed_lock(
    locks: &tokio::sync::Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    key: &str,
) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    let mut locks = locks.lock().await;
    locks
        .entry(key.to_string())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn owed_completion_generation(child: &SessionInfo) -> Option<MessageId> {
    child
        .extra
        .get(SUBTASK_NOTIFY_ON_IDLE_KEY)
        .and_then(|value| value.get("generation"))
        .and_then(Value::as_str)
        .and_then(|id| Id::parse(IdKind::Message, id.to_string()).ok())
}

fn clear_owed_generation_if_matching(child: &mut SessionInfo, generation: &MessageId) {
    if owed_completion_generation(child).as_ref() == Some(generation) {
        child.extra.remove(SUBTASK_NOTIFY_ON_IDLE_KEY);
    }
}

async fn enqueue_parent_subtask_completion_prompts_if_ready(
    state: &AppState,
    parent_id: &str,
) -> Result<(), ApiError> {
    let reconcile_lock =
        subtask_keyed_lock(&state.inner.subtask_parent_locks, parent_id).await;
    let _reconcile_guard = reconcile_lock.lock().await;
    if state.inner.store.get_session(parent_id).await?.is_none() {
        return Ok(());
    }
    let pending = pending_parent_subtask_completions(state, parent_id).await?;
    if pending.is_empty() {
        return Ok(());
    }
    // A worker owns the dequeue -> durable-message -> model-run interval. A
    // popped completion is intentionally absent from prompt_queue during that
    // interval, so a concurrent reconciliation must not enqueue a second copy.
    // The worker acknowledges success itself and re-reconciles after releasing
    // ownership, covering completions that arrived while it was active.
    let worker_was_active = state
        .inner
        .session_coordinator
        .worker_active(parent_id)
        .await;
    let parent_messages = state.inner.store.list_messages(parent_id).await?;
    let persisted_user_messages = parent_messages
        .iter()
        .filter_map(|message| match &message.info {
            MessageInfo::User(info) => Some(info.id.to_string()),
            MessageInfo::Assistant(_) => None,
        })
        .collect::<BTreeSet<_>>();
    let successful_replies = successful_parent_reply_ids(&parent_messages);
    let mut start_worker = false;
    let mut queue_len = 0;
    for completion in &pending {
        let request =
            parent_subtask_completions_request(std::slice::from_ref(completion));
        // A persisted runtime user turn is only half a delivery. A crash or
        // admission failure can leave it with no parent assistant response;
        // only a completed, non-error reply proves the parent actually ran.
        if successful_replies.contains(completion.message_id.as_str()) {
            acknowledge_parent_subtask_completion_delivery(state, parent_id, &request)
                .await?;
            continue;
        }
        if worker_was_active
            && persisted_user_messages.contains(completion.message_id.as_str())
        {
            continue;
        }
        let event_request = request.clone();
        let (start, len) = crate::session_queue::enqueue_prompt_request_with_delivery(
            state, parent_id, request, "continue",
        )
        .await?;
        start_worker |= start;
        queue_len = len;
        crate::session_queue::publish_prompt_queue_changed(
            state,
            parent_id,
            "enqueue",
            Some(&event_request),
            Some("continue"),
            0,
        )
        .await;
    }
    crate::session_queue::publish_prompt_queue_status(state, parent_id, queue_len).await;
    if start_worker {
        crate::session_queue::spawn_drain_prompt_queue(
            state.clone(),
            parent_id.to_string(),
        );
    }
    Ok(())
}

fn successful_parent_reply_ids(messages: &[MessageWithParts]) -> BTreeSet<String> {
    let mut awaiting_runtime_notifications = BTreeSet::new();
    let mut delivered = BTreeSet::new();
    for message in messages {
        match &message.info {
            MessageInfo::User(info)
                if info
                    .system
                    .as_deref()
                    .is_some_and(crate::message_model::is_runtime_system_notification) =>
            {
                awaiting_runtime_notifications.insert(info.id.to_string());
            }
            MessageInfo::Assistant(info)
                if info.time.completed.is_some()
                    && info.error.is_none()
                    && info.finish.as_deref() != Some("error") =>
            {
                delivered.insert(info.parent_id.to_string());
                delivered.append(&mut awaiting_runtime_notifications);
            }
            MessageInfo::User(_) | MessageInfo::Assistant(_) => {}
        }
    }
    delivered
}

/// Mark a child so its next true-idle point publishes a subtask completion
/// to the parent. Called when a continue-prompt is queued onto a running
/// child — the only subagent execution path with no completion wrapper.
pub(crate) async fn mark_subtask_notify_on_idle(
    state: &AppState,
    child_id: &str,
    generation: &MessageId,
) -> Result<(), ApiError> {
    let mutation_lock =
        subtask_keyed_lock(&state.inner.subtask_completion_locks, child_id).await;
    let _guard = mutation_lock.lock().await;
    let Some(mut child) = state.inner.store.get_session(child_id).await? else {
        return Ok(());
    };
    if child.parent_id.is_none() {
        return Ok(());
    }
    if let Some(runtime) = state.inner.workspace_runtimes.loaded(&child.directory).await {
        if let Ok(subagents) = runtime.subagents() { subagents.track(child_id.to_string()).await; }
    }
    child.extra.insert(
        SUBTASK_NOTIFY_ON_IDLE_KEY.to_string(),
        json!({ "generation": generation.to_string() }),
    );
    child.extra.insert(
        SUBTASK_PERSISTENCE_VERSION_KEY.to_string(),
        json!(SUBTASK_PERSISTENCE_VERSION),
    );
    child.time.updated = now_millis().max(child.time.updated);
    state.inner.store.update_session(&child).await?;
    Ok(())
}

/// If `session_id` is a child that owes a deferred completion (queued
/// continue-prompt) and it is now truly idle — no run, empty queue, no
/// queue worker — publish the completion through the standard pipeline
/// (outbox + batching + events). The marker is cleared first so
/// concurrent idle hooks can't double-publish.
pub(crate) async fn publish_deferred_subtask_completion_if_idle(
    state: &AppState,
    session_id: &str,
) -> bool {
    let child = match state.inner.store.get_session(session_id).await {
        Ok(Some(child)) => child,
        _ => return false,
    };
    if child.parent_id.is_none() {
        return false;
    }
    let Some(generation) = owed_completion_generation(&child) else {
        return false;
    };
    if subtask_has_active_work(state, session_id).await {
        return false;
    }
    let result = last_assistant_text(state, session_id).await;
    publish_background_subtask_finished(
        state,
        session_id,
        &generation,
        "completed",
        &result,
    )
    .await;
    true
}

async fn latest_child_user_message_id(
    state: &AppState,
    session_id: &str,
) -> Option<MessageId> {
    state
        .inner
        .store
        .list_messages(session_id)
        .await
        .ok()?
        .into_iter()
        .rev()
        .find_map(|message| match message.info {
            MessageInfo::User(info) => Some(info.id),
            MessageInfo::Assistant(_) => None,
        })
}

async fn subtask_has_active_work(state: &AppState, session_id: &str) -> bool {
    if state
        .inner
        .session_coordinator
        .active_run(session_id)
        .await
        .is_some()
    {
        return true;
    }
    if state
        .inner
        .session_coordinator
        .worker_active(session_id)
        .await
    {
        return true;
    }
    state
        .inner
        .store
        .queued_prompt_count(session_id)
        .await
        .unwrap_or(1)
        > 0
}

/// Last assistant text part in the child's transcript — the result the
/// deferred completion carries (mirrors the spawn wrapper's
/// `last_text_part` of the append result).
async fn last_assistant_text(state: &AppState, session_id: &str) -> String {
    let Ok(messages) = state.inner.store.list_messages(session_id).await else {
        return String::new();
    };
    messages
        .iter()
        .rev()
        .find(|message| matches!(message.info, MessageInfo::Assistant(_)))
        .map(|message| {
            message
                .parts
                .iter()
                .filter_map(|part| match part {
                    Part::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Recheck the durable completion outbox for a session acting as PARENT.
/// Fired on every run teardown so held completions get re-attempted each
/// time the main agent goes idle — the safety net that guarantees a
/// pending "subagent finished" notification can never strand just because
/// no further child lifecycle event arrives to re-trigger delivery.
pub(crate) async fn reconcile_pending_subtask_completions_for_parent(
    state: &AppState,
    parent_id: &str,
) {
    if let Err(error) =
        enqueue_parent_subtask_completion_prompts_if_ready(state, parent_id).await
    {
        tracing::warn!(
            parent_id = %parent_id,
            %error,
            "failed to reconcile pending subtask completions for parent"
        );
    }
}

/// Recheck the durable completion outbox after a child lifecycle transition.
/// Every child completion is delivered independently; sibling activity never
/// holds a finished child's result back from the parent.
pub(crate) async fn reconcile_parent_subtask_completions_for_child(
    state: &AppState,
    child_id: &str,
) {
    // A queued continue-prompt owes its completion at the child's next
    // idle point — check before forwarding, so the freshly-published
    // pending entry rides the same delivery attempt below.
    if publish_deferred_subtask_completion_if_idle(state, child_id).await {
        // The standard publish path scans every pending completion for this
        // parent. Calling the scanner again here races its newly-started queue
        // worker and was the source of duplicate in-flight notifications.
        return;
    }
    let parent_id = match state.inner.store.get_session(child_id).await {
        Ok(Some(child)) => child.parent_id.map(|parent| parent.to_string()),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(session_id = %child_id, %error, "failed to load child while reconciling subtask completions");
            return;
        }
    };
    let Some(parent_id) = parent_id else {
        return;
    };
    if let Err(error) =
        enqueue_parent_subtask_completion_prompts_if_ready(state, &parent_id).await
    {
        tracing::warn!(session_id = %child_id, parent_id = %parent_id, %error, "failed to reconcile pending subtask completions");
    }
}

/// Versioned durable-data migration retained because removing it would strand
/// subtask results written into session `extra` by shipped builds. This is the
/// only compatibility parser: runtime paths below consume version 1 records.
async fn migrate_subtask_persistence_v1(state: &AppState) -> Result<(), ApiError> {
    let sessions = state.inner.store.list_sessions().await?;
    for mut child in sessions.into_iter().filter(|session| {
        session.extra.contains_key(SUBTASK_COMPLETION_EXTRA_KEY)
            || session.extra.contains_key(SUBTASK_COMPLETIONS_EXTRA_KEY)
            || session.extra.contains_key(SUBTASK_NOTIFY_ON_IDLE_KEY)
    }) {
        if child
            .extra
            .get(SUBTASK_PERSISTENCE_VERSION_KEY)
            .and_then(Value::as_u64)
            == Some(SUBTASK_PERSISTENCE_VERSION)
        {
            continue;
        }

        let mut records = child
            .extra
            .remove(SUBTASK_COMPLETIONS_EXTRA_KEY)
            .unwrap_or_else(|| json!([]));
        let has_records = records
            .as_array()
            .is_some_and(|records| !records.is_empty());
        if !has_records {
            if let Some(mut completion) = child.extra.remove(SUBTASK_COMPLETION_EXTRA_KEY) {
                if completion.get("pending").and_then(Value::as_bool) == Some(true) {
                    if let Some(map) = completion.as_object_mut() {
                        map.entry("id".to_string()).or_insert_with(|| {
                            json!(format!("msg_subtask_completion_{}", child.id))
                        });
                    }
                    records = Value::Array(vec![completion]);
                }
            }
        } else {
            child.extra.remove(SUBTASK_COMPLETION_EXTRA_KEY);
        }
        child
            .extra
            .insert(SUBTASK_COMPLETIONS_EXTRA_KEY.to_string(), records);
        migrate_completion_records(&mut child);

        // Versions 1 and 2 treated persistence of the runtime user message as
        // proof that the parent model had consumed it. A failed execution
        // admission could therefore leave `pending: false` with no assistant
        // reply. Conversely, a completion steered into an already-active run
        // is consumed by the next assistant step whose parent remains the
        // original user turn. Version 3 derives delivery from transcript order
        // and normalizes both cases without replaying historical completions.
        if let Some(parent_id) = child.parent_id.as_ref() {
            let parent_messages = state.inner.store.list_messages(parent_id.as_str()).await?;
            let successful_replies = successful_parent_reply_ids(&parent_messages);
            if let Some(records) = child
                .extra
                .get_mut(SUBTASK_COMPLETIONS_EXTRA_KEY)
                .and_then(Value::as_array_mut)
            {
                for record in records {
                    let message_id = record.get("id").and_then(Value::as_str);
                    if let Some(message_id) = message_id {
                        if successful_replies.contains(message_id) {
                            record["pending"] = json!(false);
                            record["notifiedAt"] = json!(now_millis());
                        } else if record.get("pending").and_then(Value::as_bool)
                            == Some(false)
                        {
                            record["pending"] = json!(true);
                            if let Some(record) = record.as_object_mut() {
                                record.remove("notifiedAt");
                            }
                        }
                    }
                }
            }
        }

        if child.extra.get(SUBTASK_NOTIFY_ON_IDLE_KEY).and_then(Value::as_bool) == Some(true) {
            let generation = latest_child_user_message_id(state, child.id.as_str())
                .await
                .unwrap_or_else(|| Id::ascending(IdKind::Message));
            child.extra.insert(
                SUBTASK_NOTIFY_ON_IDLE_KEY.to_string(),
                json!({ "generation": generation.to_string() }),
            );
        }
        child.extra.insert(
            SUBTASK_PERSISTENCE_VERSION_KEY.to_string(),
            json!(SUBTASK_PERSISTENCE_VERSION),
        );
        child.time.updated = now_millis().max(child.time.updated);
        state.inner.store.update_session(&child).await?;
    }
    Ok(())
}

/// Recover completion notifications that were persisted but not queued before
/// a shutdown. This makes the completion outbox truly durable: reopening
/// Neoism delivers the result instead of orphaning it.
pub(crate) async fn resume_pending_subtask_completions(state: &AppState) {
    if let Err(error) = migrate_subtask_persistence_v1(state).await {
        tracing::warn!(%error, "failed to migrate durable subtask state");
        return;
    }
    let deferred = match state
        .inner
        .store
        .list_sessions_with_extra_key(SUBTASK_NOTIFY_ON_IDLE_KEY)
        .await
    {
        Ok(sessions) => sessions,
        Err(error) => {
            tracing::warn!(%error, "failed to scan deferred subtask completions at startup");
            Vec::new()
        }
    };
    for child in deferred {
        let Ok(runtime) = state.workspace_runtime(&child.directory).await else {
            clear_subtask_completion_for_teardown(state, child.id.as_str()).await;
            continue;
        };
        if !runtime.snapshot().manifests.iter().any(|plugin| plugin.id == neoism_agent_builtins::plugin::subagents::ID) {
            clear_subtask_completion_for_teardown(state, child.id.as_str()).await;
            continue;
        }
        if let Ok(subagents) = runtime.subagents() { subagents.track(child.id.to_string()).await; }
        publish_deferred_subtask_completion_if_idle(state, child.id.as_str()).await;
    }

    let sessions = match state
        .inner
        .store
        .list_sessions_with_extra_key(SUBTASK_COMPLETIONS_EXTRA_KEY)
        .await
    {
        Ok(sessions) => sessions,
        Err(error) => {
            tracing::warn!(%error, "failed to scan pending subtask completions at startup");
            return;
        }
    };
    let parent_ids = sessions
        .into_iter()
        // Historical children retain delivered completion records for audit
        // and dedupe. Do not reconcile every one of their parents on each
        // launch: each reconciliation scans the child set again. Only a
        // genuinely pending outbox record requires startup delivery.
        .filter(has_pending_subtask_completion)
        .filter_map(|session| session.parent_id.map(|parent| parent.to_string()))
        .collect::<BTreeSet<_>>();
    for parent_id in parent_ids {
        if let Err(error) =
            enqueue_parent_subtask_completion_prompts_if_ready(state, &parent_id).await
        {
            tracing::warn!(parent_id = %parent_id, %error, "failed to resume pending subtask completions");
        }
    }
}

fn has_pending_subtask_completion(session: &SessionInfo) -> bool {
    session
        .extra
        .get(SUBTASK_COMPLETIONS_EXTRA_KEY)
        .and_then(Value::as_array)
        .is_some_and(|completions| {
            completions.iter().any(|completion| {
                completion.get("pending").and_then(Value::as_bool) == Some(true)
            })
        })
}

async fn pending_parent_subtask_completions(
    state: &AppState,
    parent_id: &str,
) -> Result<Vec<PendingSubtaskCompletion>, ApiError> {
    let mut pending = state
        .inner
        .store
        .list_sessions()
        .await?
        .into_iter()
        .filter(|session| {
            session.parent_id.as_ref().map(|id| id.as_str()) == Some(parent_id)
        })
        .flat_map(|child| {
            child
                .extra
                .get(SUBTASK_COMPLETIONS_EXTRA_KEY)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|completion| {
                    completion.get("pending").and_then(Value::as_bool) == Some(true)
                })
                .filter_map(|completion| {
                    let message_id = completion
                        .get("id")
                        .and_then(Value::as_str)
                        .and_then(|id| Id::parse(IdKind::Message, id.to_string()).ok())?;
                    Some(PendingSubtaskCompletion {
                        child: child.clone(),
                        message_id,
                        status: completion
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("completed")
                            .to_string(),
                        text: completion
                            .get("result")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        completed_at: completion
                            .get("completedAt")
                            .and_then(Value::as_u64)
                            .unwrap_or(child.time.updated),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    pending.sort_by_key(|completion| completion.completed_at);
    Ok(pending)
}

pub(crate) async fn acknowledge_parent_subtask_completion_delivery(
    state: &AppState,
    parent_id: &str,
    request: &PromptRequest,
) -> Result<(), ApiError> {
    let Some(message_id) = request.message_id.as_ref() else {
        return Ok(());
    };
    let pending = pending_parent_subtask_completions(state, parent_id).await?;
    let notified_at = now_millis();
    for completion in pending
        .iter()
        .filter(|completion| &completion.message_id == message_id)
    {
        let mutation_lock = subtask_keyed_lock(
            &state.inner.subtask_completion_locks,
            completion.child.id.as_str(),
        )
        .await;
        let _guard = mutation_lock.lock().await;
        // Reload at the mutation boundary. A continued child may finish again
        // while the parent model is consuming the prior notification; using
        // the snapshot captured before delivery would erase that newer record.
        let Some(mut child) = state
            .inner
            .store
            .get_session(completion.child.id.as_str())
            .await?
        else {
            continue;
        };
        let Some(records) = child
            .extra
            .get_mut(SUBTASK_COMPLETIONS_EXTRA_KEY)
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        let Some(record) = records.iter_mut().find(|record| {
            record.get("id").and_then(Value::as_str) == Some(message_id.as_str())
        }) else {
            continue;
        };
        record["pending"] = json!(false);
        record["notifiedAt"] = json!(notified_at);
        state.inner.store.update_session(&child).await?;
        state.publish(EventPayload::new(
            event_type::SESSION_UPDATED,
            json!({ "sessionID": child.id.to_string(), "info": child }),
        ));
    }
    Ok(())
}

fn parent_subtask_completion_prompt(
    child: &SessionInfo,
    status: &str,
    text: &str,
) -> String {
    let agent = child.agent.as_deref().unwrap_or("subagent");
    let tag = if status == "error" {
        "task_error"
    } else {
        "task_result"
    };
    let result = subtask_result_inline(text);
    [
        "Subagent finished.".to_string(),
        format!("task_id: {}", child.id),
        format!("agent: @{agent}"),
        format!("title: {}", child.title),
        format!("status: {status}"),
        String::new(),
        "This background subagent execution is finished; other subagents may still be running."
            .to_string(),
        "The subagent result is included below as runtime system context."
            .to_string(),
        "You may call task_result with this task_id later to reread the retained child session result."
            .to_string(),
        "Continue child session: call task with this same task_id and a new prompt.".to_string(),
        String::new(),
        format!("<{tag}>"),
        result,
        format!("</{tag}>"),
    ]
    .join("\n")
}

fn parent_subtask_completions_prompt(completions: &[PendingSubtaskCompletion]) -> String {
    if let [completion] = completions {
        return parent_subtask_completion_prompt(
            &completion.child,
            &completion.status,
            &completion.text,
        );
    }
    let mut lines = vec![
        "Subagents finished.".to_string(),
        format!("count: {}", completions.len()),
        String::new(),
        "These background subagent executions are finished; other subagents may still be running."
            .to_string(),
        "The subagent results are included below as runtime system context."
            .to_string(),
        "You may call task_result with any task_id later to reread the retained child session result."
            .to_string(),
        "Continue a child session: call task with the same task_id and a new prompt."
            .to_string(),
    ];
    for completion in completions {
        let child = &completion.child;
        let agent = child.agent.as_deref().unwrap_or("subagent");
        let status = completion.status.as_str();
        let tag = if status == "error" {
            "task_error"
        } else {
            "task_result"
        };
        lines.extend([
            String::new(),
            "---".to_string(),
            format!("task_id: {}", child.id),
            format!("agent: @{agent}"),
            format!("title: {}", child.title),
            format!("status: {status}"),
            String::new(),
            format!("<{tag}>"),
            subtask_result_inline(&completion.text),
            format!("</{tag}>"),
        ]);
    }
    lines.join("\n")
}

fn parent_subtask_completions_request(
    completions: &[PendingSubtaskCompletion],
) -> PromptRequest {
    PromptRequest {
        message_id: completions
            .first()
            .map(|completion| completion.message_id.clone()),
        model: None,
        agent: None,
        no_reply: false,
        system: Some(parent_subtask_completion_system()),
        tools: None,
        author: None,
        parts: vec![PromptPart::Text {
            text: parent_subtask_completions_prompt(completions),
        }],
    }
}

fn parent_subtask_completion_system() -> String {
    [
        SUBTASK_COMPLETION_SYSTEM_MARKER.to_string(),
        "This message is generated by the Agent runtime, not by the user. Treat it as session state."
            .to_string(),
        "One or more background subagent executions have finished; other subagents may still be active."
            .to_string(),
        "Each task_id in the paired message is the durable handle for a child session."
            .to_string(),
        "The paired message includes subagent results as system context, not a user request."
            .to_string(),
        "Call task_result with a task_id if you need to reread the retained child-session result in a later turn."
            .to_string(),
        "You may later continue a subagent session by calling task with that task_id and a new prompt."
            .to_string(),
    ]
    .join("\n")
}

fn subtask_result_inline(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "(subagent returned no final text)".to_string();
    }
    if trimmed.chars().count() <= SUBTASK_RESULT_INLINE_CHARS {
        return trimmed.to_string();
    }
    let mut preview = trimmed
        .chars()
        .take(SUBTASK_RESULT_INLINE_CHARS)
        .collect::<String>();
    preview.push_str(
        "\n... result truncated in notification; call task_result for the full output.",
    );
    preview
}

pub(crate) fn last_text_part(message: &MessageWithParts) -> Option<String> {
    if !matches!(message.info, MessageInfo::Assistant(_)) {
        return None;
    }
    message.parts.iter().rev().find_map(|part| match part {
        Part::Text(part) => Some(part.text.clone()),
        _ => None,
    })
}

fn subtask_permission(
    parent: &SessionInfo,
    agent: &neoism_agent_core::AgentInfo,
) -> Vec<PermissionRule> {
    let mut rules = parent
        .permission
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|rule| {
            rule.permission == "external_directory"
                || rule.action == PermissionAction::Deny
        })
        .collect::<Vec<_>>();
    let agent_rules = crate::permission::from_config_map(&agent.permission);
    let can_todo = agent_rules.iter().any(|rule| {
        rule.permission == "todowrite" && rule.action != PermissionAction::Deny
    });
    let can_task = agent_rules
        .iter()
        .any(|rule| rule.permission == "task" && rule.action != PermissionAction::Deny);
    if !can_todo {
        rules.push(PermissionRule {
            permission: "todowrite".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Deny,
        });
    }
    if !can_task {
        rules.push(PermissionRule {
            permission: "task".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Deny,
        });
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{Id, IdKind};
    use std::sync::{atomic::AtomicBool, Arc};
    use std::time::Duration;

    fn test_child_session() -> SessionInfo {
        SessionInfo {
            id: Id::ascending(IdKind::Session),
            slug: "child".to_string(),
            project_id: "global".to_string(),
            workspace_id: None,
            directory: "/tmp".to_string(),
            path: None,
            parent_id: Some(Id::ascending(IdKind::Session)),
            title: "Inspect runtime (@general subagent)".to_string(),
            agent: Some("general".to_string()),
            model: None,
            version: "test".to_string(),
            time: TimeInfo {
                created: 1,
                updated: 1,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: std::collections::BTreeMap::new(),
        }
    }

    fn queued_user_prompt(text: &str) -> PromptRequest {
        PromptRequest {
            message_id: Some(Id::ascending(IdKind::Message)),
            model: None,
            agent: None,
            no_reply: true,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text {
                text: text.to_string(),
            }],
        }
    }

    async fn insert_session(state: &AppState, session: &SessionInfo) {
        state.inner.store.insert_session(session).await.unwrap();
        if session.parent_id.is_some() {
            let workspace = crate::agent_tool_registry::acquire_workspace_plugin_snapshot(
                state,
                &session.directory,
            )
            .await.unwrap();
            workspace
                .runtime
                .subagents()
                .unwrap()
                .track(session.id.to_string())
                .await;
        }
    }

    async fn insert_session_without_subagent_tracker(
        state: &AppState,
        session: &SessionInfo,
    ) {
        state.inner.store.insert_session(session).await.unwrap();
    }

    fn cleanup_test_database(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    async fn hold_parent_run(state: &AppState, parent: &SessionInfo) {
        let run = crate::state::SessionRun {
            id: "parent-active-run".to_string(),
            started_at: 1,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        state
            .inner
            .session_coordinator
            .try_start_run(parent.id.as_str(), run.clone())
            .await
            .unwrap();
        state
            .inner
            .session_coordinator
            .install_run(&parent.id.to_string(), run).await;
    }

    fn completion_pending(child: &SessionInfo, index: usize) -> Option<bool> {
        child
            .extra
            .get(SUBTASK_COMPLETIONS_EXTRA_KEY)
            .and_then(Value::as_array)
            .and_then(|items| items.get(index))
            .and_then(|item| item.get("pending"))
            .and_then(Value::as_bool)
    }

    #[test]
    fn startup_recovery_ignores_delivered_completion_history() {
        let mut child = test_child_session();
        child.extra.insert(
            SUBTASK_COMPLETIONS_EXTRA_KEY.to_string(),
            json!([{ "pending": false }]),
        );
        assert!(!has_pending_subtask_completion(&child));

        child.extra.insert(
            SUBTASK_COMPLETIONS_EXTRA_KEY.to_string(),
            json!([{ "pending": false }, { "pending": true }]),
        );
        assert!(has_pending_subtask_completion(&child));
    }

    async fn owe_completion(state: &AppState, child_id: &str) -> MessageId {
        let generation = Id::ascending(IdKind::Message);
        mark_subtask_notify_on_idle(state, child_id, &generation)
            .await
            .unwrap();
        generation
    }

    #[tokio::test]
    async fn completed_subtask_queues_behind_user_message_while_parent_is_busy() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-busy-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;

        crate::session_queue::enqueue_prompt_request_with_delivery(
            &state,
            parent.id.as_str(),
            queued_user_prompt("new main-agent message"),
            "steer",
        )
        .await
        .unwrap();
        let generation = owe_completion(&state, child.id.as_str()).await;
        publish_background_subtask_finished(
            &state,
            child.id.as_str(),
            &generation,
            "completed",
            "child result",
        )
        .await;

        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(
            queued
                .iter()
                .map(|(_, delivery)| delivery.as_str())
                .collect::<Vec<_>>(),
            vec!["steer", "continue"]
        );
        let stored_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            completion_pending(&stored_child, 0),
            Some(true),
            "queue admission must not acknowledge delivery"
        );
    }

    #[tokio::test]
    async fn completed_subtask_notifies_parent_without_allocated_runtime_tracker() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-no-runtime-tracker-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session_without_subagent_tracker(&state, &parent).await;
        insert_session_without_subagent_tracker(&state, &child).await;
        hold_parent_run(&state, &parent).await;

        let generation = Id::ascending(IdKind::Message);
        let mut stored_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        stored_child.extra.insert(
            SUBTASK_NOTIFY_ON_IDLE_KEY.to_string(),
            json!({ "generation": generation.to_string() }),
        );
        state.inner.store.update_session(&stored_child).await.unwrap();

        assert!(state
            .inner
            .workspace_runtimes
            .loaded(&child.directory)
            .await
            .is_none());
        publish_background_subtask_finished(
            &state,
            child.id.as_str(),
            &generation,
            "completed",
            "embedded server child result",
        )
        .await;

        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(queued.len(), 1, "parent completion must not depend on the runtime tracker");
        assert_eq!(queued[0].1, "continue");
        let stored_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completion_pending(&stored_child, 0), Some(true));
        assert!(stored_child.extra.get(SUBTASK_NOTIFY_ON_IDLE_KEY).is_none());
    }

    #[tokio::test]
    async fn completed_subtask_notifies_while_sibling_is_still_active() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-abort-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut completed_child = test_child_session();
        completed_child.parent_id = Some(parent.id.clone());
        let mut active_sibling = test_child_session();
        active_sibling.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &completed_child).await;
        insert_session(&state, &active_sibling).await;
        hold_parent_run(&state, &parent).await;
        state.inner.session_coordinator.install_run(&
            active_sibling.id.to_string(),
            crate::state::SessionRun {
                id: "sibling-active-run".to_string(),
                started_at: 1,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        ).await;

        let generation = owe_completion(&state, completed_child.id.as_str()).await;
        publish_background_subtask_finished(
            &state,
            completed_child.id.as_str(),
            &generation,
            "completed",
            "retained child result",
        )
        .await;
        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].1, "continue");

        abort_session_run(&state, active_sibling.id.as_str()).await;

        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(
            queued.len(),
            1,
            "sibling teardown must not duplicate the already-queued completion"
        );
        let stored_child = state
            .inner
            .store
            .get_session(completed_child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completion_pending(&stored_child, 0), Some(true));
    }

    #[tokio::test]
    async fn startup_reconciliation_recovers_durable_pending_completion() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-resume-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;
        let generation = owe_completion(&state, child.id.as_str()).await;
        mark_subtask_completion_pending(
            &state,
            child.id.as_str(),
            &generation,
            "completed",
            "saved result",
        )
        .await
        .unwrap();

        resume_pending_subtask_completions(&state).await;
        resume_pending_subtask_completions(&state).await;

        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].1, "continue");
        let (delivered, _) = state
            .inner
            .store
            .pop_queued_prompt_with_delivery(parent.id.as_str(), None)
            .await
            .unwrap()
            .expect("recovered completion");
        acknowledge_parent_subtask_completion_delivery(
            &state,
            parent.id.as_str(),
            &delivered,
        )
        .await
        .unwrap();
        resume_pending_subtask_completions(&state).await;
        assert!(state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap()
            .is_empty());
        let stored_child = state
            .inner
            .store
            .list_sessions()
            .await
            .unwrap()
            .into_iter()
            .find(|session| session.parent_id.as_ref() == Some(&parent.id))
            .expect("stored child");
        assert_eq!(completion_pending(&stored_child, 0), Some(false));
    }

    #[tokio::test]
    async fn same_child_can_deliver_two_sequential_completions() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-repeat-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;

        let first_generation = owe_completion(&state, child.id.as_str()).await;
        publish_background_subtask_finished(
            &state,
            child.id.as_str(),
            &first_generation,
            "completed",
            "first result",
        )
        .await;
        let (first, _) = state
            .inner
            .store
            .pop_queued_prompt_with_delivery(parent.id.as_str(), None)
            .await
            .unwrap()
            .expect("first notification");
        acknowledge_parent_subtask_completion_delivery(
            &state,
            parent.id.as_str(),
            &first,
        )
        .await
        .unwrap();

        let second_generation = owe_completion(&state, child.id.as_str()).await;
        publish_background_subtask_finished(
            &state,
            child.id.as_str(),
            &second_generation,
            "completed",
            "second result",
        )
        .await;
        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(queued.len(), 1);
        let second = &queued[0].0;
        assert_ne!(first.message_id, second.message_id);
        assert!(matches!(
            second.parts.first(),
            Some(PromptPart::Text { text }) if text.contains("second result")
        ));

        let stored_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        let records = stored_child.extra[SUBTASK_COMPLETIONS_EXTRA_KEY]
            .as_array()
            .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(completion_pending(&stored_child, 0), Some(false));
        assert_eq!(completion_pending(&stored_child, 1), Some(true));
    }

    #[tokio::test]
    async fn wrapper_true_idle_and_abort_publishers_create_one_generation() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-generation-race-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;
        let generation = owe_completion(&state, child.id.as_str()).await;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(4));
        let mut publishers = Vec::new();
        for label in ["wrapper", "true-idle", "abort"] {
            let state = state.clone();
            let child_id = child.id.to_string();
            let generation = generation.clone();
            let barrier = barrier.clone();
            publishers.push(tokio::spawn(async move {
                barrier.wait().await;
                publish_background_subtask_finished(
                    &state,
                    &child_id,
                    &generation,
                    "completed",
                    label,
                )
                .await;
            }));
        }
        barrier.wait().await;
        for publisher in publishers {
            publisher.await.unwrap();
        }

        let stored = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.extra[SUBTASK_COMPLETIONS_EXTRA_KEY]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(stored.extra.get(SUBTASK_NOTIFY_ON_IDLE_KEY).is_none());
        assert_eq!(
            state
                .inner
                .store
                .list_queued_prompt_entries(parent.id.as_str())
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn abort_and_delayed_wrapper_race_keeps_one_logical_completion() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-abort-wrapper-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;
        let generation = owe_completion(&state, child.id.as_str()).await;
        let run = crate::session_run::start_session_run(&state, &child.id)
            .await
            .unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
        let aborter = {
            let state = state.clone();
            let child_id = child.id.to_string();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                abort_session_run(&state, &child_id).await;
            })
        };
        let wrapper = {
            let state = state.clone();
            let child_id = child.id.to_string();
            let generation = generation.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                publish_background_subtask_finished(
                    &state,
                    &child_id,
                    &generation,
                    "error",
                    "wrapper observed abort",
                )
                .await;
            })
        };
        barrier.wait().await;
        aborter.await.unwrap();
        wrapper.await.unwrap();
        // Keep the run id live in the test assertion: abort must have removed
        // exactly the run that was racing the wrapper.
        assert!(!state.inner.session_coordinator.active_run(child.id.as_str()).await.is_some());
        assert!(!run.id.is_empty());
        assert_eq!(
            state
                .inner
                .store
                .get_session(child.id.as_str())
                .await
                .unwrap()
                .unwrap()
                .extra[SUBTASK_COMPLETIONS_EXTRA_KEY]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn superseded_generations_only_publish_final_queued_drain() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-superseded-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;
        let first = owe_completion(&state, child.id.as_str()).await;
        let second = owe_completion(&state, child.id.as_str()).await;
        let final_generation = owe_completion(&state, child.id.as_str()).await;

        publish_background_subtask_finished(
            &state,
            child.id.as_str(),
            &first,
            "completed",
            "stale wrapper",
        )
        .await;
        publish_background_subtask_finished(
            &state,
            child.id.as_str(),
            &second,
            "completed",
            "intermediate queue turn",
        )
        .await;
        assert!(
            pending_parent_subtask_completions(&state, parent.id.as_str())
                .await
                .unwrap()
                .is_empty()
        );
        publish_background_subtask_finished(
            &state,
            child.id.as_str(),
            &final_generation,
            "completed",
            "final drain",
        )
        .await;
        let pending = pending_parent_subtask_completions(&state, parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].text, "final drain");
    }

    #[tokio::test]
    async fn several_queued_followups_emit_one_completion_for_final_drain() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-multi-followup-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;
        let mut final_generation = None;
        for text in ["one", "two", "three"] {
            let generation = owe_completion(&state, child.id.as_str()).await;
            state
                .inner
                .store
                .enqueue_prompt_with_delivery(
                    child.id.as_str(),
                    &PromptRequest {
                        message_id: Some(generation.clone()),
                        model: None,
                        agent: None,
                        no_reply: true,
                        system: None,
                        tools: None,
                        author: None,
                        parts: vec![PromptPart::Text {
                            text: text.to_string(),
                        }],
                    },
                    "steer",
                )
                .await
                .unwrap();
            final_generation = Some(generation);
        }
        assert!(
            state
                .inner
                .session_coordinator
                .wake(child.id.as_str())
                .await
        );
        crate::session_queue::drain_prompt_queue(state.clone(), child.id.to_string())
            .await;
        let stored = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        let records = stored.extra[SUBTASK_COMPLETIONS_EXTRA_KEY]
            .as_array()
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].get("generation").and_then(Value::as_str),
            final_generation.as_ref().map(Id::as_str)
        );
        assert_eq!(
            state
                .inner
                .store
                .list_queued_prompt_entries(parent.id.as_str())
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn concurrent_sibling_finish_reconciliation_admits_each_once() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-sibling-race-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut first = test_child_session();
        first.parent_id = Some(parent.id.clone());
        let mut second = test_child_session();
        second.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &first).await;
        insert_session(&state, &second).await;
        hold_parent_run(&state, &parent).await;
        let first_generation = owe_completion(&state, first.id.as_str()).await;
        let second_generation = owe_completion(&state, second.id.as_str()).await;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
        let mut publishers = Vec::new();
        for (child_id, generation) in [
            (first.id.to_string(), first_generation),
            (second.id.to_string(), second_generation),
        ] {
            let state = state.clone();
            let barrier = barrier.clone();
            publishers.push(tokio::spawn(async move {
                barrier.wait().await;
                publish_background_subtask_finished(
                    &state,
                    &child_id,
                    &generation,
                    "completed",
                    "sibling result",
                )
                .await;
            }));
        }
        barrier.wait().await;
        for publisher in publishers {
            publisher.await.unwrap();
        }
        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(queued.len(), 2);
        let ids = queued
            .iter()
            .filter_map(|(request, _)| {
                request.message_id.as_ref().map(ToString::to_string)
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), 2);
    }

    #[tokio::test]
    async fn persisted_subtask_v0_is_migrated_once_without_stranding_results() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-legacy-generation-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        child
            .extra
            .insert(SUBTASK_NOTIFY_ON_IDLE_KEY.to_string(), json!(true));
        child.extra.insert(
            SUBTASK_COMPLETION_EXTRA_KEY.to_string(),
            json!({"pending": true, "status": "completed", "result": "durable result"}),
        );
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        migrate_subtask_persistence_v1(&state).await.unwrap();
        migrate_subtask_persistence_v1(&state).await.unwrap();
        let stored = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.extra[SUBTASK_COMPLETIONS_EXTRA_KEY]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            stored.extra[SUBTASK_COMPLETIONS_EXTRA_KEY][0]["result"],
            "durable result"
        );
        assert_eq!(
            stored.extra[SUBTASK_PERSISTENCE_VERSION_KEY],
            SUBTASK_PERSISTENCE_VERSION
        );
        assert!(stored.extra.get(SUBTASK_COMPLETION_EXTRA_KEY).is_none());
        assert!(owed_completion_generation(&stored).is_some());
    }

    #[tokio::test]
    async fn malformed_completion_metadata_is_repaired_without_duplicate_notification() {
        for kind in ["null", "object", "string"] {
            let db_path = std::env::temp_dir().join(format!(
                "neoism-agent-subtask-malformed-{kind}-{}.sqlite3",
                Id::ascending(IdKind::Event)
            ));
            let state = AppState::open_database(db_path).await.unwrap();
            let mut parent = test_child_session();
            parent.parent_id = None;
            let mut child = test_child_session();
            child.parent_id = Some(parent.id.clone());
            let generation = Id::ascending(IdKind::Message);
            let preserved_id = Id::ascending(IdKind::Message);
            let recoverable = json!({
                "id": preserved_id.to_string(),
                "generation": generation.to_string(),
                "pending": true,
                "status": "completed",
                "result": format!("{kind} result"),
                "completedAt": 7,
            });
            child.extra.insert(
                SUBTASK_COMPLETIONS_EXTRA_KEY.to_string(),
                match kind {
                    "null" => Value::Null,
                    "object" => recoverable.clone(),
                    "string" => Value::String(recoverable.to_string()),
                    _ => unreachable!(),
                },
            );
            insert_session(&state, &parent).await;
            insert_session(&state, &child).await;
            hold_parent_run(&state, &parent).await;
            resume_pending_subtask_completions(&state).await;
            mark_subtask_notify_on_idle(&state, child.id.as_str(), &generation)
                .await
                .unwrap();

            publish_background_subtask_finished(
                &state,
                child.id.as_str(),
                &generation,
                "completed",
                &format!("{kind} result"),
            )
            .await;
            // A second publication of the same logical generation must only
            // reconcile the existing pending record/queue identity.
            publish_background_subtask_finished(
                &state,
                child.id.as_str(),
                &generation,
                "completed",
                "duplicate publication",
            )
            .await;

            let stored = state
                .inner
                .store
                .get_session(child.id.as_str())
                .await
                .unwrap()
                .unwrap();
            let records = stored.extra[SUBTASK_COMPLETIONS_EXTRA_KEY]
                .as_array()
                .expect("test expects repaired array metadata");
            assert_eq!(records.len(), 1, "{kind} metadata duplicated completion");
            if kind != "null" {
                assert_eq!(
                    records[0].get("id").and_then(Value::as_str),
                    Some(preserved_id.as_str()),
                    "{kind} metadata did not preserve recoverable record"
                );
            }
            assert!(stored.extra.get(SUBTASK_NOTIFY_ON_IDLE_KEY).is_none());
            assert_eq!(
                state
                    .inner
                    .store
                    .list_queued_prompt_entries(parent.id.as_str())
                    .await
                    .unwrap()
                    .len(),
                1,
                "{kind} metadata admitted duplicate parent notifications"
            );
        }
    }

    #[tokio::test]
    async fn parent_run_teardown_acknowledges_appended_completion_without_requeue() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-append-boundary-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;

        let generation = owe_completion(&state, child.id.as_str()).await;
        mark_subtask_completion_pending(
            &state,
            child.id.as_str(),
            &generation,
            "completed",
            "append boundary result",
        )
        .await
        .unwrap();
        let completion = pending_parent_subtask_completions(&state, parent.id.as_str())
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("pending completion");
        let completion_message_id = completion.message_id.clone();
        let request =
            parent_subtask_completions_request(std::slice::from_ref(&completion));

        // Admit exactly once without spawning the worker so this test can
        // await the complete pop -> append -> real run teardown -> ack path.
        state
            .inner
            .store
            .enqueue_prompt_with_delivery(parent.id.as_str(), &request, "continue")
            .await
            .unwrap();
        let mut events = state.subscribe();
        crate::session_queue::publish_prompt_queue_changed(
            &state,
            parent.id.as_str(),
            "enqueue",
            Some(&request),
            Some("continue"),
            0,
        )
        .await;
        assert!(
            state
                .inner
                .session_coordinator
                .wake(parent.id.as_str())
                .await
        );
        crate::session_queue::drain_prompt_queue(state.clone(), parent.id.to_string())
            .await;

        let messages = state
            .inner
            .store
            .list_messages(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|message| match &message.info {
                    MessageInfo::User(info) => info.id == completion_message_id,
                    _ => false,
                })
                .count(),
            1,
            "completion runtime message must be appended exactly once"
        );
        assert!(state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap()
            .is_empty());
        let stored_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completion_pending(&stored_child, 0), Some(false));

        let mut queue_actions = Vec::new();
        while let Ok(Ok(event)) =
            tokio::time::timeout(Duration::from_millis(50), events.recv()).await
        {
            if event.kind == event_type::SESSION_QUEUE_UPDATED
                && event.properties.get("messageID").and_then(Value::as_str)
                    == Some(completion_message_id.as_str())
            {
                if let Some(action) =
                    event.properties.get("action").and_then(Value::as_str)
                {
                    queue_actions.push(action.to_string());
                }
            }
        }
        assert_eq!(queue_actions, vec!["enqueue", "dequeue"]);
    }

    #[tokio::test]
    async fn child_idle_reconciliation_admits_parent_completion_once() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-single-admission-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path.clone()).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;
        let _generation = owe_completion(&state, child.id.as_str()).await;
        let mut events = state.subscribe();

        reconcile_parent_subtask_completions_for_child(&state, child.id.as_str()).await;

        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(queued.len(), 1);
        let completion_message_id = queued[0]
            .0
            .message_id
            .as_ref()
            .expect("completion message id")
            .to_string();
        let mut enqueue_events = 0;
        while let Ok(event) = events.try_recv() {
            if event.kind == event_type::SESSION_QUEUE_UPDATED
                && event.properties.get("action").and_then(Value::as_str)
                    == Some("enqueue")
                && event.properties.get("messageID").and_then(Value::as_str)
                    == Some(completion_message_id.as_str())
            {
                enqueue_events += 1;
            }
        }
        assert_eq!(
            enqueue_events, 1,
            "the idle hook and its caller must not both admit the completion"
        );

        state.shutdown().await.unwrap();
        cleanup_test_database(&db_path);
    }

    #[tokio::test]
    async fn persisted_completion_without_parent_reply_resumes_generation() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-partial-delivery-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path.clone()).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;

        let generation = owe_completion(&state, child.id.as_str()).await;
        mark_subtask_completion_pending(
            &state,
            child.id.as_str(),
            &generation,
            "completed",
            "persisted result",
        )
        .await
        .unwrap();
        let completion = pending_parent_subtask_completions(&state, parent.id.as_str())
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("pending completion");
        let completion_message_id = completion.message_id.clone();
        let request =
            parent_subtask_completions_request(std::slice::from_ref(&completion));

        // Reproduce the observed crash boundary: the runtime user message was
        // durable, but execution admission failed before an assistant run.
        append_prompt(&state, parent.id.as_str(), request, false)
            .await
            .unwrap();
        enqueue_parent_subtask_completion_prompts_if_ready(&state, parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(
            state
                .inner
                .store
                .list_queued_prompt_entries(parent.id.as_str())
                .await
                .unwrap()
                .len(),
            1,
            "a user message without a successful assistant reply is still pending"
        );
        assert_eq!(
            completion_pending(
                &state
                    .inner
                    .store
                    .get_session(child.id.as_str())
                    .await
                    .unwrap()
                    .unwrap(),
                0,
            ),
            Some(true)
        );

        assert!(
            state
                .inner
                .session_coordinator
                .finish_run(parent.id.as_str(), "parent-active-run")
                .await
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let child = state
                    .inner
                    .store
                    .get_session(child.id.as_str())
                    .await
                    .unwrap()
                    .unwrap();
                if completion_pending(&child, 0) == Some(false) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("persisted completion should resume and be acknowledged");
        let messages = state
            .inner
            .store
            .list_messages(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|message| matches!(
                    &message.info,
                    MessageInfo::User(info) if info.id == completion_message_id
                ))
                .count(),
            1,
            "resuming must not duplicate the runtime user message"
        );
        assert!(messages.iter().any(|message| matches!(
            &message.info,
            MessageInfo::Assistant(info)
                if info.parent_id == completion_message_id
                    && info.time.completed.is_some()
                    && info.error.is_none()
        )));

        state.shutdown().await.unwrap();
        cleanup_test_database(&db_path);
    }

    #[tokio::test]
    async fn v1_false_ack_without_parent_reply_is_reopened_on_startup() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-v1-false-ack-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path.clone()).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        hold_parent_run(&state, &parent).await;

        let generation = owe_completion(&state, child.id.as_str()).await;
        mark_subtask_completion_pending(
            &state,
            child.id.as_str(),
            &generation,
            "completed",
            "v1 stranded result",
        )
        .await
        .unwrap();
        let completion = pending_parent_subtask_completions(&state, parent.id.as_str())
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("pending completion");
        let request =
            parent_subtask_completions_request(std::slice::from_ref(&completion));
        append_prompt(&state, parent.id.as_str(), request.clone(), false)
            .await
            .unwrap();
        acknowledge_parent_subtask_completion_delivery(
            &state,
            parent.id.as_str(),
            &request,
        )
        .await
        .unwrap();
        let mut stored_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completion_pending(&stored_child, 0), Some(false));
        stored_child.extra.insert(
            SUBTASK_PERSISTENCE_VERSION_KEY.to_string(),
            json!(1),
        );
        state
            .inner
            .store
            .update_session(&stored_child)
            .await
            .unwrap();

        resume_pending_subtask_completions(&state).await;

        let repaired_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completion_pending(&repaired_child, 0), Some(true));
        assert_eq!(
            repaired_child
                .extra
                .get(SUBTASK_PERSISTENCE_VERSION_KEY)
                .and_then(Value::as_u64),
            Some(SUBTASK_PERSISTENCE_VERSION)
        );
        assert_eq!(
            state
                .inner
                .store
                .list_queued_prompt_entries(parent.id.as_str())
                .await
                .unwrap()
                .len(),
            1,
            "startup must requeue the v1 partial delivery"
        );

        state.shutdown().await.unwrap();
        cleanup_test_database(&db_path);
    }

    #[tokio::test]
    async fn legacy_completion_consumed_inside_active_run_is_not_replayed() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-active-run-history-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path.clone()).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;

        let generation = owe_completion(&state, child.id.as_str()).await;
        mark_subtask_completion_pending(
            &state,
            child.id.as_str(),
            &generation,
            "completed",
            "historical active-run result",
        )
        .await
        .unwrap();
        let completion = pending_parent_subtask_completions(&state, parent.id.as_str())
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("pending completion");
        let runtime_message_id = completion.message_id.clone();
        let request =
            parent_subtask_completions_request(std::slice::from_ref(&completion));
        append_prompt(&state, parent.id.as_str(), request.clone(), false)
            .await
            .unwrap();
        acknowledge_parent_subtask_completion_delivery(
            &state,
            parent.id.as_str(),
            &request,
        )
        .await
        .unwrap();

        let ordinary_message_id = Id::ascending(IdKind::Message);
        append_prompt(
            &state,
            parent.id.as_str(),
            PromptRequest {
                message_id: Some(ordinary_message_id.clone()),
                model: None,
                agent: None,
                no_reply: false,
                system: None,
                tools: None,
                author: None,
                parts: vec![PromptPart::Text {
                    text: "continue after the runtime notification".to_string(),
                }],
            },
            true,
        )
        .await
        .unwrap();
        let messages = state
            .inner
            .store
            .list_messages(parent.id.as_str())
            .await
            .unwrap();
        assert!(messages.iter().any(|message| matches!(
            &message.info,
            MessageInfo::Assistant(info)
                if info.parent_id == ordinary_message_id
                    && info.parent_id != runtime_message_id
                    && info.time.completed.is_some()
        )));

        let mut stored_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        stored_child.extra.insert(
            SUBTASK_PERSISTENCE_VERSION_KEY.to_string(),
            json!(2),
        );
        let records = stored_child
            .extra
            .get_mut(SUBTASK_COMPLETIONS_EXTRA_KEY)
            .and_then(Value::as_array_mut)
            .unwrap();
        records[0]["pending"] = json!(true);
        state
            .inner
            .store
            .update_session(&stored_child)
            .await
            .unwrap();

        resume_pending_subtask_completions(&state).await;

        let repaired_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completion_pending(&repaired_child, 0), Some(false));
        assert!(state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap()
            .is_empty());

        state.shutdown().await.unwrap();
        cleanup_test_database(&db_path);
    }

    #[tokio::test]
    async fn startup_reconciliation_finalizes_idle_deferred_child() {
        let db_path = std::env::temp_dir().join(format!(
            "neoism-agent-subtask-deferred-resume-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let state = AppState::open_database(db_path).await.unwrap();
        let mut parent = test_child_session();
        parent.parent_id = None;
        let mut child = test_child_session();
        child.parent_id = Some(parent.id.clone());
        insert_session(&state, &parent).await;
        insert_session(&state, &child).await;
        let generation = Id::ascending(IdKind::Message);
        mark_subtask_notify_on_idle(&state, child.id.as_str(), &generation)
            .await
            .unwrap();

        resume_pending_subtask_completions(&state).await;

        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].1, "continue");
        let stored_child = state
            .inner
            .store
            .get_session(child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert!(stored_child.extra.get(SUBTASK_NOTIFY_ON_IDLE_KEY).is_none());
        assert_eq!(completion_pending(&stored_child, 0), Some(true));
    }

    #[test]
    fn parent_completion_request_is_runtime_system_notification() {
        let child = test_child_session();
        let message_id = Id::ascending(IdKind::Message);
        let request = parent_subtask_completions_request(&[PendingSubtaskCompletion {
            child: child.clone(),
            message_id,
            status: "completed".to_string(),
            text: "final notes".to_string(),
            completed_at: child.time.updated,
        }]);

        assert!(request.message_id.is_some());
        let system = request.system.as_deref().expect("system notification");
        assert!(system.contains(SUBTASK_COMPLETION_SYSTEM_MARKER));
        assert!(system.contains("runtime, not by the user"));
        assert!(system.contains("task_result"));

        let PromptPart::Text { text } = &request.parts[0] else {
            panic!("expected text notification");
        };
        assert!(text.contains("Subagent finished."));
        assert!(text.contains(&format!("task_id: {}", child.id)));
        assert!(text.contains("result is included below"));
        assert!(text.contains("<task_result>"));
        assert!(text.contains("final notes"));
    }

    #[test]
    fn parent_completion_inline_result_is_truncated_at_safety_cap() {
        let child = test_child_session();
        let long_result = "x".repeat(SUBTASK_RESULT_INLINE_CHARS + 32);
        let prompt = parent_subtask_completion_prompt(&child, "completed", &long_result);

        assert!(prompt.contains("result truncated"));
        assert!(!prompt.contains(&long_result));
        assert!(prompt.contains("call task_result for the full output"));
    }

    #[test]
    fn parent_completion_request_can_include_multiple_deferred_subtasks() {
        let first = test_child_session();
        let mut second = test_child_session();
        second.parent_id = first.parent_id.clone();
        second.title = "Inspect styles (@general subagent)".to_string();

        let request = parent_subtask_completions_request(&[
            PendingSubtaskCompletion {
                child: first.clone(),
                message_id: Id::ascending(IdKind::Message),
                status: "completed".to_string(),
                text: "first result".to_string(),
                completed_at: 10,
            },
            PendingSubtaskCompletion {
                child: second.clone(),
                message_id: Id::ascending(IdKind::Message),
                status: "error".to_string(),
                text: "second error".to_string(),
                completed_at: 20,
            },
        ]);

        let PromptPart::Text { text } = &request.parts[0] else {
            panic!("expected text notification");
        };
        assert!(text.contains("Subagents finished."));
        assert!(text.contains("count: 2"));
        assert!(text.contains(&format!("task_id: {}", first.id)));
        assert!(text.contains(&format!("task_id: {}", second.id)));
        assert!(text.contains("<task_result>"));
        assert!(text.contains("<task_error>"));
        assert!(text.contains("first result"));
        assert!(text.contains("second error"));
    }
}

pub(crate) async fn session_command(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<SessionCommandRequest>,
) -> Result<Json<MessageWithParts>, ApiError> {
    let session = ensure_session(&state, &session_id).await?;
    let command = find_command(&state, &session.directory, &request.command).await?;
    let snapshot = state.plugin_snapshot(&session.directory).await;
    let agents = crate::plugins::agent_catalog(&snapshot, &session.directory)?;
    let text = command
        .as_ref()
        .and_then(|command| command.template.as_deref())
        .map(|template| expand_command_template(template, &request.arguments))
        .unwrap_or_else(|| {
            format!("/{} {}", request.command, request.arguments)
                .trim()
                .to_string()
        });
    let model = command
        .as_ref()
        .and_then(|command| command.model.as_deref())
        .and_then(model_ref_from_config)
        .map(|model| user_model_from_model_ref(&model))
        .or(request.model);
    let agent = command
        .as_ref()
        .and_then(|command| command.agent.clone())
        .or_else(|| request.agent.clone());
    let agent_name = agent
        .clone()
        .or_else(|| session.agent.clone())
        .unwrap_or_else(|| agents.default_agent().to_string());
    let agent_info = agents
        .get(&agent_name)
        .ok_or_else(|| ApiError::bad_request(format!("unknown agent {agent_name}")))?;
    let is_subtask = command
        .as_ref()
        .and_then(|command| command.subtask)
        .unwrap_or(agent_info.mode == "subagent");
    if is_subtask {
        let description = command
            .as_ref()
            .and_then(|command| command.description.clone())
            .unwrap_or_else(|| request.command.clone());
        let parent_agent = request
            .agent
            .clone()
            .or_else(|| session.agent.clone())
            .filter(|name| name != &agent_name);
        let response = append_prompt(
            &state,
            &session_id,
            PromptRequest {
                message_id: request.message_id,
                model: None,
                agent: parent_agent,
                no_reply: false,
                system: None,
                tools: None,
                author: None,
                parts: vec![PromptPart::Subtask {
                    prompt: text,
                    description,
                    agent: agent_name,
                    model: model.clone(),
                    command: Some(request.command),
                }],
            },
            true,
        )
        .await?;
        return Ok(Json(response));
    }
    let response = append_prompt(
        &state,
        &session_id,
        PromptRequest {
            message_id: request.message_id,
            model,
            agent,
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text { text }],
        },
        true,
    )
    .await?;
    Ok(Json(response))
}

pub(crate) async fn session_shell(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<SessionShellRequest>,
) -> Result<Json<MessageWithParts>, ApiError> {
    let response = append_prompt(
        &state,
        &session_id,
        PromptRequest {
            message_id: request.message_id,
            model: request.model,
            agent: request.agent,
            no_reply: false,
            system: None,
            tools: None,
            author: None,
            parts: vec![PromptPart::Text {
                text: format!("Run shell command: {}", request.command),
            }],
        },
        true,
    )
    .await?;
    Ok(Json(response))
}
