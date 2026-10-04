//! Structured Lua LSP requests. Unlike the interactive UI lane, these jobs are
//! never coalesced and completions are retained in FIFO order until the
//! application thread delivers them to the exact plugin owner.

use super::*;
use std::collections::VecDeque;

struct LuaLspJob {
    owner: neoism_lua::PluginOwner,
    id: String,
    operation: neoism_lua::LuaLspOperation,
    target: neoism_lua::LuaLspTarget,
    root: PathBuf,
    file: PathBuf,
    text: Option<String>,
    sync_documents: Vec<(PathBuf, String)>,
    query: String,
    new_name: Option<String>,
    action: Option<CodeActionItem>,
    committed: Option<(String, serde_json::Value, neoism_lua::LuaLspMutation)>,
    open_buffers: Vec<LocalLuaLspOpenBuffer>,
    window_id: WindowId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalLuaLspOpenBuffer {
    pub route_id: usize,
    pub path: PathBuf,
    pub revision: u64,
}

pub(crate) struct LocalLuaLspCodeAction {
    pub target: neoism_lua::LuaLspTarget,
    pub root: PathBuf,
    pub file: PathBuf,
    pub item: CodeActionItem,
    pub open_buffers: Vec<LocalLuaLspOpenBuffer>,
}

pub(crate) enum LuaLspRetainedCodeAction {
    Local(LocalLuaLspCodeAction),
    Remote(super::remote::RemoteLuaLspCodeAction),
}

impl LuaLspRetainedCodeAction {
    pub fn target(&self) -> &neoism_lua::LuaLspTarget {
        match self {
            Self::Local(action) => &action.target,
            Self::Remote(action) => &action.target,
        }
    }
}

pub(crate) struct LocalLuaLspPreparedMutation {
    pub title: String,
    pub target: neoism_lua::LuaLspTarget,
    pub root: PathBuf,
    pub files: Vec<LocalLuaLspPreparedFile>,
    pub open_buffers: Vec<LocalLuaLspOpenBuffer>,
    pub command: Option<(String, serde_json::Value)>,
}

pub(crate) struct LocalLuaLspPreparedFile {
    pub path: PathBuf,
    pub edits: Vec<CodeTextEdit>,
    pub expected_closed_text: Option<String>,
}

pub(crate) enum LuaLspPrivateResult {
    CodeActions(Vec<(String, LuaLspRetainedCodeAction)>),
    Mutation(LocalLuaLspPreparedMutation),
    RemoteMutation(super::remote::RemoteLuaLspPreparedMutation),
}

struct LuaLspExecuted {
    outcome: neoism_lua::LuaLspOutcome,
    private: Option<LuaLspPrivateResult>,
}

pub(crate) struct LuaLspWorkerCompletion {
    pub owner: neoism_lua::PluginOwner,
    pub completion: neoism_lua::LuaLspCompletion,
    pub window_id: WindowId,
    pub private: Option<LuaLspPrivateResult>,
}

struct LuaLspShared {
    jobs: Sender<LuaLspJob>,
}

static LUA_LSP: OnceLock<LuaLspShared> = OnceLock::new();
static LUA_LSP_COMPLETIONS: OnceLock<Mutex<VecDeque<LuaLspWorkerCompletion>>> =
    OnceLock::new();

fn completions() -> &'static Mutex<VecDeque<LuaLspWorkerCompletion>> {
    LUA_LSP_COMPLETIONS.get_or_init(Default::default)
}

pub(crate) fn drain_lua_lsp_completions() -> Vec<LuaLspWorkerCompletion> {
    let mut completions = completions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    completions.drain(..).collect()
}

pub(super) fn enqueue_lua_lsp_completion(
    owner: neoism_lua::PluginOwner,
    completion: neoism_lua::LuaLspCompletion,
    window_id: WindowId,
) {
    completions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push_back(LuaLspWorkerCompletion {
            owner,
            completion,
            window_id,
            private: None,
        });
}

pub(super) fn enqueue_lua_lsp_private_completion(
    owner: neoism_lua::PluginOwner,
    completion: neoism_lua::LuaLspCompletion,
    window_id: WindowId,
    private: LuaLspPrivateResult,
) {
    completions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push_back(LuaLspWorkerCompletion {
            owner,
            completion,
            window_id,
            private: Some(private),
        });
}

