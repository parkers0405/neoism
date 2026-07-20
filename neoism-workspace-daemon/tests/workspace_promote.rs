//! Integration test for `POST /workspace/promote` — the SOURCE side of
//! "relocate a workspace's home to another host" (the keystone of the move
//! plane).
//!
//! Unlike `tests/workspace_receive.rs` (which drives the target half in-process
//! via `Router::oneshot`), promote is *server-driven*: the source daemon makes
//! a real `reqwest` call to the target's `/workspace/receive`. So this test
//! stands up a REAL target daemon on an ephemeral `127.0.0.1:0`
//! (`TcpListener` + `axum::serve`) and points the source at its URL — exactly
//! the production topology.
//!
//! It builds a source git repo WITH an `origin` remote pointing at a bare repo
//! (so `git_url` resolves) plus a tracked + untracked uncommitted change, then:
//!   * drives `POST /workspace/promote` against the source router,
//!   * asserts the target cloned the repo and applied the snapshot,
//!   * asserts the source's `running_on_host_id` flipped to the target URL.
//!
//! A second case asserts the no-remote 409.
//!
//! Lives in `tests/` (not `--lib`) because the daemon's unit tests are red from
//! an unrelated in-flight rename — same note as `tests/workspace_receive.rs`.

use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
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
use tower::ServiceExt; // Router::oneshot for the source side.

// ---------------------------------------------------------------------
// Env hygiene. `NEOISM_CLOUD_PROVISION_TOKEN` / `NEOISM_WORKSPACES_DIR` /
// `NEOISM_DAEMON_DATA_DIR` / `NEOISM_CONFIG_DIR` / `NEOISM_WORKSPACE_REGISTRY`
// are process globals; serialise through one lock and restore on Drop. (Same
// pattern as workspace_receive.rs.)
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
// git helpers.
// ---------------------------------------------------------------------

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

