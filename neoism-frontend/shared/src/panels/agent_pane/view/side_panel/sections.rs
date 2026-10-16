use super::super::draw::draw_rect_clipped;
use super::draw::{intersect_rect, render_scramble_text};
use super::*;

pub(crate) const SECTION_GAP: f32 = 10.0;

/// Compact token count for the context bar label (e.g. `32.1k`, `1.2m`).
fn format_count_short(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}m", value as f64 / 1_000_000.0)
    } else if value >= 10_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod task_height_tests {
    use super::{
        section_header_font_size, side_panel_task_row_height, tasks_section_height,
        SECTION_GAP,
    };

    #[test]
    fn task_section_height_includes_every_task_without_nested_cap() {
        let scale = 1.0;
        let count = 19;
        let expected = section_header_font_size(scale) * 1.5
            + count as f32 * side_panel_task_row_height(scale)
            + SECTION_GAP
            + 6.0;
        assert_eq!(tasks_section_height(count, scale), expected);
        assert!(tasks_section_height(19, scale) > tasks_section_height(8, scale));
        let source = include_str!("sections.rs");
        assert!(!source.contains(&["todos.iter().", "take("].concat()));
        assert!(!source.contains("format!(\"+{extra} more\")"));
    }
}

pub(super) fn context_fill_fraction(total: u64, limit: Option<u64>) -> f32 {
    match limit.filter(|limit| *limit > 0) {
        Some(limit) => total as f32 / limit as f32,
        // Unknown capacity has no meaningful fraction; only show its token count.
        None => 0.0,
    }
    .clamp(0.0, 1.0)
}

pub(super) fn context_count_label(total: u64, limit: Option<u64>) -> String {
    let total = format_count_short(total);
    match limit.filter(|limit| *limit > 0) {
        Some(limit) => format!("{} / {} tokens", total, format_count_short(limit)),
        None => format!("{total} tokens"),
    }
}

/// H3-heading font size for the agent side-panel section titles. Mirrors the
/// Markdown renderer's `heading_level_font_size(3)` (22.5px, on the 17px
/// markdown body baseline) rescaled to the 14px global text baseline exactly
/// like `markdown_font`, so the section titles read as a `### ` H3 heading.
pub(crate) fn section_header_font_size(s: f32) -> f32 {
    const MD_H3_FONT_SIZE: f32 = 22.5;
    const MD_BODY_FONT_SIZE: f32 = 17.0;
    const MD_GLOBAL_BASELINE_FONT_SIZE: f32 = 14.0;
    MD_H3_FONT_SIZE * (MD_GLOBAL_BASELINE_FONT_SIZE / MD_BODY_FONT_SIZE) * s
}

