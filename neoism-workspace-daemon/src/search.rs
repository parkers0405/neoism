//! Daemon-side handler for [`SearchClientMessage`].
//!
//! Wave-7 web parity: the desktop binary spawns `rg`,
//! `fff_search::FilePicker`, and `git status --porcelain` directly.
//! The web frontend can't, so the daemon runs those tools on its host
//! and streams the hits back over the WebSocket. This module
//! implements the daemon side: it spawns `rg` / `git` subprocesses on
//! the tokio blocking pool when those binaries are present and falls
//! back to a `walkdir`-based file walk + in-memory scoring when not.
//!
//! Each request carries a `req_id` from the protocol. The
//! `CancelSearch` variant aborts the in-flight task associated with
//! that id; we hold an `AbortHandle` per pending request.
//!
//! All paths flowing in/out of this module are workspace-relative —
//! traversal protection is delegated to [`crate::files::resolve_path`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use neoism_protocol::search::{
    RequestId, SearchClientMessage, SearchFileHit, SearchFileMode, SearchGitHit,
    SearchGitStatus, SearchGrepHit, SearchGrepMode, SearchServerMessage,
};
use parking_lot::Mutex;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command as TokioCommand;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::AbortHandle;
use walkdir::WalkDir;

use crate::files::resolve_path;

/// Maximum number of hits any single search reply will carry. Beyond
/// this we truncate to keep the WebSocket frame bounded.
const MAX_HITS: usize = 500;

/// Tracks in-flight tasks per request id so `CancelSearch` can abort
/// them. Cheap to clone — the inner map is shared via `Arc<Mutex<_>>`.
#[derive(Clone, Default)]
pub struct SearchRegistry {
    inflight: Arc<Mutex<HashMap<RequestId, AbortHandle>>>,
}

impl SearchRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, req_id: RequestId, handle: AbortHandle) {
        self.inflight.lock().insert(req_id, handle);
    }

    fn remove(&self, req_id: RequestId) -> Option<AbortHandle> {
        self.inflight.lock().remove(&req_id)
    }

    /// Cancel `req_id` if still pending. Returns true iff a task was
    /// aborted.
    pub fn cancel(&self, req_id: RequestId) -> bool {
        if let Some(handle) = self.remove(req_id) {
            handle.abort();
            true
        } else {
            false
        }
    }
}

/// Dispatch a single client search message. Replies are sent on `tx`
/// — long-running variants (grep, file collection) spawn a tokio task
/// so the caller's main loop is never blocked. `CancelSearch` returns
/// nothing because the cancelled task already either sent its reply
/// or never will.
pub fn dispatch(
    registry: &SearchRegistry,
    workspace_root: PathBuf,
    msg: SearchClientMessage,
    tx: UnboundedSender<SearchServerMessage>,
) {
    match msg {
        SearchClientMessage::CancelSearch { req_id } => {
            registry.cancel(req_id);
        }
        SearchClientMessage::CollectFiles { req_id, cwd } => {
            spawn_task(registry, req_id, tx.clone(), async move {
                collect_files(req_id, workspace_root, cwd).await
            });
        }
        SearchClientMessage::SearchFiles {
            req_id,
            query,
            cwd,
            mode,
        } => {
            spawn_task(registry, req_id, tx.clone(), async move {
                search_files(req_id, workspace_root, query, cwd, mode).await
            });
        }
        SearchClientMessage::SearchGrep {
            req_id,
            query,
            cwd,
            mode,
            case_sensitive,
            file_patterns,
        } => {
            spawn_task(registry, req_id, tx.clone(), async move {
                search_grep(
                    req_id,
                    workspace_root,
                    query,
                    cwd,
                    mode,
                    case_sensitive,
                    file_patterns,
                )
                .await
            });
        }
        SearchClientMessage::SearchGitChanges { req_id, cwd } => {
            spawn_task(registry, req_id, tx.clone(), async move {
                search_git_changes(req_id, workspace_root, cwd).await
            });
        }
        SearchClientMessage::GitRepoRoot { req_id, cwd } => {
            spawn_task(registry, req_id, tx.clone(), async move {
                git_repo_root(req_id, workspace_root, cwd).await
            });
        }
    }
}

