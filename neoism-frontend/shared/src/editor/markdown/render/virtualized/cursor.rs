fn set_cursor_for_item(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    item: &VirtualMarkdownDrawItem,
    x: f32,
    y: f32,
    width: f32,
    marker_len: usize,
    opts: &DrawOpts,
) {
    if pane.cursor_rect.is_some() {
        return;
    }
    let cursor_line = pane.cursor_line;
    if cursor_line < item.first_line || cursor_line >= item.first_line + item.line_count {
        return;
    }
    let local_line = cursor_line - item.first_line;
    let line_h = line_height(opts);
    let text_line = virtual_item_line(&item.text, local_line);
    let marker_len = marker_len.min(text_line.len());
    let col = pane.cursor_col.max(marker_len).min(text_line.len());
    let col = floor_char_boundary(text_line, col);
    // No leading-whitespace skip here: this only runs for the cursor's own
    // (raw-revealed) line, where the drawn rows preserve leading spaces —
    // skipping them froze the caret while typing/indenting at line start.
    let body = text_line.get(marker_len..).unwrap_or_default();
    // This only runs for the cursor's own line (guarded above), which renders
    // raw under Live Preview — so the caret maps through an identity map.
    let map = InlineSourceMap::identity(body);
    let col = col.max(marker_len);
    let text_y = y + local_line as f32 * line_h;
    if let Some((visual_line, row_prefix)) = pane.rendered_wrap_row_prefix_for_col(
        item.first_line + local_line,
        marker_len,
        col,
    ) {
        let cursor_x = (x + sugarloaf.text_mut().measure(&row_prefix, opts))
            .clamp(x, x + width.max(2.0) - 2.0);
        let cursor_y = text_y + visual_line as f32 * line_h;
        let caret_h = caret_height(opts);
        set_cursor_rect_clipped(
            pane,
            [
                cursor_x,
                cursor_y_for_text_line(cursor_y, opts),
                cursor_cell_width(opts),
                caret_h,
            ],
            opts.clip_rect,
        );
        return;
    }
    let full = visible_markdown_prefix(body, &map, map.visible_len());
    let prefix =
        visible_markdown_prefix(body, &map, map.visible_for_source(col - marker_len));
    let (cursor_x, cursor_y) = cursor_position_for_text_prefix(
        sugarloaf, x, text_y, line_h, width, opts, &full, &prefix,
    );
    let cursor_x = cursor_x.clamp(x, x + width.max(2.0) - 2.0);
    let caret_h = caret_height(opts);
    set_cursor_rect_clipped(
        pane,
        [
            cursor_x,
            cursor_y_for_text_line(cursor_y, opts),
            cursor_cell_width(opts),
            caret_h,
        ],
        opts.clip_rect,
    );
}

#[allow(clippy::too_many_arguments)]
/// Caret placement for a code-block line. Unlike `set_cursor_for_source_line`
/// this measures the raw code verbatim — no inline-markdown cleaning and no
/// whitespace collapsing — so the caret tracks exactly what is typed
/// (indentation, `*`, backticks, brackets, etc. all stay aligned).
fn set_cursor_for_code_line(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    item: &VirtualMarkdownDrawItem,
    line_ix: usize,
    text_line: &str,
    x: f32,
    y: f32,
    opts: &DrawOpts,
) {
    if pane.cursor_rect.is_some() || pane.cursor_line != line_ix {
        return;
    }
    if pane.cursor_line < item.first_line
        || pane.cursor_line >= item.first_line + item.line_count
    {
        return;
    }
    let col = floor_char_boundary(text_line, pane.cursor_col.min(text_line.len()));
    let col_chars = text_line
        .get(..col)
        .map(|prefix| prefix.chars().count())
        .unwrap_or(0);
    // Long code lines wrap; place the caret on the visual row the column
    // falls in (a column exactly on a wrap boundary belongs to the start of
    // the continuation row, matching the drawn glyphs).
    let (row_ix, row) = pane
        .block_wrap_rows
        .get(&line_ix)
        .map(|rows| {
            let mut row_ix = 0usize;
            let mut row = rows.first().copied().unwrap_or(MarkdownWrapRow {
                start: 0,
                len: text_line.chars().count(),
            });
            for (ix, candidate) in rows.iter().copied().enumerate().skip(1) {
                if col_chars >= candidate.start {
                    row_ix = ix;
                    row = candidate;
                } else {
                    break;
                }
            }
            (row_ix, row)
        })
        .unwrap_or((
            0,
            MarkdownWrapRow {
                start: 0,
                len: text_line.chars().count(),
            },
        ));
    let local = col_chars.saturating_sub(row.start).min(row.len);
    let prefix: String = text_line.chars().skip(row.start).take(local).collect();
    let cursor_x = x + sugarloaf.text_mut().measure(&prefix, opts);
    let line_h = line_height(opts);
    set_cursor_rect_clipped(
        pane,
        [
            cursor_x,
            cursor_y_for_text_line(y + row_ix as f32 * line_h, opts),
            cursor_cell_width(opts),
            caret_height(opts),
        ],
        opts.clip_rect,
    );
}

