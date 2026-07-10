use super::*;
use super::layout::{from_state_cache, into_state_cache};

impl AgentTimelineMessage for NeoismAgentMessage {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> AgentTimelineMessageKind {
        match self.kind {
            NeoismAgentMessageKind::User => AgentTimelineMessageKind::User,
            NeoismAgentMessageKind::Assistant => AgentTimelineMessageKind::Assistant,
            NeoismAgentMessageKind::Reasoning => AgentTimelineMessageKind::Reasoning,
            NeoismAgentMessageKind::Tool => AgentTimelineMessageKind::Tool,
            NeoismAgentMessageKind::System => AgentTimelineMessageKind::System,
            NeoismAgentMessageKind::Subtask => AgentTimelineMessageKind::Subtask,
            NeoismAgentMessageKind::Compaction => AgentTimelineMessageKind::Compaction,
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

    fn output_kind(&self) -> AgentTimelineOutputKind {
        match self.output_kind {
            NeoismAgentOutputKind::Text => AgentTimelineOutputKind::Text,
            NeoismAgentOutputKind::Code => AgentTimelineOutputKind::Code,
            NeoismAgentOutputKind::Todos => AgentTimelineOutputKind::Todos,
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
            kind: NeoismAgentMessageKind::Tool,
            title,
            text,
            status,
            tool: "tool_group".to_string(),
            output_kind: NeoismAgentOutputKind::Text,
            lang: String::new(),
            line_offset: None,
            todos: Vec::new(),
            detail,
            usage: None,
        }
    }
}

impl AgentTimelinePane for NeoismAgentPane {
    type Message = NeoismAgentMessage;
    type MeasureKey = TimelineMeasureKey;

    fn messages(&self) -> &[Self::Message] {
        NeoismAgentPane::messages(self)
    }

    fn timeline_scroll_offset(&self) -> f32 {
        NeoismAgentPane::timeline_scroll_offset(self)
    }

    fn has_active_selection(&self) -> bool {
        NeoismAgentPane::has_active_selection(self)
    }

    fn has_status_activity(&self) -> bool {
        NeoismAgentPane::has_status_activity(self)
    }

    fn timeline_live_trace_start(&self) -> Option<usize> {
        NeoismAgentPane::timeline_live_trace_start(self)
    }

    fn queued_prompt_count(&self) -> usize {
        NeoismAgentPane::queued_prompt_count(self)
    }

    fn set_timeline_metrics(
        &mut self,
        viewport_rect: [f32; 4],
        content_height_px: f32,
        viewport_height_px: f32,
    ) {
        NeoismAgentPane::set_timeline_metrics(
            self,
            viewport_rect,
            content_height_px,
            viewport_height_px,
        );
    }

    fn sync_virtual_timeline(
        &mut self,
        viewport_rect: [f32; 4],
        content_width: f32,
        content_height: f32,
        scroll_top: f32,
        scale: f32,
        rows: &[TimelineVirtualRowMeasurement],
    ) {
        NeoismAgentPane::sync_virtual_timeline(
            self,
            viewport_rect,
            content_width,
            content_height,
            scroll_top,
            scale,
            rows,
        );
    }

    fn uses_virtual_timeline(&self) -> bool {
        true
    }

    fn virtual_timeline_needs_measurements(
        &self,
        content_width: f32,
        scale: f32,
        row_count: usize,
        content_height: f32,
    ) -> bool {
        NeoismAgentPane::virtual_timeline_needs_measurements(
            self,
            content_width,
            scale,
            row_count,
            content_height,
        )
    }

    fn maybe_request_older_timeline_page(&mut self, scroll_top: f32, viewport_h: f32) {
        NeoismAgentPane::maybe_request_older_timeline_page(self, scroll_top, viewport_h);
    }

    fn virtual_timeline_visible_source_range(&self) -> Option<(usize, usize)> {
        NeoismAgentPane::virtual_timeline_visible_source_range(self)
    }

    fn clear_tool_hit_rects(&mut self) {
        NeoismAgentPane::clear_tool_hit_rects(self);
    }

    fn clear_permission_choice_hit_rects(&mut self) {
        NeoismAgentPane::clear_permission_choice_hit_rects(self);
    }

    fn timeline_layout_epoch(&self) -> u64 {
        NeoismAgentPane::timeline_layout_epoch(self)
    }

    fn take_timeline_dirty_marks(&mut self) -> TimelineDirtyMarks {
        let marks = NeoismAgentPane::take_timeline_dirty_marks(self);
        TimelineDirtyMarks {
            ids: marks.ids,
            indices: marks.indices,
        }
    }

    fn take_timeline_layout_cache(&self) -> Option<TimelineLayoutCache<Self::Message>> {
        NeoismAgentPane::take_timeline_layout_cache(self).map(from_state_cache)
    }

