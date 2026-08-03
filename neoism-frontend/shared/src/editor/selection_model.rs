//! Shared terminal/editor selection decisions.
//!
//! Frontends still own terminal locks, clipboard contents, renderer damage,
//! and file/hyperlink actions. This module keeps the portable selection
//! policy in one place so desktop and web classify the same mouse gestures
//! and update selections the same way.

use neoism_terminal_core::crosswords::grid::Dimensions;
use neoism_terminal_core::crosswords::pos::{Column, Line, Pos, Side};
use neoism_terminal_core::crosswords::Crosswords;
use neoism_terminal_core::selection::{Selection, SelectionRange, SelectionType};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionClickKind {
    None,
    Single,
    Double,
    Triple,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SelectionModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

impl SelectionModifiers {
    pub const fn new(shift: bool, control: bool, alt: bool) -> Self {
        Self {
            shift,
            control,
            alt,
        }
    }

    pub const fn plain(self) -> bool {
        !self.shift && !self.control && !self.alt
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HintModifierState {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl HintModifierState {
    pub const fn new(shift: bool, control: bool, alt: bool, super_key: bool) -> Self {
        Self {
            shift,
            control,
            alt,
            super_key,
        }
    }
}

pub fn hint_modifiers_match(
    required_mods: &[String],
    current_mods: HintModifierState,
) -> bool {
    if required_mods.is_empty() {
        return true;
    }

    required_mods
        .iter()
        .all(|required_mod| match required_mod.as_str() {
            "Shift" => current_mods.shift,
            "Control" | "Ctrl" => current_mods.control,
            "Alt" => current_mods.alt,
            "Super" | "Cmd" | "Command" => current_mods.super_key,
            _ => false,
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HintMouseActivation<'a> {
    pub mouse_enabled: bool,
    pub hyperlinks: bool,
    pub mouse_mods: &'a [String],
}

pub fn hint_highlight_eligible<'a>(
    hints: impl IntoIterator<Item = HintMouseActivation<'a>>,
    current_mods: HintModifierState,
) -> bool {
    hints.into_iter().any(|hint| {
        hint.mouse_enabled && hint_modifiers_match(hint.mouse_mods, current_mods)
    })
}

pub fn hyperlink_trigger_eligible<'a>(
    hints: impl IntoIterator<Item = HintMouseActivation<'a>>,
    current_mods: HintModifierState,
    has_hyperlink_range: bool,
) -> bool {
    has_hyperlink_range
        && hints.into_iter().any(|hint| {
            hint.hyperlinks && hint_modifiers_match(hint.mouse_mods, current_mods)
        })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HintLabelPlacement {
    pub position: Pos,
    pub label: Vec<char>,
    pub is_first: bool,
}

pub fn hint_label_placements(
    hint_mode_active: bool,
    match_starts: &[Pos],
    visible_labels: &[(usize, Vec<char>)],
) -> Vec<HintLabelPlacement> {
    if !hint_mode_active {
        return Vec::new();
    }

    visible_labels
        .iter()
        .filter_map(|(match_index, remaining_label)| {
            let start = match_starts.get(*match_index)?;
            Some(
                remaining_label
                    .iter()
                    .enumerate()
                    .map(|(char_index, &label_char)| HintLabelPlacement {
                        position: Pos::new(start.row, start.col + char_index),
                        label: vec![label_char],
                        is_first: char_index == 0,
                    }),
            )
        })
        .flatten()
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeftClickSelectionAction {
    None,
    Extend {
        point: Pos,
        side: Side,
    },
    Start {
        ty: SelectionType,
        point: Pos,
        side: Side,
        clear_existing: bool,
    },
}

pub fn left_click_selection_action(
    click: SelectionClickKind,
    modifiers: SelectionModifiers,
    has_selection: bool,
    point: Pos,
    side: Side,
) -> LeftClickSelectionAction {
    match click {
        SelectionClickKind::Single if modifiers.shift && has_selection => {
            LeftClickSelectionAction::Extend { point, side }
        }
        SelectionClickKind::Single => LeftClickSelectionAction::Start {
            ty: if modifiers.control {
                SelectionType::Block
            } else {
                SelectionType::Simple
            },
            point,
            side,
            clear_existing: true,
        },
        SelectionClickKind::Double => LeftClickSelectionAction::Start {
            ty: SelectionType::Semantic,
            point,
            side,
            clear_existing: false,
        },
        SelectionClickKind::Triple => LeftClickSelectionAction::Start {
            ty: SelectionType::Lines,
            point,
            side,
            clear_existing: false,
        },
        SelectionClickKind::None => LeftClickSelectionAction::None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionUpdatePlan {
    pub point: Pos,
    pub move_vi_cursor: bool,
    pub include_all: bool,
}

pub fn selection_update_plan(
    mut point: Pos,
    bottommost_line: Line,
    vi_mode: bool,
    search_active: bool,
) -> SelectionUpdatePlan {
    point.row = std::cmp::min(point.row, bottommost_line);
    let vi_selection_update = vi_mode && !search_active;

    SelectionUpdatePlan {
        point,
        move_vi_cursor: vi_selection_update,
        include_all: vi_selection_update,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionSnapshot {
    pub ty: SelectionType,
    pub is_empty: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToggleSelectionAction {
    Clear,
    RetypeExisting,
    StartAtCursor,
}

pub fn toggle_selection_action(
    existing: Option<SelectionSnapshot>,
    requested_type: SelectionType,
) -> ToggleSelectionAction {
    match existing {
        Some(selection) if selection.ty == requested_type && !selection.is_empty => {
            ToggleSelectionAction::Clear
        }
        Some(selection) if !selection.is_empty => ToggleSelectionAction::RetypeExisting,
        _ => ToggleSelectionAction::StartAtCursor,
    }
}

pub const fn toggle_action_needs_include_all(action: ToggleSelectionAction) -> bool {
    !matches!(action, ToggleSelectionAction::Clear)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionEndpoint {
    pub point: Pos,
    pub side: Side,
}

impl SelectionEndpoint {
    pub const fn new(point: Pos, side: Side) -> Self {
        Self { point, side }
    }
}

pub fn selection_with_range(
    terminal: &Crosswords,
    ty: SelectionType,
    start: SelectionEndpoint,
    end: Option<SelectionEndpoint>,
) -> (Selection, Option<SelectionRange>) {
    let mut selection = Selection::new(ty, start.point, start.side);
    if let Some(end) = end {
        selection.update(end.point, end.side);
    }
    let range = selection.to_range(terminal);
    (selection, range)
}

pub fn apply_selection_update(
    terminal: &mut Crosswords,
    point: Pos,
    side: Side,
    vi_mode: bool,
    search_active: bool,
) -> Option<SelectionRange> {
    let mut selection = terminal.selection.take()?;
    let update_plan =
        selection_update_plan(point, terminal.bottommost_line(), vi_mode, search_active);
    let point = update_plan.point;

    selection.update(point, side);

    if update_plan.move_vi_cursor {
        terminal.vi_mode_cursor.pos = point;
    }
    if update_plan.include_all {
        selection.include_all();
    }

    let selection_range = selection.to_range(terminal);
    terminal.selection = Some(selection);
    selection_range
}

pub fn include_all_current_selection(
    terminal: &mut Crosswords,
) -> Option<SelectionRange> {
    let mut selection = terminal.selection.take()?;
    selection.include_all();
    let selection_range = selection.to_range(terminal);
    terminal.selection = Some(selection);
    selection_range
}

pub fn selected_text(
    terminal: &Crosswords,
    fallback_range: Option<SelectionRange>,
) -> Option<String> {
    terminal
        .selection_to_string()
        .or_else(|| {
            fallback_range.map(|range| terminal.bounds_to_string(range.start, range.end))
        })
        .filter(|text| !text.is_empty())
}

pub fn post_process_hint_match_end(chars: &[char]) -> Option<usize> {
    if chars.is_empty() {
        return None;
    }

    let mut open_parents = 0usize;
    let mut open_brackets = 0usize;

    for (idx, c) in chars.iter().copied().enumerate() {
        match c {
            '(' => open_parents += 1,
            '[' => open_brackets += 1,
            ')' => {
                if open_parents == 0 {
                    return idx.checked_sub(1);
                }
                open_parents -= 1;
            }
            ']' => {
                if open_brackets == 0 {
                    return idx.checked_sub(1);
                }
                open_brackets -= 1;
            }
            _ => {}
        }
    }

    let mut end = chars.len() - 1;
    while end > 0 {
        if !matches!(
            chars[end],
            '.' | ',' | ':' | ';' | '?' | '!' | '(' | '[' | '\''
        ) {
            break;
        }
        end -= 1;
    }

    Some(end)
}

pub fn post_process_hyperlink_uri(uri: &str) -> String {
    let chars: Vec<char> = uri.chars().collect();
    if chars.is_empty() {
        return String::new();
    }

    let mut end_idx = chars.len() - 1;
    let mut open_parents = 0usize;
    let mut open_brackets = 0usize;

    for (idx, c) in chars.iter().copied().enumerate() {
        match c {
            '(' => open_parents += 1,
            '[' => open_brackets += 1,
            ')' => {
                if open_parents == 0 {
                    end_idx = idx.saturating_sub(1);
                    break;
                }
                open_parents -= 1;
            }
            ']' => {
                if open_brackets == 0 {
                    end_idx = idx.saturating_sub(1);
                    break;
                }
                open_brackets -= 1;
            }
            _ => {}
        }
    }

    while end_idx > 0 {
        if !matches!(
            chars[end_idx],
            '.' | ',' | ':' | ';' | '?' | '!' | '(' | '[' | '\''
        ) {
            break;
        }
        end_idx -= 1;
    }

    chars.into_iter().take(end_idx + 1).collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperlinkSpan {
    pub uri: String,
    pub start: Pos,
    pub end: Pos,
}

pub fn hyperlink_span_at(
    terminal: &Crosswords,
    point: Pos,
    post_process: bool,
) -> Option<HyperlinkSpan> {
    let grid = &terminal.grid;
    if point.row < grid.topmost_line()
        || point.row > grid.bottommost_line()
        || point.col.0 >= grid.columns()
    {
        return None;
    }

    let id = terminal.cell_hyperlink_id(point.row, point.col)?;
    let mut start_col = point.col;
    let mut end_col = point.col;

    while start_col > Column(0) {
        let prev_col = start_col - 1;
        if terminal.cell_hyperlink_id(point.row, prev_col) == Some(id) {
            start_col = prev_col;
        } else {
            break;
        }
    }

    while end_col < grid.columns() - 1 {
        let next_col = end_col + 1;
        if terminal.cell_hyperlink_id(point.row, next_col) == Some(id) {
            end_col = next_col;
        } else {
            break;
        }
    }

    let hyperlink = terminal.cell_hyperlink(point.row, point.col)?;
    let uri = if post_process {
        post_process_hyperlink_uri(hyperlink.uri())
    } else {
        hyperlink.uri().to_string()
    };

    Some(HyperlinkSpan {
        uri,
        start: Pos::new(point.row, start_col),
        end: Pos::new(point.row, end_col),
    })
}

pub fn line_for_absolute_row(abs_row: usize, history_size: usize) -> Line {
    let line = abs_row as i64 - history_size as i64;
    Line(line.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalFileLinkProbe {
    pub row_text: String,
    pub abs_row: usize,
    pub col: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalWrappedLinkProbe {
    /// Full logical terminal line with soft-wrapped physical rows joined.
    pub row_text: String,
    /// Character offset of the pointed cell in `row_text`.
    pub col: usize,
    /// Absolute physical row under the pointer.
    pub abs_row: usize,
    /// Character range occupied by that physical row inside `row_text`.
    pub physical_row_start: usize,
    pub physical_row_len: usize,
}

pub fn terminal_file_link_probe(
    terminal: &Crosswords,
    point: Pos,
) -> Option<TerminalFileLinkProbe> {
    let history_size = terminal.history_size();
    let abs_row = history_size as i64 + point.row.0 as i64;
    if abs_row < 0 {
        return None;
    }
    let abs_row = abs_row as usize;
    let line = line_for_absolute_row(abs_row, history_size);
    if line.0 > terminal.bottommost_line().0 {
        return None;
    }

    let row = &terminal.grid[line];
    let row_text: String = row
        .inner
        .iter()
        .map(|cell| {
            let c = cell.c();
            if c == '\0' {
                ' '
            } else {
                c
            }
        })
        .collect();

    let col = point.col.0;
    if col >= row_text.chars().count() {
        return None;
    }

    Some(TerminalFileLinkProbe {
        row_text,
        abs_row,
        col,
    })
}

/// Build a logical-line probe for text that the terminal soft-wrapped over
/// several physical rows. The scan is deliberately capped: links remain
/// discoverable in very narrow panes without allowing a pathological stream
/// of wrap flags to turn a mouse move into an unbounded grid walk.
pub fn terminal_wrapped_link_probe(
    terminal: &Crosswords,
    point: Pos,
) -> Option<TerminalWrappedLinkProbe> {
    const MAX_WRAPPED_ROWS: usize = 64;

    let history_size = terminal.history_size();
    let abs_row = history_size as i64 + point.row.0 as i64;
    if abs_row < 0 {
        return None;
    }
    let abs_row = abs_row as usize;
    let max_abs_row =
        history_size.saturating_add(terminal.bottommost_line().0.max(0) as usize);
    if abs_row > max_abs_row {
        return None;
    }

    let last_col = terminal.grid.last_column();
    let mut start_abs = abs_row;
    while start_abs > 0 && abs_row - start_abs < MAX_WRAPPED_ROWS - 1 {
        let previous = line_for_absolute_row(start_abs - 1, history_size);
        if !terminal.grid[previous][last_col].wrapline() {
            break;
        }
        start_abs -= 1;
    }

    let mut end_abs = abs_row;
    while end_abs < max_abs_row && end_abs - start_abs < MAX_WRAPPED_ROWS - 1 {
        let current = line_for_absolute_row(end_abs, history_size);
        if !terminal.grid[current][last_col].wrapline() {
            break;
        }
        end_abs += 1;
    }

    let mut row_text = String::new();
    let mut physical_row_start = 0usize;
    let mut physical_row_len = 0usize;
    for current_abs in start_abs..=end_abs {
        let line = line_for_absolute_row(current_abs, history_size);
        let row = &terminal.grid[line];
        if current_abs == abs_row {
            physical_row_start = row_text.chars().count();
            physical_row_len = row.inner.len();
        }
        row_text.extend(row.inner.iter().map(|cell| {
            let ch = cell.c();
            if ch == '\0' {
                ' '
            } else {
                ch
            }
        }));
    }

    let pointed_col = point.col.0;
    if pointed_col >= physical_row_len {
        return None;
    }
    Some(TerminalWrappedLinkProbe {
        row_text,
        col: physical_row_start + pointed_col,
        abs_row,
        physical_row_start,
        physical_row_len,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileLinkOpenTarget {
    Directory,
    Markdown,
    Editor,
}

pub const fn file_link_open_target(
    is_dir: bool,
    is_markdown: bool,
) -> FileLinkOpenTarget {
    if is_dir {
        FileLinkOpenTarget::Directory
    } else if is_markdown {
        FileLinkOpenTarget::Markdown
    } else {
        FileLinkOpenTarget::Editor
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalTextArea {
    pub margin_left_px: f32,
    pub margin_top_px: f32,
    pub columns: usize,
    pub lines: usize,
    pub cell_width_px: f32,
    pub cell_height_px: f32,
}

impl TerminalTextArea {
    pub fn contains_point(self, x_px: usize, y_px: usize) -> bool {
        contains_terminal_point(self, x_px, y_px)
    }
}

pub fn contains_terminal_point(area: TerminalTextArea, x_px: usize, y_px: usize) -> bool {
    let right = area.margin_left_px + area.columns as f32 * area.cell_width_px;
    let bottom = area.margin_top_px + area.lines as f32 * area.cell_height_px;
    x_px > area.margin_left_px as usize
        && x_px <= right as usize
        && y_px > area.margin_top_px as usize
        && y_px <= bottom as usize
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalBodyMouseRowInput {
    pub panel_top_logical: f32,
    pub terminal_scroll_offset_logical: f32,
    pub cell_height_logical: f32,
    pub mouse_y_logical: f32,
}

pub fn terminal_body_visual_row(input: TerminalBodyMouseRowInput) -> i32 {
    let cell_height = input.cell_height_logical.max(1.0);
    ((input.mouse_y_logical
        - input.panel_top_logical
        - input.terminal_scroll_offset_logical)
        / cell_height)
        .floor() as i32
}

pub fn should_open_file_link_on_click(
    click: SelectionClickKind,
    modifiers: SelectionModifiers,
) -> bool {
    matches!(click, SelectionClickKind::Single) && modifiers.plain()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalFileLinkHoverInput {
    pub link_col_start: usize,
    pub link_col_end: usize,
    pub panel_left_logical: f32,
    pub panel_top_logical: f32,
    pub terminal_scroll_offset_logical: f32,
    pub cell_width_logical: f32,
    pub cell_height_logical: f32,
    pub visible_lines: usize,
    pub mouse_y_logical: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalFileLinkHoverRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub fn terminal_file_link_hover_rect(
    input: TerminalFileLinkHoverInput,
) -> Option<TerminalFileLinkHoverRect> {
    let shifted_panel_top_logical =
        input.panel_top_logical + input.terminal_scroll_offset_logical;
    let displayed_row = ((input.mouse_y_logical - shifted_panel_top_logical)
        / input.cell_height_logical)
        .floor() as i32;
    if displayed_row < 0 || displayed_row as usize >= input.visible_lines {
        return None;
    }

    let cols = input.link_col_end.saturating_sub(input.link_col_start) as f32;
    if cols <= 0.0 {
        return None;
    }

    Some(TerminalFileLinkHoverRect {
        x: input.panel_left_logical
            + input.link_col_start as f32 * input.cell_width_logical,
        y: shifted_panel_top_logical
            + displayed_row as f32 * input.cell_height_logical
            + input.cell_height_logical
            - 1.5,
        width: cols * input.cell_width_logical,
        height: 1.5,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_terminal_core::ansi::CursorShape;
    use neoism_terminal_core::crosswords::pos::{Column, Direction};
    use neoism_terminal_core::crosswords::CrosswordsSize;
    use neoism_terminal_core::handler::{Processor, StdSyncHandler};

    fn pos(row: i32, col: usize) -> Pos {
        Pos::new(Line(row), Column(col))
    }

    fn terminal_with_text(width: usize, height: usize, rows: &[&str]) -> Crosswords {
        let mut terminal = Crosswords::new(
            CrosswordsSize::new(width, height),
            CursorShape::Block,
            neoism_terminal_core::TerminalId::new(0),
            10_000,
        );
        for (row_idx, text) in rows.iter().enumerate() {
            for (col_idx, c) in text.chars().take(width).enumerate() {
                terminal.grid[Line(row_idx as i32)][Column(col_idx)].set_c(c);
            }
        }
        terminal
    }

    fn terminal_with_osc8(bytes: &[u8]) -> Crosswords {
        let mut terminal = Crosswords::new(
            CrosswordsSize::new(40, 5),
            CursorShape::Block,
            neoism_terminal_core::TerminalId::new(0),
            10_000,
        );
        let mut processor = Processor::<StdSyncHandler>::new();
        processor.advance(&mut terminal, bytes);
        terminal
    }

    #[test]
    fn single_click_starts_simple_or_block_selection() {
        assert_eq!(
            left_click_selection_action(
                SelectionClickKind::Single,
                SelectionModifiers::default(),
                false,
                pos(4, 2),
                Direction::Left,
            ),
            LeftClickSelectionAction::Start {
                ty: SelectionType::Simple,
                point: pos(4, 2),
                side: Direction::Left,
                clear_existing: true,
            }
        );

        assert_eq!(
            left_click_selection_action(
                SelectionClickKind::Single,
                SelectionModifiers::new(false, true, false),
                false,
                pos(4, 2),
                Direction::Right,
            ),
            LeftClickSelectionAction::Start {
                ty: SelectionType::Block,
                point: pos(4, 2),
                side: Direction::Right,
                clear_existing: true,
            }
        );
    }

    #[test]
    fn hint_modifier_matching_accepts_aliases_and_requires_all_mods() {
        let mods = HintModifierState::new(true, true, false, true);

        assert!(hint_modifiers_match(&[], HintModifierState::default()));
        assert!(hint_modifiers_match(&["Shift".to_string()], mods));
        assert!(hint_modifiers_match(&["Ctrl".to_string()], mods));
        assert!(hint_modifiers_match(&["Control".to_string()], mods));
        assert!(hint_modifiers_match(&["Cmd".to_string()], mods));
        assert!(hint_modifiers_match(&["Command".to_string()], mods));
        assert!(hint_modifiers_match(&["Super".to_string()], mods));
        assert!(hint_modifiers_match(
            &["Shift".to_string(), "Command".to_string()],
            mods,
        ));

        assert!(!hint_modifiers_match(&["Alt".to_string()], mods));
        assert!(!hint_modifiers_match(
            &["Shift".to_string(), "Alt".to_string()],
            mods,
        ));
        assert!(!hint_modifiers_match(&["Meta".to_string()], mods));
    }

    #[test]
    fn hint_highlight_eligibility_requires_enabled_mouse_and_matching_mods() {
        let shift = vec!["Shift".to_string()];
        let alt = vec!["Alt".to_string()];
        let hints = [
            HintMouseActivation {
                mouse_enabled: false,
                hyperlinks: true,
                mouse_mods: &shift,
            },
            HintMouseActivation {
                mouse_enabled: true,
                hyperlinks: false,
                mouse_mods: &alt,
            },
        ];

        assert!(!hint_highlight_eligible(
            hints,
            HintModifierState::new(true, false, false, false)
        ));
        assert!(hint_highlight_eligible(
            hints,
            HintModifierState::new(false, false, true, false)
        ));
    }

    #[test]
    fn hyperlink_trigger_eligibility_requires_range_hyperlink_and_mods() {
        let shift = vec!["Shift".to_string()];
        let alt = vec!["Alt".to_string()];
        let hints = [
            HintMouseActivation {
                mouse_enabled: false,
                hyperlinks: true,
                mouse_mods: &shift,
            },
            HintMouseActivation {
                mouse_enabled: true,
                hyperlinks: false,
                mouse_mods: &alt,
            },
        ];

        assert!(!hyperlink_trigger_eligible(
            hints,
            HintModifierState::new(true, false, false, false),
            false,
        ));
        assert!(hyperlink_trigger_eligible(
            hints,
            HintModifierState::new(true, false, false, false),
            true,
        ));
        assert!(!hyperlink_trigger_eligible(
            hints,
            HintModifierState::new(false, false, true, false),
            true,
        ));
    }

    #[test]
    fn hint_label_placements_expand_visible_remaining_labels() {
        let placements = hint_label_placements(
            true,
            &[pos(3, 4), pos(9, 1)],
            &[(1, vec!['a', 's']), (0, vec!['d'])],
        );

        assert_eq!(
            placements,
            vec![
                HintLabelPlacement {
                    position: pos(9, 1),
                    label: vec!['a'],
                    is_first: true,
                },
                HintLabelPlacement {
                    position: pos(9, 2),
                    label: vec!['s'],
                    is_first: false,
                },
                HintLabelPlacement {
                    position: pos(3, 4),
                    label: vec!['d'],
                    is_first: true,
                },
            ]
        );
    }

    #[test]
    fn hint_label_placements_ignore_inactive_or_stale_match_indices() {
        assert_eq!(
            hint_label_placements(false, &[pos(3, 4)], &[(0, vec!['a'])]),
            Vec::new()
        );
        assert_eq!(
            hint_label_placements(true, &[pos(3, 4)], &[(99, vec!['a'])]),
            Vec::new()
        );
    }

    #[test]
    fn shifted_single_click_extends_existing_selection() {
        assert_eq!(
            left_click_selection_action(
                SelectionClickKind::Single,
                SelectionModifiers::new(true, false, false),
                true,
                pos(7, 9),
                Direction::Right,
            ),
            LeftClickSelectionAction::Extend {
                point: pos(7, 9),
                side: Direction::Right,
            }
        );

        assert!(matches!(
            left_click_selection_action(
                SelectionClickKind::Single,
                SelectionModifiers::new(true, false, false),
                false,
                pos(7, 9),
                Direction::Right,
            ),
            LeftClickSelectionAction::Start {
                ty: SelectionType::Simple,
                ..
            }
        ));
    }

    #[test]
    fn double_and_triple_click_choose_semantic_and_line_selection() {
        assert!(matches!(
            left_click_selection_action(
                SelectionClickKind::Double,
                SelectionModifiers::default(),
                false,
                pos(0, 0),
                Direction::Left,
            ),
            LeftClickSelectionAction::Start {
                ty: SelectionType::Semantic,
                ..
            }
        ));
        assert!(matches!(
            left_click_selection_action(
                SelectionClickKind::Triple,
                SelectionModifiers::default(),
                false,
                pos(0, 0),
                Direction::Left,
            ),
            LeftClickSelectionAction::Start {
                ty: SelectionType::Lines,
                ..
            }
        ));
    }

    #[test]
    fn update_plan_clamps_to_bottom_and_marks_vi_expansion() {
        assert_eq!(
            selection_update_plan(pos(99, 4), Line(12), true, false),
            SelectionUpdatePlan {
                point: pos(12, 4),
                move_vi_cursor: true,
                include_all: true,
            }
        );
        assert_eq!(
            selection_update_plan(pos(99, 4), Line(12), true, true),
            SelectionUpdatePlan {
                point: pos(12, 4),
                move_vi_cursor: false,
                include_all: false,
            }
        );
    }

    #[test]
    fn toggle_selection_matches_desktop_policy() {
        assert_eq!(
            toggle_selection_action(
                Some(SelectionSnapshot {
                    ty: SelectionType::Simple,
                    is_empty: false,
                }),
                SelectionType::Simple,
            ),
            ToggleSelectionAction::Clear
        );
        assert_eq!(
            toggle_selection_action(
                Some(SelectionSnapshot {
                    ty: SelectionType::Simple,
                    is_empty: false,
                }),
                SelectionType::Lines,
            ),
            ToggleSelectionAction::RetypeExisting
        );
        assert_eq!(
            toggle_selection_action(None, SelectionType::Lines),
            ToggleSelectionAction::StartAtCursor
        );
    }

    #[test]
    fn terminal_text_area_uses_existing_exclusive_left_top_inclusive_right_bottom_bounds()
    {
        let area = TerminalTextArea {
            margin_left_px: 10.0,
            margin_top_px: 20.0,
            columns: 5,
            lines: 2,
            cell_width_px: 8.0,
            cell_height_px: 16.0,
        };

        assert!(!area.contains_point(10, 25));
        assert!(!area.contains_point(20, 20));
        assert!(area.contains_point(11, 21));
        assert!(area.contains_point(50, 52));
        assert!(!area.contains_point(51, 52));
        assert!(!area.contains_point(50, 53));
    }

    #[test]
    fn terminal_body_visual_row_accounts_for_panel_scroll_and_cell_height() {
        assert_eq!(
            terminal_body_visual_row(TerminalBodyMouseRowInput {
                panel_top_logical: 20.0,
                terminal_scroll_offset_logical: 8.0,
                cell_height_logical: 11.0,
                mouse_y_logical: 61.0,
            }),
            3
        );
        assert_eq!(
            terminal_body_visual_row(TerminalBodyMouseRowInput {
                panel_top_logical: 20.0,
                terminal_scroll_offset_logical: 8.0,
                cell_height_logical: 0.0,
                mouse_y_logical: 30.5,
            }),
            2
        );
    }

    #[test]
    fn terminal_file_link_hover_rect_tracks_visible_mouse_row() {
        assert_eq!(
            terminal_file_link_hover_rect(TerminalFileLinkHoverInput {
                link_col_start: 3,
                link_col_end: 8,
                panel_left_logical: 10.0,
                panel_top_logical: 20.0,
                terminal_scroll_offset_logical: 4.0,
                cell_width_logical: 7.0,
                cell_height_logical: 11.0,
                visible_lines: 4,
                mouse_y_logical: 47.0,
            }),
            Some(TerminalFileLinkHoverRect {
                x: 31.0,
                y: 55.5,
                width: 35.0,
                height: 1.5,
            })
        );
    }

    #[test]
    fn terminal_file_link_hover_rect_rejects_out_of_bounds_or_empty_links() {
        let input = TerminalFileLinkHoverInput {
            link_col_start: 3,
            link_col_end: 8,
            panel_left_logical: 10.0,
            panel_top_logical: 20.0,
            terminal_scroll_offset_logical: 4.0,
            cell_width_logical: 7.0,
            cell_height_logical: 11.0,
            visible_lines: 2,
            mouse_y_logical: 60.0,
        };

        assert_eq!(terminal_file_link_hover_rect(input), None);
        assert_eq!(
            terminal_file_link_hover_rect(TerminalFileLinkHoverInput {
                link_col_start: 8,
                link_col_end: 8,
                mouse_y_logical: 30.0,
                ..input
            }),
            None
        );
    }

    #[test]
    fn selection_with_range_constructs_selection_and_matching_range() {
        let terminal = terminal_with_text(8, 2, &["abcdef"]);
        let (selection, range) = selection_with_range(
            &terminal,
            SelectionType::Simple,
            SelectionEndpoint::new(pos(0, 1), Direction::Left),
            Some(SelectionEndpoint::new(pos(0, 3), Direction::Right)),
        );

        assert_eq!(selection.ty, SelectionType::Simple);
        assert_eq!(
            range,
            Some(SelectionRange::new(pos(0, 1), pos(0, 3), false))
        );
    }

    #[test]
    fn selected_text_prefers_terminal_selection_and_filters_empty_text() {
        let mut terminal = terminal_with_text(8, 2, &["abcdef"]);
        terminal.selection = Some(
            selection_with_range(
                &terminal,
                SelectionType::Simple,
                SelectionEndpoint::new(pos(0, 1), Direction::Left),
                Some(SelectionEndpoint::new(pos(0, 3), Direction::Right)),
            )
            .0,
        );

        assert_eq!(selected_text(&terminal, None).as_deref(), Some("bcd"));

        terminal.selection = None;
        assert_eq!(
            selected_text(
                &terminal,
                Some(SelectionRange::new(pos(0, 2), pos(0, 4), false)),
            )
            .as_deref(),
            Some("cde")
        );

        let empty_terminal = terminal_with_text(4, 1, &[""]);
        assert_eq!(
            selected_text(
                &empty_terminal,
                Some(SelectionRange::new(pos(0, 0), pos(0, 0), false)),
            ),
            None
        );
    }

    #[test]
    fn apply_selection_update_mutates_selection_and_returns_render_range() {
        let mut terminal = terminal_with_text(8, 3, &["abcdef", "ghijkl"]);
        terminal.selection = Some(
            selection_with_range(
                &terminal,
                SelectionType::Simple,
                SelectionEndpoint::new(pos(0, 1), Direction::Left),
                None,
            )
            .0,
        );

        let range = apply_selection_update(
            &mut terminal,
            pos(1, 2),
            Direction::Right,
            false,
            false,
        );

        assert_eq!(
            range,
            Some(SelectionRange::new(pos(0, 1), pos(1, 2), false))
        );
        assert!(terminal.selection.is_some());
    }

    #[test]
    fn apply_selection_update_clamps_and_moves_vi_cursor_outside_search() {
        let mut terminal = terminal_with_text(8, 3, &["abcdef", "ghijkl"]);
        terminal.selection = Some(
            selection_with_range(
                &terminal,
                SelectionType::Simple,
                SelectionEndpoint::new(pos(0, 1), Direction::Left),
                None,
            )
            .0,
        );

        let range = apply_selection_update(
            &mut terminal,
            pos(99, 2),
            Direction::Right,
            true,
            false,
        );

        assert_eq!(terminal.vi_mode_cursor.pos, pos(2, 2));
        assert_eq!(
            range,
            terminal
                .selection
                .as_ref()
                .and_then(|s| s.to_range(&terminal))
        );
    }

    #[test]
    fn include_all_current_selection_expands_empty_initial_selection() {
        let mut terminal = terminal_with_text(8, 2, &["abcdef"]);
        terminal.selection = Some(
            selection_with_range(
                &terminal,
                SelectionType::Simple,
                SelectionEndpoint::new(pos(0, 2), Direction::Left),
                None,
            )
            .0,
        );

        let range = include_all_current_selection(&mut terminal);

        assert!(range.is_some());
        assert_eq!(
            range,
            terminal
                .selection
                .as_ref()
                .and_then(|s| s.to_range(&terminal))
        );
    }

    #[test]
    fn hint_match_post_processing_trims_unmatched_closing_groups() {
        let chars: Vec<char> = "https://example.test/path)".chars().collect();
        assert_eq!(post_process_hint_match_end(&chars), Some(24));

        let chars: Vec<char> = "(https://example.test/path)".chars().collect();
        assert_eq!(post_process_hint_match_end(&chars), Some(chars.len() - 1));

        let chars: Vec<char> = "]".chars().collect();
        assert_eq!(post_process_hint_match_end(&chars), None);
    }

    #[test]
    fn hint_match_post_processing_trims_trailing_delimiters() {
        let chars: Vec<char> = "https://example.test/path,.;?!".chars().collect();
        assert_eq!(post_process_hint_match_end(&chars), Some(24));

        let chars: Vec<char> = "https://example.test/path".chars().collect();
        assert_eq!(post_process_hint_match_end(&chars), Some(chars.len() - 1));
    }

    #[test]
    fn hyperlink_uri_post_processing_matches_hint_policy() {
        assert_eq!(
            post_process_hyperlink_uri("https://example.com/path(with)parens"),
            "https://example.com/path(with)parens"
        );
        assert_eq!(
            post_process_hyperlink_uri("https://example.com/path]"),
            "https://example.com/path"
        );
        assert_eq!(
            post_process_hyperlink_uri("https://example.com.'),"),
            "https://example.com"
        );
    }

    #[test]
    fn hyperlink_span_walks_same_osc8_cell_id() {
        let terminal = terminal_with_osc8(
            b"go \x1b]8;;https://example.com/path]\x07click\x1b]8;;\x07.",
        );

        assert_eq!(
            hyperlink_span_at(&terminal, pos(0, 5), true),
            Some(HyperlinkSpan {
                uri: "https://example.com/path".to_string(),
                start: pos(0, 3),
                end: pos(0, 7),
            })
        );
        assert_eq!(hyperlink_span_at(&terminal, pos(0, 8), true), None);
    }

    #[test]
    fn terminal_file_link_probe_extracts_visible_row_text_and_absolute_row() {
        let terminal = terminal_with_text(12, 2, &["src/main.rs", "other"]);

        assert_eq!(
            terminal_file_link_probe(&terminal, pos(0, 4)),
            Some(TerminalFileLinkProbe {
                row_text: "src/main.rs ".to_string(),
                abs_row: 0,
                col: 4,
            })
        );
        assert_eq!(terminal_file_link_probe(&terminal, pos(-1, 0)), None);
    }

    #[test]
    fn file_link_open_target_prefers_directory_then_markdown() {
        assert_eq!(
            file_link_open_target(true, true),
            FileLinkOpenTarget::Directory
        );
        assert_eq!(
            file_link_open_target(false, true),
            FileLinkOpenTarget::Markdown
        );
        assert_eq!(
            file_link_open_target(false, false),
            FileLinkOpenTarget::Editor
        );
    }
}
