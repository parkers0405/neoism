//! Two-daemon integration test for the Wave 6B cross-host workspace
//! promote flow.
//!
//! The test stands up two fully independent daemons (A = source, B =
//! target) on ephemeral TCP ports, each with its own auth registry,
//! workspace registry, and paired-host store, then drives the whole
//! production path end to end:
//!
//! 1. **Automated pairing** — mint a pairing code on B (`POST /pair`),
//!    hand it to A (`POST /hosts/pair`); A claims it at B's
//!    `/pair/claim`, stores the device token, and `GET /hosts` lists
//!    the pairing without ever exposing the token.
//! 2. **Workspace + tabs on A** — a real git repo (with a bare "origin"
//!    remote on disk) is opened over the websocket and two sessions
//!    ("tabs") are created, one at the repo root and one in `src/`.
//! 3. **Promote A → B** — `POST /workspace/promote { target: "laptop-b" }`
//!    on A. A pushes the branch, captures the uncommitted working
//!    state, and posts `/workspace/receive` to B with the stored
//!    bearer (Wave 6B folded the stale branch's `/workspace/adopt`
//!    into the existing receive engine).
//! 4. **Deep assertions** — B's checkout exists on disk at the same
//!    commit as A's repo *including the uncommitted change*; B's
//!    HOST>WORKSPACE>TABS tree lists the workspace (same id + display
//!    name) with both tabs, cwds remapped under B's checkout; A no
//!    longer lists the project root or its sessions. The move is a
//!    move, not a metadata copy.
//!
//! Environment hygiene mirrors `workspace_ws_integration.rs`: every
//! env-touching test serialises through `ENV_LOCK` and restores
//! previous values via the `EnvGuard` RAII type.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use neoism_protocol::pairing::PairingCodeResponse;
use neoism_protocol::workspace::{
    ProjectRootSummary, WorkspaceClientMessage, WorkspaceServerMessage,
};
use neoism_workspace_daemon::auth::AuthService;
use neoism_workspace_daemon::handshake::PairingTokenStore;
use neoism_workspace_daemon::hosts::PairedHostStore;
use neoism_workspace_daemon::server::{self, AppState};
use neoism_workspace_daemon::workspace::WorkspaceManager;
use neoism_workspace_daemon::workspace_promote::PromoteWorkspaceResponse;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::{
    connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};

// ---------------------------------------------------------------------
// Env hygiene
// ---------------------------------------------------------------------

/// Tests in this file mutate process-global env vars. Serialise them
/// through a single lock so they can't race each other or the unit
/// tests living inside the crate.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard for the env vars these tests reach into. Captures the
/// previous values on construction and restores them on Drop.
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
// Wire-shape helpers (same rationale as workspace_ws_integration.rs:
// the daemon's service envelope enums are pub(crate), so the test
// redeclares the subset it speaks).
// ---------------------------------------------------------------------

#[derive(Serialize)]
enum ClientEnvelope<'a> {
    Workspace {
        request_id: u64,
        message: &'a WorkspaceClientMessage,
    },
}

type WsClient = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_client(addr: SocketAddr) -> WsClient {
    let url = format!("ws://{addr}/session");
    let (stream, _resp) = connect_async(&url).await.expect("websocket upgrade");
    stream
}

async fn send_workspace(
    ws: &mut WsClient,
    request_id: u64,
    message: &WorkspaceClientMessage,
) {
    let env = ClientEnvelope::Workspace {
        request_id,
        message,
    };
    let payload = serde_json::to_string(&env).expect("serialize envelope");
    ws.send(Message::Text(payload))
        .await
        .expect("send websocket frame");
}

/// Read frames until the next `WorkspaceReply`, skipping the daemon's
/// unsolicited pushes (git snapshot, agent `Disabled`, status polls).
async fn recv_workspace_timeout(
    ws: &mut WsClient,
    timeout: Duration,
) -> Option<WorkspaceServerMessage> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        let frame = tokio::time::timeout(remaining, ws.next()).await.ok()??;
        let text = match frame {
            Ok(Message::Text(t)) => t,
            Ok(Message::Binary(b)) => String::from_utf8_lossy(&b).into_owned(),
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            _ => return None,
        };
        let raw: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(payload) = raw.get("WorkspaceReply") else {
            continue;
        };
        #[derive(Deserialize)]
        struct WorkspacePayload {
            message: WorkspaceServerMessage,
        }
        if let Ok(parsed) = serde_json::from_value::<WorkspacePayload>(payload.clone()) {
            return Some(parsed.message);
        }
    }
}

