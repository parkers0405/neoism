use std::collections::BTreeSet;
use std::ops::Range;
use std::rc::Rc;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::state::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentOutputKind, NeoismAgentPane,
    TimelineMeasureKey, TimelineVirtualRowMeasurement,
};

use super::assistant::ASSISTANT_TEXT_PAD_LEFT;
use super::code_block::truncate_chars;
use super::derivations::{self, ScrollFrameDerivations};
use super::markdown::{
    layout_assistant_markdown_cached, AgentMarkdownPane, AssistantMarkdownBlock,
};
use super::message_card::{measure_message_height, render_message_card};
use super::tool_message::{
    cached_edit_diff_sections_for_parts, CachedToolDiffSections, ToolDiffSection,
};
use super::user_input::render_streaming_status_row;
use super::{DEPTH, ORDER_CARET, STREAMING_STATUS_LINE_H};
use crate::primitives::ide_theme::IdeTheme;
use crate::widgets::scrollbar;

const LIVE_READ_TOOL_GROUP_MIN: usize = 3;
const TIMELINE_PAGE_SOURCE_LEN: usize = 128;

#[derive(Clone, Debug)]
pub struct TimelineLayoutRow<M> {
    pub source_index: usize,
    pub source_end_index: usize,
    pub top: f32,
    pub height: f32,
    pub display_text: Option<String>,
    pub display_message: Option<M>,
    pub markdown_blocks: Option<Rc<Vec<AssistantMarkdownBlock>>>,
    pub tool_diff_sections: Option<CachedToolDiffSections>,
    pub is_edit_tool: bool,
}

#[derive(Clone, Debug)]
pub struct TimelineLayoutPage {
    pub page_index: usize,
    pub source_start: usize,
    pub source_end: usize,
    pub row_start: usize,
    pub row_end: usize,
    pub top: f32,
    pub height: f32,
    pub measured: bool,
}

#[derive(Clone, Debug)]
pub struct TimelineLayoutCache<M> {
    pub epoch: u64,
    pub source_len: usize,
    pub width_bucket: i32,
    pub scale_bucket: i32,
    pub gap_bucket: i32,
    pub content_height: f32,
    pub pages: Vec<TimelineLayoutPage>,
    pub rows: Vec<TimelineLayoutRow<M>>,
    /// Lazy (viewport-only) measurement bookkeeping: the number of leading
    /// rows whose heights are cheap *estimates* rather than exact measurements
    /// (rows `[0..estimated_prefix_rows]`). Always 0 on the eager path. Because
    /// scroll is distance-from-bottom, an estimated prefix shifts every row's
    /// `top` and `content_height` by the same cumulative error, which cancels
    /// in `card_y = y + row.top - scroll_top` — so visible (exact-suffix) rows
    /// stay put; only the scrollbar thumb is approximate. See the reuse check
    /// in `timeline_layout` which rebuilds before an estimated row can scroll
    /// into the viewport.
    pub estimated_prefix_rows: usize,
}

#[derive(Default)]
pub struct TimelineDirtyMarks {
    pub ids: BTreeSet<String>,
    pub indices: BTreeSet<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTimelineMessageKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    System,
    Subtask,
    Compaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTimelineOutputKind {
    Text,
    Code,
    Todos,
}

pub trait AgentTimelineMessage: Clone {
    fn id(&self) -> &str;
    fn kind(&self) -> AgentTimelineMessageKind;
    fn title(&self) -> &str;
    fn text(&self) -> &str;
    fn status(&self) -> &str;
    fn tool(&self) -> &str;
    fn output_kind(&self) -> AgentTimelineOutputKind;
    fn detail(&self) -> &str;
    fn todos_empty(&self) -> bool;
    fn with_text(&self, text: String) -> Self;
    fn tool_group_message(
        id: String,
        title: String,
        text: String,
        status: String,
        detail: String,
    ) -> Self;
}

/// Whether viewport-only (lazy) timeline measurement is enabled, read once.
/// Absent on wasm (where `var_os` returns `None`), so the web pane stays eager.
pub fn lazy_timeline_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("NEOISM_AGENT_LAZY_TIMELINE").is_some())
}

pub trait AgentTimelinePane: AgentMarkdownPane {
    type Message: AgentTimelineMessage;
    type MeasureKey;

