use std::path::Path;

pub use crate::lsp::{
    language_server_adapters, language_server_adapters_for, DiagnosticsEvent,
    LspAdapterMetadata, LspAdapterOrigin, LspAdapterTransport, LspCatalogPackageMetadata,
    LspCommandSource, LspCompletionItem, LspDiagnostic, LspDocumentSymbol, LspHover,
    LspLanguageRouteMetadata, LspLocation, LspPosition, LspRange, LspServerState,
    LspStatus, WorkspaceSymbol,
};

/// Subscribe to real-time `publishDiagnostics` pushes (event-driven — the
/// daemon drains this instead of polling).
pub fn subscribe_diagnostics() -> tokio::sync::broadcast::Receiver<DiagnosticsEvent> {
    crate::lsp::subscribe_diagnostics()
}

/// Open/update a document so its server re-analyzes and pushes diagnostics on
/// the bus. Fire-and-forget.
pub fn sync_document(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    text: Option<&str>,
) -> Vec<String> {
    crate::lsp::sync_document(directory, file, text)
}

/// Notify every owning adapter that the synchronized live document was saved.
pub fn save_document(directory: impl AsRef<Path>, file: impl AsRef<Path>) -> Vec<String> {
    crate::lsp::save_document(directory, file)
}

/// Close a document in all attached servers and evict its cached state.
pub fn close_document(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> anyhow::Result<()> {
    crate::lsp::close_document(directory, file)
}

/// Cached diagnostics for `file` (populated by real-time pushes). No server
/// spawn, no wait.
pub fn cached_diagnostics(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<LspDiagnostic> {
    crate::lsp::cached_diagnostics(directory, file)
}

/// Return the Neoism-owned LSP runtime status for a workspace.
pub fn status(directory: impl AsRef<Path>, _file: Option<&Path>) -> Vec<LspStatus> {
    match _file {
        Some(file) => crate::lsp::status_for_file(directory, file),
        None => crate::lsp::status(directory),
    }
}

/// The built-in language id whose server handles `file`'s extension.
pub fn language_id_for_path(file: impl AsRef<Path>) -> Option<&'static str> {
    crate::lsp::language_id_for_path(file)
}

/// Workspace-aware language id, including project-defined adapters.
pub fn language_id_for_path_in(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Option<String> {
    crate::lsp::language_id_for_path_in(directory, file)
}

/// Whether a catalog package exposes the exact executable consumed by a
/// built-in adapter. This checks package identity as well as command identity;
/// two unrelated catalog rows must not become installable merely because
/// their selected binaries happen to share a name.
pub fn supports_language_server_package(package_id: &str, command: &str) -> bool {
    crate::lsp::supports_language_server_package(package_id, command)
}

/// Language ids with a live (connected) LSP client under `directory`.
pub fn live_languages(directory: impl AsRef<Path>) -> std::collections::BTreeSet<String> {
    crate::lsp::live_languages(directory)
}

/// Where the Neoism LSP engine would resolve `command` for server `id`:
/// extension-installed (managed bin), config path, `$PATH`, or missing.
/// Lets the Extensions page badge each language-server row with the same
/// source the engine will actually use at runtime.
pub fn command_source(id: &str, command: Vec<String>) -> LspCommandSource {
    crate::lsp::resolve_lsp_command(id, command).1
}

/// Synchronize one document into Neoism's LSP runtime and return cached diagnostics.
pub fn touch_document(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    text: Option<&str>,
) -> Vec<LspDiagnostic> {
    crate::lsp::touch_document_diagnostics(directory, file, text)
        .into_iter()
        .flat_map(|(_, _, diagnostics)| diagnostics)
        .collect()
}

pub fn hover(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspHover> {
    crate::lsp::hover(directory, file, line, character)
}

/// Completion items at the cursor from the file's language server. `text` is
/// the LIVE buffer content, synced (didChange) before the query so completion
/// reflects what the user is typing.
pub fn completion(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    text: Option<&str>,
) -> Vec<LspCompletionItem> {
    crate::lsp::completion(directory, file, line, character, text)
}

pub fn completion_with_trigger(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    text: Option<&str>,
    trigger_character: Option<&str>,
) -> Vec<LspCompletionItem> {
    crate::lsp::completion_with_trigger(
        directory,
        file,
        line,
        character,
        text,
        trigger_character,
    )
}

pub fn resolve_completion(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    item: serde_json::Value,
) -> Option<serde_json::Value> {
    crate::lsp::resolve_completion(directory, file, server_id, item)
}

pub fn execute_completion_command(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    command: serde_json::Value,
) -> Option<serde_json::Value> {
    crate::lsp::execute_completion_command(directory, file, server_id, command)
}

/// Trigger characters advertised by the file's language server.
pub fn completion_trigger_characters(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<String> {
    crate::lsp::completion_trigger_characters(directory, file)
}

pub fn definition(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    crate::lsp::definitions(directory, file, line, character)
}

pub fn references(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    crate::lsp::references(directory, file, line, character)
}

pub fn implementation(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<LspLocation> {
    crate::lsp::implementations(directory, file, line, character)
}

pub fn document_symbols(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<LspDocumentSymbol> {
    crate::lsp::document_symbols(directory, file)
}

pub fn workspace_symbols(
    directory: impl AsRef<Path>,
    query: &str,
) -> Vec<WorkspaceSymbol> {
    crate::lsp::workspace_symbols(directory, query)
}

pub fn formatting(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
) -> Vec<serde_json::Value> {
    crate::lsp::formatting(directory, file)
}

pub fn code_actions(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
) -> Vec<serde_json::Value> {
    crate::lsp::code_actions(directory, file, line, character)
}

pub fn resolve_code_action(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    action: serde_json::Value,
) -> Option<serde_json::Value> {
    crate::lsp::resolve_code_action(directory, file, server_id, action)
}

pub fn execute_command(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    server_id: &str,
    command: serde_json::Value,
) -> Option<serde_json::Value> {
    crate::lsp::execute_command(directory, file, server_id, command)
}

pub fn rename(
    directory: impl AsRef<Path>,
    file: impl AsRef<Path>,
    line: u32,
    character: u32,
    new_name: &str,
) -> Vec<serde_json::Value> {
    crate::lsp::rename(directory, file, line, character, new_name)
}
