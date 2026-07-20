use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use neoism_agent_server::language_server;
use neoism_protocol::editor::{
    EditorLspAction, EditorLspCodeAction, EditorLspCompletionItem, EditorLspLocation,
    EditorLspSymbol, EditorServerMessage,
};

use crate::nvim::NvimSessionHandle;

use super::active_buffer::read_active_file_buffer;

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
    crate::language_server::flush_document_sync(workspace_root, &buffer.path);
    let diagnostics = language_server::touch_document(workspace_root, &buffer.path, None);
    let server_count =
        language_server::status(workspace_root, Some(buffer.path.as_path())).len();
    let diagnostic_count = diagnostics.len();

    let mut hover = None;
    let mut locations = Vec::new();
    let mut symbol_count = 0;
    let mut symbols = Vec::new();
    let mut code_actions = Vec::new();
    let summary = match action {
        EditorLspAction::Hover => {
            let result = language_server::hover(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            hover = result.first().map(|item| item.contents.clone());
            format!("Neoism LSP hover returned {} item(s)", result.len())
        }
        EditorLspAction::Definition => {
            let result = language_server::definition(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            locations = result.into_iter().map(map_location).collect();
            format!("Neoism LSP definition returned {} location(s)", locations.len())
        }
        EditorLspAction::References => {
            let result = language_server::references(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            locations = result.into_iter().map(map_location).collect();
            format!("Neoism LSP references returned {} location(s)", locations.len())
        }
        EditorLspAction::Implementation => {
            let result = language_server::implementation(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            locations = result.into_iter().map(map_location).collect();
            format!("Neoism LSP implementation returned {} location(s)", locations.len())
        }
        EditorLspAction::DocumentSymbols => {
            let result = language_server::document_symbols(workspace_root, &buffer.path);
            symbols = flatten_document_symbols(&result, &buffer.path, 0);
            symbol_count = symbols.len();
            format!("Neoism LSP document symbols returned {symbol_count} item(s)")
        }
        EditorLspAction::WorkspaceSymbols => {
            let query = text.unwrap_or_default().trim();
            if query.is_empty() {
                "Neoism LSP workspace symbols need a search query".to_string()
            } else {
                let result = language_server::workspace_symbols(workspace_root, query);
                symbols = result.into_iter().map(map_workspace_symbol).collect();
                symbol_count = symbols.len();
                format!("Neoism LSP workspace symbols returned {symbol_count} item(s)")
            }
        }
        EditorLspAction::Info => format!(
            "Neoism LSP synchronized document for {action:?}: {diagnostic_count} diagnostics, {server_count} servers"
        ),
        EditorLspAction::Format => match apply_formatting(session, workspace_root, &buffer).await {
            Ok(edit_count) => format!("Neoism LSP applied formatting with {edit_count} edit(s)"),
            Err(error) => format!("Neoism LSP formatting unavailable: {error}"),
        },
        EditorLspAction::CodeActions => {
            let results = language_server::code_actions(
                workspace_root,
                &buffer.path,
                buffer.cursor_line,
                buffer.cursor_col,
            );
            code_actions = selectable_code_actions(&buffer.path, &buffer.text, &results);
            format_code_action_summary(&code_actions)
        }
        EditorLspAction::Rename => match apply_rename(session, workspace_root, &buffer, text).await {
            Ok(edit_count) => format!("Neoism LSP applied rename with {edit_count} edit(s)"),
            Err(error) => format!("Neoism LSP rename unavailable: {error}"),
        },
        EditorLspAction::ToggleInlayHints => format!(
            "Neoism LSP owns {action:?}; edit application UI is pending. Refreshed {diagnostic_count} diagnostics across {server_count} servers."
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
        code_actions,
    })
}

/// Flatten the LSP document-symbol tree (depth-first, parents before
/// children) into the wire outline. `depth` drives the picker's display
/// indentation; the jump target is the symbol's `selection_range` start
/// (falling back to its full `range`), so activating a row lands the
/// cursor on the symbol name rather than the enclosing block.
fn flatten_document_symbols(
    symbols: &[language_server::LspDocumentSymbol],
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
            .unwrap_or(language_server::LspPosition {
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

fn format_code_action_summary(actions: &[EditorLspCodeAction]) -> String {
    match actions.len() {
        0 => "Neoism LSP found no code actions".to_string(),
        1 => format!("Neoism LSP found 1 code action: {}", actions[0].title),
        count => format!(
            "Neoism LSP found {count} code actions: {}{}",
            actions
                .iter()
                .take(3)
                .map(|action| action.title.clone())
                .collect::<Vec<_>>()
                .join(", "),
            if count > 3 { ", ..." } else { "" }
        ),
    }
}

const MAX_CODE_ACTIONS: usize = 200;

fn selectable_code_actions(
    file_path: &Path,
    document_text: &str,
    results: &[serde_json::Value],
) -> Vec<EditorLspCodeAction> {
    let mut selectable = Vec::new();
    for result in results {
        let Some(server_id) = result.get("language").and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(actions) = result.get("actions").and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for payload in actions {
            let Some(title) = payload
                .get("title")
                .and_then(serde_json::Value::as_str)
                .filter(|title| !title.trim().is_empty())
            else {
                continue;
            };
            selectable.push(EditorLspCodeAction {
                server_id: server_id.to_string(),
                file_path: file_path.to_path_buf(),
                document_revision: document_revision(document_text),
                title: title.to_string(),
                kind: payload
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                preferred: payload
                    .get("isPreferred")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                disabled_reason: payload
                    .pointer("/disabled/reason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                payload: payload.clone(),
            });
            if selectable.len() == MAX_CODE_ACTIONS {
                return selectable;
            }
        }
    }
    selectable
}

async fn apply_rename(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    buffer: &crate::nvim::BufferText,
    new_name: Option<&str>,
) -> Result<usize, String> {
    let new_name = rename_name(new_name)?;
    let results = language_server::rename(
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
        let edit_count =
            apply_workspace_edits(session, workspace_root, buffer, edits).await?;
        return Ok(edit_count);
    }
    Err("rename returned no applicable workspace edits".to_string())
}

fn rename_name(new_name: Option<&str>) -> Result<&str, String> {
    new_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "missing new name".to_string())
}

pub(crate) async fn run_code_action(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    selected: EditorLspCodeAction,
) -> Result<EditorServerMessage, String> {
    let buffer = read_active_file_buffer(session).await?;
    crate::language_server::flush_document_sync(workspace_root, &buffer.path);
    if !same_file(&buffer.path, &selected.file_path) {
        return Err(format!(
            "code action belongs to `{}`, but the active buffer is `{}`; request fixes again",
            selected.file_path.display(),
            buffer.path.display()
        ));
    }
    if !selected.document_revision.is_empty()
        && selected.document_revision != document_revision(&buffer.text)
    {
        return Err(format!(
            "code action `{}` is stale because the document changed; request fixes again",
            selected.title
        ));
    }
    if let Some(reason) = selected
        .payload
        .pointer("/disabled/reason")
        .and_then(serde_json::Value::as_str)
        .or(selected.disabled_reason.as_deref())
    {
        return Err(format!(
            "code action `{}` is disabled: {reason}",
            selected.title
        ));
    }

    let mut action = selected.payload.clone();
    // A CodeAction can eagerly contain either an edit or command while
    // deferring the other property. Resolve every selected CodeAction when
    // its server advertises resolveProvider; the agent helper is a no-op when
    // it does not. A top-level Command literal is the distinct wire shape
    // `{ title, command: string, arguments? }` and must never be resolved.
    if should_resolve_code_action(&action) {
        action = language_server::resolve_code_action(
            workspace_root,
            &buffer.path,
            &selected.server_id,
            action.clone(),
        )
        .unwrap_or(action);
    }
    if let Some(reason) = action
        .pointer("/disabled/reason")
        .and_then(serde_json::Value::as_str)
    {
        return Err(format!(
            "code action `{}` is disabled: {reason}",
            selected.title
        ));
    }

    let mut edit_count = 0;
    if let Some(edit) = action.get("edit") {
        let edits = workspace_edits(edit)?;
        if !edits.is_empty() {
            edit_count =
                apply_workspace_edits(session, workspace_root, &buffer, edits).await?;
        }
    }

    let command = code_action_command_params(&action)?;
    let command_executed = if let Some(command) = command {
        if language_server::execute_command(
            workspace_root,
            &buffer.path,
            &selected.server_id,
            command,
        )
        .is_none()
        {
            return Err(if edit_count == 0 {
                format!("code action `{}` command failed", selected.title)
            } else {
                format!(
                    "code action `{}` applied {edit_count} edit(s), but its follow-up command failed",
                    selected.title
                )
            });
        }
        true
    } else {
        false
    };

    if edit_count == 0 && !command_executed {
        return Err(format!(
            "code action `{}` returned neither an edit nor a command",
            selected.title
        ));
    }
    let summary = match (edit_count, command_executed) {
        (0, false) => unreachable!("empty action rejected above"),
        (0, true) => format!("Neoism LSP executed code action `{}`", selected.title),
        (count, false) => format!(
            "Neoism LSP applied code action `{}` with {count} edit(s)",
            selected.title
        ),
        (count, true) => format!(
            "Neoism LSP applied code action `{}` with {count} edit(s) and executed its command",
            selected.title
        ),
    };
    Ok(EditorServerMessage::LspActionResult {
        surface_id: None,
        action: EditorLspAction::CodeActions,
        line: buffer.cursor_line,
        character: buffer.cursor_col,
        summary,
        hover: None,
        locations: Vec::new(),
        symbol_count: 0,
        symbols: Vec::new(),
        code_actions: Vec::new(),
    })
}

fn should_resolve_code_action(action: &serde_json::Value) -> bool {
    !action
        .get("command")
        .is_some_and(serde_json::Value::is_string)
}

/// Resolve and accept one completion against the exact document revision that
/// produced its popup. Primary `TextEdit` / `InsertReplaceEdit` and
/// `additionalTextEdits` are applied simultaneously to the active buffer;
/// plain buffer-word fallbacks synthesize the familiar prefix replacement.
pub(crate) async fn run_completion(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    selected: EditorLspCompletionItem,
    replace_prefix: &str,
) -> Result<(), String> {
    let buffer = read_active_file_buffer(session).await?;
    crate::language_server::flush_document_sync(workspace_root, &buffer.path);
    if !selected.file_path.as_os_str().is_empty()
        && !same_file(&buffer.path, &selected.file_path)
    {
        return Err(format!(
            "completion `{}` belongs to `{}`, but the active buffer is `{}`; request completion again",
            selected.label,
            selected.file_path.display(),
            buffer.path.display()
        ));
    }
    if !selected.document_revision.is_empty()
        && selected.document_revision != document_revision(&buffer.text)
    {
        return Err(format!(
            "completion `{}` is stale because the document changed; request completion again",
            selected.label
        ));
    }

    let mut payload = selected.payload.clone().unwrap_or_else(|| {
        serde_json::json!({
            "label": selected.label,
            "insertText": selected.insert_text,
            "insertTextFormat": 1
        })
    });
    if let (Some(server_id), Some(original)) =
        (selected.server_id.as_deref(), selected.payload.clone())
    {
        payload = language_server::resolve_completion(
            workspace_root,
            &buffer.path,
            server_id,
            original,
        )
        .unwrap_or(payload);
    }
    reject_unsupported_completion_modes(&payload)?;

    let marker = completion_cursor_marker(&buffer.text, &payload);
    let mut primary =
        completion_text_edits(&buffer, &selected, replace_prefix, &payload, &marker)?;
    let primary = primary
        .pop()
        .ok_or_else(|| "completion produced no primary text edit".to_string())?;
    let mut edits = Vec::new();
    if let Some(additional) = payload.get("additionalTextEdits") {
        let additional = additional.as_array().ok_or_else(|| {
            "completion additionalTextEdits is not an array".to_string()
        })?;
        edits.extend(non_overlapping_completion_edits(
            &buffer.text,
            &primary,
            additional,
        )?);
    }
    // Keep additional edits before the primary edit in this simultaneous set.
    // If an auto-import and a zero-width primary insertion both target byte 0,
    // the generic reverse-application engine will then produce
    // `import ...\nCompletedSymbol`, matching Zed/VS Code behavior.
    edits.push(primary);
    let mut text = buffer.text.clone();
    apply_lsp_text_edits(&mut text, &edits)?;
    let marker_offset = text.find(&marker).ok_or_else(|| {
        "completion cursor marker was lost while applying text edits".to_string()
    })?;
    if text[marker_offset + marker.len()..].contains(&marker) {
        return Err("completion cursor marker was inserted more than once".to_string());
    }
    text.replace_range(marker_offset..marker_offset + marker.len(), "");
    let (cursor_line, cursor_col) = lsp_position_for_offset(&text, marker_offset)?;
    let applied = session
        .apply_user_text(&buffer.path, &text, cursor_line, cursor_col)
        .await
        .map_err(|error| format!("failed to apply completion: {error}"))?;
    if !applied {
        return Err(format!(
            "completion buffer `{}` is no longer loaded",
            buffer.path.display()
        ));
    }

    // The local on_lines bridge normally delivers this immediately. Queueing
    // the known final text here also establishes an explicit didChange barrier
    // before an optional completion command, with hash dedupe preventing a
    // duplicate version when on_lines follows.
    crate::language_server::sync_document(workspace_root, &buffer.path, text);
    crate::language_server::flush_document_sync(workspace_root, &buffer.path);

    if let Some(command) = completion_command_params(&payload)? {
        let Some(server_id) = selected.server_id.as_deref() else {
            return Err(
                "buffer completion unexpectedly carried an LSP command".to_string()
            );
        };
        if language_server::execute_completion_command(
            workspace_root,
            &buffer.path,
            server_id,
            command,
        )
        .is_none()
        {
            return Err(format!(
                "completion `{}` was inserted, but its follow-up command failed",
                selected.label
            ));
        }
    }
    Ok(())
}

fn reject_unsupported_completion_modes(
    payload: &serde_json::Value,
) -> Result<(), String> {
    if payload
        .get("insertTextFormat")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|format| format == 2)
    {
        return Err(
            "language server returned snippet placeholders although Neoism advertised snippetSupport=false"
                .to_string(),
        );
    }
    if payload
        .get("insertTextMode")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|mode| mode == 2)
    {
        return Err(
            "language server returned adjustIndentation although Neoism did not advertise insertTextMode support"
                .to_string(),
        );
    }
    Ok(())
}

fn completion_text_edits(
    buffer: &crate::nvim::BufferText,
    selected: &EditorLspCompletionItem,
    replace_prefix: &str,
    payload: &serde_json::Value,
    cursor_marker: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let new_text = payload
        .pointer("/textEdit/newText")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            payload
                .get("textEditText")
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            payload
                .get("insertText")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or(&selected.insert_text);
    let marked_text = format!("{new_text}{cursor_marker}");

    let range = if let Some(text_edit) = payload.get("textEdit") {
        text_edit
            // InsertReplaceEdit: Neoism defaults to insert semantics so text
            // after the caret is never destructively replaced.
            .get("insert")
            .or_else(|| text_edit.get("range"))
            .or_else(|| text_edit.get("replace"))
            .cloned()
            .ok_or_else(|| {
                "completion textEdit has no range/insert/replace".to_string()
            })?
    } else {
        completion_prefix_range(buffer, replace_prefix)?
    };
    Ok(vec![serde_json::json!({
        "range": range,
        "newText": marked_text,
    })])
}

/// Keep valid additional edits while ignoring only those that overlap the
/// primary completion range. Some real servers return one bad overlapping
/// edit alongside useful imports; rejecting the entire completion loses both.
/// A zero-width insertion exactly on either primary boundary is legal (the
/// common file-start auto-import shape), but one strictly inside is not.
fn non_overlapping_completion_edits(
    text: &str,
    primary: &serde_json::Value,
    additional: &[serde_json::Value],
) -> Result<Vec<serde_json::Value>, String> {
    let (primary_start, primary_end) = text_edit_offsets(text, primary)?;
    let mut kept = Vec::with_capacity(additional.len());
    for edit in additional {
        let (start, end) = text_edit_offsets(text, edit)?;
        let overlaps = if start == end {
            primary_start < start && start < primary_end
        } else {
            (primary_start <= start && primary_end >= start)
                || (start <= primary_end && end >= primary_end)
        };
        if !overlaps {
            kept.push(edit.clone());
        }
    }
    Ok(kept)
}

fn text_edit_offsets(
    text: &str,
    edit: &serde_json::Value,
) -> Result<(usize, usize), String> {
    let range = edit
        .get("range")
        .ok_or_else(|| "text edit missing range".to_string())?;
    let start = range_position(range, "start")?;
    let end = range_position(range, "end")?;
    let start = offset_for_lsp_position(text, start.0, start.1)?;
    let end = offset_for_lsp_position(text, end.0, end.1)?;
    if start > end {
        return Err("text edit range is reversed".to_string());
    }
    Ok((start, end))
}

fn completion_prefix_range(
    buffer: &crate::nvim::BufferText,
    prefix: &str,
) -> Result<serde_json::Value, String> {
    let line = buffer
        .text
        .lines()
        .nth(buffer.cursor_line as usize)
        .ok_or_else(|| "completion cursor line is past document end".to_string())?;
    let end = buffer.cursor_col as usize;
    let start = end
        .checked_sub(prefix.len())
        .ok_or_else(|| "completion prefix begins before the line".to_string())?;
    if end > line.len() || !line.is_char_boundary(start) || !line.is_char_boundary(end) {
        return Err("completion prefix range is not a valid UTF-8 byte range".to_string());
    }
    if &line[start..end] != prefix {
        return Err("completion prefix no longer matches the active buffer".to_string());
    }
    Ok(serde_json::json!({
        "start": {"line": buffer.cursor_line, "character": start},
        "end": {"line": buffer.cursor_line, "character": end},
    }))
}

fn completion_cursor_marker(text: &str, payload: &serde_json::Value) -> String {
    let payload = payload.to_string();
    for ordinal in 0u32.. {
        let candidate = format!("\u{e000}neoism-completion-{ordinal}\u{e001}");
        if !text.contains(&candidate) && !payload.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("u32 completion marker space exhausted")
}

fn lsp_position_for_offset(text: &str, offset: usize) -> Result<(u32, u32), String> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err("completion cursor offset is not a UTF-8 boundary".to_string());
    }
    let before = &text[..offset];
    let line = u32::try_from(before.bytes().filter(|byte| *byte == b'\n').count())
        .map_err(|_| "completion cursor line is too large".to_string())?;
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let col = u32::try_from(offset - line_start)
        .map_err(|_| "completion cursor column is too large".to_string())?;
    Ok((line, col))
}

fn completion_command_params(
    payload: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let Some(command) = payload.get("command") else {
        return Ok(None);
    };
    let command = command
        .as_object()
        .ok_or_else(|| "completion command is not a Command object".to_string())?;
    let name = command
        .get("command")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| "completion command is missing command".to_string())?;
    let arguments = match command.get("arguments") {
        Some(serde_json::Value::Array(arguments)) => {
            serde_json::Value::Array(arguments.clone())
        }
        Some(_) => {
            return Err("completion command arguments are not an array".to_string())
        }
        None => serde_json::Value::Array(Vec::new()),
    };
    Ok(Some(serde_json::json!({
        "command": name,
        "arguments": arguments,
    })))
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

pub(super) fn document_revision(text: &str) -> String {
    // Stable FNV-1a is enough for an in-process stale-selection guard and does
    // not depend on randomized process hash keys.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Normalize either member of the LSP `CodeAction | Command` union into
/// `ExecuteCommandParams`. A command-only action stores `command` and
/// `arguments` at the top level; a CodeAction nests a Command object.
fn code_action_command_params(
    action: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let Some(command) = action.get("command") else {
        return Ok(None);
    };
    let (name, arguments) = match command {
        serde_json::Value::String(name) => (name.as_str(), action.get("arguments")),
        serde_json::Value::Object(command) => {
            let name = command
                .get("command")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "code action command is missing `command`".to_string())?;
            (name, command.get("arguments"))
        }
        _ => {
            return Err(
                "code action command is not a string or Command object".to_string()
            )
        }
    };
    if name.trim().is_empty() {
        return Err("code action command is empty".to_string());
    }
    let arguments = match arguments {
        Some(serde_json::Value::Array(arguments)) => {
            serde_json::Value::Array(arguments.clone())
        }
        Some(_) => {
            return Err("code action command arguments are not an array".to_string())
        }
        None => serde_json::Value::Array(Vec::new()),
    };
    Ok(Some(serde_json::json!({
        "command": name,
        "arguments": arguments,
    })))
}

/// One simultaneous set of text edits for a document. Separate entries are
/// retained because LSP applies `documentChanges` in array order; flattening
/// them can incorrectly reject a later edit whose coordinates refer to the
/// result of an earlier TextDocumentEdit.
#[derive(Debug, Clone)]
struct DocumentEdit {
    path: PathBuf,
    edits: Vec<serde_json::Value>,
}

fn workspace_edits(edit: &serde_json::Value) -> Result<Vec<DocumentEdit>, String> {
    let mut all = Vec::new();

    if let Some(changes) = edit.get("changes") {
        let changes = changes
            .as_object()
            .ok_or_else(|| "workspace edit `changes` is not an object".to_string())?;
        for (uri, edits) in changes {
            let edits = edits.as_array().ok_or_else(|| {
                format!("workspace edit `changes[{uri}]` is not an edit array")
            })?;
            all.push(DocumentEdit {
                path: path_for_lsp_uri(uri)?,
                edits: edits.clone(),
            });
        }
    }

    if let Some(document_changes) = edit.get("documentChanges") {
        let document_changes = document_changes.as_array().ok_or_else(|| {
            "workspace edit `documentChanges` is not an array".to_string()
        })?;
        for change in document_changes {
            if let Some(kind) = change.get("kind").and_then(serde_json::Value::as_str) {
                return Err(format!(
                    "workspace edit resource operation `{kind}` is not supported"
                ));
            }
            let text_document = change.get("textDocument").ok_or_else(|| {
                "unsupported workspace edit documentChanges entry; only TextDocumentEdit is supported"
                    .to_string()
            })?;
            let uri = text_document
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    "workspace edit TextDocumentEdit missing textDocument.uri".to_string()
                })?;
            if let Some(version) = text_document.get("version") {
                if !version.is_null() && !version.is_i64() && !version.is_u64() {
                    return Err(
                        "workspace edit textDocument.version is not an integer or null"
                            .to_string(),
                    );
                }
            }
            let edits = change
                .get("edits")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    "workspace edit TextDocumentEdit missing edits".to_string()
                })?;
            all.push(DocumentEdit {
                path: path_for_lsp_uri(uri)?,
                edits: edits.clone(),
            });
        }
    }

    Ok(all)
}

