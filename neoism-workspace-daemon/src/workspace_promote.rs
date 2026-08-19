//! Source-side helpers for `POST /workspace/promote` — the keystone of the
//! "work from anywhere" move plane.
//!
//! Promote is the *source* half of a workspace home relocation. It composes
//! primitives that already exist (it invents no new sync):
//!
//!   1. derive the workspace's git URL from its `origin` remote          (here)
//!   2. [`crate::workspace_snapshot::capture_uncommitted`] the working state
//!   3. `POST {target}/workspace/receive` — the target clones + replays it
//!      ([`crate::server::workspace_receive`], Wave 4A)
//!   4. flip `running_on_host_id` to the target via the canonical
//!      `MoveWorkspaceToHost` dispatch so subscribers see
//!      `WorkspaceControlChanged`
//!
//! This module owns only the blocking/IO-free git derivation and the request
//! shape; the orchestration (auth gate, `spawn_blocking`, reqwest call, pointer
//! flip) lives in [`crate::server`] next to `/workspace/receive` so the two
//! halves of a move read together.
//!
//! ## Two sync planes (see `WORK_FROM_ANYWHERE.md`)
//! Promote is the MOVE plane: git history + an uncommitted-diff snapshot
//! relocate a workspace's home. It deliberately does *not* touch the LIVE
//! plane (the CRDT hub): once the target rebuilds, the live plane picks up
//! there. v1 requires a git remote so committed history can travel by URL
//! rather than being bundled.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace_snapshot::WorkspaceSnapshot;

/// Request body for `POST /workspace/promote`.
///
/// * `workspace_id` — the workspace to relocate. Resolved first against the
///   host-workspace tree (`WorkspaceSummary`, the drag-a-workspace path) and
///   then against the project-root registry (`ProjectRootSummary`), so both
///   the desktop gesture and a plain HTTP caller work. Its on-disk root is
///   the repo we push + snapshot and whose `origin` we read.
/// * `target` — where to move it (Wave 6B target resolution): a paired-host
///   name (see `POST /hosts/pair`), an explicit `http(s)://host:port` base
///   URL, or a tailnet peer hostname (resolved via `tailscale status`,
///   default daemon port). Accepts the legacy field name `target_url` so the
///   existing desktop `move_workspace` client keeps working unchanged.
/// * `target_token` — bearer presented to the target's cloud-provision gate
///   ([`crate::cloud_auth::authorize_provision`]). Optional: when the target
///   resolves to a paired host its stored device token is used instead.
///   Accepts the 6B field name `token`.
/// * `git_remote` — override the derived git URL (e.g. a tailnet-reachable
///   mirror). When absent we read the repo's `origin` remote.
#[derive(Debug, Clone, Deserialize)]
pub struct PromoteWorkspaceRequest {
    pub workspace_id: String,
    #[serde(alias = "target_url")]
    pub target: String,
    #[serde(default, alias = "token")]
    pub target_token: Option<String>,
    #[serde(default)]
    pub git_remote: Option<String>,
}