pub(crate) fn render_section_header(
    sugarloaf: &mut Sugarloaf,
    label: &str,
    x: f32,
    y: f32,
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    // Keep the checkpoint's H3 layout size while compensating for P2's width.
    let header_size = section_header_font_size(s);
    let pixel_font = crate::primitives::pixel_font_id(sugarloaf);
    let drawn_size = if pixel_font.is_some() {
        header_size - 3.0 * s
    } else {
        header_size
    };
    let rest_opts = DrawOpts {
        font_size: drawn_size,
        color: theme.u8(theme.fg),
        bold: true,
        extrude: true,
        font_id: pixel_font,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };

    // Original illuminated drop cap; blackletter is the missing-P2 fallback.
    let mut chars = label.chars();
    if let Some(first) = chars.next() {
        let rest: String = chars.collect();
        let cap_font =
            pixel_font.or_else(|| crate::primitives::maguntia_font_id(sugarloaf));
        let cap_opts = DrawOpts {
            font_size: drawn_size * 1.4,
            color: theme.u8(theme.fg),
            bold: false,
            italic: false,
            extrude: true,
            font_id: cap_font,
            clip_rect: Some(clip),
        };
        let first_str = first.to_string();
        let rest_baseline = sugarloaf.text_mut().baseline_offset(&rest_opts);
        let cap_baseline = sugarloaf.text_mut().baseline_offset(&cap_opts);
        let cap_y = y + rest_baseline - cap_baseline;
        draw_text_with_occlusion(
            sugarloaf,
            x,
            cap_y,
            &first_str,
            &cap_opts,
            occlusion_rects,
        );
        if !rest.is_empty() {
            let cap_w = sugarloaf.text_mut().measure(&first_str, &cap_opts);
            draw_text_with_occlusion(
                sugarloaf,
                x + cap_w + 1.0 * s,
                y,
                &rest,
                &rest_opts,
                occlusion_rects,
            );
        }
    }
    y + header_size * 1.5
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_text_line(
    sugarloaf: &mut Sugarloaf,
    line: &str,
    x: f32,
    y: f32,
    width: f32,
    color: [u8; 4],
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let _ = theme;
    let opts = DrawOpts {
        font_size: FONT_SIZE * s * 0.95,
        color,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let truncated = truncate_sidebar_text(line, width, sugarloaf, &opts);
    draw_text_with_occlusion(sugarloaf, x, y, &truncated, &opts, occlusion_rects);
    y + FONT_SIZE * s * 1.5
}

/// Render the "Directory" section — the drop-cap H3 header plus the compacted
/// working-directory path below it. A plain, non-clickable readout. Shared by
/// chat mode (`render_session_info`) and home mode (`render_sessions_list`).
/// Returns the `y` below it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_directory_section(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentSidePanelPane,
    x: f32,
    y: f32,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let mut y = render_section_header(
        sugarloaf,
        "Directory",
        x,
        y,
        theme,
        s,
        clip,
        occlusion_rects,
    );
    let label = pane.directory_label();
    y = render_text_line(
        sugarloaf,
        &label,
        x,
        y,
        width,
        theme.u8(theme.fg),
        theme,
        s,
        clip,
        occlusion_rects,
    );
    y
}

fn render_kv_row(
    sugarloaf: &mut Sugarloaf,
    label: &str,
    value: &str,
    x: f32,
    y: f32,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let label_opts = DrawOpts {
        font_size: FONT_SIZE * s,
        color: theme.u8(theme.dim),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let value_opts = DrawOpts {
        font_size: FONT_SIZE * s,
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    draw_text_with_occlusion(sugarloaf, x, y, label, &label_opts, occlusion_rects);
    let label_w = sugarloaf.text_mut().measure(label, &label_opts);
    let value_x = x + label_w + 8.0 * s;
    let value_budget = (width - (value_x - x) - 4.0 * s).max(0.0);
    let value_str = truncate_sidebar_text(value, value_budget, sugarloaf, &value_opts);
    draw_text_with_occlusion(
        sugarloaf,
        value_x,
        y,
        &value_str,
        &value_opts,
        occlusion_rects,
    );
    y + FONT_SIZE * s * 1.55
}

pub(crate) fn render_session_info(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentSidePanelPane,
    content_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    occlusion_rects: &[[f32; 4]],
) {
    // This rail contains readouts only. Discard legacy branch hit/cursor
    // geometry even when the old roster is still retained for lifecycle IO.
    pane.side_panel_mut().clear_row_hit_rect();
    pane.side_panel_mut().clear_selected_cursor_rect();

    let [cx, cy_full, cw, ch_full] = content_rect;
    let pad_x = ROW_PADDING_X * s;
    let text_x = cx + pad_x;
    let text_w = (cw - pad_x * 2.0).max(0.0);
    let cy = cy_full;
    let ch = ch_full;
    let clip = [cx, cy, cw, ch];

    // The whole chat-mode content column scrolls as one viewport so
    // nothing (goal, branches, tasks) can fall off the bottom out of
    // reach. Every section is laid out at its natural `y` shifted up by
    // the pixel scroll offset; the `clip` keeps off-viewport draws inside
    // the panel frame. We track `content_top` to measure total laid-out
    // height at the end and feed it back as the scroll bound.
    let scroll = pane.side_panel().content_scroll_px();
    let content_top = cy + 14.0 * s;
    let mut y = content_top - scroll;

    // --- Directory ---
    y = render_directory_section(
        sugarloaf,
        pane,
        text_x,
        y,
        text_w,
        theme,
        s,
        clip,
        occlusion_rects,
    );
    y += SECTION_GAP * s;

    // --- Session ---
    y = render_section_header(
        sugarloaf,
        "Session",
        text_x,
        y,
        theme,
        s,
        clip,
        occlusion_rects,
    );
    if pane.conversation_source().provider().is_some() {
        y = render_kv_row(
            sugarloaf,
            "Source",
            pane.conversation_source().label(),
            text_x,
            y,
            text_w,
            theme,
            s,
            clip,
            occlusion_rects,
        );
    } else {
        y = render_kv_row(
            sugarloaf,
            "Agent",
            pane.agent_label(),
            text_x,
            y,
            text_w,
            theme,
            s,
            clip,
            occlusion_rects,
        );
        y = render_kv_row(
            sugarloaf,
            "Model",
            pane.model(),
            text_x,
            y,
            text_w,
            theme,
            s,
            clip,
            occlusion_rects,
        );
        y = render_kv_row(
            sugarloaf,
            "Reasoning",
            pane.thinking_label(),
            text_x,
            y,
            text_w,
            theme,
            s,
            clip,
            occlusion_rects,
        );
    }

    // --- Usage ---
    // Compact System 7-style context meter. Its square, two-step bevel and
    // quiet platinum face deliberately read as a control strip rather than a
    // modern pill; token/cost calculations remain owned by the usage policy.
    if let Some((context_total, limit)) = pane.context_usage() {
        y += SECTION_GAP * s;
        y = render_section_header(
            sugarloaf,
            "Usage",
            text_x,
            y,
            theme,
            s,
            clip,
            occlusion_rects,
        );
        let meter_y = y;
        pane.side_panel_mut()
            .update_usage_meter(context_total, limit);
        if limit.is_some_and(|limit| limit > 0) {
            let stroke = (1.0 * s).max(1.0);
            let bar_h = (11.0 * s).max(9.0);
            let bar_y = y + 2.0 * s;
            let track_w = text_w;

            // Black keyline, pale top/left highlight, then a recessed cool-gray
            // well. The unrounded corners and exact one-pixel steps are the
            // characteristic Platinum control-strip silhouette.
            draw_rect_clipped(
                sugarloaf,
                [text_x, bar_y, track_w, bar_h],
                [0.18, 0.19, 0.19, 1.0],
                ORDER_PANEL + 2,
                clip,
            );
            draw_rect_clipped(
                sugarloaf,
                [
                    text_x + stroke,
                    bar_y + stroke,
                    (track_w - stroke * 2.0).max(0.0),
                    (bar_h - stroke * 2.0).max(0.0),
                ],
                [0.86, 0.85, 0.81, 1.0],
                ORDER_PANEL + 3,
                clip,
            );
            let well_x = text_x + stroke * 2.0;
            let well_y = bar_y + stroke * 2.0;
            let well_w = (track_w - stroke * 3.0).max(0.0);
            let well_h = (bar_h - stroke * 3.0).max(0.0);
            draw_rect_clipped(
                sugarloaf,
                [well_x, well_y, well_w, well_h],
                [0.42, 0.43, 0.42, 1.0],
                ORDER_PANEL + 4,
                clip,
            );
            let fill_w = well_w * context_fill_fraction(context_total, limit);
            if fill_w > 0.0 {
                draw_rect_clipped(
                    sugarloaf,
                    [well_x, well_y, fill_w, well_h],
                    [0.69, 0.69, 0.65, 1.0],
                    ORDER_PANEL + 5,
                    clip,
                );
            }
            y = bar_y + bar_h + 6.0 * s;
        } else {
            y += 4.0 * s;
        }
        let label = context_count_label(context_total, limit);
        if let Some(elapsed_ms) = pane.side_panel().usage_scramble_elapsed_ms() {
            let opts = DrawOpts {
                font_size: FONT_SIZE * s * 0.95,
                color: theme.u8(theme.fg),
                clip_rect: Some(clip),
                ..DrawOpts::default()
            };
            render_scramble_text(sugarloaf, text_x, y, &label, &opts, elapsed_ms);
            y += FONT_SIZE * s * 1.5;
        } else {
            y = render_text_line(
                sugarloaf,
                &label,
                text_x,
                y,
                text_w,
                theme.u8(theme.fg),
                theme,
                s,
                clip,
                occlusion_rects,
            );
        }
        let usage_rect = [
            text_x,
            meter_y,
            text_w,
            (y - meter_y).max(FONT_SIZE * s * 1.5),
        ];
        if let Some(visible_rect) = intersect_rect(usage_rect, clip) {
            pane.side_panel_mut().set_usage_rect(visible_rect);
        }
    }

    // --- Goal ---
    // The session's persistent active/blocked goal sits ABOVE branches and
    // tasks, so Alt+H immediately shows what the agent is still pursuing.
    if let Some(goal) = pane.side_panel().session_goal().cloned() {
        y += SECTION_GAP * s;
        y = render_goal_section(
            sugarloaf,
            &goal,
            text_x,
            y,
            text_w,
            theme,
            s,
            clip,
            occlusion_rects,
        );
    }

    // Snapshot tasks before any `pane.side_panel_mut()` calls below so
    // the immutable messages borrow does not conflict with row layout.
    let tasks = latest_todos(pane.messages()).to_vec();
    let tasks_h = tasks_section_height(tasks.len(), s);

    // --- Tasks ---
    // Reflects the latest TodoWrite tool message verbatim. Updates are
    // entirely action-driven — every TodoWrite the model emits arrives
    // as a new message, so reading from `pane.messages()` each frame is
    // all that's needed for the check-off to mirror the chat.
    if !tasks.is_empty() {
        render_tasks_section(
            sugarloaf,
            &tasks,
            [cx, y, cw, tasks_h],
            theme,
            s,
            clip,
            occlusion_rects,
        );
        y += tasks_h;
    }

    // Total laid-out content height = where the cursor ended up (in
    // scrolled space) plus the scroll we'd already subtracted, minus the
    // content top. Feed the overflow back as the scroll bound so the
    // wheel handler can't run the column past its last section, and a
    // little bottom breathing room so the final row isn't flush.
    let content_height = (y + scroll) - content_top + 14.0 * s;
    let overflow = (content_height - ch).max(0.0);
    pane.side_panel_mut().set_content_scroll_max(overflow);
}

#[cfg(test)]
mod authoritative_todo_tests {
    use super::*;

    struct Row {
        todos: Vec<crate::panels::agent_pane::state::NeoismAgentTodo>,
    }
    impl AgentSidePanelMessage for Row {
        type Todo = crate::panels::agent_pane::state::NeoismAgentTodo;
        fn is_todos_output(&self) -> bool {
            true
        }
        fn todos(&self) -> &[Self::Todo] {
            &self.todos
        }
    }

    #[test]
    fn explicit_empty_snapshot_masks_older_todowrite() {
        let rows = [
            Row {
                todos: vec![crate::panels::agent_pane::state::NeoismAgentTodo {
                    status: "pending".into(),
                    content: "Old".into(),
                }],
            },
            Row { todos: vec![] },
        ];
        assert!(latest_todos(&rows).is_empty());
    }
}

/// Render the Goal section: a "Goal - Status" header, the goal text wrapped
/// to a few lines, and the agent's `summary` when present. Returns the `y`
/// below the section.
#[allow(clippy::too_many_arguments)]
fn render_goal_section(
    sugarloaf: &mut Sugarloaf,
    goal: &SessionGoal,
    x: f32,
    y: f32,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) -> f32 {
    let status_label = if goal.paused {
        "Paused"
    } else {
        goal.status.label()
    };
    let header = format!("Goal - {status_label}");
    let mut y =
        render_section_header(sugarloaf, &header, x, y, theme, s, clip, occlusion_rects);

    // Goal text — wrapped up to 3 lines so a long goal doesn't dominate.
    let text_opts = DrawOpts {
        font_size: FONT_SIZE * s * 0.95,
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let text_rows = wrap_text(sugarloaf, goal.text.trim(), width, &text_opts, 3);
    for row in text_rows.iter().take(3) {
        draw_text_with_occlusion(sugarloaf, x, y, row, &text_opts, occlusion_rects);
        y += FONT_SIZE * s * 1.4;
    }

    // Summary — what the agent accomplished or why it's blocked.
    let summary = goal.summary.trim();
    if !summary.is_empty() {
        y += 2.0 * s;
        let summary_opts = DrawOpts {
            font_size: FONT_SIZE * s * 0.88,
            color: theme.u8(theme.dim),
            italic: true,
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        let summary_rows = wrap_text(sugarloaf, summary, width, &summary_opts, 3);
        for row in summary_rows.iter().take(3) {
            draw_text_with_occlusion(
                sugarloaf,
                x,
                y,
                row,
                &summary_opts,
                occlusion_rects,
            );
            y += FONT_SIZE * s * 1.35;
        }
    }
    y
}

/// Find the most recent authoritative todo output, including an explicit empty
/// snapshot. An empty provider plan must mask older historical TodoWrite cards.
fn latest_todos<M: AgentSidePanelMessage>(messages: &[M]) -> &[M::Todo] {
    for message in messages.iter().rev() {
        if message.is_todos_output() {
            return message.todos();
        }
    }
    &[]
}

fn tasks_section_height(todos_len: usize, s: f32) -> f32 {
    if todos_len == 0 {
        return 0.0;
    }
    // Matches `render_section_header`'s H3 advance (`header_size * 1.5`).
    let header_h = section_header_font_size(s) * 1.5;
    let rows_h = todos_len as f32 * side_panel_task_row_height(s);
    // Standard section gap above the header, plus ~6px below the last row.
    header_h + rows_h + (SECTION_GAP + 6.0) * s
}

#[allow(clippy::too_many_arguments)]
fn render_tasks_section(
    sugarloaf: &mut Sugarloaf,
    todos: &[impl AgentSidePanelTodo],
    rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let [rx, ry, rw, _rh] = rect;
    let pad_x = ROW_PADDING_X * s;
    let text_x = rx + pad_x;
    let text_w = (rw - pad_x * 2.0).max(0.0);

    let mut y = ry + SECTION_GAP * s;
    y = render_section_header(
        sugarloaf,
        "Tasks",
        text_x,
        y,
        theme,
        s,
        clip,
        occlusion_rects,
    );

    let row_h = side_panel_task_row_height(s);
    for todo in todos {
        render_task_row(
            sugarloaf,
            todo,
            text_x,
            y,
            text_w,
            theme,
            s,
            clip,
            occlusion_rects,
        );
        y += row_h;
    }
}

#[allow(clippy::too_many_arguments)]
fn render_task_row(
    sugarloaf: &mut Sugarloaf,
    todo: &impl AgentSidePanelTodo,
    x: f32,
    y: f32,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let state = TodoVisualState::from_status(todo.status());
    draw_checkbox(sugarloaf, x + 16.0 * s, y + 1.0 * s, state, theme, s, clip);

    let text_opts = DrawOpts {
        font_size: 14.0 * s,
        color: state.text_color(theme),
        bold: state.text_bold(),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let label_x = x + 46.0 * s;
    let label_y = y + 2.0 * s;
    let label_w = (width - (label_x - x)).max(0.0);
    let rows = wrap_text(sugarloaf, todo.content(), label_w, &text_opts, 2);
    for (index, row) in rows.iter().take(2).enumerate() {
        draw_text_clipped(
            sugarloaf,
            label_x,
            label_y + index as f32 * 15.0 * s,
            row,
            &text_opts,
            occlusion_rects,
        );
    }
}

fn side_panel_task_row_height(s: f32) -> f32 {
    (TODO_ROW_HEIGHT + 18.0) * s
}
