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

use std::path::{Path, PathBuf};

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
    open_paths: &[PathBuf],
    surface_id: Option<String>,
) -> EditorServerMessage {
    let file = resolve_file(root, path);
    // Wait for every already-queued buffer sync for this document
    // (`queue_buffer_sync` runs inline in socket order) so the engine
    // resolves this position against the client's live text, never a
    // stale revision — the FIFO guarantee the desktop worker gives its
    // Sync-before-query jobs.
    super::live_sync::flush_document_sync(runtime, root, &file);
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
            let live_text = super::active_buffer::live_buffer_text(&file);
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
                    document_revision: String::new(),
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
            let locations = engine::definition(runtime, root, &file, line, character)
                .into_iter()
                .filter_map(|location| {
                    let range = location.range?;
                    Some(EditorLspLocation {
                        uri: location.path,
                        // Engine query outputs are 1-based display
                        // coordinates; the wire is 0-based.
                        line: range.start.line.saturating_sub(1),
                        character: range.start.character.saturating_sub(1),
                    })
                })
                .collect();
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
                let hit_path = PathBuf::from(&location.path);
                let (line1, col1) = location
                    .range
                    .as_ref()
                    .map(|range| (range.start.line, range.start.character))
                    .unwrap_or((1, 1));
                let line0 = line1.saturating_sub(1) as usize;
                let text = file_lines
                    .entry(hit_path.clone())
                    .or_insert_with(|| {
                        super::active_buffer::live_buffer_text(&hit_path)
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
                        document_revision: String::new(),
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
                Some(edit) => split_workspace_edit(&edit, open_paths),
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
    let file = resolve_file(root, &selected.file_path);
    let server_id = selected.server_id;
    let title = selected.title;
    let action = selected.payload;
    let is_bare_command = action.get("command").is_some_and(|c| c.is_string());
    let (edit, ran_command) = if is_bare_command {
        let _ = engine::execute_command(runtime, root, &file, &server_id, action);
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
                let _ = engine::execute_command(
                    runtime,
                    root,
                    &file,
                    &server_id,
                    command.clone(),
                );
                true
            }
            _ => false,
        };
        (edit, ran_command)
    };
    let body = match edit {
        Some(edit) => split_workspace_edit(&edit, open_paths),
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
        locations: body.locations,
        references: body.references,
        code_actions: body.code_actions,
        edits: body.edits,
        applied_files: body.applied_files,
        ran_command: body.ran_command,
        title: body.title,
    }
}

fn resolve_file(root: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    joined.canonicalize().unwrap_or(joined)
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
    edit: &serde_json::Value,
    open_paths: &[PathBuf],
) -> QueryResultBody {
    let per_file = neoism_ui::editor::code::lsp_session::workspace_edit_file_edits(edit);
    let mut body = QueryResultBody::default();
    let is_open = |path: &Path| {
        open_paths.iter().any(|open| {
            open == path
                || open.canonicalize().ok().as_deref() == Some(path)
                || path.canonicalize().ok().as_deref() == Some(open)
        })
    };
    for (path, raw_edits) in per_file {
        let typed = typed_edits(&raw_edits);
        if typed.is_empty() {
            continue;
        }
        if is_open(&path) {
            body.edits.push(EditorLspFileEdit { path, edits: typed });
        } else {
            match apply_edits_on_disk(&path, &typed) {
                Ok(()) => body.applied_files.push(path),
                Err(error) => {
                    tracing::warn!(
                        file = %path.display(),
                        %error,
                        "failed to apply workspace edit on disk"
                    );
                }
            }
        }
    }
    body
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
