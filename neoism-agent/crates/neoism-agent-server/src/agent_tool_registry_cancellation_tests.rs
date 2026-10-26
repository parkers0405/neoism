#![cfg(unix)]

use super::*;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

struct Fixture {
    root: PathBuf,
    state: AppState,
    snapshot: crate::workspace_runtime::PluginGenerationLease,
    auth: mcp_auth::McpAuthStore,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "neoism-mcp-catalog-cancel-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event),
        ));
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        std::fs::write(
            root.join(".agent/agent.json"),
            json!({"mcp": {
                "a-stalled": {"type":"local", "enabled":true, "timeout":60_000,
                    "command":["python3", "-c", "import pathlib,time; pathlib.Path('mcp-started').write_text('ready'); time.sleep(60)"]},
                "computer": {"type":"local", "command":["builtin", "computer"], "enabled":true}
            }}).to_string(),
        ).unwrap();
        let services = crate::standard_services()
            .with_config(Arc::new(
                neoism_agent_service_api::StandardConfigSourceService::new(
                    root.join("user"),
                ),
            ))
            .with_builtin_mcp(Arc::new(crate::computer_use::ComputerUse));
        let state =
            AppState::open_database_with_services(root.join("state.db"), services)
                .await
                .unwrap();
        let workspace = acquire_workspace_plugin_snapshot(&state, root.to_str().unwrap())
            .await
            .unwrap();
        let auth = mcp_auth::McpAuthStore::new(root.join("mcp-auth.json"));
        Self {
            root,
            state,
            snapshot: workspace.snapshot,
            auth,
        }
    }

    async fn execute(
        &self,
        arguments: Value,
        cancel: Arc<AtomicBool>,
    ) -> anyhow::Result<Option<tool::ToolExecutionResult>> {
        execute_mcp_gateway(
            self.root.to_str().unwrap(),
            MCP_GATEWAY_TOOL,
            arguments,
            &[PermissionRule {
                permission: "*".into(),
                pattern: "*".into(),
                action: PermissionAction::Allow,
            }],
            Some(cancel),
            Some(self.state.clone()),
            &self.snapshot,
            &neoism_agent_service_api::ExecutionPolicy::NativeLocal,
            &self.auth,
        )
        .await
    }

    async fn cleanup(self) {
        drop(self.snapshot);
        self.state.shutdown().await.unwrap();
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

#[tokio::test]
async fn catalog_discovery_stops_on_cancel_while_server_initialization_is_stalled() {
    let fixture = Fixture::new().await;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let discovery =
            fixture.execute(json!({"action":"search", "query":"tabs"}), cancel.clone());
        tokio::pin!(discovery);
        let started = async {
            while !fixture.root.join("mcp-started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::select! {
            result = &mut discovery => panic!("stalled discovery returned early: {}", result.is_ok()),
            result = tokio::time::timeout(Duration::from_secs(5), started) => result.unwrap(),
        }
        cancel.store(true, Ordering::SeqCst);
        let error = tokio::time::timeout(Duration::from_secs(1), discovery)
            .await
            .expect("Escape must not wait for the server's 60-second timeout")
            .err()
            .expect("cancelled discovery must fail");
        assert!(error.to_string().contains("discovery aborted"), "{error}");
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn scoped_computer_search_and_call_skip_unrelated_stalled_servers() {
    let fixture = Fixture::new().await;
    for arguments in [
        json!({"action":"search", "namespace":" computer ", "query":"browser_tabs"}),
        json!({"action":"call", "tool":"computer.capabilities", "arguments":{}}),
        json!({"action":"call", "tool":"mcp__computer__capabilities", "arguments":{}}),
    ] {
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            fixture.execute(arguments, Arc::new(AtomicBool::new(false))),
        )
        .await
        .expect("computer tools must not wait for another server's OAuth")
        .unwrap()
        .unwrap();
        assert!(result.output.contains("browser"), "{}", result.output);
        assert!(!fixture.root.join("mcp-started").exists());
    }
    fixture.cleanup().await;
}

#[tokio::test]
async fn cancelled_catalog_discovery_does_not_start_servers() {
    let fixture = Fixture::new().await;
    let result = fixture
        .execute(json!({"action":"search"}), Arc::new(AtomicBool::new(true)))
        .await;
    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("discovery aborted"));
    assert!(!fixture.root.join("mcp-started").exists());
    fixture.cleanup().await;
}

#[tokio::test]
async fn abort_during_provider_catalog_preparation_releases_run_and_preserves_queue() {
    let fixture = Fixture::new().await;
    let session = crate::session_routes::session_create(
        axum::extract::State(fixture.state.clone()),
        axum::extract::Query(
            serde_json::from_value(json!({
                "directory": fixture.root.to_str().unwrap(),
            }))
            .unwrap(),
        ),
        Default::default(),
        None,
        Some(axum::Json(
            serde_json::from_value(json!({
                "model": {"providerId":"neoism", "id":"stub"},
            }))
            .unwrap(),
        )),
    )
    .await
    .unwrap()
    .0;
    let request = serde_json::from_value(json!({
        "model": {"providerId":"neoism", "modelId":"stub"},
        "parts":[{"type":"text", "text":"cancel during catalog preparation"}],
    }))
    .unwrap();
    {
        let prompt = crate::session_prompt::append_prompt(
            &fixture.state,
            session.id.as_str(),
            request,
            true,
        );
        tokio::pin!(prompt);
        let started = async {
            while !fixture.root.join("mcp-started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::select! {
            result = &mut prompt => panic!("prompt returned before stalled discovery: {}", result.is_ok()),
            result = tokio::time::timeout(Duration::from_secs(10), started) => result.unwrap(),
        }
        let queued = serde_json::from_value(json!({
            "parts":[{"type":"text", "text":"preserve my queued message"}],
        }))
        .unwrap();
        fixture
            .state
            .inner
            .store
            .enqueue_prompt_with_delivery(session.id.as_str(), &queued, "queue")
            .await
            .unwrap();
        assert!(
            crate::session_actions::abort_session_run(
                &fixture.state,
                session.id.as_str(),
            )
            .await
        );
        let error = tokio::time::timeout(Duration::from_secs(2), prompt)
            .await
            .expect("abort must release a run waiting on catalog preparation")
            .err()
            .expect("the prompt must report cancellation");
        assert!(error.to_string().contains("Session aborted"), "{error}");
    }
    assert!(fixture
        .state
        .inner
        .session_coordinator
        .active_run(session.id.as_str())
        .await
        .is_none());
    assert_eq!(
        crate::session_queue::queued_prompt_count(&fixture.state, session.id.as_str(),)
            .await,
        1
    );
    fixture.cleanup().await;
}
