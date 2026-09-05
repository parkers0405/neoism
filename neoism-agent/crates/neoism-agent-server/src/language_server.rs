use std::path::Path;

pub use crate::lsp::{
    language_server_adapters, language_server_adapters_for, DiagnosticsEvent,
    LspAdapterMetadata, LspAdapterOrigin, LspAdapterTransport, LspCatalogPackageMetadata,
    LspCommandSource, LspCompletionItem, LspDiagnostic, LspDocumentHighlight,
    LspDocumentSymbol, LspHover, LspInlayHint, LspLanguageRouteMetadata, LspLocation,
    LspParameterInfo, LspPosition, LspRange, LspRuntime, LspServerState,
    LspSignatureHelp, LspSignatureInfo, LspStatus, WorkspaceSymbol,
};

/// Subscribe to real-time `publishDiagnostics` pushes (event-driven — the
/// daemon drains this instead of polling).
pub fn subscribe_diagnostics(
    runtime: &LspRuntime,
) -> tokio::sync::broadcast::Receiver<DiagnosticsEvent> {
    crate::lsp::subscribe_diagnostics(runtime)
}

/// Open/update a document so its server re-analyzes and pushes diagnostics on
/// the bus. Fire-and-forget.
pub fn sync_document(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    text: Option<&str>,
) -> Vec<String> {
    crate::lsp::sync_document(runtime, directory, file, text)
}

/// Notify every owning adapter that the synchronized live document was saved.
pub fn save_document(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<String> {
    crate::lsp::save_document(runtime, directory, file)
}

/// Close a document in all attached servers and evict its cached state.
pub fn close_document(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> anyhow::Result<()> {
    crate::lsp::close_document(runtime, directory, file)
}

/// Cached diagnostics for `file` (populated by real-time pushes). No server
/// spawn, no wait.
pub fn cached_diagnostics(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<LspDiagnostic> {
    crate::lsp::cached_diagnostics(runtime, directory, file)
}

/// Return the Neoism-owned LSP runtime status for a workspace.
pub fn status(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    _file: Option<&Path>,
) -> Vec<LspStatus> {
    match _file {
        Some(file) => crate::lsp::status_for_file(runtime, directory, file),
        None => crate::lsp::status(runtime, directory),
    }
}

/// The built-in language id whose server handles `file`'s extension.
pub fn language_id_for_path(
    runtime: &LspRuntime,
    file: impl AsRef<Path>,
) -> Option<String> {
    crate::lsp::language_id_for_path(runtime, file)
}

/// Workspace-aware language id, including project-defined adapters.
pub fn language_id_for_path_in(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Option<String> {
    crate::lsp::language_id_for_path_in(runtime, directory, file)
}

/// Whether a catalog package exposes the exact executable consumed by a
/// built-in adapter. This checks package identity as well as command identity;
/// two unrelated catalog rows must not become installable merely because
/// their selected binaries happen to share a name.
pub fn supports_language_server_package(
    runtime: &LspRuntime,
    package_id: &str,
    command: &str,
) -> bool {
    crate::lsp::supports_language_server_package(runtime, package_id, command)
}

/// Language ids with a live (connected) LSP client under `directory`.
pub fn live_languages(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
) -> std::collections::BTreeSet<String> {
    crate::lsp::live_languages(runtime, directory)
}

/// Where the Agent LSP engine would resolve `command` for server `id`:
/// adapter-managed, config path, `$PATH`, or missing.
/// Lets the Extensions page badge each language-server row with the same
/// source the engine will actually use at runtime.
pub fn command_source(
    runtime: &LspRuntime,
    id: &str,
    command: Vec<String>,
) -> LspCommandSource {
    runtime.resolve_lsp_command(id, command).1
}

/// Synchronize one document into Neoism's LSP runtime and return cached diagnostics.
pub fn touch_document(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    text: Option<&str>,
) -> Vec<LspDiagnostic> {
    crate::lsp::touch_document_diagnostics(runtime, directory, file, text)
        .into_iter()
        .flat_map(|(_, _, diagnostics)| diagnostics)
        .collect()
}

pub fn hover(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspHover> {
    crate::lsp::hover(runtime, directory, file, line, character)
}

/// Signature help at the cursor. `line`/`character` follow the same
/// convention as [`hover`]: zero-based line, zero-based UTF-8 byte column.
pub fn signature_help(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspSignatureHelp> {
    crate::lsp::signature_help(runtime, directory, file, line, character)
}

/// Inlay hints for the inclusive zero-based line range
/// `start_line..=end_line`. Returned hint positions are one-based
/// (line, UTF-8 byte column), like every [`LspRange`] this module emits.
pub fn inlay_hints(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    start_line: u32,
    end_line: u32,
) -> Vec<LspInlayHint> {
    crate::lsp::inlay_hints(runtime, directory, file, start_line, end_line)
}

/// Occurrences of the symbol under the cursor within `file`, with
/// read/write/text classification when the server provides one.
pub fn document_highlight(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspDocumentHighlight> {
    crate::lsp::document_highlights(runtime, directory, file, line, character)
}

/// Completion items at the cursor from the file's language server. `text` is
/// the LIVE buffer content, synced (didChange) before the query so completion
/// reflects what the user is typing.
pub fn completion(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    text: Option<&str>,
) -> Vec<LspCompletionItem> {
    crate::lsp::completion(runtime, directory, file, line, character, text)
}

pub fn completion_with_trigger(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    text: Option<&str>,
    trigger_character: Option<&str>,
) -> Vec<LspCompletionItem> {
    crate::lsp::completion_with_trigger(
        runtime,
        directory,
        file,
        line,
        character,
        text,
        trigger_character,
    )
}

pub fn resolve_completion(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    item: serde_json::Value,
) -> Option<serde_json::Value> {
    crate::lsp::resolve_completion(runtime, directory, file, server_id, item)
}

pub fn execute_completion_command(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    command: serde_json::Value,
) -> Option<serde_json::Value> {
    crate::lsp::execute_completion_command(runtime, directory, file, server_id, command)
}

/// Trigger characters advertised by the file's language server.
pub fn completion_trigger_characters(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<String> {
    crate::lsp::completion_trigger_characters(runtime, directory, file)
}

pub fn definition(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    crate::lsp::definitions(runtime, directory, file, line, character)
}

pub fn references(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    crate::lsp::references(runtime, directory, file, line, character)
}

pub fn implementation(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    crate::lsp::implementations(runtime, directory, file, line, character)
}

pub fn document_symbols(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<LspDocumentSymbol> {
    crate::lsp::document_symbols(runtime, directory, file)
}

pub fn workspace_symbols(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    query: &str,
) -> Vec<WorkspaceSymbol> {
    crate::lsp::workspace_symbols(runtime, directory, query)
}

pub fn formatting(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<serde_json::Value> {
    crate::lsp::formatting(runtime, directory, file)
}

pub fn code_actions(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<serde_json::Value> {
    crate::lsp::code_actions(runtime, directory, file, line, character)
}

pub fn resolve_code_action(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    action: serde_json::Value,
) -> Option<serde_json::Value> {
    crate::lsp::resolve_code_action(runtime, directory, file, server_id, action)
}

pub fn execute_command(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    command: serde_json::Value,
) -> Option<serde_json::Value> {
    crate::lsp::execute_command(runtime, directory, file, server_id, command)
}

pub fn rename(
    runtime: &LspRuntime,
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    new_name: &str,
) -> Vec<serde_json::Value> {
    crate::lsp::rename(runtime, directory, file, line, character, new_name)
}
