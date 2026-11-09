use super::*;
use axum::extract::{Query, State};

#[test]
fn neo_openapi_matches_persisted_contract() {
    let saved: Value = serde_json::from_str(include_str!("../../../openapi/v2.json")).unwrap();
    assert_eq!(crate::openapi::canonical_openapi(), saved);
    use sha2::Digest;
    let fingerprint = format!("{:x}", sha2::Sha256::digest(serde_json::to_vec(&crate::openapi::canonical_openapi()).unwrap()));
    assert_eq!(fingerprint, include_str!("../../../openapi/v2.sha256").trim());
}

#[test]
fn neo_tool_surface_is_session_specific() {
    let mut session = store_test_session(&neoism_agent_core::new_session_id(), 1);
    assert!(!crate::tool_selection::tool_allowed_for_session("neo_task", &session));
    assert!(crate::tool_selection::tool_allowed_for_session("task", &session));
    assert!(crate::tool_selection::tool_allowed_for_session("bash", &session));
    session.extra.insert("sessionRole".into(), json!("neo"));
    for tool in ["neo_task", "task_result", "stop_task", "question", "memory"] {
        assert!(crate::tool_selection::tool_allowed_for_session(tool, &session));
    }
    for tool in ["task", "bash", "apply_patch", "read", "edit", "write"] {
        assert!(!crate::tool_selection::tool_allowed_for_session(tool, &session));
    }
}

#[tokio::test]
async fn neo_root_ignores_directories_and_rejects_workspace_and_peer_capabilities() {
    let root = std::env::temp_dir().join(format!("neo-scope-{}", Id::ascending(IdKind::Event)));
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let state = AppState::open_database(root.join("agent.db")).await.unwrap();
    let mut claims = crate::caller::CallerClaims {
        subject: "user".into(), workspace_id: Some("workspace-a".into()), tenant_id: "tenant-a".into(),
        directory_prefixes: vec![a.to_string_lossy().into_owned()], hosted: true,
        max_sessions: None, max_artifacts: None, max_artifact_bytes: None, artifact_retention_days: None,
        requests_per_minute: None, max_in_flight: None, resolved: None, worker: None,
    };
    async fn get(state: &AppState, path: &std::path::Path) -> SessionInfo {
        crate::session_routes::neo_session(State(state.clone()), Query(InstanceQuery {
            directory: Some(path.to_string_lossy().into_owned()),
        }), axum::http::HeaderMap::new(), None).await.unwrap().0
    }
    let first = get(&state, &a).await;
    claims.directory_prefixes = vec![b.to_string_lossy().into_owned()];
    // Even at quota, retrieving an existing Neo remains idempotent.
    claims.max_sessions = Some(1);
    let moved = get(&state, &b).await;
    assert_eq!(moved.id, first.id);
    assert_eq!(moved.directory, first.directory);
    assert!(moved.project_id.is_empty());
    assert!(moved.workspace_id.is_none());
    assert_eq!(moved.extra["sessionScope"], "root");
    assert!(!crate::caller::allows_session(&claims, &moved));
    assert!(crate::neo::session(&state, Some(&claims)).await.is_err());
    claims.max_sessions = None;
    claims.workspace_id = Some("workspace-b".into());
    assert!(crate::neo::session(&state, Some(&claims)).await.is_err());
    claims.workspace_id = Some("workspace-a".into());
    claims.tenant_id = "tenant-b".into();
    assert!(crate::neo::session(&state, Some(&claims)).await.is_err());
    claims.workspace_id = None;
    claims.tenant_id = "local".into();
    claims.hosted = false;
    claims.directory_prefixes.clear();
    claims.subject = "daemon:local-agent".into();
    assert!(!crate::caller::allows_session(&claims, &first));
    assert!(crate::neo::session(&state, Some(&claims)).await.is_err());
    claims.subject = "local-operator".into();
    assert!(crate::caller::allows_session(&claims, &first));
    let without_workspace = crate::neo::session(&state, Some(&claims)).await.unwrap();
    assert_eq!(without_workspace.id, first.id);
    // Invalid/nonexistent directory hints are not even admitted/discovered.
    assert_eq!(get(&state, &root.join("does-not-exist")).await.id, first.id);
    let mut forged = crate::session_transfer::export_session(&state, first.id.as_str()).await.unwrap();
    forged.session = store_test_session(&neoism_agent_core::new_session_id(), now_millis());
    forged.session.extra.insert("neoSessionId".into(), json!(first.id));
    forged.session.extra.insert("subtaskCompletion".into(), json!({"pending":true,"status":"completed"}));
    claims.subject = "paired-device".into();
    claims.hosted = true;
    claims.workspace_id = Some("workspace-a".into());
    claims.directory_prefixes = vec![a.to_string_lossy().into_owned()];
    let imported = crate::session_import_route::session_import(State(state.clone()), Some(axum::Extension(claims)),
        axum::Json(crate::session_import_route::ImportSessionRequest { bundle: forged, target_workspace_root: a.to_string_lossy().into_owned() })).await.unwrap().0;
    let imported = state.inner.store.get_session(&imported.session_id).await.unwrap().unwrap();
    assert!(!imported.extra.contains_key("neoSessionId"));
    assert!(!imported.extra.contains_key("subtaskCompletion"));
}

