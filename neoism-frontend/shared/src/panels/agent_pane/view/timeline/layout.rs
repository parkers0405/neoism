use super::read_group::read_tool_group_at;
use super::render::{
    cached_message_height, display_timeline_message, f32_measure_bucket,
    is_edit_tool_message, is_subagent_runtime_notification,
};
use super::*;

// Both hosts now own the same cache representation: no row/page walks or
// replacement vector allocations are needed at the state/view boundary.
pub(crate) fn from_state_cache(
    cache: crate::panels::agent_pane::state::TimelineLayoutCache,
) -> TimelineLayoutCache<NeoismAgentMessage> {
    cache
}

pub(crate) fn into_state_cache(
    cache: TimelineLayoutCache<NeoismAgentMessage>,
) -> crate::panels::agent_pane::state::TimelineLayoutCache {
    cache
}

pub(crate) fn visible_timeline_row_range<M>(
    rows: &[TimelineLayoutRow<M>],
    visible_top: f32,
    visible_bottom: f32,
) -> Range<usize> {
    if rows.is_empty() || visible_bottom < visible_top {
        return 0..0;
    }

    // Rows are laid out in ascending `top` order. Find the first row whose
    // bottom edge can intersect the registration band, then stop once row tops
    // are below it. This keeps ordinary scroll frames proportional to visible
    // cards instead of total history length while preserving the existing
    // offscreen registration margin used for hit-testing/selection.
    let start = rows.partition_point(|row| row.top + row.height < visible_top);
    let end = start + rows[start..].partition_point(|row| row.top <= visible_bottom);
    start..end
}

pub(crate) fn timeline_row_range_for_source_range<M>(
    rows: &[TimelineLayoutRow<M>],
    source_start: usize,
    source_end: usize,
) -> Range<usize> {
    if rows.is_empty() || source_end < source_start {
        return 0..0;
    }
    let start = rows.partition_point(|row| row.source_end_index < source_start);
    let end = start + rows[start..].partition_point(|row| row.source_index <= source_end);
    start..end
}

pub(crate) fn timeline_row_range_intersects_viewport<M>(
    rows: &[TimelineLayoutRow<M>],
    range: Range<usize>,
    visible_top: f32,
    visible_bottom: f32,
) -> bool {
    if range.is_empty() || visible_bottom < visible_top {
        return false;
    }
    rows[range].iter().any(|row| {
        let bottom = row.top + row.height;
        bottom >= visible_top && row.top <= visible_bottom
    })
}

pub(crate) fn union_timeline_row_ranges(
    a: Range<usize>,
    b: Range<usize>,
) -> Range<usize> {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    a.start.min(b.start)..a.end.max(b.end)
}

pub(crate) fn timeline_virtual_row_measurements<M>(
    rows: &[TimelineLayoutRow<M>],
    gap: f32,
) -> Vec<TimelineVirtualRowMeasurement> {
    rows.iter()
        .enumerate()
        .map(|(ix, row)| {
            let trailing_gap = if ix + 1 < rows.len() { gap } else { 0.0 };
            TimelineVirtualRowMeasurement {
                source_index: row.source_index,
                source_end_index: row.source_end_index,
                height: (row.height + trailing_gap).max(0.0),
                visual_line_count: (row.height / 20.0).ceil().max(1.0) as u32,
            }
        })
        .collect()
}

pub(crate) fn timeline_layout<P, D>(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut P,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    gap: f32,
    viewport_h: f32,
    paint_h: f32,
) -> (TimelineLayoutCache<P::Message>, bool, f32)
where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    warm_staged_history::<P, D>(sugarloaf, pane, width, theme, s);
    let width_bucket = f32_measure_bucket(width);
    let scale_bucket = f32_measure_bucket(s);
    let gap_bucket = f32_measure_bucket(gap);
    let epoch = pane.timeline_layout_epoch();
    let lazy = pane.timeline_lazy_measurement();
    let mut dirty = pane.take_timeline_dirty_marks();
    if pane.any_tool_expand_animating() {
        mark_animating_tool_rows_dirty(pane, &mut dirty);
    }
    let source_len = pane.messages().len();
    // History pagination: messages were prepended at the front. Lay out only
    // the new prefix and shift the existing rows, instead of rebuilding the
    // whole transcript. This keeps each "load older" proportional to what was
    // added rather than to total loaded history — without it, every page load
    // re-measured and re-keyed every row already on screen, so pagination got
    // progressively slower with each page.
    // Measure the new prefix exactly even in lazy mode. Re-estimating a page
    // changes the prefix height under the reader and makes pagination jump.
    if let Some(delta) = pane.take_timeline_prepend() {
        // Always pull the cache out: after a prepend every existing row's
        // source index has shifted, so the normal append-patch path below
        // would mis-target. Either we fold incrementally here, or we drop the
        // cache and fall through to a clean full rebuild.
        //
        // Crucially this runs even when there are dirty marks: paginating
        // while the agent streams (the common case) always has a dirty tail,
        // and gating the fold on "no dirty" forced a full O(total) rebuild on
        // every page — the slowdown that compounded with each load. Instead we
        // fold the prepend, then patch the (index-shifted) dirty tail.
        let reusable = pane.take_timeline_layout_cache().filter(|cache| {
            delta > 0
                && cache.epoch == epoch
                && cache.width_bucket == width_bucket
                && cache.scale_bucket == scale_bucket
                && cache.gap_bucket == gap_bucket
                && cache.source_len + delta <= source_len
                && !cache.rows.is_empty()
                // A lazy cache may contain estimated tool rows with no diff
                // preparation. The prepend fold reuses its suffix and marks the
                // result exact, so only fold when every reused row is measured.
                && prepend_cache_is_exact(cache)
        });
        if let Some(mut cache) = reusable {
            let previous_content_height = cache.content_height;
            if prepend_timeline_layout::<P, D>(
                sugarloaf, pane, &mut cache, delta, width, theme, s, gap,
            ) {
                let prepend_height =
                    (cache.content_height - previous_content_height).max(0.0);
                let appended_live_tail = source_len.saturating_sub(cache.source_len);
                // The prepend shifted every existing row by `delta`; shift the
                // pending dirty indices to match, then re-lay just that region.
                if appended_live_tail > 0
                    || !dirty.ids.is_empty()
                    || !dirty.indices.is_empty()
                {
                    let mut shifted = TimelineDirtyMarks {
                        ids: dirty.ids,
                        indices: dirty
                            .indices
                            .iter()
                            .map(|index| index + delta)
                            .collect(),
                    };
                    if appended_live_tail > 0 {
                        shifted.indices.insert(cache.source_len.saturating_sub(1));
                    }
                    patch_timeline_layout::<P, D>(
                        sugarloaf, pane, &mut cache, shifted, width, theme, s, gap,
                    );
                }
                pane.set_measured_timeline_prepend(cache.content_height, prepend_height);
                return (cache, true, 0.0);
            }
        }
    }
    let offset = pane.timeline_scroll_offset();
    let cache = pane.take_timeline_layout_cache().filter(|cache| {
        reusable_timeline_cache(
            cache,
            epoch,
            width_bucket,
            scale_bucket,
            gap_bucket,
            source_len,
        )
    });
    let mut live_baseline = None;
    if let Some(mut cache) = cache {
        reconcile_artifact_rows(
            &mut cache,
            pane.messages(),
            pane.timeline_source_revision(),
            &mut dirty,
        );
        live_baseline = dirty_start_source(pane, &cache, &dirty)
            .and_then(|source| measured_tail_baseline(&cache, source));
        let covers_viewport =
            !lazy || lazy_cache_covers_viewport(&cache, offset, viewport_h, paint_h);
        let needs_patch = cache.source_len != source_len
            || !dirty.ids.is_empty()
            || !dirty.indices.is_empty();
        if covers_viewport
            && (!needs_patch
                || patch_timeline_layout::<P, D>(
                    sugarloaf, pane, &mut cache, dirty, width, theme, s, gap,
                ))
        {
            let growth = measured_tail_growth(&cache, live_baseline);
            return (cache, needs_patch, growth);
        }
    }

    let layout = build_timeline_layout::<P, D>(
        sugarloaf,
        pane,
        width,
        theme,
        s,
        gap,
        epoch,
        width_bucket,
        scale_bucket,
        gap_bucket,
        lazy,
        viewport_h,
        paint_h,
        offset,
    );
    let growth = measured_tail_growth(&layout, live_baseline);
    (layout, true, growth)
}

