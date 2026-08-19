use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use neoism_agent_core::{
    event_type, EventPayload, PromptPart, PromptRequest, SessionStatus, UserModel,
};
use serde::Serialize;
use serde_json::json;

use crate::error::ApiError;
use crate::session_run::{busy_status, publish_idle_if_no_run, session_status_payload};
use crate::state::AppState;
use crate::{append_prompt, ensure_session};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionQueueInfo {
    #[serde(rename = "sessionID")]
    session_id: String,
    count: usize,
    running: bool,
    worker: bool,
    items: Vec<SessionQueueItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionQueueItem {
    index: usize,
    text: Option<String>,
    no_reply: bool,
    agent: Option<String>,
    model: Option<UserModel>,
    part_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionQueueMutation {
    #[serde(rename = "sessionID")]
    session_id: String,
    removed: usize,
    queue: SessionQueueInfo,
}

pub(crate) async fn session_queue(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionQueueInfo>, ApiError> {
    ensure_session(&state, &session_id).await?;
    Ok(Json(session_queue_info(&state, &session_id).await))
}

pub(crate) async fn session_queue_clear(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionQueueMutation>, ApiError> {
    ensure_session(&state, &session_id).await?;
    let removed = clear_queued_prompts(&state, &session_id).await;
    publish_prompt_queue_changed(&state, &session_id, "clear", None, None, removed).await;
    publish_prompt_queue_status(&state, &session_id, 0).await;
    Ok(Json(SessionQueueMutation {
        session_id: session_id.clone(),
        removed,
        queue: session_queue_info(&state, &session_id).await,
    }))
}

pub(crate) async fn session_queue_pop(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionQueueMutation>, ApiError> {
    ensure_session(&state, &session_id).await?;
    let popped = state
        .inner
        .store
        .pop_user_queued_prompt(&session_id)
        .await
        .ok()
        .flatten();
    let removed = usize::from(popped.is_some());
    publish_prompt_queue_changed(
        &state,
        &session_id,
        "pop",
        popped.as_ref(),
        None,
        removed,
    )
    .await;
    let queue_len = queued_prompt_count(&state, &session_id).await;
    publish_prompt_queue_status(&state, &session_id, queue_len).await;
    Ok(Json(SessionQueueMutation {
        session_id: session_id.clone(),
        removed,
        queue: session_queue_info(&state, &session_id).await,
    }))
}

pub(crate) async fn prompt_async(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<PromptRequest>,
) -> Result<StatusCode, ApiError> {
    ensure_session(&state, &session_id).await?;
    let event_request = request.clone();
    let (start_worker, queue_len) =
        enqueue_prompt_request(&state, &session_id, request).await?;
    publish_prompt_queue_changed(
        &state,
        &session_id,
        "enqueue",
        Some(&event_request),
        Some("queue"),
        0,
    )
    .await;
    publish_prompt_queue_status(&state, &session_id, queue_len).await;
    if start_worker {
        tokio::spawn(drain_prompt_queue(state, session_id));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn enqueue_prompt_request(
    state: &AppState,
    session_id: &str,
    request: PromptRequest,
) -> Result<(bool, usize), ApiError> {
    enqueue_prompt_request_with_delivery(state, session_id, request, "queue").await
}

pub(crate) async fn enqueue_prompt_request_with_delivery(
    state: &AppState,
    session_id: &str,
    request: PromptRequest,
    delivery: &str,
) -> Result<(bool, usize), ApiError> {
    if !matches!(delivery, "steer" | "queue" | "continue") {
        return Err(ApiError::bad_request(format!(
            "delivery must be steer, queue, or continue, got {delivery}"
        )));
    }
    if let Some(message_id) = request.message_id.as_ref() {
        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(session_id)
            .await?;
        if let Some((existing, existing_delivery)) = queued
            .iter()
            .find(|(queued, _)| queued.message_id.as_ref() == Some(message_id))
        {
            if existing_delivery != delivery
                || serde_json::to_value(existing).ok()
                    != serde_json::to_value(&request).ok()
            {
                return Err(ApiError::conflict(format!(
                    "message {message_id} is already queued with different prompt content"
                )));
            }
            return Ok((false, queued.len()));
        }
    }
    let queue_len = state
        .enqueue_prompt_with_event(
            session_id,
            &request,
            delivery,
            EventPayload::new(
                event_type::SESSION_PROMPT_ADMITTED,
                json!({
                    "sessionID": session_id,
                    "delivery": delivery,
                    "request": request,
                }),
            ),
        )
        .await?;
    let start_worker = state.inner.session_coordinator.wake(session_id).await;
    Ok((start_worker, queue_len))
}

async fn session_queue_info(state: &AppState, session_id: &str) -> SessionQueueInfo {
    let items = state
        .inner
        .store
        .list_queued_prompt_entries(session_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(request, delivery)| (delivery != "continue").then_some(request))
        .collect::<Vec<_>>();
    let running = state.inner.runs.read().await.contains_key(session_id);
    let worker = state
        .inner
        .session_coordinator
        .worker_active(session_id)
        .await;
    SessionQueueInfo {
        session_id: session_id.to_string(),
        count: items.len(),
        running,
        worker,
        items: items
            .into_iter()
            .enumerate()
            .map(|(index, request)| queued_prompt_item(index, request))
            .collect(),
    }
}

fn queued_prompt_item(index: usize, request: PromptRequest) -> SessionQueueItem {
    let text = queued_prompt_text(&request);
    SessionQueueItem {
        index,
        text,
        no_reply: request.no_reply,
        agent: request.agent,
        model: request.model,
        part_count: request.parts.len(),
    }
}

fn queued_prompt_text(request: &PromptRequest) -> Option<String> {
    request.parts.iter().find_map(|part| match part {
        PromptPart::Text { text } => Some(truncate_queue_preview(text)),
        PromptPart::Agent { name, .. } => Some(format!("@{name}")),
        PromptPart::Subtask {
            description,
            agent,
            prompt,
            ..
        } => {
            let label = if description.trim().is_empty() {
                truncate_queue_preview(prompt)
            } else {
                truncate_queue_preview(description)
            };
            Some(format!("@{agent} {label}"))
        }
        PromptPart::File { filename, .. } => Some(format!("@{filename}")),
    })
}

fn truncate_queue_preview(text: &str) -> String {
    const MAX: usize = 120;
    let text = text.trim().replace('\n', " ");
    let mut preview = text.chars().take(MAX).collect::<String>();
    if text.chars().count() > MAX {
        preview.push_str("...");
    }
    preview
}

/// Clears any queued prompts for a session and publishes the resulting empty
/// queue, returning how many were removed. Used when forcibly stopping a
/// subagent so its pending follow-ups do not start after cancellation.
pub(crate) async fn clear_session_prompt_queue(
    state: &AppState,
    session_id: &str,
) -> usize {
    let removed = state
        .inner
        .store
        .clear_queued_prompts(session_id)
        .await
        .unwrap_or(0);
    publish_prompt_queue_changed(state, session_id, "clear-all", None, None, removed)
        .await;
    publish_prompt_queue_status(state, session_id, 0).await;
    removed
}

async fn clear_queued_prompts(state: &AppState, session_id: &str) -> usize {
    let removed = state
        .inner
        .store
        .clear_user_queued_prompts(session_id)
        .await
        .unwrap_or(0);
    removed
}

pub(crate) async fn queued_prompt_count(state: &AppState, session_id: &str) -> usize {
    state
        .inner
        .store
        .user_queued_prompt_count(session_id)
        .await
        .unwrap_or(0)
}

pub(crate) async fn queued_prompt_preview(
    state: &AppState,
    session_id: &str,
) -> Option<String> {
    state
        .inner
        .store
        .list_queued_prompt_entries(session_id)
        .await
        .ok()
        .and_then(|queue| {
            queue.into_iter().find_map(|(request, delivery)| {
                (delivery != "continue").then_some(request)
            })
        })
        .as_ref()
        .and_then(queued_prompt_text)
}

async fn next_queued_prompt(
    state: &AppState,
    session_id: &str,
) -> Option<(PromptRequest, String, usize)> {
    next_prompt_with_delivery(state, session_id, None).await
}

async fn next_prompt_with_delivery(
    state: &AppState,
    session_id: &str,
    delivery: Option<&str>,
) -> Option<(PromptRequest, String, usize)> {
    let Some((request, delivery)) = state
        .inner
        .store
        .pop_queued_prompt_with_delivery(session_id, delivery)
        .await
        .ok()
        .flatten()
    else {
        return None;
    };
    let remaining = state
        .inner
        .store
        .queued_prompt_count(session_id)
        .await
        .unwrap_or(0);
    Some((request, delivery, remaining))
}

async fn next_active_continuation_prompt(
    state: &AppState,
    session_id: &str,
) -> Option<(PromptRequest, usize)> {
    let request = state
        .inner
        .store
        .pop_active_continuation_prompt(session_id)
        .await
        .ok()
        .flatten()?;
    let remaining = state
        .inner
        .store
        .queued_prompt_count(session_id)
        .await
        .unwrap_or(0);
    Some((request, remaining))
}

pub(crate) async fn publish_prompt_queue_status(
    state: &AppState,
    session_id: &str,
    queue_len: usize,
) {
    let active_worker = state
        .inner
        .session_coordinator
        .worker_active(session_id)
        .await;
    let total_queue_len = state
        .inner
        .store
        .queued_prompt_count(session_id)
        .await
        .unwrap_or(queue_len);
    let visible_queue_len = queued_prompt_count(state, session_id).await;
    let status = if active_worker
        || total_queue_len > 0
        || state.inner.runs.read().await.contains_key(session_id)
    {
        busy_status(
            visible_queue_len,
            queued_prompt_preview(state, session_id).await,
        )
    } else {
        SessionStatus::Idle
    };
    let busy = matches!(status, SessionStatus::Busy { .. });
    if busy {
        state
            .inner
            .statuses
            .write()
            .await
            .insert(session_id.to_string(), status.clone());
    } else {
        state.inner.statuses.write().await.remove(session_id);
    }
    let mut payload = session_status_payload(state, session_id, &status).await;
    payload["queue"] = json!(visible_queue_len);
    state.publish(EventPayload::new(event_type::SESSION_STATUS, payload));
}

pub(crate) async fn publish_prompt_queue_changed(
    state: &AppState,
    session_id: &str,
    action: &str,
    request: Option<&PromptRequest>,
    delivery: Option<&str>,
    removed: usize,
) {
    let mut payload = json!({
        "sessionID": session_id,
        "action": action,
        "removed": removed,
        "queue": session_queue_info(state, session_id).await,
    });
    if let Some(request) = request {
        payload["request"] = json!(request);
        if let Some(message_id) = request.message_id.as_ref() {
            payload["messageID"] = json!(message_id);
        }
    }
    if let Some(delivery) = delivery {
        payload["delivery"] = json!(delivery);
    }
    state.publish(EventPayload::new(
        event_type::SESSION_QUEUE_UPDATED,
        payload,
    ));
}

pub(crate) async fn drain_queued_prompts_into_active_run(
    state: &AppState,
    session_id: &str,
) -> usize {
    let mut drained = 0;
    while let Some((request, remaining)) =
        next_active_continuation_prompt(state, session_id).await
    {
        publish_prompt_queue_changed(
            state,
            session_id,
            "dequeue",
            Some(&request),
            Some("steer"),
            1,
        )
        .await;
        publish_prompt_queue_status(state, session_id, remaining).await;
        drained += 1;
        if let Err(error) = append_prompt(state, session_id, request, false).await {
            state.publish(EventPayload::new(
                event_type::SESSION_ERROR,
                json!({ "sessionID": session_id, "error": { "name": "PromptError", "data": { "message": error.to_string() } } }),
            ));
        }
    }
    drained
}

async fn wait_until_session_not_running(state: &AppState, session_id: &str) {
    // Adopt any run installed by recovery/tests/older internal callers so the
    // coordinator remains compatible with durable run restoration while the
    // map is phased behind the keyed owner API.
    if let Some(run) = state.inner.runs.read().await.get(session_id).cloned() {
        let _ = state
            .inner
            .session_coordinator
            .try_start_run(session_id, run)
            .await;
    }
    state
        .inner
        .session_coordinator
        .wait_until_idle(session_id)
        .await;
}

pub(crate) async fn drain_prompt_queue(state: AppState, session_id: String) {
    loop {
        wait_until_session_not_running(&state, &session_id).await;
        let Some((request, delivery, remaining)) =
            next_queued_prompt(&state, &session_id).await
        else {
            if state
                .inner
                .session_coordinator
                .finish_worker_cycle(&session_id)
                .await
            {
                continue;
            }
            break;
        };
        publish_prompt_queue_changed(
            &state,
            &session_id,
            "dequeue",
            Some(&request),
            None,
            1,
        )
        .await;
        publish_prompt_queue_status(&state, &session_id, remaining).await;
        let create_reply = !request.no_reply;
        if let Err(error) =
            append_prompt(&state, &session_id, request.clone(), create_reply).await
        {
            // A run can claim the session between our idle observation and
            // append_prompt's own runs check (e.g. the user submits a prompt
            // right as the queue drains). The popped prompt — possibly a
            // subagent/background-task completion notification — must NOT be
            // dropped: put it back at the front of the durable queue and wait
            // for the run to finish. Only this exact transient conflict
            // requeues; content conflicts and hard errors surface as before,
            // so a poisoned prompt can never hot-loop the worker.
            let session_running = error.is_conflict()
                && error.to_string() == crate::session_prompt::SESSION_RUNNING_CONFLICT;
            if session_running {
                match state
                    .inner
                    .store
                    .requeue_prompt_front_with_delivery(&session_id, &request, &delivery)
                    .await
                {
                    Ok(queue_len) => {
                        publish_prompt_queue_changed(
                            &state,
                            &session_id,
                            "requeue",
                            Some(&request),
                            Some(&delivery),
                            0,
                        )
                        .await;
                        publish_prompt_queue_status(&state, &session_id, queue_len)
                            .await;
                        continue;
                    }
                    Err(requeue_error) => {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %requeue_error,
                            "failed to requeue prompt after run conflict; surfacing loss"
                        );
                    }
                }
            }
            state.publish(EventPayload::new(
                event_type::SESSION_ERROR,
                json!({ "sessionID": session_id, "error": { "name": "PromptError", "data": { "message": error.to_string() } } }),
            ));
        }
    }
    publish_idle_if_no_run(&state, &session_id).await;
    // The worker exiting with an empty queue is the reliable "truly idle"
    // point for a child that ran a QUEUED continue-prompt (during the run's
    // own teardown this worker still holds ownership, so the deferred
    // completion check no-ops there and must fire here).
    crate::session_actions::publish_deferred_subtask_completion_if_idle(
        &state,
        &session_id,
    )
    .await;
}

pub(crate) fn spawn_drain_prompt_queue(state: AppState, session_id: String) {
    tokio::spawn(drain_prompt_queue(state, session_id));
}

pub(crate) async fn resume_prompt_queues(state: AppState) -> anyhow::Result<()> {
    for session_id in state.inner.store.queued_session_ids().await? {
        if state.inner.session_coordinator.wake(&session_id).await {
            tokio::spawn(drain_prompt_queue(state.clone(), session_id));
        }
    }
    Ok(())
}
