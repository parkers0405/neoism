//! Durable cumulative provider activity for a conversation-family execution.
//!
//! Displayed time uses model-seconds: closed segment durations plus every
//! currently active provider segment. Concurrent parent/child requests are
//! therefore summed, rather than merged into a wall-clock interval union.

use std::sync::Arc;

use neoism_agent_core::{
    event_type, EventPayload, ExecutionActivitySnapshot, Id, IdKind, SessionInfo,
};
use serde_json::json;
use tokio::sync::Mutex;

use crate::state::AppState;

pub(crate) const EXECUTION_ID_KEY: &str = "executionID";
pub(crate) const EXECUTION_ROOT_KEY: &str = "executionRootSessionID";

async fn keyed_lock(state: &AppState, root: &str) -> Arc<Mutex<()>> {
    let mut locks = state.inner.execution_activity_locks.lock().await;
    locks
        .entry(root.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub(crate) async fn admission_guard(
    state: &AppState,
    session_id: &str,
) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    let session = state.inner.store.get_session(session_id).await.ok()??;
    let root = root_session_id(state, &session).await;
    Some(keyed_lock(state, &root).await.lock_owned().await)
}

#[derive(Clone, Debug)]
struct ProviderSegment {
    root_session_id: String,
    execution_id: String,
    segment_id: String,
}

/// Cancellation-safe ownership of one provider/model interval. Explicit
/// completion is awaited on normal paths; dropping an in-flight future queues
/// the same idempotent durable finish operation.
pub(crate) struct ProviderSegmentGuard {
    state: AppState,
    segment: Option<ProviderSegment>,
}

pub(crate) struct SubtaskAdmissionGuard {
    state: AppState,
    child_session_id: String,
    cleanup_status: String,
    armed: bool,
}

impl SubtaskAdmissionGuard {
    pub(crate) async fn admit(
        state: &AppState,
        parent: &SessionInfo,
        child_session_id: &str,
    ) -> anyhow::Result<Self> {
        let armed = register_subtask(state, parent, child_session_id).await?;
        Ok(Self {
            state: state.clone(),
            child_session_id: child_session_id.to_string(),
            cleanup_status: "failed".to_string(),
            armed,
        })
    }

    pub(crate) async fn complete(mut self, status: &str) {
        self.cleanup_status = status.to_string();
        let state = self.state.clone();
        let child = self.child_session_id.clone();
        let status = self.cleanup_status.clone();
        if try_finish_subtask_for_child(&state, &child, &status).await.is_ok() {
            self.armed = false;
        }
    }
}

impl Drop for SubtaskAdmissionGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let state = self.state.clone();
        let child = self.child_session_id.clone();
        let status = self.cleanup_status.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = try_finish_subtask_for_child(&state, &child, &status).await;
            });
        }
    }
}

impl ProviderSegmentGuard {
    pub(crate) async fn finish(mut self) {
        if let Some(segment) = self.segment.clone() {
            if try_finish_provider_segment(&self.state, &segment).await.is_ok() {
                self.segment = None;
            }
        }
    }
}

impl Drop for ProviderSegmentGuard {
    fn drop(&mut self) {
        let Some(segment) = self.segment.take() else {
            return;
        };
        let state = self.state.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { let _ = try_finish_provider_segment(&state, &segment).await; });
        }
    }
}


pub(crate) async fn root_session_id(state: &AppState, session: &SessionInfo) -> String {
    if let Some(root) = session
        .extra
        .get(EXECUTION_ROOT_KEY)
        .and_then(|v| v.as_str())
    {
        return root.to_string();
    }
    let mut current = session.clone();
    while let Some(parent) = current.parent_id.as_ref() {
        let Ok(Some(info)) = state.inner.store.get_session(parent.as_str()).await else {
            break;
        };
        current = info;
    }
    current.id.to_string()
}

