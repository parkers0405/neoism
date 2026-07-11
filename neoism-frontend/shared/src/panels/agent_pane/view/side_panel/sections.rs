use super::draw::{
    draw_status_dot_text, draw_subagent_spinner, intersect_rect,
    push_provider_icon_clipped,
};
use super::*;

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
    // Styled like a Markdown H3 heading: the mirrored H3 size, bold, drawn in
    // the whitest theme token (`theme.fg` == 0xe8e8e8 on pastel_dark; the
    // `theme.white` token is a bluish grey there) rather than the old muted
    // header grey.
    let header_size = section_header_font_size(s);
    let rest_opts = DrawOpts {
        font_size: header_size,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };

    // Illuminated drop-cap: the first letter in the UnifrakturMaguntia
    // blackletter, scaled up from the H3 header size and drawn white; the rest
    // of the label in the bold H3 header font on the same baseline. Falls back
    // to a plain header if the font is missing.
    let mut chars = label.chars();
    if let Some(first) = chars.next() {
        let rest: String = chars.collect();
        let cap_font = crate::primitives::maguntia_font_id(sugarloaf);
        let cap_size = header_size * 1.4;
        let cap_opts = DrawOpts {
            font_size: cap_size,
            color: theme.u8(theme.fg),
            bold: false,
            italic: false,
            font_id: cap_font,
            clip_rect: Some(clip),
        };
        let first_str = first.to_string();
        // Lift the taller cap so its baseline matches the rest (text y is the
        // glyph top, so a larger glyph would otherwise sit lower).
        let cap_y = y - (cap_size - header_size) * 0.75;
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
    // Advance past the taller H3 title with breathing room so the value line
    // below never rides up into its descenders or the raised drop-cap.
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
    let truncated = truncate_to_fit(line, width, sugarloaf, &opts);
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
    let value_str = truncate_to_fit(value, value_budget, sugarloaf, &value_opts);
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_session_info<I: AgentSidePanelIconHost>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentSidePanelPane,
    content_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    occlusion_rects: &[[f32; 4]],
    inner_radius: f32,
) {
    // Hydrate the branch list once when the session changes. Live
    // branch status/tool changes arrive through the session SSE stream.
    pane.maybe_refresh_side_panel_subagents();
    // Auto-hide sub-agents that finished more than a few seconds ago
    // (Claude-style). Respawned children report active status again and
    // are kept. Pruning here — not just on refresh — means the row
    // disappears on the right timer even if the server is quiet.
    pane.side_panel_mut().prune_expired_completed_subagents();

    let [cx, cy, cw, ch] = content_rect;
    let pad_x = ROW_PADDING_X * s;
    let text_x = cx + pad_x;
    let text_w = (cw - pad_x * 2.0).max(0.0);
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
    y += 6.0 * s;

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

    // --- Usage ---
    // Pulls already-formatted lines from `usage_detail_lines()`:
    //   [0] "Context X%   Y / Z tokens"  (or "Context Y tokens" if no limit)
    //   [1] "Total price $X.XX"
    //   [2] "Last turn $X.XX"
    // We render the first three — anything past that (input/output/
    // cache/etc.) is already reachable via the usage chip's context
    // menu and would crowd the panel.
    let usage_lines = pane.usage_detail_lines();
    if !usage_lines.is_empty() {
        y += 6.0 * s;
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
        for (ix, line) in usage_lines.iter().take(3).enumerate() {
            let color = if ix == 0 {
                theme.u8(theme.fg)
            } else {
                theme.u8(theme.dim)
            };
            y = render_text_line(
                sugarloaf,
                line,
                text_x,
                y,
                text_w,
                color,
                theme,
                s,
                clip,
                occlusion_rects,
            );
        }
    }

    // --- Active Goal ---
    // The session's persistent active/blocked goal sits ABOVE branches and
    // tasks, so Alt+H immediately shows what the agent is still pursuing.
    if let Some(goal) = pane.side_panel().session_goal().cloned() {
        y += 6.0 * s;
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

    // --- Branches ---
    // Single header covers both the parent ("main session") and its
    // children. The picker always returns "main session" as row 0; we
    // only render the section when there's at least one *real* child
    // below it. The list is laid out at its FULL natural height inside the
    // outer scrolled column (every row visible once you scroll to it) —
    // the page scroll, not an inner window, governs visibility.
    let subagents_count = pane.side_panel().subagents().len();
    let has_real_subagents = subagents_count > 1;
    if has_real_subagents {
        y += 6.0 * s;
        y = render_section_header(
            sugarloaf,
            "Branches",
            text_x,
            y,
            theme,
            s,
            clip,
            occlusion_rects,
        );
        let list_top = y;
        let row_h = pane.side_panel().row_height() * s;
        let list_h = row_h * subagents_count as f32;
        let list_rect = [cx, list_top, cw, list_h];
        render_subagent_rows::<I>(
            sugarloaf,
            pane,
            list_rect,
            clip,
            theme,
            s,
            now_seconds,
            occlusion_rects,
            inner_radius,
        );
        y = list_top + list_h;
    } else {
        pane.side_panel_mut().clear_row_hit_rect();
    }

    // --- Tasks ---
    // Reflects the latest TodoWrite tool message verbatim. Updates are
    // entirely action-driven — every TodoWrite the model emits arrives
    // as a new message, so reading from `pane.messages()` each frame is
    // all that's needed for the check-off to mirror the chat.
    if !tasks.is_empty() {
        if has_real_subagents {
            y += 10.0 * s;
        }
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

/// Render the Goal section: a "GOAL" header, a status badge
/// (active/complete/blocked, plus "paused" when applicable), the goal
/// text wrapped to a few lines, and the agent's `summary` when present.
/// Returns the `y` below the section.
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
    let mut y = render_section_header(
        sugarloaf,
        "Active Goal",
        x,
        y,
        theme,
        s,
        clip,
        occlusion_rects,
    );

    // Status badge — color-coded by lifecycle, mirroring the branch dots.
    let badge_color = match goal.status {
        GoalStatus::Active => theme.u8(theme.accent),
        GoalStatus::Complete => theme.u8(theme.green),
        GoalStatus::Blocked => theme.u8(theme.red),
    };
    let badge_text = if goal.paused {
        format!("{} · paused", goal.status.label())
    } else {
        goal.status.label().to_string()
    };
    let badge_opts = DrawOpts {
        font_size: FONT_SIZE * s * 0.82,
        color: badge_color,
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    draw_text_with_occlusion(
        sugarloaf,
        x,
        y,
        &badge_text.to_ascii_uppercase(),
        &badge_opts,
        occlusion_rects,
    );
    y += FONT_SIZE * s * 1.5;

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

/// Find the most recent message containing a non-empty todo list. The
/// chat already coalesces partial updates of the same tool-message id
/// (see `pane.rs` — `incoming.todos = existing.todos` carry-over), so
/// this walks history backward until it hits something with real
/// entries and returns that slice.
fn latest_todos<M: AgentSidePanelMessage>(messages: &[M]) -> &[M::Todo] {
    for message in messages.iter().rev() {
        if message.is_todos_output() && !message.todos().is_empty() {
            return message.todos();
        }
    }
    &[]
}

fn tasks_section_height(todos_len: usize, s: f32) -> f32 {
    if todos_len == 0 {
        return 0.0;
    }
    let visible = todos_len.min(TASKS_MAX_VISIBLE);
    // Matches `render_section_header`'s H3 advance (`header_size * 1.5`).
    let header_h = section_header_font_size(s) * 1.5;
    let rows_h = visible as f32 * side_panel_task_row_height(s);
    let overflow_h = if todos_len > TASKS_MAX_VISIBLE {
        FONT_SIZE * s * 1.45
    } else {
        0.0
    };
    // 6px breathing room above header, ~6px below the last row.
    header_h + rows_h + overflow_h + 12.0 * s
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

    let mut y = ry + 6.0 * s;
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
    let visible = todos.len().min(TASKS_MAX_VISIBLE);
    for todo in todos.iter().take(visible) {
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
    if todos.len() > TASKS_MAX_VISIBLE {
        let extra = todos.len() - TASKS_MAX_VISIBLE;
        let opts = DrawOpts {
            font_size: FONT_SIZE * s * 0.82,
            color: theme.u8(theme.dim),
            italic: true,
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        draw_text_with_occlusion(
            sugarloaf,
            text_x,
            y + 2.0 * s,
            &format!("+{extra} more"),
            &opts,
            occlusion_rects,
        );
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

#[allow(clippy::too_many_arguments)]
fn render_subagent_rows<I: AgentSidePanelIconHost>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentSidePanelPane,
    list_rect: [f32; 4],
    viewport: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    occlusion_rects: &[[f32; 4]],
    inner_radius: f32,
) {
    // In chat mode the branch list is rendered at its *full* natural
    // height inside the outer scrolled content column: every row is laid
    // out at its index and the whole column moves with the page scroll, so
    // there is no inner row-windowing. `list_rect` therefore spans all
    // rows (often taller than the panel) while `viewport` is the actual
    // visible content rect — all drawing/hit-testing clips to it so rows
    // scrolled off-screen don't bleed over neighbouring sections.
    let row_h = pane.side_panel().row_height() * s; // mode-aware: 2x in chat
    let half_h = row_h * 0.5;
    // Hit rect is the *visible* slice of the (taller) full list — clamped
    // to the viewport so a click in a lower section (e.g. Tasks) doesn't
    // map onto a branch row scrolled off the bottom. The row math keys off
    // the full list's top (`list_rect[1]`, already shifted by the page
    // scroll), which can sit above the viewport; `set_row_hit_rect` keeps
    // that origin so indices stay correct, only the bounds are clamped.
    let hit_rect = intersect_rect(list_rect, viewport).unwrap_or(list_rect);
    pane.side_panel_mut()
        .set_row_hit_rect_with_origin(hit_rect, list_rect[1], row_h);

    let cursor_offset = pane.side_panel_mut().tick_cursor();
    let selected = pane.side_panel().selected_index();
    let focused = pane.side_panel().is_focused();
    // Hit-test / highlight against the visible viewport, not the (taller)
    // full list rect, so a row scrolled past the panel edge isn't lit.
    let list_bottom = viewport[1] + viewport[3];
    let list_top = viewport[1];
    let pad_x = ROW_PADDING_X * s;
    let text_x = list_rect[0] + pad_x;
    let text_w = (list_rect[2] - pad_x * 2.0).max(0.0);
    let icons_ready = I::register_agent_icons(sugarloaf);

    pane.side_panel_mut().clear_selected_cursor_rect();
    let rows = pane.side_panel().subagents().to_vec();
    let rows_len = rows.len();
    if rows_len == 0 {
        return;
    }

    // Keyboard nav (Alt+arrows) moves the branch selection; nudge the
    // page scroll so the focused row stays inside the viewport, mirroring
    // how file_tree keeps its cursor visible. The branch row's offset
    // from the content top is fixed; we only need the scroll to land it
    // between the viewport edges.
    if focused && selected < rows_len {
        let row_offset_in_list = selected as f32 * row_h;
        // Row top/bottom in *unscrolled* content space, relative to the
        // list's scrolled origin: convert back to absolute by adding the
        // current scroll, then ask for a scroll that frames the row.
        let scroll = pane.side_panel().content_scroll_px();
        let row_abs_top = (list_rect[1] + scroll) + row_offset_in_list;
        let row_abs_bottom = row_abs_top + row_h;
        // Desired visible window in absolute content space.
        let view_top = viewport[1] + scroll;
        let view_bottom = view_top + viewport[3];
        if row_abs_top < view_top {
            pane.side_panel_mut()
                .scroll_content_pixels(row_abs_top - view_top);
        } else if row_abs_bottom > view_bottom {
            pane.side_panel_mut()
                .scroll_content_pixels(row_abs_bottom - view_bottom);
        }
    }

    if selected < rows_len {
        let row_ix = selected as isize;
        let row_y = list_rect[1] + row_ix as f32 * row_h + cursor_offset;
        let row_bottom = row_y + row_h;
        let visible_y = row_y.max(list_top);
        let visible_h = row_bottom.min(list_bottom) - visible_y;
        if visible_h > 0.0 {
            sugarloaf.quad(
                None,
                list_rect[0],
                visible_y,
                list_rect[2],
                visible_h,
                theme.f32_alpha(theme.surface, 0.55),
                edge_row_radii(visible_y, visible_h, list_top, list_bottom, inner_radius),
                DEPTH,
                ORDER_PANEL + 2,
            );
            if focused {
                // Cursor lands on the top (title) half of the branch
                // row — that's the user's anchor point, the activity
                // sub-row is auxiliary.
                let font_size = FONT_SIZE * s;
                let cursor_w = (font_size * 0.6).max(2.0);
                let cursor_x = list_rect[0] + (pad_x - cursor_w).max(0.0);
                let cursor_h = (half_h - 6.0 * s).max(font_size).min(half_h);
                let cursor_y = (row_y + (half_h - cursor_h) / 2.0)
                    .clamp(list_top, (list_bottom - cursor_h).max(list_top));
                pane.side_panel_mut()
                    .set_selected_cursor_rect([cursor_x, cursor_y, cursor_w, cursor_h]);
            }
        }
    }

    let title_opts = DrawOpts {
        font_size: FONT_SIZE * s,
        color: theme.u8(theme.fg),
        clip_rect: Some(viewport),
        ..DrawOpts::default()
    };
    let connector_opts = DrawOpts {
        font_size: FONT_SIZE * s * 0.95,
        color: theme.u8(theme.muted),
        clip_rect: Some(viewport),
        ..DrawOpts::default()
    };
    let activity_opts = DrawOpts {
        font_size: FONT_SIZE * s * 0.88,
        color: theme.u8(theme.dim),
        italic: true,
        clip_rect: Some(viewport),
        ..DrawOpts::default()
    };
    let current_id = pane.session_id_str().map(str::to_string);

    // Blinking white dot opacity for Active state — sin sweep keeps
    // the dot lively without wandering off into invisible.
    let blink_alpha = 0.55 + 0.45 * (now_seconds * 4.0).sin().abs();

    for absolute_ix in 0..rows_len {
        let entry = &rows[absolute_ix];
        let row_ix = absolute_ix as isize;
        let row_y = list_rect[1] + row_ix as f32 * row_h;
        let row_bottom = row_y + row_h;
        let visible_y = row_y.max(list_top);
        let visible_h = row_bottom.min(list_bottom) - visible_y;
        if visible_h <= 0.0 {
            continue;
        }
        let Some(row_clip) =
            intersect_rect(viewport, [list_rect[0], row_y, list_rect[2], row_h])
        else {
            continue;
        };
        let is_main_row = absolute_ix == 0;
        let is_current = current_id.as_deref() == Some(entry.id.as_str());
        let title_color = if is_current {
            theme.u8(theme.accent)
        } else {
            theme.u8(theme.fg)
        };

        // --- Top half: title left, status dot right. ---
        // Lead with the live status dot so activity is visible even when
        // the branch title is long.
        let activity = subagent_row_activity(pane, entry, is_main_row);
        let title_y = row_y + (half_h - FONT_SIZE * s) / 2.0;
        let dot_diameter = (FONT_SIZE * s * 0.62).max(6.0);
        let depth = if is_main_row { 0 } else { entry.depth.max(1) };
        let depth_indent = depth.saturating_sub(1) as f32 * 14.0 * s;
        let tree_x = text_x + depth_indent;
        let dot_x = tree_x;
        let dot_y = row_y + (half_h - dot_diameter) / 2.0;
        let mut title_x = if is_main_row {
            text_x
        } else {
            dot_x + dot_diameter + 8.0 * s
        };

        // A running sub-agent wears the terminal's rainbow loader spinner
        // (the same orbiting pastel trail the running-block chrome uses)
        // instead of a static dot. Every other state keeps its dot.
        let is_running_spinner = matches!(
            activity.as_ref().map(|a| a.status),
            Some(BranchStatus::Active)
        );
        let (dot_color, halo_color, halo) = match activity.as_ref().map(|a| a.status) {
            Some(BranchStatus::Completed) => {
                (theme.u8(theme.green), theme.u8(theme.white), None)
            }
            Some(BranchStatus::Stopped) => {
                (theme.u8(theme.red), theme.u8(theme.white), None)
            }
            Some(BranchStatus::WaitingPermission) => (
                theme.u8_alpha(theme.yellow, blink_alpha),
                theme.u8(theme.yellow),
                Some(blink_alpha),
            ),
            Some(BranchStatus::Active) => (
                theme.u8_alpha(theme.white, blink_alpha),
                theme.u8(theme.white),
                Some(blink_alpha),
            ),
            None => (theme.u8(theme.green), theme.u8(theme.white), None),
        };

        {
            let title_opts_row = DrawOpts {
                color: title_color,
                clip_rect: Some(row_clip),
                ..title_opts
            };
            let connector_title_opts = DrawOpts {
                clip_rect: Some(row_clip),
                ..connector_opts
            };
            if !is_main_row && depth > 1 {
                let branch_x = text_x + (depth - 2) as f32 * 14.0 * s;
                draw_text_with_occlusion(
                    sugarloaf,
                    branch_x,
                    title_y,
                    "╰─",
                    &connector_title_opts,
                    occlusion_rects,
                );
            }
            if !is_main_row {
                if is_running_spinner {
                    draw_subagent_spinner(
                        sugarloaf,
                        dot_x,
                        dot_y,
                        dot_diameter,
                        now_seconds,
                        row_clip,
                        s,
                    );
                } else {
                    draw_status_dot_text(
                        sugarloaf,
                        dot_x,
                        dot_y,
                        dot_diameter,
                        dot_color,
                        halo.map(|alpha| (halo_color, (1.0 - alpha) * 0.25)),
                        row_clip,
                        occlusion_rects,
                        s,
                    );
                }
                if let (true, Some(kind)) = (icons_ready, entry.agent_kind) {
                    let icon_size = (FONT_SIZE * s * 1.12).max(13.0);
                    let icon_y = row_y + (half_h - icon_size) / 2.0;
                    push_provider_icon_clipped(
                        sugarloaf,
                        kind,
                        [title_x, icon_y, icon_size, icon_size],
                        row_clip,
                        occlusion_rects,
                    );
                    title_x += icon_size + 7.0 * s;
                }
            }
            let title_right = list_rect[0] + list_rect[2] - pad_x;
            let title_budget = (title_right - title_x).max(0.0);
            let title_text =
                truncate_to_fit(&entry.title, title_budget, sugarloaf, &title_opts_row);
            draw_text_with_occlusion(
                sugarloaf,
                title_x,
                title_y,
                &title_text,
                &title_opts_row,
                occlusion_rects,
            );
        }

        // --- Bottom half: `╰─` connector + current tool, full-width. ---
        let bottom_y = row_y + half_h;
        let bottom_text_y = bottom_y + (half_h - FONT_SIZE * s * 0.88) / 2.0;
        let indent_x = tree_x + 4.0 * s;
        let connector_glyph = "╰─";
        let connector_activity_opts = DrawOpts {
            clip_rect: Some(row_clip),
            ..connector_opts
        };
        let activity_opts_row = DrawOpts {
            clip_rect: Some(row_clip),
            ..activity_opts
        };
        let connector_w = sugarloaf
            .text_mut()
            .measure(connector_glyph, &connector_activity_opts);
        draw_text_with_occlusion(
            sugarloaf,
            indent_x,
            bottom_text_y,
            connector_glyph,
            &connector_activity_opts,
            occlusion_rects,
        );

        let activity_x = indent_x + connector_w + 6.0 * s;
        let activity_budget = (text_x + text_w - activity_x).max(0.0);
        let activity_text = activity
            .as_ref()
            .and_then(|a| a.current_tool.clone())
            .or_else(|| activity.as_ref().map(|a| a.status.label().to_string()))
            .unwrap_or_else(|| {
                if is_main_row {
                    "main session".to_string()
                } else {
                    BranchStatus::Completed.label().to_string()
                }
            });
        let truncated_activity = truncate_to_fit(
            &activity_text,
            activity_budget,
            sugarloaf,
            &activity_opts_row,
        );
        draw_text_with_occlusion(
            sugarloaf,
            activity_x,
            bottom_text_y,
            &truncated_activity,
            &activity_opts_row,
            occlusion_rects,
        );
    }
}
