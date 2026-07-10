use super::*;

impl NeoismAgentPane {
    pub fn with_directory(directory: Option<String>) -> Self {
        let mut pane = Self {
            directory,
            ..Self::default()
        };
        pane.apply_config_defaults();
        pane
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn messages(&self) -> &[NeoismAgentMessage] {
        &self.messages
    }

    pub fn is_subagent_session(&self) -> bool {
        self.parent_session_id.is_some()
    }

    pub(crate) fn timeline_measure_key(
        message: &NeoismAgentMessage,
        width: f32,
        scale: f32,
        tool_expanded: bool,
    ) -> TimelineMeasureKey {
        TimelineMeasureKey {
            id: hash_value(&message.id),
            kind: message.kind,
            output_kind: message.output_kind,
            width_bucket: f32_measure_bucket(width),
            scale_bucket: f32_measure_bucket(scale),
            tool_expanded,
            title: hash_value(&message.title),
            text: hash_agent_message_text_for_measure(&message.text),
            status: hash_value(&message.status),
            tool: hash_value(&message.tool),
            lang: hash_value(&message.lang),
            line_offset: message.line_offset,
            todos: hash_value(&message.todos),
            detail: hash_value(&message.detail),
            selected_tool_group_child: 0,
        }
    }

    pub(crate) fn timeline_measure_key_with_selected_tool_group_child(
        message: &NeoismAgentMessage,
        width: f32,
        scale: f32,
        tool_expanded: bool,
        selected_tool_group_child: Option<&str>,
    ) -> TimelineMeasureKey {
        let mut key = Self::timeline_measure_key(message, width, scale, tool_expanded);
        key.selected_tool_group_child = selected_tool_group_child
            .map(|value| hash_value(&value))
            .unwrap_or(0);
        key
    }

    pub(crate) fn cached_timeline_measure(
        &self,
        key: &TimelineMeasureKey,
    ) -> Option<f32> {
        self.timeline_measure_cache.borrow().get(key).copied()
    }

    pub(crate) fn store_timeline_measure(&self, key: TimelineMeasureKey, height: f32) {
        // High cap so a long paginated transcript stays fully measured —
        // the wholesale clear is a re-measure cliff, so keep it out of reach
        // for realistic session sizes (entries are tiny: a key + an f32).
        const MAX_TIMELINE_MEASURE_CACHE: usize = 16384;
        let mut cache = self.timeline_measure_cache.borrow_mut();
        if cache.len() >= MAX_TIMELINE_MEASURE_CACHE {
            cache.clear();
        }
        cache.insert(key, height);
    }

    pub(crate) fn markdown_blocks_key(
        text: &str,
        width: f32,
        scale: f32,
    ) -> MarkdownBlocksKey {
        MarkdownBlocksKey {
            text_hash: hash_value(&text),
            text_len: text.len(),
            width_bucket: f32_measure_bucket(width),
            scale_bucket: f32_measure_bucket(scale),
        }
    }

    /// Bump and return the next monotonic LRU tick. `u64` never realistically
    /// wraps (would take ~1.8e19 accesses), so plain addition is fine.
    pub(crate) fn next_markdown_blocks_tick(&self) -> u64 {
        let tick = self.markdown_blocks_tick.get().saturating_add(1);
        self.markdown_blocks_tick.set(tick);
        tick
    }

    pub(crate) fn cached_markdown_blocks(
        &self,
        key: &MarkdownBlocksKey,
    ) -> Option<CachedMarkdownBlocks> {
        let tick = self.next_markdown_blocks_tick();
        let mut cache = self.markdown_blocks_cache.borrow_mut();
        let entry = cache.get_mut(key)?;
        // Promote on access so the visible working set is never the eviction
        // victim — this is what keeps long-history scroll on cache hits.
        entry.1 = tick;
        Some(entry.0.clone())
    }

    pub(crate) fn store_markdown_blocks(
        &self,
        key: MarkdownBlocksKey,
        blocks: CachedMarkdownBlocks,
    ) {
        // Sized for paginated history: scrolling back through many loaded
        // pages must not evict and re-parse cards still in reach.
        const MAX_MARKDOWN_BLOCKS_CACHE: usize = 4096;
        let tick = self.next_markdown_blocks_tick();
        let mut cache = self.markdown_blocks_cache.borrow_mut();
        // Evict the least-recently-*used* entry (smallest tick) once full. The
        // O(n) scan only runs on a miss-driven insert past the cap, which after
        // warmup is rare, so it stays off the hot scroll path.
        if !cache.contains_key(&key) && cache.len() >= MAX_MARKDOWN_BLOCKS_CACHE {
            if let Some(victim) = cache
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(victim_key, _)| *victim_key)
            {
                cache.remove(&victim);
            }
        }
        cache.insert(key, (blocks, tick));
    }

