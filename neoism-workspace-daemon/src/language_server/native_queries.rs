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
    EditorLspAction, EditorLspActionCapability, EditorLspBufferSnapshot,
    EditorLspCodeAction, EditorLspCompletionItem, EditorLspEditOperation,
    EditorLspFileEdit, EditorLspLocation, EditorLspMutationPlan, EditorLspOpenBuffer,
    EditorLspPreparedFile, EditorLspReadCapabilities, EditorLspReadClient,
    EditorLspReadDiagnostic, EditorLspReadDocumentSymbol, EditorLspReadHover,
    EditorLspReadLocation, EditorLspReadOperation, EditorLspReadOutcome,
    EditorLspReadParameter, EditorLspReadPosition, EditorLspReadRange,
    EditorLspReadRelatedInformation, EditorLspReadSignature, EditorLspReadSignatureHelp,
    EditorLspReadWorkspaceSymbol, EditorLspReference, EditorLspStructuredFileEdit,
    EditorLspTextEdit, EditorServerMessage,
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const STRUCTURED_EDIT_TTL: Duration = Duration::from_secs(5 * 60);
const STRUCTURED_ACTION_LIMIT: usize = 2048;
const STRUCTURED_PLAN_LIMIT: usize = 512;
const STRUCTURED_COMMAND_LIMIT: usize = 512;

#[derive(Default)]
pub(crate) struct StructuredEditVault {
    actions: HashMap<String, RetainedAction>,
    plans: HashMap<String, RetainedPlan>,
    commands: HashMap<String, RetainedCommand>,
}

struct RetainedAction {
    created_at: Instant,
    root: PathBuf,
    source_request: u64,
    target: PathBuf,
    line: u32,
    character: u32,
    open_revisions: HashMap<PathBuf, u64>,
    server_id: String,
    title: String,
    payload: Value,
}

struct RetainedPlan {
    created_at: Instant,
    root: PathBuf,
    source_request: u64,
    target: PathBuf,
    title: String,
    files: Vec<PreparedFile>,
    open_revisions: HashMap<PathBuf, u64>,
    command: Option<(String, Value)>,
}

struct RetainedCommand {
    created_at: Instant,
    root: PathBuf,
    target: PathBuf,
    server_id: String,
    title: String,
    command: Value,
}

struct PreparedFile {
    path: PathBuf,
    edits: Vec<EditorLspTextEdit>,
    closed_digest: Option<String>,
}

