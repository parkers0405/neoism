use super::*;
use neoism_agent_service_api::{
    ActorType, CreateWorkspaceRequest, ExecutionPolicy,
    StandaloneWorkspaceManagementService, TenantQuotas, WorkspaceManagementService,
    WorkspaceWorkerBinding, WorkspaceWorkerBootstrap, WorkspaceWorkerCredentialClaims,
    WorkspaceWorkerMcpCredentialStore, WorkspaceWorkerProviderCredentialStore,
    WorkspaceWorkerSigningKey, WorkspaceWorkerTenantResolver,
};
use std::sync::Arc;

fn binding(
    root: &std::path::Path,
    key: WorkspaceWorkerSigningKey,
    generation: u64,
) -> WorkspaceWorkerBinding {
    WorkspaceWorkerBinding::new(
        WorkspaceWorkerBootstrap {
            version: 1,
            tenant_id: "company-a".into(),
            workspace_id: "workspace-a".into(),
            root: std::fs::canonicalize(root).unwrap(),
            runtime_id: format!("runtime-{generation}"),
            runtime_generation: generation,
            expires_at: (now_millis() / 1000) as i64 + 3600,
        },
        key.verification_key(),
    )
    .unwrap()
}

fn services(
    root: &std::path::Path,
    binding: &WorkspaceWorkerBinding,
) -> neoism_agent_service_api::AgentServices {
    let management = StandaloneWorkspaceManagementService::new(
        root.join("registry.json"),
        root.join("clones"),
    );
    if management.get_workspace("workspace-a").unwrap().is_none() {
        management
            .create_workspace(CreateWorkspaceRequest {
                id: Some("workspace-a".into()),
                name: Some("Cloud workspace".into()),
                root: binding.root().to_owned(),
                create_directory: false,
            })
            .unwrap();
    }
    crate::standard_services()
        .with_workspace_management(Arc::new(management))
        .with_tenant_resolver(Arc::new(WorkspaceWorkerTenantResolver::new(
            binding.clone(),
        )))
        .with_provider_credentials(Arc::new(
            WorkspaceWorkerProviderCredentialStore::new(
                binding.clone(),
                root.join("provider-auth.json"),
            )
            .unwrap(),
        ))
        .with_mcp_credentials(Arc::new(
            WorkspaceWorkerMcpCredentialStore::new(
                binding.clone(),
                root.join("mcp-auth.json"),
            )
            .unwrap(),
        ))
        .for_workspace_worker(binding.clone())
}

