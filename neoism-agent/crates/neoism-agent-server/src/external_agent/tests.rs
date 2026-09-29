use super::*;

#[test]
fn resolves_supported_external_agents() {
    assert!(matches!(
        ExternalRuntime::resolve("opencode"),
        Some(ExternalRuntime::OpenCode)
    ));
    assert!(matches!(
        ExternalRuntime::resolve("codex"),
        Some(ExternalRuntime::Codex)
    ));
    assert!(matches!(
        ExternalRuntime::resolve("claude-code"),
        Some(ExternalRuntime::Claude)
    ));
    assert!(ExternalRuntime::resolve("general").is_none());
}

#[test]
fn external_acp_configs_use_expected_launchers() {
    let mut services = crate::standard_services();
    services.executables = std::sync::Arc::new(
        crate::executable::test_support::FakeExecutableService::with(
            "npx",
            "/injected/npx",
        ),
    );
    let codex = ExternalRuntime::Codex
        .acp_config("/tmp", &services)
        .unwrap();
    assert_eq!(
        std::path::Path::new(&codex.command)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("npx")
    );
    assert_eq!(
        codex.args,
        vec!["--yes", "@agentclientprotocol/codex-acp@1.13.1"]
    );

    let claude = ExternalRuntime::Claude
        .acp_config("/tmp", &services)
        .unwrap();
    assert_eq!(
        std::path::Path::new(&claude.command)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("npx")
    );
    assert_eq!(
        claude.args,
        vec!["--yes", "@agentclientprotocol/claude-agent-acp@0.81.1"]
    );

    services.executables = std::sync::Arc::new(
        crate::executable::test_support::FakeExecutableService::default(),
    );
    let error = ExternalRuntime::Codex
        .acp_config("/tmp", &services)
        .unwrap_err();
    assert!(error.contains("external Agent executable `npx` is unavailable"));
    assert!(error.contains("install it"));
    let missing_opencode = ExternalRuntime::OpenCode
        .acp_config("/tmp", &services)
        .unwrap_err();
    assert!(missing_opencode.contains("`opencode` is unavailable"));
}

#[test]
fn maps_neoism_permission_replies_to_acp_options() {
    let options = json!([
        { "kind": "allow_always", "optionId": "allow_always" },
        { "kind": "allow_once", "optionId": "allow" },
        { "kind": "reject_once", "optionId": "reject" }
    ]);
    let ids = vec![
        "allow_always".to_string(),
        "allow".to_string(),
        "reject".to_string(),
    ];

    assert_eq!(
        select_acp_permission_option(&options, &ids, "once").as_deref(),
        Some("allow")
    );
    assert_eq!(
        select_acp_permission_option(&options, &ids, "always").as_deref(),
        Some("allow_always")
    );
    assert_eq!(
        select_acp_permission_option(&options, &ids, "reject").as_deref(),
        Some("reject")
    );
}

#[test]
fn only_explicit_acp_diff_content_becomes_patch_metadata() {
    assert_eq!(super::events::acp_diff_content(&json!({
        "content": [{"type":"diff", "path":"src/lib.rs", "oldText":"old\n", "newText":"new\n"}]
    })).unwrap()[0]["path"], "src/lib.rs");
    assert!(super::events::acp_diff_content(&json!({
        "title":"Edited src/lib.rs", "content":[{"type":"text", "text":"done"}]
    }))
    .is_none());
}

#[test]
fn detects_provider_owned_nested_agent_tools() {
    assert!(!is_external_nested_agent_tool(
        &json!({ "kind": "think", "title": "Review code" }),
        &json!({ "prompt": "review src/lib.rs", "description": "Review code" })
    ));
    assert!(is_external_nested_agent_tool(
        &json!({ "title": "Task" }),
        &json!({ "prompt": "inspect", "subagent_type": "general" })
    ));
    assert!(is_external_nested_agent_tool(
        &json!({ "title": "Task", "toolCallId": "task-1" }),
        &json!({})
    ));
    assert!(is_external_nested_agent_tool(
        &json!({ "kind": "other", "title": "Task", "toolCallId": "task-1" }),
        &json!({})
    ));
    assert!(is_external_nested_agent_tool(
        &json!({ "title": "spawn_agent", "toolCallId": "task-2" }),
        &json!({})
    ));
    assert!(!is_external_nested_agent_tool(
        &json!({ "title": "Review code", "toolCallId": "task-3" }),
        &json!({ "prompt": "inspect", "subagent_type": "general" })
    ));
    assert!(!is_external_nested_agent_tool(
        &json!({ "sessionUpdate": "plan", "title": "Task" }),
        &json!({})
    ));
    assert!(!is_external_nested_agent_tool(
        &json!({ "kind": "execute", "title": "cargo test" }),
        &json!({ "command": "cargo test" })
    ));
}