/// Spawn `fut` on tokio, remember its `AbortHandle` for later
/// cancellation, and forward whichever `SearchServerMessage` it
/// resolves to to `tx`. The abort handle is removed from the
/// registry once the task has produced its reply.
fn spawn_task<F>(
    registry: &SearchRegistry,
    req_id: RequestId,
    tx: UnboundedSender<SearchServerMessage>,
    fut: F,
) where
    F: std::future::Future<Output = SearchServerMessage> + Send + 'static,
{
    let registry_for_task = registry.clone();
    let handle = tokio::spawn(async move {
        let reply = fut.await;
        let _ = tx.send(reply);
        registry_for_task.remove(req_id);
    });
    registry.insert(req_id, handle.abort_handle());
}

/// Resolve a workspace-relative `cwd` to an absolute path or return
/// an `Error` reply. Used at the top of every handler.
fn resolve_cwd(
    req_id: RequestId,
    workspace_root: PathBuf,
    cwd: &str,
) -> Result<PathBuf, SearchServerMessage> {
    let root = workspace_root;
    let root = if root.is_absolute() {
        root
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(root)
    };
    let cwd_path = Path::new(cwd);
    if cwd_path.is_absolute() {
        return if cwd_path.starts_with(&root) {
            Ok(cwd_path.to_path_buf())
        } else {
            Err(SearchServerMessage::SearchError {
                req_id,
                message: format!(
                    "search cwd is outside workspace root: {}",
                    cwd_path.display()
                ),
            })
        };
    }
    resolve_path(&root, cwd)
        .map_err(|message| SearchServerMessage::SearchError { req_id, message })
}

// -----------------------------------------------------------------------
// CollectFiles
// -----------------------------------------------------------------------

async fn collect_files(
    req_id: RequestId,
    workspace_root: PathBuf,
    cwd: String,
) -> SearchServerMessage {
    let cwd_abs = match resolve_cwd(req_id, workspace_root, &cwd) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // Prefer `rg --files` (respects .gitignore + .ignore the same way
    // the desktop binary does). Fall back to walkdir if rg is missing.
    match run_rg_files(&cwd_abs).await {
        Ok(paths) => SearchServerMessage::CollectFilesResult { req_id, paths },
        Err(_) => {
            let paths = walkdir_files(&cwd_abs);
            SearchServerMessage::CollectFilesResult { req_id, paths }
        }
    }
}

