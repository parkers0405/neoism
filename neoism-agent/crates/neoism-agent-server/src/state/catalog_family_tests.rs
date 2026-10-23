use super::*;

fn session(id: &str, parent: Option<&str>, hint: Option<&str>) -> SessionInfo {
    let mut value = json!({
        "id": id, "slug": id, "projectId": "global", "directory": "/tmp",
        "title": id, "version": "test", "time": {"created": 1, "updated": 1},
    });
    if let Some(parent) = parent {
        value["parentId"] = json!(parent);
    }
    if let Some(hint) = hint {
        value[crate::execution_activity::EXECUTION_ROOT_KEY] = json!(hint);
    }
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn targeted_catalog_family_matches_global_membership_and_uses_indexes() {
    let state = AppState::open_database(std::env::temp_dir().join(format!(
        "targeted-catalog-{}.db",
        neoism_agent_core::new_session_id()
    )))
    .await
    .unwrap();
    let mut sessions = vec![
        session("root", None, None),
        session("child", Some("root"), None),
        session("sibling", Some("root"), None),
        session("deep", Some("child"), None),
        session("override", Some("child"), Some("elsewhere")),
        session("override-descendant", Some("override"), None),
        session("off-tree", Some("other"), Some("root")),
        session("other", None, None),
        session("orphan", Some("missing"), None),
        session("orphan-child", Some("orphan"), None),
        session("cycle-a", Some("cycle-b"), None),
        session("cycle-b", Some("cycle-a"), None),
        session("cycle-tail", Some("cycle-a"), None),
        session("child-root", Some("other"), Some("child")),
    ];
    // Exercise more than one frontier batch, and non-string hint fallback.
    for index in 0..140 {
        sessions.push(session(&format!("wide-{index}"), Some("root"), None));
    }
    let mut invalid_hint = session("non-string-hint", Some("root"), None);
    invalid_hint.extra.insert(
        crate::execution_activity::EXECUTION_ROOT_KEY.into(),
        json!(17),
    );
    sessions.push(invalid_hint);
    for info in &sessions {
        state.inner.store.insert_session(info).await.unwrap();
    }
    let membership = crate::catalog_activity::families(&sessions);
    for key in sessions
        .iter()
        .map(|s| s.id.as_str())
        .chain(["elsewhere", "missing"])
    {
        assert_eq!(
            state.inner.store.catalog_family_members(key).await.unwrap(),
            membership.get(key).cloned().unwrap_or_default(),
            "root {key}"
        );
    }
    // EXPLAIN the full production projection and compound frontier, not just
    // a single-column equality: IN on this expression silently scans in Turso.
    const COLUMNS: &str = "id, json_extract(info_json, '$.parentId') AS parent_id, CASE WHEN json_type(info_json, '$.executionRootSessionID') = 'text' THEN json_extract(info_json, '$.executionRootSessionID') END AS execution_root";
    for size in [1, 2, 16] {
        let query = vec![
            format!("SELECT {COLUMNS} FROM sessions WHERE json_extract(info_json, '$.parentId') = ?");
            size
        ].join(" UNION ALL ");
        let plan = state.inner.store.db.fetch_all(
            &format!("EXPLAIN QUERY PLAN {query}"),
            vec![text("root"); size],
        ).await.unwrap();
        let details: Vec<_> = plan.iter().map(|row| row.get_str("detail").unwrap()).collect();
        assert_eq!(details.iter().filter(|detail| detail.contains("idx_sessions_catalog_parent") && detail.contains("SEARCH")).count(), size, "{details:?}");
        assert!(!details.iter().any(|detail| detail.contains("SCAN sessions")), "{details:?}");
    }
    // Ancestor ID IN is a different planner path: retain it only while it
    // remains a primary-key seek, including the maximum production batch.
    for size in [1, 2, 128] {
        let placeholders = vec!["?"; size].join(",");
        let plan = state.inner.store.db.fetch_all(
            &format!("EXPLAIN QUERY PLAN SELECT {COLUMNS} FROM sessions WHERE id IN ({placeholders})"),
            vec![text("root"); size],
        ).await.unwrap();
        let details: Vec<_> = plan.iter().map(|row| row.get_str("detail").unwrap()).collect();
        assert!(details.iter().any(|detail| detail.contains("SEARCH sessions") && detail.contains("(id=?)")), "{details:?}");
        assert!(!details.iter().any(|detail| detail.contains("SCAN sessions")), "{details:?}");
    }
    let plan = state.inner.store.db.fetch_all(
        &format!("EXPLAIN QUERY PLAN SELECT {COLUMNS} FROM sessions WHERE id = ? UNION SELECT {COLUMNS} FROM sessions WHERE json_extract(info_json, '$.executionRootSessionID') = ?"),
        vec![text("root"), text("root")],
    ).await.unwrap();
    assert!(plan.iter().any(|row| row.get_str("detail").unwrap().contains("idx_sessions_catalog_execution_root")));
    state.shutdown().await.unwrap();
}

async fn assert_target_matches_catalog(
    state: &AppState,
    info: &SessionInfo,
    expected: neoism_agent_core::CatalogActivity,
) {
    let targeted = crate::catalog_activity::project_one(state, info.clone())
        .await
        .unwrap();
    let global = crate::catalog_activity::project(state, vec![info.clone()])
        .await
        .unwrap();
    assert_eq!(targeted.catalog_activity, expected);
    assert_eq!(
        serde_json::to_value(targeted).unwrap(),
        serde_json::to_value(&global[0]).unwrap()
    );
}

#[tokio::test]
async fn targeted_catalog_activity_preserves_child_and_runtime_authorities_without_global_hydration(
) {
    use neoism_agent_core::{CatalogActivity as A, SessionStatus};
    let path = std::env::temp_dir().join(format!(
        "targeted-activity-{}.db",
        neoism_agent_core::new_session_id()
    ));
    let state = AppState::open_database(path.clone()).await.unwrap();
    let mut root = session("root", None, None);
    root.extra.insert(
        crate::execution_activity::EXECUTION_ID_KEY.into(),
        json!("execution"),
    );
    let child = session("child", Some("root"), None);
    let sibling = session("sibling", Some("root"), None);
    for info in [&root, &child, &sibling] {
        state.inner.store.insert_session(info).await.unwrap();
    }
    state
        .inner
        .statuses
        .write()
        .await
        .insert("child".into(), SessionStatus::Busy { queue: None });
    assert_target_matches_catalog(&state, &root, A::Idle).await;
    assert_target_matches_catalog(&state, &child, A::Running).await;
    state
        .inner
        .store
        .admit_execution_activity("root", "execution", "message", "")
        .await
        .unwrap();
    state.inner.statuses.write().await.clear();
    state
        .inner
        .session_coordinator
        .install_run(
            "child",
            SessionRun {
                id: "child-run".into(),
                started_at: crate::now_millis(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )
        .await;
    assert_target_matches_catalog(&state, &root, A::Idle).await;
    assert_target_matches_catalog(&state, &child, A::Running).await;
    // A durable roster member need not have a sessions row. Permission on that
    // branch must still win, even with a terminal lifecycle status.
    state
        .inner
        .store
        .register_execution_subtask(
            "execution",
            "root",
            "root",
            "runtime-only",
            &state.inner.execution_owner_id,
            crate::now_millis(),
        )
        .await
        .unwrap();
    state
        .inner
        .store
        .finish_execution_subtask("execution", "runtime-only", "completed")
        .await
        .unwrap();
    let pending = PermissionRequestInfo {
        id: "permission".into(),
        session_id: "runtime-only".into(),
        message_id: "message".into(),
        title: "Allow".into(),
        permission: "bash".into(),
        patterns: vec!["*".into()],
        always: vec![],
        tool: None,
        metadata: None,
    };
    state
        .inner
        .permissions
        .write()
        .await
        .insert("permission".into(), pending);
    assert_target_matches_catalog(&state, &root, A::Permission).await;
    assert_target_matches_catalog(&state, &child, A::Running).await;
    // Sibling permission belongs to the root family, not a child GET's family.
    state
        .inner
        .permissions
        .write()
        .await
        .get_mut("permission")
        .unwrap()
        .session_id = "sibling".into();
    assert_target_matches_catalog(&state, &root, A::Permission).await;
    assert_target_matches_catalog(&state, &child, A::Running).await;
    state.inner.permissions.write().await.clear();
    let runtime = state.workspace_runtime("/tmp").await.unwrap();
    let generation = runtime.snapshot();
    let background = generation.background().unwrap();
    background.jobs.write().await.insert(
        "job".into(),
        crate::background_job::BackgroundJob {
            id: "job".into(),
            session_id: "sibling".into(),
            description: "test".into(),
            command: "true".into(),
            cwd: "/tmp".into(),
            shell: "sh".into(),
            status: crate::background_job::BackgroundJobStatus::Running,
            started_at: crate::now_millis(),
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
    assert_target_matches_catalog(&state, &root, A::Background).await;
    state
        .inner
        .statuses
        .write()
        .await
        .insert("root".into(), SessionStatus::Busy { queue: None });
    assert_target_matches_catalog(&state, &root, A::Running).await;
    state
        .inner
        .store
        .associate_host_directory("/tmp", "targeted-host")
        .await
        .unwrap();
    // A valid JSON but unhydratable unrelated row makes the old global path
    // fail. GET must not deserialize it (nor consult the list sidecar).
    state
        .inner
        .store
        .db
        .execute(
            "UPDATE sessions SET info_json = '{}' WHERE id = 'sibling'",
            vec![],
        )
        .await
        .unwrap();
    state
        .inner
        .store
        .db
        .execute("DELETE FROM session_list_index", vec![])
        .await
        .unwrap();
    assert!(state.inner.store.list_sessions().await.is_err());
    let got = crate::session_routes::session_get(
        axum::extract::State(state.clone()),
        axum::extract::Path("root".into()),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(got.catalog_activity, A::Running);
    assert_eq!(got.info.id, root.id);
    assert_eq!(got.info.workspace_id.as_deref(), Some("targeted-host"));
    assert_eq!(
        crate::caller::session_tenant(&got.info),
        "workspace:targeted-host"
    );
    let missing = crate::session_routes::session_get(
        axum::extract::State(state.clone()),
        axum::extract::Path("absent".into()),
    )
    .await;
    assert!(missing.is_err());
    background.jobs.write().await.clear();
    drop(background);
    drop(generation);
    drop(runtime);
    state.shutdown().await.unwrap();
    let _ = std::fs::remove_file(path);
}