impl StructuredEditVault {
    fn expire(&mut self) {
        let now = Instant::now();
        self.actions
            .retain(|_, item| now.duration_since(item.created_at) < STRUCTURED_EDIT_TTL);
        self.plans
            .retain(|_, item| now.duration_since(item.created_at) < STRUCTURED_EDIT_TTL);
        self.commands
            .retain(|_, item| now.duration_since(item.created_at) < STRUCTURED_EDIT_TTL);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_structured_edit(
    runtime: &engine::LspRuntime,
    vault: &std::sync::Mutex<StructuredEditVault>,
    root: &Path,
    request_id: u64,
    operation: EditorLspEditOperation,
    path: &Path,
    line: u32,
    character: u32,
    argument: Option<&str>,
    action: Option<EditorLspActionCapability>,
    open_buffers: &[EditorLspOpenBuffer],
    surface_id: Option<String>,
) -> EditorServerMessage {
    let error_surface = surface_id.clone();
    let result = (|| -> Result<EditorServerMessage, String> {
        let target = scoped_file(root, path)?;
        super::live_sync::flush_document_sync(runtime, root, &target);
        let open_revisions = normalize_open_buffers(root, open_buffers)?;
        match operation {
            EditorLspEditOperation::CodeActions => {
                let groups =
                    engine::code_actions(runtime, root, &target, line, character);
                let mut retained = Vec::new();
                let mut public = Vec::new();
                for group in groups {
                    let server_id = group
                        .get("language")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    for raw in group
                        .get("actions")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if raw.get("disabled").is_some() {
                            continue;
                        }
                        let Some(title) = raw.get("title").and_then(Value::as_str) else {
                            continue;
                        };
                        let action_id = uuid::Uuid::new_v4().simple().to_string();
                        public.push(EditorLspActionCapability {
                            request_id,
                            action_id: action_id.clone(),
                            title: title.to_owned(),
                            kind: raw
                                .get("kind")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            preferred: raw
                                .get("isPreferred")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        });
                        retained.push((
                            action_id,
                            RetainedAction {
                                created_at: Instant::now(),
                                root: root.to_path_buf(),
                                source_request: request_id,
                                target: target.clone(),
                                line,
                                character,
                                open_revisions: open_revisions.clone(),
                                server_id: server_id.clone(),
                                title: title.to_owned(),
                                payload: raw.clone(),
                            },
                        ));
                    }
                }
                public.sort_by_key(|item| std::cmp::Reverse(item.preferred));
                let mut vault =
                    vault.lock().map_err(|_| "LSP edit vault is unavailable")?;
                vault.expire();
                if vault.actions.len().saturating_add(retained.len())
                    > STRUCTURED_ACTION_LIMIT
                {
                    return Err("Too many structured LSP actions are retained".into());
                }
                vault.actions.extend(retained);
                Ok(EditorServerMessage::LspEditPrepared {
                    surface_id,
                    operation,
                    actions: public,
                    plan: None,
                })
            }
            EditorLspEditOperation::ApplyCodeAction => {
                let capability = action.ok_or_else(|| {
                    "apply_code_action requires a retained action capability".to_string()
                })?;
                let retained = {
                    let mut vault =
                        vault.lock().map_err(|_| "LSP edit vault is unavailable")?;
                    vault.expire();
                    vault.actions.remove(&capability.action_id).ok_or_else(|| {
                        "Code action is stale, unknown, or already consumed".to_string()
                    })?
                };
                if retained.root != root
                    || retained.target != target
                    || retained.source_request != capability.request_id
                    || retained.line != line
                    || retained.character != character
                    || retained.open_revisions != open_revisions
                {
                    return Err(
                        "Code action capability does not match this workspace and target"
                            .into(),
                    );
                }
                let is_bare_command = retained
                    .payload
                    .get("command")
                    .is_some_and(Value::is_string);
                let mut payload = retained.payload;
                if !is_bare_command && payload.get("edit").is_none() {
                    if let Some(resolved) = engine::resolve_code_action(
                        runtime,
                        root,
                        &target,
                        &retained.server_id,
                        payload.clone(),
                    ) {
                        payload = resolved;
                    }
                }
                let edit = (!is_bare_command)
                    .then(|| payload.get("edit").filter(|value| !value.is_null()))
                    .flatten();
                let files = prepare_workspace_edit(runtime, root, edit, &open_revisions)?;
                let command = if is_bare_command {
                    Some((retained.server_id, payload))
                } else {
                    payload
                        .get("command")
                        .filter(|value| !value.is_null())
                        .cloned()
                        .map(|command| (retained.server_id, command))
                };
                retain_plan(
                    vault,
                    root,
                    request_id,
                    target,
                    retained.title,
                    files,
                    open_revisions,
                    command,
                    operation,
                    surface_id,
                )
            }
            EditorLspEditOperation::Rename => {
                let new_name = argument.unwrap_or_default().trim();
                if new_name.is_empty() {
                    return Err("rename requires a non-empty new name".into());
                }
                let groups =
                    engine::rename(runtime, root, &target, line, character, new_name);
                let edit = groups
                    .iter()
                    .find_map(|group| group.get("edit").filter(|edit| !edit.is_null()));
                let files = prepare_workspace_edit(runtime, root, edit, &open_revisions)?;
                retain_plan(
                    vault,
                    root,
                    request_id,
                    target,
                    "Rename".into(),
                    files,
                    open_revisions,
                    None,
                    operation,
                    surface_id,
                )
            }
            EditorLspEditOperation::Format => {
                if !open_revisions.contains_key(&target) {
                    return Err("Structured format requires an open target buffer".into());
                }
                let raw = engine::formatting(runtime, root, &target);
                let raw =
                    neoism_ui::editor::code::lsp_session::formatting_text_edits(&raw);
                let edits = strict_typed_edits(&raw)?;
                let files = prepare_typed_files(
                    runtime,
                    root,
                    (!edits.is_empty())
                        .then(|| vec![(target.clone(), edits)])
                        .unwrap_or_default(),
                    &open_revisions,
                )?;
                retain_plan(
                    vault,
                    root,
                    request_id,
                    target,
                    "Format".into(),
                    files,
                    open_revisions,
                    None,
                    operation,
                    surface_id,
                )
            }
        }
    })();
    result.unwrap_or_else(|message| EditorServerMessage::Error {
        surface_id: error_surface,
        message,
    })
}

#[allow(clippy::too_many_arguments)]
fn retain_plan(
    vault: &std::sync::Mutex<StructuredEditVault>,
    root: &Path,
    source_request: u64,
    target: PathBuf,
    title: String,
    files: Vec<PreparedFile>,
    open_revisions: HashMap<PathBuf, u64>,
    command: Option<(String, Value)>,
    operation: EditorLspEditOperation,
    surface_id: Option<String>,
) -> Result<EditorServerMessage, String> {
    let plan_id = uuid::Uuid::new_v4().simple().to_string();
    let public_files = files
        .iter()
        .map(|file| EditorLspPreparedFile {
            path: file.path.to_string_lossy().into_owned(),
            edit_count: file.edits.len(),
            open: file.closed_digest.is_none(),
        })
        .collect();
    let plan = EditorLspMutationPlan {
        plan_id: plan_id.clone(),
        title: title.clone(),
        files: public_files,
    };
    let mut vault = vault.lock().map_err(|_| "LSP edit vault is unavailable")?;
    vault.expire();
    if vault.plans.len() >= STRUCTURED_PLAN_LIMIT {
        return Err("Too many structured LSP mutation plans are retained".into());
    }
    vault.plans.insert(
        plan_id,
        RetainedPlan {
            created_at: Instant::now(),
            root: root.to_path_buf(),
            source_request,
            target,
            title,
            files,
            open_revisions,
            command,
        },
    );
    Ok(EditorServerMessage::LspEditPrepared {
        surface_id,
        operation,
        actions: Vec::new(),
        plan: Some(plan),
    })
}

pub(crate) fn commit_structured_edit(
    runtime: &engine::LspRuntime,
    vault: &std::sync::Mutex<StructuredEditVault>,
    root: &Path,
    plan_id: &str,
    open_buffers: &[EditorLspOpenBuffer],
    daemon_open_files: &HashSet<PathBuf>,
    surface_id: Option<String>,
) -> EditorServerMessage {
    let error_surface = surface_id.clone();
    let result = (|| -> Result<EditorServerMessage, String> {
        let current_open = normalize_open_buffers(root, open_buffers)?;
        let plan = {
            let mut vault = vault.lock().map_err(|_| "LSP edit vault is unavailable")?;
            vault.expire();
            vault.plans.remove(plan_id).ok_or_else(|| {
                "Mutation plan is stale, unknown, or already consumed".to_string()
            })?
        };
        if plan.root != root {
            return Err("Mutation plan belongs to another workspace".into());
        }
        // Exact equality makes every frontend-owned revision part of the
        // optimistic transaction, including buffers untouched by the edit.
        if current_open != plan.open_revisions {
            return Err(
                "Open buffer revisions changed; prepare the LSP edit again".into()
            );
        }
        let daemon_open_files = daemon_open_files
            .iter()
            .map(|path| scoped_file(root, path))
            .collect::<Result<HashSet<_>, _>>()?;
        for file in &plan.files {
            let text = if let Some(expected) = &file.closed_digest {
                if current_open.contains_key(&file.path)
                    || daemon_open_files.contains(&file.path)
                {
                    return Err(format!(
                        "LSP edit target became open after preparation: {}",
                        file.path.display()
                    ));
                }
                let text = std::fs::read_to_string(&file.path).map_err(|error| {
                    format!("Failed to read {}: {error}", file.path.display())
                })?;
                if text_digest(&text) != *expected {
                    return Err(format!(
                        "Closed LSP edit target changed after preparation: {}",
                        file.path.display()
                    ));
                }
                text
            } else {
                if !current_open.contains_key(&file.path) {
                    return Err(format!(
                        "LSP edit target is no longer open: {}",
                        file.path.display()
                    ));
                }
                super::active_buffer::live_buffer_text(runtime, root, &file.path)
                    .ok_or_else(|| {
                        format!("Live LSP buffer is unavailable: {}", file.path.display())
                    })?
            };
            validate_text_edits(&text, &file.edits)?;
        }
        let command_id = if let Some((server_id, command)) = plan.command.as_ref() {
            let command_id = uuid::Uuid::new_v4().simple().to_string();
            let mut vault = vault.lock().map_err(|_| "LSP edit vault is unavailable")?;
            vault.expire();
            if vault.commands.len() >= STRUCTURED_COMMAND_LIMIT {
                return Err("Too many deferred LSP commands are retained".into());
            }
            vault.commands.insert(
                command_id.clone(),
                RetainedCommand {
                    created_at: Instant::now(),
                    root: root.to_path_buf(),
                    target: plan.target.clone(),
                    server_id: server_id.clone(),
                    title: plan.title.clone(),
                    command: command.clone(),
                },
            );
            Some(command_id)
        } else {
            None
        };
        let mut edits = Vec::new();
        let mut applied_files = Vec::new();
        for file in &plan.files {
            if file.closed_digest.is_some() {
                apply_edits_on_disk(&file.path, &file.edits).map_err(|error| {
                    format!(
                        "Failed to apply LSP edit to {}: {error}",
                        file.path.display()
                    )
                })?;
                applied_files.push(file.path.to_string_lossy().into_owned());
            } else {
                edits.push(EditorLspStructuredFileEdit {
                    path: file.path.to_string_lossy().into_owned(),
                    edits: file.edits.clone(),
                });
            }
        }
        let _ = plan.source_request;
        Ok(EditorServerMessage::LspEditCommitted {
            surface_id,
            title: plan.title,
            edits,
            applied_files,
            ran_command: false,
            command_id,
        })
    })();
    result.unwrap_or_else(|message| EditorServerMessage::Error {
        surface_id: error_surface,
        message,
    })
}

pub(crate) fn finalize_structured_edit(
    runtime: &engine::LspRuntime,
    vault: &std::sync::Mutex<StructuredEditVault>,
    root: &Path,
    command_id: &str,
    buffers: &[EditorLspBufferSnapshot],
    surface_id: Option<String>,
) -> EditorServerMessage {
    let error_surface = surface_id.clone();
    let result = (|| -> Result<EditorServerMessage, String> {
        let retained = {
            let mut vault = vault.lock().map_err(|_| "LSP edit vault is unavailable")?;
            vault.expire();
            vault.commands.remove(command_id).ok_or_else(|| {
                "Deferred LSP command is stale, unknown, or already consumed".to_string()
            })?
        };
        if retained.root != root {
            return Err("Deferred LSP command belongs to another workspace".into());
        }
        let mut seen = HashSet::new();
        for buffer in buffers {
            let path = scoped_file(root, Path::new(&buffer.path))?;
            if !seen.insert(path.clone()) {
                return Err(format!(
                    "Duplicate synchronized LSP buffer: {}",
                    path.display()
                ));
            }
            let _ = buffer.revision;
            let _ = engine::sync_document(runtime, root, &path, Some(&buffer.text));
            super::live_sync::flush_document_sync(runtime, root, &path);
        }
        engine::execute_command(
            runtime,
            root,
            &retained.target,
            &retained.server_id,
            retained.command,
        )
        .ok_or_else(|| format!("Host LSP command failed: {}", retained.title))?;
        Ok(EditorServerMessage::LspEditFinalized {
            surface_id,
            ran_command: true,
        })
    })();
    result.unwrap_or_else(|message| EditorServerMessage::Error {
        surface_id: error_surface,
        message,
    })
}

fn normalize_open_buffers(
    root: &Path,
    open_buffers: &[EditorLspOpenBuffer],
) -> Result<HashMap<PathBuf, u64>, String> {
    let mut result = HashMap::new();
    for open in open_buffers {
        let path = scoped_file(root, Path::new(&open.path))?;
        if result.insert(path.clone(), open.revision).is_some() {
            return Err(format!("Duplicate open LSP buffer: {}", path.display()));
        }
    }
    Ok(result)
}

fn prepare_workspace_edit(
    runtime: &engine::LspRuntime,
    root: &Path,
    edit: Option<&Value>,
    open_revisions: &HashMap<PathBuf, u64>,
) -> Result<Vec<PreparedFile>, String> {
    let Some(edit) = edit else {
        return Ok(Vec::new());
    };
    if edit
        .get("documentChanges")
        .and_then(Value::as_array)
        .is_some_and(|changes| {
            changes.iter().any(|change| {
                change.get("kind").is_some() || change.get("textDocument").is_none()
            })
        })
    {
        return Err(
            "Resource operations are not supported by structured LSP edits".into(),
        );
    }
    let files = host_workspace_edit_targets(root, edit)?
        .into_iter()
        .map(|(path, raw)| strict_typed_edits(&raw).map(|edits| (path, edits)))
        .collect::<Result<Vec<_>, _>>()?;
    prepare_typed_files(runtime, root, files, open_revisions)
}

fn prepare_typed_files(
    runtime: &engine::LspRuntime,
    root: &Path,
    files: Vec<(PathBuf, Vec<EditorLspTextEdit>)>,
    open_revisions: &HashMap<PathBuf, u64>,
) -> Result<Vec<PreparedFile>, String> {
    let mut combined: HashMap<PathBuf, Vec<EditorLspTextEdit>> = HashMap::new();
    for (path, edits) in files {
        let path = scoped_file(root, &path)?;
        combined.entry(path).or_default().extend(edits);
    }
    combined
        .into_iter()
        .filter(|(_, edits)| !edits.is_empty())
        .map(|(path, edits)| {
            let (text, closed_digest) = if open_revisions.contains_key(&path) {
                let text = super::active_buffer::live_buffer_text(runtime, root, &path)
                    .ok_or_else(|| {
                    format!("Live LSP buffer is unavailable: {}", path.display())
                })?;
                (text, None)
            } else {
                let text = std::fs::read_to_string(&path).map_err(|error| {
                    format!("Failed to read {}: {error}", path.display())
                })?;
                let digest = Some(text_digest(&text));
                (text, digest)
            };
            validate_text_edits(&text, &edits)?;
            Ok(PreparedFile {
                path,
                edits,
                closed_digest,
            })
        })
        .collect()
}

fn strict_typed_edits(raw: &[Value]) -> Result<Vec<EditorLspTextEdit>, String> {
    let edits = typed_edits(raw);
    if edits.len() != raw.len() {
        return Err("Malformed LSP text edit".into());
    }
    Ok(edits)
}

fn text_digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn validate_text_edits(text: &str, edits: &[EditorLspTextEdit]) -> Result<(), String> {
    let cleaned = text.replace('\r', "");
    let lines: Vec<&str> = cleaned.split('\n').collect();
    let mut spans = Vec::with_capacity(edits.len());
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
            return Err("Invalid UTF-8 LSP edit range".into());
        }
        spans.push((start, end));
    }
    spans.sort_unstable();
    for pair in spans.windows(2) {
        if pair[1].0 < pair[0].1 || pair[1].0 == pair[0].0 {
            return Err("Overlapping LSP text edits are not supported".into());
        }
    }
    Ok(())
}