#[test]
fn root_routing_requires_persisted_acp_metadata_and_no_parent() {
    let id = Id::ascending(IdKind::Session);
    let mut session = SessionInfo {
        id: id.clone(),
        slug: "test".into(),
        project_id: "test".into(),
        workspace_id: None,
        directory: "/tmp".into(),
        path: None,
        parent_id: None,
        title: "ACP".into(),
        agent: Some("build".into()),
        model: None,
        version: "test".into(),
        time: TimeInfo {
            created: 0,
            updated: 0,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };
    assert!(root_runtime(&session).is_none());
    session.extra.insert(
        "externalAgent".into(),
        json!({"runtime":"acp", "provider":"codex"}),
    );
    assert_eq!(root_runtime(&session), Some(ExternalRuntime::Codex));
    session.parent_id = Some(id);
    assert!(root_runtime(&session).is_none());
}

#[tokio::test]
async fn structured_acp_plan_persists_emits_and_reloads_for_root_and_child() {
    use neoism_agent_core::CreateSessionRequest;
    let root = std::env::temp_dir()
        .join(format!("neoism-acp-plan-{}", Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(&root).unwrap();
    let db = root.join("agent.sqlite3");
    let state = AppState::open_database(db.clone()).await.unwrap();
    let session = crate::session_routes::create_session_in_directory(
        &state,
        root.to_str().unwrap(),
        CreateSessionRequest {
            parent_id: None,
            title: None,
            agent: None,
            model: None,
            permission: None,
            workspace_id: None,
            external_provider: Some("codex".into()),
            external_options: None,
        },
        BTreeMap::new(),
    )
    .await
    .unwrap();
    let mut child = session.clone();
    child.id = Id::ascending(IdKind::Session);
    child.parent_id = Some(session.id.clone());
    child.extra.get_mut("externalAgent").unwrap()["lastUpdate"] =
        json!({"sessionUpdate":"status","status":"running"});
    state.inner.store.insert_session(&child).await.unwrap();
    let mut events = state.subscribe();
    let plan = json!({"sessionUpdate":"plan", "entries":[
        {"content":"write parser", "status":"in_progress", "priority":"high"},
        {"content":"test parser", "status":"pending", "priority":"medium"}
    ]});
    for id in [&session.id, &child.id] {
        project_external_plan(&state, id.as_str(), &plan)
            .await
            .unwrap();
        let event = events.recv().await.unwrap();
        assert_eq!(event.kind, event_type::TODO_UPDATED);
        assert_eq!(event.properties["sessionID"], id.as_str());
        assert_eq!(event.properties["todos"][0]["status"], "in_progress");
        // Repeated snapshot is idempotent; a status string is not a plan.
        project_external_plan(&state, id.as_str(), &plan)
            .await
            .unwrap();
        project_external_plan(
            &state,
            id.as_str(),
            &json!({"sessionUpdate":"status","status":"completed"}),
        )
        .await
        .unwrap();
        assert!(events.try_recv().is_err());
        // Malformed entries must not silently replace an existing plan.
        project_external_plan(
            &state,
            id.as_str(),
            &json!({"sessionUpdate":"plan", "entries":[{"content":"partial"}]}),
        )
        .await
        .unwrap();
        assert!(events.try_recv().is_err());
        let todos = crate::session_routes::session_todo_list(
            axum::extract::State(state.clone()),
            axum::extract::Path(id.to_string()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(todos.len(), 2);
        project_external_plan(
            &state,
            id.as_str(),
            &json!({"sessionUpdate":"plan", "entries":[
                {"content":"write parser", "status":"completed", "priority":"high"},
                {"content":"test parser", "status":"completed", "priority":"medium"}
            ]}),
        )
        .await
        .unwrap();
        assert_eq!(
            events.recv().await.unwrap().properties["todos"][1]["status"],
            "completed"
        );
    }
    let child_loaded = state
        .inner
        .store
        .get_session(child.id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        child_loaded.extra["externalAgent"]["lastUpdate"]["sessionUpdate"],
        "status"
    );
    assert_eq!(
        child_loaded.extra["externalAgent"]["planTodos"][0]["status"],
        "completed"
    );
    drop(events);
    state.shutdown().await.unwrap();
    drop(state);
    let reopened = AppState::open_database(db).await.unwrap();
    for id in [&session.id, &child.id] {
        let todos = crate::session_routes::session_todo_list(
            axum::extract::State(reopened.clone()),
            axum::extract::Path(id.to_string()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].status, "completed");
    }
    // Empty structured plans clear tasks; an empty status never does.
    let mut events = reopened.subscribe();
    project_external_plan(
        &reopened,
        session.id.as_str(),
        &json!({"sessionUpdate":"plan", "entries":[]}),
    )
    .await
    .unwrap();
    assert_eq!(events.recv().await.unwrap().properties["todos"], json!([]));
    assert!(crate::session_routes::session_todo_list(
        axum::extract::State(reopened.clone()),
        axum::extract::Path(session.id.to_string()),
    )
    .await
    .unwrap()
    .0
    .is_empty());
    reopened.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn acp_terminal_permission_defaults_to_ask_and_children_inherit_rules() {
    use neoism_agent_core::CreateSessionRequest;
    let root = std::env::temp_dir().join(format!(
        "neoism-acp-permission-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let state = AppState::open_database(root.join("agent.sqlite3"))
        .await
        .unwrap();
    let session = crate::session_routes::create_session_in_directory(
        &state,
        root.to_str().unwrap(),
        CreateSessionRequest {
            parent_id: None,
            title: None,
            agent: None,
            model: None,
            permission: None,
            workspace_id: None,
            external_provider: Some("opencode".into()),
            external_options: None,
        },
        BTreeMap::new(),
    )
    .await
    .unwrap();
    let rules =
        requests::external_effective_permissions_for_session(&state, session.id.as_str())
            .await
            .unwrap();
    assert_eq!(
        permission::evaluate("bash", "pwd", &rules).action,
        PermissionAction::Ask
    );
    let mut restricted = session;
    restricted.permission = Some(vec![PermissionRule {
        permission: "bash".into(),
        pattern: "*".into(),
        action: PermissionAction::Deny,
    }]);
    state.inner.store.update_session(&restricted).await.unwrap();
    let child = lifecycle::create_external_subtask_session(
        &state,
        &restricted,
        "pwd",
        "test",
        ExternalRuntime::OpenCode,
    )
    .await
    .unwrap();
    for id in [&restricted.id, &child.id] {
        let rules =
            requests::external_effective_permissions_for_session(&state, id.as_str())
                .await
                .unwrap();
        assert_eq!(
            permission::evaluate("bash", "pwd", &rules).action,
            PermissionAction::Deny
        );
    }
    state.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn resumed_import_keeps_source_host_and_plan_metadata() {
    let root = std::env::temp_dir().join(format!(
        "neoism-acp-import-metadata-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let state = AppState::open_database(root.join("agent.sqlite3"))
        .await
        .unwrap();
    let mut session = crate::session_routes::create_session_in_directory(
        &state,
        root.to_str().unwrap(),
        neoism_agent_core::CreateSessionRequest {
            parent_id: None,
            title: None,
            agent: None,
            model: None,
            permission: None,
            workspace_id: None,
            external_provider: Some("claude".into()),
            external_options: None,
        },
        BTreeMap::new(),
    )
    .await
    .unwrap();
    session.extra.get_mut("externalAgent").unwrap()["sourceHost"] = json!("host-a");
    session.extra.get_mut("externalAgent").unwrap()["sourceKey"] = json!("source-a");
    session.extra.get_mut("externalAgent").unwrap()["historyState"] = json!("text_only");
    session.extra.get_mut("externalAgent").unwrap()["planTodos"] = json!([]);
    state.inner.store.update_session(&session).await.unwrap();

    update_external_session_metadata(
        &state,
        session.id.as_str(),
        ExternalRuntime::Claude,
        "native-session",
        "running",
    )
    .await
    .unwrap();
    let loaded = state
        .inner
        .store
        .get_session(session.id.as_str())
        .await
        .unwrap()
        .unwrap();
    let external = &loaded.extra["externalAgent"];
    assert_eq!(external["sourceHost"], "host-a");
    assert_eq!(external["sourceKey"], "source-a");
    assert_eq!(external["historyState"], "text_only");
    assert_eq!(external["planTodos"], json!([]));
    assert_eq!(external["externalSessionId"], "native-session");
    assert_eq!(external["status"], "running");
    state.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn newly_bound_root_matches_its_provider_catalog_identity() {
    let root = std::env::temp_dir().join(format!(
        "neoism-acp-source-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let state = AppState::open_database(root.join("agent.sqlite3"))
        .await
        .unwrap();
    let session = crate::session_routes::create_session_in_directory(
        &state,
        root.to_str().unwrap(),
        neoism_agent_core::CreateSessionRequest {
            parent_id: None,
            title: None,
            agent: None,
            model: None,
            permission: None,
            workspace_id: None,
            external_provider: Some("claude".into()),
            external_options: None,
        },
        BTreeMap::new(),
    )
    .await
    .unwrap();
    update_external_session_metadata(
        &state,
        session.id.as_str(),
        ExternalRuntime::Claude,
        "opaque-claude-id",
        "idle",
    )
    .await
    .unwrap();
    let bound = state
        .inner
        .store
        .get_session(session.id.as_str())
        .await
        .unwrap()
        .unwrap();
    let cwd = std::fs::canonicalize(&root).unwrap();
    let matching = catalog::source_key_for(
        ExternalRuntime::Claude,
        "local",
        &cwd,
        "opaque-claude-id",
    )
    .unwrap();
    assert_eq!(
        bound.extra["externalAgent"]["sourceHost"],
        catalog::native_host_id()
    );
    assert_eq!(bound.extra["externalAgent"]["sourceKey"], matching);
    assert_ne!(
        matching,
        catalog::source_key_for(ExternalRuntime::Claude, "local", &cwd, "different-id")
            .unwrap()
    );
    assert_ne!(
        matching,
        catalog::source_key_for(
            ExternalRuntime::Claude,
            "another-tenant",
            &cwd,
            "opaque-claude-id"
        )
        .unwrap()
    );
    assert_ne!(
        matching,
        catalog::source_key_for(
            ExternalRuntime::Codex,
            "local",
            &cwd,
            "opaque-claude-id"
        )
        .unwrap()
    );
    state.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn nested_reconciliation_skips_ordinary_sessions_on_restart() {
    let root = std::env::temp_dir().join(format!(
        "neoism-acp-reconcile-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let db = root.join("agent.sqlite3");
    let state = AppState::open_database(&db).await.unwrap();
    let normal = crate::session_routes::create_session_in_directory(
        &state,
        root.to_str().unwrap(),
        neoism_agent_core::CreateSessionRequest {
            parent_id: None,
            title: None,
            agent: None,
            model: None,
            permission: None,
            workspace_id: None,
            external_provider: None,
            external_options: None,
        },
        BTreeMap::new(),
    )
    .await
    .unwrap();
    assert!(!normal.extra.contains_key("externalAgent"));
    events::reconcile_interrupted_nested_sessions(&state)
        .await
        .unwrap();
    state.shutdown().await.unwrap();
    drop(state);

    let reopened = AppState::open_database(&db).await.unwrap();
    assert!(reopened
        .inner
        .store
        .get_session(normal.id.as_str())
        .await
        .unwrap()
        .is_some());
    reopened.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}
