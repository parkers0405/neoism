//! Clip-aware quad drawing shared by panels that paint rounded cards
//! inside scroll viewports (agent pane, markdown blocks, extensions
//! page, stock cards).

use sugarloaf::Sugarloaf;

use crate::primitives::geom::intersect_rect;

/// Clip the original rounded shape, rather than rounding or squaring its visible slice.
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
) {
    if intersect_rect(rect, clip).is_none() {
        return;
    }
    if let Some(id) = id {
        let [x, y, w, h] = rect;
        sugarloaf.rounded_rect(Some(id), x, y, w, h, color, depth, radius, order);
        let scale = sugarloaf.scale_factor();
        sugarloaf.set_bounds(id, Some(clip.map(|value| value * scale)));
    } else {
        sugarloaf.quad_clipped(rect, color, [radius; 4], depth, order, clip);
    }
}
