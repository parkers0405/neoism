//! LSP bridge for the native code pane: the pane is the buffer-text
//! source feeding the in-process Rust LSP engine (`neoism-agent-server`
//! runs inside this process for the agent already — same singleton).
//!
//! Data flow:
//! - edits: `pump_code_lsp` (per frame) notices a buffer-revision move
//!   and ships the full text to a worker thread that calls
//!   `sync_document` (didOpen/didChange), coalescing bursts.
//! - diagnostics: a thread blocks on the engine's `publishDiagnostics`
//!   broadcast, folds events into a global per-file store, and wakes
//!   the render loop; the pump converts UTF-16 ranges into per-line
//!   byte spans on the pane (`CodePane::diagnostics`) for squiggles.
//! - queries (completion/hover/definition): input paths enqueue
//!   seq-tokened jobs on the same worker; the worker calls the blocking
//!   facade, drops all but the newest job per kind, writes the result
//!   into a global mailbox and wakes the render loop; `pump_code_lsp`
//!   drains the mailbox into the UI session state on
//!   `Renderer::code_lsp` (stale seqs are dropped there).
//!
//! Coordinate contract: the engine facade speaks zero-based lines and
//! zero-based UTF-8 BYTE columns in both directions (`crate::lsp`
//! converts to/from each server's negotiated encoding at the wire), so
//! the pane's `cursor_col` byte offsets pass through unchanged.
//! EXCEPTION: diagnostic ranges are NOT converted by the engine
//! (`lsp_parse::parse_diagnostics` stores the raw wire range), so the
//! fold below converts assuming UTF-16 — correct for most servers;
//! UTF-8-negotiated servers only drift on non-ASCII lines.

use super::*;
use neoism_agent_server::language_server as engine;
use neoism_backend::event::{EventProxy, RioEvent, RioEventType, WindowId};
use neoism_ui::editor::code::layout::byte_for_utf16_col;
use neoism_ui::editor::code::{CodeDiagnosticSeverity, CodeLineDiagnostic};
use neoism_ui::editor_snapshot::{PopupMenu, PopupMenuItem};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