/// Serve one structured LSP read. Every path is resolved and serialized on the
/// daemon host; guests treat the returned strings as opaque identities.
pub(crate) fn read_query(
    runtime: &engine::LspRuntime,
    root: &Path,
    operation: EditorLspReadOperation,
    path: &Path,
    line: u32,
    character: u32,
    query: &str,
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
    super::live_sync::flush_document_sync(runtime, root, &file);
    let outcome = match operation {
        EditorLspReadOperation::Hover => collect_results(
            engine::hover(runtime, root, &file, line, character),
            |hover| map_read_hover(root, hover),
        )
        .map(EditorLspReadOutcome::Hover),
        EditorLspReadOperation::SignatureHelp => collect_results(
            engine::signature_help(runtime, root, &file, line, character),
            |help| map_read_signature_help(root, help),
        )
        .map(EditorLspReadOutcome::SignatureHelp),
        EditorLspReadOperation::Definition => collect_results(
            engine::definition(runtime, root, &file, line, character),
            |location| map_read_location(root, location),
        )
        .map(EditorLspReadOutcome::Definition),
        EditorLspReadOperation::References => collect_results(
            engine::references(runtime, root, &file, line, character),
            |location| map_read_location(root, location),
        )
        .map(EditorLspReadOutcome::References),
        EditorLspReadOperation::DocumentSymbols => {
            collect_results(engine::document_symbols(runtime, root, &file), |symbol| {
                map_read_document_symbol(root, symbol)
            })
            .map(EditorLspReadOutcome::DocumentSymbols)
        }
        EditorLspReadOperation::WorkspaceSymbols => {
            collect_results(engine::workspace_symbols(runtime, root, query), |symbol| {
                Ok(EditorLspReadWorkspaceSymbol {
                    name: symbol.name,
                    kind: symbol.kind,
                    path: read_result_path(root, &symbol.path)?,
                    line: symbol.line.map(|line| line.saturating_sub(1)),
                    language: symbol.language,
                })
            })
            .map(EditorLspReadOutcome::WorkspaceSymbols)
        }
        EditorLspReadOperation::Diagnostics => collect_results(
            engine::cached_diagnostics(runtime, root, &file),
            |diagnostic| map_read_diagnostic(root, diagnostic),
        )
        .map(EditorLspReadOutcome::Diagnostics),
        EditorLspReadOperation::Clients => Ok(EditorLspReadOutcome::Clients(
            engine::status(runtime, root, Some(&file))
                .into_iter()
                .map(map_read_client)
                .collect(),
        )),
    };
    match outcome {
        Ok(outcome) => EditorServerMessage::LspReadResult {
            surface_id,
            operation,
            outcome,
        },
        Err(message) => EditorServerMessage::Error {
            surface_id,
            message,
        },
    }
}