/// Per-frame budget for pre-measuring a staged history page. At least one
/// message is warmed per frame so a page always finishes.
const HISTORY_WARM_BUDGET_US: u128 = 2_000;

/// Warm the measure + markdown caches for a fetched-but-not-yet-prepended
/// history page, using the same width/scale/theme the prepend fold will use.
fn warm_staged_history<P, D>(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut P,
    width: f32,
    theme: &IdeTheme,
    s: f32,
) where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    let started = web_time::Instant::now();
    while let Some(message) = pane.next_timeline_warm_message() {
        if let Some(display) = display_timeline_message(&message, false) {
            cached_message_height::<P, D>(sugarloaf, pane, &display, width, theme, s);
            let _ = prepared_message_markdown_blocks(sugarloaf, pane, &display, width, theme, s);
        }
        if started.elapsed().as_micros() >= HISTORY_WARM_BUDGET_US {
            break;
        }
    }
}

pub(super) fn reusable_timeline_cache<M>(
    cache: &TimelineLayoutCache<M>,
    epoch: u64,
    width_bucket: i32,
    scale_bucket: i32,
    gap_bucket: i32,
    source_len: usize,
) -> bool {
    // Nonempty source history can legitimately have no visible rows (blank
    // placeholders or filtered tools). Keep its exact empty growth baseline.
    cache.epoch == epoch
        && cache.width_bucket == width_bucket
        && cache.scale_bucket == scale_bucket
        && cache.gap_bucket == gap_bucket
        && cache.source_len <= source_len
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MeasuredTailBaseline {
    Row { source: usize, height: f32 },
    Empty { source: usize },
}

// Compare only an already-measured dirty suffix. Refining older estimates must
// not become spring lag just because a live update arrived in the same frame.
pub(super) fn measured_tail_baseline<M>(
    cache: &TimelineLayoutCache<M>,
    source: usize,
) -> Option<MeasuredTailBaseline> {
    if cache.estimated_suffix_start != cache.rows.len() {
        return None;
    }
    let index = cache
        .rows
        .partition_point(|row| row.source_end_index < source);
    // Layout repair may begin in an estimated prefix while the already-measured
    // live tail grows. Its first exact row is still a valid growth boundary.
    let index = index.max(cache.estimated_prefix_rows);
    match cache.rows.get(index) {
        Some(row) => Some(MeasuredTailBaseline::Row {
            source: row.source_index,
            height: cache.content_height - row.top,
        }),
        None => Some(MeasuredTailBaseline::Empty { source }),
    }
}

pub(super) fn measured_tail_growth<M>(
    cache: &TimelineLayoutCache<M>,
    baseline: Option<MeasuredTailBaseline>,
) -> f32 {
    match baseline {
        Some(MeasuredTailBaseline::Row { source, height }) => {
            match measured_tail_baseline(cache, source) {
                Some(MeasuredTailBaseline::Row {
                    source: new_source,
                    height: new_height,
                }) if source == new_source => (new_height - height).max(0.0),
                _ => 0.0,
            }
        }
        Some(MeasuredTailBaseline::Empty { source })
            if cache.estimated_suffix_start == cache.rows.len() =>
        {
            let index = cache
                .rows
                .partition_point(|row| row.source_end_index < source);
            let Some(row) = cache.rows.get(index) else {
                return 0.0;
            };
            if index < cache.estimated_prefix_rows || row.source_index < source {
                return 0.0;
            }
            // A first visible append can follow hidden/zero-height sources.
            // Charge its leading gap too, but not any earlier remeasurement.
            let top = index.checked_sub(1).map_or(0.0, |previous| {
                let previous = &cache.rows[previous];
                previous.top + previous.height
            });
            (cache.content_height - top).max(0.0)
        }
        _ => 0.0,
    }
}

/// Work counters make the unchanged-frame contract testable without timing noise.
#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct ArtifactReconcileWork {
    pub rows_visited: usize,
    /// Upper bound on bytes offered to candidate detection, not elapsed time.
    pub candidate_body_bytes: usize,
    pub candidates: usize,
}

