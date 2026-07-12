mod bullet_align;
mod draw;
mod draw_embed;
mod illuminated;
mod inline;
mod lines;
mod mermaid;
mod scrollbar;
mod table;
mod types;
mod virtualized;

pub use inline::spelling_suggestions;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::editor::markdown::{MarkdownPane, MarkdownWrapKey};
use crate::primitives::truncate_to_fit;

use crate::editor::markdown::source_map::InlineSourceMap;
use crate::primitives::ide_theme::IdeTheme;
use crate::syntax::{highlight_line, syn_color, Lang};
use crate::widgets::diff_card;

use draw::{
    caret_height, cursor_cell_width, cursor_position_for_text_prefix,
    cursor_y_for_text_line, draw_block_actions, draw_block_chrome, draw_copy_button,
    draw_drag_ghost, draw_if_visible, draw_list_guides, draw_rect_clipped,
    draw_rounded_rect_clipped, draw_selection_for_line, draw_task_checkbox, draw_wrapped,
    draw_yank_flash_for_line, line_height, list_indent_px, markdown_font, visible_prefix,
    wrap_lines,
};
use draw_embed::render_draw_block;
use inline::{
    clean_inline_with_active_link, draw_inline_links_for_line, draw_spellcheck_underlines,
};
use lines::{
    code_block_end, heading_section_tasks_complete, is_closing_code_fence,
    is_same_paragraph_neighbor, parse_render_line,
};
use mermaid::render_mermaid_block;
use scrollbar::draw_markdown_scrollbar;
use table::{parse_table, render_table};
use types::{RenderLineKind, DEPTH, ORDER_BG, ORDER_TEXT};