/// Response body for `POST /workspace/promote`.
///
/// * `workspace` — the host-workspace *after* the pointer flip
///   (`running_on_host_id` now points at the target). Same `WorkspaceSummary`
///   the `MoveWorkspaceToHost` dispatch broadcasts as `WorkspaceControlChanged`.
/// * `target_apply_report` — the target's [`crate::workspace_snapshot::ApplyReport`]
///   verbatim, so the caller sees which hunks landed / were rejected remotely.
/// * `git_url` — the URL that was shipped to the target (derived or overridden).
///
/// `Deserialize` so the `/workspace/demote` orchestrator (which drives a remote
/// home's `/workspace/promote`) can parse the result back and pass it through.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromoteWorkspaceResponse {
    /// The host-workspace *after* the pointer flip (`running_on_host_id` now
    /// points at the target) — same `WorkspaceSummary` the
    /// `MoveWorkspaceToHost` dispatch broadcasts as `WorkspaceControlChanged`.
    /// `None` when the promoted id named a bare project root with no
    /// host-workspace in the tree (HTTP-only callers).
    #[serde(default)]
    pub workspace: Option<neoism_protocol::workspace::WorkspaceSummary>,
    pub target_apply_report: crate::workspace_snapshot::ApplyReport,
    pub git_url: String,
    /// Summary of the best-effort AI-agent session handoff that runs *after*
    /// the file/workspace move lands (Wave 4C-agent). Always present; an
    /// all-zero / error-laden summary means the agent move was attempted but
    /// did not (fully) succeed — which, by design, does **not** fail the
    /// promote. `#[serde(default)]` so `/workspace/demote` can still parse an
    /// older home's promote reply that predates this field.
    #[serde(default)]
    pub agent_ship: AgentShipSummary,
    // --- Wave 6B additions (all defaulted so /workspace/demote can parse a
    // reply from an older home daemon). ---
    /// Id of the workspace that was moved, echoed back.
    #[serde(default)]
    pub workspace_id: String,
    /// The `target` string the caller passed, echoed back.
    #[serde(default)]
    pub target: String,
    /// Base URL the move actually went to (after paired-host / tailnet
    /// resolution).
    #[serde(default)]
    pub target_base_url: String,
    /// Branch that was pushed + shipped (`None` only in legacy replies).
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    /// Checkout path on the target machine.
    #[serde(default)]
    pub remote_path: PathBuf,
    /// How many sessions ("tabs") were re-created on the target.
    #[serde(default)]
    pub sessions_moved: usize,
    /// Whether uncommitted working state (tracked diff and/or untracked
    /// files) travelled with the move.
    #[serde(default)]
    pub uncommitted_diff_carried: bool,
}

/// Best-effort summary of the agent-session handoff a promote performs after
/// the workspace files land on the target ("shut the laptop, agent keeps
/// working"). Surfaced in [`PromoteWorkspaceResponse`] so the caller can see
/// what happened *without* an agent failure ever failing the promote.
///
/// * `exported` — number of session bundles the SOURCE agent-server returned
///   for the promoted workspace root.
/// * `imported` — number of bundles the TARGET daemon confirmed it imported
///   into its local agent-server (resumable on the new home).
/// * `errors` — human-readable lines for each step that failed. Empty on a
///   clean handoff.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentShipSummary {
    pub exported: usize,
    pub imported: usize,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Errors raised before / around the network round trip. The target's own
/// apply is best-effort and reported via [`crate::workspace_snapshot::ApplyReport`]
/// rather than as an error here (mirroring `/workspace/receive`).
#[derive(Debug, thiserror::Error)]
pub enum PromoteError {
    #[error("no such workspace: {0}")]
    NoSuchWorkspace(String),
    #[error("workspace {0} has no on-disk root_dir to promote")]
    NoRootDir(String),
    #[error("workspace root does not exist: {0}")]
    RootMissing(PathBuf),
    /// The repo has no usable git remote. v1 requires one so committed history
    /// can travel to the target by URL.
    #[error(
        "promote requires a git remote the target can clone \
         (add one with `git remote add origin <url>`): {0}"
    )]
    NoGitRemote(String),
    /// Wave 6B: promote pushes the current branch so committed-but-unpushed
    /// work travels; a detached HEAD has no branch to push.
    #[error("workspace is on a detached HEAD; check out a branch before promoting")]
    DetachedHead,
    /// Wave 6B: a git subprocess (most importantly the pre-ship `git push`)
    /// failed. Without the push, committed-but-unpushed work would silently
    /// not travel — the exact "metadata-only move" failure 6B exists to fix.
    #[error("git command failed: {0}")]
    Git(String),
    #[error("failed to capture workspace snapshot: {0}")]
    Snapshot(#[from] crate::workspace_snapshot::SnapshotError),
}

