use super::*;

/// Spring-quantized editor scroll offset, split into an integer source
/// row stride and a fractional pixel residual.
///
/// `source_line_offset` is the integer number of source rows the
/// viewport has shifted (positive scrolls forward). `pixel_offset_y`
/// is the residual sub-row offset in physical pixels, already rounded
/// to whole pixels so the GPU shader uniform path doesn't introduce
/// sub-pixel "swim".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorScrollRenderOffset {
    pub source_line_offset: i32,
    pub pixel_offset_y: f32,
}

/// Convert a smooth-scroll spring + elastic rubber-band offset into a
/// Neovide/Ghostty-style row + pixel split.
///
/// The spring position is in lines (floats allowed for sub-row
/// animation). We `floor()` the spring to pick the source-row stride
/// and let the fractional remainder become the pixel offset; that
/// bundles each row-crossing into a single SHIFT plan instead of
/// dragging across many uniform-only frames.
///
/// `previous_source_line_offset` is accepted for call-site parity with
/// the historical native helper but is currently unused — the floor
/// split is stateless. Keeping the parameter avoids reshuffling every
/// caller if hysteresis returns.
pub fn editor_scroll_render_offset(
    scroll_position_lines: f32,
    elastic_offset_y: f32,
    cell_h: f32,
    previous_source_line_offset: Option<i32>,
) -> EditorScrollRenderOffset {
    if cell_h <= 0.0 || !cell_h.is_finite() {
        return EditorScrollRenderOffset::default();
    }
    let line_position = scroll_position_lines;
    if !line_position.is_finite() {
        return EditorScrollRenderOffset::default();
    }

    let source_line_offset = line_position.floor() as i32;
    let _ = previous_source_line_offset;
    // Quantize the spring offset to integer pixels so the bg-cell
    // shader lookup and the glyph origin agree on cell boundaries.
    // Sub-pixel float offsets caused text-vs-bg "swim" during
    // continuous scroll on Linux/Vulkan; rounding here keeps the
    // editor smooth-scroll path in sync with the terminal pane's
    // `offset.abs().ceil()` step.
    let pixel_offset_y = (((source_line_offset as f32 - line_position) * cell_h)
        + elastic_offset_y)
        .round();

    EditorScrollRenderOffset {
        source_line_offset,
        pixel_offset_y,
    }
}

/// Convert scroll spring position for an already-mutated daemon grid
/// snapshot. Native desktop paints from a retained grid/ring and uses
/// the Neovide floor split above. Web daemon snapshots have already
/// applied nvim's `grid_scroll`, so positive offsets need the mirrored
/// row split: keep sampling the previous visible row (`ceil`) while
/// easing the new top row into view from above.
pub fn editor_scroll_render_offset_for_mutated_snapshot(
    scroll_position_lines: f32,
    elastic_offset_y: f32,
    cell_h: f32,
    previous_source_line_offset: Option<i32>,
) -> EditorScrollRenderOffset {
    if cell_h <= 0.0 || !cell_h.is_finite() {
        return EditorScrollRenderOffset::default();
    }
    let line_position = scroll_position_lines;
    if !line_position.is_finite() {
        return EditorScrollRenderOffset::default();
    }

    let source_line_offset = if line_position > 0.0 {
        line_position.ceil() as i32
    } else {
        line_position.floor() as i32
    };
    let _ = previous_source_line_offset;
    let pixel_offset_y = (((source_line_offset as f32 - line_position) * cell_h)
        + elastic_offset_y)
        .round();

    EditorScrollRenderOffset {
        source_line_offset,
        pixel_offset_y,
    }
}

/// Invert the visible-row -> source-row mapping for cursor/cursorline
/// overlays. The render path samples `source_y = output_y +
/// source_line_offset` per visible row; the cursor row reported by
/// nvim is a live source row, so we subtract to map it back to its
/// output row.
pub fn editor_cursor_output_row(cursor_row: i32, source_line_offset: i32) -> i32 {
    editor_output_row_for_source(cursor_row, source_line_offset)
}

