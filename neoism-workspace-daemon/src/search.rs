//! Daemon-side handler for [`SearchClientMessage`].
//!
//! Wave-7 web parity: the desktop binary spawns `rg`,
//! `fff_search::FilePicker`, and `git status --porcelain` directly.
//! The web frontend can't, so the daemon runs those tools on its host
//! and streams the hits back over the WebSocket. This module
//! runs fuzzy file queries through the same warm FFF index used by the
//! desktop, on the blocking pool. Exact file matching, grep and git retain
//! their existing host-side collectors.
//!
//! Each request carries a `req_id` from the protocol. The
//! `CancelSearch` variant aborts the in-flight task associated with
//! that id and marks queued blocking work obsolete. Already-running FFF
//! calls cannot be interrupted, so shared admission stays held until they
//! finish. Distinct request IDs are independent, not a global latest query.
//!
//! All paths flowing in/out of this module are workspace-relative —
//! traversal protection is delegated to [`crate::files::resolve_path`].

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use neoism_agent_service_api::{WorkspaceSearchRootPin, WorkspaceSearchService};
use neoism_agent_workspace_search_fff::FffWorkspaceSearchService;
use neoism_protocol::search::{
    RequestId, SearchClientMessage, SearchDirectoryHit, SearchFileHit, SearchFileMode,
    SearchGitHit, SearchGitStatus, SearchGrepHit, SearchGrepMode, SearchServerMessage,
};
use parking_lot::Mutex;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command as TokioCommand;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Semaphore;
use tokio::task::AbortHandle;
use walkdir::WalkDir;

use crate::files::resolve_path;

/// Maximum number of hits any single search reply will carry. Beyond
/// this we truncate to keep the WebSocket frame bounded.
const MAX_HITS: usize = 500;
const MAX_FILE_WORKERS: usize = 2;
const EXACT_CACHE_TTL: Duration = Duration::from_secs(1);

// Process-wide, not per socket: cancelled FFF calls cannot be interrupted once
// started. Their permits must live in the blocking closures until those exit.
fn file_admission() -> Arc<Semaphore> {
    static ADMISSION: OnceLock<Arc<Semaphore>> = OnceLock::new();
    ADMISSION
        .get_or_init(|| Arc::new(Semaphore::new(MAX_FILE_WORKERS)))
        .clone()
}

struct PendingSearch {
    handle: AbortHandle,
    cancelled: Arc<AtomicBool>,
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Async admission avoids filling Tokio's blocking queue. The second check
/// skips work cancelled after admission but before a blocking thread starts.
async fn file_work<T: Send + 'static>(
    admission: Arc<Semaphore>,
    cancelled: Arc<AtomicBool>,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let permit = admission.acquire_owned().await.map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if cancelled.load(Ordering::Acquire) {
            return Err("search cancelled".into());
        }
        Ok(work())
    })
    .await
    .map_err(|e| format!("file search join error: {e}"))?
}

struct ExactCollection {
    root: PathBuf,
    collected: Instant,
    paths: Arc<Vec<String>>,
}

