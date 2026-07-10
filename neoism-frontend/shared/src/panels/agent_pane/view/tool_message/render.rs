use super::diff::{
    cached_diff_card_view, cached_edit_diff_sections, diag_footer_height,
    diag_footer_rows, diff_body_height, diff_link_target, tool_diff_card_width,
};
use super::widgets::{draw_checkbox, draw_tool_connector, draw_tool_title};
use super::*;

fn tool_message_accent(status: &str, theme: &IdeTheme) -> u32 {
    match status {
        "error" => theme.red,
        "completed" => theme.green,
        _ => theme.yellow,
    }
}

pub fn measure_tool_message_height(
    sugarloaf: &mut Sugarloaf,
    message: &impl AgentToolMessage,
    width: f32,
    s: f32,
    tool_expanded: bool,
    selected_group_child: Option<&str>,
) -> Option<f32> {
    if message.is_todos_output() {
        return None;
    }
    if let Some(sections) = cached_edit_diff_sections(message) {
        let card_w = tool_diff_card_width(width, s);
        let mut height = 30.0 * s;
        for section in sections.iter() {
            let view = cached_diff_card_view(section, card_w, s, false);
            let body_h = diff_body_height(view.preview_visual_rows, s);
            height += diff_card::HEADER_HEIGHT * s
                + body_h
                + diag_footer_height(section, s)
                + 10.0 * s;
        }
        return Some(height.max(58.0 * s));
    }

    if message.tool() == "tool_group" {
        return Some(measure_tool_group_activity_height(
            sugarloaf,
            message,
            width,
            s,
            selected_group_child,
        ));
    }

    let expanded = tool_expanded && !message.detail().trim().is_empty();
    let body = if expanded {
        message.detail()
    } else {
        message.text()
    };
    let opts = DrawOpts {
        font_size: 13.0 * s,
        ..DrawOpts::default()
    };
    let max_lines = if expanded { 12 } else { 4 };
    let rows = tool_wrapped_rows(
        sugarloaf,
        body,
        tool_body_wrap_width(width, expanded, s),
        &opts,
        max_lines,
    );
    let extra = line_count_until(body, max_lines + 1)
        .max(1)
        .saturating_sub(max_lines);
    let has_hint = extra > 0 || (!message.detail().trim().is_empty() && !expanded);
    Some(
        (28.0 * s + (rows.len() + has_hint as usize).max(1) as f32 * 20.0 * s)
            .max(58.0 * s),
    )
}

fn measure_tool_group_activity_height(
    sugarloaf: &mut Sugarloaf,
    message: &impl AgentToolMessage,
    width: f32,
    s: f32,
    selected_group_child: Option<&str>,
) -> f32 {
    let opts = DrawOpts {
        font_size: 12.5 * s,
        ..DrawOpts::default()
    };
    let preview_w = (width - 96.0 * s).max(80.0 * s);
    let previews = tool_group_child_previews(message);
    let mut rows = 0usize;
    for line in message.text().lines().take(TOOL_GROUP_PREVIEW_LINES) {
        rows += 1;
        let Some(child_key) = group_child_key(line) else {
            continue;
        };
        if selected_group_child == Some(child_key.as_str()) {
            if let Some(preview) = previews.get(&child_key) {
                rows += wrap_text(sugarloaf, preview, preview_w, &opts, 4)
                    .len()
                    .max(1);
            }
        }
    }
    (28.0 * s + rows.max(1) as f32 * 20.0 * s).max(58.0 * s)
}