pub(super) fn reconcile_artifact_rows<M: AgentTimelineMessage>(
    cache: &mut TimelineLayoutCache<M>,
    messages: &[M],
    revision: Option<u64>,
    dirty: &mut TimelineDirtyMarks,
) -> ArtifactReconcileWork {
    // Appends have their own length guard. Dirty marks still drive normal
    // suffix repair below; they need not force body scans (tool animation and
    // learned artifact metrics can dirty geometry without changing source).
    // Unknown revisions retain the defensive fallback.
    if revision.is_some()
        && cache.artifact_revision == revision
        && cache.source_len == messages.len()
    {
        return ArtifactReconcileWork::default();
    }
    let work = mark_changed_artifact_rows(cache, messages, dirty);
    cache.artifact_revision = revision;
    work
}

/// Reconcile artifact source transitions on source changes, even if an ingest
/// path advanced the revision without marking the owning row dirty. Repatching
/// also rebuilds following row tops, pages and virtual measurements.
pub(super) fn mark_changed_artifact_rows<M: AgentTimelineMessage>(
    cache: &TimelineLayoutCache<M>,
    messages: &[M],
    dirty: &mut TimelineDirtyMarks,
) -> ArtifactReconcileWork {
    let mut work = ArtifactReconcileWork::default();
    for (index, row) in cache.rows.iter().enumerate() {
        work.rows_visited += 1;
        if row.source_index != row.source_end_index {
            continue;
        }
        let Some(current) = messages.get(row.source_index) else {
            continue;
        };
        let Some(old) = row.display_message.as_ref() else {
            continue;
        };
        // Detect candidates before full-body equality. Do not narrow by kind:
        // defensive repair also covers legacy/non-assistant artifact sources.
        work.candidate_body_bytes += current.text().len() + old.text().len();
        if !current.text().contains("neoism-html") && !old.text().contains("neoism-html")
        {
            continue;
        }
        work.candidates += 1;
        if current.text() == old.text() && current.status() == old.status() {
            continue;
        }
        let previous_edit = index > 0 && cache.rows[index - 1].is_edit_tool;
        let changed =
            display_timeline_message(current, previous_edit).is_none_or(|display| {
                display.text() != old.text() || display.status() != old.status()
            });
        if changed {
            dirty.indices.insert(row.source_index);
        }
    }
    work
}

pub(super) fn prepend_cache_is_exact<M>(cache: &TimelineLayoutCache<M>) -> bool {
    cache.estimated_prefix_rows == 0 && cache.estimated_suffix_start == cache.rows.len()
}

/// Whether the exact-measured window of a lazy cache still comfortably covers
/// the viewport (kept at least one viewport inside both estimated edges). When
/// it stops covering, the caller rebuilds with a window centered on the new
/// scroll position. A fully-exact cache always covers.
pub(super) fn lazy_cache_covers_viewport<M>(
    cache: &TimelineLayoutCache<M>,
    offset: f32,
    viewport_h: f32,
    paint_h: f32,
) -> bool {
    let exact_start = cache.estimated_prefix_rows;
    let exact_end = cache.estimated_suffix_start.min(cache.rows.len());
    if exact_start == 0 && exact_end >= cache.rows.len() {
        return true;
    }
    let max_scroll = (cache.content_height - viewport_h).max(0.0);
    let scroll_top = (max_scroll - offset).clamp(0.0, max_scroll);
    let viewport_bottom = scroll_top + paint_h.max(viewport_h);
    if exact_start > 0 {
        let Some(first_exact) = cache.rows.get(exact_start) else {
            return true;
        };
        if scroll_top - viewport_h <= first_exact.top {
            return false;
        }
    }
    if exact_end < cache.rows.len() {
        let Some(last_exact) = cache.rows.get(exact_end.saturating_sub(1)) else {
            return true;
        };
        if viewport_bottom + viewport_h >= last_exact.top + last_exact.height {
            return false;
        }
    }
    true
}

#[cfg(test)]
pub(super) fn lazy_cache_covers_viewport_for_test<M>(
    cache: &TimelineLayoutCache<M>,
    offset: f32,
    viewport_h: f32,
) -> bool {
    lazy_cache_covers_viewport(cache, offset, viewport_h, viewport_h)
}

