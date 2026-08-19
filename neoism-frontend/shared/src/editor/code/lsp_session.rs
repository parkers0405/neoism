//! Shared LSP session layer for the native code pane — the state
//! machines desktop's bridge (`desktop/src/screen/bridges/code/lsp.rs`)
//! keeps on `Renderer::code_lsp`, lifted so the web host can drive the
//! exact same completion/hover/actions/rename/diagnostics behavior from
//! the daemon's editor envelopes.
//!
//! Design:
//! - IO is abstracted behind [`crate::services::LspService`] — requests
//!   are fire-and-forget with monotonic `seq` tokens; the HOST feeds
//!   results back through the `on_*` methods (desktop drains its worker
//!   mailbox, web routes daemon `EditorReply` messages). Stale seqs are
//!   dropped here, exactly like the desktop pump.
//! - Host-side effects (toasts, opening files, opening the references
//!   finder, completing a deferred save) are queued as [`LspUiEvent`]s
//!   the host drains after each call.
//! - Everything is wasm-safe: `web_time::Instant`, no threads.
//!
//! Logic is kept desktop-identical; where desktop still owns a private
//! copy (the `Screen`-threaded session structs), the duplication is
//! deliberate and noted for a later pass.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use web_time::Instant;

use crate::editor_snapshot::{PopupMenu, PopupMenuItem};
use crate::panels::finder::ReferenceRow;
use crate::panels::notifications::NotificationLevel;
use crate::services::{LspRequest, LspService};

use super::buffer::CodeTextEdit;
use super::layout::byte_for_utf16_col;
use super::types::CodePane;
use super::{CodeDiagnosticSeverity, CodeLineDiagnostic};

/// Fallback trigger characters used until a server-advertised set is
/// known (rust-analyzer & friends all advertise at least these).
/// Desktop: `bridges/code/lsp.rs::DEFAULT_TRIGGERS`.
pub const DEFAULT_TRIGGERS: [&str; 2] = [".", ":"];

/// Pointer-rest delay before a mouse hover request fires. Desktop:
/// `MOUSE_HOVER_DELAY_SECS` in `pump_code_lsp`.
pub const MOUSE_HOVER_DELAY_SECS: f32 = 0.4;

// ---------------------------------------------------------------------
// Pure helpers, ported verbatim from the desktop bridge. Desktop
// delegates to these (`bridges/code/lsp.rs` imports them) so the two
// frontends cannot drift.
// ---------------------------------------------------------------------

pub fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Byte column where the identifier containing/ending at `col` starts.
pub fn word_start_col(line: &str, col: usize) -> usize {
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
pub fn completion_prefix(line: &str, anchor: usize, cursor: usize) -> Option<String> {
    if anchor > cursor || cursor > line.len() {
        return None;
    }
    let slice = line.get(anchor..cursor)?;
    slice.chars().all(is_ident_char).then(|| slice.to_string())
}

/// Strip LSP snippet placeholders (`$0`, `$1`, `${2:default}`) from an
/// `insertTextFormat == 2` completion, keeping placeholder defaults,
/// and report the FIRST tabstop's `(byte offset, default-text len)` in
/// the stripped output — accept lands the caret there with the
/// placeholder selected, so typing replaces it (snippets v1: no
/// Tab-chain yet).
pub fn snippet_with_first_stop(text: &str) -> (String, Option<(usize, usize)>) {
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

/// Flatten per-server hover contents into the markdown-ish line list
/// the shared hover popup parses (fences kept; multiple servers
/// separated by a blank line). Desktop: `hover_card_lines`.
pub fn hover_card_lines<'a, I>(contents: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    const MAX_LINES: usize = 40;
    let mut out: Vec<String> = Vec::new();
    for hover in contents {
        if hover.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(String::new());
        }
        for line in hover.lines() {
            out.push(line.to_string());
            if out.len() >= MAX_LINES {
                return out;
            }
        }
    }
    out
}

/// Map a wire severity word onto the pane's severity enum. Desktop:
/// `map_severity`.
pub fn map_severity(severity: &str) -> CodeDiagnosticSeverity {
    match severity.to_ascii_lowercase().as_str() {
        "error" => CodeDiagnosticSeverity::Error,
        "warning" | "warn" => CodeDiagnosticSeverity::Warn,
        "hint" => CodeDiagnosticSeverity::Hint,
        _ => CodeDiagnosticSeverity::Info,
    }
}

/// One selectable row of the code-action popup. `action` is the raw
/// LSP CodeAction/Command payload, resolved lazily on accept.
/// Desktop's `CodeActionItem` re-exports this shape.
#[derive(Clone, Debug)]
pub struct LspCodeActionData {
    pub server_id: String,
    pub title: String,
    pub kind: String,
    pub action: serde_json::Value,
}

/// Short badge for a code-action kind (`quickfix` → "fix",
/// `refactor.extract` → "refactor", `source.organizeImports` →
/// "source", plain commands → "cmd").
pub fn action_kind_label(item: &LspCodeActionData) -> &'static str {
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

