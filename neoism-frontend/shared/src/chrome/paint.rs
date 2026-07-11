use super::*;

use sugarloaf::Sugarloaf;

use crate::event::{LogicalKey, UiEvent};
use crate::layout::Rect;
use crate::render_policy::{
    editor_cursor_output_row, editor_visible_row_sample, EditorScrollGridRenderState,
    EditorVisibleRowSource,
};

use crate::panels::StatusPalette;
use crate::primitives::IdeTheme;
use crate::theme::ChromeTheme;
use crate::theme::RgbTriple;

pub(crate) fn rgb_u32(c: RgbTriple) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

pub(crate) fn status_palette_from_theme(theme: &ChromeTheme) -> StatusPalette {
    StatusPalette {
        bg: rgb_u32(theme.bg),
        surface: rgb_u32(theme.bg_elevated),
        muted: rgb_u32(theme.fg_dim),
        red: rgb_u32(theme.error),
        green: rgb_u32(theme.success),
        yellow: rgb_u32(theme.yellow),
        blue: rgb_u32(theme.accent),
        magenta: rgb_u32(theme.magenta),
        cyan: rgb_u32(theme.cyan),
        black: rgb_u32(theme.black),
    }
}

pub(crate) fn pointer_inside(event: &UiEvent, rect: Rect) -> bool {
    match event {
        UiEvent::PointerMove { x, y, .. }
        | UiEvent::PointerDown { x, y, .. }
        | UiEvent::PointerUp { x, y, .. } => rect.contains(*x, *y),
        // Wheel doesn't carry coords in this event vocabulary; treat
        // as inside any rect so the priority-order top still gets to
        // consume it. PointerLeave fans out to everyone for the same
        // reason — panels self-arbitrate by tracking their hover state.
        UiEvent::Wheel { .. } | UiEvent::PointerLeave => true,
        _ => true,
    }
}

pub(crate) fn is_modal_key(key: PanelKey) -> bool {
    matches!(
        key,
        PanelKey::CommandPalette | PanelKey::Finder | PanelKey::GitDiff
    )
}

pub(crate) fn is_character_key(logical: &LogicalKey, needle: &str) -> bool {
    matches!(logical, LogicalKey::Character(ch) if ch.eq_ignore_ascii_case(needle))
}

pub(crate) fn is_colon_or_semicolon_key(logical: &LogicalKey) -> bool {
    matches!(logical, LogicalKey::Character(ch) if ch == ":" || ch == ";")
}