    pub(crate) fn timeline_layout_epoch(&self) -> u64 {
        self.timeline_layout_epoch
    }

    pub(crate) fn timeline_live_trace_start(&self) -> Option<usize> {
        self.timeline_live_trace_start
    }

    pub(crate) fn take_timeline_layout_cache(&self) -> Option<TimelineLayoutCache> {
        self.timeline_layout_cache.borrow_mut().take()
    }

    pub(crate) fn store_timeline_layout_cache(&self, cache: TimelineLayoutCache) {
        *self.timeline_layout_cache.borrow_mut() = Some(cache);
    }

    pub(crate) fn take_timeline_prepend(&mut self) -> Option<usize> {
        self.pending_timeline_prepend_count.take()
    }

    /// Record that `count` messages were prepended at the front of the
    /// transcript. The renderer folds them into the existing layout
    /// incrementally rather than rebuilding every row. Accumulates if several
    /// pages land before the next frame.
    pub(crate) fn note_timeline_prepend(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        if let Some(start) = &mut self.timeline_live_trace_start {
            *start = start.saturating_add(count);
        }
        self.pending_timeline_prepend_count =
            Some(self.pending_timeline_prepend_count.unwrap_or(0) + count);
    }

    pub(crate) fn take_timeline_dirty_marks(&mut self) -> TimelineDirtyMarks {
        TimelineDirtyMarks {
            ids: std::mem::take(&mut self.timeline_dirty_message_ids),
            indices: std::mem::take(&mut self.timeline_dirty_message_indices),
        }
    }

    pub fn model(&self) -> &str {
        if self.model.is_empty() {
            "server default"
        } else {
            &self.model
        }
    }

    pub fn agent_label(&self) -> &str {
        match self.agent.as_deref() {
            Some("build") => "Build",
            Some("plan") => "Plan",
            Some(agent) => agent,
            None => "server default",
        }
    }

    pub fn thinking_label(&self) -> &str {
        self.thinking.as_deref().unwrap_or("none")
    }