pub(crate) async fn ensure_for_prompt(
    state: &AppState,
    info: &mut SessionInfo,
    message_id: &str,
    allow_new: bool,
) -> anyhow::Result<Option<ExecutionActivitySnapshot>> {
    // A prior run can become quiescent between its last cleanup edge and the
    // next durable user turn. Reconcile before admission so a genuinely new
    // top-level prompt never inherits a settled execution's model time. The
    // admitting session's own worker/queue entries belong to THIS prompt (the
    // queue worker calls `append_prompt` inline), so they must be exempt from
    // the quiescence guards or the reconcile can never settle from inside a
    // worker — every new run then inherits the previous execution's timer.
    if allow_new {
        finish_if_quiescent_impl(state, info.id.as_str(), Some(info.id.as_str()))
            .await;
    }
    let root = root_session_id(state, info).await;
    let lock = keyed_lock(state, &root).await;
    let _guard = lock.lock().await;
    let existing = state.inner.store.get_execution_activity(&root).await?;
    let inherited = info
        .extra
        .get(EXECUTION_ID_KEY)
        .and_then(|value| value.as_str());
    let snapshot = match existing {
        Some(snapshot)
            if !snapshot.finished
                && inherited.is_none_or(|execution| execution == snapshot.execution_id) =>
        {
            snapshot
        }
        Some(snapshot)
            if allow_new && snapshot.root_message_id == message_id =>
        {
            snapshot
        }
        _ if allow_new => state
            .inner
            .store
            .admit_execution_activity(
                &root,
                &Id::ascending(IdKind::Event).to_string(),
                message_id,
                "",
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("execution {root} still has active work"))?,
        _ => return Ok(None),
    };
    info.extra
        .insert(EXECUTION_ID_KEY.to_string(), json!(snapshot.execution_id));
    info.extra
        .insert(EXECUTION_ROOT_KEY.to_string(), json!(root));
    if allow_new {
        publish_snapshot(state, &root).await;
    }
    Ok(Some(snapshot))
}

pub(crate) async fn begin_manual_action(
    state: &AppState,
    session_id: &str,
    run_id: &str,
) -> anyhow::Result<()> {
    let Some(mut info) = state.inner.store.get_session(session_id).await? else {
        anyhow::bail!("session {session_id} not found");
    };
    let action_id = Id::ascending(IdKind::Message).to_string();
    let starts_top_level_execution = info.parent_id.is_none();
    let root = root_session_id(state, &info).await;
    let snapshot = if starts_top_level_execution {
        state
            .inner
            .store
            .admit_execution_activity(
                &root,
                &Id::ascending(IdKind::Event).to_string(),
                &action_id,
                run_id,
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("execution {root} still has active work"))?
    } else {
        ensure_for_prompt(state, &mut info, &action_id, false)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no active parent execution"))?
    };
    info.extra.insert(EXECUTION_ID_KEY.to_string(), json!(snapshot.execution_id));
    info.extra.insert(EXECUTION_ROOT_KEY.to_string(), json!(root));
    state.inner.store.update_session(&info).await?;
    Ok(())
}

pub(crate) async fn begin_provider_segment(
    state: &AppState,
    session_id: &str,
) -> Option<ProviderSegmentGuard> {
    let session = state.inner.store.get_session(session_id).await.ok()??;
    let root = root_session_id(state, &session).await;
    let expected = session.extra.get(EXECUTION_ID_KEY)?.as_str()?;
    let lock = keyed_lock(state, &root).await;
    let _guard = lock.lock().await;
    let snapshot = state
        .inner
        .store
        .get_execution_activity(&root)
        .await
        .ok()??;
    if snapshot.finished || snapshot.execution_id != expected {
        return None;
    }
    let segment = Id::ascending(IdKind::Event).to_string();
    let inserted = state
        .inner
        .store
        .insert_execution_segment(
            &root,
            &snapshot.execution_id,
            &segment,
            &state.inner.execution_owner_id,
            session_id,
            crate::now_millis(),
        )
        .await
        .ok()?;
    if !inserted {
        return None;
    }
    publish_snapshot(state, &root).await;
    Some(ProviderSegmentGuard {
        state: state.clone(),
        segment: Some(ProviderSegment {
            root_session_id: root,
            execution_id: snapshot.execution_id,
            segment_id: segment,
        }),
    })
}

async fn try_finish_provider_segment(state: &AppState, segment: &ProviderSegment) -> anyhow::Result<()> {
    let root = segment.root_session_id.clone();
    let lock = keyed_lock(state, &root).await;
    let _guard = lock.lock().await;
    let finished = state
        .inner
        .store
        .finish_execution_segment(
            &root,
            &segment.execution_id,
            &segment.segment_id,
            crate::now_millis(),
        )
        .await
        ?;
    if finished {
        publish_snapshot(state, &root).await;
    }
    drop(_guard);
    // A retry may observe the segment already durably removed after the
    // original future was cancelled. Quiescence is still required.
    finish_if_quiescent(state, &root).await;
    Ok(())
}

pub(crate) async fn end_provider_segment(segment: Option<ProviderSegmentGuard>) {
    if let Some(segment) = segment {
        segment.finish().await;
    }
}