/// Paint a row-major nvim grid snapshot into the file-viewer pane.
///
/// One pass walks `grid.cells` (length `width * height`) and emits up
/// to two GPU primitives per cell: a bg quad when `cell.bg !=
/// default_bg`, and a glyph when `cell.ch` is non-empty. Pixel math
/// reuses the chrome's current `cell_w`/`cell_h` so the grid lines up
/// with the terminal's own cell metrics. `offset` is the rubber-band
/// smooth-scroll offset already accumulated by the file-viewer scroll
/// loop — applied per-row so the wheel still feels native.
///
/// Cursor paint is a single accent quad with an inverted glyph on
/// top: takes the resolved cell's fg color, blits a quad in
/// `theme.accent`, and re-draws the glyph in `theme.bg` for contrast.
/// Out-of-band cursor coordinates (row >= height / col >= width) are
/// dropped silently — nvim is supposed to clamp before publishing,
/// but the renderer stays defensive.
///
/// Rows whose top falls outside `terminal_rect` are culled cheaply by
/// the `y < rect.y - cell_h` / `y > rect.y + rect.h` guard so a tall
/// buffer doesn't issue a quad per cell every frame.
pub(crate) fn paint_editor_grid(
    sugarloaf: &mut Sugarloaf,
    grid: &crate::editor_snapshot::EditorGridSnapshot,
    scrollback_grid: Option<&crate::editor_snapshot::EditorGridSnapshot>,
    scrollback_above_rows: &[crate::editor_snapshot::GridCell],
    scrollback_below_rows: &[crate::editor_snapshot::GridCell],
    terminal_rect: Rect,
    theme: &IdeTheme,
    cell_w: f32,
    cell_h: f32,
    scroll_state: EditorScrollGridRenderState,
    cursor_shape: neoism_terminal_core::ansi::CursorShape,
    paint_static_cursor: bool,
) {
    use sugarloaf::text::DrawOpts;

    let pad_x = 0.0_f32;
    let pad_y = 0.0_f32;
    let max_w = (terminal_rect.w - pad_x * 2.0).max(0.0);
    let max_h = (terminal_rect.h - pad_y * 2.0).max(0.0);
    if max_w <= 0.0 || max_h <= 0.0 || grid.width == 0 || grid.height == 0 {
        return;
    }

    // Clip every paint to the content rect so partial cells at the
    // edges don't bleed onto the file tree / agent pane during a
    // scroll animation.
    let clip = [
        terminal_rect.x + pad_x,
        terminal_rect.y + pad_y,
        max_w,
        max_h,
    ];
    let font_size = (cell_h * 0.875).clamp(8.0, 32.0);
    let text_y_pad = ((cell_h - font_size) * 0.5).max(0.0);
    let base_opts = DrawOpts {
        font_size,
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };

    // `default_bg` is the cell-bg the daemon resolves from nvim's
    // highlight table. We use it (NOT theme.bg) to decide which cells
    // need their own bg quad — many themes intentionally pick a bg
    // that differs from the chrome's window bg.
    let default_bg = grid.default_bg;
    let width = grid.width as usize;
    let height = grid.height as usize;
    let visual_offset = if scroll_state.pixel_offset_y.is_finite() {
        scroll_state.pixel_offset_y
    } else {
        0.0
    };
    let source_line_offset = scroll_state.source_line_offset;
    let scrollback_grid = scrollback_grid.filter(|prev| {
        prev.width == grid.width
            && prev.height == grid.height
            && prev.cells.len() == grid.cells.len()
    });
    let above_edge_rows = scrollback_above_rows.len() / width;
    let below_edge_rows = scrollback_below_rows.len() / width;

    let first_output_row = -1_i32;
    let last_output_row = height.min(i32::MAX as usize) as i32;
    for output_row in first_output_row..=last_output_row {
        let y_top =
            terminal_rect.y + pad_y + (output_row as f32) * cell_h + visual_offset;
        // Cull whole rows that fall outside the visible band with one
        // row of slop on each side so a partial row at the top/bottom
        // still renders during a scroll animation.
        if y_top + cell_h < terminal_rect.y - cell_h
            || y_top > terminal_rect.y + terminal_rect.h
        {
            continue;
        }
        let text_y = y_top + text_y_pad;
        sugarloaf.rect(
            None,
            terminal_rect.x + pad_x,
            y_top,
            max_w,
            cell_h,
            theme.f32(default_bg),
            0.0,
            0,
        );
        let sample = editor_visible_row_sample(
            output_row,
            source_line_offset,
            height,
            scrollback_grid.is_some(),
            above_edge_rows,
            below_edge_rows,
        );
        let row_base = match sample.source {
            EditorVisibleRowSource::Current(source_row)
            | EditorVisibleRowSource::Scrollback(source_row)
            | EditorVisibleRowSource::AboveEdge(source_row)
            | EditorVisibleRowSource::BelowEdge(source_row) => Some(source_row * width),
            EditorVisibleRowSource::Missing => None,
        };
        let Some(row_base) = row_base else {
            continue;
        };
        for col in 0..width {
            let cell = match sample.source {
                EditorVisibleRowSource::Current(_) => grid.cells.get(row_base + col),
                EditorVisibleRowSource::Scrollback(_) => {
                    scrollback_grid.and_then(|prev| prev.cells.get(row_base + col))
                }
                EditorVisibleRowSource::AboveEdge(_) => {
                    scrollback_above_rows.get(row_base + col)
                }
                EditorVisibleRowSource::BelowEdge(_) => {
                    scrollback_below_rows.get(row_base + col)
                }
                EditorVisibleRowSource::Missing => None,
            };
            let Some(cell) = cell else { break };
            let x_left = terminal_rect.x + pad_x + (col as f32) * cell_w;
            // 1. Background quad for any cell that diverges from the
            //    grid-wide default bg. The expected hot-path
            //    cell-matches-default-bg case skips the quad entirely,
            //    keeping per-frame GPU draws bounded by the count of
            //    actually-highlighted cells (selection, search hits,
            //    diagnostics underlines, etc.).
            if cell.bg != default_bg {
                sugarloaf.rect(
                    None,
                    x_left,
                    y_top,
                    cell_w,
                    cell_h,
                    theme.f32(cell.bg),
                    0.0,
                    1,
                );
            }
            // 2. Glyph. nvim publishes `""` for the trailing half of a
            //    double-width grapheme; skip those (the leading cell
            //    already painted the wide glyph).
            if !cell.ch.is_empty() {
                let mut opts = base_opts;
                opts.color = theme.u8(cell.fg);
                sugarloaf.text_mut().draw(x_left, text_y, &cell.ch, &opts);
            }
        }
    }

    // 3. Cursor block on top of the cells. nvim's cursor is a
    //    cell-sized rect with the underlying glyph inverted; we
    //    approximate with theme.accent for the quad + theme.bg for
    //    the glyph (better contrast against the typical bright
    //    accent than re-using the cell's resolved fg, which on a
    //    highlighted line might already match the accent).
    if paint_static_cursor {
        if let Some((c_row, c_col)) = grid.cursor {
            if (c_row as usize) < height && (c_col as usize) < width {
                let x_left = terminal_rect.x + pad_x + (c_col as f32) * cell_w;
                let output_row =
                    editor_cursor_output_row(c_row as i32, source_line_offset);
                let y_top = terminal_rect.y
                    + pad_y
                    + (output_row as f32) * cell_h
                    + visual_offset;
                // Cull if the cursor scrolled off-screen.
                if y_top + cell_h >= terminal_rect.y
                    && y_top <= terminal_rect.y + terminal_rect.h
                {
                    match cursor_shape {
                        neoism_terminal_core::ansi::CursorShape::Hidden => {}
                        neoism_terminal_core::ansi::CursorShape::Beam => {
                            sugarloaf.rect(
                                None,
                                x_left,
                                y_top,
                                (cell_w * 0.15).max(2.0),
                                cell_h,
                                theme.f32(theme.accent),
                                0.0,
                                30,
                            );
                        }
                        neoism_terminal_core::ansi::CursorShape::Underline => {
                            let h = (cell_h * 0.20).max(2.0);
                            sugarloaf.rect(
                                None,
                                x_left,
                                y_top + cell_h - h,
                                cell_w,
                                h,
                                theme.f32(theme.accent),
                                0.0,
                                30,
                            );
                        }
                        neoism_terminal_core::ansi::CursorShape::Block => {
                            sugarloaf.rect(
                                None,
                                x_left,
                                y_top,
                                cell_w,
                                cell_h,
                                theme.f32(theme.accent),
                                0.0,
                                30,
                            );
                            let idx = (c_row as usize) * width + (c_col as usize);
                            if let Some(cell) = grid.cells.get(idx) {
                                if !cell.ch.is_empty() {
                                    let mut opts = base_opts;
                                    opts.color = theme.u8(theme.bg);
                                    sugarloaf.text_mut().draw(
                                        x_left,
                                        y_top + text_y_pad,
                                        &cell.ch,
                                        &opts,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[allow(dead_code)]
pub(crate) fn paint_editor_grid_cells(
    sugarloaf: &mut Sugarloaf,
    grid: &crate::editor_snapshot::EditorGridSnapshot,
    terminal_rect: Rect,
    theme: &IdeTheme,
    cell_w: f32,
    cell_h: f32,
    offset: f32,
    clip_rect: Option<[f32; 4]>,
    paint_backgrounds: bool,
) {
    use sugarloaf::text::DrawOpts;

    let pad_x = 0.0_f32;
    let pad_y = 0.0_f32;
    let font_size = (cell_h * 0.875).clamp(8.0, 32.0);
    let text_y_pad = ((cell_h - font_size) * 0.5).max(0.0);
    let base_opts = DrawOpts {
        font_size,
        color: theme.u8(theme.fg),
        clip_rect,
        ..DrawOpts::default()
    };
    let default_bg = grid.default_bg;
    let width = grid.width as usize;
    let height = grid.height as usize;

    for row in 0..height {
        let y_top = terminal_rect.y + pad_y + (row as f32) * cell_h - offset;
        if y_top + cell_h < terminal_rect.y - cell_h
            || y_top > terminal_rect.y + terminal_rect.h
        {
            continue;
        }
        let text_y = y_top + text_y_pad;
        let row_base = row * width;
        for col in 0..width {
            let Some(cell) = grid.cells.get(row_base + col) else {
                break;
            };
            let x_left = terminal_rect.x + pad_x + (col as f32) * cell_w;
            if paint_backgrounds && cell.bg != default_bg {
                sugarloaf.rect(
                    None,
                    x_left,
                    y_top,
                    cell_w,
                    cell_h,
                    theme.f32(cell.bg),
                    0.0,
                    1,
                );
            }
            if !cell.ch.is_empty() {
                let mut opts = base_opts;
                opts.color = theme.u8(cell.fg);
                sugarloaf.text_mut().draw(x_left, text_y, &cell.ch, &opts);
            }
        }
    }
}

pub(crate) fn editor_grid_has_visible_text(
    grid: &crate::editor_snapshot::EditorGridSnapshot,
) -> bool {
    grid.width > 0
        && grid.height > 0
        && grid.cells.iter().any(|cell| {
            !cell.ch.is_empty() && !cell.ch.chars().all(|ch| ch.is_whitespace())
        })
}
