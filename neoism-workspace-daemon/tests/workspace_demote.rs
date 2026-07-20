//! Integration test for `POST /workspace/demote` — the FLIP-BACK side of the
//! move plane ("bring a workspace HOME to *this* host").
//!
//! Demote is promote-in-reverse and invents no new sync: the *local* daemon
//! (this host) asks the workspace's CURRENT home to `/workspace/promote` it
//! back. So this test is fully bidirectional and uses the production topology:
//!
//!   * a REAL "remote home" daemon on an ephemeral `127.0.0.1:0` holding
//!     workspace W bound to a git repo (origin remote + tracked + untracked
//!     uncommitted change),
//!   * a REAL "local" daemon (also on an ephemeral port, because the remote
//!     home POSTs the workspace back to its `/workspace/receive`) whose
//!     registry knows W as homed on the remote (its `running_on_host_id`
//!     resolves, via the host registry's `daemon_url`, to the remote home's
//!     URL), and whose `NEOISM_HOST_URL` is its own receive address.
//!
//! The drive: `POST /workspace/demote { workspace_id }` on the local daemon.
//! Assertions: the remote home shipped W to the local's `NEOISM_WORKSPACES_DIR`
//! (tracked patch + untracked file land on disk), the pass-through promote
//! result points the workspace at the local URL, and the no-`NEOISM_HOST_URL`
//! guard returns a clear 400.
//!
//! Lives in `tests/` (not `--lib`) because the daemon's unit tests are red from
//! an unrelated in-flight rename — same note as `tests/workspace_promote.rs`.

use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use neoism_protocol::workspace::{
    HostSummary, WorkspaceClientMessage, WorkspaceServerMessage,
};
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
use tower::ServiceExt; // Router::oneshot for driving the local side.

// ---------------------------------------------------------------------
// Env hygiene. The same process globals as workspace_promote.rs; serialise
// through one lock and restore on Drop.
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

    /// Mutate one env var while the guard is held (restored on Drop because the
    /// original value was captured when the guard was built — but only if the
    /// key was part of the initial set; for keys mutated here we capture lazily).
    fn set(&mut self, key: &'static str, value: Option<&str>) {
        if !self.prev.iter().any(|(k, _)| *k == key) {
            self.prev.push((key, std::env::var(key).ok()));
        }
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
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
// A real daemon on an ephemeral port. We keep the WorkspaceManager handle so
// tests can seed/inspect registry state; the AppState's manager is the same
// Arc-backed handle so the bound server and the test share one registry.
// ---------------------------------------------------------------------

struct Daemon {
    base_url: String,
    /// A router clone for driving requests via `oneshot` without a network hop.
    router: axum::Router,
    workspaces: WorkspaceManager,
    task: Option<JoinHandle<()>>,
    _config_dir: TempDir,
    _data_dir: TempDir,
    _registry_dir: TempDir,
}

impl Daemon {
    /// Build a daemon whose registry persists under a fresh tempfile, bind it to
    /// an ephemeral port, and serve it. Caller must hold the ENV_LOCK (sets
    /// `NEOISM_*_DIR` / `NEOISM_WORKSPACE_REGISTRY` globals at bootstrap time).
    async fn spawn() -> Self {
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
            // Clone so the bound server, the oneshot router, and the test share
            // the same Arc-backed registry.
            workspaces: workspaces.clone(),
            pairing_tokens: PairingTokenStore::in_memory(),
            crdt: CrdtSyncHub::default(),
            paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
        });

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr: SocketAddr = listener.local_addr().expect("local addr");
        let serve_router = router.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                serve_router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });

        Daemon {
            base_url: format!("http://{addr}"),
            router,
            workspaces,
            task: Some(task),
            _config_dir: config_dir,
            _data_dir: data_dir,
            _registry_dir: registry_dir,
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
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
            title: Some("demote-me".to_string()),
            root_dir: Some(root.to_path_buf()),
        },
    );
    outcome
        .replies
        .iter()
        .find_map(|reply| match reply {
            WorkspaceServerMessage::HostWorkspaceUpserted { workspace } => {
                Some(workspace.id.clone())
            }
            _ => None,
        })
        .expect("CreateHostWorkspace must yield a workspace id")
}

/// Seed the LOCAL registry's view of W: a host whose `daemon_url` is the remote
/// home URL, plus the host-workspace W homed (`running_on_host_id`) on that
/// host. Host presence and workspace ownership are separate daemon-owned
/// records: `UpsertHost` advertises the dial URL, `CreateHostWorkspace`
/// creates W, and `SwitchHostWorkspace` marks it active.
fn seed_remote_homed_workspace(
    manager: &WorkspaceManager,
    workspace_id: &str,
    home_host_id: &str,
    home_daemon_url: &str,
    root: &Path,
) {
    let mut conn = ConnectionWorkspace::default();
    workspace::handle(
        manager,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::UpsertHost {
            host: HostSummary {
                id: home_host_id.to_string(),
                label: home_host_id.to_string(),
                online: true,
                peer_identity: None,
                last_seen: 0,
                active_workspace_id: None,
                daemon_url: Some(home_daemon_url.to_string()),
            },
        },
    );
    workspace::handle(
        manager,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::CreateHostWorkspace {
            host_id: home_host_id.to_string(),
            workspace_id: Some(workspace_id.to_string()),
            title: Some("demote-me".to_string()),
            root_dir: Some(root.to_path_buf()),
        },
    );
    workspace::handle(
        manager,
        &mut conn,
        None,
        None,
        WorkspaceClientMessage::SwitchHostWorkspace {
            workspace_id: workspace_id.to_string(),
        },
    );
}