fn collect_results<T, U>(
    items: Vec<T>,
    mut map: impl FnMut(T) -> Result<U, String>,
) -> Result<Vec<U>, String> {
    items.into_iter().map(&mut map).collect()
}

fn read_result_path(root: &Path, value: &str) -> Result<String, String> {
    let path = host_lsp_target(root, value).and_then(|path| scoped_file(root, &path))?;
    Ok(path.to_string_lossy().into_owned())
}

fn map_read_range(range: engine::LspRange) -> EditorLspReadRange {
    EditorLspReadRange {
        start: EditorLspReadPosition {
            line: range.start.line.saturating_sub(1),
            character: range.start.character.saturating_sub(1),
        },
        end: EditorLspReadPosition {
            line: range.end.line.saturating_sub(1),
            character: range.end.character.saturating_sub(1),
        },
    }
}

fn map_read_location(
    root: &Path,
    location: engine::LspLocation,
) -> Result<EditorLspReadLocation, String> {
    Ok(EditorLspReadLocation {
        path: read_result_path(root, &location.path)?,
        range: location.range.map(map_read_range),
        language: location.language,
    })
}

fn map_read_hover(
    root: &Path,
    hover: engine::LspHover,
) -> Result<EditorLspReadHover, String> {
    Ok(EditorLspReadHover {
        path: read_result_path(root, &hover.path)?,
        contents: hover.contents,
        kind: hover.kind,
        range: hover.range.map(map_read_range),
        language: hover.language,
    })
}