#[allow(clippy::too_many_arguments)]
pub fn render_tool_message(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentToolPane,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    message: &impl AgentToolMessage,
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
    prepared_diff_sections: Option<&[ToolDiffSection]>,
) -> f32 {
    if h <= 0.0 {
        return 0.0;
    }
    let Some(message_clip) = intersect_rect([x, y, w, h], viewport_clip) else {
        return h;
    };
    let suppress_interactions = pane.suppress_tool_interactions();
    let cached_diff_sections;
    let diff_sections = if let Some(sections) = prepared_diff_sections {
        Some(sections)
    } else {
        cached_diff_sections = cached_edit_diff_sections(message);
        cached_diff_sections
            .as_ref()
            .map(|sections| sections.as_slice())
    };
    if diff_sections.is_none() && !suppress_interactions {
        pane.register_tool_hit_rect(message.id().to_string(), [x, y, w, h]);
    }
    let accent = tool_message_accent(message.status(), theme);
    // Tighter status bullet — a 7px dot reads as a neat bullet next to
    // the tool title instead of a big circle. Vertically centered on the
    // title's optical middle (title is drawn at y+2 with a 15.5px font,
    // whose center sits ~y+10.75) so the dot lines up with the text
    // instead of sinking to the bottom of the row.
    draw_rounded_rect_clipped(
        sugarloaf,
        [x + 3.5 * s, y + 7.0 * s, 7.0 * s, 7.0 * s],
        theme.f32(accent),
        3.5 * s,
        ORDER_TEXT,
        message_clip,
    );
    let Some(title_opts) = opts_with_clip(
        DrawOpts {
            font_size: 15.5 * s,
            color: theme.u8(theme.fg),
            bold: true,
            ..DrawOpts::default()
        },
        message_clip,
    ) else {
        return h;
    };
    let title_text = message.title_text();
    if !suppress_interactions {
        let title_w = sugarloaf
            .text_mut()
            .measure(&title_text, &title_opts)
            .max(12.0);
        let title_sel = pane.register_selectable_line(
            &title_text,
            [
                x + 22.0 * s,
                y + 2.0 * s - 3.0 * s,
                title_w,
                title_opts.font_size + 8.0 * s,
            ],
        );
        if let Some((sel_left, sel_right)) = pane.selectable_line_highlight(title_sel) {
            draw_rounded_rect_clipped(
                sugarloaf,
                [
                    sel_left - 2.0,
                    y + 2.0 * s - 3.0 * s,
                    (sel_right - sel_left + 4.0).max(2.0),
                    title_opts.font_size + 8.0 * s,
                ],
                theme.f32_alpha(theme.accent, 0.22),
                4.0,
                ORDER_PANEL + 2,
                message_clip,
            );
        }
    }
    draw_tool_title(
        sugarloaf,
        x + 22.0 * s,
        y + 2.0 * s,
        &title_text,
        &title_opts,
        theme,
        occlusion_rects,
    );

    if message.is_todos_output() {
        render_tool_todos(
            sugarloaf,
            x + 30.0 * s,
            y + 28.0 * s,
            w - 40.0 * s,
            message.todos(),
            theme,
            s,
            message_clip,
            occlusion_rects,
        );
        return h;
    }

    let render_expanded = pane.tool_expanded(message.id())
        || pane.tool_expand_progress(message.id()) > 0.01;

    if message.tool() == "tool_group" {
        render_tool_group_activity(
            sugarloaf,
            pane,
            x,
            y,
            w,
            h,
            message,
            theme,
            s,
            message_clip,
            occlusion_rects,
            suppress_interactions,
        );
        return h;
    }

    if let Some(sections) = diff_sections {
        render_tool_diff_cards(
            sugarloaf,
            pane,
            message,
            x,
            y + 30.0 * s,
            w,
            sections,
            theme,
            s,
            message_clip,
            suppress_interactions,
        );
        return h;
    }

    let expanded = render_expanded && !message.detail().trim().is_empty();
    let body = if expanded {
        message.detail()
    } else {
        message.text()
    };
    let Some(body_opts) = opts_with_clip(
        DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(if expanded { theme.fg } else { theme.muted }),
            ..DrawOpts::default()
        },
        message_clip,
    ) else {
        return h;
    };
    let mut line_y = y + 26.0 * s;
    // Connector glyph "╰─" is ~24*s wide at font_size 14 — start
    // it at x+28*s, leave a small gap, then place text at x+58*s so the
    // glyph and the row label never overlap.
    let body_x = x + 58.0 * s;
    // Match the subagent branch connector for each tool row; no static
    // vertical tree bar needed.
    let nested_connector_x = x + 46.0 * s;
    let nested_body_x = x + 76.0 * s;
    let max_lines = if expanded { 12 } else { 4 };
    let wrap_width = tool_body_wrap_width(w, expanded, s);
    let wrapped_rows =
        tool_wrapped_rows(sugarloaf, body, wrap_width, &body_opts, max_lines);
    let total_lines = line_count_until(body, max_lines + 1).max(1);
    let extra_lines = total_lines.saturating_sub(max_lines);
    let has_trailing_hint =
        extra_lines > 0 || (!message.detail().trim().is_empty() && !expanded);
    let rendered_rows = wrapped_rows.len() + has_trailing_hint as usize;
    let draw_line_connectors = !expanded || rendered_rows <= 1;
    // Bright white connector — visually anchors the row to its parent
    // title (the "Read"/"Update" header above).
    let Some(connector_opts) = opts_with_clip(
        DrawOpts {
            font_size: 14.0 * s,
            color: theme.u8(theme.fg),
            bold: true,
            ..DrawOpts::default()
        },
        message_clip,
    ) else {
        return h;
    };
    let mut nested_body_opts = body_opts;
    nested_body_opts.color = theme.u8(theme.muted);
    let mut nested_connector_opts = connector_opts;
    nested_connector_opts.color = theme.u8(theme.muted);
    nested_connector_opts.bold = false;
    let row_bottom_limit = y + h - 3.0 * s;
    for (row_ix, row) in wrapped_rows.iter().enumerate() {
        if line_y + body_opts.font_size > row_bottom_limit {
            break;
        }
        // The closing "╰─" elbow belongs to the last real content row, not
        // the trailing "click to expand" / "+N lines" hint — the hint is a
        // plain affordance and must NOT spawn its own curved connector
        // below the tree (see the hint branches further down).
        let is_last = row_ix + 1 == wrapped_rows.len();
        let nested = expanded && row.nested;
        let connector_x = if nested {
            nested_connector_x
        } else {
            x + 28.0 * s
        };
        let text_x = if nested { nested_body_x } else { body_x };
        let text_opts = if nested {
            &nested_body_opts
        } else {
            &body_opts
        };
        let connector_opts = if nested {
            &nested_connector_opts
        } else {
            &connector_opts
        };
        if draw_line_connectors {
            draw_tool_connector(
                sugarloaf,
                connector_x,
                line_y,
                is_last,
                connector_opts,
                occlusion_rects,
            );
        }
        let rendered = row.text.as_str();
        if !suppress_interactions {
            let line_w = sugarloaf.text_mut().measure(rendered, text_opts).max(12.0);
            let sel_index = pane.register_selectable_line(
                rendered,
                [
                    text_x,
                    line_y - 3.0 * s,
                    line_w,
                    text_opts.font_size + 8.0 * s,
                ],
            );
            if let Some((sel_left, sel_right)) = pane.selectable_line_highlight(sel_index)
            {
                draw_rounded_rect_clipped(
                    sugarloaf,
                    [
                        sel_left - 2.0,
                        line_y - 3.0 * s,
                        (sel_right - sel_left + 4.0).max(2.0),
                        text_opts.font_size + 8.0 * s,
                    ],
                    theme.f32_alpha(theme.accent, 0.22),
                    4.0,
                    ORDER_PANEL + 2,
                    message_clip,
                );
            }
        }
        draw_text_clipped(
            sugarloaf,
            text_x,
            line_y,
            rendered,
            text_opts,
            occlusion_rects,
        );
        line_y += 20.0 * s;
    }
    let extra = total_lines.saturating_sub(max_lines);
    if extra > 0 && line_y + body_opts.font_size <= row_bottom_limit {
        let hint = if expanded {
            format!("... +{extra} lines")
        } else {
            format!("... +{extra} lines (click to expand)")
        };
        // No connector for the hint — the tree's closing elbow already
        // sits on the last content row above, so a second "╰─" here would
        // read as a stray duplicate curved line. Just the affordance text.
        draw_text_clipped(
            sugarloaf,
            body_x,
            line_y,
            &hint,
            &body_opts,
            occlusion_rects,
        );
    } else if !message.detail().trim().is_empty()
        && !expanded
        && line_y + body_opts.font_size <= row_bottom_limit
    {
        // No connector for "click to expand" — see the +N lines branch.
        draw_text_clipped(
            sugarloaf,
            body_x,
            line_y,
            "click to expand",
            &body_opts,
            occlusion_rects,
        );
    }
    h
}

