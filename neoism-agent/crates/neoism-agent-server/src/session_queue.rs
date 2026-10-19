use axum::extract::{Path, State};
use axum::Json;
use neoism_agent_core::{
    event_type, EventPayload, PromptPart, PromptRequest, SessionStatus, UserModel,
};
use serde::Serialize;
use serde_json::json;

use crate::ensure_session;
use crate::error::ApiError;
use crate::session_run::{busy_status, publish_idle_if_no_run, session_status_payload};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionQueueInfo {
    session_id: String,
    count: usize,
    running: bool,
    worker: bool,
    compaction_queued: bool,
    preview: Option<String>,
    items: Vec<serde_json::Value>,
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
    session_id: String,
    removed: usize,
    queue: SessionQueueInfo,
}

pub(crate) async fn enqueue_manual_compaction(
    state: &AppState,
    session_id: &str,
) -> Result<(), ApiError> {
    let state = state.clone();
    let session_id = session_id.to_string();
    tokio::spawn(async move { admit_manual_compaction(&state, &session_id).await })
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
}

async fn admit_manual_compaction(
    state: &AppState,
    session_id: &str,
) -> Result<(), ApiError> {
    ensure_session(state, session_id).await?;
    let admission = crate::execution_activity::admission_guard(state, session_id).await;
    let inserted = state.inner.store.enqueue_compact(session_id).await?;
    wake_prompt_queue(state, session_id).await;
    // Keep admission event/snapshot ordering consistent with Stop and claims.
    if inserted {
        state.publish(EventPayload::new(
            event_type::SESSION_COMPACTION_QUEUED,
            json!({"sessionID": session_id}),
        ));
    }
    publish_prompt_queue_changed(state, session_id, "compact", None, None, 0).await;
    drop(admission);
    publish_prompt_queue_status(state, session_id, 0).await;
    Ok(())
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
    // Visible cancellation shares admission with compact claims. Internal
    // clear-all helpers deliberately remain lock-free for already-locked callers.
    // Do not let a visible queue mutation remove a recovery token midway
    // through initialization. Lock order matches append: prompt, then execution.
    let _prompt_admission = state
        .inner
        .session_coordinator
        .prompt_admission_guard(&session_id)
        .await;
    let admission = crate::execution_activity::admission_guard(&state, &session_id).await;
    let removed = clear_queued_prompts(&state, &session_id).await;
    publish_prompt_queue_changed(&state, &session_id, "clear", None, None, removed).await;
    drop(admission);
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
    // Do not let a visible queue mutation remove a recovery token midway
    // through initialization. Lock order matches append: prompt, then execution.
    let _prompt_admission = state
        .inner
        .session_coordinator
        .prompt_admission_guard(&session_id)
        .await;
    let admission = crate::execution_activity::admission_guard(&state, &session_id).await;
    let compact_removed = state.inner.store.take_queued_compact(&session_id).await?;
    let popped = if compact_removed {
        None
    } else {
        state
            .inner
            .store
            .pop_user_queued_prompt(&session_id)
            .await?
    };
    let removed = usize::from(compact_removed || popped.is_some());
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
    drop(admission);
    publish_prompt_queue_status(&state, &session_id, queue_len).await;
    Ok(Json(SessionQueueMutation {
        session_id: session_id.clone(),
        removed,
        queue: session_queue_info(&state, &session_id).await,
    }))
}

/// Commit and schedule independently of the caller's lifetime. The legacy
/// `start_worker` return slot is always false: ownership now includes spawning.
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
    // The detached admission owns commit -> wake -> spawn even if the HTTP
    // caller goes away after the durable commit.
    let state = state.clone();
    let session_id = session_id.to_string();
    let delivery = delivery.to_string();
    tokio::spawn(
        async move { admit_prompt(&state, &session_id, request, &delivery).await },
    )
    .await
    .map_err(|error| ApiError::internal(error.to_string()))?
}

async fn admit_prompt(
    state: &AppState,
    session_id: &str,
    request: PromptRequest,
    delivery: &str,
) -> Result<(bool, usize), ApiError> {
    let _execution_admission =
        crate::execution_activity::admission_guard(state, session_id).await;
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
            wake_prompt_queue(state, session_id).await;
            return Ok((false, queued.len()));
        }
    }
    #[cfg(test)]
    if let Some(started) = state
        .inner
        .session_coordinator
        .prompt_admission_started
        .lock()
        .await
        .remove(session_id)
    {
        let _ = started.send(());
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
    wake_prompt_queue(state, session_id).await;
    let queue_len = state
        .inner
        .store
        .queued_prompt_count(session_id)
        .await
        .unwrap_or(queue_len);
    // Ownership and spawning are internal; legacy callers must not spawn twice.
    Ok((false, queue_len))
}

