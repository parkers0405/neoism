use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use tokio::sync::broadcast;

use anyhow::Context;
use neoism_agent_core::LspConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tool::{ToolContext, ToolExecutionResult};

#[path = "lsp_client.rs"]
mod lsp_client;
#[path = "lsp_languages.rs"]
mod lsp_languages;
#[path = "lsp_parse.rs"]
mod lsp_parse;
#[path = "lsp_query.rs"]
mod lsp_query;
#[path = "lsp_scan.rs"]
mod lsp_scan;
#[path = "lsp_service.rs"]
mod lsp_service;
#[path = "lsp_uri.rs"]
mod lsp_uri;
#[cfg(test)]
use lsp_client::read_lsp_message;
use lsp_languages::{LspOperation, WorkspaceScan, LANGUAGE_SPECS};
#[cfg(test)]
use lsp_query::query_workspace_symbols_with_command;
use lsp_query::{
    notify_document_touched, query_code_actions, query_definitions, query_diagnostics,
    query_document_formatting, query_document_symbols, query_hover,
    query_implementations, query_incoming_calls, query_outgoing_calls,
    query_prepare_call_hierarchy, query_references, query_workspace_symbols,
};
use lsp_scan::{
    command_available, detected_servers, file_query_specs, language_detected,
    normalized_file, normalized_root, scan_workspace, server_status,
};
use lsp_uri::path_to_file_uri;

