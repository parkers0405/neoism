//! Integration tests for the AI-agent-session handoff a `POST /workspace/promote`
//! performs after the workspace files land (Wave 4C-agent — "shut the laptop,
//! agent keeps working").
//!
//! ## What this covers (and what it deliberately doesn't)
//!
//! A real promote spans three processes: the source daemon, the source's
//! loopback agent-server, and the target daemon (which proxies bundles into
//! *its* loopback agent-server). Standing up a real `neoism-agent-server`
//! (Anthropic key, SQLite store, live sessions) is impractical in a unit test,
//! so we test the seam that is ours to test — the TARGET daemon's
//! `/workspace/receive-agent` proxy — against a FAKE agent-server, plus the
//! best-effort contract end-to-end.
//!
//! Three cases:
//!   1. `receive_agent_forwards_each_bundle` — stand up a fake agent-server that
//!      records every `/sessions/import` call, point `NEOISM_AGENT_SERVER` at
//!      it, `POST /workspace/receive-agent` with TWO bundles, and assert both
//!      were forwarded with the right `targetWorkspaceRoot` and that the route
//!      reports `imported == 2`.
//!   2. `receive_agent_requires_auth` — the proxy is gated by the same cloud
//!      auth as the other `/workspace/*` routes; no bearer → 401.
//!   3. `promote_succeeds_when_agent_export_unreachable` — a FULL promote (real
//!      target daemon, real git) with `NEOISM_AGENT_SERVER` pointed at a dead
//!      port. The promote must still return 200 (files + pointer moved); the
//!      agent handoff failure is surfaced only in `agent_ship.errors`. This is
//!      the best-effort guarantee.
//!
//! NOT covered here: the source-side `/sessions/export` happy path and the real
//! agent-server import internals (transcript rebasing, resume) — those live in
//! the agent-server crate's own `session_transfer` tests. We assert the daemon
//! *forwards* bundles verbatim; it never inspects their internals.
//!
//! Lives in `tests/` (not `--lib`) because the daemon's unit tests are red from
//! an unrelated in-flight rename — same note as `tests/workspace_promote.rs`.

use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use neoism_protocol::workspace::{WorkspaceClientMessage, WorkspaceServerMessage};
use neoism_workspace_daemon::auth::AuthService;
use neoism_workspace_daemon::cloud_auth::ENV_CLOUD_PROVISION_TOKEN;
use neoism_workspace_daemon::crdt::sync::CrdtSyncHub;
use neoism_workspace_daemon::handshake::PairingTokenStore;
use neoism_workspace_daemon::server::{self, AppState};
use neoism_workspace_daemon::sessions::SessionRegistry;
use neoism_workspace_daemon::workspace::{self, ConnectionWorkspace, WorkspaceManager};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tower::ServiceExt; // Router::oneshot.

// ---------------------------------------------------------------------
// Env hygiene. Process-global env vars are serialised through one lock and
// restored on Drop. Mirrors tests/workspace_promote.rs.
// ---------------------------------------------------------------------

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard<'a> {
    _g: std::sync::MutexGuard<'a, ()>,
    prev: Vec<(&'static str, Option<String>)>,
}