pub(super) fn mark_animating_tool_rows_dirty<P: AgentTimelinePane>(
    pane: &P,
    dirty: &mut TimelineDirtyMarks,
) {
    for (index, message) in pane.messages().iter().enumerate() {
        if pane.tool_expand_animating(message.id()) {
            dirty.indices.insert(index);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_timeline_layout<P, D>(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut P,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    gap: f32,
    epoch: u64,
    width_bucket: i32,
    scale_bucket: i32,
    gap_bucket: i32,
    lazy: bool,
    viewport_h: f32,
    paint_h: f32,
    offset: f32,
) -> TimelineLayoutCache<P::Message>
where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    let mut rows: Vec<TimelineLayoutRow<P::Message>> = Vec::new();
    let (content_height, estimated_prefix_rows, estimated_suffix_start) = if lazy {
        build_lazy_rows::<P, D>(
            sugarloaf, pane, width, theme, s, gap, viewport_h, paint_h, offset, &mut rows,
        )
    } else {
        let height = append_timeline_rows::<P, D>(
            sugarloaf, pane, width, theme, s, gap, 0, None, 0.0, false, &mut rows,
        );
        let suffix = rows.len();
        (height, 0, suffix)
    };
    let pages = build_timeline_layout_pages(&rows, pane.messages().len());
    TimelineLayoutCache {
        question_targets: Default::default(),
        epoch,
        artifact_revision: pane.timeline_source_revision(),
        source_len: pane.messages().len(),
        width_bucket,
        scale_bucket,
        gap_bucket,
        content_height,
        pages,
        rows,
        estimated_prefix_rows,
        estimated_suffix_start,
    }
}

/// Viewport-only (lazy) layout. Cheaply estimates every row to locate the
/// window, keeps off-screen prefix *and* suffix estimated, and rebuilds only
/// the on-screen band (plus overscan) EXACTLY via `append_timeline_rows`.
/// Returns `(content_height, estimated_prefix_rows, estimated_suffix_start)`.
#[allow(clippy::too_many_arguments)]
fn build_lazy_rows<P, D>(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut P,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    gap: f32,
    viewport_h: f32,
    paint_h: f32,
    offset: f32,
    rows: &mut Vec<TimelineLayoutRow<P::Message>>,
) -> (f32, usize, usize)
where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    // Pass 1: estimate all rows (no sugarloaf measurement) to place the window.
    append_estimated_rows::<P>(pane, width, s, gap, rows);
    let est_content_h = rows.last().map(|row| row.top + row.height).unwrap_or(0.0);
    if rows.is_empty() {
        return (0.0, 0, 0);
    }
    // Exact region = viewport plus ~2× overscan on both sides. Estimates lean
    // low (rich content under-counts), which only ever grows the exact region.
    let overscan = (viewport_h * 2.0).max(600.0 * s);
    let max_scroll = (est_content_h - viewport_h).max(0.0);
    let anchor = (!pane.timeline_follow_bottom())
        .then(|| pane.timeline_view_anchor())
        .flatten()
        .and_then(|(key, screen_offset)| {
            resolve_timeline_view_anchor(pane.messages(), &key)
                .map(|source| (source, screen_offset))
        });
    let (scroll_top, anchor_row) = lazy_window_position(rows, max_scroll, offset, anchor);
    let window_top = (scroll_top - overscan).max(0.0);
    let window_bottom = scroll_top + paint_h.max(viewport_h) + overscan;
    let mut exact_start_row =
        rows.partition_point(|row| row.top + row.height < window_top);
    let mut exact_end_row = exact_start_row
        + rows[exact_start_row..].partition_point(|row| row.top <= window_bottom);
    if let Some(index) = anchor_row {
        // A reader may be deep inside a tall card whose estimate is only a
        // header. Keep that card exact even if its intra-row offset exceeds it.
        exact_start_row = exact_start_row.min(index);
        exact_end_row = exact_end_row.max(index + 1);
    }
    if exact_start_row == 0 && exact_end_row >= rows.len() {
        rows.clear();
        let height = append_timeline_rows::<P, D>(
            sugarloaf, pane, width, theme, s, gap, 0, None, 0.0, false, rows,
        );
        let suffix = rows.len();
        return (height, 0, suffix);
    }
    let estimated_suffix = if exact_end_row < rows.len() {
        rows.split_off(exact_end_row)
    } else {
        Vec::new()
    };
    let end_source = rows
        .last()
        .map(|row| row.source_end_index.saturating_add(1));
    let (start_source, content_y, previous_visible_was_edit_tool) =
        if exact_start_row == 0 {
            (0, 0.0, false)
        } else {
            let prev = &rows[exact_start_row - 1];
            (
                prev.source_end_index.saturating_add(1),
                prev.top + prev.height,
                prev.is_edit_tool,
            )
        };
    let suffix_top = estimated_suffix.first().map(|row| row.top);
    rows.truncate(exact_start_row);
    append_timeline_rows::<P, D>(
        sugarloaf,
        pane,
        width,
        theme,
        s,
        gap,
        start_source,
        end_source,
        content_y,
        previous_visible_was_edit_tool,
        rows,
    );
    let exact_end = rows.len();
    if let Some(old_suffix_top) = suffix_top {
        let new_suffix_top = rows
            .last()
            .map(|row| row.top + row.height + gap)
            .unwrap_or(0.0);
        let shift = new_suffix_top - old_suffix_top;
        for mut row in estimated_suffix {
            row.top += shift;
            rows.push(row);
        }
    }
    let content_height = rows.last().map(|row| row.top + row.height).unwrap_or(0.0);
    (content_height, exact_start_row, exact_end)
}

// Bottom-distance from an exact cache cannot locate a window in fresh estimates:
// the unmeasured suffix may differ by thousands of pixels. Use the reader's
// logical row instead, then restore its viewport-relative position after layout.
pub(super) fn lazy_window_position<M>(
    rows: &[TimelineLayoutRow<M>],
    max_scroll: f32,
    offset: f32,
    anchor: Option<(usize, f32)>,
) -> (f32, Option<usize>) {
    if let Some((source, screen_offset)) = anchor {
        let index = rows.partition_point(|row| row.source_end_index < source);
        if let Some(row) = rows.get(index).filter(|row| row.source_index <= source) {
            return ((row.top - screen_offset).max(0.0), Some(index));
        }
    }
    ((max_scroll - offset).clamp(0.0, max_scroll), None)
}

/// Pass-1 estimate loop for lazy layout: mirrors `append_timeline_rows`'
/// grouping exactly but assigns each row a cheap estimated height (no sugarloaf
/// measurement, no markdown/diff prep). Only the retained prefix's rows keep
/// these estimates; the suffix is re-measured exactly by the caller.
fn append_estimated_rows<P>(
    pane: &P,
    width: f32,
    s: f32,
    gap: f32,
    rows: &mut Vec<TimelineLayoutRow<P::Message>>,
) where
    P: AgentTimelinePane,
{
    let mut content_y = 0.0f32;
    let mut previous_visible_was_edit_tool = false;
    let mut appended_any = false;
    let source_len = pane.messages().len();
    let mut source_index = 0;
    while source_index < source_len {
        let Some(item) =
            next_timeline_item::<P>(pane, source_index, previous_visible_was_edit_tool)
        else {
            source_index += 1;
            continue;
        };
        match item {
            NextTimelineItem::Group {
                source_end_exclusive,
                message: group_message,
            } => {
                let height =
                    estimate_message_height_with_pane(pane, &group_message, width, s);
                if height > 0.0 {
                    if appended_any || !rows.is_empty() {
                        content_y += gap;
                    }
                    rows.push(TimelineLayoutRow {
                        source_index,
                        source_end_index: source_end_exclusive.saturating_sub(1),
                        top: content_y,
                        height,
                        display_text: None,
                        display_message: Some(group_message),
                        markdown_blocks: None,
                        tool_diff_sections: None,
                        is_edit_tool: false,
                    });
                    content_y += height;
                    appended_any = true;
                }
                previous_visible_was_edit_tool = false;
                source_index = source_end_exclusive;
            }
            NextTimelineItem::Message { message } => {
                let height = estimate_message_height_with_pane(pane, &message, width, s);
                if height <= 0.0 {
                    source_index += 1;
                    continue;
                }
                if appended_any || !rows.is_empty() {
                    content_y += gap;
                }
                let is_edit_tool = is_edit_tool_message(&message);
                rows.push(TimelineLayoutRow {
                    source_index,
                    source_end_index: source_index,
                    top: content_y,
                    height,
                    display_text: None,
                    display_message: Some(message),
                    markdown_blocks: None,
                    tool_diff_sections: None,
                    is_edit_tool,
                });
                previous_visible_was_edit_tool = is_edit_tool;
                content_y += height;
                appended_any = true;
                source_index += 1;
            }
        }
    }
}

// Headless regression seam: run the production lazy row builder, including
// visibility, grouping, display projection, height estimates and row packing.
// This does not stand in for GPU-backed exact text measurement.
#[cfg(test)]
pub(super) fn estimated_timeline_rows_for_test(
    pane: &NeoismAgentPane,
    width: f32,
    s: f32,
    gap: f32,
) -> Vec<TimelineLayoutRow<NeoismAgentMessage>> {
    let mut rows = Vec::new();
    append_estimated_rows(pane, width, s, gap, &mut rows);
    rows
}

/// Previously laid-out artifacts retain their identity and learned footprint even
/// when lazy rebuilding moves their row outside the exact measurement window.
pub(super) fn estimate_message_height_with_pane<
    P: AgentMarkdownPane,
    M: AgentTimelineMessage,
>(
    pane: &P,
    message: &M,
    width: f32,
    s: f32,
) -> f32 {
    let text = message.text();
    if text.contains("neoism-html")
        && matches!(
            message.kind(),
            AgentTimelineMessageKind::Assistant
                | AgentTimelineMessageKind::Reasoning
                | AgentTimelineMessageKind::Compaction
        )
    {
        let body_width = (width - 48.0 * s).max(80.0 * s);
        if let Some(blocks) = pane.cached_markdown_blocks_for(text, body_width, s) {
            let body = super::super::markdown::measure_markdown_blocks_namespaced(
                &blocks,
                body_width + 30.0 * s,
                pane,
                s,
                message.id(),
            );
            let extra = match message.kind() {
                AgentTimelineMessageKind::Assistant => {
                    let image = if message.images().is_empty() {
                        0.0
                    } else {
                        164.0 * s
                    };
                    image
                        + if message.status().trim().is_empty() {
                            0.0
                        } else {
                            super::super::assistant::ASSISTANT_RESPONSE_FOOTER_H * s
                        }
                }
                _ => 42.0 * s,
            };
            return body + extra;
        }
    }
    estimate_message_height(message, width, s)
}

/// Cheap off-screen height estimate. Deliberately conservative (under-counts
/// rich content like code/diffs/images) so the exact region only ever grows —
/// visible rows are always exactly measured; this feeds the scrollbar only.
pub(super) fn estimate_message_height<M>(message: &M, width: f32, s: f32) -> f32
where
    M: AgentTimelineMessage,
{
    if message.kind() == AgentTimelineMessageKind::Tool {
        return super::super::tool_message::TOOL_HEADER_HEIGHT * s;
    }
    let text = message.text();
    if message.kind() == AgentTimelineMessageKind::User {
        // `render_user_message` and exact card measurement deliberately cap
        // prompts at six lines. The old lazy estimate counted the complete
        // underlying paste, so an estimated user row could be a viewport tall
        // while its bubble drew only six lines, producing a giant empty area.
        let chars_per_line = ((width / (7.0 * s)) as usize).max(24);
        let lines = text
            .split('\n')
            .map(|line| line.chars().count().div_ceil(chars_per_line).max(1))
            .sum::<usize>()
            .clamp(1, USER_MESSAGE_MAX_LINES);
        let image_h = if message.images().is_empty() {
            0.0
        } else {
            164.0 * s
        };
        return 24.0 * s + image_h + lines as f32 * 19.0 * s;
    }
    let image_h = if message.kind() == AgentTimelineMessageKind::Assistant
        && !message.images().is_empty()
    {
        164.0 * s
    } else {
        0.0
    };
    let base = 34.0 * s;
    if text.trim().is_empty() {
        return image_h;
    }
    let chars_per_line = ((width / (7.0 * s)) as usize).max(24);
    base + super::super::markdown::estimated_artifact_body_height(text, chars_per_line, s)
        + image_h
}

pub(super) fn patch_start_row<M>(
    cache: &TimelineLayoutCache<M>,
    start_source: usize,
) -> Option<usize> {
    let start_pos = cache
        .rows
        .iter()
        .position(|row| {
            row.source_index >= start_source || row.source_end_index >= start_source
        })
        .unwrap_or(cache.rows.len());
    // Patching beyond an estimated suffix would strand estimated rows between
    // two exact regions, which this cache cannot represent. Rebuild lazily.
    (start_pos <= cache.estimated_suffix_start).then_some(start_pos)
}

#[allow(clippy::too_many_arguments)]
fn patch_timeline_layout<P, D>(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut P,
    cache: &mut TimelineLayoutCache<P::Message>,
    dirty: TimelineDirtyMarks,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    gap: f32,
) -> bool
where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    let source_len = pane.messages().len();
    let Some(start_source) = dirty_start_source(pane, cache, &dirty) else {
        return cache.source_len == source_len;
    };
    let Some(start_pos) = patch_start_row(cache, start_source) else {
        return false;
    };
    let (scan_start, content_y, previous_visible_was_edit_tool) = if let Some(previous) =
        start_pos
            .checked_sub(1)
            .and_then(|index| cache.rows.get(index))
    {
        (
            previous.source_end_index.saturating_add(1),
            previous.top + previous.height,
            previous.is_edit_tool,
        )
    } else {
        (0, 0.0, false)
    };
    cache.rows.truncate(start_pos);
    // Rows from here down are re-measured exactly below, so no estimated row
    // survives at or past the patch start.
    cache.estimated_prefix_rows = cache.estimated_prefix_rows.min(start_pos);
    cache.estimated_suffix_start = cache.rows.len();
    cache.content_height = append_timeline_rows::<P, D>(
        sugarloaf,
        pane,
        width,
        theme,
        s,
        gap,
        scan_start,
        None,
        content_y,
        previous_visible_was_edit_tool,
        &mut cache.rows,
    );
    cache.estimated_suffix_start = cache.rows.len();
    cache.source_len = source_len;
    cache.pages = build_timeline_layout_pages(&cache.rows, source_len);
    true
}