enum CodeLspJob {
    Sync {
        root: PathBuf,
        file: PathBuf,
        text: String,
    },
    Save {
        root: PathBuf,
        file: PathBuf,
    },
    Completion {
        root: PathBuf,
        file: PathBuf,
        text: String,
        line: u32,
        character: u32,
        trigger: Option<String>,
        seq: u64,
    },
    Hover {
        root: PathBuf,
        file: PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    Definition {
        root: PathBuf,
        file: PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    /// Format-on-save: run the server formatter, ship the edits back;
    /// the pump applies them (revision-guarded) and finishes the save.
    FormatThenSave {
        root: PathBuf,
        file: PathBuf,
        revision: u64,
    },
    /// Fire-and-forget follow-up for an accepted completion item that
    /// carries an LSP `command` (`workspace/executeCommand`).
    CompletionCommand {
        root: PathBuf,
        file: PathBuf,
        server_id: String,
        command: serde_json::Value,
    },
    /// Code actions at the cursor (`<Space>a` / Ctrl+.).
    CodeActions {
        root: PathBuf,
        file: PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    /// Apply one accepted code action: resolve when it carries no
    /// edit, run its `command` when present, ship the workspace edit
    /// back for the pump to apply. Never coalesced.
    ApplyCodeAction {
        root: PathBuf,
        file: PathBuf,
        server_id: String,
        title: String,
        action: serde_json::Value,
    },
    /// Find references at the cursor (vim `gr`). The worker also
    /// reads each hit's line text so the finder rows are ready-made.
    References {
        root: PathBuf,
        file: PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    /// Rename the symbol at a position (`<Space>r` modal submit).
    Rename {
        root: PathBuf,
        file: PathBuf,
        line: u32,
        character: u32,
        new_name: String,
        seq: u64,
    },
    /// Document symbols for the finder's `@` quick-jump (Symbols
    /// mode). The worker flattens the symbol tree into ready-made
    /// finder rows.
    DocumentSymbols {
        root: PathBuf,
        file: PathBuf,
        seq: u64,
    },
    /// Signature help at the caret (auto-triggered by `(`/`,`,
    /// retriggered per keystroke while the session is live). The
    /// result rides the HOVER mailbox as one synthetic hover, so the
    /// card surface, dismissal and rendering are shared.
    SignatureHelp {
        root: PathBuf,
        file: PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
    /// Occurrences of the symbol under the caret
    /// (textDocument/documentHighlight), requested on caret idle; the
    /// painter bands them like a quiet hlsearch.
    DocumentHighlight {
        root: PathBuf,
        file: PathBuf,
        line: u32,
        character: u32,
        seq: u64,
    },
}

struct CodeLspShared {
    jobs: Sender<CodeLspJob>,
}

static CODE_LSP: OnceLock<CodeLspShared> = OnceLock::new();
/// Bumped on every diagnostics push; panes fold the store in when their
/// seen version lags.
static DIAG_VERSION: AtomicU64 = AtomicU64::new(1);
/// Monotonic token for completion/hover/definition requests. The pump
/// only installs a result whose seq matches the newest request of that
/// kind, so late replies to superseded requests are dropped.
static QUERY_SEQ: AtomicU64 = AtomicU64::new(1);

type DiagStore = Mutex<HashMap<PathBuf, HashMap<String, Vec<engine::LspDiagnostic>>>>;

fn diag_store() -> &'static DiagStore {
    static STORE: OnceLock<DiagStore> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Per-file publish sequence: bumped ONLY when a server actually
/// publishes diagnostics for that file. Unlike the global
/// `DIAG_VERSION` (which also moves on heuristic re-anchors and other
/// files' publishes), this lets the sticky-anchor path refold from the
/// raw store exactly when fresh positions exist — a version bump from
/// anywhere else must never overwrite anchor-precise spans with stale
/// store positions.
fn diag_publish_seq() -> &'static Mutex<HashMap<PathBuf, u64>> {
    static STORE: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Diagnostics are keyed by CANONICAL path: the engine's event carries
/// the path parsed from the server's URI, which can differ from the
/// pane's path in symlinks/normalization — a raw string match silently
/// drops every diagnostic.
fn canonical_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Latest result per query kind, written by the worker and drained by
/// `pump_code_lsp`. Newest-wins by design — a fresh result overwrites
/// an undrained older one.
#[derive(Default)]
struct CodeLspResults {
    completion: Option<(u64, PathBuf, Vec<engine::LspCompletionItem>)>,
    hover: Option<(u64, PathBuf, Vec<engine::LspHover>)>,
    definition: Option<(u64, Vec<engine::LspLocation>)>,
    /// (file, buffer revision the format ran against, raw LSP edits).
    format_save: Option<(PathBuf, u64, Vec<serde_json::Value>)>,
    /// Flattened code actions for the popup menu.
    code_actions: Option<(u64, PathBuf, Vec<CodeActionItem>)>,
    /// `(title, workspace edit, ran a command)` for an accepted action.
    action_applied: Option<(String, Option<serde_json::Value>, bool)>,
    /// `(seq, workspace root, ready-made finder rows)`.
    references: Option<(u64, PathBuf, Vec<neoism_ui::panels::finder::ReferenceRow>)>,
    /// Per-server `{language, path, edit}` groups from `engine::rename`.
    rename: Option<(u64, Vec<serde_json::Value>)>,
    /// `(seq, flattened symbol rows)` for the finder Symbols mode.
    document_symbols: Option<(u64, Vec<neoism_ui::panels::finder::SymbolRow>)>,
    /// `(seq, file, zero-based (line, byte start, byte end) spans)` of
    /// the symbol under the caret (documentHighlight).
    occurrences: Option<(u64, PathBuf, Vec<(usize, usize, usize)>)>,
}

fn results_store() -> &'static Mutex<CodeLspResults> {
    static STORE: OnceLock<Mutex<CodeLspResults>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Per-file completion trigger characters advertised by the server,
/// fetched by the worker after document sync (the facade call can block
/// on server startup, so the UI thread only ever reads this cache).
fn trigger_store() -> &'static Mutex<HashMap<PathBuf, Vec<String>>> {
    static STORE: OnceLock<Mutex<HashMap<PathBuf, Vec<String>>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Fallback trigger characters used until the server's own set has been
/// fetched (rust-analyzer & friends all advertise at least these).
const DEFAULT_TRIGGERS: [&str; 2] = [".", ":"];

fn lsp_log() -> bool {
    std::env::var_os("NEOISM_LSP_LOG").is_some()
}

/// Status-pill cache: (panel state, server label, popup server rows)
/// per file, refreshed by the worker after each document sync —
/// `engine::status` can block behind server startup locks, so the UI
/// thread only reads this.
type LspPillStore = Mutex<
    HashMap<
        PathBuf,
        (
            neoism_ui::panels::status_line::LspStatus,
            String,
            Vec<neoism_ui::panels::lsp_popup::LspServerRow>,
        ),
    >,
>;

fn lsp_pill_store() -> &'static LspPillStore {
    static STORE: OnceLock<LspPillStore> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

fn refresh_lsp_pill(root: &Path, file: &Path) {
    use neoism_ui::panels::status_line::LspStatus as Pill;
    // engine::status() WALKS the workspace (up to 10k files) — running
    // it per keystroke-sync starves the worker and delays didChange
    // delivery (stale diagnostics while typing). Throttle per file.
    {
        static LAST_REFRESH: OnceLock<Mutex<HashMap<PathBuf, std::time::Instant>>> =
            OnceLock::new();
        let mut last = match LAST_REFRESH.get_or_init(Default::default).lock() {
            Ok(last) => last,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = std::time::Instant::now();
        match last.get(file) {
            Some(at) if now.duration_since(*at).as_secs_f32() < 3.0 => return,
            _ => {
                last.insert(file.to_path_buf(), now);
            }
        }
    }
    let statuses = engine::status(root, Some(file));
    let connected: Vec<&str> = statuses
        .iter()
        .filter(|s| s.status == engine::LspServerState::Connected)
        .map(|s| s.name.as_str())
        .collect();
    // Popup rows (the status-pill click-through "Server Details").
    let rows: Vec<neoism_ui::panels::lsp_popup::LspServerRow> = statuses
        .iter()
        .map(|s| {
            use neoism_ui::panels::lsp_popup::LspServerState as RowState;
            neoism_ui::panels::lsp_popup::LspServerRow {
                name: s.name.clone(),
                binary: s.command.first().cloned(),
                filetype: Some(s.language.clone()),
                state: match s.status {
                    engine::LspServerState::Connected => RowState::Active,
                    engine::LspServerState::Available => RowState::Ready,
                    engine::LspServerState::Error => RowState::Errored,
                },
                message: None,
                level: None,
                diagnostics: Default::default(),
                source: Some(
                    match s.command_source {
                        engine::LspCommandSource::BuiltIn => "built-in",
                        engine::LspCommandSource::Extension => "managed",
                        engine::LspCommandSource::Config => "config",
                        engine::LspCommandSource::Path => "path",
                        engine::LspCommandSource::Missing => "missing",
                    }
                    .to_string(),
                ),
            }
        })
        .collect();
    let entry = if !connected.is_empty() {
        let label = match connected.len() {
            1 => connected[0].to_string(),
            n => format!("{}+{}", connected[0], n - 1),
        };
        (Pill::Active, label, rows)
    } else if statuses.is_empty() {
        (Pill::Missing, String::new(), rows)
    } else {
        (Pill::Initializing, statuses[0].name.clone(), rows)
    };
    let mut store = match lsp_pill_store().lock() {
        Ok(store) => store,
        Err(poisoned) => poisoned.into_inner(),
    };
    store.insert(file.to_path_buf(), entry);
}

fn parse_lsp_text_edits(
    edits: &[serde_json::Value],
) -> Vec<neoism_ui::editor::code::buffer::CodeTextEdit> {
    edits
        .iter()
        .filter_map(|edit| {
            Some(neoism_ui::editor::code::buffer::CodeTextEdit {
                start_line: edit.pointer("/range/start/line")?.as_u64()? as usize,
                start_col: edit.pointer("/range/start/character")?.as_u64()? as usize,
                end_line: edit.pointer("/range/end/line")?.as_u64()? as usize,
                end_col: edit.pointer("/range/end/character")?.as_u64()? as usize,
                text: edit.get("newText")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// One selectable row of the code-action popup. `action` is the raw
/// LSP CodeAction/Command payload, resolved lazily on accept.
#[derive(Clone)]
pub struct CodeActionItem {
    pub server_id: String,
    pub title: String,
    pub kind: String,
    pub action: serde_json::Value,
}

/// Flatten the engine's per-server `{language, path, actions}` groups
/// into popup rows. Preferred actions (server hint) bubble to the top,
/// otherwise server order is kept.
fn flatten_code_actions(groups: &[serde_json::Value]) -> Vec<CodeActionItem> {
    let mut items = Vec::new();
    for group in groups {
        let server_id = group
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let Some(actions) = group.get("actions").and_then(|a| a.as_array()) else {
            continue;
        };
        for action in actions {
            let Some(title) = action.get("title").and_then(|t| t.as_str()) else {
                continue;
            };
            let kind = action
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or_default()
                .to_string();
            items.push(CodeActionItem {
                server_id: server_id.clone(),
                title: title.to_string(),
                kind,
                action: action.clone(),
            });
        }
    }
    items.sort_by_key(|item| {
        std::cmp::Reverse(
            item.action
                .get("isPreferred")
                .and_then(|p| p.as_bool())
                .unwrap_or(false),
        )
    });
    items
}

/// Short badge for a code-action kind (`quickfix` → "fix",
/// `refactor.extract` → "refactor", `source.organizeImports` →
/// "source", plain commands → "cmd").
fn action_kind_label(item: &CodeActionItem) -> &'static str {
    let head = item.kind.split('.').next().unwrap_or("");
    match head {
        "quickfix" => "fix",
        "refactor" => "refactor",
        "source" => "source",
        "" => {
            if item.action.get("command").is_some_and(|c| c.is_string()) {
                "cmd"
            } else {
                "action"
            }
        }
        _ => "action",
    }
}

/// Minimal `file://` URI → path decoder (percent-decoded). Workspace
/// edits key files by URI; the engine's own decoder isn't exported.
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///path` → rest starts at '/'; `file://host/path` drops host.
    let path_start = rest.find('/')?;
    let raw = rest[path_start..].as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut ix = 0;
    while ix < raw.len() {
        if raw[ix] == b'%' && ix + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[ix + 1..ix + 3]).ok()?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            bytes.push(byte);
            ix += 3;
        } else {
            bytes.push(raw[ix]);
            ix += 1;
        }
    }
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

/// Collect a WorkspaceEdit's text edits per file, in byte coords (the
/// engine transport already converted them). Handles both the
/// `changes` uri-map and `documentChanges` TextDocumentEdit entries;
/// resource ops (create/rename/delete file) are skipped.
fn workspace_edit_file_edits(
    edit: &serde_json::Value,
) -> Vec<(PathBuf, Vec<serde_json::Value>)> {
    let mut per_file: Vec<(PathBuf, Vec<serde_json::Value>)> = Vec::new();
    let mut push = |path: PathBuf, edits: &[serde_json::Value]| {
        if edits.is_empty() {
            return;
        }
        if let Some(entry) = per_file.iter_mut().find(|(p, _)| *p == path) {
            entry.1.extend(edits.iter().cloned());
        } else {
            per_file.push((path, edits.to_vec()));
        }
    };
    if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            let Some(path) = file_uri_to_path(uri) else {
                continue;
            };
            if let Some(list) = edits.as_array() {
                push(path, list);
            }
        }
    }
    if let Some(doc_changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in doc_changes {
            let Some(uri) = change.pointer("/textDocument/uri").and_then(|u| u.as_str())
            else {
                // CreateFile/RenameFile/DeleteFile — unsupported here.
                continue;
            };
            let Some(path) = file_uri_to_path(uri) else {
                continue;
            };
            if let Some(list) = change.get("edits").and_then(|e| e.as_array()) {
                push(path, list);
            }
        }
    }
    per_file
}

fn floor_char_boundary_of(text: &str, mut ix: usize) -> usize {
    ix = ix.min(text.len());
    while ix > 0 && !text.is_char_boundary(ix) {
        ix -= 1;
    }
    ix
}

/// Read-patch-write for files without an open pane: apply byte-coord
/// LSP edits bottom-up (mirrors `CodeBuffer::apply_text_edits`) and
/// preserve the file's newline flavor / trailing newline.
fn apply_edits_on_disk(
    path: &Path,
    edits: &[neoism_ui::editor::code::buffer::CodeTextEdit],
) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let crlf = text.contains("\r\n");
    let cleaned = text.replace('\r', "");
    let trailing_newline = cleaned.ends_with('\n');
    let mut lines: Vec<String> = cleaned.split('\n').map(str::to_string).collect();
    if trailing_newline {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    let mut sorted: Vec<&neoism_ui::editor::code::buffer::CodeTextEdit> =
        edits.iter().collect();
    sorted.sort_by(|a, b| (b.start_line, b.start_col).cmp(&(a.start_line, a.start_col)));
    for edit in sorted {
        let last = lines.len().saturating_sub(1);
        let sl = edit.start_line.min(last);
        let el = edit.end_line.min(last).max(sl);
        let sc = floor_char_boundary_of(&lines[sl], edit.start_col);
        let ec = floor_char_boundary_of(&lines[el], edit.end_col);
        let head = lines[sl][..sc].to_string();
        let tail = lines[el][ec..].to_string();
        let replacement = format!("{head}{}{tail}", edit.text.replace('\r', ""));
        let new_lines: Vec<String> =
            replacement.split('\n').map(str::to_string).collect();
        lines.splice(sl..=el, new_lines);
    }
    let newline = if crlf { "\r\n" } else { "\n" };
    let mut out = lines.join(newline);
    if trailing_newline {
        out.push_str(newline);
    }
    std::fs::write(path, out)
}

/// Git gutter baseline: the file's HEAD content as lines. `None` value
/// = fetched and absent (untracked / not a repo); missing key = not
/// fetched yet. Filled by a spawned `git show` thread; the UI thread
/// only reads.
type GitBaselineStore =
    Mutex<HashMap<PathBuf, Option<std::sync::Arc<Vec<String>>>>>;

fn git_baseline_store() -> &'static GitBaselineStore {
    static STORE: OnceLock<GitBaselineStore> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

fn git_baseline_inflight() -> &'static Mutex<std::collections::HashSet<PathBuf>> {
    static STORE: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> =
        OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// Baseline for `file`, spawning a one-shot `git show HEAD:./name`
/// fetch when it hasn't been loaded yet (the wake repaints once it
/// lands). Returns `None` until the fetch resolves or when the file
/// has no baseline.
fn git_baseline(
    file: &Path,
    proxy: &EventProxy,
    window_id: WindowId,
) -> Option<std::sync::Arc<Vec<String>>> {
    let key = canonical_key(file);
    {
        let store = match git_baseline_store().lock() {
            Ok(store) => store,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(entry) = store.get(&key) {
            return entry.clone();
        }
    }
    {
        let mut inflight = match git_baseline_inflight().lock() {
            Ok(inflight) => inflight,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !inflight.insert(key.clone()) {
            return None;
        }
    }
    let fetch_file = key.clone();
    let proxy = proxy.clone();
    let _ = std::thread::Builder::new()
        .name("code-git-baseline".into())
        .spawn(move || {
            let baseline = fetch_file.parent().and_then(|dir| {
                let name = fetch_file.file_name()?;
                let output = std::process::Command::new("git")
                    .arg("-C")
                    .arg(dir)
                    .arg("show")
                    .arg(format!("HEAD:./{}", name.to_string_lossy()))
                    .output()
                    .ok()?;
                if !output.status.success() {
                    return None;
                }
                let text =
                    String::from_utf8_lossy(&output.stdout).replace('\r', "");
                let mut lines: Vec<String> =
                    text.split('\n').map(str::to_string).collect();
                if text.ends_with('\n') {
                    lines.pop();
                }
                Some(std::sync::Arc::new(lines))
            });
            {
                let mut store = match git_baseline_store().lock() {
                    Ok(store) => store,
                    Err(poisoned) => poisoned.into_inner(),
                };
                store.insert(fetch_file.clone(), baseline);
            }
            {
                let mut inflight = match git_baseline_inflight().lock() {
                    Ok(inflight) => inflight,
                    Err(poisoned) => poisoned.into_inner(),
                };
                inflight.remove(&fetch_file);
            }
            proxy.send_event(RioEventType::Rio(RioEvent::Render), window_id);
        });
    None
}

/// Anchor-lite for diagnostics (Zed keeps real anchors; we shift the
/// stored ranges by the line-span delta of each edit): compare the
/// buffer against its previous snapshot, and when lines were inserted
/// or removed, move every stored diagnostic that sits BELOW the edited
/// span. Squiggles then track edits instead of drifting until the
/// server's next publish (which replaces the set wholesale).
fn reanchor_diagnostics(file: &Path, current: &[String]) {
    static PREV: OnceLock<Mutex<HashMap<PathBuf, Vec<String>>>> = OnceLock::new();
    let key = canonical_key(file);
    let prev = {
        let mut map = match PREV.get_or_init(Default::default).lock() {
            Ok(map) => map,
            Err(poisoned) => poisoned.into_inner(),
        };
        map.insert(key.clone(), current.to_vec())
    };
    let Some(prev) = prev else {
        return;
    };
    if prev.len() == current.len() {
        // Same line count: per-line edits don't move ranges.
        return;
    }
    let mut prefix = 0usize;
    let max_prefix = prev.len().min(current.len());
    while prefix < max_prefix && prev[prefix] == current[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    let max_suffix = max_prefix - prefix;
    while suffix < max_suffix
        && prev[prev.len() - 1 - suffix] == current[current.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let old_len = prev.len() - prefix - suffix;
    let new_len = current.len() - prefix - suffix;
    let delta = new_len as i64 - old_len as i64;
    if delta == 0 {
        return;
    }
    // First OLD line index at/after which ranges must shift.
    let boundary = (prefix + old_len) as i64;
    let shift = |line: &mut u32| {
        let shifted = (*line as i64 + delta).max(0);
        *line = shifted as u32;
    };
    let mut store = match diag_store().lock() {
        Ok(store) => store,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(by_server) = store.get_mut(&key) else {
        return;
    };
    let mut moved = false;
    for diags in by_server.values_mut() {
        for diag in diags.iter_mut() {
            let Some(range) = diag.range.as_mut() else {
                continue;
            };
            if (range.start.line as i64) >= boundary {
                shift(&mut range.start.line);
                shift(&mut range.end.line);
                moved = true;
            } else if (range.end.line as i64) >= boundary {
                shift(&mut range.end.line);
                moved = true;
            }
        }
    }
    drop(store);
    if moved {
        DIAG_VERSION.fetch_add(1, Ordering::SeqCst);
    }
}

/// Rebuild the pane's per-line diagnostic spans by resolving its
/// sticky anchors against the live CRDT doc — the Zed-grade path for
/// doc-bound panes. Local AND remote edits move the anchors with the
/// text they were pinned to (char-precise), where the line-shift
/// heuristic above only tracks whole-line insertions/deletions. The
/// per-line splitting/clamping mirrors the raw-store fold exactly so
/// the squiggle geometry is identical at publish time.
fn fold_anchored_diagnostics(
    code: &neoism_ui::editor::code::CodePane,
    binding: &neoism_ui::editor::code::doc_sync::CodeDocBinding,
) -> HashMap<usize, Vec<CodeLineDiagnostic>> {
    let mut per_line: HashMap<usize, Vec<CodeLineDiagnostic>> = HashMap::new();
    for anchor in &code.diag_anchors {
        let Some(start) = binding.resolve_sticky_anchor(&anchor.start) else {
            continue;
        };
        let Some(end) = binding.resolve_sticky_anchor(&anchor.end) else {
            continue;
        };
        // A delete spanning the range can leave the endpoints reversed;
        // normalize rather than dropping the diagnostic.
        let ((start_line, start_col), (end_line, end_col)) =
            if end < start { (end, start) } else { (start, end) };
        for line_ix in start_line..=end_line {
            let Some(line) = code.buffer.lines.get(line_ix) else {
                break;
            };
            let mut from = if line_ix == start_line {
                start_col.min(line.len())
            } else {
                0
            };
            let mut to = if line_ix == end_line {
                end_col.min(line.len())
            } else {
                line.len()
            };
            // Zero-width / end-of-line ranges still get a visible
            // one-cell underline (same policy as the store fold).
            if to <= from {
                if from >= line.len() && !line.is_empty() {
                    from = line.len() - 1;
                }
                to = (from + 1).min(line.len());
            }
            if from < to {
                per_line
                    .entry(line_ix)
                    .or_default()
                    .push(CodeLineDiagnostic {
                        start: from,
                        end: to,
                        severity: anchor.severity,
                        message: if line_ix == start_line {
                            anchor.message.clone()
                        } else {
                            String::new()
                        },
                    });
            }
        }
    }
    per_line
}

fn map_severity(severity: &str) -> CodeDiagnosticSeverity {
    match severity.to_ascii_lowercase().as_str() {
        "error" => CodeDiagnosticSeverity::Error,
        "warning" | "warn" => CodeDiagnosticSeverity::Warn,
        "hint" => CodeDiagnosticSeverity::Hint,
        _ => CodeDiagnosticSeverity::Info,
    }
}

// ---------------------------------------------------------------------
// UI session state (lives on `Renderer::code_lsp`, mutated by the
// Screen input/pump paths, read by the chrome pass in `host/run.rs`).
// ---------------------------------------------------------------------

/// Native code pane LSP popup state. One completion session and one
/// hover card at most; a newer request supersedes the older one.
#[derive(Default)]
pub struct CodeLspUiState {
    pub completion: Option<CodeCompletionSession>,
    pub hover: Option<CodeHoverCard>,
    /// Open code-action menu (`<Space>a` / Ctrl+.).
    pub actions: Option<CodeActionSession>,
    /// Seq of the in-flight go-to-definition request (newest wins).
    pub definition_seq: Option<u64>,
    /// Seq of the in-flight find-references request (newest wins).
    pub references_seq: Option<u64>,
    /// Seq of the in-flight rename request (newest wins).
    pub rename_seq: Option<u64>,
    /// Position captured when the rename modal opened; consumed by the
    /// modal submit arm.
    pub pending_rename: Option<PendingCodeRename>,
    /// Pointer-idle hover tracking (mouse LSP info).
    pub mouse_hover: Option<CodeMouseHover>,
    /// Live signature-help session: seq of the current card. Typing
    /// retriggers at the new caret; `)`/motions/Esc end it.
    pub signature_seq: Option<u64>,
    /// Caret-idle probe for documentHighlight (symbol occurrences).
    pub occurrence_probe: Option<CodeOccurrenceProbe>,
}

/// Caret-idle occurrence probe: armed when the caret comes to rest at
/// a new position, requested once it has been still for a beat.
pub struct CodeOccurrenceProbe {
    pub line: usize,
    pub col: usize,
    pub revision: u64,
    pub since: std::time::Instant,
    pub requested: bool,
    pub seq: u64,
}

impl CodeLspUiState {
    pub fn dismiss_popups(&mut self) {
        self.completion = None;
        self.hover = None;
        self.actions = None;
    }

    fn has_session_state(&self) -> bool {
        self.completion.is_some()
            || self.hover.is_some()
            || self.actions.is_some()
            || self.definition_seq.is_some()
            || self.references_seq.is_some()
            || self.rename_seq.is_some()
            || self.pending_rename.is_some()
    }
}

/// The rename target frozen at modal-open time (the modal is blocking,
/// so the cursor cannot move underneath it).
pub struct PendingCodeRename {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

/// An open code-action menu anchored at the request position. `items`
/// stays empty until the worker result lands; the popup only shows
/// once `display` has rows.
pub struct CodeActionSession {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    /// Stable per-session id (feeds `PopupMenu::grid`).
    pub id: u64,
    pub seq: u64,
    pub items: Vec<CodeActionItem>,
    pub selected: usize,
    pub display: PopupMenu,
}

fn build_action_popup(session: &CodeActionSession) -> PopupMenu {
    let items: Vec<PopupMenuItem> = session
        .items
        .iter()
        .map(|item| PopupMenuItem {
            word: item.title.clone(),
            kind: action_kind_label(item).to_string(),
            menu: item.server_id.clone(),
            info: String::new(),
        })
        .collect();
    let max_word_chars = items
        .iter()
        .map(|item| item.word.chars().count())
        .max()
        .unwrap_or(0);
    PopupMenu {
        items,
        selected: Some(session.selected),
        anchor_row: 0,
        anchor_col: 0,
        grid: session.id,
        max_word_chars,
    }
}

/// An open completion menu on the code pane. `display` is the popup
/// snapshot the shared `completion_menu` panel renders; it is rebuilt
/// whenever items/filter/selection change (not per frame).
pub struct CodeCompletionSession {
    pub path: PathBuf,
    /// Line the session opened on (menu dismisses if the cursor leaves).
    pub line: usize,
    /// Byte column where the to-be-replaced word starts.
    pub anchor_col: usize,
    /// Stable per-session id (feeds `PopupMenu::grid` so the panel's
    /// scroll animation resets between sessions).
    pub id: u64,
    /// Seq of the newest completion request for this session.
    pub seq: u64,
    /// Engine items, sorted by `sort_text`/label.
    pub items: Vec<engine::LspCompletionItem>,
    /// Indices into `items` surviving the current prefix filter.
    pub filtered: Vec<usize>,
    /// Index into `filtered` of the highlighted row.
    pub selected: usize,
    pub display: PopupMenu,
}

/// A hover card pinned to the buffer position it was requested at.
/// `lines` stays empty until the result arrives; the render pass skips
/// empty cards and the pump dismisses the card when the cursor moves.
pub struct CodeHoverCard {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub seq: u64,
    pub lines: Vec<String>,
    /// Mouse-idle hover (vs `K`/Ctrl+K): pinned to the hovered cell,
    /// dismissed when the pointer leaves it instead of on cursor move.
    pub from_mouse: bool,
}

/// Pointer-idle hover candidate: armed on mouse move over a cell,
/// requested once the pointer has rested on it long enough.
pub struct CodeMouseHover {
    pub line: usize,
    pub col: usize,
    pub since: std::time::Instant,
    pub requested: bool,
}

/// What a standard-path keystroke did to the buffer, for the LSP
/// after-key hook (menu refilter / trigger / dismissal decisions).
#[derive(Clone, Copy, Debug)]
pub(crate) enum CodeKeyEdit {
    Char(char),
    Backspace,
    Other,
}

pub(crate) fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte column where the identifier containing/ending at `col` starts.
fn word_start_col(line: &str, col: usize) -> usize {
    let col = col.min(line.len());
    let mut start = col;
    for (i, c) in line[..col].char_indices().rev() {
        if is_ident_char(c) {
            start = i;
        } else {
            break;
        }
    }
    start
}

/// The typed prefix between the session anchor and the cursor. `None`
/// when the range is invalid or contains non-identifier chars — the
/// session should dismiss then.
fn completion_prefix(line: &str, anchor: usize, cursor: usize) -> Option<String> {
    if anchor > cursor || cursor > line.len() {
        return None;
    }
    let slice = line.get(anchor..cursor)?;
    slice.chars().all(is_ident_char).then(|| slice.to_string())
}

/// Strip LSP snippet placeholders (`$0`, `$1`, `${2:default}`) from an
/// `insertTextFormat == 2` completion, keeping placeholder defaults.
/// v1 has no tab-stop editing — the caret lands after the insertion.
/// Like `strip_snippet_placeholders`, but also reports the FIRST
/// tabstop's `(byte offset, default-text len)` in the stripped output —
/// accept lands the caret there with the placeholder selected, so
/// typing replaces it (snippets v1: no Tab-chain yet).
fn snippet_with_first_stop(text: &str) -> (String, Option<(usize, usize)>) {
    let mut out = String::with_capacity(text.len());
    let mut first_stop: Option<(usize, usize)> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                if next == '$' || next == '}' || next == '\\' {
                    out.push(next);
                    chars.next();
                    continue;
                }
            }
            out.push(c);
            continue;
        }
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('{') => {
                chars.next();
                let start = out.len();
                let mut saw_colon = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        break;
                    }
                    if saw_colon {
                        out.push(inner);
                    } else if inner == ':' {
                        saw_colon = true;
                    }
                }
                if first_stop.is_none() {
                    first_stop = Some((start, out.len() - start));
                }
            }
            Some(d) if d.is_ascii_digit() => {
                while matches!(chars.peek(), Some(d) if d.is_ascii_digit()) {
                    chars.next();
                }
                if first_stop.is_none() {
                    first_stop = Some((out.len(), 0));
                }
            }
            _ => out.push('$'),
        }
    }
    (out, first_stop)
}

/// Flat placeholder strip (`${N:default}` → `default`, `$N` → ``);
/// superseded by `snippet_with_first_stop` on the accept path but kept
/// for callers that only need the text.
#[allow(dead_code)]
fn strip_snippet_placeholders(text: &str) -> String {
    snippet_with_first_stop(text).0
}

fn build_completion_popup(session: &CodeCompletionSession) -> PopupMenu {
    let items: Vec<PopupMenuItem> = session
        .filtered
        .iter()
        .filter_map(|&ix| session.items.get(ix))
        .map(|item| PopupMenuItem {
            word: item.label.clone(),
            kind: item.kind.clone(),
            menu: item.detail.clone().unwrap_or_default(),
            info: item.documentation.clone().unwrap_or_default(),
        })
        .collect();
    let max_word_chars = items
        .iter()
        .map(|item| item.word.chars().count())
        .max()
        .unwrap_or(0);
    PopupMenu {
        items,
        selected: Some(session.selected),
        anchor_row: 0,
        anchor_col: 0,
        grid: session.id,
        max_word_chars,
    }
}

/// Recompute `filtered`/`selected`/`display` for a prefix. Keeps the
/// server's preselect hint when it survives the filter.
fn rebuild_completion_filter(session: &mut CodeCompletionSession, prefix: &str) {
    let needle = prefix.to_lowercase();
    session.filtered = session
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            if needle.is_empty() {
                return true;
            }
            let haystack = item.filter_text.as_deref().unwrap_or(&item.label);
            haystack.to_lowercase().starts_with(&needle)
        })
        .map(|(ix, _)| ix)
        .collect();
    session.selected = session
        .filtered
        .iter()
        .position(|&ix| session.items[ix].preselect)
        .unwrap_or(0);
    session.display = build_completion_popup(session);
}

/// Flatten engine hovers into the markdown-ish line list the shared
/// hover popup parses (fences kept; multiple servers separated by a
/// blank line).
fn hover_card_lines(hovers: &[engine::LspHover]) -> Vec<String> {
    const MAX_LINES: usize = 40;
    let mut out: Vec<String> = Vec::new();
    for hover in hovers {
        if hover.contents.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(String::new());
        }
        for line in hover.contents.lines() {
            out.push(line.to_string());
            if out.len() >= MAX_LINES {
                return out;
            }
        }
    }
    out
}

fn ensure_workers(proxy: EventProxy, window_id: WindowId) -> &'static CodeLspShared {
    CODE_LSP.get_or_init(move || {
        let (tx, rx) = mpsc::channel::<CodeLspJob>();
        let query_proxy = proxy.clone();
        let _ = std::thread::Builder::new()
            .name("code-lsp-sync".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    // Coalesce a burst: only the newest Sync per file and
                    // the newest query per kind matter; Saves and
                    // completion follow-up commands all run.
                    let mut batch = vec![first];
                    while let Ok(next) = rx.try_recv() {
                        batch.push(next);
                    }
                    let mut newest_sync: HashMap<PathBuf, usize> = HashMap::new();
                    let mut newest_completion: Option<usize> = None;
                    let mut newest_hover: Option<usize> = None;
                    let mut newest_definition: Option<usize> = None;
                    let mut newest_actions: Option<usize> = None;
                    let mut newest_references: Option<usize> = None;
                    let mut newest_rename: Option<usize> = None;
                    let mut newest_symbols: Option<usize> = None;
                    let mut newest_signature: Option<usize> = None;
                    let mut newest_occurrences: Option<usize> = None;
                    for (ix, job) in batch.iter().enumerate() {
                        match job {
                            CodeLspJob::Sync { file, .. } => {
                                newest_sync.insert(file.clone(), ix);
                            }
                            CodeLspJob::Completion { .. } => {
                                newest_completion = Some(ix);
                            }
                            CodeLspJob::Hover { .. } => newest_hover = Some(ix),
                            CodeLspJob::Definition { .. } => {
                                newest_definition = Some(ix)
                            }
                            CodeLspJob::CodeActions { .. } => {
                                newest_actions = Some(ix)
                            }
                            CodeLspJob::References { .. } => {
                                newest_references = Some(ix)
                            }
                            CodeLspJob::Rename { .. } => newest_rename = Some(ix),
                            CodeLspJob::DocumentSymbols { .. } => {
                                newest_symbols = Some(ix)
                            }
                            CodeLspJob::SignatureHelp { .. } => {
                                newest_signature = Some(ix)
                            }
                            CodeLspJob::DocumentHighlight { .. } => {
                                newest_occurrences = Some(ix)
                            }
                            _ => {}
                        }
                    }
                    let mut woke = false;
                    for (ix, job) in batch.iter().enumerate() {
                        match job {
                            CodeLspJob::Sync { root, file, text } => {
                                if newest_sync.get(file) == Some(&ix) {
                                    let _ =
                                        engine::sync_document(root, file, Some(text));
                                    // Trigger characters piggyback on the
                                    // sync lane: fetch until the server
                                    // reports a non-empty set.
                                    let missing = {
                                        let store = match trigger_store().lock() {
                                            Ok(store) => store,
                                            Err(poisoned) => poisoned.into_inner(),
                                        };
                                        store
                                            .get(file)
                                            .is_none_or(|chars| chars.is_empty())
                                    };
                                    if missing {
                                        let chars =
                                            engine::completion_trigger_characters(
                                                root, file,
                                            );
                                        if !chars.is_empty() {
                                            let mut store =
                                                match trigger_store().lock() {
                                                    Ok(store) => store,
                                                    Err(poisoned) => {
                                                        poisoned.into_inner()
                                                    }
                                                };
                                            store.insert(file.clone(), chars);
                                        }
                                    }
                                    refresh_lsp_pill(root, file);
                                }
                            }
                            CodeLspJob::Save { root, file } => {
                                let _ = engine::save_document(root, file);
                            }
                            CodeLspJob::Completion {
                                root,
                                file,
                                text,
                                line,
                                character,
                                trigger,
                                seq,
                            } => {
                                if newest_completion != Some(ix) {
                                    continue;
                                }
                                let items = engine::completion_with_trigger(
                                    root,
                                    file,
                                    *line,
                                    *character,
                                    Some(text),
                                    trigger.as_deref(),
                                );
                                if lsp_log() {
                                    eprintln!(
                                        "neoism::lsp code completion result: seq={seq} items={} at {line}:{character}",
                                        items.len()
                                    );
                                }
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.completion = Some((*seq, file.clone(), items));
                                woke = true;
                            }
                            CodeLspJob::Hover {
                                root,
                                file,
                                line,
                                character,
                                seq,
                            } => {
                                if newest_hover != Some(ix) {
                                    continue;
                                }
                                let hovers =
                                    engine::hover(root, file, *line, *character);
                                if lsp_log() {
                                    eprintln!(
                                        "neoism::lsp code hover result: seq={seq} hovers={} at {line}:{character}",
                                        hovers.len()
                                    );
                                }
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.hover = Some((*seq, file.clone(), hovers));
                                woke = true;
                            }
                            CodeLspJob::SignatureHelp {
                                root,
                                file,
                                line,
                                character,
                                seq,
                            } => {
                                if newest_signature != Some(ix) {
                                    continue;
                                }
                                let helps = engine::signature_help(
                                    root, file, *line, *character,
                                );
                                // One synthetic hover carrying the active
                                // signature + parameter — rides the hover
                                // mailbox so the card surface is shared.
                                let hovers: Vec<engine::LspHover> = helps
                                    .into_iter()
                                    .next()
                                    .and_then(|help| {
                                        let count = help.signatures.len();
                                        let active = (help
                                            .active_signature
                                            .unwrap_or(0)
                                            as usize)
                                            .min(count.checked_sub(1)?);
                                        let sig = &help.signatures[active];
                                        let mut contents = sig.label.clone();
                                        let param_ix = sig
                                            .active_parameter
                                            .or(help.active_parameter)
                                            .unwrap_or(0)
                                            as usize;
                                        if let Some(param) =
                                            sig.parameters.get(param_ix)
                                        {
                                            contents.push_str("\n▸ ");
                                            contents.push_str(&param.label);
                                            if let Some(doc) = param
                                                .documentation
                                                .as_deref()
                                                .and_then(|doc| doc.lines().next())
                                            {
                                                contents.push_str(" — ");
                                                contents.push_str(doc);
                                            }
                                        }
                                        if let Some(doc) = sig
                                            .documentation
                                            .as_deref()
                                            .and_then(|doc| doc.lines().next())
                                        {
                                            contents.push('\n');
                                            contents.push_str(doc);
                                        }
                                        Some(engine::LspHover {
                                            path: String::new(),
                                            contents,
                                            kind: Some("plaintext".to_string()),
                                            range: None,
                                            language: None,
                                        })
                                    })
                                    .into_iter()
                                    .collect();
                                if lsp_log() {
                                    eprintln!(
                                        "neoism::lsp signature result: seq={seq} present={} at {line}:{character}",
                                        !hovers.is_empty()
                                    );
                                }
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.hover = Some((*seq, file.clone(), hovers));
                                woke = true;
                            }
                            CodeLspJob::DocumentHighlight {
                                root,
                                file,
                                line,
                                character,
                                seq,
                            } => {
                                if newest_occurrences != Some(ix) {
                                    continue;
                                }
                                let highlights = engine::document_highlight(
                                    root, file, *line, *character,
                                );
                                // Engine query outputs are ONE-based
                                // (line + byte col); convert to the
                                // pane's zero-based space and keep only
                                // single-line spans.
                                let mut spans: Vec<(usize, usize, usize)> =
                                    highlights
                                        .into_iter()
                                        .filter_map(|highlight| {
                                            let range = highlight.range?;
                                            if range.start.line != range.end.line {
                                                return None;
                                            }
                                            let line0 = (range.start.line
                                                as usize)
                                                .checked_sub(1)?;
                                            let start = (range.start.character
                                                as usize)
                                                .saturating_sub(1);
                                            let end = (range.end.character
                                                as usize)
                                                .saturating_sub(1);
                                            (end > start)
                                                .then_some((line0, start, end))
                                        })
                                        .collect();
                                spans.sort_unstable();
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.occurrences =
                                    Some((*seq, file.clone(), spans));
                                woke = true;
                            }
                            CodeLspJob::Definition {
                                root,
                                file,
                                line,
                                character,
                                seq,
                            } => {
                                if newest_definition != Some(ix) {
                                    continue;
                                }
                                let locations =
                                    engine::definition(root, file, *line, *character);
                                if lsp_log() {
                                    eprintln!(
                                        "neoism::lsp code definition result: seq={seq} locations={} at {line}:{character}",
                                        locations.len()
                                    );
                                }
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.definition = Some((*seq, locations));
                                woke = true;
                            }
                            CodeLspJob::FormatThenSave {
                                root,
                                file,
                                revision,
                            } => {
                                let edits = engine::formatting(root, file);
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.format_save =
                                    Some((file.clone(), *revision, edits));
                                drop(results);
                                woke = true;
                            }
                            CodeLspJob::DocumentSymbols { root, file, seq } => {
                                if newest_symbols != Some(ix) {
                                    continue;
                                }
                                let symbols = engine::document_symbols(root, file);
                                fn flatten(
                                    out: &mut Vec<neoism_ui::panels::finder::SymbolRow>,
                                    nodes: &[engine::LspDocumentSymbol],
                                ) {
                                    for node in nodes {
                                        let pos = node
                                            .selection_range
                                            .as_ref()
                                            .or(node.range.as_ref())
                                            .map(|r| (r.start.line, r.start.character))
                                            .unwrap_or((0, 0));
                                        out.push(
                                            neoism_ui::panels::finder::SymbolRow {
                                                kind: node.kind.to_lowercase(),
                                                name: node.name.clone(),
                                                line: pos.0 + 1,
                                                column: pos.1,
                                            },
                                        );
                                        flatten(out, &node.children);
                                    }
                                }
                                let mut rows = Vec::new();
                                flatten(&mut rows, &symbols);
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.document_symbols = Some((*seq, rows));
                                drop(results);
                                woke = true;
                            }
                            CodeLspJob::CompletionCommand {
                                root,
                                file,
                                server_id,
                                command,
                            } => {
                                let _ = engine::execute_completion_command(
                                    root,
                                    file,
                                    server_id,
                                    command.clone(),
                                );
                            }
                            CodeLspJob::CodeActions {
                                root,
                                file,
                                line,
                                character,
                                seq,
                            } => {
                                if newest_actions != Some(ix) {
                                    continue;
                                }
                                let groups =
                                    engine::code_actions(root, file, *line, *character);
                                let items = flatten_code_actions(&groups);
                                if lsp_log() {
                                    eprintln!(
                                        "neoism::lsp code actions result: seq={seq} actions={} at {line}:{character}",
                                        items.len()
                                    );
                                }
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.code_actions =
                                    Some((*seq, file.clone(), items));
                                woke = true;
                            }
                            CodeLspJob::ApplyCodeAction {
                                root,
                                file,
                                server_id,
                                title,
                                action,
                            } => {
                                // Bare Command actions (LSP `Command`,
                                // `command` is a string) go straight to
                                // workspace/executeCommand.
                                let is_bare_command = action
                                    .get("command")
                                    .is_some_and(|c| c.is_string());
                                let (edit, ran_command) = if is_bare_command {
                                    let _ = engine::execute_command(
                                        root,
                                        file,
                                        server_id,
                                        action.clone(),
                                    );
                                    (None, true)
                                } else {
                                    let mut action = action.clone();
                                    // No edit inline → codeAction/resolve
                                    // fills it in (rust-analyzer style).
                                    if action.get("edit").is_none() {
                                        if let Some(resolved) =
                                            engine::resolve_code_action(
                                                root,
                                                file,
                                                server_id,
                                                action.clone(),
                                            )
                                        {
                                            action = resolved;
                                        }
                                    }
                                    let edit = action
                                        .get("edit")
                                        .filter(|edit| !edit.is_null())
                                        .cloned();
                                    let ran_command = match action.get("command") {
                                        Some(command) if !command.is_null() => {
                                            let _ = engine::execute_command(
                                                root,
                                                file,
                                                server_id,
                                                command.clone(),
                                            );
                                            true
                                        }
                                        _ => false,
                                    };
                                    (edit, ran_command)
                                };
                                if lsp_log() {
                                    eprintln!(
                                        "neoism::lsp code action apply: title={title:?} edit={} command={ran_command}",
                                        edit.is_some()
                                    );
                                }
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.action_applied =
                                    Some((title.clone(), edit, ran_command));
                                woke = true;
                            }
                            CodeLspJob::References {
                                root,
                                file,
                                line,
                                character,
                                seq,
                            } => {
                                if newest_references != Some(ix) {
                                    continue;
                                }
                                let locations =
                                    engine::references(root, file, *line, *character);
                                if lsp_log() {
                                    eprintln!(
                                        "neoism::lsp code references result: seq={seq} locations={} at {line}:{character}",
                                        locations.len()
                                    );
                                }
                                // Build ready-made finder rows: read each
                                // hit's line text (per-file cache), path
                                // relative to the workspace root. Disk
                                // text can trail an unsaved buffer —
                                // cosmetic only, the jump target is exact.
                                let mut file_lines: HashMap<PathBuf, Vec<String>> =
                                    HashMap::new();
                                let mut rows: Vec<
                                    neoism_ui::panels::finder::ReferenceRow,
                                > = Vec::new();
                                for location in &locations {
                                    let path = PathBuf::from(&location.path);
                                    let (line0, col) = location
                                        .range
                                        .as_ref()
                                        .map(|range| {
                                            (
                                                range.start.line as usize,
                                                range.start.character,
                                            )
                                        })
                                        .unwrap_or((0, 0));
                                    let text = file_lines
                                        .entry(path.clone())
                                        .or_insert_with(|| {
                                            std::fs::read_to_string(&path)
                                                .map(|text| {
                                                    text.lines()
                                                        .map(str::to_string)
                                                        .collect()
                                                })
                                                .unwrap_or_default()
                                        })
                                        .get(line0)
                                        .cloned()
                                        .unwrap_or_default();
                                    let rel = path
                                        .strip_prefix(root)
                                        .unwrap_or(&path)
                                        .display()
                                        .to_string();
                                    rows.push(
                                        neoism_ui::panels::finder::ReferenceRow {
                                            path: rel,
                                            line: (line0 + 1) as u32,
                                            column: col,
                                            text: text.trim().to_string(),
                                        },
                                    );
                                }
                                rows.sort_by(|a, b| {
                                    a.path
                                        .cmp(&b.path)
                                        .then(a.line.cmp(&b.line))
                                        .then(a.column.cmp(&b.column))
                                });
                                rows.dedup_by(|a, b| {
                                    a.path == b.path
                                        && a.line == b.line
                                        && a.column == b.column
                                });
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.references =
                                    Some((*seq, root.clone(), rows));
                                woke = true;
                            }
                            CodeLspJob::Rename {
                                root,
                                file,
                                line,
                                character,
                                new_name,
                                seq,
                            } => {
                                if newest_rename != Some(ix) {
                                    continue;
                                }
                                let groups = engine::rename(
                                    root, file, *line, *character, new_name,
                                );
                                if lsp_log() {
                                    eprintln!(
                                        "neoism::lsp code rename result: seq={seq} groups={} at {line}:{character}",
                                        groups.len()
                                    );
                                }
                                let mut results = match results_store().lock() {
                                    Ok(results) => results,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                results.rename = Some((*seq, groups));
                                woke = true;
                            }
                        }
                    }
                    if woke {
                        query_proxy.send_event(
                            RioEventType::Rio(RioEvent::Render),
                            window_id,
                        );
                    }
                }
            });
        let _ = std::thread::Builder::new()
            .name("code-lsp-diags".into())
            .spawn(move || {
                let mut rx = engine::subscribe_diagnostics();
                loop {
                    match rx.blocking_recv() {
                        Ok(event) => {
                            if lsp_log() {
                                eprintln!(
                                    "neoism::lsp diag event: file={} n={}",
                                    event.file,
                                    event.diagnostics.len()
                                );
                            }
                            let file = canonical_key(Path::new(&event.file));
                            {
                                let mut store = match diag_store().lock() {
                                    Ok(store) => store,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                store
                                    .entry(file.clone())
                                    .or_default()
                                    .insert(event.server_id.clone(), event.diagnostics);
                            }
                            {
                                let mut seq = match diag_publish_seq().lock() {
                                    Ok(seq) => seq,
                                    Err(poisoned) => poisoned.into_inner(),
                                };
                                *seq.entry(file).or_insert(0) += 1;
                            }
                            DIAG_VERSION.fetch_add(1, Ordering::SeqCst);
                            proxy.send_event(
                                RioEventType::Rio(RioEvent::Render),
                                window_id,
                            );
                        }
                        // The bus is a process-global static that never
                        // closes; the only error is Lagged — skip ahead.
                        Err(_) => continue,
                    }
                }
            });
        CodeLspShared { jobs: tx }
    })
}

impl Screen<'_> {
    fn code_lsp_shared(&self) -> &'static CodeLspShared {
        let proxy = self.context_manager.event_proxy_clone();
        let window_id = self.context_manager.window_id();
        ensure_workers(proxy, window_id)
    }