fn set_cursor_for_source_line(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    item: &VirtualMarkdownDrawItem,
    line_ix: usize,
    text_line: &str,
    x: f32,
    y: f32,
    width: f32,
    marker_len: usize,
    hang_px: f32,
    opts: &DrawOpts,
) {
    if pane.cursor_rect.is_some() || pane.cursor_line != line_ix {
        return;
    }
    if pane.cursor_line < item.first_line
        || pane.cursor_line >= item.first_line + item.line_count
    {
        return;
    }
    let line_h = line_height(opts);
    let marker_len = marker_len.min(text_line.len());
    let col = pane.cursor_col.max(marker_len).min(text_line.len());
    let col = floor_char_boundary(text_line, col);
    let body = text_line.get(marker_len..).unwrap_or_default();
    // Cursor's own line (guarded above) renders raw under Live Preview — the
    // caret maps through an identity map so raw col == drawn position.
    let map = InlineSourceMap::identity(body);
    let full = visible_markdown_prefix(body, &map, map.visible_len());
    let col = col.max(marker_len);
    if let Some((visual_line, row_prefix)) =
        pane.rendered_wrap_row_prefix_for_col(line_ix, marker_len, col)
    {
        // Wrapped continuation rows of the revealed line draw at x + hang_px;
        // the caret follows the same hanging indent.
        let row_hang = if visual_line > 0 { hang_px } else { 0.0 };
        let cursor_x = (x + row_hang + sugarloaf.text_mut().measure(&row_prefix, opts))
            .clamp(x, x + width.max(2.0) - 2.0);
        let cursor_y = y + visual_line as f32 * line_h;
        set_cursor_rect_clipped(
            pane,
            [
                cursor_x,
                cursor_y_for_text_line(cursor_y, opts),
                cursor_cell_width(opts),
                caret_height(opts),
            ],
            opts.clip_rect,
        );
        return;
    }
    let prefix =
        visible_markdown_prefix(body, &map, map.visible_for_source(col - marker_len));
    let (cursor_x, cursor_y) = cursor_position_for_text_prefix(
        sugarloaf, x, y, line_h, width, opts, &full, &prefix,
    );
    let cursor_x = cursor_x.clamp(x, x + width.max(2.0) - 2.0);
    set_cursor_rect_clipped(
        pane,
        [
            cursor_x,
            cursor_y_for_text_line(cursor_y, opts),
            cursor_cell_width(opts),
            caret_height(opts),
        ],
        opts.clip_rect,
    );
}

