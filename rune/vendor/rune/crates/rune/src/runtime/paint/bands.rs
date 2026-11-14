//! A blurred shadow under an opaque box, and a border with nothing inside it, cover a quad that
//! shows nothing in its middle. Drawn whole, a window-sized panel's shadow shades every pixel of
//! the panel each frame. Past a size they draw as four bands around that middle instead: `uv`
//! gives each band's window relative to the rect, `[x0, y0, -x1, y1]` (a negative `uv.z` marks
//! it; shader.wgsl `base_vs`), and each band shades exactly what the whole quad did there.
use crate::render::{Instance, corner_extent};

/// The middle must be at least this many device pixels both ways for bands to pay.
const MIN_HOLE: f32 = 96.0;

/// Pushes the outer shadow `inst` of an opaque box at `caster` (device px, x y w h) with
/// corner `radii` (negative for continuous corners): the box hides the shadow inside it.
pub(super) fn push_shadow(out: &mut Vec<Instance>, inst: Instance, caster: [f32; 4], radii: [f32; 4]) {
    let e = radii.map(corner_extent).into_iter().fold(0.0, f32::max) + 1.0;
    let (x, y) = (caster[0] - inst.rect[0], caster[1] - inst.rect[1]);
    push(out, inst, [x + e, y + e, x + caster[2] - e, y + caster[3] - e], inst.params[2] * 3.0 + 1.0);
}

/// Pushes the border-only `inst` (its fill clear): nothing draws past its ring.
pub(super) fn push_ring(out: &mut Vec<Instance>, inst: Instance) {
    let e = inst.radii.map(corner_extent).into_iter().fold(0.0, f32::max) + inst.params[0] + inst.params[2].max(0.5) + 1.0;
    let [_, _, w, h] = inst.rect;
    push(out, inst, [e, e, w - e, h - e], 1.0 + inst.params[2] * 2.0);
}

/// `inst` whole, or as bands around `hole` (x0 y0 x1 y1 relative to its rect) when that's big
/// enough. `grow` is how far the shader grows the quad past the rect.
fn push(out: &mut Vec<Instance>, inst: Instance, hole: [f32; 4], grow: f32) {
    let [hx0, hy0, hx1, hy1] = hole;
    let [qx0, qy0, qx1, qy1] = [-grow, -grow, inst.rect[2] + grow, inst.rect[3] + grow];
    // The left band's right edge carries the mark as its sign, so it must be past the origin.
    if hx1 - hx0 < MIN_HOLE || hy1 - hy0 < MIN_HOLE || hx0 < 1.0 || hy0 <= qy0 || hx1 >= qx1 || hy1 >= qy1 || inst.uv != [0.0; 4] {
        out.push(inst);
        return;
    }
    for [x0, y0, x1, y1] in [[qx0, qy0, qx1, hy0], [qx0, hy0, hx0, hy1], [hx1, hy0, qx1, hy1], [qx0, hy1, qx1, qy1]] {
        out.push(Instance { uv: [x0, y0, -x1, y1], ..inst });
    }
}

#[cfg(test)]
#[path = "../bands_tests.rs"]
mod tests;