/// Drain workspace frames until one satisfies `extract`, returning the
/// extracted value. Times out (returns None) after `timeout`.
async fn recv_until<T>(
    ws: &mut WsClient,
    timeout: Duration,
    mut extract: impl FnMut(WorkspaceServerMessage) -> Option<T>,
) -> Option<T> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::from_millis(0));
        let msg = recv_workspace_timeout(ws, remaining).await?;
        if let Some(value) = extract(msg) {
            return Some(value);
        }
    }
}

// ---------------------------------------------------------------------
// Daemon harness
// ---------------------------------------------------------------------

/// A live daemon bound to an ephemeral port, with every disk-touching
/// subsystem rooted in per-daemon temp dirs. Aborted on Drop.
struct Daemon {
    addr: SocketAddr,
    task: Option<JoinHandle<()>>,
    _config_dir: TempDir,
    _data_dir: TempDir,
    _registry_dir: TempDir,
}

impl Daemon {
    async fn spawn() -> Self {
        let config_dir = TempDir::new().expect("pairing config tempdir");
        let data_dir = TempDir::new().expect("auth data tempdir");
        let registry_dir = TempDir::new().expect("registry tempdir");
        let registry_file = registry_dir.path().join("workspaces.json");

        // These env vars are read once at bootstrap/load time (the
        // caller holds ENV_LOCK for the whole test), so spawning the
        // two daemons sequentially gives each its own registry.
        std::env::set_var("NEOISM_CONFIG_DIR", config_dir.path());
        std::env::set_var("NEOISM_DAEMON_DATA_DIR", data_dir.path());
        std::env::set_var("NEOISM_WORKSPACE_REGISTRY", &registry_file);

        let auth = AuthService::bootstrap(data_dir.path()).expect("auth bootstrap");
        let workspaces = WorkspaceManager::bootstrap();
        let pairing_tokens =
            PairingTokenStore::load(config_dir.path()).expect("pairing store load");
        let paired_hosts =
            PairedHostStore::load(data_dir.path()).expect("paired-host store load");

        let app = server::router(AppState {
            auth,
            sessions: neoism_workspace_daemon::sessions::SessionRegistry::shared(),
            workspaces,
            pairing_tokens,
            crdt: neoism_workspace_daemon::crdt::sync::CrdtSyncHub::default(),
            paired_hosts,
        });

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Daemon {
            addr,
            task: Some(task),
            _config_dir: config_dir,
            _data_dir: data_dir,
            _registry_dir: registry_dir,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

// ---------------------------------------------------------------------
// Git fixture
// ---------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A work repo with one commit and a bare `origin` remote next to it,
/// mirroring the real promote requirement (a remote the target can
/// clone). On a real two-machine move the remote is a hosted URL; the
/// file path stands in for it because both test daemons share a disk.
struct GitFixture {
    _root: TempDir,
    work: PathBuf,
}

impl GitFixture {
    fn create() -> Self {
        let root = TempDir::new().expect("git fixture tempdir");
        let origin = root.path().join("origin.git");
        let work = root.path().join("work");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        git(&origin, &["init", "--bare"]);
        git(&work, &["init"]);
        git(&work, &["config", "user.email", "test@example.com"]);
        git(&work, &["config", "user.name", "Neoism Test"]);
        std::fs::write(work.join("README.md"), "hello from daemon A\n").unwrap();
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::write(work.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "initial"]);
        git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let branch = git_stdout(&work, &["rev-parse", "--abbrev-ref", "HEAD"]);
        git(&work, &["push", "-u", "origin", &branch]);
        Self { _root: root, work }
    }

    fn head(&self) -> String {
        git_stdout(&self.work, &["rev-parse", "HEAD"])
    }
}

async fn list_project_roots(
    ws: &mut WsClient,
    request_id: u64,
) -> Vec<ProjectRootSummary> {
    send_workspace(ws, request_id, &WorkspaceClientMessage::ListProjectRoots).await;
    recv_until(ws, Duration::from_secs(5), |msg| match msg {
        WorkspaceServerMessage::ProjectRootList { project_roots } => Some(project_roots),
        _ => None,
    })
    .await
    .expect("ProjectRootList reply")
}

async fn open_project_root(
    ws: &mut WsClient,
    request_id: u64,
    path: PathBuf,
    init_if_missing: bool,
) -> ProjectRootSummary {
    send_workspace(
        ws,
        request_id,
        &WorkspaceClientMessage::OpenProjectRoot {
            path,
            init_if_missing,
        },
    )
    .await;
    recv_until(ws, Duration::from_secs(5), |msg| match msg {
        WorkspaceServerMessage::ProjectRootOpened { project_root } => Some(project_root),
        _ => None,
    })
    .await
    .expect("ProjectRootOpened reply")
}

// ---------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn promote_moves_workspace_between_two_daemons() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed; skipping");
        return;
    }