fn signed(
    key: &WorkspaceWorkerSigningKey,
    binding: &WorkspaceWorkerBinding,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Request<Body> {
    signed_for(
        key,
        binding,
        binding.root(),
        &["agent:use"],
        method,
        path,
        body,
    )
}

fn signed_for(
    key: &WorkspaceWorkerSigningKey,
    binding: &WorkspaceWorkerBinding,
    prefix: &std::path::Path,
    scopes: &[&str],
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Request<Body> {
    let now = (now_millis() / 1000) as i64;
    let token = key
        .issue(&WorkspaceWorkerCredentialClaims {
            version: 1,
            tenant_id: binding.tenant_id().into(),
            workspace_id: binding.workspace_id().into(),
            runtime_id: binding.runtime_id().into(),
            runtime_generation: binding.runtime_generation(),
            subject: "member-a".into(),
            actor_type: ActorType::Human,
            directory_prefix: prefix.to_owned(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            quotas: TenantQuotas::default(),
            issued_at: now,
            expires_at: now + 60,
        })
        .unwrap();
    let mut req = request(method, path, body);
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    req
}

#[tokio::test]
async fn worker_browser_mcp_callback_is_state_bound_without_bearer_or_directory_input() {
    use neoism_agent_service_api::{CredentialScope, McpConnectionRef, McpOAuthAttempt};
    let root = std::env::temp_dir().join(format!("worker-mcp-callback-{}", neoism_agent_core::new_session_id()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(workspace.join(".agent")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let token_url = format!("http://{}/token", listener.local_addr().unwrap());
    let token_server = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().route("/token", axum::routing::post(|body: String| async move {
            let form: std::collections::HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
            assert_eq!(form["code_verifier"], "private-pkce-verifier");
            assert_eq!(form["redirect_uri"], "https://gateway.example/workspaces/workspace-a/callback");
            axum::Json(json!({"access_token": "worker-secret", "token_type": "Bearer"}))
        }))).await.unwrap();
    });
    std::fs::write(workspace.join(".agent/agent.json"), serde_json::to_vec(&json!({
        "mcp": {"remote": {"type": "remote", "url": "https://mcp.example/mcp", "oauth": {
            "clientId": "client", "authorizationUrl": "https://auth.example/authorize", "tokenUrl": token_url
        }}}
    })).unwrap()).unwrap();
    let key = WorkspaceWorkerSigningKey::new([38u8; 32]).unwrap();
    let binding = binding(&workspace, key.clone(), 1);
    let services = services(&root, &binding);
    let store = services.mcp_credentials.clone();
    let scope = CredentialScope { tenant_id: binding.tenant_id().into(), workspace_id: Some(binding.workspace_id().into()) };
    let connection = McpConnectionRef { connection_id: "remote".into(), server_url: "https://mcp.example/mcp".into() };
    let attempt = McpOAuthAttempt {
        scope: scope.clone(), connection: connection.clone(), state: "browser-state".into(),
        code_verifier: "private-pkce-verifier".into(),
        redirect_uri: "https://gateway.example/workspaces/workspace-a/callback".into(),
        directory: workspace.to_string_lossy().into_owned(), expires_at: (now_millis() / 1000) as u64 + 60,
    };
    store.put_attempt(attempt.clone()).await.unwrap();
    let state = AppState::open_database_with_services(root.join("agent.db"), services).await.unwrap();
    let app = crate::app_router::app(state);
    for scopes in [&["agent:use"][..], &["workspace:admin"][..]] {
        let response = app.clone().oneshot(signed_for(
            &key, &binding, binding.root(), scopes, Method::GET,
            "/v2/providers/openai/auth", None,
        )).await.unwrap();
        assert_ne!(response.status(), StatusCode::FORBIDDEN);
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(app.clone().oneshot(signed_for(
        &key, &binding, binding.root(), &["agent:read"], Method::GET,
        "/v2/providers/openai/auth", None,
    )).await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(app.clone().oneshot(signed(
        &key, &binding, Method::GET, "/v2/identity", None,
    )).await.unwrap().status(), StatusCode::FORBIDDEN);
    let path = "/v2/plugins/dev.neoism.mcp/remote/auth/callback?code=code&state=browser-state";
    let response = app.clone().oneshot(request(Method::GET, path, None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(store.get(&scope, &connection).await.unwrap().unwrap().tokens.unwrap().access_token, "worker-secret");
    assert!(store.get(&CredentialScope::local(), &connection).await.is_err());
    assert_eq!(app.clone().oneshot(request(Method::GET, path, None)).await.unwrap().status(), StatusCode::BAD_REQUEST);
    store.put_attempt(attempt).await.unwrap();
    // A forged directory must be rejected before plugin generation/config IO.
    let response = app.clone().oneshot(request(Method::GET, &format!("{path}&directory=/outside-worker"), None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(app.oneshot(request(Method::GET, "/v2/plugins/dev.neoism.mcp/remote/auth/callback?code=code&state=unknown", None)).await.unwrap().status(), StatusCode::BAD_REQUEST);
    token_server.abort();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn worker_http_shares_one_root_and_restores_logical_sessions() {
    let root = std::env::temp_dir().join(format!(
        "neoism-worker-http-{}",
        neoism_agent_core::new_session_id()
    ));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let key = WorkspaceWorkerSigningKey::new([7u8; 32]).unwrap();
    let original = binding(&workspace, key.clone(), 1);
    let state = AppState::open_database_with_services(
        root.join("agent.db"),
        services(&root, &original),
    )
    .await
    .unwrap();
    let app = crate::app_router::app(state.clone());
    assert_eq!(
        app.clone()
            .oneshot(request(Method::GET, "/v2/runtime", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let runtime: Value = response_json(
        app.clone()
            .oneshot(signed(&key, &original, Method::GET, "/v2/runtime", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(runtime["deployment"], "workspace-worker");
    assert_eq!(runtime["worker"]["runtimeGeneration"], 1);
    assert_eq!(runtime["worker"]["root"], original.root().to_string_lossy().as_ref());
    assert_eq!(runtime["worker"]["version"], 1);
    assert!(runtime["worker"].get("key").is_none());
    assert_eq!(
        app.clone()
            .oneshot(signed_for(
                &key,
                &original,
                original.root(),
                &[],
                Method::GET,
                "/v2/runtime",
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.clone()
            .oneshot(signed_for(
                &key,
                &original,
                original.root(),
                &["agent:read"],
                Method::POST,
                "/v2/sessions",
                Some(json!({}))
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let narrow = workspace.join("narrow");
    std::fs::create_dir_all(&narrow).unwrap();
    assert_eq!(
        app.clone()
            .oneshot(signed_for(
                &key,
                &original,
                &narrow,
                &["agent:read"],
                Method::GET,
                "/v2/runtime",
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone()
            .oneshot(signed_for(
                &key,
                &original,
                &narrow,
                &["agent:read"],
                Method::GET,
                "/v2/events",
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    let mut sessions = Vec::new();
    for agent in ["build", "explore"] {
        let response = app
            .clone()
            .oneshot(signed(
                &key,
                &original,
                Method::POST,
                "/v2/sessions",
                Some(json!({"agent": agent})),
            ))
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        let session: SessionInfo = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(session.workspace_id.as_deref(), Some("workspace-a"));
        assert_eq!(session.directory, workspace.to_string_lossy());
        assert_eq!(
            crate::caller::session_execution_policy(state.services(), &session),
            ExecutionPolicy::NativeLocal
        );
        assert!(crate::tool::ToolContext::new(&workspace)
            .with_state(Some(state.clone()))
            .with_session_id(Some(session.id.to_string()))
            .with_session_scope()
            .await
            .unwrap()
            .assert_native_execution()
            .await
            .is_ok());
        sessions.push(session);
    }
    state
        .put_artifact_blob("company-a", "worker-artifact", b"cloud output")
        .await
        .unwrap();
    assert_eq!(
        state
            .get_artifact_blob("company-a", "worker-artifact")
            .await
            .unwrap()
            .as_deref(),
        Some(b"cloud output".as_slice())
    );
    assert!(state
        .get_artifact_blob("company-b", "worker-artifact")
        .await
        .is_err());
    let path = format!("/v2/sessions?directory={}", root.to_string_lossy());
    assert_eq!(
        app.clone()
            .oneshot(signed(&key, &original, Method::GET, &path, None))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    state.shutdown().await.unwrap();
    drop(app);
    drop(state);

    let next_key = WorkspaceWorkerSigningKey::new([8u8; 32]).unwrap();
    let replacement = binding(&workspace, next_key.clone(), 2);
    let state = AppState::open_database_with_services(
        root.join("agent.db"),
        services(&root, &replacement),
    )
    .await
    .unwrap();
    let app = crate::app_router::app(state.clone());
    assert_eq!(
        app.clone()
            .oneshot(signed(&key, &original, Method::GET, "/v2/runtime", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let restored: SessionInfo = response_json(
        app.clone()
            .oneshot(signed(
                &next_key,
                &replacement,
                Method::GET,
                &format!("/v2/sessions/{}", sessions[0].id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(restored.id, sessions[0].id);
    assert_eq!(
        crate::caller::session_execution_policy(state.services(), &restored),
        ExecutionPolicy::NativeLocal
    );
    state.shutdown().await.unwrap();
    drop(app);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
