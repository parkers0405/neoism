use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use neoism_agent_server::rust_lsp;
use neoism_protocol::{
    diagnostics::{DiagnosticItem, LspState},
    editor::{
        EditorLspAction, EditorLspCompletionItem, EditorLspLocation, EditorLspSymbol,
        EditorServerMessage, LspSnapshotServer,
    },
};

use crate::nvim::{DiagnosticsFetch, NvimSessionHandle};

#[derive(Debug, Clone)]
pub(crate) struct RustLspSnapshot {
    diagnostics: Vec<DiagnosticItem>,
    states: Vec<(String, LspState)>,
    /// Servers relevant to the active buffer's language, mapped for the
    /// status-bar pill / popup (`EditorServerMessage::LspSnapshot`).
    servers: Vec<LspSnapshotServer>,
    /// The active buffer's language id (empty when no bundled spec claims
    /// its extension).
    filetype: String,
    /// Active buffer path, used as the `file_path` on the desktop
    /// `EditorServerMessage::Diagnostics`.
    file: std::path::PathBuf,
}

/// One poll of the Rust LSP runtime for the active buffer, producing BOTH
/// the diagnostics fetch (web pill / diagnostics gutter) and the editor
/// `LspSnapshot` message (desktop status-bar pill + popup). Reads the
/// buffer and queries the engine once so the two surfaces stay in sync
/// without a double round-trip. Returns `(None, None)` when there is no
/// file-backed active buffer.
pub(crate) async fn poll(
    session: &NvimSessionHandle,
    workspace_root: &Path,
) -> (Option<DiagnosticsFetch>, Vec<EditorServerMessage>) {
    let Some(snapshot) = snapshot(session, workspace_root).await else {
        return (None, Vec::new());
    };
    let snapshot_message = EditorServerMessage::LspSnapshot {
        surface_id: None,
        filetype: snapshot.filetype,
        servers: snapshot.servers,
    };
    let diagnostics_message = diagnostics_message(&snapshot.diagnostics, &snapshot.file);
    let fetch =
        DiagnosticsFetch::from_parts(Some(snapshot.diagnostics), Some(snapshot.states));
    // Diagnostics are pushed immediately through `subscribe_diagnostics`, but
    // also replayed here from the active-buffer cache so a missed/early push
    // recovers on the next poll instead of leaving stale inline errors.
    (Some(fetch), vec![snapshot_message, diagnostics_message])
}

/// Subscribe to the engine's real-time `publishDiagnostics` bus. The socket
/// loop drains this and forwards to the editor with zero polling.
pub(crate) fn subscribe_diagnostics(
) -> tokio::sync::broadcast::Receiver<rust_lsp::DiagnosticsEvent> {
    rust_lsp::subscribe_diagnostics()
}

/// Convert an engine diagnostics push into the editor message.
pub(crate) fn diagnostics_event_message(
    event: rust_lsp::DiagnosticsEvent,
) -> EditorServerMessage {
    let diagnostics: Vec<DiagnosticItem> =
        event.diagnostics.into_iter().map(map_diagnostic).collect();
    diagnostics_message(&diagnostics, std::path::Path::new(&event.file))
}

/// The file a diagnostics push is for (so the socket loop can drop pushes for
/// buffers other than the active one).
pub(crate) fn diagnostics_event_file(event: &rust_lsp::DiagnosticsEvent) -> &str {
    &event.file
}

/// Build the desktop inline-diagnostics message (`EditorServerMessage::
/// Diagnostics`) from the engine's diagnostics for the active buffer,
/// tallying severities the way nvim's `rio_diagnostics` used to.
fn diagnostics_message(
    diagnostics: &[DiagnosticItem],
    file: &Path,
) -> EditorServerMessage {
    use neoism_protocol::editor::{
        DiagnosticItem as EditorDiagnostic, DiagnosticSeverity,
    };
    let (mut error, mut warn, mut info, mut hint) = (0u64, 0u64, 0u64, 0u64);
    let items = diagnostics
        .iter()
        .map(|diagnostic| {
            match diagnostic.severity {
                1 => error += 1,
                2 => warn += 1,
                3 => info += 1,
                _ => hint += 1,
            }
            EditorDiagnostic {
                severity: DiagnosticSeverity::from_u8(diagnostic.severity),
                message: diagnostic.message.clone(),
                source: diagnostic.source.clone(),
                line: diagnostic.line,
                col: diagnostic.col,
                lnum: diagnostic.line.saturating_add(1),
            }
        })
        .collect();
    EditorServerMessage::Diagnostics {
        surface_id: None,
        error,
        warn,
        info,
        hint,
        file_path: Some(file.to_path_buf()),
        items,
    }
}

