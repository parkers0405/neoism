use std::path::Path;

use neoism_agent_server::language_server;
use neoism_protocol::editor::{
    EditorLspAction, EditorLspCodeAction, EditorLspCompletionItem, EditorLspSymbol,
    EditorServerMessage,
};

/// The embedded nvim session used to supply the active buffer (path, text,
/// cursor) and the write-back path for workspace edits. Both are gone; the
/// text source returns with the native editor. Until then only requests that
/// can be served from disk state (workspace-wide symbol search) succeed.
const NO_ACTIVE_BUFFER: &str =
    "LSP request needs the active editor buffer; text source returns with the native editor";

pub(crate) fn run_action(
    workspace_root: &Path,
    action: EditorLspAction,
    text: Option<&str>,
) -> Result<EditorServerMessage, String> {
    match action {
        EditorLspAction::WorkspaceSymbols => {
            let query = text.unwrap_or_default().trim();
            let (symbols, summary) = if query.is_empty() {
                (
                    Vec::new(),
                    "Neoism LSP workspace symbols need a search query".to_string(),
                )
            } else {
                let result = language_server::workspace_symbols(workspace_root, query);
                let symbols: Vec<EditorLspSymbol> =
                    result.into_iter().map(map_workspace_symbol).collect();
                let summary = format!(
                    "Neoism LSP workspace symbols returned {} item(s)",
                    symbols.len()
                );
                (symbols, summary)
            };
            Ok(EditorServerMessage::LspActionResult {
                surface_id: None,
                action,
                line: 0,
                character: 0,
                summary,
                hover: None,
                locations: Vec::new(),
                symbol_count: symbols.len(),
                symbols,
                code_actions: Vec::new(),
            })
        }
        // Every other action resolves against the active buffer's text and
        // cursor position.
        _ => Err(NO_ACTIVE_BUFFER.to_string()),
    }
}

pub(crate) fn run_code_action(
    _workspace_root: &Path,
    _selected: EditorLspCodeAction,
) -> Result<EditorServerMessage, String> {
    // Applying a code action revalidates the live document revision and
    // writes the edited text back into the editor buffer.
    Err(NO_ACTIVE_BUFFER.to_string())
}

pub(crate) fn run_completion(
    _workspace_root: &Path,
    _selected: EditorLspCompletionItem,
    _replace_prefix: &str,
) -> Result<(), String> {
    // Accepting a completion applies text edits to the live buffer.
    Err(NO_ACTIVE_BUFFER.to_string())
}

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