const MAX_SCAN_FILES: usize = 10_000;
const MAX_EVIDENCE: usize = 8;
const MAX_SYMBOLS: usize = 100;
const MAX_COMPLETIONS: usize = 200;
const MAX_LSP_SERVERS_PER_QUERY: usize = 3;
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(3);
const SYMBOL_TIMEOUT: Duration = Duration::from_secs(5);
const DOCUMENT_TIMEOUT: Duration = Duration::from_secs(5);
const DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(2);
/// Latency ceiling for the post-edit `touch` step. Unlike the explicit
/// `diagnostics` tool (which can afford the full 2s), an edit/write/patch must
/// return promptly: we pull diagnostics (fast) when the server supports it, and
/// otherwise wait only briefly for a `publishDiagnostics` push. Slow flycheck
/// errors (rust-analyzer's `cargo check`) land in the cache and surface on the
/// next tool call — matching opencode's eventually-consistent diagnostics.
const TOUCH_DIAGNOSTIC_TIMEOUT: Duration = Duration::from_millis(600);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);
const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".venv",
    "venv",
    "__pycache__",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspStatus {
    pub id: String,
    pub name: String,
    pub status: LspServerState,
    pub language: String,
    pub command: Vec<String>,
    pub command_source: LspCommandSource,
    pub workspace: LspWorkspace,
    pub capabilities: LspCapabilities,
    pub detected: LspDetection,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LspServerState {
    Available,
    Connected,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LspCommandSource {
    Extension,
    Config,
    Path,
    Missing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspWorkspace {
    pub root: String,
    pub root_uri: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspCapabilities {
    pub workspace_symbols: bool,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub implementation: bool,
    pub call_hierarchy: bool,
    pub diagnostics: bool,
    pub document_symbols: bool,
    pub formatting: bool,
    pub code_actions: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspDetection {
    pub files: usize,
    pub markers: Vec<String>,
    pub extensions: BTreeMap<String, usize>,
    pub command_available: bool,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: Option<u32>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspLocation {
    pub path: String,
    pub range: Option<LspRange>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspDiagnostic {
    pub path: String,
    pub range: Option<LspRange>,
    pub severity: String,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspHover {
    pub path: String,
    pub contents: String,
    pub kind: Option<String>,
    pub range: Option<LspRange>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspCompletionItem {
    /// Display label (what the popup row shows).
    pub label: String,
    /// LSP CompletionItemKind mapped to a lowercase word ("function",
    /// "method", "variable", "keyword", …) for the popup icon/tag.
    pub kind: String,
    /// Right-hand detail (type signature / container).
    pub detail: Option<String>,
    /// Documentation (markdown or plaintext), if the server sent it inline.
    pub documentation: Option<String>,
    /// Text actually inserted on accept (`insertText`/`textEdit.newText`,
    /// falling back to `label`).
    pub insert_text: String,
    /// Text the client filters against as the user types (defaults to
    /// `label`).
    pub filter_text: Option<String>,
    /// Server-provided ordering key; the client sorts by this before label.
    pub sort_text: Option<String>,
    /// The server suggests this item be pre-selected.
    pub preselect: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspDocumentSymbol {
    pub name: String,
    pub kind: String,
    pub detail: Option<String>,
    pub path: String,
    pub range: Option<LspRange>,
    pub selection_range: Option<LspRange>,
    pub children: Vec<LspDocumentSymbol>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspCallHierarchyItem {
    pub name: String,
    pub kind: String,
    pub detail: Option<String>,
    pub path: String,
    pub range: Option<LspRange>,
    pub selection_range: Option<LspRange>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspCallHierarchyCall {
    pub item: LspCallHierarchyItem,
    pub ranges: Vec<LspRange>,
    pub direction: String,
    pub language: Option<String>,
}

pub fn status(directory: impl AsRef<Path>) -> Vec<LspStatus> {
    let root = normalized_root(directory.as_ref());
    let scan = scan_workspace(&root);
    let mut statuses = detected_servers(&root, &scan);
    let seen = statuses
        .iter()
        .map(|status| status.id.clone())
        .collect::<BTreeSet<_>>();
    statuses.extend(
        configured_servers(&root)
            .into_iter()
            .filter(|status| !seen.contains(&status.id)),
    );
    statuses
}

/// Status narrowed to the language(s) that can handle one concrete file.
/// This intentionally avoids recursively scanning the whole workspace; the
/// file extension is stronger evidence and this path runs in the editor's
/// periodic status refresh.
pub fn status_for_file(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<LspStatus> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let Some(extension) = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return status(root);
    };

    let matching_specs = LANGUAGE_SPECS
        .iter()
        .filter(|spec| spec.extensions.contains(&extension.as_str()))
        .collect::<Vec<_>>();
    if matching_specs.is_empty() {
        return status(root);
    }

    let mut evidence = WorkspaceScan {
        files: 1,
        ..WorkspaceScan::default()
    };
    evidence.extensions.insert(extension.clone(), 1);
    let mut statuses = matching_specs
        .iter()
        .map(|spec| server_status(&root, &evidence, spec))
        .collect::<Vec<_>>();
    let builtin_languages = matching_specs
        .iter()
        .map(|spec| spec.id)
        .collect::<BTreeSet<_>>();
    let seen = statuses
        .iter()
        .map(|status| status.id.clone())
        .collect::<BTreeSet<_>>();
    statuses.extend(configured_servers(&root).into_iter().filter(|status| {
        !seen.contains(&status.id)
            && (builtin_languages.contains(status.language.as_str())
                || status.detected.extensions.contains_key(&extension))
    }));
    statuses
}

/// File-scoped operations do not need a recursive workspace scan when a
/// built-in language claims the extension. Unknown/extensionless files retain
/// marker-based detection as a fallback.
fn workspace_scan_for_file(root: &Path, file: &Path) -> WorkspaceScan {
    if language_id_for_path(file).is_some() {
        WorkspaceScan::default()
    } else {
        scan_workspace(root)
    }
}

/// The built-in language id whose server handles `file`'s extension, if
/// any (e.g. `foo.rs` -> "rust"). Used to narrow the workspace server
/// list to the servers relevant to the open buffer for the status-bar
/// pill. Returns `None` for extensions no bundled spec claims.
pub fn language_id_for_path(file: impl AsRef<Path>) -> Option<&'static str> {
    let extension = file.as_ref().extension().and_then(|ext| ext.to_str())?;
    lsp_languages::LANGUAGE_SPECS
        .iter()
        .find(|spec| spec.extensions.contains(&extension))
        .map(|spec| spec.id)
}

/// Language ids with a live (spawned + initialized) LSP client under
/// `directory`. The status pill uses this to show a server as actually
/// "attached" rather than merely "available on PATH".
pub fn live_languages(directory: impl AsRef<Path>) -> std::collections::BTreeSet<String> {
    let root = normalized_root(directory.as_ref());
    lsp_service::service().live_languages(&root)
}

/// A `textDocument/publishDiagnostics` push from a language server, delivered
/// in REAL TIME (event-driven, no polling) to subscribers. This is the golden
/// path: the server pushes as it analyzes, we fan it out immediately.
#[derive(Clone, Debug)]
pub struct DiagnosticsEvent {
    pub root: PathBuf,
    pub language: String,
    pub file: String,
    pub diagnostics: Vec<LspDiagnostic>,
}

fn diagnostics_bus() -> &'static broadcast::Sender<DiagnosticsEvent> {
    static BUS: OnceLock<broadcast::Sender<DiagnosticsEvent>> = OnceLock::new();
    BUS.get_or_init(|| broadcast::channel(512).0)
}

/// Subscribe to real-time `publishDiagnostics` pushes from every LSP client.
/// The daemon drains this and forwards to the editor with zero polling.
pub fn subscribe_diagnostics() -> broadcast::Receiver<DiagnosticsEvent> {
    diagnostics_bus().subscribe()
}

/// Fan a raw `publishDiagnostics` notification onto the bus. Called by the LSP
/// client's reader thread the instant the server sends it.
pub(crate) fn dispatch_diagnostics(root: &Path, language: &str, params: &Value) {
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return;
    };
    let file = match lsp_uri::file_uri_to_path(uri) {
        Some(path) => path,
        None => return,
    };
    let diagnostics = lsp_parse::parse_diagnostics(root, &file, language, params.clone());
    // Keep the pull cache fresh from the push so `cached_diagnostics` readers
    // (web fetch, pill) don't need an aggressive `touch`.
    lsp_service::service().store_diagnostics(&file, diagnostics.clone());
    // send() only errors when there are no live receivers — harmless.
    let _ = diagnostics_bus().send(DiagnosticsEvent {
        root: root.to_path_buf(),
        language: language.to_string(),
        file: file.to_string_lossy().into_owned(),
        diagnostics,
    });
    if std::env::var_os("NEOISM_LSP_LOG").is_some() {
        eprintln!(
            "neoism::lsp publishDiagnostics: lang={language} file={} count={}",
            file.display(),
            params
                .get("diagnostics")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0),
        );
    }
}

fn configured_servers(root: &Path) -> Vec<LspStatus> {
    let Ok(loaded) = crate::config::load(&root.to_string_lossy()) else {
        return Vec::new();
    };
    let LspConfig::Servers(servers) = loaded.info.lsp else {
        return Vec::new();
    };
    servers
        .into_iter()
        .filter_map(|(id, value)| configured_server_status(root, &id, value))
        .collect()
}

fn configured_server_status(root: &Path, id: &str, value: Value) -> Option<LspStatus> {
    if value.as_bool() == Some(false) {
        return None;
    }
    let object = value.as_object()?;
    if object.get("enabled").and_then(Value::as_bool) == Some(false)
        || object.get("disabled").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let raw_command = configured_command(object.get("command")?)?;
    let (command, command_source) = resolve_lsp_command(id, raw_command);
    let available = command_source != LspCommandSource::Missing;
    let language = object
        .get("language")
        .or_else(|| object.get("languageId"))
        .and_then(Value::as_str)
        .unwrap_or(id)
        .to_string();
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(id)
        .to_string();
    Some(LspStatus {
        id: id.to_string(),
        name,
        status: if available {
            LspServerState::Available
        } else {
            LspServerState::Error
        },
        language,
        command: command.clone(),
        command_source,
        workspace: LspWorkspace {
            root: root.display().to_string(),
            root_uri: path_to_file_uri(root),
        },
        capabilities: configured_capabilities(object, available),
        detected: LspDetection {
            files: 0,
            markers: string_array(object.get("markers")),
            extensions: string_array(object.get("extensions"))
                .into_iter()
                .map(|extension| (extension, 0))
                .collect(),
            command_available: available,
            message: (!available).then(|| {
                format!(
                    "configured LSP server `{id}` command `{}` was not found in Neoism extensions or PATH",
                    command.first().cloned().unwrap_or_default()
                )
            }),
        },
    })
}

pub(crate) fn resolve_lsp_command(
    id: &str,
    mut command: Vec<String>,
) -> (Vec<String>, LspCommandSource) {
    let Some(first) = command.first_mut() else {
        return (command, LspCommandSource::Missing);
    };
    if command_bin_is_explicit(first) {
        let source = if command_available(first) {
            LspCommandSource::Config
        } else {
            LspCommandSource::Missing
        };
        return (command, source);
    }
    if let Ok(managed_bins) = neoism_extensions::managed_bin::managed_bin_map() {
        if let Some(managed_bin) = managed_bins
            .get(id)
            .or_else(|| managed_bins.get(first.as_str()))
        {
            // Only use the extension-installed binary if it ACTUALLY EXISTS.
            // A stale/incomplete `installed.json` entry (managed bin path that
            // isn't on disk) must NOT shadow a perfectly good system binary on
            // PATH — otherwise the engine reports "Binary missing" for a server
            // that is really running from PATH, and (if it ever spawned this
            // path) would fail to launch.
            if command_available(managed_bin) {
                *first = managed_bin.clone();
                return (command, LspCommandSource::Extension);
            }
        }
    }
    if command_available(first) {
        return (command, LspCommandSource::Path);
    }
    (command, LspCommandSource::Missing)
}

pub(crate) fn lsp_command_available(id: &str, command: &[&str]) -> bool {
    let command = command.iter().map(|part| (*part).to_string()).collect();
    resolve_lsp_command(id, command).1 != LspCommandSource::Missing
}

fn command_bin_is_explicit(bin: &str) -> bool {
    let path = Path::new(bin);
    path.is_absolute() || path.components().count() > 1
}

fn configured_command(value: &Value) -> Option<Vec<String>> {
    if let Some(command) = value.as_str() {
        let parts = command
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        return (!parts.is_empty()).then_some(parts);
    }
    let parts = value
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!parts.is_empty()).then_some(parts)
}

fn configured_capabilities(
    object: &serde_json::Map<String, Value>,
    available: bool,
) -> LspCapabilities {
    let enabled = |key: &str| {
        object
            .get("capabilities")
            .and_then(|capabilities| capabilities.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(true)
            && available
    };
    LspCapabilities {
        workspace_symbols: enabled("workspaceSymbols"),
        hover: enabled("hover"),
        definition: enabled("definition"),
        references: enabled("references"),
        implementation: enabled("implementation"),
        call_hierarchy: enabled("callHierarchy"),
        diagnostics: enabled("diagnostics"),
        document_symbols: enabled("documentSymbols"),
        formatting: enabled("formatting"),
        code_actions: enabled("codeActions"),
    }
}

fn unavailable_lsp_error(
    root: &Path,
    file: Option<&Path>,
    operation: LspOperation,
) -> Option<anyhow::Error> {
    let scan = scan_workspace(root);
    let mut matching = Vec::new();
    let mut missing = Vec::new();

    for spec in LANGUAGE_SPECS.iter() {
        if !spec_supports_operation(spec, operation) {
            continue;
        }
        let matches = if let Some(file) = file {
            let extension = file.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            spec.extensions.contains(&extension)
        } else {
            language_detected(spec, &scan)
        };
        if !matches {
            continue;
        }
        matching.push(spec);
        if !command_available(spec.command[0]) {
            missing.push(spec);
        }
    }

    if matching.is_empty() || missing.len() != matching.len() {
        return None;
    }

    let target = file
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .unwrap_or_else(|| root.display().to_string());
    let servers = missing
        .iter()
        .map(|spec| format!("{} (`{}`)", spec.name, spec.command.join(" ")))
        .collect::<Vec<_>>()
        .join(", ");
    Some(anyhow::anyhow!(
        "LSP unavailable for {operation:?} on {target}: detected matching language server(s), but none are available on PATH: {servers}. Run the lsp status operation for install/status details."
    ))
}

fn spec_supports_operation(
    spec: &lsp_languages::LanguageSpec,
    operation: LspOperation,
) -> bool {
    match operation {
        LspOperation::WorkspaceSymbols => spec.workspace_symbols,
        // Every bundled language server provides completion; there is no
        // per-spec flag, and the runtime capability check
        // (`completion_provider`) gates the actual request.
        LspOperation::Completion => true,
        LspOperation::Hover => spec.hover,
        LspOperation::Definition => spec.definition,
        LspOperation::References => spec.references,
        LspOperation::Implementation => spec.implementation,
        LspOperation::CallHierarchy => spec.call_hierarchy,
        LspOperation::Diagnostics => spec.diagnostics,
        LspOperation::DocumentSymbols => spec.document_symbols,
        LspOperation::Formatting | LspOperation::CodeActions => true,
        LspOperation::Rename => spec.rename,
    }
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn workspace_symbols(
    directory: impl AsRef<Path>,
    query: &str,
) -> Vec<WorkspaceSymbol> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let root = normalized_root(directory.as_ref());
    let scan = scan_workspace(&root);
    let mut symbols = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in LANGUAGE_SPECS
        .iter()
        .filter(|spec| spec.workspace_symbols && language_detected(spec, &scan))
        .filter(|spec| command_available(spec.command[0]))
        .take(MAX_LSP_SERVERS_PER_QUERY)
    {
        match lsp_service::service()
            .workspace_symbols(&root, query, spec)
            .or_else(|_| query_workspace_symbols(&root, query, spec))
        {
            Ok(found) => {
                for symbol in found {
                    let key = (
                        symbol.name.clone(),
                        symbol.path.clone(),
                        symbol.line.unwrap_or_default(),
                    );
                    if seen.insert(key) {
                        symbols.push(symbol);
                    }
                    if symbols.len() >= MAX_SYMBOLS {
                        return symbols;
                    }
                }
            }
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "workspace symbol query failed"
                );
            }
        }
    }

    symbols
}

fn position_candidates(line: u32, character: u32) -> Vec<(u32, u32)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for candidate in [
        (line, character),
        (line.saturating_add(1), character.saturating_add(1)),
        (line.saturating_add(1), character),
        (line, character.saturating_add(1)),
        (line.saturating_sub(1), character.saturating_sub(1)),
        (line.saturating_sub(1), character),
        (line, character.saturating_sub(1)),
    ] {
        if seen.insert(candidate) {
            out.push(candidate);
        }
    }
    out
}

pub fn hover(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspHover> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&scan, &file, LspOperation::Hover) {
        let found = position_candidates(line, character).into_iter().find_map(
            |(line, character)| {
                lsp_service::service()
                    .hover(&root, &file, line, character, spec)
                    .or_else(|_| query_hover(&root, &file, line, character, spec))
                    .ok()
                    .filter(|found| !found.is_empty())
            },
        );
        match found {
            Some(found) => {
                for hover in found {
                    let key = (
                        hover.path.clone(),
                        hover.contents.clone(),
                        hover.range.clone(),
                    );
                    if seen.insert(key) {
                        results.push(hover);
                    }
                }
            }
            None => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    "hover query failed"
                );
            }
        }
    }

    results
}

/// Completion items at `(line, character)` from the language server for
/// `file`. Exact cursor position (no multi-position fallback — completion is
/// position-precise). Returns the first non-empty server response.
pub fn completion(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    text: Option<&str>,
) -> Vec<LspCompletionItem> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    for spec in file_query_specs(&scan, &file, LspOperation::Completion) {
        match lsp_service::service().completion(&root, &file, line, character, text, spec)
        {
            Ok(items) if !items.is_empty() => return items,
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(language = spec.id, %error, "completion query failed");
            }
        }
    }
    Vec::new()
}