fn path_for_lsp_uri(uri: &str) -> Result<PathBuf, String> {
    let parsed = url::Url::parse(uri)
        .map_err(|error| format!("invalid workspace edit uri `{uri}`: {error}"))?;
    if parsed.scheme() != "file" {
        return Err(format!(
            "unsupported workspace edit uri scheme `{}`; only file URIs are supported",
            parsed.scheme()
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!(
            "workspace edit file uri must not contain a query or fragment: `{uri}`"
        ));
    }
    parsed.to_file_path().map_err(|()| {
        format!("workspace edit uri is not a local absolute file path: `{uri}`")
    })
}

#[derive(Debug)]
struct PreparedDocumentEdit {
    /// Canonical path used for containment and non-active file writes.
    path: PathBuf,
    text: String,
    active: bool,
    edit_count: usize,
}

#[derive(Debug)]
struct PreparedWorkspaceEdit {
    documents: BTreeMap<PathBuf, PreparedDocumentEdit>,
    edit_count: usize,
}

fn canonical_workspace_root(workspace_root: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(workspace_root).map_err(|error| {
        format!(
            "failed to resolve workspace root `{}`: {error}",
            workspace_root.display()
        )
    })?;
    if !root.is_dir() {
        return Err(format!(
            "workspace root is not a directory: `{}`",
            workspace_root.display()
        ));
    }
    Ok(root)
}

#[derive(Debug)]
struct ResolvedWorkspacePath {
    path: PathBuf,
    exists: bool,
}

fn resolve_workspace_path(
    root: &Path,
    path: &Path,
) -> Result<ResolvedWorkspacePath, String> {
    if !path.is_absolute() {
        return Err(format!(
            "workspace edit target is not absolute: `{}`",
            path.display()
        ));
    }

    match fs::canonicalize(path) {
        Ok(canonical) => {
            if !canonical.starts_with(root) {
                return Err(format!(
                    "workspace edit target escapes workspace root: `{}`",
                    path.display()
                ));
            }
            if !canonical.is_file() {
                return Err(format!(
                    "workspace edit target is not a regular file: `{}`",
                    path.display()
                ));
            }
            Ok(ResolvedWorkspacePath {
                path: canonical,
                exists: true,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A file-backed nvim buffer may be new and not saved yet. Resolve
            // its existing parent so symlinked parents and `..` components
            // cannot smuggle the candidate outside the workspace.
            let parent = path.parent().ok_or_else(|| {
                format!("workspace edit target has no parent: `{}`", path.display())
            })?;
            let file_name = path.file_name().ok_or_else(|| {
                format!(
                    "workspace edit target has no file name: `{}`",
                    path.display()
                )
            })?;
            let canonical_parent = fs::canonicalize(parent).map_err(|parent_error| {
                format!(
                    "failed to resolve workspace edit target `{}`: {parent_error}",
                    path.display()
                )
            })?;
            if !canonical_parent.is_dir() || !canonical_parent.starts_with(root) {
                return Err(format!(
                    "workspace edit target escapes workspace root: `{}`",
                    path.display()
                ));
            }
            Ok(ResolvedWorkspacePath {
                path: canonical_parent.join(file_name),
                exists: false,
            })
        }
        Err(error) => Err(format!(
            "failed to resolve workspace edit target `{}`: {error}",
            path.display()
        )),
    }
}

/// Resolve every target, load every source text and simulate every edit before
/// returning. The caller can therefore commit without discovering a malformed
/// range in a later document after an earlier document was already changed.
fn prepare_workspace_edits(
    workspace_root: &Path,
    buffer: &crate::nvim::BufferText,
    document_edits: Vec<DocumentEdit>,
) -> Result<PreparedWorkspaceEdit, String> {
    let root = canonical_workspace_root(workspace_root)?;
    let active_path = resolve_workspace_path(&root, &buffer.path).map_err(|error| {
        format!("active buffer is outside the workspace or unavailable: {error}")
    })?;
    let mut documents: BTreeMap<PathBuf, PreparedDocumentEdit> = BTreeMap::new();
    let mut edit_count = 0usize;

    for document_edit in document_edits {
        let target = resolve_workspace_path(&root, &document_edit.path)?;
        let active = target.path == active_path.path;
        if !target.exists && !active {
            return Err(format!(
                "workspace edit target does not exist and is not the active buffer: `{}`",
                document_edit.path.display()
            ));
        }
        let prepared = match documents.entry(target.path.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let text = if active {
                    buffer.text.clone()
                } else {
                    fs::read_to_string(&target.path).map_err(|error| {
                        format!("failed to read `{}`: {error}", target.path.display())
                    })?
                };
                entry.insert(PreparedDocumentEdit {
                    path: target.path,
                    text,
                    active,
                    edit_count: 0,
                })
            }
        };

        // Each DocumentEdit is simultaneous internally. A subsequent entry
        // for the same document is applied to this result in protocol order.
        apply_lsp_text_edits(&mut prepared.text, &document_edit.edits)?;
        prepared.edit_count = prepared
            .edit_count
            .checked_add(document_edit.edits.len())
            .ok_or_else(|| "workspace edit count overflow".to_string())?;
        edit_count = edit_count
            .checked_add(document_edit.edits.len())
            .ok_or_else(|| "workspace edit count overflow".to_string())?;
    }

    Ok(PreparedWorkspaceEdit {
        documents,
        edit_count,
    })
}

async fn apply_workspace_edits(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    buffer: &crate::nvim::BufferText,
    document_edits: Vec<DocumentEdit>,
) -> Result<usize, String> {
    let prepared = prepare_workspace_edits(workspace_root, buffer, document_edits)?;

    for document in prepared.documents.values() {
        if document.edit_count == 0 {
            continue;
        }
        if document.active {
            session
                // Use the buffer identity that nvim reported, even when the
                // server used a different canonical URI for the same file.
                .apply_authoritative_text(&buffer.path, &document.text)
                .await
                .map_err(|error| {
                    format!("failed to apply active buffer text: {error}")
                })?;
        } else {
            fs::write(&document.path, &document.text).map_err(|error| {
                format!("failed to write `{}`: {error}", document.path.display())
            })?;
        }
    }
    Ok(prepared.edit_count)
}

async fn apply_formatting(
    session: &NvimSessionHandle,
    workspace_root: &Path,
    buffer: &crate::nvim::BufferText,
) -> Result<usize, String> {
    let edits = language_server::formatting(workspace_root, &buffer.path);
    if edits.is_empty() {
        return Ok(0);
    }
    let root = canonical_workspace_root(workspace_root)?;
    resolve_workspace_path(&root, &buffer.path).map_err(|error| {
        format!("active buffer is outside the workspace or unavailable: {error}")
    })?;
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
    for (ordinal, edit) in edits.iter().enumerate() {
        let range = edit
            .get("range")
            .ok_or_else(|| "text edit missing range".to_string())?;
        let start = range_position(range, "start")?;
        let end = range_position(range, "end")?;
        let replacement = edit
            .get("newText")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "text edit missing newText".to_string())?
            .to_string();
        let start_offset = offset_for_lsp_position(text, start.0, start.1)?;
        let end_offset = offset_for_lsp_position(text, end.0, end.1)?;
        if start_offset > end_offset {
            return Err("text edit range is reversed".to_string());
        }
        replacements.push((start_offset, end_offset, ordinal, replacement));
    }

    // Validate the complete simultaneous edit set before changing even this
    // in-memory document. Touching ranges are legal, as are multiple inserts
    // at one position; a point strictly inside a replacement is not.
    let mut ranges = replacements
        .iter()
        .map(|(start, end, _, _)| (*start, *end))
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    let mut last_start = text.len();
    for &(start, end) in ranges.iter().rev() {
        if end > last_start {
            return Err("text edits overlap".to_string());
        }
        last_start = start;
    }

    // Work from the end so earlier offsets remain valid. For same-position
    // inserts, apply later array entries first so the final text preserves the
    // server's declared array order.
    replacements
        .sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.2.cmp(&left.2)));
    for (start, end, _, replacement) in replacements {
        text.replace_range(start..end, &replacement);
    }
    Ok(())
}

