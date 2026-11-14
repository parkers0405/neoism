//! Composed `El::rotate` affine transforms (`Turn`), turned layer openings and clip bounds.
use super::*;

/// A 2×3 affine in device pixels, `[a, b, c, d, tx, ty]`: `(x, y)` lands at
/// `(a·x + b·y + tx, c·x + d·y + ty)`. The composed `El::rotate` layers above a node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Turn {
    pub(super) mat: [f32; 6],
    pub(super) rect: [f32; 4],
    pub(super) angle: f32,
    pub(super) pivot: [f32; 2],
    pub(super) outer_clip: [f32; 4],
}

/// The composed turn for a node under `Inh::turn` index `inherited` turning `own` radians about
/// `pivot`: pushed onto `turns` when it turns itself. Off the paint loop's common path.
#[cold]
#[inline(never)]
pub(super) fn compose_turn(
    turns: &mut Vec<Turn>,
    inherited: u32,
    own: Option<f32>,
    (rect, pivot, outer_clip): ([f32; 4], [f32; 2], [f32; 4]),
) -> (Option<Turn>, u32) {
    let above = inherited.checked_sub(1).map(|k| turns[k as usize]);
    match own {
        None => (above, inherited),
        Some(a) => {
            let own = Turn::about(a, pivot, rect, outer_clip);
            let t = above.map_or(own, |t| t.after(own));
            turns.push(t);
            (Some(t), turns.len() as u32)
        }
    }
}

/// Open the layer a turned node draws into, upright, at `rect` (x, y, w, h in device pixels).
/// The layer takes the clips from above, so the node's own clips start fresh inside it; a
/// rectangular cut through the inherited rounded mask gets an outer scope of its own.
#[cold]
#[inline(never)]
pub(super) fn open_turned_layer(instances: &mut Vec<Instance>, layers: &mut Vec<(usize, usize, usize)>, (i, surfaces): (usize, usize),
    rect: [f32; 4], (angle, pivot): (f32, [f32; 2]),
    (clip, fade, fade_origin, rounded): (&mut [f32; 4], &mut [f32; 4], &mut [f32; 4], &mut Option<([f32; 4], [f32; 4])>))
{
    if let Some((mask, _)) = *rounded && intersect(*clip, mask) != mask {
        let mut marker = Instance::begin_layer(rect, *clip, Default::default());
        marker.fade = crate::render::pack_fade(*fade, *fade_origin, *clip);
        layers.push((i, instances.len(), surfaces));
        instances.push(marker);
        (*clip, *fade, *fade_origin) = (NO_CLIP, [0.0; 4], NO_CLIP);
    }
    if let Some((mask, _)) = *rounded { *clip = intersect(*clip, mask); }
    let mut marker = Instance::begin_layer(rect, *clip, Default::default()).turned(angle, pivot);
    marker.fade = crate::render::pack_fade(*fade, *fade_origin, *clip);
    if let Some((_, radii)) = *rounded { marker.round_clip_corners(radii); }
    layers.push((i, instances.len(), surfaces));
    instances.push(marker);
    (*clip, *fade, *fade_origin, *rounded) = (NO_CLIP, [0.0; 4], NO_CLIP, None);
}

/// The overlap of two `[x0, y0, x1, y1]` rects (inverted when they don't meet).
pub(crate) fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

pub(super) fn screen_clip_of(parent: Option<usize>, inh: &[Inh], turns: &[Turn]) -> [f32; 4] {
    let Some(pi) = parent else { return NO_CLIP };
    let Some(h) = inh.get(pi) else { return NO_CLIP };
    if let Some(&t) = h.turn.checked_sub(1).and_then(|k| turns.get(k as usize)) {
        let ic = if h.clip == NO_CLIP { NO_CLIP } else { t.bbox(h.clip) };
        let oc = t.outer_clip;
        intersect(oc, ic)
    } else {
        h.clip
    }
}

impl Turn {
    /// `angle` radians clockwise about `p`.
    pub(super) fn about(angle: f32, p: [f32; 2], rect: [f32; 4], outer_clip: [f32; 4]) -> Turn {
        let (sn, c) = angle.sin_cos();
        Turn {
            mat: [c, -sn, sn, c, p[0] - c * p[0] + sn * p[1], p[1] - sn * p[0] - c * p[1]],
            rect,
            angle,
            pivot: p,
            outer_clip,
        }
    }

    /// `inner`, then `self`.
    pub(super) fn after(self, inner: Turn) -> Turn {
        let [a, b, c, d, x, y] = self.mat;
        let [e, f, g, h, u, v] = inner.mat;
        let ic = if inner.outer_clip == NO_CLIP { NO_CLIP } else { self.bbox(inner.outer_clip) };
        let oc = self.outer_clip;
        Turn {
            mat: [a * e + b * g, a * f + b * h, c * e + d * g, c * f + d * h, a * u + b * v + x, c * u + d * v + y],
            outer_clip: intersect(oc, ic),
            ..inner
        }
    }

    /// The upright bounding box of edges `r` once turned.
    pub(super) fn bbox(self, r: [f32; 4]) -> [f32; 4] {
        let [a, b, c, d, x, y] = self.mat;
        let mut out = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for (px, py) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
            let (qx, qy) = (a * px + b * py + x, c * px + d * py + y);
            out = [out[0].min(qx), out[1].min(qy), out[2].max(qx), out[3].max(qy)];
        }
        out
    }

    /// Unturns a screen-space point `q` back into the layer's upright device pixels.
    pub(super) fn unturn(self, [qx, qy]: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d, tx, ty] = self.mat;
        let (dx, dy) = (qx - tx, qy - ty);
        [a * dx + c * dy, b * dx + d * dy]
    }
}
