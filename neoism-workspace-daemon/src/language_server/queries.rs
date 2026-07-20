use std::path::Path;

use neoism_protocol::editor::EditorServerMessage;

/// Completion at the active editor cursor. The cursor position and live
/// document text came from the embedded nvim session; the text source
/// returns with the native editor. Until then the reply is an empty item
/// set (the client renders no popup).
pub(crate) fn completion(
    _workspace_root: &Path,
    seq: u64,
    _trigger_character: Option<&str>,
) -> EditorServerMessage {
    EditorServerMessage::LspCompletions {
        surface_id: None,
        seq,
        replace_prefix: String::new(),
        items: Vec::new(),
    }
}

/// Hover docs at a rendered grid cell. Grid-cell-to-buffer-position
/// resolution was owned by the embedded nvim session; the text source
/// returns with the native editor. Until then the reply carries empty
/// contents (the client shows no hover card).
pub(crate) fn hover_at(
    _workspace_root: &Path,
    seq: u64,
    _grid: i64,
    _row: i64,
    _col: i64,
) -> EditorServerMessage {
    EditorServerMessage::LspHoverResult {
        surface_id: None,
        seq,
        line: 0,
        character: 0,
        contents: String::new(),
    }
}