/// GPU grid row for an editor cursor sprite/uniform. Editor panes keep
/// hidden rows around the visible viewport for fractional scroll, so
/// the cursor's visible output row is shifted by `buffer_above` and
/// clamped to the total resident grid height.
pub fn editor_cursor_grid_row(
    cursor_row: i32,
    source_line_offset: i32,
    visible_rows: u32,
    buffer_above: u32,
    buffer_below: u32,
) -> u32 {
    let raw = editor_cursor_output_row(cursor_row, source_line_offset)
        .saturating_add(buffer_above.min(i32::MAX as u32) as i32);
    let max = grid_total_row_count(visible_rows, buffer_above, buffer_below)
        .saturating_sub(1)
        .min(i32::MAX as u32) as i32;
    raw.clamp(0, max) as u32
}

/// Lower-level inverse of the scroll-offset row sampler.
pub fn editor_output_row_for_source(source_y: i32, source_line_offset: i32) -> i32 {
    source_y - source_line_offset
}

/// Source row that should be sampled for one visible editor output
/// row. Desktop's grid renderer applies the same relation when it
/// fills resident GPU rows: the source index is the output row plus
/// the integer scroll stride, while the fractional residual stays a
/// pixel uniform.
pub fn editor_source_row_for_output(output_row: i32, source_line_offset: i32) -> i32 {
    output_row + source_line_offset
}

/// Physical-pixel pane margin for the grid panel host. Mirrors
/// sugarloaf's `SugarloafLayout::margin.top/left/right/bottom` after
/// scale-factor application; we keep it as a POD here so policy code
/// doesn't depend on the sugarloaf layout type.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScaledMargin {
    pub top: f32,
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
}
const _: () = {
    // Keep ScaledMargin small/POD — only changes if we add fields.
    assert!(std::mem::size_of::<ScaledMargin>() == 16);
};

impl ScaledMargin {
    /// Construct a `ScaledMargin` from a `(top, right, bottom, left)`
    /// tuple — the canonical CSS-style ordering used by
    /// `neoism_backend::config::layout::Margin`. Lifted from
    /// `screen/render/mod.rs` where the same 4-field reorder was
    /// duplicated at every site that built the policy input.
    pub const fn from_trbl(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self {
            top,
            left,
            right,
            bottom,
        }
    }
}

/// POD layout slice for a grid panel: the on-screen panel rectangle in
/// physical pixels (`[x, y, w, h]`), the cell width/height already
/// rounded to whole pixels with a 1px floor, and the grid column count.
/// All three values match what the GPU cell pipeline uses, so policy
/// outputs sit on the same pixel lattice as the rendered glyphs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridPanelGeometry {
    pub panel_rect: [f32; 4],
    pub scaled_margin: ScaledMargin,
    pub cell_width: f32,
    pub cell_height: f32,
    pub columns: u32,
}

/// Editor-grid scroll state needed for cursor/cursorline target math.
/// Already-resolved (no Mutex inside) so policy stays lock-free.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorScrollState {
    pub scroll_position_lines: f32,
    pub elastic_offset_y: f32,
    pub previous_source_line_offset: Option<i32>,
}

/// Inputs to the terminal-grid trail-cursor planner.
///
/// `visible_rows` is the number of physically-visible rows the
/// terminal exposes (`terminal.screen_lines()` on native, falling back
/// to `dimension.lines`). It's a POD so the policy doesn't need to
/// acquire the terminal lock on the host side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailCursorPlanInput {
    pub geometry: GridPanelGeometry,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub visible_rows: f32,
    /// `Some(scroll)` when the active pane hosts an editor grid (the
    /// trail snaps through the spring-quantized scroll offset); `None`
    /// for a raw terminal pane (no editor scroll math).
    pub editor_scroll: Option<EditorScrollState>,
    /// Identifier of the cursor cell the previous frame's trail
    /// destination was set against. When `Some` and equal to the
    /// current `(rich_text_id, cursor_row, cursor_col)`, the host
    /// should call `set_destination_no_jump`; otherwise
    /// `set_destination`. `rich_text_id` is opaque to the policy.
    pub last_editor_trail_cursor_cell: Option<(usize, usize, usize)>,
    /// Active pane's `rich_text_id`, used to compose the cell key the
    /// host stores back as `last_editor_trail_cursor_cell`.
    pub rich_text_id: usize,
}