pub(crate) async fn run_action(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    action: EditorLspAction,
    text: Option<&str>,
) -> Result<EditorServerMessage, String> {
    let buffer = read_active_file_buffer(session).await?;
    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
        eprintln!(
            "neoism::lsp action {action:?}: file={} cursor=({},{})",
            buffer.path.display(),
            buffer.cursor_line,
            buffer.cursor_col
        );
    }
    let diagnostics =
        rust_lsp::touch_document(workspace_root, &buffer.path, Some(&buffer.text));
    let server_count =
        rust_lsp::status(workspace_root, Some(buffer.path.as_path())).len();
    let diagnostic_count = diagnostics.len();

    let mut hover = None;
    let mut locations = Vec::new();
    let mut symbol_count = 0;
    let mut symbols = Vec::new();
    let summary = match action {
        EditorLspAction::Hover => {
            let result = rust_lsp::hover(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            hover = result.first().map(|item| item.contents.clone());
            format!("Rust LSP hover returned {} item(s)", result.len())
        }
        EditorLspAction::Definition => {
            let result = rust_lsp::definition(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            locations = result.into_iter().map(map_location).collect();
            format!("Rust LSP definition returned {} location(s)", locations.len())
        }
        EditorLspAction::References => {
            let result = rust_lsp::references(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            locations = result.into_iter().map(map_location).collect();
            format!("Rust LSP references returned {} location(s)", locations.len())
        }
        EditorLspAction::Implementation => {
            let result = rust_lsp::implementation(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            locations = result.into_iter().map(map_location).collect();
            format!("Rust LSP implementation returned {} location(s)", locations.len())
        }
        EditorLspAction::DocumentSymbols => {
            let result = rust_lsp::document_symbols(workspace_root, &buffer.path);
            symbols = flatten_document_symbols(&result, &buffer.path, 0);
            symbol_count = symbols.len();
            format!("Rust LSP document symbols returned {symbol_count} item(s)")
        }
        EditorLspAction::WorkspaceSymbols => {
            let query = text.unwrap_or_default().trim();
            if query.is_empty() {
                "Rust LSP workspace symbols need a search query".to_string()
            } else {
                let result = rust_lsp::workspace_symbols(workspace_root, query);
                symbols = result.into_iter().map(map_workspace_symbol).collect();
                symbol_count = symbols.len();
                format!("Rust LSP workspace symbols returned {symbol_count} item(s)")
            }
        }
        EditorLspAction::Info => format!(
            "Rust LSP synchronized document for {action:?}: {diagnostic_count} diagnostics, {server_count} servers"
        ),
        EditorLspAction::Format => match apply_formatting(session, workspace_root, &buffer).await {
            Ok(edit_count) => format!("Rust LSP applied formatting with {edit_count} edit(s)"),
            Err(error) => format!("Rust LSP formatting unavailable: {error}"),
        },
        EditorLspAction::CodeActions => {
            let actions = rust_lsp::code_actions(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            match apply_first_current_file_code_action(session, workspace_root, &buffer, &actions).await {
                Ok(Some((title, edit_count))) if edit_count == 0 => {
                    format!("Rust LSP executed code action `{title}`")
                }
                Ok(Some((title, edit_count))) => {
                    format!("Rust LSP applied code action `{title}` with {edit_count} edit(s)")
                }
                Ok(None) => format_code_action_summary(&actions),
                Err(error) => format!("Rust LSP code action unavailable: {error}"),
            }
        }
        EditorLspAction::Rename => match apply_rename(session, workspace_root, &buffer, text).await {
            Ok(edit_count) => format!("Rust LSP applied rename with {edit_count} edit(s)"),
            Err(error) => format!("Rust LSP rename unavailable: {error}"),
        },
        EditorLspAction::ToggleInlayHints => format!(
            "Rust LSP owns {action:?}; edit application UI is pending. Refreshed {diagnostic_count} diagnostics across {server_count} servers."
        ),
    };
    Ok(EditorServerMessage::LspActionResult {
        surface_id: None,
        action,
        line: buffer.cursor_line,
        character: buffer.cursor_col,
        summary,
        hover,
        locations,
        symbol_count,
        symbols,
    })
}

/// Flatten the LSP document-symbol tree (depth-first, parents before
/// children) into the wire outline. `depth` drives the picker's display
/// indentation; the jump target is the symbol's `selection_range` start
/// (falling back to its full `range`), so activating a row lands the
/// cursor on the symbol name rather than the enclosing block.
fn flatten_document_symbols(
    symbols: &[rust_lsp::LspDocumentSymbol],
    fallback_path: &Path,
    depth: u32,
) -> Vec<EditorLspSymbol> {
    let mut out = Vec::new();
    for symbol in symbols {
        let target = symbol
            .selection_range
            .as_ref()
            .or(symbol.range.as_ref())
            .map(|range| range.start.clone())
            .unwrap_or(rust_lsp::LspPosition {
                line: 0,
                character: 0,
            });
        let uri = if symbol.path.is_empty() {
            fallback_path.to_string_lossy().into_owned()
        } else {
            symbol.path.clone()
        };
        out.push(EditorLspSymbol {
            name: symbol.name.clone(),
            kind: symbol.kind.clone(),
            detail: symbol.detail.clone(),
            uri,
            line: target.line,
            character: target.character,
            depth,
        });
        out.extend(flatten_document_symbols(
            &symbol.children,
            fallback_path,
            depth + 1,
        ));
    }
    out
}

/// Completion at the active buffer's cursor, served from the Rust engine.
/// The engine request is blocking (spawns/queries the language server), so it
/// runs on the blocking pool — the daemon's async loop (and therefore nvim
/// keystroke dispatch) never stalls. `seq` is echoed so the client can drop a
/// response that a newer keystroke already superseded.
pub(crate) async fn completion(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    seq: u64,
) -> EditorServerMessage {
    let empty = EditorServerMessage::LspCompletions {
        surface_id: None,
        seq,
        replace_prefix: String::new(),
        items: Vec::new(),
    };
    let log = std::env::var_os("NEOISM_LSP_LOG").is_some();
    let buffer = match read_active_file_buffer(session).await {
        Ok(buffer) => buffer,
        Err(error) => {
            if log {
                eprintln!(
                    "neoism::lsp completion seq={seq}: no active file buffer ({error})"
                );
            }
            return empty;
        }
    };
    let replace_prefix = identifier_prefix(&buffer);
    let root = workspace_root.to_path_buf();
    let path = buffer.path.clone();
    let line = buffer.cursor_line;
    let character = buffer.cursor_col;
    if log {
        eprintln!(
            "neoism::lsp completion seq={seq}: requesting file={} line={line} char={character} prefix={replace_prefix:?}",
            path.display()
        );
    }
    let query_path = path.clone();
    let query_text = buffer.text.clone();
    let items = tokio::task::spawn_blocking(move || {
        rust_lsp::completion(&root, &query_path, line, character, Some(&query_text))
    })
    .await
    .unwrap_or_default();
    if log {
        eprintln!(
            "neoism::lsp completion seq={seq}: engine returned {} items (first={:?})",
            items.len(),
            items.first().map(|item| item.label.clone())
        );
    }
    EditorServerMessage::LspCompletions {
        surface_id: None,
        seq,
        replace_prefix,
        items: items.into_iter().map(map_completion_item).collect(),
    }
}

/// Hover docs at an EXPLICIT buffer position (the mouse cell), without moving
/// the cursor — powers VS Code-style hover-on-mouseover. Syncs the live buffer
/// first so the hover reflects unsaved edits, then queries the engine.
pub(crate) async fn hover_at(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    seq: u64,
    line: u32,
    character: u32,
) -> EditorServerMessage {
    let empty = EditorServerMessage::LspHoverResult {
        surface_id: None,
        seq,
        line,
        character,
        contents: String::new(),
    };
    let buffer = match read_active_file_buffer(session).await {
        Ok(buffer) => buffer,
        Err(_) => return empty,
    };
    let root = workspace_root.to_path_buf();
    let path = buffer.path.clone();
    let text = buffer.text.clone();
    let contents = tokio::task::spawn_blocking(move || {
        // Keep rust-analyzer's copy in step with the live buffer (deduped on
        // unchanged text), then hover at the requested cell.
        rust_lsp::sync_document(&root, &path, Some(&text));
        rust_lsp::hover(&root, &path, line, character)
            .first()
            .map(|item| item.contents.clone())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    EditorServerMessage::LspHoverResult {
        surface_id: None,
        seq,
        line,
        character,
        contents,
    }
}

/// The identifier already typed immediately before the cursor (the trailing
/// run of `[A-Za-z0-9_]`), which the client backspaces before inserting a
/// chosen item so prefix/member completion replaces cleanly.
fn identifier_prefix(buffer: &crate::nvim::BufferText) -> String {
    let line = buffer
        .text
        .lines()
        .nth(buffer.cursor_line as usize)
        .unwrap_or("");
    let col = (buffer.cursor_col as usize).min(line.chars().count());
    let before = line.chars().take(col).collect::<String>();
    let mut prefix = before
        .chars()
        .rev()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect::<Vec<_>>();
    prefix.reverse();
    prefix.into_iter().collect()
}

fn map_completion_item(item: rust_lsp::LspCompletionItem) -> EditorLspCompletionItem {
    EditorLspCompletionItem {
        label: item.label,
        kind: item.kind,
        detail: item.detail,
        documentation: item.documentation,
        insert_text: item.insert_text,
        filter_text: item.filter_text,
        sort_text: item.sort_text,
        preselect: item.preselect,
    }
}

fn format_code_action_summary(results: &[serde_json::Value]) -> String {
    let mut titles = Vec::new();
    for result in results {
        let Some(actions) = result.get("actions").and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for action in actions {
            if let Some(title) = action.get("title").and_then(serde_json::Value::as_str) {
                titles.push(title.to_string());
            }
        }
    }
    match titles.len() {
        0 => "Rust LSP found no code actions".to_string(),
        1 => format!("Rust LSP code action: {}", titles[0]),
        count => format!(
            "Rust LSP found {count} code actions: {}{}",
            titles
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            if count > 3 { ", ..." } else { "" }
        ),
    }
}

async fn apply_rename(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    buffer: &crate::nvim::BufferText,
    new_name: Option<&str>,
) -> Result<usize, String> {
    let new_name = rename_name(new_name)?;
    let results = rust_lsp::rename(
        workspace_root,
        &buffer.path,
        buffer.cursor_line,
        buffer.cursor_col,
        new_name,
    );
    for result in results {
        let Some(edit) = result.get("edit") else {
            continue;
        };
        let edits = workspace_edits(edit)?;
        if edits.is_empty() {
            continue;
        }
        let edit_count = apply_workspace_edits(session, buffer, edits).await?;
        return Ok(edit_count);
    }
    Err("rename returned no current-file edits".to_string())
}

fn rename_name(new_name: Option<&str>) -> Result<&str, String> {
    new_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "missing new name".to_string())
}

async fn apply_first_current_file_code_action(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    buffer: &crate::nvim::BufferText,
    results: &[serde_json::Value],
) -> Result<Option<(String, usize)>, String> {
    for action in iter_code_actions(results) {
        let title = action
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Untitled code action")
            .to_string();
        let action = if action.get("edit").is_some() {
            action.clone()
        } else {
            rust_lsp::resolve_code_action(workspace_root, &buffer.path, action.clone())
                .unwrap_or_else(|| action.clone())
        };
        let Some(edit) = action.get("edit") else {
            if let Some(command) = action.get("command") {
                if rust_lsp::execute_command(
                    workspace_root,
                    &buffer.path,
                    command.clone(),
                )
                .is_some()
                {
                    return Ok(Some((title, 0)));
                }
            }
            continue;
        };
        let edits = workspace_edits(edit)?;
        if edits.is_empty() {
            continue;
        }
        let edit_count = apply_workspace_edits(session, buffer, edits).await?;
        return Ok(Some((title, edit_count)));
    }
    Ok(None)
}

fn iter_code_actions(results: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    let mut actions = Vec::new();
    for result in results {
        if let Some(items) = result.get("actions").and_then(serde_json::Value::as_array) {
            actions.extend(items.iter());
        } else if result.get("title").is_some() {
            actions.push(result);
        }
    }
    actions
}

fn workspace_edits(
    edit: &serde_json::Value,
) -> Result<BTreeMap<PathBuf, Vec<serde_json::Value>>, String> {
    if let Some(changes) = edit.get("changes").and_then(serde_json::Value::as_object) {
        return edits_for_uri_map(changes);
    }
    if let Some(document_changes) = edit
        .get("documentChanges")
        .and_then(serde_json::Value::as_array)
    {
        let mut all: BTreeMap<PathBuf, Vec<serde_json::Value>> = BTreeMap::new();
        for change in document_changes {
            let Some(text_document) = change.get("textDocument") else {
                return Err("code action documentChanges with resource operations are not supported".to_string());
            };
            let Some(uri) = text_document.get("uri").and_then(serde_json::Value::as_str)
            else {
                return Err(
                    "code action documentChange missing textDocument.uri".to_string()
                );
            };
            let path = path_for_lsp_uri(uri)?;
            let Some(edits) = change.get("edits").and_then(serde_json::Value::as_array)
            else {
                return Err("code action documentChange missing edits".to_string());
            };
            all.entry(path).or_default().extend(edits.iter().cloned());
        }
        return Ok(all);
    }
    Ok(BTreeMap::new())
}

fn edits_for_uri_map(
    changes: &serde_json::Map<String, serde_json::Value>,
) -> Result<BTreeMap<PathBuf, Vec<serde_json::Value>>, String> {
    let mut grouped = BTreeMap::new();
    for (uri, edits) in changes {
        let path = path_for_lsp_uri(uri)?;
        let Some(edits) = edits.as_array() else {
            return Err("code action changes entry is not an edit array".to_string());
        };
        grouped
            .entry(path)
            .or_insert_with(Vec::new)
            .extend(edits.iter().cloned());
    }
    Ok(grouped)
}

fn path_for_lsp_uri(uri: &str) -> Result<PathBuf, String> {
    uri.strip_prefix("file://")
        .map(PathBuf::from)
        .or_else(|| uri.starts_with('/').then(|| PathBuf::from(uri)))
        .ok_or_else(|| format!("unsupported workspace edit uri `{uri}`"))
}

async fn apply_workspace_edits(
    session: &NvimSessionHandle,
    buffer: &crate::nvim::BufferText,
    edits_by_path: BTreeMap<PathBuf, Vec<serde_json::Value>>,
) -> Result<usize, String> {
    let mut total = 0usize;
    for (path, edits) in edits_by_path {
        if edits.is_empty() {
            continue;
        }
        let mut text = if path == buffer.path {
            buffer.text.clone()
        } else {
            std::fs::read_to_string(&path).map_err(|error| {
                format!("failed to read `{}`: {error}", path.display())
            })?
        };
        apply_lsp_text_edits(&mut text, &edits)?;
        if path == buffer.path {
            session
                .apply_authoritative_text(&path, &text)
                .await
                .map_err(|error| {
                    format!("failed to apply active buffer text: {error}")
                })?;
        } else {
            std::fs::write(&path, text).map_err(|error| {
                format!("failed to write `{}`: {error}", path.display())
            })?;
        }
        total += edits.len();
    }
    Ok(total)
}

async fn apply_formatting(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    buffer: &crate::nvim::BufferText,
) -> Result<usize, String> {
    let edits = rust_lsp::formatting(workspace_root, &buffer.path);
    if edits.is_empty() {
        return Ok(0);
    }
    let mut text = buffer.text.clone();
    apply_lsp_text_edits(&mut text, &edits)?;
    session
        .apply_authoritative_text(&buffer.path, &text)
        .await
        .map_err(|error| format!("failed to apply formatted text: {error}"))?;
    Ok(edits.len())
}

fn apply_lsp_text_edits(
    text: &mut String,
    edits: &[serde_json::Value],
) -> Result<(), String> {
    let mut replacements = Vec::with_capacity(edits.len());
    for edit in edits {
        let range = edit
            .get("range")
            .ok_or_else(|| "format edit missing range".to_string())?;
        let start = range_position(range, "start")?;
        let end = range_position(range, "end")?;
        let replacement = edit
            .get("newText")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "format edit missing newText".to_string())?
            .to_string();
        let start_offset = offset_for_lsp_position(text, start.0, start.1)?;
        let end_offset = offset_for_lsp_position(text, end.0, end.1)?;
        if start_offset > end_offset {
            return Err("format edit range is reversed".to_string());
        }
        replacements.push((start_offset, end_offset, replacement));
    }
    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut last_start = text.len();
    for (start, end, replacement) in replacements {
        if end > last_start {
            return Err("format edits overlap".to_string());
        }
        text.replace_range(start..end, &replacement);
        last_start = start;
    }
    Ok(())
}

fn range_position(range: &serde_json::Value, key: &str) -> Result<(u32, u32), String> {
    let position = range
        .get(key)
        .ok_or_else(|| format!("format edit missing {key}"))?;
    let line = position
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("format edit {key} missing line"))? as u32;
    let character = position
        .get("character")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("format edit {key} missing character"))?
        as u32;
    Ok((line, character))
}

fn offset_for_lsp_position(
    text: &str,
    line: u32,
    character: u32,
) -> Result<usize, String> {
    let mut offset = 0usize;
    for (idx, segment) in text.split_inclusive('\n').enumerate() {
        if idx as u32 == line {
            let line_without_newline = segment.strip_suffix('\n').unwrap_or(segment);
            let character = character as usize;
            if character > line_without_newline.len() {
                return Err("format edit character is past line end".to_string());
            }
            if !line_without_newline.is_char_boundary(character) {
                return Err("format edit character splits utf-8 codepoint".to_string());
            }
            return Ok(offset + character);
        }
        offset += segment.len();
    }
    if line as usize == text.lines().count() && character == 0 {
        return Ok(text.len());
    }
    Err("format edit line is past document end".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn applies_format_edits_from_bottom_to_top() {
        let mut text = "one\ntwo\nthree".to_string();
        apply_lsp_text_edits(
            &mut text,
            &[
                json!({
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}},
                    "newText": "ONE"
                }),
                json!({
                    "range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 5}},
                    "newText": "THREE"
                }),
            ],
        )
        .unwrap();

        assert_eq!(text, "ONE\ntwo\nTHREE");
    }

    #[test]
    fn rejects_overlapping_format_edits() {
        let mut text = "abcdef".to_string();
        let error = apply_lsp_text_edits(
            &mut text,
            &[
                json!({
                    "range": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 4}},
                    "newText": "X"
                }),
                json!({
                    "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 5}},
                    "newText": "Y"
                }),
            ],
        )
        .unwrap_err();

        assert!(error.contains("overlap"));
    }

    #[test]
    fn summarizes_code_action_titles() {
        let summary = format_code_action_summary(&[json!({
            "actions": [
                {"title": "Import foo"},
                {"title": "Create function"}
            ]
        })]);

        assert_eq!(
            summary,
            "Rust LSP found 2 code actions: Import foo, Create function"
        );
    }

    #[test]
    fn extracts_current_file_code_action_changes() {
        let edits = workspace_edits(
            &json!({
                "changes": {
                    "file:///tmp/main.rs": [
                        {
                            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                            "newText": "use foo;\n"
                        }
                    ]
                }
            }),
        )
        .unwrap();

        assert_eq!(edits.get(Path::new("/tmp/main.rs")).unwrap().len(), 1);
    }

    #[test]
    fn extracts_cross_file_code_action_changes() {
        let edits = workspace_edits(&json!({
            "changes": {
                "file:///tmp/main.rs": [],
                "file:///tmp/other.rs": []
            }
        }))
        .unwrap();

        assert!(edits.contains_key(Path::new("/tmp/main.rs")));
        assert!(edits.contains_key(Path::new("/tmp/other.rs")));
    }

    #[test]
    fn rename_requires_non_empty_name() {
        let error = rename_name(Some("   ")).unwrap_err();

        assert_eq!(error, "missing new name");
    }

    fn symbol(
        name: &str,
        kind: &str,
        line: u32,
        children: Vec<rust_lsp::LspDocumentSymbol>,
    ) -> rust_lsp::LspDocumentSymbol {
        rust_lsp::LspDocumentSymbol {
            name: name.to_string(),
            kind: kind.to_string(),
            detail: None,
            path: String::new(),
            range: None,
            selection_range: Some(rust_lsp::LspRange {
                start: rust_lsp::LspPosition { line, character: 4 },
                end: rust_lsp::LspPosition { line, character: 8 },
            }),
            children,
            language: None,
        }
    }

    #[test]
    fn flattens_symbol_tree_depth_first_with_indentation_depth() {
        let tree = vec![
            symbol(
                "Config",
                "struct",
                10,
                vec![symbol("new", "method", 12, Vec::new())],
            ),
            symbol("main", "function", 30, Vec::new()),
        ];

        let flat = flatten_document_symbols(&tree, Path::new("/tmp/main.rs"), 0);

        let shape: Vec<(&str, u32, u32)> = flat
            .iter()
            .map(|s| (s.name.as_str(), s.depth, s.line))
            .collect();
        assert_eq!(
            shape,
            vec![("Config", 0, 10), ("new", 1, 12), ("main", 0, 30)]
        );
        // Missing per-symbol path falls back to the active buffer path so
        // the picker can still open the location.
        assert_eq!(flat[0].uri, "/tmp/main.rs");
        // Jump target is the selection range start (the symbol name), not
        // column 0 of the enclosing block.
        assert_eq!(flat[0].character, 4);
    }
}

