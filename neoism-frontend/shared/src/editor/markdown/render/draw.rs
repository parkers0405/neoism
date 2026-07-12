use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::editor::markdown::{source_map::InlineSourceMap, MarkdownPane};
use crate::widgets::markdown as md;

use super::types::{
    BLOCK_RADIUS, DEPTH, GLOBAL_TEXT_BASELINE_FONT_SIZE, LIST_INDENT_PX,
    MARKDOWN_BODY_FONT_SIZE, ORDER_BG, ORDER_TEXT,
};
use crate::primitives::ide_theme::IdeTheme;

pub(super) fn intersect_rect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    md::intersect_rect(a, b)
}

pub(super) fn draw_rect_clipped(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    depth: f32,
    order: u8,
) {
    let [clip_x, clip_y, clip_w, clip_h] = clip;
    let x0 = x.max(clip_x);
    let y0 = y.max(clip_y);
    let x1 = (x + w).min(clip_x + clip_w);
    let y1 = (y + h).min(clip_y + clip_h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    sugarloaf.rect(None, x0, y0, x1 - x0, y1 - y0, color, depth, order);
}

pub(super) fn draw_rounded_rect_clipped(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: [f32; 4],
    depth: f32,
    order: u8,
) {
    // Historical semantics here were exact fully-inside; sub-epsilon
    // tolerance matches it (intersect returns exact copies when
    // contained).
    crate::widgets::quad::rounded_rect_clipped(
        sugarloaf,
        clip,
        None,
        [x, y, w, h],
        color,
        depth,
        radius,
        order,
        f32::EPSILON,
    );
}

pub(super) fn draw_if_visible(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    clip_top: f32,
    clip_bottom: f32,
    occlusions: &[[f32; 4]],
) {
    let h = opts.font_size * 1.5;
    if y + h >= clip_top && y <= clip_bottom {
        if occlusions.is_empty() {
            sugarloaf.text_mut().draw(x, y, text, opts);
            return;
        }
        let w = sugarloaf.text_mut().measure(text, opts);
        if occlusions
            .iter()
            .any(|rect| rects_intersect([x, y, w, h], *rect))
        {
            return;
        }
        sugarloaf.text_mut().draw(x, y, text, opts);
    }
}

pub(super) fn draw_wrapped(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    mut y: f32,
    text: &str,
    max_w: f32,
    opts: &DrawOpts,
    clip_top: f32,
    clip_bottom: f32,
    occlusions: &[[f32; 4]],
) -> f32 {
    let line_h = line_height(opts);
    for line in wrap_lines(sugarloaf, text, max_w, opts) {
        draw_if_visible(
            sugarloaf,
            x,
            y,
            &line,
            opts,
            clip_top,
            clip_bottom,
            occlusions,
        );
        y += line_h;
    }
    y
}

pub(super) fn rects_intersect(a: [f32; 4], b: [f32; 4]) -> bool {
    md::rects_intersect(a, b)
}

pub(super) fn point_in_rect(x: f32, y: f32, rect: [f32; 4]) -> bool {
    md::point_in_rect(x, y, rect)
}

pub(super) fn wrap_lines(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> Vec<String> {
    md::wrap_words_measured(sugarloaf, text, max_w, opts)
}

pub(super) fn line_height(opts: &DrawOpts) -> f32 {
    md::line_height(opts)
}

pub(super) fn markdown_font(size: f32, scale: f32) -> f32 {
    size * scale.clamp(0.5, 3.0)
        * (GLOBAL_TEXT_BASELINE_FONT_SIZE / MARKDOWN_BODY_FONT_SIZE)
}

/// Font id for the Mash Up Pack markdown font override
/// (`[look.markdown] font-family`). `None` — no override configured, or
/// the family isn't loaded — keeps today's rendering byte-identical
/// (primary cascade with per-char fallback). `Some` shapes the run in
/// that one already-loaded family; resolution is a read-lock + map hit
/// in the font library, so calling this per `DrawOpts` construction is
/// fine. Measure and draw sites MUST pair on the same value or the
/// virtual layout heights / caret x drift — thread it through both.
/// Code blocks, inline code chips, and the illuminated/drop-cap path
/// keep their own fonts and never take this id.
pub(super) fn md_font_id(sugarloaf: &Sugarloaf) -> Option<usize> {
    let family = crate::primitives::look::markdown_font_family()?;
    sugarloaf.font_id_for_family(&family)
}

pub(super) fn caret_height(opts: &DrawOpts) -> f32 {
    md::caret_height(opts)
}

pub(super) fn cursor_y_for_text_line(text_y: f32, opts: &DrawOpts) -> f32 {
    text_y + (caret_height(opts) - opts.font_size).max(0.0) * 0.25
}

pub(super) fn cursor_cell_width(opts: &DrawOpts) -> f32 {
    md::cursor_cell_width(opts)
}

pub(super) fn draw_task_checkbox(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    x: f32,
    y: f32,
    checked: bool,
    theme: &IdeTheme,
    font_scale: f32,
) {
    use crate::primitives::look::{markdown_checkbox_look, CheckboxLook};
    if markdown_checkbox_look() == CheckboxLook::Retro95 {
        draw_task_checkbox_retro95(sugarloaf, clip, x, y, checked, theme, font_scale);
        return;
    }
    let size = 13.0 * font_scale;
    let stroke = 1.0 * font_scale;
    draw_rect_clipped(
        sugarloaf,
        clip,
        x,
        y,
        size,
        stroke,
        theme.f32(theme.muted),
        DEPTH,
        ORDER_TEXT,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        x,
        y + size,
        size,
        stroke,
        theme.f32(theme.muted),
        DEPTH,
        ORDER_TEXT,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        x,
        y,
        stroke,
        size,
        theme.f32(theme.muted),
        DEPTH,
        ORDER_TEXT,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        x + size,
        y,
        stroke,
        size + stroke,
        theme.f32(theme.muted),
        DEPTH,
        ORDER_TEXT,
    );
    if checked {
        let font_size = 11.0 * font_scale;
        let opts = DrawOpts {
            font_size,
            color: theme.u8(theme.green),
            bold: true,
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        let glyph = "✓";
        let glyph_w = sugarloaf.text_mut().measure(glyph, &opts);
        let cx = x + (size - glyph_w) * 0.5;
        let cy = y + (size - font_size) * 0.5 - font_scale;
        sugarloaf.text_mut().draw(cx, cy, glyph, &opts);
    }
}

/// Windows-3.1 style task checkbox: chunky 2px box strokes in the
/// text color over a light well, checked = bold X. Same 13px envelope
/// as the modern style so `list_marker_metrics` / `checkbox_y` /
/// `register_task_rect` geometry is untouched.
fn draw_task_checkbox_retro95(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    x: f32,
    y: f32,
    checked: bool,
    theme: &IdeTheme,
    font_scale: f32,
) {
    let size = 13.0 * font_scale;
    let stroke = 2.0 * font_scale;
    let ink = theme.f32(theme.fg);
    // Well behind the box — reads as the classic sunken checkbox on
    // light themes and as a subtle plate on dark ones.
    draw_rect_clipped(
        sugarloaf,
        clip,
        x,
        y,
        size + stroke,
        size + stroke,
        theme.f32_alpha(theme.white, 0.25),
        DEPTH,
        ORDER_TEXT,
    );
    for (rx, ry, rw, rh) in [
        (x, y, size + stroke, stroke),
        (x, y + size, size + stroke, stroke),
        (x, y, stroke, size + stroke),
        (x + size, y, stroke, size + stroke),
    ] {
        draw_rect_clipped(
            sugarloaf, clip, rx, ry, rw, rh, ink, DEPTH, ORDER_TEXT,
        );
    }
    if checked {
        let font_size = 11.0 * font_scale;
        let opts = DrawOpts {
            font_size,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        let glyph = "X";
        let glyph_w = sugarloaf.text_mut().measure(glyph, &opts);
        let cx = x + stroke + (size - stroke - glyph_w) * 0.5;
        let cy = y + (size - font_size) * 0.5 - font_scale;
        sugarloaf.text_mut().draw(cx, cy, glyph, &opts);
    }
}

pub(super) fn list_indent_px(depth: usize) -> f32 {
    depth as f32 * LIST_INDENT_PX
}

pub(super) fn floor_char_boundary(text: &str, index: usize) -> usize {
    md::floor_char_boundary(text, index)
}

pub(super) fn draw_list_guides(
    sugarloaf: &mut Sugarloaf,
    content_x: f32,
    y: f32,
    h: f32,
    depth: usize,
    theme: &IdeTheme,
    clip: [f32; 4],
) {
    // Draw only the closest parent guide. Stacking every ancestor makes
    // double-indented tasks show parallel disconnected lines; the closest
    // guide matches the visual tree users follow while scanning a nested list.
    if depth == 0 {
        return;
    }
    let level = depth - 1;
    draw_rect_clipped(
        sugarloaf,
        clip,
        content_x + (level as f32 + 0.5) * LIST_INDENT_PX,
        y,
        1.0,
        h,
        theme.f32_alpha(theme.border, 0.5),
        DEPTH,
        ORDER_BG + 1,
    );
}

pub(super) fn cursor_position_for_prefix(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    line_h: f32,
    wrap_width: f32,
    opts: &DrawOpts,
    prefix: &str,
) -> (f32, f32) {
    if prefix.is_empty() {
        return (x, y);
    }
    let wrapped = wrap_lines(sugarloaf, prefix, wrap_width, opts);
    let mut visual_line = wrapped.len().saturating_sub(1);
    let current = wrapped.last().map(String::as_str).unwrap_or("");
    let trailing_space = prefix.ends_with(char::is_whitespace);
    let mut width = sugarloaf.text_mut().measure(current, opts);
    if trailing_space {
        let space_w = sugarloaf.text_mut().measure(" ", opts);
        if !current.is_empty() && width + space_w > wrap_width.max(space_w) {
            visual_line += 1;
            width = space_w;
        } else {
            width += space_w;
        }
    }
    (x + width, y + visual_line as f32 * line_h)
}

pub(super) fn cursor_position_for_text_prefix(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    line_h: f32,
    wrap_width: f32,
    opts: &DrawOpts,
    full_text: &str,
    prefix: &str,
) -> (f32, f32) {
    let trailing_spaces = prefix
        .chars()
        .rev()
        .take_while(|ch| ch.is_whitespace())
        .count();
    let mut cursor = (x, y);
    let wrapped = wrap_lines(sugarloaf, &full_text, wrap_width, opts);
    if !prefix.is_empty() {
        let mut remaining = prefix.chars().count();
        for (visual_line, rendered) in wrapped.iter().enumerate() {
            let rendered_chars = rendered.chars().count();
            if remaining <= rendered_chars {
                let current = rendered.chars().take(remaining).collect::<String>();
                cursor = (
                    x + sugarloaf.text_mut().measure(&current, opts),
                    y + visual_line as f32 * line_h,
                );
                return advance_cursor_by_spaces(
                    sugarloaf,
                    x,
                    cursor,
                    line_h,
                    wrap_width,
                    opts,
                    missing_trailing_spaces(&current, trailing_spaces),
                );
            }
            remaining = remaining.saturating_sub(rendered_chars);
            if remaining == 0 {
                cursor = (
                    x + sugarloaf.text_mut().measure(rendered, opts),
                    y + visual_line as f32 * line_h,
                );
                return advance_cursor_by_spaces(
                    sugarloaf,
                    x,
                    cursor,
                    line_h,
                    wrap_width,
                    opts,
                    missing_trailing_spaces(rendered, trailing_spaces),
                );
            }
            remaining = remaining.saturating_sub(1);
        }
        let visual_line = wrapped.len().saturating_sub(1);
        let rendered = wrapped.last().map(String::as_str).unwrap_or("");
        cursor = (
            x + sugarloaf.text_mut().measure(rendered, opts),
            y + visual_line as f32 * line_h,
        );
        return advance_cursor_by_spaces(
            sugarloaf,
            x,
            cursor,
            line_h,
            wrap_width,
            opts,
            missing_trailing_spaces(rendered, trailing_spaces),
        );
    }
    advance_cursor_by_spaces(
        sugarloaf,
        x,
        cursor,
        line_h,
        wrap_width,
        opts,
        trailing_spaces,
    )
}

/// How many of the prefix's trailing spaces are NOT already part of the
/// measured row text. The wrapper preserves interior/trailing whitespace, so
/// blindly advancing by every trailing space double-counted them — typing
/// Tab/space mid-word drew the caret one cell past the insertion point.
fn missing_trailing_spaces(measured: &str, trailing_spaces: usize) -> usize {
    let present = measured
        .chars()
        .rev()
        .take_while(|ch| ch.is_whitespace())
        .count();
    trailing_spaces.saturating_sub(present)
}

fn advance_cursor_by_spaces(
    sugarloaf: &mut Sugarloaf,
    line_x: f32,
    mut cursor: (f32, f32),
    line_h: f32,
    wrap_width: f32,
    opts: &DrawOpts,
    count: usize,
) -> (f32, f32) {
    let space_w = sugarloaf.text_mut().measure(" ", opts);
    if space_w <= 0.0 {
        return cursor;
    }
    for _ in 0..count {
        let used_w = cursor.0 - line_x;
        if used_w > 0.0 && used_w + space_w > wrap_width.max(space_w) {
            cursor.0 = line_x + space_w;
            cursor.1 += line_h;
        } else {
            cursor.0 += space_w;
        }
    }
    cursor
}

pub(super) fn draw_copy_button(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    theme: &IdeTheme,
    clip: [f32; 4],
    font_scale: f32,
) {
    draw_rect_clipped(
        sugarloaf,
        clip,
        rect[0],
        rect[1],
        rect[2],
        rect[3],
        theme.f32_alpha(theme.hover, 0.64),
        DEPTH,
        ORDER_BG + 3,
    );
    let opts = DrawOpts {
        font_size: markdown_font(15.0, font_scale),
        color: theme.u8_alpha(theme.fg, 0.82),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    sugarloaf
        .text_mut()
        .draw(rect[0] + 5.0, rect[1] + 3.0, "󰆏", &opts);
}

pub(super) fn draw_block_chrome(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    handle_visible: bool,
    chrome_visible: bool,
    dragging: bool,
) {
    if y + h < clip_top || y > clip_bottom {
        return;
    }
    if !(chrome_visible || dragging) {
        return;
    }
    if handle_visible || dragging {
        draw_block_actions(sugarloaf, [x, y, w, h], theme, clip, dragging);
    }
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        x,
        y,
        w,
        h,
        BLOCK_RADIUS,
        theme.f32_alpha(theme.surface, 0.72),
        DEPTH,
        ORDER_BG + 1,
    );
}

pub(super) fn draw_block_actions(
    sugarloaf: &mut Sugarloaf,
    block_rect: [f32; 4],
    theme: &IdeTheme,
    clip: [f32; 4],
    dragging: bool,
) {
    let x = block_rect[0] - 33.0;
    let y = block_rect[1] + 7.0;
    draw_block_handle(sugarloaf, x, y, theme, clip, dragging);
}

pub(super) fn draw_block_handle(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    theme: &IdeTheme,
    clip: [f32; 4],
    dragging: bool,
) {
    let bg = if dragging { theme.accent } else { theme.hover };
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        x,
        y,
        24.0,
        24.0,
        6.0,
        theme.f32_alpha(bg, if dragging { 0.95 } else { 0.82 }),
        DEPTH,
        ORDER_BG + 2,
    );
    let opts = DrawOpts {
        font_size: 13.0,
        color: if dragging {
            theme.u8(theme.bg)
        } else {
            theme.u8_alpha(theme.fg, 0.82)
        },
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let glyph = "⋮";
    let glyph_w = sugarloaf.text_mut().measure(glyph, &opts);
    sugarloaf
        .text_mut()
        .draw(x + (24.0 - glyph_w) * 0.5, y + 3.0, glyph, &opts);
}

pub(super) fn draw_drag_ghost(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    source: &str,
    theme: &IdeTheme,
    clip: [f32; 4],
    font_scale: f32,
) {
    draw_rect_clipped(
        sugarloaf,
        clip,
        x,
        y,
        w,
        42.0,
        theme.f32_alpha(theme.hover, 0.92),
        DEPTH,
        ORDER_TEXT + 1,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        x,
        y,
        4.0,
        42.0,
        theme.f32(theme.accent),
        DEPTH,
        ORDER_TEXT + 2,
    );
    let opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let label = source.trim();
    sugarloaf.text_mut().draw(
        x + 18.0,
        y + 12.0,
        if label.is_empty() {
            "Empty block"
        } else {
            label
        },
        &opts,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_selection_for_line(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    line_ix: usize,
    line: &str,
    text_x: f32,
    text_y: f32,
    marker_len: usize,
    line_h: f32,
    wrap_width: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
) {
    let Some((raw_start, raw_end)) = pane.selection_for_line(line_ix) else {
        return;
    };
    draw_text_range_highlight(
        sugarloaf,
        line,
        raw_start,
        raw_end,
        text_x,
        text_y,
        marker_len,
        line_h,
        wrap_width,
        opts,
        theme.f32_alpha(theme.accent, 0.26),
        clip,
        clip_top,
        clip_bottom,
        ORDER_BG + 3,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_yank_flash_for_line(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    line_ix: usize,
    line: &str,
    text_x: f32,
    text_y: f32,
    marker_len: usize,
    line_h: f32,
    wrap_width: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
) {
    let Some((raw_start, raw_end, alpha)) = pane.yank_flash_for_line(line_ix) else {
        return;
    };
    if alpha <= 0.001 {
        return;
    }
    draw_text_range_highlight(
        sugarloaf,
        line,
        raw_start,
        raw_end,
        text_x,
        text_y,
        marker_len,
        line_h,
        wrap_width,
        opts,
        theme.f32_alpha(theme.yellow, alpha),
        clip,
        clip_top,
        clip_bottom,
        ORDER_BG + 4,
    );
}

/// Paint the `/` incremental-search matches on `line_ix`: a dim yellow
/// wash over every occurrence and a brighter one on the focused match,
/// mirroring nvim's `Search` / `IncSearch` split. Reuses the same range
/// highlighter as selection so wrapped lines stay pixel-aligned.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_search_matches_for_line(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    line_ix: usize,
    line: &str,
    text_x: f32,
    text_y: f32,
    marker_len: usize,
    line_h: f32,
    wrap_width: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
) {
    let matches = pane.search_matches_for_line(line_ix);
    if matches.is_empty() {
        return;
    }
    for (raw_start, raw_end, is_current) in matches {
        let color = if is_current {
            theme.f32_alpha(theme.yellow, 0.52)
        } else {
            theme.f32_alpha(theme.yellow, 0.24)
        };
        draw_text_range_highlight(
            sugarloaf,
            line,
            raw_start,
            raw_end,
            text_x,
            text_y,
            marker_len,
            line_h,
            wrap_width,
            opts,
            color,
            clip,
            clip_top,
            clip_bottom,
            ORDER_BG + 4,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_text_range_highlight(
    sugarloaf: &mut Sugarloaf,
    line: &str,
    raw_start: usize,
    raw_end: usize,
    text_x: f32,
    text_y: f32,
    marker_len: usize,
    line_h: f32,
    wrap_width: f32,
    opts: &DrawOpts,
    color: [f32; 4],
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    order: u8,
) {
    let marker_len = marker_len.min(line.len());
    let start = raw_start.max(marker_len).min(line.len());
    let end = raw_end.max(marker_len).min(line.len());
    if start > end {
        return;
    }
    let (start_line, start_x) =
        selection_point_for_col(sugarloaf, line, start, marker_len, wrap_width, opts);
    let (end_line, end_x) =
        selection_point_for_col(sugarloaf, line, end, marker_len, wrap_width, opts);
    let cell_w = cursor_cell_width(opts);

    if start_line == end_line {
        let width = (end_x - start_x).abs().max(cell_w);
        draw_rect_clipped(
            sugarloaf,
            clip,
            text_x + start_x.min(end_x),
            text_y + start_line as f32 * line_h,
            width,
            line_h,
            color,
            DEPTH,
            order,
        );
        return;
    }

    for visual_line in start_line..=end_line {
        let x = if visual_line == start_line {
            start_x
        } else {
            0.0
        };
        let w = if visual_line == end_line {
            end_x.max(cell_w)
        } else {
            (wrap_width - x).max(cell_w)
        };
        let y = text_y + visual_line as f32 * line_h;
        if y + line_h >= clip_top && y <= clip_bottom {
            draw_rect_clipped(
                sugarloaf,
                clip,
                text_x + x,
                y,
                w,
                line_h,
                color,
                DEPTH,
                order,
            );
        }
    }
}

pub(super) fn selection_point_for_col(
    sugarloaf: &mut Sugarloaf,
    line: &str,
    col: usize,
    marker_len: usize,
    wrap_width: f32,
    opts: &DrawOpts,
) -> (usize, f32) {
    if col <= marker_len || marker_len >= line.len() {
        return (0, 0.0);
    }
    let end = floor_char_boundary(line, col.min(line.len()));
    let marker_len = marker_len.min(end);
    let body = &line[marker_len..];
    let map = InlineSourceMap::new(body);
    let full = visible_markdown_prefix(body, &map, map.visible_len());
    let prefix =
        visible_markdown_prefix(body, &map, map.visible_for_source(end - marker_len));
    if prefix.is_empty() {
        return (0, 0.0);
    }
    let line_h = line_height(opts);
    let (x, y) = cursor_position_for_text_prefix(
        sugarloaf, 0.0, 0.0, line_h, wrap_width, opts, &full, &prefix,
    );
    let visual_line = (y / line_h.max(1.0)).round().max(0.0) as usize;
    (visual_line, x)
}

pub(super) fn visible_prefix(line: &str, cursor_col: usize, marker_len: usize) -> String {
    if cursor_col <= marker_len || marker_len >= line.len() {
        return String::new();
    }
    let end = floor_char_boundary(line, cursor_col.min(line.len()));
    let marker_len = marker_len.min(end);
    let body = &line[marker_len..];
    let map = InlineSourceMap::new(body);
    visible_markdown_prefix(body, &map, map.visible_for_source(end - marker_len))
}

pub(super) fn visible_markdown_prefix(
    _source: &str,
    map: &InlineSourceMap,
    visible_len: usize,
) -> String {
    map.visible_prefix(visible_len)
}
