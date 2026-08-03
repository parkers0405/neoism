//! Text / wrap / occluded-draw helpers shared across chrome panels.

use sugarloaf::Sugarloaf;
use sugarloaf::text::DrawOpts;

use super::geom::rects_intersect;

/// Truncate `text` so its shaped width fits inside `available_w` pixels,
/// adding an ellipsis when we cut. Uses Sugarloaf's actual shaping so
/// long single words and fallback-font glyphs don't spill past the
/// container's right edge.
pub fn truncate_to_fit(
    text: &str,
    available_w: f32,
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> String {
    if available_w <= 0.0 || text.is_empty() {
        return String::new();
    }
    if sugarloaf.text_mut().measure(text, opts) <= available_w {
        return text.to_string();
    }
    if sugarloaf.text_mut().measure("…", opts) >= available_w {
        return "…".to_string();
    }

    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let mut candidate: String = chars[..mid].iter().collect();
        candidate.push('…');
        if sugarloaf.text_mut().measure(&candidate, opts) <= available_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let mut out: String = chars[..lo].iter().collect();
    out.push('…');
    out
}

/// Draw `text` at `(x, y)` with `opts`, but punch holes anywhere
/// `occlusion_rects` overlap the text's bounding rect. Returns measured
/// width.
pub fn draw_text_with_occlusion(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    if occlusion_rects.is_empty() {
        return sugarloaf.text_mut().draw(x, y, text, opts);
    }

    let width = sugarloaf.text_mut().measure(text, opts);
    if width <= 0.0 {
        return 0.0;
    }

    let Some(base_clip) = opts.clip_rect else {
        return sugarloaf.text_mut().draw(x, y, text, opts);
    };
    let text_h = (opts.font_size * 1.8).max(opts.font_size + 8.0);
    let text_rect = [x, y - 4.0, width, text_h];
    let mut intervals = vec![(base_clip[0], base_clip[0] + base_clip[2])];

    for rect in occlusion_rects {
        if !rects_intersect(text_rect, *rect) {
            continue;
        }
        let cut_start = rect[0].max(base_clip[0]);
        let cut_end = (rect[0] + rect[2]).min(base_clip[0] + base_clip[2]);
        if cut_end <= cut_start {
            continue;
        }

        let mut next = Vec::with_capacity(intervals.len() + 1);
        for (start, end) in intervals {
            if cut_end <= start || cut_start >= end {
                next.push((start, end));
                continue;
            }
            if cut_start > start {
                next.push((start, cut_start));
            }
            if cut_end < end {
                next.push((cut_end, end));
            }
        }
        intervals = next;
        if intervals.is_empty() {
            return width;
        }
    }

    for (start, end) in intervals {
        let clip_w = end - start;
        if clip_w <= 0.0 {
            continue;
        }
        let mut clipped = *opts;
        clipped.clip_rect = Some([start, base_clip[1], clip_w, base_clip[3]]);
        sugarloaf.text_mut().draw(x, y, text, &clipped);
    }

    width
}

/// Draw an icon glyph and center its rasterized ink vertically inside
/// `rect`; optionally center it horizontally as well.
///
/// Unlike labels, icon fonts should not be aligned by their em box or text
/// advance: Nerd Font and fallback glyphs often have asymmetric bearings.
/// Recording the emitted instances and centering their real bitmap bounds
/// keeps toolbar, tab, and tree icons symmetric for every loaded family.
pub fn draw_icon_centered_with_occlusion(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    rect: [f32; 4],
    icon: &str,
    opts: &DrawOpts,
    occlusion_rects: &[[f32; 4]],
    center_x: bool,
) -> f32 {
    let first_instance = sugarloaf.text_mut().instances().len();
    let width =
        draw_text_with_occlusion(sugarloaf, x, rect[1], icon, opts, occlusion_rects);
    sugarloaf
        .text_mut()
        .center_instances_in_rect(first_instance, rect, center_x, true);
    width
}

/// Overlay-pass counterpart to [`draw_icon_centered_with_occlusion`].
/// Modals and popovers use Sugarloaf's overlay text buffer, so their glyph
/// instances must be measured and shifted in that same buffer.
pub fn draw_overlay_icon_centered(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    rect: [f32; 4],
    icon: &str,
    opts: &DrawOpts,
    center_x: bool,
) -> f32 {
    let first_instance = sugarloaf.overlay_text_mut().instances().len();
    let width = sugarloaf.overlay_text_mut().draw(x, rect[1], icon, opts);
    sugarloaf.overlay_text_mut().center_instances_in_rect(
        first_instance,
        rect,
        center_x,
        true,
    );
    width
}