    // B clones adopted workspaces under this root (read at adopt time).
    let b_workspaces_root = TempDir::new().expect("target workspaces tempdir");
    let _env = EnvGuard::new(&[
        // Auto-approve so B's `/pair/claim` grants A's device token
        // without an interactive operator (the approval gate itself is
        // covered by the in-crate permissions/auth tests).
        ("NEOISM_AUTO_APPROVE", Some("1")),
        ("NEOISM_REQUIRE_AUTH", None),
        ("NEOISM_DAEMON_TOKEN", None),
        ("NEOISM_CLOUD_PROVISION_TOKEN", None),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(b_workspaces_root.path().to_str().unwrap()),
        ),
    ]);

    let fixture = GitFixture::create();
    let daemon_a = Daemon::spawn().await;
    let daemon_b = Daemon::spawn().await;
    let http = reqwest::Client::new();

    // ----- 1. Automated pairing: B mints a code, A claims it. -----
    let code: PairingCodeResponse = http
        .post(format!("{}/pair", daemon_b.base_url()))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("POST /pair on B")
        .json()
        .await
        .expect("pairing code body");

    let pair_resp = http
        .post(format!("{}/hosts/pair", daemon_a.base_url()))
        .json(&serde_json::json!({
            "name": "laptop-b",
            "base_url": daemon_b.base_url(),
            "code": code.code,
        }))
        .send()
        .await
        .expect("POST /hosts/pair on A");
    assert_eq!(pair_resp.status(), 200, "host pairing should succeed");
    let pair_body = pair_resp.text().await.unwrap();
    assert!(pair_body.contains("laptop-b"), "summary echoes the name");
    assert!(
        !pair_body.contains("\"token\""),
        "pairing reply must not leak the device token: {pair_body}"
    );

    let hosts_body = http
        .get(format!("{}/hosts", daemon_a.base_url()))
        .send()
        .await
        .expect("GET /hosts on A")
        .text()
        .await
        .unwrap();
    assert!(hosts_body.contains("laptop-b"));
    assert!(
        !hosts_body.contains("\"token\""),
        "GET /hosts must not leak tokens: {hosts_body}"
    );

    // ----- 2. Workspace + two tabs on A. -----
    let mut ws_a = connect_client(daemon_a.addr).await;
    let workspace = open_project_root(&mut ws_a, 1, fixture.work.clone(), false).await;

    send_workspace(
        &mut ws_a,
        2,
        &WorkspaceClientMessage::NewSession {
            cwd: None,
            label: Some("shell".into()),
        },
    )
    .await;
    recv_until(&mut ws_a, Duration::from_secs(5), |msg| match msg {
        WorkspaceServerMessage::SessionCreated { session } => Some(session),
        _ => None,
    })
    .await
    .expect("first SessionCreated");

    let src_cwd = fixture.work.join("src").to_string_lossy().into_owned();
    send_workspace(
        &mut ws_a,
        3,
        &WorkspaceClientMessage::NewSession {
            cwd: Some(src_cwd),
            label: Some("editor".into()),
        },
    )
    .await;
    recv_until(&mut ws_a, Duration::from_secs(5), |msg| match msg {
        WorkspaceServerMessage::SessionCreated { session } => Some(session),
        _ => None,
    })
    .await
    .expect("second SessionCreated");

    // ----- 3. Uncommitted (tracked) work that must travel. -----
    let mut readme = std::fs::read_to_string(fixture.work.join("README.md")).unwrap();
    readme.push_str("wip: uncommitted change\n");
    std::fs::write(fixture.work.join("README.md"), &readme).unwrap();

    let branch = git_stdout(&fixture.work, &["rev-parse", "--abbrev-ref", "HEAD"]);

    // ----- 4. Promote A -> B by paired-host name. -----
    let promote_resp = http
        .post(format!("{}/workspace/promote", daemon_a.base_url()))
        .json(&serde_json::json!({
            "workspace_id": workspace.id,
            "target": "laptop-b",
        }))
        .send()
        .await
        .expect("POST /workspace/promote on A");
    let status = promote_resp.status();
    let promote_body = promote_resp.text().await.unwrap();
    assert_eq!(status, 200, "promote should succeed: {promote_body}");
    let promoted: PromoteWorkspaceResponse =
        serde_json::from_str(&promote_body).expect("promote response shape");

    assert_eq!(promoted.workspace_id, workspace.id);
    assert_eq!(promoted.target, "laptop-b");
    assert_eq!(promoted.target_base_url, daemon_b.base_url());
    assert_eq!(promoted.sessions_moved, 2);
    assert!(promoted.uncommitted_diff_carried);
    assert_eq!(promoted.git_ref.as_deref(), Some(branch.as_str()));

    // ----- 5. The repo genuinely landed on B's disk. -----
    let remote_path = promoted.remote_path.clone();
    assert!(
        remote_path.starts_with(b_workspaces_root.path()),
        "checkout should land under B's workspaces root: {}",
        remote_path.display()
    );
    assert!(
        remote_path.join(".git").is_dir(),
        "real clone, not metadata"
    );
    assert_eq!(
        git_stdout(&remote_path, &["rev-parse", "HEAD"]),
        fixture.head(),
        "B is at A's HEAD (promote pushed the branch)"
    );
    let remote_readme = std::fs::read_to_string(remote_path.join("README.md")).unwrap();
    assert!(remote_readme.contains("hello from daemon A"));
    assert!(
        remote_readme.contains("wip: uncommitted change"),
        "uncommitted tracked diff must travel with the move"
    );
    assert!(remote_path.join("src/main.rs").is_file());

    // ----- 6. B's tree lists the workspace (same id + name) with both
    // tabs, cwds remapped under B's checkout. -----
    let mut ws_b = connect_client(daemon_b.addr).await;
    send_workspace(
        &mut ws_b,
        10,
        &WorkspaceClientMessage::ListHostWorkspaces { host_id: None },
    )
    .await;
    let b_workspaces = recv_until(&mut ws_b, Duration::from_secs(5), |msg| match msg {
        WorkspaceServerMessage::HostWorkspaceList { workspaces } => Some(workspaces),
        _ => None,
    })
    .await
    .expect("HostWorkspaceList on B");
    let adopted = b_workspaces
        .iter()
        .find(|w| w.id == workspace.id)
        .expect("B's tree lists the adopted workspace under the carried id");
    assert_eq!(adopted.title, workspace.name, "display name carried over");
    assert_eq!(
        adopted.root_dir.as_deref(),
        Some(remote_path.as_path()),
        "tree root_dir is B's checkout"
    );
    // B also registered the checkout as a project root.
    let b_roots = list_project_roots(&mut ws_b, 11).await;
    assert!(
        b_roots.iter().any(|r| r.path == remote_path),
        "B lists the checkout as a project root: {b_roots:?}"
    );

    send_workspace(
        &mut ws_b,
        12,
        &WorkspaceClientMessage::ListWorkspaceTabs {
            workspace_id: workspace.id.clone(),
        },
    )
    .await;
    let tabs = recv_until(&mut ws_b, Duration::from_secs(5), |msg| match msg {
        WorkspaceServerMessage::WorkspaceTabList { tabs } => Some(tabs),
        _ => None,
    })
    .await
    .expect("WorkspaceTabList on B");
    assert_eq!(tabs.len(), 2, "both tabs adopted: {tabs:?}");
    let shell = tabs.iter().find(|t| t.title == "shell").expect("shell tab");
    assert_eq!(shell.cwd.as_deref(), Some(remote_path.as_path()));
    let editor = tabs
        .iter()
        .find(|t| t.title == "editor")
        .expect("editor tab");
    assert_eq!(
        editor.cwd.as_deref(),
        Some(remote_path.join("src").as_path()),
        "tab cwd remapped under B's checkout"
    );

    // ----- 7. A reflects the move: project root + sessions are gone. -----
    let a_roots = list_project_roots(&mut ws_a, 4).await;
    assert!(
        !a_roots.iter().any(|r| r.id == workspace.id),
        "A must no longer list the promoted project root"
    );
    send_workspace(
        &mut ws_a,
        5,
        &WorkspaceClientMessage::GetProjectRootInfo {
            id: workspace.id.clone(),
        },
    )
    .await;
    let gone = recv_until(&mut ws_a, Duration::from_secs(5), |msg| match msg {
        WorkspaceServerMessage::Error { message } => Some(message),
        WorkspaceServerMessage::ProjectRootInfo { .. } => {
            panic!("workspace still resolvable on A after promote")
        }
        _ => None,
    })
    .await
    .expect("Error reply for moved workspace");
    assert!(gone.contains("no such workspace"));
}