/// Tracks in-flight tasks per request id so `CancelSearch` can abort
/// them. Cheap to clone — the inner map is shared via `Arc<Mutex<_>>`.
#[derive(Clone)]
pub struct SearchRegistry {
    inflight: Arc<Mutex<HashMap<RequestId, PendingSearch>>>,
    files: Arc<FffFileSearch>,
}
/// Pins warm FFF indices for this client; closing the socket releases watchers.
/// The bounded set follows the same root-scoped index service as Agent tools.
struct FffFileSearch {
    service: FffWorkspaceSearchService,
    roots: Mutex<VecDeque<(PathBuf, Arc<dyn WorkspaceSearchRootPin>)>>,
    // Short-lived snapshots, not a filesystem index: creates/deletes and ignore
    // edits become visible at expiry, or immediately on explicit CollectFiles.
    // Holding this async lock through collection coalesces concurrent cold reads.
    exact: tokio::sync::Mutex<VecDeque<ExactCollection>>,
}
impl Default for SearchRegistry {
    fn default() -> Self {
        Self {
            inflight: Default::default(),
            files: Arc::new(FffFileSearch {
                service: FffWorkspaceSearchService::new(),
                roots: Mutex::new(VecDeque::new()),
                exact: Default::default(),
            }),
        }
    }
}
impl FffFileSearch {
    fn search(&self, root: &Path, query: &str) -> Result<Vec<SearchFileHit>, String> {
        let evicted = {
            let mut roots = self.roots.lock();
            if let Some(index) = roots.iter().position(|(path, _)| path == root) {
                let entry = roots.remove(index).unwrap();
                roots.push_back(entry);
            } else {
                let pin = self
                    .service
                    .pin_root(root)
                    .map_err(|error| error.to_string())?;
                roots.push_back((root.to_path_buf(), pin));
            }
            if roots.len() > 8 {
                roots.pop_front()
            } else {
                None
            }
        };
        // Dropping a pin can stop/join a watcher. Never do that under roots.
        drop(evicted);
        self.service
            .with_picker(root, |picker| {
                let parser = fff_search::QueryParser::default();
                let matches = picker.fuzzy_search(
                    &parser.parse(query),
                    None,
                    fff_search::FuzzySearchOptions {
                        max_threads: 0,
                        current_file: None,
                        pagination: fff_search::PaginationArgs {
                            offset: 0,
                            limit: MAX_HITS,
                        },
                        ..Default::default()
                    },
                );
                matches
                    .items
                    .iter()
                    .zip(matches.scores.iter())
                    .map(|(item, score)| SearchFileHit {
                        path: item.relative_path(picker),
                        score: score.total,
                    })
                    .collect()
            })
            .map_err(|error| error.to_string())
    }

    async fn exact_paths(
        &self,
        root: PathBuf,
        refresh: bool,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Arc<Vec<String>>, String> {
        let mut cache = self.exact.lock().await;
        if let Some(index) = cache.iter().position(|entry| entry.root == root) {
            let entry = cache.remove(index).unwrap();
            if !refresh && entry.collected.elapsed() < EXACT_CACHE_TTL {
                let paths = entry.paths.clone();
                cache.push_back(entry);
                return Ok(paths);
            }
        }
        let paths = match run_rg_files(&root, cancelled.clone()).await {
            Ok(paths) => paths,
            Err(_) => {
                let walk_root = root.clone();
                file_work(file_admission(), cancelled, move || {
                    walkdir_files(&walk_root)
                })
                .await?
            }
        };
        let paths = Arc::new(paths);
        cache.push_back(ExactCollection {
            root: root.clone(),
            collected: Instant::now(),
            paths: paths.clone(),
        });
        while cache.len() > 8 {
            cache.pop_front();
        }
        Ok(paths)
    }
}

impl SearchRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancel `req_id` if still pending. Returns true iff a task was
    /// aborted.
    pub fn cancel(&self, req_id: RequestId) -> bool {
        if let Some(pending) = self.inflight.lock().remove(&req_id) {
            pending.cancelled.store(true, Ordering::Release);
            pending.handle.abort();
            true
        } else {
            false
        }
    }
}