/// What the host should pass to `set_destination` /
/// `set_destination_no_jump` after the policy collapses pane bounds +
/// editor-scroll spring into a single physical-pixel destination.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailCursorDestination {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// `true` -> host should call `set_destination_no_jump` (this is
    /// the same cell as last frame, the move is scroll-driven so the
    /// corner ranking must not snap); `false` -> `set_destination`.
    pub no_jump: bool,
    /// Echo of the cell key the host should remember for next frame.
    /// `None` when the pane is not an editor grid (raw terminal panes
    /// don't track jump suppression).
    pub next_last_cell: Option<(usize, usize, usize)>,
}

/// Compute the trail cursor's destination for a terminal/editor grid
/// pane. Pure math — no Sugarloaf, no terminal lock. The host applies
/// the returned destination to its `TrailCursor` (or equivalent).
///
/// Mirrors the historical inline math in
/// `frontends/neoism/src/screen/render/mod.rs` for the
/// `TrailCursorOverlayTarget::TerminalGrid` branch, including the
/// `pane_top_phys` clamp that keeps a phantom cursor from flying into
/// the chrome band during a smooth-scroll spring.
pub fn terminal_grid_trail_cursor_destination(
    input: TrailCursorPlanInput,
) -> TrailCursorDestination {
    terminal_grid_trail_cursor_destination_inner(input, false)
}

fn terminal_grid_trail_cursor_destination_inner(
    input: TrailCursorPlanInput,
    mutated_snapshot: bool,
) -> TrailCursorDestination {
    let TrailCursorPlanInput {
        geometry,
        cursor_row,
        cursor_col,
        visible_rows,
        editor_scroll,
        last_editor_trail_cursor_cell,
        rich_text_id,
    } = input;

    let cell_width = geometry.cell_width;
    let cell_height = geometry.cell_height;
    let origin_x = geometry.panel_rect[0] + geometry.scaled_margin.left;
    let pane_top_phys = geometry.panel_rect[1] + geometry.scaled_margin.top;

    let cursor_px_x = origin_x + cursor_col as f32 * cell_width;
    let mut cursor_px_y = match editor_scroll {
        Some(scroll) => {
            let scroll_offset = if mutated_snapshot {
                editor_scroll_render_offset_for_mutated_snapshot(
                    scroll.scroll_position_lines,
                    scroll.elastic_offset_y,
                    cell_height,
                    scroll.previous_source_line_offset,
                )
            } else {
                editor_scroll_render_offset(
                    scroll.scroll_position_lines,
                    scroll.elastic_offset_y,
                    cell_height,
                    scroll.previous_source_line_offset,
                )
            };
            pane_top_phys
                + editor_cursor_output_row(
                    cursor_row as i32,
                    scroll_offset.source_line_offset,
                ) as f32
                    * cell_height
                + scroll_offset.pixel_offset_y
        }
        None => pane_top_phys + cursor_row as f32 * cell_height,
    };

    // Clamp the trail destination to the visible pane in physical
    // pixels. Without this the scroll spring's residual would push the
    // trail past the pane bottom into the chrome band as a phantom
    // cursor.
    let pane_bottom_phys = pane_top_phys + visible_rows * cell_height;
    let trail_top_min = pane_top_phys;
    let trail_top_max = (pane_bottom_phys - cell_height).max(pane_top_phys);
    cursor_px_y = cursor_px_y.clamp(trail_top_min, trail_top_max);

    let (no_jump, next_last_cell) = match editor_scroll {
        Some(_) => {
            let cell = (rich_text_id, cursor_row, cursor_col);
            (last_editor_trail_cursor_cell == Some(cell), Some(cell))
        }
        None => (false, None),
    };

    TrailCursorDestination {
        x: cursor_px_x,
        y: cursor_px_y,
        width: cell_width,
        height: cell_height,
        no_jump,
        next_last_cell,
    }
}