async fn read_active_file_buffer(
    session: &NvimSessionHandle,
) -> Result<crate::nvim::BufferText, String> {
    match session.read_active_buffer().await {
        Ok(Some(buffer)) if !buffer.path.as_os_str().is_empty() => Ok(buffer),
        Ok(Some(_)) | Ok(None) => {
            Err("Rust LSP needs a file-backed active buffer".to_string())
        }
        Err(error) => Err(format!("Rust LSP active buffer read failed: {error}")),
    }
}

async fn snapshot(
    session: &NvimSessionHandle,
    workspace_root: &Path,
) -> Option<RustLspSnapshot> {
    let buffer = match session.poll_active_buffer().await {
        Ok(Some(buffer)) => buffer,
        Ok(None) => return None,
        Err(error) => {
            tracing::debug!(error = %error, "rust lsp active buffer read failed");
            return None;
        }
    };
    if buffer.path.as_os_str().is_empty() {
        return None;
    }

    // The probe only includes text when nvim's changedtick advanced. Idle
    // polls therefore do not clone/hash/serialize the whole buffer. Large
    // documents stay in editor-only mode and never become full-text LSP
    // payloads.
    if !buffer.too_large {
        if let Some(text) = buffer.text.as_deref() {
            rust_lsp::sync_document(workspace_root, &buffer.path, Some(text));
        }
    }
    let diagnostics = if buffer.too_large {
        Vec::new()
    } else {
        rust_lsp::cached_diagnostics(workspace_root, &buffer.path)
    };
    let statuses = rust_lsp::status(workspace_root, Some(buffer.path.as_path()));
    let filetype = rust_lsp::language_id_for_path(&buffer.path)
        .unwrap_or_default()
        .to_string();

    // States feed the diagnostics-gutter/web pill and cover every
    // workspace server; the popup `servers` are narrowed to the ones that
    // handle the open buffer's language so the pill reflects "active for
    // this file".
    let live = rust_lsp::live_languages(workspace_root);
    let states = statuses
        .iter()
        .map(|status| (status.id.clone(), map_lsp_state(status.status.clone())))
        .collect();
    let mut servers: Vec<LspSnapshotServer> = statuses
        .iter()
        .filter(|status| filetype.is_empty() || status.language == filetype)
        .map(|status| map_snapshot_server(status, live.contains(&status.language)))
        .collect();
    if buffer.too_large {
        let limit_mib = crate::nvim::MAX_LSP_DOCUMENT_BYTES / (1024 * 1024);
        let size_mib = buffer.byte_len as f64 / (1024.0 * 1024.0);
        for server in &mut servers {
            server.state = "disabled".to_string();
            server.message = Some(format!(
                "Large-file mode: {:.1} MiB document exceeds the {limit_mib} MiB LSP limit",
                size_mib
            ));
        }
    }

    let diagnostics: Vec<DiagnosticItem> =
        diagnostics.into_iter().map(map_diagnostic).collect();
    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
        eprintln!(
            "neoism::lsp snapshot: file={} filetype={} bytes={} large={} servers={} live={:?} diagnostics={} states={:?}",
            buffer.path.display(),
            filetype,
            buffer.byte_len,
            buffer.too_large,
            servers.len(),
            live,
            diagnostics.len(),
            servers.iter().map(|s| format!("{}:{}", s.name, s.state)).collect::<Vec<_>>(),
        );
    }

    Some(RustLspSnapshot {
        diagnostics,
        states,
        servers,
        filetype,
        file: buffer.path.clone(),
    })
}

