use super::*;

impl NeoismAgentPane {
    pub fn commit_picker(&mut self) -> bool {
        let Some(picker) = self.picker.take() else {
            return false;
        };
        let Some(option) = picker.selected_option().cloned() else {
            return true;
        };
        match picker.kind {
            NeoismAgentPickerKind::Slash => {
                self.input.clear();
                self.execute_slash_text(&option.value);
            }
            NeoismAgentPickerKind::Agent => self.apply_agent(option.value),
            NeoismAgentPickerKind::Model => {
                self.remember_model_option(&option);
                self.apply_model(option.value)
            }
            NeoismAgentPickerKind::Thinking => self.apply_thinking(option.value),
            NeoismAgentPickerKind::Session | NeoismAgentPickerKind::Subagent => {
                self.switch_session(option.value);
            }
            NeoismAgentPickerKind::Skill => self.apply_skill_mention(option),
            NeoismAgentPickerKind::SkillMention => {
                self.apply_inline_skill_mention(option)
            }
            NeoismAgentPickerKind::FileMention => self.apply_file_mention(option.value),
        }
        true
    }

    pub fn submit(&mut self) -> bool {
        if self.commit_picker() {
            return true;
        }
        if self.is_subagent_session() {
            return false;
        }
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return false;
        }
        let prompt = self.expand_text_attachments(&text);
        self.input.clear();
        self.cursor_byte = 0;
        self.history_index = None;
        self.file_mention_anchor = None;
        if text.starts_with('/') {
            self.input_attachments.clear();
            self.execute_slash_text(&text);
            return true;
        }
        self.remember_sent_prompt(&text);
        let was_streaming = self.is_streaming();
        self.abort_requested_at = None;
        // For a fresh run, show activity immediately. During an active run,
        // keep the current state and show the queued-message line instead.
        if !was_streaming {
            self.note_streaming(NeoismAgentStreamingState::Thinking, None);
        }
        let send_result = self.send_prompt_with_echo(&prompt, &text, !was_streaming);
        self.input_attachments.clear();
        match send_result {
            Ok(()) if was_streaming => {
                self.queued_prompt_count =
                    self.queued_prompt_count.saturating_add(1).max(1);
                self.queued_prompt_preview.get_or_insert(prompt);
            }
            Ok(()) => {}
            Err(error) => {
                self.system_message("Prompt failed", error);
                if !was_streaming {
                    self.note_streaming(NeoismAgentStreamingState::Idle, None);
                }
            }
        }
        true
    }

    pub(in crate::panels::agent_pane::state) fn sync_input_pickers(&mut self) {
        self.sync_slash_picker();
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Slash)
        {
            self.file_mention_anchor = None;
            return;
        }
        self.sync_skill_mention_picker();
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention)
        {
            return;
        }
        self.sync_file_mention_picker();
    }

    pub(in crate::panels::agent_pane::state) fn sync_skill_mention_picker(&mut self) {
        let Some((anchor, query)) = self.active_prefixed_token('$') else {
            if self
                .picker
                .as_ref()
                .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention)
            {
                self.picker = None;
            }
            return;
        };
        self.file_mention_anchor = Some(anchor);
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::SkillMention)
        {
            picker.replace_options(self.skill_options.clone());
        } else {
            self.picker = Some(NeoismAgentPicker::new(
                NeoismAgentPickerKind::SkillMention,
                "Skills",
                self.skill_options.clone(),
                0,
            ));
        }
        self.set_picker_query(query);
    }

    pub(in crate::panels::agent_pane::state) fn sync_slash_picker(&mut self) {
        if !self.input.starts_with('/') || self.input.contains(char::is_whitespace) {
            if self
                .picker
                .as_ref()
                .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Slash)
            {
                self.picker = None;
            }
            return;
        }
        if self
            .picker
            .as_ref()
            .is_none_or(|picker| picker.kind != NeoismAgentPickerKind::Slash)
        {
            self.picker = Some(NeoismAgentPicker::new(
                NeoismAgentPickerKind::Slash,
                "Commands",
                slash_options(),
                0,
            ));
        }
        let query = self.input.trim_start_matches('/').to_string();
        self.set_picker_query(query);
    }

    pub(in crate::panels::agent_pane::state) fn sync_file_mention_picker(&mut self) {
        let Some((anchor, query)) = self.active_file_mention() else {
            self.file_mention_anchor = None;
            if self
                .picker
                .as_ref()
                .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::FileMention)
            {
                self.picker = None;
            }
            return;
        };
        self.file_mention_anchor = Some(anchor);
        let options = self.file_mention_options(&query);
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::FileMention)
        {
            picker.set_pre_filtered_options(query, options);
        } else {
            let mut picker = NeoismAgentPicker::new(
                NeoismAgentPickerKind::FileMention,
                "Files",
                Vec::new(),
                0,
            );
            picker.set_pre_filtered_options(query, options);
            self.picker = Some(picker);
        }
    }

    pub(in crate::panels::agent_pane::state) fn active_file_mention(&self) -> Option<(usize, String)> {
        self.active_prefixed_token('@')
    }

    pub(in crate::panels::agent_pane::state) fn active_prefixed_token(&self, trigger_char: char) -> Option<(usize, String)> {
        let cursor = self.cursor_byte();
        let prefix = &self.input[..cursor];
        let (trigger, _) = prefix
            .char_indices()
            .rev()
            .find(|(_, ch)| *ch == trigger_char)?;
        if trigger > 0 {
            let previous = prefix[..trigger].chars().last()?;
            if !previous.is_whitespace()
                && !matches!(previous, '(' | '[' | '{' | '"' | '\'')
            {
                return None;
            }
        }
        let query = &prefix[trigger + trigger_char.len_utf8()..];
        (!query.contains(char::is_whitespace)).then(|| (trigger, query.to_string()))
    }

    pub(in crate::panels::agent_pane::state) fn remember_sent_prompt(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if self.sent_history.last().is_none_or(|last| last != text) {
            self.sent_history.push(text.to_string());
        }
        const MAX_HISTORY: usize = 100;
        if self.sent_history.len() > MAX_HISTORY {
            let extra = self.sent_history.len() - MAX_HISTORY;
            self.sent_history.drain(0..extra);
        }
    }

    pub(in crate::panels::agent_pane::state) fn update_picker_query(&mut self, text: &str) {
        let mut query = self
            .picker
            .as_ref()
            .map(|picker| picker.query.clone())
            .unwrap_or_default();
        query.push_str(text);
        self.set_picker_query(query);
    }

    pub(in crate::panels::agent_pane::state) fn set_picker_query(&mut self, query: String) {
        if let Some(picker) = self.picker.as_mut() {
            if picker.kind == NeoismAgentPickerKind::Slash {
                self.input = format!("/{query}");
            }
            picker.set_query(query);
        }
    }

    pub(in crate::panels::agent_pane::state) fn file_mention_options(&self, _query: &str) -> Vec<NeoismAgentPickerOption> {
        Vec::new()
    }

    pub(in crate::panels::agent_pane::state) fn file_mention_root(&self) -> std::path::PathBuf {
        self.directory
            .as_deref()
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }

    pub(in crate::panels::agent_pane::state) fn apply_file_mention(&mut self, value: String) {
        let Some(anchor) = self.file_mention_anchor.take() else {
            return;
        };
        let cursor = self.cursor_byte();
        let token = format!("@{value}");
        self.input.replace_range(anchor..cursor, &token);
        self.cursor_byte = anchor.saturating_add(token.len());
        if self
            .input
            .get(self.cursor_byte()..)
            .and_then(|rest| rest.chars().next())
            .is_none_or(|ch| !ch.is_whitespace())
        {
            self.input.insert(self.cursor_byte(), ' ');
            self.cursor_byte = self.cursor_byte().saturating_add(1);
        }
        self.remember_file_mention(&token, &value);
        self.history_index = None;
    }

    pub fn system_message(&mut self, title: impl Into<String>, text: impl Into<String>) {
        self.messages.push(NeoismAgentMessage::system(title, text));
        self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
    }

    pub(in crate::panels::agent_pane::state) fn send_prompt_with_echo(
        &mut self,
        prompt: &str,
        echo_prompt: &str,
        transcript_echo: bool,
    ) -> Result<(), String> {
        let echo_prompt = echo_prompt.to_string();
        if transcript_echo {
            self.messages
                .push(NeoismAgentMessage::user(echo_prompt.clone()));
            self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
        }
        // Mirror the desktop's `commands::send_prompt`: build the
        // expanded prompt parts + skill-augmented system prompt and
        // hand both off to the host. The host is responsible for
        // ensuring a session exists before delivering the prompt — we
        // additionally fire `EnsureSession` here when we don't yet
        // have a session id so the desktop runtime / wasm bridge can
        // route the two commands in order.
        if self.session_id.is_none() {
            self.push_outbound(OutboundAgentCommand::EnsureSession);
        }
        let parts = self.prompt_parts_for(prompt);
        let system = self.prompt_system_for(prompt);
        self.push_outbound(OutboundAgentCommand::SendPrompt {
            text: prompt.to_string(),
            parts,
            system,
            agent: self.agent.clone(),
            model: self.model.clone(),
            thinking: self.thinking.clone(),
            transcript_echo,
        });
        if transcript_echo {
            self.remember_pending_user_prompt(&echo_prompt);
        }
        Ok(())
    }

    pub fn apply_agent(&mut self, value: String) {
        let trimmed = self.set_agent_local(value);
        if let Some(session_id) = self.session_id.clone() {
            self.push_outbound(OutboundAgentCommand::ApplyAgent {
                session_id,
                agent: trimmed,
            });
        }
    }

    pub(in crate::panels::agent_pane::state) fn set_agent_local(&mut self, value: String) -> String {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            self.agent = None;
        } else {
            self.agent = Some(trimmed.clone());
        }
        if trimmed == "plan" {
            self.mode = NeoismAgentMode::Plan;
        } else if trimmed == "build" {
            self.mode = NeoismAgentMode::Build;
        }
        trimmed
    }

    /// Reset to a fresh conversation — the `/new` slash behaviour.
    /// Hosts also call this when the user explicitly re-invokes
    /// "Neoism Agent" while a conversation is already showing.
    pub fn start_new_conversation(&mut self) {
        self.session_id = None;
        self.parent_session_id = None;
        self.clear_pending_user_prompts();
        self.messages.clear();
        self.invalidate_timeline_layout();
    }

    pub(in crate::panels::agent_pane::state) fn switch_session(&mut self, session_id: String) {
        let trimmed = session_id.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        self.session_id = Some(trimmed.clone());
        self.messages.clear();
        self.timeline_layout_epoch = self.timeline_layout_epoch.wrapping_add(1);
        self.push_outbound(OutboundAgentCommand::SwitchSession {
            session_id: trimmed,
        });
    }

    pub(in crate::panels::agent_pane::state) fn execute_slash_text(&mut self, text: &str) {
        // Mirrors the desktop dispatcher in
        // `frontends/neoism/src/neoism/agent/commands.rs` minus the
        // direct HTTP calls. Anything that's purely an in-memory state
        // mutation runs inline; anything that needs IO records the
        // request on the outbound queue.
        let trimmed = text.trim();
        let mut parts = trimmed.split_whitespace();
        let raw = parts.next().unwrap_or_default();
        let args_vec: Vec<&str> = parts.collect();
        let args_tail = args_vec.join(" ");
        match raw {
            // -- pure in-memory ----------------------------------------
            "/clear" => {
                self.messages.clear();
                self.timeline_layout_epoch = self.timeline_layout_epoch.wrapping_add(1);
            }
            "/exit" => self.request_close_tab(),
            "/new" => self.start_new_conversation(),

            // -- picker openers / arg-setters --------------------------
            //
            // These match the desktop's dispatch shape: if the user passed
            // an argument we apply it directly (which itself may push an
            // outbound command — e.g. `apply_model` doesn't here but
            // `switch_session` does); without an argument we open the
            // matching picker. Picker open is pure UI.
            "/model" => {
                if let Some(model) = args_vec.first() {
                    self.apply_model((*model).to_string());
                } else {
                    self.open_model_picker();
                }
            }
            "/think" | "/reasoning" => {
                if let Some(value) = args_vec.first() {
                    self.apply_thinking((*value).to_string());
                } else {
                    self.open_thinking_picker();
                }
            }
            "/agent" => {
                if let Some(agent) = args_vec.first() {
                    self.apply_agent((*agent).to_string());
                } else {
                    self.open_agent_picker();
                }
            }
            "/sessions" | "/session" => {
                if let Some(id) = args_vec.first() {
                    self.switch_session((*id).to_string());
                } else {
                    self.open_sessions_picker();
                }
            }
            "/sub-agent" | "/subagents" | "/sub" => self.open_subagent_picker(),
            "/skill" | "/skills" => {
                if args_vec.is_empty() {
                    self.open_skill_picker();
                } else {
                    // Anything beyond a bare `/skill` (info / list / a
                    // skill name) needs the host to talk to the
                    // agent-server: defer to the slash-command queue.
                    self.push_outbound(OutboundAgentCommand::SlashCommand {
                        name: raw.trim_start_matches('/').to_string(),
                        args: args_tail,
                    });
                }
            }

            // -- session-level IO routed through dedicated commands ----
            "/compact" | "/compaction" | "/comapction" => self.compact_session(),
            "/abort" => self.abort_session(),

            // -- everything else needs the daemon: queue it ------------
            other if other.starts_with('/') => {
                self.push_outbound(OutboundAgentCommand::SlashCommand {
                    name: other.trim_start_matches('/').to_string(),
                    args: args_tail,
                });
            }
            _ => {}
        }
    }

    pub fn apply_model(&mut self, value: String) {
        self.remember_model_value(&value);
        self.set_model_local(value.clone());
        if let Some(session_id) = self.session_id.clone() {
            self.push_outbound(OutboundAgentCommand::ApplyModel {
                session_id,
                model: crate::panels::agent_pane::api_mapping::session_model_json(
                    &value,
                    self.thinking.as_deref(),
                )
                .unwrap_or_else(|| serde_json::Value::String(value)),
            });
        }
        self.refresh_model_context_limit();
    }

    pub(in crate::panels::agent_pane::state) fn set_model_local(&mut self, value: String) {
        self.model = value;
    }

    pub(in crate::panels::agent_pane::state) fn remember_model_value(&mut self, value: &str) {
        if value.trim().is_empty() {
            return;
        }
        if let Some(option) = self
            .model_options
            .iter()
            .chain(self.recent_model_options.iter())
            .find(|option| option.value == value && option.is_selectable())
            .cloned()
        {
            self.remember_model_option(&option);
            return;
        }
        let title = value
            .split_once('/')
            .map(|(_, model)| model)
            .unwrap_or(value);
        let provider = value
            .split_once('/')
            .map(|(provider, _)| provider)
            .unwrap_or("");
        let option = NeoismAgentPickerOption::new(title, provider, "", value);
        self.remember_model_option(&option);
    }

    pub(in crate::panels::agent_pane::state) fn remember_model_option(&mut self, option: &NeoismAgentPickerOption) {
        if !option.is_selectable() || option.value.trim().is_empty() {
            return;
        }
        let mut recent = option.clone();
        recent.is_header = false;
        if recent.description.is_empty() && !recent.section.is_empty() {
            recent.description = recent.section.clone();
        }
        recent.section = "Recent".to_string();
        self.recent_model_options
            .retain(|existing| existing.value != recent.value);
        self.recent_model_options.insert(0, recent);
        self.recent_model_options.truncate(8);
    }

    pub fn apply_thinking(&mut self, value: String) {
        let thinking = self.set_thinking_local(value);
        if let Some(session_id) = self.session_id.clone() {
            self.push_outbound(OutboundAgentCommand::ApplyThinking {
                session_id,
                model: self.model.clone(),
                thinking,
            });
        }
    }

    pub(in crate::panels::agent_pane::state) fn set_thinking_local(&mut self, value: String) -> Option<String> {
        let thinking = (!value.trim().is_empty()).then_some(value);
        self.thinking = thinking.clone();
        thinking
    }

    pub(in crate::panels::agent_pane::state) fn abort_session(&mut self) {
        self.abort_requested_at = Some(Instant::now());
        self.note_streaming(NeoismAgentStreamingState::Idle, None);
        // Without a session id there's nothing for the host to abort;
        // mirrors desktop `commands::abort_session`'s early return.
        if self.session_id.is_some() {
            self.push_outbound(OutboundAgentCommand::AbortSession);
        }
    }

    pub(in crate::panels::agent_pane::state) fn compact_session(&mut self) {
        // No session yet → nothing to compact (matches desktop).
        if self.session_id.is_none() {
            return;
        }
        // The host turns the `CompactSession` outbound command into the
        // actual `/api/session/.../compact` POST. The pane enters Compacting
        // only after the backend emits the same compaction.started event used
        // by auto compaction.
        self.push_outbound(OutboundAgentCommand::CompactSession);
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane) fn insert_skill_mention_by_name(&mut self, name: String) {
        self.apply_skill_mention(NeoismAgentPickerOption::new(&name, "", "skill", &name));
    }

    pub(in crate::panels::agent_pane::state) fn apply_skill_mention(&mut self, option: NeoismAgentPickerOption) {
        if self.is_subagent_session() {
            return;
        }
        let name = if option.value.trim().is_empty() {
            option.title.trim().to_string()
        } else {
            option.value.trim().to_string()
        };
        if name.is_empty() {
            return;
        }
        let token = format!("${name}");
        self.close_picker();
        self.insert_input_token(&token);
        self.input_attachments
            .retain(|attachment| attachment.token() != token);
        self.input_attachments
            .push(NeoismAgentInputAttachment::Skill {
                token,
                name,
                description: option.description,
            });
        self.history_index = None;
        self.sync_input_pickers();
    }

    pub(in crate::panels::agent_pane::state) fn apply_inline_skill_mention(&mut self, option: NeoismAgentPickerOption) {
        let Some(anchor) = self.file_mention_anchor.take() else {
            self.apply_skill_mention(option);
            return;
        };
        if self.is_subagent_session() {
            return;
        }
        let name = if option.value.trim().is_empty() {
            option.title.trim().trim_start_matches('$').to_string()
        } else {
            option.value.trim().trim_start_matches('$').to_string()
        };
        if name.is_empty() {
            return;
        }
        let token = format!("${name}");
        let cursor = self.cursor_byte();
        self.input.replace_range(anchor..cursor, &token);
        self.cursor_byte = anchor.saturating_add(token.len());
        if self
            .input
            .get(self.cursor_byte()..)
            .and_then(|rest| rest.chars().next())
            .is_none_or(|ch| !ch.is_whitespace())
        {
            self.input.insert(self.cursor_byte(), ' ');
            self.cursor_byte = self.cursor_byte().saturating_add(1);
        }
        self.input_attachments
            .retain(|attachment| attachment.token() != token);
        self.input_attachments
            .push(NeoismAgentInputAttachment::Skill {
                token,
                name,
                description: option.description,
            });
        self.history_index = None;
        self.sync_input_pickers();
    }

    pub(in crate::panels::agent_pane::state) fn remember_file_mention(&mut self, token: &str, value: &str) {
        let root = self.file_mention_root();
        let path = root.join(value.trim_end_matches('/'));
        if !path.is_file() {
            return;
        }
        let mime = input_controller::mime_for_path(&path);
        self.input_attachments
            .retain(|attachment| attachment.token() != token);
        self.input_attachments
            .push(NeoismAgentInputAttachment::File {
                token: token.to_string(),
                filename: value.trim_end_matches('/').to_string(),
                url: input_controller::file_url(&path),
                mime: mime.to_string(),
            });
    }

    pub(in crate::panels::agent_pane::state) fn pasted_attachment_path(&self, _text: &str) -> Option<std::path::PathBuf> {
        None
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn display_path_for_attachment(
        &self,
        path: &std::path::Path,
        is_dir: bool,
    ) -> String {
        let root = self.file_mention_root();
        let mut display = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if is_dir && !display.ends_with('/') {
            display.push('/');
        }
        display
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn file_attachment_token(&self, path: &std::path::Path, mime: &str) -> String {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file");
        if mime.starts_with("image/") {
            let next = self.file_attachment_count(|mime| mime.starts_with("image/")) + 1;
            return format!("[image{next}]");
        }
        if mime == "application/pdf" {
            let next = self.file_attachment_count(|mime| mime == "application/pdf") + 1;
            return format!("[pdf{next}]");
        }
        let next = self.file_attachment_count(|mime| {
            !mime.starts_with("image/") && mime != "application/pdf"
        }) + 1;
        format!("[file{next}: {filename}]")
    }

    #[allow(dead_code)]
    pub(in crate::panels::agent_pane::state) fn file_attachment_count<F>(&self, mut predicate: F) -> usize
    where
        F: FnMut(&str) -> bool,
    {
        self.input_attachments
            .iter()
            .filter(|attachment| match attachment {
                NeoismAgentInputAttachment::File { mime, .. } => predicate(mime),
                NeoismAgentInputAttachment::Text { .. }
                | NeoismAgentInputAttachment::Skill { .. } => false,
            })
            .count()
    }

    pub(in crate::panels::agent_pane::state) fn unique_attachment_token(&self, base: &str) -> String {
        if !self.input.contains(base)
            && !self
                .input_attachments
                .iter()
                .any(|attachment| attachment.token() == base)
        {
            return base.to_string();
        }
        let stem = base.strip_suffix(']').unwrap_or(base);
        for index in 2.. {
            let candidate = if base.ends_with(']') {
                format!("{stem} #{index}]")
            } else {
                format!("{base} #{index}")
            };
            if !self.input.contains(&candidate)
                && !self
                    .input_attachments
                    .iter()
                    .any(|attachment| attachment.token() == candidate)
            {
                return candidate;
            }
        }
        base.to_string()
    }

    pub(in crate::panels::agent_pane) fn prompt_parts_for(&self, text: &str) -> Vec<Value> {
        let mut parts = vec![json!({ "type": "text", "text": text })];
        let mut seen = BTreeSet::new();
        for attachment in &self.input_attachments {
            let NeoismAgentInputAttachment::File {
                token,
                filename,
                url,
                mime,
            } = attachment
            else {
                continue;
            };
            if text.contains(token) && seen.insert(token.clone()) {
                parts.push(json!({
                    "type": "file",
                    "url": url,
                    "filename": filename,
                    "mime": mime,
                }));
            }
        }
        parts
    }

    pub(in crate::panels::agent_pane) fn prompt_system_for(&self, text: &str) -> Option<String> {
        let mut seen = BTreeSet::new();
        let mut output = String::new();
        for attachment in &self.input_attachments {
            let NeoismAgentInputAttachment::Skill {
                token,
                name,
                description,
            } = attachment
            else {
                continue;
            };
            if !text.contains(token) || !seen.insert(name.clone()) {
                continue;
            }
            if output.is_empty() {
                output.push_str("The user selected these skills for this request. Load each selected skill with the skill tool before applying it:");
            }
            output.push('\n');
            if description.trim().is_empty() {
                output.push_str(&format!(
                    "- {name}: call the skill tool with name \"{name}\"."
                ));
            } else {
                output.push_str(&format!(
                    "- {name}: {} Call the skill tool with name \"{name}\".",
                    description.trim()
                ));
            }
        }
        (!output.is_empty()).then_some(output)
    }

    pub(in crate::panels::agent_pane::state) fn expand_text_attachments(&self, text: &str) -> String {
        let mut expanded = text.to_string();
        for attachment in &self.input_attachments {
            let NeoismAgentInputAttachment::Text {
                token,
                text: content,
            } = attachment
            else {
                continue;
            };
            if expanded.contains(token) {
                expanded = expanded.replace(token, content);
            }
        }
        expanded
    }
}
