use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use tokio::sync::broadcast;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tool::{ToolContext, ToolExecutionResult};

#[path = "lsp_adapters.rs"]
mod lsp_adapters;
#[path = "lsp_client.rs"]
mod lsp_client;
#[path = "lsp_languages.rs"]
mod lsp_languages;
#[path = "lsp_parse.rs"]
mod lsp_parse;
#[path = "lsp_position.rs"]
mod lsp_position;
#[cfg(test)]
#[path = "lsp_query.rs"]
mod lsp_query;
#[path = "lsp_scan.rs"]
mod lsp_scan;
#[path = "lsp_service.rs"]
mod lsp_service;
#[path = "lsp_uri.rs"]
mod lsp_uri;
use lsp_adapters::{
    adapters_for_root, best_route_in, AdapterOrigin, LanguageAdapter,
    ResolvedLanguageRoute, ResolvedLspTransport,
};
#[cfg(test)]
use lsp_client::read_lsp_message;
use lsp_languages::{LspOperation, WorkspaceScan, LANGUAGE_SPECS};
#[cfg(test)]
use lsp_query::query_workspace_symbols_with_command;
use lsp_scan::{
    adapter_endpoint_available, command_available, detected_servers,
    file_lifecycle_specs, file_query_specs, language_detected, normalized_file,
    normalized_root, operation_supported, scan_workspace, server_status_for_file,
};
use lsp_uri::path_to_file_uri;

const MAX_SCAN_FILES: usize = 10_000;
const MAX_EVIDENCE: usize = 8;
const MAX_SYMBOLS: usize = 100;
const MAX_COMPLETIONS: usize = 200;
const MAX_LSP_SERVERS_PER_QUERY: usize = 3;
// Initialization can legitimately include cold JVM startup, Haskell package
// discovery, or rust-analyzer workspace loading. Keep it bounded without
// treating normal cold starts as broken servers.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);
// A TCP endpoint that is not listening should fail quickly (not inherit the
// slower initialization budget used after a connection is established).
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
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
    /// A built-in endpoint supplied by the host application (for example,
    /// Godot's editor-owned TCP language server), not an installed binary.
    BuiltIn,
    Extension,
    Config,
    Path,
    Missing,
}