    fn messages(&self) -> &[Self::Message];
    fn timeline_scroll_offset(&self) -> f32;
    fn has_active_selection(&self) -> bool;
    fn has_status_activity(&self) -> bool;
    /// First source row observed live during this visit to the session. `None`
    /// means the transcript is a reloaded, settled history projection.
    fn timeline_live_trace_start(&self) -> Option<usize>;
    fn queued_prompt_count(&self) -> usize;
    fn set_timeline_metrics(
        &mut self,
        viewport_rect: [f32; 4],
        content_height_px: f32,
        viewport_height_px: f32,
    );
    fn sync_virtual_timeline(
        &mut self,
        _viewport_rect: [f32; 4],
        _content_width: f32,
        _content_height: f32,
        _scroll_top: f32,
        _scale: f32,
        _rows: &[TimelineVirtualRowMeasurement],
    ) {
    }
    /// Whether this pane feeds per-row measurements into a virtual surface
    /// (`sync_virtual_timeline`). Panes that draw straight from the windowed
    /// `layout.rows` (e.g. desktop) return `false` so the renderer skips
    /// building the full-history measurement `Vec` every frame — that build
    /// is O(total history) and is the reason scrolling degraded as the
    /// transcript grew, even though drawing itself is already windowed.
    fn uses_virtual_timeline(&self) -> bool {
        false
    }
    /// Opt-in (env `NEOISM_AGENT_LAZY_TIMELINE`) viewport-only layout: on a full
    /// rebuild, measure exactly only the rows from just above the viewport down
    /// to the end, and cheaply *estimate* the off-screen prefix above. Keeps the
    /// occasional full rebuild / huge-transcript load proportional to the
    /// viewport instead of the whole history. Default off (and always off on
    /// wasm, where the env var is absent) so the low-count scroll feel is
    /// untouched. Streaming (dirty-tail patch) and pagination (prepend) still
    /// run exact — only the full rebuild goes windowed.
    fn timeline_lazy_measurement(&self) -> bool {
        lazy_timeline_enabled()
    }
    fn virtual_timeline_needs_measurements(
        &self,
        _content_width: f32,
        _scale: f32,
        _row_count: usize,
        _content_height: f32,
    ) -> bool {
        false
    }
    fn maybe_request_older_timeline_page(&mut self, _scroll_top: f32, _viewport_h: f32) {}
    /// Number of messages just prepended to the front of the transcript
    /// (history pagination), consumed once. When present, the renderer
    /// lays out only the new prefix and shifts the existing rows instead of
    /// rebuilding the whole timeline — keeping each "load older" O(added)
    /// rather than O(total loaded), so pagination never degrades. Panes that
    /// don't paginate return `None`.
    fn take_timeline_prepend(&mut self) -> Option<usize> {
        None
    }
    fn virtual_timeline_visible_source_range(&self) -> Option<(usize, usize)> {
        None
    }
    fn clear_tool_hit_rects(&mut self);
    fn timeline_layout_epoch(&self) -> u64;
    fn take_timeline_dirty_marks(&mut self) -> TimelineDirtyMarks;
    fn take_timeline_layout_cache(&self) -> Option<TimelineLayoutCache<Self::Message>>;
    fn store_timeline_layout_cache(&self, cache: TimelineLayoutCache<Self::Message>);
    fn any_tool_expand_animating(&self) -> bool;
    fn tool_expand_animating(&self, id: &str) -> bool;
    fn tool_expanded(&self, id: &str) -> bool;
    fn tool_expand_progress(&self, id: &str) -> f32;
    fn selected_tool_group_child(&self, group_id: &str) -> Option<&str>;
    fn timeline_measure_key(
        &self,
        message: &Self::Message,
        width: f32,
        scale: f32,
        tool_expanded: bool,
        selected_tool_group_child: Option<&str>,
    ) -> Self::MeasureKey;
    fn cached_timeline_measure(&self, key: &Self::MeasureKey) -> Option<f32>;
    fn store_timeline_measure(&self, key: Self::MeasureKey, height: f32);
    fn timeline_scrollbar_state(&self) -> Option<(f32, f32, f32, f32)>;
    fn set_scrollbar_geometry(
        &mut self,
        track: Option<[f32; 4]>,
        thumb: Option<[f32; 4]>,
    );
    fn log_timeline_perf(
        &self,
        row_count: usize,
        rendered_rows: usize,
        rendered_text_bytes: usize,
        rendered_row_start: usize,
        rendered_row_end: usize,
        viewport_h: f32,
        content_h: f32,
        cacheable_layout: bool,
        layout_us: Option<u128>,
        rows_us: Option<u128>,
        prep_us: Option<u128>,
        post_us: Option<u128>,
        derivations: ScrollFrameDerivations,
        total_us: u128,
    );
    /// Whether agent-ui perf tracing is enabled (mirrors the host's perf
    /// flag). Lets the renderer emit ad-hoc sub-phase timings without
    /// threading more args through `log_timeline_perf`.
    fn timeline_perf_enabled(&self) -> bool {
        false
    }
}