async fn run_rg_files(cwd: &Path) -> Result<Vec<String>, String> {
    if which("rg").is_none() {
        return Err("rg not on PATH".into());
    }
    let mut cmd = TokioCommand::new("rg");
    #[cfg(windows)]
    crate::hide_tokio_command(&mut cmd);
    cmd.arg("--files")
        .arg("--no-messages")
        .arg("--hidden")
        .arg("--glob")
        .arg("!.git")
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = cmd.output().await.map_err(|e| format!("rg spawn: {e}"))?;
    if !output.status.success() && output.stdout.is_empty() {
        return Err(format!("rg exited with status {:?}", output.status.code()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().map(|l| l.to_string()).collect())
}

/// Walkdir fallback that skips obvious noise directories. Honours
/// `.gitignore` semantics only loosely — the daemon-host without
/// ripgrep falls back to a stat-based skip list. Good enough to keep
/// the finder usable; production hosts should install ripgrep.
fn walkdir_files(cwd: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let walker = WalkDir::new(cwd).into_iter().filter_entry(|e| {
        // Skip a small denylist of obvious noise directories. We
        // intentionally don't parse .gitignore here — the rg path
        // above is the production code path.
        if !e.file_type().is_dir() {
            return true;
        }
        let name = e.file_name().to_string_lossy();
        !matches!(
            name.as_ref(),
            ".git"
                | "node_modules"
                | "target"
                | ".venv"
                | "venv"
                | "__pycache__"
                | ".cache"
                | "dist"
                | "build"
        )
    });
    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(cwd) {
            out.push(rel.to_string_lossy().into_owned());
        }
        if out.len() >= 20_000 {
            break;
        }
    }
    out
}

// -----------------------------------------------------------------------
// SearchFiles (fuzzy / exact file picker)
// -----------------------------------------------------------------------

async fn search_files(
    req_id: RequestId,
    workspace_root: PathBuf,
    query: String,
    cwd: String,
    mode: SearchFileMode,
) -> SearchServerMessage {
    let cwd_abs = match resolve_cwd(req_id, workspace_root, &cwd) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let paths = match run_rg_files(&cwd_abs).await {
        Ok(p) => p,
        Err(_) => walkdir_files(&cwd_abs),
    };
    let trimmed = query.trim();
    let hits: Vec<SearchFileHit> = if trimmed.is_empty() {
        paths
            .into_iter()
            .take(MAX_HITS)
            .map(|path| SearchFileHit { score: 0, path })
            .collect()
    } else {
        let smart_case = trimmed.chars().any(|c| c.is_ascii_uppercase());
        let mut scored: Vec<SearchFileHit> = paths
            .iter()
            .filter_map(|path| {
                let score = match mode {
                    SearchFileMode::Exact => {
                        exact_file_match_score(trimmed, path, smart_case)
                    }
                    SearchFileMode::Fuzzy => fuzzy_file_match_score(trimmed, path),
                }?;
                Some(SearchFileHit {
                    score,
                    path: path.clone(),
                })
            })
            .collect();
        scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        scored.truncate(MAX_HITS);
        scored
    };
    SearchServerMessage::SearchFilesResult { req_id, hits }
}

fn exact_file_match_score(query: &str, path: &str, smart_case: bool) -> Option<i32> {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if str_eq(path, query, smart_case) {
        return Some(12_000 - path.len() as i32);
    }
    if str_eq(file_name, query, smart_case) {
        return Some(11_000 - path.len() as i32);
    }
    if let Some(pos) = str_find(file_name, query, smart_case) {
        return Some(10_000 - pos as i32 * 10 - path.len() as i32);
    }
    if let Some(pos) = str_find(path, query, smart_case) {
        return Some(9_000 - pos as i32 - path.len() as i32);
    }
    None
}

/// Minimal subsequence-style fuzzy match. We deliberately avoid
/// pulling in a fuzzy crate just to keep the dep footprint small —
/// the desktop binary's `fff_search` integration is richer but lives
/// behind the protocol from the client's perspective. This is good
/// enough to keep the web finder usable for short queries.
fn fuzzy_file_match_score(query: &str, path: &str) -> Option<i32> {
    let q = query.to_ascii_lowercase();
    let h = path.to_ascii_lowercase();
    let mut score: i32 = 0;
    let mut last: Option<usize> = None;
    let h_bytes = h.as_bytes();
    let mut i = 0;
    for qc in q.bytes() {
        // Advance i to the next match for qc in h_bytes.
        let mut found = None;
        while i < h_bytes.len() {
            if h_bytes[i] == qc {
                found = Some(i);
                break;
            }
            i += 1;
        }
        let pos = found?;
        // Reward consecutive matches; penalise gaps.
        if let Some(prev) = last {
            let gap = pos - prev;
            score -= gap as i32;
        }
        last = Some(pos);
        i = pos + 1;
        score += 100;
    }
    // Slight bias toward shorter paths.
    Some(score - path.len() as i32)
}

#[inline]
fn str_eq(haystack: &str, needle: &str, smart_case: bool) -> bool {
    if smart_case {
        haystack == needle
    } else {
        haystack.eq_ignore_ascii_case(needle)
    }
}

#[inline]
fn str_find(haystack: &str, needle: &str, smart_case: bool) -> Option<usize> {
    if smart_case {
        haystack.find(needle)
    } else {
        // ASCII-case-insensitive find.
        if needle.is_empty() {
            return Some(0);
        }
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        h.windows(n.len()).position(|w| w.eq_ignore_ascii_case(n))
    }
}

// -----------------------------------------------------------------------
// SearchGrep
// -----------------------------------------------------------------------

async fn search_grep(
    req_id: RequestId,
    workspace_root: PathBuf,
    query: String,
    cwd: String,
    mode: SearchGrepMode,
    case_sensitive: Option<bool>,
    file_patterns: Vec<String>,
) -> SearchServerMessage {
    let cwd_abs = match resolve_cwd(req_id, workspace_root, &cwd) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if query.trim().is_empty() {
        return SearchServerMessage::SearchGrepResult {
            req_id,
            hits: Vec::new(),
        };
    }
    if which("rg").is_none() {
        return SearchServerMessage::SearchError {
            req_id,
            message: "rg (ripgrep) not on PATH; daemon cannot run grep search".into(),
        };
    }

    let mut cmd = TokioCommand::new("rg");
    #[cfg(windows)]
    crate::hide_tokio_command(&mut cmd);
    cmd.arg("--json").arg("--no-messages").arg("--hidden");
    cmd.arg("--glob").arg("!.git");
    // Mode flags. Fuzzy is a UI concept; rg has no fuzzy mode, so we
    // treat Fuzzy as "fixed strings, case-insensitive unless smart"
    // and let the client do any further re-scoring.
    match mode {
        SearchGrepMode::Regex => {}
        SearchGrepMode::Exact | SearchGrepMode::Fuzzy => {
            cmd.arg("--fixed-strings");
        }
    }
    let smart_case =
        case_sensitive.unwrap_or_else(|| query.chars().any(|c| c.is_ascii_uppercase()));
    if !smart_case {
        cmd.arg("--ignore-case");
    }
    for pat in &file_patterns {
        cmd.arg("--glob").arg(pat);
    }
    cmd.arg("--").arg(&query);
    cmd.current_dir(&cwd_abs)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return SearchServerMessage::SearchError {
                req_id,
                message: format!("rg spawn failed: {e}"),
            }
        }
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            return SearchServerMessage::SearchError {
                req_id,
                message: "rg stdout missing".into(),
            }
        }
    };
    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = reader.lines();
    let mut hits: Vec<SearchGrepHit> = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if hits.len() >= MAX_HITS {
            break;
        }
        if let Some(hit) = parse_rg_json_match(&line) {
            hits.push(hit);
        }
    }
    let _ = child.kill().await;
    SearchServerMessage::SearchGrepResult { req_id, hits }
}