fn prepared_message_markdown_blocks<P>(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &P,
    message: &P::Message,
    width: f32,
    theme: &IdeTheme,
    s: f32,
) -> Option<Rc<Vec<AssistantMarkdownBlock>>>
where
    P: AgentTimelinePane,
{
    let text = message.text();
    if text.trim().is_empty() {
        return None;
    }
    let markdown_width = match message.kind() {
        AgentTimelineMessageKind::Assistant => {
            (width - 30.0 * s - ASSISTANT_TEXT_PAD_LEFT * s).max(80.0 * s)
        }
        AgentTimelineMessageKind::Reasoning | AgentTimelineMessageKind::Compaction => {
            (width - 48.0 * s).max(80.0 * s)
        }
        _ => return None,
    };
    Some(layout_assistant_markdown_cached(
        sugarloaf,
        pane,
        text,
        markdown_width,
        theme,
        s,
    ))
}

pub(crate) fn prepared_message_tool_diff_sections<M>(
    message: &M,
    expanded: bool,
) -> Option<CachedToolDiffSections>
where
    M: AgentTimelineMessage,
{
    if message.kind() != AgentTimelineMessageKind::Tool
        || !crate::panels::agent_pane::view::tool_message::show_tool_diff_cards(
            message.tool(),
            message.status(),
            expanded,
            false,
        )
    {
        return None;
    }
    if crate::panels::agent_pane::view::tool_message::is_unsettled_edit_tool(
        message.tool(),
        message.status(),
    ) {
        return None;
    }
    cached_edit_diff_sections_for_parts(
        message.id(),
        message.title(),
        message.text(),
        message.status(),
        message.tool(),
        message.detail(),
    )
}

