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
use super::user_input::{
    render_streaming_status_row, streaming_status_line_count,
    streaming_status_primary_line_count,
};
use super::{DEPTH, ORDER_CARET, STREAMING_STATUS_LINE_H, USER_MESSAGE_MAX_LINES};
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

/// Stable identity for the timeline row held under a reader during relayout.
///
/// Message ids alone are not identities here: optimistic messages can have an
/// empty id, ids can be duplicated, and an optimistic id can become durable
/// while the model is streaming. The source position makes those cases
/// unambiguous; the id ordinals and source length let the same message be found
/// again when history is inserted around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineViewAnchorKey {
    source_index: usize,
    source_len: usize,
    message_id: String,
    id_ordinal_from_start: usize,
    id_ordinal_from_end: usize,
    id_count: usize,
    message_kind: Option<AgentTimelineMessageKind>,
    title: String,
    text: String,
    tool: String,
}

impl TimelineViewAnchorKey {
    pub fn is_for_source(&self, source_index: usize, source_len: usize) -> bool {
        self.source_index == source_index && self.source_len == source_len
    }

    pub fn at_source(
        source_index: usize,
        source_len: usize,
        message_id: impl Into<String>,
    ) -> Self {
        Self {
            source_index,
            source_len,
            message_id: message_id.into(),
            id_ordinal_from_start: 0,
            id_ordinal_from_end: 0,
            id_count: 1,
            message_kind: None,
            title: String::new(),
            text: String::new(),
            tool: String::new(),
        }
    }

    pub fn for_source<M: AgentTimelineMessage>(
        messages: &[M],
        source_index: usize,
    ) -> Option<Self> {
        let message = messages.get(source_index)?;
        let message_id = message.id().to_string();
        Some(Self {
            source_index,
            source_len: messages.len(),
            message_id: message_id.clone(),
            id_ordinal_from_start: messages[..source_index]
                .iter()
                .filter(|candidate| candidate.id() == message_id)
                .count(),
            id_ordinal_from_end: messages[source_index + 1..]
                .iter()
                .filter(|candidate| candidate.id() == message_id)
                .count(),
            id_count: messages
                .iter()
                .filter(|candidate| candidate.id() == message_id)
                .count(),
            message_kind: Some(message.kind()),
            title: message.title().to_string(),
            text: message.text().to_string(),
            tool: message.tool().to_string(),
        })
    }
}

fn matches_anchor_signature<M: AgentTimelineMessage>(
    message: &M,
    key: &TimelineViewAnchorKey,
) -> bool {
    key.message_kind.is_some_and(|kind| kind == message.kind())
        && key.title == message.title()
        && key.tool == message.tool()
        // Assistant/reasoning text can grow while the anchor is held.
        && (key.text == message.text()
            || message.text().starts_with(&key.text)
            || key.text.starts_with(message.text()))
}

fn nth_message_with_id<M: AgentTimelineMessage>(
    messages: &[M],
    id: &str,
    ordinal: usize,
) -> Option<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.id() == id)
        .nth(ordinal)
        .map(|(index, _)| index)
}

pub(crate) fn resolve_timeline_view_anchor<M: AgentTimelineMessage>(
    messages: &[M],
    key: &TimelineViewAnchorKey,
) -> Option<usize> {
    if key.message_id.is_empty() {
        if key.message_kind.is_none() {
            return (messages.len() == key.source_len)
                .then_some(key.source_index)
                .filter(|index| *index < messages.len());
        }
        // Legacy optimistic rows may move and acquire a server id. Signature
        // matching is safe only when exactly one row can be the old bubble.
        let mut candidates = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| matches_anchor_signature(*message, key));
        let candidate = candidates.next().map(|(index, _)| index)?;
        return candidates.next().is_none().then_some(candidate);
    }

    let id_candidates = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.id() == key.message_id)
        .collect::<Vec<_>>();
    if id_candidates.len() == 1 && key.id_count == 1 {
        return Some(id_candidates[0].0);
    }
    if id_candidates.is_empty() {
        return None;
    }

    let signature_matches = id_candidates
        .iter()
        .filter(|(_, message)| matches_anchor_signature(*message, key))
        .map(|(index, _)| *index)
        .collect::<Vec<_>>();
    if signature_matches.len() == 1 {
        return Some(signature_matches[0]);
    }

    let from_start =
        nth_message_with_id(messages, &key.message_id, key.id_ordinal_from_start);
    let from_end = messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, message)| message.id() == key.message_id)
        .nth(key.id_ordinal_from_end)
        .map(|(index, _)| index);
    match (from_start, from_end) {
        (Some(start), Some(end)) if start == end => {
            signature_matches.contains(&start).then_some(start)
        }
        _ => None,
    }
}

