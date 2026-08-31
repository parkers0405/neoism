use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use axum::{
    extract::State,
    http::{HeaderMap as AxumHeaderMap, StatusCode as AxumStatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use neoism_agent_core::{Id, IdKind, McpOAuthConfig, McpOAuthSetting};
use serde_json::json;
use tokio::sync::oneshot;

use crate::mcp_auth::McpAuthTokens;

use super::mcp_runtime::MCP_PROTOCOL_VERSION;
use super::*;

#[test]
fn standalone_server_services_inject_the_local_mcp_store() {
    let services = crate::standard_services();
    assert!(!services.mcp_credentials.supports_hosted_scopes());
}

struct FakeBuiltinMcp;

impl neoism_agent_service_api::BuiltinMcpService for FakeBuiltinMcp {
    fn id(&self) -> &str { "fake-service" }

    fn tools(&self) -> Vec<neoism_agent_service_api::BuiltinMcpTool> {
        vec![neoism_agent_service_api::BuiltinMcpTool {
            name: "fake.call".to_string(),
            description: Some("Call fake service".to_string()),
            input_schema: json!({"type":"object"}),
            annotations: None,
        }]
    }

    fn call_tool(&self, _working_directory: &std::path::Path, tool: &str, _arguments: Value) -> Result<neoism_agent_service_api::BuiltinMcpCallResult, neoism_agent_service_api::ServiceError> {
        if tool != "fake.call" {
            return Err(neoism_agent_service_api::ServiceError::new("unknown fake tool"));
        }
        Ok(neoism_agent_service_api::BuiltinMcpCallResult {
            content: vec![neoism_agent_service_api::BuiltinMcpContent::Text { text: "fake result".to_string(), annotations: None }],
            is_error: None,
        })
    }
}

#[tokio::test]
async fn injected_builtin_registry_is_discoverable_while_absent_services_are_uncallable() {
    let root = temp_dir("builtin-registry");
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    let absent = crate::state::AppState::open_database_with_services(
        root.join("absent.db"),
        crate::standard_services(),
    ).await.unwrap();
    assert!(!configured_servers(root.to_str().unwrap(), Some(&absent)).unwrap().contains_key("fake-service"));
    let error = tools_with_state(root.to_str().unwrap(), "fake-service", &store, absent).await.unwrap_err();
    assert!(error.to_string().contains("not configured"));

    let services = crate::standard_services()
        .with_builtin_mcp(Arc::new(FakeBuiltinMcp));
    let state = crate::state::AppState::open_database_with_services(root.join("present.db"), services).await.unwrap();
    let catalog = catalog_with_state(root.to_str().unwrap(), &store, Some(&state)).await.unwrap();
    assert!(matches!(catalog["fake-service"].status, McpStatus::Connected));
    assert_eq!(tools_with_state(root.to_str().unwrap(), "fake-service", &store, state.clone()).await.unwrap()[0].name, "fake.call");
    let result = call_tool_with_state(root.to_str().unwrap(), "fake-service", "fake.call", json!({}), &store, state.clone()).await.unwrap();
    assert_eq!(tool_result_text(&result), "fake result");

    fs::write(
        root.join(".agent/agent.json"),
        r#"{"mcp":{"fake-service":{"type":"local","command":["builtin","fake-service"],"enabled":false}}}"#,
    )
    .unwrap();
    let catalog = catalog_with_state(root.to_str().unwrap(), &store, Some(&state)).await.unwrap();
    assert!(matches!(catalog["fake-service"].status, McpStatus::Disabled));
    let error = tools_with_state(root.to_str().unwrap(), "fake-service", &store, state).await.unwrap_err();
    assert!(error.to_string().contains("disabled"));
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn status_marks_remote_oauth_without_tokens_as_needs_auth() {
    let store = McpAuthStore::new(temp_auth_path("status"));
    let mut config = BTreeMap::new();
    config.insert(
        "remote".to_string(),
        McpConfig::Remote {
            url: "https://example.com/mcp".to_string(),
            enabled: Some(true),
            headers: None,
            oauth: Some(McpOAuthSetting::Config(McpOAuthConfig {
                client_id: Some("client".to_string()),
                client_secret: None,
                scope: None,
                redirect_uri: None,
                authorization_url: None,
                token_url: None,
                registration_url: None,
            })),
            timeout: None,
        },
    );

    let status = status_for_config(&config, &store).await;
    assert!(matches!(status["remote"], McpStatus::NeedsAuth));
}

#[test]
fn env_placeholders_expand_in_mcp_maps() {
    let home = std::env::var("HOME").unwrap_or_default();
    let map = BTreeMap::from([
        ("Authorization".to_string(), "Bearer {env:HOME}".to_string()),
        (
            "Missing".to_string(),
            "value-{env:NEOISM_AGENT_TEST_MISSING_ENV}".to_string(),
        ),
        ("Literal".to_string(), "no placeholder".to_string()),
    ]);

    let expanded = expand_env_map(Some(&map)).expect("map should expand");

    assert_eq!(expanded["Authorization"], format!("Bearer {home}"));
    assert_eq!(expanded["Missing"], "value-");
    assert_eq!(expanded["Literal"], "no placeholder");
}

#[tokio::test]
async fn auth_start_builds_authorization_url_and_persists_transient_fields() {
    let root = temp_dir("auth-start");
    fs::write(
        root.join(".agent/agent.json"),
        r#"{
              "mcp": {
                "remote": {
                  "type": "remote",
                  "url": "https://example.com/mcp",
                  "oauth": {
                    "clientId": "client",
                    "scope": "tools read",
                    "redirectUri": "http://127.0.0.1/callback",
                    "authorizationUrl": "https://auth.example.com/oauth/authorize"
                  }
                }
              }
            }"#,
    )
    .unwrap();
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    let directory = root.to_str().unwrap();

    let response = auth_start(&crate::standard_services(), directory, "remote", &store).await.unwrap();

    assert!(response
        .authorization_url
        .starts_with("https://auth.example.com/oauth/authorize?"));
    assert!(response.authorization_url.contains("client_id=client"));
    assert!(response.authorization_url.contains("scope=tools+read"));
    assert!(response
        .authorization_url
        .contains("code_challenge_method=S256"));
    let attempt = store.consume_attempt(&response.oauth_state, true).await.unwrap().unwrap();
    assert_eq!(attempt.connection.server_url, "https://example.com/mcp");
    assert_eq!(attempt.directory, directory);
    assert_eq!(attempt.redirect_uri, "http://127.0.0.1/callback");
    assert!(!attempt.code_verifier.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn origin_extracts_url_origin_without_path() {
    assert_eq!(
        origin("https://example.com/api/mcp").as_deref(),
        Some("https://example.com")
    );
    assert_eq!(
        origin("http://localhost:3000/mcp").as_deref(),
        Some("http://localhost:3000")
    );
}

#[test]
fn default_oauth_redirect_uses_the_active_agent_server() {
    let previous = std::env::var("NEOISM_SERVER").ok();
    std::env::set_var("NEOISM_SERVER", "http://127.0.0.1:39319/");
    assert_eq!(
        super::mcp_oauth::redirect_uri_for_test("webflow", &Default::default()),
        "http://127.0.0.1:39319/v2/plugins/dev.neoism.mcp/webflow/auth/callback"
    );
    match previous {
        Some(value) => std::env::set_var("NEOISM_SERVER", value),
        None => std::env::remove_var("NEOISM_SERVER"),
    }
}

#[tokio::test]
async fn local_stdio_runtime_lists_and_calls_tools() {
    let root = temp_dir("stdio-runtime");
    let server = mock_mcp_server(&root);
    fs::write(
        root.join(".agent/agent.json"),
        format!(
            r#"{{
                  "mcp": {{
                    "mock": {{
                      "type": "local",
                      "command": ["{}"],
                      "timeout": 2000
                    }}
                  }}
                }}"#,
            server.display()
        ),
    )
    .unwrap();
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    let directory = root.to_str().unwrap();
    let state = test_state(&root).await;

    let connected = connect(directory, "mock", &store, state.clone()).await.unwrap();
    assert!(matches!(connected, McpStatus::Connected));
    assert!(matches!(
        status(directory, &store, &state).await.unwrap()["mock"],
        McpStatus::Connected
    ));

    let tools = tools(directory, "mock", &store, state.clone()).await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].client, "mock");
    assert_eq!(tool_runtime_id("mock", "echo-tool"), "mcp__mock__echo_tool");

    let resources = resources(directory, "mock", &store, state.clone()).await.unwrap();
    assert_eq!(resources[0].uri, "file:///tmp/example.txt");

    let prompts = prompts(directory, "mock", &store, state.clone()).await.unwrap();
    assert_eq!(prompts[0].arguments[0].name, "topic");

    let result = call_tool(
        directory,
        "mock",
        "echo",
        json!({ "text": "hello" }),
        &store,
        state.clone(),
    )
    .await
    .unwrap();
    assert_eq!(tool_result_text(&result), "ok");

    assert!(disconnect(&state, directory, "mock").await.unwrap());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn killed_local_runtime_reconnects_before_the_next_call() {
    let root = temp_dir("stdio-reconnect");
    let server = configurable_mock_mcp_server(&root);
    let pid_file = root.join("pids");
    write_local_mock_config(&root, &server, &pid_file, "first", true);
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    let directory = root.to_str().unwrap();
    let state = test_state(&root).await;

    let first = call_tool(directory, "mock", "echo", json!({}), &store, state.clone())
        .await
        .unwrap();
    assert_eq!(tool_result_text(&first), "first");
    let first_pid = wait_for_pid_count(&pid_file, 1)[0];
    assert!(std::process::Command::new("kill")
        .args(["-9", &first_pid.to_string()])
        .status()
        .unwrap()
        .success());

    let second = call_tool(directory, "mock", "echo", json!({}), &store, state.clone())
        .await
        .unwrap();
    assert_eq!(tool_result_text(&second), "first");
    let pids = wait_for_pid_count(&pid_file, 2);
    assert_ne!(pids[0], pids[1]);

    assert!(disconnect(&state, directory, "mock").await.unwrap());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn changed_local_config_restarts_the_runtime() {
    let root = temp_dir("stdio-config-reload");
    let server = configurable_mock_mcp_server(&root);
    let pid_file = root.join("pids");
    write_local_mock_config(&root, &server, &pid_file, "first", true);
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    let directory = root.to_str().unwrap();
    let state = test_state(&root).await;

    let first = call_tool(directory, "mock", "echo", json!({}), &store, state.clone())
        .await
        .unwrap();
    assert_eq!(tool_result_text(&first), "first");
    write_local_mock_config(&root, &server, &pid_file, "second", true);

    let second = call_tool(directory, "mock", "echo", json!({}), &store, state.clone())
        .await
        .unwrap();
    assert_eq!(tool_result_text(&second), "second");
    let pids = wait_for_pid_count(&pid_file, 2);
    assert_ne!(pids[0], pids[1]);

    assert!(disconnect(&state, directory, "mock").await.unwrap());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn manually_disabled_local_config_disconnects_the_runtime() {
    let root = temp_dir("stdio-disable-reload");
    let server = configurable_mock_mcp_server(&root);
    let pid_file = root.join("pids");
    write_local_mock_config(&root, &server, &pid_file, "first", true);
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    let directory = root.to_str().unwrap();
    let state = test_state(&root).await;

    connect(directory, "mock", &store, state.clone()).await.unwrap();
    write_local_mock_config(&root, &server, &pid_file, "first", false);
    let error = tools(directory, "mock", &store, state.clone()).await.unwrap_err();

    assert!(error.to_string().contains("disabled"));
    assert!(!disconnect(&state, directory, "mock").await.unwrap());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn app_states_isolate_and_shutdown_local_mcp_runtimes() {
    let root = temp_dir("state-isolation-shutdown");
    let server = configurable_mock_mcp_server(&root);
    let pid_file = root.join("pids");
    write_local_mock_config(&root, &server, &pid_file, "isolated", true);
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    let directory = root.to_str().unwrap();
    let first = AppState::open_database(root.join("first.db")).await.unwrap();
    let second = AppState::open_database(root.join("second.db")).await.unwrap();

    connect(directory, "mock", &store, first.clone())
        .await
        .unwrap();
    connect(directory, "mock", &store, second.clone())
        .await
        .unwrap();
    let pids = wait_for_pid_count(&pid_file, 2);
    assert_ne!(pids[0], pids[1]);

    first.shutdown().await.unwrap();
    assert!(first.inner.workspace_runtimes.loaded(directory).await.is_none());
    assert!(matches!(
        second.workspace_runtime(directory).await.unwrap().mcp().unwrap().status(directory, "mock"),
        Some(McpStatus::Connected)
    ));
    assert_eq!(
        tool_result_text(
            &call_tool(directory, "mock", "echo", json!({}), &store, second.clone())
                .await
                .unwrap()
        ),
        "isolated"
    );

    second.shutdown().await.unwrap();
    for pid in pids {
        assert!(!process_is_alive(pid), "MCP child {pid} survived shutdown");
    }
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn remote_http_runtime_lists_and_calls_tools_with_headers_and_bearer_token() {
    let root = temp_dir("remote-http-runtime");
    let mock = RemoteMockState::default();
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
        Err(error) => panic!("failed to bind remote MCP test server: {error}"),
    };
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let app = Router::new()
        .route("/mcp", post(remote_mcp_handler))
        .with_state(mock.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    fs::write(
        root.join(".agent/agent.json"),
        format!(
            r#"{{
                  "mcp": {{
                    "remote": {{
                      "type": "remote",
                      "url": "{url}",
                      "headers": {{ "x-test": "configured" }},
                      "oauth": {{ "clientId": "client" }},
                      "timeout": 2000
                    }}
                  }}
                }}"#
        ),
    )
    .unwrap();
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    store
        .update_tokens(
            "remote",
            &url,
            McpAuthTokens {
                access_token: "secret-token".to_string(),
                refresh_token: None,
                expires_at: None,
            },
        )
        .await
        .unwrap();
    let directory = root.to_str().unwrap();
    let state = test_state(&root).await;

    let connected = connect(directory, "remote", &store, state.clone()).await.unwrap();
    assert!(matches!(connected, McpStatus::Connected));
    assert!(matches!(
        status(directory, &store, &state).await.unwrap()["remote"],
        McpStatus::Connected
    ));

    let tools = tools(directory, "remote", &store, state.clone()).await.unwrap();
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].client, "remote");

    let resources = resources(directory, "remote", &store, state.clone()).await.unwrap();
    assert_eq!(resources[0].uri, "https://example.com/resource");

    let prompts = prompts(directory, "remote", &store, state.clone()).await.unwrap();
    assert_eq!(prompts[0].name, "summarize");

    let result = call_tool(
        directory,
        "remote",
        "echo",
        json!({ "text": "hello" }),
        &store,
        state.clone(),
    )
    .await
    .unwrap();
    assert_eq!(tool_result_text(&result), "remote ok");

    let headers = mock.headers.lock().unwrap();
    assert!(headers
        .iter()
        .any(|seen| seen.x_test.as_deref() == Some("configured")));
    assert!(headers
        .iter()
        .any(|seen| seen.authorization.as_deref() == Some("Bearer secret-token")));
    assert!(headers
        .iter()
        .any(|seen| seen.session_id.as_deref() == Some("session-1")));
    drop(headers);

    let methods = mock.methods.lock().unwrap().clone();
    assert!(methods.contains(&"initialize".to_string()));
    assert!(methods.contains(&"tools/list".to_string()));
    assert!(methods.contains(&"resources/list".to_string()));
    assert!(methods.contains(&"prompts/list".to_string()));
    assert!(methods.contains(&"tools/call".to_string()));

    assert!(disconnect(&state, directory, "remote").await.unwrap());
    let _ = shutdown_tx.send(());
    let _ = server.await;
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn remote_tool_failure_keeps_connection_and_catalog() {
    let root = temp_dir("remote-tool-failure");
    let mock = RemoteMockState::default();
    mock.fail_tool_calls.store(true, Ordering::Relaxed);
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
        Err(error) => panic!("failed to bind remote MCP test server: {error}"),
    };
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let app = Router::new()
        .route("/mcp", post(remote_mcp_handler))
        .with_state(mock.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    fs::write(
        root.join(".agent/agent.json"),
        format!(
            r#"{{"mcp":{{"remote":{{"type":"remote","url":"{url}","timeout":2000}}}}}}"#
        ),
    )
    .unwrap();
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    let directory = root.to_str().unwrap();
    let state = test_state(&root).await;

    connect(directory, "remote", &store, state.clone()).await.unwrap();
    let error = call_tool(directory, "remote", "echo", json!({}), &store, state.clone())
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("forced tool failure"));
    assert!(matches!(
        status(directory, &store, &state).await.unwrap()["remote"],
        McpStatus::Connected
    ));
    assert_eq!(
        tools(directory, "remote", &store, state.clone()).await.unwrap()[0].name,
        "echo"
    );
    assert_eq!(
        mock.methods
            .lock()
            .unwrap()
            .iter()
            .filter(|method| method.as_str() == "initialize")
            .count(),
        1
    );

    assert!(disconnect(&state, directory, "remote").await.unwrap());
    let _ = shutdown_tx.send(());
    let _ = server.await;
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn remote_http_connect_invalidates_stale_bearer_token_on_unauthorized() {
    let root = temp_dir("remote-http-stale-token");
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
        Err(error) => panic!("failed to bind remote MCP test server: {error}"),
    };
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let app = Router::new().route("/mcp", post(unauthorized_mcp_handler));
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    fs::write(
        root.join(".agent/agent.json"),
        format!(
            r#"{{
                  "mcp": {{
                    "remote": {{
                      "type": "remote",
                      "url": "{url}",
                      "oauth": {{ "clientId": "client" }},
                      "timeout": 2000
                    }}
                  }}
                }}"#
        ),
    )
    .unwrap();
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    store
        .update_tokens(
            "remote",
            &url,
            McpAuthTokens {
                access_token: "stale-token".to_string(),
                refresh_token: None,
                expires_at: None,
            },
        )
        .await
        .unwrap();

    let state = test_state(&root).await;
    let status = connect(root.to_str().unwrap(), "remote", &store, state)
        .await
        .unwrap();

    assert!(matches!(status, McpStatus::NeedsAuth));
    assert!(store.get_for_url("remote", &url).await.unwrap().unwrap().tokens.is_none());

    let _ = shutdown_tx.send(());
    let _ = server.await;
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn expired_refresh_token_is_cleared_and_reports_needs_auth() {
    let root = temp_dir("remote-refresh-invalid");
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
        Err(error) => panic!("failed to bind remote MCP test server: {error}"),
    };
    let base = format!("http://{}", listener.local_addr().unwrap());
    let url = format!("{base}/mcp");
    let token_url = format!("{base}/token");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let app = Router::new().route("/token", post(invalid_refresh_handler));
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    fs::write(
        root.join(".agent/agent.json"),
        format!(
            r#"{{
                  "mcp": {{
                    "remote": {{
                      "type": "remote",
                      "url": "{url}",
                      "oauth": {{ "clientId": "client", "tokenUrl": "{token_url}" }},
                      "timeout": 2000
                    }}
                  }}
                }}"#
        ),
    )
    .unwrap();
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    store
        .update_tokens(
            "remote",
            &url,
            McpAuthTokens {
                access_token: "expired-token".to_string(),
                refresh_token: Some("revoked-refresh".to_string()),
                expires_at: Some(1),
            },
        )
        .await
        .unwrap();

    let state = test_state(&root).await;
    let status = connect(root.to_str().unwrap(), "remote", &store, state)
        .await
        .unwrap();

    assert!(matches!(status, McpStatus::NeedsAuth));
    assert!(store.get_for_url("remote", &url).await.unwrap().unwrap().tokens.is_none());

    let _ = shutdown_tx.send(());
    let _ = server.await;
    let _ = fs::remove_dir_all(root);
}

