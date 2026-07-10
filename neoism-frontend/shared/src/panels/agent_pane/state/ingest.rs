use super::*;

impl NeoismAgentPane {
    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn merge_pending_user_prompts(
        &mut self,
        mut server_messages: Vec<NeoismAgentMessage>,
    ) -> Vec<NeoismAgentMessage> {
        if self.pending_user_prompts.is_empty() {
            return server_messages;
        }

        let previous_messages = self.messages.clone();
        let pending = std::mem::take(&mut self.pending_user_prompts);
        let mut unresolved = Vec::new();
        let mut inserts = Vec::new();

        for prompt in pending {
            if let Some(server_index) = server_messages
                .iter()
                .position(|message| is_user_prompt(message, &prompt))
            {
                if let Some(previous_index) = previous_messages
                    .iter()
                    .rposition(|message| is_user_prompt(message, &prompt))
                    .filter(|previous_index| *previous_index < server_index)
                {
                    let message = server_messages.remove(server_index);
                    server_messages
                        .insert(previous_index.min(server_messages.len()), message);
                }
                continue;
            }
            let previous_index = previous_messages
                .iter()
                .rposition(|message| is_user_prompt(message, &prompt))
                .unwrap_or(server_messages.len());
            inserts.push((previous_index, NeoismAgentMessage::user(prompt.clone())));
            unresolved.push(prompt);
        }

        inserts.sort_by_key(|(index, _)| *index);
        for (offset, (index, message)) in inserts.into_iter().enumerate() {
            server_messages.insert((index + offset).min(server_messages.len()), message);
        }
        self.pending_user_prompts = unresolved;
        server_messages
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn preserve_streamed_response_text(
        &self,
        mut server_messages: Vec<NeoismAgentMessage>,
    ) -> Vec<NeoismAgentMessage> {
        for incoming in &mut server_messages {
            if !is_streamed_live_part(incoming) {
                continue;
            }
            let Some(existing) = self
                .messages
                .iter()
                .find(|existing| same_streamed_part_identity(existing, incoming))
            else {
                continue;
            };
            *incoming = merge_part_message(existing.clone(), incoming.clone());
        }

        server_messages
    }

    pub fn apply_part_delta(
        &mut self,
        message_id: Option<String>,
        part_id: Option<String>,
        kind: Option<String>,
        delta: &str,
    ) {
        if delta.is_empty() {
            return;
        }
        if matches!(kind.as_deref(), Some("reasoning" | "thinking")) {
            self.retain_current_turn_trace();
        }
        if let Some(message_id) = message_id.as_deref().filter(|id| !id.is_empty()) {
            if let Some(index) = self
                .messages
                .iter()
                .position(|message| message.id == message_id)
            {
                self.messages[index].text.push_str(delta);
                self.mark_timeline_message_dirty_at(index);
                return;
            }
        }
        if let Some(part_id) = part_id.as_deref().filter(|id| !id.is_empty()) {
            if let Some(index) = self
                .messages
                .iter()
                .position(|message| message.id == part_id)
            {
                self.messages[index].text.push_str(delta);
                self.mark_timeline_message_dirty_at(index);
                return;
            }
            let message = match kind.as_deref() {
                Some("reasoning" | "thinking") => {
                    NeoismAgentMessage::reasoning(delta).with_id(part_id.to_string())
                }
                _ => NeoismAgentMessage::assistant(delta).with_id(part_id.to_string()),
            };
            self.upsert_part_message(message);
            return;
        }

        let message_kind = part_delta_message_kind(kind.as_deref());
        if let Some(index) = self
            .messages
            .iter()
            .rposition(|message| message.kind == message_kind)
        {
            self.messages[index].text.push_str(delta);
            self.mark_timeline_message_dirty_at(index);
            return;
        }

        self.messages.push(match message_kind {
            NeoismAgentMessageKind::Reasoning => NeoismAgentMessage::reasoning(delta),
            _ => NeoismAgentMessage::assistant(delta),
        });
        self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
    }

    pub fn start_compaction_message(&mut self, _id: String, reason: String) {
        let _ = reason;
    }

    pub fn apply_compaction_delta(&mut self, delta: &str) {
        let _ = delta;
    }

    pub fn finish_compaction_message(&mut self, summary: &str, kind: &str) {
        let _ = (summary, kind);
    }

    pub fn upsert_part_message(&mut self, message: NeoismAgentMessage) {
        if matches!(
            message.kind,
            NeoismAgentMessageKind::Reasoning
                | NeoismAgentMessageKind::Tool
                | NeoismAgentMessageKind::Subtask
                | NeoismAgentMessageKind::Compaction
        ) {
            self.retain_current_turn_trace();
        }
        if message.kind == NeoismAgentMessageKind::Assistant
            && message.text.is_empty()
            && !message.id.is_empty()
            && self
                .messages
                .iter()
                .any(|existing| existing.id == message.id)
        {
            return;
        }
        // The agent-server echoes user-prompt parts to every attached
        // client so other devices see the prompt live. On the SENDING
        // device the prompt is already in the timeline as the
        // optimistic local copy (pushed with an empty id at submit
        // time) — adopt the server id onto that copy instead of
        // appending a duplicate bubble.
        if message.kind == NeoismAgentMessageKind::User && !message.id.is_empty() {
            if let Some(index) = self.messages.iter().position(|existing| {
                existing.kind == NeoismAgentMessageKind::User
                    && existing.id.is_empty()
                    && existing.text.trim() == message.text.trim()
            }) {
                self.messages[index].id = message.id;
                self.mark_timeline_message_dirty_at(index);
                return;
            }
        }
        if !message.id.is_empty() {
            if let Some(index) = self
                .messages
                .iter()
                .position(|existing| existing.id == message.id)
            {
                let merged = merge_part_message(self.messages[index].clone(), message);
                self.messages[index] = merged;
                if self.messages[index].kind == NeoismAgentMessageKind::Reasoning {
                    self.move_previous_assistant_after_reasoning(index);
                } else {
                    self.mark_timeline_message_and_next_dirty_at(index);
                }
                return;
            }
        }
        if let Some(index) = self.match_running_tool_part(&message) {
            let merged = merge_part_message(self.messages[index].clone(), message);
            self.messages[index] = merged;
            self.mark_timeline_message_and_next_dirty_at(index);
            return;
        }
        if message.kind == NeoismAgentMessageKind::Reasoning {
            if self
                .messages
                .iter()
                .any(|existing| existing.kind == NeoismAgentMessageKind::Assistant)
            {
                self.messages.push(message);
                self.move_previous_assistant_after_reasoning(
                    self.messages.len().saturating_sub(1),
                );
                self.invalidate_timeline_layout();
                return;
            }
        }
        self.messages.push(message);
        self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
    }

    pub(in crate::panels::agent_pane::state) fn match_running_tool_part(&self, message: &NeoismAgentMessage) -> Option<usize> {
        if message.kind != NeoismAgentMessageKind::Tool
            || message.status == "running"
            || message.status == "pending"
            || message.tool.is_empty()
        {
            return None;
        }
        let mut matches = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, existing)| {
                existing.kind == NeoismAgentMessageKind::Tool
                    && existing.status == "running"
                    && existing.tool == message.tool
                    && (message.title.is_empty()
                        || existing.title.is_empty()
                        || existing.title == message.title)
            })
            .map(|(index, _)| index);
        let index = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(index)
    }

    /// When a reasoning part lands at `index` we *may* need to pull a
    /// just-opened assistant placeholder back below it so the model's
    /// thinking renders above the answer it produced. This is only safe
    /// for an *empty* assistant part — a provider that opens the turn with
    /// a blank text part before streaming its reasoning.
    ///
    /// A non-empty assistant part is a *completed answer* (or one already
    /// streaming visible text). The stream is chronological, so an answer
    /// that finished before this reasoning started must stay above it —
    /// reordering it here is the "finished answer drops below a later
    /// thinking block" bug. We keep insertion order for those and never
    /// move them.
    pub(in crate::panels::agent_pane::state) fn move_previous_assistant_after_reasoning(&mut self, index: usize) {
        let turn_start = self.messages[..index]
            .iter()
            .rposition(|message| message.kind == NeoismAgentMessageKind::User)
            .map(|user_index| user_index + 1)
            .unwrap_or(0);
        let Some(assistant_index) = self.messages[turn_start..index]
            .iter()
            .rposition(|message| {
                // Only an *empty* placeholder answer is eligible — a
                // streamed/finished answer keeps its chronological slot.
                message.kind == NeoismAgentMessageKind::Assistant
                    && message.text.is_empty()
            })
            .map(|relative_index| turn_start + relative_index)
        else {
            self.mark_timeline_message_and_next_dirty_at(index);
            return;
        };
        let assistant = self.messages.remove(assistant_index);
        let reasoning_index = index.saturating_sub(1);
        self.messages.insert(reasoning_index + 1, assistant);
        self.invalidate_timeline_layout();
    }

    pub fn remove_part_message(&mut self, part_id: &str) {
        if part_id.is_empty() {
            return;
        }
        let before = self.messages.len();
        self.messages.retain(|message| message.id != part_id);
        if self.messages.len() != before {
            self.invalidate_timeline_layout();
        }
    }

}
