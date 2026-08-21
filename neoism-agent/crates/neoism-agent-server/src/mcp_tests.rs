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
fn status_marks_remote_oauth_without_tokens_as_needs_auth() {
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

    let status = status_for_config(&config, &store);
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
        root.join("neoism.json"),
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

    let response = auth_start(directory, "remote", &store).await.unwrap();

    assert!(response
        .authorization_url
        .starts_with("https://auth.example.com/oauth/authorize?"));
    assert!(response.authorization_url.contains("client_id=client"));
    assert!(response.authorization_url.contains("scope=tools+read"));
    assert!(response
        .authorization_url
        .contains("code_challenge_method=S256"));
    let entry = store.get("remote").unwrap().unwrap();
    assert_eq!(
        entry.oauth_state.as_deref(),
        Some(response.oauth_state.as_str())
    );
    assert_eq!(entry.server_url.as_deref(), Some("https://example.com/mcp"));
    assert_eq!(entry.oauth_directory.as_deref(), Some(directory));
    assert_eq!(
        entry.oauth_redirect_uri.as_deref(),
        Some("http://127.0.0.1/callback")
    );
    assert!(entry.code_verifier.is_some());
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
        "http://127.0.0.1:39319/mcp/webflow/auth/callback"
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
        root.join("neoism.json"),
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

    let connected = connect(directory, "mock", &store).await.unwrap();
    assert!(matches!(connected, McpStatus::Connected));
    assert!(matches!(
        status(directory, &store).unwrap()["mock"],
        McpStatus::Connected
    ));

    let tools = tools(directory, "mock", &store).await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].client, "mock");
    assert_eq!(tool_runtime_id("mock", "echo-tool"), "mcp__mock__echo_tool");

    let resources = resources(directory, "mock", &store).await.unwrap();
    assert_eq!(resources[0].uri, "file:///tmp/example.txt");

    let prompts = prompts(directory, "mock", &store).await.unwrap();
    assert_eq!(prompts[0].arguments[0].name, "topic");

    let result = call_tool(
        directory,
        "mock",
        "echo",
        json!({ "text": "hello" }),
        &store,
    )
    .await
    .unwrap();
    assert_eq!(tool_result_text(&result), "ok");

    assert!(disconnect(directory, "mock").await.unwrap());
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
        root.join("neoism.json"),
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
            McpAuthTokens {
                access_token: "secret-token".to_string(),
                refresh_token: None,
                expires_at: None,
                scope: None,
            },
            Some(&url),
        )
        .unwrap();
    let directory = root.to_str().unwrap();

    let connected = connect(directory, "remote", &store).await.unwrap();
    assert!(matches!(connected, McpStatus::Connected));
    assert!(matches!(
        status(directory, &store).unwrap()["remote"],
        McpStatus::Connected
    ));

    let tools = tools(directory, "remote", &store).await.unwrap();
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].client, "remote");

    let resources = resources(directory, "remote", &store).await.unwrap();
    assert_eq!(resources[0].uri, "https://example.com/resource");

    let prompts = prompts(directory, "remote", &store).await.unwrap();
    assert_eq!(prompts[0].name, "summarize");

    let result = call_tool(
        directory,
        "remote",
        "echo",
        json!({ "text": "hello" }),
        &store,
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

    assert!(disconnect(directory, "remote").await.unwrap());
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
        root.join("neoism.json"),
        format!(
            r#"{{"mcp":{{"remote":{{"type":"remote","url":"{url}","timeout":2000}}}}}}"#
        ),
    )
    .unwrap();
    let store = McpAuthStore::new(root.join("mcp-auth.json"));
    let directory = root.to_str().unwrap();

    connect(directory, "remote", &store).await.unwrap();
    let error = call_tool(directory, "remote", "echo", json!({}), &store)
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("forced tool failure"));
    assert!(matches!(
        status(directory, &store).unwrap()["remote"],
        McpStatus::Connected
    ));
    assert_eq!(
        tools(directory, "remote", &store).await.unwrap()[0].name,
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

    assert!(disconnect(directory, "remote").await.unwrap());
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
        root.join("neoism.json"),
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
            McpAuthTokens {
                access_token: "stale-token".to_string(),
                refresh_token: None,
                expires_at: None,
                scope: None,
            },
            Some(&url),
        )
        .unwrap();

    let status = connect(root.to_str().unwrap(), "remote", &store)
        .await
        .unwrap();

    assert!(matches!(status, McpStatus::NeedsAuth));
    assert!(store.get("remote").unwrap().unwrap().tokens.is_none());

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
        root.join("neoism.json"),
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
            McpAuthTokens {
                access_token: "expired-token".to_string(),
                refresh_token: Some("revoked-refresh".to_string()),
                expires_at: Some(1),
                scope: None,
            },
            Some(&url),
        )
        .unwrap();

    let status = connect(root.to_str().unwrap(), "remote", &store)
        .await
        .unwrap();

    assert!(matches!(status, McpStatus::NeedsAuth));
    assert!(store.get("remote").unwrap().unwrap().tokens.is_none());

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
    fs::create_dir_all(&path).unwrap();
    path
}

fn temp_auth_path(name: &str) -> std::path::PathBuf {
    let dir = temp_dir(name);
    dir.join("mcp-auth.json")
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
