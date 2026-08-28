//! Capability traits panels use instead of touching the platform.
//!
//! Each trait is object-safe and `Send + Sync` so the host can wrap
//! the active impl in an `Arc<dyn Trait>` and hand panels a single
//! `Services<'_>` bundle per frame.
//!
//! On native, the impls forward to `std::fs`, the OS clipboard, and
//! local process spawn. On web, they marshal the call into a wire
//! message over the daemon WebSocket; when the reply is not yet
//! available the call returns `IoError::Pending(req_id)` and the
//! panel re-runs after the host delivers `UiEvent::ServiceReply`.
//!
//! Synchronous shape is intentional: panels run on the render thread
//! and consume cached state.

pub mod file_tree_watcher_filter;
pub mod git_watcher_filter;
pub mod note_roots;
pub mod title_format;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// Correlation id tying an `IoError::Pending` / `CommandError::Pending`
/// reply back to its originating call via `UiEvent::ServiceReply`.
pub type RequestId = u64;

#[derive(Debug, Error)]
pub enum IoError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("io: {0}")]
    Other(String),
    /// Web/remote: the request is in-flight; reply will arrive as
    /// `UiEvent::ServiceReply` with this request id.
    #[error("pending request {0}")]
    Pending(RequestId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

pub trait FilesService: Send + Sync {
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, IoError>;
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, IoError>;
    fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<(), IoError>;
    fn stat(&self, path: &Path) -> Result<DirEntry, IoError>;
}

pub trait ClipboardService: Send + Sync {
    fn read(&self) -> Option<String>;
    fn write(&self, text: &str);
}

/// Severity hint for an OS-level notification request. Backends are
/// free to ignore this (the Linux D-Bus path has no equivalent today)
/// — it exists so future native impls (macOS `UNNotificationContent`
/// `interruptionLevel`, Windows `ToastImportance`) can map it onto
/// platform semantics, and so the web bridge can request the
/// `Notification` API at the right "noisy"/"quiet" tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationLevel {
    Info,
    Warn,
    Error,
}

impl Default for NotificationLevel {
    fn default() -> Self {
        NotificationLevel::Info
    }
}

/// Cross-frontend OS-notification surface. Native binds to
/// `neoism-notifier` (macOS `UNUserNotificationCenter` / Linux D-Bus /
/// Windows toasts). Web binds to a JS bridge that calls the browser's
/// `Notification` API after lazily requesting permission, falling back
/// to the in-app toast stack when permission is denied or the API is
/// unavailable.
///
/// Object-safe + `Send + Sync` so the host can wrap the active impl
/// in an `Arc<dyn …>` and hand panels a single `Services<'_>` bundle
/// per frame, matching `ClipboardService` / `FilesService`.
///
/// `notify` is fire-and-forget — the trait does not surface delivery
/// errors because the backends themselves are best-effort (D-Bus
/// connection refused, browser permission denied, macOS no bundle id,
/// etc. — all silently dropped today). Future revisions can add a
/// `try_notify -> Result<…>` shape if a caller needs to react to a
/// rejected toast.
pub trait NotificationService: Send + Sync {
    /// Show an OS notification with the given title, body, and
    /// severity. Backends may render the level visually (color, icon)
    /// or use it as a hint to escalate (sound, banner persistence) —
    /// callers should pick the level that matches the urgency, not
    /// the "ok we're done" / "oh no we failed" axis the in-app toast
    /// stack uses for color alone.
    fn notify(&self, title: &str, body: &str, level: NotificationLevel);
}

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("unknown command: {0}")]
    Unknown(String),
    #[error("denied")]
    Denied,
    #[error("io: {0}")]
    Io(String),
    #[error("pending request {0}")]
    Pending(RequestId),
}

pub trait CommandService: Send + Sync {
    fn run(&self, command: &str) -> Result<(), CommandError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    pub branch: Option<String>,
    pub dirty: bool,
}

