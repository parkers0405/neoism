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
                self.apply_inline_skill_mention(option);
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
        self.input.clear();
        self.cursor_byte = 0;
        self.history_index = None;
        self.file_mention_anchor = None;
        if text.starts_with('/') {
            self.input_attachments.clear();
            self.execute_slash_text(&text);
            return true;
        }
        if text.eq_ignore_ascii_case("exit")
            && self.session_id.is_none()
            && !self.has_conversation()
        {
            self.input_attachments.clear();
            self.request_close_tab();
            return true;
        }
        self.remember_sent_prompt(&text);
        let expanded = self.expand_text_attachments(&text);
        if expanded.trim() != text.trim() {
            self.prompt_echo_aliases
                .push((expanded.trim().to_string(), text.clone()));
            if self.prompt_echo_aliases.len() > 16 {
                self.prompt_echo_aliases.remove(0);
            }
        }
        let prompt = text.clone();
        let was_streaming = self.is_streaming();
        if !was_streaming {
            self.messages.push(NeoismAgentMessage::user(text));
            self.mark_timeline_message_dirty_at(self.messages.len().saturating_sub(1));
        }
        self.abort_requested_at = None;
        // For a fresh run, show activity immediately. During an active run,
        // keep the current state and show the queued-message line instead.
        if !was_streaming {
            self.note_streaming(NeoismAgentStreamingState::Thinking, None);
        }
        let send_result = self.send_prompt(&prompt, !was_streaming);
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

    pub(crate) fn sync_input_pickers(&mut self) {
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

    pub(crate) fn sync_skill_mention_picker(&mut self) {
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
        self.ensure_skill_options();
        let options = self.skill_options.clone();
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::SkillMention)
        {
            picker.replace_options(options);
        } else {
            self.picker = Some(NeoismAgentPicker::new(
                NeoismAgentPickerKind::SkillMention,
                "Skills",
                options,
                0,
            ));
        }
        self.set_picker_query(query);
    }

    pub(crate) fn sync_slash_picker(&mut self) {
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

    pub(crate) fn sync_file_mention_picker(&mut self) {
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

    pub(crate) fn active_file_mention(&self) -> Option<(usize, String)> {
        self.active_prefixed_token('@')
    }

    pub(crate) fn active_prefixed_token(&self, trigger_char: char) -> Option<(usize, String)> {
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

    pub(crate) fn remember_sent_prompt(&mut self, text: &str) {
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

    pub(crate) fn update_picker_query(&mut self, text: &str) {
        let mut query = self
            .picker
            .as_ref()
            .map(|picker| picker.query.clone())
            .unwrap_or_default();
        query.push_str(text);
        self.set_picker_query(query);
    }

    pub(crate) fn set_picker_query(&mut self, query: String) {
        if let Some(picker) = self.picker.as_mut() {
            if picker.kind == NeoismAgentPickerKind::Slash {
                self.input = format!("/{query}");
            }
            picker.set_query(query);
        }
    }

    pub(crate) fn file_mention_options(&self, query: &str) -> Vec<NeoismAgentPickerOption> {
        file_mention_options(&self.file_mention_root(), query, FILE_MENTION_LIMIT)
    }

    pub(crate) fn file_mention_root(&self) -> PathBuf {
        self.directory
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub(crate) fn apply_file_mention(&mut self, value: String) {
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

    pub(crate) fn insert_skill_mention_by_name(&mut self, name: String) {
        match self.refresh_skill_options() {
            Ok(()) => {
                let needle = name.to_ascii_lowercase();
                if let Some(option) = self
                    .skill_options
                    .iter()
                    .find(|option| {
                        option.value.eq_ignore_ascii_case(&name)
                            || option.title.eq_ignore_ascii_case(&name)
                            || option.title.to_ascii_lowercase().contains(&needle)
                    })
                    .cloned()
                {
                    self.apply_skill_mention(option);
                } else {
                    self.system_message("Skill", format!("skill {name} not found"));
                }
            }
            Err(error) => self.system_message("Skill", error),
        }
    }

    pub(crate) fn apply_skill_mention(&mut self, option: NeoismAgentPickerOption) {
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

    pub(crate) fn apply_inline_skill_mention(&mut self, option: NeoismAgentPickerOption) {
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

    pub(crate) fn remember_file_mention(&mut self, token: &str, value: &str) {
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
                url: attachment_url_for_path(&path, mime),
                mime: mime.to_string(),
            });
    }

    pub(crate) fn pasted_attachment_path(&self, text: &str) -> Option<PathBuf> {
        let raw = text.trim();
        if raw.is_empty() || raw.contains('\n') {
            return None;
        }
        let candidate = input_controller::path_from_pasted_reference(raw)?;
        let root = self.file_mention_root();
        let candidates = if candidate.is_absolute() {
            vec![candidate]
        } else {
            vec![root.join(&candidate), candidate]
        };
        candidates.into_iter().find(|path| {
            path.is_file()
                && input_controller::mime_can_attach_from_paste(
                    input_controller::mime_for_path(path),
                )
        })
    }

    pub(crate) fn display_path_for_attachment(&self, path: &Path, is_dir: bool) -> String {
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

    pub(crate) fn file_attachment_token(&self, path: &Path, mime: &str) -> String {
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

    pub(crate) fn file_attachment_count<F>(&self, mut predicate: F) -> usize
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

    pub(crate) fn unique_attachment_token(&self, base: &str) -> String {
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

    pub(crate) fn prompt_parts_for(&self, text: &str) -> Vec<Value> {
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

    pub(crate) fn prompt_system_for(&self, text: &str) -> Option<String> {
        let mut seen = BTreeSet::new();
        let mut lines = Vec::new();
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
            if description.trim().is_empty() {
                lines.push(format!(
                    "- {name}: call the skill tool with name \"{name}\"."
                ));
            } else {
                lines.push(format!(
                    "- {name}: {} Call the skill tool with name \"{name}\".",
                    description.trim()
                ));
            }
        }
        (!lines.is_empty()).then(|| {
            format!(
                "The user selected these skills for this request. Load each selected skill with the skill tool before applying it:\n{}",
                lines.join("\n")
            )
        })
    }

    pub(crate) fn expand_text_attachments(&self, text: &str) -> String {
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
