use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;

use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, MessageId, MessageInfo, MessageWithParts, Part,
    PermissionAction, PermissionRule, PromptPart, PromptRequest, SessionInfo,
    TimeInfo, UserModel,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::AgentCatalog;
use crate::command_routes::{expand_command_template, find_command};
use crate::error::ApiError;
use crate::state::AppState;
use crate::{
    append_prompt, ensure_session, model_ref_from_config, model_ref_from_user_model,
    now_millis, publish_idle_if_no_run, slug, user_model_from_model_ref,
};

const SUBTASK_COMPLETION_SYSTEM_MARKER: &str =
    "Neoism runtime notification: background subagent completion.";
const SUBTASK_RESULT_INLINE_CHARS: usize = 32_000;
const SUBTASK_COMPLETION_EXTRA_KEY: &str = "subtaskCompletion";
/// Set on a child when a continue-prompt is QUEUED onto it (the task tool's
/// child-already-running branch). The queued prompt runs through the generic
/// queue worker — no spawn wrapper exists to publish the completion — so the
/// child's next true-idle point publishes it instead.
const SUBTASK_NOTIFY_ON_IDLE_KEY: &str = "subtaskNotifyOnIdle";

#[derive(Clone, Debug)]
struct PendingSubtaskCompletion {
    child: SessionInfo,
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
    let cancelled = state.inner.runs.write().await.remove(session_id);
    let coordinated = state.inner.session_coordinator.abort_run(session_id).await;
    if let Some(cancelled) = cancelled.as_ref().or(coordinated.as_ref()) {
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

    reconcile_parent_subtask_completions_for_child(state, session_id).await;

    cancelled.is_some() || coordinated.is_some() || was_busy
}

pub(crate) async fn create_subtask_session(
    state: &AppState,
    parent: &SessionInfo,
    command: &str,
    description: &str,
    agent: &str,
    model: Option<UserModel>,
) -> Result<SessionInfo, ApiError> {
    let agents = AgentCatalog::load(&parent.directory)?;
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
        extra: BTreeMap::new(),
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
    prompt: &str,
    agent: String,
    model: Option<UserModel>,
) -> Result<MessageWithParts, ApiError> {
    Box::pin(append_prompt(
        state,
        child_id,
        PromptRequest {
            message_id: None,
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
    prompt: String,
    agent: String,
    model: Option<UserModel>,
) {
    tokio::spawn(async move {
        match append_child_subtask_prompt(&state, &child_id, &prompt, agent, model).await
        {
            Ok(message) => {
                let result = last_text_part(&message).unwrap_or_default();
                publish_background_subtask_finished(
                    &state,
                    &child_id,
                    "completed",
                    &result,
                )
                .await;
            }
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(
                    session_id = %child_id,
                    error = %message,
                    "background subtask failed"
                );
                publish_background_subtask_finished(&state, &child_id, "error", &message)
                    .await;
            }
        }
    });
}

pub(crate) async fn publish_background_subtask_finished(
    state: &AppState,
    child_id: &str,
    status: &str,
    text: &str,
) {
    let Ok(Some(mut child)) = state.inner.store.get_session(child_id).await else {
        return;
    };
    let Some(parent_id) = child.parent_id.as_ref().map(ToString::to_string) else {
        return;
    };
    // A task continuation can already be waiting behind the run whose wrapper
    // called us. Completion is authoritative only after the child has drained
    // that work; otherwise the UI terminal-locks a child that is still active
    // and the parent can receive the first result instead of the final one.
    if subtask_has_active_work(state, child_id).await {
        return;
    }
    // Clear the deferred obligation in the same whole-session write that
    // persists its completion. A separate clear introduced a crash window
    // where neither marker nor completion survived.
    child.extra.remove(SUBTASK_NOTIFY_ON_IDLE_KEY);
    let inline_result = subtask_result_inline(text);
    let child =
        match mark_subtask_completion_pending(state, child, status, &inline_result).await
        {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(
                    session_id = %child_id,
                    parent_id = %parent_id,
                    error = %error,
                    "failed to persist pending subtask completion"
                );
                return;
            }
        };
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
    state.publish(EventPayload::new(
        event_type::SESSION_SUBTASK_COMPLETED,
        payload,
    ));
    if let Err(error) =
        enqueue_parent_subtask_completion_prompts_if_ready(state, &parent_id).await
    {
        tracing::warn!(
            session_id = %child.id,
            parent_id = %parent_id,
            error = %error,
            "failed to notify parent session about completed subtask"
        );
    }
}

async fn mark_subtask_completion_pending(
    state: &AppState,
    mut child: SessionInfo,
    status: &str,
    text: &str,
) -> Result<SessionInfo, ApiError> {
    let completed_at = now_millis();
    child.extra.insert(
        SUBTASK_COMPLETION_EXTRA_KEY.to_string(),
        json!({
            "pending": true,
            "status": status,
            "result": text,
            "completedAt": completed_at,
        }),
    );
    child.time.updated = completed_at;
    state.inner.store.update_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": child.id.to_string(), "info": child.clone() }),
    ));
    Ok(child)
}