pub trait GitService: Send + Sync {
    fn status(&self, repo: &Path) -> Result<GitStatus, IoError>;
    fn diff(&self, repo: &Path, path: Option<&Path>) -> Result<String, IoError>;
    // wave6 file_tree port: raw `git status --porcelain=v1 -z` bytes
    // for `repo`; the file_tree panel parses these locally so the
    // service layer stays untyped. Empty Vec = clean / not a repo.
    fn status_porcelain(&self, _repo: &Path) -> Result<Vec<u8>, IoError> {
        Ok(Vec::new())
    }
    // wave6 file_tree port: `git rev-parse --show-toplevel` for `cwd`;
    // mirrors `SearchService::git_repo_root` but lives on `GitService`
    // so the file_tree port doesn't have to touch the search trait.
    fn repo_root(&self, _cwd: &Path) -> Option<std::path::PathBuf> {
        None
    }
    // wave6 file_tree port: `git rev-parse --absolute-git-dir` for `cwd`,
    // used to set up filesystem watchers in the host.
    fn absolute_git_dir(&self, _cwd: &Path) -> Option<std::path::PathBuf> {
        None
    }
}

pub trait ClockService: Send + Sync {
    fn now_monotonic(&self) -> web_time::Duration;
}

/// File-search mode (mirrors the finder's `FileSearchMode` enum but
/// lives here so service impls don't have to depend on panel types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFileMode {
    Fuzzy,
    Exact,
}

/// Grep-search mode (mirrors the finder's `GrepSearchMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchGrepMode {
    Fuzzy,
    Exact,
    Regex,
}

/// One scored file path returned by the search service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchFileHit {
    pub score: i32,
    pub path: String,
}

/// One scored directory path returned by the search service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchDirectoryHit {
    pub score: i32,
    pub path: String,
}

/// One scored grep match (path/line/column/text) returned by the
/// search service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchGrepHit {
    pub score: i32,
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub text: String,
}

/// Git porcelain status group for a single changed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchGitStatus {
    Modified,
    Staged,
    Mixed,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

/// One row in the git-changes finder mode — path + porcelain status +
/// the first changed line plus its text (for preview).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchGitHit {
    pub path: String,
    pub status: SearchGitStatus,
    pub line: u32,
    pub text: String,
}

/// Higher-level search capability surfaced to panels. On native this
/// wraps `rg`, `fff_search::FilePicker`, and `git status --porcelain`;
/// on web the host marshals each call across the daemon WebSocket.
///
/// All calls are synchronous from the panel's perspective; web impls
/// return `IoError::Pending(req_id)` and the panel re-runs after
/// `UiEvent::ServiceReply` lands.
pub trait SearchService: Send + Sync {
    /// `rg --files` rooted at `cwd`; relative paths.
    /// Added wave6: replaces direct `Command::new("rg")` in finder.
    fn collect_files(&self, cwd: &Path) -> Result<Vec<String>, IoError>;

    /// Fuzzy/exact file picker over the same `cwd`.
    /// Added wave6: hides `fff_search::FilePicker` behind the trait.
    fn search_files(
        &self,
        cwd: &Path,
        query: &str,
        mode: SearchFileMode,
    ) -> Result<Vec<SearchFileHit>, IoError>;

    /// Fuzzy directory-only search rooted at `cwd`; paths are relative.
    fn search_directories(
        &self,
        cwd: &Path,
        query: &str,
    ) -> Result<Vec<SearchDirectoryHit>, IoError>;

    /// `rg <query>` (or fuzzy/regex variant) rooted at `cwd`.
    /// Added wave6: replaces direct `Command::new("rg")` in finder.
    fn search_grep(
        &self,
        cwd: &Path,
        query: &str,
        mode: SearchGrepMode,
    ) -> Result<Vec<SearchGrepHit>, IoError>;

    /// `git status --porcelain=v1 -z` parsed into change rows for the
    /// repository containing `cwd`. Added wave6.
    fn collect_git_changes(&self, cwd: &Path) -> Result<Vec<SearchGitHit>, IoError>;

    /// Resolve `git rev-parse --show-toplevel` for `cwd`. Added wave6.
    fn git_repo_root(&self, cwd: &Path) -> Option<std::path::PathBuf>;
}

