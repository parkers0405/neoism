//! Painter-stream instances for vector shapes (`El::shape`).
use super::*;

/// Instances for a vector shape whose origin sits at `origin` (device px), with `k` device px per
/// coordinate unit on each axis and `k_px` device px per point for stroke widths.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_shape(out: &mut Vec<Instance>, shape: &Shape, origin: [f32; 2], k: [f32; 2], k_px: f32, opacity: f32, srgb: bool, clip: [f32; 4], fade: [f32; 4]) {
    let at = |p: [f32; 2]| [origin[0] + p[0] * k[0], origin[1] + p[1] * k[1]];
    for op in &shape.ops {
        match op {
            Op::Stroke { points, closed, width, cap, color } => {
                let fill = premul(*color, opacity, srgb);
                if fill[3] <= 0.0005 {
                    continue;
                }
                let hw = width * k_px * 0.5;
                let pad = hw.max(0.5) + 1.0;
                let n = points.len();
                let count = if *closed { n } else { n - 1 };
                for i in 0..count {
                    let (a, b) = (at(points[i]), at(points[(i + 1) % n]));
                    let (x0, y0) = (a[0].min(b[0]) - pad, a[1].min(b[1]) - pad);
                    out.push(Instance {
                        rect: [x0, y0, a[0].max(b[0]) + pad - x0, a[1].max(b[1]) + pad - y0],
                        radii: [a[0], a[1], b[0], b[1]],
                        fill,
                        border: [0.0; 4],
                        params: [hw, KIND_SEGMENT, if *cap == LineCap::Round { 1.0 } else { 0.0 }, 1.0],
                        uv: [0.0; 4],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            }
            Op::Fill { points, color } => {
                let fill = premul(*color, opacity, srgb);
                if fill[3] <= 0.0005 {
                    continue;
                }
                for ([ia, ib, ic], aa) in crate::shape::triangulate(points) {
                    let (a, b, c) = (at(points[ia]), at(points[ib]), at(points[ic]));
                    let (x0, y0) = (a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1]));
                    let (x1, y1) = (a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1]));
                    out.push(Instance {
                        rect: [x0, y0, x1 - x0, y1 - y0],
                        radii: [a[0], a[1], b[0], b[1]],
                        fill,
                        border: [0.0; 4],
                        params: [aa as f32, KIND_TRIANGLE, 0.0, 1.0],
                        uv: [c[0], c[1], 0.0, 0.0],
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            }
        }
    }
}
