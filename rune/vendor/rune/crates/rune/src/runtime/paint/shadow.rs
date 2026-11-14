//! Painter-stream instances for an element's outer shadows (`El::box_shadow`).
use super::*;

/// The box casting the shadows and how they paint, in device pixels.
pub(super) struct Caster {
    pub rect: [f32; 4],
    /// Circular corner radii; shadows keep circular corners.
    pub radii: [f32; 4],
    /// The radii the box itself draws with (negative for continuous corners), for `bands`.
    pub shape: [f32; 4],
    /// Device pixels per point.
    pub k_px: f32,
    pub scale: f32,
    pub opacity: f32,
    pub srgb: bool,
    pub clip: [f32; 4],
    pub fade: [f32; 4],
    /// The radius of a hole the shadows leave over the box (negative for continuous corners).
    pub hole: Option<f32>,
    /// The box paints itself solid, hiding the middle of its shadows (bands.rs).
    pub opaque: bool,
}

/// One outer shadow `sh` in colour `c` (straight alpha) under `cast`.
pub(super) fn push_shadow(out: &mut Vec<Instance>, sh: &crate::el::Shadow, c: Color, cast: &Caster) {
    let [x, y, w, h] = cast.rect;
    let sp = sh.spread * cast.k_px;
    let rect = [x + sh.x * cast.k_px - sp, y + sh.y * cast.k_px - sp, w + 2.0 * sp, h + 2.0 * sp];
    let sradii = cast.radii.map(|v| if v > 0.0 { (v + sp).max(0.0) } else { 0.0 });
    let (clip, fade) = (cast.clip, crate::render::pack_fade(cast.fade, cast.clip, cast.clip));
    let fill = premul(c, cast.opacity, cast.srgb);
    let ring = sh.x == 0.0 && sh.y == 0.0;
    // A crisp offset copy over a hole draws as the sharpest blurred shadow, which can leave one.
    if sh.blur <= 0.0 && (ring || cast.hole.is_none()) {
        // A crisp shadow: a ring when it sits exactly around the box, else a solid
        // offset copy (hidden under an opaque element, as in CSS).
        if ring && sp <= 0.0 {
            return;
        }
        out.push(Instance {
            rect,
            radii: sradii,
            fill: if ring { [0.0; 4] } else { fill },
            border: if ring { fill } else { [0.0; 4] },
            params: [if ring { sp } else { 0.0 }, KIND_RECT, 0.0, 1.0],
            uv: [0.0; 4],
            clip,
            fade,
        });
        return;
    }
    // Over a native material, backdrop blur or dimmed box, the shadow leaves the interior clear:
    // `uv` holds the hole relative to the shadow's rect, `radii[3]` its radius.
    let (radii, uv) = match cast.hole {
        Some(hole) => ([sradii[0], sradii[1], sradii[2], hole], [x - rect[0], y - rect[1], w, h]),
        None => (sradii, [0.0; 4]),
    };
    let inst = Instance {
        rect,
        radii,
        fill,
        border: [0.0; 4],
        params: [0.0, KIND_SHADOW, (sh.blur * cast.scale * 0.5).max(0.5), 1.0],
        uv,
        clip,
        fade,
    };
    if cast.opaque {
        bands::push_shadow(out, inst, cast.rect, cast.shape);
    } else {
        out.push(inst);
    }
}