/// Flatten the engine's per-server `{language, path, actions}` groups
/// into popup rows. Preferred actions (server hint) bubble to the top,
/// otherwise server order is kept. Desktop: `flatten_code_actions`.
pub fn flatten_code_actions(groups: &[serde_json::Value]) -> Vec<LspCodeActionData> {
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
            items.push(LspCodeActionData {
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

/// Minimal `file://` URI → path decoder (percent-decoded). Workspace
/// edits key files by URI. Desktop: `file_uri_to_path`.
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
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

/// Parse raw LSP text edits (byte-coordinate boundary — the engine
/// transport already converted them) into buffer edits. Desktop:
/// `parse_lsp_text_edits`.
pub fn parse_lsp_text_edits(edits: &[serde_json::Value]) -> Vec<CodeTextEdit> {
    edits
        .iter()
        .filter_map(|edit| {
            Some(CodeTextEdit {
                start_line: edit.pointer("/range/start/line")?.as_u64()? as usize,
                start_col: edit.pointer("/range/start/character")?.as_u64()? as usize,
                end_line: edit.pointer("/range/end/line")?.as_u64()? as usize,
                end_col: edit.pointer("/range/end/character")?.as_u64()? as usize,
                text: edit.get("newText")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// Collect a WorkspaceEdit's text edits per file, in byte coords.
/// Handles both the `changes` uri-map and `documentChanges`
/// TextDocumentEdit entries; resource ops are skipped. Desktop:
/// `workspace_edit_file_edits` (which parses to raw JSON — this one
/// parses straight to buffer edits since every consumer does next).
pub fn workspace_edit_file_edits(
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

// ---------------------------------------------------------------------
// Result data shapes (host-neutral: desktop maps engine types, web maps
// daemon wire types).
// ---------------------------------------------------------------------

/// One completion candidate as the session consumes it.
#[derive(Clone, Debug, Default)]
pub struct LspCompletionData {
    pub server_id: Option<String>,
    pub label: String,
    /// Lowercase kind word ("function", "variable", …).
    pub kind: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: String,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,
    pub preselect: bool,
    /// Original CompletionItem (byte-coordinate boundary) for
    /// textEdit / additionalTextEdits / command handling on accept.
    pub payload: serde_json::Value,
}

/// One definition/reference location, 0-based line + UTF-8 byte col.
#[derive(Clone, Debug)]
pub struct LspLocationData {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

/// One stored diagnostic in wire coordinates: 0-based lines, 0-based
/// UTF-16 columns (the daemon's `DiagnosticItem` contract — the fold
/// converts to byte columns per line).
#[derive(Clone, Debug)]
pub struct LspStoredDiagnostic {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub severity: CodeDiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
}

// ---------------------------------------------------------------------
// Diagnostics store: per-file per-server raw sets + the re-anchor-lite
// heuristic + the per-line byte-span fold. Port of the desktop store
// (`diag_store` + `reanchor_diagnostics` + the remote fold in
// `apply_remote_code_lsp_message`).
// ---------------------------------------------------------------------

#[derive(Default)]
pub struct LspDiagnosticsStore {
    /// server-keyed raw sets per file, replaced wholesale per publish.
    files: HashMap<PathBuf, HashMap<String, Vec<LspStoredDiagnostic>>>,
    /// Bumped on every publish AND on heuristic re-anchors — panes
    /// refold when their seen version lags (desktop `DIAG_VERSION`).
    version: u64,
    /// Bumped ONLY on a real publish for that file (desktop
    /// `diag_publish_seq`).
    publish_seq: HashMap<PathBuf, u64>,
    /// Previous buffer lines per file for the line-shift heuristic
    /// (desktop `reanchor_diagnostics`'s `PREV`).
    prev_lines: HashMap<PathBuf, Vec<String>>,
}

impl LspDiagnosticsStore {
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn publish_seq(&self, file: &Path) -> u64 {
        self.publish_seq.get(file).copied().unwrap_or(0)
    }

    /// Install one publish: replaces `server`'s set for `file` and
    /// bumps both counters. `server` should be the publishing server id
    /// (web falls back to the first item's `source`, desktop-remote
    /// parity).
    pub fn publish(
        &mut self,
        file: PathBuf,
        server: String,
        items: Vec<LspStoredDiagnostic>,
    ) {
        self.files.entry(file.clone()).or_default().insert(server, items);
        *self.publish_seq.entry(file).or_insert(0) += 1;
        self.version = self.version.wrapping_add(1).max(1);
    }

    /// Anchor-lite for diagnostics (Zed keeps real anchors; we shift
    /// the stored ranges by the line-span delta of each edit). Port of
    /// desktop `reanchor_diagnostics`, on 0-based store lines.
    pub fn reanchor(&mut self, file: &Path, current: &[String]) {
        let prev = self
            .prev_lines
            .insert(file.to_path_buf(), current.to_vec());
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
        let shift = |line: &mut usize| {
            *line = (*line as i64 + delta).max(0) as usize;
        };
        let Some(by_server) = self.files.get_mut(file) else {
            return;
        };
        let mut moved = false;
        for diags in by_server.values_mut() {
            for diag in diags.iter_mut() {
                if (diag.line as i64) >= boundary {
                    shift(&mut diag.line);
                    shift(&mut diag.end_line);
                    moved = true;
                } else if (diag.end_line as i64) >= boundary {
                    shift(&mut diag.end_line);
                    moved = true;
                }
            }
        }
        if moved {
            self.version = self.version.wrapping_add(1).max(1);
        }
    }

    /// Rebuild `pane.diagnostics` from the raw store — the per-line
    /// byte-span fold, port of the desktop remote-diagnostics arm
    /// (`apply_remote_code_lsp_message::Diagnostics`): 0-based lines,
    /// UTF-16 wire columns → byte columns, zero-width ranges widened to
    /// a one-cell underline.
    pub fn fold_into_pane(&self, pane: &mut CodePane) {
        let mut per_line: HashMap<usize, Vec<CodeLineDiagnostic>> = HashMap::new();
        if let Some(by_server) = self.files.get(&pane.path) {
            for diags in by_server.values() {
                for item in diags {
                    let start_line = item.line;
                    let end_line = item.end_line.max(start_line);
                    for line_ix in start_line..=end_line {
                        let Some(line) = pane.buffer.lines.get(line_ix) else {
                            break;
                        };
                        let mut from = if line_ix == start_line {
                            byte_for_utf16_col(line, item.col)
                        } else {
                            0
                        };
                        let mut to = if line_ix == end_line {
                            byte_for_utf16_col(line, item.end_col)
                        } else {
                            line.len()
                        };
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
                                    severity: item.severity,
                                    message: if line_ix == start_line {
                                        item.message.clone()
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
        pane.diag_anchors.clear();
        pane.diagnostics = per_line;
    }

    /// Exact per-diagnostic counts for the file from the raw store
    /// (the pane's per-line span map would overcount multi-line
    /// diagnostics). Desktop: `code_diagnostic_counts`.
    pub fn counts_for(&self, file: &Path) -> crate::panels::status_line::DiagnosticCounts {
        let mut counts = crate::panels::status_line::DiagnosticCounts::default();
        if let Some(by_server) = self.files.get(file) {
            for diags in by_server.values() {
                for diag in diags {
                    match diag.severity {
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
    /// Desktop: `code_diagnostic_popup_items`.
    pub fn popup_items(
        &self,
        file: &Path,
        pill: crate::panels::status_line::DiagnosticPill,
    ) -> Vec<crate::panels::diagnostics_popup::PopupItem> {
        use crate::panels::diagnostics_popup::{PopupItem, Severity};
        let mut items = Vec::new();
        if let Some(by_server) = self.files.get(file) {
            for diags in by_server.values() {
                for diag in diags {
                    let wanted = match pill {
                        crate::panels::status_line::DiagnosticPill::Error => {
                            diag.severity == CodeDiagnosticSeverity::Error
                        }
                        crate::panels::status_line::DiagnosticPill::Warn => {
                            diag.severity == CodeDiagnosticSeverity::Warn
                        }
                    };
                    if !wanted {
                        continue;
                    }
                    items.push(PopupItem {
                        lnum: diag.line as u64 + 1,
                        severity: match diag.severity {
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
}

// ---------------------------------------------------------------------
// UI session state — port of desktop `CodeLspUiState` and friends.
// ---------------------------------------------------------------------

/// An open completion menu on the code pane. `display` is the popup
/// snapshot the shared `completion_menu` panel renders; it is rebuilt
/// whenever items/filter/selection change (not per frame).
pub struct LspCompletionSession {
    pub path: PathBuf,
    /// Line the session opened on (menu dismisses if the cursor leaves).
    pub line: usize,
    /// Byte column where the to-be-replaced word starts.
    pub anchor_col: usize,
    /// Stable per-session id (feeds `PopupMenu::grid`).
    pub id: u64,
    /// Seq of the newest completion request for this session.
    pub seq: u64,
    pub items: Vec<LspCompletionData>,
    /// Indices into `items` surviving the current prefix filter.
    pub filtered: Vec<usize>,
    /// Index into `filtered` of the highlighted row.
    pub selected: usize,
    pub display: PopupMenu,
}

/// A hover card pinned to the buffer position it was requested at.
/// `lines` stays empty until the result arrives; the render pass skips
/// empty cards and the pump dismisses the card when the cursor moves.
pub struct LspHoverCard {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub seq: u64,
    pub lines: Vec<String>,
    /// Mouse-idle hover (vs `K`): pinned to the hovered cell,
    /// dismissed when the pointer leaves it instead of on cursor move.
    pub from_mouse: bool,
}

/// Pointer-idle hover candidate: armed on mouse move over a cell,
/// requested once the pointer has rested on it long enough.
pub struct LspMouseHover {
    pub line: usize,
    pub col: usize,
    pub since: Instant,
    pub requested: bool,
}

/// An open code-action menu anchored at the request position. `items`
/// stays empty until the result lands; the popup only shows once
/// `display` has rows.
pub struct LspActionSession {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub id: u64,
    pub seq: u64,
    pub items: Vec<LspCodeActionData>,
    pub selected: usize,
    pub display: PopupMenu,
}

/// The rename target frozen at prompt-open time.
pub struct PendingLspRename {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

/// Host-side effect queued by the session; the host drains these after
/// every session call (`take_events`).
pub enum LspUiEvent {
    Toast {
        message: String,
        level: NotificationLevel,
    },
    /// Land on a definition target: open (or refocus) `path` and park
    /// the cursor at the 0-based `(line, byte col)`.
    OpenLocation {
        path: PathBuf,
        line: usize,
        col: usize,
    },
    /// Open the shared finder's References mode over the hits.
    OpenReferences {
        root: PathBuf,
        rows: Vec<ReferenceRow>,
    },
    /// A workspace edit touched an OPEN file that is not the active
    /// pane — the host applies the edits to that pane's buffer.
    ApplyEditsToFile {
        path: PathBuf,
        edits: Vec<CodeTextEdit>,
    },
    /// Format-on-save edits were applied (or none were needed) —
    /// finish the deferred save now.
    SaveAfterFormat,
    /// `<Space>r`: the host should prompt for the new name (desktop
    /// uses its modal) and call `submit_rename` with the answer.
    OpenRenamePrompt { word: String },
}

fn build_completion_popup(session: &LspCompletionSession) -> PopupMenu {
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
fn rebuild_completion_filter(session: &mut LspCompletionSession, prefix: &str) {
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

fn build_action_popup(session: &LspActionSession) -> PopupMenu {
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

/// What a standard-path keystroke did to the buffer, for the LSP
/// after-key hook (menu refilter / trigger / dismissal decisions).
/// Desktop: `CodeKeyEdit`.
#[derive(Clone, Copy, Debug)]
pub enum LspKeyEdit {
    Char(char),
    Backspace,
    Other,
}

/// Native code pane LSP session state — the shared twin of desktop's
/// `CodeLspUiState`, plus the request plumbing the desktop keeps on
/// its worker bridge.
#[derive(Default)]
pub struct CodeLspUi {
    /// IO backend; requests silently no-op while unset.
    service: Option<Arc<dyn LspService>>,
    pub completion: Option<LspCompletionSession>,
    pub hover: Option<LspHoverCard>,
    pub actions: Option<LspActionSession>,
    pub definition_seq: Option<u64>,
    pub references_seq: Option<u64>,
    pub rename_seq: Option<u64>,
    /// In-flight code-action apply (seq of the `ApplyCodeAction`).
    pub action_apply_seq: Option<u64>,
    pub pending_rename: Option<PendingLspRename>,
    pub mouse_hover: Option<LspMouseHover>,
    /// Live signature-help session: seq of the current card.
    pub signature_seq: Option<u64>,
    /// In-flight format-on-save: `(seq, revision the format ran on)`.
    pub format_seq: Option<(u64, u64)>,
    pub diagnostics: LspDiagnosticsStore,
    /// True while THIS session last fed the shared completion-menu
    /// panel's stored popup — lets the popup host clear the menu when
    /// the session ends without clobbering other feeders.
    pub owns_menu_popup: bool,
    events: Vec<LspUiEvent>,
    next_seq: u64,
    /// `(path, revision)` last shipped through `LspRequest::Sync`.
    synced: Option<(PathBuf, u64)>,
    /// Per-file server-advertised trigger characters (web currently
    /// leaves this empty → `DEFAULT_TRIGGERS`).
    pub triggers: HashMap<PathBuf, Vec<String>>,
}

impl CodeLspUi {
    pub fn install_service(&mut self, service: Arc<dyn LspService>) {
        self.service = Some(service);
    }

    pub fn has_service(&self) -> bool {
        self.service.is_some()
    }

    fn fire(&self, request: LspRequest) {
        if let Some(service) = self.service.as_ref() {
            let _ = service.request(request);
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        self.next_seq
    }

    fn toast(&mut self, message: impl Into<String>, level: NotificationLevel) {
        self.events.push(LspUiEvent::Toast {
            message: message.into(),
            level,
        });
    }

    /// Drain queued host effects.
    pub fn take_events(&mut self) -> Vec<LspUiEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn dismiss_popups(&mut self) {
        self.completion = None;
        self.hover = None;
        self.actions = None;
    }

    pub fn has_session_state(&self) -> bool {
        self.completion.is_some()
            || self.hover.is_some()
            || self.actions.is_some()
            || self.definition_seq.is_some()
            || self.references_seq.is_some()
            || self.rename_seq.is_some()
            || self.pending_rename.is_some()
    }

    /// Focus left the code pane — no stale popups may linger. Keeps the
    /// diagnostics store and service.
    pub fn clear_sessions(&mut self) {
        self.completion = None;
        self.hover = None;
        self.actions = None;
        self.definition_seq = None;
        self.references_seq = None;
        self.rename_seq = None;
        self.action_apply_seq = None;
        self.pending_rename = None;
        self.mouse_hover = None;
        self.signature_seq = None;
        self.format_seq = None;
    }

    /// Whether `c` is a completion trigger character for the focused
    /// file (server-advertised set, `DEFAULT_TRIGGERS` until fetched).
    pub fn completion_trigger(&self, path: &Path, c: char) -> Option<String> {
        let matched = match self.triggers.get(path) {
            Some(chars) if !chars.is_empty() => chars
                .iter()
                .any(|trigger| trigger.chars().eq(std::iter::once(c))),
            _ => DEFAULT_TRIGGERS
                .iter()
                .any(|trigger| trigger.chars().eq(std::iter::once(c))),
        };
        matched.then(|| c.to_string())
    }

    // -----------------------------------------------------------------
    // Per-frame pump (desktop `pump_code_lsp` minus the worker drains,
    // which arrive through `on_*`).
    // -----------------------------------------------------------------

    /// Ship the buffer to the backend when its revision moved since the
    /// last sync. Every position-carrying request calls this FIRST so
    /// the backend resolves it against the live text — the wire is a
    /// FIFO, mirroring the ordering the desktop worker channel gives
    /// its Sync-before-query jobs. Also runs the per-revision
    /// diagnostics anchor-lite pass.
    fn ensure_synced(&mut self, pane: &CodePane) {
        let revision = pane.buffer.revision;
        let needs_sync = self
            .synced
            .as_ref()
            .map(|(path, rev)| path != &pane.path || *rev != revision)
            .unwrap_or(true);
        if !needs_sync {
            return;
        }
        self.synced = Some((pane.path.clone(), revision));
        self.fire(LspRequest::Sync {
            path: pane.path.clone(),
            text: pane.buffer.text(),
            revision,
        });
        // Keep the raw store's line numbers roughly right between
        // publishes (desktop's per-revision anchor-lite pass).
        let lines = pane.buffer.lines.clone();
        self.diagnostics.reanchor(&pane.path, &lines);
    }

    /// Returns true when visible state changed (host should redraw).
    pub fn pump(&mut self, pane: &mut CodePane) -> bool {
        let mut dirty = false;

        // Ship buffer revisions to the backend (didOpen/didChange).
        self.ensure_synced(pane);

        // Fold fresh diagnostics into the pane when the store moved.
        let version = self.diagnostics.version();
        if pane.lsp_diag_version != version {
            pane.lsp_diag_version = version;
            pane.lsp_diag_publish_seq = self.diagnostics.publish_seq(&pane.path);
            self.diagnostics.fold_into_pane(pane);
            dirty = true;
        }

        // Pointer-idle hover: request once the pointer has rested long
        // enough on one cell (armed by `note_mouse_move`).
        let matured = self.mouse_hover.as_ref().is_some_and(|c| {
            !c.requested && c.since.elapsed().as_secs_f32() >= MOUSE_HOVER_DELAY_SECS
        });
        if matured {
            let (line, col) = {
                let cand = self.mouse_hover.as_mut().unwrap();
                cand.requested = true;
                (cand.line, cand.col)
            };
            let over_diagnostic = pane.diagnostics.get(&line).is_some_and(|spans| {
                spans
                    .iter()
                    .any(|d| col >= d.start && col < d.end && !d.message.is_empty())
            });
            if over_diagnostic {
                // The diagnostic is what the pointer is asking about —
                // show it instantly (local), like VS Code's merged
                // hover with the problem on top.
                self.show_diagnostic_card_at(pane, line, col);
            } else {
                self.request_hover_at(pane, line, col, true);
            }
            dirty = true;
        }

        // Position-based dismissal (belt and braces on top of the input
        // hooks): hover pins to the exact request position, completion
        // to its line + anchor, actions to the request position.
        let cursor = pane.buffer.cursor();
        if let Some(card) = self.hover.as_ref() {
            if card.path != pane.path
                || (!card.from_mouse
                    && (card.line != cursor.line || card.col != cursor.col))
            {
                if self.signature_seq == Some(card.seq) {
                    self.signature_seq = None;
                }
                self.hover = None;
                dirty = true;
            }
        }
        if let Some(session) = self.completion.as_ref() {
            if session.path != pane.path
                || session.line != cursor.line
                || cursor.col < session.anchor_col
            {
                self.completion = None;
                dirty = true;
            }
        }
        if let Some(session) = self.actions.as_ref() {
            if session.path != pane.path
                || session.line != cursor.line
                || session.col != cursor.col
            {
                self.actions = None;
                dirty = true;
            }
        }

        dirty
    }

    /// Pointer moved over the pane text area: arm/refresh the
    /// mouse-idle hover candidate. Pass `None` when the pointer is
    /// outside the pane. Desktop: `note_code_mouse_hover`.
    pub fn note_mouse_move(
        &mut self,
        pane: &CodePane,
        pos: Option<(f32, f32)>,
    ) -> bool {
        let Some((mx, my)) = pos else {
            let mut dirty = self.mouse_hover.take().is_some();
            if self.hover.as_ref().is_some_and(|card| card.from_mouse) {
                self.hover = None;
                dirty = true;
            }
            return dirty;
        };
        let [gx, gy, gw, gh] = pane.geometry.rect;
        let inside = gw > 0.0
            && mx >= pane.geometry.text_x
            && mx <= gx + gw
            && my >= gy
            && my <= gy + gh;
        if !inside {
            self.mouse_hover = None;
            if self.hover.as_ref().is_some_and(|card| card.from_mouse) {
                self.hover = None;
                return true;
            }
            return false;
        }
        let (line, col) = pane.geometry.hit_position(&pane.buffer.lines, mx, my);
        let same_cell = self
            .mouse_hover
            .as_ref()
            .is_some_and(|cand| cand.line == line && cand.col == col);
        if same_cell {
            return false;
        }
        // Pointer moved to a new cell: any mouse card dies with it.
        let mut dirty = false;
        if self.hover.as_ref().is_some_and(|card| {
            card.from_mouse && (card.line != line || card.col != col)
        }) {
            self.hover = None;
            dirty = true;
        }
        self.mouse_hover = Some(LspMouseHover {
            line,
            col,
            since: Instant::now(),
            requested: false,
        });
        dirty
    }

    /// Click on a diagnostic span: show its message(s) as a hover-style
    /// card pinned to the span start. Desktop:
    /// `show_code_diagnostic_card_at`.
    pub fn show_diagnostic_card_at(&mut self, pane: &CodePane, line: usize, col: usize) {
        let spans = pane.diagnostics.get(&line).cloned().unwrap_or_default();
        let mut hits: Vec<&CodeLineDiagnostic> = spans
            .iter()
            .filter(|d| col >= d.start && col < d.end)
            .collect();
        if hits.is_empty() {
            // Click landed past the text (the inline `■ message` zone):
            // open the line's strongest messaged diagnostic instead.
            let line_len = pane.buffer.lines.get(line).map(|l| l.len()).unwrap_or(0);
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
        let seq = self.next_seq();
        self.hover = Some(LspHoverCard {
            path: pane.path.clone(),
            line,
            col: anchor_col,
            seq,
            lines,
            from_mouse: true,
        });
    }

    // -----------------------------------------------------------------
    // Requests (input paths call these; results land via `on_*`).
    // -----------------------------------------------------------------

    /// Open/refresh a completion session at the cursor. Desktop:
    /// `request_code_completion`.
    pub fn request_completion(&mut self, pane: &CodePane, trigger: Option<String>) {
        self.ensure_synced(pane);
        let cursor = pane.buffer.cursor();
        let line_text = pane
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
        let seq = self.next_seq();
        match self.completion.as_mut() {
            // Same anchor: keep the visible menu while the re-query is
            // in flight, just retoken it so only the newest installs.
            Some(session)
                if session.path == pane.path
                    && session.line == cursor.line
                    && session.anchor_col == anchor_col =>
            {
                session.seq = seq;
            }
            _ => {
                self.completion = Some(LspCompletionSession {
                    path: pane.path.clone(),
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
        self.fire(LspRequest::Completion {
            path: pane.path.clone(),
            line: cursor.line as u32,
            character: cursor.col as u32,
            trigger,
            seq,
        });
    }

    /// Request hover docs at the cursor (vim `K`). Desktop:
    /// `request_code_hover`.
    pub fn request_hover(&mut self, pane: &CodePane) {
        let cursor = pane.buffer.cursor();
        self.request_hover_at(pane, cursor.line, cursor.col, false);
    }

    pub fn request_hover_at(
        &mut self,
        pane: &CodePane,
        line: usize,
        col: usize,
        from_mouse: bool,
    ) {
        self.ensure_synced(pane);
        let seq = self.next_seq();
        self.completion = None;
        self.hover = Some(LspHoverCard {
            path: pane.path.clone(),
            line,
            col,
            seq,
            lines: Vec::new(),
            from_mouse,
        });
        self.fire(LspRequest::Hover {
            path: pane.path.clone(),
            line: line as u32,
            character: col as u32,
            seq,
        });
    }

    /// Signature help at the caret: install/refresh the card (the
    /// result rides the hover surface as a synthetic hover). Carries
    /// the previous card's lines while retriggering so the popup never
    /// flickers empty between keystrokes. Desktop:
    /// `request_code_signature_help`.
    pub fn request_signature_help(&mut self, pane: &CodePane) {
        self.ensure_synced(pane);
        let line = pane.buffer.cursor_line;
        let col = pane.buffer.cursor_col;
        let seq = self.next_seq();
        let carried_lines = match (self.signature_seq, self.hover.take()) {
            (Some(previous), Some(card)) if card.seq == previous => card.lines,
            (_, other) => {
                self.hover = other;
                Vec::new()
            }
        };
        self.signature_seq = Some(seq);
        self.hover = Some(LspHoverCard {
            path: pane.path.clone(),
            line,
            col,
            seq,
            lines: carried_lines,
            from_mouse: false,
        });
        self.fire(LspRequest::SignatureHelp {
            path: pane.path.clone(),
            line: line as u32,
            character: col as u32,
            seq,
        });
    }

    /// End the signature-help session (typed `)`, committed the line,
    /// or moved away), dismissing its card if still up.
    pub fn end_signature_help(&mut self) {
        if let Some(seq) = self.signature_seq.take() {
            if self.hover.as_ref().is_some_and(|card| card.seq == seq) {
                self.hover = None;
            }
        }
    }

    /// Request go-to-definition at an explicit buffer position
    /// (Ctrl+Click hit or the cursor for vim `gd`).
    pub fn request_definition_at(&mut self, pane: &CodePane, line: usize, col: usize) {
        self.ensure_synced(pane);
        let seq = self.next_seq();
        self.definition_seq = Some(seq);
        self.fire(LspRequest::Definition {
            path: pane.path.clone(),
            line: line as u32,
            character: col as u32,
            seq,
        });
    }

    /// Request find-references at the cursor (vim `gr`).
    pub fn request_references(&mut self, pane: &CodePane) {
        self.ensure_synced(pane);
        let cursor = pane.buffer.cursor();
        let seq = self.next_seq();
        self.dismiss_popups();
        self.references_seq = Some(seq);
        self.fire(LspRequest::References {
            path: pane.path.clone(),
            line: cursor.line as u32,
            character: cursor.col as u32,
            seq,
        });
    }

    /// Request code actions at the cursor and open a (initially empty)
    /// menu session pinned there (`<Space>a` / Ctrl+.).
    pub fn request_code_actions(&mut self, pane: &CodePane) {
        self.ensure_synced(pane);
        let cursor = pane.buffer.cursor();
        let seq = self.next_seq();
        self.completion = None;
        self.hover = None;
        self.actions = Some(LspActionSession {
            path: pane.path.clone(),
            line: cursor.line,
            col: cursor.col,
            id: seq,
            seq,
            items: Vec::new(),
            selected: 0,
            display: PopupMenu::default(),
        });
        self.fire(LspRequest::CodeActions {
            path: pane.path.clone(),
            line: cursor.line as u32,
            character: cursor.col as u32,
            seq,
        });
    }

    /// Whether the code-action menu is visibly open (results arrived).
    pub fn action_menu_open(&self) -> bool {
        self.actions
            .as_ref()
            .is_some_and(|session| !session.display.items.is_empty())
    }

    pub fn move_action_selection(&mut self, delta: isize) {
        if let Some(session) = self.actions.as_mut() {
            let len = session.items.len();
            if len == 0 {
                return;
            }
            let current = session.selected as isize;
            let next = (current + delta).rem_euclid(len as isize) as usize;
            session.selected = next;
            session.display.selected = Some(next);
        }
    }

    /// Enter on the action menu: hand the highlighted action to the
    /// backend (resolve → edit → execute) and close the menu. The edit
    /// lands back through `on_workspace_edit_result`.
    pub fn apply_selected_action(&mut self, pane: &CodePane) -> bool {
        let Some(session) = self.actions.take() else {
            return false;
        };
        let Some(item) = session.items.get(session.selected).cloned() else {
            return false;
        };
        let seq = self.next_seq();
        self.action_apply_seq = Some(seq);
        self.fire(LspRequest::ApplyCodeAction {
            path: pane.path.clone(),
            server_id: item.server_id,
            title: item.title,
            action: item.action,
            seq,
        });
        true
    }

    /// `<Space>r`: freeze the rename target and ask the host to prompt
    /// for the new name. Desktop opens its modal; web hosts prompt
    /// however they can, then call [`Self::submit_rename`].
    pub fn open_rename_prompt(&mut self, pane: &CodePane) {
        use crate::editor::markdown::vim::vim_word_under_cursor;
        let cursor = pane.buffer.cursor();
        let line_text = pane
            .buffer
            .lines
            .get(cursor.line)
            .cloned()
            .unwrap_or_default();
        let Some((start, end)) = vim_word_under_cursor(&line_text, cursor.col) else {
            self.toast("No symbol under cursor", NotificationLevel::Info);
            return;
        };
        let word = line_text[start..end].to_string();
        self.dismiss_popups();
        self.pending_rename = Some(PendingLspRename {
            path: pane.path.clone(),
            line: cursor.line,
            col: cursor.col,
        });
        self.events.push(LspUiEvent::OpenRenamePrompt { word });
    }

    /// Prompt submit: fire the rename request at the frozen position.
    pub fn submit_rename(&mut self, pane: &CodePane, new_name: String) {
        let Some(pending) = self.pending_rename.take() else {
            return;
        };
        let new_name = new_name.trim().to_string();
        if new_name.is_empty() {
            return;
        }
        if pane.path != pending.path {
            self.toast("Rename target changed — aborted", NotificationLevel::Warn);
            return;
        }
        self.ensure_synced(pane);
        let seq = self.next_seq();
        self.rename_seq = Some(seq);
        self.fire(LspRequest::Rename {
            path: pane.path.clone(),
            line: pending.line as u32,
            character: pending.col as u32,
            new_name,
            seq,
        });
    }

    /// Format-on-save entry: fire the formatter; the edits land through
    /// `on_workspace_edit_result` which applies them revision-guarded
    /// and queues `SaveAfterFormat`. Returns false when no backend is
    /// installed (caller saves directly).
    pub fn queue_format_then_save(&mut self, pane: &CodePane) -> bool {
        if self.service.is_none() {
            return false;
        }
        self.ensure_synced(pane);
        let seq = self.next_seq();
        self.format_seq = Some((seq, pane.buffer.revision));
        self.fire(LspRequest::Format {
            path: pane.path.clone(),
            revision: pane.buffer.revision,
            seq,
        });
        true
    }

    /// Notify the backend after a successful save (didSave triggers
    /// slow-lane checks like `cargo check` on rust-analyzer).
    pub fn notify_saved(&mut self, path: &Path) {
        self.fire(LspRequest::SaveNotify {
            path: path.to_path_buf(),
        });
    }

    // -----------------------------------------------------------------
    // Completion menu interaction (desktop parity).
    // -----------------------------------------------------------------

    /// Whether the completion menu is visibly open.
    pub fn completion_menu_open(&self) -> bool {
        self.completion
            .as_ref()
            .is_some_and(|session| !session.display.items.is_empty())
    }

    pub fn move_completion_selection(&mut self, delta: isize) {
        if let Some(session) = self.completion.as_mut() {
            let len = session.filtered.len();
            if len == 0 {
                return;
            }
            let current = session.selected as isize;
            let next = (current + delta).rem_euclid(len as isize) as usize;
            session.selected = next;
            session.display.selected = Some(next);
        }
    }

    /// Insert the highlighted completion, replacing the typed prefix
    /// (or the server's `textEdit` start when it names one on the same
    /// line). Fires the item's follow-up `command` when present
    /// (executed server-side as a bare Command action). Desktop:
    /// `accept_code_completion`.
    pub fn accept_completion(&mut self, pane: &mut CodePane) -> bool {
        let Some(session) = self.completion.take() else {
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
            .zip(item.payload.get("command").cloned())
            .map(|(server_id, command)| (server_id, item.label.clone(), command));
        // Auto-imports and friends: extra edits the server wants applied
        // alongside the accepted item (byte-coordinate boundary).
        let additional_edits = item
            .payload
            .get("additionalTextEdits")
            .and_then(|edits| edits.as_array())
            .cloned()
            .unwrap_or_default();

        {
            if pane.path != session.path || pane.buffer.cursor_line != session.line {
                return false;
            }
            let cursor_col = pane.buffer.cursor_col;
            let start = edit_start.unwrap_or(session.anchor_col).min(cursor_col);
            let start = if pane
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
                pane.buffer.set_cursor_position(session.line, start, false);
                pane.buffer
                    .set_cursor_position(session.line, cursor_col, true);
            }
            pane.buffer.insert_text(&insert);
            pane.buffer.follow_cursor = true;

            // Apply the additional edits AFTER the completion text:
            // auto-import inserts live above the caret; the caret is
            // re-parked by the net line delta of edits above it.
            let mut import_line_shift: i64 = 0;
            if !additional_edits.is_empty() {
                let parsed = parse_lsp_text_edits(&additional_edits);
                if !parsed.is_empty() {
                    let cursor_line = pane.buffer.cursor_line;
                    let cursor_col = pane.buffer.cursor_col;
                    let line_shift: i64 = parsed
                        .iter()
                        .filter(|edit| edit.end_line < cursor_line)
                        .map(|edit| {
                            edit.text.matches('\n').count() as i64
                                - (edit.end_line - edit.start_line) as i64
                        })
                        .sum();
                    pane.buffer.apply_text_edits(&parsed);
                    let target = (cursor_line as i64 + line_shift).max(0) as usize;
                    pane.buffer.set_cursor_position(target, cursor_col, false);
                    pane.buffer.follow_cursor = true;
                    import_line_shift = line_shift;
                }
            }

            // Snippet: land the caret on the FIRST tabstop with its
            // placeholder selected, so typing replaces it.
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
                pane.buffer.set_cursor_position(stop_line, stop_col, false);
                if len > 0 {
                    pane.buffer
                        .set_cursor_position(stop_line, stop_col + len, true);
                }
                pane.buffer.follow_cursor = true;
            }
        }

        if let Some((server_id, title, command)) = follow_up {
            let seq = self.next_seq();
            self.fire(LspRequest::ApplyCodeAction {
                path: pane.path.clone(),
                server_id,
                title,
                // A bare Command payload — the backend routes it to
                // workspace/executeCommand without a resolve step.
                action: serde_json::json!({ "command": command }),
                seq,
            });
        }

        true
    }

    /// Refilter the open session against the typed prefix; dismisses
    /// when the prefix stops being an identifier or nothing matches.
    /// Desktop: `refilter_code_completion`.
    pub fn refilter_completion(&mut self, pane: &CodePane) {
        let cursor = pane.buffer.cursor();
        let line_text = pane
            .buffer
            .lines
            .get(cursor.line)
            .cloned()
            .unwrap_or_default();
        let Some(session) = self.completion.as_mut() else {
            return;
        };
        if session.path != pane.path || session.line != cursor.line {
            self.completion = None;
            return;
        }
        let keep = match completion_prefix(&line_text, session.anchor_col, cursor.col) {
            Some(prefix) => {
                if session.items.is_empty() {
                    // Results still in flight; the install path filters
                    // with the then-current prefix.
                    true
                } else {
                    rebuild_completion_filter(session, &prefix);
                    !session.filtered.is_empty()
                }
            }
            None => false,
        };
        if !keep {
            self.completion = None;
        }
    }

    /// Post-edit hook from the standard key path: drives completion
    /// open/refilter/dismiss, signature retriggering, and hover
    /// dismissal on any edit. Desktop: `code_lsp_after_key`.
    pub fn after_key(&mut self, pane: &CodePane, edit: LspKeyEdit) {
        let signature_active = self.signature_seq.is_some();
        if self.hover.is_some() && !signature_active {
            self.hover = None;
        }
        let session_open = self.completion.is_some();
        match edit {
            LspKeyEdit::Char(c) => {
                if c == '(' || c == ',' {
                    self.request_signature_help(pane);
                } else if signature_active {
                    if c == ')' {
                        self.end_signature_help();
                    } else {
                        self.request_signature_help(pane);
                    }
                }
                if let Some(trigger) = self.completion_trigger(&pane.path, c) {
                    self.request_completion(pane, Some(trigger));
                } else if is_ident_char(c) {
                    if session_open {
                        self.refilter_completion(pane);
                    } else {
                        self.request_completion(pane, None);
                    }
                } else if session_open {
                    self.completion = None;
                }
            }
            LspKeyEdit::Backspace => {
                if signature_active {
                    self.request_signature_help(pane);
                }
                if session_open {
                    self.refilter_completion(pane);
                }
            }
            LspKeyEdit::Other => {
                if signature_active {
                    self.end_signature_help();
                }
                if session_open {
                    self.completion = None;
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Result ingestion (host pushes; stale seqs drop here). Port of the
    // desktop drain (`drain_code_lsp_results`).
    // -----------------------------------------------------------------

    pub fn on_completion_result(
        &mut self,
        pane: &CodePane,
        seq: u64,
        path: &Path,
        mut items: Vec<LspCompletionData>,
    ) -> bool {
        let cursor = pane.buffer.cursor();
        let line_text = pane
            .buffer
            .lines
            .get(cursor.line)
            .cloned()
            .unwrap_or_default();
        let Some(session) = self.completion.as_mut() else {
            return false;
        };
        if session.seq != seq || session.path != path {
            return false;
        }
        items.sort_by(|a, b| {
            let ka = a.sort_text.as_deref().unwrap_or(&a.label);
            let kb = b.sort_text.as_deref().unwrap_or(&b.label);
            ka.cmp(kb).then_with(|| a.label.cmp(&b.label))
        });
        session.items = items;
        let installed =
            match completion_prefix(&line_text, session.anchor_col, cursor.col) {
                Some(prefix) => {
                    rebuild_completion_filter(session, &prefix);
                    !session.filtered.is_empty()
                }
                None => false,
            };
        if !installed {
            self.completion = None;
        }
        true
    }

    /// `contents` is the per-server hover contents (already fetched);
    /// empty ⇒ dismiss the card. Also serves signature-help results
    /// (they ride the hover card, desktop parity).
    pub fn on_hover_result(&mut self, seq: u64, path: &Path, contents: &[String]) -> bool {
        let Some(card) = self.hover.as_mut() else {
            return false;
        };
        if card.seq != seq || card.path != path {
            return false;
        }
        let lines = hover_card_lines(contents.iter().map(|s| s.as_str()));
        if lines.is_empty() {
            self.hover = None;
        } else {
            card.lines = lines;
        }
        true
    }

    pub fn on_definition_result(
        &mut self,
        seq: u64,
        locations: Vec<LspLocationData>,
    ) -> bool {
        if self.definition_seq != Some(seq) {
            return false;
        }
        self.definition_seq = None;
        match locations.into_iter().next() {
            Some(location) => self.events.push(LspUiEvent::OpenLocation {
                path: location.path,
                line: location.line,
                col: location.col,
            }),
            None => self.toast("No definition found", NotificationLevel::Info),
        }
        true
    }

    pub fn on_references_result(
        &mut self,
        seq: u64,
        root: PathBuf,
        rows: Vec<ReferenceRow>,
    ) -> bool {
        if self.references_seq != Some(seq) {
            return false;
        }
        self.references_seq = None;
        if rows.is_empty() {
            self.toast("No references found", NotificationLevel::Info);
        } else {
            self.events.push(LspUiEvent::OpenReferences { root, rows });
        }
        true
    }

    pub fn on_code_actions_result(
        &mut self,
        seq: u64,
        path: &Path,
        items: Vec<LspCodeActionData>,
    ) -> bool {
        let Some(session) = self.actions.as_mut() else {
            return false;
        };
        if session.seq != seq || session.path != path {
            return false;
        }
        if items.is_empty() {
            self.actions = None;
            self.toast("No code actions", NotificationLevel::Info);
        } else {
            session.items = items;
            session.selected = 0;
            session.display = build_action_popup(session);
        }
        true
    }

    /// Which in-flight edit-shaped request a workspace-edit result
    /// answers.
    fn classify_edit_seq(&mut self, seq: u64) -> Option<EditResultKind> {
        if self.format_seq.map(|(s, _)| s) == Some(seq) {
            let (_, revision) = self.format_seq.take().unwrap();
            return Some(EditResultKind::Format { revision });
        }
        if self.rename_seq == Some(seq) {
            self.rename_seq = None;
            return Some(EditResultKind::Rename);
        }
        if self.action_apply_seq == Some(seq) {
            self.action_apply_seq = None;
            return Some(EditResultKind::AppliedAction);
        }
        None
    }

    /// Land an edit-shaped result (format-on-save / rename / applied
    /// code action). `per_file` carries typed edits for OPEN files (the
    /// active pane is applied here, others become `ApplyEditsToFile`
    /// events); `applied_files` are files the backend already patched
    /// on disk.
    #[allow(clippy::too_many_arguments)]
    pub fn on_workspace_edit_result(
        &mut self,
        pane: &mut CodePane,
        seq: u64,
        title: &str,
        per_file: Vec<(PathBuf, Vec<CodeTextEdit>)>,
        applied_files: usize,
        ran_command: bool,
    ) -> bool {
        let Some(kind) = self.classify_edit_seq(seq) else {
            return false;
        };
        match kind {
            EditResultKind::Format { revision } => {
                if let Some((_, edits)) =
                    per_file.into_iter().find(|(path, _)| path == &pane.path)
                {
                    // Apply only if the buffer hasn't moved since the
                    // format ran (desktop's revision guard).
                    if pane.buffer.revision == revision && !edits.is_empty() {
                        pane.buffer.apply_text_edits(&edits);
                    }
                }
                self.events.push(LspUiEvent::SaveAfterFormat);
            }
            EditResultKind::Rename | EditResultKind::AppliedAction => {
                let mut touched = applied_files;
                for (path, edits) in per_file {
                    if edits.is_empty() {
                        continue;
                    }
                    if path == pane.path {
                        pane.buffer.apply_text_edits(&edits);
                        pane.buffer.follow_cursor = true;
                        touched += 1;
                    } else {
                        self.events
                            .push(LspUiEvent::ApplyEditsToFile { path, edits });
                        touched += 1;
                    }
                }
                let message = match kind {
                    EditResultKind::Rename => match touched {
                        0 => "Rename produced no edits".to_string(),
                        1 => "Renamed in 1 file".to_string(),
                        n => format!("Renamed in {n} files"),
                    },
                    _ => {
                        if touched > 1 {
                            format!("{title} — edited {touched} files")
                        } else if touched == 1 || ran_command {
                            title.to_string()
                        } else {
                            format!("{title}: no edit returned")
                        }
                    }
                };
                let level = if touched == 0 && !ran_command {
                    NotificationLevel::Warn
                } else {
                    NotificationLevel::Info
                };
                self.toast(message, level);
            }
        }
        true
    }

    /// A pending edit-shaped request failed backend-side; surface the
    /// error and unblock any deferred save.
    pub fn on_request_error(&mut self, message: &str) {
        if self.format_seq.take().is_some() {
            // Formatting failed — finish the save unformatted rather
            // than dropping the user's `:w` on the floor.
            self.events.push(LspUiEvent::SaveAfterFormat);
        }
        self.definition_seq = None;
        self.references_seq = None;
        self.rename_seq = None;
        self.action_apply_seq = None;
        if !message.is_empty() {
            self.toast(message.to_string(), NotificationLevel::Warn);
        }
    }
}

enum EditResultKind {
    Format { revision: u64 },
    Rename,
    AppliedAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_first_stop_strips_placeholders() {
        let (text, stop) = snippet_with_first_stop("println!(\"${1:msg}\")$0");
        assert_eq!(text, "println!(\"msg\")");
        assert_eq!(stop, Some((10, 3)));
    }

    #[test]
    fn completion_prefix_rejects_non_ident() {
        assert_eq!(completion_prefix("foo.bar", 4, 7), Some("bar".into()));
        assert_eq!(completion_prefix("foo.bar", 0, 7), None);
    }

    #[test]
    fn reanchor_shifts_lines_below_edit() {
        let mut store = LspDiagnosticsStore::default();
        store.publish(
            PathBuf::from("/f.rs"),
            "srv".into(),
            vec![LspStoredDiagnostic {
                line: 5,
                col: 0,
                end_line: 5,
                end_col: 3,
                severity: CodeDiagnosticSeverity::Error,
                message: "broken".into(),
                source: None,
            }],
        );
        let before: Vec<String> = (0..10).map(|i| format!("line{i}")).collect();
        store.reanchor(Path::new("/f.rs"), &before);
        // Insert one line at index 2.
        let mut after = before.clone();
        after.insert(2, "inserted".into());
        store.reanchor(Path::new("/f.rs"), &after);
        let diag = &store.files[Path::new("/f.rs")]["srv"][0];
        assert_eq!(diag.line, 6);
        assert_eq!(diag.end_line, 6);
    }
}