fn ensure_worker(
    proxy: EventProxy,
    runtime: engine::LspRuntime,
) -> &'static LuaLspShared {
    LUA_LSP.get_or_init(move || {
        let (tx, rx) = mpsc::channel::<LuaLspJob>();
        let _ = std::thread::Builder::new()
            .name("lua-lsp-query".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let operation = job.operation;
                    let target = job.target.clone();
                    let id = job.id.clone();
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            execute(&runtime, &job)
                        }));
                    let (completion, private) = match result {
                        Ok(Ok(result)) => (
                            neoism_lua::LuaLspCompletion::success(
                                id,
                                operation,
                                target,
                                result.outcome,
                            ),
                            result.private,
                        ),
                        Ok(Err((code, message))) => (
                            neoism_lua::LuaLspCompletion::failed(
                                id,
                                operation,
                                Some(target),
                                code,
                                message,
                            ),
                            None,
                        ),
                        Err(_) => (
                            neoism_lua::LuaLspCompletion::failed(
                                id,
                                operation,
                                Some(target),
                                "worker_panic",
                                "the LSP worker panicked while processing the request",
                            ),
                            None,
                        ),
                    };
                    completions()
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push_back(LuaLspWorkerCompletion {
                            owner: job.owner,
                            completion,
                            window_id: job.window_id,
                            private,
                        });
                    proxy.send_event(RioEventType::Rio(RioEvent::Render), job.window_id);
                }
            });
        LuaLspShared { jobs: tx }
    })
}

