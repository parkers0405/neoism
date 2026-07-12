pub(super) fn render_virtual(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    rect: [f32; 4],
    theme: &IdeTheme,
    mouse: Option<[f32; 2]>,
    text_occlusions: &[[f32; 4]],
    font_scale: f32,
    animation_phase: f32,
) -> bool {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return true;
    }

    let bg = theme.f32(theme.bg);
    sugarloaf.rect(None, x, y, w, h, bg, DEPTH, ORDER_BG);

    // Obsidian-style inline title: the frontmatter `title:` when set (edit
    // it right in the metadata rows), otherwise the file name.
    let title_text: String = pane
        .frontmatter_title()
        .or_else(|| {
            pane.path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .filter(|stem| !stem.is_empty())
        })
        .unwrap_or_else(|| pane.title.clone());
    let title_opts = DrawOpts {
        font_size: markdown_font(28.0, font_scale),
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some([x, y, w, h]),
        // Inline note title is heading text — take the pack font.
        // Layout is unaffected: `title_h` comes from `line_height`,
        // which only reads `font_size`.
        font_id: md_font_id(sugarloaf),
        ..DrawOpts::default()
    };
    let title_h = line_height(&title_opts) + 16.0;

    let pad_x = 48.0;
    let pad_top = 38.0 + title_h;
    let content_w = (w - pad_x * 2.0).clamp(220.0, 920.0);
    let content_x = x + ((w - content_w) * 0.5).max(pad_x.min(w * 0.08));
    let viewport_h = (h - pad_top).max(1.0);
    let clip = [x, y, w, h];
    let bottom = y + h;

    pane.begin_block_layout();
    pane.set_cursor_rect(None);

    if let Some(err) = pane.error.clone() {
        let opts = DrawOpts {
            font_size: markdown_font(16.0, font_scale),
            color: theme.u8(theme.red),
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        draw_wrapped(
            sugarloaf,
            content_x,
            y + pad_top - pane.scroll_y,
            &err,
            content_w,
            &opts,
            y,
            bottom,
            text_occlusions,
        );
        pane.set_content_height(160.0, h);
        return true;
    }

    // A zoom (Ctrl+/-) invalidates every measured block height — stale
    // heights make scaled-up text overlap. A source-revision bump is NOT
    // enough: identical source text takes prepare_surface's empty-batch
    // fast path and keeps the old measured layouts. Mark every node
    // Layout-dirty instead — resolve_dirty_layout resets them to
    // estimates (visual_line_count = 0), so commit_visible_measurements
    // re-measures the visible set at the new scale this same frame.
    let scale_bucket = font_scale_fine_bucket(font_scale);
    if pane.virtual_render.font_scale_bucket != scale_bucket {
        pane.virtual_render.font_scale_bucket = scale_bucket;
        let node_count = pane.virtual_render.surface.nodes().len();
        if node_count > 0 {
            let _ = pane
                .virtual_render
                .surface
                .apply(VirtualSurfaceCommand::MarkRangeDirty {
                    start: 0,
                    end: node_count,
                    kind: DirtyKind::Layout,
                });
        }
    }
    // A Mash Up Pack markdown font override change is a zoom-shaped
    // event: every measured wrap width was built with the old glyph
    // metrics. Mirror the scale-bucket sweep and drop the measurement
    // cache (its key doesn't carry the font). No override ever set →
    // `None == None`, zero work.
    let md_font = md_font_id(sugarloaf);
    if pane.virtual_render.md_font_id != md_font {
        pane.virtual_render.md_font_id = md_font;
        pane.virtual_render.measurement_cache.clear();
        let node_count = pane.virtual_render.surface.nodes().len();
        if node_count > 0 {
            let _ = pane
                .virtual_render
                .surface
                .apply(VirtualSurfaceCommand::MarkRangeDirty {
                    start: 0,
                    end: node_count,
                    kind: DirtyKind::Layout,
                });
        }
    }
    // The cursor's line reveals raw markup (Live Preview) and can wrap to a
    // different row count than the rendered view. When the cursor changes
    // lines, re-measure the node(s) it left and entered so the revealed line
    // never paints over the block below it (or leaves a stale gap).
    if pane.virtual_render.measured_cursor_line != Some(pane.cursor_line) {
        let previous = pane.virtual_render.measured_cursor_line;
        pane.virtual_render.measured_cursor_line = Some(pane.cursor_line);
        // Held arrow keys fire faster than the reveal can re-measure each line
        // it sweeps; flag the stream so measurement keeps the cursor line at
        // its rendered height (no per-line row bounce). An isolated press has a
        // long gap before it, so it stays revealed.
        let now = Instant::now();
        pane.virtual_render.cursor_reveal_suppressed = pane
            .virtual_render
            .last_cursor_change_at
            .is_some_and(|prev| now.saturating_duration_since(prev) < CURSOR_REVEAL_FAST_REPEAT);
        pane.virtual_render.last_cursor_change_at = Some(now);
        let mut dirty_nodes: Vec<usize> = previous
            .into_iter()
            .chain(std::iter::once(pane.cursor_line))
            .filter_map(|line| node_for_line(&pane.virtual_render, line))
            .map(|(node_ix, ..)| node_ix)
            .collect();
        dirty_nodes.sort_unstable();
        dirty_nodes.dedup();
        for node_ix in dirty_nodes {
            let _ = pane
                .virtual_render
                .surface
                .apply(VirtualSurfaceCommand::MarkRangeDirty {
                    start: node_ix,
                    end: node_ix + 1,
                    kind: DirtyKind::Layout,
                });
        }
    }
    if !prepare_surface(pane, content_w, y + pad_top, viewport_h) {
        return false;
    }
    // Click-to-jump (outline rows, roster dots): center the target line
    // with a GLIDE — compute the destination scroll via the reveal, then
    // restore the current position (pane AND surface, so this frame doesn't
    // flash the destination) and let the per-frame settle ease toward it.
    if let Some(line) = pane.pending_reveal_line.take() {
        let current = pane.scroll_y;
        reveal_virtual_line(pane, line, VirtualRevealAlign::Center);
        pane.target_scroll_y = pane.scroll_y;
        pane.scroll_y = current;
        let _ = pane
            .virtual_render
            .surface
            .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
                scroll_y: current.max(0.0),
                velocity_y: pane.scroll_velocity_px_s,
            }));
    }
    if pane.follow_cursor {
        reveal_virtual_cursor_source(pane);
    }

    let mut items = collect_visible_items(pane);
    if commit_visible_measurements(
        sugarloaf, pane, &items, content_w, clip, theme, font_scale,
    ) {
        // Draw from the post-measure positions in the same frame. Otherwise
        // an edited row can render at the estimate once, then snap on the
        // next frame when measured heights have shifted the rows below it.
        items = collect_visible_items(pane);
    }
    let content_height = pane.virtual_render.surface.content_height();

    let total_height = content_height + pad_top + 60.0;
    pane.set_content_height(total_height, h);
    if (pane.virtual_render.surface.scroll().scroll_y - pane.scroll_y).abs() > 0.5 {
        let _ = pane
            .virtual_render
            .surface
            .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
                scroll_y: pane.scroll_y.max(0.0),
                velocity_y: pane.scroll_velocity_px_s,
            }));
        items = collect_visible_items(pane);
    }

    // Trailing blank lines after the last block belong to no node, so no
    // draw_item ever places a caret there — remember where the lowest
    // visible node ends so the caret can be synthesized below it.
    let tail_anchor = items
        .iter()
        .map(|item| {
            (
                item.first_line + item.line_count,
                item.screen_y + item.bounds.height.max(1.0),
            )
        })
        .max_by_key(|(end_line, _)| *end_line);

    draw_if_visible(
        sugarloaf,
        content_x,
        y + 30.0 - pane.scroll_y,
        &title_text,
        &title_opts,
        y,
        bottom,
        text_occlusions,
    );

    for item in items {
        draw_item(
            sugarloaf,
            pane,
            &item,
            content_x,
            content_w,
            clip,
            y,
            bottom,
            theme,
            mouse,
            text_occlusions,
            font_scale,
            animation_phase,
        );
    }

    draw_markdown_outline(
        sugarloaf,
        pane,
        rect,
        content_x,
        content_w,
        pad_top,
        &title_text,
        theme,
        font_scale,
        mouse,
        text_occlusions,
    );
    draw_drag_drop_preview(sugarloaf, pane, content_x, content_w, theme, clip, font_scale);

    set_cursor_for_trailing_empty_lines(pane, tail_anchor, content_x, font_scale, clip);
    set_fallback_cursor_for_empty_virtual_markdown(
        pane,
        content_x,
        y + pad_top,
        content_w,
        font_scale,
        clip,
    );
    ensure_virtual_cursor_visible(pane, clip);
    draw_remote_markdown_carets(sugarloaf, pane, theme, clip, font_scale);
    draw_markdown_scrollbar(sugarloaf, pane, rect, total_height, theme, mouse, clip);
    draw_markdown_roster(sugarloaf, pane, rect, theme, mouse, clip, font_scale);
    true
}