async fn enqueue_parent_subtask_completion_prompts_if_ready(
    state: &AppState,
    parent_id: &str,
) -> Result<(), ApiError> {
    if state.inner.store.get_session(parent_id).await?.is_none() {
        return Ok(());
    }
    if parent_has_active_subtasks(state, parent_id).await? {
        return Ok(());
    }
    let pending = pending_parent_subtask_completions(state, parent_id).await?;
    if pending.is_empty() {
        return Ok(());
    }
    let request = parent_subtask_completions_request(&pending);
    let event_request = request.clone();
    let (start_worker, queue_len) =
        crate::session_queue::enqueue_prompt_request_with_delivery(
            state, parent_id, request, "continue",
        )
        .await?;
    mark_parent_subtask_completions_sent(state, &pending).await?;
    crate::session_queue::publish_prompt_queue_changed(
        state,
        parent_id,
        "enqueue",
        Some(&event_request),
        Some("continue"),
        0,
    )
    .await;
    crate::session_queue::publish_prompt_queue_status(state, parent_id, queue_len).await;
    if start_worker {
        crate::session_queue::spawn_drain_prompt_queue(
            state.clone(),
            parent_id.to_string(),
        );
    }
    Ok(())
}

/// Mark a child so its next true-idle point publishes a subtask completion
/// to the parent. Called when a continue-prompt is queued onto a running
/// child — the only subagent execution path with no completion wrapper.
pub(crate) async fn mark_subtask_notify_on_idle(
    state: &AppState,
    child_id: &str,
) -> Result<(), ApiError> {
    let Some(mut child) = state.inner.store.get_session(child_id).await? else {
        return Ok(());
    };
    if child.parent_id.is_none() {
        return Ok(());
    }
    child
        .extra
        .insert(SUBTASK_NOTIFY_ON_IDLE_KEY.to_string(), json!(true));
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
    worker_processed_subtask_prompt: bool,
) {
    let child = match state.inner.store.get_session(session_id).await {
        Ok(Some(child)) => child,
        _ => return,
    };
    let marked = child
        .extra
        .get(SUBTASK_NOTIFY_ON_IDLE_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if child.parent_id.is_none() || (!marked && !worker_processed_subtask_prompt) {
        return;
    }
    if subtask_has_active_work(state, session_id).await {
        return;
    }
    let result = last_assistant_text(state, session_id).await;
    publish_background_subtask_finished(state, session_id, "completed", &result).await;
}

async fn subtask_has_active_work(state: &AppState, session_id: &str) -> bool {
    if state.inner.runs.read().await.contains_key(session_id) {
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
/// A completion can initially be held while a sibling is still active; an
/// abort or late run teardown must give it another chance to reach the parent.
pub(crate) async fn reconcile_parent_subtask_completions_for_child(
    state: &AppState,
    child_id: &str,
) {
    // A queued continue-prompt owes its completion at the child's next
    // idle point — check before forwarding, so the freshly-published
    // pending entry rides the same delivery attempt below.
    publish_deferred_subtask_completion_if_idle(state, child_id, false).await;
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

/// Recover completion notifications that were persisted but not queued before
/// a shutdown (or by an older build). This makes the completion outbox truly
/// durable: reopening Neoism delivers the result instead of orphaning it.
pub(crate) async fn resume_pending_subtask_completions(state: &AppState) {
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
        publish_deferred_subtask_completion_if_idle(state, child.id.as_str(), false)
            .await;
    }

    let sessions = match state
        .inner
        .store
        .list_sessions_with_extra_key(SUBTASK_COMPLETION_EXTRA_KEY)
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
        .filter(|session| {
            session
                .extra
                .get(SUBTASK_COMPLETION_EXTRA_KEY)
                .and_then(|completion| completion.get("pending"))
                .and_then(Value::as_bool)
                == Some(true)
        })
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
        .filter_map(|child| {
            let completion = child.extra.get(SUBTASK_COMPLETION_EXTRA_KEY)?;
            if completion.get("pending").and_then(Value::as_bool) != Some(true) {
                return None;
            }
            Some(PendingSubtaskCompletion {
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
                child,
            })
        })
        .collect::<Vec<_>>();
    pending.sort_by_key(|completion| completion.completed_at);
    Ok(pending)
}

async fn mark_parent_subtask_completions_sent(
    state: &AppState,
    pending: &[PendingSubtaskCompletion],
) -> Result<(), ApiError> {
    let notified_at = now_millis();
    for completion in pending {
        let mut child = completion.child.clone();
        if let Some(Value::Object(map)) =
            child.extra.get_mut(SUBTASK_COMPLETION_EXTRA_KEY)
        {
            map.insert("pending".to_string(), json!(false));
            map.insert("notifiedAt".to_string(), json!(notified_at));
        } else {
            continue;
        }
        state.inner.store.update_session(&child).await?;
        state.publish(EventPayload::new(
            event_type::SESSION_UPDATED,
            json!({ "sessionID": child.id.to_string(), "info": child }),
        ));
    }
    Ok(())
}

async fn parent_has_active_subtasks(
    state: &AppState,
    parent_id: &str,
) -> Result<bool, ApiError> {
    let children = state.inner.store.list_sessions().await?;
    let child_ids = children
        .iter()
        .filter(|session| {
            session.parent_id.as_ref().map(|id| id.as_str()) == Some(parent_id)
        })
        .map(|session| session.id.to_string())
        .collect::<Vec<_>>();
    if child_ids.is_empty() {
        return Ok(false);
    }
    let runs = state.inner.runs.read().await;
    if child_ids.iter().any(|id| runs.contains_key(id)) {
        return Ok(true);
    }
    drop(runs);
    // "Active" means genuinely executing: a live run (above) or a queue
    // worker mid-drain. The derived `statuses` map is deliberately NOT
    // consulted here — a stale Busy entry (e.g. a child with an orphaned
    // queued prompt) would hold every future completion notification
    // hostage with nothing left to clear it.
    for child_id in &child_ids {
        if state
            .inner
            .session_coordinator
            .worker_active(child_id)
            .await
        {
            return Ok(true);
        }
    }
    Ok(false)
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
        "All currently running background subagents for this parent session are finished."
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
        "All currently running background subagents for this parent session are finished."
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
    let completion_key = completions
        .iter()
        .map(|completion| completion.child.id.as_str())
        .collect::<Vec<_>>()
        .join("_");
    PromptRequest {
        message_id: Some(
            Id::parse(
                IdKind::Message,
                format!("msg_subtask_completion_{completion_key}"),
            )
            .expect("runtime completion message id"),
        ),
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
        "This message is generated by the runtime, not by the user. Treat it as session state."
            .to_string(),
        "One or more background subagents have finished, and no other background subagents for this parent session are currently active."
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

fn last_text_part(message: &MessageWithParts) -> Option<String> {
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
            extra: BTreeMap::new(),
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
            .runs
            .write()
            .await
            .insert(parent.id.to_string(), run);
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
        publish_background_subtask_finished(
            &state,
            child.id.as_str(),
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
            stored_child.extra[SUBTASK_COMPLETION_EXTRA_KEY]["pending"].as_bool(),
            Some(false)
        );
    }

    #[tokio::test]
    async fn aborting_last_active_sibling_releases_pending_completion() {
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
        state.inner.runs.write().await.insert(
            active_sibling.id.to_string(),
            crate::state::SessionRun {
                id: "sibling-active-run".to_string(),
                started_at: 1,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );

        publish_background_subtask_finished(
            &state,
            completed_child.id.as_str(),
            "completed",
            "retained child result",
        )
        .await;
        assert!(state
            .inner
            .store
            .list_queued_prompt_entries(parent.id.as_str())
            .await
            .unwrap()
            .is_empty());

        abort_session_run(&state, active_sibling.id.as_str()).await;

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
            .get_session(completed_child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored_child.extra[SUBTASK_COMPLETION_EXTRA_KEY]["pending"].as_bool(),
            Some(false)
        );
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
        mark_subtask_completion_pending(&state, child, "completed", "saved result")
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
            .list_sessions()
            .await
            .unwrap()
            .into_iter()
            .find(|session| session.parent_id.as_ref() == Some(&parent.id))
            .expect("stored child");
        assert_eq!(
            stored_child.extra[SUBTASK_COMPLETION_EXTRA_KEY]["pending"].as_bool(),
            Some(false)
        );
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
        mark_subtask_notify_on_idle(&state, child.id.as_str())
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
        assert_eq!(
            stored_child.extra[SUBTASK_COMPLETION_EXTRA_KEY]["pending"].as_bool(),
            Some(false)
        );
    }

    #[test]
    fn parent_completion_request_is_runtime_system_notification() {
        let child = test_child_session();
        let request = parent_subtask_completions_request(&[PendingSubtaskCompletion {
            child: child.clone(),
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
                status: "completed".to_string(),
                text: "first result".to_string(),
                completed_at: 10,
            },
            PendingSubtaskCompletion {
                child: second.clone(),
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
    let command = find_command(&session.directory, &request.command)?;
    let agents = AgentCatalog::load(&session.directory)?;
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