    pub fn directory_label(&self) -> String {
        let raw = self
            .directory
            .clone()
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| ".".to_string());
        compact_directory_label(&raw)
    }

    pub fn picker(&self) -> Option<&NeoismAgentPicker> {
        self.picker.as_ref()
    }

    pub fn picker_mut(&mut self) -> Option<&mut NeoismAgentPicker> {
        self.picker.as_mut()
    }

    pub(crate) fn background_sender(&self) -> Sender<NeoismAgentBackgroundUpdate> {
        self.background_tx.clone()
    }

    pub(crate) fn push_outbound(&mut self, command: OutboundAgentCommand) {
        self.pending_outbound.push_back(command);
    }

    pub(crate) fn drain_pending_outbound(&mut self) -> Vec<OutboundAgentCommand> {
        self.pending_outbound.drain(..).collect()
    }

    pub(crate) fn apply_config_defaults(&mut self) {
        self.push_outbound(OutboundAgentCommand::ApplyConfigDefaults);
    }

    pub(crate) fn execute_apply_config_defaults_command(&mut self) {
        let Ok(defaults) = fetch_config_defaults(&self.server, self.directory.as_deref())
        else {
            return;
        };
        if let Some(agent) = defaults.agent {
            match agent.as_str() {
                "build" => self.mode = NeoismAgentMode::Build,
                "plan" => self.mode = NeoismAgentMode::Plan,
                _ => {}
            }
            self.agent = Some(agent);
        }
        if let Some(model) = defaults.model {
            self.model = model;
        }
        self.thinking = defaults.thinking;
        self.execute_refresh_model_context_limit_command();
    }

    pub fn cursor_rect(&self) -> Option<[f32; 4]> {
        self.cursor_rect
    }

    pub fn cursor_byte(&self) -> usize {
        self.cursor_byte.min(self.input.len())
    }

    pub(crate) fn input_buffer(&self) -> AgentInputBuffer {
        AgentInputBuffer::new(
            self.input.clone(),
            self.cursor_byte,
            self.sent_history.clone(),
            self.history_index,
            self.history_draft.clone(),
        )
    }

    pub(crate) fn apply_input_buffer(&mut self, buffer: AgentInputBuffer) {
        self.input = buffer.input;
        self.cursor_byte = buffer.cursor_byte;
        self.sent_history = buffer.sent_history;
        self.history_index = buffer.history_index;
        self.history_draft = buffer.history_draft;
    }

    pub fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>) {
        self.cursor_rect = rect;
    }

    pub fn set_input_wrap_ranges(&mut self, ranges: Vec<(usize, usize)>) {
        self.input_wrap_len = self.input.len();
        self.input_wrap_ranges = ranges;
    }

    /// Wrap ranges registered by the renderer, but only if they still
    /// describe the current input (a keystroke can land between edit
    /// and redraw).
    pub(crate) fn current_input_wrap_ranges(&self) -> Option<&[(usize, usize)]> {
        (!self.input_wrap_ranges.is_empty()
            && self.input_wrap_len == self.input.len())
        .then_some(self.input_wrap_ranges.as_slice())
    }

    pub fn clear_tool_hit_rects(&mut self) {
        self.tool_hit_rects.clear();
        self.diff_scroll_rects.clear();
        self.link_hit_rects.clear();
        // Keep the Vec + its String allocations; just mark it logically empty.
        // Re-registration this frame overwrites slots in place.
        self.selectable_lines_len = 0;
    }

    pub fn register_diff_scroll_rect(
        &mut self,
        key: String,
        rect: [f32; 4],
        max_scroll: f32,
    ) {
        interaction_policy::register_diff_scroll_rect(
            &mut self.diff_scroll_rects,
            key,
            rect,
            max_scroll,
        );
    }

    pub fn diff_scroll_offset(&mut self, key: &str, max_scroll: f32) -> f32 {
        interaction_policy::diff_scroll_offset(
            &mut self.diff_scroll_offsets,
            key,
            max_scroll,
        )
    }

    pub fn scroll_diff_at(&mut self, x: f32, y: f32, delta_pixels: f32) -> Option<bool> {
        interaction_policy::scroll_diff_at(
            &self.diff_scroll_rects,
            &mut self.diff_scroll_offsets,
            x,
            y,
            delta_pixels,
        )
    }

    pub fn clear_usage_chip_rect(&mut self) {
        self.usage_chip_rect = None;
    }

    pub fn register_usage_chip_rect(&mut self, rect: [f32; 4]) {
        self.usage_chip_rect = Some(rect);
    }

    pub fn usage_chip_contains(&self, x: f32, y: f32) -> bool {
        self.usage_chip_rect
            .is_some_and(|rect| interaction_policy::rect_contains(rect, x, y))
    }

    pub fn clear_status_chip_rects(&mut self) {
        self.status_chip_rects = [None; 3];
    }

    pub fn register_status_chip_rect(&mut self, index: usize, rect: [f32; 4]) {
        if let Some(slot) = self.status_chip_rects.get_mut(index) {
            *slot = Some(rect);
        }
    }

    /// Which dropdown chip (0 = agent, 1 = model, 2 = thinking) sits
    /// under the pointer, if any.
    pub fn status_chip_at(&self, x: f32, y: f32) -> Option<usize> {
        self.status_chip_rects.iter().position(|slot| {
            slot.is_some_and(|rect| interaction_policy::rect_contains(rect, x, y))
        })
    }

    /// Open the "/" picker matching a clicked dropdown chip.
    pub fn open_status_chip_picker(&mut self, index: usize) {
        match index {
            0 => self.open_agent_picker(),
            1 => self.open_model_picker(),
            _ => self.open_thinking_picker(),
        }
    }

    pub fn register_background_status_rect(&mut self, rect: [f32; 4]) {
        self.background_status_rect = Some(rect);
    }

    pub fn clear_background_status_rect(&mut self) {
        self.background_status_rect = None;
    }

    pub fn background_status_contains(&self, x: f32, y: f32) -> bool {
        self.background_status_rect
            .is_some_and(|rect| interaction_policy::rect_contains(rect, x, y))
    }

    pub fn background_task_details_expanded(&self) -> bool {
        self.background_task_details_expanded && self.running_background_task_count() > 0
    }

    pub fn active_background_task_summaries(&self) -> Vec<String> {
        active_background_task_summaries(&self.messages)
    }

    pub fn register_tool_hit_rect(&mut self, id: String, rect: [f32; 4]) {
        interaction_policy::register_hit_rect(&mut self.tool_hit_rects, id, rect);
    }

    pub fn selected_tool_group_child(&self, group_id: &str) -> Option<&str> {
        self.selected_tool_group_child
            .as_ref()
            .filter(|(selected_group, _)| selected_group == group_id)
            .map(|(_, child)| child.as_str())
    }

    pub fn tool_expanded(&self, id: &str) -> bool {
        !id.is_empty() && self.expanded_tool_ids.contains(id)
    }

    pub fn tool_expand_progress(&self, id: &str) -> f32 {
        if id.is_empty() {
            return 0.0;
        }
        let settled = if self.tool_expanded(id) { 1.0 } else { 0.0 };
        self.tool_expand_anims
            .get(id)
            .filter(|anim| anim.is_active())
            .map(|anim| anim.progress())
            .unwrap_or(settled)
    }

    pub fn tool_expand_animating(&self, id: &str) -> bool {
        !id.is_empty()
            && self
                .tool_expand_anims
                .get(id)
                .is_some_and(|anim| anim.is_active())
    }

    pub(crate) fn any_tool_expand_animating(&self) -> bool {
        self.tool_expand_anims.values().any(|anim| anim.is_active())
    }

    pub fn toggle_tool_at(&mut self, x: f32, y: f32) -> bool {
        let Some((id, rect)) =
            interaction_policy::hit_rect_target(&self.tool_hit_rects, x, y)
        else {
            return false;
        };
        let anchor_screen_y = self
            .timeline_viewport_rect
            .map(|[_, vy, _, vh]| rect[1].clamp(vy, vy + vh))
            .unwrap_or(rect[1]);
        self.pending_timeline_anchor = Some(TimelineAnchor {
            content_y: self.content_y_for_screen_y(anchor_screen_y),
            screen_y: anchor_screen_y,
        });
        self.timeline_velocity_px_s = 0.0;
        self.timeline_last_tick_at = None;

        if let Some((group_id, child_id)) = id.split_once("::child::") {
            let next = (group_id.to_string(), child_id.to_string());
            if self.selected_tool_group_child.as_ref() == Some(&next) {
                self.selected_tool_group_child = None;
            } else {
                self.selected_tool_group_child = Some(next);
            }
            self.invalidate_timeline_layout();
            return true;
        }

        let expanding = !self.expanded_tool_ids.contains(&id);
        if expanding {
            self.expanded_tool_ids.insert(id.clone());
        } else {
            self.expanded_tool_ids.remove(&id);
        }
        if let Some(index) = self.messages.iter().position(|message| message.id == id) {
            self.mark_timeline_message_and_next_dirty_at(index);
        } else {
            self.invalidate_timeline_layout();
        }
        self.tool_expand_anims.insert(
            id,
            ToolExpandAnimation {
                started_at: Instant::now(),
                expanding,
            },
        );
        true
    }

    pub fn register_link_hit_rect(&mut self, target: String, rect: [f32; 4]) {
        interaction_policy::register_hit_rect(&mut self.link_hit_rects, target, rect);
    }

    pub fn link_at(&self, x: f32, y: f32) -> Option<String> {
        interaction_policy::hit_rect_target(&self.link_hit_rects, x, y)
            .map(|(target, _)| target)
    }

    pub fn update_link_hover_at(&mut self, x: f32, y: f32) -> bool {
        let next = self.link_at(x, y);
        interaction_policy::update_hover_target(&mut self.hover_link_target, next)
    }

    pub fn link_hovered(&self, target: &str) -> bool {
        self.hover_link_target.as_deref() == Some(target)
    }

    pub fn mermaid_raw_mode(&self, key: u64) -> bool {
        self.mermaid_raw_blocks.contains(&key)
    }

    pub fn toggle_mermaid_raw_mode(&mut self, key: u64) -> bool {
        if !self.mermaid_raw_blocks.insert(key) {
            self.mermaid_raw_blocks.remove(&key);
        }
        self.invalidate_timeline_layout();
        true
    }

    pub fn link_hover_active(&self) -> bool {
        self.hover_link_target.is_some()
    }

}