#[allow(clippy::too_many_arguments)]
fn render_tool_group_activity(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentToolPane,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    message: &impl AgentToolMessage,
    theme: &IdeTheme,
    s: f32,
    message_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
    suppress_interactions: bool,
) {
    let Some(body_opts) = opts_with_clip(
        DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.muted),
            ..DrawOpts::default()
        },
        message_clip,
    ) else {
        return;
    };
    let Some(preview_opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.5 * s,
            color: theme.u8(theme.fg),
            ..DrawOpts::default()
        },
        message_clip,
    ) else {
        return;
    };
    let body_x = x + 58.0 * s;
    let row_h = 20.0 * s;
    let mut line_y = y + 26.0 * s;
    let row_bottom_limit = y + h - 3.0 * s;
    let previews = tool_group_child_previews(message);
    let selected_child = pane
        .selected_tool_group_child(message.id())
        .map(str::to_string);
    for line in message.text().lines().take(TOOL_GROUP_PREVIEW_LINES) {
        if line_y + body_opts.font_size > row_bottom_limit {
            break;
        }
        let child_key = group_child_key(line);
        if let Some(child_key) = child_key.as_ref().filter(|_| !suppress_interactions) {
            pane.register_tool_hit_rect(
                format!("{}::child::{}", message.id(), child_key),
                [
                    body_x - 8.0 * s,
                    line_y - 4.0 * s,
                    (w - 70.0 * s).max(40.0 * s),
                    row_h,
                ],
            );
        }
        let selected = child_key
            .as_deref()
            .zip(selected_child.as_deref())
            .is_some_and(|(child, selected)| child == selected);
        if selected {
            draw_rounded_rect_clipped(
                sugarloaf,
                [
                    body_x - 8.0 * s,
                    line_y - 4.0 * s,
                    (w - 70.0 * s).max(40.0 * s),
                    row_h,
                ],
                theme.f32_alpha(theme.accent, 0.16),
                7.0 * s,
                ORDER_PANEL + 1,
                message_clip,
            );
        }
        draw_text_clipped(
            sugarloaf,
            body_x,
            line_y,
            &truncate_chars(line, ((w / (8.0 * s)).floor().max(18.0)) as usize),
            &body_opts,
            occlusion_rects,
        );
        line_y += row_h;
        if selected {
            let preview = child_key
                .as_ref()
                .and_then(|child_key| previews.get(child_key))
                .map(String::as_str)
                .unwrap_or("No preview available");
            let preview_w = (w - 96.0 * s).max(80.0 * s);
            for preview_line in wrap_text(sugarloaf, preview, preview_w, &preview_opts, 4)
            {
                if line_y + preview_opts.font_size > row_bottom_limit {
                    break;
                }
                draw_text_clipped(
                    sugarloaf,
                    body_x + 18.0 * s,
                    line_y,
                    &preview_line,
                    &preview_opts,
                    occlusion_rects,
                );
                line_y += row_h;
            }
        }
    }
}

