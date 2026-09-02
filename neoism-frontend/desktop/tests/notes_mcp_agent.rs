use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use neoism_agent_core::ToolListItem;
use neoism_agent_server::AppState;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tower::ServiceExt;

fn request(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap()
}

async fn response_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn desktop_notes_are_discovered_and_called_through_agent_mcp() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let notes_home = root.path().join("vaults");
    std::fs::create_dir_all(&workspace).unwrap();
    unsafe { std::env::set_var("NEOISM_NOTES_HOME", &notes_home) };

    let services = neoism_desktop::notes_mcp::install_with_executable(
        neoism_agent_neoism_adapter::neoism_services(),
        env!("CARGO_BIN_EXE_neoism"),
    );
    let state = AppState::open_database_with_services(root.path().join("agent.db"), services)
        .await
        .unwrap();
    let app = neoism_agent_server::app(state.clone());
    let directory = workspace.to_string_lossy();

    let tools: Vec<ToolListItem> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/tools?directory={directory}"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let runtime_ids = tools
        .iter()
        .filter(|tool| tool.id.starts_with("mcp__notes__"))
        .map(|tool| tool.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        runtime_ids,
        [
            "mcp__notes__create",
            "mcp__notes__list",
            "mcp__notes__read",
            "mcp__notes__search",
            "mcp__notes__taskToggle",
            "mcp__notes__tasks",
            "mcp__notes__write",
        ]
    );

    let endpoint = |tool: &str| {
        format!(
            "/v2/plugins/dev.neoism.mcp/notes/tools/{tool}?directory={directory}"
        )
    };
    let created: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &endpoint("create"),
                Some(json!({"title":"Agent Note","content":"first"})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(created["content"][0]["text"], "Agent Note.md");

    let written: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &endpoint("write"),
                Some(json!({"path":"Agent Note.md","content":"updated"})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(written["isError"], false);

    let read: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &endpoint("read"),
                Some(json!({"path":"Agent Note.md"})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(read["content"][0]["text"], "updated");

    let missing: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &endpoint("read"),
                Some(json!({"path":"Missing.md"})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(missing["isError"], true);

    state.shutdown().await.unwrap();
}