    /// `(workspace root, file)` for the focused code pane.
    fn code_lsp_target(&self) -> Option<(PathBuf, PathBuf)> {
        let root = self.active_pane_workspace_root();
        let code = self.context_manager.current().code.as_ref()?;
        let file = code.path.clone();
        let root = root.or_else(|| file.parent().map(Path::to_path_buf))?;
        Some((root, file))
    }

    /// Per-frame LSP pump for the focused code pane: ship buffer edits
    /// to the engine, fold fresh diagnostics into the pane, and drain
    /// completion/hover/definition results into the UI session state.
    pub(crate) fn pump_code_lsp(&mut self) {
        if self.context_manager.current().code.is_none() {
            // Focus left the code pane — no stale popups may linger.
            let ui = &mut self.renderer.code_lsp;
            if ui.has_session_state() {
                *ui = Default::default();
            }
            return;
        }
        let proxy = self.context_manager.event_proxy_clone();
        let git_proxy = self.context_manager.event_proxy_clone();
        let window_id = self.context_manager.window_id();
        let shared = ensure_workers(proxy, window_id);
        let global_version = DIAG_VERSION.load(Ordering::SeqCst);
        let root = self.active_pane_workspace_root();
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return;
        };
        let file = code.path.clone();
        let Some(root) = root.or_else(|| file.parent().map(Path::to_path_buf)) else {
            return;
        };
        // The pane's live CRDT binding (None → not doc-bound; the
        // line-shift heuristic serves those panes instead). Disjoint
        // field of `self` from the `code` borrow above.
        let diag_binding = self
            .code_crdt
            .binding_for(&crate::screen::markdown_crdt::buffer_id_for_markdown_path(
                &file,
            ))
            .filter(|binding| binding.is_seeded());