/// Parse one line of `rg --json` output. Only `{"type":"match", ...}`
/// frames produce a hit; everything else (`begin`, `end`, `summary`)
/// is ignored.
fn parse_rg_json_match(line: &str) -> Option<SearchGrepHit> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "match" {
        return None;
    }
    let data = v.get("data")?;
    let path = data
        .get("path")?
        .get("text")
        .and_then(|t| t.as_str())?
        .to_string();
    let line_num = data.get("line_number")?.as_u64().unwrap_or(0) as u32;
    let text = data
        .get("lines")
        .and_then(|l| l.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim_end_matches('\n')
        .to_string();
    // First submatch column (1-based on the wire from rg, but we keep
    // the raw value for the client to interpret consistently with
    // its native impl which also uses rg's `start`).
    let column = data
        .get("submatches")
        .and_then(|s| s.as_array())
        .and_then(|arr| arr.first())
        .and_then(|m| m.get("start"))
        .and_then(|s| s.as_u64())
        .unwrap_or(0) as u32;
    Some(SearchGrepHit {
        score: 0,
        path,
        line: line_num,
        column,
        text,
    })
}

// -----------------------------------------------------------------------
// SearchGitChanges
// -----------------------------------------------------------------------

async fn search_git_changes(
    req_id: RequestId,
    workspace_root: PathBuf,
    cwd: String,
) -> SearchServerMessage {
    let cwd_abs = match resolve_cwd(req_id, workspace_root, &cwd) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd_for_blocking = cwd_abs.clone();
    let result =
        tokio::task::spawn_blocking(move || run_git_status_porcelain(&cwd_for_blocking))
            .await;
    match result {
        Ok(Ok(hits)) => SearchServerMessage::SearchGitChangesResult { req_id, hits },
        Ok(Err(message)) => SearchServerMessage::SearchError { req_id, message },
        Err(e) => SearchServerMessage::SearchError {
            req_id,
            message: format!("git status join error: {e}"),
        },
    }
}

