use super::*;

impl NeoismAgentPane {
    /// The compact composer form for a prompt the server echoes back
    /// expanded. Canonicalizing every inbound user text through this
    /// keeps ONE transcript bubble instead of a token + expanded
    /// duplicate pair. Mirrors the desktop pane's helper of the same
    /// name (`desktop/src/neoism/agent/pane/ingest.rs`).
    pub(in crate::panels::agent_pane::state) fn compact_user_prompt_text(
        &self,
        text: &str,
    ) -> Option<String> {
        let trimmed = text.trim();
        self.prompt_echo_aliases
            .iter()
            .rev()
            .find(|(expanded, _)| expanded == trimmed)
            .map(|(_, echo)| echo.clone())
    }

    pub(in crate::panels::agent_pane::state) fn compact_inbound_user_texts(
        &self,
        mut messages: Vec<NeoismAgentMessage>,
    ) -> Vec<NeoismAgentMessage> {
        if self.prompt_echo_aliases.is_empty() {
            return messages;
        }
        for message in &mut messages {
            if message.kind == NeoismAgentMessageKind::User {
                if let Some(echo) = self.compact_user_prompt_text(&message.text) {
                    message.text = echo;
                }
            }
        }
        messages
    }

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
        let mut consumed_server = vec![false; server_messages.len()];
        let mut consumed_previous = vec![false; previous_messages.len()];

        for prompt in pending {
            if let Some(server_index) =
                server_messages
                    .iter()
                    .enumerate()
                    .position(|(index, message)| {
                        !consumed_server[index] && is_user_prompt(message, &prompt)
                    })
            {
                consumed_server[server_index] = true;
                if let Some(previous_index) = previous_messages
                    .iter()
                    .enumerate()
                    .position(|(index, message)| {
                        !consumed_previous[index] && is_user_prompt(message, &prompt)
                    })
                {
                    consumed_previous[previous_index] = true;
                }
                continue;
            }
            let previous_index =
                previous_messages
                    .iter()
                    .enumerate()
                    .position(|(index, message)| {
                        !consumed_previous[index] && is_user_prompt(message, &prompt)
                    });
            let message = previous_index
                .map(|index| {
                    consumed_previous[index] = true;
                    previous_messages[index].clone()
                })
                .unwrap_or_else(|| NeoismAgentMessage::user(prompt.clone()));
            inserts.push((previous_index.unwrap_or(server_messages.len()), message));
            unresolved.push(prompt);
        }

