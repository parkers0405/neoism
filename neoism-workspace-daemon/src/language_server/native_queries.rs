//! Position-explicit LSP queries for the native editor
//! (`EditorClientMessage::LspQueryAt` / `ApplyLspCodeActionAt`).
//!
//! The nvim-era `LspAction`/`LspComplete` paths resolved the cursor from
//! the embedded nvim session; the native editor owns its buffer and
//! sends the position with every request instead. Coordinate contract:
//! request positions are 0-based line + 0-based UTF-8 BYTE column (the
//! engine facade's input contract); the engine's 1-based display
//! OUTPUTS are normalized back to 0-based here before shipping, so
//! clients never see the desktop's historical off-by-one.
//!
//! Edit-shaped results (rename / format / applied code action) follow
//! the desktop split: typed edits are returned for the request's
//! `open_paths` (the client applies them to its live buffers); every
//! other touched file is patched directly on disk here, since the
//! daemon owns the workspace files.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn document_revision(runtime: &engine::LspRuntime, root: &Path, file: &Path) -> String {
    let text = super::active_buffer::live_buffer_text(runtime, root, file)
        .or_else(|| std::fs::read_to_string(file).ok())
        .unwrap_or_default();
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

use neoism_agent_server::language_server as engine;
use neoism_protocol::editor::{
    EditorLspAction, EditorLspCodeAction, EditorLspCompletionItem, EditorLspFileEdit,
    EditorLspLocation, EditorLspReference, EditorLspTextEdit, EditorServerMessage,
};