fn range_position(range: &serde_json::Value, key: &str) -> Result<(u32, u32), String> {
    let position = range
        .get(key)
        .ok_or_else(|| format!("text edit missing {key}"))?;
    let line = position
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("text edit {key} missing line"))?;
    let character = position
        .get("character")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("text edit {key} missing character"))?;
    let line =
        u32::try_from(line).map_err(|_| format!("text edit {key} line is too large"))?;
    let character = u32::try_from(character)
        .map_err(|_| format!("text edit {key} character is too large"))?;
    Ok((line, character))
}

fn offset_for_lsp_position(
    text: &str,
    line: u32,
    character: u32,
) -> Result<usize, String> {
    let mut line_number = 0u32;
    let mut line_start = 0usize;
    loop {
        let remaining = &text[line_start..];
        let newline = remaining.find('\n');
        let raw_line_end = newline
            .map(|relative| line_start + relative)
            .unwrap_or(text.len());
        // In CRLF text, neither terminator byte belongs to the LSP line.
        // A lone final CR remains ordinary document content.
        let line_end = if newline.is_some()
            && raw_line_end > line_start
            && text.as_bytes()[raw_line_end - 1] == b'\r'
        {
            raw_line_end - 1
        } else {
            raw_line_end
        };

        if line_number == line {
            let character = character as usize;
            let line_text = &text[line_start..line_end];
            if character > line_text.len() {
                return Err("text edit character is past line end".to_string());
            }
            if !line_text.is_char_boundary(character) {
                return Err("text edit character splits utf-8 codepoint".to_string());
            }
            return Ok(line_start + character);
        }

        let Some(_) = newline else {
            return Err("text edit line is past document end".to_string());
        };
        line_start = raw_line_end + 1;
        line_number = line_number
            .checked_add(1)
            .ok_or_else(|| "text edit line count overflow".to_string())?;
    }
}

#[cfg(test)]
mod tests;

fn map_workspace_symbol(symbol: language_server::WorkspaceSymbol) -> EditorLspSymbol {
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

fn map_location(location: language_server::LspLocation) -> EditorLspLocation {
    let start =
        location
            .range
            .map(|range| range.start)
            .unwrap_or(language_server::LspPosition {
                line: 0,
                character: 0,
            });
    EditorLspLocation {
        uri: location.path,
        line: start.line,
        character: start.character,
    }
}