#[test]
fn http_rpc_parser_extracts_sse_data_response() {
    let response = parse_http_rpc_response(
            "tools/list",
            Some("text/event-stream"),
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"tools\":[{\"name\":\"echo\"}]}}\n\n",
        )
        .unwrap()
        .unwrap();

    assert_eq!(response.id, Some(7));
    assert_eq!(
        response.result.unwrap()["tools"][0]["name"].as_str(),
        Some("echo")
    );
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-mcp-{name}-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(path.join(".agent")).unwrap();
    path
}

fn temp_auth_path(name: &str) -> std::path::PathBuf {
    let dir = temp_dir(name);
    dir.join("mcp-auth.json")
}

async fn test_state(root: &std::path::Path) -> AppState {
    AppState::open_database(root.join("agent-test.db")).await.unwrap()
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn mock_mcp_server(root: &std::path::Path) -> std::path::PathBuf {
    let path = root.join("mock-mcp.sh");
    fs::write(
            &path,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"notifications/initialized"'*)
      ;;
    *'"id":1'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"mock","version":"1"}}}'
      ;;
    *'"id":2'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}'
      ;;
    *'"id":3'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"resources":[{"name":"example","uri":"file:///tmp/example.txt","mimeType":"text/plain"}]}}'
      ;;
    *'"id":4'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"prompts":[{"name":"summarize","arguments":[{"name":"topic","required":true}]}]}}'
      ;;
    *'"id":5'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":5,"result":{"content":[{"type":"text","text":"ok"}],"isError":false}}'
      ;;
  esac