        if code.lsp_synced_revision != Some(code.buffer.revision) {
            code.lsp_synced_revision = Some(code.buffer.revision);
            let _ = shared.jobs.send(CodeLspJob::Sync {
                root: root.clone(),
                file: file.clone(),
                text: code.buffer.text(),
            });
        }

        // Per-revision pass: git gutter marks + diagnostic anchor-lite.
        // Gated by its own key so it also runs on the FIRST frame of a
        // freshly opened pane (revision 0).
        {
            static GIT_PASS_REV: OnceLock<Mutex<HashMap<PathBuf, u64>>> =
                OnceLock::new();
            let revision = code.buffer.revision;
            let needs_pass = {
                let mut map = match GIT_PASS_REV.get_or_init(Default::default).lock()
                {
                    Ok(map) => map,
                    Err(poisoned) => poisoned.into_inner(),
                };
                map.insert(file.clone(), revision) != Some(revision)
            };
            // 512KB gate matches the highlight/outline cutoffs.
            let small_enough = code
                .buffer
                .lines
                .iter()
                .map(|line| line.len() + 1)
                .sum::<usize>()
                <= 512 * 1024;
            if needs_pass && small_enough {
                // The heuristic keeps the RAW store's line numbers
                // roughly right (status counts, problems popup and
                // diagnostic cards read it) — the publish gate below
                // stops its version bump from ever overwriting the
                // anchored fold.
                reanchor_diagnostics(&file, &code.buffer.lines);
                if let Some(binding) = diag_binding {
                    // Doc-bound: char-precise squiggles by re-resolving
                    // sticky anchors against the live CRDT doc.
                    let folded = fold_anchored_diagnostics(code, binding);
                    code.diagnostics = folded;
                }
                match git_baseline(&file, &git_proxy, window_id) {
                    Some(baseline) => {
                        code.git_marks = neoism_ui::editor::code::gitdiff::compute_git_marks(
                            &baseline,
                            &code.buffer.lines,
                        );
                    }
                    None => {
                        if !code.git_marks.is_empty() {
                            code.git_marks = Default::default();
                        }
                    }
                }
            }
        }

