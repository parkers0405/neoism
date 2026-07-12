use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::editor::markdown::{
    parse_table_cell_bounds, source_map::InlineSourceMap, MarkdownPane,
    MarkdownWrapHitRow,
};

use super::types::{ParsedTable, TableCursorPosition, DEPTH, ORDER_BG};
use crate::editor::markdown::render::draw::{
    caret_height, cursor_cell_width, cursor_position_for_prefix, cursor_y_for_text_line,
    draw_block_chrome, draw_copy_button, draw_if_visible, draw_rect_clipped,
    draw_rounded_rect_clipped, floor_char_boundary, intersect_rect, line_height,
    markdown_font, md_font_id, point_in_rect, wrap_lines,
};
use crate::primitives::look::scrollbar_style;
use crate::editor::markdown::render::inline::{
    clean_inline_with_active_link, draw_inline_links_for_line,
};
use crate::primitives::ide_theme::IdeTheme;

const LARGE_TABLE_VIRTUALIZE_ROWS: usize = 256;

#[derive(Clone, Debug)]
pub(super) struct TableMeasurement {
    pub(super) height: f32,
    pub(super) visual_line_count: u32,
    col_widths: Vec<f32>,
    col_count: usize,
    header_row_h: f32,
    row_heights: Vec<f32>,
    min_row_h: f32,
    top_pad: f32,
    bottom_pad: f32,
}

pub(super) fn parse_table(lines: &[String], start: usize) -> Option<ParsedTable> {
    let header = parse_table_row(lines.get(start)?)?;
    if header.len() < 2 {
        return None;
    }
    let separator = parse_table_row(lines.get(start + 1)?)?;
    if !is_table_separator(&separator) {
        return None;
    }

    let mut rows = Vec::new();
    let mut ix = start + 2;
    while let Some(row) = lines.get(ix).and_then(|line| parse_table_row(line)) {
        if is_table_separator(&row) {
            break;
        }
        rows.push(row);
        ix += 1;
    }

    Some(ParsedTable {
        header,
        rows,
        end_line: ix,
    })
}

pub(super) fn parse_table_row(line: &str) -> Option<Vec<String>> {
    let bounds = parse_table_cell_bounds(line)?;
    let cells = bounds
        .iter()
        .map(|cell| line[cell.content_start..cell.content_end].to_string())
        .collect::<Vec<_>>();
    (cells.len() >= 2).then_some(cells)
}