/// Body we `POST` to the target's `/workspace/receive`. Field names match
/// [`crate::server::ReceiveWorkspaceRequest`] (`ref` for the git ref) so the
/// target deserializes it directly.
///
/// Wave 6B extends the payload with the workspace's *identity* and *tabs*:
/// `title` (display name), `sessions` (workspace-relative cwds the target
/// remaps under its checkout) and `preferences` (carried verbatim), so the
/// move relocates the whole workspace, not just its files.
#[derive(Debug, Clone, Serialize)]
pub struct ReceivePayload {
    pub git_url: String,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub snapshot: WorkspaceSnapshot,
    pub workspace_id: Option<String>,
    pub title: Option<String>,
    pub sessions: Vec<PortableSession>,
    pub preferences: Option<neoism_protocol::workspace::WorkplacePreferences>,
    pub source_host: Option<String>,
}

/// A session ("tab") flattened for transport (Wave 6B). `cwd` is
/// workspace-relative (`"."` for the root) so the target can remap it under
/// its own checkout path. Ids are preserved (they're UUIDs) so a round-trip
/// promote doesn't mint duplicate tabs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableSession {
    pub id: String,
    pub cwd: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub last_active: i64,
}

/// Resolve the git URL to ship to the target.
///
/// Prefers an explicit `git_remote` override; otherwise reads the repo's
/// `origin` remote via `git -C <path> remote get-url origin`. Returns
/// [`PromoteError::NoGitRemote`] when neither is available — v1 requires a
/// remote so committed history travels by URL rather than being bundled.
///
/// Blocking (shells out to `git`); call from `spawn_blocking`.
pub fn derive_git_url(
    repo: &Path,
    override_remote: Option<&str>,
) -> Result<String, PromoteError> {
    if let Some(remote) = override_remote {
        let trimmed = remote.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    git_origin_url(repo)
        .ok_or_else(|| PromoteError::NoGitRemote(repo.display().to_string()))
}

/// `git -C <repo> remote get-url origin`, trimmed. `None` when there is no
/// `origin` remote (fresh repo / local-only) or git errors. We intentionally
/// only consult `origin`: it is the conventional canonical remote and keeping
/// the contract to one name keeps the error message ("promote requires a git
/// remote") unambiguous.
pub fn git_origin_url(repo: &Path) -> Option<String> {
    let output = crate::hidden_std_command("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// Wave-5 5E: normalize a git remote URL so two spellings of the same repo
/// compare equal. Reduces the common gratuitous differences:
///
///   * surrounding whitespace,
///   * a single trailing `.git` suffix,
///   * a trailing `/`,
///   * `scp`-style ssh (`git@host:owner/repo`) vs URL ssh
///     (`ssh://git@host/owner/repo`) — both collapse to `host/owner/repo`,
///   * the scheme (`https://`, `http://`, `ssh://`, `git://`) and any
///     `user@` userinfo, so an https clone and an ssh clone of the same
///     remote still match.
///
/// This is deliberately conservative — it normalizes *form*, not identity:
/// it does not resolve redirects, case-fold hosts, or expand insteadOf
/// rewrites. It's good enough to recognise "the clone I made by hand points
/// at the same GitHub repo this snapshot came from".
pub fn normalize_git_url(url: &str) -> String {
    let mut s = url.trim();
    // Strip a known scheme prefix if present.
    for scheme in ["https://", "http://", "ssh://", "git://", "git+ssh://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest;
            break;
        }
    }
    // Drop any `user@` userinfo on the (now scheme-less) authority.
    let mut s = s.to_string();
    if let Some(at) = s.find('@') {
        // Only treat it as userinfo if it precedes the first `/` or `:`
        // (i.e. it's part of the authority, not somewhere in the path).
        let boundary = s.find(['/', ':']).map(|i| at < i).unwrap_or(true);
        if boundary {
            s = s[at + 1..].to_string();
        }
    }
    // Collapse the scp-style `host:owner/repo` separator to `host/owner/repo`
    // so it matches the URL form. We only rewrite the FIRST `:` and only when
    // it isn't an explicit `:port` (all-digit segment up to the next `/`).
    if let Some(colon) = s.find(':') {
        let after = &s[colon + 1..];
        let segment_end = after.find('/').unwrap_or(after.len());
        let looks_like_port = !after[..segment_end].is_empty()
            && after[..segment_end].chars().all(|c| c.is_ascii_digit());
        if !looks_like_port {
            s.replace_range(colon..=colon, "/");
        }
    }
    // Trim trailing slash, then a single `.git`, then any leftover slash.
    let trimmed = s
        .trim_end_matches('/')
        .strip_suffix(".git")
        .map(|base| base.trim_end_matches('/'))
        .unwrap_or_else(|| s.trim_end_matches('/'));
    trimmed.to_string()
}

/// Wave-5 5E: among `candidates` (local repo paths the daemon already knows
/// about), find the first whose `origin` remote URL matches `target_url`
/// after [`normalize_git_url`] normalization. Returns its path so
/// `/workspace/receive` can reuse the existing clone instead of cloning a
/// fresh copy into the managed workspaces dir.
///
/// Blocking (shells out to `git remote get-url origin` per candidate); call
/// from `spawn_blocking`. Candidates that are missing, not a git repo, or
/// have no `origin` are silently skipped.
pub fn find_matching_local_repo(
    target_url: &str,
    candidates: &[PathBuf],
) -> Option<PathBuf> {
    let target = normalize_git_url(target_url);
    if target.is_empty() {
        return None;
    }
    for candidate in candidates {
        if !candidate.exists() {
            continue;
        }
        let Some(origin) = git_origin_url(candidate) else {
            continue;
        };
        if normalize_git_url(&origin) == target {
            return Some(candidate.clone());
        }
    }
    None
}

/// Resolve the current `HEAD` symbolic ref (e.g. `main`) so the target checks
/// out the same branch. Falls back to `None` (the target then uses the remote
/// default branch) on a detached HEAD / unborn branch / git error.
pub fn current_branch(repo: &Path) -> Option<String> {
    let output = crate::hidden_std_command("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() || name == "HEAD" {
        None
    } else {
        Some(name)
    }
}

/// Wave 6B: push `branch` to `remote` (a remote name *or* URL) so the target's
/// clone/pull sees every local commit. Without this the move silently drops
/// committed-but-unpushed work — the exact "metadata-only move" failure 6B
/// exists to fix.
///
/// Blocking (shells out to `git push`); call from `spawn_blocking`.
pub fn push_branch(repo: &Path, remote: &str, branch: &str) -> Result<(), PromoteError> {
    let output = crate::hidden_std_command("git")
        .arg("-C")
        .arg(repo)
        .args(["push", remote, branch])
        .output()
        .map_err(|err| PromoteError::Git(format!("git push spawn failed: {err}")))?;
    if !output.status.success() {
        return Err(PromoteError::Git(format!(
            "git push {remote} {branch}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Session cwd remapping (Wave 6B)
// ---------------------------------------------------------------------

/// Source side: flatten a session cwd to a workspace-relative path.
/// Absolute paths under the workspace root are stripped; absolute paths
/// *outside* the root degrade to `"."` (the tab still lands somewhere sane
/// on the target); relative paths pass through.
pub fn relative_session_cwd(workspace_path: &Path, cwd: &str) -> String {
    let trimmed = cwd.trim();
    if trimmed.is_empty() || trimmed == "." {
        return ".".to_string();
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return trimmed.to_string();
    }
    match path.strip_prefix(workspace_path) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => ".".to_string(),
    }
}

/// Target side: rebuild an absolute cwd under the adopted checkout.
pub fn remap_session_cwd(target_root: &Path, relative_cwd: &str) -> String {
    let trimmed = relative_cwd.trim();
    if trimmed.is_empty() || trimmed == "." {
        return target_root.to_string_lossy().into_owned();
    }
    target_root.join(trimmed).to_string_lossy().into_owned()
}

/// Join a target base URL with `/workspace/receive`, tolerating a trailing
/// slash on the base.
pub fn receive_endpoint(target_url: &str) -> String {
    format!("{}/workspace/receive", target_url.trim_end_matches('/'))
}

// ---------------------------------------------------------------------
// Demote — the *flip-back* half of the move plane.
//
// Demote is promote-in-reverse and invents no new sync: this host asks the
// workspace's CURRENT home daemon to `/workspace/promote` the workspace BACK
// here. We are the *target* of that remote promote, so we hand the remote
// home our own receive URL (from `NEOISM_HOST_URL`) and a bearer it can
// present to our cloud gate. The remote home then captures + ships its
// working state to our `/workspace/receive` and flips the pointer to us.
//
// This module owns only the request shape + the two pure URL helpers
// (`promote_endpoint`, `same_host_url`); the orchestration (auth gate,
// home-host resolution, this-host resolution, reqwest call) lives in
// [`crate::server`] next to `/workspace/promote` so the two directions read
// together.
// ---------------------------------------------------------------------

/// Request body for `POST /workspace/demote`.
///
/// * `workspace_id` — the host-workspace to bring HOME to *this* host. Its
///   `running_on_host_id` names the current home; we resolve that host's
///   `daemon_url` (the remote home daemon) from the host registry.
/// * `target_token` — bearer the remote home presents to *our*
///   `/workspace/receive` cloud gate when it ships the workspace back, and
///   that we present to the remote home's `/workspace/promote` gate. Optional
///   so loopback / unauthenticated dev targets work.
#[derive(Debug, Clone, Deserialize)]
pub struct DemoteWorkspaceRequest {
    pub workspace_id: String,
    #[serde(default)]
    pub target_token: Option<String>,
}

/// Join a remote-home base URL with `/workspace/promote`, tolerating a
/// trailing slash on the base. We call the remote home's promote with *our*
/// receive URL as its `target_url`, flipping the workspace back to us.
pub fn promote_endpoint(home_url: &str) -> String {
    format!("{}/workspace/promote", home_url.trim_end_matches('/'))
}

/// True when two daemon URLs name the same host endpoint, ignoring a trailing
/// slash and surrounding whitespace. Used to short-circuit a demote whose
/// workspace is already homed at *this* host (a no-op): comparing the
/// workspace's resolved home URL against our own `NEOISM_HOST_URL`.
pub fn same_host_url(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

/// Join a target daemon base URL with `/workspace/receive-agent`, tolerating a
/// trailing slash. The NEW endpoint promote ships agent bundles to: the source
/// can't reach the target's *loopback* agent-server, so the target daemon
/// proxies each bundle into its own local agent-server (see
/// [`crate::server::workspace_receive_agent`]).
pub fn receive_agent_endpoint(target_url: &str) -> String {
    format!(
        "{}/workspace/receive-agent",
        target_url.trim_end_matches('/')
    )
}

// =====================================================================
// Agent-session handoff (Wave 4C-agent)
//
// After the workspace files land on the target, promote also relocates the
// workspace's AI-agent session(s) so the agent resumes on the new home. The
// two daemons can each only reach their OWN loopback agent-server, so the move
// is a two-hop proxy:
//
//   SOURCE promote ── POST {AGENT}/sessions/export ──► source agent-server
//                  ◄── { bundles: [...] } ───────────────────────────────┘
//   SOURCE promote ── POST {target}/workspace/receive-agent {bundles,root} ─► TARGET daemon
//   TARGET daemon  ── POST {AGENT}/sessions/import { bundle, .. } ──────────► target agent-server
//
// Every step is best-effort: any failure is recorded in [`AgentShipSummary`]
// but never fails the file/workspace promote.
// =====================================================================

/// Default agent-server base URL when `NEOISM_AGENT_SERVER` is unset. Kept in
/// lock-step with `crate::agent`'s private `DEFAULT_AGENT_SERVER`.
const DEFAULT_AGENT_SERVER: &str = "http://127.0.0.1:4096";

/// Resolve the base URL of *this host's* local agent-server. Honors
/// `NEOISM_AGENT_SERVER`, falls back to `NEOISM_SERVER`, then
/// [`DEFAULT_AGENT_SERVER`]. Trimmed of whitespace + trailing slash.
pub fn agent_server_url() -> String {
    std::env::var("NEOISM_AGENT_SERVER")
        .ok()
        .or_else(|| std::env::var("NEOISM_SERVER").ok())
        .map(|server| server.trim().trim_end_matches('/').to_string())
        .filter(|server| !server.is_empty())
        .unwrap_or_else(|| DEFAULT_AGENT_SERVER.to_string())
}

/// `{agent_server}/sessions/export` — source agent-server endpoint promote
/// calls to gather session bundles under the promoted workspace root.
pub fn agent_export_endpoint(agent_server: &str) -> String {
    format!("{}/sessions/export", agent_server.trim_end_matches('/'))
}

/// `{agent_server}/sessions/import` — target agent-server endpoint the target
/// daemon forwards each received bundle to.
pub fn agent_import_endpoint(agent_server: &str) -> String {
    format!("{}/sessions/import", agent_server.trim_end_matches('/'))
}

/// Body `POST`ed to the source agent-server's `/sessions/export`. camelCase to
/// match the agent-server wire contract (`workspaceRoot`).
#[derive(Debug, Clone, Serialize)]
pub struct ExportSessionsRequest {
    #[serde(rename = "workspaceRoot")]
    pub workspace_root: String,
}

/// Reply from the source agent-server's `/sessions/export`. Each bundle is an
/// opaque [`serde_json::Value`] so the daemon relays it verbatim without
/// depending on the agent-server's `SessionBundle` shape.
#[derive(Debug, Clone, Deserialize)]
pub struct ExportSessionsResponse {
    #[serde(default)]
    pub bundles: Vec<serde_json::Value>,
}

/// Body the SOURCE daemon `POST`s to the TARGET daemon's
/// `/workspace/receive-agent`. The target proxies each bundle into its own
/// loopback agent-server (which the source cannot reach directly).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiveAgentRequest {
    #[serde(default)]
    pub bundles: Vec<serde_json::Value>,
    pub target_workspace_root: String,
}

/// Body the TARGET daemon `POST`s to its local agent-server's
/// `/sessions/import`, one per bundle. camelCase wire contract.
#[derive(Debug, Clone, Serialize)]
pub struct ImportSessionRequest {
    pub bundle: serde_json::Value,
    #[serde(rename = "targetWorkspaceRoot")]
    pub target_workspace_root: String,
}

/// Reply from `/workspace/receive-agent`: how many bundles the target imported
/// and any per-bundle errors. The source folds these into the promote's
/// [`AgentShipSummary`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiveAgentResponse {
    pub imported: usize,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_remote_wins_and_is_trimmed() {
        let url = derive_git_url(Path::new("/nonexistent"), Some("  https://x/y.git  "))
            .expect("override remote used");
        assert_eq!(url, "https://x/y.git");
    }

    #[test]
    fn blank_override_falls_through_to_no_remote() {
        // A blank override + a path with no origin → NoGitRemote (clear 400).
        let err = derive_git_url(Path::new("/nonexistent"), Some("   ")).unwrap_err();
        assert!(matches!(err, PromoteError::NoGitRemote(_)));
    }

    // NOTE: the 5E `normalize_git_url` / `find_matching_local_repo` unit
    // tests live in `tests/workspace_receive.rs` (the integration target),
    // not here. The daemon's in-crate `--lib` test build is currently red
    // due to an unrelated in-flight `WorkspaceSummary` rename, so anything
    // added to this `mod tests` would never compile/run. The integration
    // target compiles against the green production lib instead.

    #[test]
    fn receive_endpoint_handles_trailing_slash() {
        assert_eq!(
            receive_endpoint("http://h:1/"),
            "http://h:1/workspace/receive"
        );
        assert_eq!(
            receive_endpoint("http://h:1"),
            "http://h:1/workspace/receive"
        );
    }

    #[test]
    fn promote_endpoint_handles_trailing_slash() {
        assert_eq!(
            promote_endpoint("http://home:9/"),
            "http://home:9/workspace/promote"
        );
        assert_eq!(
            promote_endpoint("http://home:9"),
            "http://home:9/workspace/promote"
        );
    }

    #[test]
    fn same_host_url_ignores_trailing_slash_and_whitespace() {
        assert!(same_host_url("http://h:1", "http://h:1/"));
        assert!(same_host_url("  http://h:1/  ", "http://h:1"));
        assert!(!same_host_url("http://h:1", "http://h:2"));
    }

    #[test]
    fn receive_agent_endpoint_handles_trailing_slash() {
        assert_eq!(
            receive_agent_endpoint("http://h:1/"),
            "http://h:1/workspace/receive-agent"
        );
        assert_eq!(
            receive_agent_endpoint("http://h:1"),
            "http://h:1/workspace/receive-agent"
        );
    }

    #[test]
    fn agent_endpoints_append_path() {
        assert_eq!(
            agent_export_endpoint("http://127.0.0.1:4096"),
            "http://127.0.0.1:4096/sessions/export"
        );
        assert_eq!(
            agent_import_endpoint("http://127.0.0.1:4096/"),
            "http://127.0.0.1:4096/sessions/import"
        );
    }

    // Env-var resolution touches a process global; serialize through one lock so
    // the cases don't race each other's set_var / remove_var.
    static AGENT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct AgentEnvGuard {
        _g: std::sync::MutexGuard<'static, ()>,
        prev_agent: Option<String>,
        prev_server: Option<String>,
    }
    impl AgentEnvGuard {
        fn new() -> Self {
            let g = AGENT_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev_agent = std::env::var("NEOISM_AGENT_SERVER").ok();
            let prev_server = std::env::var("NEOISM_SERVER").ok();
            std::env::remove_var("NEOISM_AGENT_SERVER");
            std::env::remove_var("NEOISM_SERVER");
            Self {
                _g: g,
                prev_agent,
                prev_server,
            }
        }
    }
    impl Drop for AgentEnvGuard {
        fn drop(&mut self) {
            match &self.prev_agent {
                Some(v) => std::env::set_var("NEOISM_AGENT_SERVER", v),
                None => std::env::remove_var("NEOISM_AGENT_SERVER"),
            }
            match &self.prev_server {
                Some(v) => std::env::set_var("NEOISM_SERVER", v),
                None => std::env::remove_var("NEOISM_SERVER"),
            }
        }
    }

    #[test]
    fn agent_server_url_defaults_when_unset() {
        let _g = AgentEnvGuard::new();
        assert_eq!(agent_server_url(), DEFAULT_AGENT_SERVER);
    }

    #[test]
    fn agent_server_url_honors_env_override_and_trims() {
        let _g = AgentEnvGuard::new();
        std::env::set_var("NEOISM_AGENT_SERVER", "  http://10.0.0.5:9000/  ");
        assert_eq!(agent_server_url(), "http://10.0.0.5:9000");
    }

    #[test]
    fn agent_server_url_falls_back_to_neoism_server() {
        let _g = AgentEnvGuard::new();
        std::env::set_var("NEOISM_SERVER", "http://host:7000");
        assert_eq!(agent_server_url(), "http://host:7000");
    }
}
