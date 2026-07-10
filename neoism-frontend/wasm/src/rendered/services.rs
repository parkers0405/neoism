use super::*;
use neoism_protocol::search::{
    SearchClientMessage, SearchFileMode as ProtoSearchFileMode,
    SearchGrepMode as ProtoSearchGrepMode,
};
use neoism_ui::services::{
    ClipboardService, ClockService, CommandError, CommandService, DirEntry, FilesService,
    GitService, GitStatus, IoError, NotificationLevel, NotificationService, RequestId,
    SearchFileHit, SearchFileMode, SearchGitHit, SearchGrepHit, SearchGrepMode,
    SearchService,
};
use std::path::Path;
use web_time::Duration;

fn call_with_id_and_path(cb: Option<&js_sys::Function>, id: RequestId, path: &Path) {
    if let Some(f) = cb {
        let _ = f.call2(
            &JsValue::NULL,
            &JsValue::from_f64(id as f64),
            &JsValue::from_str(&path.to_string_lossy()),
        );
    }
}

fn call_with_id_path_bytes(
    cb: Option<&js_sys::Function>,
    id: RequestId,
    path: &Path,
    bytes: &[u8],
) {
    if let Some(f) = cb {
        // Pass bytes as a Uint8Array so JS sees a real byte view
        // rather than a stringified path.
        let buf = js_sys::Uint8Array::from(bytes);
        let _ = f.call3(
            &JsValue::NULL,
            &JsValue::from_f64(id as f64),
            &JsValue::from_str(&path.to_string_lossy()),
            &buf.into(),
        );
    }
}

pub(crate) struct JsFilesService(pub(crate) SharedState);

impl FilesService for JsFilesService {
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.list_dir.clone();
        drop(s);
        call_with_id_and_path(cb.as_ref(), id, path);
        Err(IoError::Pending(id))
    }

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.read_file.clone();
        drop(s);
        call_with_id_and_path(cb.as_ref(), id, path);
        Err(IoError::Pending(id))
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<(), IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.write_file.clone();
        drop(s);
        call_with_id_path_bytes(cb.as_ref(), id, path, bytes);
        Err(IoError::Pending(id))
    }

    fn stat(&self, path: &Path) -> Result<DirEntry, IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.stat.clone();
        drop(s);
        call_with_id_and_path(cb.as_ref(), id, path);
        Err(IoError::Pending(id))
    }
}

pub(crate) struct JsClipboardService(pub(crate) SharedState);

impl ClipboardService for JsClipboardService {
    fn read(&self) -> Option<String> {
        // Clipboard read is sync-shaped in the trait. We answer
        // from the cache JS populates via `set_clipboard_value`
        // and additionally fire the async cb so JS can refresh
        // the cache for the next call.
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.clipboard_read.clone();
        let cached = s.clipboard_cached.clone();
        drop(s);
        if let Some(f) = cb {
            let _ = f.call1(&JsValue::NULL, &JsValue::from_f64(id as f64));
        }
        cached
    }

    fn write(&self, text: &str) {
        let s = self.0 .0.borrow();
        let cb = s.clipboard_write.clone();
        drop(s);
        if let Some(f) = cb {
            let _ = f.call1(&JsValue::NULL, &JsValue::from_str(text));
        }
    }
}

pub(crate) struct JsCommandService(pub(crate) SharedState);

impl CommandService for JsCommandService {
    fn run(&self, command: &str) -> Result<(), CommandError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.command_run.clone();
        drop(s);
        if let Some(f) = cb {
            let _ = f.call2(
                &JsValue::NULL,
                &JsValue::from_f64(id as f64),
                &JsValue::from_str(command),
            );
        }
        Err(CommandError::Pending(id))
    }
}

pub(crate) struct JsGitService(pub(crate) SharedState);

impl GitService for JsGitService {
    fn status(&self, repo: &Path) -> Result<GitStatus, IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.git_status.clone();
        drop(s);
        call_with_id_and_path(cb.as_ref(), id, repo);
        Err(IoError::Pending(id))
    }

    fn diff(&self, repo: &Path, path: Option<&Path>) -> Result<String, IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.git_diff.clone();
        drop(s);
        if let Some(f) = cb {
            let p_path = match path {
                Some(p) => JsValue::from_str(&p.to_string_lossy()),
                None => JsValue::NULL,
            };
            let _ = f.call3(
                &JsValue::NULL,
                &JsValue::from_f64(id as f64),
                &JsValue::from_str(&repo.to_string_lossy()),
                &p_path,
            );
        }
        Err(IoError::Pending(id))
    }
}

pub(crate) struct JsSearchService(pub(crate) SharedState);

