//! Text / wrap / occluded-draw helpers shared across chrome panels.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use super::geom::rects_intersect;

const NERD_ICON_FONT_FAMILY: &str = "Symbols Nerd Font Mono";

/// Select the app-bundled Nerd Font face for private-use UI icons.
/// Browser builds have no system font cascade, and inherited explicit font
/// slots bypass fallback resolution, so icon draws must choose this shared
/// face rather than relying on the surrounding text font.
fn icon_draw_opts(sugarloaf: &mut Sugarloaf, icon: &str, opts: &DrawOpts) -> DrawOpts {
    let uses_nerd_pua = icon.chars().any(|ch| {
        matches!(ch as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD)
    });
    if !uses_nerd_pua {
        return *opts;
    }

    let mut icon_opts = *opts;
    icon_opts.font_id =
        sugarloaf
            .font_id_for_family(NERD_ICON_FONT_FAMILY)
            .or_else(|| {
                sugarloaf.ensure_static_font(
                    sugarloaf::font::constants::FONT_SYMBOLS_NERD_FONT_MONO,
                )
            });
    icon_opts
}

/// Draw a hard two-step text extrusion followed by the original foreground.
/// The caller's clip rectangle and alpha are preserved for every layer.
pub fn draw_text_extruded(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    scale: f32,
) -> f32 {
    let mut far = *opts;
    far.color = [10, 10, 14, opts.color[3].saturating_mul(5) / 6];
    sugarloaf
        .text_mut()
        .draw(x + 3.0 * scale, y + 3.0 * scale, text, &far);

    let mut near = *opts;
    near.color = [62, 62, 72, opts.color[3]];
    sugarloaf
        .text_mut()
        .draw(x + 1.5 * scale, y + 1.5 * scale, text, &near);

    sugarloaf.text_mut().draw(x, y, text, opts)
}

/// Occlusion-aware form of [`draw_text_extruded`].
pub fn draw_text_extruded_with_occlusion(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    scale: f32,
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let mut far = *opts;
    far.color = [10, 10, 14, opts.color[3].saturating_mul(5) / 6];
    draw_text_with_occlusion(
        sugarloaf,
        x + 3.0 * scale,
        y + 3.0 * scale,
        text,
        &far,
        occlusion_rects,
    );

    let mut near = *opts;
    near.color = [62, 62, 72, opts.color[3]];
    draw_text_with_occlusion(
        sugarloaf,
        x + 1.5 * scale,
        y + 1.5 * scale,
        text,
        &near,
        occlusion_rects,
    );

    draw_text_with_occlusion(sugarloaf, x, y, text, opts, occlusion_rects)
}

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
    let mut visible = vec![base_clip];

    for cut in occlusion_rects {
        if !rects_intersect(text_rect, *cut) {
            continue;
        }
        let mut next = Vec::with_capacity(visible.len() + 3);
        for rect in visible {
            if !rects_intersect(rect, *cut) {
                next.push(rect);
                continue;
            }
            let left = rect[0];
            let top = rect[1];
            let right = left + rect[2];
            let bottom = top + rect[3];
            let cut_left = cut[0].max(left);
            let cut_top = cut[1].max(top);
            let cut_right = (cut[0] + cut[2]).min(right);
            let cut_bottom = (cut[1] + cut[3]).min(bottom);
            if cut_top > top {
                next.push([left, top, rect[2], cut_top - top]);
            }
            if cut_bottom < bottom {
                next.push([left, cut_bottom, rect[2], bottom - cut_bottom]);
            }
            if cut_left > left && cut_bottom > cut_top {
                next.push([left, cut_top, cut_left - left, cut_bottom - cut_top]);
            }
            if cut_right < right && cut_bottom > cut_top {
                next.push([cut_right, cut_top, right - cut_right, cut_bottom - cut_top]);
            }
        }
        visible = next;
        if visible.is_empty() {
            return width;
        }
    }

    for rect in visible {
        if rect[2] <= 0.0 || rect[3] <= 0.0 {
            continue;
        }
        let mut clipped = *opts;
        clipped.clip_rect = Some(rect);
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
    let icon_opts = icon_draw_opts(sugarloaf, icon, opts);
    let first_instance = sugarloaf.text_mut().instances().len();
    let width = draw_text_with_occlusion(
        sugarloaf,
        x,
        rect[1],
        icon,
        &icon_opts,
        occlusion_rects,
    );
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
    let icon_opts = icon_draw_opts(sugarloaf, icon, opts);
    let first_instance = sugarloaf.overlay_text_mut().instances().len();
    let width = sugarloaf
        .overlay_text_mut()
        .draw(x, rect[1], icon, &icon_opts);
    sugarloaf.overlay_text_mut().center_instances_in_rect(
        first_instance,
        rect,
        center_x,
        true,
    );
    width
}