fn execute(
    runtime: &engine::LspRuntime,
    job: &LuaLspJob,
) -> Result<LuaLspExecuted, (String, String)> {
    if let Some(text) = job.text.as_deref() {
        let _ = engine::sync_document(runtime, &job.root, &job.file, Some(text));
    }
    for (path, text) in &job.sync_documents {
        let _ = engine::sync_document(runtime, &job.root, path, Some(text));
    }
    if let Some((server_id, command, mutation)) = &job.committed {
        if engine::execute_command(
            runtime,
            &job.root,
            &job.file,
            server_id,
            command.clone(),
        )
        .is_none()
        {
            return Err((
                "command_failed".into(),
                "the language server rejected the deferred code action command".into(),
            ));
        }
        let mut mutation = mutation.clone();
        mutation.ran_command = true;
        return Ok(LuaLspExecuted {
            outcome: neoism_lua::LuaLspOutcome::ApplyCodeAction(mutation),
            private: None,
        });
    }
    let line = job.target.line;
    let character = job.target.character;
    let outcome = match job.operation {
        neoism_lua::LuaLspOperation::Hover => neoism_lua::LuaLspOutcome::Hover(
            engine::hover(runtime, &job.root, &job.file, line, character)
                .into_iter()
                .map(|hover| map_hover(&job.root, hover))
                .collect(),
        ),
        neoism_lua::LuaLspOperation::SignatureHelp => {
            neoism_lua::LuaLspOutcome::SignatureHelp(
                engine::signature_help(runtime, &job.root, &job.file, line, character)
                    .into_iter()
                    .map(|help| map_signature_help(&job.root, help))
                    .collect(),
            )
        }
        neoism_lua::LuaLspOperation::Definition => neoism_lua::LuaLspOutcome::Definition(
            engine::definition(runtime, &job.root, &job.file, line, character)
                .into_iter()
                .map(|location| map_location(&job.root, location))
                .collect(),
        ),
        neoism_lua::LuaLspOperation::References => neoism_lua::LuaLspOutcome::References(
            engine::references(runtime, &job.root, &job.file, line, character)
                .into_iter()
                .map(|location| map_location(&job.root, location))
                .collect(),
        ),
        neoism_lua::LuaLspOperation::DocumentSymbols => {
            neoism_lua::LuaLspOutcome::DocumentSymbols(
                engine::document_symbols(runtime, &job.root, &job.file)
                    .into_iter()
                    .map(|symbol| map_document_symbol(&job.root, symbol))
                    .collect(),
            )
        }
        neoism_lua::LuaLspOperation::WorkspaceSymbols => {
            neoism_lua::LuaLspOutcome::WorkspaceSymbols(
                engine::workspace_symbols(runtime, &job.root, &job.query)
                    .into_iter()
                    .map(|symbol| neoism_lua::LuaLspWorkspaceSymbol {
                        name: symbol.name,
                        kind: symbol.kind,
                        path: local_result_path(&job.root, symbol.path),
                        line: symbol.line.map(|line| line.saturating_sub(1)),
                        language: symbol.language,
                    })
                    .collect(),
            )
        }
        neoism_lua::LuaLspOperation::Diagnostics => {
            neoism_lua::LuaLspOutcome::Diagnostics(
                engine::cached_diagnostics(runtime, &job.root, &job.file)
                    .into_iter()
                    .map(|diagnostic| map_diagnostic(&job.root, diagnostic))
                    .collect(),
            )
        }
        neoism_lua::LuaLspOperation::Clients => neoism_lua::LuaLspOutcome::Clients(
            engine::status(runtime, &job.root, Some(&job.file))
                .into_iter()
                .map(map_client)
                .collect(),
        ),
        neoism_lua::LuaLspOperation::CodeActions => {
            let items = flatten_code_actions(&engine::code_actions(
                runtime, &job.root, &job.file, line, character,
            ));
            let mut private = Vec::new();
            let public = items
                .into_iter()
                .filter(|item| item.action.get("disabled").is_none())
                .map(|item| {
                    let action_id = uuid::Uuid::new_v4().simple().to_string();
                    let summary = neoism_lua::LuaLspCodeAction {
                        id: action_id.clone(),
                        request_id: job.id.clone(),
                        title: item.title.clone(),
                        kind: (!item.kind.is_empty()).then(|| item.kind.clone()),
                        preferred: item
                            .action
                            .get("isPreferred")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    };
                    private.push((
                        action_id,
                        LuaLspRetainedCodeAction::Local(LocalLuaLspCodeAction {
                            target: job.target.clone(),
                            root: job.root.clone(),
                            file: job.file.clone(),
                            item,
                            open_buffers: job.open_buffers.clone(),
                        }),
                    ));
                    summary
                })
                .collect();
            return Ok(LuaLspExecuted {
                outcome: neoism_lua::LuaLspOutcome::CodeActions(public),
                private: Some(LuaLspPrivateResult::CodeActions(private)),
            });
        }
        neoism_lua::LuaLspOperation::Format => {
            let groups = engine::formatting(runtime, &job.root, &job.file);
            let edits =
                neoism_ui::editor::code::lsp_session::formatting_text_edits(&groups);
            let parsed = parse_structured_text_edits(&edits)?;
            let files = prepare_local_files(
                &job.root,
                &job.open_buffers,
                (!parsed.is_empty())
                    .then(|| vec![(job.file.clone(), parsed)])
                    .unwrap_or_default(),
            )?;
            let mutation = LocalLuaLspPreparedMutation {
                title: "Format".into(),
                target: job.target.clone(),
                root: job.root.clone(),
                files,
                open_buffers: job.open_buffers.clone(),
                command: None,
            };
            return Ok(LuaLspExecuted {
                outcome: neoism_lua::LuaLspOutcome::Format(neoism_lua::LuaLspMutation {
                    title: mutation.title.clone(),
                    changed_files: Vec::new(),
                    ran_command: false,
                }),
                private: Some(LuaLspPrivateResult::Mutation(mutation)),
            });
        }
        neoism_lua::LuaLspOperation::Rename => {
            let new_name = job.new_name.as_deref().ok_or_else(|| {
                ("invalid_arguments".into(), "rename requires newName".into())
            })?;
            let groups =
                engine::rename(runtime, &job.root, &job.file, line, character, new_name);
            let edit = groups.iter().find_map(|group| {
                group.get("edit").filter(|edit| !edit.is_null()).cloned()
            });
            if edit
                .as_ref()
                .is_some_and(workspace_edit_has_resource_operations)
            {
                return Err((
                    "unsupported_edit".into(),
                    "resource operations are not supported by structured LSP edits"
                        .into(),
                ));
            }
            let files = prepare_local_files(
                &job.root,
                &job.open_buffers,
                edit.as_ref()
                    .map(workspace_edit_file_edits_strict)
                    .transpose()?
                    .unwrap_or_default(),
            )?;
            let mutation = LocalLuaLspPreparedMutation {
                title: "Rename".into(),
                target: job.target.clone(),
                root: job.root.clone(),
                files,
                open_buffers: job.open_buffers.clone(),
                command: None,
            };
            return Ok(LuaLspExecuted {
                outcome: neoism_lua::LuaLspOutcome::Rename(neoism_lua::LuaLspMutation {
                    title: mutation.title.clone(),
                    changed_files: Vec::new(),
                    ran_command: false,
                }),
                private: Some(LuaLspPrivateResult::Mutation(mutation)),
            });
        }
        neoism_lua::LuaLspOperation::ApplyCodeAction => {
            let item = job.action.as_ref().ok_or_else(|| {
                (
                    "invalid_action".into(),
                    "the code action capability is missing".into(),
                )
            })?;
            let is_bare_command = item
                .action
                .get("command")
                .is_some_and(serde_json::Value::is_string);
            let mut action = item.action.clone();
            if !is_bare_command && action.get("edit").is_none() {
                if let Some(resolved) = engine::resolve_code_action(
                    runtime,
                    &job.root,
                    &job.file,
                    &item.server_id,
                    action.clone(),
                ) {
                    action = resolved;
                }
            }
            let edit = (!is_bare_command)
                .then(|| action.get("edit").filter(|edit| !edit.is_null()).cloned())
                .flatten();
            if edit
                .as_ref()
                .is_some_and(workspace_edit_has_resource_operations)
            {
                return Err((
                    "unsupported_edit".into(),
                    "resource operations are not supported by structured LSP edits"
                        .into(),
                ));
            }
            let files = prepare_local_files(
                &job.root,
                &job.open_buffers,
                edit.as_ref()
                    .map(workspace_edit_file_edits_strict)
                    .transpose()?
                    .unwrap_or_default(),
            )?;
            let command = if is_bare_command {
                Some((item.server_id.clone(), action))
            } else {
                action
                    .get("command")
                    .filter(|command| !command.is_null())
                    .cloned()
                    .map(|command| (item.server_id.clone(), command))
            };
            let mutation = LocalLuaLspPreparedMutation {
                title: item.title.clone(),
                target: job.target.clone(),
                root: job.root.clone(),
                files,
                open_buffers: job.open_buffers.clone(),
                command,
            };
            return Ok(LuaLspExecuted {
                outcome: neoism_lua::LuaLspOutcome::ApplyCodeAction(
                    neoism_lua::LuaLspMutation {
                        title: mutation.title.clone(),
                        changed_files: Vec::new(),
                        ran_command: false,
                    },
                ),
                private: Some(LuaLspPrivateResult::Mutation(mutation)),
            });
        }
    };
    Ok(LuaLspExecuted {
        outcome,
        private: None,
    })
}