/// Serve one `LspQueryAt`. Blocking — call from `spawn_blocking`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn query_at(
    runtime: &engine::LspRuntime,
    root: &Path,
    seq: u64,
    action: EditorLspAction,
    path: &Path,
    line: u32,
    character: u32,
    text: Option<&str>,
    buffer_text: Option<&str>,
    open_paths: &[PathBuf],
    surface_id: Option<String>,
) -> EditorServerMessage {
    let file = match scoped_file(root, path) {
        Ok(file) => file,
        Err(message) => {
            return EditorServerMessage::Error {
                surface_id,
                message,
            }
        }
    };
    if let Some(text) = buffer_text {
        super::active_buffer::queue_buffer_sync(runtime, root, &file, text.to_string());
    }
    // Wait for every already-queued buffer sync for this document
    // (`queue_buffer_sync` runs inline in socket order) so the engine
    // resolves this position against the client's live text, never a
    // stale revision — the FIFO guarantee the desktop worker gives its
    // Sync-before-query jobs.
    super::live_sync::flush_document_sync(runtime, root, &file);
    let status = engine::status(runtime, root, Some(&file));
    if !status.is_empty()
        && status
            .iter()
            .all(|s| s.status == engine::LspServerState::Error)
    {
        return EditorServerMessage::Error {
            surface_id,
            message: status
                .iter()
                .map(|s| {
                    format!(
                        "{}: {}",
                        s.name,
                        s.detected
                            .message
                            .as_deref()
                            .unwrap_or("language server failed")
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        };
    }
    let supported = status.iter().any(|s| match action {
        EditorLspAction::Hover => s.capabilities.hover,
        EditorLspAction::Definition => s.capabilities.definition,
        EditorLspAction::References => s.capabilities.references,
        EditorLspAction::DocumentSymbols => s.capabilities.document_symbols,
        EditorLspAction::Format => s.capabilities.formatting,
        EditorLspAction::CodeActions => s.capabilities.code_actions,
        EditorLspAction::Rename => s.capabilities.rename,
        _ => true,
    });
    if !supported && !matches!(action, EditorLspAction::Completion) {
        return EditorServerMessage::Error {
            surface_id,
            message: format!(
                "No workspace language server supports {action:?} for {}",
                file.display()
            ),
        };
    }
    match action {
        EditorLspAction::Hover => {
            let hovers = engine::hover(runtime, root, &file, line, character);
            let mut contents = String::new();
            for hover in hovers {
                if hover.contents.trim().is_empty() {
                    continue;
                }
                if !contents.is_empty() {
                    contents.push_str("\n\n");
                }
                contents.push_str(&hover.contents);
            }
            EditorServerMessage::LspHoverResult {
                surface_id,
                seq,
                line,
                character,
                contents,
            }
        }
        EditorLspAction::SignatureHelp => {
            // One synthetic hover carrying the active signature +
            // parameter — rides the hover surface so the card,
            // dismissal and rendering are shared (desktop parity).
            let contents = engine::signature_help(runtime, root, &file, line, character)
                .into_iter()
                .next()
                .and_then(|help| {
                    let count = help.signatures.len();
                    let active = (help.active_signature.unwrap_or(0) as usize)
                        .min(count.checked_sub(1)?);
                    let sig = &help.signatures[active];
                    let mut contents = sig.label.clone();
                    let param_ix =
                        sig.active_parameter.or(help.active_parameter).unwrap_or(0)
                            as usize;
                    if let Some(param) = sig.parameters.get(param_ix) {
                        contents.push_str("\n\u{25b8} ");
                        contents.push_str(&param.label);
                        if let Some(doc) = param
                            .documentation
                            .as_deref()
                            .and_then(|doc| doc.lines().next())
                        {
                            contents.push_str(" \u{2014} ");
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
                    Some(contents)
                })
                .unwrap_or_default();
            EditorServerMessage::LspHoverResult {
                surface_id,
                seq,
                line,
                character,
                contents,
            }
        }
        EditorLspAction::Completion => {
            let live_text = super::active_buffer::live_buffer_text(runtime, root, &file);
            let items = engine::completion_with_trigger(
                runtime,
                root,
                &file,
                line,
                character,
                live_text.as_deref(),
                text,
            );
            let items = items
                .into_iter()
                .map(|item| EditorLspCompletionItem {
                    server_id: item.server_id,
                    file_path: file.clone(),
                    document_revision: document_revision(runtime, root, &file),
                    label: item.label,
                    kind: item.kind,
                    detail: item.detail,
                    documentation: item.documentation,
                    insert_text: item.insert_text,
                    filter_text: item.filter_text,
                    sort_text: item.sort_text,
                    preselect: item.preselect,
                    payload: Some(item.payload),
                })
                .collect();
            EditorServerMessage::LspCompletions {
                surface_id,
                seq,
                replace_prefix: String::new(),
                items,
            }
        }
        EditorLspAction::Definition => {
            let mut locations = Vec::new();
            for location in engine::definition(runtime, root, &file, line, character) {
                let Some(range) = location.range else {
                    continue;
                };
                let target = match host_lsp_target(root, &location.path)
                    .and_then(|path| scoped_file(root, &path))
                {
                    Ok(path) => path,
                    Err(message) => {
                        return EditorServerMessage::Error {
                            surface_id,
                            message,
                        }
                    }
                };
                let target = target.to_string_lossy().into_owned();
                locations.push(EditorLspLocation {
                    uri: target.clone(),
                    host_path: Some(target),
                    line: range.start.line.saturating_sub(1),
                    character: range.start.character.saturating_sub(1),
                });
            }
            query_result(
                surface_id,
                seq,
                action,
                root,
                QueryResultBody {
                    locations,
                    ..Default::default()
                },
            )
        }
        EditorLspAction::References => {
            let locations = engine::references(runtime, root, &file, line, character);
            // Ready-made reference rows: read each hit's line text
            // (live buffer first, then disk), path relative to the
            // workspace root — desktop worker parity.
            let mut file_lines: std::collections::HashMap<PathBuf, Vec<String>> =
                std::collections::HashMap::new();
            let mut references: Vec<EditorLspReference> = Vec::new();
            for location in &locations {
                let Ok(hit_path) = host_lsp_target(root, &location.path)
                    .and_then(|path| scoped_file(root, &path))
                else {
                    continue;
                };
                let (line1, col1) = location
                    .range
                    .as_ref()
                    .map(|range| (range.start.line, range.start.character))
                    .unwrap_or((1, 1));
                let line0 = line1.saturating_sub(1) as usize;
                let text = file_lines
                    .entry(hit_path.clone())
                    .or_insert_with(|| {
                        super::active_buffer::live_buffer_text(runtime, root, &hit_path)
                            .or_else(|| std::fs::read_to_string(&hit_path).ok())
                            .map(|text| text.lines().map(str::to_string).collect())
                            .unwrap_or_default()
                    })
                    .get(line0)
                    .cloned()
                    .unwrap_or_default();
                let rel = hit_path
                    .strip_prefix(root)
                    .unwrap_or(&hit_path)
                    .display()
                    .to_string();
                references.push(EditorLspReference {
                    path: rel,
                    line: line0 as u32 + 1,
                    column: col1.saturating_sub(1),
                    text: text.trim().to_string(),
                });
            }
            references.sort_by(|a, b| {
                a.path
                    .cmp(&b.path)
                    .then(a.line.cmp(&b.line))
                    .then(a.column.cmp(&b.column))
            });
            references.dedup_by(|a, b| {
                a.path == b.path && a.line == b.line && a.column == b.column
            });
            query_result(
                surface_id,
                seq,
                action,
                root,
                QueryResultBody {
                    references,
                    ..Default::default()
                },
            )
        }
        EditorLspAction::CodeActions => {
            let groups = engine::code_actions(runtime, root, &file, line, character);
            let mut code_actions = Vec::new();
            for group in &groups {
                let server_id = group
                    .get("language")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let Some(actions) = group.get("actions").and_then(|a| a.as_array())
                else {
                    continue;
                };
                for raw in actions {
                    let Some(title) = raw.get("title").and_then(|t| t.as_str()) else {
                        continue;
                    };
                    code_actions.push(EditorLspCodeAction {
                        server_id: server_id.clone(),
                        file_path: file.clone(),
                        document_revision: document_revision(runtime, root, &file),
                        title: title.to_string(),
                        kind: raw
                            .get("kind")
                            .and_then(|k| k.as_str())
                            .map(str::to_string),
                        preferred: raw
                            .get("isPreferred")
                            .and_then(|p| p.as_bool())
                            .unwrap_or(false),
                        disabled_reason: raw
                            .pointer("/disabled/reason")
                            .and_then(|r| r.as_str())
                            .map(str::to_string),
                        payload: raw.clone(),
                    });
                }
            }
            // Preferred actions bubble to the top (server hint),
            // otherwise server order is kept — desktop parity.
            code_actions.sort_by_key(|action| std::cmp::Reverse(action.preferred));
            query_result(
                surface_id,
                seq,
                action,
                root,
                QueryResultBody {
                    code_actions,
                    ..Default::default()
                },
            )
        }
        EditorLspAction::Rename => {
            let new_name = text.unwrap_or_default().trim();
            if new_name.is_empty() {
                return EditorServerMessage::Error {
                    surface_id,
                    message: "rename needs a new name".to_string(),
                };
            }
            let groups = engine::rename(runtime, root, &file, line, character, new_name);
            // Per-server groups; the first with a real edit wins so a
            // multi-server file can't double-apply (desktop parity).
            let edit = groups.iter().find_map(|group| {
                group.get("edit").filter(|edit| !edit.is_null()).cloned()
            });
            let body = match edit {
                Some(edit) => match split_workspace_edit(root, &edit, open_paths) {
                    Ok(body) => body,
                    Err(message) => {
                        return EditorServerMessage::Error {
                            surface_id,
                            message,
                        }
                    }
                },
                None => QueryResultBody::default(),
            };
            query_result(
                surface_id,
                seq,
                action,
                root,
                QueryResultBody {
                    title: "Rename".to_string(),
                    ..body
                },
            )
        }
        EditorLspAction::Format => {
            let edits = engine::formatting(runtime, root, &file);
            let edits =
                neoism_ui::editor::code::lsp_session::formatting_text_edits(&edits);
            let typed = typed_edits(&edits);
            query_result(
                surface_id,
                seq,
                action,
                root,
                QueryResultBody {
                    edits: if typed.is_empty() {
                        Vec::new()
                    } else {
                        vec![EditorLspFileEdit {
                            path: file.clone(),
                            edits: typed,
                        }]
                    },
                    title: "Format".to_string(),
                    ..Default::default()
                },
            )
        }
        EditorLspAction::DocumentSymbols => {
            fn flatten(
                out: &mut Vec<neoism_protocol::editor::EditorLspSymbol>,
                file: &Path,
                nodes: &[engine::LspDocumentSymbol],
                depth: u32,
            ) {
                for node in nodes {
                    let pos = node
                        .selection_range
                        .as_ref()
                        .or(node.range.as_ref())
                        .map(|r| (r.start.line, r.start.character))
                        .unwrap_or((0, 0));
                    out.push(neoism_protocol::editor::EditorLspSymbol {
                        name: node.name.clone(),
                        kind: node.kind.to_lowercase(),
                        detail: None,
                        uri: file.to_string_lossy().into_owned(),
                        line: pos.0,
                        character: pos.1,
                        depth,
                    });
                    flatten(out, file, &node.children, depth + 1);
                }
            }
            let mut symbols = Vec::new();
            flatten(
                &mut symbols,
                &file,
                &engine::document_symbols(runtime, root, &file),
                0,
            );
            query_result(
                surface_id,
                seq,
                action,
                root,
                QueryResultBody {
                    symbols,
                    ..Default::default()
                },
            )
        }
        EditorLspAction::DocumentHighlight => {
            let highlights =
                engine::document_highlight(runtime, root, &file, line, character)
                    .into_iter()
                    .filter_map(|h| {
                        let r = h.range?;
                        (r.start.line == r.end.line).then_some((
                            r.start.line.saturating_sub(1),
                            r.start.character.saturating_sub(1),
                            r.end.character.saturating_sub(1),
                        ))
                    })
                    .collect();
            query_result(
                surface_id,
                seq,
                action,
                root,
                QueryResultBody {
                    highlights,
                    ..Default::default()
                },
            )
        }
        // Implementation / DocumentSymbols / WorkspaceSymbols / Info /
        // ToggleInlayHints keep their legacy paths (or are not served
        // position-explicitly yet).
        other => EditorServerMessage::Error {
            surface_id,
            message: format!("LspQueryAt does not serve {other:?} yet"),
        },
    }
}

/// Serve one `ApplyLspCodeActionAt`: bare Command payloads go straight
/// to `workspace/executeCommand`; full actions resolve when they carry
/// no inline edit, run their `command` when present, and the workspace
/// edit is split between typed client edits (open files) and on-disk
/// patches. Blocking — call from `spawn_blocking`.
pub(crate) fn apply_code_action_at(
    runtime: &engine::LspRuntime,
    root: &Path,
    seq: u64,
    selected: EditorLspCodeAction,
    open_paths: &[PathBuf],
    surface_id: Option<String>,
) -> EditorServerMessage {
    let file = match scoped_file(root, &selected.file_path) {
        Ok(file) => file,
        Err(message) => {
            return EditorServerMessage::Error {
                surface_id,
                message,
            }
        }
    };
    super::live_sync::flush_document_sync(runtime, root, &file);
    if !selected.document_revision.is_empty()
        && selected.document_revision != document_revision(runtime, root, &file)
    {
        return EditorServerMessage::Error {
            surface_id,
            message: "Code action is stale; request fresh actions after editing".into(),
        };
    }
    if let Some(reason) = selected.disabled_reason {
        return EditorServerMessage::Error {
            surface_id,
            message: format!("Code action unavailable: {reason}"),
        };
    }
    let server_id = selected.server_id;
    let title = selected.title;
    let action = selected.payload;
    let is_bare_command = action.get("command").is_some_and(|c| c.is_string());
    let (edit, ran_command) = if is_bare_command {
        if engine::execute_command(runtime, root, &file, &server_id, action).is_none() {
            return EditorServerMessage::Error {
                surface_id,
                message: format!("Host LSP command failed: {title}"),
            };
        }
        (None, true)
    } else {
        let mut action = action;
        // No edit inline → codeAction/resolve fills it in
        // (rust-analyzer style).
        if action.get("edit").is_none() {
            if let Some(resolved) = engine::resolve_code_action(
                runtime,
                root,
                &file,
                &server_id,
                action.clone(),
            ) {
                action = resolved;
            }
        }
        let edit = action.get("edit").filter(|edit| !edit.is_null()).cloned();
        let ran_command = match action.get("command") {
            Some(command) if !command.is_null() => {
                if engine::execute_command(
                    runtime,
                    root,
                    &file,
                    &server_id,
                    command.clone(),
                )
                .is_none()
                {
                    return EditorServerMessage::Error {
                        surface_id,
                        message: format!("Host LSP command failed: {title}"),
                    };
                }
                true
            }
            _ => false,
        };
        (edit, ran_command)
    };
    let body = match edit {
        Some(edit) => match split_workspace_edit(root, &edit, open_paths) {
            Ok(body) => body,
            Err(message) => {
                return EditorServerMessage::Error {
                    surface_id,
                    message,
                }
            }
        },
        None => QueryResultBody::default(),
    };
    query_result(
        surface_id,
        seq,
        EditorLspAction::CodeActions,
        root,
        QueryResultBody {
            ran_command,
            title,
            ..body
        },
    )
}

#[derive(Default)]
struct QueryResultBody {
    symbols: Vec<neoism_protocol::editor::EditorLspSymbol>,
    highlights: Vec<(u32, u32, u32)>,
    locations: Vec<EditorLspLocation>,
    references: Vec<EditorLspReference>,
    code_actions: Vec<EditorLspCodeAction>,
    edits: Vec<EditorLspFileEdit>,
    applied_files: Vec<PathBuf>,
    ran_command: bool,
    title: String,
}

fn query_result(
    surface_id: Option<String>,
    seq: u64,
    action: EditorLspAction,
    root: &Path,
    body: QueryResultBody,
) -> EditorServerMessage {
    EditorServerMessage::LspQueryResult {
        surface_id,
        seq,
        action,
        root: Some(root.to_path_buf()),
        symbols: body.symbols,
        highlights: body.highlights,
        locations: body.locations,
        references: body.references,
        code_actions: body.code_actions,
        edits: body.edits,
        applied_files: body.applied_files,
        ran_command: body.ran_command,
        title: body.title,
    }
}

fn host_lsp_target(root: &Path, value: &str) -> Result<PathBuf, String> {
    let root = neoism_protocol::host_path::HostPath::new(root.to_string_lossy());
    neoism_protocol::editor::decode_host_lsp_path(&root, value)
        .map(|path| PathBuf::from(path.as_str()))
        .ok_or_else(|| format!("Unsupported host LSP file URI: {value}"))
}

/// Unlike the legacy local-native helper, this uses the declared HOST style
/// for drive/UNC URIs and reports undecodable targets instead of dropping edits.
fn host_workspace_edit_targets(
    root: &Path,
    edit: &Value,
) -> Result<Vec<(PathBuf, Vec<Value>)>, String> {
    let mut targets: Vec<(PathBuf, Vec<Value>)> = Vec::new();
    let mut add = |uri: &str, edits: &Value| -> Result<(), String> {
        let path = host_lsp_target(root, uri)?;
        let edits = edits
            .as_array()
            .ok_or_else(|| "Malformed LSP workspace edits".to_string())?;
        if let Some((_, existing)) = targets
            .iter_mut()
            .find(|(p, _)| p.as_os_str() == path.as_os_str())
        {
            existing.extend(edits.iter().cloned());
        } else {
            targets.push((path, edits.clone()));
        }
        Ok(())
    };
    if let Some(changes) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            add(uri, edits)?;
        }
    }
    if let Some(changes) = edit.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            if let Some(uri) = change.pointer("/textDocument/uri").and_then(Value::as_str)
            {
                add(
                    uri,
                    change
                        .get("edits")
                        .ok_or_else(|| "Missing LSP document edits".to_string())?,
                )?;
            }
        }
    }
    Ok(targets)
}

/// Resolve on the HOST only. No suffix matching or guest-OS interpretation.
pub(crate) fn scoped_file(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let file = crate::path::canonicalize(&joined).map_err(|error| {
        format!("LSP path cannot be resolved: {}: {error}", joined.display())
    })?;
    if !file.starts_with(root) {
        return Err(format!(
            "LSP path is outside workspace root: {}",
            file.display()
        ));
    }
    Ok(file)
}

/// Parse raw LSP text edits (already at the engine's 0-based
/// byte-coordinate boundary) into wire edits.
fn typed_edits(edits: &[serde_json::Value]) -> Vec<EditorLspTextEdit> {
    edits
        .iter()
        .filter_map(|edit| {
            Some(EditorLspTextEdit {
                start_line: edit.pointer("/range/start/line")?.as_u64()? as u32,
                start_col: edit.pointer("/range/start/character")?.as_u64()? as u32,
                end_line: edit.pointer("/range/end/line")?.as_u64()? as u32,
                end_col: edit.pointer("/range/end/character")?.as_u64()? as u32,
                new_text: edit.get("newText")?.as_str()?.to_string(),
            })
        })
        .collect()
}

/// Split a WorkspaceEdit between typed client edits (files in
/// `open_paths`) and on-disk patches (everything else) — the client
/// owns its live buffers, the daemon owns the files.
fn split_workspace_edit(
    root: &Path,
    edit: &serde_json::Value,
    open_paths: &[PathBuf],
) -> Result<QueryResultBody, String> {
    if edit
        .get("documentChanges")
        .and_then(|v| v.as_array())
        .is_some_and(|changes| changes.iter().any(|change| change.get("kind").is_some()))
    {
        return Err("LSP file create/delete/rename operations are not supported by the native text-edit pipeline".into());
    }
    let open_paths = open_paths
        .iter()
        .filter_map(|path| scoped_file(root, path).ok())
        .collect::<std::collections::HashSet<_>>();
    let per_file = host_workspace_edit_targets(root, edit)?;
    let mut body = QueryResultBody::default();
    let is_open = |path: &Path| {
        open_paths.iter().any(|open| {
            open == path
                || open.canonicalize().ok().as_deref() == Some(path)
                || path.canonicalize().ok().as_deref() == Some(open)
        })
    };
    // Validate every target before any disk write (including symlink escapes).
    let per_file = per_file
        .into_iter()
        .map(|(path, edits)| scoped_file(root, &path).map(|path| (path, edits)))
        .collect::<Result<Vec<_>, _>>()?;
    for (path, raw_edits) in per_file {
        let typed = typed_edits(&raw_edits);
        if typed.len() != raw_edits.len() {
            return Err("Malformed LSP text edit".into());
        }
        if typed.is_empty() {
            continue;
        }
        if is_open(&path) {
            body.edits.push(EditorLspFileEdit { path, edits: typed });
        } else {
            match apply_edits_on_disk(&path, &typed) {
                Ok(()) => body.applied_files.push(path),
                Err(error) => {
                    return Err(format!(
                        "Failed to apply LSP edit to {}: {error}",
                        path.display()
                    ))
                }
            }
        }
    }
    Ok(body)
}

fn floor_char_boundary_of(text: &str, mut ix: usize) -> usize {
    ix = ix.min(text.len());
    while ix > 0 && !text.is_char_boundary(ix) {
        ix -= 1;
    }
    ix
}

/// Read-patch-write for files without an open buffer: apply byte-coord
/// LSP edits bottom-up (mirrors `CodeBuffer::apply_text_edits`) and
/// preserve the file's newline flavor / trailing newline. Port of the
/// desktop bridge's `apply_edits_on_disk`.
fn apply_edits_on_disk(path: &Path, edits: &[EditorLspTextEdit]) -> std::io::Result<()> {
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
    for edit in edits {
        let start = (edit.start_line as usize, edit.start_col as usize);
        let end = (edit.end_line as usize, edit.end_col as usize);
        if start > end
            || lines
                .get(start.0)
                .is_none_or(|line| !line.is_char_boundary(start.1))
            || lines
                .get(end.0)
                .is_none_or(|line| !line.is_char_boundary(end.1))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid LSP edit range",
            ));
        }
    }
    let mut sorted: Vec<&EditorLspTextEdit> = edits.iter().collect();
    sorted.sort_by(|a, b| (b.start_line, b.start_col).cmp(&(a.start_line, a.start_col)));
    for edit in sorted {
        let last = lines.len().saturating_sub(1);
        let sl = (edit.start_line as usize).min(last);
        let el = (edit.end_line as usize).min(last).max(sl);
        let sc = floor_char_boundary_of(&lines[sl], edit.start_col as usize);
        let ec = floor_char_boundary_of(&lines[el], edit.end_col as usize);
        let head = lines[sl][..sc].to_string();
        let tail = lines[el][ec..].to_string();
        let replacement = format!("{head}{}{tail}", edit.new_text.replace('\r', ""));
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_lsp_workspace_targets_decode_host_uris_not_guest_paths() {
        for (root, uri, expected) in [
            (
                "/host/work",
                "file:///host/work/space%20%E9%A1%B9%E7%9B%AE/%2520.rs",
                "/host/work/space 项目/%20.rs",
            ),
            (
                r"C:\Work",
                "file:///C:/Work/space%20%E9%A1%B9%E7%9B%AE.rs",
                r"C:\Work\space 项目.rs",
            ),
            (
                r"\\Server\Share",
                "file://Server/Share/space%20%E9%A1%B9%E7%9B%AE.rs",
                r"\\Server\Share\space 项目.rs",
            ),
        ] {
            let edit = serde_json::json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"new"});
            for payload in [
                serde_json::json!({"changes":{uri:[edit.clone()]}}),
                serde_json::json!({"documentChanges":[{"textDocument":{"uri":uri,"version":1},"edits":[edit.clone()]}]}),
            ] {
                let targets =
                    host_workspace_edit_targets(Path::new(root), &payload).unwrap();
                assert_eq!(targets[0].0.as_os_str(), std::ffi::OsStr::new(expected));
                assert_eq!(targets[0].1, vec![edit.clone()]);
            }
        }
        assert!(host_workspace_edit_targets(
            Path::new("/host"),
            &serde_json::json!({"changes":{"file:///host/%XX":[]}})
        )
        .is_err());
    }

    #[test]
    fn native_lsp_workspace_edit_rejects_outside_targets_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let root = crate::path::canonicalize(&root).unwrap();
        let inside = root.join("inside.txt");
        let outside = temp.path().join("outside.txt");
        std::fs::write(&inside, "old").unwrap();
        std::fs::write(&outside, "old").unwrap();
        let edit = |path: &Path| {
            (
                url::Url::from_file_path(path).unwrap().to_string(),
                serde_json::json!([{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"new"}]),
            )
        };
        let changes = [edit(&inside), edit(&outside)]
            .into_iter()
            .collect::<serde_json::Map<_, _>>();
        let result =
            split_workspace_edit(&root, &serde_json::json!({"changes":changes}), &[]);
        assert!(
            matches!(result, Err(ref message) if message.contains("outside workspace"))
        );
        assert_eq!(std::fs::read_to_string(inside).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "old");
    }
    #[test]
    fn native_lsp_disk_edit_rejects_invalid_utf8_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("unicode.txt");
        std::fs::write(&file, "évalue").unwrap();
        assert!(apply_edits_on_disk(
            &file,
            &[EditorLspTextEdit {
                start_line: 0,
                start_col: 1,
                end_line: 0,
                end_col: 2,
                new_text: "bad".into()
            }]
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "évalue");
    }
    #[cfg(unix)]
    #[test]
    fn native_lsp_query_rejects_symlink_escape() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let outside = temp.path().join("secret");
        std::fs::write(&outside, "private").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert!(scoped_file(&root, Path::new("link"))
            .unwrap_err()
            .contains("outside workspace"));
    }
}
