use super::layout::{
    timeline_layout, timeline_row_range_for_source_range,
    timeline_row_range_intersects_viewport, timeline_virtual_row_measurements,
    union_timeline_row_ranges, visible_timeline_row_range,
};
use super::*;

// These are ink bounds, not another footer: the word moves ±3.2px and
// its far echo adds 3.5px; the text renderer allows 14px bottom ink slack.
const STATUS_TOP_INK: f32 = 4.0;
const STATUS_BOTTOM_INK: f32 = 14.0;

#[derive(Debug)]
pub(super) struct ActivityGeometry {
    pub content_h: f32,
    pub body_clip: [f32; 4],
    pub status_rect: [f32; 4],
}

pub(super) fn activity_geometry(
    rect: [f32; 4],
    real_content_h: f32,
    status_h: f32,
    gap: f32,
    s: f32,
    following: bool,
    scroll_top: f32,
) -> ActivityGeometry {
    let [x, y, w, h] = rect;
    let h = h.max(0.0);
    let tail_gap = if real_content_h > 0.0 { gap } else { 0.0 };
    let top_ink = STATUS_TOP_INK * s;
    let bottom_ink = STATUS_BOTTOM_INK * s;
    let content_h = real_content_h
        + if status_h > 0.0 {
            tail_gap + top_ink + status_h + bottom_ink
        } else {
            0.0
        };
    let attached_y = y + real_content_h + tail_gap + top_ink - scroll_top;
    // On a short viewport preserve the primary activity at the top and clip
    // overflowing child lines. Never move the whole block under the composer.
    let status_y = if following {
        (y + h - bottom_ink - status_h).max(y + top_ink).min(y + h)
    } else {
        attached_y
    };
    let body_h = if following && status_h > 0.0 {
        (status_y - top_ink - tail_gap - y).clamp(0.0, h)
    } else {
        h
    };
    ActivityGeometry {
        content_h,
        body_clip: [x, y, w, body_h],
        status_rect: [
            x,
            status_y,
            w,
            if following {
                status_h.min((y + h - status_y).max(0.0))
            } else {
                status_h
            },
        ],
    }
}

