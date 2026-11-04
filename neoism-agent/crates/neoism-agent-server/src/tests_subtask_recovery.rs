use super::*;

#[tokio::test]
async fn explicit_stop_repairs_orphan_branch_and_timer_without_touching_family() {
    tokio::time::timeout(Duration::from_secs(20), async {
    for via_tool in [false, true] {
        let path = std::env::temp_dir().join(format!("subtask-stop-{}.sqlite3", Id::ascending(IdKind::Event)));
        let state = AppState::open_database(path.clone()).await.unwrap();
        let mut parent = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
        parent.extra.insert(crate::execution_activity::EXECUTION_ID_KEY.into(), json!("execution"));
        let mut child = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
        child.parent_id = Some(parent.id.clone());
        let mut sibling = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
        sibling.parent_id = Some(parent.id.clone());
        for session in [&parent, &child, &sibling] {
            state.inner.store.insert_session(session).await.unwrap();
        }
        state.inner.store.admit_execution_activity(parent.id.as_str(), "execution", "message", "").await.unwrap().unwrap();
        for session in [&child, &sibling] {
            crate::execution_activity::register_subtask(&state, &parent, session.id.as_str()).await.unwrap();
        }
        for (session, segment) in [(&parent, "parent-segment"), (&child, "orphan-segment"), (&sibling, "sibling-segment")] {
            assert!(state.inner.store.insert_execution_segment(
                parent.id.as_str(), "execution", segment, &state.inner.execution_owner_id,
                session.id.as_str(), now_millis().saturating_sub(100),
            ).await.unwrap());
        }
        let before = state.inner.store.get_session_runtime_snapshot(parent.id.as_str()).await.unwrap();
        assert!(!crate::tool_runtime::session_is_running(&state, child.id.as_str()).await);
        assert_eq!(crate::session_queue::queued_prompt_count(&state, child.id.as_str()).await, 0);
        if via_tool {
            let generation = state.plugin_snapshot_for_session(&parent.directory, parent.id.as_str()).await;
            let result = crate::plugins::subagents::stop_task_tool(
                &state, &generation, &parent.id, json!({"task_id": child.id}),
            ).await.unwrap();
            assert_eq!(result.metadata.unwrap()["stopped"], json!([child.id]));
        } else {
            let response = app(state.clone()).oneshot(request(
                Method::POST, &format!("/v2/sessions/{}/abort", child.id), None,
            )).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response_json::<bool>(response).await);
        }
        let after = state.inner.store.get_session_runtime_snapshot(parent.id.as_str()).await.unwrap();
        assert!(after.family_revision > before.family_revision);
        assert_eq!(after.branches.iter().find(|b| b.session_id == child.id.as_str()).unwrap().status, "failed");
        assert_eq!(after.branches.iter().find(|b| b.session_id == sibling.id.as_str()).unwrap().status, "outstanding");
        let activity = after.execution.as_ref().unwrap();
        assert!(!activity.finished);
        assert_eq!(activity.active_segments.len(), 2);
        assert!(activity.active_segments.contains_key("parent-segment"));
        assert!(activity.active_segments.contains_key("sibling-segment"));
        assert!(activity.session_activities[child.id.as_str()].active_segments.is_empty());
        assert!(!crate::session_actions::abort_session_run(&state, child.id.as_str()).await);
        let again = state.inner.store.get_session_runtime_snapshot(parent.id.as_str()).await.unwrap();
        assert_eq!(again.family_revision, after.family_revision);
        assert_eq!(again.execution.unwrap().completed_ms, activity.completed_ms);
        // A completion guard arriving late cannot undo explicit cancellation.
        crate::execution_activity::finish_subtask_for_child(&state, child.id.as_str(), "completed").await.unwrap();
        assert_eq!(state.inner.store.execution_subtask_status("execution", child.id.as_str()).await.unwrap().as_deref(), Some("failed"));
        state.shutdown().await.unwrap();
        cleanup_sqlite_files(&path);
    }
    }).await.expect("orphan cancellation must not block on its own admission lock");
}

#[tokio::test]
async fn unfinished_resumed_subtask_is_not_a_completed_result_or_idle_notification() {
    let path = std::env::temp_dir().join(format!("subtask-result-{}.sqlite3", Id::ascending(IdKind::Event)));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let parent = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    let mut child = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    child.parent_id = Some(parent.id.clone());
    for session in [&parent, &child] { state.inner.store.insert_session(session).await.unwrap(); }
    // Start with a completed generation, then append a resumed prompt with no reply.
    let [old_user, old_reply] = test_compaction_pair(&child.id, None, "old result");
    for message in [&old_user, &old_reply] { state.inner.store.append_message(child.id.as_str(), message).await.unwrap(); }
    assert_eq!(crate::tool_runtime::task_result_output_for_child(&state, &child).await.unwrap().0, "completed");
    let user = store_test_message(&child.id, now_millis(), "follow-up");
    let MessageInfo::User(user_info) = &user.info else { unreachable!() };
    state.inner.store.append_message(child.id.as_str(), &user).await.unwrap();
    assert_eq!(crate::tool_runtime::task_result_output_for_child(&state, &child).await.unwrap().0, "pending");
    let mut reply = old_reply.clone();
    let MessageInfo::Assistant(info) = &mut reply.info else { unreachable!() };
    info.id = Id::ascending(IdKind::Message);
    info.parent_id = user_info.id.clone();
    info.time.created = now_millis();
    info.time.completed = None;
    info.finish = None;
    reply.parts = vec![Part::Text(TextPart {
        id: Id::ascending(IdKind::Part), session_id: child.id.clone(),
        message_id: info.id.clone(), text: String::new(), synthetic: None, time: None,
    })];
    state.inner.store.append_message(child.id.as_str(), &reply).await.unwrap();
    crate::session_actions::mark_subtask_notify_on_idle(&state, child.id.as_str(), &user_info.id).await.unwrap();
    assert_eq!(crate::tool_runtime::task_result_output_for_child(&state, &child).await.unwrap().0, "pending");
    assert!(!crate::session_actions::publish_deferred_subtask_completion_if_idle(&state, child.id.as_str()).await);
    // A completed tool step is also not a terminal assistant reply.
    let MessageInfo::Assistant(info) = &mut reply.info else { unreachable!() };
    info.time.completed = Some(now_millis());
    info.finish = Some("tool-calls".into());
    state.inner.store.update_message(child.id.as_str(), &reply).await.unwrap();
    assert_eq!(crate::tool_runtime::task_result_output_for_child(&state, &child).await.unwrap().0, "pending");
    assert!(!crate::session_actions::publish_deferred_subtask_completion_if_idle(&state, child.id.as_str()).await);
    let MessageInfo::Assistant(info) = &mut reply.info else { unreachable!() };
    info.finish = Some("stop".into());
    state.inner.store.update_message(child.id.as_str(), &reply).await.unwrap();
    assert_eq!(crate::tool_runtime::task_result_output_for_child(&state, &child).await.unwrap().0, "completed");
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}