fn run_git_status_porcelain(cwd: &Path) -> Result<Vec<SearchGitHit>, String> {
    let mut command = std::process::Command::new("git");
    #[cfg(windows)]
    crate::hide_std_command(&mut command);
    let output = command
        .arg("-C")
        .arg(cwd)
        .arg("status")
        .arg("--porcelain=v1")
        .output()
        .map_err(|e| format!("git status spawn: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git status exited with status {:?}",
            output.status.code()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut hits = Vec::new();
    for line in stdout.lines() {
        // Format: "XY path" (or "XY orig -> path" for renames).
        if line.len() < 4 {
            continue;
        }
        let xy = &line[..2];
        let path_part = &line[3..];
        // Pull rename target if present.
        let path = if let Some((_, after)) = path_part.split_once(" -> ") {
            after.to_string()
        } else {
            path_part.to_string()
        };
        let status = classify_xy(xy);
        hits.push(SearchGitHit {
            path,
            status,
            line: 0,
            text: String::new(),
        });
    }
    Ok(hits)
}

fn classify_xy(xy: &str) -> SearchGitStatus {
    if xy == "??" {
        return SearchGitStatus::Untracked;
    }
    let b = xy.as_bytes();
    let x = b[0] as char;
    let y = b[1] as char;
    if x == 'U' || y == 'U' || (x == 'D' && y == 'D') || (x == 'A' && y == 'A') {
        return SearchGitStatus::Conflict;
    }
    if x == 'R' || y == 'R' {
        return SearchGitStatus::Renamed;
    }
    if x == 'A' && y == ' ' {
        return SearchGitStatus::Added;
    }
    if x == 'D' || y == 'D' {
        return SearchGitStatus::Deleted;
    }
    let staged = x != ' ' && x != '?';
    let unstaged = y != ' ' && y != '?';
    match (staged, unstaged) {
        (true, true) => SearchGitStatus::Mixed,
        (true, false) => SearchGitStatus::Staged,
        _ => SearchGitStatus::Modified,
    }
}

// -----------------------------------------------------------------------
// GitRepoRoot
// -----------------------------------------------------------------------

async fn git_repo_root(
    req_id: RequestId,
    workspace_root: PathBuf,
    cwd: String,
) -> SearchServerMessage {
    let cwd_abs = match resolve_cwd(req_id, workspace_root, &cwd) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cwd_for_blocking = cwd_abs.clone();
    let join = tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new("git");
        #[cfg(windows)]
        crate::hide_std_command(&mut command);
        let out = command
            .arg("-C")
            .arg(&cwd_for_blocking)
            .arg("rev-parse")
            .arg("--show-toplevel")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    })
    .await;
    let path = join.ok().flatten();
    SearchServerMessage::GitRepoRootResult { req_id, path }
}

// -----------------------------------------------------------------------
// Utilities
// -----------------------------------------------------------------------

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_xy_basic() {
        assert!(matches!(classify_xy("??"), SearchGitStatus::Untracked));
        assert!(matches!(classify_xy("A "), SearchGitStatus::Added));
        assert!(matches!(classify_xy(" M"), SearchGitStatus::Modified));
        assert!(matches!(classify_xy("M "), SearchGitStatus::Staged));
        assert!(matches!(classify_xy("MM"), SearchGitStatus::Mixed));
        assert!(matches!(classify_xy("D "), SearchGitStatus::Deleted));
        assert!(matches!(classify_xy(" D"), SearchGitStatus::Deleted));
        assert!(matches!(classify_xy("UU"), SearchGitStatus::Conflict));
        assert!(matches!(classify_xy("R "), SearchGitStatus::Renamed));
    }

    #[test]
    fn fuzzy_score_matches_subsequence() {
        assert!(fuzzy_file_match_score("src", "src/lib.rs").is_some());
        assert!(fuzzy_file_match_score("slr", "src/lib.rs").is_some());
        assert!(fuzzy_file_match_score("xyz", "src/lib.rs").is_none());
    }

    #[test]
    fn exact_score_filename_hit_beats_path_hit() {
        let a = exact_file_match_score("lib.rs", "src/lib.rs", false).unwrap();
        let b = exact_file_match_score("src", "src/lib.rs", false).unwrap();
        assert!(
            a > b,
            "exact filename match should outrank a path-component hit"
        );
    }

    #[test]
    fn registry_cancel_removes_entry() {
        let reg = SearchRegistry::new();
        let handle = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let handle = tokio::spawn(async { std::future::pending::<()>().await });
                let abort = handle.abort_handle();
                drop(handle);
                abort
            });
        reg.insert(1, handle);
        assert!(reg.cancel(1));
        assert!(!reg.cancel(1));
    }

    #[test]
    fn parse_rg_json_match_extracts_basic_fields() {
        let line = r#"{"type":"match","data":{"path":{"text":"src/lib.rs"},"lines":{"text":"hello world\n"},"line_number":3,"absolute_offset":0,"submatches":[{"match":{"text":"hello"},"start":0,"end":5}]}}"#;
        let hit = parse_rg_json_match(line).unwrap();
        assert_eq!(hit.path, "src/lib.rs");
        assert_eq!(hit.line, 3);
        assert_eq!(hit.text, "hello world");
        assert_eq!(hit.column, 0);
    }

    #[test]
    fn parse_rg_json_ignores_non_match_frames() {
        let line = r#"{"type":"begin","data":{"path":{"text":"x"}}}"#;
        assert!(parse_rg_json_match(line).is_none());
    }
}
