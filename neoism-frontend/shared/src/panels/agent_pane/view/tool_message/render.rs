use super::diff::{
    cached_diff_card_view, cached_edit_diff_sections, diag_footer_height,
    diag_footer_rows, diff_body_height, diff_link_target, tool_diff_card_width,
};
use super::widgets::{draw_checkbox, draw_tool_connector, draw_tool_symbol};
use super::*;
use crate::primitives::truncate_to_fit;

fn tool_message_accent(status: &str, theme: &IdeTheme) -> u32 {
    match status {
        "error" => theme.red,
        "completed" => theme.green,
        _ => theme.yellow,
    }
}

/// Ordinary collapsed calls have a fixed height; edit diffs keep their cards.
pub const TOOL_HEADER_HEIGHT: f32 = 30.0;
const TOOL_GROUP_BODY_Y: f32 = 36.0;

fn tool_status_label(status: &str) -> &str {
    match status {
        "completed" => "",
        _ => status,
    }
}

fn fixed_diff_viewport_height(preview_rows: usize, s: f32) -> f32 {
    diff_body_height(preview_rows, s)
}

pub fn measure_tool_message_height(
    sugarloaf: &mut Sugarloaf,
    pane: &impl AgentToolPane,
    message: &impl AgentToolMessage,
    width: f32,
    s: f32,
    tool_expanded: bool,
    selected_group_child: Option<&str>,
) -> Option<f32> {
    if message.is_todos_output() {
        let opts = DrawOpts {
            font_size: 14.0 * s,
            ..DrawOpts::default()
        };
        let rows = message
            .todos()
            .iter()
            .take(12)
            .map(|todo| {
                wrap_todo_text(
                    sugarloaf,
                    todo.content(),
                    (width - 86.0 * s).max(40.0 * s),
                    &opts,
                )
                .len()
            })
            .sum::<usize>()
            .max(1);
        return Some(42.0 * s + rows as f32 * TODO_ROW_HEIGHT * s);
    }
    if show_tool_diff_cards(
        message.tool(),
        message.status(),
        tool_expanded,
        pane.tool_archived(message.id()),
    ) {
        if let Some(sections) = cached_edit_diff_sections(message) {
            let card_w = tool_diff_card_width(width, s);
            let mut height = TOOL_HEADER_HEIGHT * s;
            for (section_index, section) in sections.iter().enumerate() {
                let card_key = format!("{}:{section_index}", message.id());
                let card_expanded = pane.tool_expanded(&card_key);
                let view = cached_diff_card_view(section, card_w, s, card_expanded);
                height += diff_card::HEADER_HEIGHT * s
                    + fixed_diff_viewport_height(view.preview_visual_rows, s)
                    + diag_footer_height(section, s)
                    + 10.0 * s;
            }
            return Some(height);
        }
    }
    // Ordinary collapsed calls need no title shaping or output wrapping.
    if !tool_expanded {
        return Some(TOOL_HEADER_HEIGHT * s);
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

    let body = if !message.detail().trim().is_empty() {
        message.detail()
    } else {
        message.text()
    };
    let opts = DrawOpts {
        font_size: 13.0 * s,
        ..DrawOpts::default()
    };
    let max_lines = 12;
    let rows = tool_wrapped_rows(
        sugarloaf,
        body,
        tool_body_wrap_width(width, s),
        &opts,
        max_lines,
    );
    let has_hint = line_count_until(body, max_lines + 1) > max_lines;
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
    ((TOOL_GROUP_BODY_Y + 2.0) * s + rows.max(1) as f32 * 20.0 * s).max(58.0 * s)
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
    let accent = tool_message_accent(message.status(), theme);
    draw_status_dot_text(
        sugarloaf,
        x + 3.5 * s,
        y + 7.0 * s,
        7.0 * s,
        theme.u8(accent),
        (message.status() == "completed").then_some((theme.u8(accent), 0.35)),
        message_clip,
        occlusion_rects,
        s,
    );
    let Some(title_opts) = opts_with_clip(
        DrawOpts {
            font_size: 15.5 * s,
            color: theme.u8(theme.fg),
            ..DrawOpts::default()
        },
        message_clip,
    ) else {
        return h;
    };
    let mut symbol_opts = title_opts;
    symbol_opts.bold = false;
    symbol_opts.color = theme.u8(theme.muted);
    draw_tool_symbol(
        sugarloaf,
        [x + 20.0 * s, y, 18.0 * s, 22.0 * s],
        message.tool(),
        &symbol_opts,
        occlusion_rects,
    );
    // Reserve the lifecycle label before truncating so pending/running cannot
    // disappear behind a long path or command.
    let status = tool_status_label(message.status());
    let mut status_opts = title_opts;
    status_opts.font_size = 12.0 * s;
    status_opts.bold = false;
    status_opts.color = theme.u8(accent);
    let status_w = if status.is_empty() {
        0.0
    } else {
        sugarloaf.text_mut().measure(status, &status_opts) + 12.0 * s
    };
    let title_avail_w = (w - 66.0 * s - status_w).max(0.0);
    let title =
        truncate_to_fit(&message.title_text(), title_avail_w, sugarloaf, &title_opts);
    let text_x = x + 42.0 * s;
    let line_y = y + 2.0 * s;
    if !suppress_interactions {
        let title_w = sugarloaf.text_mut().measure(&title, &title_opts).max(12.0);
        let stops = measured_caret_stops(sugarloaf, &title, &title_opts, text_x);
        let title_sel = pane.register_selectable_line_with_caret_stops(
            &title,
            [
                text_x,
                line_y - 3.0 * s,
                title_w,
                title_opts.font_size + 8.0 * s,
            ],
            &stops,
        );
        if let Some((sel_left, sel_right)) = pane.selectable_line_highlight(title_sel) {
            draw_rounded_rect_clipped(
                sugarloaf,
                [
                    sel_left - 2.0,
                    line_y - 3.0 * s,
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
    draw_text_clipped(
        sugarloaf,
        text_x,
        line_y,
        &title,
        &title_opts,
        occlusion_rects,
    );
    if !status.is_empty() {
        draw_text_clipped(
            sugarloaf,
            x + w - 24.0 * s - status_w + 12.0 * s,
            y + 4.0 * s,
            status,
            &status_opts,
            occlusion_rects,
        );
    }

    if message.is_todos_output() {
        render_tool_todos(
            sugarloaf,
            pane,
            x + 30.0 * s,
            y + 28.0 * s,
            w - 40.0 * s,
            message.todos(),
            theme,
            s,
            message_clip,
            occlusion_rects,
            suppress_interactions,
        );
        return h;
    }

    let render_expanded = pane.tool_expanded(message.id())
        || pane.tool_expand_progress(message.id()) > 0.01;
    let archived = pane.tool_archived(message.id());
    let cached_diff_sections;
    let diff_sections = if !show_tool_diff_cards(
        message.tool(),
        message.status(),
        render_expanded,
        archived,
    ) {
        None
    } else if let Some(sections) = prepared_diff_sections {
        Some(sections)
    } else {
        cached_diff_sections = cached_edit_diff_sections(message);
        cached_diff_sections
            .as_ref()
            .map(|sections| sections.as_slice())
    };
    // Live edit cards use their per-file toggles rather than a parent toggle.
    if (diff_sections.is_none() || archived) && !suppress_interactions {
        if let Some(header_clip) =
            intersect_rect([x, y, w, TOOL_HEADER_HEIGHT * s], message_clip)
        {
            pane.register_tool_hit_rect(message.id().to_string(), header_clip);
        }
    }
    if !render_expanded && diff_sections.is_none() {
        return h;
    }
    if let Some(sections) = diff_sections {
        render_tool_diff_cards(
            sugarloaf,
            pane,
            message,
            x,
            y + TOOL_HEADER_HEIGHT * s,
            w,
            sections,
            theme,
            s,
            message_clip,
            suppress_interactions,
        );
        return h;
    }

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
    draw_tool_connector(
        sugarloaf,
        x + 28.0 * s,
        y + if message.tool() == "tool_group" {
            TOOL_GROUP_BODY_Y
        } else {
            26.0
        } * s,
        &connector_opts,
        occlusion_rects,
    );

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

    let body = if !message.detail().trim().is_empty() {
        message.detail()
    } else {
        message.text()
    };
    let Some(body_opts) = opts_with_clip(
        DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.fg),
            ..DrawOpts::default()
        },
        message_clip,
    ) else {
        return h;
    };
    let mut line_y = y + 26.0 * s;
    let body_x = x + 58.0 * s;
    let nested_body_x = x + 76.0 * s;
    let max_lines = 12;
    let wrap_width = tool_body_wrap_width(w, s);
    let wrapped_rows =
        tool_wrapped_rows(sugarloaf, body, wrap_width, &body_opts, max_lines);
    let total_lines = line_count_until(body, max_lines + 1).max(1);
    let mut nested_body_opts = body_opts;
    nested_body_opts.color = theme.u8(theme.muted);
    let row_bottom_limit = y + h - 3.0 * s;
    for row in wrapped_rows.iter() {
        if line_y + body_opts.font_size > row_bottom_limit {
            break;
        }
        let nested = row.nested;
        let text_x = if nested { nested_body_x } else { body_x };
        let text_opts = if nested {
            &nested_body_opts
        } else {
            &body_opts
        };
        let rendered = row.text.as_str();
        if !suppress_interactions {
            let line_w = sugarloaf.text_mut().measure(rendered, text_opts).max(12.0);
            let stops = measured_caret_stops(sugarloaf, rendered, text_opts, text_x);
            let sel_index = pane.register_selectable_line_with_caret_stops(
                rendered,
                [
                    text_x,
                    line_y - 3.0 * s,
                    line_w,
                    text_opts.font_size + 8.0 * s,
                ],
                &stops,
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
        let hint = format!("... +{extra} lines");
        draw_text_clipped(
            sugarloaf,
            body_x,
            line_y,
            &hint,
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
    let mut line_y = y + TOOL_GROUP_BODY_Y * s;
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
        let label = line.split_once('\t').map_or(line, |(_, label)| label);
        let child_rect = [
            body_x - 8.0 * s,
            line_y - 4.0 * s,
            (w - 70.0 * s).max(40.0 * s),
            row_h,
        ];
        if let Some(child_key) = child_key.as_ref().filter(|_| !suppress_interactions) {
            if let Some(rect) = intersect_rect(child_rect, message_clip) {
                pane.register_tool_hit_rect(
                    format!("{}::child::{}", message.id(), child_key),
                    rect,
                );
            }
        }
        let selected = child_key
            .as_deref()
            .zip(selected_child.as_deref())
            .is_some_and(|(child, selected)| child == selected);
        if selected {
            draw_rounded_rect_clipped(
                sugarloaf,
                child_rect,
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
            &truncate_chars(label, ((w / (8.0 * s)).floor().max(18.0)) as usize),
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
    line.split_once('\t').map(|(id, _)| id.to_string())
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
        let card_expanded = pane.tool_expanded(&card_key);
        let view = cached_diff_card_view(section, card_w, s, card_expanded);
        let full_body_h = diff_body_height(view.visual_rows, s);
        let body_h = fixed_diff_viewport_height(view.preview_visual_rows, s);
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
            if let Some(rect) = intersect_rect(
                [card_x, card_y, card_w, layout.total_height],
                viewport_clip,
            ) {
                pane.register_tool_hit_rect(card_key.clone(), rect);
            }
        }
        if let Some(target) = link_target.filter(|_| !suppress_interactions) {
            if let Some(rect) = layout
                .header_link_rect
                .and_then(|rect| intersect_rect(rect, viewport_clip))
            {
                pane.register_link_hit_rect(target.to_string(), rect);
            }
        }
        if card_expanded && full_body_h > body_h + 1.0 {
            if !suppress_interactions {
                if let Some(rect) = intersect_rect(
                    [
                        card_x,
                        card_y + diff_card::HEADER_HEIGHT * s,
                        card_w,
                        body_h,
                    ],
                    viewport_clip,
                ) {
                    pane.register_diff_scroll_rect(
                        scroll_key,
                        rect,
                        full_body_h - body_h,
                    );
                }
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

#[cfg(test)]
mod tests {
    use super::{
        diff_body_height, fixed_diff_viewport_height, group_child_key,
        tool_group_child_previews, tool_message_accent, tool_status_label,
        ToolMessageParts, TOOL_GROUP_BODY_Y, TOOL_HEADER_HEIGHT,
    };
    use crate::primitives::ide_theme::IdeTheme;

    #[test]
    fn read_group_children_use_ids_not_duplicate_labels() {
        let message = ToolMessageParts {
            id: "read-a..",
            title: "Reading/searching 2 items",
            text: "read-a\tRead(src/same.rs) [done]\nread-b\tRead(src/same.rs) [done]",
            status: "completed",
            tool: "tool_group",
            detail: "read-a\tfirst output\nread-b\tsecond output",
        };
        let previews = tool_group_child_previews(&message);
        let keys = message
            .text
            .lines()
            .map(group_child_key)
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [Some("read-a".to_string()), Some("read-b".to_string())]
        );
        assert_eq!(previews["read-a"], "first output");
        assert_eq!(previews["read-b"], "second output");
        assert_eq!(
            group_child_key("read-a\tRead(src/same.rs) [running]"),
            keys[0]
        );
        assert!(group_child_key("+3 more").is_none());
    }

    #[test]
    fn read_group_children_start_below_the_header_click_target() {
        for scale in [1.0, 2.0] {
            let header_bottom = TOOL_HEADER_HEIGHT * scale;
            let first_child_top = (TOOL_GROUP_BODY_Y - 4.0) * scale;
            assert!(first_child_top > header_bottom);
        }
    }

    #[test]
    fn compact_rows_keep_lifecycle_labels_and_status_dot_colors() {
        let theme = IdeTheme::default();
        for status in ["pending", "running", "streaming"] {
            assert_eq!(tool_status_label(status), status);
            assert_eq!(tool_message_accent(status, &theme), theme.yellow);
        }
        assert_eq!(tool_status_label("completed"), "");
        assert_eq!(tool_message_accent("completed", &theme), theme.green);
        assert_eq!(tool_status_label("error"), "error");
        assert_eq!(tool_message_accent("error", &theme), theme.red);
    }

    #[test]
    fn expanded_diff_keeps_the_collapsed_viewport_height() {
        let preview_rows = 6;
        let collapsed_height = fixed_diff_viewport_height(preview_rows, 1.0);

        for full_rows in [6, 20, 100, 1_000] {
            let expanded_height = fixed_diff_viewport_height(preview_rows, 1.0);
            let overflow = (diff_body_height(full_rows, 1.0) - expanded_height).max(0.0);

            assert_eq!(expanded_height, collapsed_height);
            assert_eq!(
                overflow,
                (diff_body_height(full_rows, 1.0) - collapsed_height).max(0.0)
            );
        }
    }
}

/// Render the LSP diagnostics footer beneath a diff card: errors in red,
/// warnings/info muted. Returns the height consumed for the next element.
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
        opts.color = theme.u8(theme.readable_accent(if diag.is_error {
            theme.red
        } else {
            theme.yellow
        }));
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
    let bar_x = body_x + body_w - scrollbar::width() - 3.0 * s;
    // Track clipped to the same viewport band as the thumb.
    scrollbar::draw_track(
        sugarloaf,
        bar_x,
        track_top.max(clip_top),
        (track_top + track_h).min(clip_bottom) - track_top.max(clip_top),
        1.0,
        0.0,
        ORDER_TEXT + 1,
    );
    scrollbar::draw_thumb(
        sugarloaf,
        bar_x,
        thumb_y.max(clip_top),
        (thumb_y + thumb_h).min(clip_bottom) - thumb_y.max(clip_top),
        1.0,
        false,
        0.0,
        ORDER_TEXT + 1,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn render_tool_todos<Todo: AgentToolTodo, P: AgentToolPane>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    x: f32,
    y: f32,
    w: f32,
    todos: &[Todo],
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
    suppress_interactions: bool,
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
    let todo_rows = todos
        .iter()
        .take(12)
        .map(|todo| {
            wrap_todo_text(
                sugarloaf,
                todo.content(),
                (w - 46.0 * s).max(40.0 * s),
                &opts,
            )
            .len()
        })
        .sum::<usize>()
        .max(1);
    draw_rect_clipped(
        sugarloaf,
        [
            x,
            y - 4.0 * s,
            1.0 * s,
            todo_rows as f32 * TODO_ROW_HEIGHT * s,
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
        for line in wrap_todo_text(
            sugarloaf,
            todo.content(),
            (w - 46.0 * s).max(40.0 * s),
            &text_opts,
        ) {
            let text_x = x + 46.0 * s;
            if !suppress_interactions {
                let line_w = sugarloaf.text_mut().measure(&line, &text_opts).max(12.0);
                let stops = measured_caret_stops(sugarloaf, &line, &text_opts, text_x);
                let selection_index = pane.register_selectable_line_with_caret_stops(
                    &line,
                    [
                        text_x,
                        line_y - 3.0 * s,
                        line_w,
                        text_opts.font_size + 8.0 * s,
                    ],
                    &stops,
                );
                if let Some((left, right)) =
                    pane.selectable_line_highlight(selection_index)
                {
                    draw_rounded_rect_clipped(
                        sugarloaf,
                        [
                            left,
                            line_y - 3.0 * s,
                            right - left,
                            text_opts.font_size + 8.0 * s,
                        ],
                        theme.f32_alpha(theme.accent, 0.22),
                        4.0,
                        ORDER_PANEL + 2,
                        viewport_clip,
                    );
                }
            }
            draw_text_clipped(
                sugarloaf,
                text_x,
                line_y,
                &line,
                &text_opts,
                occlusion_rects,
            );
            line_y += TODO_ROW_HEIGHT * s;
        }
    }
}