        let fold_from_store = code.lsp_diag_version != global_version && {
            code.lsp_diag_version = global_version;
            // Publish gate for the sticky-anchor path: the global
            // version also moves on other files' publishes and on the
            // line-shift heuristic; refolding from the store then
            // would overwrite anchor-precise spans with stale
            // positions. A doc-bound pane refolds (and re-pins its
            // anchors) only when a server actually published for THIS
            // file.
            let publish_seq = {
                let seq = match diag_publish_seq().lock() {
                    Ok(seq) => seq,
                    Err(poisoned) => poisoned.into_inner(),
                };
                seq.get(&canonical_key(&file)).copied().unwrap_or(0)
            };
            let fresh_publish = publish_seq != code.lsp_diag_publish_seq;
            code.lsp_diag_publish_seq = publish_seq;
            diag_binding.is_none() || fresh_publish
        };
        if fold_from_store {
            let store = match diag_store().lock() {
                Ok(store) => store,
                Err(poisoned) => poisoned.into_inner(),
            };
            let canonical = canonical_key(&file);
            if lsp_log() {
                eprintln!(
                    "neoism::lsp diag fold: pane={} canonical={} store_keys={:?}",
                    file.display(),
                    canonical.display(),
                    store.keys().collect::<Vec<_>>()
                );
            }
            let mut per_line: HashMap<usize, Vec<CodeLineDiagnostic>> = HashMap::new();
            let mut anchors: Vec<neoism_ui::editor::code::CodeDiagAnchor> = Vec::new();
            if let Some(by_server) = store.get(&canonical) {
                for diags in by_server.values() {
                    for diag in diags {
                        let Some(range) = diag.range.as_ref() else {
                            continue;
                        };
                        let severity = map_severity(&diag.severity);
                        let start_line = range.start.line as usize;
                        let end_line = (range.end.line as usize).max(start_line);
                        // Pin the published range into the CRDT doc
                        // while the pane is bound: the squiggle then
                        // tracks edits char-precisely until the next
                        // publish (start follows its character, end
                        // stays put on inserts at the range edge).
                        if let Some(binding) = diag_binding {
                            if let (Some(start), Some(end)) = (
                                binding.sticky_anchor_at_utf16(
                                    start_line,
                                    range.start.character as usize,
                                    true,
                                ),
                                binding.sticky_anchor_at_utf16(
                                    end_line,
                                    range.end.character as usize,
                                    false,
                                ),
                            ) {
                                anchors.push(neoism_ui::editor::code::CodeDiagAnchor {
                                    start,
                                    end,
                                    severity,
                                    message: diag.message.clone(),
                                });
                            }
                        }
                        for line_ix in start_line..=end_line {
                            let Some(line) = code.buffer.lines.get(line_ix) else {
                                break;
                            };
                            let mut from = if line_ix == start_line {
                                byte_for_utf16_col(line, range.start.character as usize)
                            } else {
                                0
                            };
                            let mut to = if line_ix == end_line {
                                byte_for_utf16_col(line, range.end.character as usize)
                            } else {
                                line.len()
                            };
                            // Zero-width / end-of-line ranges still get
                            // a visible one-cell underline.
                            if to <= from {
                                if from >= line.len() && !line.is_empty() {
                                    from = line.len() - 1;
                                }
                                to = (from + 1).min(line.len());
                            }
                            if from < to {
                                per_line.entry(line_ix).or_default().push(
                                    CodeLineDiagnostic {
                                        start: from,
                                        end: to,
                                        severity,
                                        message: if line_ix == start_line {
                                            diag.message.clone()
                                        } else {
                                            String::new()
                                        },
                                    },
                                );
                            }
                        }
                    }
                }
            }
            code.diag_anchors = anchors;
            code.diagnostics = per_line;
        }

        // Caret-idle occurrence probe (documentHighlight): once the
        // caret rests at a new position for a beat, ask for the
        // symbol's occurrences; any motion or edit re-arms the probe
        // and drops the previous bands.
        const OCCURRENCE_IDLE_SECS: f32 = 0.35;
        {
            let snapshot = self.context_manager.current().code.as_ref().map(|code| {
                (
                    code.buffer.cursor_line,
                    code.buffer.cursor_col,
                    code.buffer.revision,
                    code.occurrence_spans.is_empty(),
                )
            });
            if let Some((line, col, revision, spans_empty)) = snapshot {
                let ui = &mut self.renderer.code_lsp;
                let moved = ui.occurrence_probe.as_ref().map_or(true, |probe| {
                    probe.line != line
                        || probe.col != col
                        || probe.revision != revision
                });
                if moved {
                    ui.occurrence_probe = Some(CodeOccurrenceProbe {
                        line,
                        col,
                        revision,
                        since: std::time::Instant::now(),
                        requested: false,
                        seq: 0,
                    });
                    if !spans_empty {
                        if let Some(code) =
                            self.context_manager.current_mut().code.as_mut()
                        {
                            code.occurrence_spans.clear();
                        }
                        self.mark_dirty();
                    }
                } else {
                    let due = ui.occurrence_probe.as_ref().is_some_and(|probe| {
                        !probe.requested
                            && probe.since.elapsed().as_secs_f32()
                                >= OCCURRENCE_IDLE_SECS
                    });
                    if due {
                        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
                        if let Some(probe) = ui.occurrence_probe.as_mut() {
                            probe.requested = true;
                            probe.seq = seq;
                        }
                        let _ = shared.jobs.send(CodeLspJob::DocumentHighlight {
                            root: root.clone(),
                            file: file.clone(),
                            line: line as u32,
                            character: col as u32,
                            seq,
                        });
                    }
                }
            }
        }

        // Pointer-idle hover: request once the pointer has rested long
        // enough on one cell (armed by `note_code_mouse_hover`).
        const MOUSE_HOVER_DELAY_SECS: f32 = 0.4;
        let matured = self
            .renderer
            .code_lsp
            .mouse_hover
            .as_ref()
            .is_some_and(|c| {
                !c.requested && c.since.elapsed().as_secs_f32() >= MOUSE_HOVER_DELAY_SECS
            });
        if matured {
            let (line, col) = {
                let cand = self.renderer.code_lsp.mouse_hover.as_mut().unwrap();
                cand.requested = true;
                (cand.line, cand.col)
            };
            let over_diagnostic = self
                .context_manager
                .current()
                .code
                .as_ref()
                .and_then(|code| code.diagnostics.get(&line))
                .is_some_and(|spans| {
                    spans
                        .iter()
                        .any(|d| col >= d.start && col < d.end && !d.message.is_empty())
                });
            if over_diagnostic {
                // The diagnostic is what the pointer is asking about —
                // show it instantly (local), like VS Code's merged
                // hover with the problem on top.
                self.show_code_diagnostic_card_at(line, col);
            } else {
                self.request_code_hover_at(line, col, true);
            }
        }

