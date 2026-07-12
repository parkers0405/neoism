fn draw_code_block(
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
    animation_phase: f32,
) {
    if !virtual_code_cursor_inside(pane, item) {
        if let Some(rendered) = draw_mermaid_code_block(
            sugarloaf,
            item,
            x,
            width,
            clip,
            clip_top,
            clip_bottom,
            theme,
            font_scale,
        ) {
            if rendered {
                return;
            }
        }
    }

    let header_h = CODE_BLOCK_HEADER_H;
    let top_pad = CODE_BLOCK_BODY_PAD;
    let block_x = x - 12.0;
    let block_y = item.screen_y + 2.0;
    let block_w = width + 24.0;
    let block_h = (item.bounds.height - 4.0).max(1.0);

    let opts = DrawOpts {
        font_size: markdown_font(15.0, font_scale),
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let line_h = line_height(&opts);
    let local_lines = virtual_item_lines(&item.text);
    let opening_fence = local_lines
        .iter()
        .find(|line| line.trim_start().starts_with("```"))
        .map(|line| line.as_str());
    let raw_lang_label = opening_fence
        .map(|line| line.trim_start().trim_start_matches('`').trim())
        .filter(|label| !label.is_empty())
        .unwrap_or_default();
    let code_lang_label = opening_fence
        .map(virtual_markdown_code_label)
        .filter(|label| !label.is_empty())
        .unwrap_or_default();
    let lang = virtual_markdown_code_lang(code_lang_label);
    if let Some(meta) = parse_notebook_output_meta(raw_lang_label) {
        draw_notebook_output_block(
            sugarloaf,
            pane,
            item,
            &local_lines,
            &meta,
            x,
            width,
            clip,
            clip_top,
            clip_bottom,
            theme,
            text_occlusions,
            font_scale,
            animation_phase,
        );
        return;
    }
    let notebook_meta = parse_notebook_code_meta(raw_lang_label);
    let lang_label = notebook_meta
        .as_ref()
        .map(|meta| meta.lang.as_str())
        .unwrap_or(code_lang_label);

    // Border ring, mirroring the git-diff card frame: a slightly larger
    // rounded backing in `theme.border` so the body + header fills above it
    // leave a clean stroke around the whole card.
    let stroke = 1.0;
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        block_x - stroke,
        block_y - stroke,
        block_w + stroke * 2.0,
        block_h + stroke * 2.0,
        BLOCK_RADIUS + stroke,
        theme.f32(theme.border),
        DEPTH,
        ORDER_BG,
    );
    // Body fill — the git-diff card body sits on the page background.
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        block_x,
        block_y,
        block_w,
        block_h,
        BLOCK_RADIUS,
        theme.f32(theme.bg),
        DEPTH,
        ORDER_BG + 1,
    );
    // Header bar (rounded top only) in `theme.surface`, like the git-diff
    // card header; the square-off rect hides the rounded bottom corners.
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        block_x,
        block_y,
        block_w,
        header_h,
        BLOCK_RADIUS,
        theme.f32(theme.surface),
        DEPTH,
        ORDER_BG + 2,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        block_x,
        block_y + header_h - BLOCK_RADIUS,
        block_w,
        BLOCK_RADIUS,
        theme.f32(theme.surface),
        DEPTH,
        ORDER_BG + 2,
    );
    draw_rect_clipped(
        sugarloaf,
        clip,
        block_x,
        block_y + header_h,
        block_w,
        1.0,
        theme.f32_alpha(theme.border, 0.7),
        DEPTH,
        ORDER_BG + 3,
    );
    // Live Preview for the opening fence: with the cursor on it, the header
    // shows the raw ```lang text (editable — backspace the lang, retype, or
    // delete the fence itself) instead of the pretty label.
    let open_fence_revealed = pane.cursor_line == item.first_line
        && local_lines
            .first()
            .is_some_and(|line| line.trim_start().starts_with("```"));
    let header_text_y = block_y + (header_h - opts.font_size) * 0.5;
    let copy_size = 22.0;
    let copy_rect = [
        block_x + block_w - copy_size - 8.0,
        block_y + (header_h - copy_size) * 0.5,
        copy_size,
        copy_size,
    ];
    if open_fence_revealed {
        let fence_line = local_lines.first().map(String::as_str).unwrap_or("");
        register_code_row_geometry(
            sugarloaf,
            pane,
            item.first_line,
            fence_line,
            [block_x, block_y, block_w, header_h],
            x,
            header_text_y,
            width,
            line_h,
            &opts,
        );
        draw_if_visible(
            sugarloaf,
            x,
            header_text_y,
            fence_line,
            &opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
    } else {
        let header_opts = DrawOpts {
            font_size: markdown_font(12.0, font_scale),
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        let label_x = block_x + 14.0;
        let mut action_left = copy_rect[0];
        if let Some(meta) = notebook_meta.as_ref() {
            let button_gap = 6.0;
            let mut action_right = copy_rect[0] - 8.0;
            let mut hovered_tooltip = None;
            let action_specs = [
                (
                    crate::editor::notebook::NotebookCellAction::ClearOutput,
                    "\u{f12d}",
                    "",
                    theme.u8(theme.yellow),
                    28.0,
                ),
                (
                    crate::editor::notebook::NotebookCellAction::RunAndBelow,
                    "\u{f063}",
                    "",
                    theme.u8(theme.blue),
                    28.0,
                ),
                (
                    crate::editor::notebook::NotebookCellAction::Run,
                    if meta.running { "\u{f110}" } else { "\u{f04b}" },
                    if meta.running { "Running..." } else { "Run" },
                    theme.u8(if meta.running {
                        theme.cyan
                    } else {
                        theme.green
                    }),
                    if meta.running { 92.0 } else { 60.0 },
                ),
            ];
            for (action, icon, label, icon_color, min_w) in action_specs {
                let icon_opts = DrawOpts {
                    font_size: markdown_font(12.0, font_scale),
                    color: icon_color,
                    bold: true,
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let label_opts = DrawOpts {
                    font_size: markdown_font(12.0, font_scale),
                    color: theme.u8(theme.fg),
                    bold: true,
                    clip_rect: Some(clip),
                    ..DrawOpts::default()
                };
                let icon_w = sugarloaf.text_mut().measure(icon, &icon_opts);
                let label_w = if label.is_empty() {
                    0.0
                } else {
                    sugarloaf.text_mut().measure(label, &label_opts)
                };
                let label_gap = if label.is_empty() { 0.0 } else { 8.0 };
                let button_w = (icon_w + label_gap + label_w + 16.0).max(min_w);
                let rect = [
                    action_right - button_w,
                    block_y + 5.0,
                    button_w,
                    header_h - 10.0,
                ];
                let hovered = pane.notebook_action_hovered.is_some_and(|hover| {
                    hover.cell_index == meta.cell_index && hover.action == action
                });
                draw_rounded_rect_clipped(
                    sugarloaf,
                    clip,
                    rect[0],
                    rect[1],
                    rect[2],
                    rect[3],
                    5.0,
                    theme.f32_alpha(
                        theme.hover,
                        if hovered {
                            0.98
                        } else if meta.running {
                            0.9
                        } else {
                            0.72
                        },
                    ),
                    DEPTH,
                    ORDER_BG + 4,
                );
                let content_w = icon_w + label_gap + label_w;
                let content_x = rect[0] + (rect[2] - content_w) * 0.5;
                draw_if_visible(
                    sugarloaf,
                    content_x,
                    block_y + (header_h - icon_opts.font_size) * 0.5 - 1.0,
                    icon,
                    &icon_opts,
                    clip_top,
                    clip_bottom,
                    text_occlusions,
                );
                if !label.is_empty() {
                    draw_if_visible(
                        sugarloaf,
                        content_x + icon_w + label_gap,
                        block_y + (header_h - label_opts.font_size) * 0.5,
                        label,
                        &label_opts,
                        clip_top,
                        clip_bottom,
                        text_occlusions,
                    );
                }
                pane.register_notebook_action_rect(rect, meta.cell_index, action);
                if hovered {
                    hovered_tooltip = Some((rect, action));
                }
                action_left = rect[0];
                action_right = rect[0] - button_gap;
            }
            if let Some((rect, action)) = hovered_tooltip {
                draw_notebook_cell_action_tooltip(
                    sugarloaf,
                    rect,
                    notebook_cell_action_tooltip(action),
                    clip,
                    clip_top,
                    clip_bottom,
                    theme,
                    font_scale,
                );
            }
        }
        let label = if let Some(meta) = notebook_meta.as_ref() {
            format!(
                "{} - In [{}]",
                lang_label,
                meta.count.as_deref().unwrap_or(" ")
            )
        } else {
            lang_label.to_string()
        };
        let label = truncate_to_fit(
            &label,
            (action_left - label_x - 12.0).max(24.0),
            sugarloaf,
            &header_opts,
        );
        draw_if_visible(
            sugarloaf,
            label_x,
            block_y + (header_h - header_opts.font_size) * 0.5,
            &label,
            &header_opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
        // Clicking the header still lands the caret on the fence line (the
        // fence snap in cursor_col_from_point puts it after ```lang).
        pane.register_block_rect(
            item.first_line,
            [block_x, block_y, block_w, header_h],
            [block_x - 30.0, block_y, 22.0, header_h],
            x,
            header_text_y,
            0,
            cursor_cell_width(&opts).max(1.0),
            line_h,
            width,
            None,
        );
    }
    // Copy button on the header's right edge, opposite the language label.
    // `copy_at` copies the inner code (lines between the fences).
    let code_end = item.first_line + local_lines.len().saturating_sub(1);
    pane.register_copy_code_rect(copy_rect, item.first_line, code_end);
    draw_copy_button(sugarloaf, copy_rect, theme, clip, font_scale);

    // Body: only the inner code lines. The ``` fences are intentionally not
    // drawn — the header already names the block, matching the git-diff card.
    // Long lines wrap (terminal-style, breaking after whitespace when one
    // fits the row) instead of running off the card.
    let mut row = 0usize;
    for (local_ix, line) in local_lines.iter().enumerate() {
        let source_line = item.first_line + local_ix;
        if line.trim_start().starts_with("```") {
            if local_ix == 0 {
                // Opening fence: drawn (raw or as label) in the header above.
                continue;
            }
            // Closing fence: revealed raw when the cursor sits on it, so the
            // ``` itself can be edited/deleted; otherwise it stays hidden but
            // keeps a click strip + caret slot at the card's footer.
            let fence_y = block_y + header_h + top_pad + row as f32 * line_h;
            if pane.cursor_line == source_line {
                register_code_row_geometry(
                    sugarloaf,
                    pane,
                    source_line,
                    line,
                    [block_x, fence_y, block_w, line_h],
                    x,
                    fence_y,
                    width,
                    line_h,
                    &opts,
                );
                draw_if_visible(
                    sugarloaf,
                    x,
                    fence_y,
                    line,
                    &opts,
                    clip_top,
                    clip_bottom,
                    text_occlusions,
                );
            } else {
                pane.register_block_rect(
                    source_line,
                    [block_x, fence_y, block_w, top_pad.max(10.0)],
                    [block_x - 30.0, fence_y, 22.0, line_h],
                    x,
                    fence_y,
                    0,
                    cursor_cell_width(&opts).max(1.0),
                    line_h,
                    width,
                    None,
                );
            }
            set_cursor_for_code_line(
                sugarloaf,
                pane,
                item,
                source_line,
                line,
                x,
                fence_y,
                &opts,
            );
            continue;
        }
        let stops = measured_stops_for_text(sugarloaf, line, &opts);
        let chars: Vec<char> = line.chars().collect();
        let ranges = code_wrap_ranges(&stops, &chars, width);
        let row_count = ranges.len().max(1);
        let line_y = block_y + header_h + top_pad + row as f32 * line_h;
        row += row_count;
        if line_y + row_count as f32 * line_h < clip_top || line_y > clip_bottom {
            continue;
        }
        // Every inner code line gets its own block rect + per-row measured
        // stops so a click lands the caret ON that visual row (identity-mapped
        // — code text is never markdown-cleaned). Without these the whole
        // card hit-tested to the opening fence line.
        pane.register_block_rect(
            source_line,
            [block_x, line_y, block_w, line_h * row_count as f32],
            [block_x - 30.0, line_y, 22.0, line_h * row_count as f32],
            x,
            line_y,
            0,
            cursor_cell_width(&opts).max(1.0),
            line_h,
            width,
            None,
        );
        pane.register_block_wrap_row_spans(
            source_line,
            ranges
                .iter()
                .map(|&(start, end)| MarkdownWrapRow {
                    start,
                    len: end - start,
                })
                .collect(),
        );
        pane.register_block_wrap_hit_stops(
            source_line,
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
        draw_selection_for_line(
            sugarloaf,
            pane,
            source_line,
            line,
            x,
            line_y,
            0,
            line_h,
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
            source_line,
            line,
            x,
            line_y,
            0,
            line_h,
            width,
            &opts,
            theme,
            clip,
            clip_top,
            clip_bottom,
        );
        for (row_ix, &(start, end)) in ranges.iter().enumerate() {
            let segment: String = chars[start..end].iter().collect();
            draw_virtualized_code_line(
                sugarloaf,
                x,
                line_y + row_ix as f32 * line_h,
                &segment,
                lang,
                &opts,
                theme,
                text_occlusions,
            );
        }
        set_cursor_for_code_line(
            sugarloaf,
            pane,
            item,
            source_line,
            line,
            x,
            line_y,
            &opts,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_mermaid_code_block(
    sugarloaf: &mut Sugarloaf,
    item: &VirtualMarkdownDrawItem,
    x: f32,
    width: f32,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    font_scale: f32,
) -> Option<bool> {
    let local_lines = virtual_item_lines(&item.text);
    let first = local_lines.first()?.trim_start();
    if !virtual_markdown_code_label(first).eq_ignore_ascii_case("mermaid") {
        return None;
    }
    let source = local_lines
        .iter()
        .skip(1)
        .take(local_lines.len().saturating_sub(2))
        .map(|line| line.trim_end_matches(['\r', '\n']))
        .collect::<Vec<_>>()
        .join("\n");
    let diagram = parse_mermaid_diagram(&source)?;

    let block_x = x - 12.0;
    let block_y = item.screen_y + 2.0;
    let block_w = width + 24.0;
    let block_h = (item.bounds.height - 4.0).max(1.0);
    if block_y + block_h < clip_top || block_y > clip_bottom {
        return Some(true);
    }
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        block_x,
        block_y,
        block_w,
        block_h,
        10.0,
        theme.f32(theme.panel_bg()),
        DEPTH,
        ORDER_BG,
    );
    let header_opts = DrawOpts {
        font_size: markdown_font(12.0, font_scale),
        color: theme.u8_alpha(theme.muted, 0.95),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    draw_if_visible(
        sugarloaf,
        x + 4.0,
        block_y + 9.0,
        "mermaid",
        &header_opts,
        clip_top,
        clip_bottom,
        &[],
    );
    let scene = mermaid_scene(&diagram, theme, font_scale);
    let Some(bounds) = scene.bounds() else {
        return Some(true);
    };
    let diagram_rect = [
        x,
        block_y + CODE_BLOCK_HEADER_H + 10.0,
        width,
        block_h - CODE_BLOCK_HEADER_H - 18.0,
    ];
    if diagram_rect[3] <= 0.0 {
        return Some(true);
    }
    let pad = 12.0 * font_scale;
    let avail_w = (diagram_rect[2] - pad * 2.0).max(1.0);
    let avail_h = (diagram_rect[3] - pad * 2.0).max(1.0);
    let zoom = (avail_w / bounds.width().max(1.0))
        .min(avail_h / bounds.height().max(1.0))
        .min(2.0);
    let center = bounds.center();
    let camera = Camera {
        pan: Vec2::new(
            diagram_rect[0] + diagram_rect[2] * 0.5 - center.x * zoom,
            diagram_rect[1] + diagram_rect[3] * 0.5 - center.y * zoom,
        ),
        zoom,
    };
    render_scene(sugarloaf, &scene, &camera, clip, DEPTH, ORDER_BG + 2);
    Some(true)
}

fn virtual_code_cursor_inside(
    pane: &MarkdownPane,
    item: &VirtualMarkdownDrawItem,
) -> bool {
    let line_count = virtual_item_lines(&item.text).len().max(1);
    let cursor = pane.cursor_line;
    cursor >= item.first_line && cursor < item.first_line + line_count
}

fn virtual_markdown_code_label(fence: &str) -> &str {
    fence
        .trim_start()
        .trim_start_matches('`')
        .trim_start_matches('~')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('.')
}

struct NotebookCodeMeta {
    cell_index: usize,
    lang: String,
    running: bool,
    count: Option<String>,
}

struct NotebookOutputMeta {
    prompt: String,
    elapsed: Option<String>,
    running: bool,
}

struct NotebookOutputLine<'a> {
    prompt: String,
    elapsed: Option<String>,
    running: bool,
    text: &'a str,
}

fn parse_notebook_code_meta(label: &str) -> Option<NotebookCodeMeta> {
    let mut parts = label.split_whitespace();
    let lang = parts.next()?.to_string();
    let mut cell_index = None;
    let mut running = false;
    let mut count = None;
    for part in parts {
        if let Some(value) = part.strip_prefix("neoism_notebook_cell=") {
            cell_index = value.parse::<usize>().ok();
        } else if let Some(value) = part.strip_prefix("neoism_state=") {
            running = value == "running";
        } else if let Some(value) = part.strip_prefix("neoism_count=") {
            if value != "_" {
                count = Some(value.to_string());
            }
        }
    }
    Some(NotebookCodeMeta {
        cell_index: cell_index?,
        lang,
        running,
        count,
    })
}

fn parse_notebook_output_meta(label: &str) -> Option<NotebookOutputMeta> {
    let mut parts = label.split_whitespace();
    let _lang = parts.next()?;
    let mut prompt = None;
    let mut elapsed = None;
    let mut running = false;
    for part in parts {
        if let Some(value) = part.strip_prefix("neoism_notebook_output=") {
            prompt = Some(unescape_notebook_meta(value));
        } else if let Some(value) = part.strip_prefix("neoism_elapsed=") {
            elapsed = Some(unescape_notebook_meta(value));
        } else if let Some(value) = part.strip_prefix("neoism_state=") {
            running = value == "running";
        }
    }
    Some(NotebookOutputMeta {
        prompt: prompt?,
        elapsed,
        running,
    })
}

fn unescape_notebook_meta(value: &str) -> String {
    if value == "_" {
        String::new()
    } else {
        value.replace('_', " ")
    }
}

fn notebook_cell_action_tooltip(
    action: crate::editor::notebook::NotebookCellAction,
) -> &'static str {
    match action {
        crate::editor::notebook::NotebookCellAction::Run => "Run Cell",
        crate::editor::notebook::NotebookCellAction::RunAndBelow => "Run Cell And Below",
        crate::editor::notebook::NotebookCellAction::ClearOutput => "Clear Cell Output",
    }
}

fn draw_notebook_running_loader(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    left: f32,
    row_top: f32,
    row_h: f32,
    font_size: f32,
    animation_phase: f32,
) {
    let slot_w = (font_size * 1.08).max(12.0);
    let side = (font_size * 0.86).min(row_h * 0.74).max(10.0);
    let half = side * 0.5;
    let dot = (side * 0.34).clamp(3.5, 5.8);
    let center_x = left + slot_w * 0.5;
    let center_y = row_top + row_h * 0.5;
    let loader_frame = crate::render_policy::loader_animation_frame(animation_phase);

    for (trail, alpha) in [1.0, 0.58, 0.32, 0.16].into_iter().enumerate() {
        let (dx, dy) = crate::render_policy::loader_orbit_position(
            loader_frame.phase - trail as f32 * 0.075,
            half,
        );
        let x = center_x + dx - dot * 0.5;
        let y = center_y + dy - dot * 0.5;
        let dot_rect = [x, y, dot, dot];
        if crate::primitives::intersect_rect(dot_rect, clip).is_none() {
            continue;
        }
        if trail <= 1 {
            let glow = dot * 1.85;
            draw_rounded_rect_clipped(
                sugarloaf,
                clip,
                center_x + dx - glow * 0.5,
                center_y + dy - glow * 0.5,
                glow,
                glow,
                glow * 0.5,
                crate::render_policy::loader_pastel_color(
                    loader_frame.tick,
                    trail,
                    alpha * 0.24,
                ),
                DEPTH,
                ORDER_BG + 7,
            );
        }
        draw_rounded_rect_clipped(
            sugarloaf,
            clip,
            x,
            y,
            dot,
            dot,
            dot * 0.42,
            crate::render_policy::loader_pastel_color(loader_frame.tick, trail, alpha),
            DEPTH,
            ORDER_BG + 8,
        );
    }
}

fn draw_notebook_cell_action_tooltip(
    sugarloaf: &mut Sugarloaf,
    anchor: [f32; 4],
    label: &str,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    font_scale: f32,
) {
    let font_size = markdown_font(11.0, font_scale);
    let opts = DrawOpts {
        font_size,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let pad_x = 7.0;
    let tooltip_h = 21.0;
    let tooltip_w = sugarloaf.text_mut().measure(label, &opts) + pad_x * 2.0;
    let margin = 5.0;
    let min_x = clip[0] + margin;
    let max_x = (clip[0] + clip[2] - tooltip_w - margin).max(min_x);
    let tooltip_x = (anchor[0] + anchor[2] * 0.5 - tooltip_w * 0.5).clamp(min_x, max_x);
    let above_y = anchor[1] - tooltip_h - margin;
    let tooltip_y = if above_y >= clip[1] + margin {
        above_y
    } else {
        anchor[1] + anchor[3] + margin
    };

    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        tooltip_x,
        tooltip_y,
        tooltip_w,
        tooltip_h,
        5.0,
        theme.f32(theme.surface),
        DEPTH,
        ORDER_BG + 8,
    );
    draw_if_visible(
        sugarloaf,
        tooltip_x + pad_x,
        tooltip_y + (tooltip_h - font_size) * 0.5,
        label,
        &opts,
        clip_top,
        clip_bottom,
        &[],
    );
}

fn parse_notebook_output_line(line: &str) -> Option<NotebookOutputLine<'_>> {
    let rest = line.strip_prefix("%%neoism_notebook_output ")?;
    let mut parts = rest.splitn(4, ' ');
    let prompt = unescape_notebook_meta(parts.next()?);
    let elapsed = parts
        .next()
        .filter(|value| *value != "_")
        .map(unescape_notebook_meta);
    let third = parts.next().unwrap_or_default();
    let (running, text) = if third == "neoism_state=running" {
        (true, parts.next().unwrap_or_default())
    } else {
        (false, third)
    };
    Some(NotebookOutputLine {
        prompt,
        elapsed,
        running,
        text,
    })
}

#[allow(clippy::too_many_arguments)]
fn draw_notebook_output_block(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    item: &VirtualMarkdownDrawItem,
    local_lines: &[String],
    meta: &NotebookOutputMeta,
    x: f32,
    width: f32,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    theme: &IdeTheme,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
    animation_phase: f32,
) {
    let opts = DrawOpts {
        font_size: markdown_font(14.0, font_scale),
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let prompt_opts = DrawOpts {
        font_size: markdown_font(13.0, font_scale),
        color: theme.u8(if meta.prompt.starts_with("Err") {
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
    let has_prompt = !meta.prompt.is_empty();
    let prompt_w = if has_prompt {
        76.0 * font_scale
    } else {
        0.0
    };
    let body_x = x + prompt_w;
    let body_w = (width - prompt_w).max(36.0);
    let prompt_y = item.screen_y + if has_prompt { 6.0 } else { 2.0 };

    if meta.running {
        draw_notebook_running_loader(
            sugarloaf,
            clip,
            body_x,
            prompt_y,
            line_h,
            markdown_font(14.0, font_scale),
            animation_phase,
        );
    } else if has_prompt {
        let prompt = format!("{}:", meta.prompt);
        draw_if_visible(
            sugarloaf,
            x,
            prompt_y,
            &prompt,
            &prompt_opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
    }
    if let Some(elapsed) = meta.elapsed.as_ref().filter(|elapsed| !elapsed.is_empty()) {
        draw_if_visible(
            sugarloaf,
            x,
            prompt_y + line_h,
            elapsed,
            &detail_opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
    }

    let mut y = prompt_y;
    let mut output_rows = 0usize;
    for (local_ix, line) in local_lines.iter().enumerate() {
        let source_line = item.first_line + local_ix;
        if line.trim_start().starts_with("```") {
            pane.register_block_rect(
                source_line,
                [body_x, y, body_w, line_h.max(1.0)],
                [x - 30.0, y, 22.0, line_h.max(1.0)],
                body_x,
                y,
                0,
                cursor_cell_width(&opts).max(1.0),
                line_h,
                body_w,
                None,
            );
            pane.register_block_wrap_row_spans(
                source_line,
                vec![MarkdownWrapRow { start: 0, len: 0 }],
            );
            pane.register_block_wrap_hit_stops(
                source_line,
                vec![MarkdownWrapHitRow {
                    start: 0,
                    stops: vec![0.0],
                }],
            );
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let stops = measured_stops_for_text(sugarloaf, line, &opts);
        let ranges = code_wrap_ranges(&stops, &chars, body_w);
        let row_count = ranges.len().max(1);
        pane.register_block_rect(
            source_line,
            [body_x, y, body_w, line_h * row_count as f32],
            [x - 30.0, y, 22.0, line_h * row_count as f32],
            body_x,
            y,
            0,
            cursor_cell_width(&opts).max(1.0),
            line_h,
            body_w,
            None,
        );
        pane.register_block_wrap_row_spans(
            source_line,
            ranges
                .iter()
                .map(|&(start, end)| MarkdownWrapRow {
                    start,
                    len: end.saturating_sub(start),
                })
                .collect(),
        );
        pane.register_block_wrap_hit_stops(
            source_line,
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
            let row_y = y + row_ix as f32 * line_h;
            draw_if_visible(
                sugarloaf,
                body_x,
                row_y,
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
            source_line,
            line,
            body_x,
            y,
            &opts,
        );
        y += line_h * row_count as f32;
        output_rows += row_count;
    }

    if output_rows == 0 && !meta.elapsed.as_deref().unwrap_or_default().is_empty() {
        draw_if_visible(
            sugarloaf,
            body_x,
            prompt_y,
            "done",
            &detail_opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
    }
}

#[cfg(test)]
mod virtualized_mermaid_tests {
    use super::*;

    #[test]
    fn code_label_detects_mermaid_fence_variants() {
        assert_eq!(virtual_markdown_code_label("```"), "");
        assert_eq!(virtual_markdown_code_label("```mermaid"), "mermaid");
        assert_eq!(virtual_markdown_code_label("~~~ .mermaid extra"), "mermaid");
        assert_eq!(
            virtual_markdown_code_label(
                "```python neoism_notebook_cell=1 neoism_state=idle neoism_count=4"
            ),
            "python"
        );
    }
}

/// Register the block rect, single wrap row, and measured stops for one
/// verbatim code/fence row so clicks and the caret map 1:1 onto its glyphs.
#[allow(clippy::too_many_arguments)]
fn register_code_row_geometry(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    source_line: usize,
    line: &str,
    rect: [f32; 4],
    text_x: f32,
    text_y: f32,
    wrap_width: f32,
    line_h: f32,
    opts: &DrawOpts,
) {
    pane.register_block_rect(
        source_line,
        rect,
        [rect[0] - 30.0, rect[1], 22.0, rect[3]],
        text_x,
        text_y,
        0,
        cursor_cell_width(opts).max(1.0),
        line_h,
        wrap_width,
        None,
    );
    pane.register_block_wrap_row_spans(
        source_line,
        vec![MarkdownWrapRow {
            start: 0,
            len: line.chars().count(),
        }],
    );
    pane.register_block_wrap_hit_stops(
        source_line,
        vec![MarkdownWrapHitRow {
            start: 0,
            stops: measured_stops_for_text(sugarloaf, line, opts),
        }],
    );
}

/// Terminal-style wrap for one code line: ranges of char indices per visual
/// row, breaking after the last whitespace that fits (else mid-word). Derived
/// from the line's cumulative measured stops, so no extra text shaping.
fn code_wrap_ranges(stops: &[f32], chars: &[char], max_w: f32) -> Vec<(usize, usize)> {
    let n = chars.len();
    if n == 0 {
        return vec![(0, 0)];
    }
    let max_w = max_w.max(24.0);
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < n {
        let base = stops[start];
        // Largest fitting prefix; always take at least one char.
        let fitting = stops[start + 1..=n].partition_point(|stop| stop - base <= max_w);
        let mut end = (start + fitting.max(1)).min(n);
        if end < n {
            if let Some(break_ix) = (start..end)
                .rev()
                .find(|&ix| chars[ix].is_whitespace() && ix > start)
            {
                end = break_ix + 1;
            }
        }
        out.push((start, end));
        start = end;
    }
    out
}

fn virtual_code_lang_for_line(
    lines: &[String],
    local_ix: usize,
    in_code_block: bool,
) -> Lang {
    if !in_code_block {
        return Lang::Other;
    }
    lines[..local_ix]
        .iter()
        .rev()
        .find(|line| line.trim_start().starts_with("```"))
        .map(|line| virtual_markdown_code_lang(virtual_markdown_code_label(line)))
        .unwrap_or(Lang::Other)
}

fn virtual_markdown_code_lang(lang: &str) -> Lang {
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

#[allow(clippy::too_many_arguments)]
fn draw_virtualized_code_line(
    sugarloaf: &mut Sugarloaf,
    mut x: f32,
    y: f32,
    line: &str,
    lang: Lang,
    opts: &DrawOpts,
    theme: &IdeTheme,
    occlusions: &[[f32; 4]],
) {
    for (tok, slice) in highlight_line(line, lang) {
        let mut tok_opts = *opts;
        tok_opts.color = syn_color(tok, theme, false);
        draw_if_visible(
            sugarloaf,
            x,
            y,
            slice,
            &tok_opts,
            opts.clip_rect.map(|r| r[1]).unwrap_or(f32::MIN),
            opts.clip_rect.map(|r| r[1] + r[3]).unwrap_or(f32::MAX),
            occlusions,
        );
        x += sugarloaf.text_mut().measure(slice, &tok_opts);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_table_block(
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
) {
    let local_lines = virtual_item_lines(&item.text);
    if let Some(table) = parse_table(&local_lines, 0) {
        let _ = render_table_with_source_base(
            sugarloaf,
            pane,
            &table,
            &local_lines,
            item.first_line,
            item.first_line,
            pane.cursor_line,
            pane.cursor_col,
            x,
            item.screen_y,
            width,
            clip,
            clip_top,
            clip_bottom,
            theme,
            mouse,
            text_occlusions,
            font_scale,
        );
    } else {
        draw_literal_block(
            sugarloaf,
            pane,
            item,
            x,
            width,
            clip,
            clip_top,
            clip_bottom,
            theme,
            text_occlusions,
            font_scale,
            true,
        );
    }
}

fn draw_literal_block(
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
    table: bool,
) {
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        x - 12.0,
        item.screen_y + 2.0,
        width + 24.0,
        (item.bounds.height - 4.0).max(1.0),
        BLOCK_RADIUS,
        theme.f32_alpha(theme.surface, if table { 0.7 } else { 0.82 }),
        DEPTH,
        ORDER_BG + 1,
    );
    let opts = DrawOpts {
        font_size: markdown_font(15.0, font_scale),
        color: theme.u8(if table { theme.fg } else { theme.muted }),
        clip_rect: Some(clip),
        // Literal table-text fallback (parse failed) is still table
        // text; measure_item's Table fallback only takes line_height
        // (font_size-only), so heights stay paired.
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let line_h = line_height(&opts);
    let local_top = (clip_top - item.screen_y - 10.0).max(0.0);
    let first_visible = (local_top / line_h).floor().max(0.0) as usize;
    let visible_lines = ((clip_bottom - clip_top) / line_h).ceil().max(1.0) as usize + 3;
    let mut y = item.screen_y + 10.0 + first_visible as f32 * line_h;
    let local_lines = virtual_item_lines(&item.text);
    for (local_ix, line) in local_lines
        .iter()
        .map(String::as_str)
        .enumerate()
        .skip(first_visible)
        .take(visible_lines)
    {
        draw_selection_for_line(
            sugarloaf,
            pane,
            item.first_line + local_ix,
            line,
            x,
            y,
            0,
            line_h,
            width - 8.0,
            &opts,
            theme,
            clip,
            clip_top,
            clip_bottom,
        );
        draw_search_matches_for_line(
            sugarloaf,
            pane,
            item.first_line + local_ix,
            line,
            x,
            y,
            0,
            line_h,
            width - 8.0,
            &opts,
            theme,
            clip,
            clip_top,
            clip_bottom,
        );
        let rendered = truncate_to_fit(line, width - 8.0, sugarloaf, &opts);
        draw_if_visible(
            sugarloaf,
            x,
            y,
            &rendered,
            &opts,
            clip_top,
            clip_bottom,
            text_occlusions,
        );
        y += line_h;
    }
    set_cursor_for_item(
        sugarloaf,
        pane,
        item,
        x,
        item.screen_y + 10.0,
        width,
        0,
        &opts,
    );
}
