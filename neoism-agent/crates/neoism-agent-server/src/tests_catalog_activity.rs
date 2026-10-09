use super::*;

async fn next_activity<S, T, E>(body: &mut S) -> Value
where
    S: futures::Stream<Item = Result<T, E>> + Unpin,
    T: AsRef<[u8]>,
    E: std::fmt::Debug,
{
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let chunk = body.next().await.expect("SSE open").expect("SSE data");
            let text = std::str::from_utf8(chunk.as_ref()).unwrap();
            assert!(
                !text.contains("event: session.status"),
                "raw status must not override family: {text}"
            );
            if text.contains("event: session.catalog.activity") {
                let data = text.lines().find_map(|l| l.strip_prefix("data: ")).unwrap();
                let envelope: Value = serde_json::from_str(data).unwrap();
                return envelope["data"].clone();
            }
        }
    })
    .await
    .expect("catalog projection arrives")
}

async fn stream(state: &AppState) -> axum::response::Response {
    app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/v2/session-catalog/events?directory=%2Ftmp")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn wait_index(state: &AppState) {
    // The production serve entrypoint starts backfill; bare test routers do not.
    if !state.inner.store.session_list_index_ready().await.unwrap() {
        state.start_session_list_backfill();
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while !state.inner.store.session_list_index_ready().await.unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("catalog index ready");
}

async fn assert_list(state: &AppState, root: &str, expected: &str) {
    wait_index(state).await;
    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/v2/sessions?roots=true&directory=%2Ftmp&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page: Value = response_json(response).await;
    let item = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == root)
        .unwrap();
    assert_eq!(item["catalogActivity"], expected, "{page}");
    let info: Value = response_json(
        app(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("/v2/sessions/{root}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(info["catalogActivity"], expected);
    assert!(!state
        .inner
        .store
        .get_session(root)
        .await
        .unwrap()
        .unwrap()
        .extra
        .contains_key("catalogActivity"));
}

async fn reply_permission(state: &AppState, id: &str) {
    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/v2/interactions/permissions/{id}/reply"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"reply":"once"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

fn pending(child: &SessionInfo) -> PermissionRequestInfo {
    PermissionRequestInfo {
        id: "permission-catalog".into(),
        session_id: child.id.to_string(),
        message_id: "message".into(),
        title: "Allow".into(),
        permission: "read".into(),
        patterns: vec!["*".into()],
        always: vec![],
        tool: None,
        metadata: None,
    }
}

#[tokio::test]
async fn catalog_activity_outstanding_child_baseline_reconnect_and_stale_edges() {
    let path = std::env::temp_dir().join(format!(
        "catalog-activity-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let mut root = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    let mut child =
        store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    child.parent_id = Some(root.id.clone());
    child.directory = "/tmp/catalog-child-worktree".into();
    let mut foreign =
        store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    foreign.directory = "/foreign".into();
    for s in [&root, &child, &foreign] {
        state.inner.store.insert_session(s).await.unwrap();
    }
    let execution = state
        .inner
        .store
        .admit_execution_activity(root.id.as_str(), "catalog-execution", "message", "")
        .await
        .unwrap()
        .unwrap();
    root.extra.insert(
        crate::execution_activity::EXECUTION_ID_KEY.into(),
        json!(execution.execution_id),
    );
    root.extra.insert(
        crate::execution_activity::EXECUTION_ROOT_KEY.into(),
        json!(root.id),
    );
    state.inner.store.update_session(&root).await.unwrap();
    crate::execution_activity::register_subtask(&state, &root, child.id.as_str())
        .await
        .unwrap();
    assert_list(&state, root.id.as_str(), "running").await;
    let mut body = stream(&state).await.into_body().into_data_stream();
    assert_eq!(
        next_activity(&mut body).await,
        json!({"sessionID": root.id, "activity": "running"})
    );
    // Foreign trigger is excluded; cross-directory child's trigger belongs to root.
    for id in [&foreign.id, &child.id, &root.id] {
        state.publish(EventPayload::new(
            event_type::SESSION_STATUS,
            json!({"sessionID": id, "status": {"type": "idle"}}),
        ));
    }
    for _ in 0..2 {
        assert_eq!(next_activity(&mut body).await["activity"], "running");
    }
    drop(body);
    let mut body = stream(&state).await.into_body().into_data_stream();
    assert_eq!(next_activity(&mut body).await["activity"], "running");
    // Pending permission wins even over an outstanding branch.
    let request = pending(&child);
    state
        .inner
        .permissions
        .write()
        .await
        .insert(request.id.clone(), request.clone());
    state.publish(EventPayload::new(
        event_type::PERMISSION_ASKED,
        json!(request),
    ));
    assert_eq!(next_activity(&mut body).await["activity"], "permission");
    assert_list(&state, root.id.as_str(), "permission").await;
    reply_permission(&state, &request.id).await;
    assert_eq!(next_activity(&mut body).await["activity"], "running");
    let stale_runtime = state
        .inner
        .store
        .get_session_runtime_snapshot(root.id.as_str())
        .await
        .unwrap();
    crate::execution_activity::finish_subtask_for_child(
        &state,
        child.id.as_str(),
        "completed",
    )
    .await
    .unwrap();
    // Branch-terminal and execution-terminal edges both project Idle.
    for _ in 0..2 {
        assert_eq!(next_activity(&mut body).await["activity"], "idle");
    }
    // Late busy payload cannot revive a completed branch.
    state.publish(EventPayload::new(
        event_type::SESSION_STATUS,
        json!({"sessionID": child.id, "status": {"type": "busy"}}),
    ));
    assert_eq!(next_activity(&mut body).await["activity"], "idle");
    state.publish(EventPayload::new(
        event_type::SESSION_EXECUTION_UPDATED,
        json!({"sessionID": root.id, "runtime": stale_runtime}),
    ));
    assert_eq!(next_activity(&mut body).await["activity"], "idle");
    assert_list(&state, root.id.as_str(), "idle").await;
    drop(body);
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn catalog_activity_background_permission_and_model_overlap_priority() {
    use crate::background_job::{BackgroundJob, BackgroundJobStatus};
    let path = std::env::temp_dir().join(format!(
        "catalog-background-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    let mut child =
        store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    child.parent_id = Some(root.id.clone());
    for s in [&root, &child] {
        state.inner.store.insert_session(s).await.unwrap();
    }
    // An unfinished execution without live work must NOT turn background red white.
    state
        .inner
        .store
        .admit_execution_activity(root.id.as_str(), "background-execution", "message", "")
        .await
        .unwrap();
    let runtime = state.workspace_runtime(&root.directory).await.unwrap();
    let generation = runtime.snapshot();
    let background = generation.background().unwrap();
    background.jobs.write().await.insert(
        "catalog-job".into(),
        BackgroundJob {
            id: "catalog-job".into(),
            session_id: child.id.to_string(),
            description: "job".into(),
            command: "true".into(),
            cwd: "/tmp".into(),
            shell: "sh".into(),
            status: BackgroundJobStatus::Running,
            started_at: now_millis(),
            finished_at: None,
            exit_code: None,
            signal: None,
            pid: None,
            timeout_ms: 1000,
            output_limit_bytes: 1000,
            output_truncated: false,
            output: String::new(),
            error: None,
            command_patterns: vec![],
            always_patterns: vec![],
            external_dirs: vec![],
        },
    );
    let mut body = stream(&state).await.into_body().into_data_stream();
    assert_eq!(next_activity(&mut body).await["activity"], "background");
    assert_list(&state, root.id.as_str(), "background").await;
    state
        .inner
        .statuses
        .write()
        .await
        .insert(child.id.to_string(), SessionStatus::Busy { queue: None });
    state.publish(EventPayload::new(
        event_type::SESSION_STATUS,
        json!({"sessionID": child.id, "status": {"type": "idle"}}),
    ));
    assert_eq!(next_activity(&mut body).await["activity"], "running");
    let request = pending(&child);
    state
        .inner
        .permissions
        .write()
        .await
        .insert(request.id.clone(), request.clone());
    state.publish(EventPayload::new(
        event_type::PERMISSION_ASKED,
        json!(request),
    ));
    assert_eq!(next_activity(&mut body).await["activity"], "permission");
    state.inner.permissions.write().await.clear();
    state.inner.statuses.write().await.clear();
    // Unknown requestID also clears yellow via an authoritative scoped baseline.
    state.publish(EventPayload::new(
        event_type::PERMISSION_REPLIED,
        json!({"requestID": "unknown"}),
    ));
    assert_eq!(next_activity(&mut body).await["activity"], "background");
    // A terminal child with an actual residual Busy ledger must not mask red.
    state
        .inner
        .store
        .register_execution_subtask(
            "background-execution",
            root.id.as_str(),
            root.id.as_str(),
            child.id.as_str(),
            &state.inner.execution_owner_id,
            now_millis(),
        )
        .await
        .unwrap();
    assert!(state
        .inner
        .store
        .finish_execution_subtask("background-execution", child.id.as_str(), "completed")
        .await
        .unwrap());
    state
        .inner
        .statuses
        .write()
        .await
        .insert(child.id.to_string(), SessionStatus::Busy { queue: None });
    state.publish(EventPayload::new(
        event_type::SESSION_STATUS,
        json!({"sessionID": child.id, "status": {"type": "busy"}}),
    ));
    assert_eq!(next_activity(&mut body).await["activity"], "background");
    assert_list(&state, root.id.as_str(), "background").await;
    background.jobs.write().await.clear();
    state.publish(EventPayload::new(
        event_type::SESSION_BACKGROUND_TASKS_UPDATED,
        json!({"sessionID": child.id, "tasks": ["stale"]}),
    ));
    assert_eq!(next_activity(&mut body).await["activity"], "idle");
    drop(body);
    drop(background);
    drop(generation);
    drop(runtime);
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn catalog_activity_terminal_child_busy_ledger_is_not_resurrection_authority() {
    let path = std::env::temp_dir().join(format!(
        "catalog-terminal-ledger-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    let mut child =
        store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    child.parent_id = Some(root.id.clone());
    for s in [&root, &child] {
        state.inner.store.insert_session(s).await.unwrap();
    }
    state
        .inner
        .store
        .admit_execution_activity(root.id.as_str(), "terminal-ledger", "message", "")
        .await
        .unwrap();
    state
        .inner
        .store
        .register_execution_subtask(
            "terminal-ledger",
            root.id.as_str(),
            root.id.as_str(),
            child.id.as_str(),
            &state.inner.execution_owner_id,
            now_millis(),
        )
        .await
        .unwrap();
    assert!(state
        .inner
        .store
        .finish_execution_subtask("terminal-ledger", child.id.as_str(), "failed")
        .await
        .unwrap());
    state
        .inner
        .statuses
        .write()
        .await
        .insert(child.id.to_string(), SessionStatus::Busy { queue: None });
    let snapshot = state
        .inner
        .store
        .get_session_runtime_snapshot(root.id.as_str())
        .await
        .unwrap();
    assert_eq!(snapshot.branches[0].status, "failed");
    assert!(snapshot.execution.unwrap().active_segments.is_empty());
    assert!(state
        .inner
        .session_coordinator
        .active_runs()
        .await
        .is_empty());
    assert!(
        !state
            .inner
            .session_coordinator
            .worker_active(child.id.as_str())
            .await
    );
    assert_eq!(
        crate::session_queue::queued_prompt_count(&state, child.id.as_str()).await,
        0
    );
    assert_list(&state, root.id.as_str(), "idle").await;
    let mut body = stream(&state).await.into_body().into_data_stream();
    assert_eq!(next_activity(&mut body).await["activity"], "idle");
    state.publish(EventPayload::new(
        event_type::SESSION_STATUS,
        json!({"sessionID": child.id, "status": {"type": "busy"}}),
    ));
    assert_eq!(next_activity(&mut body).await["activity"], "idle");
    drop(body);
    // Manual prompts and external-agent child resumes claim this same live run
    // authority, even if an earlier durable child lifecycle is still terminal.
    let run = crate::session_run::start_session_run(&state, &child.id)
        .await
        .unwrap();
    assert_list(&state, root.id.as_str(), "running").await;
    crate::session_run::finish_session_run(&state, child.id.as_str(), &run.id).await;
    state
        .inner
        .statuses
        .write()
        .await
        .insert(child.id.to_string(), SessionStatus::Busy { queue: None });
    assert_list(&state, root.id.as_str(), "idle").await;
    // Root status fallback is deliberately preserved.
    state
        .inner
        .statuses
        .write()
        .await
        .insert(root.id.to_string(), SessionStatus::Busy { queue: None });
    assert_list(&state, root.id.as_str(), "running").await;
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn catalog_activity_real_permission_reply_sessionless_fallback_reprojects() {
    let path = std::env::temp_dir().join(format!(
        "catalog-real-reply-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    let mut child =
        store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    child.parent_id = Some(root.id.clone());
    for s in [&root, &child] {
        state.inner.store.insert_session(s).await.unwrap();
    }
    let request = pending(&child);
    state
        .inner
        .store
        .save_permission_request(&request)
        .await
        .unwrap();
    state
        .inner
        .permissions
        .write()
        .await
        .insert(request.id.clone(), request.clone());
    let mut body = stream(&state).await.into_body().into_data_stream();
    assert_eq!(next_activity(&mut body).await["activity"], "permission");
    // Leave only durable authority so interaction.rs takes its real sessionless
    // fallback branch (not a hand-published mock PERMISSION_REPLIED payload).
    state.inner.permissions.write().await.remove(&request.id);
    let mut events = state.subscribe();
    reply_permission(&state, &request.id).await;
    let reply = events.recv().await.unwrap();
    assert_eq!(reply.kind, event_type::PERMISSION_REPLIED);
    assert_eq!(reply.properties["requestID"], request.id);
    assert!(reply.properties.get("sessionID").is_none());
    assert_eq!(
        next_activity(&mut body).await,
        json!({"sessionID": root.id, "activity": "idle"})
    );
    drop(body);
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn catalog_activity_authorizes_owning_root_not_cross_directory_child() {
    use axum::{
        extract::{Query, State},
        response::IntoResponse,
        Extension,
    };
    let path = std::env::temp_dir().join(format!(
        "catalog-auth-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    let mut child =
        store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    child.parent_id = Some(root.id.clone());
    child.directory = "/foreign-worktree".into();
    let mut foreign =
        store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    foreign.extra.insert(
        crate::caller::TENANT_EXTRA_KEY.into(),
        json!("other-tenant"),
    );
    for s in [&root, &child, &foreign] {
        state.inner.store.insert_session(s).await.unwrap();
    }
    let claims = crate::caller::CallerClaims {
        subject: "test".into(),
        workspace_id: None,
        tenant_id: "local".into(),
        directory_prefixes: vec!["/tmp".into()],
        hosted: true,
        max_sessions: None,
        max_artifacts: None,
        max_artifact_bytes: None,
        artifact_retention_days: None,
        requests_per_minute: None,
        max_in_flight: None,
        resolved: None,
        worker: None,
    };
    let response = crate::v2_routes::v2_session_catalog_events(
        State(state.clone()),
        Some(Extension(claims.clone())),
        axum::http::HeaderMap::new(),
        Query(crate::InstanceQuery {
            directory: Some("/tmp".into()),
        }),
    )
    .await
    .into_response();
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        next_activity(&mut body).await,
        json!({"sessionID": root.id, "activity": "idle"})
    );
    state
        .inner
        .statuses
        .write()
        .await
        .insert(child.id.to_string(), SessionStatus::Busy { queue: None });
    for s in [&foreign, &child] {
        state.publish(EventPayload::new(
            event_type::SESSION_STATUS,
            json!({"sessionID": s.id}),
        ));
    }
    assert_eq!(
        next_activity(&mut body).await,
        json!({"sessionID": root.id, "activity": "running"})
    );
    wait_index(&state).await;
    let page = crate::v2_routes::v2_session_list(
        State(state.clone()),
        Query(crate::SessionListQuery {
            directory: Some("/tmp".into()),
            roots: Some("true".into()),
            path: None,
            start: None,
            search: None,
            limit: Some(10),
            cursor: None,
        }),
        Some(Extension(claims)),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].info.id, root.id);
    assert_eq!(
        serde_json::to_value(&page.items[0]).unwrap()["catalogActivity"],
        "running"
    );
    drop(body);
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn catalog_activity_retains_root_metadata_and_does_not_revive_deleted_roots() {
    let path = std::env::temp_dir().join(format!(
        "catalog-metadata-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let mut root = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    state.inner.store.insert_session(&root).await.unwrap();
    let mut body = stream(&state).await.into_body().into_data_stream();
    assert_eq!(next_activity(&mut body).await["activity"], "idle");
    let stale_info = root.clone();
    root.title = "Fresh authoritative title".into();
    state.inner.store.update_session(&root).await.unwrap();
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({"sessionID": root.id, "info": stale_info}),
    ));
    let update = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let update = String::from_utf8_lossy(&update);
    assert!(
        update.contains("event: session.updated")
            && update.contains("Fresh authoritative title")
            && update.contains("catalogActivity"),
        "{update}"
    );
    assert_eq!(next_activity(&mut body).await["activity"], "idle");
    state
        .inner
        .store
        .delete_session(root.id.as_str())
        .await
        .unwrap();
    state.publish(EventPayload::new(
        event_type::SESSION_DELETED,
        json!({"sessionID": root.id, "info": root}),
    ));
    let deleted = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&deleted).contains("event: session.deleted"));
    assert_eq!(next_activity(&mut body).await["activity"], "idle");
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({"sessionID": root.id, "info": stale_info}),
    ));
    let next = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    state.inner.store.insert_session(&next).await.unwrap();
    state.publish(EventPayload::new(
        event_type::SESSION_CREATED,
        json!({"sessionID": next.id, "info": next}),
    ));
    let created = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let created = String::from_utf8_lossy(&created);
    assert!(
        created.contains("event: session.created")
            && created.contains(next.id.as_str())
            && !created.contains(root.id.as_str()),
        "{created}"
    );
    drop(body);
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn catalog_activity_existing_lease_loop_notifies_expired_provider_repair() {
    let path = std::env::temp_dir().join(format!(
        "catalog-repair-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    state.inner.store.insert_session(&root).await.unwrap();
    state
        .inner
        .store
        .admit_execution_activity(root.id.as_str(), "repair-execution", "message", "")
        .await
        .unwrap();
    state
        .inner
        .store
        .heartbeat_execution_owner("expired-owner", now_millis())
        .await
        .unwrap();
    state
        .inner
        .store
        .insert_execution_segment(
            root.id.as_str(),
            "repair-execution",
            "orphan-segment",
            "expired-owner",
            root.id.as_str(),
            1,
        )
        .await
        .unwrap();
    let mut body = stream(&state).await.into_body().into_data_stream();
    assert_eq!(next_activity(&mut body).await["activity"], "running");
    state
        .inner
        .store
        .heartbeat_execution_owner("expired-owner", 1)
        .await
        .unwrap();
    assert_eq!(next_activity(&mut body).await["activity"], "idle");
    drop(body);
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}