        self.drain_code_lsp_results();
    }

    /// Fold worker results into the UI state and enforce position-based
    /// dismissal (cursor left the hover/anchor position).
    fn drain_code_lsp_results(&mut self) {
        let Some((path, cursor, line_text)) =
            self.context_manager.current().code.as_ref().map(|code| {
                (
                    code.path.clone(),
                    code.buffer.cursor(),
                    code.buffer
                        .lines
                        .get(code.buffer.cursor_line)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
        else {
            return;
        };

        // Position-based dismissal (belt and braces on top of the input
        // hooks): hover pins to the exact request position, completion
        // to its line + anchor.
        let ui = &mut self.renderer.code_lsp;
        if let Some(card) = ui.hover.as_ref() {
            if card.path != path
                || (!card.from_mouse
                    && (card.line != cursor.line || card.col != cursor.col))
            {
                // A dismissed signature card ends its session too.
                if ui.signature_seq == Some(card.seq) {
                    ui.signature_seq = None;
                }
                ui.hover = None;
            }
        }
        if let Some(session) = ui.completion.as_ref() {
            if session.path != path
                || session.line != cursor.line
                || cursor.col < session.anchor_col
            {
                ui.completion = None;
            }
        }
        // Action menu pins to the exact request position, like hover.
        if let Some(session) = ui.actions.as_ref() {
            if session.path != path
                || session.line != cursor.line
                || session.col != cursor.col
            {
                ui.actions = None;
            }
        }

        let (
            completion,
            hover,
            definition,
            format_save,
            code_actions,
            action_applied,
            references,
            rename,
            document_symbols,
            occurrences,
        ) = {
            let mut results = match results_store().lock() {
                Ok(results) => results,
                Err(poisoned) => poisoned.into_inner(),
            };
            (
                results.completion.take(),
                results.hover.take(),
                results.definition.take(),
                results.format_save.take(),
                results.code_actions.take(),
                results.action_applied.take(),
                results.references.take(),
                results.rename.take(),
                results.document_symbols.take(),
                results.occurrences.take(),
            )
        };

        // Symbol occurrences: install into the pane when the probe that
        // requested them is still current (stale seqs just drop).
        if let Some((seq, file, spans)) = occurrences {
            let probe_current = self
                .renderer
                .code_lsp
                .occurrence_probe
                .as_ref()
                .is_some_and(|probe| probe.seq == seq);
            if probe_current && file == path {
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    let mut per_line: HashMap<usize, Vec<(usize, usize)>> =
                        HashMap::new();
                    for (line, start, end) in spans {
                        per_line.entry(line).or_default().push((start, end));
                    }
                    code.occurrence_spans = per_line;
                }
                self.mark_dirty();
            }
        }

        // Symbols-mode rows: install while the finder is showing them
        // (stale seqs and closed finders just drop the result).
        if let Some((_seq, rows)) = document_symbols {
            if self.renderer.finder.is_enabled()
                && self.renderer.finder.mode()
                    == neoism_ui::panels::finder::FinderMode::Symbols
            {
                self.renderer.finder.set_symbol_rows(rows);
                self.mark_dirty();
            }
        }

        // Format-on-save landing: apply the edits only if the buffer
        // hasn't moved since the format ran, then finish the save.
        if let Some((edit_file, revision, edits)) = format_save {
            if edit_file == path {
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    if code.buffer.revision == revision && !edits.is_empty() {
                        let parsed = parse_lsp_text_edits(&edits);
                        code.buffer.apply_text_edits(&parsed);
                    }
                }
                // Doc-bound panes finish through the daemon: the save
                // flushes the just-applied format edits into the CRDT
                // (peers converge on the formatted text) and the daemon
                // writes the doc. Unbound panes write locally as before.
                if self.save_current_code_via_daemon() {
                    self.mark_dirty();
                } else {
                    self.finish_code_save();
                }
            }
        }

        if let Some((seq, file, mut items)) = completion {
            let ui = &mut self.renderer.code_lsp;
            if let Some(session) = ui.completion.as_mut() {
                if session.seq == seq && session.path == file {
                    items.sort_by(|a, b| {
                        let ka = a.sort_text.as_deref().unwrap_or(&a.label);
                        let kb = b.sort_text.as_deref().unwrap_or(&b.label);
                        ka.cmp(kb).then_with(|| a.label.cmp(&b.label))
                    });
                    session.items = items;
                    let installed = match completion_prefix(
                        &line_text,
                        session.anchor_col,
                        cursor.col,
                    ) {
                        Some(prefix) => {
                            rebuild_completion_filter(session, &prefix);
                            !session.filtered.is_empty()
                        }
                        None => false,
                    };
                    if !installed {
                        ui.completion = None;
                    }
                    self.mark_dirty();
                }
            }
        }

        if let Some((seq, file, hovers)) = hover {
            let ui = &mut self.renderer.code_lsp;
            if let Some(card) = ui.hover.as_mut() {
                if card.seq == seq && card.path == file {
                    let lines = hover_card_lines(&hovers);
                    if lines.is_empty() {
                        ui.hover = None;
                    } else {
                        card.lines = lines;
                    }
                    self.mark_dirty();
                }
            }
        }

        if let Some((seq, locations)) = definition {
            if self.renderer.code_lsp.definition_seq == Some(seq) {
                self.renderer.code_lsp.definition_seq = None;
                match locations.into_iter().next() {
                    Some(location) => self.jump_to_code_location(location),
                    None => self.renderer.notifications.push(
                        "No definition found",
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    ),
                }
            }
        }

        if let Some((seq, file, items)) = code_actions {
            let ui = &mut self.renderer.code_lsp;
            if let Some(session) = ui.actions.as_mut() {
                if session.seq == seq && session.path == file {
                    if items.is_empty() {
                        ui.actions = None;
                        self.renderer.notifications.push(
                            "No code actions",
                            neoism_ui::panels::notifications::NotificationLevel::Info,
                        );
                    } else {
                        session.items = items;
                        session.selected = 0;
                        session.display = build_action_popup(session);
                    }
                    self.mark_dirty();
                }
            }
        }

        if let Some((title, edit, ran_command)) = action_applied {
            match edit {
                Some(edit) => {
                    let per_file = workspace_edit_file_edits(&edit);
                    let touched = self.apply_code_workspace_edit(per_file);
                    let message = if touched > 1 {
                        format!("{title} — edited {touched} files")
                    } else {
                        title
                    };
                    self.renderer.notifications.push(
                        message,
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    );
                }
                None if ran_command => {
                    // Command-only action: the server does the work (and
                    // may push edits through its own channels).
                    self.renderer.notifications.push(
                        title,
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    );
                }
                None => {
                    self.renderer.notifications.push(
                        format!("{title}: no edit returned"),
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                }
            }
            self.mark_dirty();
        }

        if let Some((seq, root, rows)) = references {
            if self.renderer.code_lsp.references_seq == Some(seq) {
                self.renderer.code_lsp.references_seq = None;
                if rows.is_empty() {
                    self.renderer.notifications.push(
                        "No references found",
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    );
                } else {
                    self.finder_target_route = None;
                    self.renderer.file_tree.set_focused(false);
                    self.renderer.finder.open_references(root, rows);
                }
                self.mark_dirty();
            }
        }

        if let Some((seq, groups)) = rename {
            if self.renderer.code_lsp.rename_seq == Some(seq) {
                self.renderer.code_lsp.rename_seq = None;
                // Per-server groups; the first with a real edit wins so
                // a multi-server file can't double-apply.
                let edit = groups.iter().find_map(|group| {
                    group.get("edit").filter(|edit| !edit.is_null()).cloned()
                });
                match edit {
                    Some(edit) => {
                        let per_file = workspace_edit_file_edits(&edit);
                        let touched = self.apply_code_workspace_edit(per_file);
                        let message = match touched {
                            0 => "Rename produced no edits".to_string(),
                            1 => "Renamed in 1 file".to_string(),
                            n => format!("Renamed in {n} files"),
                        };
                        self.renderer.notifications.push(
                            message,
                            neoism_ui::panels::notifications::NotificationLevel::Info,
                        );
                    }
                    None => self.renderer.notifications.push(
                        "No rename available here",
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    ),
                }
                self.mark_dirty();
            }
        }
    }

    /// Apply a WorkspaceEdit's per-file text edits: the focused pane
    /// and other open panes through their buffers (one undo step per
    /// file), unopened files by read-patch-write on disk (with a toast
    /// per file touched). Files are independent, so cross-file order
    /// doesn't matter; within a file `apply_text_edits` runs bottom-up.
    /// Returns how many files changed.
    fn apply_code_workspace_edit(
        &mut self,
        per_file: Vec<(PathBuf, Vec<serde_json::Value>)>,
    ) -> usize {
        let mut touched = 0usize;
        for (path, raw_edits) in per_file {
            let parsed = parse_lsp_text_edits(&raw_edits);
            if parsed.is_empty() {
                continue;
            }
            let canonical = canonical_key(&path);
            let is_current =
                self.context_manager
                    .current()
                    .code
                    .as_ref()
                    .is_some_and(|code| {
                        code.path == path || canonical_key(&code.path) == canonical
                    });
            if is_current {
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    code.buffer.apply_text_edits(&parsed);
                    code.buffer.follow_cursor = true;
                }
                self.sync_active_code_modified();
                touched += 1;
                continue;
            }
            let mut open_pane: Option<PathBuf> = None;
            if self.context_manager.code_pane_mut_by_path(&path).is_some() {
                open_pane = Some(path.clone());
            } else if canonical != path
                && self
                    .context_manager
                    .code_pane_mut_by_path(&canonical)
                    .is_some()
            {
                open_pane = Some(canonical.clone());
            }
            if let Some(pane_path) = open_pane {
                if let Some(pane) = self.context_manager.code_pane_mut_by_path(&pane_path)
                {
                    pane.buffer.apply_text_edits(&parsed);
                    let dirty = pane.is_dirty();
                    self.sync_markdown_tab_modified(&pane_path, dirty);
                    touched += 1;
                }
                continue;
            }
            match apply_edits_on_disk(&path, &parsed) {
                Ok(()) => {
                    touched += 1;
                    self.renderer.notifications.push(
                        format!("Edited {}", path.display()),
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    );
                }
                Err(err) => {
                    self.renderer.notifications.push(
                        format!("Failed to edit {}: {err}", path.display()),
                        neoism_ui::panels::notifications::NotificationLevel::Error,
                    );
                }
            }
        }
        if touched > 0 {
            self.mark_dirty();
        }
        touched
    }

    /// Land on a definition target: same file moves the cursor, another
    /// file opens (or refocuses) its code pane first.
    fn jump_to_code_location(&mut self, location: engine::LspLocation) {
        let target = PathBuf::from(&location.path);
        let (line, col) = location
            .range
            .as_ref()
            .map(|range| (range.start.line as usize, range.start.character as usize))
            .unwrap_or((0, 0));
        self.open_code_location(target, line, col);
    }

    /// Open `target` in a code pane and park the cursor at a 0-based
    /// `(line, byte col)` — the gd cross-file pattern, also used by the
    /// references finder on Enter.
    pub(crate) fn open_code_location(
        &mut self,
        target: PathBuf,
        line: usize,
        col: usize,
    ) {
        let same_file = self
            .context_manager
            .current()
            .code
            .as_ref()
            .is_some_and(|code| code.path == target);
        if !same_file {
            self.open_path_in_code(target.clone());
        }
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            if code.path == target {
                let line = line.min(code.buffer.lines.len().saturating_sub(1));
                code.buffer.set_cursor_position(line, col, false);
                code.buffer.follow_cursor = true;
            }
        }
        self.mark_dirty();
    }

    /// Finder Symbols mode: fetch the active file's document symbols
    /// on the worker. False when there's no LSP target.
    pub(crate) fn request_code_document_symbols(&mut self) -> bool {
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return false;
        };
        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        shared
            .jobs
            .send(CodeLspJob::DocumentSymbols { root, file, seq })
            .is_ok()
    }

    /// Status-pill feed for the active code pane: `(state, label)`
    /// from the worker-maintained cache (empty label → generic "LSP").
    pub(crate) fn code_lsp_pill(
        &self,
        file: &Path,
    ) -> Option<(neoism_ui::panels::status_line::LspStatus, Option<String>)> {
        let store = match lsp_pill_store().lock() {
            Ok(store) => store,
            Err(poisoned) => poisoned.into_inner(),
        };
        store.get(file).map(|(status, label, _)| {
            let label = (!label.is_empty()).then(|| label.clone());
            (*status, label)
        })
    }

    /// Click on a diagnostic span: show its message(s) as a hover-style
    /// card pinned to the span start. Non-consuming — the click also
    /// moved the cursor as usual.
    pub(crate) fn show_code_diagnostic_card_at(&mut self, line: usize, col: usize) {
        let Some((path, spans)) =
            self.context_manager.current().code.as_ref().map(|code| {
                (
                    code.path.clone(),
                    code.diagnostics.get(&line).cloned().unwrap_or_default(),
                )
            })
        else {
            return;
        };
        let mut hits: Vec<&CodeLineDiagnostic> = spans
            .iter()
            .filter(|d| col >= d.start && col < d.end)
            .collect();
        if hits.is_empty() {
            // Click landed past the text (the inline `■ message` zone,
            // clamped by hit_position to the line end): open the
            // line's strongest messaged diagnostic instead of nothing.
            let line_len = self
                .context_manager
                .current()
                .code
                .as_ref()
                .and_then(|code| code.buffer.lines.get(line).map(|l| l.len()))
                .unwrap_or(0);
            if col + 1 >= line_len {
                if let Some(strongest) = spans
                    .iter()
                    .filter(|d| !d.message.is_empty())
                    .max_by_key(|d| d.severity)
                {
                    hits.push(strongest);
                }
            }
        }
        if hits.is_empty() {
            return;
        }
        let mut lines: Vec<String> = Vec::new();
        for hit in &hits {
            if hit.message.is_empty() {
                continue;
            }
            for msg_line in hit.message.lines() {
                lines.push(msg_line.to_string());
            }
        }
        if lines.is_empty() {
            return;
        }
        let anchor_col = hits.iter().map(|d| d.start).min().unwrap_or(col);
        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        self.renderer.code_lsp.hover = Some(CodeHoverCard {
            path,
            line,
            col: anchor_col,
            seq,
            lines,
            from_mouse: true,
        });
        self.mark_dirty();
    }

    /// Status-bar error/warning pills: exact per-diagnostic counts for
    /// the file from the raw store (the pane's per-line span map would
    /// overcount multi-line diagnostics).
    pub(crate) fn code_diagnostic_counts(
        &self,
        file: &Path,
    ) -> neoism_ui::panels::status_line::DiagnosticCounts {
        let store = match diag_store().lock() {
            Ok(store) => store,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut counts = neoism_ui::panels::status_line::DiagnosticCounts::default();
        if let Some(by_server) = store.get(&canonical_key(file)) {
            for diags in by_server.values() {
                for diag in diags {
                    match map_severity(&diag.severity) {
                        CodeDiagnosticSeverity::Error => counts.error += 1,
                        CodeDiagnosticSeverity::Warn => counts.warn += 1,
                        CodeDiagnosticSeverity::Info => counts.info += 1,
                        CodeDiagnosticSeverity::Hint => counts.hint += 1,
                    }
                }
            }
        }
        counts
    }

    /// Rows for the status-bar diagnostics popup (error or warn pill):
    /// 1-based line + message per diagnostic of that severity class.
    pub(crate) fn code_diagnostic_popup_items(
        &self,
        file: &Path,
        pill: neoism_ui::panels::status_line::DiagnosticPill,
    ) -> Vec<neoism_ui::panels::diagnostics_popup::PopupItem> {
        use neoism_ui::panels::diagnostics_popup::{PopupItem, Severity};
        let store = match diag_store().lock() {
            Ok(store) => store,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut items = Vec::new();
        if let Some(by_server) = store.get(&canonical_key(file)) {
            for diags in by_server.values() {
                for diag in diags {
                    let severity = map_severity(&diag.severity);
                    let wanted = match pill {
                        neoism_ui::panels::status_line::DiagnosticPill::Error => {
                            severity == CodeDiagnosticSeverity::Error
                        }
                        neoism_ui::panels::status_line::DiagnosticPill::Warn => {
                            severity == CodeDiagnosticSeverity::Warn
                        }
                    };
                    if !wanted {
                        continue;
                    }
                    items.push(PopupItem {
                        lnum: diag
                            .range
                            .as_ref()
                            .map(|r| r.start.line as u64 + 1)
                            .unwrap_or(1),
                        severity: match severity {
                            CodeDiagnosticSeverity::Error => Severity::Error,
                            CodeDiagnosticSeverity::Warn => Severity::Warn,
                            CodeDiagnosticSeverity::Info => Severity::Info,
                            CodeDiagnosticSeverity::Hint => Severity::Hint,
                        },
                        message: diag.message.replace('\n', "  "),
                    });
                }
            }
        }
        items.sort_by_key(|item| item.lnum);
        items
    }

    /// Popup rows for the LSP status pill's "Server Details" card.
    pub(crate) fn code_lsp_server_rows(
        &self,
        file: &Path,
    ) -> Vec<neoism_ui::panels::lsp_popup::LspServerRow> {
        let store = match lsp_pill_store().lock() {
            Ok(store) => store,
            Err(poisoned) => poisoned.into_inner(),
        };
        store
            .get(file)
            .map(|(_, _, rows)| rows.clone())
            .unwrap_or_default()
    }

    /// Format-on-save entry: enqueue formatter + deferred save. False
    /// when there's no LSP target (caller saves directly).
    pub(crate) fn queue_code_format_then_save(&mut self) -> bool {
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return false;
        };
        let Some(revision) = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| code.buffer.revision)
        else {
            return false;
        };
        shared
            .jobs
            .send(CodeLspJob::FormatThenSave {
                root,
                file,
                revision,
            })
            .is_ok()
    }

    /// Notify the engine after a successful save (didSave triggers
    /// slow-lane checks like `cargo check` on rust-analyzer).
    pub(crate) fn notify_code_lsp_saved(&mut self, file: &Path) {
        let Some(shared) = CODE_LSP.get() else {
            return;
        };
        let root = self
            .active_pane_workspace_root()
            .or_else(|| file.parent().map(Path::to_path_buf));
        if let Some(root) = root {
            let _ = shared.jobs.send(CodeLspJob::Save {
                root,
                file: file.to_path_buf(),
            });
        }
    }

    // -----------------------------------------------------------------
    // Requests (input paths call these; results land via the pump).
    // -----------------------------------------------------------------

    /// Open/refresh a completion session at the cursor. `trigger` is a
    /// server trigger character just typed (anchors the replace range
    /// at the cursor, empty prefix); `None` anchors at the identifier
    /// start under the cursor.
    pub(crate) fn request_code_completion(&mut self, trigger: Option<String>) {
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return;
        };
        let Some(code) = self.context_manager.current().code.as_ref() else {
            return;
        };
        let cursor = code.buffer.cursor();
        let line_text = code
            .buffer
            .lines
            .get(cursor.line)
            .cloned()
            .unwrap_or_default();
        let anchor_col = if trigger.is_some() {
            cursor.col
        } else {
            word_start_col(&line_text, cursor.col)
        };
        let text = code.buffer.text();
        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        if lsp_log() {
            eprintln!(
                "neoism::lsp code completion request: seq={seq} at {}:{} trigger={trigger:?}",
                cursor.line, cursor.col
            );
        }
        let ui = &mut self.renderer.code_lsp;
        match ui.completion.as_mut() {
            // Same anchor: keep the visible menu while the re-query is
            // in flight, just retoken it so only the newest installs.
            Some(session)
                if session.path == file
                    && session.line == cursor.line
                    && session.anchor_col == anchor_col =>
            {
                session.seq = seq;
            }
            _ => {
                ui.completion = Some(CodeCompletionSession {
                    path: file.clone(),
                    line: cursor.line,
                    anchor_col,
                    id: seq,
                    seq,
                    items: Vec::new(),
                    filtered: Vec::new(),
                    selected: 0,
                    display: PopupMenu::default(),
                });
            }
        }
        let _ = shared.jobs.send(CodeLspJob::Completion {
            root,
            file,
            text,
            line: cursor.line as u32,
            character: cursor.col as u32,
            trigger,
            seq,
        });
    }

    /// Request hover docs at the cursor (Ctrl+K / vim `K`).
    pub(crate) fn request_code_hover(&mut self) {
        let Some(cursor) = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| code.buffer.cursor())
        else {
            return;
        };
        self.request_code_hover_at(cursor.line, cursor.col, false);
    }

    pub(crate) fn request_code_hover_at(
        &mut self,
        line: usize,
        col: usize,
        from_mouse: bool,
    ) {
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return;
        };
        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        if lsp_log() {
            eprintln!("neoism::lsp code hover request: seq={seq} at {line}:{col}");
        }
        self.renderer.code_lsp.completion = None;
        self.renderer.code_lsp.hover = Some(CodeHoverCard {
            path: file.clone(),
            line,
            col,
            seq,
            lines: Vec::new(),
            from_mouse,
        });
        let _ = shared.jobs.send(CodeLspJob::Hover {
            root,
            file,
            line: line as u32,
            character: col as u32,
            seq,
        });
    }

    /// Signature help at the caret: install/refresh the card (the
    /// result rides the hover mailbox as a synthetic hover). Carries
    /// the previous card's lines while retriggering so the popup never
    /// flickers empty between keystrokes.
    pub(crate) fn request_code_signature_help(&mut self) {
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return;
        };
        let Some((line, col)) = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| (code.buffer.cursor_line, code.buffer.cursor_col))
        else {
            return;
        };
        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        let ui = &mut self.renderer.code_lsp;
        let carried_lines = match (ui.signature_seq, ui.hover.take()) {
            (Some(previous), Some(card)) if card.seq == previous => card.lines,
            (_, other) => {
                ui.hover = other;
                Vec::new()
            }
        };
        ui.signature_seq = Some(seq);
        ui.hover = Some(CodeHoverCard {
            path: file.clone(),
            line,
            col,
            seq,
            lines: carried_lines,
            from_mouse: false,
        });
        let _ = shared.jobs.send(CodeLspJob::SignatureHelp {
            root,
            file,
            line: line as u32,
            character: col as u32,
            seq,
        });
    }

    /// End the signature-help session (typed `)`, committed the line,
    /// or moved away), dismissing its card if still up.
    fn end_code_signature_help(&mut self) {
        let ui = &mut self.renderer.code_lsp;
        if let Some(seq) = ui.signature_seq.take() {
            if ui.hover.as_ref().is_some_and(|card| card.seq == seq) {
                ui.hover = None;
            }
        }
    }

    /// Palette "Project Problems": every diagnostic in the store, all
    /// files, as finder References rows (path:line + severity-tagged
    /// message) — Enter/preview jump straight to the problem.
    pub(crate) fn open_project_problems(&mut self) {
        let Some(cwd) = self
            .active_pane_workspace_root()
            .or_else(|| self.active_workspace_root.clone())
        else {
            return;
        };
        let mut rows: Vec<neoism_ui::panels::finder::ReferenceRow> = Vec::new();
        {
            let store = match diag_store().lock() {
                Ok(store) => store,
                Err(poisoned) => poisoned.into_inner(),
            };
            for (path, by_server) in store.iter() {
                let display = path
                    .strip_prefix(&cwd)
                    .unwrap_or(path.as_path())
                    .to_string_lossy()
                    .into_owned();
                for diags in by_server.values() {
                    for diag in diags {
                        let Some(range) = diag.range.as_ref() else {
                            continue;
                        };
                        let mut message =
                            diag.message.lines().next().unwrap_or("").to_string();
                        if message.chars().count() > 160 {
                            message = message.chars().take(160).collect();
                        }
                        rows.push(neoism_ui::panels::finder::ReferenceRow {
                            path: display.clone(),
                            // Store ranges are raw wire (zero-based);
                            // rows are 1-based like references.
                            line: range.start.line + 1,
                            column: range.start.character,
                            text: format!("{}: {}", diag.severity, message),
                        });
                    }
                }
            }
        }
        if rows.is_empty() {
            self.renderer.notifications.push(
                "No problems reported",
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
            self.mark_dirty();
            return;
        }
        rows.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        self.finder_target_route = None;
        self.renderer.file_tree.set_focused(false);
        self.renderer.finder.open_references(cwd, rows);
        self.mark_dirty();
    }

    /// Pointer moved: arm/refresh the mouse-idle hover candidate (the
    /// pump requests once the pointer rests ~400ms on one cell).
    pub(crate) fn note_code_mouse_hover(&mut self) {
        let [mx, my] = self.markdown_mouse_logical();
        let Some(code) = self.context_manager.current().code.as_ref() else {
            if self.renderer.code_lsp.mouse_hover.take().is_some() {
                self.mark_dirty();
            }
            return;
        };
        let [gx, gy, gw, gh] = code.geometry.rect;
        let inside = gw > 0.0
            && mx >= code.geometry.text_x
            && mx <= gx + gw
            && my >= gy
            && my <= gy + gh;
        if !inside {
            self.renderer.code_lsp.mouse_hover = None;
            if self
                .renderer
                .code_lsp
                .hover
                .as_ref()
                .is_some_and(|card| card.from_mouse)
            {
                self.renderer.code_lsp.hover = None;
                self.mark_dirty();
            }
            return;
        }
        let (line, col) = code.geometry.hit_position(&code.buffer.lines, mx, my);
        let mut dismissed = false;
        {
            let ui = &mut self.renderer.code_lsp;
            let same_cell = ui
                .mouse_hover
                .as_ref()
                .is_some_and(|cand| cand.line == line && cand.col == col);
            if same_cell {
                return;
            }
            // Pointer moved to a new cell: any mouse card dies with it.
            if ui.hover.as_ref().is_some_and(|card| {
                card.from_mouse && (card.line != line || card.col != col)
            }) {
                ui.hover = None;
                dismissed = true;
            }
            ui.mouse_hover = Some(CodeMouseHover {
                line,
                col,
                since: std::time::Instant::now(),
                requested: false,
            });
        }
        if dismissed {
            self.mark_dirty();
        }
    }

    /// Request go-to-definition at an explicit buffer position
    /// (Ctrl+Click hit or the cursor for vim `gd`).
    pub(crate) fn request_code_definition_at(&mut self, line: usize, col: usize) {
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return;
        };
        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        if lsp_log() {
            eprintln!("neoism::lsp code definition request: seq={seq} at {line}:{col}");
        }
        self.renderer.code_lsp.definition_seq = Some(seq);
        let _ = shared.jobs.send(CodeLspJob::Definition {
            root,
            file,
            line: line as u32,
            character: col as u32,
            seq,
        });
    }

    pub(crate) fn dismiss_code_lsp_popups(&mut self) {
        let ui = &mut self.renderer.code_lsp;
        if ui.completion.is_some() || ui.hover.is_some() || ui.actions.is_some() {
            ui.dismiss_popups();
            self.mark_dirty();
        }
    }

    // -----------------------------------------------------------------
    // Code actions (`<Space>a` / Ctrl+.).
    // -----------------------------------------------------------------

    /// Request code actions at the cursor and open a (initially empty)
    /// menu session pinned there; the drain fills it or dismisses with
    /// a "No code actions" toast.
    pub(crate) fn request_code_actions(&mut self) {
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return;
        };
        let Some(cursor) = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| code.buffer.cursor())
        else {
            return;
        };
        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        if lsp_log() {
            eprintln!(
                "neoism::lsp code actions request: seq={seq} at {}:{}",
                cursor.line, cursor.col
            );
        }
        let ui = &mut self.renderer.code_lsp;
        ui.completion = None;
        ui.hover = None;
        ui.actions = Some(CodeActionSession {
            path: file.clone(),
            line: cursor.line,
            col: cursor.col,
            id: seq,
            seq,
            items: Vec::new(),
            selected: 0,
            display: PopupMenu::default(),
        });
        let _ = shared.jobs.send(CodeLspJob::CodeActions {
            root,
            file,
            line: cursor.line as u32,
            character: cursor.col as u32,
            seq,
        });
        self.mark_dirty();
    }

    /// Whether the code-action menu is visibly open (results arrived).
    pub(crate) fn code_action_menu_open(&self) -> bool {
        self.renderer
            .code_lsp
            .actions
            .as_ref()
            .is_some_and(|session| !session.display.items.is_empty())
    }

    pub(crate) fn move_code_action_selection(&mut self, delta: isize) {
        if let Some(session) = self.renderer.code_lsp.actions.as_mut() {
            let len = session.items.len();
            if len == 0 {
                return;
            }
            let current = session.selected as isize;
            let next = (current + delta).rem_euclid(len as isize) as usize;
            session.selected = next;
            session.display.selected = Some(next);
        }
        self.mark_dirty();
    }

    /// Enter on the action menu: hand the highlighted action to the
    /// worker (resolve → edit → execute) and close the menu. The edit
    /// lands back through the drain and is applied there.
    pub(crate) fn apply_selected_code_action(&mut self) -> bool {
        let Some(session) = self.renderer.code_lsp.actions.take() else {
            return false;
        };
        let Some(item) = session.items.get(session.selected).cloned() else {
            return false;
        };
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return false;
        };
        let _ = shared.jobs.send(CodeLspJob::ApplyCodeAction {
            root,
            file,
            server_id: item.server_id,
            title: item.title,
            action: item.action,
        });
        self.mark_dirty();
        true
    }

    // -----------------------------------------------------------------
    // References (vim `gr`).
    // -----------------------------------------------------------------

    /// Request find-references at the cursor; the drain opens the
    /// finder's References mode over the hits (or toasts when empty).
    pub(crate) fn request_code_references(&mut self) {
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return;
        };
        let Some(cursor) = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| code.buffer.cursor())
        else {
            return;
        };
        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        if lsp_log() {
            eprintln!(
                "neoism::lsp code references request: seq={seq} at {}:{}",
                cursor.line, cursor.col
            );
        }
        self.renderer.code_lsp.dismiss_popups();
        self.renderer.code_lsp.references_seq = Some(seq);
        let _ = shared.jobs.send(CodeLspJob::References {
            root,
            file,
            line: cursor.line as u32,
            character: cursor.col as u32,
            seq,
        });
    }

    // -----------------------------------------------------------------
    // Rename (`<Space>r` → modal → workspace edit).
    // -----------------------------------------------------------------

    /// Open the rename modal prefilled with the word under the cursor.
    /// The request position freezes now (the modal is blocking); the
    /// submit arm routes into `submit_code_rename`.
    pub(crate) fn open_code_rename_prompt(&mut self) {
        use neoism_ui::editor::markdown::vim::vim_word_under_cursor;
        use neoism_ui::widgets::modal::{ModalFormField, ModalFormSpec};
        let Some((path, cursor, line_text)) =
            self.context_manager.current().code.as_ref().map(|code| {
                (
                    code.path.clone(),
                    code.buffer.cursor(),
                    code.buffer
                        .lines
                        .get(code.buffer.cursor_line)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
        else {
            return;
        };
        let Some((start, end)) = vim_word_under_cursor(&line_text, cursor.col) else {
            self.renderer.notifications.push(
                "No symbol under cursor",
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
            return;
        };
        let word = line_text[start..end].to_string();
        self.renderer.code_lsp.dismiss_popups();
        self.renderer.code_lsp.pending_rename = Some(PendingCodeRename {
            path,
            line: cursor.line,
            col: cursor.col,
        });
        self.renderer.modal.open_form(ModalFormSpec {
            title: format!("Rename `{word}`"),
            fields: vec![ModalFormField {
                id: "code_rename_to".into(),
                label: "New name".into(),
                value: word,
                placeholder: "new_name".into(),
                secret: false,
            }],
            submit_label: "Rename".into(),
        });
        self.mark_dirty();
    }

    /// Modal submit: fire the rename request at the frozen position.
    pub(crate) fn submit_code_rename(&mut self, new_name: String) {
        let Some(pending) = self.renderer.code_lsp.pending_rename.take() else {
            return;
        };
        let new_name = new_name.trim().to_string();
        if new_name.is_empty() {
            return;
        }
        let shared = self.code_lsp_shared();
        let Some((root, file)) = self.code_lsp_target() else {
            return;
        };
        if file != pending.path {
            self.renderer.notifications.push(
                "Rename target changed — aborted",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }
        let seq = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        if lsp_log() {
            eprintln!(
                "neoism::lsp code rename request: seq={seq} at {}:{} -> {new_name:?}",
                pending.line, pending.col
            );
        }
        self.renderer.code_lsp.rename_seq = Some(seq);
        let _ = shared.jobs.send(CodeLspJob::Rename {
            root,
            file,
            line: pending.line as u32,
            character: pending.col as u32,
            new_name,
            seq,
        });
    }

    // -----------------------------------------------------------------
    // Completion menu interaction.
    // -----------------------------------------------------------------

    /// Whether the completion menu is visibly open on the focused pane.
    pub(crate) fn code_completion_menu_open(&self) -> bool {
        self.renderer
            .code_lsp
            .completion
            .as_ref()
            .is_some_and(|session| !session.display.items.is_empty())
    }

    pub(crate) fn move_code_completion_selection(&mut self, delta: isize) {
        if let Some(session) = self.renderer.code_lsp.completion.as_mut() {
            let len = session.filtered.len();
            if len == 0 {
                return;
            }
            let current = session.selected as isize;
            let next = (current + delta).rem_euclid(len as isize) as usize;
            session.selected = next;
            session.display.selected = Some(next);
        }
        self.mark_dirty();
    }

    /// Insert the highlighted completion, replacing the typed prefix
    /// (or the server's `textEdit` start when it names one on the same
    /// line). Runs the item's follow-up `command` when present.
    pub(crate) fn accept_code_completion(&mut self) -> bool {
        let Some(session) = self.renderer.code_lsp.completion.take() else {
            return false;
        };
        let Some(&item_ix) = session.filtered.get(session.selected) else {
            return false;
        };
        let Some(item) = session.items.get(item_ix) else {
            return false;
        };

        let edit_start = item
            .payload
            .get("textEdit")
            .and_then(|edit| edit.get("range").or_else(|| edit.get("insert")))
            .and_then(|range| range.get("start"))
            .and_then(|start| {
                let line = start.get("line")?.as_u64()? as usize;
                let character = start.get("character")?.as_u64()? as usize;
                (line == session.line).then_some(character)
            });
        let is_snippet = item
            .payload
            .get("insertTextFormat")
            .and_then(|format| format.as_u64())
            == Some(2);
        let (insert, first_stop) = if is_snippet {
            snippet_with_first_stop(&item.insert_text)
        } else {
            (item.insert_text.clone(), None)
        };
        let follow_up = item
            .server_id
            .clone()
            .zip(item.payload.get("command").cloned());
        // Auto-imports and friends: extra edits the server wants applied
        // alongside the accepted item (positions already at the engine's
        // byte-coordinate boundary, same contract as format-on-save).
        let additional_edits = item
            .payload
            .get("additionalTextEdits")
            .and_then(|edits| edits.as_array())
            .cloned()
            .unwrap_or_default();

        {
            let Some(code) = self.context_manager.current_mut().code.as_mut() else {
                return false;
            };
            if code.path != session.path || code.buffer.cursor_line != session.line {
                return false;
            }
            let cursor_col = code.buffer.cursor_col;
            let start = edit_start.unwrap_or(session.anchor_col).min(cursor_col);
            let start = if code
                .buffer
                .lines
                .get(session.line)
                .is_some_and(|line| line.is_char_boundary(start))
            {
                start
            } else {
                session.anchor_col.min(cursor_col)
            };
            if cursor_col > start {
                code.buffer.set_cursor_position(session.line, start, false);
                code.buffer
                    .set_cursor_position(session.line, cursor_col, true);
            }
            code.buffer.insert_text(&insert);
            code.buffer.follow_cursor = true;

            // Apply the additional edits AFTER the completion text:
            // auto-import inserts live above the caret and are
            // unaffected by the at-cursor insert; `apply_text_edits`
            // runs bottom-up so multiple edits stay position-stable.
            let mut import_line_shift: i64 = 0;
            if !additional_edits.is_empty() {
                let parsed = parse_lsp_text_edits(&additional_edits);
                if !parsed.is_empty() {
                    let cursor_line = code.buffer.cursor_line;
                    let cursor_col = code.buffer.cursor_col;
                    // Net line delta of edits strictly above the caret
                    // — an inserted `use` line must not leave the caret
                    // one line off.
                    let line_shift: i64 = parsed
                        .iter()
                        .filter(|edit| edit.end_line < cursor_line)
                        .map(|edit| {
                            edit.text.matches('\n').count() as i64
                                - (edit.end_line - edit.start_line) as i64
                        })
                        .sum();
                    code.buffer.apply_text_edits(&parsed);
                    let target = (cursor_line as i64 + line_shift).max(0) as usize;
                    code.buffer.set_cursor_position(target, cursor_col, false);
                    code.buffer.follow_cursor = true;
                    import_line_shift = line_shift;
                }
            }

            // Snippet: land the caret on the FIRST tabstop with its
            // placeholder selected, so typing replaces it. Runs after
            // the import edits (which may have shifted lines above).
            if let Some((offset, len)) = first_stop {
                let before = &insert[..offset.min(insert.len())];
                let line_delta = before.matches('\n').count();
                let stop_line = (session.line as i64
                    + line_delta as i64
                    + import_line_shift)
                    .max(0) as usize;
                let stop_col = if line_delta == 0 {
                    start + before.len()
                } else {
                    before.rsplit('\n').next().map(str::len).unwrap_or(0)
                };
                code.buffer.set_cursor_position(stop_line, stop_col, false);
                if len > 0 {
                    code.buffer.set_cursor_position(stop_line, stop_col + len, true);
                }
                code.buffer.follow_cursor = true;
            }
        }

        if let Some((server_id, command)) = follow_up {
            let shared = self.code_lsp_shared();
            if let Some((root, file)) = self.code_lsp_target() {
                let _ = shared.jobs.send(CodeLspJob::CompletionCommand {
                    root,
                    file,
                    server_id,
                    command,
                });
            }
        }

        self.sync_active_code_modified();
        self.mark_dirty();
        true
    }

    /// Refilter the open session against the typed prefix; dismisses
    /// when the prefix stops being an identifier or nothing matches.
    pub(crate) fn refilter_code_completion(&mut self) {
        let Some((path, cursor, line_text)) =
            self.context_manager.current().code.as_ref().map(|code| {
                (
                    code.path.clone(),
                    code.buffer.cursor(),
                    code.buffer
                        .lines
                        .get(code.buffer.cursor_line)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
        else {
            self.renderer.code_lsp.completion = None;
            return;
        };
        let ui = &mut self.renderer.code_lsp;
        let Some(session) = ui.completion.as_mut() else {
            return;
        };
        if session.path != path || session.line != cursor.line {
            ui.completion = None;
            self.mark_dirty();
            return;
        }
        let keep = match completion_prefix(&line_text, session.anchor_col, cursor.col) {
            Some(prefix) => {
                if session.items.is_empty() {
                    // Results still in flight; the pump filters at
                    // install time with the then-current prefix.
                    true
                } else {
                    rebuild_completion_filter(session, &prefix);
                    !session.filtered.is_empty()
                }
            }
            None => false,
        };
        if !keep {
            ui.completion = None;
        }
        self.mark_dirty();
    }

    /// Whether `c` is a completion trigger character for the focused
    /// file (server-advertised set, `DEFAULT_TRIGGERS` until fetched).
    pub(crate) fn code_completion_trigger(&self, c: char) -> Option<String> {
        let code = self.context_manager.current().code.as_ref()?;
        let store = match trigger_store().lock() {
            Ok(store) => store,
            Err(poisoned) => poisoned.into_inner(),
        };
        let matched = match store.get(&code.path) {
            Some(chars) if !chars.is_empty() => chars
                .iter()
                .any(|trigger| trigger.chars().eq(std::iter::once(c))),
            _ => DEFAULT_TRIGGERS
                .iter()
                .any(|trigger| trigger.chars().eq(std::iter::once(c))),
        };
        matched.then(|| c.to_string())
    }

    /// Post-edit hook from the standard key path (both input modes'
    /// insert sites funnel here): drives completion open/refilter/
    /// dismiss and clears the hover card on any edit.
    pub(crate) fn code_lsp_after_key(&mut self, edit: CodeKeyEdit) {
        // A live signature session survives keystrokes (retriggered at
        // the new caret below); ordinary hover cards dismiss on typing.
        let signature_active = self.renderer.code_lsp.signature_seq.is_some();
        if self.renderer.code_lsp.hover.is_some() && !signature_active {
            self.renderer.code_lsp.hover = None;
        }
        let session_open = self.renderer.code_lsp.completion.is_some();
        match edit {
            CodeKeyEdit::Char(c) => {
                // Signature help: `(`/`,` opens or refreshes; `)` ends
                // the session; anything else retriggers at the caret
                // while a session is live (the card follows the args).
                if c == '(' || c == ',' {
                    self.request_code_signature_help();
                } else if signature_active {
                    if c == ')' {
                        self.end_code_signature_help();
                    } else {
                        self.request_code_signature_help();
                    }
                }
                if let Some(trigger) = self.code_completion_trigger(c) {
                    self.request_code_completion(Some(trigger));
                } else if is_ident_char(c) {
                    if session_open {
                        self.refilter_code_completion();
                    } else {
                        self.request_code_completion(None);
                    }
                } else if session_open {
                    self.renderer.code_lsp.completion = None;
                }
            }
            CodeKeyEdit::Backspace => {
                if signature_active {
                    self.request_code_signature_help();
                }
                if session_open {
                    self.refilter_code_completion();
                }
            }
            CodeKeyEdit::Other => {
                if signature_active {
                    self.end_code_signature_help();
                }
                if session_open {
                    self.renderer.code_lsp.completion = None;
                }
            }
        }
    }
}