async fn wake_prompt_queue(state: &AppState, session_id: &str) {
    if state.inner.session_coordinator.wake(session_id).await {
        spawn_drain_prompt_queue(state.clone(), session_id.to_string());
    }
}

pub(crate) async fn queue_status_snapshot(
    state: &AppState,
    session_id: &str,
) -> serde_json::Value {
    let info = session_queue_info(state, session_id).await;
    json!({"count": info.count, "preview": info.preview, "compactionQueued": info.compaction_queued})
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
    let running = state
        .inner
        .session_coordinator
        .active_run(session_id)
        .await
        .is_some();
    let worker = state
        .inner
        .session_coordinator
        .worker_active(session_id)
        .await;
    let compaction_queued = state
        .inner
        .store
        .compaction_queued(session_id)
        .await
        .unwrap_or(false);
    let mut items = items
        .into_iter()
        .enumerate()
        .map(|(index, request)| json!(queued_prompt_item(index, request)))
        .collect::<Vec<_>>();
    if compaction_queued {
        items.insert(
            0,
            json!({"type": "compact", "preview": "Compaction queued"}),
        );
    }
    SessionQueueInfo {
        session_id: session_id.to_string(),
        count: items.len(),
        running,
        worker,
        compaction_queued,
        preview: queued_prompt_preview(state, session_id).await,
        items,
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
    if state
        .inner
        .store
        .compaction_queued(session_id)
        .await
        .unwrap_or(false)
    {
        return Some("Compaction queued".to_string());
    }
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
        || state
            .inner
            .session_coordinator
            .active_run(session_id)
            .await
            .is_some()
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

pub(crate) async fn acknowledge_prompt_admission(
    state: &AppState,
    session_id: &str,
    id: &str,
    request: Option<&PromptRequest>,
    delivery: &str,
) -> Result<(), ApiError> {
    state.inner.store.acknowledge_prompt_admission(id).await?;
    publish_prompt_queue_changed(
        state,
        session_id,
        "dequeue",
        request,
        Some(delivery),
        1,
    )
    .await;
    publish_prompt_queue_status(state, session_id, 0).await;
    Ok(())
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
    loop {
        if let Some(run) = state.inner.session_coordinator.active_run(session_id).await {
            crate::session_context::drain_manual_compaction_for_run(
                state, session_id, &run.id,
            )
            .await;
            if run.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
        }
        let admission = state
            .inner
            .store
            .peek_prompt_admission(session_id, true)
            .await;
        let (id, request, delivery, saved) = match admission {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, session_id, "cannot read active prompt admission");
                break;
            }
        };
        if let Err(error) = crate::session_prompt::append_prompt_admission(
            state,
            session_id,
            request.clone(),
            false,
            Some((&id, saved, &delivery)),
        )
        .await
        {
            state.publish(EventPayload::new(
                event_type::SESSION_ERROR,
                json!({ "sessionID": session_id, "error": { "code": "prompt.failed", "message": error.to_string(), "retryable": true, "details": {} } }),
            ));
            break; // Retain failed admission; never hot-loop it.
        }
        drained += 1;
        publish_prompt_queue_changed(
            state,
            session_id,
            "snapshot",
            Some(&request),
            Some(&delivery),
            1,
        )
        .await;
        publish_prompt_queue_status(state, session_id, 0).await;
    }
    drained
}

async fn wait_until_session_not_running(state: &AppState, session_id: &str) {
    state
        .inner
        .session_coordinator
        .wait_until_idle(session_id)
        .await;
}