/// Read a host-workspace's `running_on_host_id` via `ListHostWorkspaces`.
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

/// POST a JSON body to a router via `oneshot`; return status + parsed JSON.
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

/// The target clones each repo into a slug subdir under NEOISM_WORKSPACES_DIR.
/// We don't reconstruct the slug; just find the single child directory.
fn locate_clone(workspaces_dir: &Path) -> std::path::PathBuf {
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
// Happy path: demote asks the remote home to promote W back here; W's files
// land locally and the pointer flips home.
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn demote_brings_workspace_home_to_this_host() {
    if !git_available() {
        eprintln!("git unavailable; skipping");
        return;
    }

    const TOKEN: &str = "demote-provision-token-cafef00d";

    // The local daemon receives the demoted workspace into this dir.
    let local_workspaces = TempDir::new().expect("local workspaces tempdir");

    // --- bare "origin" so the remote home's repo resolves a git URL; the
    // committed history travels by this URL, the working state by the snapshot.
    let bare = TempDir::new().expect("bare remote tempdir");
    git(bare.path(), &["init", "--bare"]);
    let bare_url = bare.path().to_string_lossy().to_string();

    // --- the remote home's repo: a committed file pushed to origin, then a
    // tracked + an untracked uncommitted change only the snapshot carries.
    let home_repo = TempDir::new().expect("home repo tempdir");
    init_repo(home_repo.path(), "tracked.txt", "line1\nline2\nline3\n");
    git(home_repo.path(), &["remote", "add", "origin", &bare_url]);
    git(home_repo.path(), &["push", "origin", "HEAD"]);
    std::fs::write(
        home_repo.path().join("tracked.txt"),
        "line1\nCHANGED\nline3\n",
    )
    .unwrap();
    std::fs::write(home_repo.path().join("new.txt"), "brand new\n").unwrap();

    // Hold the env lock for the whole flow: bring-up sets per-daemon registry
    // env at bootstrap; drive-time needs the cloud token + the LOCAL's
    // NEOISM_WORKSPACES_DIR + NEOISM_HOST_URL all live when demote runs.
    let mut env = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        // The remote home will POST W's snapshot to the LOCAL's
        // /workspace/receive, which clones into NEOISM_WORKSPACES_DIR.
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&local_workspaces.path().to_string_lossy()),
        ),
    ]);

    // --- stand up both real daemons. Each bootstraps its own registry file
    // (set inside spawn() while we hold the env lock).
    let remote_home = Daemon::spawn().await;
    let local = Daemon::spawn().await;

    // The LOCAL host's advertised receive URL. The remote home's promote ships
    // back here; demote reads this to fill promote's `target_url`.
    env.set("NEOISM_HOST_URL", Some(&local.base_url));

    // --- the remote home's registry: W bound to its repo, homed on the remote.
    let workspace_id =
        register_host_workspace(&remote_home.workspaces, "remote-home", home_repo.path());

    // --- the local's registry: the same W, but homed on the remote host whose
    // `daemon_url` is the remote home's base URL (how the local resolves the
    // current home → a dialable URL).
    //
    // The local's view of W carries the REMOTE home's on-disk path as
    // `root_dir`. In production that path lives on the remote machine and is
    // absent on the local host, so Wave-5 5E's "reuse a local clone matched by
    // origin" must NOT fire here — the local has no such clone and should
    // clone fresh into NEOISM_WORKSPACES_DIR. We model that by pointing the
    // local registry at a non-existent path (both daemons share one filesystem
    // in this single-process test, so reusing `home_repo.path()` verbatim would
    // spuriously trigger 5E reuse).
    let local_view_root =
        std::env::temp_dir().join("neoism-demote-remote-home-root-not-present-xyz");
    seed_remote_homed_workspace(
        &local.workspaces,
        &workspace_id,
        "remote-home",
        &remote_home.base_url,
        &local_view_root,
    );
    // Sanity: from the local's view W is homed on the remote host id.
    assert_eq!(
        running_on_host(&local.workspaces, &workspace_id).as_deref(),
        Some("remote-home"),
        "local should see W homed on the remote host before demote"
    );

    // --- drive demote on the LOCAL daemon ---
    let (status, json) = post_json(
        local.router.clone(),
        "/workspace/demote",
        Some(TOKEN),
        serde_json::json!({
            "workspace_id": workspace_id,
            "target_token": TOKEN,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "demote should succeed: {json}");

    // The pass-through promote result: git_url derived from the home's origin.
    assert_eq!(
        json["git_url"].as_str(),
        Some(bare_url.as_str()),
        "demote should pass through the home's derived git_url: {json}"
    );

    // The remote home applied + shipped the snapshot; the report is verbatim.
    let report = &json["target_apply_report"];
    let applied: Vec<String> = report["applied_files"]
        .as_array()
        .expect("applied_files array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        applied.iter().any(|p| p == "tracked.txt"),
        "tracked.txt should be applied on the local clone: {report}"
    );
    let wrote: Vec<String> = report["wrote_untracked"]
        .as_array()
        .expect("wrote_untracked array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        wrote.iter().any(|p| p == "new.txt"),
        "new.txt should be written on the local clone: {report}"
    );

    // The flipped workspace points at the LOCAL URL — it's homed here now.
    assert_eq!(
        json["workspace"]["running_on_host_id"].as_str(),
        Some(local.base_url.as_str()),
        "demoted workspace must point at the local URL: {json}"
    );

    // The workspace actually landed on the local daemon's disk.
    let local_clone = locate_clone(local_workspaces.path());
    assert_eq!(
        read(&local_clone, "tracked.txt"),
        "line1\nCHANGED\nline3\n",
        "tracked patch must be applied on the local clone"
    );
    assert_eq!(read(&local_clone, "new.txt"), "brand new\n");

    // The remote home's pointer flipped to the local URL (it shipped W away).
    assert_eq!(
        running_on_host(&remote_home.workspaces, &workspace_id).as_deref(),
        Some(local.base_url.as_str()),
        "remote home's running_on_host_id must flip to the local URL"
    );
}

// ---------------------------------------------------------------------
// Guard: demote without NEOISM_HOST_URL → 400 (we have nowhere to receive at).
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demote_without_host_url_is_bad_request() {
    const TOKEN: &str = "demote-provision-token-deadc0de";

    let local_workspaces = TempDir::new().expect("workspaces tempdir");
    // Explicitly clear NEOISM_HOST_URL for this case.
    let _env = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&local_workspaces.path().to_string_lossy()),
        ),
        ("NEOISM_HOST_URL", None),
    ]);

    let local = Daemon::spawn().await;
    // Seed a workspace homed on some remote so resolution would otherwise
    // proceed; the missing host URL must short-circuit with a 400.
    seed_remote_homed_workspace(
        &local.workspaces,
        "ws-needs-home",
        "remote-home",
        "http://127.0.0.1:1",
        Path::new("/tmp/does-not-matter"),
    );

    let (status, json) = post_json(
        local.router.clone(),
        "/workspace/demote",
        Some(TOKEN),
        serde_json::json!({ "workspace_id": "ws-needs-home" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "demote without NEOISM_HOST_URL must be a 400: {json}"
    );
}

// ---------------------------------------------------------------------
// No-op: a workspace already homed at THIS host returns 200 with a clear
// message (no network call to a remote home).
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demote_already_home_is_noop() {
    if !git_available() {
        eprintln!("git unavailable; skipping");
        return;
    }

    const TOKEN: &str = "demote-provision-token-0ddba11";

    let local_workspaces = TempDir::new().expect("workspaces tempdir");
    let mut env = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&local_workspaces.path().to_string_lossy()),
        ),
    ]);

    let local = Daemon::spawn().await;
    // This host advertises its own URL; the workspace's home resolves to it.
    env.set("NEOISM_HOST_URL", Some(&local.base_url));

    // Register W as homed on a host whose daemon_url IS this local URL.
    let repo = TempDir::new().expect("repo tempdir");
    init_repo(repo.path(), "f.txt", "x\n");
    seed_remote_homed_workspace(
        &local.workspaces,
        "ws-home",
        "this-host",
        &local.base_url,
        repo.path(),
    );

    let (status, json) = post_json(
        local.router.clone(),
        "/workspace/demote",
        Some(TOKEN),
        serde_json::json!({ "workspace_id": "ws-home" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "demote of an already-home workspace must be a 200 no-op: {json}"
    );
    assert_eq!(
        json["noop"].as_bool(),
        Some(true),
        "no-op response must flag noop=true: {json}"
    );
}

/// Missing Authorization → the cloud gate rejects before any resolution work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn demote_without_token_is_unauthorized() {
    const TOKEN: &str = "demote-provision-token-feedbeef";
    let local_workspaces = TempDir::new().expect("workspaces tempdir");
    let _env = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&local_workspaces.path().to_string_lossy()),
        ),
    ]);
    let local = Daemon::spawn().await;
    let (status, _json) = post_json(
        local.router.clone(),
        "/workspace/demote",
        None,
        serde_json::json!({ "workspace_id": "does-not-matter" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