done
"#,
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
    path
}

#[cfg(unix)]
fn configurable_mock_mcp_server(root: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = root.join("configurable-mock-mcp.sh");
    fs::write(
        &path,
        r#"#!/bin/sh
printf '%s\n' "$$" >> "$PID_FILE"
while IFS= read -r line; do
  case "$line" in
    *'"method":"notifications/initialized"'*) ;;
    *'"method":"initialize"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"mock","version":"1"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}\n' "$id"
      ;;
    *'"method":"resources/list"'*|*'"method":"prompts/list"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"%s"}],"isError":false}}\n' "$id" "$RESULT"
      ;;
  esac
done
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

#[cfg(unix)]
fn write_local_mock_config(
    root: &std::path::Path,
    server: &std::path::Path,
    pid_file: &std::path::Path,
    result: &str,
    enabled: bool,
) {
    fs::write(
        root.join(".agent/agent.json"),
        format!(
            r#"{{"mcp":{{"mock":{{"type":"local","command":["{}"],"environment":{{"PID_FILE":"{}","RESULT":"{result}"}},"enabled":{enabled},"timeout":2000}}}}}}"#,
            server.display(),
            pid_file.display()
        ),
    )
    .unwrap();
}

#[cfg(unix)]
fn wait_for_pid_count(path: &std::path::Path, count: usize) -> Vec<u32> {
    for _ in 0..100 {
        let pids = fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.parse().ok())
            .collect::<Vec<_>>();
        if pids.len() >= count {
            return pids;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting for {count} mock MCP process IDs");
}

#[derive(Clone, Default)]
struct RemoteMockState {
    methods: Arc<StdMutex<Vec<String>>>,
    headers: Arc<StdMutex<Vec<SeenHeaders>>>,
    fail_tool_calls: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
struct SeenHeaders {
    x_test: Option<String>,
    authorization: Option<String>,
    session_id: Option<String>,
}

async fn remote_mcp_handler(
    State(state): State<RemoteMockState>,
    headers: AxumHeaderMap,
    Json(request): Json<Value>,
) -> (AxumHeaderMap, Json<Value>) {
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    state.methods.lock().unwrap().push(method.clone());
    state.headers.lock().unwrap().push(SeenHeaders {
        x_test: header_string(&headers, "x-test"),
        authorization: header_string(&headers, "authorization"),
        session_id: header_string(&headers, "mcp-session-id"),
    });

    let result = match method.as_str() {
        "initialize" => json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "remote-mock", "version": "1" }
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "echo",
                "description": "Echo text",
                "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } }
            }]
        }),
        "resources/list" => json!({
            "resources": [{
                "name": "example",
                "uri": "https://example.com/resource",
                "mimeType": "text/plain"
            }]
        }),
        "prompts/list" => json!({
            "prompts": [{
                "name": "summarize",
                "arguments": [{ "name": "topic", "required": true }]
            }]
        }),
        "tools/call" => json!({
            "content": [{ "type": "text", "text": "remote ok" }],
            "isError": false
        }),
        _ => json!({}),
    };
    let mut response_headers = AxumHeaderMap::new();
    if method == "initialize" {
        response_headers.insert("mcp-session-id", "session-1".parse().unwrap());
    }
    let body = if method == "tools/call" && state.fail_tool_calls.load(Ordering::Relaxed)
    {
        Json(json!({
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "error": { "code": -32000, "message": "forced tool failure" }
        }))
    } else if let Some(id) = request.get("id") {
        Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    } else {
        Json(json!({ "jsonrpc": "2.0", "result": result }))
    };
    (response_headers, body)
}

async fn unauthorized_mcp_handler() -> impl IntoResponse {
    (
        AxumStatusCode::UNAUTHORIZED,
        Json(json!({ "error": "invalid_token" })),
    )
}

async fn invalid_refresh_handler() -> impl IntoResponse {
    (
        AxumStatusCode::BAD_REQUEST,
        Json(json!({ "error": "invalid_grant" })),
    )
}

fn header_string(headers: &AxumHeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}