        inserts.sort_by_key(|(index, _)| *index);
        for (offset, (index, message)) in inserts.into_iter().enumerate() {
            server_messages.insert((index + offset).min(server_messages.len()), message);
        }
        self.pending_user_prompts = unresolved;
        server_messages
    }

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

    /// Keep every landed background-task completion card through a
    /// snapshot replacement. The live `session.background_task.completed`
    /// event injects the card immediately, but the server only persists its
    /// equivalent (the `msg_background_completion_{job}` runtime prompt,
    /// mapped back to the SAME card id by `api_mapping`) once the queued
    /// notification prompt drains — a history refresh inside that window
    /// would silently wipe the card. Re-insert any copy the snapshot lacks
    /// at its chronological position; when the snapshot DOES carry the
    /// server copy (same id, or same job id) it replaces the client copy —
    /// no duplicate. Mirrors the desktop pane's helper of the same name
    /// (`desktop/src/neoism/agent/pane/ingest.rs`).
    pub(in crate::panels::agent_pane::state) fn preserve_background_completion_cards(
        &self,
        mut server_messages: Vec<NeoismAgentMessage>,
    ) -> Vec<NeoismAgentMessage> {
        for (index, existing) in self.messages.iter().enumerate() {
            if !is_background_completion_card(existing) {
                continue;
            }
            let job_id = background_job_id_from_message(existing);
            let matching_index = server_messages.iter().position(|incoming| {
                incoming.id == existing.id
                    || (job_id.is_some()
                        && background_completion_job_id_from_message(incoming) == job_id)
            });
            let card = match matching_index {
                Some(matching_index)
                    if server_messages[matching_index].id == existing.id
                        || is_background_completion_card(&server_messages[matching_index]) =>
                {
                    server_messages.remove(matching_index)
                }
                Some(_) => continue,
                None => existing.clone(),
            };
            let insert_at = background_completion_anchor_index(
                &self.messages,
                index,
                &server_messages,
            );
            server_messages.insert(insert_at, card);
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
        ) && !is_background_completion_card(&message)
        {
            // Open the live-trace window for real turn output, but NOT for
            // the background-task completion card. That card lands in an
            // already-settled session and is mask-exempt (it shows either
            // way), so revealing for it only un-hid the whole previous
            // turn's trace, which the next idle refresh then re-masked -
            // rows appearing and vanishing while the user watched.
            //
            // This must NOT be gated on `is_streaming()`: the web host's
            // `MessageUpdated` path calls `upsert_part_message` and nothing
            // else, so this is web's ONLY opener of the window. Desktop
            // additionally calls `note_streaming_from_part` right after.
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
        // Text and image fragments are broadcast independently. Fold both
        // into the optimistic local card, including the image-first ordering
        // where a server-id row already exists before text arrives.
        if message.kind == NeoismAgentMessageKind::User && !message.id.is_empty() {
            let optimistic_index = (!message.text.trim().is_empty())
                .then(|| {
                    self.messages.iter().rposition(|existing| {
                        existing.kind == NeoismAgentMessageKind::User
                            && existing.id.is_empty()
                            && existing.text.trim() == message.text.trim()
                    })
                })
                .flatten();
            let server_index = self
                .messages
                .iter()
                .position(|existing| existing.id == message.id);
            match (optimistic_index, server_index) {
                (Some(optimistic_index), Some(server_index))
                    if optimistic_index != server_index =>
                {
                    let server_fragment = self.messages.remove(server_index);
                    let optimistic_index = optimistic_index
                        .saturating_sub(usize::from(server_index < optimistic_index));
                    let merged = merge_part_message(
                        self.messages[optimistic_index].clone(),
                        server_fragment,
                    );
                    self.messages[optimistic_index] = merge_part_message(merged, message);
                    self.rebase_current_turn_trace();
                    self.invalidate_timeline_layout();
                    return;
                }
                (Some(index), _) => {
                    self.messages[index] =
                        merge_part_message(self.messages[index].clone(), message);
                    self.mark_timeline_message_and_next_dirty_at(index);
                    return;
                }
                (None, Some(index)) => {
                    self.messages[index] =
                        merge_part_message(self.messages[index].clone(), message);
                    self.mark_timeline_message_and_next_dirty_at(index);
                    return;
                }
                (None, None) => {}
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
        if is_background_completion_card(&message) {
            let live_kind = match self.streaming_state {
                NeoismAgentStreamingState::Generating => {
                    Some(NeoismAgentMessageKind::Assistant)
                }
                NeoismAgentStreamingState::Thinking => {
                    Some(NeoismAgentMessageKind::Reasoning)
                }
                _ => None,
            };
            if let Some(index) = live_kind.and_then(|kind| {
                self.messages
                    .iter()
                    .rposition(|existing| existing.kind == kind)
            }) {
                self.messages.insert(index, message);
                self.invalidate_timeline_layout();
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

    pub(in crate::panels::agent_pane::state) fn match_running_tool_part(
        &self,
        message: &NeoismAgentMessage,
    ) -> Option<usize> {
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
    pub(in crate::panels::agent_pane::state) fn move_previous_assistant_after_reasoning(
        &mut self,
        index: usize,
    ) {
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

/// The durable background-task completion card (`api_mapping`'s
/// `background_completion_card`). It reports work that finished while the
/// user was elsewhere, is exempt from the timeline visibility mask, and
/// must not drag the whole settled turn back into view with it.
fn is_background_completion_card(message: &NeoismAgentMessage) -> bool {
    message.tool == "background_task_result" && message.id.starts_with("background-task-")
}

fn background_completion_anchor_index(
    local_messages: &[NeoismAgentMessage],
    local_index: usize,
    server_messages: &[NeoismAgentMessage],
) -> usize {
    if let Some(position) = local_messages[..local_index]
        .iter()
        .rev()
        .filter(|message| !message.id.is_empty())
        .find_map(|prior| {
            server_messages
                .iter()
                .position(|incoming| incoming.id == prior.id)
        })
    {
        return position + 1;
    }
    local_messages[local_index.saturating_add(1)..]
        .iter()
        .filter(|message| !message.id.is_empty())
        .find_map(|following| {
            server_messages
                .iter()
                .position(|incoming| incoming.id == following.id)
        })
        .unwrap_or(server_messages.len())
}