fn map_read_signature_help(
    root: &Path,
    help: engine::LspSignatureHelp,
) -> Result<EditorLspReadSignatureHelp, String> {
    Ok(EditorLspReadSignatureHelp {
        path: read_result_path(root, &help.path)?,
        signatures: help
            .signatures
            .into_iter()
            .map(|signature| EditorLspReadSignature {
                label: signature.label,
                documentation: signature.documentation,
                parameters: signature
                    .parameters
                    .into_iter()
                    .map(|parameter| EditorLspReadParameter {
                        label: parameter.label,
                        documentation: parameter.documentation,
                    })
                    .collect(),
                active_parameter: signature.active_parameter,
            })
            .collect(),
        active_signature: help.active_signature,
        active_parameter: help.active_parameter,
        language: help.language,
    })
}

fn map_read_document_symbol(
    root: &Path,
    symbol: engine::LspDocumentSymbol,
) -> Result<EditorLspReadDocumentSymbol, String> {
    Ok(EditorLspReadDocumentSymbol {
        name: symbol.name,
        kind: symbol.kind,
        detail: symbol.detail,
        path: read_result_path(root, &symbol.path)?,
        range: symbol.range.map(map_read_range),
        selection_range: symbol.selection_range.map(map_read_range),
        children: collect_results(symbol.children, |child| {
            map_read_document_symbol(root, child)
        })?,
        language: symbol.language,
    })
}