pub(crate) fn timeline_row_for_anchor_source<M>(
    rows: &[TimelineLayoutRow<M>],
    source_index: usize,
) -> Option<&TimelineLayoutRow<M>> {
    rows.iter().find(|row| {
        row.source_index <= source_index && source_index <= row.source_end_index
    })
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
    fn images(&self) -> &[crate::panels::agent_pane::state::NeoismAgentImage];
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

/// Whether viewport-only timeline measurement is enabled, read once.
/// Native panes default to lazy measurement so entering a long conversation
/// never synchronously lays out the entire transcript. Set
/// `NEOISM_AGENT_EAGER_TIMELINE=1` only for diagnostics/comparison.
pub fn lazy_timeline_enabled() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::OnceLock;
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("NEOISM_AGENT_EAGER_TIMELINE").is_none())
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

pub trait AgentTimelinePane: AgentMarkdownPane {
    type Message: AgentTimelineMessage;
    type MeasureKey;

    fn messages(&self) -> &[Self::Message];
    fn timeline_scroll_offset(&self) -> f32;
    fn has_active_selection(&self) -> bool;
    fn has_status_activity(&self) -> bool;
    fn streaming_label(&self) -> String;
    /// First source row observed live during this visit to the session. `None`
    /// means the transcript is a reloaded, settled history projection.
    fn timeline_live_trace_start(&self) -> Option<usize>;
    fn queued_prompt_count(&self) -> usize;
    fn running_background_task_count(&self) -> usize;
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
    /// Native viewport-only layout: on a full rebuild, measure exactly only
    /// the rows from just above the viewport down to the end, and cheaply
    /// estimate the off-screen prefix above. Streaming (dirty-tail patch) and
    /// pagination (prepend) remain exact.
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

    fn set_measured_timeline_prepend(
        &mut self,
        _content_height: f32,
        _prepend_height: f32,
    ) {
    }
    fn timeline_view_anchor(&self) -> Option<(TimelineViewAnchorKey, f32)> {
        None
    }
    fn timeline_view_anchor_matches(
        &self,
        _source_index: usize,
        _source_len: usize,
    ) -> bool {
        false
    }
    fn restore_timeline_view_anchor(&mut self, _content_y: f32, _screen_offset: f32) {}
    fn set_timeline_view_anchor(
        &mut self,
        _key: Option<TimelineViewAnchorKey>,
        _screen_offset: f32,
    ) {
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

            fn images(&self) -> &[$crate::panels::agent_pane::state::NeoismAgentImage] {
                &self.images
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
                    author: None,
                    images: Vec::new(),
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

            fn streaming_label(&self) -> String {
                <$pane>::streaming_label(self)
            }

            fn timeline_live_trace_start(&self) -> Option<usize> {
                <$pane>::timeline_live_trace_start(self)
            }

            fn queued_prompt_count(&self) -> usize {
                <$pane>::queued_prompt_count(self)
            }

            fn running_background_task_count(&self) -> usize {
                <$pane>::running_background_task_count(self)
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

            fn set_measured_timeline_prepend(
                &mut self,
                content_height: f32,
                prepend_height: f32,
            ) {
                <$pane>::set_measured_timeline_prepend(self, content_height, prepend_height)
            }

            fn timeline_view_anchor(
                &self,
            ) -> Option<(
                $crate::panels::agent_pane::view::timeline::TimelineViewAnchorKey,
                f32,
            )> {
                <$pane>::timeline_view_anchor(self)
            }

            fn timeline_view_anchor_matches(
                &self,
                source_index: usize,
                source_len: usize,
            ) -> bool {
                <$pane>::timeline_view_anchor_matches(self, source_index, source_len)
            }

            fn restore_timeline_view_anchor(&mut self, content_y: f32, screen_offset: f32) {
                <$pane>::restore_timeline_view_anchor(self, content_y, screen_offset)
            }

            fn set_timeline_view_anchor(
                &mut self,
                key: Option<
                    $crate::panels::agent_pane::view::timeline::TimelineViewAnchorKey,
                >,
                screen_offset: f32,
            ) {
                <$pane>::set_timeline_view_anchor(self, key, screen_offset)
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
                    <$pane>::tool_archived(self, $crate::panels::agent_pane::view::timeline::AgentTimelineMessage::id(message)),
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