pub(crate) async fn register_subtask(
    state: &AppState,
    parent: &SessionInfo,
    child_session_id: &str,
) -> anyhow::Result<bool> {
    let Some(execution_id) = parent
        .extra
        .get(EXECUTION_ID_KEY)
        .and_then(|value| value.as_str())
    else {
        return Ok(false);
    };
    let root = root_session_id(state, parent).await;
    let lock = keyed_lock(state, &root).await;
    let _guard = lock.lock().await;
    let Ok(Some(mut child)) = state.inner.store.get_session(child_session_id).await else {
        anyhow::bail!("child session {child_session_id} not found");
    };
    child.extra.insert(
        EXECUTION_ID_KEY.to_string(),
        json!(execution_id.to_string()),
    );
    child
        .extra
        .insert(EXECUTION_ROOT_KEY.to_string(), json!(root.clone()));
    state.inner.store.update_session(&child).await?;
    let registration = state
        .inner
        .store
        .register_execution_subtask(
            execution_id,
            &root,
            parent.id.as_str(),
            child_session_id,
            crate::now_millis(),
        )
        .await?;
    if registration == crate::state::ExecutionSubtaskRegistration::Rejected {
        anyhow::bail!("execution {execution_id} is no longer active");
    }
    if registration == crate::state::ExecutionSubtaskRegistration::AlreadyPresent {
        let status = state
            .inner
            .store
            .execution_subtask_status(execution_id, child_session_id)
            .await?;
        if status.as_deref() != Some("outstanding") {
            anyhow::bail!("subtask {child_session_id} is already terminal for execution {execution_id}");
        }
    }
    publish_snapshot(state, &root).await;
    Ok(true)
}

pub(crate) async fn finish_subtask_for_child(
    state: &AppState,
    child_session_id: &str,
    status: &str,
) {
    let _ = try_finish_subtask_for_child(state, child_session_id, status).await;
}

async fn try_finish_subtask_for_child(
    state: &AppState,
    child_session_id: &str,
    status: &str,
) -> anyhow::Result<()> {
    let Ok(Some(child)) = state.inner.store.get_session(child_session_id).await else {
        return Ok(());
    };
    let Some(execution_id) = child
        .extra
        .get(EXECUTION_ID_KEY)
        .and_then(|value| value.as_str())
    else {
        return Ok(());
    };
    let root = root_session_id(state, &child).await;
    let lock = keyed_lock(state, &root).await;
    let _guard = lock.lock().await;
    let terminal = if status == "completed" {
        "completed"
    } else {
        "failed"
    };
    let changed = state
        .inner
        .store
        .finish_execution_subtask(execution_id, child_session_id, terminal)
        .await?;
    if changed {
        publish_snapshot(state, &root).await;
    }
    drop(_guard);
    finish_if_quiescent(state, &root).await;
    Ok(())
}

async fn publish_snapshot(state: &AppState, root: &str) {
    let Ok(runtime) = state.inner.store.get_session_runtime_snapshot(root).await else {
        return;
    };
    let Some(snapshot) = runtime.execution.clone() else {
        return;
    };
    state.publish(EventPayload::new(
        event_type::SESSION_EXECUTION_UPDATED,
        json!({ "sessionID": root, "snapshot": snapshot, "runtime": runtime }),
    ));
}

pub(crate) async fn finish_if_quiescent(state: &AppState, session_id: &str) {
    finish_if_quiescent_impl(state, session_id, None).await;
}