fn map_read_diagnostic(
    root: &Path,
    diagnostic: engine::LspDiagnostic,
) -> Result<EditorLspReadDiagnostic, String> {
    Ok(EditorLspReadDiagnostic {
        path: read_result_path(root, &diagnostic.path)?,
        range: diagnostic.range.map(map_read_range),
        severity: diagnostic.severity,
        code: diagnostic.code,
        code_description: diagnostic.code_description,
        source: diagnostic.source,
        message: diagnostic.message,
        tags: diagnostic.tags,
        related_information: collect_results(
            diagnostic.related_information,
            |related| {
                Ok(EditorLspReadRelatedInformation {
                    path: read_result_path(root, &related.path)?,
                    range: related.range.map(map_read_range),
                    message: related.message,
                })
            },
        )?,
        data: diagnostic.data,
        language: diagnostic.language,
    })
}

fn map_read_client(status: engine::LspStatus) -> EditorLspReadClient {
    let capabilities = status.capabilities;
    EditorLspReadClient {
        id: status.id,
        name: status.name,
        status: match status.status {
            engine::LspServerState::Available => "available",
            engine::LspServerState::Connected => "connected",
            engine::LspServerState::Error => "error",
        }
        .into(),
        language: status.language,
        command: status.command,
        workspace_root: status.workspace.root,
        capabilities: EditorLspReadCapabilities {
            workspace_symbols: capabilities.workspace_symbols,
            completion: capabilities.completion,
            hover: capabilities.hover,
            definition: capabilities.definition,
            references: capabilities.references,
            implementation: capabilities.implementation,
            call_hierarchy: capabilities.call_hierarchy,
            diagnostics: capabilities.diagnostics,
            document_symbols: capabilities.document_symbols,
            formatting: capabilities.formatting,
            code_actions: capabilities.code_actions,
            rename: capabilities.rename,
        },
    }
}

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
    // Occurrence highlighting is an idle probe, not a user command. Files
    // without an LSP route have no occurrences, rather than an error.
    if status.is_empty() && action == EditorLspAction::DocumentHighlight {
        return query_result(surface_id, seq, action, root, QueryResultBody::default());
    }
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
    fn structured_lsp_ranges_are_normalized_to_zero_based_coordinates() {
        let range = map_read_range(engine::LspRange {
            start: engine::LspPosition {
                line: 1,
                character: 7,
            },
            end: engine::LspPosition {
                line: 4,
                character: 11,
            },
        });
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 6);
        assert_eq!(range.end.line, 3);
        assert_eq!(range.end.character, 10);
    }

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

    #[test]
    fn structured_edit_validation_rejects_overlap_and_out_of_bounds_ranges() {
        let edit = |start_col, end_col| EditorLspTextEdit {
            start_line: 0,
            start_col,
            end_line: 0,
            end_col,
            new_text: "x".into(),
        };
        assert!(validate_text_edits("abcdef", &[edit(1, 4), edit(3, 5)])
            .unwrap_err()
            .contains("Overlapping"));
        assert!(validate_text_edits("abcdef", &[edit(7, 7)])
            .unwrap_err()
            .contains("range"));
        assert!(validate_text_edits("é", &[edit(1, 2)])
            .unwrap_err()
            .contains("UTF-8"));
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
