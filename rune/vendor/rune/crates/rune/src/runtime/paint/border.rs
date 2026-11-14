//! Painter-stream instances for what CSS layers between an element's background and its border:
//! the fill drawn on its own under the ring, and inset shadows inside the border
//! (`El::border`, `El::box_shadow`).
use super::*;

/// `rect` and its signed `radii` brought in by `by` device pixels on each side, `[top, right,
/// bottom, left]`; each corner's radius shrinks by the wider of its two sides, as the inner
/// edge of a CSS border does.
pub(super) fn inset_box(rect: [f32; 4], radii: [f32; 4], by: [f32; 4]) -> ([f32; 4], [f32; 4]) {
    let [t, r, b, l] = by;
    let cut = [t.max(l), t.max(r), b.max(r), b.max(l)];
    (
        [rect[0] + l, rect[1] + t, (rect[2] - l - r).max(0.0), (rect[3] - t - b).max(0.0)],
        std::array::from_fn(|k| (radii[k].abs() - cut[k]).max(0.0).copysign(radii[k])),
    )
}

/// The fill of the bordered box `boxed` (its `ring` instance with the fill set) drawn without
/// its ring, which then draws over what goes between. With `shrink`, an opaque border all round
/// a rounded box pulls it in by half the border's width, as the shader does for a fill and ring
/// drawn together (shader.wgsl `border_over`), and as Chromium's bleed avoidance does.
pub(super) fn fill_under(boxed: Instance, shrink: bool) -> Instance {
    let [bw, kind, soft, o] = boxed.params;
    let opaque = |c: [f32; 4]| c[3] / o.clamp(1e-6, 1.0) > 0.999;
    let uniform = kind == KIND_GRADIENT || boxed.uv == [0.0; 4];
    let hides = opaque(boxed.border) && (kind != KIND_GRADIENT || opaque(boxed.uv));
    let (rect, radii) = if shrink && uniform && hides && boxed.radii != [0.0; 4] {
        inset_box(boxed.rect, boxed.radii, [bw * 0.5; 4])
    } else {
        (boxed.rect, boxed.radii)
    };
    Instance { rect, radii, border: [0.0; 4], params: [0.0, KIND_RECT, soft, o], uv: [0.0; 4], ..boxed }
}

/// The inset shadow `sh` in premultiplied `fill`, inside the box `inner` (the padding box, so
/// inside any border). `scale` is device pixels per point and the window's scale.
pub(super) fn inset_shadow(sh: &crate::el::Shadow, fill: [f32; 4], inner: ([f32; 4], [f32; 4]), scale: (f32, f32), clip: [f32; 4], fade: [f32; 4]) -> Instance {
    let (k_px, s) = scale;
    Instance {
        rect: inner.0,
        radii: inner.1,
        fill,
        border: [0.0; 4],
        params: [sh.spread * k_px, KIND_INSET, if sh.blur > 0.0 { (sh.blur * s * 0.5).max(0.5) } else { 0.0 }, 1.0],
        uv: [sh.x * k_px, sh.y * k_px, 0.0, 0.0],
        clip,
        fade: crate::render::pack_fade(fade, clip, clip),
    }
}
