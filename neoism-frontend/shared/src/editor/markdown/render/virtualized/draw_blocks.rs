fn measure_item(
    sugarloaf: &mut Sugarloaf,
    pane: &MarkdownPane,
    item: &VirtualMarkdownDrawItem,
    width: f32,
    clip: [f32; 4],
    theme: &IdeTheme,
    font_scale: f32,
    // Cursor line when it falls inside this item: the revealed (raw) line
    // wraps differently than the rendered view, so heights must follow it.
    cursor_line: Option<usize>,
) -> (f32, u32) {
    match item.kind {
        VirtualNodeKind::Heading => {
            let raw = item.text.trim_end_matches(['\r', '\n']);
            let (level, heading_marker) = heading_node_level_and_marker(raw);
            // Mirror draw_heading's Live Preview reveal: the cursor's line
            // wraps with the `### ` markup shown.
            let marker_len = if cursor_line == Some(item.first_line) {
                0
            } else {
                heading_marker
            };
            let opts = DrawOpts {
                font_size: markdown_font(
                    heading_level_font_size(level as usize),
                    font_scale,
                ),
                color: theme.u8(theme.fg),
                bold: true,
                clip_rect: Some(clip),
                font_id: md_font_id(sugarloaf),
                ..DrawOpts::default()
            };
            let text = raw.get(marker_len..).unwrap_or_default();
            let lines = if cursor_line == Some(item.first_line) {
                inline_wrapped_lines(sugarloaf, text, width.max(24.0), &opts)
            } else {
                inline_wrapped_lines_dropcap(sugarloaf, text, width.max(24.0), &opts).0
            };
            let lines = inline_visual_row_count(&lines);
            (lines as f32 * line_height(&opts) + 12.0, lines as u32)
        }
        VirtualNodeKind::CodeBlock => {
            let opts = DrawOpts {
                font_size: markdown_font(15.0, font_scale),
                color: theme.u8(theme.muted),
                clip_rect: Some(clip),
                ..DrawOpts::default()
            };
            // The ``` fences are hidden behind the git-diff-style header bar,
            // so only the inner code rows (wrap-aware) take body height —
            // plus one extra row when the cursor reveals the closing fence.
            let local_lines = virtual_item_lines(&item.text);
            if let Some(first) = local_lines.first() {
                if let Some(meta) = parse_notebook_output_meta(
                    first.trim_start().trim_start_matches('`').trim(),
                ) {
                    return measure_notebook_output_lines(
                        sugarloaf,
                        &local_lines,
                        &meta,
                        width,
                        clip,
                        theme,
                        font_scale,
                    );
                }
            }
            let cursor_inside = cursor_line.is_some_and(|line| {
                line >= item.first_line
                    && line < item.first_line + local_lines.len().max(1)
            });
            if !cursor_inside {
                if let Some(first) = local_lines.first() {
                    if virtual_markdown_code_label(first).eq_ignore_ascii_case("mermaid")
                    {
                        let source = local_lines
                            .iter()
                            .skip(1)
                            .take(local_lines.len().saturating_sub(2))
                            .map(|line| line.trim_end_matches(['\r', '\n']))
                            .collect::<Vec<_>>()
                            .join("\n");
                        if parse_mermaid_diagram(&source).is_some() {
                            let height = CODE_BLOCK_HEADER_H + 238.0 * font_scale;
                            return (height.max(190.0 * font_scale), 10);
                        }
                    }
                }
            }
            let mut inner_rows = 0usize;
            let mut fence_reveal_rows = 0usize;
            for (local_ix, line) in local_lines.iter().enumerate() {
                if line.trim_start().starts_with("```") {
                    if local_ix > 0 && cursor_line == Some(item.first_line + local_ix) {
                        fence_reveal_rows = 1;
                    }
                    continue;
                }
                let stops = measured_stops_for_text(sugarloaf, line, &opts);
                let chars: Vec<char> = line.chars().collect();
                inner_rows += code_wrap_ranges(&stops, &chars, width).len();
            }
            let rows = inner_rows.max(1) + fence_reveal_rows;
            let height = CODE_BLOCK_HEADER_H
                + CODE_BLOCK_BODY_PAD * 2.0
                + rows as f32 * line_height(&opts);
            (height, (rows + 1) as u32)
        }
        VirtualNodeKind::Table => {
            let lines = virtual_item_lines(&item.text);
            if let Some(table) = parse_table(&lines, 0) {
                let measurement =
                    measure_table(sugarloaf, &table, width, theme, font_scale);
                (measurement.height, measurement.visual_line_count)
            } else {
                let rows = lines.len().max(1);
                let row_h = line_height(&DrawOpts {
                    font_size: markdown_font(15.0, font_scale),
                    color: theme.u8(theme.muted),
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                });
                (rows as f32 * row_h, rows as u32)
            }
        }
        _ => {
            let opts = DrawOpts {
                font_size: markdown_font(17.0, font_scale),
                color: theme.u8(theme.fg),
                clip_rect: Some(clip),
                font_id: md_font_id(sugarloaf),
                ..DrawOpts::default()
            };
            let local_lines = virtual_item_lines(&item.text);
            let mut measure_in_code_block = false;
            let mut local_ix = 0usize;
            let mut height = 8.0;
            let mut visual_lines = 0usize;
            let frontmatter = pane.frontmatter_range();
            while local_ix < local_lines.len() {
                let raw = local_lines[local_ix].trim_end_matches(['\r', '\n']);
                // Mirror draw_markdown_line: whitespace-only lines keep their
                // real chars; only empty lines get the " " placeholder.
                let text = if raw.is_empty() { " " } else { raw };
                if parse_notebook_output_line(raw).is_some() {
                    let mut outputs = Vec::new();
                    while local_ix < local_lines.len() {
                        let raw = local_lines[local_ix].trim_end_matches(['\r', '\n']);
                        let Some(output) = parse_notebook_output_line(raw) else {
                            break;
                        };
                        outputs.push(output);
                        local_ix += 1;
                    }
                    let (output_h, output_rows) = measure_notebook_output_text_group(
                        sugarloaf, &outputs, width, clip, theme, font_scale,
                    );
                    height += output_h;
                    visual_lines += output_rows as usize;
                    continue;
                }
                let code_fence = text.trim_start().starts_with("```");
                if code_fence
                    && parse_notebook_output_meta(
                        text.trim_start().trim_start_matches('`').trim(),
                    )
                    .is_some()
                {
                    let meta = parse_notebook_output_meta(
                        text.trim_start().trim_start_matches('`').trim(),
                    );
                    let has_prompt =
                        meta.as_ref().is_some_and(|meta| !meta.prompt.is_empty());
                    let output_opts = DrawOpts {
                        font_size: markdown_font(14.0, font_scale),
                        color: theme.u8(theme.fg),
                        clip_rect: Some(clip),
                        ..DrawOpts::default()
                    };
                    let line_h = line_height(&output_opts);
                    let prompt_w = if has_prompt { 76.0 * font_scale } else { 0.0 };
                    let body_width = (width - prompt_w).max(36.0);
                    let mut rows = 0usize;
                    local_ix += 1;
                    while local_ix < local_lines.len() {
                        let raw = local_lines[local_ix].trim_end_matches(['\r', '\n']);
                        if raw.trim_start().starts_with("```") {
                            local_ix += 1;
                            break;
                        }
                        let chars: Vec<char> = raw.chars().collect();
                        let stops = measured_stops_for_text(sugarloaf, raw, &output_opts);
                        rows += code_wrap_ranges(&stops, &chars, body_width).len().max(1);
                        local_ix += 1;
                    }
                    height += (rows.max(1) as f32 * line_h)
                        + if has_prompt { 6.0 } else { 2.0 };
                    visual_lines += rows.max(1);
                    continue;
                }
                let line_ix = item.first_line + local_ix;
                if frontmatter
                    .as_ref()
                    .is_some_and(|fm| fm.contains(&line_ix))
                    && cursor_line != Some(line_ix)
                {
                    let (row_h, rows) = measure_frontmatter_row(
                        sugarloaf, text, width, line_height(&opts), &opts, theme,
                        font_scale, clip,
                    );
                    height += row_h;
                    visual_lines += rows;
                    local_ix += 1;
                    continue;
                }
                if measure_in_code_block || code_fence {
                    let code_opts = DrawOpts {
                        font_size: markdown_font(15.0, font_scale),
                        color: theme.u8(theme.muted),
                        clip_rect: Some(clip),
                        ..DrawOpts::default()
                    };
                    height += line_height(&code_opts);
                    visual_lines += 1;
                    if code_fence {
                        measure_in_code_block = !measure_in_code_block;
                    }
                    local_ix += 1;
                    continue;
                }
                if let Some(table) = parse_table(&local_lines, local_ix) {
                    let table_measure =
                        measure_table(sugarloaf, &table, width, theme, font_scale);
                    height += table_measure.height;
                    visual_lines += table_measure.visual_line_count as usize;
                    local_ix = table.end_line;
                    continue;
                }
                if let Some((level, _, _)) = parse_heading_line(text) {
                    let heading_opts = DrawOpts {
                        font_size: markdown_font(
                            heading_level_font_size(level as usize),
                            font_scale,
                        ),
                        color: theme.u8(theme.fg),
                        bold: true,
                        clip_rect: Some(clip),
                        font_id: md_font_id(sugarloaf),
                        ..DrawOpts::default()
                    };
                    height += line_height(&heading_opts);
                    visual_lines += 1;
                    local_ix += 1;
                    continue;
                }
                // Mirror draw: the cursor's divider line reveals raw `---`
                // (measured by the generic reveal branch below).
                if is_divider(text.trim())
                    && cursor_line != Some(item.first_line + local_ix)
                {
                    height += 18.0;
                    visual_lines += 1;
                    local_ix += 1;
                    continue;
                }
                // Mirror draw_markdown_line: the cursor's line wraps RAW (full
                // marker prefix shown, hanging continuation rows) — measuring
                // the rendered body instead let the revealed line grow a row
                // and paint over the block below it.
                let lines = if cursor_line == Some(item.first_line + local_ix) {
                    let hang = raw_line_hang_px(sugarloaf, text, &opts);
                    inline_wrapped_lines_raw(sugarloaf, text, width, hang, &opts)
                        .len()
                        .max(1)
                } else if let Some(quote_len) = quote_marker_len(text) {
                    // Mirror the styled blockquote: `> ` hidden, body indented
                    // past the accent bar, italic metrics.
                    let body = text.get(quote_len.min(text.len())..).unwrap_or_default();
                    let quote_opts = DrawOpts {
                        italic: true,
                        ..opts
                    };
                    let inline_lines = inline_wrapped_lines_dropcap(
                        sugarloaf,
                        body,
                        (width - QUOTE_BODY_INDENT).max(24.0),
                        &quote_opts,
                    )
                    .0;
                    inline_visual_row_count(&inline_lines)
                } else {
                    let (body_offset, body, _) = line_marker_layout(text, width, &opts);
                    let body_width = (width - body_offset).max(24.0);
                    let inline_lines =
                        inline_wrapped_lines_dropcap(sugarloaf, body, body_width, &opts)
                            .0;
                    inline_visual_row_count(&inline_lines)
                };
                let preview_h = pane.notebook_image_preview_extra_h(
                    item.first_line + local_ix,
                    width,
                    font_scale,
                );
                height += lines as f32 * line_height(&opts) + preview_h;
                visual_lines +=
                    lines + (preview_h / line_height(&opts)).ceil().max(0.0) as usize;
                local_ix += 1;
            }
            (height, visual_lines.max(1) as u32)
        }
    }
}