/// Trigger characters the file's language server advertises (e.g. `.`, `::`),
/// so the client knows when to auto-open completion mid-token. Empty when no
/// server is available or it advertises none.
pub fn completion_trigger_characters(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<String> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    for spec in file_query_specs(&scan, &file, LspOperation::Completion) {
        if let Ok(chars) =
            lsp_service::service().completion_trigger_characters(&root, spec)
        {
            if !chars.is_empty() {
                return chars;
            }
        }
    }
    Vec::new()
}

pub fn definitions(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&scan, &file, LspOperation::Definition) {
        let found = position_candidates(line, character).into_iter().find_map(
            |(line, character)| {
                lsp_service::service()
                    .definitions(&root, &file, line, character, spec)
                    .or_else(|_| query_definitions(&root, &file, line, character, spec))
                    .ok()
                    .filter(|found| !found.is_empty())
            },
        );
        match found {
            Some(found) => {
                for location in found {
                    let key = (location.path.clone(), location.range.clone());
                    if seen.insert(key) {
                        results.push(location);
                    }
                    if results.len() >= MAX_SYMBOLS {
                        return results;
                    }
                }
            }
            None => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    "definition query failed"
                );
            }
        }
    }

    results
}

pub fn references(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&scan, &file, LspOperation::References) {
        match lsp_service::service()
            .references(&root, &file, line, character, spec)
            .or_else(|_| query_references(&root, &file, line, character, spec))
        {
            Ok(found) => {
                for location in found {
                    let key = (location.path.clone(), location.range.clone());
                    if seen.insert(key) {
                        results.push(location);
                    }
                    if results.len() >= MAX_SYMBOLS {
                        return results;
                    }
                }
            }
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "references query failed"
                );
            }
        }
    }

    results
}