fn ensure_virtual_cursor_visible(pane: &mut MarkdownPane, clip: [f32; 4]) {
    if pane.cursor_rect.is_some() {
        return;
    }
    let Some(block) = pane
        .block_rects
        .iter()
        .rev()
        .find(|block| block.line == pane.cursor_line)
        .copied()
    else {
        return;
    };
    let Some(line) = pane.lines.get(block.line) else {
        return;
    };
    let marker_len = block.marker_len.min(line.len());
    let Some((visual_line, visual_col)) = pane
        .visual_position_for_col_from_wrap_rows(block.line, marker_len, pane.cursor_col)
    else {
        return;
    };
    let Some(measured_x) = pane
        .block_wrap_hit_stops
        .get(&block.line)
        .and_then(|rows| rows.get(visual_line))
        .and_then(|row| row.stops.get(visual_col.min(row.stops.len().saturating_sub(1))))
        .copied()
    else {
        return;
    };
    let x = (block.text_x + measured_x)
        .clamp(block.text_x, block.text_x + block.wrap_width.max(2.0) - 2.0);
    let y = block.text_y + visual_line as f32 * block.line_height;
    if y + block.line_height < clip[1] || y > clip[1] + clip[3] {
        return;
    }
    let caret_h = (block.line_height * 0.82).max(10.0);
    set_cursor_rect_clipped(
        pane,
        [
            x,
            y + (block.line_height - caret_h).max(0.0) * 0.25,
            block.cell_width,
            caret_h,
        ],
        Some(clip),
    );
}

fn set_cursor_rect_clipped(
    pane: &mut MarkdownPane,
    rect: [f32; 4],
    clip: Option<[f32; 4]>,
) {
    // Clamp (not just intersect-test) the caret to the pane clip: the
    // trail-cursor overlay draws this rect verbatim, so a caret on a
    // line half-scrolled under the chrome must shrink to its visible
    // slice instead of phasing through the top bar.
    let Some(clip) = clip else {
        pane.set_cursor_rect(Some(rect));
        return;
    };
    let x0 = rect[0].max(clip[0]);
    let y0 = rect[1].max(clip[1]);
    let x1 = (rect[0] + rect[2]).min(clip[0] + clip[2]);
    let y1 = (rect[1] + rect[3]).min(clip[1] + clip[3]);
    if x1 > x0 && y1 > y0 {
        pane.set_cursor_rect(Some([x0, y0, x1 - x0, y1 - y0]));
    }
}

/// Caret for the cursor on trailing blank lines. Markdown nodes never
/// cover blank lines after the last block, so no draw item places the
/// caret there (down-arrow onto the file's trailing newline made it
/// vanish). Anchors the caret below the lowest visible node instead.
fn set_cursor_for_trailing_empty_lines(
    pane: &mut MarkdownPane,
    tail_anchor: Option<(usize, f32)>,
    content_x: f32,
    font_scale: f32,
    clip: [f32; 4],
) {
    if pane.cursor_rect.is_some() {
        return;
    }
    let Some((end_line, bottom_y)) = tail_anchor else {
        return;
    };
    if pane.cursor_line < end_line || pane.cursor_line >= pane.lines.len() {
        return;
    }
    // Only synthesize when everything from the anchor down to the cursor
    // really is blank — if there's content there, its own node draws the
    // caret (or the anchor isn't the document tail).
    let all_blank = (end_line..=pane.cursor_line)
        .all(|ix| pane.lines.get(ix).is_some_and(|line| line.trim().is_empty()));
    if !all_blank {
        return;
    }
    let opts = DrawOpts {
        font_size: markdown_font(16.0, font_scale),
        ..DrawOpts::default()
    };
    let line_h = line_height(&opts);
    let text_y = bottom_y + (pane.cursor_line - end_line) as f32 * line_h;
    set_cursor_rect_clipped(
        pane,
        [
            content_x,
            cursor_y_for_text_line(text_y, &opts),
            cursor_cell_width(&opts),
            caret_height(&opts),
        ],
        Some(clip),
    );
}

fn set_fallback_cursor_for_empty_virtual_markdown(
    pane: &mut MarkdownPane,
    content_x: f32,
    text_y: f32,
    width: f32,
    font_scale: f32,
    clip: [f32; 4],
) {
    if pane.cursor_rect.is_some()
        || !(pane.lines.is_empty()
            || pane.lines.iter().all(|line| line.trim().is_empty()))
    {
        return;
    }
    let opts = DrawOpts {
        font_size: markdown_font(16.0, font_scale),
        ..DrawOpts::default()
    };
    let cursor_w = cursor_cell_width(&opts).max(1.0).min(width.max(1.0));
    set_cursor_rect_clipped(
        pane,
        [
            content_x,
            cursor_y_for_text_line(text_y, &opts),
            cursor_w,
            caret_height(&opts),
        ],
        Some(clip),
    );
}