/// Rebuild the "On this page" heading outline only when the source changed —
/// the per-frame cost is a revision compare.
fn ensure_markdown_outline(pane: &mut MarkdownPane) {
    if pane.virtual_render.outline_revision == pane.source_revision {
        return;
    }
    let mut outline = Vec::new();
    for (ix, line) in pane.lines.iter().enumerate() {
        if line.trim_start().starts_with("```") || pane.is_inside_code_block(ix) {
            continue;
        }
        if let Some((level, _, text)) = parse_heading_line(line) {
            let text = text.trim();
            if !text.is_empty() {
                outline.push(MarkdownOutlineEntry {
                    line: ix,
                    level,
                    text: text.to_string(),
                });
            }
        }
    }
    pane.virtual_render.outline_revision = pane.source_revision;
    pane.virtual_render.outline = outline;
}

/// Docs-site style page outline in the right gutter (the editor centers
/// content at ≤920px, so wide windows have dead space there). Headed by the
/// page title, headings indented by level, the section under the viewport
/// top marked with an accent tick, hovered rows slide in with a soft pill,
/// click reveals. Drawn only when the gutter is wide enough — narrow
/// windows lose nothing.
#[allow(clippy::too_many_arguments)]
fn draw_markdown_outline(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    rect: [f32; 4],
    content_x: f32,
    content_w: f32,
    pad_top: f32,
    title: &str,
    theme: &IdeTheme,
    font_scale: f32,
    mouse: Option<[f32; 2]>,
    occlusions: &[[f32; 4]],
) {
    ensure_markdown_outline(pane);
    if pane.virtual_render.outline.is_empty() {
        return;
    }
    let [x, y, w, h] = rect;
    let gutter_x = content_x + content_w + 30.0;
    let avail_w = (x + w) - gutter_x - 20.0;
    if avail_w < 140.0 {
        return;
    }
    let panel_w = avail_w.min(232.0);
    let text_x = gutter_x + 10.0;
    let title_opts = DrawOpts {
        font_size: markdown_font(13.0, font_scale),
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(rect),
        ..DrawOpts::default()
    };
    let row_opts_base = DrawOpts {
        font_size: markdown_font(12.5, font_scale),
        color: theme.u8_alpha(theme.muted, 0.92),
        clip_rect: Some(rect),
        ..DrawOpts::default()
    };
    let row_h = line_height(&row_opts_base) + 7.0;
    let top = y + pad_top.min(64.0);
    let clip_bottom = y + h;
    let title_label =
        truncate_to_fit(title, (panel_w - 10.0).max(24.0), sugarloaf, &title_opts);
    draw_if_visible(
        sugarloaf,
        text_x,
        top,
        &title_label,
        &title_opts,
        y,
        clip_bottom,
        occlusions,
    );

    // Active section: the last heading at or above the first visible line.
    let first_visible_line = pane
        .block_rects
        .iter()
        .filter(|block| block.rect[1] + block.rect[3] > top)
        .map(|block| block.line)
        .min()
        .unwrap_or(0);
    let outline = std::mem::take(&mut pane.virtual_render.outline);
    let active_ix = outline
        .iter()
        .rposition(|entry| entry.line <= first_visible_line)
        .unwrap_or(0);

    let list_top = top + line_height(&title_opts) + 12.0;
    let max_rows = (((clip_bottom - 16.0 - list_top) / row_h).floor().max(1.0)) as usize;
    pane.virtual_render.outline_panel_rect = Some([
        gutter_x - 8.0,
        top,
        panel_w + 16.0,
        (clip_bottom - 16.0 - top).max(0.0),
    ]);
    // Window the list when it overflows: follow the active section, or the
    // user's own wheel position after a manual scroll (clicking resumes
    // the auto-follow).
    let max_start = outline.len().saturating_sub(max_rows);
    let start = if outline.len() <= max_rows {
        pane.virtual_render.outline_scroll = 0.0;
        pane.virtual_render.outline_manual = false;
        0
    } else if pane.virtual_render.outline_manual {
        let clamped = pane
            .virtual_render
            .outline_scroll
            .clamp(0.0, max_start as f32);
        pane.virtual_render.outline_scroll = clamped;
        clamped.round() as usize
    } else {
        let start = active_ix.saturating_sub(max_rows / 2).min(max_start);
        pane.virtual_render.outline_scroll = start as f32;
        start
    };

    // Hover tracking: remember when the pointer arrived on a row so the
    // highlight can ease in instead of popping.
    let hovered_entry_ix = mouse.and_then(|[mx, my]| {
        let inside_x = mx >= gutter_x - 4.0 && mx <= gutter_x + panel_w + 6.0;
        let row = ((my - list_top) / row_h).floor();
        (inside_x && row >= 0.0 && my >= list_top).then(|| start + row as usize)
    });
    match (hovered_entry_ix, pane.virtual_render.outline_hover) {
        (Some(ix), Some((current, _))) if ix == current => {}
        (Some(ix), _) if ix < outline.len() => {
            pane.virtual_render.outline_hover = Some((ix, Instant::now()));
        }
        (None, Some(_)) => pane.virtual_render.outline_hover = None,
        _ => {}
    }
    let hover_progress = |since: Instant| -> f32 {
        let t = (Instant::now().saturating_duration_since(since).as_secs_f32() / 0.14)
            .clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    };

    for (row_ix, entry) in outline
        .iter()
        .enumerate()
        .skip(start)
        .take(max_rows)
        .map(|(ix, entry)| (ix - start, entry))
    {
        let entry_ix = row_ix + start;
        let row_y = list_top + row_ix as f32 * row_h;
        let indent = (entry.level.saturating_sub(1)) as f32 * 12.0;
        let active = entry_ix == active_ix;
        let hover = pane
            .virtual_render
            .outline_hover
            .filter(|(ix, _)| *ix == entry_ix)
            .map(|(_, since)| hover_progress(since))
            .unwrap_or(0.0);
        if hover > 0.0 {
            // Soft pill easing in behind the hovered row.
            draw_rounded_rect_clipped(
                sugarloaf,
                rect,
                gutter_x,
                row_y,
                panel_w,
                row_h - 2.0,
                6.0,
                theme.f32_alpha(theme.hover, 0.42 * hover),
                DEPTH,
                ORDER_BG + 1,
            );
        }
        // Click pulse: an accent flash that fades while the page glides.
        if let Some((_, since)) = pane
            .virtual_render
            .outline_click
            .filter(|(ix, _)| *ix == entry_ix)
        {
            let t = (Instant::now().saturating_duration_since(since).as_secs_f32()
                / 0.35)
                .clamp(0.0, 1.0);
            if t >= 1.0 {
                pane.virtual_render.outline_click = None;
            } else {
                let fade = 1.0 - t * t;
                draw_rounded_rect_clipped(
                    sugarloaf,
                    rect,
                    gutter_x,
                    row_y,
                    panel_w,
                    row_h - 2.0,
                    6.0,
                    theme.f32_alpha(theme.accent, 0.30 * fade),
                    DEPTH,
                    ORDER_BG + 2,
                );
            }
        }
        if active {
            draw_rounded_rect_clipped(
                sugarloaf,
                rect,
                gutter_x + 2.0,
                row_y + 3.0,
                2.5,
                row_h - 8.0,
                1.25,
                theme.f32_alpha(theme.accent, 0.9),
                DEPTH,
                ORDER_BG + 2,
            );
        }
        let mut row_opts = row_opts_base;
        if active || hover >= 1.0 {
            row_opts.color = theme.u8(theme.fg);
        } else if hover > 0.0 {
            row_opts.color = theme.u8_alpha(theme.fg, 0.65 + 0.35 * hover);
        }
        let slide = 3.0 * hover;
        let text = truncate_to_fit(
            &entry.text,
            (panel_w - indent - 14.0 - slide).max(24.0),
            sugarloaf,
            &row_opts,
        );
        draw_if_visible(
            sugarloaf,
            text_x + indent + slide,
            row_y + (row_h - row_opts.font_size) * 0.5 - 2.0,
            &text,
            &row_opts,
            y,
            clip_bottom,
            occlusions,
        );
        pane.outline_rects
            .push(([gutter_x - 4.0, row_y, panel_w + 10.0, row_h], entry.line));
    }
    pane.virtual_render.outline = outline;
}