fn workspace_edit_has_resource_operations(edit: &serde_json::Value) -> bool {
    edit.get("documentChanges")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|changes| {
            changes.iter().any(|change| {
                change.get("kind").is_some() || change.get("textDocument").is_none()
            })
        })
}

fn parse_structured_text_edits(
    edits: &[serde_json::Value],
) -> Result<Vec<CodeTextEdit>, (String, String)> {
    let parsed = parse_lsp_text_edits(edits);
    if parsed.len() != edits.len() {
        return Err((
            "invalid_edit".into(),
            "the language server returned a malformed text edit".into(),
        ));
    }
    Ok(parsed)
}

fn workspace_edit_file_edits_strict(
    edit: &serde_json::Value,
) -> Result<Vec<(PathBuf, Vec<CodeTextEdit>)>, (String, String)> {
    let mut expected_edits = 0usize;
    if let Some(changes) = edit.get("changes") {
        let changes = changes.as_object().ok_or_else(|| {
            (
                "invalid_edit".into(),
                "workspace edit `changes` must be an object".into(),
            )
        })?;
        for edits in changes.values() {
            expected_edits = expected_edits.saturating_add(
                edits
                    .as_array()
                    .ok_or_else(|| {
                        (
                            "invalid_edit".into(),
                            "workspace edit file edits must be an array".into(),
                        )
                    })?
                    .len(),
            );
        }
    }
    if let Some(changes) = edit.get("documentChanges") {
        let changes = changes.as_array().ok_or_else(|| {
            (
                "invalid_edit".into(),
                "workspace edit `documentChanges` must be an array".into(),
            )
        })?;
        for change in changes {
            expected_edits = expected_edits.saturating_add(
                change
                    .get("edits")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| {
                        (
                            "invalid_edit".into(),
                            "text document edit is missing its edits array".into(),
                        )
                    })?
                    .len(),
            );
        }
    }
    let raw = workspace_edit_file_edits(edit);
    if raw.iter().map(|(_, edits)| edits.len()).sum::<usize>() != expected_edits {
        return Err((
            "invalid_edit_path".into(),
            "workspace edit contains an invalid or unsupported document URI".into(),
        ));
    }
    raw.into_iter()
        .map(|(path, edits)| Ok((path, parse_structured_text_edits(&edits)?)))
        .collect()
}

fn prepare_local_files(
    root: &Path,
    open_buffers: &[LocalLuaLspOpenBuffer],
    files: Vec<(PathBuf, Vec<CodeTextEdit>)>,
) -> Result<Vec<LocalLuaLspPreparedFile>, (String, String)> {
    let canonical_root = std::fs::canonicalize(root).map_err(|error| {
        (
            "invalid_workspace".into(),
            format!("failed to resolve structured LSP workspace: {error}"),
        )
    })?;
    files
        .into_iter()
        .map(|(path, edits)| {
            let canonical_path = std::fs::canonicalize(&path).map_err(|error| {
                (
                    "invalid_edit_path".into(),
                    format!(
                        "failed to resolve LSP edit path {}: {error}",
                        path.display()
                    ),
                )
            })?;
            if !canonical_path.starts_with(&canonical_root) {
                return Err((
                    "outside_workspace".into(),
                    format!("LSP edit path is outside the workspace: {}", path.display()),
                ));
            }
            let expected_closed_text = if open_buffers
                .iter()
                .any(|open| canonical_key(&open.path) == canonical_key(&canonical_path))
            {
                None
            } else {
                Some(std::fs::read_to_string(&canonical_path).map_err(|error| {
                    (
                        "read_failed".into(),
                        format!(
                            "failed to read LSP edit target {}: {error}",
                            path.display()
                        ),
                    )
                })?)
            };
            Ok(LocalLuaLspPreparedFile {
                path: canonical_path,
                edits,
                expected_closed_text,
            })
        })
        .collect()
}

fn local_result_path(root: &Path, path: String) -> String {
    if path.is_empty() || Path::new(&path).is_absolute() {
        path
    } else {
        root.join(path).to_string_lossy().into_owned()
    }
}

fn map_range(range: engine::LspRange) -> neoism_lua::LuaLspRange {
    neoism_lua::LuaLspRange {
        start: neoism_lua::LuaLspPosition {
            line: range.start.line.saturating_sub(1),
            character: range.start.character.saturating_sub(1),
        },
        end: neoism_lua::LuaLspPosition {
            line: range.end.line.saturating_sub(1),
            character: range.end.character.saturating_sub(1),
        },
    }
}