impl<'a> EnvGuard<'a> {
    fn new(vars: &[(&'static str, Option<&str>)]) -> Self {
        let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut prev = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            prev.push((*key, std::env::var(key).ok()));
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        Self { _g: g, prev }
    }
}

impl Drop for EnvGuard<'_> {
    fn drop(&mut self) {
        for (key, value) in &self.prev {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

// ---------------------------------------------------------------------
// Fake agent-server: records every `/sessions/import` call so the test can
// assert what the target daemon forwarded. Replaces the real
// `neoism-agent-server` (which needs an API key + store).
// ---------------------------------------------------------------------

/// One recorded `/sessions/import` request, as the fake agent-server saw it.
#[derive(Clone, Debug)]
struct ImportCall {
    target_workspace_root: String,
    bundle: serde_json::Value,
}

#[derive(Clone, Default)]
struct FakeAgentState {
    calls: Arc<Mutex<Vec<ImportCall>>>,
}

async fn fake_import(
    State(state): State<FakeAgentState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let target_workspace_root = body
        .get("targetWorkspaceRoot")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let bundle = body
        .get("bundle")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    state.calls.lock().unwrap().push(ImportCall {
        target_workspace_root,
        bundle: bundle.clone(),
    });
    // Echo a plausible `/sessions/import` reply so the daemon counts it imported.
    let session_id = bundle
        .get("session")
        .and_then(|s| s.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("fake-session")
        .to_string();
    Json(serde_json::json!({ "sessionId": session_id }))
}

struct FakeAgentServer {
    base_url: String,
    calls: Arc<Mutex<Vec<ImportCall>>>,
    task: Option<JoinHandle<()>>,
}

impl FakeAgentServer {
    async fn spawn() -> Self {
        let state = FakeAgentState::default();
        let calls = state.calls.clone();
        let app = Router::new()
            .route("/sessions/import", post(fake_import))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake agent-server");
        let addr: SocketAddr = listener.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service()).await;
        });
        FakeAgentServer {
            base_url: format!("http://{addr}"),
            calls,
            task: Some(task),
        }
    }

    fn recorded(&self) -> Vec<ImportCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl Drop for FakeAgentServer {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

// ---------------------------------------------------------------------
// Daemon router under test (the TARGET host whose /workspace/receive-agent we
// drive). In-process via Router::oneshot.
// ---------------------------------------------------------------------

struct Daemon {
    router: Router,
    _config_dir: TempDir,
    _data_dir: TempDir,
    _registry_dir: TempDir,
}

fn build_daemon() -> Daemon {
    let config_dir = TempDir::new().expect("pairing config tempdir");
    let data_dir = TempDir::new().expect("auth data tempdir");
    let registry_dir = TempDir::new().expect("registry tempdir");
    let registry_file = registry_dir.path().join("workspaces.json");
    std::env::set_var("NEOISM_CONFIG_DIR", config_dir.path());
    std::env::set_var("NEOISM_DAEMON_DATA_DIR", data_dir.path());
    std::env::set_var("NEOISM_WORKSPACE_REGISTRY", &registry_file);

    let auth = AuthService::bootstrap(data_dir.path()).expect("auth bootstrap");
    let router = server::router(AppState {
        auth,
        sessions: SessionRegistry::shared(),
        workspaces: WorkspaceManager::bootstrap(),
        pairing_tokens: PairingTokenStore::in_memory(),
        crdt: CrdtSyncHub::default(),
        paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
    });
    Daemon {
        router,
        _config_dir: config_dir,
        _data_dir: data_dir,
        _registry_dir: registry_dir,
    }
}

async fn post_json(
    router: Router,
    path: &str,
    bearer: Option<&str>,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .expect("build request");
    let response = router.oneshot(request).await.expect("router oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

fn bundle(session_id: &str) -> serde_json::Value {
    // Opaque to the daemon — it forwards verbatim. Shape mirrors a real
    // SessionBundle just enough that the fake server can echo a sessionId.
    serde_json::json!({
        "version": 1,
        "session": { "id": session_id, "directory": "/old/home/proj" },
        "messages": [],
        "queuedPrompts": [],
        "workspaceRoot": "/old/home"
    })
}

// =====================================================================
// 1. The TARGET proxy forwards every bundle to its local agent-server with the
//    right targetWorkspaceRoot.
// =====================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receive_agent_forwards_each_bundle() {
    const TOKEN: &str = "receive-agent-token-0001";
    let fake = FakeAgentServer::spawn().await;

    let _g = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        ("NEOISM_AGENT_SERVER", Some(&fake.base_url)),
        // Make sure the fallback env can't sneak in a different address.
        ("NEOISM_SERVER", None),
    ]);

    let daemon = build_daemon();
    let target_root = "/srv/work/relocated-proj";

    let (status, json) = post_json(
        daemon.router.clone(),
        "/workspace/receive-agent",
        Some(TOKEN),
        serde_json::json!({
            "bundles": [bundle("sess-aaa"), bundle("sess-bbb")],
            "target_workspace_root": target_root,
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "receive-agent should succeed: {json}"
    );
    assert_eq!(
        json["imported"].as_u64(),
        Some(2),
        "both bundles should import: {json}"
    );
    assert!(
        json["errors"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "no import errors expected: {json}"
    );

    // The fake agent-server saw BOTH bundles, each rebased onto target_root.
    let recorded = fake.recorded();
    assert_eq!(recorded.len(), 2, "fake agent-server must see two imports");
    for call in &recorded {
        assert_eq!(
            call.target_workspace_root, target_root,
            "every import must carry the target workspace root"
        );
    }
    let session_ids: Vec<String> = recorded
        .iter()
        .map(|c| {
            c.bundle["session"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert!(
        session_ids.contains(&"sess-aaa".to_string()),
        "{session_ids:?}"
    );
    assert!(
        session_ids.contains(&"sess-bbb".to_string()),
        "{session_ids:?}"
    );
}

// =====================================================================
// Agent-server URL resolution + endpoint helpers. These live in
// `workspace_promote` (re-tested here from `tests/` because the daemon's
// `--lib` test build is red from an unrelated rename — see module docs).
// =====================================================================

#[test]
fn agent_server_url_default_and_env_override() {
    use neoism_workspace_daemon::workspace_promote::agent_server_url;
    // Serialise env mutation against the other tests sharing ENV_LOCK.
    let _g = EnvGuard::new(&[("NEOISM_AGENT_SERVER", None), ("NEOISM_SERVER", None)]);
    // Default when unset.
    assert_eq!(agent_server_url(), "http://127.0.0.1:4096");
    // Override wins, trimmed of whitespace + trailing slash.
    std::env::set_var("NEOISM_AGENT_SERVER", "  http://10.0.0.5:9000/  ");
    assert_eq!(agent_server_url(), "http://10.0.0.5:9000");
    std::env::remove_var("NEOISM_AGENT_SERVER");
    // Falls back to NEOISM_SERVER (the republished address).
    std::env::set_var("NEOISM_SERVER", "http://host:7000");
    assert_eq!(agent_server_url(), "http://host:7000");
}

#[test]
fn agent_endpoint_helpers_append_paths() {
    use neoism_workspace_daemon::workspace_promote::{
        agent_export_endpoint, agent_import_endpoint, receive_agent_endpoint,
    };
    assert_eq!(
        agent_export_endpoint("http://127.0.0.1:4096"),
        "http://127.0.0.1:4096/sessions/export"
    );
    assert_eq!(
        agent_import_endpoint("http://127.0.0.1:4096/"),
        "http://127.0.0.1:4096/sessions/import"
    );
    assert_eq!(
        receive_agent_endpoint("http://h:1/"),
        "http://h:1/workspace/receive-agent"
    );
}

// =====================================================================
// 2. The proxy is gated by the same cloud auth as the other /workspace/* routes.
// =====================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receive_agent_requires_auth() {
    const TOKEN: &str = "receive-agent-token-0002";
    let _g = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        ("NEOISM_AGENT_SERVER", Some("http://127.0.0.1:1")),
        ("NEOISM_SERVER", None),
    ]);
    let daemon = build_daemon();
    let (status, _json) = post_json(
        daemon.router.clone(),
        "/workspace/receive-agent",
        None, // no bearer
        serde_json::json!({
            "bundles": [],
            "target_workspace_root": "/srv/work/proj",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// =====================================================================
// 3. Best-effort: a full promote whose source agent-server is unreachable still
//    succeeds (files + pointer move); the agent failure is reported, not fatal.
// =====================================================================

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn init_repo(dir: &Path, file: &str, contents: &str) {
    git(dir, &["init"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Neoism Test"]);
    std::fs::write(dir.join(file), contents).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-m", "initial"]);
}

/// A real target daemon on an ephemeral port (so the source's reqwest call to
/// `/workspace/receive` and `/workspace/receive-agent` hits a real server).
struct TargetDaemon {
    base_url: String,
    task: Option<JoinHandle<()>>,
    _config_dir: TempDir,
    _data_dir: TempDir,
    _registry_dir: TempDir,
}

impl TargetDaemon {
    async fn spawn() -> Self {
        let config_dir = TempDir::new().expect("config tempdir");
        let data_dir = TempDir::new().expect("data tempdir");
        let registry_dir = TempDir::new().expect("registry tempdir");
        let registry_file = registry_dir.path().join("workspaces.json");
        std::env::set_var("NEOISM_CONFIG_DIR", config_dir.path());
        std::env::set_var("NEOISM_DAEMON_DATA_DIR", data_dir.path());
        std::env::set_var("NEOISM_WORKSPACE_REGISTRY", &registry_file);

        let auth = AuthService::bootstrap(data_dir.path()).expect("auth bootstrap");
        let app = server::router(AppState {
            auth,
            sessions: SessionRegistry::shared(),
            workspaces: WorkspaceManager::bootstrap(),
            pairing_tokens: PairingTokenStore::in_memory(),
            crdt: CrdtSyncHub::default(),
            paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("addr");
        let task = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        TargetDaemon {
            base_url: format!("http://{addr}"),
            task: Some(task),
            _config_dir: config_dir,
            _data_dir: data_dir,
            _registry_dir: registry_dir,
        }
    }
}

impl Drop for TargetDaemon {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn register_host_workspace(
    manager: &WorkspaceManager,
    host_id: &str,
    root: &Path,
) -> String {
    let mut conn = ConnectionWorkspace::default();
    let outcome = workspace::handle(
        manager,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::CreateHostWorkspace {
            host_id: host_id.to_string(),
            workspace_id: None,
            title: Some("promote-me".to_string()),
            root_dir: Some(root.to_path_buf()),
        },
    );
    outcome
        .replies
        .iter()
        .find_map(|reply| match reply {
            WorkspaceServerMessage::HostWorkspaceChanged {
                workspace_id: Some(id),
                ..
            } => Some(id.clone()),
            _ => None,
        })
        .expect("CreateHostWorkspace must yield a workspace id")
}

fn running_on_host(manager: &WorkspaceManager, workspace_id: &str) -> Option<String> {
    let mut conn = ConnectionWorkspace::default();
    let outcome = workspace::handle(
        manager,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::ListHostWorkspaces { host_id: None },
    );
    outcome.replies.iter().find_map(|reply| match reply {
        WorkspaceServerMessage::HostWorkspaceList { workspaces } => workspaces
            .iter()
            .find(|w| w.id == workspace_id)
            .and_then(|w| w.running_on_host_id.clone()),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn promote_succeeds_when_agent_export_unreachable() {
    if !git_available() {
        eprintln!("git unavailable; skipping");
        return;
    }
    const TOKEN: &str = "promote-agent-best-effort-cafe";

    let target_workspaces = TempDir::new().expect("target workspaces tempdir");
    let _g = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&target_workspaces.path().to_string_lossy()),
        ),
        // Point the SOURCE's agent-server at a closed port so /sessions/export
        // is unreachable. The promote must still succeed (best-effort).
        ("NEOISM_AGENT_SERVER", Some("http://127.0.0.1:1")),
        ("NEOISM_SERVER", None),
    ]);

    // bare origin so the source repo resolves a git_url.
    let bare = TempDir::new().expect("bare remote tempdir");
    git(bare.path(), &["init", "--bare"]);
    let bare_url = bare.path().to_string_lossy().to_string();

    // source repo with a committed file pushed to origin + an uncommitted edit.
    let src = TempDir::new().expect("source tempdir");
    init_repo(src.path(), "tracked.txt", "a\nb\nc\n");
    git(src.path(), &["remote", "add", "origin", &bare_url]);
    git(src.path(), &["push", "origin", "HEAD"]);
    std::fs::write(src.path().join("tracked.txt"), "a\nCHANGED\nc\n").unwrap();

    let target = TargetDaemon::spawn().await;

    // source daemon + registered host-workspace.
    let config_dir = TempDir::new().expect("config");
    let data_dir = TempDir::new().expect("data");
    let registry_dir = TempDir::new().expect("registry");
    std::env::set_var("NEOISM_CONFIG_DIR", config_dir.path());
    std::env::set_var("NEOISM_DAEMON_DATA_DIR", data_dir.path());
    std::env::set_var(
        "NEOISM_WORKSPACE_REGISTRY",
        registry_dir.path().join("workspaces.json"),
    );
    let auth = AuthService::bootstrap(data_dir.path()).expect("auth");
    let workspaces = WorkspaceManager::bootstrap();
    let router = server::router(AppState {
        auth,
        sessions: SessionRegistry::shared(),
        workspaces: workspaces.clone(),
        pairing_tokens: PairingTokenStore::in_memory(),
        crdt: CrdtSyncHub::default(),
        paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
    });
    let workspace_id = register_host_workspace(&workspaces, "source-host", src.path());

    let (status, json) = post_json(
        router.clone(),
        "/workspace/promote",
        Some(TOKEN),
        serde_json::json!({
            "workspace_id": workspace_id,
            "target_url": target.base_url,
            "target_token": TOKEN,
        }),
    )
    .await;

    // The promote itself SUCCEEDS despite the agent export being unreachable.
    assert_eq!(
        status,
        StatusCode::OK,
        "promote must succeed even when agent export fails: {json}"
    );
    // Files moved: tracked.txt patched on the target.
    let applied: Vec<String> = json["target_apply_report"]["applied_files"]
        .as_array()
        .expect("applied_files")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(applied.iter().any(|p| p == "tracked.txt"), "{json}");
    // Pointer flipped to the target.
    assert_eq!(
        running_on_host(&workspaces, &workspace_id).as_deref(),
        Some(target.base_url.as_str()),
        "pointer must flip to target despite agent failure: {json}"
    );
    // The agent handoff failure is reported in agent_ship, NOT fatal.
    let agent_ship = &json["agent_ship"];
    assert_eq!(
        agent_ship["exported"].as_u64(),
        Some(0),
        "nothing exported when source agent-server is down: {agent_ship}"
    );
    assert_eq!(agent_ship["imported"].as_u64(), Some(0), "{agent_ship}");
    assert!(
        agent_ship["errors"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "an unreachable agent export must record an error: {agent_ship}"
    );
}