/// Fire a search callback with `(req_id, envelope_json)`. The
/// envelope is a serialized `SearchClientMessage` the host
/// forwards over the daemon websocket; the matching reply lands
/// back through `service_reply`.
fn call_with_id_and_envelope(
    cb: Option<&js_sys::Function>,
    id: RequestId,
    envelope: &SearchClientMessage,
) {
    if let Some(f) = cb {
        // Serialization can't realistically fail for these PODs,
        // but if it does we drop the call silently — matches the
        // "callback missing" branch above.
        let Ok(json) = serde_json::to_string(envelope) else {
            return;
        };
        let _ = f.call2(
            &JsValue::NULL,
            &JsValue::from_f64(id as f64),
            &JsValue::from_str(&json),
        );
    }
}

fn map_file_mode(mode: SearchFileMode) -> ProtoSearchFileMode {
    match mode {
        SearchFileMode::Fuzzy => ProtoSearchFileMode::Fuzzy,
        SearchFileMode::Exact => ProtoSearchFileMode::Exact,
    }
}

fn map_grep_mode(mode: SearchGrepMode) -> ProtoSearchGrepMode {
    match mode {
        SearchGrepMode::Fuzzy => ProtoSearchGrepMode::Fuzzy,
        SearchGrepMode::Exact => ProtoSearchGrepMode::Exact,
        SearchGrepMode::Regex => ProtoSearchGrepMode::Regex,
    }
}

impl SearchService for JsSearchService {
    fn collect_files(&self, cwd: &Path) -> Result<Vec<String>, IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.search_collect_files.clone();
        drop(s);
        let envelope = SearchClientMessage::CollectFiles {
            req_id: id,
            cwd: cwd.to_string_lossy().into_owned(),
        };
        call_with_id_and_envelope(cb.as_ref(), id, &envelope);
        Err(IoError::Pending(id))
    }

    fn search_files(
        &self,
        cwd: &Path,
        query: &str,
        mode: SearchFileMode,
    ) -> Result<Vec<SearchFileHit>, IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.search_files.clone();
        drop(s);
        let envelope = SearchClientMessage::SearchFiles {
            req_id: id,
            query: query.to_string(),
            cwd: cwd.to_string_lossy().into_owned(),
            mode: map_file_mode(mode),
        };
        call_with_id_and_envelope(cb.as_ref(), id, &envelope);
        Err(IoError::Pending(id))
    }

    fn search_grep(
        &self,
        cwd: &Path,
        query: &str,
        mode: SearchGrepMode,
    ) -> Result<Vec<SearchGrepHit>, IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.search_grep.clone();
        drop(s);
        let envelope = SearchClientMessage::SearchGrep {
            req_id: id,
            query: query.to_string(),
            cwd: cwd.to_string_lossy().into_owned(),
            mode: map_grep_mode(mode),
            case_sensitive: None,
            file_patterns: Vec::new(),
        };
        call_with_id_and_envelope(cb.as_ref(), id, &envelope);
        Err(IoError::Pending(id))
    }

    fn collect_git_changes(&self, cwd: &Path) -> Result<Vec<SearchGitHit>, IoError> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.search_git_changes.clone();
        drop(s);
        let envelope = SearchClientMessage::SearchGitChanges {
            req_id: id,
            cwd: cwd.to_string_lossy().into_owned(),
        };
        call_with_id_and_envelope(cb.as_ref(), id, &envelope);
        Err(IoError::Pending(id))
    }

    fn git_repo_root(&self, cwd: &Path) -> Option<std::path::PathBuf> {
        let mut s = self.0 .0.borrow_mut();
        let id = s.alloc_request_id();
        let cb = s.search_git_repo_root.clone();
        drop(s);
        let envelope = SearchClientMessage::GitRepoRoot {
            req_id: id,
            cwd: cwd.to_string_lossy().into_owned(),
        };
        call_with_id_and_envelope(cb.as_ref(), id, &envelope);
        // Sync-shaped trait method: we fire the request for the
        // host to cache the answer, but have nothing to return
        // until the reply lands via `service_reply`.
        None
    }
}

pub(crate) struct JsClockService(pub(crate) SharedState);

impl ClockService for JsClockService {
    fn now_monotonic(&self) -> Duration {
        let ms = self.0 .0.borrow().now_ms;
        Duration::from_micros((ms * 1000.0).max(0.0) as u64)
    }
}

pub(crate) struct JsNotificationService(pub(crate) SharedState);

impl NotificationService for JsNotificationService {
    fn notify(&self, title: &str, body: &str, level: NotificationLevel) {
        // Resolve the outbox up front and drop the borrow before
        // calling JS — the callback could re-enter the bridge.
        let s = self.0 .0.borrow();
        let cb = s.notification_outbox.clone();
        drop(s);
        let Some(f) = cb else {
            // No outbox installed yet: silently drop, matching how
            // the other service shims behave before JS wires their
            // callbacks. The OS notification path is best-effort.
            return;
        };
        let level_str = match level {
            NotificationLevel::Info => "info",
            NotificationLevel::Warn => "warn",
            NotificationLevel::Error => "error",
        };
        let _ = f.call3(
            &JsValue::NULL,
            &JsValue::from_str(title),
            &JsValue::from_str(body),
            &JsValue::from_str(level_str),
        );
    }
}