/// While a block handle is being dragged: dim the lifted block, float a ghost
/// of it under the pointer, and draw an accent insertion line at the exact
/// spot it will land if released. After the drop, the moved block flashes
/// briefly so the eye can track where it went.
fn draw_drag_drop_preview(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    content_x: f32,
    content_w: f32,
    theme: &IdeTheme,
    clip: [f32; 4],
    font_scale: f32,
) {
    if let Some((range, progress)) = pane.drag_drop_flash_progress() {
        let alpha = (1.0 - progress) * 0.30;
        let mut flash_rects: std::collections::HashMap<usize, [f32; 4]> =
            std::collections::HashMap::new();
        for block in &pane.block_rects {
            if range.contains(&block.line) {
                flash_rects.insert(block.line, block.rect);
            }
        }
        for rect in flash_rects.values() {
            draw_rounded_rect_clipped(
                sugarloaf,
                clip,
                rect[0],
                rect[1],
                rect[2],
                rect[3],
                BLOCK_RADIUS,
                theme.f32_alpha(theme.accent, alpha),
                DEPTH,
                ORDER_BG + 3,
            );
        }
    }

    let Some(line_ix) = pane.dragging_line else {
        return;
    };
    if !pane.drag_moved {
        return;
    }
    let source_range = pane.drag_block_range(line_ix);

    // Dim the lifted block so it reads as picked up, not duplicated.
    let mut lifted_rects: std::collections::HashMap<usize, [f32; 4]> =
        std::collections::HashMap::new();
    for block in &pane.block_rects {
        if source_range.contains(&block.line) {
            lifted_rects.insert(block.line, block.rect);
        }
    }
    for rect in lifted_rects.values() {
        draw_rounded_rect_clipped(
            sugarloaf,
            clip,
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            BLOCK_RADIUS,
            theme.f32_alpha(theme.bg, 0.55),
            DEPTH,
            ORDER_TEXT + 2,
        );
    }

    // Insertion indicator: where the block lands on release. Skipped when the
    // pointer is still over the block's own range (a no-op drop).
    if let Some(target) = pane
        .drag_target_line(pane.drag_mouse_y)
        .filter(|target| !(source_range.start..=source_range.end).contains(target))
    {
        let next_top = pane
            .block_rects
            .iter()
            .filter(|block| block.line >= target)
            .map(|block| block.rect[1])
            .fold(f32::INFINITY, f32::min);
        let indicator_y = if next_top.is_finite() {
            next_top - 3.0
        } else {
            pane.block_rects
                .iter()
                .map(|block| block.rect[1] + block.rect[3])
                .fold(clip[1], f32::max)
                + 3.0
        };
        draw_rect_clipped(
            sugarloaf,
            clip,
            content_x - 18.0,
            indicator_y,
            content_w + 36.0,
            3.0,
            theme.f32(theme.accent),
            DEPTH,
            ORDER_TEXT + 3,
        );
        draw_rounded_rect_clipped(
            sugarloaf,
            clip,
            content_x - 26.0,
            indicator_y - 3.5,
            10.0,
            10.0,
            5.0,
            theme.f32(theme.accent),
            DEPTH,
            ORDER_TEXT + 3,
        );
    }

    // Floating ghost of the dragged block under the pointer.
    if let Some(source) = pane.lines.get(line_ix) {
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

fn reveal_virtual_cursor_source(pane: &mut MarkdownPane) {
    let state = &mut pane.virtual_render;
    if state.line_starts.is_empty() {
        return;
    }
    let line = pane
        .cursor_line
        .min(state.line_starts.len().saturating_sub(1));
    if cursor_line_is_in_visible_virtual_node(state, line) {
        state.pending_measure_anchor = None;
        return;
    }
    reveal_virtual_line(pane, line, VirtualRevealAlign::Nearest);
}

/// Scroll the virtual surface so `line` (0-based) is in view with the
/// requested alignment. Shared by follow-cursor reveal (`Nearest`) and
/// the Wave 7G roster click-to-jump (`Center`).
fn reveal_virtual_line(pane: &mut MarkdownPane, line: usize, align: VirtualRevealAlign) {
    let state = &mut pane.virtual_render;
    if state.line_starts.is_empty() {
        return;
    }
    let line = line.min(state.line_starts.len().saturating_sub(1));
    let Some(start) = state.line_starts.get(line).copied() else {
        return;
    };
    let end = state
        .line_starts
        .get(line + 1)
        .copied()
        .unwrap_or_else(|| {
            start
                + pane
                    .lines
                    .get(line)
                    .map(|line| line.len())
                    .unwrap_or_default()
        })
        .max(start + 1)
        .min(state.source.len().max(start + 1));
    let source = markdown_node_source(&state.source_id);
    let target = VirtualRevealTarget::new(
        source,
        NodeSourceRange::new(start as u64, end as u64),
        align,
    );
    if state
        .surface
        .apply(VirtualSurfaceCommand::RevealSource(target))
        .is_err()
    {
        return;
    }

    let surface_scroll_y = state.surface.scroll().scroll_y;
    apply_virtual_surface_scroll_to_pane(pane, surface_scroll_y);
}

fn markdown_node_source(source_id: &str) -> NodeSource {
    if source_id.contains('/') || source_id.contains('.') {
        NodeSource::File {
            path: source_id.to_string(),
        }
    } else {
        NodeSource::Synthetic {
            namespace: source_id.to_string(),
        }
    }
}

fn cursor_line_is_in_visible_virtual_node(
    state: &mut MarkdownVirtualRenderState,
    line: usize,
) -> bool {
    let visible = state.surface.visible_set();
    visible.nodes.into_iter().any(|visible_node| {
        let Some(node) = state.surface.nodes().get(visible_node.index) else {
            return false;
        };
        let Some(content) = node.content.as_ref() else {
            return false;
        };
        let start = content.line_start as usize;
        let end = start.saturating_add((content.line_count as usize).max(1));
        line >= start && line < end
    })
}

fn apply_virtual_surface_scroll_to_pane(pane: &mut MarkdownPane, surface_scroll_y: f32) {
    let surface_max_scroll = (pane.virtual_render.surface.content_height()
        - pane.virtual_render.surface.viewport().height)
    .max(0.0);
    let pane_max_scroll = (pane.content_height - pane.scroll_viewport_height).max(0.0);
    let scroll_y = if pane.scroll_y > surface_max_scroll + 0.5
        && (surface_scroll_y - surface_max_scroll).abs() <= 0.5
    {
        pane_max_scroll
    } else {
        surface_scroll_y
    };

    pane.scroll_y = scroll_y;
    pane.target_scroll_y = scroll_y;
    let _ = pane
        .virtual_render
        .surface
        .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
            scroll_y: scroll_y.max(0.0),
            velocity_y: pane.scroll_velocity_px_s,
        }));
}

