use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::editor::markdown::{
    parse_table_cell_bounds, source_map::InlineSourceMap, MarkdownPane,
    MarkdownWrapHitRow,
};

use super::types::{ParsedTable, TableCursorPosition, DEPTH, ORDER_BG};
use crate::editor::markdown::render::draw::{
    caret_height, cursor_cell_width, cursor_y_for_text_line, draw_copy_button,
    draw_if_visible, draw_rect_clipped, draw_rounded_rect_clipped, floor_char_boundary,
    intersect_rect, line_height, markdown_font, md_font_id, point_in_rect,
};
use crate::editor::markdown::render::inline::draw_spellcheck_underlines;
use crate::primitives::ide_theme::IdeTheme;
use crate::primitives::look::scrollbar_style;

#[derive(Clone, Debug)]
pub(super) struct TableMeasurement {
    pub(super) height: f32,
    pub(super) visual_line_count: u32,
    col_widths: Vec<f32>,
    col_count: usize,
    header_row_h: f32,
    row_heights: Vec<f32>,
    top_pad: f32,
    bottom_pad: f32,
}

pub(super) fn parse_table(lines: &[String], start: usize) -> Option<ParsedTable> {
    let header = parse_table_row(lines.get(start)?)?;
    if header.is_empty() {
        return None;
    }
    let separator = parse_table_row(lines.get(start + 1)?)?;
    if separator.len() != header.len() || !is_table_separator(&separator) {
        return None;
    }

    let mut rows = Vec::new();
    let mut ix = start + 2;
    while let Some(row) = lines.get(ix).and_then(|line| parse_table_row(line)) {
        rows.push(row);
        ix += 1;
    }

    Some(ParsedTable {
        alignments: separator
            .iter()
            .map(|cell| {
                if cell.ends_with(':') {
                    if cell.starts_with(':') {
                        0.5
                    } else {
                        1.0
                    }
                } else {
                    0.0
                }
            })
            .collect(),
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
    (!cells.is_empty()).then_some(cells)
}

pub(super) fn is_table_separator(cells: &[String]) -> bool {
    crate::widgets::markdown::is_table_separator_trimmed(cells)
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
    let text_clip = intersect_rect(
        pane_clip,
        [content_x, pane_clip[1], content_w, pane_clip[3]],
    )
    .unwrap_or([content_x, pane_clip[1], 0.0, 0.0]);
    let header_opts = DrawOpts {
        font_size: markdown_font(16.0, font_scale),
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(text_clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let body_opts = DrawOpts {
        font_size: markdown_font(15.0, font_scale),
        color: theme.u8_alpha(theme.fg, 0.86),
        clip_rect: Some(text_clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let measurement = measure_table_with_opts(
        sugarloaf,
        pane,
        start_line,
        table,
        content_w,
        &header_opts,
        &body_opts,
        font_scale,
    );
    let TableMeasurement {
        height: table_h,
        col_widths,
        col_count,
        header_row_h,
        row_heights,
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
    let handle_rect = super::super::helpers::block_handle_rect(block_rect);
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
    let edge_hovered = mouse.is_some_and(|[x, y]| {
        point_in_rect(
            x,
            y,
            [content_x - 30.0, cursor_y, content_w + 56.0, table_h + 8.0],
        )
    });
    let table_active = !pane.read_only
        && pane.mode == crate::editor::markdown::MarkdownMode::Insert
        && (active
            || edge_hovered
            || (start_line..table_end_line).contains(&cursor_line));
    if active || dragging {
        super::draw::draw_block_actions(
            sugarloaf, block_rect, theme, pane_clip, dragging,
        );
    }

    if active {
        let copy_rect = [content_x + content_w - 24.0, cursor_y - 8.0, 22.0, 22.0];
        pane.register_copy_lines_rect(copy_rect, start_line, table_end_line);
        draw_copy_button(sugarloaf, copy_rect, theme, pane_clip, font_scale);
    }

    let table_content_w = col_widths.iter().sum::<f32>().max(content_w);
    let table_clip = intersect_rect(pane_clip, [content_x, cursor_y, content_w, table_h])
        .unwrap_or(pane_clip);
    let max_scroll = (table_content_w - content_w).max(0.0);
    let mut scroll_x = pane.table_scroll_x(start_line).clamp(0.0, max_scroll);
    if pane.follow_cursor && (start_line..table_end_line).contains(&cursor_line) {
        let cursor_opts = if cursor_line == start_line {
            &header_opts
        } else {
            &body_opts
        };
        if let Some(source_x) = table_source_cursor_position(
            table,
            pane,
            source_lines
                .get(cursor_line.saturating_sub(source_base_line))
                .map(String::as_str)
                .unwrap_or(""),
            cursor_line.saturating_sub(start_line),
            cursor_col,
            &col_widths,
            sugarloaf,
            cursor_opts,
        ) {
            scroll_x = super::table_layout::reveal_column(
                scroll_x,
                content_w,
                &col_widths,
                source_x.cell_ix,
                source_x.x,
            );
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

    let grid_y = cursor_y + top_pad;
    let grid_h = table_h - top_pad - bottom_pad;
    draw_rect_clipped(
        sugarloaf,
        table_clip,
        content_x,
        grid_y,
        content_w,
        header_row_h,
        theme.f32_alpha(theme.surface, 0.45),
        DEPTH,
        ORDER_BG,
    );
    for border_y in [grid_y, grid_y + grid_h] {
        draw_rect_clipped(
            sugarloaf,
            pane_clip,
            content_x,
            border_y,
            content_w,
            1.0,
            theme.f32_alpha(theme.border, 0.65),
            DEPTH,
            ORDER_BG + 1,
        );
    }
    for edge in [content_x, content_x + content_w - 1.0] {
        draw_rect_clipped(
            sugarloaf,
            table_clip,
            edge,
            grid_y,
            1.0,
            grid_h,
            theme.f32_alpha(theme.border, 0.5),
            DEPTH,
            ORDER_BG + 1,
        );
    }
    let mut boundary_x = content_x - scroll_x;
    for width in std::iter::once(0.0).chain(col_widths.iter().copied()) {
        boundary_x += width;
        if boundary_x > content_x && boundary_x < content_x + content_w - 1.0 {
            draw_rect_clipped(
                sugarloaf,
                table_clip,
                boundary_x,
                grid_y,
                1.0,
                grid_h,
                theme.f32_alpha(theme.border, 0.5),
                DEPTH,
                ORDER_BG + 1,
            );
        }
    }
    if table_active && !pane.read_only {
        let append = [content_x, grid_y + grid_h + 4.0, content_w, 20.0];
        let after = if table.rows.is_empty() {
            start_line
        } else {
            table_end_line.saturating_sub(1)
        };
        let hovered = pane.register_table_add_row_rect(after, append, mouse);
        draw_table_action_button(
            sugarloaf, append, "+", hovered, theme, pane_clip, font_scale,
        );
    }
    if !pane.read_only {
        let mut column_x = content_x - scroll_x;
        for (col_ix, width) in col_widths.iter().copied().enumerate() {
            let hover_region = [column_x, grid_y - 20.0, width, header_row_h + 20.0];
            if mouse.is_some_and(|[x, y]| point_in_rect(x, y, hover_region)) {
                let left = column_x.max(content_x);
                let right = (column_x + width).min(content_x + content_w);
                let menu = [(left + right) * 0.5 - 12.0, grid_y - 18.0, 24.0, 18.0];
                if let Some(rect) = intersect_rect(
                    menu,
                    [content_x, pane_clip[1], content_w, pane_clip[3]],
                ) {
                    let hovered = pane.register_table_action_rect(
                        crate::editor::markdown::MarkdownTableAction::ColumnMenu {
                            start_line,
                            col_ix,
                        },
                        rect,
                        mouse,
                    );
                    draw_table_action_button(
                        sugarloaf, rect, "...", hovered, theme, pane_clip, font_scale,
                    );
                }
            }
            column_x += width;
        }
    }
    let mut row_y = grid_y;
    draw_table_yank_flash_row(
        sugarloaf,
        pane,
        start_line,
        source_lines
            .get(start_line.saturating_sub(source_base_line))
            .map(String::as_str)
            .unwrap_or(""),
        &table.header,
        &col_widths,
        &table.alignments,
        content_x - scroll_x,
        row_y,
        header_row_h,
        &header_opts,
        theme,
        table_clip,
        clip_top,
        clip_bottom,
    );
    draw_table_selection_row(
        sugarloaf,
        pane,
        start_line,
        source_lines
            .get(start_line.saturating_sub(source_base_line))
            .map(String::as_str)
            .unwrap_or(""),
        &table.header,
        &col_widths,
        &table.alignments,
        content_x - scroll_x,
        row_y,
        header_row_h,
        &header_opts,
        theme,
        table_clip,
        clip_top,
        clip_bottom,
    );
    render_table_row(
        sugarloaf,
        pane,
        start_line,
        &table.header,
        &col_widths,
        &table.alignments,
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
        row_y + header_row_h,
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
    let mut offsets = Vec::with_capacity(row_heights.len() + 1);
    offsets.push(body_top_y);
    for height in &row_heights {
        offsets.push(offsets.last().copied().unwrap() + height);
    }
    let first_body_row = offsets
        .partition_point(|end| *end < clip_top)
        .saturating_sub(1)
        .min(table.rows.len());
    let last_body_row = offsets
        .partition_point(|start| *start <= clip_bottom)
        .min(table.rows.len());
    row_y = offsets[first_body_row];
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
            source_lines
                .get(source_line.saturating_sub(source_base_line))
                .map(String::as_str)
                .unwrap_or(""),
            row,
            &col_widths,
            &table.alignments,
            content_x - scroll_x,
            row_y,
            row_h,
            &body_opts,
            theme,
            table_clip,
            clip_top,
            clip_bottom,
        );
        draw_table_selection_row(
            sugarloaf,
            pane,
            source_line,
            source_lines
                .get(source_line.saturating_sub(source_base_line))
                .map(String::as_str)
                .unwrap_or(""),
            row,
            &col_widths,
            &table.alignments,
            content_x - scroll_x,
            row_y,
            row_h,
            &body_opts,
            theme,
            table_clip,
            clip_top,
            clip_bottom,
        );
        pane.register_block_rect(
            source_line,
            [block_rect[0], row_y, block_rect[2], row_h],
            [-1_000_000.0, -1_000_000.0, 0.0, 0.0],
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
            &table.alignments,
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
                row_y + row_h,
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
        let thumb_y = cursor_y + table_h - thumb_h - 4.0;
        let track_rect = [content_x, thumb_y - 3.0, content_w, thumb_h + 6.0];
        let thumb_rect = [thumb_x, thumb_y, thumb_w, thumb_h];
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
    pane: &MarkdownPane,
    source_line: usize,
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
                let visible = pane.table_source_map(source_line, ix, cell).visible_text();
                super::table_layout::wrap_cell(
                    &visible,
                    (*width - 32.0).max(16.0),
                    |text| sugarloaf.text_mut().measure(text, opts),
                )
                .len()
            })
        })
        .max()
        .unwrap_or(1);
    (line_h * max_lines.max(1) as f32 + 14.0).max(min_row_h)
}

#[derive(Clone, Debug)]
struct MeasuredCellRow {
    text: String,
    hit: MarkdownWrapHitRow,
}

fn measured_table_cell_rows(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    width: f32,
    opts: &DrawOpts,
    alignment: f32,
) -> Vec<MeasuredCellRow> {
    super::table_layout::wrap_cell(text, width, |value| {
        sugarloaf.text_mut().measure(value, opts)
    })
    .into_iter()
    .map(|row| {
        let text: String = text.chars().skip(row.start).take(row.len).collect();
        let mut stops = vec![0.0];
        let mut prefix = String::new();
        for ch in text.chars() {
            prefix.push(ch);
            stops.push(sugarloaf.text_mut().measure(&prefix, opts));
        }
        let offset = (width - stops.last().copied().unwrap_or(0.0)).max(0.0) * alignment;
        for stop in &mut stops {
            *stop += offset;
        }
        MeasuredCellRow {
            text,
            hit: MarkdownWrapHitRow {
                start: row.start,
                stops,
            },
        }
    })
    .collect()
}

pub(super) fn measure_table(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    start_line: usize,
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
    measure_table_with_opts(
        sugarloaf,
        pane,
        start_line,
        table,
        content_w,
        &header_opts,
        &body_opts,
        font_scale,
    )
}

fn measure_table_with_opts(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    start_line: usize,
    table: &ParsedTable,
    content_w: f32,
    header_opts: &DrawOpts,
    body_opts: &DrawOpts,
    font_scale: f32,
) -> TableMeasurement {
    let top_pad = 16.0;
    let col_count = table
        .rows
        .iter()
        .map(Vec::len)
        .chain(std::iter::once(table.header.len()))
        .max()
        .unwrap_or(0);
    let max_width = (content_w - 24.0).clamp(48.0, 440.0);
    let min_width = 116.0_f32.min(max_width);
    let mut col_widths = vec![min_width; col_count];
    for (ix, cell) in table.header.iter().enumerate() {
        let visible = InlineSourceMap::for_table(cell).visible_text();
        let natural = visible
            .lines()
            .map(|text| sugarloaf.text_mut().measure(text, header_opts))
            .fold(0.0_f32, f32::max);
        col_widths[ix] = col_widths[ix].max(natural + 32.0).min(max_width);
    }
    for row in &table.rows {
        for (ix, cell) in row.iter().enumerate() {
            if col_widths[ix] >= max_width {
                continue;
            }
            let visible = InlineSourceMap::for_table(cell).visible_text();
            let natural = visible
                .lines()
                .map(|text| sugarloaf.text_mut().measure(text, body_opts))
                .fold(0.0_f32, f32::max);
            col_widths[ix] = col_widths[ix].max(natural + 32.0).min(max_width);
        }
    }
    let spare =
        (content_w - col_widths.iter().sum::<f32>()).max(0.0) / col_count.max(1) as f32;
    for width in &mut col_widths {
        *width += spare;
    }
    let bottom_pad = if col_widths.iter().sum::<f32>() > content_w + 0.5 {
        44.0
    } else {
        28.0
    };
    let min_row_h = (line_height(body_opts) + 12.0).max(38.0 * font_scale.min(1.4));
    let header_row_h = table_row_height(
        sugarloaf,
        pane,
        start_line,
        &table.header,
        &col_widths,
        header_opts,
        min_row_h,
    );
    let row_heights = table
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            table_row_height(
                sugarloaf,
                pane,
                start_line + index + 2,
                row,
                &col_widths,
                body_opts,
                min_row_h,
            )
        })
        .collect::<Vec<_>>();
    let height = top_pad + header_row_h + row_heights.iter().sum::<f32>() + bottom_pad;
    TableMeasurement {
        height,
        visual_line_count: (1 + table.rows.len()).max(1) as u32,
        col_widths,
        col_count,
        header_row_h,
        row_heights,
        top_pad,
        bottom_pad,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_table_selection_row(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    line_ix: usize,
    source_line: &str,
    row: &[String],
    col_widths: &[f32],
    alignments: &[f32],
    x: f32,
    y: f32,
    h: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
) {
    if let Some((raw_start, raw_end)) = pane.selection_for_line(line_ix) {
        draw_table_text_range_highlight(
            sugarloaf,
            pane,
            line_ix,
            source_line,
            row,
            col_widths,
            alignments,
            raw_start,
            raw_end,
            x,
            y,
            h,
            opts,
            theme.f32_alpha(theme.accent, 0.22),
            clip,
            clip_top,
            clip_bottom,
            ORDER_BG + 3,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_table_yank_flash_row(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    line_ix: usize,
    source_line: &str,
    row: &[String],
    col_widths: &[f32],
    alignments: &[f32],
    x: f32,
    y: f32,
    h: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
) {
    if let Some((raw_start, raw_end, alpha)) = pane.yank_flash_for_line(line_ix) {
        if alpha <= 0.001 {
            return;
        }
        draw_table_text_range_highlight(
            sugarloaf,
            pane,
            line_ix,
            source_line,
            row,
            col_widths,
            alignments,
            raw_start,
            raw_end,
            x,
            y,
            h,
            opts,
            theme.f32_alpha(theme.yellow, alpha),
            clip,
            clip_top,
            clip_bottom,
            ORDER_BG + 4,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_table_text_range_highlight(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    source_index: usize,
    source_line: &str,
    row: &[String],
    col_widths: &[f32],
    alignments: &[f32],
    raw_start: usize,
    raw_end: usize,
    x: f32,
    y: f32,
    row_h: f32,
    opts: &DrawOpts,
    color: [f32; 4],
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    order: u8,
) {
    let Some(bounds) = parse_table_cell_bounds(source_line) else {
        return;
    };
    let line_h = line_height(opts);
    let mut cell_x = x;
    for (ix, width) in col_widths.iter().enumerate() {
        let Some(cell_bounds) = bounds.get(ix) else {
            break;
        };
        let Some(cell) = row.get(ix) else {
            cell_x += *width;
            continue;
        };
        let start = raw_start
            .max(cell_bounds.content_start)
            .min(cell_bounds.content_end);
        let end = raw_end
            .max(cell_bounds.content_start)
            .min(cell_bounds.content_end);
        if start < end {
            let map = pane.table_source_map(source_index, ix, cell);
            let start = map.visible_for_source(start - cell_bounds.content_start);
            let end = map.visible_for_source(end - cell_bounds.content_start);
            let wrapped = measured_table_cell_rows(
                sugarloaf,
                &map.visible_text(),
                (*width - 32.0).max(16.0),
                opts,
                alignments.get(ix).copied().unwrap_or(0.0),
            );
            for (index, row) in wrapped.iter().enumerate() {
                let len = row.hit.stops.len().saturating_sub(1);
                let a = start.saturating_sub(row.hit.start).min(len);
                let b = end.saturating_sub(row.hit.start).min(len);
                let text_y = y + 7.0 + index as f32 * line_h;
                if a < b && text_y + line_h >= clip_top && text_y <= clip_bottom {
                    if let Some(cell_clip) =
                        intersect_rect(clip, [cell_x, y, *width, row_h])
                    {
                        draw_rect_clipped(
                            sugarloaf,
                            cell_clip,
                            cell_x + 16.0 + row.hit.stops[a],
                            text_y,
                            row.hit.stops[b] - row.hit.stops[a],
                            line_h,
                            color,
                            DEPTH,
                            order,
                        );
                    }
                }
            }
        }
        cell_x += *width;
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
    if !table_active || pane.read_only {
        return;
    }
    let _ = (row_has_cursor, content_w);
    let button_rect = [content_x - 24.0, row_y + row_h - 9.0, 18.0, 18.0];
    let boundary_gutter = [content_x - 30.0, row_y + row_h - 12.0, 32.0, 24.0];
    if !mouse.is_some_and(|[x, y]| point_in_rect(x, y, boundary_gutter)) {
        return;
    }
    let hovered = pane.register_table_add_row_rect(after_line, button_rect, mouse);
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
    if !table_active || pane.read_only || col_count == 0 {
        return;
    }
    let right_rect = [content_x + content_w + 4.0, table_y, 20.0, table_h];
    let hovered =
        pane.register_table_add_column_rect(start_line, col_count, right_rect, mouse);
    draw_table_action_button(
        sugarloaf, right_rect, "+", hovered, theme, clip, font_scale,
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
            theme.f32_alpha(theme.hover, 0.7)
        } else {
            theme.f32_alpha(theme.surface, 0.35)
        },
        DEPTH,
        ORDER_BG + 5,
    );
    if matches!(icon, "+" | "-") {
        let color = theme.f32(if hovered { theme.fg } else { theme.muted });
        let cx = rect[0] + rect[2] * 0.5;
        let cy = rect[1] + rect[3] * 0.5;
        draw_rect_clipped(
            sugarloaf,
            clip,
            cx - 4.0,
            cy - 0.75,
            8.0,
            1.5,
            color,
            DEPTH,
            ORDER_BG + 6,
        );
        if icon == "+" {
            draw_rect_clipped(
                sugarloaf,
                clip,
                cx - 0.75,
                cy - 4.0,
                1.5,
                8.0,
                color,
                DEPTH,
                ORDER_BG + 6,
            );
        }
        return;
    }
    if icon == "..." {
        let diameter = 2.5_f32.min(rect[3] * 0.2);
        let color = theme.f32(if hovered { theme.fg } else { theme.muted });
        for offset in [-5.0, 0.0, 5.0] {
            draw_rounded_rect_clipped(
                sugarloaf,
                clip,
                rect[0] + rect[2] * 0.5 + offset - diameter * 0.5,
                rect[1] + (rect[3] - diameter) * 0.5,
                diameter,
                diameter,
                diameter * 0.5,
                color,
                DEPTH,
                ORDER_BG + 6,
            );
        }
        return;
    }
    let opts = DrawOpts {
        font_size: markdown_font(12.0, font_scale),
        color: if hovered {
            theme.u8(theme.fg)
        } else {
            theme.u8(theme.muted)
        },
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let icon_w = sugarloaf.text_mut().measure(icon, &opts);
    sugarloaf.text_mut().draw(
        rect[0] + ((rect[2] - icon_w) * 0.5).max(3.0),
        rect[1] + (rect[3] - opts.font_size) * 0.5,
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
    alignments: &[f32],
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
            let Some(cell_clip) = intersect_rect(clip, [cell_x, y, *width, row_h]) else {
                cell_x += *width;
                continue;
            };
            clipped_opts.clip_rect = Some(cell_clip);
            let wrap_width = (*width - 32.0).max(16.0);
            let map = pane.table_source_map(source_line, ix, cell);
            let visible = map.visible_text();
            let wrapped = measured_table_cell_rows(
                sugarloaf,
                &visible,
                wrap_width,
                opts,
                alignments.get(ix).copied().unwrap_or(0.0),
            );
            let mut text_y = y + 7.0;
            let cell_text_y = text_y;
            if let Some(hit_rect) = intersect_rect(clip, [cell_x, y, *width, row_h]) {
                let hit_rows = wrapped.iter().map(|row| row.hit.clone()).collect();
                pane.register_table_cell_rect(
                    source_line,
                    ix,
                    hit_rect,
                    cell_x + 16.0,
                    text_y,
                    (*width - 32.0).max(16.0),
                    cursor_cell_width(opts),
                    line_h,
                    hit_rows,
                );
            }
            for rendered in &wrapped {
                let text_x =
                    cell_x + 16.0 + rendered.hit.stops.first().copied().unwrap_or(0.0);
                draw_if_visible(
                    sugarloaf,
                    text_x,
                    text_y,
                    &rendered.text,
                    &clipped_opts,
                    clip_top,
                    clip_bottom,
                    text_occlusions,
                );
                if pane.spellcheck_enabled {
                    draw_spellcheck_underlines(
                        sugarloaf,
                        text_x,
                        text_y,
                        line_h,
                        wrap_width,
                        &clipped_opts,
                        theme,
                        clip,
                        clip_top,
                        clip_bottom,
                        text_occlusions,
                        &rendered.text,
                    );
                }
                text_y += line_h;
            }
            draw_table_cell_links(
                sugarloaf,
                pane,
                cell,
                &map,
                &wrapped,
                cell_x + 16.0,
                cell_text_y,
                opts,
                theme,
                cell_clip,
                text_occlusions,
            );
        }
        cell_x += *width;
    }
}

fn draw_table_cell_links(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    source: &str,
    map: &InlineSourceMap,
    rows: &[MeasuredCellRow],
    x: f32,
    y: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    occlusions: &[[f32; 4]],
) {
    use crate::widgets::markdown::{parse_markdown_link, web_link_at_start};
    let mut offset = 0;
    while offset < source.len() {
        let rest = &source[offset..];
        if rest.starts_with('\\') {
            offset += 1;
            offset += source[offset..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0);
            continue;
        }
        if rest.starts_with('`') {
            let ticks = rest.bytes().take_while(|b| *b == b'`').count();
            if let Some(end) = rest[ticks..].find(&rest[..ticks]) {
                offset += ticks + end + ticks;
                continue;
            }
        }
        let link = if let Some(inner) = rest
            .strip_prefix("[[")
            .and_then(|rest| rest.split_once("]]"))
        {
            Some((inner.0.to_string(), 0, inner.0.len() + 4, inner.0.len() + 4))
        } else if let Some(link) = parse_markdown_link(rest) {
            Some((
                link.target.to_string(),
                1,
                1 + link.label.len(),
                link.consumed,
            ))
        } else {
            web_link_at_start(rest)
                .map(|link| (link.target, link.label_start, link.label_end, link.raw_end))
        };
        if let Some((destination, start, end, consumed)) = link {
            if let Some(target) = pane.resolve_markdown_link(&destination) {
                let start = map.visible_for_source(offset + start);
                let end = map.visible_for_source(offset + end);
                for (index, row) in rows.iter().enumerate() {
                    let len = row.hit.stops.len().saturating_sub(1);
                    let a = start.saturating_sub(row.hit.start).min(len);
                    let b = end.saturating_sub(row.hit.start).min(len);
                    if a >= b {
                        continue;
                    }
                    let rect = [
                        x + row.hit.stops[a],
                        y + index as f32 * line_height(opts),
                        row.hit.stops[b] - row.hit.stops[a],
                        line_height(opts),
                    ];
                    if let Some(hit) = intersect_rect(rect, clip) {
                        pane.register_link_rect(hit, target.clone());
                        let mut link_opts = opts.clone();
                        link_opts.color = theme.u8(theme.accent);
                        link_opts.clip_rect = Some(clip);
                        let label: String =
                            row.text.chars().skip(a).take(b - a).collect();
                        draw_if_visible(
                            sugarloaf,
                            rect[0],
                            rect[1],
                            &label,
                            &link_opts,
                            clip[1],
                            clip[1] + clip[3],
                            occlusions,
                        );
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            rect[0],
                            rect[1] + rect[3] - 3.0,
                            rect[2],
                            1.0,
                            theme.f32(theme.accent),
                            DEPTH,
                            ORDER_BG + 4,
                        );
                    }
                }
            }
            offset += consumed;
        } else {
            offset += rest.chars().next().unwrap().len_utf8();
        }
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
        pane,
        source_line,
        table_row_ix,
        cursor_col,
        col_widths,
        sugarloaf,
        opts,
    ) else {
        return;
    };
    let _ = (table, col_widths, row_h);
    let line_h = line_height(opts);
    let text_y = y + 7.0;
    let caret_h = caret_height(opts);
    let rect = [
        content_x + position.x - scroll_x,
        cursor_y_for_text_line(text_y + position.visual_line as f32 * line_h, opts),
        cursor_cell_width(opts),
        caret_h,
    ];
    match opts.clip_rect {
        Some(clip) => super::publish_cursor_rect(pane, rect, clip),
        None => pane.set_cursor_rect(Some(rect)),
    }
}

pub(super) fn table_source_cursor_position(
    table: &ParsedTable,
    pane: &MarkdownPane,
    source_line: &str,
    table_row_ix: usize,
    cursor_col: usize,
    col_widths: &[f32],
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> Option<TableCursorPosition> {
    table_row_for_ix(table, table_row_ix)?;
    let bounds = parse_table_cell_bounds(source_line)?;
    let source_col = floor_char_boundary(source_line, cursor_col.min(source_line.len()));
    let ix = bounds
        .iter()
        .position(|cell| source_col <= cell.raw_end)
        .unwrap_or(bounds.len().saturating_sub(1));
    let cell = bounds.get(ix)?;
    let width = *col_widths.get(ix)?;
    let map = pane.table_source_map(
        pane.cursor_line,
        ix,
        &source_line[cell.content_start..cell.content_end],
    );
    let visible_col = map.visible_for_source(
        source_col.clamp(cell.content_start, cell.content_end) - cell.content_start,
    );
    let rows = measured_table_cell_rows(
        sugarloaf,
        &map.visible_text(),
        (width - 32.0).max(16.0),
        opts,
        table.alignments.get(ix).copied().unwrap_or(0.0),
    );
    let row_ix = rows
        .iter()
        .rposition(|row| row.hit.start <= visible_col)
        .unwrap_or(0);
    let row = rows.get(row_ix)?;
    let stop = visible_col
        .saturating_sub(row.hit.start)
        .min(row.hit.stops.len().saturating_sub(1));
    let trailing = if source_col > cell.content_end && source_col <= cell.raw_end {
        sugarloaf
            .text_mut()
            .measure(&source_line[cell.content_end..source_col], opts)
    } else {
        0.0
    };
    Some(TableCursorPosition {
        x: col_widths.iter().take(ix).sum::<f32>()
            + 16.0
            + (row.hit.stops[stop] + trailing).min((width - 32.0).max(16.0)),
        visual_line: row_ix,
        cell_ix: ix,
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
