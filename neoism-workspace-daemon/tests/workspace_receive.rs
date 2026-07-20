//! Integration test for `POST /workspace/receive` — the target side of
//! "promote a workspace to this host".
//!
//! The route composes the two existing "work from anywhere" primitives:
//!   * `workspace_provision::provision_from_git` — clone/reuse the repo.
//!   * `workspace_snapshot::apply_snapshot`      — replay the source's
//!     uncommitted working state on top.
//! and then registers the result via `OpenProjectRoot`, exactly like the
//! pre-existing `/workspace/from-git` handler.
//!
//! This test lives in `tests/` (not `--lib`) because the daemon's unit
//! tests are currently broken by an unrelated in-flight rename; the
//! integration target compiles against the green production lib (matching
//! the note atop `tests/workspace_snapshot.rs`).
//!
//! It drives the route through the real `axum::Router` via
//! `tower::ServiceExt::oneshot` (no TCP listener needed), presenting the
//! cloud provision token as `Authorization: Bearer`. The env-var hygiene
//! and `AppState` wiring are modelled on `daemon_token_smoke.rs`.

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use neoism_workspace_daemon::auth::AuthService;
use neoism_workspace_daemon::cloud_auth::ENV_CLOUD_PROVISION_TOKEN;
use neoism_workspace_daemon::handshake::PairingTokenStore;
use neoism_workspace_daemon::server::{self, AppState};
use neoism_workspace_daemon::workspace::{
    self as workspace_handler, ConnectionWorkspace, WorkspaceManager,
};
use neoism_workspace_daemon::workspace_promote::{
    find_matching_local_repo, normalize_git_url,
};
use neoism_workspace_daemon::workspace_snapshot::capture_uncommitted;
use tempfile::TempDir;
use tower::ServiceExt; // for Router::oneshot

// ---------------------------------------------------------------------
// Env hygiene. `NEOISM_CLOUD_PROVISION_TOKEN` / `NEOISM_WORKSPACES_DIR`
// / `NEOISM_DAEMON_DATA_DIR` / `NEOISM_CONFIG_DIR` /
// `NEOISM_WORKSPACE_REGISTRY` are process-globals; serialise through one
// lock and restore on Drop. (Same pattern as daemon_token_smoke.rs.)
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
// git helpers (mirroring tests/workspace_snapshot.rs).
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
// AppState builder. Same shape as daemon_token_smoke::Daemon::spawn, but
// we keep the router in-process (no TCP listener) and pin
// NEOISM_WORKSPACES_DIR so the target clone lands in a temp dir we own.
// ---------------------------------------------------------------------

struct Harness {
    router: axum::Router,
    /// A clone of the manager wired into the router. `WorkspaceManager` is
    /// Arc-backed, so this shares the same registry the route mutates —
    /// the 5E reuse test pre-registers a custom-path clone through it.
    workspaces: WorkspaceManager,
    _data_dir: TempDir,
    _config_dir: TempDir,
    _registry_dir: TempDir,
}

fn build_harness() -> Harness {
    let data_dir = TempDir::new().expect("auth data tempdir");
    let config_dir = TempDir::new().expect("pairing config tempdir");
    let registry_dir = TempDir::new().expect("registry tempdir");
    let registry_file = registry_dir.path().join("workspaces.json");
    std::env::set_var("NEOISM_CONFIG_DIR", config_dir.path());
    std::env::set_var("NEOISM_DAEMON_DATA_DIR", data_dir.path());
    std::env::set_var("NEOISM_WORKSPACE_REGISTRY", &registry_file);

    let auth = AuthService::bootstrap(data_dir.path()).expect("auth bootstrap");
    let workspaces = WorkspaceManager::bootstrap();
    let pairing_tokens = PairingTokenStore::in_memory();

    let router = server::router(AppState {
        auth,
        sessions: neoism_workspace_daemon::sessions::SessionRegistry::shared(),
        workspaces: workspaces.clone(),
        pairing_tokens,
        crdt: neoism_workspace_daemon::crdt::sync::CrdtSyncHub::default(),
        paired_hosts: neoism_workspace_daemon::hosts::PairedHostStore::in_memory(),
    });

    Harness {
        router,
        workspaces,
        _data_dir: data_dir,
        _config_dir: config_dir,
        _registry_dir: registry_dir,
    }
}