fn prepare_surface(
    pane: &mut MarkdownPane,
    content_w: f32,
    viewport_y: f32,
    viewport_h: f32,
) -> bool {
    let source_id = pane.path.to_string_lossy().into_owned();
    let needs_source_rebuild = pane.virtual_render.source_id != source_id
        || pane.virtual_render.source_revision != pane.source_revision;

    if needs_source_rebuild {
        if large_virtual_markdown(pane) {
            if !prepare_large_line_surface(pane, &source_id, viewport_h) {
                return false;
            }
            let state = &mut pane.virtual_render;
            return state
                .surface
                .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
                    0.0, viewport_y, content_w, viewport_h, 1.0,
                )))
                .is_ok()
                && state
                    .surface
                    .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
                        scroll_y: pane.scroll_y.max(0.0),
                        velocity_y: pane.scroll_velocity_px_s,
                    }))
                    .is_ok();
        }
        if apply_tail_inline_append(pane, &source_id, content_w, viewport_y, viewport_h) {
            return true;
        }
        let source = source_from_lines(&pane.lines);
        let line_starts = line_starts(&source);
        let revision = pane.source_revision.max(1);
        let state = &mut pane.virtual_render;
        let same_source = state.source_id == source_id;
        let append_segment = same_source
            .then(|| {
                appended_line_segment(&state.source, &source, state.line_starts.len())
            })
            .flatten();
        if state.source_id != source_id {
            state.adapter =
                sugarloaf::VirtualMarkdownAdapter::new("neoism-markdown-pane");
            state.surface = sugarloaf::VirtualSurface::new(VirtualSurfaceConfig {
                overscan_px: 260.0,
                warm_distance_px: 12_000.0,
                cold_distance_px: 60_000.0,
                tile_height_px: 768.0,
                max_retained_chunks: 32_768,
                ..VirtualSurfaceConfig::default()
            });
        }
        let (mut batch, replaced) =
            if let Some((append_text, source_start_byte, source_start_line)) =
                append_segment.filter(|(text, _, _)| !text.is_empty())
            {
                (
                    state.adapter.build_append_batch_at(
                        &source_id,
                        append_text,
                        source_start_byte,
                        source_start_line,
                        VirtualSourceRevision(revision),
                    ),
                    false,
                )
            } else if same_source && state.source == source {
                (
                    sugarloaf::VirtualSurfaceBatch::for_route(
                        sugarloaf::VirtualSurfaceRoute::markdown_file(&source_id),
                        VirtualSourceRevision(revision),
                    ),
                    false,
                )
            } else {
                (
                    state.adapter.build_replace_batch(
                        &source_id,
                        &source,
                        VirtualSourceRevision(revision),
                    ),
                    true,
                )
            };
        let preserve_scroll_anchor = replaced && !pane.follow_cursor;
        let measure_anchor = preserve_scroll_anchor
            .then(|| {
                state
                    .surface
                    .capture_scroll_anchor((viewport_h * 0.35).max(0.0))
            })
            .flatten();
        if preserve_scroll_anchor {
            batch = batch.preserving_anchor((viewport_h * 0.35).max(0.0));
        }
        if batch.apply_to(&mut state.surface).is_err() {
            return false;
        }
        state.source_id = source_id;
        state.source_revision = revision;
        state.source = source;
        state.line_starts = line_starts;
        if preserve_scroll_anchor {
            let restored_scroll_y = state.surface.scroll().scroll_y;
            pane.scroll_y = restored_scroll_y;
            pane.target_scroll_y = restored_scroll_y;
            state.pending_measure_anchor = measure_anchor;
        }
    }

    let state = &mut pane.virtual_render;
    state
        .surface
        .apply(VirtualSurfaceCommand::SetViewport(VirtualViewport::new(
            0.0, viewport_y, content_w, viewport_h, 1.0,
        )))
        .is_ok()
        && state
            .surface
            .apply(VirtualSurfaceCommand::SetScroll(VirtualScroll {
                scroll_y: pane.scroll_y.max(0.0),
                velocity_y: pane.scroll_velocity_px_s,
            }))
            .is_ok()
}