fn measure_notebook_output_lines(
    sugarloaf: &mut Sugarloaf,
    local_lines: &[String],
    meta: &NotebookOutputMeta,
    width: f32,
    clip: [f32; 4],
    theme: &IdeTheme,
    font_scale: f32,
) -> (f32, u32) {
    let output_opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let line_h = line_height(&output_opts);
    let has_prompt_chrome = !meta.prompt.is_empty();
    let prompt_w = if has_prompt_chrome {
        76.0 * font_scale
    } else {
        0.0
    };
    let connector_opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8_alpha(theme.muted, 0.9),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let connector_w = sugarloaf.text_mut().measure("╰─", &connector_opts);
    let body_width = (width - prompt_w - connector_w - 10.0 * font_scale).max(36.0);
    let mut rows = 0usize;
    for line in local_lines.iter() {
        let raw = line.trim_end_matches(['\r', '\n']);
        if raw.trim_start().starts_with("```") {
            continue;
        }
        let chars: Vec<char> = raw.chars().collect();
        let stops = measured_stops_for_text(sugarloaf, raw, &output_opts);
        rows += code_wrap_ranges(&stops, &chars, body_width).len().max(1);
    }
    let rows = rows.max(1);
    let top_pad = if has_prompt_chrome { 6.0 } else { 2.0 };
    (rows as f32 * line_h + top_pad, rows as u32)
}