pub trait AgentTimelineDelegate<P: AgentTimelinePane> {
    fn measure_message_height(
        sugarloaf: &mut Sugarloaf,
        pane: &mut P,
        message: &P::Message,
        width: f32,
        theme: &IdeTheme,
        s: f32,
        tool_expanded: bool,
        tool_expand_progress: f32,
    ) -> f32;

    #[allow(clippy::too_many_arguments)]
    fn render_message_card(
        sugarloaf: &mut Sugarloaf,
        x: f32,
        y: f32,
        w: f32,
        measured_h: f32,
        pane: &mut P,
        message: &P::Message,
        markdown_blocks: Option<&[AssistantMarkdownBlock]>,
        tool_diff_sections: Option<&[ToolDiffSection]>,
        theme: &IdeTheme,
        s: f32,
        now_seconds: f32,
        mouse: Option<(f32, f32)>,
        viewport_clip: [f32; 4],
        occlusion_rects: &[[f32; 4]],
    ) -> f32;

    fn render_streaming_status_row(
        sugarloaf: &mut Sugarloaf,
        pane: &mut P,
        rect: [f32; 4],
        theme: &IdeTheme,
        s: f32,
        now_seconds: f32,
        viewport_clip: [f32; 4],
        occlusion_rects: &[[f32; 4]],
    );
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_timeline_message {
    ($message:ty, $kind:ident, $output_kind:ident) => {
        impl $crate::panels::agent_pane::view::timeline::AgentTimelineMessage
            for $message
        {
            fn id(&self) -> &str {
                &self.id
            }

            fn kind(
                &self,
            ) -> $crate::panels::agent_pane::view::timeline::AgentTimelineMessageKind {
                match self.kind {
                    $kind::User => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineMessageKind::User
                    }
                    $kind::Assistant => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineMessageKind::Assistant
                    }
                    $kind::Reasoning => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineMessageKind::Reasoning
                    }
                    $kind::Tool => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineMessageKind::Tool
                    }
                    $kind::System => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineMessageKind::System
                    }
                    $kind::Subtask => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineMessageKind::Subtask
                    }
                    $kind::Compaction => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineMessageKind::Compaction
                    }
                }
            }

            fn title(&self) -> &str {
                &self.title
            }

            fn text(&self) -> &str {
                &self.text
            }

            fn status(&self) -> &str {
                &self.status
            }

            fn tool(&self) -> &str {
                &self.tool
            }

            fn output_kind(
                &self,
            ) -> $crate::panels::agent_pane::view::timeline::AgentTimelineOutputKind {
                match self.output_kind {
                    $output_kind::Text => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineOutputKind::Text
                    }
                    $output_kind::Code => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineOutputKind::Code
                    }
                    $output_kind::Todos => {
                        $crate::panels::agent_pane::view::timeline::AgentTimelineOutputKind::Todos
                    }
                }
            }

            fn detail(&self) -> &str {
                &self.detail
            }

            fn todos_empty(&self) -> bool {
                self.todos.is_empty()
            }

            fn with_text(&self, text: String) -> Self {
                let mut message = self.clone();
                message.text = text;
                message
            }

            fn tool_group_message(
                id: String,
                title: String,
                text: String,
                status: String,
                detail: String,
            ) -> Self {
                Self {
                    id,
                    kind: $kind::Tool,
                    title,
                    text,
                    status,
                    tool: "tool_group".to_string(),
                    output_kind: $output_kind::Text,
                    lang: String::new(),
                    line_offset: None,
                    todos: Vec::new(),
                    detail,
                    usage: None,
                }
            }
        }
    };
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_timeline_pane {
    ($pane:ty, $message:ty, $measure_key:ty, $perf_enabled:path) => {
        impl $crate::panels::agent_pane::view::timeline::AgentTimelinePane for $pane {
            type Message = $message;
            type MeasureKey = $measure_key;

            fn messages(&self) -> &[Self::Message] {
                <$pane>::messages(self)
            }

            fn timeline_scroll_offset(&self) -> f32 {
                <$pane>::timeline_scroll_offset(self)
            }

            fn has_active_selection(&self) -> bool {
                <$pane>::has_active_selection(self)
            }

            fn has_status_activity(&self) -> bool {
                <$pane>::has_status_activity(self)
            }

            fn timeline_live_trace_start(&self) -> Option<usize> {
                <$pane>::timeline_live_trace_start(self)
            }

            fn queued_prompt_count(&self) -> usize {
                <$pane>::queued_prompt_count(self)
            }

            fn set_timeline_metrics(
                &mut self,
                viewport_rect: [f32; 4],
                content_height_px: f32,
                viewport_height_px: f32,
            ) {
                <$pane>::set_timeline_metrics(
                    self,
                    viewport_rect,
                    content_height_px,
                    viewport_height_px,
                );
            }

            fn clear_tool_hit_rects(&mut self) {
                <$pane>::clear_tool_hit_rects(self);
            }

            fn timeline_perf_enabled(&self) -> bool {
                $perf_enabled()
            }

            fn timeline_layout_epoch(&self) -> u64 {
                <$pane>::timeline_layout_epoch(self)
            }

            fn take_timeline_dirty_marks(
                &mut self,
            ) -> $crate::panels::agent_pane::view::timeline::TimelineDirtyMarks {
                let marks = <$pane>::take_timeline_dirty_marks(self);
                $crate::panels::agent_pane::view::timeline::TimelineDirtyMarks {
                    ids: marks.ids,
                    indices: marks.indices,
                }
            }

            fn take_timeline_layout_cache(
                &self,
            ) -> Option<
                $crate::panels::agent_pane::view::timeline::TimelineLayoutCache<
                    Self::Message,
                >,
            > {
                <$pane>::take_timeline_layout_cache(self)
            }

            fn store_timeline_layout_cache(
                &self,
                cache: $crate::panels::agent_pane::view::timeline::TimelineLayoutCache<
                    Self::Message,
                >,
            ) {
                <$pane>::store_timeline_layout_cache(self, cache);
            }

            fn maybe_request_older_timeline_page(&mut self, scroll_top: f32, viewport_h: f32) {
                <$pane>::maybe_request_older_timeline_page(self, scroll_top, viewport_h);
            }

            fn take_timeline_prepend(&mut self) -> Option<usize> {
                <$pane>::take_timeline_prepend(self)
            }

            fn any_tool_expand_animating(&self) -> bool {
                <$pane>::any_tool_expand_animating(self)
            }

            fn tool_expand_animating(&self, id: &str) -> bool {
                <$pane>::tool_expand_animating(self, id)
            }

            fn tool_expanded(&self, id: &str) -> bool {
                <$pane>::tool_expanded(self, id)
            }

            fn tool_expand_progress(&self, id: &str) -> f32 {
                <$pane>::tool_expand_progress(self, id)
            }

            fn selected_tool_group_child(&self, group_id: &str) -> Option<&str> {
                <$pane>::selected_tool_group_child(self, group_id)
            }

            fn timeline_measure_key(
                &self,
                message: &Self::Message,
                width: f32,
                scale: f32,
                tool_expanded: bool,
                selected_tool_group_child: Option<&str>,
            ) -> Self::MeasureKey {
                <$pane>::timeline_measure_key_with_selected_tool_group_child(
                    message,
                    width,
                    scale,
                    tool_expanded,
                    selected_tool_group_child,
                )
            }

            fn cached_timeline_measure(&self, key: &Self::MeasureKey) -> Option<f32> {
                <$pane>::cached_timeline_measure(self, key)
            }

            fn store_timeline_measure(&self, key: Self::MeasureKey, height: f32) {
                <$pane>::store_timeline_measure(self, key, height);
            }

            fn timeline_scrollbar_state(&self) -> Option<(f32, f32, f32, f32)> {
                let (offset, content_h, viewport_h, last_scroll) =
                    <$pane>::timeline_scrollbar_state(self)?;
                let opacity = if <$pane>::scrollbar_dragging(self) || offset > 0.0 {
                    0.9
                } else {
                    $crate::widgets::scrollbar::opacity_from_last_scroll(last_scroll, false)
                };
                Some((offset, content_h, viewport_h, opacity))
            }

            fn set_scrollbar_geometry(
                &mut self,
                track: Option<[f32; 4]>,
                thumb: Option<[f32; 4]>,
            ) {
                <$pane>::set_scrollbar_geometry(self, track, thumb);
            }

            fn log_timeline_perf(
                &self,
                row_count: usize,
                rendered_rows: usize,
                rendered_text_bytes: usize,
                rendered_row_start: usize,
                rendered_row_end: usize,
                viewport_h: f32,
                content_h: f32,
                cacheable_layout: bool,
                layout_us: Option<u128>,
                rows_us: Option<u128>,
                prep_us: Option<u128>,
                post_us: Option<u128>,
                derivations: $crate::panels::agent_pane::view::derivations::ScrollFrameDerivations,
                total_us: u128,
            ) {
                if !$perf_enabled() {
                    return;
                }
                tracing::info!(
                    target: "neoism::agent_ui_perf",
                    messages = self.messages().len(),
                    rows = row_count,
                    rendered_rows,
                    rendered_row_start,
                    rendered_row_end,
                    rendered_text_bytes,
                    viewport_h,
                    content_h,
                    scroll_px = self.timeline_scroll_offset(),
                    cacheable_layout,
                    layout_cache_hit = layout_us.is_none(),
                    layout_us,
                    rows_us,
                    prep_us,
                    post_us,
                    derivations_total = derivations.total(),
                    markdown_layouts = derivations.markdown_layouts,
                    tool_diff_sections = derivations.tool_diff_sections,
                    tool_wraps = derivations.tool_wraps,
                    diff_wraps = derivations.diff_wraps,
                    diff_highlights = derivations.diff_highlights,
                    code_line_ranges = derivations.code_line_ranges,
                    code_highlights = derivations.code_highlights,
                    message_clones = derivations.message_clones,
                    total_us,
                    "agent timeline render"
                );
            }
        }
    };
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_timeline_delegate {
    (
        $delegate:ty,
        $pane:ty,
        $message:ty,
        measure_message_height = $measure_message_height:path,
        render_message_card = $render_message_card:path,
        render_streaming_status_row = $render_streaming_status_row:path $(,)?
    ) => {
        impl $crate::panels::agent_pane::view::timeline::AgentTimelineDelegate<$pane>
            for $delegate
        {
            fn measure_message_height(
                sugarloaf: &mut $crate::sugarloaf::Sugarloaf,
                pane: &mut $pane,
                message: &$message,
                width: f32,
                theme: &$crate::primitives::ide_theme::IdeTheme,
                s: f32,
                tool_expanded: bool,
                tool_expand_progress: f32,
            ) -> f32 {
                $measure_message_height(
                    sugarloaf,
                    pane,
                    message,
                    width,
                    theme,
                    s,
                    tool_expanded,
                    tool_expand_progress,
                )
            }

            fn render_message_card(
                sugarloaf: &mut $crate::sugarloaf::Sugarloaf,
                x: f32,
                y: f32,
                w: f32,
                measured_h: f32,
                pane: &mut $pane,
                message: &$message,
                markdown_blocks: Option<
                    &[
                        $crate::panels::agent_pane::view::markdown::AssistantMarkdownBlock
                    ],
                >,
                tool_diff_sections: Option<
                    &[
                        $crate::panels::agent_pane::view::tool_message::ToolDiffSection
                    ],
                >,
                theme: &$crate::primitives::ide_theme::IdeTheme,
                s: f32,
                now_seconds: f32,
                mouse: Option<(f32, f32)>,
                viewport_clip: [f32; 4],
                occlusion_rects: &[[f32; 4]],
            ) -> f32 {
                $render_message_card(
                    sugarloaf,
                    x,
                    y,
                    w,
                    measured_h,
                    pane,
                    message,
                    markdown_blocks,
                    tool_diff_sections,
                    theme,
                    s,
                    now_seconds,
                    mouse,
                    viewport_clip,
                    occlusion_rects,
                )
            }

            fn render_streaming_status_row(
                sugarloaf: &mut $crate::sugarloaf::Sugarloaf,
                pane: &mut $pane,
                rect: [f32; 4],
                theme: &$crate::primitives::ide_theme::IdeTheme,
                s: f32,
                now_seconds: f32,
                viewport_clip: [f32; 4],
                occlusion_rects: &[[f32; 4]],
            ) {
                $render_streaming_status_row(
                    sugarloaf,
                    pane,
                    rect,
                    theme,
                    s,
                    now_seconds,
                    viewport_clip,
                    occlusion_rects,
                );
            }
        }
    };
}

pub struct SharedTimelineDelegate;

mod impls;
mod layout;
mod read_group;
mod render;
#[cfg(test)]
mod tests;

pub use render::{render_timeline_scrollbar_with, render_timeline_with};

#[allow(dead_code)]
pub(super) fn render_timeline_scrollbar(
    sugarloaf: &mut Sugarloaf,
    pane: &mut NeoismAgentPane,
    rect: [f32; 4],
    s: f32,
) {
    render_timeline_scrollbar_with(sugarloaf, pane, rect, s);
}