pub fn implementations(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&scan, &file, LspOperation::Implementation) {
        match lsp_service::service()
            .implementations(&root, &file, line, character, spec)
            .or_else(|_| query_implementations(&root, &file, line, character, spec))
        {
            Ok(found) => {
                for location in found {
                    let key = (location.path.clone(), location.range.clone());
                    if seen.insert(key) {
                        results.push(location);
                    }
                    if results.len() >= MAX_SYMBOLS {
                        return results;
                    }
                }
            }
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "implementation query failed"
                );
            }
        }
    }

    results
}

pub fn prepare_call_hierarchy(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspCallHierarchyItem> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&scan, &file, LspOperation::CallHierarchy) {
        match lsp_service::service()
            .prepare_call_hierarchy(&root, &file, line, character, spec)
            .or_else(|_| {
                query_prepare_call_hierarchy(&root, &file, line, character, spec)
            }) {
            Ok(found) => {
                for item in found {
                    let key = (item.name.clone(), item.path.clone(), item.range.clone());
                    if seen.insert(key) {
                        results.push(item);
                    }
                    if results.len() >= MAX_SYMBOLS {
                        return results;
                    }
                }
            }
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "prepare call hierarchy query failed"
                );
            }
        }
    }

    results
}

pub fn incoming_calls(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspCallHierarchyCall> {
    call_hierarchy_calls(directory, file, line, character, true)
}

pub fn outgoing_calls(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspCallHierarchyCall> {
    call_hierarchy_calls(directory, file, line, character, false)
}

fn call_hierarchy_calls(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    incoming: bool,
) -> Vec<LspCallHierarchyCall> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&scan, &file, LspOperation::CallHierarchy) {
        let found = lsp_service::service()
            .call_hierarchy_calls(&root, &file, line, character, spec, incoming)
            .or_else(|_| {
                if incoming {
                    query_incoming_calls(&root, &file, line, character, spec)
                } else {
                    query_outgoing_calls(&root, &file, line, character, spec)
                }
            });
        match found {
            Ok(found) => {
                for call in found {
                    let key = (
                        call.direction.clone(),
                        call.item.name.clone(),
                        call.item.path.clone(),
                        call.ranges.clone(),
                    );
                    if seen.insert(key) {
                        results.push(call);
                    }
                    if results.len() >= MAX_SYMBOLS {
                        return results;
                    }
                }
            }
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "call hierarchy query failed"
                );
            }
        }
    }

    results
}

