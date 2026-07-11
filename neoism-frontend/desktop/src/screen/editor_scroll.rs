// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::*;
use neoism_backend::clipboard::{Clipboard, ClipboardType};
use neoism_ui::editor::scroll_model::{
    advance_block_scroll_cursor as shared_advance_block_scroll_cursor,
    drop_composer_prompt_row as shared_drop_composer_prompt_row,
    parse_ex_command as shared_parse_ex_command,
    raw_scroll_has_room as shared_raw_scroll_has_room,
    scrollbar_drag_target_rich_text_id as shared_scrollbar_drag_target_rich_text_id,
    AgentTag, BlockContentTopPick, DiagnosticsPopupWheel, DiagnosticsPopupWheelContext,
    EditorKeyDispatchContext, EditorKeyDispatchPlan, EditorModeClass,
    GlobalExCommandPlan, MarkdownExCommandPlan, ScrollbarClickContext,
    ScrollbarClickPlan, ScrollbarPaneKind, TerminalAlternateScrollCsi,
    TerminalBlockScrollCursor as SharedBlockScrollCursor, TerminalMouseModeWheelReport,
};
use neoism_window::event::{ElementState, MouseButton};

/// Convert a winit-style `MouseScrollDelta` to the shared `ScrollDelta`
/// type the policy helpers consume.
fn shared_scroll_delta(
    delta: &neoism_window::event::MouseScrollDelta,
) -> neoism_ui::panels::completion_menu::ScrollDelta {
    use neoism_ui::panels::completion_menu::ScrollDelta;
    use neoism_window::event::MouseScrollDelta;
    match delta {
        MouseScrollDelta::LineDelta(x, y) => ScrollDelta::Lines { x: *x, y: *y },
        MouseScrollDelta::PixelDelta(pos) => ScrollDelta::Pixels {
            x: pos.x as f32,
            y: pos.y as f32,
        },
    }
}

fn diagnostic_counts_for_lsp(
    diagnostics: Option<&neoism_backend::performer::nvim::DiagnosticsNotification>,
    server_name: &str,
    single_server: bool,
    aggregate: neoism_ui::panels::status_line::DiagnosticCounts,
) -> neoism_ui::panels::status_line::DiagnosticCounts {
    let Some(diagnostics) = diagnostics else {
        return neoism_ui::panels::status_line::DiagnosticCounts::default();
    };
    let mut counts = neoism_ui::panels::status_line::DiagnosticCounts::default();
    let mut saw_source = false;
    for item in &diagnostics.items {
        let Some(source) = item.source.as_deref().filter(|source| !source.is_empty())
        else {
            continue;
        };
        saw_source = true;
        if !lsp_source_matches(source, server_name) {
            continue;
        }
        match item.severity {
            1 => counts.error = counts.error.saturating_add(1),
            2 => counts.warn = counts.warn.saturating_add(1),
            3 => counts.info = counts.info.saturating_add(1),
            _ => counts.hint = counts.hint.saturating_add(1),
        }
    }
    let matched_total = counts.error + counts.warn + counts.info + counts.hint;
    if single_server && (!saw_source || matched_total == 0) {
        aggregate
    } else {
        counts
    }
}

fn lsp_source_matches(source: &str, server_name: &str) -> bool {
    let source = normalize_lsp_match_token(source);
    let server = normalize_lsp_match_token(server_name);
    !source.is_empty()
        && !server.is_empty()
        && (source == server || source.contains(&server) || server.contains(&source))
}

fn normalize_lsp_match_token(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

/// Bridge between the native `BlockScrollCursor` stored on the renderer
/// and the shared `TerminalBlockScrollCursor` used by the policy helpers.
fn shared_block_cursor(
    cursor: crate::terminal::scroll::BlockScrollCursor,
) -> SharedBlockScrollCursor {
    SharedBlockScrollCursor {
        raw_top_abs: cursor.raw_top_abs,
        chrome_row: cursor.chrome_row,
    }
}

fn native_block_cursor(
    cursor: SharedBlockScrollCursor,
) -> crate::terminal::scroll::BlockScrollCursor {
    crate::terminal::scroll::BlockScrollCursor {
        raw_top_abs: cursor.raw_top_abs,
        chrome_row: cursor.chrome_row,
    }
}

/// Map a backend `EditorMode` discriminant to the shared
/// [`EditorModeClass`] used by `EditorKeyDispatchPlan`. The shared
/// crate must not depend on `neoism_backend`, so the conversion lives
/// here in the desktop host. `Unknown(_)` collapses to the catch-all
/// `EditorModeClass::Unknown` — the dispatch policy treats Unknown the
/// same as Normal for tab/glyph/leader intercept purposes.
fn editor_mode_class(
    mode: &neoism_backend::performer::nvim_events::EditorMode,
) -> EditorModeClass {
    use neoism_backend::performer::nvim_events::EditorMode;
    match mode {
        EditorMode::Normal => EditorModeClass::Normal,
        EditorMode::Insert => EditorModeClass::Insert,
        EditorMode::Visual => EditorModeClass::Visual,
        EditorMode::Replace => EditorModeClass::Replace,
        EditorMode::CmdLine => EditorModeClass::CmdLine,
        EditorMode::Unknown(_) => EditorModeClass::Unknown,
    }
}

mod editor_command;
mod editor_input;
mod overlay_scroll;
mod status_minimap;
