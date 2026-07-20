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
    GlobalExCommandPlan, MarkdownExCommandPlan, ScrollbarClickContext,
    ScrollbarClickPlan, ScrollbarPaneKind, TerminalAlternateScrollCsi,
    TerminalBlockScrollCursor as SharedBlockScrollCursor, TerminalMouseModeWheelReport,
};
use neoism_window::event::ElementState;

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

mod editor_command;
mod editor_input;
mod overlay_scroll;
mod status_minimap;