fn group_child_key(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with('+') {
        return None;
    }
    Some(trimmed.to_string())
}

fn tool_group_child_previews(message: &impl AgentToolMessage) -> HashMap<String, String> {
    message
        .detail()
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(key, preview)| (key.to_string(), preview.to_string()))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render_tool_diff_cards(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentToolPane,
    message: &impl AgentToolMessage,
    x: f32,
    y: f32,
    w: f32,
    sections: &[ToolDiffSection],
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    suppress_interactions: bool,
) {
    let card_x = x + 30.0 * s;
    let card_w = tool_diff_card_width(w, s);
    let clip_top = viewport_clip[1];
    let clip_bottom = viewport_clip[1] + viewport_clip[3];
    let mut card_y = y;
    for (section_index, section) in sections.iter().enumerate() {
        // Each file in a multi-file patch carries its own expand/scroll state so
        // a click toggles only the card under the cursor. Previously every card
        // shared the message id, so clicking one expanded (and made scrollable)
        // every card in the patch.
        let card_key = format!("{}:{section_index}", message.id());
        let card_expanded =
            pane.tool_expanded(&card_key) || pane.tool_expand_progress(&card_key) > 0.01;
        let view = cached_diff_card_view(section, card_w, s, card_expanded);
        let full_body_h = diff_body_height(view.visual_rows, s);
        let body_h = diff_body_height(view.preview_visual_rows, s);
        let scroll_key = card_key.clone();
        let body_scroll = if card_expanded {
            pane.diff_scroll_offset(&scroll_key, (full_body_h - body_h).max(0.0))
        } else {
            0.0
        };
        let link_target = diff_link_target(&section.link_target);
        let link_hovered = !suppress_interactions
            && link_target.is_some_and(|target| pane.link_hovered(target));
        let spec = CardSpec {
            path: &section.path,
            link_target,
            link_hovered,
            additions: section.additions,
            deletions: section.deletions,
            lang: Lang::from_path(&section.path),
            diff_lines: view.rows.as_slice(),
            visual_row_offsets: Some(view.visual_row_offsets.as_slice()),
            body_scroll,
        };
        let layout = diff_card::render(
            sugarloaf,
            card_x,
            card_y,
            card_w,
            body_h,
            &spec,
            s,
            theme,
            0.0,
            ORDER_PANEL,
            clip_top,
            clip_bottom,
        );
        if !suppress_interactions {
            pane.register_tool_hit_rect(
                card_key.clone(),
                [card_x, card_y, card_w, layout.total_height],
            );
        }
        if let Some(target) = link_target.filter(|_| !suppress_interactions) {
            pane.register_link_hit_rect(
                target.to_string(),
                [
                    card_x + diff_card::HEADER_PAD_X * s,
                    card_y,
                    (card_w - diff_card::HEADER_PAD_X * 2.0 * s).max(0.0),
                    diff_card::HEADER_HEIGHT * s,
                ],
            );
        }
        if card_expanded && full_body_h > body_h + 1.0 {
            if !suppress_interactions {
                pane.register_diff_scroll_rect(
                    scroll_key,
                    [
                        card_x,
                        card_y + diff_card::HEADER_HEIGHT * s,
                        card_w,
                        body_h,
                    ],
                    full_body_h - body_h,
                );
            }
            draw_diff_body_scrollbar(
                sugarloaf,
                card_x,
                card_y + diff_card::HEADER_HEIGHT * s,
                card_w,
                body_h,
                body_scroll,
                full_body_h,
                s,
                viewport_clip,
            );
        }
        card_y += layout.total_height;
        card_y += render_diff_card_diagnostics(
            sugarloaf,
            section,
            card_x,
            card_y,
            card_w,
            theme,
            s,
            viewport_clip,
        );
        card_y += 10.0 * s;
    }
}

