use super::*;

impl NeoismAgentPane {
    pub fn clear_tool_hit_rects(&mut self) {
        self.tool_hit_rects.clear();
        self.diff_scroll_rects.clear();
        self.markdown_horizontal_scroll_rects.clear();
        self.markdown_horizontal_scrollbars.clear();
        self.link_hit_rects.clear();
        // Retain the Vec + String allocations; reset logical length only.
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

    /// `Some` means the pointer is over a horizontally-scrollable Markdown
    /// block. The boolean reports whether its offset actually moved.
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
        if let Some(viewport) = self.timeline_viewport_rect {
            self.timeline_view_anchor = Some(TimelineViewAnchor {
                message_id: parent_id.to_string(),
                screen_offset: rect[1] - viewport[1],
            });
        }
        if let Some(index) = self
            .messages
            .iter()
            .position(|message| message.id == parent_id)
        {
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