pub(crate) async fn drain_prompt_queue(state: AppState, session_id: String) {
    let mut prompt_failed = false;
    loop {
        wait_until_session_not_running(&state, &session_id).await;
        let compact = match state.inner.store.pending_compact_id(&session_id).await {
            Ok(compact) => compact,
            Err(error) => {
                tracing::warn!(%error, session_id, "queue control read failed; retaining durable work");
                prompt_failed = true;
                if state
                    .inner
                    .session_coordinator
                    .park_failed_worker(&session_id)
                    .await
                {
                    continue;
                }
                break;
            }
        };
        if let Some(control_id) = compact {
            match crate::session_context::compact_queued_session_context(
                &state,
                &session_id,
            )
            .await
            {
                Ok(_) => {}
                // Claim-before-consume leaves the control durable on this race.
                Err(error)
                    if error.is_conflict()
                        && error.to_string() == "Session is already running" =>
                {
                    continue;
                }
                Err(error) => {
                    state.publish(EventPayload::new(event_type::SESSION_ERROR, json!({"sessionID": session_id, "error": {"code": "compaction.failed", "message": error.to_string(), "retryable": false, "details": {}}})));
                    prompt_failed = true;
                    // Terminally remove only the failed admission, never a new
                    // compact admitted while its predecessor was executing.
                    if let Err(cleanup_error) =
                        state.inner.store.remove_compact_control(&control_id).await
                    {
                        tracing::warn!(%cleanup_error, session_id, "compact cleanup failed; parking worker until a later wake or restart");
                        if state
                            .inner
                            .session_coordinator
                            .park_failed_worker(&session_id)
                            .await
                        {
                            continue;
                        }
                        break;
                    }
                    publish_prompt_queue_changed(
                        &state,
                        &session_id,
                        "compact-failed",
                        None,
                        None,
                        0,
                    )
                    .await;
                }
            }
            continue;
        }
        let admission = state
            .inner
            .store
            .peek_prompt_admission(&session_id, false)
            .await;
        let entry = match admission {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(%error, session_id, "queue read failed; retaining durable work");
                if state
                    .inner
                    .session_coordinator
                    .park_failed_worker(&session_id)
                    .await
                {
                    continue;
                }
                prompt_failed = true;
                break;
            }
        };
        let Some((id, request, delivery, saved)) = entry else {
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
        let create_reply = !request.no_reply;
        match crate::session_prompt::append_prompt_admission(
            &state,
            &session_id,
            request.clone(),
            create_reply,
            Some((&id, saved, &delivery)),
        )
        .await
        {
            Ok(_) => {
                publish_prompt_queue_changed(
                    &state,
                    &session_id,
                    "snapshot",
                    Some(&request),
                    Some(&delivery),
                    1,
                )
                .await;
                publish_prompt_queue_status(&state, &session_id, 0).await;
                if let Err(error) = crate::session_actions::acknowledge_parent_subtask_completion_delivery(
                    &state,
                    &session_id,
                    &request,
                )
                .await
                {
                    tracing::warn!(session_id = %session_id, %error, "failed to acknowledge delivered subtask completion");
                }
            }
            Err(error) => {
                // The original row remains durable until initialization hands
                // off. Run-claim races can retry without deleting/reinserting.
                let session_running = error.is_conflict()
                    && error.to_string()
                        == crate::session_prompt::SESSION_RUNNING_CONFLICT;
                if session_running {
                    continue;
                }
                state.publish(EventPayload::new(
                event_type::SESSION_ERROR,
                json!({ "sessionID": session_id, "error": { "code": "prompt.failed", "message": error.to_string(), "retryable": false, "details": {} } }),
            ));
                prompt_failed = true;
                // Once handed off, an execution error must not strand unrelated
                // later admissions, nor cause this turn to be replayed.
                if matches!(
                    state.inner.store.prompt_admission_saved(&id).await,
                    Ok(None)
                ) {
                    continue;
                }
                if state
                    .inner
                    .session_coordinator
                    .park_failed_worker(&session_id)
                    .await
                {
                    continue;
                }
                break;
            }
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
    // Same reasoning for execution activity: run teardown happens inside
    // `append_prompt` on this worker's stack, so its quiescence check always
    // sees this worker as active and bails. Settle here — the worker released
    // ownership above — or the execution stays unfinished and the next
    // top-level prompt inherits the previous run's timer.
    crate::execution_activity::finish_if_quiescent(&state, &session_id).await;
    // A completion can arrive while this worker owns the dequeue/in-flight
    // window. Reconcile only after ownership is released, so it is neither
    // duplicated nor stranded. Hard failures stay pending for a later run or
    // restart instead of immediately hot-looping the same poisoned prompt.
    if !prompt_failed {
        crate::session_actions::reconcile_pending_subtask_completions_for_parent(
            &state,
            &session_id,
        )
        .await;
    }
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