/// POST a JSON body to `path` through the router with the given bearer
/// token; return the status and parsed JSON body.
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
// The test.
// ---------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receive_clones_applies_snapshot_and_registers_workspace() {
    if !git_available() {
        eprintln!("git unavailable; skipping");
        return;
    }

    const TOKEN: &str = "receive-provision-token-cafef00d";

    // Target workspaces dir + the cloud provision gate.
    let workspaces_dir = TempDir::new().expect("workspaces tempdir");
    let _g = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&workspaces_dir.path().to_string_lossy()),
        ),
    ]);

    // --- source repo: one committed file, plus a tracked + an untracked
    // uncommitted change. This is exactly what travels: git history via
    // clone, working state via the snapshot.
    let src = TempDir::new().expect("source tempdir");
    init_repo(src.path(), "tracked.txt", "line1\nline2\nline3\n");
    std::fs::write(src.path().join("tracked.txt"), "line1\nCHANGED\nline3\n").unwrap();
    std::fs::write(src.path().join("new.txt"), "brand new\n").unwrap();

    let snapshot = capture_uncommitted(src.path()).expect("capture snapshot");
    assert!(
        !snapshot.tracked_patch.is_empty(),
        "expected a tracked diff in the snapshot"
    );
    assert!(
        snapshot
            .untracked
            .iter()
            .any(|(p, _)| p.to_string_lossy() == "new.txt"),
        "expected new.txt in the untracked snapshot"
    );

    let harness = build_harness();

    // --- drive the receive flow through the route ---
    let git_url = src.path().to_string_lossy().to_string();
    let request_body = serde_json::json!({
        "git_url": git_url,
        "snapshot": snapshot,
    });
    let (status, json) = post_json(
        harness.router.clone(),
        "/workspace/receive",
        Some(TOKEN),
        request_body,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "receive should succeed, body: {json}"
    );

    // Provision side: a fresh clone happened.
    assert_eq!(json["cloned"], serde_json::json!(true), "expected a clone");
    assert_eq!(json["reused"], serde_json::json!(false));

    // Registration side: a ProjectRootSummary with a real id + path.
    let workspace = &json["workspace"];
    assert!(
        workspace["id"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "workspace must have a non-empty id: {json}"
    );
    let target_path = json["path"].as_str().expect("path in response").to_string();
    assert!(
        target_path.starts_with(&workspaces_dir.path().to_string_lossy().to_string()),
        "target path {target_path} should be under the workspaces dir"
    );

    // Apply side: the report records the applied tracked file + untracked
    // write, with no rejected hunks.
    let report = &json["apply_report"];
    let applied: Vec<String> = report["applied_files"]
        .as_array()
        .expect("applied_files array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        applied.iter().any(|p| p == "tracked.txt"),
        "tracked.txt should be in applied_files: {report}"
    );
    let wrote: Vec<String> = report["wrote_untracked"]
        .as_array()
        .expect("wrote_untracked array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        wrote.iter().any(|p| p == "new.txt"),
        "new.txt should be in wrote_untracked: {report}"
    );
    assert!(
        report["failed_hunks"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "expected no failed hunks: {report}"
    );

    // --- on-disk truth on the target path ---
    let target = Path::new(&target_path);
    // Cloned history is present (the committed file at its committed value
    // before the patch, then the patch landed on top).
    assert_eq!(
        read(target, "tracked.txt"),
        "line1\nCHANGED\nline3\n",
        "tracked patch must be applied on the target"
    );
    // Untracked file from the snapshot was written.
    assert_eq!(read(target, "new.txt"), "brand new\n");

    // --- the workspace is registered in the manager's registry ---
    // List project roots over the same router and confirm our path is there.
    // We re-bootstrap a connection by hitting ListProjectRoots through the
    // workspace manager directly via a second receive (idempotent reuse).
    let (status2, json2) = post_json(
        harness.router.clone(),
        "/workspace/receive",
        Some(TOKEN),
        // Omit `snapshot` entirely — the request struct defaults it to an
        // empty WorkspaceSnapshot, so this exercises the "nothing to apply"
        // path (provision reuse only).
        serde_json::json!({
            "git_url": git_url,
            "pull": false,
        }),
    )
    .await;
    assert_eq!(status2, StatusCode::OK, "second receive (reuse): {json2}");
    // Second call reuses the existing clone (same slug) — proves the
    // registry/provision are keyed on the on-disk path, not minted anew.
    assert_eq!(json2["reused"], serde_json::json!(true), "expected reuse");
    assert_eq!(json2["cloned"], serde_json::json!(false));
    assert_eq!(
        json2["workspace"]["id"], workspace["id"],
        "reusing the same path must register the same workspace id"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receive_without_token_is_unauthorized() {
    const TOKEN: &str = "receive-provision-token-feedface";
    let workspaces_dir = TempDir::new().expect("workspaces tempdir");
    let _g = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&workspaces_dir.path().to_string_lossy()),
        ),
    ]);
    let harness = build_harness();

    // No Authorization header → the cloud_auth gate rejects before any
    // git work runs. `snapshot` is omitted (defaults to empty) so the
    // body still deserializes and we exercise the auth path, not a 422.
    let (status, _json) = post_json(
        harness.router.clone(),
        "/workspace/receive",
        None,
        serde_json::json!({
            "git_url": "/does/not/matter",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Wave-5 5E: when the daemon already has a registered local clone whose
/// `origin` remote matches the incoming snapshot's repo URL, `/workspace/
/// receive` REUSES that hand-made clone (at its custom path) and applies
/// the snapshot onto it — instead of cloning a fresh copy into the managed
/// workspaces dir.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receive_reuses_registered_local_clone_matched_by_origin() {
    if !git_available() {
        eprintln!("git unavailable; skipping");
        return;
    }

    const TOKEN: &str = "receive-provision-token-5e5e5e5e";

    let workspaces_dir = TempDir::new().expect("workspaces tempdir");
    let _g = EnvGuard::new(&[
        (ENV_CLOUD_PROVISION_TOKEN, Some(TOKEN)),
        (
            "NEOISM_WORKSPACES_DIR",
            Some(&workspaces_dir.path().to_string_lossy()),
        ),
    ]);

    // --- "remote" repo: the canonical history the snapshot was captured
    // against. Its filesystem path doubles as the git URL.
    let remote = TempDir::new().expect("remote tempdir");
    init_repo(remote.path(), "tracked.txt", "line1\nline2\nline3\n");
    let git_url = remote.path().to_string_lossy().to_string();

    // --- the SOURCE working copy: same repo, with a tracked + untracked
    // uncommitted change. That working state is what travels in the snapshot.
    let src = TempDir::new().expect("source tempdir");
    git(
        Path::new("."),
        &["clone", "--", &git_url, &src.path().to_string_lossy()],
    );
    std::fs::write(src.path().join("tracked.txt"), "line1\nCHANGED\nline3\n").unwrap();
    std::fs::write(src.path().join("new.txt"), "brand new\n").unwrap();
    let snapshot = capture_uncommitted(src.path()).expect("capture snapshot");

    // --- the TARGET's hand-made clone at a CUSTOM path (NOT under the
    // managed workspaces dir). `git clone` sets its `origin` to `git_url`,
    // which is exactly what 5E matches on.
    let custom = TempDir::new().expect("custom clone tempdir");
    let custom_clone = custom.path().join("my-handmade-checkout");
    git(
        Path::new("."),
        &["clone", "--", &git_url, &custom_clone.to_string_lossy()],
    );
    assert_eq!(
        read(&custom_clone, "tracked.txt"),
        "line1\nline2\nline3\n",
        "custom clone should hold the committed baseline before the snapshot"
    );

    let harness = build_harness();

    // Register the custom clone as a known project root so it becomes a
    // reuse CANDIDATE for receive (mirrors a user having opened it locally).
    let mut conn = ConnectionWorkspace::default();
    let _ = workspace_handler::handle(
        &harness.workspaces,
        &mut conn,
        None,
        None,
        neoism_protocol::workspace::WorkspaceClientMessage::OpenProjectRoot {
            path: custom_clone.clone(),
            init_if_missing: false,
        },
    );

    // --- drive receive. We send the git URL with a trailing slash to also
    // exercise the URL normalization (origin has no trailing slash).
    let (status, json) = post_json(
        harness.router.clone(),
        "/workspace/receive",
        Some(TOKEN),
        serde_json::json!({
            "git_url": format!("{git_url}/"),
            "snapshot": snapshot,
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "receive should succeed, body: {json}"
    );

    // Reuse — NOT a clone — and the reused path is our custom hand-made
    // checkout, NOT a fresh dir under the managed workspaces root.
    assert_eq!(
        json["cloned"],
        serde_json::json!(false),
        "should reuse, not clone: {json}"
    );
    assert_eq!(
        json["reused"],
        serde_json::json!(true),
        "should report reuse: {json}"
    );
    let target_path = json["path"].as_str().expect("path in response").to_string();
    // The registry canonicalizes the path it stores, so compare against the
    // canonical form of our custom clone.
    let canonical_custom = std::fs::canonicalize(&custom_clone)
        .unwrap_or_else(|_| custom_clone.clone())
        .to_string_lossy()
        .to_string();
    assert_eq!(
        target_path, canonical_custom,
        "receive must land on the existing custom clone path"
    );
    assert!(
        !target_path.starts_with(&workspaces_dir.path().to_string_lossy().to_string()),
        "receive must NOT have cloned into the managed workspaces dir: {target_path}"
    );

    // The snapshot was applied in place onto the reused clone.
    assert_eq!(
        read(&custom_clone, "tracked.txt"),
        "line1\nCHANGED\nline3\n",
        "tracked patch must be applied onto the reused clone"
    );
    assert_eq!(read(&custom_clone, "new.txt"), "brand new\n");

    // And the managed workspaces dir holds no clone of this repo.
    let managed_slug =
        neoism_workspace_daemon::workspace_provision::slug_for_git_url(&git_url);
    assert!(
        !workspaces_dir.path().join(managed_slug).exists(),
        "no managed clone should have been created when reusing a local clone"
    );
}

// ---------------------------------------------------------------------
// Wave-5 5E unit-level coverage of the URL normalization + matcher.
//
// These live here (not in `src/workspace_promote.rs`'s `mod tests`) because
// the daemon's in-crate `--lib` test build is currently red from an
// unrelated in-flight rename; the integration target compiles against the
// green production lib. The matcher functions are `pub`, so they're
// reachable from here.
// ---------------------------------------------------------------------

#[test]
fn normalize_collapses_gratuitous_url_differences() {
    // https vs ssh URL form of the same repo collapse equal.
    assert_eq!(
        normalize_git_url("https://github.com/owner/repo.git"),
        normalize_git_url("ssh://git@github.com/owner/repo")
    );
    // scp-style ssh matches the URL forms too.
    assert_eq!(
        normalize_git_url("git@github.com:owner/repo.git"),
        normalize_git_url("https://github.com/owner/repo")
    );
    // trailing slash + trailing .git + whitespace are all ignored.
    assert_eq!(
        normalize_git_url("  https://h/o/r.git/  "),
        normalize_git_url("https://h/o/r")
    );
}

#[test]
fn normalize_keeps_distinct_repos_distinct() {
    assert_ne!(
        normalize_git_url("https://github.com/owner/repo.git"),
        normalize_git_url("https://github.com/owner/other.git")
    );
    // An explicit :port is NOT mistaken for the scp `host:path` separator.
    assert_eq!(
        normalize_git_url("ssh://git@host:2222/owner/repo.git"),
        "host:2222/owner/repo"
    );
}

#[test]
fn find_matching_local_repo_skips_missing_and_empty_target() {
    use std::path::PathBuf;
    // A non-existent candidate is silently skipped → no match.
    assert!(find_matching_local_repo(
        "https://example.com/o/r.git",
        &[PathBuf::from("/definitely/does/not/exist")]
    )
    .is_none());
    // An empty target never matches.
    assert!(find_matching_local_repo("   ", &[PathBuf::from("/tmp")]).is_none());
}

#[test]
fn find_matching_local_repo_matches_origin_by_remote() {
    if !git_available() {
        eprintln!("git unavailable; skipping");
        return;
    }
    let dir = TempDir::new().unwrap();
    let repo = dir.path();
    git(repo, &["init"]);
    // Add an `origin` whose URL is an ssh-form spelling of the target.
    git(
        repo,
        &["remote", "add", "origin", "git@github.com:owner/repo.git"],
    );

    // The incoming snapshot's URL is the https form of the same repo.
    let matched = find_matching_local_repo(
        "https://github.com/owner/repo.git",
        &[std::path::PathBuf::from("/missing"), repo.to_path_buf()],
    );
    assert_eq!(matched.as_deref(), Some(repo));

    // A different repo URL does not match this candidate.
    assert!(find_matching_local_repo(
        "https://github.com/owner/other.git",
        &[repo.to_path_buf()],
    )
    .is_none());
}