pub(super) fn is_table_separator(cells: &[String]) -> bool {
    cells.iter().all(|cell| {
        let trimmed = cell.trim();
        trimmed.contains('-')
            && trimmed
                .chars()
                .all(|ch| matches!(ch, '-' | ':' | ' ' | '\t'))
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_table(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    table: &ParsedTable,
    source_lines: &[String],
    start_line: usize,
    cursor_line: usize,
    cursor_col: usize,
    content_x: f32,
    cursor_y: f32,
    content_w: f32,
    pane_clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
) -> f32 {
    render_table_with_source_base(
        sugarloaf,
        pane,
        table,
        source_lines,
        0,
        start_line,
        cursor_line,
        cursor_col,
        content_x,
        cursor_y,
        content_w,
        pane_clip,
        clip_top,
        clip_bottom,
        theme,
        mouse,
        text_occlusions,
        font_scale,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_table_with_source_base(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    table: &ParsedTable,
    source_lines: &[String],
    source_base_line: usize,
    start_line: usize,
    cursor_line: usize,
    cursor_col: usize,
    content_x: f32,
    cursor_y: f32,
    content_w: f32,
    pane_clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
) -> f32 {
    let header_opts = DrawOpts {
        font_size: markdown_font(16.0, font_scale),
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(pane_clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let body_opts = DrawOpts {
        font_size: markdown_font(15.0, font_scale),
        color: theme.u8_alpha(theme.fg, 0.86),
        clip_rect: Some(pane_clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let large_table = table.rows.len() > LARGE_TABLE_VIRTUALIZE_ROWS;
    let measurement =
        measure_table_with_opts(sugarloaf, table, &header_opts, &body_opts, font_scale);
    let TableMeasurement {
        height: table_h,
        col_widths,
        col_count,
        header_row_h,
        row_heights,
        min_row_h,
        top_pad,
        bottom_pad,
        ..
    } = measurement;
    let block_rect = [
        content_x - 18.0,
        cursor_y - 8.0,
        content_w + 36.0,
        table_h + 16.0,
    ];
    let handle_rect = [block_rect[0] - 36.0, block_rect[1], 34.0, block_rect[3]];
    let dragging = pane.dragging_line == Some(start_line);
    let active = pane.register_block_rect(
        start_line,
        block_rect,
        handle_rect,
        content_x,
        cursor_y + top_pad,
        0,
        cursor_cell_width(&body_opts),
        header_row_h,
        content_w,
        mouse,
    );
    let table_end_line = source_base_line + table.end_line;
    let table_active = active || (start_line..table_end_line).contains(&cursor_line);
    draw_block_chrome(
        sugarloaf,
        block_rect[0],
        block_rect[1],
        block_rect[2],
        block_rect[3],
        theme,
        pane_clip,
        clip_top,
        clip_bottom,
        active,
        table_active,
        dragging,
    );

    let copy_rect = [content_x + content_w - 30.0, cursor_y + 3.0, 24.0, 24.0];
    pane.register_copy_lines_rect(copy_rect, start_line, table_end_line);
    draw_copy_button(sugarloaf, copy_rect, theme, pane_clip, font_scale);

    let table_content_w = col_widths.iter().sum::<f32>().max(content_w);
    let table_clip = intersect_rect(pane_clip, [content_x, cursor_y, content_w, table_h])
        .unwrap_or(pane_clip);
    let max_scroll = (table_content_w - content_w).max(0.0);
    let mut scroll_x = pane.table_scroll_x(start_line).clamp(0.0, max_scroll);
    if (start_line..table_end_line).contains(&cursor_line) {
        let cursor_opts = if cursor_line == start_line {
            &header_opts
        } else {
            &body_opts
        };
        if let Some(source_x) = table_source_cursor_position(
            table,
            source_lines
                .get(cursor_line.saturating_sub(source_base_line))
                .map(String::as_str)
                .unwrap_or(""),
            cursor_line.saturating_sub(start_line),
            cursor_col,
            &col_widths,
            sugarloaf,
            cursor_opts,
        )
        .map(|position| position.x)
        {
            let margin = 48.0;
            if source_x - scroll_x < margin {
                scroll_x = (source_x - margin).clamp(0.0, max_scroll);
            } else if source_x - scroll_x > content_w - margin {
                scroll_x = (source_x - (content_w - margin)).clamp(0.0, max_scroll);
            }
            pane.set_table_scroll_x(start_line, scroll_x, content_w, table_content_w);
        }
    }
    pane.register_table_rect(start_line, block_rect, content_w, table_content_w);
    draw_table_column_insert_controls(
        sugarloaf,
        pane,
        start_line,
        col_count,
        content_x,
        cursor_y + top_pad,
        content_w,
        table_h - top_pad - bottom_pad,
        table_active || dragging,
        theme,
        pane_clip,
        font_scale,
        mouse,
    );

    let mut row_y = cursor_y + top_pad;
    draw_table_yank_flash_row(
        sugarloaf,
        pane,
        start_line,
        content_x,
        row_y,
        content_w,
        header_row_h,
        theme,
        pane_clip,
    );
    draw_table_selection_row(
        sugarloaf,
        pane,
        start_line,
        content_x,
        row_y,
        content_w,
        header_row_h,
        theme,
        pane_clip,
    );
    render_table_row(
        sugarloaf,
        pane,
        start_line,
        &table.header,
        &col_widths,
        content_x - scroll_x,
        row_y,
        header_row_h,
        &header_opts,
        theme,
        table_clip,
        clip_top,
        clip_bottom,
        text_occlusions,
    );
    draw_table_row_insert_control(
        sugarloaf,
        pane,
        start_line,
        content_x,
        row_y,
        content_w,
        header_row_h,
        table_active,
        cursor_line == start_line,
        theme,
        pane_clip,
        font_scale,
        mouse,
    );
    draw_rect_clipped(
        sugarloaf,
        pane_clip,
        content_x,
        row_y + header_row_h - 4.0,
        content_w,
        1.0,
        theme.f32_alpha(theme.border, 0.72),
        DEPTH,
        ORDER_BG + 2,
    );
    if cursor_line == start_line {
        set_table_cursor_rect(
            sugarloaf,
            pane,
            table,
            source_lines
                .get(start_line.saturating_sub(source_base_line))
                .map(String::as_str)
                .unwrap_or(""),
            0,
            cursor_col,
            &col_widths,
            content_x,
            scroll_x,
            row_y,
            header_row_h,
            &header_opts,
        );
    }
    row_y += header_row_h;

    let body_top_y = row_y;
    let (first_body_row, last_body_row) = if large_table {
        let first = ((clip_top - body_top_y) / min_row_h).floor().max(0.0) as usize;
        let last = ((clip_bottom - body_top_y) / min_row_h).ceil().max(0.0) as usize + 2;
        (
            first.saturating_sub(2).min(table.rows.len()),
            last.min(table.rows.len()),
        )
    } else {
        (0, table.rows.len())
    };
    if large_table {
        row_y = body_top_y + first_body_row as f32 * min_row_h;
    }
    for (row_ix, row) in table
        .rows
        .iter()
        .enumerate()
        .skip(first_body_row)
        .take(last_body_row.saturating_sub(first_body_row))
    {
        let row_h = row_heights[row_ix];
        let source_line = start_line + row_ix + 2;
        draw_table_yank_flash_row(
            sugarloaf,
            pane,
            source_line,
            content_x,
            row_y,
            content_w,
            row_h,
            theme,
            pane_clip,
        );
        draw_table_selection_row(
            sugarloaf,
            pane,
            source_line,
            content_x,
            row_y,
            content_w,
            row_h,
            theme,
            pane_clip,
        );
        pane.register_block_rect(
            source_line,
            [block_rect[0], row_y, block_rect[2], row_h],
            handle_rect,
            content_x,
            row_y,
            0,
            cursor_cell_width(&body_opts),
            row_h,
            content_w,
            mouse,
        );
        render_table_row(
            sugarloaf,
            pane,
            source_line,
            row,
            &col_widths,
            content_x - scroll_x,
            row_y,
            row_h,
            &body_opts,
            theme,
            table_clip,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
        draw_table_row_insert_control(
            sugarloaf,
            pane,
            source_line,
            content_x,
            row_y,
            content_w,
            row_h,
            table_active,
            cursor_line == source_line,
            theme,
            pane_clip,
            font_scale,
            mouse,
        );
        if row_ix + 1 < table.rows.len() {
            draw_rect_clipped(
                sugarloaf,
                pane_clip,
                content_x,
                row_y + row_h - 2.0,
                content_w,
                1.0,
                theme.f32_alpha(theme.border, 0.22),
                DEPTH,
                ORDER_BG + 1,
            );
        }
        if cursor_line == source_line {
            set_table_cursor_rect(
                sugarloaf,
                pane,
                table,
                source_lines
                    .get(source_line.saturating_sub(source_base_line))
                    .map(String::as_str)
                    .unwrap_or(""),
                row_ix + 2,
                cursor_col,
                &col_widths,
                content_x,
                scroll_x,
                row_y,
                row_h,
                &body_opts,
            );
        }
        row_y += row_h;
    }

    if max_scroll > 0.0 {
        // Mash Up Pack scrollbar restyle. This bar is HORIZONTAL, so
        // `width_or` maps to the thumb's thickness (height, site
        // default 7px) and `min_thumb_or` to its minimum length
        // (width, site default 48px). Track thickness stays
        // proportional (3px at the default 7px thumb) and both stay
        // vertically co-centered. Defaults reproduce today's bar
        // exactly.
        let style = scrollbar_style();
        let thumb_h = style.width_or(7.0).max(1.0);
        let track_h = thumb_h * (3.0 / 7.0);
        let min_thumb_w = style.min_thumb_or(48.0).min(content_w);
        let thumb_w =
            (content_w * content_w / table_content_w).clamp(min_thumb_w, content_w);
        let thumb_x =
            content_x + (content_w - thumb_w) * (scroll_x / max_scroll.max(1.0));
        let track_rect = [
            content_x,
            cursor_y + table_h - 1.0,
            content_w,
            thumb_h + 6.0,
        ];
        let thumb_rect = [thumb_x, cursor_y + table_h, thumb_w, thumb_h];
        pane.register_table_scrollbar_rect(
            start_line,
            track_rect,
            thumb_rect,
            content_w,
            table_content_w,
        );
        // Rounding applies to the thumb's thickness; the site default is
        // square, and radius 0 keeps the plain-rect draw call so the
        // no-override frame stays byte-identical.
        let radius = style.radius(thumb_h, 0.0);
        if let Some(track_color) =
            style.track_or(Some(theme.f32_alpha(theme.border, 0.28)))
        {
            let track_y = thumb_rect[1] + (thumb_h - track_h) * 0.5;
            if radius > 0.0 {
                draw_rounded_rect_clipped(
                    sugarloaf,
                    pane_clip,
                    content_x,
                    track_y,
                    content_w,
                    track_h,
                    style.radius(track_h, 0.0),
                    track_color,
                    DEPTH,
                    ORDER_BG + 1,
                );
            } else {
                draw_rect_clipped(
                    sugarloaf,
                    pane_clip,
                    content_x,
                    track_y,
                    content_w,
                    track_h,
                    track_color,
                    DEPTH,
                    ORDER_BG + 1,
                );
            }
        }
        let thumb_color = style.thumb_or(theme.f32_alpha(theme.fg, 0.46));
        if radius > 0.0 {
            draw_rounded_rect_clipped(
                sugarloaf,
                pane_clip,
                thumb_rect[0],
                thumb_rect[1],
                thumb_rect[2],
                thumb_rect[3],
                radius,
                thumb_color,
                DEPTH,
                ORDER_BG + 2,
            );
        } else {
            draw_rect_clipped(
                sugarloaf,
                pane_clip,
                thumb_rect[0],
                thumb_rect[1],
                thumb_rect[2],
                thumb_rect[3],
                thumb_color,
                DEPTH,
                ORDER_BG + 2,
            );
        }
    }

    cursor_y + table_h + 18.0
}

pub(super) fn table_row_height(
    sugarloaf: &mut Sugarloaf,
    row: &[String],
    col_widths: &[f32],
    opts: &DrawOpts,
    min_row_h: f32,
) -> f32 {
    let line_h = line_height(opts);
    let max_lines = col_widths
        .iter()
        .enumerate()
        .filter_map(|(ix, width)| {
            row.get(ix).map(|cell| {
                let visible = clean_inline_with_active_link(cell, None);
                wrap_lines(sugarloaf, &visible, (*width - 28.0).max(48.0), opts).len()
            })
        })
        .max()
        .unwrap_or(1);
    (line_h * max_lines.max(1) as f32 + 14.0).max(min_row_h)
}

fn measured_table_cell_hit_rows(
    sugarloaf: &mut Sugarloaf,
    rows: &[String],
    opts: &DrawOpts,
) -> Vec<MarkdownWrapHitRow> {
    let mut visible_start = 0usize;
    rows.iter()
        .map(|row| {
            let mut stops = Vec::with_capacity(row.chars().count().saturating_add(1));
            let mut prefix = String::new();
            stops.push(0.0);
            for ch in row.chars() {
                prefix.push(ch);
                stops.push(sugarloaf.text_mut().measure(&prefix, opts));
            }
            let hit_row = MarkdownWrapHitRow {
                start: visible_start,
                stops,
            };
            visible_start = visible_start
                .saturating_add(row.chars().count())
                .saturating_add(1);
            hit_row
        })
        .collect()
}

pub(super) fn measure_table(
    sugarloaf: &mut Sugarloaf,
    table: &ParsedTable,
    content_w: f32,
    theme: &IdeTheme,
    font_scale: f32,
) -> TableMeasurement {
    let clip = [0.0, 0.0, content_w.max(1.0), f32::MAX / 4.0];
    let header_opts = DrawOpts {
        font_size: markdown_font(16.0, font_scale),
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let body_opts = DrawOpts {
        font_size: markdown_font(15.0, font_scale),
        color: theme.u8_alpha(theme.fg, 0.86),
        clip_rect: Some(clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    measure_table_with_opts(sugarloaf, table, &header_opts, &body_opts, font_scale)
}

fn measure_table_with_opts(
    sugarloaf: &mut Sugarloaf,
    table: &ParsedTable,
    header_opts: &DrawOpts,
    body_opts: &DrawOpts,
    font_scale: f32,
) -> TableMeasurement {
    let top_pad = 16.0;
    let bottom_pad = 14.0;
    let col_count = table
        .rows
        .iter()
        .map(Vec::len)
        .chain(std::iter::once(table.header.len()))
        .max()
        .unwrap_or(0);
    let mut col_widths = vec![116.0_f32; col_count];
    let large_table = table.rows.len() > LARGE_TABLE_VIRTUALIZE_ROWS;
    for (ix, cell) in table.header.iter().enumerate() {
        let visible = clean_inline_with_active_link(cell, None);
        col_widths[ix] = col_widths[ix]
            .max(sugarloaf.text_mut().measure(&visible, header_opts) + 54.0);
    }
    for row in table.rows.iter().take(if large_table {
        LARGE_TABLE_VIRTUALIZE_ROWS
    } else {
        usize::MAX
    }) {
        for (ix, cell) in row.iter().enumerate() {
            let visible = clean_inline_with_active_link(cell, None);
            col_widths[ix] = col_widths[ix]
                .max(sugarloaf.text_mut().measure(&visible, body_opts) + 54.0);
        }
    }
    for width in &mut col_widths {
        *width = (*width).clamp(116.0, 440.0);
    }
    let min_row_h = (line_height(body_opts) + 12.0).max(38.0 * font_scale.min(1.4));
    let header_row_h = table_row_height(
        sugarloaf,
        &table.header,
        &col_widths,
        header_opts,
        min_row_h,
    );
    let row_heights = if large_table {
        vec![min_row_h; table.rows.len()]
    } else {
        table
            .rows
            .iter()
            .map(|row| {
                table_row_height(sugarloaf, row, &col_widths, body_opts, min_row_h)
            })
            .collect::<Vec<_>>()
    };
    let height = top_pad + header_row_h + row_heights.iter().sum::<f32>() + bottom_pad;
    TableMeasurement {
        height,
        visual_line_count: (1 + table.rows.len()).max(1) as u32,
        col_widths,
        col_count,
        header_row_h,
        row_heights,
        min_row_h,
        top_pad,
        bottom_pad,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_table_selection_row(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    line_ix: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    theme: &IdeTheme,
    clip: [f32; 4],
) {
    if pane.selection_for_line(line_ix).is_some() {
        draw_rect_clipped(
            sugarloaf,
            clip,
            x,
            y + 2.0,
            w,
            (h - 4.0).max(8.0),
            theme.f32_alpha(theme.accent, 0.22),
            DEPTH,
            ORDER_BG + 3,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_table_yank_flash_row(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    line_ix: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    theme: &IdeTheme,
    clip: [f32; 4],
) {
    if let Some((_, _, alpha)) = pane.yank_flash_for_line(line_ix) {
        if alpha <= 0.001 {
            return;
        }
        draw_rect_clipped(
            sugarloaf,
            clip,
            x,
            y + 2.0,
            w,
            (h - 4.0).max(8.0),
            theme.f32_alpha(theme.yellow, alpha),
            DEPTH,
            ORDER_BG + 4,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_table_row_insert_control(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    after_line: usize,
    content_x: f32,
    row_y: f32,
    content_w: f32,
    row_h: f32,
    table_active: bool,
    row_has_cursor: bool,
    theme: &IdeTheme,
    clip: [f32; 4],
    font_scale: f32,
    mouse: Option<[f32; 2]>,
) {
    if !table_active {
        return;
    }
    let row_rect = [content_x, row_y, content_w, row_h];
    let mouse_in_row = mouse.is_some_and(|[x, y]| point_in_rect(x, y, row_rect));
    if !(row_has_cursor || mouse_in_row) {
        return;
    }

    let button_rect = [content_x - 12.0, row_y + row_h - 11.0, 22.0, 20.0];
    let hovered = pane.register_table_add_row_rect(after_line, button_rect, mouse);
    draw_rect_clipped(
        sugarloaf,
        clip,
        content_x + 4.0,
        row_y + row_h - 1.5,
        (content_w - 8.0).max(0.0),
        1.5,
        theme.f32_alpha(theme.accent, if hovered { 0.72 } else { 0.32 }),
        DEPTH,
        ORDER_BG + 4,
    );
    draw_table_action_button(
        sugarloaf,
        button_rect,
        "+",
        hovered,
        theme,
        clip,
        font_scale,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_table_column_insert_controls(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    start_line: usize,
    col_count: usize,
    content_x: f32,
    table_y: f32,
    content_w: f32,
    table_h: f32,
    table_active: bool,
    theme: &IdeTheme,
    clip: [f32; 4],
    font_scale: f32,
    mouse: Option<[f32; 2]>,
) {
    if !table_active || col_count == 0 {
        return;
    }

    let left_rect = [content_x - 12.0, table_y + 8.0, 22.0, 20.0];
    let right_rect = [content_x + content_w - 10.0, table_y + 8.0, 22.0, 20.0];
    let left_hovered =
        pane.register_table_add_column_rect(start_line, 0, left_rect, mouse);
    let right_hovered =
        pane.register_table_add_column_rect(start_line, col_count, right_rect, mouse);
    if left_hovered {
        draw_rect_clipped(
            sugarloaf,
            clip,
            content_x,
            table_y + 3.0,
            1.5,
            (table_h - 6.0).max(8.0),
            theme.f32_alpha(theme.accent, 0.72),
            DEPTH,
            ORDER_BG + 4,
        );
    }
    if right_hovered {
        draw_rect_clipped(
            sugarloaf,
            clip,
            content_x + content_w - 1.5,
            table_y + 3.0,
            1.5,
            (table_h - 6.0).max(8.0),
            theme.f32_alpha(theme.accent, 0.72),
            DEPTH,
            ORDER_BG + 4,
        );
    }
    draw_table_action_button(
        sugarloaf,
        left_rect,
        "+",
        left_hovered,
        theme,
        clip,
        font_scale,
    );
    draw_table_action_button(
        sugarloaf,
        right_rect,
        "+",
        right_hovered,
        theme,
        clip,
        font_scale,
    );
}

pub(super) fn draw_table_action_button(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    icon: &str,
    hovered: bool,
    theme: &IdeTheme,
    clip: [f32; 4],
    font_scale: f32,
) {
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        rect[0],
        rect[1],
        rect[2],
        rect[3],
        6.0,
        if hovered {
            theme.f32_alpha(theme.accent, 0.95)
        } else {
            theme.f32_alpha(theme.hover, 0.82)
        },
        DEPTH,
        ORDER_BG + 5,
    );
    let opts = DrawOpts {
        font_size: markdown_font(12.0, font_scale),
        color: if hovered {
            theme.u8(theme.bg)
        } else {
            theme.u8_alpha(theme.fg, 0.82)
        },
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let icon_w = sugarloaf.text_mut().measure(icon, &opts);
    sugarloaf.text_mut().draw(
        rect[0] + ((rect[2] - icon_w) * 0.5).max(3.0),
        rect[1] + 3.0,
        icon,
        &opts,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_table_row(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    source_line: usize,
    row: &[String],
    col_widths: &[f32],
    x: f32,
    y: f32,
    row_h: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    text_occlusions: &[[f32; 4]],
) {
    let mut cell_x = x;
    let mut clipped_opts = opts.clone();
    clipped_opts.clip_rect = Some(clip);
    let line_h = line_height(opts);
    for (ix, width) in col_widths.iter().enumerate() {
        if let Some(cell) = row.get(ix) {
            let wrap_width = (*width - 28.0).max(48.0);
            let visible = clean_inline_with_active_link(cell, None);
            let wrapped = wrap_lines(sugarloaf, &visible, wrap_width, opts);
            let text_h = line_h * wrapped.len().max(1) as f32;
            let mut text_y = y + ((row_h - text_h) * 0.5).max(7.0);
            let cell_text_y = text_y;
            if let Some(hit_rect) = intersect_rect(clip, [cell_x, y, *width, row_h]) {
                let hit_rows = measured_table_cell_hit_rows(sugarloaf, &wrapped, opts);
                pane.register_table_cell_rect(
                    source_line,
                    ix,
                    hit_rect,
                    cell_x + 16.0,
                    text_y,
                    (*width - 28.0).max(48.0),
                    cursor_cell_width(opts),
                    line_h,
                    hit_rows,
                );
            }
            for rendered in wrapped {
                draw_if_visible(
                    sugarloaf,
                    cell_x + 16.0,
                    text_y,
                    &rendered,
                    &clipped_opts,
                    clip_top,
                    clip_bottom,
                    text_occlusions,
                );
                text_y += line_h;
            }
            draw_inline_links_for_line(
                sugarloaf,
                pane,
                cell,
                cell_x + 16.0,
                cell_text_y,
                0,
                line_h,
                wrap_width,
                opts,
                theme,
                clip,
                clip_top,
                clip_bottom,
                text_occlusions,
                None,
            );
        }
        cell_x += *width;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn set_table_cursor_rect(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    table: &ParsedTable,
    source_line: &str,
    table_row_ix: usize,
    cursor_col: usize,
    col_widths: &[f32],
    content_x: f32,
    scroll_x: f32,
    y: f32,
    row_h: f32,
    opts: &DrawOpts,
) {
    let Some(position) = table_source_cursor_position(
        table,
        source_line,
        table_row_ix,
        cursor_col,
        col_widths,
        sugarloaf,
        opts,
    ) else {
        return;
    };
    let Some(row) = table_row_for_ix(table, table_row_ix) else {
        return;
    };
    let cell_text = row.get(position.cell_ix).map(String::as_str).unwrap_or("");
    let cell_width = col_widths.get(position.cell_ix).copied().unwrap_or(116.0);
    let line_h = line_height(opts);
    let wrapped = wrap_lines(sugarloaf, cell_text, (cell_width - 28.0).max(48.0), opts);
    let text_h = line_h * wrapped.len().max(1) as f32;
    let text_y = y + ((row_h - text_h) * 0.5).max(7.0);
    let caret_h = caret_height(opts);
    pane.set_cursor_rect(Some([
        content_x + position.x - scroll_x,
        cursor_y_for_text_line(text_y + position.visual_line as f32 * line_h, opts),
        cursor_cell_width(opts),
        caret_h,
    ]));
}

pub(super) fn table_source_cursor_position(
    table: &ParsedTable,
    source_line: &str,
    table_row_ix: usize,
    cursor_col: usize,
    col_widths: &[f32],
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> Option<TableCursorPosition> {
    let row = table_row_for_ix(table, table_row_ix)?;
    let line_h = line_height(opts);
    if let Some(bounds) = parse_table_cell_bounds(source_line) {
        let source_col =
            floor_char_boundary(source_line, cursor_col.min(source_line.len()));
        let mut x = 0.0;
        for (ix, width) in col_widths.iter().enumerate() {
            let Some(cell_bounds) = bounds.get(ix).copied() else {
                break;
            };
            if source_col <= cell_bounds.raw_end || ix + 1 == col_widths.len() {
                let cell_col =
                    source_col.clamp(cell_bounds.content_start, cell_bounds.content_end);
                let cell_source =
                    &source_line[cell_bounds.content_start..cell_bounds.content_end];
                let map = InlineSourceMap::new(cell_source);
                let prefix = map.visible_prefix(
                    map.visible_for_source(cell_col - cell_bounds.content_start),
                );
                let (prefix_x, prefix_y) = cursor_position_for_prefix(
                    sugarloaf,
                    0.0,
                    0.0,
                    line_h,
                    (*width - 28.0).max(48.0),
                    opts,
                    &prefix,
                );
                return Some(TableCursorPosition {
                    x: x + 16.0 + prefix_x,
                    visual_line: (prefix_y / line_h.max(1.0)).round().max(0.0) as usize,
                    cell_ix: ix,
                });
            }
            x += *width;
        }
    }

    let mut remaining = cursor_col;
    let mut x = 0.0;
    for (ix, width) in col_widths.iter().enumerate() {
        let cell = row.get(ix).map(String::as_str).unwrap_or("");
        let cell_len = cell.len();
        if remaining <= cell_len || ix + 1 == col_widths.len() {
            let cell_col = floor_char_boundary(cell, remaining.min(cell_len));
            let map = InlineSourceMap::new(cell);
            let prefix = map.visible_prefix(map.visible_for_source(cell_col));
            let (prefix_x, prefix_y) = cursor_position_for_prefix(
                sugarloaf,
                0.0,
                0.0,
                line_h,
                (*width - 28.0).max(48.0),
                opts,
                &prefix,
            );
            return Some(TableCursorPosition {
                x: x + 16.0 + prefix_x,
                visual_line: (prefix_y / line_h.max(1.0)).round().max(0.0) as usize,
                cell_ix: ix,
            });
        }
        remaining = remaining.saturating_sub(cell_len + 3);
        x += *width;
    }
    Some(TableCursorPosition {
        x,
        visual_line: 0,
        cell_ix: 0,
    })
}

pub(super) fn table_row_for_ix(
    table: &ParsedTable,
    table_row_ix: usize,
) -> Option<&[String]> {
    if table_row_ix == 0 {
        Some(&table.header)
    } else if table_row_ix >= 2 {
        table.rows.get(table_row_ix - 2).map(Vec::as_slice)
    } else {
        None
    }
}
