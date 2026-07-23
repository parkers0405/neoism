use super::*;

impl NeoismAgentPane {
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
        tool_archived: bool,
    ) -> TimelineMeasureKey {
        TimelineMeasureKey {
            id: hash_value(&message.id),
            kind: message.kind,
            output_kind: message.output_kind,
            width_bucket: f32_measure_bucket(width),
            scale_bucket: f32_measure_bucket(scale),
            tool_expanded,
            tool_archived,
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
        tool_archived: bool,
        selected_tool_group_child: Option<&str>,
    ) -> TimelineMeasureKey {
        let mut key = Self::timeline_measure_key(
            message,
            width,
            scale,
            tool_expanded,
            tool_archived,
        );
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
        // High cap so a long paginated transcript stays fully measured — the
        // wholesale clear is a re-measure cliff, kept out of reach for
        // realistic session sizes (entries are a key + an f32).
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

    pub(in crate::panels::agent_pane::state) fn next_markdown_blocks_tick(&self) -> u64 {
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

    pub(crate) fn take_timeline_layout_cache(&self) -> Option<TimelineLayoutCache> {
        self.timeline_layout_cache.borrow_mut().take()
    }

    pub(crate) fn store_timeline_layout_cache(&self, cache: TimelineLayoutCache) {
        *self.timeline_layout_cache.borrow_mut() = Some(cache);
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

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn background_sender(&self) {}

    /// Record a pending outbound IO request. Cheap; the host drains the
    /// queue via `drain_pending_outbound` between event cycles.
    pub(in crate::panels::agent_pane::state) fn push_outbound(
        &mut self,
        command: OutboundAgentCommand,
    ) {
        self.pending_outbound.push_back(command);
    }

    /// Drain every queued outbound command. The host is expected to
    /// translate each command into the right IO action (HTTP request
    /// on desktop, `AgentClientMessage` envelope on web). Returns an
    /// owned `Vec` rather than a `Drain` iterator so the host can keep
    /// other `&mut` borrows on the pane while it processes commands
    /// (e.g. pushing notices on failure).
    pub fn drain_pending_outbound(&mut self) -> Vec<OutboundAgentCommand> {
        self.pending_outbound.drain(..).collect()
    }

    /// Cheap "is there work for the host to do?" probe. Useful for
    /// avoiding the `Vec` allocation in `drain_pending_outbound` when
    /// the queue is empty (the hot path).
    pub fn has_pending_outbound(&self) -> bool {
        !self.pending_outbound.is_empty()
    }

    pub fn running_background_task_count(&self) -> usize {
        running_background_task_count(&self.messages)
    }

    pub(in crate::panels::agent_pane::state) fn running_background_task_started_at(
        &self,
    ) -> Option<Instant> {
        self.background_tasks_started_at
    }

    pub(in crate::panels::agent_pane::state) fn refresh_background_task_activity_clock(
        &mut self,
    ) {
        if self.running_background_task_count() > 0 {
            if self.background_tasks_started_at.is_none() {
                self.background_tasks_started_at = Some(Instant::now());
            }
        } else {
            self.background_tasks_started_at = None;
        }
    }

    pub(in crate::panels::agent_pane::state) fn apply_config_defaults(&mut self) {
        // In-memory: nothing to mutate without the response. Record the
        // request so the host can fetch config defaults and feed the
        // result back via the snapshot / setters.
        self.push_outbound(OutboundAgentCommand::ApplyConfigDefaults);
    }

    pub fn cursor_rect(&self) -> Option<[f32; 4]> {
        self.cursor_rect
    }

    pub fn cursor_byte(&self) -> usize {
        self.cursor_byte.min(self.input.len())
    }

    pub(in crate::panels::agent_pane::state) fn input_buffer(&self) -> AgentInputBuffer {
        AgentInputBuffer::new(
            self.input.clone(),
            self.cursor_byte,
            self.sent_history.clone(),
            self.history_index,
            self.history_draft.clone(),
        )
        .with_goal_x(self.input_goal_x)
    }

    pub(in crate::panels::agent_pane::state) fn apply_input_buffer(
        &mut self,
        buffer: AgentInputBuffer,
    ) {
        self.input = buffer.input;
        self.cursor_byte = buffer.cursor_byte;
        self.sent_history = buffer.sent_history;
        self.history_index = buffer.history_index;
        self.history_draft = buffer.history_draft;
        self.input_goal_x = buffer.goal_x;
    }

    pub fn set_cursor_rect(&mut self, rect: Option<[f32; 4]>) {
        self.cursor_rect = rect;
    }

    pub fn set_input_wrap_rows(&mut self, rows: Vec<InputWrapRow>) {
        self.input_wrap_len = self.input.len();
        self.input_wrap_rows = rows;
    }

    /// Wrap rows registered by the renderer, but only if they still
    /// describe the current input (a keystroke can land between edit
    /// and redraw).
    pub(in crate::panels::agent_pane::state) fn current_input_wrap_rows(
        &self,
    ) -> Option<&[InputWrapRow]> {
        (!self.input_wrap_rows.is_empty() && self.input_wrap_len == self.input.len())
            .then_some(self.input_wrap_rows.as_slice())
    }
}
