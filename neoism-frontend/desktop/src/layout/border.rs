use neoism_backend::config::layout::Margin;
use neoism_backend::sugarloaf::{layout::TextDimensions, Object, Rect};
use neoism_ui::session_layout::tree::SessionTreeLeafId;

pub const MIN_COLS: usize = 2;
pub const MIN_LINES: usize = 1;

/// Direction of a draggable panel border
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderDirection {
    /// Border between left/right panels (drag horizontally)
    Vertical,
    /// Border between top/bottom panels (drag vertically)
    Horizontal,
}

/// Describes a draggable divider between two sibling subtrees in the
/// canonical `SessionTree`. Identified by `anchor_leaf` (the first leaf
/// of the left/top child at this gap), which re-resolves to the split
/// node + gap index at resize time. Carries the split's main-axis extent
/// and the current cumulative ratio so a drag maps pixels → ratio without
/// touching Taffy. All fields are `Copy`.
#[derive(Debug, Clone, Copy)]
pub struct PanelBorder {
    pub direction: BorderDirection,
    /// First leaf of the child immediately before this gap.
    pub anchor_leaf: SessionTreeLeafId,
    /// Main-axis pixel extent of the owning split node.
    pub node_extent: f32,
    /// Cumulative split ratio at this gap when the drag began.
    pub start_ratio: f32,
}

pub(crate) fn compute(
    width: f32,
    height: f32,
    dimensions: TextDimensions,
    line_height: f32,
    margin: Margin,
) -> (usize, usize) {
    // Ensure we have positive dimensions
    if width <= 0.0 || height <= 0.0 || dimensions.scale <= 0.0 || line_height <= 0.0 {
        return (MIN_COLS, MIN_LINES);
    }

    // Calculate available space accounting for margins (scale margins to physical pixels)
    let scale = dimensions.scale;
    let available_width = width - (margin.left * scale) - (margin.right * scale);
    let available_height = height - (margin.top * scale) - (margin.bottom * scale);

    // Ensure we have positive available space
    if available_width <= 0.0 || available_height <= 0.0 {
        return (MIN_COLS, MIN_LINES);
    }

    // Calculate columns - divide by the ROUNDED cell width.
    // rounds `face_width` once in `font/Metrics.zig:265` (`cell_width =
    // @round(face_width)`) and uses that integer everywhere — cols,
    // grid shader, cursor hit-testing. Rio's grid renderer already
    // does `.round()` on `cell_w` when building `GridUniforms`, so the
    // column count has to use the same integer or the right edge of
    // the grid floats `cols * (face_width - cell_width)` pixels short
    // of the panel. Matches ; sacrifices at most 1 col vs
    // fractional divide but keeps the render perfectly aligned.
    let cell_width = dimensions.width.round().max(1.0);
    let visible_columns =
        std::cmp::max((available_width / cell_width) as usize, MIN_COLS);

    // Same treatment for rows: grid renders at `.round()`ed cell
    // height, so cols-and-rows share the same integer snap. Use
    // `.floor()` so terminal panes get the conservative row count
    // (every row fully visible — the shell prints to all `lines`
    // rows, so a partial row at the bottom would cut off the last
    // line of output). Editor (nvim) contexts keep this conservative
    // count, then distribute the fractional remainder across those
    // complete rows once tabs, breadcrumbs, splits, and status geometry
    // are known.
    let cell_height = dimensions.height.round().max(1.0);
    let lines = (available_height / cell_height).floor();
    let visible_lines = std::cmp::max(lines as usize, MIN_LINES);

    (visible_columns, visible_lines)
}

#[inline]
pub(crate) fn create_border(
    color: [f32; 4],
    position: [f32; 2],
    size: [f32; 2],
) -> Object {
    Object::Rect(Rect::new(position[0], position[1], size[0], size[1], color))
}

/// Separator configuration for split panels
#[derive(Debug, Clone, Copy)]
pub struct BorderConfig {
    pub width: f32,
    pub color: [f32; 4],
}

impl Default for BorderConfig {
    fn default() -> Self {
        Self {
            width: 2.0,
            color: [0.8, 0.8, 0.8, 1.0],
        }
    }
}