/// Init a repo with a deterministic identity and one committed file.
fn init_repo(dir: &Path, file: &str, contents: &str) {
    git(dir, &["init"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Neoism Test"]);
    std::fs::write(dir.join(file), contents).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-m", "initial"]);
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

// ---------------------------------------------------------------------
// Target daemon: a real server on an ephemeral port, reachable via reqwest.
// ---------------------------------------------------------------------

struct TargetDaemon {
    base_url: String,
    task: Option<JoinHandle<()>>,
    _config_dir: TempDir,
    _data_dir: TempDir,
    _registry_dir: TempDir,
}

impl TargetDaemon {
    async fn spawn() -> Self {
        let config_dir = TempDir::new().expect("pairing config tempdir");
        let data_dir = TempDir::new().expect("auth data tempdir");
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

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr: SocketAddr = listener.local_addr().expect("local addr");
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

// ---------------------------------------------------------------------
// Source side: an in-process router whose WorkspaceManager we keep a handle to
// so we can register the host-workspace first and inspect the flipped pointer
// afterwards.
// ---------------------------------------------------------------------

struct Source {
    router: axum::Router,
    workspaces: WorkspaceManager,
    _config_dir: TempDir,
    _data_dir: TempDir,
    _registry_dir: TempDir,
}

fn build_source() -> Source {
    let config_dir = TempDir::new().expect("pairing config tempdir");
    let data_dir = TempDir::new().expect("auth data tempdir");
    let registry_dir = TempDir::new().expect("registry tempdir");
    let registry_file = registry_dir.path().join("workspaces.json");
    std::env::set_var("NEOISM_CONFIG_DIR", config_dir.path());
    std::env::set_var("NEOISM_DAEMON_DATA_DIR", data_dir.path());
    std::env::set_var("NEOISM_WORKSPACE_REGISTRY", &registry_file);

    let auth = AuthService::bootstrap(data_dir.path()).expect("auth bootstrap");
    let workspaces = WorkspaceManager::bootstrap();

    let router = server::router(AppState {
        auth,
        sessions: SessionRegistry::shared(),
        // Clone so the route's AppState and the test share the same registry
        // (WorkspaceManager is an Arc-backed handle).
        workspaces: workspaces.clone(),
        pairing_tokens: PairingTokenStore::in_memory(),
        crdt: CrdtSyncHub::default(),
        paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
    });

    Source {
        router,
        workspaces,
        _config_dir: config_dir,
        _data_dir: data_dir,
        _registry_dir: registry_dir,
    }
}

/// Register a host-workspace on `manager` whose `root_dir` is `root`, returning
/// its workspace id. Uses the canonical `CreateHostWorkspace` dispatch.
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

/// Read a host-workspace's `running_on_host_id` from the source manager via the
/// `ListHostWorkspaces` dispatch.
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

/// POST a JSON body to the source router with the given bearer; return status +
/// parsed JSON.
async fn post_json(
    router: axum::Router,
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

// ---------------------------------------------------------------------
// The happy path: promote ships to a real target and flips the pointer.
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn promote_ships_to_target_and_flips_pointer() {
    if !git_available() {
        eprintln!("git unavailable; skipping");
        return;
    }

    const TOKEN: &str = "promote-provision-token-cafebabe";

    // The target clones into its own NEOISM_WORKSPACES_DIR; the cloud-provision
    // token gates BOTH halves (source auth + the target's /workspace/receive).
    let target_workspaces = TempDir::new().expect("target workspaces tempdir");
    let _g = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&target_workspaces.path().to_string_lossy()),
        ),
    ]);

    // --- bare "origin" the source repo can resolve a URL from. Committed
    // history travels by this URL; the working state travels via the snapshot.
    let bare = TempDir::new().expect("bare remote tempdir");
    git(bare.path(), &["init", "--bare"]);
    let bare_url = bare.path().to_string_lossy().to_string();

    // --- source repo: one committed file pushed to origin, then a tracked +
    // an untracked uncommitted change that only the snapshot will carry.
    let src = TempDir::new().expect("source tempdir");
    init_repo(src.path(), "tracked.txt", "line1\nline2\nline3\n");
    git(src.path(), &["remote", "add", "origin", &bare_url]);
    // Push so the target's clone of `origin` actually contains the commit.
    git(src.path(), &["push", "origin", "HEAD"]);
    std::fs::write(src.path().join("tracked.txt"), "line1\nCHANGED\nline3\n").unwrap();
    std::fs::write(src.path().join("new.txt"), "brand new\n").unwrap();

    // --- target daemon: a real server on an ephemeral port.
    let target = TargetDaemon::spawn().await;

    // --- source daemon + registered host-workspace bound to the repo path.
    let source = build_source();
    let workspace_id =
        register_host_workspace(&source.workspaces, "source-host", src.path());
    // Sanity: it starts running on the source host.
    assert_eq!(
        running_on_host(&source.workspaces, &workspace_id).as_deref(),
        Some("source-host"),
        "workspace should start on the source host"
    );

    // --- drive promote ---
    let (status, json) = post_json(
        source.router.clone(),
        "/workspace/promote",
        Some(TOKEN),
        serde_json::json!({
            "workspace_id": workspace_id,
            "target_url": target.base_url,
            "target_token": TOKEN,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "promote should succeed: {json}");

    // git_url was derived from the repo's origin remote.
    assert_eq!(
        json["git_url"].as_str(),
        Some(bare_url.as_str()),
        "git_url should be the derived origin remote: {json}"
    );

    // The target applied the snapshot: tracked.txt patched, new.txt written,
    // nothing rejected.
    let report = &json["target_apply_report"];
    let applied: Vec<String> = report["applied_files"]
        .as_array()
        .expect("applied_files array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        applied.iter().any(|p| p == "tracked.txt"),
        "tracked.txt should be applied on the target: {report}"
    );
    let wrote: Vec<String> = report["wrote_untracked"]
        .as_array()
        .expect("wrote_untracked array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        wrote.iter().any(|p| p == "new.txt"),
        "new.txt should be written on the target: {report}"
    );
    assert!(
        report["failed_hunks"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "expected no failed hunks: {report}"
    );

    // The flipped workspace in the response points at the target URL.
    assert_eq!(
        json["workspace"]["running_on_host_id"].as_str(),
        Some(target.base_url.as_str()),
        "response workspace must point at the target: {json}"
    );

    // --- the target actually has the workspace with the snapshot on disk ---
    let target_path = locate_target_clone(target_workspaces.path());
    assert_eq!(
        read(&target_path, "tracked.txt"),
        "line1\nCHANGED\nline3\n",
        "tracked patch must be applied on the target clone"
    );
    assert_eq!(read(&target_path, "new.txt"), "brand new\n");

    // --- the source's pointer flipped to the target URL ---
    assert_eq!(
        running_on_host(&source.workspaces, &workspace_id).as_deref(),
        Some(target.base_url.as_str()),
        "source running_on_host_id must flip to the target URL"
    );
}

/// The target clones each repo into a slug subdir under NEOISM_WORKSPACES_DIR.
/// We don't reconstruct the slug; just find the single child directory.
fn locate_target_clone(workspaces_dir: &Path) -> std::path::PathBuf {
    let mut dirs: Vec<_> = std::fs::read_dir(workspaces_dir)
        .expect("read workspaces dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    assert_eq!(
        dirs.len(),
        1,
        "expected exactly one cloned workspace under {}, found {:?}",
        workspaces_dir.display(),
        dirs
    );
    dirs.pop().unwrap()
}

// ---------------------------------------------------------------------
// No-remote case: a repo with no origin gets a clear 409 (Wave 6B: promote
// pushes the branch, so a remote is a hard repo-state requirement).
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn promote_without_git_remote_is_rejected() {
    if !git_available() {
        eprintln!("git unavailable; skipping");
        return;
    }

    const TOKEN: &str = "promote-provision-token-feedface";
    let target_workspaces = TempDir::new().expect("workspaces tempdir");
    let _g = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&target_workspaces.path().to_string_lossy()),
        ),
    ]);

    // Source repo with NO `origin` remote → promote must refuse.
    let src = TempDir::new().expect("source tempdir");
    init_repo(src.path(), "tracked.txt", "x\n");

    let source = build_source();
    let workspace_id =
        register_host_workspace(&source.workspaces, "source-host", src.path());

    let (status, json) = post_json(
        source.router.clone(),
        "/workspace/promote",
        Some(TOKEN),
        serde_json::json!({
            "workspace_id": workspace_id,
            // A bogus target URL is fine — we refuse before any network call.
            "target_url": "http://127.0.0.1:1",
            "target_token": TOKEN,
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "no-remote promote must be a 409: {json}"
    );

    // The pointer must NOT have flipped (still on the source host).
    assert_eq!(
        running_on_host(&source.workspaces, &workspace_id).as_deref(),
        Some("source-host"),
        "a refused promote must leave running_on_host_id untouched"
    );
}

/// Missing Authorization → the source-side cloud gate rejects before any git
/// work or network call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn promote_without_token_is_unauthorized() {
    const TOKEN: &str = "promote-provision-token-deadbeef";
    let target_workspaces = TempDir::new().expect("workspaces tempdir");
    let _g = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&target_workspaces.path().to_string_lossy()),
        ),
    ]);
    let source = build_source();
    let (status, _json) = post_json(
        source.router.clone(),
        "/workspace/promote",
        None,
        serde_json::json!({
            "workspace_id": "does-not-matter",
            "target_url": "http://127.0.0.1:1",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
