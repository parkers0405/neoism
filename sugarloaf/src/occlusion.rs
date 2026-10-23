//! CPU-side inverse clips shared by immediate-mode output recorders.
//! Coordinates here are physical pixels; the Sugarloaf scope boundary scales once.
pub(crate) type Rect = [f32; 4];

pub(crate) fn valid(r: Rect) -> bool {
    r.iter().all(|v| v.is_finite())
        && r[2] > 0.0
        && r[3] > 0.0
        && (r[0] + r[2]).is_finite()
        && (r[1] + r[3]).is_finite()
}

pub(crate) fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    let r = [
        x,
        y,
        (a[0] + a[2]).min(b[0] + b[2]) - x,
        (a[1] + a[3]).min(b[1] + b[3]) - y,
    ];
    valid(r).then_some(r)
}

/// `None` means no hole intersects: retain the instance verbatim, including its
/// disabled clip sentinel. Otherwise return disjoint visible clips, possibly empty.
pub(crate) fn visible(bounds: Rect, clip: Rect, holes: &[Rect]) -> Option<Vec<Rect>> {
    if !valid(bounds) {
        return Some(Vec::new());
    }
    let bounds = if clip[2] > 0.0 && clip[3] > 0.0 {
        match intersection(bounds, clip) {
            Some(r) => r,
            None => return Some(Vec::new()),
        }
    } else {
        bounds
    };
    if !holes.iter().any(|h| intersection(bounds, *h).is_some()) {
        return None;
    }
    let mut fragments = vec![bounds];
    let mut next = Vec::new();
    for hole in holes {
        if intersection(bounds, *hole).is_none() {
            continue;
        }
        next.clear();
        for r in fragments.drain(..) {
            let Some(i) = intersection(r, *hole) else {
                next.push(r);
                continue;
            };
            // Top/bottom span the full width; left/right only span the intersection.
            for part in [
                [r[0], r[1], r[2], i[1] - r[1]],
                [r[0], i[1] + i[3], r[2], r[1] + r[3] - i[1] - i[3]],
                [r[0], i[1], i[0] - r[0], i[3]],
                [i[0] + i[2], i[1], r[0] + r[2] - i[0] - i[2], i[3]],
            ] {
                if valid(part) {
                    next.push(part);
                }
            }
        }
        std::mem::swap(&mut fragments, &mut next);
        if fragments.is_empty() {
            break;
        }
    }
    Some(fragments)
}

/// Split only appended output; earlier content and all original geometry survive.
pub(crate) fn filter_tail<T: Clone>(
    items: &mut Vec<T>,
    first: usize,
    mut split: impl FnMut(&T) -> Option<Vec<T>>,
) {
    let first = first.min(items.len());
    let tail = items.split_off(first);
    for item in tail {
        match split(&item) {
            Some(parts) => items.extend(parts),
            None => items.push(item),
        }
    }
}

pub(crate) fn image_parts(
    image: &crate::sugarloaf::graphics::GraphicOverlay,
    holes: &[Rect],
) -> Option<Vec<crate::sugarloaf::graphics::GraphicOverlay>> {
    let bounds = [image.x, image.y, image.width, image.height];
    visible(bounds, [0.0; 4], holes).map(|clips| {
        clips
            .into_iter()
            .map(|r| {
                let mut part = image.clone();
                let uv = image.source_rect;
                part.source_rect = [
                    uv[0] + (uv[2] - uv[0]) * (r[0] - image.x) / image.width,
                    uv[1] + (uv[3] - uv[1]) * (r[1] - image.y) / image.height,
                    uv[0] + (uv[2] - uv[0]) * (r[0] + r[2] - image.x) / image.width,
                    uv[1] + (uv[3] - uv[1]) * (r[1] + r[3] - image.y) / image.height,
                ];
                part.x = r[0];
                part.y = r[1];
                part.width = r[2];
                part.height = r[3];
                part
            })
            .collect()
    })
}

pub(crate) fn glyph_parts(
    instance: &crate::text::TextInstance,
    holes: &[Rect],
) -> Option<Vec<crate::text::TextInstance>> {
    let scale = if instance.raster_scale > 0.0 {
        instance.raster_scale
    } else {
        1.0
    };
    let bounds = [
        instance.pos[0] + f32::from(instance.bearings[0]) * scale,
        instance.pos[1] + f32::from(instance.bearings[1]) * scale,
        instance.glyph_size[0] as f32 * scale,
        instance.glyph_size[1] as f32 * scale,
    ];
    visible(bounds, instance.clip_rect, holes).map(|clips| {
        clips
            .into_iter()
            .map(|clip| {
                let mut part = *instance;
                part.clip_rect = clip;
                part
            })
            .collect()
    })
}

#[cfg(test)]
mod tests;