/// Render the LSP diagnostics footer beneath a diff card (opencode's
/// diff-then-diagnostics layout): errors in red, warnings/info muted. Returns the
/// vertical space consumed so the caller can advance past it.
#[allow(clippy::too_many_arguments)]
fn render_diff_card_diagnostics(
    sugarloaf: &mut Sugarloaf,
    section: &ToolDiffSection,
    x: f32,
    y: f32,
    w: f32,
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
) -> f32 {
    let rows = diag_footer_rows(section);
    if rows == 0 {
        return 0.0;
    }
    let Some(base_opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.red),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return 0.0;
    };
    let mut line_y = y + 4.0 * s;
    let max_chars = ((w / (7.0 * s)).floor().max(20.0)) as usize;
    for diag in section.diagnostics.iter().take(MAX_DIAG_LINES_PER_CARD) {
        let mut opts = base_opts;
        opts.color = theme.u8(if diag.is_error {
            theme.red
        } else {
            theme.yellow
        });
        draw_text_clipped(
            sugarloaf,
            x + 4.0 * s,
            line_y,
            &truncate_chars(&diag.text, max_chars),
            &opts,
            &[],
        );
        line_y += DIAG_LINE_HEIGHT * s;
    }
    let total = section.diagnostics.len();
    if total > MAX_DIAG_LINES_PER_CARD {
        let mut opts = base_opts;
        opts.color = theme.u8(theme.muted);
        draw_text_clipped(
            sugarloaf,
            x + 4.0 * s,
            line_y,
            &format!("... +{} more", total - MAX_DIAG_LINES_PER_CARD),
            &opts,
            &[],
        );
    }
    diag_footer_height(section, s)
}

