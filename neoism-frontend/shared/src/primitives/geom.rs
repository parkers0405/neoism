//! Pure geometry helpers shared across chrome panels.
//!
//! Every helper is a pure free function with no panel-specific deps —
//! state stays in the panel.

#[inline]
pub fn snap_to_device_px(value: f32, scale_factor: f32) -> f32 {
    if scale_factor <= 0.0 {
        value
    } else {
        (value * scale_factor).round() / scale_factor
    }
}

pub fn rects_intersect(a: [f32; 4], b: [f32; 4]) -> bool {
    let (ax1, ay1, ax2, ay2) = (a[0], a[1], a[0] + a[2], a[1] + a[3]);
    let (bx1, by1, bx2, by2) = (b[0], b[1], b[0] + b[2], b[1] + b[3]);
    ax1 < bx2 && ax2 > bx1 && ay1 < by2 && ay2 > by1
}

#[allow(dead_code)]
pub fn intersect_rect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let left = a[0].max(b[0]);
    let top = a[1].max(b[1]);
    let right = (a[0] + a[2]).min(b[0] + b[2]);
    let bottom = (a[1] + a[3]).min(b[1] + b[3]);
    (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
}

/// Visible pieces of a hit region after viewport clipping and overlay occlusion.
pub fn visible_rect_fragments(
    rect: [f32; 4],
    clip: [f32; 4],
    occlusions: &[[f32; 4]],
) -> Vec<[f32; 4]> {
    let valid = |rect: &[f32; 4]| {
        rect.iter().all(|v| v.is_finite())
            && rect[2] > 0.0
            && rect[3] > 0.0
            && (rect[0] + rect[2]).is_finite()
            && (rect[1] + rect[3]).is_finite()
    };
    if !valid(&rect) || !valid(&clip) {
        return Vec::new();
    }
    let Some(rect) = intersect_rect(rect, clip) else {
        return Vec::new();
    };
    let mut visible = vec![rect];
    for cut in occlusions.iter().filter(|cut| valid(cut)) {
        let mut next = Vec::new();
        for rect in visible {
            let Some(overlap) = intersect_rect(rect, *cut) else {
                next.push(rect);
                continue;
            };
            let [x, y, w, h] = rect;
            let [cx, cy, cw, ch] = overlap;
            for piece in [
                [x, y, w, cy - y],
                [x, cy + ch, w, y + h - cy - ch],
                [x, cy, cx - x, ch],
                [cx + cw, cy, x + w - cx - cw, ch],
            ] {
                if piece[2] > 0.0 && piece[3] > 0.0 {
                    next.push(piece);
                }
            }
        }
        visible = next;
        if visible.is_empty() {
            break;
        }
    }
    visible
}

pub fn edge_row_radii(
    y: f32,
    h: f32,
    clip_top: f32,
    clip_bottom: f32,
    radius: f32,
) -> [f32; 4] {
    let top = if y <= clip_top + 0.5 { radius } else { 0.0 };
    let bottom = if y + h >= clip_bottom - 0.5 {
        radius
    } else {
        0.0
    };
    [top, top, bottom, bottom]
}

pub fn edge_left_row_radii(
    y: f32,
    h: f32,
    clip_top: f32,
    clip_bottom: f32,
    radius: f32,
) -> [f32; 4] {
    let top = if y <= clip_top + 0.5 { radius } else { 0.0 };
    let bottom = if y + h >= clip_bottom - 0.5 {
        radius
    } else {
        0.0
    };
    [top, 0.0, 0.0, bottom]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_hit_regions_are_clipped_and_do_not_cover_overlays() {
        let clip = [5.0, 5.0, 10.0, 10.0];
        let cut = [8.0, 8.0, 4.0, 4.0];
        let pieces = visible_rect_fragments([0.0, 0.0, 20.0, 20.0], clip, &[cut]);
        assert_eq!(pieces.len(), 4);
        assert_eq!(
            pieces.iter().map(|rect| rect[2] * rect[3]).sum::<f32>(),
            84.0
        );
        for piece in pieces {
            assert_eq!(intersect_rect(piece, clip), Some(piece));
            assert!(!rects_intersect(piece, cut));
        }
    }

    #[test]
    fn invisible_or_invalid_hit_regions_cannot_receive_clicks() {
        let rect = [0.0, 0.0, 20.0, 20.0];
        assert!(visible_rect_fragments(rect, rect, &[rect]).is_empty());
        assert!(visible_rect_fragments(rect, [30.0, 30.0, 5.0, 5.0], &[]).is_empty());
        assert!(
            visible_rect_fragments([f32::NAN, 0.0, 10.0, 10.0], rect, &[]).is_empty()
        );
    }
}
