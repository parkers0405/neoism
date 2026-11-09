use super::*;
use axum::extract::{Query, State};

#[test]
fn neo_openapi_matches_persisted_contract() {
    let saved: Value = serde_json::from_str(include_str!("../../../openapi/v2.json")).unwrap();
    assert_eq!(crate::openapi::canonical_openapi(), saved);
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
async fn neo_workspace_identity_ignores_root_changes_but_isolates_tenants_and_workspaces() {
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
    async fn get(state: &AppState, path: &std::path::Path, claims: crate::caller::CallerClaims) -> SessionInfo {
        crate::session_routes::neo_session(State(state.clone()), Query(InstanceQuery {
            directory: Some(path.to_string_lossy().into_owned()),
        }), axum::http::HeaderMap::new(), Some(axum::Extension(claims))).await.unwrap().0
    }
    let first = get(&state, &a, claims.clone()).await;
    claims.directory_prefixes = vec![b.to_string_lossy().into_owned()];
    // Even at quota, retrieving an existing Neo remains idempotent.
    claims.max_sessions = Some(1);
    let moved = get(&state, &b, claims.clone()).await;
    assert_eq!(moved.id, first.id);
    assert_eq!(moved.directory, b.to_string_lossy());
    claims.max_sessions = None;
    claims.workspace_id = Some("workspace-b".into());
    let other_workspace = get(&state, &b, claims.clone()).await;
    assert_ne!(other_workspace.id, first.id);
    claims.workspace_id = Some("workspace-a".into());
    claims.tenant_id = "tenant-b".into();
    let other_tenant = get(&state, &b, claims).await;
    assert_ne!(other_tenant.id, first.id);
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

    let child = crate::session_actions::create_subtask_session(&state, &a, "implement", "Implement feature", "build", None).await.unwrap();
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
}