#[allow(clippy::too_many_arguments)]
fn draw_diff_body_scrollbar(
    sugarloaf: &mut Sugarloaf,
    body_x: f32,
    body_y: f32,
    body_w: f32,
    body_h: f32,
    body_scroll: f32,
    full_body_h: f32,
    s: f32,
    viewport_clip: [f32; 4],
) {
    let visible_rows = ((body_h / (diff_card::LINE_HEIGHT * s)).floor() as usize).max(1);
    let total_rows =
        ((full_body_h / (diff_card::LINE_HEIGHT * s)).ceil() as usize).max(visible_rows);
    let track_top = body_y + 4.0 * s;
    let track_h = (body_h - 8.0 * s).max(0.0);
    let progress = body_scroll / (full_body_h - body_h).max(1.0);
    let Some((thumb_y, thumb_h)) =
        scrollbar::compute_thumb(visible_rows, total_rows, track_top, track_h, progress)
    else {
        return;
    };
    let clip_top = viewport_clip[1];
    let clip_bottom = viewport_clip[1] + viewport_clip[3];
    if thumb_y + thumb_h < clip_top || thumb_y > clip_bottom {
        return;
    }
    scrollbar::draw_thumb(
        sugarloaf,
        body_x + body_w - scrollbar::SCROLLBAR_WIDTH - 3.0 * s,
        thumb_y.max(clip_top),
        (thumb_y + thumb_h).min(clip_bottom) - thumb_y.max(clip_top),
        1.0,
        false,
        0.0,
        ORDER_TEXT + 1,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn render_tool_todos<Todo: AgentToolTodo>(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    todos: &[Todo],
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let Some(opts) = opts_with_clip(
        DrawOpts {
            font_size: 14.0 * s,
            color: theme.u8(theme.fg),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return;
    };
    let mut muted = opts;
    muted.color = theme.u8(theme.muted);
    let mut line_y = y;
    draw_rect_clipped(
        sugarloaf,
        [
            x,
            y - 4.0 * s,
            1.0 * s,
            (todos.len().max(1) as f32 * TODO_ROW_HEIGHT * s).max(0.0),
        ],
        theme.f32(theme.border),
        ORDER_TEXT,
        viewport_clip,
    );
    if todos.is_empty() {
        draw_text_clipped(
            sugarloaf,
            x + 18.0 * s,
            line_y,
            "todos updated",
            &muted,
            occlusion_rects,
        );
        return;
    }
    for todo in todos.iter().take(12) {
        let state = TodoVisualState::from_status(todo.status());
        draw_checkbox(
            sugarloaf,
            x + 16.0 * s,
            line_y - 1.0 * s,
            state,
            theme,
            s,
            viewport_clip,
        );
        let mut text_opts = opts;
        text_opts.color = state.text_color(theme);
        text_opts.bold = state.text_bold();
        draw_text_clipped(
            sugarloaf,
            x + 46.0 * s,
            line_y,
            &truncate_chars(todo.content(), ((w / (8.0 * s)).floor().max(12.0)) as usize),
            &text_opts,
            occlusion_rects,
        );
        line_y += TODO_ROW_HEIGHT * s;
    }
}
