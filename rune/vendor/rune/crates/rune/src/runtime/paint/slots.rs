//! Paint records that keep a place in the painter stream for pixels Rune doesn't paint itself:
//! effect slots a compositor fills (`El::effect`) and backdrop blurs (`El::backdrop_blur`).
use super::intersect;
use crate::render::{Instance, KIND_BACKDROP_BLUR, KIND_EFFECT, LayerSpec, NO_CLIP};

/// Pushes the effect slot `effect` tagged `mark` over `rect` with corner `radii`, clipped by
/// `clip` and the packed `fade`, at `k_px` device pixels per point and paint opacity `o`.
/// A compositor paints whatever it likes at a slot, so a faded slot gets its own layer that
/// applies the opacity, instead of a factor it could forget. Returns the index of that slot.
pub(super) fn push_effect(instances: &mut Vec<Instance>, (effect, mark): (crate::el::effect::EffectSlot, u32),
    (rect, radii, clip, fade): ([f32; 4], [f32; 4], [f32; 4], [u32; 4]), k_px: f32, o: f32) -> Option<usize> {
    let slot = Instance {
        rect,
        radii,
        fill: [0.0, 0.0, 0.0, 1.0],
        border: [(mark & 0xFFFF) as f32, (mark >> 16) as f32, 0.0, 0.0],
        params: [if effect.backdrop { 1.0 } else { 0.0 }, KIND_EFFECT, effect.bleed * k_px, k_px],
        uv: effect.data,
        clip,
        fade,
    };
    if o >= 1.0 {
        instances.push(slot);
        return None;
    }
    let mut marker = Instance::begin_layer(rect, clip, LayerSpec { opacity: o, ..Default::default() });
    marker.fade = fade;
    instances.push(marker);
    instances.push(Instance { clip: NO_CLIP, ..slot });
    instances.push(Instance::end_layer());
    Some(instances.len() - 2)
}

/// Pushes a backdrop blur record (`El::backdrop_blur`) of `sigma` points over `rect` with corner
/// `radii` (continuous when `continuous`), cut by `clip`, the soft edges `fade` from
/// `fade_origin` and an ancestor's rounded clip `rounded`, at `k_px` device pixels per point and
/// opacity `o`. Returns the signed corner radius an outer shadow leaves clear over the blur.
pub(super) fn push_backdrop_blur(instances: &mut Vec<Instance>, sigma: f32, (rect, radii, continuous): ([f32; 4], [f32; 4], bool),
    (clip, fade, fade_origin): ([f32; 4], [f32; 4], [f32; 4]), rounded: Option<([f32; 4], [f32; 4])>, k_px: f32, o: f32) -> f32 {
    // One clip rectangle fits the packed ABI: a rounded clip cut by `clip` keeps only the cut.
    let (clip, round) = match rounded {
        Some((mask, radii)) if intersect(clip, mask) == mask => (mask, Some(radii)),
        Some((mask, _)) => (intersect(clip, mask), None),
        None => (clip, None),
    };
    let mut record = Instance {
        rect,
        radii: if continuous { radii.map(|v| -v) } else { radii },
        fill: [0.0, 0.0, 0.0, o],
        border: [0.0; 4],
        params: [sigma * k_px, KIND_BACKDROP_BLUR, 0.0, k_px],
        uv: [0.0; 4],
        clip,
        fade: crate::render::pack_fade(fade, fade_origin, clip),
    };
    if let Some(radii) = round { record.round_clip_corners(radii); }
    instances.push(record);
    let radius = crate::system_material::clamp_radius(radii, rect[2], rect[3]);
    if continuous { -radius } else { radius }
}