/// Incrementally fold `delta` freshly-prepended messages into an existing
/// layout. Lays out the new prefix from source 0, then — as soon as the
/// running edit-tool state lines up with an existing cached row boundary —
/// reuses every row from there on by shifting its source index (`+delta`)
/// and its `top` (by the height the prefix added). Returns `false` if it
/// can't safely converge, leaving the caller to fall back to a full rebuild.
///
/// Correctness: an existing row's shape depends only on its (immutable)
/// message and the `previous_visible_was_edit_tool` flowing in. Once that
/// incoming state matches at a shared boundary, the remaining cached rows
/// are identical in shape and only their vertical offset changes — so the
/// shift is exact.
#[allow(clippy::too_many_arguments)]
fn prepend_timeline_layout<P, D>(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut P,
    cache: &mut TimelineLayoutCache<P::Message>,
    delta: usize,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    gap: f32,
) -> bool
where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    let source_len = pane.messages().len();
    if delta == 0 || cache.source_len + delta != source_len {
        return false;
    }

    // Map: old cached row at old source index `k` now lives at new index
    // `k + delta`. Find, for a given new source index, the cached row that
    // starts exactly there (cached rows are sorted ascending by source index).
    let cached_row_starting_at = |new_source_index: usize| -> Option<usize> {
        new_source_index.checked_sub(delta).and_then(|old_index| {
            cache
                .rows
                .binary_search_by(|row| row.source_index.cmp(&old_index))
                .ok()
        })
    };
    // The edit-tool state the cached layout had flowing into row `pos`.
    let cached_incoming_edit_tool = |pos: usize| -> bool {
        pos.checked_sub(1)
            .map(|prev| cache.rows[prev].is_edit_tool)
            .unwrap_or(false)
    };

    let mut new_rows: Vec<TimelineLayoutRow<P::Message>> = Vec::new();
    let mut content_y = 0.0_f32;
    let mut previous_visible_was_edit_tool = false;
    let mut source_index = 0usize;
    let mut converge: Option<(usize, f32)> = None;

    while source_index < source_len {
        // Once we reach previously-laid-out territory, try to converge: if the
        // running state matches a cached row boundary, every row from there on
        // is reusable as-is (only its offset changes).
        if source_index >= delta {
            if let Some(pos) = cached_row_starting_at(source_index) {
                if cached_incoming_edit_tool(pos) == previous_visible_was_edit_tool {
                    converge = Some((pos, content_y));
                    break;
                }
            }
        }

        let Some(item) =
            next_timeline_item::<P>(pane, source_index, previous_visible_was_edit_tool)
        else {
            source_index += 1;
            continue;
        };
        match item {
            NextTimelineItem::Group {
                source_end_exclusive,
                message: group_message,
            } => {
                let height = cached_message_height::<P, D>(
                    sugarloaf,
                    pane,
                    &group_message,
                    width,
                    theme,
                    s,
                );
                if height > 0.0 {
                    if !new_rows.is_empty() {
                        content_y += gap;
                    }
                    let markdown_blocks = prepared_message_markdown_blocks(
                        sugarloaf,
                        pane,
                        &group_message,
                        width,
                        theme,
                        s,
                    );
                    let tool_diff_sections = prepared_message_tool_diff_sections(
                        &group_message,
                        pane.tool_expand_progress(group_message.id()) > 0.001,
                    );
                    new_rows.push(TimelineLayoutRow {
                        source_index,
                        source_end_index: source_end_exclusive.saturating_sub(1),
                        top: content_y,
                        height,
                        display_text: None,
                        display_message: Some(group_message),
                        markdown_blocks,
                        tool_diff_sections,
                        is_edit_tool: false,
                    });
                    content_y += height;
                }
                previous_visible_was_edit_tool = false;
                source_index = source_end_exclusive;
            }
            NextTimelineItem::Message { message } => {
                let height = cached_message_height::<P, D>(
                    sugarloaf, pane, &message, width, theme, s,
                );
                if height <= 0.0 {
                    source_index += 1;
                    continue;
                }
                if !new_rows.is_empty() {
                    content_y += gap;
                }
                let is_edit_tool = is_edit_tool_message(&message);
                let markdown_blocks = prepared_message_markdown_blocks(
                    sugarloaf, pane, &message, width, theme, s,
                );
                let tool_diff_sections = prepared_message_tool_diff_sections(
                    &message,
                    pane.tool_expand_progress(message.id()) > 0.001,
                );
                new_rows.push(TimelineLayoutRow {
                    source_index,
                    source_end_index: source_index,
                    top: content_y,
                    height,
                    display_text: None,
                    display_message: Some(message),
                    markdown_blocks,
                    tool_diff_sections,
                    is_edit_tool,
                });
                previous_visible_was_edit_tool = is_edit_tool;
                content_y += height;
                source_index += 1;
            }
        }
    }

    if let Some((pos, prefix_end_y)) = converge {
        // Splice: new prefix rows, then the reused cached suffix, shifted.
        let gap_before_suffix = if new_rows.is_empty() { 0.0 } else { gap };
        let offset = (prefix_end_y + gap_before_suffix) - cache.rows[pos].top;
        let mut rows = new_rows;
        rows.reserve(cache.rows.len() - pos);
        for mut row in cache.rows.drain(pos..) {
            row.source_index += delta;
            row.source_end_index += delta;
            row.top += offset;
            rows.push(row);
        }
        cache.rows = rows;
    } else {
        // Never converged — `new_rows` is a full layout of the whole
        // transcript (equivalent to a rebuild), so adopt it wholesale.
        cache.rows = new_rows;
    }

    cache.content_height = cache
        .rows
        .last()
        .map(|row| row.top + row.height)
        .unwrap_or(0.0);
    cache.source_len = source_len;
    cache.pages = build_timeline_layout_pages(&cache.rows, source_len);
    // Prepend rebuilt the prefix exactly. Treat the whole cache as exact so
    // the next scroll frame cannot reuse a stale estimated window.
    cache.estimated_prefix_rows = 0;
    cache.estimated_suffix_start = cache.rows.len();
    true
}

