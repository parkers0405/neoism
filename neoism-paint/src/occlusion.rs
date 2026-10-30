//! CPU-side inverse clips shared by immediate-mode output recorders.
//! Coordinates here are physical pixels; the host scope boundary scales once.
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

/// Precompute the broad-phase bounds once per scope, not once per glyph.
pub(crate) struct Occlusions<'a> {
    holes: &'a [Rect],
    bounds: Option<Rect>,
}

impl<'a> Occlusions<'a> {
    pub(crate) fn new(holes: &'a [Rect]) -> Self {
        let bounds = holes.iter().copied().filter(|r| valid(*r)).reduce(|a, b| {
            let x = a[0].min(b[0]);
            let y = a[1].min(b[1]);
            [
                x,
                y,
                (a[0] + a[2]).max(b[0] + b[2]) - x,
                (a[1] + a[3]).max(b[1] + b[3]) - y,
            ]
        });
        Self { holes, bounds }
    }

    /// `None` retains the instance verbatim, including its disabled clip sentinel.
    pub(crate) fn visible(&self, bounds: Rect, clip: Rect) -> Option<Vec<Rect>> {
        if !valid(bounds) {
            return Some(Vec::new());
        }
        let effective = if clip[2] > 0.0 && clip[3] > 0.0 {
            match intersection(bounds, clip) {
                Some(rect) => rect,
                None => return Some(Vec::new()),
            }
        } else {
            bounds
        };
        // Almost all transcript glyphs are above the floating composer.
        if self
            .bounds
            .is_none_or(|hole| valid(hole) && intersection(effective, hole).is_none())
        {
            return None;
        }
        visible_intersecting(bounds, clip, self.holes)
    }
}

#[cfg(test)]
pub(crate) fn visible(bounds: Rect, clip: Rect, holes: &[Rect]) -> Option<Vec<Rect>> {
    Occlusions::new(holes).visible(bounds, clip)
}

fn visible_intersecting(bounds: Rect, clip: Rect, holes: &[Rect]) -> Option<Vec<Rect>> {
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

/// Compact removals in place. Only actual split primitives need extra storage;
/// unchanged output keeps its allocation and is neither cloned nor copied aside.
pub(crate) fn filter_tail<T>(
    items: &mut Vec<T>,
    first: usize,
    mut split: impl FnMut(&T) -> Option<Vec<T>>,
) {
    let mut source_index = 0;
    let mut output_index = 0;
    let mut expansions = Vec::new();
    items.retain_mut(|item| {
        let untouched = source_index < first;
        source_index += 1;
        if untouched {
            output_index += 1;
            return true;
        }
        if let Some(mut parts) = split(item) {
            if parts.is_empty() {
                return false;
            }
            *item = parts.remove(0);
            if !parts.is_empty() {
                expansions.push((output_index + 1, parts));
            }
        }
        output_index += 1;
        true
    });
    // Indices describe the compacted output, before any extra pieces are inserted.
    for (index, parts) in expansions.into_iter().rev() {
        items.splice(index..index, parts);
    }
}

#[cfg(test)]
pub(crate) fn image_parts(
    image: &crate::graphics::GraphicOverlay,
    holes: &[Rect],
) -> Option<Vec<crate::graphics::GraphicOverlay>> {
    image_parts_with(image, &Occlusions::new(holes))
}

pub(crate) fn image_parts_with(
    image: &crate::graphics::GraphicOverlay,
    holes: &Occlusions<'_>,
) -> Option<Vec<crate::graphics::GraphicOverlay>> {
    let bounds = [image.x, image.y, image.width, image.height];
    holes.visible(bounds, [0.0; 4]).map(|clips| {
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

#[cfg(test)]
pub(crate) fn glyph_parts(
    instance: &crate::text::TextInstance,
    holes: &[Rect],
) -> Option<Vec<crate::text::TextInstance>> {
    glyph_parts_with(instance, &Occlusions::new(holes))
}

pub(crate) fn glyph_parts_with(
    instance: &crate::text::TextInstance,
    holes: &Occlusions<'_>,
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
    holes.visible(bounds, instance.clip_rect).map(|clips| {
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