/// Keep the protocol's inner ID distinct from its WebSocket envelope ID.
pub fn request_id(message: &SearchClientMessage) -> RequestId {
    match message {
        SearchClientMessage::CollectFiles { req_id, .. }
        | SearchClientMessage::SearchFiles { req_id, .. }
        | SearchClientMessage::SearchDirectories { req_id, .. }
        | SearchClientMessage::SearchGrep { req_id, .. }
        | SearchClientMessage::SearchGitChanges { req_id, .. }
        | SearchClientMessage::GitRepoRoot { req_id, .. }
        | SearchClientMessage::CancelSearch { req_id } => *req_id,
    }
}
pub fn response_id(message: &SearchServerMessage) -> RequestId {
    match message {
        SearchServerMessage::CollectFilesResult { req_id, .. }
        | SearchServerMessage::SearchFilesResult { req_id, .. }
        | SearchServerMessage::SearchDirectoriesResult { req_id, .. }
        | SearchServerMessage::SearchGrepResult { req_id, .. }
        | SearchServerMessage::SearchGitChangesResult { req_id, .. }
        | SearchServerMessage::GitRepoRootResult { req_id, .. }
        | SearchServerMessage::SearchProgress { req_id, .. }
        | SearchServerMessage::SearchError { req_id, .. } => *req_id,
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
            let files = registry.files.clone();
            spawn_task(registry, req_id, tx.clone(), move |cancelled| async move {
                collect_files(req_id, workspace_root, cwd, files, cancelled).await
            });
        }
        SearchClientMessage::SearchFiles {
            req_id,
            query,
            cwd,
            mode,
        } => {
            let files = registry.files.clone();
            spawn_task(registry, req_id, tx.clone(), move |cancelled| async move {
                search_files(req_id, workspace_root, query, cwd, mode, files, cancelled)
                    .await
            });
        }
        SearchClientMessage::SearchDirectories { req_id, query, cwd } => {
            spawn_task(registry, req_id, tx.clone(), move |_| async move {
                search_directories(req_id, workspace_root, query, cwd).await
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
            spawn_task(registry, req_id, tx.clone(), move |_| async move {
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
            spawn_task(registry, req_id, tx.clone(), move |_| async move {
                search_git_changes(req_id, workspace_root, cwd).await
            });
        }
        SearchClientMessage::GitRepoRoot { req_id, cwd } => {
            spawn_task(registry, req_id, tx.clone(), move |_| async move {
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
    make_future: impl FnOnce(Arc<AtomicBool>) -> F,
) where
    F: std::future::Future<Output = SearchServerMessage> + Send + 'static,
{
    // Register under the same lock used for completion, so even an immediately
    // ready future cannot leave a stale entry behind. Reusing an ID replaces
    // only that request; distinct IDs (even at the same root) remain independent.
    let mut inflight = registry.inflight.lock();
    let cancelled = Arc::new(AtomicBool::new(false));
    let task_cancelled = cancelled.clone();
    let guard = CancelOnDrop(cancelled.clone());
    let fut = make_future(cancelled.clone());
    let registry_for_task = registry.clone();
    let handle = tokio::spawn(async move {
        let _guard = guard;
        let reply = fut.await;
        let mut inflight = registry_for_task.inflight.lock();
        if inflight
            .get(&req_id)
            .is_some_and(|pending| Arc::ptr_eq(&pending.cancelled, &task_cancelled))
        {
            let _ = tx.send(reply);
            inflight.remove(&req_id);
        }
    });
    if let Some(old) = inflight.insert(
        req_id,
        PendingSearch {
            handle: handle.abort_handle(),
            cancelled,
        },
    ) {
        old.cancelled.store(true, Ordering::Release);
        old.handle.abort();
    }
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
    files: Arc<FffFileSearch>,
    cancelled: Arc<AtomicBool>,
) -> SearchServerMessage {
    let cwd_abs = match resolve_cwd(req_id, workspace_root, &cwd) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // Prefer `rg --files` (respects .gitignore + .ignore the same way
    // the desktop binary does). Fall back to walkdir if rg is missing.
    match files.exact_paths(cwd_abs, true, cancelled.clone()).await {
        Ok(paths) => {
            match file_work(file_admission(), cancelled, move || (*paths).clone()).await {
                Ok(paths) => SearchServerMessage::CollectFilesResult { req_id, paths },
                Err(message) => SearchServerMessage::SearchError { req_id, message },
            }
        }
        Err(message) => SearchServerMessage::SearchError { req_id, message },
    }
}

async fn run_rg_files(
    cwd: &Path,
    cancelled: Arc<AtomicBool>,
) -> Result<Vec<String>, String> {
    if which("rg").is_none() {
        return Err("rg not on PATH".into());
    }
    let mut cmd = TokioCommand::new("rg");
    #[cfg(windows)]
    crate::hide_tokio_command(&mut cmd);
    cmd.arg("--files")
        .kill_on_drop(true)
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
    file_work(file_admission(), cancelled, move || {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect()
    })
    .await
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
// SearchDirectories (bounded directory-only fuzzy walk)
// -----------------------------------------------------------------------

async fn search_directories(
    req_id: RequestId,
    workspace_root: PathBuf,
    query: String,
    cwd: String,
) -> SearchServerMessage {
    let cwd_abs = match resolve_cwd(req_id, workspace_root, &cwd) {
        Ok(path) => path,
        Err(error) => return error,
    };
    let join =
        tokio::task::spawn_blocking(move || directory_hits(&cwd_abs, &query)).await;
    match join {
        Ok(hits) => SearchServerMessage::SearchDirectoriesResult { req_id, hits },
        Err(error) => SearchServerMessage::SearchError {
            req_id,
            message: format!("directory search join error: {error}"),
        },
    }
}

fn directory_hits(cwd: &Path, query: &str) -> Vec<SearchDirectoryHit> {
    const MAX_SCANNED_DIRECTORIES: usize = 20_000;
    const MAX_DEPTH: usize = 32;
    let trimmed = query.trim();
    let mut hits = Vec::new();
    let walker = WalkDir::new(cwd)
        .max_depth(MAX_DEPTH)
        .into_iter()
        .filter_entry(|entry| {
            if !entry.file_type().is_dir() {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
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
    for entry in walker
        .flatten()
        .filter(|entry| entry.file_type().is_dir())
        .skip(1)
        .take(MAX_SCANNED_DIRECTORIES)
    {
        let Ok(relative) = entry.path().strip_prefix(cwd) else {
            continue;
        };
        let path = relative.to_string_lossy().into_owned();
        let score = if trimmed.is_empty() {
            0
        } else if let Some(score) = fuzzy_file_match_score(trimmed, &path) {
            score
        } else {
            continue;
        };
        hits.push(SearchDirectoryHit { score, path });
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(MAX_HITS);
    hits
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
    files: Arc<FffFileSearch>,
    cancelled: Arc<AtomicBool>,
) -> SearchServerMessage {
    let cwd_abs = match resolve_cwd(req_id, workspace_root.clone(), &cwd) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if mode == SearchFileMode::Fuzzy {
        let cwd_abs = match cwd_abs.canonicalize().and_then(|candidate| {
            workspace_root.canonicalize().map(|root| (candidate, root))
        }) {
            Ok((candidate, root)) if candidate.starts_with(&root) => candidate,
            Ok(_) => {
                return SearchServerMessage::SearchError {
                    req_id,
                    message: "search cwd escapes the workspace through a symlink".into(),
                }
            }
            Err(error) => {
                return SearchServerMessage::SearchError {
                    req_id,
                    message: format!("resolve search root: {error}"),
                }
            }
        };
        return match file_work(file_admission(), cancelled, move || {
            files.search(&cwd_abs, &query)
        })
        .await
        {
            Ok(Ok(hits)) => SearchServerMessage::SearchFilesResult { req_id, hits },
            Ok(Err(message)) => SearchServerMessage::SearchError { req_id, message },
            Err(error) => SearchServerMessage::SearchError {
                req_id,
                message: format!("FFF file search failed: {error}"),
            },
        };
    }
    let paths = match files.exact_paths(cwd_abs, false, cancelled.clone()).await {
        Ok(paths) => paths,
        Err(message) => return SearchServerMessage::SearchError { req_id, message },
    };
    match file_work(file_admission(), cancelled, move || {
        exact_hits(&paths, &query)
    })
    .await
    {
        Ok(hits) => SearchServerMessage::SearchFilesResult { req_id, hits },
        Err(message) => SearchServerMessage::SearchError { req_id, message },
    }
}

fn exact_hits(paths: &[String], query: &str) -> Vec<SearchFileHit> {
    let trimmed = query.trim();
    let hits: Vec<SearchFileHit> = if trimmed.is_empty() {
        paths
            .iter()
            .take(MAX_HITS)
            .map(|path| SearchFileHit {
                score: 0,
                path: path.clone(),
            })
            .collect()
    } else {
        let smart_case = trimmed.chars().any(|c| c.is_ascii_uppercase());
        let mut scored: Vec<SearchFileHit> = paths
            .iter()
            .filter_map(|path| {
                let score = exact_file_match_score(trimmed, path, smart_case)?;
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
    hits
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
    cmd.kill_on_drop(true)
        .arg("--json")
        .arg("--no-messages")
        .arg("--hidden");
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

    fn empty_reply(req_id: RequestId) -> SearchServerMessage {
        SearchServerMessage::SearchFilesResult {
            req_id,
            hits: Vec::new(),
        }
    }

    #[test]
    fn evicted_root_pin_is_dropped_outside_roots_lock() {
        struct CheckedPin {
            root: PathBuf,
            files: std::sync::Weak<FffFileSearch>,
            dropped: Arc<AtomicBool>,
        }
        impl WorkspaceSearchRootPin for CheckedPin {
            fn root(&self) -> &Path {
                &self.root
            }
        }
        impl Drop for CheckedPin {
            fn drop(&mut self) {
                if let Some(files) = self.files.upgrade() {
                    assert!(
                        files.roots.try_lock().is_some(),
                        "pin dropped under roots lock"
                    );
                    self.dropped.store(true, Ordering::Release);
                }
            }
        }
        let reg = SearchRegistry::new();
        let dropped = Arc::new(AtomicBool::new(false));
        for index in 0..8 {
            let root = PathBuf::from(format!("unused-root-{index}"));
            reg.files.roots.lock().push_back((
                root.clone(),
                Arc::new(CheckedPin {
                    root,
                    files: Arc::downgrade(&reg.files),
                    dropped: dropped.clone(),
                }),
            ));
        }
        let root = tempfile::tempdir().unwrap();
        git2::Repository::init(root.path()).unwrap();
        std::fs::write(root.path().join("sample.rs"), "").unwrap();
        reg.files.search(root.path(), "sample").unwrap();
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(reg.files.roots.lock().len(), 8);
    }

    #[tokio::test]
    async fn cancellation_keeps_running_permit_and_skips_waiting_work() {
        let reg = SearchRegistry::new();
        let admission = Arc::new(Semaphore::new(1));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_admission = admission.clone();
        spawn_task(&reg, 1, tx.clone(), move |cancelled| async move {
            file_work(first_admission, cancelled, move || {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            })
            .await
            .unwrap();
            empty_reply(1)
        });
        started_rx.await.unwrap();
        assert!(reg.cancel(1));
        tokio::task::yield_now().await;
        assert_eq!(
            admission.available_permits(),
            0,
            "abort must not release running work's permit"
        );

        let obsolete_ran = Arc::new(AtomicBool::new(false));
        let obsolete_flag = obsolete_ran.clone();
        let waiting_admission = admission.clone();
        let (waiting_tx, waiting_rx) = tokio::sync::oneshot::channel();
        spawn_task(&reg, 2, tx.clone(), move |cancelled| async move {
            waiting_tx.send(()).unwrap();
            file_work(waiting_admission, cancelled, move || {
                obsolete_flag.store(true, Ordering::Release)
            })
            .await
            .unwrap();
            empty_reply(2)
        });
        waiting_rx.await.unwrap();
        assert!(reg.cancel(2));

        // Two unrelated request IDs both survive; there is no latest-global
        // suppression, even when both wait on the same admission queue.
        for id in [3, 4] {
            let admission = admission.clone();
            spawn_task(&reg, id, tx.clone(), move |cancelled| async move {
                file_work(admission, cancelled, || ()).await.unwrap();
                empty_reply(id)
            });
        }
        release_tx.send(()).unwrap();
        let mut ids = Vec::new();
        for _ in 0..2 {
            ids.push(response_id(
                &tokio::time::timeout(Duration::from_secs(5), rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            ));
        }
        ids.sort();
        assert_eq!(ids, [3, 4]);
        assert!(!obsolete_ran.load(Ordering::Acquire));
        assert!(reg.inflight.lock().is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn cancelled_work_in_blocking_pool_queue_never_executes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            started_rx.await.unwrap();
            let reg = SearchRegistry::new();
            let admission = Arc::new(Semaphore::new(1));
            let ran = Arc::new(AtomicBool::new(false));
            let ran_work = ran.clone();
            let work_admission = admission.clone();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            spawn_task(&reg, 1, tx, move |cancelled| async move {
                let _ = file_work(work_admission, cancelled, move || {
                    ran_work.store(true, Ordering::Release)
                })
                .await;
                empty_reply(1)
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                while admission.available_permits() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(admission.available_permits(), 0);
            assert!(reg.cancel(1));
            release_tx.send(()).unwrap();
            blocker.await.unwrap();
            let _permit =
                tokio::time::timeout(Duration::from_secs(5), admission.acquire())
                    .await
                    .unwrap()
                    .unwrap();
            assert!(!ran.load(Ordering::Acquire));
            assert!(rx.try_recv().is_err());
        });
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reused_ids_replace_only_their_request_and_ready_tasks_leave_no_entries() {
        let reg = SearchRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_task(&reg, 1, tx.clone(), |_| {
            std::future::pending::<SearchServerMessage>()
        });
        let old_cancelled = reg.inflight.lock().get(&1).unwrap().cancelled.clone();
        spawn_task(&reg, 1, tx.clone(), |_| async { empty_reply(1) });
        assert!(old_cancelled.load(Ordering::Acquire));
        for id in 2..102 {
            spawn_task(
                &reg,
                id,
                tx.clone(),
                move |_| async move { empty_reply(id) },
            );
        }
        let mut ids = Vec::new();
        for _ in 0..101 {
            ids.push(response_id(
                &tokio::time::timeout(Duration::from_secs(5), rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            ));
        }
        ids.sort();
        assert_eq!(ids, (1..102).collect::<Vec<_>>());
        assert!(reg.inflight.lock().is_empty());
    }

    #[tokio::test]
    async fn exact_collection_reuses_refreshes_expires_and_isolates_roots() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("first.rs"), "").unwrap();
        std::fs::write(other.path().join("other.rs"), "").unwrap();
        let reg = SearchRegistry::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (first, coalesced) = tokio::join!(
            reg.files
                .exact_paths(root.path().into(), false, cancelled.clone()),
            reg.files
                .exact_paths(root.path().into(), false, cancelled.clone())
        );
        let first = first.unwrap();
        assert!(Arc::ptr_eq(&first, &coalesced.unwrap()));
        let (second, concurrent) = tokio::join!(
            reg.files
                .exact_paths(root.path().into(), false, cancelled.clone()),
            reg.files
                .exact_paths(root.path().into(), false, cancelled.clone())
        );
        assert!(Arc::ptr_eq(&first, &second.unwrap()));
        assert!(Arc::ptr_eq(&first, &concurrent.unwrap()));
        assert_eq!(exact_hits(&first, "first")[0].path, "first.rs");
        std::fs::remove_file(root.path().join("first.rs")).unwrap();
        std::fs::write(root.path().join("new.rs"), "").unwrap();
        let refreshed = reg
            .files
            .exact_paths(root.path().into(), true, cancelled.clone())
            .await
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &refreshed));
        assert!(refreshed.iter().any(|p| p == "new.rs"));
        assert!(!refreshed.iter().any(|p| p == "first.rs"));
        std::fs::write(root.path().join("later.rs"), "").unwrap();
        // Age deterministically rather than sleeping through the TTL.
        reg.files.exact.lock().await.front_mut().unwrap().collected =
            Instant::now() - EXACT_CACHE_TTL;
        let expired = reg
            .files
            .exact_paths(root.path().into(), false, cancelled.clone())
            .await
            .unwrap();
        assert!(expired.iter().any(|p| p == "later.rs"));
        let isolated = reg
            .files
            .exact_paths(other.path().into(), false, cancelled.clone())
            .await
            .unwrap();
        assert_eq!(&**isolated, &["other.rs".to_string()]);
        if which("rg").is_some() {
            std::fs::write(root.path().join(".ignore"), "later.rs\n").unwrap();
            // Exercise the public dispatch path: CollectFiles invalidates a
            // still-warm snapshot, including changes to ignore rules.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            dispatch(
                &reg,
                root.path().into(),
                SearchClientMessage::CollectFiles {
                    req_id: 20,
                    cwd: String::new(),
                },
                tx.clone(),
            );
            let reply = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            let SearchServerMessage::CollectFilesResult { req_id, paths } = reply else {
                panic!("collection failed: {reply:?}");
            };
            assert_eq!(req_id, 20);
            assert!(!paths.iter().any(|p| p == "later.rs"));
            for (req_id, query) in [(21, "later"), (22, "new")] {
                dispatch(
                    &reg,
                    root.path().into(),
                    SearchClientMessage::SearchFiles {
                        req_id,
                        query: query.into(),
                        cwd: String::new(),
                        mode: SearchFileMode::Exact,
                    },
                    tx.clone(),
                );
            }
            let mut ids = Vec::new();
            for _ in 0..2 {
                let reply = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
                let SearchServerMessage::SearchFilesResult { req_id, hits } = reply
                else {
                    panic!("exact search failed: {reply:?}");
                };
                ids.push(req_id);
                if req_id == 21 {
                    assert!(hits.is_empty());
                } else {
                    assert_eq!(hits[0].path, "new.rs");
                }
            }
            ids.sort();
            assert_eq!(ids, [21, 22]);
        }
    }

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

    #[tokio::test]
    async fn file_picker_uses_warm_fff_for_empty_and_unicode_queries() {
        let root = tempfile::tempdir().unwrap();
        git2::Repository::init(root.path()).unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "pub fn sample() {}\n").unwrap();
        std::fs::write(root.path().join("src/école.rs"), "// unicode\n").unwrap();
        std::fs::write(root.path().join(".gitignore"), "ignored.rs\n").unwrap();
        std::fs::write(root.path().join("ignored.rs"), "ignored\n").unwrap();
        let registry = SearchRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for (id, query) in [(1, ""), (2, "slr"), (3, "école")] {
            dispatch(
                &registry,
                root.path().to_path_buf(),
                SearchClientMessage::SearchFiles {
                    req_id: id,
                    query: query.into(),
                    cwd: String::new(),
                    mode: SearchFileMode::Fuzzy,
                },
                tx.clone(),
            );
            let reply =
                tokio::time::timeout(std::time::Duration::from_secs(15), rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
            let SearchServerMessage::SearchFilesResult { req_id, hits } = reply else {
                panic!("FFF picker failed: {reply:?}");
            };
            assert_eq!(req_id, id);
            assert!(!hits.is_empty());
            assert!(!hits
                .iter()
                .any(|hit| hit.path == "ignored.rs" || hit.path.starts_with(".git/")));
            let expected = if id == 3 {
                "src/école.rs"
            } else {
                "src/lib.rs"
            };
            assert!(
                hits.iter().any(|hit| hit.path == expected),
                "query={query:?}: {hits:?}"
            );
        }
        assert_eq!(
            registry.files.roots.lock().len(),
            1,
            "queries must retain the same warm index pin"
        );
    }

    #[test]
    fn fuzzy_score_matches_subsequence() {
        assert!(fuzzy_file_match_score("src", "src/lib.rs").is_some());
        assert!(fuzzy_file_match_score("slr", "src/lib.rs").is_some());
        assert!(fuzzy_file_match_score("xyz", "src/lib.rs").is_none());
    }

    #[test]
    fn directory_hits_never_include_files() {
        let root = std::env::temp_dir()
            .join(format!("neoism-directory-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::write(root.join("src/not-a-directory.rs"), "").unwrap();
        let hits = directory_hits(&root, "src");
        assert!(hits.iter().any(|hit| hit.path == "src"));
        assert!(hits.iter().all(|hit| !hit.path.ends_with(".rs")));
        let _ = std::fs::remove_dir_all(root);
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

    #[tokio::test]
    async fn registry_cancel_removes_entry() {
        let reg = SearchRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_task(&reg, 1, tx, |_| {
            std::future::pending::<SearchServerMessage>()
        });
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