fn prepare_large_line_surface(
    pane: &mut MarkdownPane,
    source_id: &str,
    viewport_h: f32,
) -> bool {
    let revision = pane.source_revision.max(1);
    let cursor_line = pane.cursor_line.min(pane.lines.len().saturating_sub(1));
    let pending_line_edit = pane.pending_line_edit;
    let mut append_tail_line_start = None;
    let mut truncate_line_starts_to = None;
    let mut line_starts_edit = None;
    let mut line_starts = Vec::new();
    let preserve_scroll_anchor;
    let mut measure_anchor = None;

    {
        let state = &mut pane.virtual_render;
        let new_source = state.source_id != source_id;
        if new_source {
            state.adapter =
                sugarloaf::VirtualMarkdownAdapter::new("neoism-markdown-pane");
            state.surface = sugarloaf::VirtualSurface::new(VirtualSurfaceConfig {
                overscan_px: 260.0,
                warm_distance_px: 12_000.0,
                cold_distance_px: 60_000.0,
                tile_height_px: 768.0,
                max_retained_chunks: 32_768,
                ..VirtualSurfaceConfig::default()
            });
            state.source.clear();
            state.line_starts.clear();
        }

        let previous_line_count = state.line_starts.len();
        let has_nodes = !state.surface.nodes().is_empty();
        let rebuild_all = new_source || !has_nodes || state.source_revision == 0;
        let same_line_count = previous_line_count == pane.lines.len();
        let single_line_insert = matches!(
            pending_line_edit,
            Some(MarkdownPendingLineEdit::Insert { .. })
        ) && pane.lines.len()
            == previous_line_count.saturating_add(1)
            && previous_line_count > 0;
        let single_line_delete = matches!(
            pending_line_edit,
            Some(MarkdownPendingLineEdit::Delete { .. })
        ) && previous_line_count
            == pane.lines.len().saturating_add(1)
            && !pane.lines.is_empty();
        let tail_line_insert = !rebuild_all
            && !single_line_insert
            && pane.lines.len() == previous_line_count.saturating_add(1)
            && cursor_line >= previous_line_count.saturating_sub(1)
            && previous_line_count > 0;
        let tail_line_delete = !rebuild_all
            && !single_line_delete
            && previous_line_count == pane.lines.len().saturating_add(1)
            && cursor_line >= pane.lines.len().saturating_sub(1)
            && !pane.lines.is_empty();
        if tail_line_insert {
            let previous_tail_line = previous_line_count.saturating_sub(1);
            append_tail_line_start =
                state.line_starts.get(previous_tail_line).map(|start| {
                    start
                        .saturating_add(
                            pane.lines
                                .get(previous_tail_line)
                                .map(String::len)
                                .unwrap_or_default(),
                        )
                        .saturating_add(1)
                });
        }
        if tail_line_delete {
            truncate_line_starts_to = Some(pane.lines.len());
        }
        let rebuild_line_starts = rebuild_all
            || (!same_line_count
                && !single_line_insert
                && !single_line_delete
                && !tail_line_insert
                && !tail_line_delete);
        if rebuild_line_starts {
            line_starts = line_starts_from_lines(&pane.lines);
        }

        let (mut batch, preserve_after_apply) = if rebuild_all {
            (
                state.adapter.build_replace_batch_from_lines(
                    source_id,
                    &pane.lines,
                    VirtualSourceRevision(revision),
                ),
                true,
            )
        } else if single_line_insert {
            let Some(MarkdownPendingLineEdit::Insert { line, byte_delta }) =
                pending_line_edit
            else {
                return false;
            };
            let affected_line = line.min(previous_line_count.saturating_sub(1));
            let Some((node_index, node_id, line_start, line_count, kind)) =
                node_for_line(state, affected_line)
            else {
                return false;
            };
            let new_line_count = line_count
                .saturating_add(1)
                .min(pane.lines.len().saturating_sub(line_start))
                .max(1);
            let source_start = state
                .line_starts
                .get(line_start)
                .copied()
                .unwrap_or(line_start) as u64;
            let line_end = line_start
                .saturating_add(new_line_count)
                .min(pane.lines.len());
            let source_end = source_start.saturating_add(joined_line_range_len(
                &pane.lines,
                line_start,
                line_end,
            ) as u64);
            let mut batch = state.adapter.build_existing_line_node_update_batch(
                source_id,
                &pane.lines,
                node_id,
                node_index as u64,
                line_start,
                new_line_count,
                source_start,
                source_end,
                kind,
                VirtualSourceRevision(revision),
            );
            batch.push(VirtualSurfaceCommand::RebaseSourceAfter {
                start: node_index.saturating_add(1),
                byte_delta,
                line_delta: 1,
            });
            line_starts_edit = Some(LargeLineStartsEdit::Insert { line, byte_delta });
            (batch, false)
        } else if single_line_delete {
            let Some(MarkdownPendingLineEdit::Delete { line, byte_delta }) =
                pending_line_edit
            else {
                return false;
            };
            let affected_line = line.min(previous_line_count.saturating_sub(1));
            let Some((node_index, node_id, line_start, line_count, kind)) =
                node_for_line(state, affected_line)
            else {
                return false;
            };
            let new_line_count = line_count.saturating_sub(1);
            let rebase_start = if new_line_count == 0 {
                node_index
            } else {
                node_index.saturating_add(1)
            };
            let mut batch = if new_line_count == 0 {
                let mut batch = sugarloaf::VirtualSurfaceBatch::for_route(
                    sugarloaf::VirtualSurfaceRoute::markdown_file(source_id),
                    VirtualSourceRevision(revision),
                );
                batch.push(VirtualSurfaceCommand::SpliceNodes {
                    start: node_index,
                    delete: 1,
                    insert: Vec::new(),
                });
                batch
            } else {
                let source_start = state
                    .line_starts
                    .get(line_start)
                    .copied()
                    .unwrap_or(line_start) as u64;
                let line_end = line_start
                    .saturating_add(new_line_count)
                    .min(pane.lines.len());
                let source_end = source_start.saturating_add(joined_line_range_len(
                    &pane.lines,
                    line_start,
                    line_end,
                ) as u64);
                state.adapter.build_existing_line_node_update_batch(
                    source_id,
                    &pane.lines,
                    node_id,
                    node_index as u64,
                    line_start,
                    new_line_count,
                    source_start,
                    source_end,
                    kind,
                    VirtualSourceRevision(revision),
                )
            };
            batch.push(VirtualSurfaceCommand::RebaseSourceAfter {
                start: rebase_start,
                byte_delta,
                line_delta: -1,
            });
            line_starts_edit = Some(LargeLineStartsEdit::Delete { line, byte_delta });
            (batch, false)
        } else if same_line_count {
            let Some((node_index, node_id, line_start, line_count, kind)) =
                node_for_line(state, cursor_line)
            else {
                return false;
            };
            let source_start = state
                .line_starts
                .get(line_start)
                .copied()
                .unwrap_or(line_start) as u64;
            let line_end = line_start.saturating_add(line_count).min(pane.lines.len());
            let source_end = source_start.saturating_add(joined_line_range_len(
                &pane.lines,
                line_start,
                line_end,
            ) as u64);
            (
                state.adapter.build_existing_line_node_update_batch(
                    source_id,
                    &pane.lines,
                    node_id,
                    node_index as u64,
                    line_start,
                    line_count,
                    source_start,
                    source_end,
                    kind,
                    VirtualSourceRevision(revision),
                ),
                false,
            )
        } else {
            let affected_line = if tail_line_insert {
                cursor_line.saturating_sub(1)
            } else {
                cursor_line
            }
            .min(previous_line_count.saturating_sub(1));
            let Some((node_index, _, line_start, _, _)) =
                node_for_line(state, affected_line)
            else {
                return false;
            };
            let delete_nodes = state.surface.nodes().len().saturating_sub(node_index);
            let source_start = state
                .line_starts
                .get(line_start)
                .copied()
                .unwrap_or(line_start) as u64;
            (
                state.adapter.build_line_tail_splice_batch(
                    source_id,
                    &pane.lines,
                    node_index,
                    delete_nodes,
                    line_start,
                    source_start,
                    VirtualSourceRevision(revision),
                ),
                false,
            )
        };

        preserve_scroll_anchor = preserve_after_apply && !pane.follow_cursor;
        if preserve_scroll_anchor {
            measure_anchor = state
                .surface
                .capture_scroll_anchor((viewport_h * 0.35).max(0.0));
            batch = batch.preserving_anchor((viewport_h * 0.35).max(0.0));
        }
        if batch.apply_to(&mut state.surface).is_err() {
            return false;
        }
        state.source_id = source_id.to_string();
        state.source_revision = revision;
        state.source.clear();
        if rebuild_line_starts {
            state.line_starts = line_starts;
        } else if let Some(edit) = line_starts_edit {
            apply_large_line_starts_edit(&mut state.line_starts, edit, &pane.lines);
        } else if let Some(start) = append_tail_line_start {
            if state.line_starts.len() == previous_line_count {
                state.line_starts.push(start);
            }
        } else if let Some(len) = truncate_line_starts_to {
            state.line_starts.truncate(len);
        }
    }

    pane.pending_line_edit = None;

    if preserve_scroll_anchor {
        let restored_scroll_y = pane.virtual_render.surface.scroll().scroll_y;
        pane.scroll_y = restored_scroll_y;
        pane.target_scroll_y = restored_scroll_y;
        pane.virtual_render.pending_measure_anchor = measure_anchor;
    }

    true
}