pub fn diagnostics(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<LspDiagnostic> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&scan, &file, LspOperation::Diagnostics) {
        match lsp_service::service()
            .diagnostics(&root, &file, spec)
            .or_else(|_| query_diagnostics(&root, &file, spec))
        {
            Ok(found) => {
                for diagnostic in found {
                    let key = (
                        diagnostic.path.clone(),
                        diagnostic.range.clone(),
                        diagnostic.severity.clone(),
                        diagnostic.message.clone(),
                    );
                    if seen.insert(key) {
                        results.push(diagnostic);
                    }
                    if results.len() >= MAX_SYMBOLS {
                        return results;
                    }
                }
            }
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "diagnostics query failed"
                );
            }
        }
    }

    results
}

/// Cached diagnostics for a single file, with no language-server round-trip.
/// Mirrors opencode's `lsp.diagnostics()` reading each client's already-published
/// `client.diagnostics` map: the blocking wait happens once in [`touch_document`]
/// (opencode's `touchFile(_, "document")`), and this just reads the result.
pub fn cached_diagnostics(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<LspDiagnostic> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    lsp_service::service().cached_diagnostics(&file)
}

/// Snapshot of every file the language servers have published diagnostics for,
/// limited to the given workspace. Mirrors opencode's `lsp.diagnostics()` record
/// of all known diagnostics (we never spawn a server here — it only reads the
/// cache that prior touch/diagnostics queries populated).
pub fn cached_project_diagnostics(
    directory: impl AsRef<Path>,
) -> Vec<(PathBuf, Vec<LspDiagnostic>)> {
    let root = normalized_root(directory.as_ref());
    lsp_service::service()
        .cached_diagnostics_snapshot()
        .into_iter()
        .filter(|(path, _)| path.starts_with(&root))
        .collect()
}