pub fn render_timeline_with<P, D>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    occlusion_rects: &[[f32; 4]],
    plugins: Option<&neoism_lua::PluginSnapshot>,
) where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    derivations::reset();
    pane.set_visible_user_orb_active(false);
    pane.set_visible_running_tool_active(false);
    let render_started = web_time::Instant::now();
    let [x, y, w, h] = rect;
    let viewport_h = h.max(0.0);
    let gap = 18.0 * s;
    let layout_started = web_time::Instant::now();
    let (layout, did_layout_work) =
        timeline_layout::<P, D>(sugarloaf, pane, w, theme, s, gap, viewport_h);
    let layout_elapsed_us = web_time::Instant::now()
        .saturating_duration_since(layout_started)
        .as_micros();
    let layout_us = did_layout_work.then_some(layout_elapsed_us);
    let cacheable_layout = true;
    // Everything between layout and the draw loop (set_timeline_metrics,
    // clears, visible-range math, virtual sync) is timed as `prep` so we can
    // see fixed per-frame overhead distinctly from drawing.
    let prep_started = web_time::Instant::now();
    let real_content_h = layout.content_height;
    let row_count = layout.rows.len();
    let status_h = if pane.has_status_activity() {
        let primary_lines =
            streaming_status_primary_line_count(sugarloaf, &pane.streaming_label(), w, s);
        let lines = streaming_status_line_count(
            primary_lines,
            pane.queued_prompt_count(),
            pane.running_background_task_count(),
        );
        STREAMING_STATUS_LINE_H * s * lines as f32
    } else {
        0.0
    };
    // One full-content metric includes the footer exactly once. Do not also
    // subtract its reserve from the metrics viewport (which doubles max-scroll).
    let content_h =
        activity_geometry(rect, real_content_h, status_h, gap, s, false, 0.0).content_h;
    pane.set_timeline_metrics(rect, content_h, viewport_h);
    if did_layout_work {
        if let Some((key, screen_offset)) = pane.timeline_view_anchor() {
            if let Some(row) = resolve_timeline_view_anchor(pane.messages(), &key)
                .and_then(|source_index| {
                    timeline_row_for_anchor_source(&layout.rows, source_index)
                })
            {
                pane.restore_timeline_view_anchor(row.top, screen_offset);
            }
        }
    }
    let set_metrics_done = web_time::Instant::now();
    pane.clear_tool_hit_rects();
    let metrics_done = web_time::Instant::now();

    if viewport_h <= 0.0 {
        if cacheable_layout {
            pane.store_timeline_layout_cache(layout);
        }
        pane.log_timeline_perf(
            row_count,
            0,
            0,
            0,
            0,
            viewport_h,
            content_h,
            cacheable_layout,
            layout_us,
            None,
            None,
            None,
            derivations::take(),
            web_time::Instant::now()
                .saturating_duration_since(render_started)
                .as_micros(),
        );
        return;
    }

    let max_scroll = (content_h - viewport_h).max(0.0);
    let scroll_top = (max_scroll - pane.timeline_scroll_offset()).clamp(0.0, max_scroll);
    let mut draw_scroll_top = scroll_top;
    let geometry = activity_geometry(
        rect,
        real_content_h,
        status_h,
        gap,
        s,
        pane.timeline_follow_bottom(),
        snap_px(scroll_top),
    );
    let body_viewport_h = geometry.body_clip[3];
    // Only materialise per-row measurements when a virtual surface consumes
    // them. On panes that draw straight from the windowed `layout.rows`, this
    // would be O(total history) work allocated every frame for nothing.
    let virtual_rows = if pane.uses_virtual_timeline() {
        if pane.virtual_timeline_needs_measurements(
            w,
            s,
            layout.rows.len(),
            real_content_h,
        ) {
            timeline_virtual_row_measurements(&layout.rows, gap)
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    pane.maybe_request_older_timeline_page(scroll_top, viewport_h);
    pane.sync_virtual_timeline(
        geometry.body_clip,
        w,
        real_content_h,
        scroll_top,
        s,
        &virtual_rows,
    );
    // Keep scroll/range math fractional, but snap draw-space coordinates to
    // physical pixels. Text-heavy rows (especially code blocks) get visibly
    // soft if their glyph baselines ride half-pixels during inertial scroll.
    let render_scroll_top = snap_px(draw_scroll_top);
    let viewport_clip = geometry.body_clip;
    // Keep ordinary scroll frames tight, but preserve the wide registration
    // band while selecting text so drag-to-select still works past the edge.
    // The markdown renderer now culls visible lines itself, so normal scroll
    // only needs a small row overscan for cards near the viewport boundary.
    let register_margin = if pane.has_active_selection() {
        (viewport_h * 4.0).max(2000.0)
    } else {
        (viewport_h * 0.15).max(140.0 * s)
    };
    let visible_range = visible_timeline_row_range(
        &layout.rows,
        draw_scroll_top - register_margin,
        draw_scroll_top + body_viewport_h + register_margin,
    );
    let mut row_range = if pane.has_active_selection() {
        visible_range.clone()
    } else if let Some((source_start, source_end)) =
        pane.virtual_timeline_visible_source_range()
    {
        let virtual_range =
            timeline_row_range_for_source_range(&layout.rows, source_start, source_end);
        if timeline_row_range_intersects_viewport(
            &layout.rows,
            virtual_range.clone(),
            draw_scroll_top,
            draw_scroll_top + body_viewport_h,
        ) {
            union_timeline_row_ranges(virtual_range, visible_range.clone())
        } else {
            visible_range.clone()
        }
    } else {
        visible_range.clone()
    };
    if !timeline_row_range_intersects_viewport(
        &layout.rows,
        row_range.clone(),
        draw_scroll_top - register_margin,
        draw_scroll_top + body_viewport_h + register_margin,
    ) {
        row_range = visible_range.clone();
    }
    if row_range.is_empty() && !visible_range.is_empty() {
        row_range = visible_range.clone();
    }
    let anchor_range = visible_timeline_row_range(
        &layout.rows,
        scroll_top,
        scroll_top + body_viewport_h,
    );
    if let Some(row) = layout.rows.get(anchor_range.start) {
        if !pane.timeline_view_anchor_matches(row.source_index, pane.messages().len()) {
            let key =
                TimelineViewAnchorKey::for_source(pane.messages(), row.source_index);
            pane.set_timeline_view_anchor(key, row.top - scroll_top);
        }
    } else {
        pane.set_timeline_view_anchor(None, 0.0);
    }
    if !layout.rows.is_empty()
        && row_range.is_empty()
        && visible_timeline_row_range(
            &layout.rows,
            draw_scroll_top,
            draw_scroll_top + body_viewport_h,
        )
        .is_empty()
    {
        draw_scroll_top = scroll_top.clamp(0.0, max_scroll);
        pane.sync_virtual_timeline(
            geometry.body_clip,
            w,
            real_content_h,
            draw_scroll_top,
            s,
            &virtual_rows,
        );
        row_range = visible_timeline_row_range(
            &layout.rows,
            draw_scroll_top - register_margin,
            draw_scroll_top + body_viewport_h + register_margin,
        );
    }
    let rendered_row_start = row_range.start;
    let rendered_row_end = row_range.end;
    let rows_started = web_time::Instant::now();
    let prep_us = Some(
        rows_started
            .saturating_duration_since(prep_started)
            .as_micros(),
    );
    if pane.timeline_perf_enabled() {
        // Split `prep` so we can see whether the cost is set_timeline_metrics
        // + clears (`metrics`) or the visible-range / request logic (`range`).
        let metrics_us = metrics_done
            .saturating_duration_since(prep_started)
            .as_micros();
        let set_metrics_us = set_metrics_done
            .saturating_duration_since(prep_started)
            .as_micros();
        let clear_us = metrics_done
            .saturating_duration_since(set_metrics_done)
            .as_micros();
        let range_us = rows_started
            .saturating_duration_since(metrics_done)
            .as_micros();
        tracing::info!(
            target: "neoism::agent_ui_perf",
            metrics_us,
            set_metrics_us,
            clear_us,
            range_us,
            row_range_len = row_range.len(),
            "agent timeline prep split"
        );
    }
    let mut rendered_rows = 0usize;
    let mut rendered_text_bytes = 0usize;
    let mut visible_user_orb = false;
    for row in &layout.rows[row_range.clone()] {
        let card_h = row.height;
        let card_y = snap_px(y + row.top - render_scroll_top);
        let card_bottom = card_y + card_h;

        if card_bottom < y - register_margin
            || card_y > y + body_viewport_h + register_margin
        {
            continue;
        }
        if card_bottom >= y && card_y <= y + body_viewport_h {
            let kind = row.display_message.as_ref().map(|message| message.kind());
            let kind = kind.or_else(|| {
                pane.messages()
                    .get(row.source_index)
                    .map(|message| message.kind())
            });
            if kind == Some(AgentTimelineMessageKind::User) {
                visible_user_orb = true;
            }
        }
        let markdown_blocks =
            row.markdown_blocks.as_ref().map(|blocks| blocks.as_slice());
        let tool_diff_sections = row
            .tool_diff_sections
            .as_ref()
            .map(|sections| sections.as_slice());
        rendered_rows += 1;
        if let Some(message) = row.display_message.as_ref() {
            rendered_text_bytes += message.text().len();
            let message_theme = message_theme(theme, plugins, message.kind());
            D::render_message_card(
                sugarloaf,
                x,
                card_y,
                w,
                card_h,
                pane,
                message,
                markdown_blocks,
                tool_diff_sections,
                &message_theme,
                s,
                now_seconds,
                mouse,
                viewport_clip,
                occlusion_rects,
            );
        } else {
            let mut message =
                if let Some(source_message) = pane.messages().get(row.source_index) {
                    derivations::bump_message_clone();
                    source_message.clone()
                } else {
                    continue;
                };
            if let Some(text) = &row.display_text {
                derivations::bump_message_clone();
                message = message.with_text(text.clone());
            }
            rendered_text_bytes += message.text().len();
            let message_theme = message_theme(theme, plugins, message.kind());
            D::render_message_card(
                sugarloaf,
                x,
                card_y,
                w,
                card_h,
                pane,
                &message,
                markdown_blocks,
                tool_diff_sections,
                &message_theme,
                s,
                now_seconds,
                mouse,
                viewport_clip,
                occlusion_rects,
            );
        }
    }
    let rows_ended = web_time::Instant::now();
    let rows_us = Some(
        rows_ended
            .saturating_duration_since(rows_started)
            .as_micros(),
    );
    debug_agent_timeline_black_frame(
        pane.messages().len(),
        row_count,
        rendered_rows,
        row_range.start,
        row_range.end,
        draw_scroll_top,
        viewport_h,
        content_h,
        pane.timeline_scroll_offset(),
        pane.virtual_timeline_visible_source_range(),
    );

    // Following owns a pinned footer even while its spring has positive lag.
    // Browsing retains the original content-attached/offscreen semantics.
    if status_h > 0.0 {
        let row_y = geometry.status_rect[1];
        if row_y + status_h + STATUS_BOTTOM_INK * s >= y
            && row_y - STATUS_TOP_INK * s < y + viewport_h
        {
            D::render_streaming_status_row(
                sugarloaf,
                pane,
                geometry.status_rect,
                theme,
                s,
                now_seconds,
                [x, y, w, viewport_h],
                occlusion_rects,
            );
        }
    }

    pane.set_visible_user_orb_active(visible_user_orb);
    render_timeline_scrollbar_with(sugarloaf, pane, [x, y, w, viewport_h], s);
    if cacheable_layout {
        pane.store_timeline_layout_cache(layout);
    }
    let derivations = derivations::take();
    pane.log_timeline_perf(
        row_count,
        rendered_rows,
        rendered_text_bytes,
        rendered_row_start,
        rendered_row_end,
        viewport_h,
        content_h,
        cacheable_layout,
        layout_us,
        rows_us,
        prep_us,
        Some(
            web_time::Instant::now()
                .saturating_duration_since(rows_ended)
                .as_micros(),
        ),
        derivations,
        web_time::Instant::now()
            .saturating_duration_since(render_started)
            .as_micros(),
    );
}

fn message_theme(
    theme: &IdeTheme,
    plugins: Option<&neoism_lua::PluginSnapshot>,
    kind: AgentTimelineMessageKind,
) -> IdeTheme {
    let Some(plugins) = plugins else {
        return *theme;
    };
    let selector = match kind {
        AgentTimelineMessageKind::User => "agent.chat.message.user",
        AgentTimelineMessageKind::Assistant => "agent.chat.message.assistant",
        AgentTimelineMessageKind::Tool | AgentTimelineMessageKind::Subtask => {
            "agent.chat.tool"
        }
        AgentTimelineMessageKind::Reasoning
        | AgentTimelineMessageKind::System
        | AgentTimelineMessageKind::Compaction => "agent.chat.message",
    };
    crate::customization::styled_ide_theme(
        *theme,
        &crate::primitives::surface_background::resolve_style(selector, &plugins.styles),
    )
}

#[allow(clippy::too_many_arguments)]
fn debug_agent_timeline_black_frame(
    messages: usize,
    rows: usize,
    rendered_rows: usize,
    row_start: usize,
    row_end: usize,
    scroll_top: f32,
    viewport_h: f32,
    content_h: f32,
    scroll_px: f32,
    virtual_range: Option<(usize, usize)>,
) {
    if messages == 0 || rows == 0 || rendered_rows > 0 {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    {
        if !wasm_agent_timeline_debug_enabled() {
            return;
        }
        let message = format!(
            "[neoism-agent-timeline] black-frame messages={messages} rows={rows} rendered_rows={rendered_rows} row_range={row_start}..{row_end} scroll_top={scroll_top:.1} viewport_h={viewport_h:.1} content_h={content_h:.1} scroll_px={scroll_px:.1} virtual_range={virtual_range:?}"
        );
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&message));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (
        messages,
        rows,
        rendered_rows,
        row_start,
        row_end,
        scroll_top,
        viewport_h,
        content_h,
        scroll_px,
        virtual_range,
    );
}

#[cfg(target_arch = "wasm32")]
fn wasm_agent_timeline_debug_enabled() -> bool {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| {
            storage
                .get_item("neoism_debug_agent_timeline")
                .ok()
                .flatten()
        })
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub(crate) fn cached_message_height<P, D>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    message: &P::Message,
    width: f32,
    theme: &IdeTheme,
    s: f32,
) -> f32
where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    let tool_expanded = pane.tool_expanded(message.id());
    let tool_expand_progress = pane.tool_expand_progress(message.id());
    let tool_expand_animating = pane.tool_expand_animating(message.id());
    let selected_group_child = pane.selected_tool_group_child(message.id());
    let key =
        pane.timeline_measure_key(message, width, s, tool_expanded, selected_group_child);
    if !tool_expand_animating {
        if let Some(height) = pane.cached_timeline_measure(&key) {
            return height;
        }
    }
    let height = D::measure_message_height(
        sugarloaf,
        pane,
        message,
        width,
        theme,
        s,
        tool_expanded,
        tool_expand_progress,
    );
    if !tool_expand_animating {
        pane.store_timeline_measure(key, height);
    }
    height
}