/// Promote refuses to run without a git remote — there is nothing the
/// target could clone, so failing loudly beats a metadata-only "move".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn promote_without_git_remote_is_rejected() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed; skipping");
        return;
    }
    let _env = EnvGuard::new(&[
        ("NEOISM_AUTO_APPROVE", Some("1")),
        ("NEOISM_REQUIRE_AUTH", None),
        ("NEOISM_DAEMON_TOKEN", None),
        ("NEOISM_CLOUD_PROVISION_TOKEN", None),
    ]);

    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init"]);
    git(repo.path(), &["config", "user.email", "test@example.com"]);
    git(repo.path(), &["config", "user.name", "Neoism Test"]);
    std::fs::write(repo.path().join("a.txt"), "a\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "initial"]);

    let daemon = Daemon::spawn().await;
    let mut ws = connect_client(daemon.addr).await;
    let workspace = open_project_root(&mut ws, 1, repo.path().to_path_buf(), false).await;

    let resp = reqwest::Client::new()
        .post(format!("{}/workspace/promote", daemon.base_url()))
        .json(&serde_json::json!({
            "workspace_id": workspace.id,
            "target": "http://127.0.0.1:1",
        }))
        .send()
        .await
        .expect("POST /workspace/promote");
    assert_eq!(resp.status(), 409, "no-remote promote must conflict");
    let body = resp.text().await.unwrap();
    assert!(body.contains("remote"), "actionable error: {body}");

    // Nothing was moved or deleted locally.
    let roots = list_project_roots(&mut ws, 2).await;
    assert!(roots.iter().any(|r| r.id == workspace.id));
}

/// An unresolvable target (not paired, not a URL, not a tailnet peer)
/// is a 400 before any git work happens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn promote_to_unknown_target_is_a_bad_request() {
    let _env = EnvGuard::new(&[
        ("NEOISM_AUTO_APPROVE", None),
        ("NEOISM_REQUIRE_AUTH", None),
        ("NEOISM_DAEMON_TOKEN", None),
        ("NEOISM_CLOUD_PROVISION_TOKEN", None),
    ]);
    let workdir = TempDir::new().unwrap();
    let daemon = Daemon::spawn().await;
    let mut ws = connect_client(daemon.addr).await;
    let workspace =
        open_project_root(&mut ws, 1, workdir.path().to_path_buf(), true).await;

    let resp = reqwest::Client::new()
        .post(format!("{}/workspace/promote", daemon.base_url()))
        .json(&serde_json::json!({
            "workspace_id": workspace.id,
            "target": "definitely-not-a-paired-host",
        }))
        .send()
        .await
        .expect("POST /workspace/promote");
    assert_eq!(resp.status(), 400);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("/hosts/pair"),
        "error points at pairing: {body}"
    );
}