/// Map an engine `LspStatus` onto the wire `LspSnapshotServer` the
/// status-bar popup renders, carrying the resolution source
/// (extension/config/path/missing) so the popup can show where the binary
/// came from.
fn map_snapshot_server(status: &rust_lsp::LspStatus, is_live: bool) -> LspSnapshotServer {
    LspSnapshotServer {
        name: status.name.clone(),
        binary: status.command.first().cloned().unwrap_or_default(),
        filetype: status.language.clone(),
        state: snapshot_state_label(&status.status, &status.command_source, is_live)
            .to_string(),
        source: Some(command_source_label(&status.command_source).to_string()),
        message: status.detected.message.clone(),
        level: None,
    }
}

/// Popup state label (mirrors `lsp_popup::LspServerState::from_str`): a live
/// engine client reads as "attached", a resolvable-but-not-yet-running server
/// as "available", an unresolved binary as "missing".
fn snapshot_state_label(
    state: &rust_lsp::LspServerState,
    source: &rust_lsp::LspCommandSource,
    is_live: bool,
) -> &'static str {
    use rust_lsp::{LspCommandSource as C, LspServerState as S};
    match (state, source) {
        (S::Error, _) => "error",
        (_, C::Missing) => "missing",
        _ if is_live => "attached",
        (S::Connected, _) => "attached",
        (S::Available, _) => "available",
    }
}

