use super::*;
use axum::extract::{Path, State};

async fn fixture() -> (AppState, SessionInfo, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "neoism-manual-compact-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let info: SessionInfo = serde_json::from_value(json!({
        "id": neoism_agent_core::new_session_id(), "slug": "manual-control", "projectId": "global",
        "directory": "/tmp", "title": "Manual control", "version": "1",
        "agent": "build", "model": {"providerId": "neoism", "id": "stub"},
        "time": {"created": 1, "updated": 1}
    })).unwrap();
    state.inner.store.insert_session(&info).await.unwrap();
    (state, info, path)
}

fn prompt(text: &str, no_reply: bool) -> PromptRequest {
    serde_json::from_value(json!({"model": {"providerId": "neoism", "modelId": "stub"},
        "noReply": no_reply, "parts": [{"type": "text", "text": text}]}))
    .unwrap()
}

async fn pause_at_compaction_start(
    state: &AppState,
    id: &str,
) -> tokio::sync::oneshot::Sender<()> {
    let (release, gate) = tokio::sync::oneshot::channel();
    state
        .inner
        .session_coordinator
        .compaction_started_gates
        .lock()
        .await
        .insert(id.to_string(), gate);
    release
}

async fn settle(state: &AppState, id: &str) {
    tokio::time::timeout(
        Duration::from_secs(10),
        state.inner.session_coordinator.wait_until_settled(id),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn visible_queue_cancellation_serializes_with_compact_admission() {
    for clear in [true, false] {
        let (state, info, path) = fixture().await;
        let id = info.id.as_str();
        state.inner.store.enqueue_compact(id).await.unwrap();
        state
            .inner
            .store
            .enqueue_prompt_with_delivery(id, &prompt("queued prompt", true), "queue")
            .await
            .unwrap();
        let admission = crate::execution_activity::admission_guard(&state, id).await;
        let operation = async {
            if clear {
                crate::session_queue::session_queue_clear(
                    State(state.clone()),
                    Path(id.to_string()),
                )
                .await
            } else {
                crate::session_queue::session_queue_pop(
                    State(state.clone()),
                    Path(id.to_string()),
                )
                .await
            }
        };
        tokio::pin!(operation);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut operation)
                .await
                .is_err()
        );
        assert_eq!(state.inner.store.queued_prompt_count(id).await.unwrap(), 2);
        drop(admission);
        let result = tokio::time::timeout(Duration::from_secs(2), &mut operation)
            .await
            .unwrap()
            .unwrap();
        let snapshot = serde_json::to_value(result.0).unwrap();
        assert_eq!(snapshot["removed"], if clear { 2 } else { 1 });
        // Pop removes the priority compact control, not the ordinary prompt.
        // Clear removes both. Neither operation leaves a compact queued.
        assert_eq!(snapshot["queue"]["compactionQueued"], false);
        assert_eq!(snapshot["queue"]["count"], usize::from(!clear));
        let prompts = state
            .inner
            .store
            .list_queued_prompt_entries(id)
            .await
            .unwrap();
        assert_eq!(prompts.len(), usize::from(!clear));
        if !clear {
            assert_eq!(
                serde_json::to_value(&prompts[0].0).unwrap(),
                serde_json::to_value(prompt("queued prompt", true)).unwrap()
            );
        }
        if clear {
            // A stale worker observation cannot authorize a summary after Clear.
            let mut events = state.subscribe();
            assert!(
                crate::session_context::compact_queued_session_context(&state, id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(state
                .inner
                .session_coordinator
                .active_run(id)
                .await
                .is_none());
            while let Ok(event) = events.try_recv() {
                assert_ne!(event.kind, event_type::SESSION_COMPACTION_STARTED);
            }
        } else {
            // Trusted/internal clear-all callers may already hold admission.
            let admission = crate::execution_activity::admission_guard(&state, id).await;
            assert_eq!(
                tokio::time::timeout(
                    Duration::from_secs(2),
                    crate::session_queue::clear_session_prompt_queue(&state, id)
                )
                .await
                .unwrap(),
                1
            );
            drop(admission);
        }
        cleanup_sqlite_files(&path);
    }
}

#[tokio::test]
async fn manual_storage_and_provider_failures_always_end_once_without_stealing_run() {
    for in_run in [true, false] {
        for storage_failure in [true, false] {
            let (state, mut info, path) = fixture().await;
            let id = info.id.as_str();
            append_prompt(
                &state,
                id,
                prompt("context for a local summary", true),
                false,
            )
            .await
            .unwrap();
            if storage_failure {
                // Fail the assistant insert AFTER the marker and started event,
                // outside the existing handled provider-failure path.
                state.inner.store.fail_next_assistant_insert_for_test();
            } else {
                info.model.as_mut().unwrap().provider_id =
                    "missing-local-test-provider".into();
                state.inner.store.update_session(&info).await.unwrap();
            }
            let mut events = state.subscribe();
            let run = if in_run {
                let run = crate::session_run::start_session_run(&state, &info.id)
                    .await
                    .unwrap();
                state.inner.store.enqueue_compact(id).await.unwrap();
                assert!(
                    crate::session_context::drain_manual_compaction_for_run(
                        &state, id, &run.id
                    )
                    .await
                );
                let owner = state
                    .inner
                    .session_coordinator
                    .active_run(id)
                    .await
                    .unwrap();
                assert_eq!(owner.id, run.id);
                assert!(std::sync::Arc::ptr_eq(&owner.cancel, &run.cancel));
                Some(run)
            } else {
                crate::session_queue::enqueue_manual_compaction(&state, id)
                    .await
                    .unwrap();
                settle(&state, id).await;
                None
            };
            assert_eq!(
                state
                    .inner
                    .store
                    .get_session(id)
                    .await
                    .unwrap()
                    .unwrap()
                    .time
                    .compacting,
                None
            );
            assert!(!state.inner.store.compaction_queued(id).await.unwrap());
            let mut starts = 0;
            let mut ends = 0;
            while let Ok(event) = events.try_recv() {
                if event.kind == event_type::SESSION_COMPACTION_STARTED {
                    starts += 1;
                }
                if event.kind == event_type::SESSION_COMPACTION_ENDED {
                    assert_eq!(starts, 1);
                    assert_eq!(event.properties["status"], "error");
                    assert_eq!(
                        event
                            .properties
                            .get("runContinues")
                            .and_then(Value::as_bool),
                        in_run.then_some(true)
                    );
                    if storage_failure {
                        assert!(event.properties["error"]["message"]
                            .as_str()
                            .unwrap()
                            .contains("injected assistant insert failure"));
                    }
                    ends += 1;
                }
                if in_run {
                    assert_ne!(event.kind, event_type::SESSION_ERROR);
                    if event.kind == event_type::SESSION_STATUS {
                        assert_ne!(event.properties["status"]["type"], "idle");
                    }
                }
            }
            assert_eq!(
                (starts, ends),
                (1, 1),
                "storage_failure={storage_failure}, in_run={in_run}"
            );
            if let Some(run) = run {
                crate::session_run::finish_session_run(&state, id, &run.id).await;
            }
            cleanup_sqlite_files(&path);
        }
    }
}

#[tokio::test]
async fn pop_removes_pending_compact_before_ordinary_prompts() {
    let (state, info, path) = fixture().await;
    let id = info.id.as_str();
    let run = crate::session_run::start_session_run(&state, &info.id)
        .await
        .unwrap();
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(id, &prompt("keep this queued", true), "queue")
        .await
        .unwrap();
    state.inner.store.enqueue_compact(id).await.unwrap();

    let mutation = crate::session_queue::session_queue_pop(
        State(state.clone()),
        Path(id.to_string()),
    )
    .await
    .unwrap()
    .0;
    let mutation = serde_json::to_value(mutation).unwrap();
    assert_eq!(mutation["removed"], 1);
    assert_eq!(mutation["queue"]["count"], 1);
    assert_eq!(mutation["queue"]["compactionQueued"], false);
    assert!(!state.inner.store.compaction_queued(id).await.unwrap());
    assert_eq!(
        state.inner.store.list_queued_prompt_entries(id).await.unwrap().len(),
        1
    );
    assert_eq!(
        state.inner.session_coordinator.active_run(id).await.unwrap().id,
        run.id
    );

    crate::session_run::finish_session_run(&state, id, &run.id).await;
    drop(state);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn compact_worker_claim_race_leaves_control_durable_and_late_wake_is_not_lost() {
    let (state, info, path) = fixture().await;
    let id = info.id.as_str();
    append_prompt(&state, id, prompt("context", true), false)
        .await
        .unwrap();
    let run = crate::session_run::start_session_run(&state, &info.id)
        .await
        .unwrap();
    state.inner.store.enqueue_compact(id).await.unwrap();
    // Simulate an idle observation followed by another command winning the run.
    let error = crate::session_context::compact_queued_session_context(&state, id)
        .await
        .unwrap_err();
    assert!(error.is_conflict());
    assert!(state.inner.store.compaction_queued(id).await.unwrap());
    let owner = state
        .inner
        .session_coordinator
        .active_run(id)
        .await
        .unwrap();
    assert_eq!(owner.id, run.id);
    assert!(std::sync::Arc::ptr_eq(&owner.cancel, &run.cancel));
    // Admission at the exiting run's final boundary must still reach a worker,
    // even if the active loop has already performed its final queue check.
    crate::session_queue::enqueue_manual_compaction(&state, id)
        .await
        .unwrap();
    crate::session_run::finish_session_run(&state, id, &run.id).await;
    settle(&state, id).await;
    assert!(!state.inner.store.compaction_queued(id).await.unwrap());
    assert_eq!(
        state
            .inner
            .store
            .list_messages(id)
            .await
            .unwrap()
            .iter()
            // Each attempt has a user control AND an assistant summary part.
            .filter(|m| matches!(m.info, MessageInfo::User(_)))
            .flat_map(|m| &m.parts)
            .filter(|p| matches!(p, Part::Compaction(_)))
            .count(),
        1
    );
    let messages = state.inner.store.list_messages(id).await.unwrap();
    let summaries = messages
        .iter()
        .filter_map(|m| match &m.info {
            MessageInfo::Assistant(a)
                if m.parts.iter().any(|p| matches!(p, Part::Compaction(_))) =>
            {
                Some(a)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(summaries.len(), 1, "exactly one summary for the control");
    assert!(messages.iter().any(|m| matches!(&m.info,
        MessageInfo::User(u) if u.id == summaries[0].parent_id
    ) && m.parts.iter().any(|p| matches!(p, Part::Compaction(_)))));
    drop(state);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn compact_busy_endpoints_dedupe_and_stop_preserves_prompts_and_owner() {
    let (state, info, path) = fixture().await;
    let id = info.id.as_str();
    let run = crate::session_run::start_session_run(&state, &info.id)
        .await
        .unwrap();
    let mut events = state.subscribe();
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(id, &prompt("ordinary queue", true), "queue")
        .await
        .unwrap();
    for endpoint in [
        format!("/v2/sessions/{id}/compact"),
        format!("/v2/sessions/{id}/summarize"),
    ] {
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            app(state.clone()).oneshot(request(Method::POST, &endpoint, Some(json!({})))),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            response.status().is_success(),
            "{endpoint}: {}",
            response.status()
        );
    }
    assert_eq!(state.inner.store.queued_prompt_count(id).await.unwrap(), 2);
    let snapshot =
        crate::session_queue::session_queue(State(state.clone()), Path(id.to_string()))
            .await
            .unwrap()
            .0;
    let snapshot = serde_json::to_value(snapshot).unwrap();
    assert_eq!(snapshot["compactionQueued"], true);
    assert_eq!(snapshot["preview"], "Compaction queued");
    assert_eq!(snapshot["items"][0]["type"], "compact");
    let mut admitted = 0;
    while let Ok(event) = events.try_recv() {
        if event.kind == event_type::SESSION_COMPACTION_QUEUED {
            admitted += 1;
        }
    }
    assert_eq!(admitted, 1);
    crate::session_actions::abort_session_run(&state, id).await;
    assert!(run.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!state.inner.store.compaction_queued(id).await.unwrap());
    assert_ne!(
        serde_json::to_value(state.inner.statuses.read().await.get(id)).unwrap()["queue"]
            ["preview"],
        "Compaction queued"
    );
    assert_eq!(
        state
            .inner
            .store
            .list_queued_prompt_entries(id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        state
            .inner
            .session_coordinator
            .active_run(id)
            .await
            .unwrap()
            .id,
        run.id
    );
    crate::session_run::finish_session_run(&state, id, &run.id).await;
    settle(&state, id).await;
    drop(state);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn manual_boundary_has_priority_over_steers_and_keeps_owning_cancel_token() {
    let (state, info, path) = fixture().await;
    let id = info.id.as_str();
    append_prompt(&state, id, prompt("remember this", true), false)
        .await
        .unwrap();
    let run = crate::session_run::start_session_run(&state, &info.id)
        .await
        .unwrap();
    state
        .inner
        .store
        .enqueue_prompt_with_delivery(id, &prompt("steer after summary", true), "steer")
        .await
        .unwrap();
    state.inner.store.enqueue_compact(id).await.unwrap();
    let mut events = state.subscribe();
    assert_eq!(
        crate::session_queue::drain_queued_prompts_into_active_run(&state, id).await,
        1
    );
    assert_eq!(
        state
            .inner
            .session_coordinator
            .active_run(id)
            .await
            .unwrap()
            .id,
        run.id
    );
    let mut compacted = false;
    let mut started = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == event_type::SESSION_COMPACTION_STARTED {
            assert_eq!(event.properties["reason"], "manual");
            started = true;
        }
        if event.kind == event_type::SESSION_COMPACTION_ENDED
            || event.kind == event_type::SESSION_COMPACTED
        {
            assert_eq!(event.properties["runContinues"], true);
            compacted = true;
        }
        if event.kind == event_type::SESSION_QUEUE_UPDATED
            && event.properties["request"].is_object()
        {
            assert!(compacted, "steer consumed before compact");
        }
    }
    assert!(started && compacted);
    // A compact-only boundary reports no steering, so it cannot force a followup.
    state.inner.store.enqueue_compact(id).await.unwrap();
    assert_eq!(
        crate::session_queue::drain_queued_prompts_into_active_run(&state, id).await,
        0
    );
    crate::session_run::finish_session_run(&state, id, &run.id).await;
    drop(state);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn compact_idle_worker_claims_run_and_failed_control_is_terminal() {
    for fail in [false, true] {
        let (state, mut info, path) = fixture().await;
        if fail {
            info.model.as_mut().unwrap().provider_id =
                "missing-local-test-provider".into();
            state.inner.store.update_session(&info).await.unwrap();
        }
        let id = info.id.as_str();
        append_prompt(&state, id, prompt("remember this", true), false)
            .await
            .unwrap();
        // append_prompt updates the model, so restore the intentional local failure.
        if fail {
            state.inner.store.update_session(&info).await.unwrap();
        }
        let mut events = state.subscribe();
        crate::session_queue::enqueue_manual_compaction(&state, id)
            .await
            .unwrap();
        settle(&state, id).await;
        assert!(!state.inner.store.compaction_queued(id).await.unwrap());
        assert_eq!(state.inner.store.queued_prompt_count(id).await.unwrap(), 0);
        let mut starts = 0;
        while let Ok(event) = events.try_recv() {
            if event.kind == event_type::SESSION_COMPACTION_STARTED {
                starts += 1;
            }
            if event.kind == event_type::SESSION_COMPACTION_ENDED
                || event.kind == event_type::SESSION_COMPACTED
            {
                assert!(event.properties.get("runContinues").is_none());
            }
        }
        assert_eq!(starts, 1, "failed controls must not hot-loop");
        drop(state);
        cleanup_sqlite_files(&path);
    }
}

#[tokio::test]
async fn pending_compact_survives_restart_and_transfer_without_becoming_prompt() {
    let (state, info, path) = fixture().await;
    let id = info.id.as_str();
    append_prompt(&state, id, prompt("retained context", true), false)
        .await
        .unwrap();
    assert!(state.inner.store.enqueue_compact(id).await.unwrap());
    assert!(!state.inner.store.enqueue_compact(id).await.unwrap());
    assert!(state
        .inner
        .store
        .list_queued_prompt_entries(id)
        .await
        .unwrap()
        .is_empty());
    let bundle = crate::session_transfer::export_session(&state, id)
        .await
        .unwrap();
    assert!(bundle.compaction_queued);
    assert!(bundle.queued_prompts.is_empty());
    let old_bundle = {
        let mut value = serde_json::to_value(&bundle).unwrap();
        value.as_object_mut().unwrap().remove("compactionQueued");
        value["version"] = json!(2);
        serde_json::from_value::<crate::session_transfer::SessionBundle>(value).unwrap()
    };
    assert!(!old_bundle.compaction_queued);
    let (target, _, target_path) = fixture().await;
    crate::session_transfer::import_session(&target, bundle, "/tmp")
        .await
        .unwrap();
    assert!(target.inner.store.compaction_queued(id).await.unwrap());
    state.inner.store.close().await;
    drop(state);
    let restarted = AppState::open_database(path.clone()).await.unwrap();
    settle(&restarted, id).await;
    assert!(!restarted.inner.store.compaction_queued(id).await.unwrap());
    assert!(restarted
        .inner
        .store
        .list_messages(id)
        .await
        .unwrap()
        .iter()
        .flat_map(|m| &m.parts)
        .any(|p| matches!(p, Part::Compaction(_))));
    crate::session_queue::resume_prompt_queues(target.clone())
        .await
        .unwrap();
    settle(&target, id).await;
    assert!(!target.inner.store.compaction_queued(id).await.unwrap());
    drop(restarted);
    drop(target);
    cleanup_sqlite_files(&path);
    cleanup_sqlite_files(&target_path);
}

#[tokio::test]
async fn compact_orphan_child_setup_error_is_terminal() {
    let (state, mut info, path) = fixture().await;
    info.parent_id = Some(neoism_agent_core::new_session_id());
    state.inner.store.update_session(&info).await.unwrap();
    crate::session_queue::enqueue_manual_compaction(&state, info.id.as_str())
        .await
        .unwrap();
    settle(&state, info.id.as_str()).await;
    assert!(!state
        .inner
        .store
        .compaction_queued(info.id.as_str())
        .await
        .unwrap());
    assert!(
        !state
            .inner
            .session_coordinator
            .worker_active(info.id.as_str())
            .await
    );
    drop(state);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn stop_during_initial_compact_prevents_first_ordinary_model_step() {
    let (state, info, path) = fixture().await;
    let id = info.id.as_str();
    append_prompt(&state, id, prompt("existing context", true), false)
        .await
        .unwrap();
    state.inner.store.enqueue_compact(id).await.unwrap();
    // A broadcast is an observation, not a scheduling barrier. Hold generation
    // at the started boundary until Stop has actually set the owning token.
    let mut release = Some(pause_at_compaction_start(&state, id).await);
    let mut events = state.subscribe();
    let generation = append_prompt(
        &state,
        id,
        prompt("must not generate after stop", false),
        true,
    );
    tokio::pin!(generation);
    let mut cancelled = false;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                biased;
                Ok(event) = events.recv() => {
                    if event.kind == event_type::SESSION_COMPACTION_STARTED {
                        assert!(crate::session_actions::abort_session_run(&state, id).await);
                        let owner = state.inner.session_coordinator.active_run(id).await.unwrap();
                        assert!(owner.cancel.load(std::sync::atomic::Ordering::SeqCst));
                        release.take().unwrap().send(()).unwrap();
                        cancelled = true;
                    }
                }
                result = &mut generation => { result.unwrap(); break; }
            }
        }
    })
    .await
    .unwrap();
    assert!(cancelled);
    let messages = state.inner.store.list_messages(id).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|m| matches!(m.info, MessageInfo::Assistant(_)))
            .count(),
        1,
        "only the cancelled summary assistant may exist"
    );
    assert!(state
        .inner
        .session_coordinator
        .active_run(id)
        .await
        .is_none());
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn stop_during_idle_compact_keeps_other_sessions_independent() {
    let (state, info, path) = fixture().await;
    let id = info.id.as_str();
    append_prompt(&state, id, prompt("cancel this summary", true), false)
        .await
        .unwrap();
    let release = pause_at_compaction_start(&state, id).await;
    let mut events = state.subscribe();
    crate::session_queue::enqueue_manual_compaction(&state, id)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == event_type::SESSION_COMPACTION_STARTED {
                break;
            }
        }
    })
    .await
    .unwrap();
    let owner = state.inner.session_coordinator.active_run(id).await.unwrap();
    assert!(crate::session_actions::abort_session_run(&state, id).await);
    assert!(owner.cancel.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        state.inner.session_coordinator.active_run(id).await.unwrap().id,
        owner.id
    );

    // Exercise another session on the SAME state while the first is held. No
    // global env/provider fixture or coordinator lock may serialize these runs.
    let mut other = info.clone();
    other.id = neoism_agent_core::new_session_id();
    state.inner.store.insert_session(&other).await.unwrap();
    let other_id = other.id.as_str();
    append_prompt(
        &state,
        other_id,
        prompt("summarize independently", true),
        false,
    )
    .await
    .unwrap();
    crate::session_queue::enqueue_manual_compaction(&state, other_id)
        .await
        .unwrap();
    settle(&state, other_id).await;
    assert!(state
        .inner
        .store
        .get_session(other_id)
        .await
        .unwrap()
        .unwrap()
        .extra
        .contains_key("summary"));
    assert_eq!(
        state.inner.session_coordinator.active_run(id).await.unwrap().id,
        owner.id
    );

    release.send(()).unwrap();
    settle(&state, id).await;
    let messages = state.inner.store.list_messages(id).await.unwrap();
    let assistants = messages
        .iter()
        .filter_map(|m| match &m.info {
            MessageInfo::Assistant(a) => Some(a),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(assistants.len(), 1);
    assert!(
        assistants[0].error.is_some(),
        "cancelled summary must not succeed"
    );
    assert!(!state.inner.store.compaction_queued(id).await.unwrap());
    let mut ended = 0;
    while let Ok(event) = events.try_recv() {
        if event.kind == event_type::SESSION_COMPACTION_ENDED
            && event.properties["sessionID"] == id
        {
            ended += 1;
            assert_eq!(event.properties["status"], "error");
            assert_eq!(event.properties["error"]["message"], "compaction cancelled");
        }
    }
    assert_eq!(ended, 1);
    drop(state);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn first_step_consumes_manual_control_without_extra_model_followup() {
    let (state, info, path) = fixture().await;
    let id = info.id.as_str();
    append_prompt(&state, id, prompt("existing context", true), false)
        .await
        .unwrap();
    state.inner.store.enqueue_compact(id).await.unwrap();
    let mut events = state.subscribe();
    append_prompt(&state, id, prompt("answer once", false), true)
        .await
        .unwrap();
    assert!(!state.inner.store.compaction_queued(id).await.unwrap());
    let mut compacted = false;
    while let Ok(event) = events.try_recv() {
        if event.kind == event_type::SESSION_COMPACTED {
            compacted = true;
        }
    }
    assert!(compacted);
    let messages = state.inner.store.list_messages(id).await.unwrap();
    let marker = messages
        .iter()
        .position(|m| m.parts.iter().any(|p| matches!(p, Part::Compaction(_))))
        .unwrap();
    assert_eq!(
        messages[..marker]
            .iter()
            .filter(|m| matches!(m.info, MessageInfo::Assistant(_)))
            .count(),
        0,
        "first model step preceded compaction"
    );
    assert_eq!(
        messages[marker..]
            .iter()
            .filter(|m| matches!(m.info, MessageInfo::Assistant(_)))
            .count(),
        2,
        "one summary and exactly one ordinary reply"
    );
    drop(state);
    cleanup_sqlite_files(&path);
}