#[tokio::test]
async fn neo_root_adopts_owned_legacy_history_without_adopting_foreign_tenants() {
    let root = std::env::temp_dir().join(format!("neo-adopt-{}", Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(&root).unwrap();
    let state = AppState::open_database(root.join("agent.db")).await.unwrap();
    let mut legacy = store_test_session(&neoism_agent_core::new_session_id(), 2);
    legacy.directory = root.to_string_lossy().into_owned();
    legacy.extra.insert("sessionRole".into(), json!("neo"));
    legacy.extra.insert("neoWorkspace".into(), json!("old-workspace-key"));
    legacy.extra.insert(crate::caller::CREATED_BY_EXTRA_KEY.into(), json!("local-operator"));
    legacy.model = model_ref_from_config("neoism/stub");
    // Simulate the pre-root implementation, whose Neo could be associated
    // with a locally hosted workspace before the new role exclusion existed.
    legacy.extra.remove("sessionRole");
    state.inner.store.insert_session(&legacy).await.unwrap();
    state.inner.store.associate_host_directory(&legacy.directory, "old-workspace").await.unwrap();
    legacy.extra.insert("sessionRole".into(), json!("neo"));
    state.inner.store.update_session(&legacy).await.unwrap();
    state.inner.store.append_message(legacy.id.as_str(), &store_test_message(&legacy.id, 3, "remember my birthday")).await.unwrap();
    let mut foreign = legacy.clone();
    foreign.id = neoism_agent_core::new_session_id();
    foreign.time.created = 1;
    foreign.extra.insert("neoWorkspace".into(), json!("foreign-key"));
    foreign.extra.insert(crate::caller::TENANT_EXTRA_KEY.into(), json!("foreign-tenant"));
    state.inner.store.insert_session(&foreign).await.unwrap();
    let adopted = crate::neo::session(&state, None).await.unwrap();
    assert_eq!(adopted.id, legacy.id);
    assert!(adopted.project_id.is_empty());
    assert!(adopted.workspace_id.is_none());
    assert!(!adopted.extra.contains_key("neoWorkspace"));
    assert_eq!(adopted.model.unwrap().id, "stub");
    let hydrated = state.inner.store.get_session(adopted.id.as_str()).await.unwrap().unwrap();
    assert!(hydrated.workspace_id.is_none());
    assert_eq!(crate::caller::session_tenant(&hydrated), "local");
    let messages = state.inner.store.list_messages(adopted.id.as_str()).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert!(serde_json::to_string(&messages).unwrap().contains("remember my birthday"));
    assert_eq!(crate::caller::session_tenant(&state.inner.store.get_session(foreign.id.as_str()).await.unwrap().unwrap()), "foreign-tenant");
}

#[tokio::test]
async fn neo_task_explicit_workspaces_complete_into_one_root_outbox() {
    use neoism_agent_service_api::workspace_management::{CreateWorkspaceRequest, StandaloneWorkspaceManagementService};
    let root = std::env::temp_dir().join(format!("neo-targets-{}", Id::ascending(IdKind::Event)));
    let a = root.join("project-a");
    let b = root.join("project-b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let mut services = crate::standard_services();
    services.workspace_management = std::sync::Arc::new(StandaloneWorkspaceManagementService::new(root.join("workspaces.json"), root.join("managed")));
    for (id, directory) in [("project-a", &a), ("project-b", &b)] {
        services.workspace_management.create_workspace(CreateWorkspaceRequest {
            id: Some(id.into()), name: None, root: directory.clone(), create_directory: false,
        }).unwrap();
    }
    let state = AppState::open_database_with_services(root.join("agent.db"), services).await.unwrap();
    let mut neo = crate::neo::session(&state, None).await.unwrap();
    neo.model = model_ref_from_config("neoism/stub");
    state.inner.store.update_session(&neo).await.unwrap();
    assert!(crate::neo::delegate(&state, &neo, &json!({"workspace_id":"project-a", "directory":b})).await.is_err());
    for (id, directory) in [("project-a", &a), ("project-b", &b)] {
        let snapshot = state.plugin_snapshot(&neo.directory).await;
        let result = crate::tool_runtime::execute_tool_call_in_generation(&state, &neo.id, &Id::ascending(IdKind::Message),
            &neo.directory, vec![neoism_agent_core::PermissionRule { permission: "neo_task".into(), pattern: "*".into(), action: neoism_agent_core::PermissionAction::Allow }], "neo-task-test", "neo_task",
            json!({"description":"Investigate", "prompt":"Say hello", "workspace_id":id}), snapshot).await.unwrap();
        let metadata = result.metadata.unwrap();
        let session_id = metadata["sessionId"].as_str().unwrap().to_owned();
        let child = state.inner.store.get_session(&session_id).await.unwrap().unwrap();
        assert!(child.parent_id.is_none());
        assert_eq!(child.directory, directory.to_string_lossy());
        assert_eq!(child.agent.as_deref(), Some("build"));
        assert!(child.workspace_id.is_some());
        assert_ne!(crate::caller::session_tenant(&child), crate::caller::session_tenant(&neo));
        assert_eq!(child.task_parent_id(), Some(neo.id.clone()));
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let messages = state.inner.store.list_messages(neo.id.as_str()).await.unwrap();
                if serde_json::to_string(&messages).unwrap().contains(&session_id)
                    && messages.iter().any(|m| matches!(&m.info, MessageInfo::Assistant(a) if a.time.completed.is_some())) { break; }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.expect("delegate completion in another workspace must wake the root Neo");
    }
    assert_eq!(crate::neo::session(&state, None).await.unwrap().id, neo.id);
    assert_eq!(state.inner.store.list_task_sessions(neo.id.as_str()).await.unwrap().len(), 2);
}

#[tokio::test]
async fn neo_singleton_survives_concurrency_restart_and_catalog_projection() {
    let root = std::env::temp_dir().join(format!("neo-{}", Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("agent.db");
    let state = AppState::open_database(database.clone()).await.unwrap();
    let get = || crate::session_routes::neo_session(
        State(state.clone()), Query(InstanceQuery { directory: Some(root.to_string_lossy().into_owned()) }),
        axum::http::HeaderMap::new(), None,
    );
    let (a, b) = tokio::join!(get(), get());
    let a = a.unwrap().0;
    assert_eq!(a.id, b.unwrap().0.id);
    assert!(a.is_neo());
    assert_eq!(a.agent.as_deref(), Some("neo"));
    let reopened = AppState::open_database(database).await.unwrap();
    let again = crate::session_routes::neo_session(
        State(reopened.clone()), Query(InstanceQuery { directory: Some(root.to_string_lossy().into_owned()) }),
        axum::http::HeaderMap::new(), None,
    ).await.unwrap().0;
    assert_eq!(a.id, again.id);
    // The event sequencer is process-owned; keep mutations on one live writer
    // after proving a second server can resolve the persisted singleton.
    let reopened = state.clone();
    assert!(crate::session_routes::session_delete(State(reopened.clone()), axum::extract::Path(a.id.to_string())).await.is_err());
    for update in [json!({"agent":"build"}), json!({"title":"Renamed"})] {
        assert!(crate::session_routes::session_update(State(reopened.clone()), axum::extract::Path(a.id.to_string()), None,
            axum::Json(serde_json::from_value(update).unwrap())).await.is_err());
    }
    let updated = crate::session_routes::session_update(State(reopened.clone()), axum::extract::Path(a.id.to_string()), None,
        axum::Json(serde_json::from_value(json!({"agent":"neo", "model":{"providerId":"neoism", "id":"stub", "connectionId":"connection", "variant":"high"}})).unwrap())).await.unwrap().0;
    assert_eq!(updated.agent.as_deref(), Some("neo"));
    let model = updated.model.unwrap();
    assert_eq!(model.connection_id.as_deref(), Some("connection"));
    assert_eq!(model.variant.as_deref(), Some("high"));
    let prompt = crate::session_prompt::append_prompt(&state, a.id.as_str(),
        serde_json::from_value(json!({"agent":"build", "model":{"providerId":"neoism", "modelId":"stub"}, "parts":[{"type":"text", "text":"Coordinate work"}]})).unwrap(), false).await.unwrap();
    assert_eq!(serde_json::to_value(prompt.info).unwrap()["agent"], "neo", "restored or stale frontend selections cannot change Neo routing");

    assert!(crate::neo::delegate(&state, &a, &json!({"description":"work", "prompt":"implement"})).await.is_err());
    let child = crate::neo::delegate(&state, &a, &json!({"description":"work", "prompt":"implement", "directory":root})).await.unwrap();
    assert_eq!(child.directory, root.to_string_lossy());
    assert_ne!(child.directory, a.directory);
    assert!(!child.project_id.is_empty());
    assert_eq!(crate::neo::delegate(&state, &a, &json!({"session_id":child.id})).await.unwrap().id, child.id);
    assert!(child.parent_id.is_none());
    assert_eq!(child.agent.as_deref(), Some("build"));
    assert_eq!(child.task_parent_id(), Some(a.id.clone()));
    assert!(!child.is_neo());
    crate::tool_runtime::ensure_child_task_belongs_to_parent(&reopened, &a, &child).await.unwrap();
    assert_eq!(reopened.inner.store.list_task_sessions(a.id.as_str()).await.unwrap().len(), 1);
    assert!(reopened.inner.store.list_child_sessions(a.id.as_str()).await.unwrap().is_empty());
    let snapshot = reopened.plugin_snapshot(&a.directory).await;
    let status = crate::plugins::subagents::task_result_tool(&reopened, &snapshot, &a.id, json!({})).await.unwrap();
    let tasks = status.metadata.unwrap();
    assert_eq!(tasks["tasks"][0]["sessionId"], child.id.as_str());
    assert_eq!(tasks["tasks"][0]["nested"], false);
    let mut stranger = child.clone();
    stranger.extra.remove("neoSessionId");
    assert!(crate::tool_runtime::ensure_child_task_belongs_to_parent(&state, &a, &stranger).await.is_err());
    let catalog = crate::catalog_activity::CatalogProjection::load(&reopened).await.unwrap()
        .baseline(&reopened, None, None).await.unwrap();
    assert!(catalog.iter().all(|event| event.properties.get("sessionID").and_then(Value::as_str) != Some(a.id.as_str())));
    assert!(catalog.iter().any(|event| event.properties.get("sessionID").and_then(Value::as_str) == Some(child.id.as_str())));
    reopened.start_session_list_backfill();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !reopened.inner.store.session_list_index_ready().await.unwrap() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    for query in [json!({"roots":"true"}), json!({})] {
        let page = crate::v2_routes::v2_session_list(State(reopened.clone()), Query(serde_json::from_value(query).unwrap()), None).await.unwrap().0;
        let value = serde_json::to_value(page).unwrap();
        let ids = value["items"].as_array().unwrap().iter().filter_map(|item| item["id"].as_str()).collect::<Vec<_>>();
        assert!(!ids.contains(&a.id.as_str()));
        assert!(ids.contains(&child.id.as_str()));
    }
    let direct = crate::session_routes::session_get(State(reopened.clone()), axum::extract::Path(a.id.to_string())).await.unwrap().0;
    assert_eq!(serde_json::to_value(direct).unwrap()["id"], a.id.as_str());
    let mut bundle = crate::session_transfer::export_session(&state, a.id.as_str()).await.unwrap();
    assert!(crate::session_transfer::import_session(&state, bundle.clone(), &root).await.is_err());
    bundle.session.extra.clear();
    bundle.session.agent = Some("build".into());
    assert!(crate::session_transfer::import_session(&state, bundle, &root).await.is_err(), "ordinary-looking workspace import must not overwrite the root ID");
}