pub(crate) fn f32_measure_bucket(value: f32) -> i32 {
    (value.max(0.0) * 4.0).round() as i32
}

fn snap_px(value: f32) -> f32 {
    if value.is_finite() {
        value.round()
    } else {
        value
    }
}

pub(crate) fn display_timeline_message<M: AgentTimelineMessage>(
    message: &M,
    previous_visible_was_edit_tool: bool,
) -> Option<M> {
    if is_subagent_runtime_notification(message) {
        return None;
    }
    if message.kind() == AgentTimelineMessageKind::System
        && message.tool() != "location_notice"
    {
        return None;
    }
    let mut display_message = message.clone();
    if matches!(
        message.kind(),
        AgentTimelineMessageKind::Assistant | AgentTimelineMessageKind::Reasoning
    ) {
        let safe_text = super::super::markdown::safe_canvas_markdown(message.text());
        if safe_text.trim().is_empty() {
            // A Markdown HTML comment/declaration is a non-rendering node, not
            // a tiny text message. Drop the timeline item before both eager
            // measurement and lazy estimation so it cannot leave a phantom
            // row or scrollbar height behind.
            return None;
        }
        if safe_text.as_ref() != message.text() {
            display_message = message.with_text(safe_text.into_owned());
        }
    }
    if previous_visible_was_edit_tool
        && display_message.kind() == AgentTimelineMessageKind::Assistant
    {
        if let Some(text) = strip_redundant_edit_recap_code(display_message.text()) {
            if text.trim().is_empty() {
                return None;
            }
            return Some(display_message.with_text(text));
        }
    }
    Some(display_message)
}