pub(crate) fn tool_result_has_diagnostics(
    result: &crate::tool::ToolExecutionResult,
) -> bool {
    result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("diagnostics"))
        .is_some()
}

pub fn shutdown_all() {
    lsp_service::service().shutdown_all();
}

pub fn document_symbols(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<LspDocumentSymbol> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();

    for spec in file_query_specs(&scan, &file, LspOperation::DocumentSymbols) {
        match lsp_service::service()
            .document_symbols(&root, &file, spec)
            .or_else(|_| query_document_symbols(&root, &file, spec))
        {
            Ok(found) => {
                results.extend(found);
                if results.len() >= MAX_SYMBOLS {
                    results.truncate(MAX_SYMBOLS);
                    return results;
                }
            }
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "document symbol query failed"
                );
            }
        }
    }

    results
}

pub fn formatting(directory: impl AsRef<Path>, file: impl AsRef<Path>) -> Vec<Value> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();

    for spec in file_query_specs(&scan, &file, LspOperation::Formatting) {
        match lsp_service::service()
            .formatting(&root, &file, spec)
            .or_else(|_| query_document_formatting(&root, &file, spec))
        {
            Ok(result) => results.push(json!({
                "language": spec.id,
                "path": file.display().to_string(),
                "edits": result,
            })),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "document formatting query failed"
                );
            }
        }
    }

    results
}

pub fn code_actions(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<Value> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();

    for spec in file_query_specs(&scan, &file, LspOperation::CodeActions) {
        match lsp_service::service()
            .code_actions(&root, &file, line, character, spec)
            .or_else(|_| query_code_actions(&root, &file, line, character, spec))
        {
            Ok(result) => results.push(json!({
                "language": spec.id,
                "path": file.display().to_string(),
                "actions": result,
            })),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "code action query failed"
                );
            }
        }
    }

    results
}

pub fn resolve_code_action(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    action: Value,
) -> Option<Value> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);

    for spec in file_query_specs(&scan, &file, LspOperation::Rename) {
        match lsp_service::service().resolve_code_action(&root, spec, action.clone()) {
            Ok(resolved) => return Some(resolved),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "code action resolve failed"
                );
            }
        }
    }

    None
}

pub fn execute_command(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    command: Value,
) -> Option<Value> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);

    for spec in file_query_specs(&scan, &file, LspOperation::CodeActions) {
        match lsp_service::service().execute_command(&root, spec, command.clone()) {
            Ok(result) => return Some(result),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "workspace execute command failed"
                );
            }
        }
    }

    None
}

pub fn rename(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    new_name: &str,
) -> Vec<Value> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();

    for spec in file_query_specs(&scan, &file, LspOperation::Rename) {
        match lsp_service::service().rename(&root, &file, line, character, new_name, spec)
        {
            Ok(result) if !result.is_null() => results.push(json!({
                "language": spec.id,
                "path": file.display().to_string(),
                "edit": result,
            })),
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "rename query failed"
                );
            }
        }
    }

    results
}

pub fn touch_document(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    text: Option<&str>,
) -> Vec<Value> {
    let diagnostics = touch_document_diagnostics(directory.as_ref(), file.as_ref(), text);
    diagnostics
        .into_iter()
        .map(|(language, path, cached_diagnostics)| {
            json!({
                "language": language,
                "path": path.display().to_string(),
                "notified": true,
                "cachedDiagnostics": cached_diagnostics.len(),
            })
        })
        .collect()
}

