// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::*;
use crate::input::kitty_keyboard::build_key_sequence;
use neoism_backend::clipboard::{Clipboard, ClipboardType};
use neoism_terminal_core::crosswords::pos::{Direction, Line};
use neoism_terminal_core::selection::SelectionType;
use neoism_ui::editor::selection_model::{
    apply_selection_update, file_link_open_target, hint_highlight_eligible,
    hint_label_placements, hint_modifiers_match, hyperlink_span_at,
    hyperlink_trigger_eligible, include_all_current_selection,
    left_click_selection_action, post_process_hint_match_end, selected_text,
    selection_with_range, should_open_file_link_on_click, terminal_body_visual_row,
    terminal_file_link_hover_rect, terminal_file_link_probe,
    toggle_action_needs_include_all, toggle_selection_action, FileLinkOpenTarget,
    HintModifierState, HintMouseActivation, LeftClickSelectionAction, SelectionClickKind,
    SelectionEndpoint, SelectionModifiers, SelectionSnapshot, TerminalBodyMouseRowInput,
    TerminalFileLinkHoverInput, TerminalTextArea, ToggleSelectionAction,
};
use neoism_ui::key_policy::{
    ime_cursor_pixel_position, ime_cursor_position_significantly_changed,
    workspace_index_for_alt_digit, ImeCursorInput,
};
use neoism_ui::mouse_policy::{
    encode_normal_mouse_report, encode_sgr_mouse_report, mouse_report_legacy_button_byte,
    mouse_report_modifier_bits,
};
use neoism_ui::paste_policy::{
    paste_payload, PastePayload, BRACKETED_PASTE_END, BRACKETED_PASTE_START,
};
use neoism_window::event::{ElementState, Modifiers, MouseButton};
use neoism_window::keyboard::{Key, ModifiersState, NamedKey};

mod file_link_mouse;
mod hints;
mod key_bindings;
mod key_event;
mod mouse_position;
mod selection_ops;
