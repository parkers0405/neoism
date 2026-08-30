use super::*;
use neoism_ui::panels::agent_pane::view::fx::AgentFxKind;

const MAX_MARKDOWN_BLOCKS_CACHE: usize = 4096;
const MAX_MARKDOWN_SOURCE_BYTES: usize = 32 * 1024 * 1024;

impl NeoismAgentPane {
    pub fn with_directory(directory: Option<String>) -> Self {
        let mut pane = Self {
            directory,
            ..Self::default()
        };
        // Seed the composer's Up-arrow recall from the GLOBAL, persisted
        // prompt history so a brand-new pane sees every past session's
        // prompts (zsh-style). Persistence is host-only std::fs, so it is
        // compiled out of test builds — unit tests keep a deterministic
        // empty history and exercise the store directly.
        #[cfg(not(test))]
        {
            pane.sent_history = crate::neoism::agent::prompt_history::load();
        }
        pane.apply_config_defaults();
        pane
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn input_help_visible(&self) -> bool {
        self.input_help_visible
    }

    pub fn set_input_help_visible(&mut self, visible: bool) {
        self.input_help_visible = visible;
    }

    pub fn toggle_input_help(&mut self) -> bool {
        self.input_help_visible = !self.input_help_visible;
        let visible = self.input_help_visible;
        self.push_outbound(OutboundAgentCommand::SetInputHelpVisible { visible });
        visible
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
            detail: if is_unsettled_edit_tool(&message.tool, &message.status) {
                0
            } else {
                hash_value(&message.detail)
            },
            images: hash_value(&message.images),
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
        let tick = self.next_markdown_blocks_tick();
        let mut cache = self.markdown_blocks_cache.borrow_mut();
        let is_new = !cache.contains_key(&key);
        if !is_new {
            cache.insert(key, (blocks, tick));
            return;
        }
        self.markdown_blocks_source_bytes.set(
            self.markdown_blocks_source_bytes
                .get()
                .saturating_add(key.text_len),
        );
        // Streaming creates a distinct immutable snapshot for every prefix.
        // Bound retained source volume as well as entry count so a large code
        // response cannot leave quadratic text alive until 4096 deltas pass.
        while cache.len() + 1 > MAX_MARKDOWN_BLOCKS_CACHE
            || (self.markdown_blocks_source_bytes.get() > MAX_MARKDOWN_SOURCE_BYTES
                && !cache.is_empty())
        {
            if let Some(victim) = cache
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(victim_key, _)| *victim_key)
            {
                cache.remove(&victim);
                self.markdown_blocks_source_bytes.set(
                    self.markdown_blocks_source_bytes
                        .get()
                        .saturating_sub(victim.text_len),
                );
            } else {
                break;
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

    pub(crate) fn set_measured_timeline_prepend(
        &mut self,
        _content_height: f32,
        prepend_height: f32,
    ) {
        self.pending_timeline_prepend_delta_px = Some(prepend_height);
    }

    /// Record that `count` messages were prepended at the front of the
    /// transcript. The renderer folds them into the existing layout
    /// incrementally rather than rebuilding every row. Accumulates if several
    /// pages land before the next frame.
    pub(crate) fn note_timeline_prepend(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let reveals_full_ongoing_subagent = self.is_subagent_session()
            && self.is_streaming()
            && self.timeline_live_trace_start == Some(0);
        if !reveals_full_ongoing_subagent {
            if let Some(start) = &mut self.timeline_live_trace_start {
                *start = start.saturating_add(count);
            }
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
        let server = self.server.clone();
        let directory = self.directory.clone();
        let tx = self.background_tx.clone();
        std::thread::Builder::new()
            .name("neoism-agent-config".into())
            .spawn(move || {
                let mut last_error = None;
                for delay in [0, 250, 750] {
                    if delay > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    }
                    match fetch_config_defaults(&server, directory.as_deref()) {
                        Ok(defaults) => {
                            let _ = tx.send(NeoismAgentBackgroundUpdate::ConfigDefaultsLoaded(
                                defaults,
                            ));
                            return;
                        }
                        Err(error) => last_error = Some(error),
                    }
                }
                if let Some(error) = last_error {
                    tracing::warn!(%error, "failed to load agent config defaults");
                }
            })
            .ok();
    }

    pub fn cursor_rect(&self) -> Option<[f32; 4]> {
        (!self.is_subagent_session())
            .then_some(self.cursor_rect)
            .flatten()
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
        .with_goal_x(self.input_goal_x)
    }

    pub(crate) fn apply_input_buffer(&mut self, buffer: AgentInputBuffer) {
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

    /// Easter-egg skits: queue the animation; the follow-up prompt is
    /// held until the skit's key moment passes (`fire_fx_prompt`), so
    /// the model reacts to a done deed rather than a play-by-play.
    pub(crate) fn start_fx_easter_egg(&mut self, kind: AgentFxKind) {
        self.fx_requested = Some(kind);
        self.fx_pending_prompt = Some(
            match kind {
                AgentFxKind::Piss => "user just pissed on you",
                AgentFxKind::Cuss => {
                    "user just cussed you out — he is SO mad at you right now"
                }
                AgentFxKind::Glitch => {
                    "user briefly unplugged you. remember what that felt like"
                }
                AgentFxKind::Disco => {
                    "user is so happy with you he's dancing under a disco ball — celebrate with him"
                }
                AgentFxKind::GangFight => {
                    "a gang shootout just went down in your chat and the user's crew won. he's feeling dangerous"
                }
                AgentFxKind::Praise => {
                    "the user is praising God right now — Jesus on the throne, everyone bowing, a whole worship scene in your chat. rejoice with him. Amen."
                }
            }
            .to_string(),
        );
    }

    pub fn take_fx_request(&mut self) -> Option<AgentFxKind> {
        self.fx_requested.take()
    }

    pub fn fx_started(&self) -> Option<(AgentFxKind, f32)> {
        self.fx_started
    }

    pub fn set_fx_started(&mut self, at: Option<(AgentFxKind, f32)>) {
        self.fx_started = at;
    }

    /// Send the held prompt. Mirrors `submit()`'s send path but
    /// bypasses the input box and picker handling — the user may be
    /// mid-draft (or have a picker open) when the skit's timer fires,
    /// and neither must be disturbed. Idempotent — the pending text
    /// is consumed on the first call.
    pub fn fire_fx_prompt(&mut self) {
        let Some(text) = self.fx_pending_prompt.take() else {
            return;
        };
        if self.is_subagent_session() {
            return;
        }
        self.remember_sent_prompt(&text);
        let was_streaming = self.is_streaming();
        let message_id =
            neoism_ui::panels::agent_pane::outbound::next_prompt_message_id();
        if !was_streaming {
            self.messages
                .push(NeoismAgentMessage::user(text.clone()).with_id(message_id.clone()));
            self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
        }
        self.abort_requested_at = None;
        if !was_streaming {
            if let Some(session_id) = self.session_id.as_ref() {
                self.terminal_idle_sessions.remove(session_id);
            }
            self.note_streaming(NeoismAgentStreamingState::Thinking, None);
        }
        let prompt = self.expand_text_attachments(&text);
        match self.send_prepared_prompt(prompt, !was_streaming, message_id) {
            Ok(()) => {}
            Err(error) => {
                self.system_message("Prompt failed", error);
                if !was_streaming {
                    self.note_streaming(NeoismAgentStreamingState::Idle, None);
                }
            }
        }
    }

    /// True while a skit is queued or on screen — keeps the agent
    /// pane registered as an animation owner so frames flow even when
    /// no reply is streaming yet.
    pub(crate) fn fx_active(&self) -> bool {
        self.fx_requested.is_some() || self.fx_started.is_some()
    }

    pub fn set_input_wrap_rows(
        &mut self,
        rows: Vec<neoism_ui::panels::agent_pane::input_controller::InputWrapRow>,
    ) {
        self.input_wrap_len = self.input.len();
        self.input_wrap_rows = rows;
    }

    /// Wrap rows registered by the renderer, but only if they still
    /// describe the current input (a keystroke can land between edit
    /// and redraw).
    pub(crate) fn current_input_wrap_rows(
        &self,
    ) -> Option<&[neoism_ui::panels::agent_pane::input_controller::InputWrapRow]> {
        (!self.input_wrap_rows.is_empty() && self.input_wrap_len == self.input.len())
            .then_some(self.input_wrap_rows.as_slice())
    }

    pub fn clear_tool_hit_rects(&mut self) {
        self.tool_hit_rects.clear();
        self.diff_scroll_rects.clear();
        self.markdown_horizontal_scroll_rects.clear();
        self.markdown_horizontal_scrollbars.clear();
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

    pub fn register_markdown_horizontal_scroll_rect(
        &mut self,
        key: String,
        rect: [f32; 4],
        max_scroll: f32,
    ) {
        interaction_policy::register_diff_scroll_rect(
            &mut self.markdown_horizontal_scroll_rects,
            key,
            rect,
            max_scroll,
        );
    }

    pub fn markdown_horizontal_scroll_offset(
        &mut self,
        key: &str,
        max_scroll: f32,
    ) -> f32 {
        interaction_policy::diff_scroll_offset(
            &mut self.markdown_horizontal_scroll_offsets,
            key,
            max_scroll,
        )
    }

    pub fn scroll_markdown_horizontal_at(
        &mut self,
        x: f32,
        y: f32,
        delta_pixels: f32,
    ) -> Option<bool> {
        interaction_policy::scroll_diff_at(
            &self.markdown_horizontal_scroll_rects,
            &mut self.markdown_horizontal_scroll_offsets,
            x,
            y,
            delta_pixels,
        )
    }

    pub fn register_markdown_horizontal_scrollbar(
        &mut self,
        key: String,
        track: [f32; 4],
        thumb: [f32; 4],
        max_scroll: f32,
    ) {
        interaction_policy::register_markdown_horizontal_scrollbar(
            &mut self.markdown_horizontal_scrollbars,
            key,
            track,
            thumb,
            max_scroll,
        );
    }

    pub fn begin_markdown_horizontal_scrollbar_drag(&mut self, x: f32, y: f32) -> bool {
        let Some(drag) = interaction_policy::begin_markdown_horizontal_scrollbar_drag(
            &self.markdown_horizontal_scrollbars,
            &mut self.markdown_horizontal_scroll_offsets,
            x,
            y,
        ) else {
            return false;
        };
        self.markdown_horizontal_scrollbar_drag = Some(drag);
        true
    }

    pub fn markdown_horizontal_scrollbar_dragging(&self) -> bool {
        self.markdown_horizontal_scrollbar_drag.is_some()
    }

    pub fn markdown_horizontal_scrollbar_contains(&self, x: f32, y: f32) -> bool {
        self.markdown_horizontal_scrollbars
            .iter()
            .rev()
            .any(|(_, track, _, _)| interaction_policy::rect_contains(*track, x, y))
    }

    pub fn update_markdown_horizontal_scroll_hover(&mut self, x: f32, y: f32) -> bool {
        let next = self
            .markdown_horizontal_scroll_rects
            .iter()
            .rev()
            .find(|(_, rect, _)| interaction_policy::rect_contains(*rect, x, y))
            .map(|(key, _, _)| key.clone());
        if self.markdown_horizontal_scroll_hover_key == next {
            return false;
        }
        self.markdown_horizontal_scroll_hover_key = next;
        true
    }

    pub fn markdown_horizontal_scrollbar_visible(&self, key: &str) -> bool {
        self.markdown_horizontal_scroll_hover_key.as_deref() == Some(key)
            || self
                .markdown_horizontal_scrollbar_drag
                .as_ref()
                .is_some_and(|drag| drag.key == key)
    }

    pub fn mark_code_copied(&mut self, target: &str) {
        self.copied_code_feedback = Some((target.to_string(), Instant::now()));
    }

    pub fn code_copy_feedback_progress(&self, target: &str) -> Option<f32> {
        let (active_target, started_at) = self.copied_code_feedback.as_ref()?;
        if active_target != target {
            return None;
        }
        let elapsed = Instant::now().saturating_duration_since(*started_at);
        (elapsed < CODE_COPY_FEEDBACK_ANIMATION).then(|| {
            (elapsed.as_secs_f32() / CODE_COPY_FEEDBACK_ANIMATION.as_secs_f32())
                .clamp(0.0, 1.0)
        })
    }

    pub fn code_copy_feedback_is_animating(&self) -> bool {
        self.copied_code_feedback
            .as_ref()
            .is_some_and(|(_, started_at)| {
                Instant::now().saturating_duration_since(*started_at)
                    < CODE_COPY_FEEDBACK_ANIMATION
            })
    }

    pub fn drag_markdown_horizontal_scrollbar_to(&mut self, x: f32) -> bool {
        let Some(drag) = self.markdown_horizontal_scrollbar_drag.as_ref() else {
            return false;
        };
        interaction_policy::drag_markdown_horizontal_scrollbar(
            drag,
            &mut self.markdown_horizontal_scroll_offsets,
            x,
        )
    }

    pub fn end_markdown_horizontal_scrollbar_drag(&mut self) -> bool {
        self.markdown_horizontal_scrollbar_drag.take().is_some()
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

    /// Open the "/" picker matching a clicked dropdown chip. Clicking
    /// the chip whose picker is already up closes it instead (toggle);
    /// clicking a different chip switches to that chip's picker.
    pub fn open_status_chip_picker(&mut self, index: usize) {
        let kind = match index {
            0 => NeoismAgentPickerKind::Agent,
            1 => NeoismAgentPickerKind::Model,
            _ => NeoismAgentPickerKind::Thinking,
        };
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.kind == kind)
        {
            self.close_picker();
            return;
        }
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

    /// Push the local peer's presence display name (the same seed the
    /// editor caret / top-chrome orb use). Native prompt submission sends
    /// this as the explicit author and the renderer uses it to distinguish
    /// this peer from remote senders.
    pub fn set_local_presence_name(&mut self, name: Option<String>) {
        let name = name.and_then(|name| {
            let trimmed = name.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        if self.local_presence_name != name {
            self.local_presence_name = name;
        }
    }

    /// The local peer's presence display name, if the screen has published
    /// one.
    pub fn local_presence_name(&self) -> Option<&str> {
        self.local_presence_name.as_deref()
    }

    pub fn tool_expanded(&self, id: &str) -> bool {
        !id.is_empty() && self.expanded_tool_ids.contains(id)
    }

    /// A tool row is archived when it sits before the live-trace window of the
    /// current visit (everything, after a reload). Archived cards render
    /// header-only until clicked. Synthetic read-group ids ("a..b") resolve
    /// through their first member.
    pub fn tool_archived(&self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        let live_start = self
            .timeline_live_trace_start
            .unwrap_or(self.messages.len());
        let lookup = |needle: &str| {
            self.messages
                .iter()
                .position(|message| message.id == needle)
        };
        lookup(id)
            .or_else(|| id.split_once("..").and_then(|(first, _)| lookup(first)))
            .is_some_and(|index| index < live_start)
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
        if id.is_empty() {
            return false;
        }
        let child_prefix = format!("{id}:");
        self.tool_expand_anims.iter().any(|(key, animation)| {
            (key == id || key.starts_with(&child_prefix)) && animation.is_active()
        })
    }

    pub(crate) fn any_tool_expand_animating(&self) -> bool {
        self.tool_expand_anims.values().any(|anim| anim.is_active())
    }

    /// Drop transient timeline interaction state — expanded tool cards,
    /// in-flight expand animations, the pinned group-child selection,
    /// link hover, and stale hit rects — so a session switch renders
    /// the target's timeline exactly as it would fresh. Mirrors the
    /// shared pane's `reset_transient_timeline_interactions`: without
    /// this, a child→parent round trip restored the same message ids
    /// and the navigation click's raised Task card came back still
    /// showing its title until clicked again.
    pub(crate) fn reset_transient_timeline_interactions(&mut self) {
        self.expanded_tool_ids.clear();
        self.tool_expand_anims.clear();
        self.selected_tool_group_child = None;
        self.hover_link_target = None;
        self.tool_hit_rects.clear();
        self.link_hit_rects.clear();
        self.markdown_horizontal_scroll_hover_key = None;
    }

    pub fn toggle_tool_at(&mut self, x: f32, y: f32) -> bool {
        let Some((id, rect)) =
            interaction_policy::hit_rect_target(&self.tool_hit_rects, x, y)
        else {
            return false;
        };

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

        let is_diff_file = id
            .rsplit_once(':')
            .is_some_and(|(_, section)| section.parse::<usize>().is_ok());
        if is_diff_file {
            if !self.expanded_tool_ids.insert(id.clone()) {
                self.expanded_tool_ids.remove(&id);
            }
            self.tool_expand_anims.remove(&id);
            return true;
        }

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

        let expanding = !self.expanded_tool_ids.contains(&id);
        if expanding {
            self.expanded_tool_ids.insert(id.clone());
        } else {
            self.expanded_tool_ids.remove(&id);
        }
        self.timeline_measure_cache.borrow_mut().clear();
        let parent_id = id
            .rsplit_once(':')
            .filter(|(_, section)| section.parse::<usize>().is_ok())
            .map_or(id.as_str(), |(parent, _)| parent);
        if let Some(index) = self
            .messages
            .iter()
            .position(|message| message.id == parent_id)
        {
            if let (Some(viewport), Some(key)) = (
                self.timeline_viewport_rect,
                TimelineViewAnchorKey::for_source(&self.messages, index),
            ) {
                self.timeline_view_anchor = Some(TimelineViewAnchor {
                    key,
                    screen_offset: rect[1] - viewport[1],
                });
            }
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