/// Public, read-only view of the runtime adapter registry. Extension/catalog
/// UI consumes this instead of maintaining a second list of languages that
/// may or may not actually attach.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspAdapterMetadata {
    pub id: String,
    pub name: String,
    pub origin: LspAdapterOrigin,
    pub transport: LspAdapterTransport,
    /// Exact package/executable pairs that can supply this adapter's stdio
    /// command. Catalog consumers use this to avoid duplicating an installable
    /// package row with an adapter-only integration row.
    pub catalog_packages: Vec<LspCatalogPackageMetadata>,
    pub routes: Vec<LspLanguageRouteMetadata>,
    pub markers: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub capabilities: LspCapabilities,
    pub initialization_options: Option<Value>,
    pub settings: Option<Value>,
    pub configuration_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspCatalogPackageMetadata {
    pub package_id: String,
    pub executable: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LspAdapterOrigin {
    BuiltIn,
    Configured,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LspAdapterTransport {
    Stdio {
        command: Vec<String>,
    },
    Tcp {
        default_host: String,
        default_port: u16,
        host_env: Option<String>,
        port_env: Option<String>,
    },
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspLanguageRouteMetadata {
    pub id: String,
    pub document_language_id: String,
    pub extensions: Vec<String>,
    pub filename_patterns: Vec<String>,
}

pub fn language_server_adapters() -> Vec<LspAdapterMetadata> {
    LANGUAGE_SPECS
        .iter()
        .map(LanguageAdapter::from_builtin)
        .map(|adapter| adapter_metadata(&adapter))
        .collect()
}

/// The complete runtime registry for one workspace, after project config has
/// added, replaced, disabled, or overridden adapters.
pub fn language_server_adapters_for(
    directory: impl AsRef<Path>,
) -> Vec<LspAdapterMetadata> {
    let root = normalized_root(directory.as_ref());
    adapters_for_root(&root)
        .iter()
        .map(adapter_metadata)
        .collect()
}

fn adapter_metadata(adapter: &LanguageAdapter) -> LspAdapterMetadata {
    let (transport, environment) = match &adapter.transport {
        ResolvedLspTransport::Stdio { command, env } => (
            LspAdapterTransport::Stdio {
                command: command.clone(),
            },
            env.clone(),
        ),
        ResolvedLspTransport::Tcp { host, port, .. } => (
            LspAdapterTransport::Tcp {
                default_host: host.clone(),
                default_port: *port,
                host_env: None,
                port_env: None,
            },
            BTreeMap::new(),
        ),
        ResolvedLspTransport::Invalid => (LspAdapterTransport::Invalid, BTreeMap::new()),
    };
    LspAdapterMetadata {
        id: adapter.id.clone(),
        name: adapter.name.clone(),
        origin: match adapter.origin {
            AdapterOrigin::BuiltIn => LspAdapterOrigin::BuiltIn,
            AdapterOrigin::Configured => LspAdapterOrigin::Configured,
        },
        transport,
        catalog_packages: adapter
            .catalog_packages
            .iter()
            .map(|package| LspCatalogPackageMetadata {
                package_id: package.package_id.clone(),
                executable: package.executable.clone(),
            })
            .collect(),
        routes: adapter
            .routes
            .iter()
            .map(|route| LspLanguageRouteMetadata {
                id: route.id.clone(),
                document_language_id: route.document_language_id.clone(),
                extensions: route.extensions.clone(),
                filename_patterns: route.filename_patterns.clone(),
            })
            .collect(),
        markers: adapter.markers.clone(),
        environment,
        capabilities: LspCapabilities {
            workspace_symbols: adapter.workspace_symbols,
            completion: adapter.completion,
            hover: adapter.hover,
            definition: adapter.definition,
            references: adapter.references,
            implementation: adapter.implementation,
            call_hierarchy: adapter.call_hierarchy,
            diagnostics: adapter.diagnostics,
            document_symbols: adapter.document_symbols,
            formatting: adapter.formatting,
            code_actions: adapter.code_actions,
            rename: adapter.rename,
        },
        initialization_options: adapter.initialization_options.clone(),
        settings: adapter.settings.clone(),
        configuration_error: adapter.configuration_error.clone(),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspWorkspace {
    pub root: String,
    pub root_uri: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspCapabilities {
    pub workspace_symbols: bool,
    pub completion: bool,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub implementation: bool,
    pub call_hierarchy: bool,
    pub diagnostics: bool,
    pub document_symbols: bool,
    pub formatting: bool,
    pub code_actions: bool,
    pub rename: bool,
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
    pub code_description: Option<String>,
    pub source: Option<String>,
    pub message: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub related_information: Vec<LspDiagnosticRelatedInformation>,
    /// Opaque server-owned payload. Some servers require this exact value in
    /// the diagnostic context before they will return a matching code action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LspDiagnosticRelatedInformation {
    pub path: String,
    pub range: Option<LspRange>,
    pub message: String,
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
    /// Exact adapter/server that produced the item. Completion resolve must go
    /// back to this client in a multi-server workspace.
    pub server_id: Option<String>,
    /// Display label (what the popup row shows).
    pub label: String,
    /// LSP CompletionItemKind mapped to a lowercase word ("function",
    /// "method", "variable", "keyword", …) for the popup icon/tag.
    pub kind: String,
    /// Right-hand detail (type signature / container).
    pub detail: Option<String>,
    /// Documentation (markdown or plaintext), if the server sent it inline.
    pub documentation: Option<String>,
    /// Text actually inserted on accept when no explicit edit range is
    /// present (`insertText`/`textEdit.newText`, falling back to `label`).
    pub insert_text: String,
    /// Text the client filters against as the user types (defaults to
    /// `label`).
    pub filter_text: Option<String>,
    /// Server-provided ordering key; the client sorts by this before label.
    pub sort_text: Option<String>,
    /// The server suggests this item be pre-selected.
    pub preselect: bool,
    /// Original CompletionItem with list-level defaults expanded and all LSP
    /// positions converted to Neoism's UTF-8 byte-coordinate boundary. It is
    /// preserved for completionItem/resolve and correct textEdit /
    /// additionalTextEdits application; clients must not interpret it as
    /// display text.
    pub payload: serde_json::Value,
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
    let adapters = adapters_for_root(&root);
    let scan = scan_workspace(&root, &adapters);
    detected_servers(&root, &scan, &adapters)
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
    let adapters = adapters_for_root(&root);
    let strongest_match = adapters
        .iter()
        .filter_map(|adapter| adapter.match_priority(&file))
        .max();
    let matching_adapters = adapters
        .iter()
        .filter(|adapter| {
            strongest_match
                .is_some_and(|score| adapter.match_priority(&file) == Some(score))
        })
        .collect::<Vec<_>>();
    if matching_adapters.is_empty() {
        return Vec::new();
    }

    let mut evidence = WorkspaceScan {
        files: 1,
        ..WorkspaceScan::default()
    };
    if let Some(extension) = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
    {
        evidence.extensions.insert(extension, 1);
    }
    matching_adapters
        .iter()
        .map(|adapter| {
            let mut status = server_status_for_file(&root, &file, &evidence, adapter);
            if let Some(language) = adapter.logical_language_for_path(&file) {
                status.language = language.to_string();
            }
            status
        })
        .collect()
}

/// File-scoped operations are selected solely from the concrete file. A
/// workspace marker is evidence that a language exists somewhere in the
/// project, never evidence that its server may parse the active buffer.
#[cfg(test)]
fn workspace_scan_for_file(_root: &Path, _file: &Path) -> WorkspaceScan {
    WorkspaceScan::default()
}

/// The built-in language id whose server handles `file`'s extension, if
/// any (e.g. `foo.rs` -> "rust"). Used to narrow the workspace server
/// list to the servers relevant to the open buffer for the status-bar
/// pill. Returns `None` for extensions no bundled spec claims.
pub fn language_id_for_path(file: impl AsRef<Path>) -> Option<&'static str> {
    lsp_languages::logical_language_for_path(file.as_ref())
}

/// Workspace-aware language routing, including arbitrary adapters declared in
/// `neoism.json`. This is the authoritative API for editor buffers.
pub fn language_id_for_path_in(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Option<String> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let adapters = adapters_for_root(&root);
    let strongest = adapters
        .iter()
        .filter_map(|adapter| adapter.match_priority(&file))
        .max()?;
    adapters
        .iter()
        .filter(|adapter| adapter.is_valid())
        .filter(|adapter| adapter.match_priority(&file) == Some(strongest))
        .find_map(|adapter| {
            adapter
                .logical_language_for_path(&file)
                .map(ToOwned::to_owned)
        })
}

/// Whether a specific catalog package exposes the executable declared by a
/// built-in adapter. Package identity is part of the match so similarly named
/// binaries cannot accidentally advertise an unusable attachment.
pub fn supports_language_server_package(package_id: &str, command: &str) -> bool {
    lsp_languages::adapter_supports_catalog_package(package_id, command)
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
    pub server_id: String,
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
fn dispatch_diagnostics(
    root: &Path,
    server_id: &str,
    routes: &[ResolvedLanguageRoute],
    params: &Value,
) {
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return;
    };
    let file = match lsp_uri::file_uri_to_path(uri) {
        Some(path) => path,
        None => return,
    };
    let Some(language) = best_route_in(routes, &file).map(|route| route.id.as_str())
    else {
        tracing::debug!(
            server_id,
            file = %file.display(),
            "ignored publishDiagnostics for a URI outside the adapter's declared routes"
        );
        return;
    };
    let diagnostics = lsp_parse::parse_diagnostics(root, &file, language, params.clone());
    // Keep the pull cache fresh from the push so `cached_diagnostics` readers
    // (web fetch, pill) don't need an aggressive `touch`.
    let version = params
        .get("version")
        .and_then(Value::as_i64)
        .and_then(|version| i32::try_from(version).ok());
    if !lsp_service::service().store_versioned_diagnostics(
        root,
        &file,
        server_id,
        language,
        version,
        diagnostics,
    ) {
        if std::env::var_os("NEOISM_LSP_LOG").is_some() {
            eprintln!(
                "neoism::lsp ignored stale publishDiagnostics: lang={language} file={} version={version:?}",
                file.display(),
            );
        }
        return;
    }
    let diagnostics = lsp_service::service().cached_diagnostics(root, &file);
    // send() only errors when there are no live receivers — harmless.
    let _ = diagnostics_bus().send(DiagnosticsEvent {
        root: root.to_path_buf(),
        server_id: server_id.to_string(),
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

fn command_bin_is_explicit(bin: &str) -> bool {
    let path = Path::new(bin);
    path.is_absolute() || path.components().count() > 1
}

fn unavailable_lsp_error(
    root: &Path,
    file: Option<&Path>,
    operation: LspOperation,
) -> Option<anyhow::Error> {
    let adapters = adapters_for_root(root);
    let scan = scan_workspace(root, &adapters);
    let mut matching = Vec::new();
    let mut missing = Vec::new();

    for adapter in &adapters {
        if !operation_supported(adapter, operation) {
            continue;
        }
        let matches = if let Some(file) = file {
            adapter.matches_path(file)
        } else {
            language_detected(adapter, &scan)
        };
        if !matches {
            continue;
        }
        matching.push(adapter);
        if !adapter.is_valid() || !adapter_endpoint_available(adapter) {
            missing.push(adapter);
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
        .map(|adapter| {
            let endpoint = match &adapter.transport {
                ResolvedLspTransport::Stdio { command, .. } => command.join(" "),
                ResolvedLspTransport::Tcp { host, port, .. } => {
                    format!("tcp://{host}:{port}")
                }
                ResolvedLspTransport::Invalid => adapter
                    .configuration_error
                    .clone()
                    .unwrap_or_else(|| "invalid configuration".to_string()),
            };
            format!("{} (`{endpoint}`)", adapter.name)
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(anyhow::anyhow!(
        "LSP unavailable for {operation:?} on {target}: matching adapter(s) are unavailable or invalid: {servers}. Run the LSP status operation for exact configuration/install details."
    ))
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
    let adapters = adapters_for_root(&root);
    let scan = scan_workspace(&root, &adapters);
    let mut symbols = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in adapters
        .iter()
        .filter(|spec| spec.workspace_symbols && language_detected(spec, &scan))
        .filter(|spec| spec.is_valid())
        .filter(|spec| adapter_endpoint_available(spec))
        .take(MAX_LSP_SERVERS_PER_QUERY)
    {
        match lsp_service::service().workspace_symbols(&root, query, spec) {
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
                    language = %spec.id,
                    command = ?spec.transport,
                    error = %error,
                    "workspace symbol query failed"
                );
            }
        }
    }

    symbols
}

/// Hover at an exact editor position. `line` is zero-based and `character` is
/// a zero-based UTF-8 byte column; the client converts it to the server's
/// negotiated wire encoding exactly once.
pub fn hover(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspHover> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&root, &file, LspOperation::Hover) {
        match lsp_service::service().hover(&root, &file, line, character, &spec) {
            Ok(found) => {
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
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command(),
                    %error,
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
    completion_with_trigger(directory, file, line, character, text, None)
}

pub fn completion_with_trigger(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    text: Option<&str>,
    trigger_character: Option<&str>,
) -> Vec<LspCompletionItem> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    for spec in file_query_specs(&root, &file, LspOperation::Completion) {
        match lsp_service::service().completion(
            &root,
            &file,
            line,
            character,
            text,
            trigger_character,
            &spec,
        ) {
            Ok(items) if !items.is_empty() => return items,
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(language = spec.id, %error, "completion query failed");
            }
        }
    }
    Vec::new()
}

/// Resolve one completion item on the exact server that produced it. The
/// opaque item is kept in Neoism byte coordinates at this API boundary; the
/// client transport converts positions back to the negotiated encoding for
/// the request and to bytes again for the response.
pub fn resolve_completion(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    item: Value,
) -> Option<Value> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    for spec in file_query_specs(&root, &file, LspOperation::Completion)
        .into_iter()
        .filter(|spec| spec.id == server_id)
    {
        match lsp_service::service().resolve_completion(&root, &file, &spec, item.clone())
        {
            Ok(resolved) => return Some(resolved),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command(),
                    error = %error,
                    "completion resolve failed"
                );
            }
        }
    }
    None
}

pub fn execute_completion_command(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    command: Value,
) -> Option<Value> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    for spec in file_query_specs(&root, &file, LspOperation::Completion)
        .into_iter()
        .filter(|spec| spec.id == server_id)
    {
        match lsp_service::service().execute_command(&root, &file, &spec, command.clone())
        {
            Ok(result) => return Some(result),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command(),
                    error = %error,
                    "completion follow-up command failed"
                );
            }
        }
    }
    None
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
    for spec in file_query_specs(&root, &file, LspOperation::Completion) {
        if let Ok(chars) =
            lsp_service::service().completion_trigger_characters(&root, &file, &spec)
        {
            if !chars.is_empty() {
                return chars;
            }
        }
    }
    Vec::new()
}

/// Definitions at an exact zero-based line and zero-based UTF-8 byte column.
pub fn definitions(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&root, &file, LspOperation::Definition) {
        match lsp_service::service().definitions(&root, &file, line, character, &spec) {
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
                    command = ?spec.command(),
                    %error,
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
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&root, &file, LspOperation::References) {
        match lsp_service::service().references(&root, &file, line, character, &spec) {
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
                    command = ?spec.command(),
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
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&root, &file, LspOperation::Implementation) {
        match lsp_service::service().implementations(&root, &file, line, character, &spec)
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
                    command = ?spec.command(),
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
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&root, &file, LspOperation::CallHierarchy) {
        match lsp_service::service()
            .prepare_call_hierarchy(&root, &file, line, character, &spec)
        {
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
                    command = ?spec.command(),
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
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&root, &file, LspOperation::CallHierarchy) {
        let found = lsp_service::service()
            .call_hierarchy_calls(&root, &file, line, character, &spec, incoming);
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
                    command = ?spec.command(),
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
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();

    for spec in file_query_specs(&root, &file, LspOperation::Diagnostics) {
        match lsp_service::service().diagnostics(&root, &file, &spec) {
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
                    command = ?spec.command(),
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
    if language_id_for_path_in(&root, &file).is_none() {
        lsp_service::service().clear_diagnostics(&root, &file);
        return Vec::new();
    }
    lsp_service::service().cached_diagnostics(&root, &file)
}

/// Snapshot of every file the language servers have published diagnostics for,
/// limited to the given workspace. Mirrors opencode's `lsp.diagnostics()` record
/// of all known diagnostics (we never spawn a server here — it only reads the
/// cache that prior touch/diagnostics queries populated).
pub fn cached_project_diagnostics(
    directory: impl AsRef<Path>,
) -> Vec<(PathBuf, Vec<LspDiagnostic>)> {
    let root = normalized_root(directory.as_ref());
    lsp_service::service().cached_diagnostics_snapshot(&root)
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
    let mut results = Vec::new();

    for spec in file_query_specs(&root, &file, LspOperation::DocumentSymbols) {
        match lsp_service::service().document_symbols(&root, &file, &spec) {
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
                    command = ?spec.command(),
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
    let mut results = Vec::new();

    for spec in file_query_specs(&root, &file, LspOperation::Formatting) {
        match lsp_service::service().formatting(&root, &file, &spec) {
            Ok(result) => results.push(json!({
                "language": spec.id,
                "path": file.display().to_string(),
                "edits": result,
            })),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command(),
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
    let mut results = Vec::new();

    for spec in file_query_specs(&root, &file, LspOperation::CodeActions) {
        match lsp_service::service().code_actions(&root, &file, line, character, &spec) {
            Ok(result) => results.push(json!({
                "language": spec.id,
                "path": file.display().to_string(),
                "actions": result,
            })),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command(),
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
    server_id: &str,
    action: Value,
) -> Option<Value> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());

    for spec in file_query_specs(&root, &file, LspOperation::CodeActions)
        .into_iter()
        .filter(|spec| spec.id == server_id)
    {
        match lsp_service::service().resolve_code_action(
            &root,
            &file,
            &spec,
            action.clone(),
        ) {
            Ok(resolved) => return Some(resolved),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command(),
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
    server_id: &str,
    command: Value,
) -> Option<Value> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());

    for spec in file_query_specs(&root, &file, LspOperation::CodeActions)
        .into_iter()
        .filter(|spec| spec.id == server_id)
    {
        match lsp_service::service().execute_command(&root, &file, &spec, command.clone())
        {
            Ok(result) => return Some(result),
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command(),
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
    let mut results = Vec::new();

    for spec in file_query_specs(&root, &file, LspOperation::Rename) {
        match lsp_service::service()
            .rename(&root, &file, line, character, new_name, &spec)
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
                    command = ?spec.command(),
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
    let mut results = Vec::new();

    for spec in file_query_specs(&root, &file, LspOperation::Diagnostics) {
        match lsp_service::service().touch(&root, &file, text, &spec) {
            Ok(diagnostics) => {
                results.push((spec.id.to_string(), file.clone(), diagnostics))
            }
            Err(error) => {
                tracing::debug!(
                    language = spec.id,
                    command = ?spec.command(),
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
    let mut synced = Vec::new();
    for spec in file_lifecycle_specs(&root, &file) {
        match lsp_service::service().sync(&root, &file, text, &spec) {
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

/// Send textDocument/didSave to every attached adapter that owns this file and
/// negotiated save notifications. Edits must be synchronized first; callers
/// that use Neoism's ordered live-document queue get that guarantee.
pub fn save_document(directory: impl AsRef<Path>, file: impl AsRef<Path>) -> Vec<String> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    let mut saved = Vec::new();
    for spec in file_lifecycle_specs(&root, &file) {
        match lsp_service::service().save(&root, &file, &spec) {
            Ok(()) => saved.push(spec.id.to_string()),
            Err(error) => tracing::debug!(
                language = spec.id,
                file = %file.display(),
                %error,
                "document save notification failed"
            ),
        }
    }
    saved
}

/// Send `textDocument/didClose` to every attached adapter that owns this
/// document, then evict its versions and diagnostics from the workspace cache.
pub fn close_document(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let root = normalized_root(directory.as_ref());
    let file = normalized_file(&root, file.as_ref());
    lsp_service::service().close_document(&root, &file)
}

pub(crate) async fn lsp_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    tokio::task::spawn_blocking(move || lsp_tool_blocking(context, arguments))
        .await
        .context("LSP tool worker failed")?
}

fn lsp_tool_blocking(
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