pub fn touch_document_diagnostics(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    text: Option<&str>,
) -> Vec<(String, PathBuf, Vec<LspDiagnostic>)> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut results = Vec::new();

    for spec in file_query_specs(&scan, &file, LspOperation::Diagnostics) {
        match lsp_service::service()
            .touch(&root, &file, text, spec)
            .or_else(|_| {
                notify_document_touched(&root, &file, text, spec).map(|_| Vec::new())
            }) {
            Ok(diagnostics) => {
                results.push((spec.id.to_string(), file.clone(), diagnostics))
            }
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command,
                    error = %error,
                    "document touch notification failed"
                );
            }
        }
    }

    results
}

/// Open/update the document in the language server (didOpen/didChange) so it
/// re-analyzes and PUSHES `publishDiagnostics` — which arrive on the event bus
/// ([`subscribe_diagnostics`]). Fire-and-forget: no diagnostics are returned
/// or waited for here. Returns the language ids that were synced.
pub fn sync_document(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    text: Option<&str>,
) -> Vec<String> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let scan = workspace_scan_for_file(&root, &file);
    let mut synced = Vec::new();
    for spec in file_query_specs(&scan, &file, LspOperation::Diagnostics) {
        match lsp_service::service().sync(&root, &file, text, spec) {
            Ok(()) => synced.push(spec.id.to_string()),
            Err(error) => {
                if std::env::var_os("NEOISM_LSP_LOG").is_some() {
                    eprintln!(
                        "neoism::lsp sync failed: lang={} file={} error={error}",
                        spec.id,
                        file.display()
                    );
                }
            }
        }
    }
    synced
}

pub(crate) async fn lsp_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let operation =
        string_arg_either_many(&arguments, &["operation", "op", "command"])
            .ok_or_else(|| anyhow::anyhow!("tool argument operation is required"))?;
    match normalize_operation(&operation).as_str() {
        "status" => {
            context.ensure_allowed("lsp", "*")?;
            let result = status(&context.cwd);
            tool_result("LSP status", "status", json!(result))
        }
        "workspace_symbol" => {
            context.ensure_allowed("lsp", "*")?;
            let query = string_arg_either_many(&arguments, &["query", "symbol", "name"])
                .ok_or_else(|| anyhow::anyhow!("tool argument query is required"))?;
            let result = workspace_symbols(&context.cwd, &query);
            if result.is_empty() {
                if let Some(error) = unavailable_lsp_error(
                    &normalized_root(&context.cwd),
                    None,
                    LspOperation::WorkspaceSymbols,
                ) {
                    return Err(error);
                }
            }
            tool_result("LSP workspace symbols", "workspaceSymbol", json!(result))
        }
        "hover" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let (line, character) = position_args(&arguments)?;
            let result = hover(&context.cwd, &file, line, character);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::Hover, &result)?;
            tool_result("LSP hover", "hover", json!(result))
        }
        "definition" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let (line, character) = position_args(&arguments)?;
            let result = definitions(&context.cwd, &file, line, character);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::Definition, &result)?;
            tool_result("LSP definitions", "goToDefinition", json!(result))
        }
        "references" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let (line, character) = position_args(&arguments)?;
            let result = references(&context.cwd, &file, line, character);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::References, &result)?;
            tool_result("LSP references", "findReferences", json!(result))
        }
        "implementation" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let (line, character) = position_args(&arguments)?;
            let result = implementations(&context.cwd, &file, line, character);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::Implementation, &result)?;
            tool_result("LSP implementations", "goToImplementation", json!(result))
        }
        "prepare_call_hierarchy" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let (line, character) = position_args(&arguments)?;
            let result = prepare_call_hierarchy(&context.cwd, &file, line, character);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::CallHierarchy, &result)?;
            tool_result(
                "LSP call hierarchy",
                "prepareCallHierarchy",
                json!(result),
            )
        }
        "incoming_calls" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let (line, character) = position_args(&arguments)?;
            let result = incoming_calls(&context.cwd, &file, line, character);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::CallHierarchy, &result)?;
            tool_result("LSP incoming calls", "incomingCalls", json!(result))
        }
        "outgoing_calls" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let (line, character) = position_args(&arguments)?;
            let result = outgoing_calls(&context.cwd, &file, line, character);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::CallHierarchy, &result)?;
            tool_result("LSP outgoing calls", "outgoingCalls", json!(result))
        }
        "diagnostics" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let result = diagnostics(&context.cwd, &file);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::Diagnostics, &result)?;
            tool_result("LSP diagnostics", "diagnostics", json!(result))
        }
        "document_symbol" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let result = document_symbols(&context.cwd, &file);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::DocumentSymbols, &result)?;
            tool_result("LSP document symbols", "documentSymbol", json!(result))
        }
        "formatting" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let result = formatting(&context.cwd, &file);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::Formatting, &result)?;
            tool_result("LSP formatting", "formatting", json!(result))
        }
        "code_action" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let (line, character) = position_args(&arguments)?;
            let result = code_actions(&context.cwd, &file, line, character);
            ensure_available_if_empty(&context.cwd, &file, LspOperation::CodeActions, &result)?;
            tool_result("LSP code actions", "codeAction", json!(result))
        }
        "touch" => {
            let file = checked_lsp_file(&context, &file_arg(&arguments)?)?;
            let text = string_arg_either_many(&arguments, &["text", "content"]);
            let result = touch_document(&context.cwd, &file, text.as_deref());
            ensure_available_if_empty(&context.cwd, &file, LspOperation::Diagnostics, &result)?;
            tool_result("LSP touch", "touch", json!(result))
        }
        other => anyhow::bail!(
            "unsupported lsp operation {other}. Supported operations: status, workspaceSymbol, hover, goToDefinition, findReferences, goToImplementation, prepareCallHierarchy, incomingCalls, outgoingCalls, diagnostics, documentSymbol, formatting, codeAction, touch"
        ),
    }
}