pub(crate) fn is_subagent_runtime_notification<M: AgentTimelineMessage>(
    message: &M,
) -> bool {
    // This is provider context injected by the runtime, never a human or
    // assistant transcript row. Check the reserved opening immediately so a
    // partial live delta cannot flash before its role/system metadata lands.
    message
        .text()
        .trim_start()
        .starts_with("Subagent finished.")
}

fn strip_redundant_edit_recap_code(text: &str) -> Option<String> {
    if !text.contains("```") {
        return None;
    }
    let without_code = strip_fenced_code_blocks(text);
    if without_code == text {
        return None;
    }
    let prose = collapse_blank_lines(without_code.trim());
    if prose.is_empty() || looks_like_edit_recap(&prose) {
        return Some(prose);
    }
    None
}

fn strip_fenced_code_blocks(text: &str) -> String {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push(line);
        }
    }
    out.join("\n")
}

fn collapse_blank_lines(text: &str) -> String {
    let mut out = Vec::new();
    let mut previous_blank = false;
    for line in text.lines() {
        let blank = line.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        out.push(line.trim_end());
        previous_blank = blank;
    }
    out.join("\n").trim().to_string()
}

fn looks_like_edit_recap(text: &str) -> bool {
    if text.lines().count() > 4 || text.chars().count() > 280 {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    [
        "added", "adding", "updated", "changed", "edited", "created", "removed",
        "deleted", "applied", "wrote", "writing", "replaced",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

pub(crate) fn is_edit_tool_message<M: AgentTimelineMessage>(message: &M) -> bool {
    if message.kind() != AgentTimelineMessageKind::Tool {
        return false;
    }
    let normalized = message
        .tool()
        .chars()
        .filter(|ch| *ch != '_' && *ch != '-')
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "applypatch" | "patch" | "edit" | "write" | "multiedit"
    )
}

pub fn render_timeline_scrollbar_with<P: AgentTimelinePane>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    rect: [f32; 4],
    s: f32,
) {
    let Some((offset, content_h, viewport_h, opacity)) = pane.timeline_scrollbar_state()
    else {
        pane.set_scrollbar_geometry(None, None);
        return;
    };
    let [x, y, w, h] = rect;
    let track_h = h.max(0.0);
    // Offset counts up from the BOTTOM here; the shared helper wants
    // distance from the top.
    let max_scroll = (content_h - viewport_h).max(1.0);
    let scroll_top = max_scroll - offset.clamp(0.0, max_scroll);
    let Some((thumb_y, thumb_h)) = scrollbar::compute_thumb_px(
        viewport_h,
        content_h,
        y,
        track_h,
        scroll_top,
        scrollbar::min_thumb_height() * s,
    ) else {
        pane.set_scrollbar_geometry(None, None);
        return;
    };
    let thumb_x = x + w - scrollbar::width() * s - scrollbar::SCROLLBAR_MARGIN * s;
    let thumb_w = scrollbar::width() * s;
    // Hit-test rect is a bit wider than the visible thumb so clicks don't
    // demand pixel-perfect aim. Track rect spans the whole viewport height
    // along the same x band.
    let hit_pad = 4.0 * s;
    let track_rect = [thumb_x - hit_pad, y, thumb_w + hit_pad * 2.0, track_h];
    let thumb_hit_rect = [thumb_x - hit_pad, thumb_y, thumb_w + hit_pad * 2.0, thumb_h];
    pane.set_scrollbar_geometry(Some(track_rect), Some(thumb_hit_rect));
    scrollbar::draw_track_scaled(
        sugarloaf,
        false,
        thumb_x,
        y,
        track_h,
        s,
        opacity,
        DEPTH,
        ORDER_CARET,
    );
    scrollbar::draw_thumb_scaled(
        sugarloaf,
        false,
        thumb_x,
        thumb_y,
        thumb_h,
        s,
        opacity,
        false,
        DEPTH,
        ORDER_CARET,
    );
}
