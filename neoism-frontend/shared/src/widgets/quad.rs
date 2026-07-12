//! Clip-aware quad drawing shared by panels that paint rounded cards
//! inside scroll viewports (agent pane, markdown blocks, extensions
//! page, stock cards).

use sugarloaf::Sugarloaf;

use crate::primitives::geom::intersect_rect;

/// Draw `rect` as a rounded rect when (within `tolerance`) it is fully
/// visible inside `clip`; when partially clipped, draw the visible
/// slice as a sharp-cornered rect instead — sugarloaf's rounded quads
/// can't be scissored, and the eye doesn't catch the corner change at
/// chrome scale. Fully-outside rects draw nothing. `tolerance` is the
/// caller's historical "counts as fully visible" slack (sub-pixel
/// values behave like exact equality since `intersect_rect` returns
/// exact copies for contained rects).
#[allow(clippy::too_many_arguments)]
pub fn rounded_rect_clipped(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    id: Option<usize>,
    rect: [f32; 4],
    color: [f32; 4],
    depth: f32,
    radius: f32,
    order: u8,
    tolerance: f32,
) {
    let Some(visible) = intersect_rect(rect, clip) else {
        return;
    };
    let fully_visible = (visible[0] - rect[0]).abs() < tolerance
        && (visible[1] - rect[1]).abs() < tolerance
        && (visible[2] - rect[2]).abs() < tolerance
        && (visible[3] - rect[3]).abs() < tolerance;
    if fully_visible {
        let [x, y, w, h] = rect;
        sugarloaf.rounded_rect(id, x, y, w, h, color, depth, radius, order);
    } else {
        let [x, y, w, h] = visible;
        sugarloaf.rect(id, x, y, w, h, color, depth, order);
    }
}