fn measure_notebook_output_text_group(
    sugarloaf: &mut Sugarloaf,
    outputs: &[NotebookOutputLine<'_>],
    width: f32,
    clip: [f32; 4],
    theme: &IdeTheme,
    font_scale: f32,
) -> (f32, u32) {
    let opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let line_h = line_height(&opts).max(1.0);
    let has_prompt = outputs.iter().any(|output| !output.prompt.is_empty());
    let has_elapsed = outputs.iter().any(|output| {
        output
            .elapsed
            .as_ref()
            .is_some_and(|elapsed| !elapsed.is_empty())
    });
    let has_prompt_chrome = has_prompt;
    let prompt_w = if has_prompt_chrome {
        76.0 * font_scale
    } else {
        0.0
    };
    let connector_opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8_alpha(theme.muted, 0.9),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let connector_w = sugarloaf.text_mut().measure("╰─", &connector_opts);
    let body_w = (width - prompt_w - connector_w - 10.0 * font_scale).max(36.0);
    let mut body_h = 0.0f32;
    let mut rows = 0usize;
    for output in outputs {
        let chars: Vec<char> = output.text.chars().collect();
        let stops = measured_stops_for_text(sugarloaf, output.text, &opts);
        let row_count = code_wrap_ranges(&stops, &chars, body_w).len().max(1);
        let mut output_h = row_count as f32 * line_h;
        if let Some((_, preview_h)) =
            notebook_image_preview_size(output.text, body_w, font_scale)
        {
            let preview_extra_h = 8.0 * font_scale + preview_h;
            output_h += preview_extra_h;
            rows += (preview_extra_h / line_h).ceil().max(1.0) as usize;
        }
        body_h += output_h;
        rows += row_count;
    }
    let rows = rows.max(1);
    let body_h = body_h.max(line_h);
    let detail_h = if has_elapsed {
        line_height(&DrawOpts {
            font_size: markdown_font(12.0, font_scale),
            color: theme.u8_alpha(theme.muted, 0.86),
            clip_rect: Some(clip),
            ..DrawOpts::default()
        })
    } else {
        0.0
    };
    let top_pad = if has_prompt_chrome { 2.0 } else { 0.0 };
    let bottom_pad = 1.0;
    (
        top_pad + body_h + detail_h + bottom_pad,
        (rows + usize::from(has_elapsed)).max(1) as u32,
    )
}

fn notebook_image_preview_size(
    text: &str,
    body_w: f32,
    font_scale: f32,
) -> Option<(f32, f32)> {
    let (width, height) = notebook_image_output_dimensions(text)?;
    if width == 0.0 || height == 0.0 {
        return None;
    }
    let max_w = body_w.min(640.0 * font_scale).max(1.0);
    let max_h = 360.0 * font_scale;
    let fit = (max_w / width).min(max_h / height).min(1.0);
    Some(((width * fit).max(1.0), (height * fit).max(1.0)))
}

fn notebook_image_output_dimensions(text: &str) -> Option<(f32, f32)> {
    let rest = text.strip_prefix("Image output: ")?;
    for part in rest.split(',') {
        let value = part.trim();
        let Some((width, height)) = value.split_once('x') else {
            continue;
        };
        let width = width.trim().parse::<f32>().ok()?;
        let height = height.trim().parse::<f32>().ok()?;
        return Some((width, height));
    }
    None
}

fn draw_item(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    item: &VirtualMarkdownDrawItem,
    content_x: f32,
    content_w: f32,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
    animation_phase: f32,
) {
    let node_x = content_x + item.bounds.x;
    let node_y = item.screen_y;
    let node_h = item.bounds.height.max(1.0);
    let node_rect = [node_x, node_y, content_w, node_h];
    let handle_rect = block_handle_rect(node_rect);
    let metric_opts = DrawOpts {
        font_size: markdown_font(17.0, font_scale),
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    // Obsidian Live Preview: the cursor's own line renders RAW (markup shown),
    // so its click/cursor mapping is identity — drop the marker offset for the
    // block rect on that line so hit-testing matches the revealed source.
    let marker_len = if pane.cursor_line == item.first_line {
        0
    } else {
        pane.visible_start_col(item.first_line)
    };
    let line_h_hint = line_height(&metric_opts);
    let cell_width = cursor_cell_width(&metric_opts).max(1.0);
    let active = pane.register_block_rect(
        item.first_line,
        node_rect,
        handle_rect,
        node_x,
        node_y,
        marker_len,
        cell_width,
        line_h_hint,
        content_w,
        mouse,
    );

    if active {
        draw_block_actions(
            sugarloaf,
            node_rect,
            theme,
            clip,
            pane.dragging_line == Some(item.first_line),
        );
    }

    match item.kind {
        VirtualNodeKind::Heading => draw_heading(
            sugarloaf,
            pane,
            item,
            node_x,
            content_w,
            clip,
            clip_top,
            clip_bottom,
            theme,
            text_occlusions,
            font_scale,
        ),
        VirtualNodeKind::CodeBlock => draw_code_block(
            sugarloaf,
            pane,
            item,
            node_x,
            content_w,
            clip,
            clip_top,
            clip_bottom,
            theme,
            text_occlusions,
            font_scale,
            animation_phase,
        ),
        VirtualNodeKind::Table => draw_table_block(
            sugarloaf,
            pane,
            item,
            node_x,
            content_w,
            clip,
            clip_top,
            clip_bottom,
            theme,
            mouse,
            text_occlusions,
            font_scale,
        ),
        _ => draw_markdown_line(
            sugarloaf,
            pane,
            item,
            node_x,
            content_w,
            clip,
            clip_top,
            clip_bottom,
            theme,
            mouse,
            text_occlusions,
            font_scale,
            animation_phase,
        ),
    }
}

fn draw_heading(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    item: &VirtualMarkdownDrawItem,
    x: f32,
    width: f32,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
) {
    let raw = item.text.trim_end_matches(['\r', '\n']);
    let (level, heading_marker) = heading_node_level_and_marker(raw);
    // Obsidian Live Preview: reveal the `### ` markup on the cursor's own line
    // (still drawn at heading size), so the rendered text equals the buffer and
    // cursor/click/wrap math is identity. Off the line it collapses to styled.
    let marker_len = if pane.cursor_line == item.first_line {
        0
    } else {
        heading_marker
    };
    let text = raw.get(marker_len..).unwrap_or(raw);
    let opts = DrawOpts {
        font_size: markdown_font(heading_level_font_size(level as usize), font_scale),
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let text_y = item.screen_y + 4.0;
    let heading_lines = if pane.cursor_line == item.first_line {
        inline_wrapped_lines(sugarloaf, text, width.max(24.0), &opts)
    } else {
        inline_wrapped_lines_dropcap(sugarloaf, text, width.max(24.0), &opts).0
    };
    let heading_lines = if heading_lines.is_empty() {
        vec![InlineWrappedLine::default()]
    } else {
        heading_lines
    };
    pane.register_block_wrap_row_spans(item.first_line, inline_wrap_rows(&heading_lines));
    pane.register_block_wrap_hit_stops(
        item.first_line,
        measured_inline_wrap_hit_rows(sugarloaf, &heading_lines, 0.0, &opts),
    );
    draw_selection_for_line(
        sugarloaf,
        pane,
        item.first_line,
        raw,
        x,
        text_y,
        marker_len,
        line_height(&opts),
        width,
        &opts,
        theme,
        clip,
        clip_top,
        clip_bottom,
    );
    draw_search_matches_for_line(
        sugarloaf,
        pane,
        item.first_line,
        raw,
        x,
        text_y,
        marker_len,
        line_height(&opts),
        width,
        &opts,
        theme,
        clip,
        clip_top,
        clip_bottom,
    );
    draw_inline_wrapped_lines(
        sugarloaf,
        pane,
        &heading_lines,
        x,
        text_y,
        0.0,
        &opts,
        theme,
        clip,
        clip_top,
        clip_bottom,
        text_occlusions,
    );
    set_cursor_for_item(sugarloaf, pane, item, x, text_y, width, marker_len, &opts);
}

/// Indent of a styled blockquote's body past the accent bar.
const QUOTE_BODY_INDENT: f32 = 18.0;

/// Distinct per-level heading sizes (body text is 17px): the old
/// `26 - level` formula put adjacent levels 1px apart — visually identical.
fn heading_level_font_size(level: usize) -> f32 {
    match level.clamp(1, 6) {
        1 => 30.0,
        2 => 26.0,
        3 => 22.5,
        4 => 20.0,
        5 => 18.5,
        _ => 17.5,
    }
}

/// Level + marker byte length for a heading-node line, tolerating leading
/// indent. The surface adapter classifies "  ### x" / "\t### x" as headings
/// (it trims first) — computing the level on the UNTRIMMED line read zero
/// hashes, and `.clamp(1, 6)` silently called that h1: typing a Tab or
/// space in front of a `###` heading blew it up to 30px with raw hashes.
fn heading_node_level_and_marker(raw: &str) -> (usize, usize) {
    let trimmed = raw.trim_start();
    let indent = raw.len() - trimmed.len();
    let level = trimmed
        .chars()
        .take_while(|ch| *ch == '#')
        .count()
        .clamp(1, 6);
    (level, indent + heading_marker_len(trimmed, level))
}

fn heading_marker_len(raw: &str, level: usize) -> usize {
    raw.get(level..)
        .and_then(|rest| rest.chars().next())
        .filter(|ch| ch.is_whitespace())
        .map(|ch| level + ch.len_utf8())
        .unwrap_or(level)
        .min(raw.len())
}

#[allow(clippy::too_many_arguments)]
fn draw_notebook_output_group(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    item: &VirtualMarkdownDrawItem,
    start_local_ix: usize,
    outputs: &[NotebookOutputLine<'_>],
    x: f32,
    y: f32,
    width: f32,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
    animation_phase: f32,
) -> f32 {
    let has_prompt = outputs.iter().any(|output| !output.prompt.is_empty());
    let has_running = outputs.iter().any(|output| output.running);
    let prompt = outputs
        .iter()
        .find_map(|output| (!output.prompt.is_empty()).then_some(output.prompt.as_str()))
        .unwrap_or_default();
    let elapsed = outputs.iter().find_map(|output| {
        output
            .elapsed
            .as_ref()
            .filter(|elapsed| !elapsed.is_empty())
            .map(String::as_str)
    });
    let opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let prompt_opts = DrawOpts {
        font_size: markdown_font(13.0, font_scale),
        color: theme.u8(if prompt.starts_with("Err") {
            theme.red
        } else {
            theme.accent
        }),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let detail_opts = DrawOpts {
        font_size: markdown_font(12.0, font_scale),
        color: theme.u8_alpha(theme.muted, 0.86),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let line_h = line_height(&opts).max(1.0);
    let has_prompt_chrome = has_prompt;
    let prompt_w = if has_prompt_chrome {
        76.0 * font_scale
    } else {
        0.0
    };
    let connector = "╰─";
    let connector_opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8_alpha(theme.muted, 0.9),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let connector_x = x + prompt_w + 2.0 * font_scale;
    let connector_w = sugarloaf.text_mut().measure(connector, &connector_opts);
    let connector_gap = 8.0 * font_scale;
    let body_x = connector_x + connector_w + connector_gap;
    let body_w = (width - (body_x - x)).max(36.0);
    let row_y = y + if has_prompt_chrome { 2.0 } else { 0.0 };

    if has_running {
        draw_notebook_running_loader(
            sugarloaf,
            clip,
            body_x,
            row_y,
            line_h,
            markdown_font(14.0, font_scale),
            animation_phase,
        );
    } else if has_prompt {
        let prompt = format!("{prompt}:");
        draw_if_visible(
            sugarloaf,
            x,
            row_y,
            &prompt,
            &prompt_opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
    }

    draw_if_visible(
        sugarloaf,
        connector_x,
        row_y,
        connector,
        &connector_opts,
        clip_top,
        clip_bottom,
        text_occlusions,
    );

    let mut current_y = row_y;
    for (offset, output) in outputs.iter().enumerate() {
        let line_ix = item.first_line + start_local_ix + offset;
        let chars: Vec<char> = output.text.chars().collect();
        let stops = measured_stops_for_text(sugarloaf, output.text, &opts);
        let ranges = code_wrap_ranges(&stops, &chars, body_w);
        let row_count = ranges.len().max(1);
        let text_h = line_h * row_count as f32;
        let preview_h = notebook_image_preview_size(output.text, body_w, font_scale)
            .map(|(_, height)| 8.0 * font_scale + height)
            .unwrap_or(0.0);
        pane.register_block_rect(
            line_ix,
            [body_x, current_y, body_w, text_h + preview_h],
            [x - 30.0, current_y, 22.0, text_h + preview_h],
            body_x,
            current_y,
            0,
            cursor_cell_width(&opts).max(1.0),
            line_h,
            body_w,
            None,
        );
        pane.register_block_wrap_row_spans(
            line_ix,
            ranges
                .iter()
                .map(|&(start, end)| MarkdownWrapRow {
                    start,
                    len: end.saturating_sub(start),
                })
                .collect(),
        );
        pane.register_block_wrap_hit_stops(
            line_ix,
            ranges
                .iter()
                .map(|&(start, end)| MarkdownWrapHitRow {
                    start,
                    stops: stops[start..=end]
                        .iter()
                        .map(|stop| stop - stops[start])
                        .collect(),
                })
                .collect(),
        );
        for (row_ix, &(start, end)) in ranges.iter().enumerate() {
            let row: String = chars[start..end].iter().collect();
            draw_if_visible(
                sugarloaf,
                body_x,
                current_y + row_ix as f32 * line_h,
                if row.is_empty() { " " } else { &row },
                &opts,
                clip_top,
                clip_bottom,
                text_occlusions,
            );
        }
        set_cursor_for_code_line(
            sugarloaf,
            pane,
            item,
            line_ix,
            output.text,
            body_x,
            current_y,
            &opts,
        );
        current_y += text_h + preview_h;
    }

    if let Some(elapsed) = elapsed {
        draw_if_visible(
            sugarloaf,
            x,
            current_y,
            elapsed,
            &detail_opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
        current_y += line_height(&detail_opts);
    }

    current_y - y + 1.0
}

fn draw_markdown_line(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    item: &VirtualMarkdownDrawItem,
    x: f32,
    width: f32,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
    animation_phase: f32,
) {
    let opts = DrawOpts {
        font_size: markdown_font(17.0, font_scale),
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let line_h = line_height(&opts);
    let mut text_y = item.screen_y + 3.0;
    let first_local_ix = 0usize;
    text_y += first_local_ix as f32 * line_h;
    let mut in_code_block =
        item.text
            .lines()
            .take(first_local_ix)
            .fold(false, |in_code, line| {
                if line.trim_start().starts_with("```") {
                    !in_code
                } else {
                    in_code
                }
            });
    let local_lines = virtual_item_lines(&item.text);
    let frontmatter = pane.frontmatter_range();
    let mut skip_until_local_ix = 0usize;
    for (local_ix, raw_line) in local_lines.iter().enumerate().skip(first_local_ix) {
        if local_ix < skip_until_local_ix {
            continue;
        }
        let raw = raw_line.trim_end_matches(['\r', '\n']);
        // Only a truly EMPTY line borrows a " " placeholder (so it still has
        // a row/caret slot). A whitespace-only line keeps its real chars —
        // collapsing "  " to " " capped the caret at col 1, so Tab on an
        // empty line visibly moved the cursor once and then never again.
        let text = if raw.is_empty() { " " } else { raw };
        let line_ix = item.first_line + local_ix;
        if parse_notebook_output_line(raw).is_some() {
            let start_local_ix = local_ix;
            let mut outputs = Vec::new();
            let mut group_end = local_ix;
            while group_end < local_lines.len() {
                let raw = local_lines[group_end].trim_end_matches(['\r', '\n']);
                let Some(output) = parse_notebook_output_line(raw) else {
                    break;
                };
                outputs.push(output);
                group_end += 1;
            }
            text_y += draw_notebook_output_group(
                sugarloaf,
                pane,
                item,
                start_local_ix,
                &outputs,
                x,
                text_y,
                width,
                clip,
                clip_top,
                clip_bottom,
                theme,
                text_occlusions,
                font_scale,
                animation_phase,
            );
            skip_until_local_ix = group_end;
            continue;
        }
        // YAML frontmatter renders as Obsidian-style properties (label, key:
        // value rows, tag chips). The cursor's line falls through to the
        // normal raw reveal so every property stays editable in place.
        if let Some(fm) = frontmatter
            .as_ref()
            .filter(|fm| fm.contains(&line_ix) && pane.cursor_line != line_ix)
        {
            text_y += draw_frontmatter_row(
                sugarloaf,
                pane,
                line_ix,
                text,
                x,
                text_y,
                width,
                line_h,
                fm.start,
                &opts,
                theme,
                clip,
                clip_top,
                clip_bottom,
                text_occlusions,
                font_scale,
            );
            if text_y > clip_bottom + line_h * 2.0 {
                break;
            }
            continue;
        }
        let code_fence = text.trim_start().starts_with("```");
        if in_code_block || code_fence {
            let code_opts = DrawOpts {
                font_size: markdown_font(15.0, font_scale),
                color: theme.u8(theme.muted),
                clip_rect: Some(clip),
                ..DrawOpts::default()
            };
            let code_h = line_height(&code_opts);
            draw_rounded_rect_clipped(
                sugarloaf,
                clip,
                x - 10.0,
                text_y - 3.0,
                width + 20.0,
                code_h + 6.0,
                3.0,
                theme.f32_alpha(theme.surface, 0.76),
                DEPTH,
                ORDER_BG + 1,
            );
            pane.register_block_rect(
                line_ix,
                [x - 10.0, text_y - 3.0, width + 20.0, code_h + 6.0],
                [x - 42.0, text_y - 3.0, 22.0, code_h + 6.0],
                x,
                text_y,
                0,
                cursor_cell_width(&code_opts).max(1.0),
                code_h,
                width,
                None,
            );
            draw_selection_for_line(
                sugarloaf,
                pane,
                line_ix,
                text,
                x,
                text_y,
                0,
                code_h,
                width,
                &code_opts,
                theme,
                clip,
                clip_top,
                clip_bottom,
            );
            draw_search_matches_for_line(
                sugarloaf,
                pane,
                line_ix,
                text,
                x,
                text_y,
                0,
                code_h,
                width,
                &code_opts,
                theme,
                clip,
                clip_top,
                clip_bottom,
            );
            if code_fence {
                // Hide the raw ``` fence: the opening fence shows the language
                // label like the git-diff card header; the closing fence is blank.
                if !in_code_block {
                    let label = text.trim_start().trim_start_matches('`').trim();
                    let header_opts = DrawOpts {
                        font_size: markdown_font(12.0, font_scale),
                        color: theme.u8(theme.fg),
                        bold: true,
                        clip_rect: Some(clip),
                        ..DrawOpts::default()
                    };
                    draw_if_visible(
                        sugarloaf,
                        x,
                        text_y,
                        label,
                        &header_opts,
                        clip_top,
                        clip_bottom,
                        text_occlusions,
                    );
                }
            } else {
                draw_virtualized_code_line(
                    sugarloaf,
                    x,
                    text_y,
                    text,
                    virtual_code_lang_for_line(&local_lines, local_ix, in_code_block),
                    &code_opts,
                    theme,
                    text_occlusions,
                );
            }
            set_cursor_for_code_line(
                sugarloaf, pane, item, line_ix, text, x, text_y, &code_opts,
            );
            text_y += code_h;
            if code_fence {
                in_code_block = !in_code_block;
            }
            if text_y > clip_bottom + line_h * 2.0 {
                break;
            }
            continue;
        }
        if let Some(table) = parse_table(&local_lines, local_ix) {
            text_y = render_table_with_source_base(
                sugarloaf,
                pane,
                &table,
                &local_lines,
                item.first_line,
                line_ix,
                pane.cursor_line,
                pane.cursor_col,
                x,
                text_y,
                width,
                clip,
                clip_top,
                clip_bottom,
                theme,
                mouse,
                text_occlusions,
                font_scale,
            );
            skip_until_local_ix = table.end_line;
            if text_y > clip_bottom + line_h * 2.0 {
                break;
            }
            continue;
        }
        if let Some((level, marker_len, heading_text)) = parse_heading_line(text) {
            let heading_opts = DrawOpts {
                font_size: markdown_font(
                    heading_level_font_size(level as usize),
                    font_scale,
                ),
                color: theme.u8(theme.fg),
                bold: true,
                clip_rect: Some(clip),
                font_id: md_font_id(sugarloaf),
                ..DrawOpts::default()
            };
            let heading_h = line_height(&heading_opts);
            // Live Preview: reveal the raw `### ` markup on the cursor's own
            // line (mirrors draw_heading). Without this the marker stays
            // hidden, so a space typed at/inside it vanished into the
            // swallowed prefix and the caret (clamped to marker_len) froze.
            let reveal = pane.cursor_line == line_ix;
            let (marker_len, heading_text) = if reveal {
                (0usize, text)
            } else {
                (marker_len, heading_text)
            };
            draw_selection_for_line(
                sugarloaf,
                pane,
                line_ix,
                text,
                x,
                text_y,
                marker_len,
                heading_h,
                width,
                &heading_opts,
                theme,
                clip,
                clip_top,
                clip_bottom,
            );
            draw_search_matches_for_line(
                sugarloaf,
                pane,
                line_ix,
                text,
                x,
                text_y,
                marker_len,
                heading_h,
                width,
                &heading_opts,
                theme,
                clip,
                clip_top,
                clip_bottom,
            );
            if reveal {
                // Draw the buffer verbatim so the identity caret map matches
                // the glyphs 1:1 (no inline-marker hiding, no ws collapsing).
                draw_if_visible(
                    sugarloaf,
                    x,
                    text_y,
                    heading_text,
                    &heading_opts,
                    clip_top,
                    clip_bottom,
                    text_occlusions,
                );
            } else {
                draw_inline_unwrapped_text(
                    sugarloaf,
                    pane,
                    heading_text,
                    x,
                    text_y,
                    &heading_opts,
                    theme,
                    clip,
                    clip_top,
                    clip_bottom,
                    text_occlusions,
                );
            }
            set_cursor_for_source_line(
                sugarloaf,
                pane,
                item,
                line_ix,
                text,
                x,
                text_y,
                width,
                marker_len,
                0.0,
                &heading_opts,
            );
            text_y += heading_h;
            if text_y > clip_bottom + line_h * 2.0 {
                break;
            }
            continue;
        }
        // Live Preview: the cursor's own divider line reveals its raw `---`
        // (generic path below) so the caret stays visible and editable —
        // the rendered rule had no caret slot at all.
        if is_divider(text.trim()) && pane.cursor_line != line_ix {
            pane.register_block_rect(
                line_ix,
                [x - 18.0, text_y - 4.0, width + 36.0, line_h + 8.0],
                [x - 42.0, text_y - 4.0, 22.0, line_h + 8.0],
                x,
                text_y,
                0,
                cursor_cell_width(&opts).max(1.0),
                line_h,
                width,
                None,
            );
            draw_rect_clipped(
                sugarloaf,
                clip,
                x,
                text_y + line_h * 0.5,
                width,
                1.4,
                theme.f32_alpha(theme.border, 0.72),
                DEPTH,
                ORDER_BG + 2,
            );
            text_y += 18.0;
            if text_y > clip_bottom + line_h * 2.0 {
                break;
            }
            continue;
        }
        let (text_x, body, marker_len, list_depth) = draw_line_marker(
            sugarloaf, pane, line_ix, text, x, text_y, width, clip, theme, font_scale,
        );
        let checked_task = task_marker_checked(text);
        let body_width = if pane.cursor_line == line_ix {
            width.max(24.0)
        } else {
            (width - (text_x - x)).max(24.0)
        };
        // Blockquotes render styled (accent bar + italic body, `> ` hidden)
        // except on the cursor's own line, which reveals raw like everything.
        let quote_styled =
            pane.cursor_line != line_ix && quote_marker_len(text).is_some();
        let line_opts = if quote_styled {
            DrawOpts {
                italic: true,
                color: theme.u8_alpha(theme.fg, 0.92),
                ..opts
            }
        } else {
            opts
        };
        let table_line = looks_like_inline_table_line(text);
        if table_line {
            draw_rounded_rect_clipped(
                sugarloaf,
                clip,
                x - 8.0,
                text_y - 3.0,
                width + 16.0,
                line_h + 6.0,
                3.0,
                theme.f32_alpha(theme.surface, 0.52),
                DEPTH,
                ORDER_BG + 1,
            );
        }
        // Obsidian Live Preview: the cursor's own line renders raw (markup shown
        // literally) so the drawn text equals the buffer and the caret maps 1:1.
        // Its wrapped rows hang at the indent+marker prefix width so the body
        // column holds across wraps like the rendered view.
        let raw_hang = if pane.cursor_line == line_ix {
            raw_line_hang_px(sugarloaf, text, &line_opts)
        } else {
            0.0
        };
        let inline_lines = if pane.cursor_line == line_ix {
            inline_wrapped_lines_raw(sugarloaf, body, body_width, raw_hang, &line_opts)
        } else {
            inline_wrapped_lines_dropcap(sugarloaf, body, body_width, &line_opts).0
        };
        let visual_lines = inline_visual_row_count(&inline_lines);
        // Draw the nesting tree guide now that we know how many visual rows
        // this list item occupies, so the guide spans the WHOLE item (incl.
        // wrapped lines) and overlaps the next item's guide — consecutive
        // list rows then read as one connected line instead of detached
        // dashes. Height matches the single-line case (+6) but multiplied by
        // the wrapped row count.
        if list_depth > 0 {
            draw_list_guides(
                sugarloaf,
                x,
                text_y - 2.0,
                line_h * visual_lines as f32 + 6.0,
                list_depth,
                theme,
                clip,
            );
        }
        if quote_styled {
            // Accent bar spanning every wrapped row of the quote.
            draw_rounded_rect_clipped(
                sugarloaf,
                clip,
                x + 4.0,
                text_y - 1.0,
                3.5,
                line_h * visual_lines as f32 + 2.0,
                1.75,
                theme.f32_alpha(theme.accent, 0.85),
                DEPTH,
                ORDER_BG + 2,
            );
        }
        let attachment_preview_h =
            pane.notebook_image_preview_extra_h(line_ix, body_width, font_scale);
        let block_h = line_h * visual_lines as f32 + 8.0 + attachment_preview_h;
        pane.register_block_wrap_row_spans(line_ix, inline_wrap_rows(&inline_lines));
        pane.register_block_wrap_hit_stops(
            line_ix,
            measured_inline_wrap_hit_rows(sugarloaf, &inline_lines, raw_hang, &line_opts),
        );
        pane.register_block_rect(
            line_ix,
            [x - 18.0, text_y - 4.0, width + 36.0, block_h],
            [x - 42.0, text_y - 4.0, 22.0, block_h],
            text_x,
            text_y,
            marker_len,
            cursor_cell_width(&line_opts).max(1.0),
            line_h,
            body_width,
            None,
        );
        draw_selection_for_line(
            sugarloaf,
            pane,
            line_ix,
            text,
            text_x,
            text_y,
            marker_len,
            line_h,
            body_width,
            &line_opts,
            theme,
            clip,
            clip_top,
            clip_bottom,
        );
        draw_search_matches_for_line(
            sugarloaf,
            pane,
            line_ix,
            text,
            text_x,
            text_y,
            marker_len,
            line_h,
            body_width,
            &line_opts,
            theme,
            clip,
            clip_top,
            clip_bottom,
        );
        draw_inline_wrapped_lines(
            sugarloaf,
            pane,
            &inline_lines,
            text_x,
            text_y,
            raw_hang,
            &line_opts,
            theme,
            clip,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
        if checked_task {
            draw_checked_task_strike(
                sugarloaf,
                &inline_lines,
                text_x,
                text_y,
                raw_hang,
                line_h,
                &line_opts,
                theme,
                clip,
                clip_top,
                clip_bottom,
            );
        }
        set_cursor_for_source_line(
            sugarloaf, pane, item, line_ix, text, text_x, text_y, body_width, marker_len,
            raw_hang, &line_opts,
        );
        text_y += line_h * visual_lines as f32 + attachment_preview_h;
        if text_y > clip_bottom + line_h * 2.0 {
            break;
        }
    }
}

/// One frontmatter row: the opening `---` becomes a "PROPERTIES" label, the
/// closing `---` a faint rule, and `key: value` lines render as muted key +
/// value (tags as chips). Returns the vertical advance, which matches what
/// `measure_item` assigned this line (18px for divider rows, wrapped
/// paragraph rows otherwise) so the styled rows never drift from the layout.
#[allow(clippy::too_many_arguments)]
fn draw_frontmatter_row(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    line_ix: usize,
    raw: &str,
    x: f32,
    y: f32,
    width: f32,
    line_h: f32,
    fm_start: usize,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    occlusions: &[[f32; 4]],
    font_scale: f32,
) -> f32 {
    if raw.trim() == "---" {
        let source_lines =
            inline_wrapped_lines_raw(sugarloaf, raw, width.max(24.0), 0.0, opts);
        pane.register_block_wrap_row_spans(line_ix, inline_wrap_rows(&source_lines));
        pane.register_block_wrap_hit_stops(
            line_ix,
            measured_inline_wrap_hit_rows(sugarloaf, &source_lines, 0.0, opts),
        );
        pane.register_block_rect(
            line_ix,
            [x - 18.0, y - 4.0, width + 36.0, 26.0],
            [x - 42.0, y - 4.0, 22.0, 26.0],
            x,
            y,
            0,
            cursor_cell_width(opts).max(1.0),
            line_h,
            width,
            None,
        );
        if line_ix == fm_start {
            // Opening fence: intentionally unlabeled — the key/value rows
            // speak for themselves (the old "PROPERTIES" caption read as
            // clutter).
        } else {
            // Closing fence: a real divider separating the metadata
            // section from the page content (same weight as a `---` rule).
            draw_rect_clipped(
                sugarloaf,
                clip,
                x,
                y + 9.0,
                width,
                1.4,
                theme.f32_alpha(theme.border, 0.72),
                DEPTH,
                ORDER_BG + 2,
            );
        }
        return 18.0;
    }
    let (advance, _rows) =
        measure_frontmatter_row(sugarloaf, raw, width, line_h, opts, theme, font_scale, clip);
    let source_lines =
        inline_wrapped_lines_raw(sugarloaf, raw, width.max(24.0), 0.0, opts);
    pane.register_block_wrap_row_spans(line_ix, inline_wrap_rows(&source_lines));
    pane.register_block_wrap_hit_stops(
        line_ix,
        measured_inline_wrap_hit_rows(sugarloaf, &source_lines, 0.0, opts),
    );
    pane.register_block_rect(
        line_ix,
        [x - 18.0, y - 4.0, width + 36.0, advance + 8.0],
        [x - 42.0, y - 4.0, 22.0, advance + 8.0],
        x,
        y,
        0,
        cursor_cell_width(opts).max(1.0),
        line_h,
        width,
        None,
    );
    let Some((key, value)) = raw.split_once(':') else {
        let plain_opts = DrawOpts {
            font_size: markdown_font(15.0, font_scale),
            color: theme.u8_alpha(theme.muted, 0.9),
            clip_rect: Some(clip),
            font_id: md_font_id(sugarloaf),
            ..DrawOpts::default()
        };
        let plain_lines =
            inline_wrapped_lines_raw(sugarloaf, raw.trim(), width.max(24.0), 0.0, &plain_opts);
        draw_inline_wrapped_lines(
            sugarloaf,
            pane,
            &plain_lines,
            x,
            y,
            0.0,
            &plain_opts,
            theme,
            clip,
            clip_top,
            clip_bottom,
            occlusions,
        );
        return advance;
    };
    let key_label = key.trim();
    let key_opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8_alpha(theme.muted, 0.95),
        clip_rect: Some(clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    draw_if_visible(
        sugarloaf,
        x,
        y + 1.0,
        key_label,
        &key_opts,
        clip_top,
        clip_bottom,
        occlusions,
    );
    let key_w = sugarloaf.text_mut().measure(key_label, &key_opts);
    let value_x = x + (key_w + 18.0).max(96.0);
    let value_width = (x + width - value_x).max(48.0);
    let value = value.trim();
    if matches!(key_label.to_ascii_lowercase().as_str(), "tags" | "tag") {
        let tag_opts = DrawOpts {
            font_size: markdown_font(13.0, font_scale),
            color: theme.u8(theme.green),
            clip_rect: Some(clip),
            font_id: md_font_id(sugarloaf),
            ..DrawOpts::default()
        };
        let mut chip_x = value_x;
        let mut chip_y = y;
        let max_x = x + width;
        for tag in value
            .split(&[',', ' ', '[', ']', '#', '"'][..])
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
        {
            let label = format!("#{tag}");
            let label_w = sugarloaf.text_mut().measure(&label, &tag_opts);
            let chip_w = label_w + 12.0;
            if chip_x > value_x && chip_x + chip_w > max_x {
                chip_x = value_x;
                chip_y += line_h;
            }
            draw_rounded_rect_clipped(
                sugarloaf,
                clip,
                chip_x - 6.0,
                chip_y - 1.0,
                chip_w,
                line_h - 2.0,
                (line_h - 2.0) * 0.5,
                theme.f32_alpha(theme.surface, 0.95),
                DEPTH,
                ORDER_BG + 2,
            );
            draw_if_visible(
                sugarloaf,
                chip_x,
                chip_y + 1.0,
                &label,
                &tag_opts,
                clip_top,
                clip_bottom,
                occlusions,
            );
            chip_x += label_w + 22.0;
        }
    } else {
        let value_opts = DrawOpts {
            font_size: markdown_font(15.0, font_scale),
            color: theme.u8(theme.fg),
            clip_rect: Some(clip),
            font_id: md_font_id(sugarloaf),
            ..DrawOpts::default()
        };
        let value_lines =
            inline_wrapped_lines(sugarloaf, value, value_width, &value_opts);
        draw_inline_wrapped_lines(
            sugarloaf,
            pane,
            &value_lines,
            value_x,
            y,
            0.0,
            &value_opts,
            theme,
            clip,
            clip_top,
            clip_bottom,
            occlusions,
        );
    }
    advance
}

#[allow(clippy::too_many_arguments)]
fn measure_frontmatter_row(
    sugarloaf: &mut Sugarloaf,
    raw: &str,
    width: f32,
    line_h: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    font_scale: f32,
    clip: [f32; 4],
) -> (f32, usize) {
    if raw.trim() == "---" {
        return (18.0, 1);
    }
    let source_rows = inline_visual_row_count(&inline_wrapped_lines_raw(
        sugarloaf,
        raw,
        width.max(24.0),
        0.0,
        opts,
    ));
    let styled_rows = if let Some((key, value)) = raw.split_once(':') {
        let key_label = key.trim();
        let key_opts = DrawOpts {
            font_size: markdown_font(14.0, font_scale),
            color: theme.u8_alpha(theme.muted, 0.95),
            clip_rect: Some(clip),
            font_id: md_font_id(sugarloaf),
            ..DrawOpts::default()
        };
        let key_w = sugarloaf.text_mut().measure(key_label, &key_opts);
        let value_x_offset = (key_w + 18.0).max(96.0);
        let value_width = (width - value_x_offset).max(48.0);
        let value = value.trim();
        if matches!(key_label.to_ascii_lowercase().as_str(), "tags" | "tag") {
            frontmatter_tag_rows(sugarloaf, value, value_width, font_scale, theme, clip)
        } else {
            let value_opts = DrawOpts {
                font_size: markdown_font(15.0, font_scale),
                color: theme.u8(theme.fg),
                clip_rect: Some(clip),
                font_id: md_font_id(sugarloaf),
                ..DrawOpts::default()
            };
            inline_visual_row_count(&inline_wrapped_lines(
                sugarloaf,
                value,
                value_width,
                &value_opts,
            ))
        }
    } else {
        let plain_opts = DrawOpts {
            font_size: markdown_font(15.0, font_scale),
            color: theme.u8_alpha(theme.muted, 0.9),
            clip_rect: Some(clip),
            font_id: md_font_id(sugarloaf),
            ..DrawOpts::default()
        };
        inline_visual_row_count(&inline_wrapped_lines_raw(
            sugarloaf,
            raw.trim(),
            width.max(24.0),
            0.0,
            &plain_opts,
        ))
    };
    let rows = source_rows.max(styled_rows).max(1);
    (line_h * rows as f32, rows)
}

fn frontmatter_tag_rows(
    sugarloaf: &mut Sugarloaf,
    value: &str,
    value_width: f32,
    font_scale: f32,
    theme: &IdeTheme,
    clip: [f32; 4],
) -> usize {
    let tag_opts = DrawOpts {
        font_size: markdown_font(13.0, font_scale),
        color: theme.u8(theme.green),
        clip_rect: Some(clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let mut rows = 1usize;
    let mut row_w = 0.0f32;
    for tag in value
        .split(&[',', ' ', '[', ']', '#', '"'][..])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
    {
        let label = format!("#{tag}");
        let chip_w = sugarloaf.text_mut().measure(&label, &tag_opts) + 22.0;
        if row_w > 0.0 && row_w + chip_w > value_width {
            rows += 1;
            row_w = 0.0;
        }
        row_w += chip_w;
    }
    rows
}

fn draw_line_marker<'a>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    line_ix: usize,
    raw: &'a str,
    x: f32,
    y: f32,
    width: f32,
    clip: [f32; 4],
    theme: &IdeTheme,
    font_scale: f32,
) -> (f32, &'a str, usize, usize) {
    // Obsidian Live Preview: on the cursor's own line reveal the raw markup —
    // don't draw the bullet/checkbox glyph or strip the `- `/`1. `/`[ ] ` marker,
    // so the laid-out text matches the buffer and cursor/click/wrap math is
    // identity. (Full Notion-style hide — marker gone even here — can come later.)
    if pane.cursor_line == line_ix {
        return (x, raw, 0, 0);
    }
    let Some(marker) = parse_markdown_list_marker(raw) else {
        // Blockquote: hide the `> ` marker and indent the body past the
        // accent bar (the caller draws the bar once it knows the wrapped
        // row count).
        if let Some(quote_len) = quote_marker_len(raw) {
            let quote_len = quote_len.min(raw.len());
            let body = raw.get(quote_len..).unwrap_or_default();
            return (x + QUOTE_BODY_INDENT, body, quote_len, 0);
        }
        return (x, raw, pane.visible_start_col(line_ix).min(raw.len()), 0);
    };
    let opts = DrawOpts {
        font_size: markdown_font(16.0, font_scale),
        color: theme.u8_alpha(theme.muted, 0.9),
        clip_rect: Some(clip),
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    // Markers/checkboxes draw at their own 16px display size but must be
    // vertically centered against the 17px BODY text they sit beside —
    // centering on the 16px marker size leaves them sitting a touch high.
    let body_font = markdown_font(17.0, font_scale);
    let (body_offset, body, marker_len) = line_marker_layout(raw, width, &opts);
    let cell_w = cursor_cell_width(&opts).max(1.0);
    let (depth, indent_px, _) = list_marker_metrics(&marker, cell_w);
    // Marker sits at the nesting indent; the body (returned below) sits one
    // marker-slot further right. The guides share this indent so the bullets,
    // checkboxes, and the vertical tree lines all align.
    let indent_x = x + indent_px;
    // The vertical tree guide is drawn by the caller (`draw_markdown_line`)
    // AFTER the body wrap count is known, so the guide spans the full
    // height of a wrapped list item and connects to the next item's guide
    // instead of leaving a gap. We just hand `depth` back.
    match marker.kind {
        MarkdownListMarkerKind::Task { .. } => {
            let checked = task_marker_checked(raw);
            let size = 13.0 * font_scale;
            let box_y = bullet_align::checkbox_y(y, body_font, size);
            let rect = [indent_x, box_y, size, size];
            pane.register_task_rect(line_ix, rect);
            draw_task_checkbox(
                sugarloaf, clip, indent_x, box_y, checked, theme, font_scale,
            );
        }
        MarkdownListMarkerKind::Bullet(marker) => {
            let glyph = if marker == '-' {
                "•".to_string()
            } else {
                marker.to_string()
            };
            draw_if_visible(
                sugarloaf,
                indent_x,
                bullet_align::text_marker_y(y, body_font, opts.font_size),
                &glyph,
                &opts,
                clip[1],
                clip[1] + clip[3],
                &[],
            );
        }
        MarkdownListMarkerKind::Number { .. } | MarkdownListMarkerKind::Letter { .. } => {
            let label = raw
                .get(marker.indent..marker.marker_len)
                .unwrap_or_default()
                .trim();
            draw_if_visible(
                sugarloaf,
                indent_x,
                bullet_align::text_marker_y(y, body_font, opts.font_size),
                label,
                &opts,
                clip[1],
                clip[1] + clip[3],
                &[],
            );
        }
    }
    (x + body_offset, body, marker_len, depth)
}

fn draw_checked_task_strike(
    sugarloaf: &mut Sugarloaf,
    lines: &[InlineWrappedLine],
    x: f32,
    y: f32,
    hang_px: f32,
    line_h: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
) {
    let visible_lines = if lines.is_empty() { 1 } else { lines.len() };
    for ix in 0..visible_lines {
        let line_y = y + ix as f32 * line_h;
        if line_y + line_h < clip_top || line_y > clip_bottom {
            continue;
        }
        let line_w = lines
            .get(ix)
            .map(|line| sugarloaf.text_mut().measure(&line.text, opts))
            .unwrap_or_else(|| cursor_cell_width(opts));
        draw_rect_clipped(
            sugarloaf,
            clip,
            x + if ix > 0 { hang_px } else { 0.0 },
            // Centre the strike through the glyph body (font-relative, so it
            // tracks the text rather than sitting low in the line box).
            line_y + opts.font_size * 0.5,
            line_w.max(cursor_cell_width(opts)),
            1.5,
            theme.f32_alpha(theme.red, 0.9),
            DEPTH,
            ORDER_TEXT + 2,
        );
    }
}
