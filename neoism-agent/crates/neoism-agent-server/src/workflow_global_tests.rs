use super::*;
use neoism_agent_builtins::plugin::workflows::{WorkflowAction, WorkflowsHost};
use neoism_agent_plugin_api::{RouteRequest, RouteResponse};
use neoism_agent_service_api::StandardConfigSourceService;
use std::sync::Arc;

async fn call(
    state: &AppState,
    action: WorkflowAction,
    scope: Option<&str>,
    directory: Option<&FsPath>,
    body: Value,
    revision: Option<&str>,
) -> RouteResponse {
    let mut request = RouteRequest {
        actor: Some("operator".into()),
        workspace: directory.map(FsPath::to_path_buf),
        body,
        ..Default::default()
    };
    request
        .path
        .insert("workflow_id".into(), "global-flow".into());
    if let Some(scope) = scope {
        request.query.insert("scope".into(), vec![scope.into()]);
    }
    if let Some(revision) = revision {
        request.headers.insert("if-match".into(), revision.into());
    }
    crate::plugin_adapters::Workflows(state.clone())
        .execute(action, request)
        .await
        .unwrap()
}

#[tokio::test]
async fn installation_workflow_uses_configured_root_and_scoped_lifecycle() {
    let temp = std::env::temp_dir().join(format!(
        "agent-global-flow-{}",
        Id::ascending(IdKind::Event)
    ));
    let global = temp.join("custom-installation");
    let project = temp.join("unrelated-project");
    fs::create_dir_all(&project).unwrap();
    let mut services = crate::standard_services();
    services.config = Arc::new(StandardConfigSourceService::new(&global));
    let state = AppState::open_database_with_services_and_management(
        temp.join("state.db"),
        services,
        crate::ManagementPolicy::enabled(),
    )
    .await
    .unwrap();
    let definition = json!({"id": "global-flow", "name": "Global flow", "prompt": "Instructions", "active": false, "directory": project, "schedule": {"frequency": "daily", "time": "09:00"}});
    let created = call(
        &state,
        WorkflowAction::Create,
        Some("installation"),
        None,
        definition.clone(),
        None,
    )
    .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let path = global.join("workflows/global-flow.md");
    assert_eq!(created.body["sourcePath"], path.display().to_string());
    assert!(path.is_file());
    assert!(!project.join(".agent").exists());
    assert!(!global.join(".agent").exists());
    let context = installation_context(state.services()).unwrap();
    assert_eq!(
        workflow_discovery_roots(state.services(), &context),
        vec![global.clone()]
    );
    assert!(workflow_watch_paths(state.services(), &[context.clone()])
        .iter()
        .any(|(watched, _)| path.starts_with(watched)));
    assert!(state
        .inner
        .workflow_workspaces
        .read()
        .await
        .contains(&context));
    {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let watcher =
            install_workflow_watches(state.services(), &[context.clone()], tx).unwrap();
        fs::write(&path, fs::read(&path).unwrap()).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .is_some());
        drop(watcher);
    }
    assert!(workflow_source_is_managed(
        &context,
        &find_source(state.services(), &context, "global-flow")
            .await
            .unwrap()
    ));

    let listed = call(
        &state,
        WorkflowAction::List,
        Some("installation"),
        Some(&project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(listed.body["workflows"].as_array().unwrap().len(), 1);
    let activated = call(
        &state,
        WorkflowAction::Activate,
        Some("installation"),
        None,
        Value::Null,
        None,
    )
    .await;
    assert!(activated.body["active"].as_bool().unwrap());
    let paused = call(
        &state,
        WorkflowAction::Pause,
        Some("installation"),
        Some(&project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(paused.status, 200);
    let projection = state
        .inner
        .store
        .get_workflow(&activation_id(&context, "global-flow"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        workflow_execution_directory(state.services(), &projection).unwrap(),
        project.display().to_string()
    );
    let history = call(
        &state,
        WorkflowAction::History,
        Some("installation"),
        None,
        Value::Null,
        None,
    )
    .await;
    assert!(history.body["runs"].as_array().unwrap().is_empty());
    let updated = call(
        &state,
        WorkflowAction::Patch,
        Some("installation"),
        Some(&project),
        json!({"prompt": "Updated", "directory": null}),
        Some(created.body["revision"].as_str().unwrap()),
    )
    .await;
    assert_eq!(updated.status, 200, "{}", updated.body);
    assert!(fs::read_to_string(&path).unwrap().contains("Updated"));

    // Omitted and explicit workspace scopes retain the shipped .agent destination.
    let mut definition = definition;
    definition["id"] = json!("workspace-flow");
    let workspace_created = call(
        &state,
        WorkflowAction::Create,
        None,
        Some(&project),
        definition,
        None,
    )
    .await;
    assert_eq!(workspace_created.status, 201);
    assert!(project.join(".agent/workflows/workspace-flow.md").is_file());
    let deleted = call(
        &state,
        WorkflowAction::Delete,
        Some("installation"),
        None,
        Value::Null,
        Some(updated.body["revision"].as_str().unwrap()),
    )
    .await;
    assert_eq!(deleted.status, 204);
    assert!(!path.exists());
    assert!(project.join(".agent/workflows/workspace-flow.md").is_file());
    let listed = call(
        &state,
        WorkflowAction::List,
        Some("installation"),
        Some(&project),
        Value::Null,
        None,
    )
    .await;
    assert!(listed.body["workflows"].as_array().unwrap().is_empty());
    state.shutdown().await.unwrap();
    fs::remove_dir_all(temp).unwrap();
}

#[tokio::test]
async fn installation_http_routes_enforce_real_root_and_operator_boundaries() {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    let temp = std::env::temp_dir().join(format!("agent-global-route-{}", crate::slug()));
    let global = temp.join("configured-global");
    fs::create_dir_all(&global).unwrap();
    fs::write(
        global.join(neoism_agent_service_api::STANDARD_AGENT_CONFIG_FILENAME),
        r#"{"agent":{"global-only":{"description":"Global agent","mode":"primary"}}}"#,
    )
    .unwrap();
    let mut services = crate::standard_services();
    services.config = Arc::new(StandardConfigSourceService::new(&global));
    let state = AppState::open_database_with_services_and_management(
        temp.join("state.db"),
        services.clone(),
        crate::ManagementPolicy::enabled(),
    )
    .await
    .unwrap();
    let claims = crate::caller::CallerClaims {
        subject: "operator".into(),
        workspace_id: None,
        tenant_id: "local".into(),
        directory_prefixes: vec![global.display().to_string()],
        hosted: false,
        max_sessions: None,
        max_artifacts: None,
        max_artifact_bytes: None,
        artifact_retention_days: None,
        requests_per_minute: None,
        max_in_flight: None,
    };
    let definition = json!({"id":"http-global", "name":"HTTP global", "prompt":"Instructions", "active":false, "schedule":{"frequency":"daily"}});
    for (caller, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (
            Some(crate::caller::CallerClaims {
                directory_prefixes: vec![temp.join("project").display().to_string()],
                ..claims.clone()
            }),
            StatusCode::FORBIDDEN,
        ),
        (
            Some(crate::caller::CallerClaims {
                hosted: true,
                ..claims.clone()
            }),
            StatusCode::FORBIDDEN,
        ),
        (Some(claims.clone()), StatusCode::CREATED),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/v2/plugins/dev.neoism.workflows?scope=installation")
            .header("content-type", "application/json")
            .body(Body::from(definition.to_string()))
            .unwrap();
        if let Some(caller) = caller {
            request.extensions_mut().insert(caller);
        }
        let response = crate::app_router::app(state.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
    }
    assert!(global.join("workflows/http-global.md").is_file());
    for path in [
        "/v2/agents?scope=installation",
        "/v2/skills?scope=installation",
        "/v2/providers/configured?scope=installation",
    ] {
        let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
        request.extensions_mut().insert(claims.clone());
        let response = crate::app_router::app(state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        if path.starts_with("/v2/agents") {
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains("global-only"));
        }
    }
    let disabled = AppState::open_database_with_services_and_management(
        temp.join("disabled.db"),
        services,
        crate::ManagementPolicy::disabled(),
    )
    .await
    .unwrap();
    let mut request = Request::builder()
        .method("POST")
        .uri("/v2/plugins/dev.neoism.workflows?scope=installation")
        .header("content-type", "application/json")
        .body(Body::from(definition.to_string()))
        .unwrap();
    request.extensions_mut().insert(claims);
    let response = crate::app_router::app(disabled.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    disabled.shutdown().await.unwrap();
    state.shutdown().await.unwrap();
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn workflow_scope_is_explicit_and_workspace_compatible() {
    assert!(matches!(
        serde_json::from_value::<WorkflowQuery>(json!({}))
            .unwrap()
            .scope,
        WorkflowScope::Workspace
    ));
    assert!(matches!(
        serde_json::from_value::<WorkflowQuery>(json!({"scope":"installation"}))
            .unwrap()
            .scope,
        WorkflowScope::Installation
    ));
    assert!(
        serde_json::from_value::<WorkflowQuery>(json!({"scope":"global-guess"})).is_err()
    );
}