fn dirty_start_source<P: AgentTimelinePane>(
    pane: &P,
    cache: &TimelineLayoutCache<P::Message>,
    dirty: &TimelineDirtyMarks,
) -> Option<usize> {
    let source_len = pane.messages().len();
    let mut start = if source_len > cache.source_len {
        cache.source_len.checked_sub(1).or(Some(cache.source_len))
    } else {
        None
    };
    for index in dirty
        .indices
        .iter()
        .copied()
        .filter(|index| *index < source_len)
    {
        start = Some(start.map_or(index, |current| current.min(index)));
    }
    for id in &dirty.ids {
        let Some(index) = pane
            .messages()
            .iter()
            .position(|message| message.id() == id)
        else {
            return Some(0);
        };
        start = Some(start.map_or(index, |current| current.min(index)));
    }
    start
}

#[allow(clippy::too_many_arguments)]
fn append_timeline_rows<P, D>(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    pane: &mut P,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    gap: f32,
    start_index: usize,
    end_index: Option<usize>,
    mut content_y: f32,
    mut previous_visible_was_edit_tool: bool,
    rows: &mut Vec<TimelineLayoutRow<P::Message>>,
) -> f32
where
    P: AgentTimelinePane,
    D: AgentTimelineDelegate<P>,
{
    let mut appended_any = false;
    let source_len = end_index
        .unwrap_or_else(|| pane.messages().len())
        .min(pane.messages().len());
    let mut source_index = start_index;
    while source_index < source_len {
        let Some(item) =
            next_timeline_item::<P>(pane, source_index, previous_visible_was_edit_tool)
        else {
            source_index += 1;
            continue;
        };
        match item {
            NextTimelineItem::Group {
                source_end_exclusive,
                message: group_message,
            } => {
                let height = cached_message_height::<P, D>(
                    sugarloaf,
                    pane,
                    &group_message,
                    width,
                    theme,
                    s,
                );
                if height > 0.0 {
                    if appended_any || !rows.is_empty() {
                        content_y += gap;
                    }
                    let markdown_blocks = prepared_message_markdown_blocks(
                        sugarloaf,
                        pane,
                        &group_message,
                        width,
                        theme,
                        s,
                    );
                    let tool_diff_sections = prepared_message_tool_diff_sections(
                        &group_message,
                        pane.tool_expand_progress(group_message.id()) > 0.001,
                    );
                    rows.push(TimelineLayoutRow {
                        source_index,
                        source_end_index: source_end_exclusive.saturating_sub(1),
                        top: content_y,
                        height,
                        display_text: None,
                        display_message: Some(group_message),
                        markdown_blocks,
                        tool_diff_sections,
                        is_edit_tool: false,
                    });
                    content_y += height;
                    appended_any = true;
                }
                previous_visible_was_edit_tool = false;
                source_index = source_end_exclusive;
            }
            NextTimelineItem::Message { message } => {
                let height = cached_message_height::<P, D>(
                    sugarloaf, pane, &message, width, theme, s,
                );
                if height <= 0.0 {
                    source_index += 1;
                    continue;
                }
                if appended_any || !rows.is_empty() {
                    content_y += gap;
                }
                let is_edit_tool = is_edit_tool_message(&message);
                let markdown_blocks = prepared_message_markdown_blocks(
                    sugarloaf, pane, &message, width, theme, s,
                );
                let tool_diff_sections = prepared_message_tool_diff_sections(
                    &message,
                    pane.tool_expand_progress(message.id()) > 0.001,
                );
                rows.push(TimelineLayoutRow {
                    source_index,
                    source_end_index: source_index,
                    top: content_y,
                    height,
                    display_text: None,
                    display_message: Some(message),
                    markdown_blocks,
                    tool_diff_sections,
                    is_edit_tool,
                });
                previous_visible_was_edit_tool = is_edit_tool;
                content_y += height;
                appended_any = true;
                source_index += 1;
            }
        }
    }
    content_y
}

