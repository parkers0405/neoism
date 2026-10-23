use super::layout::{
    timeline_layout, timeline_row_range_for_source_range,
    timeline_row_range_intersects_viewport, timeline_virtual_row_measurements,
    union_timeline_row_ranges, visible_timeline_row_range,
};
use super::*;
use crate::panels::agent_pane::question_rail::{
    question_index_at, TimelineQuestionTarget, TimelineQuestionTargets,
};
use sugarloaf::text::DrawOpts;

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
    // Short conversations keep activity at the transcript tail, not across an
    // empty viewport. Long responses retain the existing following placement.
    // On a short viewport preserve the primary activity and clip child lines.
    let status_y = if following {
        (y + h - bottom_ink - status_h)
            .max(y + top_ink)
            .min(y + h)
            .min(attached_y)
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

/// Following activity keeps its body reservation; browsing can paint to the pane bottom.
pub(super) fn body_paint_clip(
    geometry: &ActivityGeometry,
    paint_rect: [f32; 4],
    following: bool,
    status_h: f32,
) -> [f32; 4] {
    if following && status_h > 0.0 {
        geometry.body_clip
    } else {
        [
            geometry.body_clip[0],
            geometry.body_clip[1],
            geometry.body_clip[2],
            paint_rect[3].max(geometry.body_clip[3]),
        ]
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
    render_timeline_in_paint_viewport_with::<P, D>(
        sugarloaf,
        pane,
        rect,
        rect,
        theme,
        s,
        now_seconds,
        mouse,
        occlusion_rects,
        occlusion_rects,
        plugins,
        None,
    );
}

/// The paint viewport shares the metrics origin/width, but may extend below it.
/// Keep silhouette hit masks separate from legacy text occlusions: revealed
/// text treats those as full-line cuts. The renderer clips final glyphs in 2D.
#[allow(clippy::too_many_arguments)]
pub fn render_timeline_in_paint_viewport_with<P, D>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    rect: [f32; 4],
    paint_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    occlusion_rects: &[[f32; 4]],
    hit_occlusion_rects: &[[f32; 4]],
    plugins: Option<&neoism_lua::PluginSnapshot>,
    question_gutter: Option<[f32; 4]>,
) -> Option<TimelineQuestionRailFrame>
where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    let rail_mouse = mouse.filter(|(x, y)| {
        !hit_occlusion_rects.iter().any(|r| {
            crate::panels::agent_pane::interaction_policy::rect_contains(*r, *x, *y)
        })
    });
    derivations::reset();
    pane.set_visible_user_orb_active(false);
    pane.set_visible_running_tool_active(false);
    let render_started = web_time::Instant::now();
    let [x, y, w, h] = rect;
    let viewport_h = h.max(0.0);
    let gap = 18.0 * s;
    let layout_started = web_time::Instant::now();
    let (mut layout, did_layout_work, live_growth_px) = timeline_layout::<P, D>(
        sugarloaf,
        pane,
        w,
        theme,
        s,
        gap,
        viewport_h,
        paint_rect[3],
    );
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
    // Every new/reconciled layout is marked as work, including empty histories.
    // Populate even a zero-height layout before caching it, so its first visible
    // frame can reuse the projection without scanning assistant-only histories.
    if did_layout_work {
        layout.question_targets = question_targets(&layout.rows, pane.messages());
    }
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
    pane.set_timeline_metrics(rect, content_h, viewport_h, live_growth_px, s);
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
        pane.set_timeline_question_rail_targets(Rc::default());
        pane.set_scrollbar_geometry(None, None);
        pane.set_timeline_paint_region(TimelinePaintRegion {
            viewport: paint_rect,
            occlusion_rects: hit_occlusion_rects.to_vec(),
        });
        return None;
    }

    let max_scroll = (content_h - viewport_h).max(0.0);
    let scroll_top = (max_scroll - pane.timeline_scroll_offset()).clamp(0.0, max_scroll);
    pane.set_timeline_question_rail_targets(layout.question_targets.clone());
    let mut rail_frame = question_gutter.and_then(|gutter| {
        timeline_scroll_rail_geometry(gutter, s, layout.question_targets.len())
    });
    if let Some(frame) = &mut rail_frame {
        frame.hovered = frame.hovered_tick(rail_mouse);
        let next = layout
            .question_targets
            .partition_point(|target| target.top < scroll_top);
        frame.active = if layout
            .question_targets
            .get(next)
            .is_some_and(|target| target.top < scroll_top + viewport_h)
        {
            Some(next)
        } else {
            next.checked_sub(1)
        };
        if let Some(index) = frame.hovered {
            frame.preview = build_question_preview(
                sugarloaf,
                &layout.question_targets[index],
                pane.messages(),
                rect,
                frame,
                index,
                s,
            );
        }
    }
    let mut local_hit_occlusions = hit_occlusion_rects.to_vec();
    let mut previous_renderer_holes = None;
    if let Some(preview) = rail_frame.as_ref().and_then(|frame| frame.preview.as_ref()) {
        // Keep this a 2D mask; legacy text occlusions cut entire lines.
        local_hit_occlusions.push(preview.rect);
        let previous = sugarloaf.set_content_occlusion_rects(Vec::new());
        let mut holes = previous.clone();
        holes.push(preview.rect);
        sugarloaf.set_content_occlusion_rects(holes);
        previous_renderer_holes = Some(previous);
    }
    let paint_region = TimelinePaintRegion {
        viewport: paint_rect,
        occlusion_rects: local_hit_occlusions,
    };
    let mouse = mouse.filter(|(x, y)| paint_region.contains(*x, *y));
    pane.set_timeline_paint_region(paint_region);
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
    let body_clip = body_paint_clip(
        &geometry,
        paint_rect,
        pane.timeline_follow_bottom(),
        status_h,
    );
    let body_viewport_h = body_clip[3];
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
        body_clip,
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
    let viewport_clip = body_clip;
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
        scroll_top + geometry.body_clip[3],
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
            body_clip,
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

    // Following caps activity at the viewport bottom once the transcript fills it,
    // even while its spring has positive lag. Browsing stays content-attached.
    if status_h > 0.0 {
        let row_y = geometry.status_rect[1];
        let status_clip = if pane.timeline_follow_bottom() {
            rect
        } else {
            paint_rect
        };
        if row_y + status_h + STATUS_BOTTOM_INK * s >= y
            && row_y - STATUS_TOP_INK * s < status_clip[1] + status_clip[3]
        {
            D::render_streaming_status_row(
                sugarloaf,
                pane,
                geometry.status_rect,
                theme,
                s,
                now_seconds,
                status_clip,
                occlusion_rects,
            );
        }
    }

    pane.set_visible_user_orb_active(visible_user_orb);
    if let Some(previous) = previous_renderer_holes {
        sugarloaf.set_content_occlusion_rects(previous);
    }
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
    rail_frame
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
    // Artifact metrics are host-fed independently of message text. Layout dirty
    // marks must remeasure them rather than revive a pre-metric cached height.
    let cacheable = !tool_expand_animating
        && message.tool() != "compose"
        && !message.text().contains("neoism-html");
    if cacheable {
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
    if cacheable {
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
        if safe_text.trim().is_empty()
            && (message.kind() != AgentTimelineMessageKind::Assistant
                || message.images().is_empty())
        {
            // Non-rendering Markdown must not leave a phantom text row, but
            // assistant image attachments still need a card of their own.
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
            if text.trim().is_empty() && display_message.images().is_empty() {
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

/// Derived exclusively from displayed rows; tool/reasoning rows never end a turn.
pub(super) fn question_targets<M: AgentTimelineMessage>(
    rows: &[TimelineLayoutRow<M>],
    messages: &[M],
) -> TimelineQuestionTargets {
    let mut targets: Vec<TimelineQuestionTarget> = Vec::new();
    for row in rows {
        let Some(message) = row
            .display_message
            .as_ref()
            .or_else(|| messages.get(row.source_index))
        else {
            continue;
        };
        match message.kind() {
            AgentTimelineMessageKind::User => targets.push(TimelineQuestionTarget {
                source_index: row.source_index,
                message_id: message.id().to_owned(),
                top: row.top,
                assistant_source_index: None,
            }),
            AgentTimelineMessageKind::Assistant => {
                if let Some(target) = targets.last_mut() {
                    target.assistant_source_index = Some(row.source_index);
                }
            }
            _ => {}
        }
    }
    Rc::new(targets)
}

#[derive(Debug)]
pub struct TimelineQuestionRailFrame {
    pub track: [f32; 4],
    pub tick_count: usize,
    pub hovered: Option<usize>,
    pub active: Option<usize>,
    pub preview: Option<QuestionPreview>,
    scale: f32,
}

#[derive(Debug)]
pub struct QuestionPreview {
    pub rect: [f32; 4],
    pub user: String,
    pub assistant: Vec<String>,
}

impl TimelineQuestionRailFrame {
    pub fn hovered_tick(&self, mouse: Option<(f32, f32)>) -> Option<usize> {
        question_index_at(self.track, self.tick_count, mouse)
    }

    pub fn tick_rect(&self, index: usize, hovered: Option<usize>) -> [f32; 4] {
        let ratio = match hovered.map(|h| h.abs_diff(index)) {
            Some(0) => 1.0,
            Some(1) => 2.0 / 3.0,
            Some(2) => 5.0 / 12.0,
            _ => 1.0 / 3.0,
        };
        let pad = self.scale.min(self.track[2] * 0.125);
        [
            self.track[0] + pad,
            self.track[1] + index as f32 * self.track[3] / (self.tick_count - 1) as f32
                - self.scale,
            (self.track[2] - 2.0 * pad) * ratio,
            2.0 * self.scale,
        ]
    }
}

/// The rect is the existing right gutter, never the transcript column.
pub(super) fn timeline_scroll_rail_geometry(
    [x, y, w, h]: [f32; 4],
    s: f32,
    count: usize,
) -> Option<TimelineQuestionRailFrame> {
    if count < 2
        || ![x, y, w, h, s].iter().all(|v| v.is_finite())
        || s <= 0.0
        || w < 6.0 * s
        || h < 4.0 * s
    {
        return None;
    }
    let rail_h = ((count - 1) as f32 * 8.0 * s).min((h - 4.0 * s).max(0.0));
    let width = w.min(26.0 * s);
    Some(TimelineQuestionRailFrame {
        track: [
            x + ((w - width) * 0.5).min(8.0 * s),
            y + (h - rail_h) * 0.5,
            width,
            rail_h,
        ],
        tick_count: count,
        hovered: None,
        active: None,
        preview: None,
        scale: s,
    })
}

pub(super) fn compact_question_preview(source: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, TagEnd};
    // Bound before either HTML safety projection or Markdown parsing.
    let bounded: String = source.chars().take(1500).collect();
    let safe = super::super::markdown::safe_canvas_markdown(&bounded);
    let mut plain = String::new();
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(&safe, options) {
        match event {
            Event::Text(text) | Event::Code(text) => plain.push_str(&text),
            Event::SoftBreak | Event::HardBreak | Event::Rule => plain.push(' '),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::TableCell
                | TagEnd::TableRow,
            ) => plain.push(' '),
            _ => {}
        }
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn question_preview_rect(
    pane: [f32; 4],
    track: [f32; 4],
    index: usize,
    count: usize,
    height: f32,
    s: f32,
) -> Option<[f32; 4]> {
    let right = (track[0] - 6.0 * s).min(pane[0] + pane[2]);
    let width = (right - pane[0] - 4.0 * s).min(340.0 * s);
    let height = height.min(pane[3]);
    if width < 40.0 * s || height < 30.0 * s {
        return None;
    }
    let tick_y = track[1] + index as f32 * track[3] / (count - 1) as f32;
    let top = (tick_y - height * 0.5).clamp(pane[1], pane[1] + pane[3] - height);
    Some([right - width, top, width, height])
}

/// Width-measured compact lines, shared by the one-line question and answer.
pub(super) fn question_preview_lines(
    text: &str,
    width: f32,
    max_lines: usize,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut remaining = text;
    for line in 0..max_lines {
        if remaining.is_empty() {
            break;
        }
        if measure(remaining) <= width {
            lines.push(remaining.to_owned());
            break;
        }
        let last = line + 1 == max_lines;
        let mut end = 0;
        for (i, ch) in remaining.char_indices() {
            let next = i + ch.len_utf8();
            let candidate = if last {
                format!("{}…", &remaining[..next])
            } else {
                remaining[..next].to_owned()
            };
            if measure(&candidate) > width {
                break;
            }
            end = next;
        }
        if last {
            lines.push(format!("{}…", remaining[..end].trim_end()));
            break;
        }
        if end == 0 {
            break;
        }
        if let Some(space) = remaining[..end].rfind(' ').filter(|space| *space > 0) {
            end = space;
        }
        lines.push(remaining[..end].trim().to_owned());
        remaining = remaining[end..].trim_start();
    }
    lines
}

pub(super) fn question_user_preview(text: &str, has_images: bool) -> String {
    let text = compact_question_preview(text);
    if text.is_empty() {
        let fallback = if has_images {
            "Image attachment"
        } else {
            "User question"
        };
        fallback.to_owned()
    } else {
        text
    }
}

fn build_question_preview<M: AgentTimelineMessage>(
    sugarloaf: &mut Sugarloaf,
    target: &TimelineQuestionTarget,
    messages: &[M],
    pane_rect: [f32; 4],
    frame: &TimelineQuestionRailFrame,
    index: usize,
    s: f32,
) -> Option<QuestionPreview> {
    let user = messages.get(target.source_index)?;
    let text = question_user_preview(user.text(), !user.images().is_empty());
    let answer = target
        .assistant_source_index
        .and_then(|i| messages.get(i))
        .map(|m| compact_question_preview(m.text()))
        .unwrap_or_default();
    let opts = DrawOpts {
        font_size: 12.0 * s,
        ..DrawOpts::default()
    };
    let provisional =
        question_preview_rect(pane_rect, frame.track, index, frame.tick_count, 84.0 * s, s)?;
    let width = provisional[2] - 20.0 * s;
    let mut measure = |text: &str| sugarloaf.overlay_text_mut().measure(text, &opts);
    let user = question_preview_lines(&text, width, 1, &mut measure)
        .pop()
        .unwrap_or_default();
    let available_lines = ((pane_rect[3] - 36.0 * s) / (16.0 * s)).floor().max(0.0) as usize;
    let assistant = question_preview_lines(&answer, width, available_lines.min(3), &mut measure);
    let height = 32.0 * s
        + if assistant.is_empty() {
            0.0
        } else {
            4.0 * s + 16.0 * s * assistant.len() as f32
        };
    let rect = question_preview_rect(pane_rect, frame.track, index, frame.tick_count, height, s)?;
    Some(QuestionPreview {
        rect,
        user,
        assistant,
    })
}

/// Paint the exact snapshot whose preview was masked before transcript glyphs.
pub fn render_timeline_scrollbar_with<P: AgentTimelinePane>(
    sugarloaf: &mut Sugarloaf,
    pane: &mut P,
    theme: &IdeTheme,
    s: f32,
    frame: Option<&TimelineQuestionRailFrame>,
) {
    let Some(frame) = frame else {
        pane.set_scrollbar_geometry(None, None);
        return;
    };
    pane.set_scrollbar_geometry(Some(frame.track), None);
    for index in 0..frame.tick_count {
        let [x, y, w, h] = frame.tick_rect(index, frame.hovered);
        let color = if frame.active == Some(index) {
            theme.f32_alpha(theme.fg, 0.9)
        } else {
            theme.f32_alpha(
                theme.muted,
                if frame.hovered == Some(index) {
                    0.75
                } else {
                    0.35
                },
            )
        };
        sugarloaf.rect(None, x, y, w, h, color, DEPTH, ORDER_CARET);
    }
    if let Some(preview) = &frame.preview {
        let [x, y, w, h] = preview.rect;
        // Both surface and text use the dedicated overlay layer, after transcript
        // occlusion holes have been restored by the timeline/chat renderers.
        sugarloaf.overlay_rounded_rect(x, y, w, h, theme.f32(theme.border), DEPTH, 6.0 * s, 185);
        sugarloaf.overlay_rounded_rect(
            x + s,
            y + s,
            w - 2.0 * s,
            h - 2.0 * s,
            theme.f32(theme.surface),
            DEPTH,
            5.0 * s,
            186,
        );
        let mut opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.fg),
            ..DrawOpts::default()
        };
        sugarloaf
            .overlay_text_mut()
            .draw(x + 10.0 * s, y + 8.0 * s, &preview.user, &opts);
        opts.color = theme.u8(theme.muted);
        for (i, line) in preview.assistant.iter().enumerate() {
            sugarloaf.overlay_text_mut().draw(
                x + 10.0 * s,
                y + 28.0 * s + i as f32 * 16.0 * s,
                line,
                &opts,
            );
        }
    }
}