fn checked_lsp_file(context: &ToolContext, raw: &str) -> anyhow::Result<PathBuf> {
    let base = context.cwd.canonicalize().with_context(|| {
        format!(
            "failed to resolve project directory {}",
            context.cwd.display()
        )
    })?;
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        base.join(raw)
    };
    let path = candidate
        .canonicalize()
        .with_context(|| format!("failed to resolve path {}", candidate.display()))?;
    if !path.starts_with(&base) {
        context.ensure_explicit_allowed(
            "external_directory",
            &external_directory_pattern(&path),
        )?;
    }
    context.ensure_allowed("lsp", raw)?;
    Ok(path)
}

fn external_directory_pattern(path: &Path) -> String {
    let directory = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    format!("{}/*", directory.display())
}

fn tool_result(
    title: &str,
    operation: &str,
    result: Value,
) -> anyhow::Result<ToolExecutionResult> {
    Ok(ToolExecutionResult {
        title: title.to_string(),
        output: serde_json::to_string_pretty(&result)?,
        metadata: Some(json!({
            "lsp": {
                "operation": operation,
                "result": result,
            }
        })),
    })
}

fn ensure_available_if_empty<T>(
    directory: &Path,
    file: &Path,
    operation: LspOperation,
    result: &[T],
) -> anyhow::Result<()> {
    if !result.is_empty() {
        return Ok(());
    }
    let root = normalized_root(directory);
    if let Some(error) = unavailable_lsp_error(&root, Some(file), operation) {
        return Err(error);
    }
    Ok(())
}

fn normalize_operation(operation: &str) -> String {
    let normalized = operation
        .trim()
        .replace(['-', ' '], "_")
        .to_ascii_lowercase();
    match normalized.as_str() {
        "workspacesymbol" | "workspace_symbols" | "workspace_symbol" => {
            "workspace_symbol".to_string()
        }
        "gotodefinition" | "go_to_definition" | "definition" | "definitions" => {
            "definition".to_string()
        }
        "findreferences" | "find_references" | "reference" | "references" => {
            "references".to_string()
        }
        "gotoimplementation"
        | "go_to_implementation"
        | "implementation"
        | "implementations" => "implementation".to_string(),
        "preparecallhierarchy" | "prepare_call_hierarchy" | "call_hierarchy" => {
            "prepare_call_hierarchy".to_string()
        }
        "incomingcalls" | "incoming_calls" => "incoming_calls".to_string(),
        "outgoingcalls" | "outgoing_calls" => "outgoing_calls".to_string(),
        "diagnostic" | "diagnostics" => "diagnostics".to_string(),
        "documentsymbol" | "document_symbols" | "document_symbol" => {
            "document_symbol".to_string()
        }
        "format" | "formatting" | "documentformatting" | "document_formatting" => {
            "formatting".to_string()
        }
        "codeaction" | "code_actions" | "code_action" | "quickfix" | "quick_fix" => {
            "code_action".to_string()
        }
        "touch" | "didopen" | "didchange" | "didsave" | "did_open" | "did_change"
        | "did_save" => "touch".to_string(),
        other => other.to_string(),
    }
}

fn file_arg(arguments: &Value) -> anyhow::Result<String> {
    string_arg_either_many(arguments, &["file", "path", "filePath"])
        .ok_or_else(|| anyhow::anyhow!("tool argument file is required"))
}

fn position_args(arguments: &Value) -> anyhow::Result<(u32, u32)> {
    let line = u32_arg(arguments, "line")
        .ok_or_else(|| anyhow::anyhow!("tool argument line is required"))?;
    let character = u32_arg(arguments, "character")
        .or_else(|| u32_arg(arguments, "column"))
        .ok_or_else(|| anyhow::anyhow!("tool argument character is required"))?;
    Ok((line, character))
}

fn string_arg_either_many(arguments: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| arguments.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn u32_arg(arguments: &Value, key: &str) -> Option<u32> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn capability_enabled(value: &Value) -> bool {
    match value {
        Value::Bool(enabled) => *enabled,
        Value::Null => false,
        Value::Object(_) => true,
        _ => true,
    }
}

#[cfg(test)]
#[path = "lsp_tests.rs"]
mod tests;