/// One LSP request fired by the shared code-editor session layer
/// (`crate::editor::code::lsp_session`). Positions follow the engine
/// facade contract: 0-based line, 0-based UTF-8 BYTE column. `seq` is
/// the session's monotonic request token — hosts echo it on the
/// matching result push so stale replies are dropped by the session.
///
/// Opaque payloads (`action`) are raw LSP JSON only the language-server
/// side interprets, mirroring the desktop worker's job shapes.
#[derive(Debug, Clone)]
pub enum LspRequest {
    /// Ship the full buffer text (didOpen/didChange coalesced by the
    /// backend). Fired whenever the pane revision moves.
    Sync {
        path: std::path::PathBuf,
        text: String,
        revision: u64,
    },
    /// didSave follow-up after a successful write.
    SaveNotify { path: std::path::PathBuf },
    Completion {
        path: std::path::PathBuf,
        line: u32,
        character: u32,
        trigger: Option<String>,
        seq: u64,
    },
    Hover {
        path: std::path::PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    SignatureHelp {
        path: std::path::PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    Definition {
        path: std::path::PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    References {
        path: std::path::PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    CodeActions {
        path: std::path::PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    /// Apply one accepted code action (resolve → edit/command on the
    /// server side). `action` is the raw LSP CodeAction/Command payload
    /// from the CodeActions result row.
    ApplyCodeAction {
        path: std::path::PathBuf,
        server_id: String,
        title: String,
        action: serde_json::Value,
        seq: u64,
    },
    Rename {
        path: std::path::PathBuf,
        line: u32,
        character: u32,
        new_name: String,
        seq: u64,
    },
    /// Format the document; the session applies the returned edits
    /// revision-guarded and then finishes the save (format-on-save).
    Format {
        path: std::path::PathBuf,
        revision: u64,
        seq: u64,
    },
}

/// Language-server capability surfaced to the shared code-editor LSP
/// session layer. Requests are fire-and-forget with seq tokens —
/// results are pushed back into the session by the host (desktop
/// drains its worker mailbox; web routes daemon `EditorReply`
/// envelopes), so unlike `FilesService` there is no synchronous
/// return payload. Web impls marshal each request into a daemon
/// editor envelope and may return `IoError::Pending(req)` when they
/// want the reply correlated by request id in addition to the seq.
pub trait LspService: Send + Sync {
    fn request(&self, request: LspRequest) -> Result<(), IoError>;
}

/// Inert `LspService` that drops every request — the default for hosts
/// without a language-server backend (mirrors `NullSearchService`).
pub struct NullLspService;

impl LspService for NullLspService {
    fn request(&self, _request: LspRequest) -> Result<(), IoError> {
        Ok(())
    }
}

/// Bundle of borrowed service references passed to panels per frame.
///
/// Panels reach across to any capability without owning `Arc`s, and
/// the host stays free to swap impls (native vs web) without
/// touching panel code.
pub struct Services<'a> {
    pub files: &'a dyn FilesService,
    pub clipboard: &'a dyn ClipboardService,
    pub commands: &'a dyn CommandService,
    pub git: &'a dyn GitService,
    pub clock: &'a dyn ClockService,
    pub search: &'a dyn SearchService,
    pub notifications: &'a dyn NotificationService,
}

/// Inert `SearchService` that returns empty results for every call. Useful
/// for tests and for hosts that don't yet wire a real search backend.
pub struct NullSearchService;

impl SearchService for NullSearchService {
    fn collect_files(&self, _cwd: &Path) -> Result<Vec<String>, IoError> {
        Ok(Vec::new())
    }
    fn search_files(
        &self,
        _cwd: &Path,
        _query: &str,
        _mode: SearchFileMode,
    ) -> Result<Vec<SearchFileHit>, IoError> {
        Ok(Vec::new())
    }
    fn search_directories(
        &self,
        _cwd: &Path,
        _query: &str,
    ) -> Result<Vec<SearchDirectoryHit>, IoError> {
        Ok(Vec::new())
    }
    fn search_grep(
        &self,
        _cwd: &Path,
        _query: &str,
        _mode: SearchGrepMode,
    ) -> Result<Vec<SearchGrepHit>, IoError> {
        Ok(Vec::new())
    }
    fn collect_git_changes(&self, _cwd: &Path) -> Result<Vec<SearchGitHit>, IoError> {
        Ok(Vec::new())
    }
    fn git_repo_root(&self, _cwd: &Path) -> Option<std::path::PathBuf> {
        None
    }
}

/// Inert `NotificationService` that silently drops every request. The
/// default for tests and for hosts that don't (yet) want to surface OS
/// notifications. Mirrors `NullSearchService` in spirit — present so
/// `Services { … }` constructors don't have to reach for an `Arc<…>`
/// of a real backend just to compile.
pub struct NullNotificationService;

impl NotificationService for NullNotificationService {
    fn notify(&self, _title: &str, _body: &str, _level: NotificationLevel) {}
}