pub fn render(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
    animation_phase: f32,
) {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let font_scale = font_scale.clamp(0.5, 3.0);

    if virtualized::render_virtual(
        sugarloaf,
        pane,
        rect,
        theme,
        mouse,
        text_occlusions,
        font_scale,
        animation_phase,
    ) {
        return;
    }

    let bg = theme.f32(theme.bg);
    let surface = theme.f32(theme.surface);
    sugarloaf.rect(None, x, y, w, h, bg, DEPTH, ORDER_BG);

    let pad_x = 48.0;
    let pad_top = 38.0;
    let content_w = (w - pad_x * 2.0).clamp(220.0, 920.0);
    let content_x = x + ((w - content_w) * 0.5).max(pad_x.min(w * 0.08));
    let clip = [x, y, w, h];
    let mut cursor_y = y + pad_top - pane.scroll_y;
    let bottom = y + h;
    pane.begin_block_layout();

    pane.set_cursor_rect(None);

    if let Some(err) = pane.error.as_ref() {
        let opts = DrawOpts {
            font_size: markdown_font(16.0, font_scale),
            color: theme.u8(theme.red),
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        draw_wrapped(
            sugarloaf,
            content_x,
            cursor_y,
            err,
            content_w,
            &opts,
            y,
            bottom,
            text_occlusions,
        );
        pane.set_content_height(cursor_y - y + 80.0 + pane.scroll_y, h);
        return;
    }

    let lines = pane.lines.clone();
    let cursor_line = pane.cursor_line;
    let cursor_col = pane.cursor_col;
    let mut in_code = false;
    let parsed_lines = lines
        .iter()
        .map(|line| {
            let mut parsed = parse_render_line(line, in_code);
            if matches!(parsed.kind, RenderLineKind::CodeFence) {
                in_code = !in_code;
            } else if in_code {
                parsed.kind = RenderLineKind::Code;
                parsed.marker_len = 0;
                parsed.text = line.as_str();
            }
            parsed
        })
        .collect::<Vec<_>>();

    let mut skip_until = 0usize;
    for (line_ix, (line, parsed)) in lines.iter().zip(parsed_lines.iter()).enumerate() {
        if line_ix < skip_until {
            continue;
        }
        let is_cursor_line = line_ix == cursor_line;
        let text_x;
        let text_y;
        let opts;
        let line_h;
        let cursor_wrap_width;
        let cursor_marker_len = parsed.marker_len;

        if let Some(table) = parse_table(&lines, line_ix) {
            skip_until = table.end_line;
            cursor_y = render_table(
                sugarloaf,
                pane,
                &table,
                &lines,
                line_ix,
                cursor_line,
                cursor_col,
                content_x,
                cursor_y,
                content_w,
                clip,
                y,
                bottom,
                theme,
                mouse,
                text_occlusions,
                font_scale,
            );
            continue;
        }

        if matches!(parsed.kind, RenderLineKind::CodeFence)
            && is_closing_code_fence(&parsed_lines, line_ix)
        {
            continue;
        }

        if matches!(parsed.kind, RenderLineKind::CodeFence)
            && parsed.text.eq_ignore_ascii_case("draw")
        {
            let code_end = code_block_end(&lines, line_ix);
            let cursor_inside = (line_ix..=code_end).contains(&cursor_line);
            if !cursor_inside {
                let code = lines
                    .get(line_ix + 1..code_end.min(lines.len()))
                    .unwrap_or_default()
                    .join("\n");
                if let Some(rendered_y) = render_draw_block(
                    sugarloaf, pane, &code, content_x, cursor_y, content_w, clip, y,
                    bottom, theme, font_scale,
                ) {
                    skip_until = code_end.saturating_add(1);
                    cursor_y = rendered_y;
                    continue;
                }
            }
        }

        if matches!(parsed.kind, RenderLineKind::CodeFence)
            && parsed.text.eq_ignore_ascii_case("mermaid")
        {
            let code_end = code_block_end(&lines, line_ix);
            let cursor_inside = (line_ix..=code_end).contains(&cursor_line);
            if !cursor_inside {
                let code = lines
                    .get(line_ix + 1..code_end.min(lines.len()))
                    .unwrap_or_default()
                    .join("\n");
                if let Some(rendered_y) = render_mermaid_block(
                    sugarloaf,
                    pane,
                    line_ix,
                    code_end,
                    &code,
                    content_x,
                    cursor_y,
                    content_w,
                    clip,
                    y,
                    bottom,
                    theme,
                    mouse,
                    text_occlusions,
                    font_scale,
                ) {
                    skip_until = code_end.saturating_add(1);
                    cursor_y = rendered_y;
                    continue;
                }
            }
        }

        match parsed.kind {
            RenderLineKind::Empty => {
                let empty_opts = DrawOpts {
                    font_size: markdown_font(17.0, font_scale),
                    color: theme.u8(theme.fg),
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let caret_h = caret_height(&empty_opts);
                let paragraph_continuation = is_cursor_line
                    && pane.is_enter_continuation_line(line_ix)
                    && is_same_paragraph_neighbor(&parsed_lines, line_ix, -1);
                let line_h = line_height(&empty_opts);
                let row_h = if paragraph_continuation {
                    line_h + 12.0
                } else {
                    line_h + 20.0
                };
                let text_y = cursor_y + 4.0;
                let caret_y = cursor_y_for_text_line(text_y, &empty_opts);
                let block_rect = if paragraph_continuation {
                    [
                        content_x - 18.0,
                        cursor_y - 8.0,
                        content_w + 36.0,
                        row_h + 8.0,
                    ]
                } else {
                    [content_x - 18.0, cursor_y, content_w + 36.0, row_h]
                };
                let handle_rect =
                    [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                let active = pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    content_x + 10.0,
                    text_y,
                    0,
                    cursor_cell_width(&empty_opts),
                    line_h,
                    content_w,
                    mouse,
                );
                if paragraph_continuation {
                    draw_block_chrome(
                        sugarloaf,
                        block_rect[0],
                        block_rect[1],
                        block_rect[2],
                        block_rect[3],
                        theme,
                        clip,
                        y,
                        bottom,
                        active,
                        active || is_cursor_line,
                        dragging,
                    );
                } else if active {
                    draw_block_actions(sugarloaf, block_rect, theme, clip, dragging);
                }
                if is_cursor_line {
                    pane.set_cursor_rect(Some([
                        content_x + 10.0,
                        caret_y,
                        cursor_cell_width(&empty_opts),
                        caret_h,
                    ]));
                }
                draw_selection_for_line(
                    sugarloaf,
                    pane,
                    line_ix,
                    line,
                    content_x + 10.0,
                    text_y,
                    0,
                    line_h,
                    content_w,
                    &empty_opts,
                    theme,
                    clip,
                    y,
                    bottom,
                );
                draw_yank_flash_for_line(
                    sugarloaf,
                    pane,
                    line_ix,
                    line,
                    content_x + 10.0,
                    text_y,
                    0,
                    line_h,
                    content_w,
                    &empty_opts,
                    theme,
                    clip,
                    y,
                    bottom,
                );
                if is_cursor_line && !paragraph_continuation {
                    let placeholder_opts = DrawOpts {
                        font_size: empty_opts.font_size,
                        color: theme.u8_alpha(theme.muted, 0.58),
                        clip_rect: Some(clip),
                        ..DrawOpts::default()
                    };
                    draw_if_visible(
                        sugarloaf,
                        content_x + 10.0,
                        text_y,
                        "Type '/' for commands",
                        &placeholder_opts,
                        y,
                        bottom,
                        text_occlusions,
                    );
                }
                cursor_y += row_h;
                continue;
            }
            RenderLineKind::Heading(level) => {
                let font_size = match level {
                    1 => markdown_font(44.0, font_scale),
                    2 => markdown_font(31.0, font_scale),
                    3 => markdown_font(23.0, font_scale),
                    _ => markdown_font(18.0, font_scale),
                };
                cursor_y += if level == 1 { 10.0 } else { 16.0 };
                opts = DrawOpts {
                    font_size,
                    color: theme.u8(theme.fg),
                    bold: true,
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                text_x = content_x;
                text_y = cursor_y;
                line_h = line_height(&opts);
                cursor_wrap_width = content_w;
                let active_col = if is_cursor_line && cursor_col >= cursor_marker_len {
                    Some(cursor_col - cursor_marker_len)
                } else {
                    None
                };
                let clean = clean_inline_with_active_link(parsed.text, active_col);
                let wrapped =
                    wrap_lines_cached(sugarloaf, pane, &clean, cursor_wrap_width, &opts);
                let block_h = line_h * wrapped.len().max(1) as f32 + 12.0;
                let block_rect =
                    [content_x - 18.0, cursor_y - 6.0, content_w + 36.0, block_h];
                let handle_rect =
                    [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                if pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    text_x,
                    text_y,
                    cursor_marker_len,
                    cursor_cell_width(&opts),
                    line_h,
                    cursor_wrap_width,
                    mouse,
                ) {
                    draw_block_actions(sugarloaf, block_rect, theme, clip, dragging);
                }
                let mut heading_y = text_y;
                let section_complete =
                    heading_section_tasks_complete(&parsed_lines, line_ix, level);
                for rendered in wrapped {
                    draw_if_visible(
                        sugarloaf,
                        text_x,
                        heading_y,
                        &rendered,
                        &opts,
                        y,
                        bottom,
                        text_occlusions,
                    );
                    if section_complete && heading_y + line_h >= y && heading_y <= bottom
                    {
                        let strike_w = sugarloaf.text_mut().measure(&rendered, &opts);
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            text_x,
                            heading_y + line_h * 0.56,
                            strike_w,
                            2.0,
                            theme.f32_alpha(theme.muted, 0.78),
                            DEPTH,
                            ORDER_TEXT + 1,
                        );
                    }
                    heading_y += line_h;
                }
                cursor_y += block_h + if level == 1 { 20.0 } else { 14.0 };
            }
            RenderLineKind::Task { checked, depth } => {
                opts = DrawOpts {
                    font_size: markdown_font(16.0, font_scale),
                    color: if checked {
                        theme.u8(theme.muted)
                    } else {
                        theme.u8(theme.fg)
                    },
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let indent_x = list_indent_px(depth);
                let checkbox_x = content_x + indent_x + 14.0;
                text_x = checkbox_x + 30.0 * font_scale;
                line_h = line_height(&opts);
                cursor_wrap_width =
                    (content_w - indent_x - (text_x - content_x)).max(140.0);
                let wrapped = wrap_lines_cached(
                    sugarloaf,
                    pane,
                    parsed.text,
                    cursor_wrap_width,
                    &opts,
                );
                let text_h = line_h * wrapped.len().max(1) as f32;
                let row_h = text_h + 22.0;
                text_y = cursor_y + (row_h - text_h) * 0.5;
                let block_rect =
                    [content_x - 18.0, cursor_y - 6.0, content_w + 36.0, row_h];
                let handle_rect =
                    [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                let active = pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    text_x,
                    text_y,
                    cursor_marker_len,
                    cursor_cell_width(&opts),
                    line_h,
                    cursor_wrap_width,
                    mouse,
                );
                draw_block_chrome(
                    sugarloaf,
                    block_rect[0],
                    block_rect[1],
                    block_rect[2],
                    block_rect[3],
                    theme,
                    clip,
                    y,
                    bottom,
                    active,
                    active || is_cursor_line,
                    dragging,
                );
                if cursor_y + row_h >= y && cursor_y <= bottom {
                    draw_list_guides(
                        sugarloaf,
                        content_x,
                        cursor_y - 5.0,
                        row_h,
                        depth,
                        theme,
                        clip,
                    );
                    let checkbox_size = 13.0 * font_scale;
                    // Center the checkbox on the text's x-height midline so
                    // the box reads as visually aligned with the label.
                    let box_y =
                        bullet_align::checkbox_y(text_y, opts.font_size, checkbox_size);
                    let check_rect = [checkbox_x, box_y, checkbox_size, checkbox_size];
                    pane.register_task_rect(line_ix, check_rect);
                    draw_task_checkbox(
                        sugarloaf, clip, checkbox_x, box_y, checked, theme, font_scale,
                    );
                }
                let mut line_y = text_y;
                for rendered in wrapped {
                    draw_if_visible(
                        sugarloaf,
                        text_x,
                        line_y,
                        &rendered,
                        &opts,
                        y,
                        bottom,
                        text_occlusions,
                    );
                    if checked && line_y + line_h >= y && line_y <= bottom {
                        let strike_w = sugarloaf.text_mut().measure(&rendered, &opts);
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            text_x,
                            line_y + line_h * 0.58,
                            strike_w,
                            1.5,
                            theme.f32(theme.muted),
                            DEPTH,
                            ORDER_TEXT,
                        );
                    }
                    line_y += line_h;
                }
                cursor_y += row_h;
            }
            RenderLineKind::Bullet { depth } => {
                opts = DrawOpts {
                    font_size: markdown_font(17.0, font_scale),
                    color: theme.u8(theme.fg),
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let indent_x = list_indent_px(depth);
                text_x = content_x + indent_x + 38.0;
                line_h = line_height(&opts);
                cursor_wrap_width = (content_w - indent_x - 38.0).max(140.0);
                let wrapped = wrap_lines_cached(
                    sugarloaf,
                    pane,
                    parsed.text,
                    cursor_wrap_width,
                    &opts,
                );
                let text_h = line_h * wrapped.len().max(1) as f32;
                let row_h = text_h + 20.0;
                text_y = cursor_y + (row_h - text_h) * 0.5;
                let block_rect =
                    [content_x - 18.0, cursor_y - 6.0, content_w + 36.0, row_h];
                let handle_rect =
                    [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                let active = pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    text_x,
                    text_y,
                    cursor_marker_len,
                    cursor_cell_width(&opts),
                    line_h,
                    cursor_wrap_width,
                    mouse,
                );
                draw_block_chrome(
                    sugarloaf,
                    block_rect[0],
                    block_rect[1],
                    block_rect[2],
                    block_rect[3],
                    theme,
                    clip,
                    y,
                    bottom,
                    active,
                    active || is_cursor_line,
                    dragging,
                );
                if cursor_y + row_h >= y && cursor_y <= bottom {
                    draw_list_guides(
                        sugarloaf,
                        content_x,
                        cursor_y - 5.0,
                        row_h,
                        depth,
                        theme,
                        clip,
                    );
                    // Center the bullet dot on the x-height midline of the
                    // first text line (not the geometric middle of the row
                    // box, which sits visually low). See `bullet_align`.
                    let bullet_size = 5.0;
                    draw_rect_clipped(
                        sugarloaf,
                        clip,
                        content_x + indent_x + 18.0,
                        bullet_align::bullet_dot_y(text_y, opts.font_size, bullet_size),
                        bullet_size,
                        bullet_size,
                        theme.f32(theme.accent),
                        DEPTH,
                        ORDER_BG + 2,
                    );
                    let mut line_y = text_y;
                    for rendered in wrapped {
                        draw_if_visible(
                            sugarloaf,
                            text_x,
                            line_y,
                            &rendered,
                            &opts,
                            y,
                            bottom,
                            text_occlusions,
                        );
                        line_y += line_h;
                    }
                }
                cursor_y += row_h;
            }
            RenderLineKind::Ordered { depth } => {
                opts = DrawOpts {
                    font_size: markdown_font(17.0, font_scale),
                    color: theme.u8(theme.fg),
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let indent_x = list_indent_px(depth);
                text_x = content_x + indent_x + 48.0;
                line_h = line_height(&opts);
                cursor_wrap_width = (content_w - indent_x - 48.0).max(140.0);
                let wrapped = wrap_lines_cached(
                    sugarloaf,
                    pane,
                    parsed.text,
                    cursor_wrap_width,
                    &opts,
                );
                let text_h = line_h * wrapped.len().max(1) as f32;
                let row_h = text_h + 20.0;
                text_y = cursor_y + (row_h - text_h) * 0.5;
                let block_rect =
                    [content_x - 18.0, cursor_y - 6.0, content_w + 36.0, row_h];
                let handle_rect =
                    [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                let active = pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    text_x,
                    text_y,
                    cursor_marker_len,
                    cursor_cell_width(&opts),
                    line_h,
                    cursor_wrap_width,
                    mouse,
                );
                draw_block_chrome(
                    sugarloaf,
                    block_rect[0],
                    block_rect[1],
                    block_rect[2],
                    block_rect[3],
                    theme,
                    clip,
                    y,
                    bottom,
                    active,
                    active || is_cursor_line,
                    dragging,
                );
                if cursor_y + row_h >= y && cursor_y <= bottom {
                    draw_list_guides(
                        sugarloaf,
                        content_x,
                        cursor_y - 5.0,
                        row_h,
                        depth,
                        theme,
                        clip,
                    );
                    let marker = parsed.list_marker.unwrap_or("1)");
                    let marker_opts = DrawOpts {
                        font_size: markdown_font(14.0, font_scale),
                        color: theme.u8(theme.accent),
                        bold: true,
                        clip_rect: Some(clip),
                        ..DrawOpts::default()
                    };
                    // Align the marker's x-height midline with the text's,
                    // so `1.` sits visually centered next to the first line
                    // of text rather than baseline-aligned at the row top.
                    let marker_y = bullet_align::text_marker_y(
                        text_y,
                        opts.font_size,
                        marker_opts.font_size,
                    );
                    draw_if_visible(
                        sugarloaf,
                        content_x + indent_x + 8.0,
                        marker_y,
                        marker,
                        &marker_opts,
                        y,
                        bottom,
                        text_occlusions,
                    );
                    let mut line_y = text_y;
                    for rendered in wrapped {
                        draw_if_visible(
                            sugarloaf,
                            text_x,
                            line_y,
                            &rendered,
                            &opts,
                            y,
                            bottom,
                            text_occlusions,
                        );
                        line_y += line_h;
                    }
                }
                cursor_y += row_h;
            }
            RenderLineKind::CodeFence => {
                let label = parsed.text;
                opts = DrawOpts {
                    font_size: diff_card::HEADER_FONT_SIZE * font_scale,
                    color: theme.u8(theme.muted),
                    bold: true,
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let header_h = diff_card::HEADER_HEIGHT * font_scale;
                let card_x = content_x - 14.0;
                let card_w = content_w + 28.0;
                text_x = card_x + diff_card::HEADER_PAD_X * font_scale;
                text_y = cursor_y + (header_h - opts.font_size) * 0.5;
                line_h = header_h;
                let block_rect = [card_x, cursor_y, card_w, header_h];
                let handle_rect =
                    [block_rect[0] - 40.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                cursor_wrap_width =
                    content_w - diff_card::HEADER_PAD_X * 2.0 * font_scale;
                let active = pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    text_x,
                    text_y,
                    cursor_marker_len,
                    cursor_cell_width(&opts),
                    line_h,
                    cursor_wrap_width,
                    mouse,
                );
                if cursor_y + line_h >= y && cursor_y <= bottom {
                    if active {
                        draw_block_actions(sugarloaf, block_rect, theme, clip, dragging);
                    }
                    draw_rounded_rect_clipped(
                        sugarloaf,
                        clip,
                        block_rect[0],
                        block_rect[1],
                        block_rect[2],
                        block_rect[3],
                        diff_card::CARD_RADIUS * font_scale,
                        surface,
                        DEPTH,
                        ORDER_BG + 1,
                    );
                    let code_end = code_block_end(&lines, line_ix);
                    let copy_rect = [
                        block_rect[0] + block_rect[2] - 30.0,
                        block_rect[1] + 3.0,
                        24.0,
                        24.0,
                    ];
                    pane.register_copy_code_rect(copy_rect, line_ix, code_end);
                    draw_copy_button(sugarloaf, copy_rect, theme, clip, font_scale);
                    draw_if_visible(
                        sugarloaf,
                        text_x,
                        text_y,
                        label,
                        &opts,
                        y,
                        bottom,
                        text_occlusions,
                    );
                }
                cursor_y += line_h;
            }
            RenderLineKind::Code => {
                let code_start =
                    code_block_start(&parsed_lines, line_ix).unwrap_or(line_ix);
                let lang_label = parsed_lines
                    .get(code_start)
                    .map(|line| line.text)
                    .unwrap_or_default();
                let lang = markdown_code_lang(lang_label);
                let line_no = line_ix.saturating_sub(code_start);
                let card_x = content_x - 14.0;
                let card_w = content_w + 28.0;
                let gutter_w = diff_card::GUTTER_WIDTH * font_scale;
                let body_pad_x = diff_card::BODY_PAD_X * font_scale;
                opts = DrawOpts {
                    font_size: diff_card::FONT_SIZE * font_scale,
                    color: theme.u8(theme.fg),
                    bold: true,
                    clip_rect: Some([
                        card_x + gutter_w + body_pad_x,
                        cursor_y,
                        (card_w - gutter_w - body_pad_x * 2.0).max(0.0),
                        diff_card::LINE_HEIGHT * font_scale,
                    ]),
                    ..DrawOpts::default()
                };
                line_h = diff_card::LINE_HEIGHT * font_scale;
                text_x = card_x + gutter_w + body_pad_x;
                text_y = cursor_y + (line_h - opts.font_size) * 0.5;
                cursor_wrap_width = 1_000_000.0;
                let block_h = line_h;
                let block_rect = [card_x, cursor_y, card_w, block_h];
                let handle_rect =
                    [block_rect[0] - 40.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                let active = pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    text_x,
                    text_y,
                    cursor_marker_len,
                    cursor_cell_width(&opts),
                    line_h,
                    cursor_wrap_width,
                    mouse,
                );
                if cursor_y + block_h >= y && cursor_y <= bottom {
                    if active {
                        draw_block_actions(sugarloaf, block_rect, theme, clip, dragging);
                    }
                    draw_rect_clipped(
                        sugarloaf,
                        clip,
                        block_rect[0],
                        block_rect[1],
                        block_rect[2],
                        block_rect[3],
                        theme.f32(theme.panel_bg()),
                        DEPTH,
                        ORDER_BG + 1,
                    );
                    if is_cursor_line {
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            block_rect[0],
                            block_rect[1],
                            block_rect[2],
                            block_rect[3],
                            theme.f32_alpha(theme.accent, 0.10),
                            DEPTH,
                            ORDER_BG + 2,
                        );
                    }
                    if let Some(diff) = markdown_code_diff_kind(lang_label, parsed.text) {
                        let color = markdown_code_diff_color(diff, theme);
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            text_x - body_pad_x,
                            cursor_y,
                            (card_w - gutter_w).max(0.0),
                            line_h,
                            theme.f32_alpha(color, 0.16),
                            DEPTH,
                            ORDER_BG + 3,
                        );
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            text_x - body_pad_x,
                            cursor_y,
                            3.0 * font_scale,
                            line_h,
                            theme.f32(color),
                            DEPTH,
                            ORDER_BG + 4,
                        );
                    }
                    let gutter_opts = DrawOpts {
                        font_size: diff_card::GUTTER_FONT_SIZE * font_scale,
                        color: theme.u8(theme.muted),
                        bold: true,
                        clip_rect: Some([card_x, cursor_y, gutter_w, line_h]),
                        ..DrawOpts::default()
                    };
                    let line_no_text = line_no.max(1).to_string();
                    let line_no_w =
                        sugarloaf.text_mut().measure(&line_no_text, &gutter_opts);
                    sugarloaf.text_mut().draw(
                        card_x + gutter_w - line_no_w - 6.0 * font_scale,
                        cursor_y + (line_h - gutter_opts.font_size) * 0.5,
                        &line_no_text,
                        &gutter_opts,
                    );
                    draw_markdown_code_line(
                        sugarloaf,
                        text_x,
                        text_y,
                        parsed.text,
                        lang,
                        markdown_code_diff_kind(lang_label, parsed.text),
                        &opts,
                        theme,
                        text_occlusions,
                    );
                }
                cursor_y += block_h;
            }
            RenderLineKind::Quote => {
                opts = DrawOpts {
                    font_size: markdown_font(16.0, font_scale),
                    color: theme.u8(theme.muted),
                    italic: true,
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                text_x = content_x + 28.0;
                text_y = cursor_y + 4.0;
                cursor_wrap_width = content_w - 28.0;
                line_h = line_height(&opts);
                let wrapped = wrap_lines_cached(
                    sugarloaf,
                    pane,
                    parsed.text,
                    cursor_wrap_width,
                    &opts,
                );
                let block_h = line_h * wrapped.len().max(1) as f32 + 20.0;
                let block_rect =
                    [content_x - 18.0, cursor_y - 6.0, content_w + 36.0, block_h];
                let handle_rect =
                    [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                let active = pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    text_x,
                    text_y,
                    cursor_marker_len,
                    cursor_cell_width(&opts),
                    line_h,
                    cursor_wrap_width,
                    mouse,
                );
                draw_block_chrome(
                    sugarloaf,
                    block_rect[0],
                    block_rect[1],
                    block_rect[2],
                    block_rect[3],
                    theme,
                    clip,
                    y,
                    bottom,
                    active,
                    active || is_cursor_line,
                    dragging,
                );
                draw_rect_clipped(
                    sugarloaf,
                    clip,
                    content_x + 10.0,
                    cursor_y + 2.0,
                    3.0,
                    block_h - 16.0,
                    theme.f32(theme.accent),
                    DEPTH,
                    ORDER_BG + 1,
                );
                let mut line_y = text_y;
                for rendered in wrapped {
                    draw_if_visible(
                        sugarloaf,
                        text_x,
                        line_y,
                        &rendered,
                        &opts,
                        y,
                        bottom,
                        text_occlusions,
                    );
                    line_y += line_h;
                }
                cursor_y += block_h + 8.0;
            }
            RenderLineKind::Divider => {
                let block_rect =
                    [content_x - 18.0, cursor_y - 4.0, content_w + 36.0, 28.0];
                let handle_rect =
                    [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                let div_opts = DrawOpts {
                    font_size: markdown_font(17.0, font_scale),
                    color: theme.u8(theme.fg),
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                cursor_wrap_width = content_w;
                let active = pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    content_x,
                    cursor_y,
                    cursor_marker_len,
                    cursor_cell_width(&div_opts),
                    line_height(&div_opts),
                    cursor_wrap_width,
                    mouse,
                );
                let line_y = cursor_y + 12.0;
                if line_y >= y && line_y <= bottom {
                    if active {
                        draw_block_actions(sugarloaf, block_rect, theme, clip, dragging);
                    }
                    draw_rect_clipped(
                        sugarloaf,
                        clip,
                        content_x - 10.0,
                        line_y,
                        content_w + 20.0,
                        1.0,
                        theme.f32_alpha(theme.border, 0.85),
                        DEPTH,
                        ORDER_BG + 1,
                    );
                }
                if is_cursor_line {
                    let div_line_h = line_height(&div_opts);
                    let caret_h = caret_height(&div_opts);
                    pane.set_cursor_rect(Some([
                        content_x,
                        cursor_y + (div_line_h - caret_h) * 0.5,
                        cursor_cell_width(&div_opts),
                        caret_h,
                    ]));
                }
                cursor_y += 28.0;
                continue;
            }
            RenderLineKind::Paragraph => {
                let active_col = if is_cursor_line && cursor_col >= cursor_marker_len {
                    Some(cursor_col - cursor_marker_len)
                } else {
                    None
                };
                let clean = clean_inline_with_active_link(parsed.text, active_col);
                opts = DrawOpts {
                    font_size: markdown_font(17.0, font_scale),
                    color: theme.u8(theme.fg),
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let wrapped =
                    wrap_lines_cached(sugarloaf, pane, &clean, content_w - 44.0, &opts);
                line_h = line_height(&opts);
                let continues_from_previous =
                    is_same_paragraph_neighbor(&parsed_lines, line_ix, -1);
                let continues_to_next =
                    is_same_paragraph_neighbor(&parsed_lines, line_ix, 1);
                let next_is_blank = parsed_lines
                    .get(line_ix + 1)
                    .is_some_and(|line| matches!(line.kind, RenderLineKind::Empty));
                let top_pad = if continues_from_previous { 0.0 } else { 4.0 };
                let trailing_gap = if continues_to_next || next_is_blank {
                    0.0
                } else {
                    4.0
                };
                let text_h = line_h * wrapped.len().max(1) as f32;
                let block_h = text_h + 24.0;
                let block_rect = [
                    content_x - 18.0,
                    cursor_y + top_pad - 12.0,
                    content_w + 36.0,
                    block_h,
                ];
                let handle_rect =
                    [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
                let dragging = pane.dragging_line == Some(line_ix);
                text_x = content_x + 10.0;
                text_y = cursor_y + top_pad;
                cursor_wrap_width = content_w - 44.0;
                let active = pane.register_block_rect(
                    line_ix,
                    block_rect,
                    handle_rect,
                    text_x,
                    text_y,
                    cursor_marker_len,
                    cursor_cell_width(&opts),
                    line_h,
                    cursor_wrap_width,
                    mouse,
                );
                draw_block_chrome(
                    sugarloaf,
                    block_rect[0],
                    block_rect[1],
                    block_rect[2],
                    block_rect[3],
                    theme,
                    clip,
                    y,
                    bottom,
                    active,
                    active || is_cursor_line,
                    dragging,
                );
                let mut line_y = text_y;
                for rendered in wrapped {
                    draw_if_visible(
                        sugarloaf,
                        text_x,
                        line_y,
                        &rendered,
                        &opts,
                        y,
                        bottom,
                        text_occlusions,
                    );
                    line_y += line_h;
                }
                cursor_y += top_pad + text_h + 12.0 + trailing_gap;
            }
        }

        draw_selection_for_line(
            sugarloaf,
            pane,
            line_ix,
            line,
            text_x,
            text_y,
            cursor_marker_len,
            line_h.max(line_height(&opts)),
            cursor_wrap_width,
            &opts,
            theme,
            clip,
            y,
            bottom,
        );
        draw_yank_flash_for_line(
            sugarloaf,
            pane,
            line_ix,
            line,
            text_x,
            text_y,
            cursor_marker_len,
            line_h.max(line_height(&opts)),
            cursor_wrap_width,
            &opts,
            theme,
            clip,
            y,
            bottom,
        );

        if !matches!(
            parsed.kind,
            RenderLineKind::Code | RenderLineKind::CodeFence | RenderLineKind::Divider
        ) {
            let active_col = if is_cursor_line && cursor_col >= cursor_marker_len {
                Some(cursor_col - cursor_marker_len)
            } else {
                None
            };
            let spellcheck_text = clean_inline_with_active_link(parsed.text, active_col);
            draw_spellcheck_underlines(
                sugarloaf,
                text_x,
                text_y,
                line_h.max(line_height(&opts)),
                cursor_wrap_width,
                &opts,
                theme,
                clip,
                y,
                bottom,
                text_occlusions,
                &spellcheck_text,
            );
            draw_inline_links_for_line(
                sugarloaf,
                pane,
                line,
                text_x,
                text_y,
                cursor_marker_len,
                line_h.max(line_height(&opts)),
                cursor_wrap_width,
                &opts,
                theme,
                clip,
                y,
                bottom,
                text_occlusions,
                if is_cursor_line && cursor_col >= cursor_marker_len {
                    Some(cursor_col - cursor_marker_len)
                } else {
                    None
                },
            );
        }

        if is_cursor_line && pane.cursor_rect.is_none() {
            if !matches!(parsed.kind, RenderLineKind::Code) {
                if let Some((visual_line, row_prefix)) = pane
                    .rendered_wrap_row_prefix_for_col(
                        line_ix,
                        cursor_marker_len,
                        cursor_col,
                    )
                {
                    let cursor_x = (text_x
                        + sugarloaf.text_mut().measure(&row_prefix, &opts))
                    .clamp(text_x, text_x + cursor_wrap_width.max(2.0) - 2.0);
                    let cursor_y = text_y + visual_line as f32 * line_h;
                    pane.set_cursor_rect(Some([
                        cursor_x,
                        cursor_y_for_text_line(cursor_y, &opts),
                        cursor_cell_width(&opts),
                        caret_height(&opts),
                    ]));
                    continue;
                }
            }
            let prefix = if matches!(parsed.kind, RenderLineKind::Code) {
                let end = crate::widgets::markdown::floor_char_boundary(
                    line,
                    cursor_col.min(line.len()),
                );
                line[..end].to_string()
            } else {
                visible_prefix(line, cursor_col, cursor_marker_len)
            };
            let full_text = if matches!(parsed.kind, RenderLineKind::Code) {
                line.to_string()
            } else {
                line.get(cursor_marker_len.min(line.len())..)
                    .map(|text| InlineSourceMap::new(text).visible_text())
                    .unwrap_or_default()
            };
            let cursor_line_h = line_h.max(line_height(&opts));
            let (cursor_x, cursor_y) = cursor_position_for_text_prefix(
                sugarloaf,
                text_x,
                text_y,
                cursor_line_h,
                cursor_wrap_width,
                &opts,
                &full_text,
                &prefix,
            );
            let caret_h = caret_height(&opts);
            pane.set_cursor_rect(Some([
                cursor_x,
                cursor_y_for_text_line(cursor_y, &opts),
                cursor_cell_width(&opts),
                caret_h,
            ]));
        }
    }

    if let Some(line_ix) = pane.dragging_line {
        if let Some(source) = lines.get(line_ix) {
            draw_drag_ghost(
                sugarloaf,
                content_x - 18.0,
                pane.drag_mouse_y - 20.0,
                content_w + 36.0,
                source,
                theme,
                clip,
                font_scale,
            );
        }
    }

    let content_height = cursor_y - y + 60.0 + pane.scroll_y;
    pane.set_content_height(content_height, h);
    draw_markdown_scrollbar(sugarloaf, pane, rect, content_height, theme, mouse, clip);
}

fn code_block_start(
    lines: &[types::ParsedRenderLine<'_>],
    line_ix: usize,
) -> Option<usize> {
    (0..line_ix)
        .rev()
        .find(|ix| matches!(lines[*ix].kind, RenderLineKind::CodeFence))
}

fn markdown_code_lang(lang: &str) -> Lang {
    match lang.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => Lang::Rust,
        "javascript" | "js" | "mjs" | "cjs" => Lang::Javascript,
        "jsx" => Lang::Jsx,
        "typescript" | "ts" => Lang::Typescript,
        "tsx" => Lang::Tsx,
        "python" | "py" => Lang::Python,
        "go" => Lang::Go,
        "lua" => Lang::Lua,
        "toml" => Lang::Toml,
        "json" | "jsonc" => Lang::Json,
        _ => Lang::Other,
    }
}

#[derive(Clone, Copy)]
enum MarkdownCodeDiffKind {
    Add,
    Remove,
}

fn markdown_code_diff_kind(lang: &str, line: &str) -> Option<MarkdownCodeDiffKind> {
    match lang.trim().to_ascii_lowercase().as_str() {
        "diff" | "patch" => {
            let trimmed = line.trim_start();
            if trimmed.starts_with("+++") || trimmed.starts_with("---") {
                None
            } else if trimmed.starts_with('+') {
                Some(MarkdownCodeDiffKind::Add)
            } else if trimmed.starts_with('-') {
                Some(MarkdownCodeDiffKind::Remove)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn markdown_code_diff_color(kind: MarkdownCodeDiffKind, theme: &IdeTheme) -> u32 {
    match kind {
        MarkdownCodeDiffKind::Add => theme.green,
        MarkdownCodeDiffKind::Remove => theme.red,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_markdown_code_line(
    sugarloaf: &mut Sugarloaf,
    mut x: f32,
    y: f32,
    line: &str,
    lang: Lang,
    diff: Option<MarkdownCodeDiffKind>,
    opts: &DrawOpts,
    theme: &IdeTheme,
    occlusions: &[[f32; 4]],
) {
    if let Some(kind) = diff {
        let mut diff_opts = *opts;
        diff_opts.color = theme.u8(markdown_code_diff_color(kind, theme));
        draw_if_visible(
            sugarloaf,
            x,
            y,
            line,
            &diff_opts,
            y - 1.0,
            y + diff_card::LINE_HEIGHT,
            occlusions,
        );
        return;
    }

    let right_edge = opts
        .clip_rect
        .map(|[clip_x, _, clip_w, _]| clip_x + clip_w)
        .unwrap_or(f32::MAX);
    for (tok, slice) in highlight_line(line, lang) {
        if x >= right_edge {
            return;
        }
        let mut span_opts = *opts;
        span_opts.color = syn_color(tok, theme, false);
        let budget = (right_edge - x).max(0.0);
        let measured = sugarloaf.text_mut().measure(slice, &span_opts);
        if measured <= budget {
            x += sugarloaf.text_mut().draw(x, y, slice, &span_opts);
        } else {
            let fit = truncate_to_fit(slice, budget, sugarloaf, &span_opts);
            let _ = sugarloaf.text_mut().draw(x, y, fit.as_str(), &span_opts);
            return;
        }
    }
}

fn wrap_lines_cached(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    text: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> Vec<String> {
    let key = MarkdownWrapKey::new(text, max_w, opts);
    if let Some(lines) = pane.cached_wrap_lines(&key) {
        return lines;
    }
    let lines = wrap_lines(sugarloaf, text, max_w, opts);
    pane.store_wrap_lines(key, lines.clone());
    lines
}

#[cfg(test)]
mod tests {
    use super::inline::{
        clean_inline_with_active_link, collect_inline_tags, collect_inline_wiki_links,
    };
    use super::inline::{normalized_spellcheck_word, spellcheck_words};
    use super::lines::{heading_section_tasks_complete, parse_render_line};
    use super::table::parse_table_row;

    #[test]
    fn active_wiki_link_stays_rendered_while_typing() {
        let text = "See [[@notes/page.md-12]] today";

        assert_eq!(
            clean_inline_with_active_link(text, None),
            "See page.md:12 today"
        );
        assert_eq!(
            clean_inline_with_active_link(text, Some("See [[@notes/page.".len())),
            "See page.md:12 today"
        );
    }

    #[test]
    fn html_comments_are_hidden_from_inline_text() {
        assert_eq!(
            clean_inline_with_active_link(
                "Ship it <!-- neoism-task:abcd:42 --> now",
                None
            ),
            "Ship it  now"
        );
    }

    #[test]
    fn bare_wiki_link_and_tags_are_collected_for_inline_rendering() {
        let text = "See [[Roadmap#Now|roadmap]] #neoism/workspace";
        let links = collect_inline_wiki_links(text);
        let tags = collect_inline_tags(text, &links);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].label, "roadmap");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].label, "#neoism/workspace");
    }

    #[test]
    fn heading_section_is_complete_only_when_all_tasks_are_checked() {
        let source = [
            "# Project".to_string(),
            "- [x] one".to_string(),
            "## Child".to_string(),
            "- [x] two".to_string(),
            "# Next".to_string(),
        ];
        let parsed = source
            .iter()
            .map(|line| parse_render_line(line, false))
            .collect::<Vec<_>>();
        assert!(heading_section_tasks_complete(&parsed, 0, 1));

        let source = ["# Project".to_string(), "- [ ] todo".to_string()];
        let parsed = source
            .iter()
            .map(|line| parse_render_line(line, false))
            .collect::<Vec<_>>();
        assert!(!heading_section_tasks_complete(&parsed, 0, 1));

        let source = ["# Project".to_string(), "- [] todo".to_string()];
        let parsed = source
            .iter()
            .map(|line| parse_render_line(line, false))
            .collect::<Vec<_>>();
        assert!(!heading_section_tasks_complete(&parsed, 0, 1));

        let source = ["# Project".to_string(), "Plain note".to_string()];
        let parsed = source
            .iter()
            .map(|line| parse_render_line(line, false))
            .collect::<Vec<_>>();
        assert!(!heading_section_tasks_complete(&parsed, 0, 1));
    }

    #[test]
    fn spellcheck_tokenization_skips_short_codey_and_camel_words() {
        let words = spellcheck_words("This typoooo won't flag pathLike or id42.");
        assert_eq!(
            words.iter().map(|word| word.text).collect::<Vec<_>>(),
            vec!["This", "typoooo", "won't", "flag", "pathLike", "or", "id"]
        );

        assert_eq!(
            normalized_spellcheck_word("typoooo"),
            Some("typoooo".to_string())
        );
        assert_eq!(normalized_spellcheck_word("pathLike"), None);
        assert_eq!(normalized_spellcheck_word("id42"), None);
        assert_eq!(normalized_spellcheck_word("THE"), None);
        assert_eq!(normalized_spellcheck_word("cat"), None);
    }

    #[test]
    fn table_row_preserves_editable_trailing_space() {
        assert_eq!(
            parse_table_row("| foo | bar |").unwrap()[0],
            "foo".to_string()
        );
        assert_eq!(
            parse_table_row("| foo  | bar |").unwrap()[0],
            "foo ".to_string()
        );
    }
}