enum NextTimelineItem<M> {
    Group {
        source_end_exclusive: usize,
        message: M,
    },
    Message {
        message: M,
    },
}

fn next_timeline_item<P>(
    pane: &P,
    source_index: usize,
    previous_visible_was_edit_tool: bool,
) -> Option<NextTimelineItem<P::Message>>
where
    P: AgentTimelinePane,
{
    let messages = pane.messages();
    // Filter before grouping. Otherwise an archived read-tool run could enter
    // through the group fast path even though its typed rows are hidden.
    let message = messages.get(source_index)?;
    let live_start = pane
        .timeline_live_trace_start()
        .unwrap_or(messages.len())
        .min(messages.len());
    if !timeline_message_visible(message, source_index, live_start) {
        return None;
    }
    if let Some((source_end_exclusive, group_message)) =
        read_tool_group_at(messages, source_index)
    {
        return Some(NextTimelineItem::Group {
            source_end_exclusive,
            message: group_message,
        });
    }
    display_timeline_message(message, previous_visible_was_edit_tool)
        .map(|message| NextTimelineItem::Message { message })
}

/// Build a presentation mask without mutating the persisted transcript.
///
/// Settled turns (everything before `live_trace_start`; the whole transcript
/// after a reload) show ONLY user prompts and assistant text — reasoning,
/// tools, edits, subtasks, and compaction rows are hidden, exactly like the
/// original decluttered design. The live window shows everything as it
/// streams (and stays revealed until the session is left).
///
/// The one deliberate difference from the original mask: assistant text is
/// NEVER hidden, in any turn. The old rule kept only each turn's trailing
/// text part — and hid even the final answer when a turn ended on a tool
/// call — which wiped re-entered transcripts down to fragments. Answers must
/// survive every reload and session switch.
#[cfg(test)]
pub(crate) fn timeline_message_visibility<M: AgentTimelineMessage>(
    messages: &[M],
    live_trace_start: Option<usize>,
) -> Vec<bool> {
    let live_start = live_trace_start
        .unwrap_or(messages.len())
        .min(messages.len());
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| timeline_message_visible(message, index, live_start))
        .collect()
}

/// Visibility is row-local. Tail repairs must not allocate a mask or walk the
/// archived prefix just to decide whether a handful of new rows are visible.
fn timeline_message_visible<M: AgentTimelineMessage>(
    message: &M,
    index: usize,
    live_start: usize,
) -> bool {
    if is_subagent_runtime_notification(message)
        || is_background_completion_result_card(message)
    {
        return false;
    }
    match message.kind() {
        AgentTimelineMessageKind::System => message.tool() == "location_notice",
        AgentTimelineMessageKind::Reasoning
        | AgentTimelineMessageKind::Tool
        | AgentTimelineMessageKind::Subtask
        | AgentTimelineMessageKind::Compaction => index >= live_start,
        AgentTimelineMessageKind::User | AgentTimelineMessageKind::Assistant => true,
    }
}

/// The runtime completion sentinel exists only to settle background-task
/// activity. It is not a human turn or a model tool call, and placing it in
/// the visible transcript requires guessing an impossible mid-stream anchor.
/// Genuine model-issued `background_task_result` calls have normal part ids
/// and remain subject to the ordinary live-trace visibility rules.
fn is_background_completion_result_card<M: AgentTimelineMessage>(message: &M) -> bool {
    message.tool() == "background_task_result"
        && message.id().starts_with("background-task-")
}

fn build_timeline_layout_pages<M>(
    rows: &[TimelineLayoutRow<M>],
    source_len: usize,
) -> Vec<TimelineLayoutPage> {
    if source_len == 0 {
        return Vec::new();
    }

    let page_count = source_len.div_ceil(TIMELINE_PAGE_SOURCE_LEN);
    let mut pages = Vec::with_capacity(page_count);
    let mut row_cursor = 0;
    for page_index in 0..page_count {
        let source_start = page_index * TIMELINE_PAGE_SOURCE_LEN;
        let source_end = ((page_index + 1) * TIMELINE_PAGE_SOURCE_LEN).min(source_len);
        while row_cursor < rows.len() && rows[row_cursor].source_end_index < source_start
        {
            row_cursor += 1;
        }
        let row_start = row_cursor;
        let mut row_end = row_start;
        while row_end < rows.len() && rows[row_end].source_index < source_end {
            row_end += 1;
        }
        let (top, height, measured) = if row_start < row_end {
            let top = rows[row_start].top;
            let bottom = rows[row_end - 1].top + rows[row_end - 1].height;
            (top, (bottom - top).max(0.0), true)
        } else {
            (0.0, 0.0, false)
        };
        pages.push(TimelineLayoutPage {
            page_index,
            source_start,
            source_end,
            row_start,
            row_end,
            top,
            height,
            measured,
        });
        row_cursor = row_end;
    }
    pages
}
