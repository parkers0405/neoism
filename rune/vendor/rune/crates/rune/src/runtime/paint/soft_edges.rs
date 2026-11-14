//! Soft clip edges: how a fade inherits down the tree, and whether a rect reaches into one.

// Carry the declaring edge with the inherited width. Descendant clipping must not restart
// its ramp; an inner fade takes over once the ancestor's ramp is fully opaque there. Widths
// compare by size: a negative one is a linear ramp (`El::clip_fade_linear`).
pub(super) fn inherit_fade(width: &mut [f32; 4], origin: &mut [f32; 4], rect: [f32; 4], own: [f32; 4]) {
    for edge in 0..4 {
        let inset = if edge < 2 { rect[edge] - origin[edge] } else { origin[edge] - rect[edge] };
        let (o, w) = (own[edge].abs(), width[edge].abs());
        if o > 0.0 && (o >= w || inset >= w) {
            width[edge] = own[edge];
            origin[edge] = rect[edge];
        }
    }
}

/// Whether `r` (x0, y0, x1, y1) reaches into a soft edge of `clip`: one fading over `width`
/// from `origin`, edge by edge (left, top, right, bottom).
pub(super) fn in_fade(r: [f32; 4], clip: [f32; 4], width: [f32; 4], origin: [f32; 4]) -> bool {
    let width = width.map(f32::abs);
    (width[0] > 0.0 && r[0] < origin[0] + width[0] && r[2] > clip[0])
        || (width[1] > 0.0 && r[1] < origin[1] + width[1] && r[3] > clip[1])
        || (width[2] > 0.0 && r[2] > origin[2] - width[2] && r[0] < clip[2])
        || (width[3] > 0.0 && r[3] > origin[3] - width[3] && r[1] < clip[3])
}
