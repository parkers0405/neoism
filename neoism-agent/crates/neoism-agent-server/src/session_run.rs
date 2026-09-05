use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, SessionQueueStatus, SessionStatus,
};
use serde_json::{json, Value};

use crate::session_queue::{queued_prompt_count, queued_prompt_preview};
use crate::state::{AppState, SessionRun};

pub(crate) fn busy_status(queue_count: usize, preview: Option<String>) -> SessionStatus {
    SessionStatus::Busy {
        queue: (queue_count > 0).then_some(SessionQueueStatus {
            count: queue_count,
            preview,
        }),
    }
}

pub(crate) async fn start_session_run(
    state: &AppState,
    session_id: &Id,
) -> Result<SessionRun, SessionRun> {
    let _execution_admission =
        crate::execution_activity::admission_guard(state, session_id.as_str()).await;
    let run = SessionRun {
        id: Id::ascending(IdKind::Event).to_string(),
        started_at: crate::now_millis(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let session_key = session_id.to_string();
    state
        .inner
        .session_coordinator
        .try_start_run(&session_key, run.clone())
        .await?;
    let _ = state.inner.store.start_run(&run.id, &session_key).await;
    let queue_count = queued_prompt_count(state, &session_key).await;
    let status = busy_status(
        queue_count,
        queued_prompt_preview(state, &session_key).await,
    );
    state
        .inner
        .statuses
        .write()
        .await
        .insert(session_key.clone(), status.clone());
    publish_session_status(state, session_id.as_str(), &status).await;
    Ok(run)
}

pub(crate) async fn finish_session_run(state: &AppState, session_id: &str, run_id: &str) {
    if let Err(error) = try_finish_session_run(state, session_id, run_id).await {
        tracing::warn!(%error, %session_id, %run_id, "failed to durably finish session run");
    }
}

pub(crate) async fn try_finish_session_run(
    state: &AppState,
    session_id: &str,
    run_id: &str,
) -> anyhow::Result<()> {
    // Durable state goes first. Cancellation or an I/O error leaves in-memory
    // ownership intact so a cleanup guard can retry the exact same run.
    state
        .inner
        .store
        .finish_run(run_id, "completed", None)
        .await?;
    if !state
        .inner
        .session_coordinator
        .finish_run(session_id, run_id)
        .await
    {
        return Ok(());
    }
    publish_idle_if_no_run(state, session_id).await;
    crate::session_actions::reconcile_parent_subtask_completions_for_child(
        state, session_id,
    )
    .await;
    // This session may also be a PARENT with held child completions —
    // its own turn ending is exactly when a queued "subagent finished"
    // notification can finally go out. Without this, a completion held
    // for any reason at last-child-finish time strands forever.
    crate::session_actions::reconcile_pending_subtask_completions_for_parent(
        state, session_id,
    )
    .await;
    crate::execution_activity::finish_if_quiescent(state, session_id).await;
    Ok(())
}

pub(crate) async fn publish_idle_if_no_run(state: &AppState, session_id: &str) {
    if state
        .inner
        .session_coordinator
        .active_run(session_id)
        .await
        .is_some()
    {
        return;
    }
    let queue_count = queued_prompt_count(state, session_id).await;
    let has_worker = state
        .inner
        .session_coordinator
        .worker_active(session_id)
        .await;
    if has_worker || queue_count > 0 {
        let status =
            busy_status(queue_count, queued_prompt_preview(state, session_id).await);
        state
            .inner
            .statuses
            .write()
            .await
            .insert(session_id.to_string(), status.clone());
        publish_session_status(state, session_id, &status).await;
        return;
    }
    state.inner.statuses.write().await.remove(session_id);
    publish_session_status(state, session_id, &SessionStatus::Idle).await;
}

pub(crate) async fn publish_session_status(
    state: &AppState,
    session_id: &str,
    status: &SessionStatus,
) {
    state.publish(EventPayload::new(
        event_type::SESSION_STATUS,
        session_status_payload(state, session_id, status).await,
    ));
}

pub(crate) async fn session_status_payload(
    state: &AppState,
    session_id: &str,
    status: &SessionStatus,
) -> Value {
    let mut payload = json!({ "sessionID": session_id, "status": status });
    if let Some(run) = state.inner.session_coordinator.active_run(session_id).await {
        payload["runID"] = json!(run.id.clone());
        payload["startedAt"] = json!(run.started_at);
        if let Some(status) = payload.get_mut("status") {
            status["runID"] = json!(run.id);
            status["startedAt"] = json!(run.started_at);
        }
    }
    if let Ok(Some(session)) = state.inner.store.get_session(session_id).await {
        if let Some(parent_id) = session.parent_id.as_ref() {
            payload["parentSessionID"] = json!(parent_id);
            payload["sourceSessionID"] = json!(session.id.to_string());
            payload["sourceTitle"] = json!(session.title);
            if let Some(agent) = session.agent.as_ref() {
                payload["sourceAgent"] = json!(agent);
            }
        }
    }
    payload
}