    fn store_timeline_layout_cache(&self, cache: TimelineLayoutCache<Self::Message>) {
        NeoismAgentPane::store_timeline_layout_cache(self, into_state_cache(cache));
    }

    fn any_tool_expand_animating(&self) -> bool {
        NeoismAgentPane::any_tool_expand_animating(self)
    }

    fn tool_expand_animating(&self, id: &str) -> bool {
        NeoismAgentPane::tool_expand_animating(self, id)
    }

    fn tool_expanded(&self, id: &str) -> bool {
        NeoismAgentPane::tool_expanded(self, id)
    }

    fn tool_expand_progress(&self, id: &str) -> f32 {
        NeoismAgentPane::tool_expand_progress(self, id)
    }

    fn selected_tool_group_child(&self, group_id: &str) -> Option<&str> {
        NeoismAgentPane::selected_tool_group_child(self, group_id)
    }

    fn timeline_measure_key(
        &self,
        message: &Self::Message,
        width: f32,
        scale: f32,
        tool_expanded: bool,
        selected_tool_group_child: Option<&str>,
    ) -> Self::MeasureKey {
        NeoismAgentPane::timeline_measure_key_with_selected_tool_group_child(
            message,
            width,
            scale,
            tool_expanded,
            selected_tool_group_child,
        )
    }

    fn cached_timeline_measure(&self, key: &Self::MeasureKey) -> Option<f32> {
        NeoismAgentPane::cached_timeline_measure(self, key)
    }

    fn store_timeline_measure(&self, key: Self::MeasureKey, height: f32) {
        NeoismAgentPane::store_timeline_measure(self, key, height);
    }

    fn timeline_scrollbar_state(&self) -> Option<(f32, f32, f32, f32)> {
        let (offset, content_h, viewport_h, last_scroll) =
            NeoismAgentPane::timeline_scrollbar_state(self)?;
        let opacity = if NeoismAgentPane::scrollbar_dragging(self) || offset > 0.0 {
            0.9
        } else {
            scrollbar::opacity_from_last_scroll(last_scroll, false)
        };
        Some((offset, content_h, viewport_h, opacity))
    }

    fn set_scrollbar_geometry(
        &mut self,
        track: Option<[f32; 4]>,
        thumb: Option<[f32; 4]>,
    ) {
        NeoismAgentPane::set_scrollbar_geometry(self, track, thumb);
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
        derivations: ScrollFrameDerivations,
        total_us: u128,
    ) {
        if !crate::panels::agent_pane::state::perf::enabled() {
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

impl AgentTimelineDelegate<NeoismAgentPane> for SharedTimelineDelegate {
    fn measure_message_height(
        sugarloaf: &mut Sugarloaf,
        pane: &mut NeoismAgentPane,
        message: &NeoismAgentMessage,
        width: f32,
        theme: &IdeTheme,
        s: f32,
        tool_expanded: bool,
        tool_expand_progress: f32,
    ) -> f32 {
        measure_message_height(
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
        sugarloaf: &mut Sugarloaf,
        x: f32,
        y: f32,
        w: f32,
        measured_h: f32,
        pane: &mut NeoismAgentPane,
        message: &NeoismAgentMessage,
        markdown_blocks: Option<&[AssistantMarkdownBlock]>,
        tool_diff_sections: Option<&[ToolDiffSection]>,
        theme: &IdeTheme,
        s: f32,
        now_seconds: f32,
        mouse: Option<(f32, f32)>,
        viewport_clip: [f32; 4],
        occlusion_rects: &[[f32; 4]],
    ) -> f32 {
        render_message_card(
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

    fn measure_permission_prompt_height(pane: &NeoismAgentPane, s: f32) -> f32 {
        measure_permission_prompt_height(pane, s)
    }

    fn render_permission_prompt_row(
        sugarloaf: &mut Sugarloaf,
        pane: &mut NeoismAgentPane,
        rect: [f32; 4],
        theme: &IdeTheme,
        s: f32,
        viewport_clip: [f32; 4],
        occlusion_rects: &[[f32; 4]],
    ) {
        render_permission_prompt_row(
            sugarloaf,
            pane,
            rect,
            theme,
            s,
            viewport_clip,
            occlusion_rects,
        );
    }

    fn render_streaming_status_row(
        sugarloaf: &mut Sugarloaf,
        pane: &mut NeoismAgentPane,
        rect: [f32; 4],
        theme: &IdeTheme,
        s: f32,
        now_seconds: f32,
        viewport_clip: [f32; 4],
        occlusion_rects: &[[f32; 4]],
    ) {
        render_streaming_status_row(
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