/// `admitting_session`: a session whose worker/queue activity belongs to a NEW
/// prompt being admitted rather than to the execution under reconciliation —
/// its entries do not count against quiescence.
async fn finish_if_quiescent_impl(
    state: &AppState,
    session_id: &str,
    admitting_session: Option<&str>,
) {
    let Ok(Some(session)) = state.inner.store.get_session(session_id).await else {
        return;
    };
    let root_id = root_session_id(state, &session).await;
    let lock = keyed_lock(state, &root_id).await;
    let _guard = lock.lock().await;
    let Ok(Some(_)) = state.inner.store.get_session(&root_id).await else {
        return;
    };
    let Ok(Some(execution)) = state.inner.store.get_execution_activity(&root_id).await else {
        return;
    };
    if execution.finished || !execution.active_segments.is_empty() {
        return;
    }
    let Ok(branches) = state
        .inner
        .store
        .list_execution_subtasks(&execution.execution_id)
        .await
    else {
        return;
    };
    let children = branches
        .iter()
        .map(|branch| branch.session_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut family = children.clone();
    family.insert(root_id.clone());
    let running = state.inner.session_coordinator.active_runs().await;
    if family.iter().any(|session| running.contains_key(session)) {
        return;
    }
    drop(running);
    for session in &family {
        if admitting_session == Some(session.as_str()) {
            continue;
        }
        if state
            .inner
            .session_coordinator
            .worker_active(session)
            .await
            || crate::session_queue::queued_prompt_count(state, session).await > 0
        {
            return;
        }
    }
    if crate::background_job::family_has_running_jobs(state, &children, &root_id).await {
        return;
    }
    // Every live authority is now quiescent. An outstanding durable branch at
    // this point is an orphan from interrupted teardown; treating the row as
    // authority creates a permanent deadlock (and resurrects the child in
    // `/runtime` on every reopen). Terminalize it under the root lock before
    // settling the execution.
    for branch in branches
        .iter()
        .filter(|branch| branch.status == "outstanding")
    {
        match state
            .inner
            .store
            .finish_execution_subtask(&execution.execution_id, &branch.session_id, "failed")
            .await
        {
            Ok(true) => tracing::warn!(
                root_session_id = %root_id,
                child_session_id = %branch.session_id,
                "terminalized orphaned outstanding subtask during quiescence"
            ),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, child_session_id = %branch.session_id, "failed to reconcile orphaned subtask");
                return;
            }
        }
    }
    // The family is quiescent by every live authority, so any store run row
    // still 'running' is a leak from an interrupted teardown. Left alone it
    // vetoes `mark_execution_finished`'s run guard forever — the execution
    // never settles and every later prompt inherits its timer. Run starts
    // serialize on this root's keyed lock (held here), so reconciling now
    // cannot race a genuinely starting run.
    let family_ids = family.iter().cloned().collect::<Vec<_>>();
    match state.inner.store.interrupt_abandoned_runs(&family_ids).await {
        Ok(0) => {}
        Ok(reconciled) => {
            tracing::warn!(
                root_session_id = %root_id,
                reconciled,
                "interrupted abandoned store run rows during quiescence"
            );
        }
        Err(error) => {
            tracing::warn!(%error, root_session_id = %root_id, "failed to reconcile abandoned runs");
        }
    }
    if state
        .inner
        .store
        .mark_execution_finished(&root_id, &execution.execution_id)
        .await
        .unwrap_or(false)
    {
        publish_snapshot(state, &root_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{ExecutionActivitySnapshot, SessionInfo, TimeInfo};
    use std::collections::BTreeMap;

    #[test]
    fn concurrent_segments_use_model_seconds() {
        let snapshot = ExecutionActivitySnapshot {
            completed_ms: 500,
            active_segments: [("a".into(), 1_000), ("b".into(), 1_500)].into(),
            ..Default::default()
        };
        assert_eq!(snapshot.elapsed_ms_at(2_000), 2_000);
    }

    #[tokio::test]
    async fn new_top_level_prompt_reconciles_settled_execution_before_admission() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-next-prompt-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        let now = crate::now_millis();
        let mut session = SessionInfo {
            id: neoism_agent_core::new_session_id(),
            slug: "next-prompt".into(),
            project_id: "global".into(),
            workspace_id: None,
            directory: "/tmp".into(),
            path: None,
            parent_id: None,
            title: "Next prompt".into(),
            agent: None,
            model: None,
            version: env!("CARGO_PKG_VERSION").into(),
            time: TimeInfo {
                created: now,
                updated: now,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: BTreeMap::new(),
        };
        state.inner.store.insert_session(&session).await.unwrap();
        let root = session.id.to_string();
        let previous = state
            .inner
            .store
            .admit_execution_activity(&root, "execution-old", "message-old", "")
            .await
            .unwrap()
            .unwrap();
        assert!(!previous.finished);

        let next = ensure_for_prompt(&state, &mut session, "message-new", true)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(next.execution_id, previous.execution_id);
        assert_eq!(next.root_message_id, "message-new");
        assert_eq!(next.completed_ms, 0);
        assert!(state
            .inner
            .store
            .get_execution_activity(&root)
            .await
            .unwrap()
            .is_some_and(|activity| activity.execution_id == next.execution_id));
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn quiescence_reconciles_leaked_running_run_row_and_settles() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-leaked-run-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        let now = crate::now_millis();
        let mut session = SessionInfo {
            id: neoism_agent_core::new_session_id(),
            slug: "leaked-run".into(),
            project_id: "global".into(),
            workspace_id: None,
            directory: "/tmp".into(),
            path: None,
            parent_id: None,
            title: "Leaked run".into(),
            agent: None,
            model: None,
            version: env!("CARGO_PKG_VERSION").into(),
            time: TimeInfo {
                created: now,
                updated: now,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: BTreeMap::new(),
        };
        state.inner.store.insert_session(&session).await.unwrap();
        let root = session.id.to_string();
        let previous = state
            .inner
            .store
            .admit_execution_activity(&root, "execution-poisoned", "message-old", "")
            .await
            .unwrap()
            .unwrap();
        // The production poison: a store run row left 'running' by an
        // interrupted teardown, with no matching coordinator run. It vetoes
        // mark_execution_finished's guard, so before reconciliation the
        // execution could never settle and every new prompt inherited it.
        state
            .inner
            .store
            .start_run("leaked-run-row", &root)
            .await
            .unwrap();

        finish_if_quiescent(&state, &root).await;
        let settled = state
            .inner
            .store
            .get_execution_activity(&root)
            .await
            .unwrap()
            .unwrap();
        assert!(
            settled.finished,
            "quiescence must interrupt the abandoned run row and settle"
        );

        // A genuinely new prompt now mints a fresh execution — no timer
        // inheritance.
        let next = ensure_for_prompt(&state, &mut session, "message-new", true)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(next.execution_id, previous.execution_id);
        assert_eq!(next.completed_ms, 0);
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn quiescence_terminalizes_orphaned_outstanding_subtask() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-orphan-subtask-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        let now = crate::now_millis();
        let root_id = neoism_agent_core::new_session_id();
        let root = SessionInfo {
            id: root_id.clone(),
            slug: "orphan-root".into(),
            project_id: "global".into(),
            workspace_id: None,
            directory: "/tmp".into(),
            path: None,
            parent_id: None,
            title: "Orphan root".into(),
            agent: None,
            model: None,
            version: env!("CARGO_PKG_VERSION").into(),
            time: TimeInfo {
                created: now,
                updated: now,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: BTreeMap::new(),
        };
        let child_id = neoism_agent_core::new_session_id();
        let mut child = root.clone();
        child.id = child_id.clone();
        child.slug = "orphan-child".into();
        child.parent_id = Some(root_id.clone());
        state.inner.store.insert_session(&root).await.unwrap();
        state.inner.store.insert_session(&child).await.unwrap();
        state
            .inner
            .store
            .admit_execution_activity(root_id.as_str(), "execution-orphan", "message-root", "")
            .await
            .unwrap()
            .unwrap();
        state
            .inner
            .store
            .register_execution_subtask(
                "execution-orphan",
                root_id.as_str(),
                root_id.as_str(),
                child_id.as_str(),
                now,
            )
            .await
            .unwrap();

        state
            .inner
            .store
            .insert_execution_segment(
                root_id.as_str(),
                "execution-orphan",
                "live-segment",
                &state.inner.execution_owner_id,
                child_id.as_str(),
                now,
            )
            .await
            .unwrap();
        finish_if_quiescent(&state, root_id.as_str()).await;
        assert_eq!(
            state
                .inner
                .store
                .execution_subtask_status("execution-orphan", child_id.as_str())
                .await
                .unwrap()
                .as_deref(),
            Some("outstanding"),
            "a live provider segment must protect real child work"
        );
        state
            .inner
            .store
            .finish_execution_segment(
                root_id.as_str(),
                "execution-orphan",
                "live-segment",
                now + 1,
            )
            .await
            .unwrap();

        finish_if_quiescent(&state, root_id.as_str()).await;

        assert_eq!(
            state
                .inner
                .store
                .execution_subtask_status("execution-orphan", child_id.as_str())
                .await
                .unwrap()
                .as_deref(),
            Some("failed")
        );
        assert!(state
            .inner
            .store
            .get_execution_activity(root_id.as_str())
            .await
            .unwrap()
            .is_some_and(|execution| execution.finished));
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn new_top_level_prompt_settles_prior_execution_despite_own_worker() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-own-worker-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        let now = crate::now_millis();
        let mut session = SessionInfo {
            id: neoism_agent_core::new_session_id(),
            slug: "own-worker".into(),
            project_id: "global".into(),
            workspace_id: None,
            directory: "/tmp".into(),
            path: None,
            parent_id: None,
            title: "Own worker".into(),
            agent: None,
            model: None,
            version: env!("CARGO_PKG_VERSION").into(),
            time: TimeInfo {
                created: now,
                updated: now,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: BTreeMap::new(),
        };
        state.inner.store.insert_session(&session).await.unwrap();
        let root = session.id.to_string();
        let previous = state
            .inner
            .store
            .admit_execution_activity(&root, "execution-old", "message-old", "")
            .await
            .unwrap()
            .unwrap();
        assert!(!previous.finished);
        // Steady state after a finished run (or a daemon restart): the session
        // still carries the previous execution id, and the NEW prompt reaches
        // `ensure_for_prompt` from inside the queue worker, whose ownership
        // flag is already set for this very prompt.
        session.extra.insert(
            EXECUTION_ID_KEY.to_string(),
            json!(previous.execution_id.clone()),
        );
        assert!(state.inner.session_coordinator.wake(&root).await);

        let next = ensure_for_prompt(&state, &mut session, "message-new", true)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            next.execution_id, previous.execution_id,
            "a new top-level prompt must not inherit the settled execution's timer"
        );
        assert_eq!(next.root_message_id, "message-new");
        assert_eq!(next.completed_ms, 0);
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn turso_snapshot_survives_reopen_and_terminal_segment_is_deduped() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        let snapshot = ExecutionActivitySnapshot {
            execution_id: "execution-a".into(),
            root_session_id: "root".into(),
            root_message_id: "message-a".into(),
            revision: 1,
            ..Default::default()
        };
        state
            .inner
            .store
            .replace_execution_activity(&snapshot)
            .await
            .unwrap();
        state
            .inner
            .store
            .insert_execution_segment(
                "root",
                "execution-a",
                "parent",
                &state.inner.execution_owner_id,
                "root",
                1_000,
            )
            .await
            .unwrap();
        assert!(!state
            .inner
            .store
            .insert_execution_segment(
                "root",
                "execution-a",
                "parent",
                &state.inner.execution_owner_id,
                "root",
                1_250,
            )
            .await
            .unwrap());
        state
            .inner
            .store
            .insert_execution_segment(
                "root",
                "execution-a",
                "child",
                &state.inner.execution_owner_id,
                "child",
                1_500,
            )
            .await
            .unwrap();
        assert_eq!(
            state
                .inner
                .store
                .get_execution_activity("root")
                .await
                .unwrap()
                .unwrap()
                .elapsed_ms_at(2_000),
            1_500
        );
        assert!(state
            .inner
            .store
            .finish_execution_segment("root", "execution-a", "parent", 2_000)
            .await
            .unwrap());
        assert!(!state
            .inner
            .store
            .finish_execution_segment("root", "execution-a", "parent", 2_500)
            .await
            .unwrap());
        drop(state);

        let reopened = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        let restored = reopened
            .inner
            .store
            .get_execution_activity("root")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.completed_ms, 1_000);
        assert_eq!(restored.active_segments.get("child"), Some(&1_500));
        assert_eq!(
            restored
                .session_activities
                .get("root")
                .map(|activity| activity.completed_ms),
            Some(1_000)
        );
        assert_eq!(
            restored
                .session_activities
                .get("child")
                .map(|activity| activity.elapsed_ms_at(2_000)),
            Some(500)
        );
        reopened
            .inner
            .store
            .register_execution_subtask("execution-a", "root", "root", "child", 2_000)
            .await
            .unwrap();
        assert!(reopened
            .inner
            .store
            .finish_execution_subtask("execution-a", "child", "completed")
            .await
            .unwrap());
        let revision_before_duplicate = reopened
            .inner
            .store
            .get_session_runtime_snapshot("root")
            .await
            .unwrap()
            .family_revision;
        assert_eq!(reopened
            .inner
            .store
            .register_execution_subtask("execution-a", "root", "root", "child", 9_000)
            .await
            .unwrap(), crate::state::ExecutionSubtaskRegistration::AlreadyPresent);
        assert_eq!(
            reopened
                .inner
                .store
                .execution_subtask_status("execution-a", "child")
                .await
                .unwrap()
                .as_deref(),
            Some("completed")
        );
        assert_eq!(
            reopened
                .inner
                .store
                .get_session_runtime_snapshot("root")
                .await
                .unwrap()
                .family_revision,
            revision_before_duplicate
        );
        let runtime = reopened
            .inner
            .store
            .get_session_runtime_snapshot("root")
            .await
            .unwrap();
        assert!(runtime.family_revision >= 3);
        assert_eq!(runtime.branches.len(), 1);
        assert_eq!(runtime.branches[0].status, "completed");
        assert_eq!(
            runtime
                .execution
                .as_ref()
                .and_then(|execution| execution.active_segments.get("child")),
            Some(&1_500)
        );
        reopened.inner.store.replace_execution_activity(&ExecutionActivitySnapshot {
            execution_id: "execution-b".into(),
            root_session_id: "root".into(),
            root_message_id: "message-b".into(),
            revision: runtime.execution.as_ref().unwrap().revision + 1,
            ..Default::default()
        }).await.unwrap();
        assert_eq!(reopened
            .inner
            .store
            .register_execution_subtask("execution-b", "root", "root", "child", 3_000)
            .await
            .unwrap(), crate::state::ExecutionSubtaskRegistration::Inserted);
        assert_eq!(
            reopened
                .inner
                .store
                .list_execution_subtasks("execution-b")
                .await
                .unwrap()[0]
                .status,
            "outstanding"
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn migration_backfills_active_segment_owner_before_exact_once_finish() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-migration-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let database = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let connection = database.connect().unwrap();
        connection
            .execute(
                "CREATE TABLE execution_activity (root_session_id TEXT PRIMARY KEY, execution_id TEXT NOT NULL, root_message_id TEXT NOT NULL, completed_ms INTEGER NOT NULL, revision INTEGER NOT NULL, family_revision INTEGER NOT NULL DEFAULT 0, finished INTEGER NOT NULL DEFAULT 0, updated INTEGER NOT NULL)",
                Vec::<turso::Value>::new(),
            )
            .await
            .unwrap();
        connection
            .execute(
                "CREATE TABLE execution_activity_segments (segment_id TEXT PRIMARY KEY, root_session_id TEXT NOT NULL, execution_id TEXT NOT NULL, owner_instance_id TEXT NOT NULL, session_id TEXT NOT NULL, started_at INTEGER NOT NULL)",
                Vec::<turso::Value>::new(),
            )
            .await
            .unwrap();
        connection
            .execute(
                "INSERT INTO execution_activity VALUES ('root', 'execution', 'message', 400, 1, 0, 0, 1000)",
                Vec::<turso::Value>::new(),
            )
            .await
            .unwrap();
        connection
            .execute(
                "INSERT INTO execution_activity_segments VALUES ('segment', 'root', 'execution', 'old-owner', 'child', 1000)",
                Vec::<turso::Value>::new(),
            )
            .await
            .unwrap();
        drop(connection);
        drop(database);

        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        assert_eq!(
            state
                .inner
                .store
                .get_execution_activity("root")
                .await
                .unwrap()
                .unwrap()
                .session_activities["child"]
                .completed_ms,
            0
        );
        let database = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let connection = database.connect().unwrap();
        connection
            .execute(
                "DELETE FROM execution_session_activity WHERE session_id = 'child'",
                Vec::<turso::Value>::new(),
            )
            .await
            .unwrap();
        drop(connection);
        drop(database);
        assert!(state
            .inner
            .store
            .finish_execution_segment("root", "execution", "segment", 2_000)
            .await
            .unwrap());
        assert!(!state
            .inner
            .store
            .finish_execution_segment("root", "execution", "segment", 3_000)
            .await
            .unwrap());
        let restored = state
            .inner
            .store
            .get_execution_activity("root")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.completed_ms, 1_400);
        assert_eq!(
            restored
                .session_activities
                .get("root")
                .map(|activity| activity.completed_ms),
            Some(400)
        );
        assert_eq!(
            restored
                .session_activities
                .get("child")
                .map(|activity| activity.completed_ms),
            Some(1_000)
        );
        drop(state);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn cancelled_provider_finish_keeps_guard_armed_for_retry() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-guard-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        state
            .inner
            .store
            .replace_execution_activity(&ExecutionActivitySnapshot {
                execution_id: "execution-guard".into(),
                root_session_id: "root-guard".into(),
                root_message_id: "message-guard".into(),
                revision: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        state
            .inner
            .store
            .insert_execution_segment(
                "root-guard",
                "execution-guard",
                "segment-guard",
                &state.inner.execution_owner_id,
                "root-guard",
                crate::now_millis().saturating_sub(100),
            )
            .await
            .unwrap();
        let writer = state.inner.store.lock_writer_for_test().await;
        let finish = tokio::spawn(ProviderSegmentGuard {
            state: state.clone(),
            segment: Some(ProviderSegment {
                root_session_id: "root-guard".into(),
                execution_id: "execution-guard".into(),
                segment_id: "segment-guard".into(),
            }),
        }.finish());
        tokio::task::yield_now().await;
        finish.abort();
        let _ = finish.await;
        drop(writer);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let snapshot = state
                    .inner
                    .store
                    .get_execution_activity("root-guard")
                    .await
                    .unwrap()
                    .unwrap();
                if snapshot.active_segments.is_empty() {
                    assert!(snapshot.completed_ms >= 100);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("drop cleanup should finish promptly");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn provider_admission_and_finalization_have_one_atomic_winner() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-race-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        state
            .inner
            .store
            .replace_execution_activity(&ExecutionActivitySnapshot {
                execution_id: "execution-race".into(),
                root_session_id: "root-race".into(),
                root_message_id: "message-race".into(),
                revision: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        let insert = state.inner.store.insert_execution_segment(
            "root-race",
            "execution-race",
            "segment-race",
            &state.inner.execution_owner_id,
            "root-race",
            crate::now_millis(),
        );
        let finish = state
            .inner
            .store
            .mark_execution_finished("root-race", "execution-race");
        let (inserted, finished) = tokio::join!(insert, finish);
        let inserted = inserted.unwrap();
        let finished = finished.unwrap();
        assert_ne!(inserted, finished);
        let snapshot = state
            .inner
            .store
            .get_execution_activity("root-race")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.finished, finished);
        assert_eq!(snapshot.active_segments.contains_key("segment-race"), inserted);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn stale_owner_reconciliation_stops_time_at_last_heartbeat() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-stale-{}.sqlite3",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        let state = crate::state::AppState::open_database(path.clone())
            .await
            .unwrap();
        state
            .inner
            .store
            .replace_execution_activity(&ExecutionActivitySnapshot {
                execution_id: "execution-stale".into(),
                root_session_id: "root-stale".into(),
                root_message_id: "message-stale".into(),
                revision: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        state
            .inner
            .store
            .heartbeat_execution_owner("dead-owner", 1_000)
            .await
            .unwrap();
        state
            .inner
            .store
            .insert_execution_segment(
                "root-stale",
                "execution-stale",
                "segment-stale",
                "dead-owner",
                "root-stale",
                100,
            )
            .await
            .unwrap();
        assert_eq!(
            state
                .inner
                .store
                .reconcile_stale_execution_segments(2_000)
                .await
                .unwrap(),
            1
        );
        let snapshot = state
            .inner
            .store
            .get_execution_activity("root-stale")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.completed_ms, 900);
        assert!(snapshot.active_segments.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn cross_state_execution_admission_cannot_replace_active_segments() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-cross-state-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let first = crate::state::AppState::open_database(path.clone()).await.unwrap();
        let second = crate::state::AppState::open_database(path.clone()).await.unwrap();
        let admitted = first.inner.store
            .admit_execution_activity("root", "execution-a", "message-a", "")
            .await.unwrap().unwrap();
        assert_eq!(admitted.execution_id, "execution-a");
        first.inner.store.insert_execution_segment(
            "root", "execution-a", "segment", &first.inner.execution_owner_id, "root", 10,
        ).await.unwrap();
        assert!(second.inner.store
            .admit_execution_activity("root", "execution-b", "message-b", "")
            .await.unwrap().is_none());
        assert_eq!(second.inner.store.get_execution_activity("root").await.unwrap().unwrap().execution_id, "execution-a");
        first.inner.store.finish_execution_segment(
            "root", "execution-a", "segment", 20,
        ).await.unwrap();
        first.inner.store.mark_execution_finished("root", "execution-a").await.unwrap();
        let left = first.inner.store.admit_execution_activity(
            "root", "execution-b", "message-b", "",
        );
        let right = second.inner.store.admit_execution_activity(
            "root", "execution-c", "message-c", "",
        );
        let (left, right) = tokio::join!(left, right);
        assert_ne!(left.unwrap().is_some(), right.unwrap().is_some());
        let winner = first.inner.store.get_execution_activity("root").await.unwrap().unwrap();
        assert!(matches!(winner.execution_id.as_str(), "execution-b" | "execution-c"));
        assert_eq!(winner.revision, 5);
        first.shutdown().await.unwrap();
        second.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn execution_lease_task_stops_on_repeated_shutdown() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-shutdown-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        for _ in 0..3 {
            let state = crate::state::AppState::open_database(path.clone()).await.unwrap();
            state.shutdown().await.unwrap();
            assert!(!state.execution_lease_running());
        }
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn manual_execution_admission_owns_its_run_and_blocks_prompt_race() {
        let path = std::env::temp_dir().join(format!(
            "neoism-execution-manual-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let manual = crate::state::AppState::open_database(path.clone()).await.unwrap();
        let competing = crate::state::AppState::open_database(path.clone()).await.unwrap();
        manual.inner.store.start_run("manual-run", "root").await.unwrap();
        assert!(competing.inner.store
            .admit_execution_activity("root", "prompt-execution", "prompt-message", "")
            .await.unwrap().is_none());
        let admitted = manual.inner.store
            .admit_execution_activity("root", "manual-execution", "manual-action", "manual-run")
            .await.unwrap().unwrap();
        assert_eq!(admitted.execution_id, "manual-execution");
        assert!(competing.inner.store
            .admit_execution_activity("root", "prompt-execution", "prompt-message", "")
            .await.unwrap().is_none());
        manual.shutdown().await.unwrap();
        competing.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn cross_state_stale_subtask_registration_is_rejected_after_finish() {
        let path = std::env::temp_dir().join(format!(
            "neoism-subtask-stale-registration-{}.sqlite3",
            Id::ascending(IdKind::Event)
        ));
        let owner = crate::state::AppState::open_database(path.clone()).await.unwrap();
        let stale = crate::state::AppState::open_database(path.clone()).await.unwrap();
        owner.inner.store
            .admit_execution_activity("root", "execution-a", "message-a", "")
            .await.unwrap().unwrap();
        assert!(owner.inner.store.mark_execution_finished("root", "execution-a").await.unwrap());
        assert_eq!(
            stale.inner.store.register_execution_subtask(
                "execution-a", "root", "root", "late-child", 10,
            ).await.unwrap(),
            crate::state::ExecutionSubtaskRegistration::Rejected,
        );
        assert!(stale.inner.store.list_execution_subtasks("execution-a").await.unwrap().is_empty());
        let next = owner.inner.store
            .admit_execution_activity("root", "execution-b", "message-b", "")
            .await.unwrap().unwrap();
        assert_eq!(next.execution_id, "execution-b");
        owner.shutdown().await.unwrap();
        stale.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }
}