fn commit_visible_measurements(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    items: &[VirtualMarkdownDrawItem],
    width: f32,
    clip: [f32; 4],
    theme: &IdeTheme,
    font_scale: f32,
) -> bool {
    if items.is_empty() {
        return false;
    }
    let mut measurements = Vec::with_capacity(items.len());
    let mut layout_changed = false;
    // While a held-arrow stream is in flight, measure the cursor line as
    // rendered (don't reveal raw): its height then stays put as the caret
    // sweeps, so the blocks below it don't bounce a row per keystroke. The draw
    // still reveals the markup (only the layout height is frozen), and it
    // re-measures with the reveal the instant the caret settles.
    let reveal_active = pane.cursor_reveal_active();
    for item in items {
        if item.measured_layout {
            continue;
        }
        let cursor_inside = reveal_active
            && pane.cursor_line >= item.first_line
            && pane.cursor_line < item.first_line + item.line_count;
        let key = MarkdownVirtualMeasureKey {
            text_hash: item.text_hash,
            kind_tag: virtual_markdown_kind_tag(&item.kind),
            width_bucket: virtual_measure_bucket(width),
            font_scale_bucket: font_scale_fine_bucket(font_scale),
            cursor_token: if cursor_inside {
                (pane.cursor_line - item.first_line) as u32 + 1
            } else {
                0
            },
        };
        let measurement =
            if let Some(hit) = pane.virtual_render.measurement_cache.get(&key) {
                pane.virtual_render.measurement_cache_hits =
                    pane.virtual_render.measurement_cache_hits.saturating_add(1);
                *hit
            } else {
                pane.virtual_render.measurement_cache_misses = pane
                    .virtual_render
                    .measurement_cache_misses
                    .saturating_add(1);
                let (height, visual_line_count) = measure_item(
                    sugarloaf,
                    pane,
                    item,
                    width,
                    clip,
                    theme,
                    font_scale,
                    cursor_inside.then_some(pane.cursor_line),
                );
                let measurement = MarkdownVirtualMeasurement {
                    height,
                    visual_line_count,
                };
                pane.virtual_render
                    .measurement_cache
                    .insert(key, measurement);
                measurement
            };
        layout_changed |= (measurement.height - item.bounds.height).abs() > 0.25;
        measurements.push(VirtualMeasuredLayout::new(
            item.node,
            item.revision,
            measurement.height,
            0.0,
            measurement.visual_line_count,
        ));
    }

    let measure_anchor = pane.virtual_render.pending_measure_anchor.take();

    if !measurements.is_empty() {
        let _ = pane
            .virtual_render
            .surface
            .apply(VirtualSurfaceCommand::CommitMeasuredLayouts(measurements));
    }
    if let Some(anchor) = measure_anchor {
        if pane
            .virtual_render
            .surface
            .restore_scroll_anchor(anchor)
            .is_ok()
        {
            let restored_scroll_y = pane.virtual_render.surface.scroll().scroll_y;
            apply_virtual_surface_scroll_to_pane(pane, restored_scroll_y);
        }
    }
    layout_changed
}

/// Fine-grained scale bucket: glyph sizes change with every zoom step,
/// so the 0.25-granular measure bucket is too coarse — a 1.0 -> 1.1
/// zoom would reuse stale heights and overlap lines.
fn font_scale_fine_bucket(scale: f32) -> i32 {
    (scale.max(0.0) * 100.0).round() as i32
}

fn virtual_measure_bucket(value: f32) -> i32 {
    (value.max(0.0) * 4.0).round() as i32
}
