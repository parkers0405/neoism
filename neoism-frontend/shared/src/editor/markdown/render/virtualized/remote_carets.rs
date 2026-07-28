/// Wave 7C: draw remote collaborators' carets (colored bar + name tag)
/// over the virtualized markdown surface.
///
/// Positions map through the SAME wrap-row + hit-stop data the local
/// caret uses (`visual_position_for_col_from_wrap_rows` +
/// `block_wrap_hit_stops`), so a remote caret lands exactly where that
/// character is drawn in MY view — including my Live Preview reveal
/// state (their line renders styled for me unless it's also my cursor
/// line, and the raw→visible mapping accounts for either).
fn draw_remote_markdown_carets(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    theme: &IdeTheme,
    clip: [f32; 4],
    font_scale: f32,
) {
    if pane.remote_cursors.is_empty() {
        return;
    }
    let clip_top = clip[1];
    let clip_bottom = clip[1] + clip[3];
    for cursor in &pane.remote_cursors {
        let Some(line) = pane.lines.get(cursor.line) else {
            continue;
        };
        // Only lines inside a visible block can anchor a caret; remote
        // cursors on scrolled-out lines simply don't draw this frame.
        let Some(block) = pane
            .block_rects
            .iter()
            .rev()
            .find(|block| block.line == cursor.line)
        else {
            continue;
        };
        let byte_col = byte_col_for_utf16_col(line, cursor.col_utf16);
        let marker_len = block.marker_len.min(line.len());
        let Some((visual_line, visual_col)) = pane.visual_position_for_col_from_wrap_rows(
            cursor.line,
            marker_len,
            byte_col,
        ) else {
            continue;
        };
        let Some(x_offset) = pane
            .block_wrap_hit_stops
            .get(&cursor.line)
            .and_then(|rows| rows.get(visual_line))
            .and_then(|row| {
                row.stops
                    .get(visual_col.min(row.stops.len().saturating_sub(1)))
            })
            .copied()
        else {
            continue;
        };
        let x = (block.text_x + x_offset)
            .clamp(block.text_x, block.text_x + block.wrap_width.max(2.0) - 2.0);
        let y = block.text_y + visual_line as f32 * block.line_height;
        if y + block.line_height < clip_top || y > clip_bottom {
            continue;
        }
        // Rainbow peers animate locally on the shared clock; everyone
        // else paints in the color they broadcast.
        let color = if cursor.rainbow {
            let c = crate::cursor_style::rainbow_color_f32(
                crate::cursor_style::rainbow_now_seconds(),
            );
            [c[0], c[1], c[2], 0.9]
        } else {
            [
                cursor.color[0] as f32 / 255.0,
                cursor.color[1] as f32 / 255.0,
                cursor.color[2] as f32 / 255.0,
                0.9,
            ]
        };
        let caret_h = (block.line_height * 0.82).max(10.0);
        let caret_y = y + (block.line_height - caret_h).max(0.0) * 0.25;
        draw_rect_clipped(
            sugarloaf,
            clip,
            x,
            caret_y,
            2.0,
            caret_h,
            color,
            DEPTH,
            ORDER_TEXT,
        );
        // Name tag riding the caret top: small colored flag with the
        // collaborator's name, like every co-editing UI.
        if cursor.name.is_empty() {
            continue;
        }
        let name_opts = DrawOpts {
            font_size: (markdown_font(16.0, font_scale) * 0.58).max(8.0),
            color: theme.u8(theme.bg),
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        let name_w = sugarloaf.text_mut().measure(&cursor.name, &name_opts);
        let tag_h = name_opts.font_size + 4.0;
        let tag_y = (caret_y - tag_h).max(clip_top);
        // Notion-style presence chip: the peer's pixel-plasma orb, then
        // their name flag. The orb is seeded by the display name, so the
        // same host always wears the same orb. Still frame (t = 0.6) —
        // the markdown pane doesn't repaint just for presence.
        let orb_size = tag_h;
        let orb_gap = 2.0;
        draw_presence_orb_clipped(
            sugarloaf, clip, &cursor.name, x, tag_y, orb_size, 0.6, DEPTH, ORDER_TEXT,
        );
        let flag_x = x + orb_size + orb_gap;
        draw_rounded_rect_clipped(
            sugarloaf,
            clip,
            flag_x,
            tag_y,
            name_w + 8.0,
            tag_h,
            3.0,
            color,
            DEPTH,
            ORDER_TEXT,
        );
        sugarloaf.text_mut().draw(
            flag_x + 4.0,
            tag_y + 2.0,
            &cursor.name,
            &name_opts,
        );
    }
}

/// Byte column in `line` for a UTF-16 column (the presence wire format).
/// Clamps to the line end; lands on a char boundary by construction.
fn byte_col_for_utf16_col(line: &str, col_utf16: usize) -> usize {
    let mut utf16 = 0usize;
    for (byte, ch) in line.char_indices() {
        if utf16 >= col_utf16 {
            return byte;
        }
        utf16 += ch.len_utf16();
    }
    line.len()
}