fn command_source_label(source: &rust_lsp::LspCommandSource) -> &'static str {
    match source {
        rust_lsp::LspCommandSource::Extension => "extension",
        rust_lsp::LspCommandSource::Config => "config",
        rust_lsp::LspCommandSource::Path => "path",
        rust_lsp::LspCommandSource::Missing => "missing",
    }
}

fn map_lsp_state(state: rust_lsp::LspServerState) -> LspState {
    match state {
        rust_lsp::LspServerState::Available | rust_lsp::LspServerState::Connected => {
            LspState::Ready
        }
        rust_lsp::LspServerState::Error => LspState::Failed {
            message: "rust lsp server unavailable".to_string(),
        },
    }
}

fn map_workspace_symbol(symbol: rust_lsp::WorkspaceSymbol) -> EditorLspSymbol {
    EditorLspSymbol {
        name: symbol.name,
        kind: symbol.kind,
        detail: symbol.language,
        uri: symbol.path,
        // WorkspaceSymbol::line comes from the agent API as a 1-based display
        // line; OpenBuffer expects LSP/editor coordinates.
        line: symbol.line.unwrap_or(1).saturating_sub(1),
        character: 0,
        depth: 0,
    }
}

fn map_location(location: rust_lsp::LspLocation) -> EditorLspLocation {
    let start =
        location
            .range
            .map(|range| range.start)
            .unwrap_or(rust_lsp::LspPosition {
                line: 0,
                character: 0,
            });
    EditorLspLocation {
        uri: location.path,
        line: start.line,
        character: start.character,
    }
}

fn map_diagnostic(diagnostic: rust_lsp::LspDiagnostic) -> DiagnosticItem {
    let range = diagnostic.range.unwrap_or(rust_lsp::LspRange {
        start: rust_lsp::LspPosition {
            line: 0,
            character: 0,
        },
        end: rust_lsp::LspPosition {
            line: 0,
            character: 0,
        },
    });
    DiagnosticItem {
        line: range.start.line,
        col: range.start.character,
        severity: match diagnostic.severity.as_str() {
            "error" => 1,
            "warning" => 2,
            "information" => 3,
            "hint" => 4,
            _ => 2,
        },
        message: diagnostic.message,
        source: diagnostic.source,
    }
}