fn map_location(
    root: &Path,
    location: engine::LspLocation,
) -> neoism_lua::LuaLspLocation {
    neoism_lua::LuaLspLocation {
        path: local_result_path(root, location.path),
        range: location.range.map(map_range),
        language: location.language,
    }
}

fn map_hover(root: &Path, hover: engine::LspHover) -> neoism_lua::LuaLspHover {
    neoism_lua::LuaLspHover {
        path: local_result_path(root, hover.path),
        contents: hover.contents,
        kind: hover.kind,
        range: hover.range.map(map_range),
        language: hover.language,
    }
}

fn map_signature_help(
    root: &Path,
    help: engine::LspSignatureHelp,
) -> neoism_lua::LuaLspSignatureHelp {
    neoism_lua::LuaLspSignatureHelp {
        path: local_result_path(root, help.path),
        signatures: help
            .signatures
            .into_iter()
            .map(|signature| neoism_lua::LuaLspSignature {
                label: signature.label,
                documentation: signature.documentation,
                parameters: signature
                    .parameters
                    .into_iter()
                    .map(|parameter| neoism_lua::LuaLspParameter {
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
    }
}

fn map_document_symbol(
    root: &Path,
    symbol: engine::LspDocumentSymbol,
) -> neoism_lua::LuaLspDocumentSymbol {
    neoism_lua::LuaLspDocumentSymbol {
        name: symbol.name,
        kind: symbol.kind,
        detail: symbol.detail,
        path: local_result_path(root, symbol.path),
        range: symbol.range.map(map_range),
        selection_range: symbol.selection_range.map(map_range),
        children: symbol
            .children
            .into_iter()
            .map(|child| map_document_symbol(root, child))
            .collect(),
        language: symbol.language,
    }
}

fn map_diagnostic(
    root: &Path,
    diagnostic: engine::LspDiagnostic,
) -> neoism_lua::LuaLspDiagnostic {
    neoism_lua::LuaLspDiagnostic {
        path: local_result_path(root, diagnostic.path),
        range: diagnostic.range.map(map_range),
        severity: diagnostic.severity,
        code: diagnostic.code,
        code_description: diagnostic.code_description,
        source: diagnostic.source,
        message: diagnostic.message,
        tags: diagnostic.tags,
        related_information: diagnostic
            .related_information
            .into_iter()
            .map(|related| neoism_lua::LuaLspRelatedInformation {
                path: local_result_path(root, related.path),
                range: related.range.map(map_range),
                message: related.message,
            })
            .collect(),
        data: diagnostic.data,
        language: diagnostic.language,
    }
}

fn map_client(status: engine::LspStatus) -> neoism_lua::LuaLspClient {
    let capabilities = status.capabilities;
    neoism_lua::LuaLspClient {
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
        capabilities: neoism_lua::LuaLspCapabilities {
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

pub(super) fn argument_u32(
    arguments: &serde_json::Value,
    names: &[&str],
    default: usize,
) -> Result<u32, String> {
    let value = match names.iter().find_map(|name| arguments.get(*name)) {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{} must be a non-negative integer", names[0]))?,
        None => default as u64,
    };
    u32::try_from(value)
        .map_err(|_| format!("{} exceeds the supported coordinate range", names[0]))
}

pub(super) fn argument_str<'a>(
    arguments: &'a serde_json::Value,
    name: &str,
) -> Result<Option<&'a str>, String> {
    arguments
        .get(name)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("{name} must be a string"))
        })
        .transpose()
}

impl Screen<'_> {
    pub(crate) fn dispatch_local_lua_lsp_command(
        &mut self,
        owner: neoism_lua::PluginOwner,
        id: String,
        target: neoism_lua::LuaLspTarget,
        root: PathBuf,
        server_id: String,
        command: serde_json::Value,
        mutation: neoism_lua::LuaLspMutation,
    ) -> Result<(), String> {
        let changed = mutation
            .changed_files
            .iter()
            .map(|file| canonical_key(Path::new(&file.path)))
            .collect::<std::collections::HashSet<_>>();
        let mut seen = std::collections::HashSet::new();
        let sync_documents = self
            .context_manager
            .all_grids()
            .iter()
            .flat_map(|grid| grid.contexts().values())
            .filter_map(|item| item.context().code.as_ref())
            .filter(|code| {
                !code_uses_host_lsp(code) && changed.contains(&canonical_key(&code.path))
            })
            .filter_map(|code| {
                let key = canonical_key(&code.path);
                seen.insert(key)
                    .then(|| (code.path.clone(), code.buffer.text()))
            })
            .collect();
        let job = LuaLspJob {
            owner,
            id,
            operation: neoism_lua::LuaLspOperation::ApplyCodeAction,
            file: PathBuf::from(&target.path),
            target,
            root,
            text: None,
            sync_documents,
            query: String::new(),
            new_name: None,
            action: None,
            committed: Some((server_id, command, mutation)),
            open_buffers: Vec::new(),
            window_id: self.context_manager.window_id(),
        };
        let proxy = self.context_manager.event_proxy_clone();
        let runtime = self.code_lsp_shared().runtime.clone();
        ensure_worker(proxy, runtime)
            .jobs
            .send(job)
            .map_err(|_| "the structured LSP worker is unavailable".to_string())
    }

    pub(crate) fn dispatch_local_lua_code_action(
        &mut self,
        owner: neoism_lua::PluginOwner,
        id: String,
        action: LocalLuaLspCodeAction,
    ) -> Result<neoism_lua::LuaLspTarget, String> {
        let target = action.target.clone();
        let route_id = usize::try_from(target.pane_id)
            .map_err(|_| "structured LSP target pane is invalid".to_string())?;
        let text = {
            let code = self
                .context_manager
                .get_by_route_id(route_id)
                .and_then(|item| item.context().code.as_ref())
                .ok_or_else(|| {
                    "structured LSP target buffer is no longer open".to_string()
                })?;
            if code_uses_host_lsp(code)
                || canonical_key(&code.path) != canonical_key(&action.file)
                || target.buffer_revision != Some(code.buffer.revision)
            {
                return Err(
                    "structured LSP target buffer changed before action selection".into(),
                );
            }
            code.buffer.text()
        };
        for open in &action.open_buffers {
            let current = self
                .context_manager
                .get_by_route_id(open.route_id)
                .and_then(|item| item.context().code.as_ref())
                .ok_or_else(|| {
                    "an open buffer changed before action selection".to_string()
                })?;
            if code_uses_host_lsp(current)
                || canonical_key(&current.path) != canonical_key(&open.path)
                || current.buffer.revision != open.revision
            {
                return Err("an open buffer changed before action selection".into());
            }
        }
        let job = LuaLspJob {
            owner,
            id,
            operation: neoism_lua::LuaLspOperation::ApplyCodeAction,
            target: target.clone(),
            root: action.root,
            file: action.file,
            text: Some(text),
            sync_documents: Vec::new(),
            query: String::new(),
            new_name: None,
            action: Some(action.item),
            committed: None,
            open_buffers: action.open_buffers,
            window_id: self.context_manager.window_id(),
        };
        let proxy = self.context_manager.event_proxy_clone();
        let runtime = self.code_lsp_shared().runtime.clone();
        ensure_worker(proxy, runtime)
            .jobs
            .send(job)
            .map_err(|_| "the structured LSP worker is unavailable".to_string())?;
        Ok(target)
    }

    pub(crate) fn apply_local_lua_lsp_mutation(
        &mut self,
        mutation: LocalLuaLspPreparedMutation,
    ) -> Result<neoism_lua::LuaLspMutation, String> {
        let canonical_root = std::fs::canonicalize(&mutation.root)
            .map_err(|error| format!("failed to resolve LSP workspace: {error}"))?;
        let target_route = usize::try_from(mutation.target.pane_id)
            .map_err(|_| "structured LSP target pane is invalid".to_string())?;
        if let Some(revision) = mutation.target.buffer_revision {
            let target = self
                .context_manager
                .get_by_route_id(target_route)
                .and_then(|item| item.context().code.as_ref())
                .ok_or_else(|| {
                    "structured LSP target buffer is no longer open".to_string()
                })?;
            if code_uses_host_lsp(target)
                || canonical_key(&target.path)
                    != canonical_key(Path::new(&mutation.target.path))
                || target.buffer.revision != revision
            {
                return Err("structured LSP target buffer changed before mutation".into());
            }
        }

        let touched = mutation
            .files
            .iter()
            .map(|file| canonical_key(&file.path))
            .collect::<std::collections::HashSet<_>>();
        for open in mutation
            .open_buffers
            .iter()
            .filter(|open| touched.contains(&canonical_key(&open.path)))
        {
            let current = self
                .context_manager
                .get_by_route_id(open.route_id)
                .and_then(|item| item.context().code.as_ref())
                .ok_or_else(|| {
                    "an LSP edit buffer was closed before mutation".to_string()
                })?;
            if code_uses_host_lsp(current)
                || canonical_key(&current.path) != canonical_key(&open.path)
                || current.buffer.revision != open.revision
            {
                return Err("an LSP edit buffer changed before mutation".into());
            }
        }

        let mut open_targets = Vec::new();
        let mut closed_targets = Vec::new();
        for file in &mutation.files {
            let canonical_path = std::fs::canonicalize(&file.path)
                .map_err(|error| format!("failed to resolve LSP edit path: {error}"))?;
            if !canonical_path.starts_with(&canonical_root) {
                return Err("LSP edit path is outside the captured workspace".into());
            }
            let captured = mutation
                .open_buffers
                .iter()
                .filter(|open| {
                    canonical_key(&open.path) == canonical_key(&canonical_path)
                })
                .collect::<Vec<_>>();
            let current_routes = self
                .context_manager
                .all_grids()
                .iter()
                .flat_map(|grid| grid.contexts().values())
                .filter_map(|item| {
                    item.context()
                        .code
                        .as_ref()
                        .filter(|code| {
                            !code_uses_host_lsp(code)
                                && canonical_key(&code.path)
                                    == canonical_key(&canonical_path)
                        })
                        .map(|_| item.context().route_id)
                })
                .collect::<Vec<_>>();
            if captured.is_empty() {
                if !current_routes.is_empty() {
                    return Err(
                        "a closed LSP edit target became open before mutation".into()
                    );
                }
                let expected = file.expected_closed_text.as_ref().ok_or_else(|| {
                    "closed LSP edit target is missing its captured content".to_string()
                })?;
                let current =
                    std::fs::read_to_string(&canonical_path).map_err(|error| {
                        format!("failed to read LSP edit target: {error}")
                    })?;
                if &current != expected {
                    return Err("a closed LSP edit target changed before mutation".into());
                }
                let mut buffer = neoism_ui::editor::code::CodeBuffer::from_text(&current);
                buffer.validate_text_edits(&file.edits)?;
                buffer.apply_text_edits(&file.edits);
                closed_targets.push((canonical_path, file.edits.len(), buffer.text()));
            } else {
                if current_routes.len() != captured.len()
                    || captured
                        .iter()
                        .any(|open| !current_routes.contains(&open.route_id))
                {
                    return Err(
                        "LSP edit buffer ownership changed before mutation".into()
                    );
                }
                for route_id in current_routes {
                    let code = self
                        .context_manager
                        .get_by_route_id(route_id)
                        .and_then(|item| item.context().code.as_ref())
                        .ok_or_else(|| "LSP edit buffer disappeared".to_string())?;
                    code.buffer.validate_text_edits(&file.edits)?;
                    open_targets.push((
                        route_id,
                        canonical_path.clone(),
                        file.edits.clone(),
                    ));
                }
            }
        }

        let mut changed_files = Vec::new();
        for (path, edit_count, text) in closed_targets {
            std::fs::write(&path, text).map_err(|error| {
                format!("failed to apply closed-file LSP edits: {error}")
            })?;
            changed_files.push(neoism_lua::LuaLspChangedFile {
                path: path.to_string_lossy().into_owned(),
                edit_count,
                applied_by: neoism_lua::LuaLspMutationOwner::Desktop,
            });
        }
        let mut reported_open = std::collections::HashSet::new();
        for (route_id, path, edits) in open_targets {
            let code = self
                .context_manager
                .get_by_route_id(route_id)
                .and_then(|item| item.context_mut().code.as_mut())
                .ok_or_else(|| {
                    "LSP edit buffer disappeared during mutation".to_string()
                })?;
            code.buffer.apply_text_edits(&edits);
            code.buffer.follow_cursor = true;
            if reported_open.insert(canonical_key(&path)) {
                changed_files.push(neoism_lua::LuaLspChangedFile {
                    path: path.to_string_lossy().into_owned(),
                    edit_count: edits.len(),
                    applied_by: neoism_lua::LuaLspMutationOwner::Frontend,
                });
            }
        }
        self.sync_active_code_modified();
        self.mark_dirty();
        Ok(neoism_lua::LuaLspMutation {
            title: mutation.title,
            changed_files,
            ran_command: false,
        })
    }

    pub(crate) fn dispatch_lua_lsp_request(
        &mut self,
        owner: neoism_lua::PluginOwner,
        id: String,
        operation: neoism_lua::LuaLspOperation,
        arguments: &serde_json::Value,
    ) -> Result<neoism_lua::LuaLspTarget, String> {
        if self.code_lsp_is_remote() {
            return self.dispatch_remote_lua_lsp_request(owner, id, operation, arguments);
        }
        let (root, focused_file) = self.code_lsp_target().ok_or_else(|| {
            "there is no focused code buffer with an LSP target".to_string()
        })?;
        if let Some(requested_root) = argument_str(arguments, "root")? {
            if Path::new(requested_root) != root {
                return Err(
                    "structured LSP requests cannot leave the focused workspace root"
                        .into(),
                );
            }
        }
        let requested_path = argument_str(arguments, "path")?.map(PathBuf::from);
        if requested_path.as_ref().is_some_and(|path| {
            path.components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        }) {
            return Err("structured LSP request paths cannot contain `..`".into());
        }
        let file = match requested_path {
            Some(path) if path.is_absolute() => path,
            Some(path) => root.join(path),
            None => focused_file,
        };
        if !file.starts_with(&root) {
            return Err(
                "structured LSP requests cannot leave the focused workspace root".into(),
            );
        }
        let code = self
            .context_manager
            .current()
            .code
            .as_ref()
            .ok_or_else(|| "there is no focused code buffer".to_string())?;
        let line = argument_u32(arguments, &["line"], code.buffer.cursor_line)?;
        let character =
            argument_u32(arguments, &["character", "column"], code.buffer.cursor_col)?;
        if operation == neoism_lua::LuaLspOperation::ApplyCodeAction {
            return Err("apply_code_action requires a retained action capability".into());
        }
        let new_name = argument_str(arguments, "newName")?.map(str::to_owned);
        if operation == neoism_lua::LuaLspOperation::Rename
            && new_name
                .as_deref()
                .is_none_or(|name| name.trim().is_empty())
        {
            return Err("rename requires a non-empty newName".into());
        }
        let mut open_buffers = Vec::new();
        let mut target_live: Option<(usize, u64, String)> = None;
        for item in self
            .context_manager
            .all_grids()
            .iter()
            .flat_map(|grid| grid.contexts().values())
        {
            let Some(open) = item.context().code.as_ref() else {
                continue;
            };
            if code_uses_host_lsp(open) || !open.path.starts_with(&root) {
                continue;
            }
            open_buffers.push(LocalLuaLspOpenBuffer {
                route_id: item.context().route_id,
                path: open.path.clone(),
                revision: open.buffer.revision,
            });
            if open.path == file {
                let candidate = (
                    item.context().route_id,
                    open.buffer.revision,
                    open.buffer.text(),
                );
                if target_live.as_ref().is_some_and(|existing| {
                    existing.1 != candidate.1 || existing.2 != candidate.2
                }) {
                    return Err(
                        "structured LSP target has conflicting open buffer owners".into(),
                    );
                }
                target_live = Some(candidate);
            }
        }
        let (pane_id, buffer_revision, text) = match target_live {
            Some((route_id, revision, text)) => {
                (route_id as u64, Some(revision), Some(text))
            }
            None => (self.context_manager.current().route_id as u64, None, None),
        };
        if operation == neoism_lua::LuaLspOperation::Format && buffer_revision.is_none() {
            return Err("structured format requires an open target buffer".into());
        }
        let target = neoism_lua::LuaLspTarget {
            root: root.to_string_lossy().into_owned(),
            path: file.to_string_lossy().into_owned(),
            line,
            character,
            buffer_revision,
            pane_id,
        };
        let job = LuaLspJob {
            owner,
            id,
            operation,
            target: target.clone(),
            root,
            file,
            text,
            sync_documents: Vec::new(),
            query: argument_str(arguments, "query")?
                .unwrap_or_default()
                .to_string(),
            new_name,
            action: None,
            committed: None,
            open_buffers,
            window_id: self.context_manager.window_id(),
        };
        let proxy = self.context_manager.event_proxy_clone();
        let runtime = self.code_lsp_shared().runtime.clone();
        ensure_worker(proxy, runtime)
            .jobs
            .send(job)
            .map_err(|_| "the structured LSP worker is unavailable".to_string())?;
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_ranges_cross_the_lua_boundary_as_zero_based_utf8_bytes() {
        let range = map_range(engine::LspRange {
            start: engine::LspPosition {
                line: 3,
                character: 5,
            },
            end: engine::LspPosition {
                line: 3,
                character: 9,
            },
        });
        assert_eq!(range.start.line, 2);
        assert_eq!(range.start.character, 4);
        assert_eq!(range.end.line, 2);
        assert_eq!(range.end.character, 8);
    }

    #[test]
    fn local_relative_result_paths_bind_to_the_captured_root() {
        assert_eq!(
            local_result_path(Path::new("/workspace"), "src/main.rs".into()),
            "/workspace/src/main.rs"
        );
    }

    #[test]
    fn malformed_coordinates_are_rejected_instead_of_using_the_cursor() {
        let arguments = serde_json::json!({ "line": -1, "character": "4" });
        assert_eq!(
            argument_u32(&arguments, &["line"], 8).unwrap_err(),
            "line must be a non-negative integer"
        );
        assert_eq!(
            argument_u32(&arguments, &["character", "column"], 2).unwrap_err(),
            "character must be a non-negative integer"
        );
    }

    #[test]
    fn structured_edits_reject_malformed_payloads_instead_of_dropping_them() {
        let edits = vec![serde_json::json!({
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0 }
            },
            "newText": "x"
        })];
        assert_eq!(
            parse_structured_text_edits(&edits).unwrap_err().0,
            "invalid_edit"
        );
        let workspace_edit = serde_json::json!({
            "changes": { "https://example.com/file.rs": edits }
        });
        assert_eq!(
            workspace_edit_file_edits_strict(&workspace_edit)
                .unwrap_err()
                .0,
            "invalid_edit_path"
        );
    }

    #[test]
    fn structured_edits_detect_resource_operations() {
        assert!(workspace_edit_has_resource_operations(&serde_json::json!({
            "documentChanges": [{
                "kind": "rename",
                "oldUri": "file:///tmp/old.rs",
                "newUri": "file:///tmp/new.rs"
            }]
        })));
    }
}
