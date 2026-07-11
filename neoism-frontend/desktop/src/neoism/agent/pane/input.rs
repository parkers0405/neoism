use super::*;

impl NeoismAgentPane {
    pub(crate) fn insert_input_text(&mut self, text: &str) {
        let mut buffer = self.input_buffer();
        buffer.insert_text(text);
        self.apply_input_buffer(buffer);
    }

    pub(crate) fn insert_input_token(&mut self, token: &str) {
        let mut buffer = self.input_buffer();
        buffer.insert_token(token);
        self.apply_input_buffer(buffer);
    }

    pub(crate) fn delete_input_char_before_cursor(&mut self) -> bool {
        let mut buffer = self.input_buffer();
        let deleted = buffer.delete_char_before_cursor();
        self.apply_input_buffer(buffer);
        deleted
    }

    /// Backspace that deletes composer tokens (`[pasted N lines]`,
    /// `@file` mentions, skill chips) as single units.
    pub(crate) fn delete_token_or_char_before_cursor(&mut self) {
        let tokens: Vec<String> = self
            .input_attachments
            .iter()
            .map(|attachment| attachment.token().to_string())
            .collect();
        let token_refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
        let mut buffer = self.input_buffer();
        if let Some(deleted) = buffer.delete_token_before_cursor(&token_refs) {
            self.apply_input_buffer(buffer);
            if !self.input.contains(&deleted) {
                self.input_attachments
                    .retain(|attachment| attachment.token() != deleted);
            }
            return;
        }
        let _ = buffer.delete_char_before_cursor();
        self.apply_input_buffer(buffer);
    }

    pub fn insert_text(&mut self, text: &str) {
        if self.is_subagent_session() && self.picker.is_none() {
            return;
        }
        if let Some(kind) = self.picker.as_ref().map(|picker| picker.kind) {
            match kind {
                // FileMention and Slash both type into the input buffer so
                // cursor_byte advances with every character and
                // sync_input_pickers() can derive the picker query from the
                // buffer. For Slash + whitespace: pin cursor_byte to the
                // current input length first (set_picker_query syncs
                // self.input but never advances cursor_byte, so it may lag
                // behind) so the space lands at the true end of the command.
                NeoismAgentPickerKind::FileMention
                | NeoismAgentPickerKind::SkillMention => {
                    self.insert_input_text(text);
                    self.sync_input_pickers();
                }
                NeoismAgentPickerKind::Slash => {
                    if text.contains(char::is_whitespace) {
                        self.cursor_byte = self.input.len();
                    }
                    self.insert_input_text(text);
                    self.sync_input_pickers();
                }
                _ => self.update_picker_query(text),
            }
            return;
        }
        self.insert_input_text(text);
        self.sync_input_pickers();
    }

    pub fn insert_paste(&mut self, text: &str) {
        if self.is_subagent_session() && self.picker.is_none() {
            return;
        }
        let text = input_controller::normalize_paste(text);
        if text.is_empty() {
            return;
        }
        // A picker whose input is its own query row (model / connect / the
        // OAuth-token & API-key entry, etc.) must receive the paste directly.
        // Otherwise a long value like an OAuth code or JWT trips the
        // "compact long paste" path below and lands in the composer as a
        // `[pasted …]` attachment instead of the field the user is looking at.
        // Only the composer-driven pickers (slash / @file / skill mentions)
        // intentionally let paste fall through to the composer.
        if let Some(kind) = self.picker.as_ref().map(|picker| picker.kind) {
            if !matches!(
                kind,
                NeoismAgentPickerKind::Slash
                    | NeoismAgentPickerKind::FileMention
                    | NeoismAgentPickerKind::SkillMention
            ) {
                self.update_picker_query(&text);
                return;
            }
        }
        if let Some(path) = self.pasted_attachment_path(&text) {
            if self.attach_path(&path) {
                return;
            }
        }
        if input_controller::paste_should_compact(&text) {
            self.insert_pasted_text_attachment(text);
        } else {
            self.insert_text(&text);
        }
    }

    pub fn attach_path(&mut self, path: &Path) -> bool {
        if self.is_subagent_session() {
            return false;
        }
        if path.is_dir() {
            let display = self.display_path_for_attachment(path, true);
            self.close_picker();
            self.insert_input_token(&format!("@{display}"));
            self.sync_input_pickers();
            return true;
        }
        if !path.is_file() {
            return false;
        }
        let mime = input_controller::mime_for_path(path);
        let filename = self.display_path_for_attachment(path, false);
        let token = self.unique_attachment_token(&self.file_attachment_token(path, mime));
        let url = attachment_url_for_path(path, mime);
        self.close_picker();
        self.insert_input_token(&token);
        self.input_attachments
            .push(NeoismAgentInputAttachment::File {
                token,
                filename,
                url,
                mime: mime.to_string(),
            });
        self.sync_input_pickers();
        true
    }

    pub fn attach_clipboard_image(&mut self, image: ClipboardImage) -> bool {
        if self.is_subagent_session() {
            return false;
        }
        if image.bytes.is_empty() || !image.mime.starts_with("image/") {
            return false;
        }
        let next = self.file_attachment_count(|mime| mime.starts_with("image/")) + 1;
        let token = self.unique_attachment_token(&format!("[image{next}]"));
        let filename = if image.filename.trim().is_empty() {
            format!(
                "clipboard-image-{next}.{}",
                input_controller::extension_for_mime(&image.mime)
            )
        } else {
            image.filename
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(image.bytes);
        let url = format!("data:{};base64,{encoded}", image.mime);
        self.close_picker();
        self.insert_input_token(&token);
        self.input_attachments
            .push(NeoismAgentInputAttachment::File {
                token,
                filename,
                url,
                mime: image.mime,
            });
        self.sync_input_pickers();
        true
    }

    pub(crate) fn insert_pasted_text_attachment(&mut self, text: String) {
        let token = self.unique_attachment_token(&input_controller::paste_token(&text));
        self.close_picker();
        self.insert_input_token(&token);
        self.input_attachments
            .push(NeoismAgentInputAttachment::Text { token, text });
        self.sync_input_pickers();
    }

    pub fn insert_newline(&mut self) {
        if self.is_subagent_session() && self.picker.is_none() {
            return;
        }
        if self.picker.is_none() {
            self.insert_input_text("\n");
            self.sync_input_pickers();
        }
    }

    pub fn backspace(&mut self) {
        if self.is_subagent_session() && self.picker.is_none() {
            return;
        }
        if let Some(kind) = self.picker.as_ref().map(|picker| picker.kind) {
            match kind {
                NeoismAgentPickerKind::FileMention
                | NeoismAgentPickerKind::SkillMention
                | NeoismAgentPickerKind::Slash => {
                    // Pin cursor_byte to input end first: set_picker_query syncs
                    // self.input but never advances cursor_byte, so after typing
                    // /ses the caret may still sit at 1. This ensures we delete
                    // the last real character instead of whatever was before the
                    // stale cursor position.
                    if self.cursor_byte < self.input.len() {
                        self.cursor_byte = self.input.len();
                    }
                    let deleted = self.delete_input_char_before_cursor();
                    self.history_index = None;
                    if !deleted && kind == NeoismAgentPickerKind::Slash {
                        self.input.clear();
                        self.cursor_byte = 0;
                        self.picker = None;
                        return;
                    }
                    self.sync_input_pickers();
                }
                _ => {
                    let mut next = self
                        .picker
                        .as_ref()
                        .map(|picker| picker.query.clone())
                        .unwrap_or_default();
                    next.pop();
                    self.set_picker_query(next);
                }
            }
        } else {
            self.delete_token_or_char_before_cursor();
            self.history_index = None;
            self.sync_input_pickers();
        }
    }

    pub fn clear_or_abort(&mut self) {
        let now = Instant::now();
        if !self.input.is_empty() {
            self.input.clear();
            self.cursor_byte = 0;
            self.history_index = None;
            self.picker = None;
            self.file_mention_anchor = None;
            self.input_attachments.clear();
            self.last_control_c_at = Some(now);
            return;
        }
        // While a run is active (including compaction), a single Esc aborts it
        // immediately — the composer is empty and there's a live thing to stop,
        // so "esc to cancel" should just work. When idle, keep the double-press
        // guard so a stray Esc on an empty composer does nothing surprising.
        if self.is_streaming() {
            self.last_control_c_at = Some(now);
            self.abort_session();
            self.messages
                .push(NeoismAgentMessage::subtask("Interrupted", "run stopped"));
            return;
        }
        let double = self
            .last_control_c_at
            .is_some_and(|last| now.duration_since(last) <= Duration::from_millis(1400));
        self.last_control_c_at = Some(now);
        if double {
            self.abort_session();
            self.messages
                .push(NeoismAgentMessage::subtask("Interrupted", "run stopped"));
        }
    }

    pub fn move_input_up_or_history(&mut self) {
        if self.picker.is_some() {
            let _ = self.move_picker_selection(-1);
            return;
        }
        if self.is_subagent_session() {
            return;
        }
        let mut buffer = self.input_buffer();
        match self.current_input_wrap_ranges() {
            Some(ranges) => buffer.move_up_with_history_visual(ranges),
            None => buffer.move_up_with_history(),
        }
        self.apply_input_buffer(buffer);
        self.sync_input_pickers();
    }

    pub fn move_input_down_or_history(&mut self) {
        if self.picker.is_some() {
            let _ = self.move_picker_selection(1);
            return;
        }
        if self.is_subagent_session() {
            return;
        }
        let mut buffer = self.input_buffer();
        match self.current_input_wrap_ranges() {
            Some(ranges) => buffer.move_down_with_history_visual(ranges),
            None => buffer.move_down_with_history(),
        }
        self.apply_input_buffer(buffer);
        self.sync_input_pickers();
    }

    pub fn move_input_left(&mut self) {
        if self.picker.is_some() || self.is_subagent_session() {
            return;
        }
        let mut buffer = self.input_buffer();
        buffer.move_left();
        self.apply_input_buffer(buffer);
    }

    pub fn move_input_right(&mut self) {
        if self.picker.is_some() || self.is_subagent_session() {
            return;
        }
        let mut buffer = self.input_buffer();
        buffer.move_right();
        self.apply_input_buffer(buffer);
    }

    pub fn move_input_home(&mut self) {
        if self.picker.is_some() || self.is_subagent_session() {
            return;
        }
        let mut buffer = self.input_buffer();
        buffer.move_home();
        self.apply_input_buffer(buffer);
    }

    pub fn move_input_end(&mut self) {
        if self.picker.is_some() || self.is_subagent_session() {
            return;
        }
        let mut buffer = self.input_buffer();
        buffer.move_end();
        self.apply_input_buffer(buffer);
    }

    pub fn toggle_mode(&mut self) {
        let next = match self.mode {
            NeoismAgentMode::Build => "plan",
            NeoismAgentMode::Plan => "build",
        };
        self.apply_agent(next.to_string());
    }

    pub fn open_agent_picker(&mut self) {
        let options = fetch_agent_options(&self.server, self.directory.as_deref())
            .unwrap_or_else(|error| {
                vec![NeoismAgentPickerOption::new(
                    "Neoism Agent server not reachable",
                    &error,
                    "offline",
                    "",
                )]
            });
        let selected = options
            .iter()
            .position(|option| self.agent.as_deref().unwrap_or("") == option.value)
            .unwrap_or(0);
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Agent,
            "Agents",
            options,
            selected,
        ));
    }

    pub fn open_model_picker(&mut self) {
        let options = fetch_model_options(&self.server).unwrap_or_else(|error| {
            vec![NeoismAgentPickerOption::new(
                "Neoism Agent server not reachable",
                &error,
                "offline",
                "",
            )]
        });
        let options = self.model_picker_options(options);
        let selected = options
            .iter()
            .position(|option| option.is_selectable() && option.value == self.model)
            .unwrap_or(0);
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Model,
            "Select model",
            options,
            selected,
        ));
    }

    pub(crate) fn model_picker_options(
        &self,
        model_options: Vec<NeoismAgentPickerOption>,
    ) -> Vec<NeoismAgentPickerOption> {
        let mut options = Vec::new();
        options.push(self.current_model_picker_option(&model_options));
        if !self.recent_model_options.is_empty() {
            options.push(NeoismAgentPickerOption::header("Recent"));
            options.extend(self.recent_model_options.clone());
        }
        options.extend(model_options);
        options
    }

    pub(crate) fn current_model_picker_option(
        &self,
        model_options: &[NeoismAgentPickerOption],
    ) -> NeoismAgentPickerOption {
        if self.model.trim().is_empty() {
            return NeoismAgentPickerOption::new(
                "server default",
                "Use Neoism Agent default",
                "selected",
                "",
            );
        }
        if let Some(option) = model_options
            .iter()
            .chain(self.recent_model_options.iter())
            .find(|option| option.value == self.model && option.is_selectable())
        {
            let mut current = option.clone();
            current.is_header = false;
            if current.description.is_empty() && !current.section.is_empty() {
                current.description = current.section.clone();
            }
            current.footer = "selected".to_string();
            current.section = "Current".to_string();
            return current;
        }
        let title = self
            .model
            .split_once('/')
            .map(|(_, model)| model)
            .unwrap_or(self.model.as_str());
        let provider = self
            .model
            .split_once('/')
            .map(|(provider, _)| provider)
            .unwrap_or("");
        NeoismAgentPickerOption::new(title, provider, "selected", &self.model)
    }

    pub(crate) fn remember_model_value(&mut self, value: &str) {
        if value.trim().is_empty() {
            return;
        }
        if let Some(option) = self
            .recent_model_options
            .iter()
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

    pub(crate) fn remember_model_option(&mut self, option: &NeoismAgentPickerOption) {
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

    pub fn open_thinking_picker(&mut self) {
        let mut options = vec![
            NeoismAgentPickerOption::new(
                "none",
                "Use model default reasoning",
                "default",
                "",
            ),
            NeoismAgentPickerOption::new("low", "Fastest reasoning", "think", "low"),
            NeoismAgentPickerOption::new(
                "medium",
                "Balanced reasoning",
                "think",
                "medium",
            ),
            NeoismAgentPickerOption::new("high", "More reasoning", "think", "high"),
            NeoismAgentPickerOption::new("xhigh", "Maximum reasoning", "think", "xhigh"),
        ];
        // GPT-5.6's ultra mode (Responses API multi-agent orchestration) —
        // only offered where the server can actually enable it.
        if self.model.contains("gpt-5.6") {
            options.push(NeoismAgentPickerOption::new(
                "ultra",
                "Multi-agent reasoning (GPT-5.6)",
                "think",
                "ultra",
            ));
        }
        let selected = options
            .iter()
            .position(|option| option.value == self.thinking.as_deref().unwrap_or(""))
            .unwrap_or(0);
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Thinking,
            "Reasoning",
            options,
            selected,
        ));
    }

    pub fn open_sessions_picker(&mut self) {
        match fetch_session_options(
            &self.server,
            self.session_id.as_deref(),
            self.directory.as_deref(),
        ) {
            Ok(options) if !options.is_empty() => {
                let selected = options
                    .iter()
                    .position(|option| {
                        Some(option.value.as_str()) == self.session_id.as_deref()
                    })
                    .unwrap_or(0);
                self.session_picker_base = options.clone();
                self.picker = Some(NeoismAgentPicker::new(
                    NeoismAgentPickerKind::Session,
                    "Sessions",
                    options,
                    selected,
                ));
            }
            Ok(_) => self.system_message("Sessions", "no sessions"),
            Err(error) => self.system_message("Sessions", error),
        }
    }

    /// Selected `(session_id, title, pinned)` in an open `/sessions` picker.
    fn selected_session_row(&self) -> Option<(String, String, bool)> {
        let picker = self.picker.as_ref()?;
        if picker.kind != NeoismAgentPickerKind::Session {
            return None;
        }
        let option = picker.selected_option()?;
        if option.value.trim().is_empty() {
            return None;
        }
        Some((option.value.clone(), option.title.clone(), option.pinned))
    }

    /// Re-fetch sessions and refresh the open Session picker + side panel
    /// after a pin / delete / rename mutation.
    fn refresh_sessions_after_mutation(&mut self) {
        let current = self.session_id.clone();
        let directory = self.directory.clone();
        if let Ok(options) =
            fetch_session_options(&self.server, current.as_deref(), directory.as_deref())
        {
            self.session_picker_base = options.clone();
            if let Some(picker) = self
                .picker
                .as_mut()
                .filter(|picker| picker.kind == NeoismAgentPickerKind::Session)
            {
                picker.replace_options(options);
            }
        }
        if let Ok(entries) =
            fetch_session_entries(&self.server, current.as_deref(), directory.as_deref())
        {
            self.side_panel.set_sessions(entries);
        }
    }

    /// Whether an open picker is the `/sessions` picker (gates the
    /// pin/delete/rename shortcuts + inline rename in the key bridge).
    pub fn session_picker_open(&self) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Session)
    }

    /// `ctrl+f` — toggle the pinned flag of the selected session.
    pub fn toggle_selected_session_pin(&mut self) -> bool {
        let Some((id, _title, pinned)) = self.selected_session_row() else {
            return false;
        };
        if let Err(error) = set_session_pinned(&self.server, &id, !pinned) {
            self.system_message("Sessions", error);
        } else {
            self.refresh_sessions_after_mutation();
        }
        true
    }

    /// `ctrl+d` — delete the selected session.
    pub fn delete_selected_session(&mut self) -> bool {
        let Some((id, _title, _pinned)) = self.selected_session_row() else {
            return false;
        };
        if let Err(error) = delete_session(&self.server, &id) {
            self.system_message("Sessions", error);
            return true;
        }
        if self.session_id.as_deref() == Some(id.as_str()) {
            self.create_new_session();
        }
        self.refresh_sessions_after_mutation();
        true
    }

    /// `ctrl+r` — start an inline rename of the selected session.
    pub fn begin_selected_session_rename(&mut self) -> bool {
        let Some((id, title, _pinned)) = self.selected_session_row() else {
            return false;
        };
        self.session_rename = Some((id, title));
        true
    }

    pub fn session_rename_active(&self) -> bool {
        self.session_rename.is_some()
    }

    pub fn session_rename_buffer(&self) -> Option<String> {
        self.session_rename
            .as_ref()
            .map(|(_, buffer)| buffer.clone())
    }

    pub fn push_session_rename(&mut self, text: &str) {
        if let Some((_, buffer)) = self.session_rename.as_mut() {
            buffer.push_str(text);
        }
    }

    pub fn backspace_session_rename(&mut self) {
        if let Some((_, buffer)) = self.session_rename.as_mut() {
            buffer.pop();
        }
    }

    pub fn cancel_session_rename(&mut self) {
        self.session_rename = None;
    }

    /// Commit the inline rename: PATCH the session title and refresh.
    pub fn commit_session_rename(&mut self) -> bool {
        let Some((id, buffer)) = self.session_rename.take() else {
            return false;
        };
        let title = buffer.trim().to_string();
        if title.is_empty() {
            return true;
        }
        if let Err(error) = rename_session(&self.server, &id, &title) {
            self.system_message("Sessions", error);
        } else {
            self.refresh_sessions_after_mutation();
        }
        true
    }

    pub(crate) fn set_skill_options(
        &mut self,
        directory: Option<String>,
        options: Vec<NeoismAgentPickerOption>,
    ) {
        self.skill_options = options;
        self.skill_options_directory = Some(directory);
        let options = self.skill_options.clone();
        if let Some(picker) = self.picker.as_mut().filter(|picker| {
            matches!(
                picker.kind,
                NeoismAgentPickerKind::Skill | NeoismAgentPickerKind::SkillMention
            )
        }) {
            picker.replace_options(options);
        }
    }

    pub(crate) fn refresh_skill_options(&mut self) -> Result<(), String> {
        let directory = self.directory.clone();
        let options = fetch_skill_options(&self.server, directory.as_deref())?;
        self.set_skill_options(directory, options);
        Ok(())
    }

    pub(crate) fn ensure_skill_options(&mut self) {
        let directory = self.directory.clone();
        if self.skill_options_directory.as_ref() == Some(&directory) {
            return;
        }
        if let Ok(options) = fetch_skill_options(&self.server, directory.as_deref()) {
            self.set_skill_options(directory, options);
        }
    }

    pub fn open_skill_picker(&mut self) {
        match self.refresh_skill_options() {
            Ok(()) if !self.skill_options.is_empty() => {
                self.picker = Some(NeoismAgentPicker::new(
                    NeoismAgentPickerKind::Skill,
                    "Skills",
                    self.skill_options.clone(),
                    0,
                ));
            }
            Ok(()) => self.system_message("Skills", "no skills discovered"),
            Err(error) => self.system_message("Skills", error),
        }
    }

    pub fn open_subagent_picker(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Subagents", "no session has started yet");
            return;
        };
        match fetch_subagent_options(&self.server, &session_id) {
            Ok(options) if !options.is_empty() => {
                let selected = options
                    .iter()
                    .position(|option| option.value == session_id)
                    .unwrap_or(0);
                self.picker = Some(NeoismAgentPicker::new(
                    NeoismAgentPickerKind::Subagent,
                    "Subagents",
                    options,
                    selected,
                ));
            }
            Ok(_) => self.system_message("Subagents", "no subagent sessions"),
            Err(error) => self.system_message("Subagents", error),
        }
    }

    pub fn close_picker(&mut self) {
        // The `/connect` flow is multi-stage: ESC steps back one screen (like
        // the per-screen "esc" affordance) rather than dismissing everything.
        if let Some(kind) = self.picker.as_ref().map(|picker| picker.kind) {
            match kind {
                NeoismAgentPickerKind::ConnectSecret => {
                    if let Some(provider_id) =
                        self.connect.as_ref().and_then(|flow| flow.provider_id())
                    {
                        self.enter_connect_auth(&provider_id);
                        return;
                    }
                    self.close_connect();
                    return;
                }
                NeoismAgentPickerKind::ConnectAuth => {
                    self.reopen_connect_provider_picker();
                    return;
                }
                NeoismAgentPickerKind::Connect => {
                    self.close_connect();
                    return;
                }
                _ => {}
            }
        }
        self.picker = None;
        self.session_rename = None;
        self.file_mention_anchor = None;
        if self.input == "/" {
            self.input.clear();
            self.cursor_byte = 0;
        }
    }

    pub fn move_picker_selection(&mut self, delta: isize) -> bool {
        let Some(picker) = self.picker.as_mut() else {
            return false;
        };
        picker.move_selection(delta);
        true
    }

    pub fn scroll_picker_pixels(&mut self, delta_pixels: f32) -> bool {
        self.picker
            .as_mut()
            .is_some_and(|picker| picker.scroll_pixels(delta_pixels))
    }

    pub fn picker_contains_point(&self, x: f32, y: f32) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|picker| picker.contains_point(x, y))
    }

    /// Rect of the open inline picker card as painted last frame. The
    /// host feeds this into the text-occlusion list so chrome text
    /// (tab-strip labels etc.) never bleeds through the modal.
    pub fn picker_card_rect(&self) -> Option<[f32; 4]> {
        self.picker.as_ref().and_then(|picker| picker.last_rect)
    }

    /// If the click lands on a picker row, move selection there and
    /// commit it (the conventional "single-click picks" UX). Returns true
    /// when the click was handled by the picker overlay.
    pub fn pick_at(&mut self, x: f32, y: f32) -> bool {
        let Some(picker) = self.picker.as_mut() else {
            return false;
        };
        if !picker.contains_point(x, y) {
            return false;
        }
        if picker.activate_row_at(x, y) {
            // Activating a row commits the picker. Mirror the Enter
            // path so the user gets the same behaviour regardless of
            // input device.
            self.commit_picker();
            return true;
        }
        // Click was inside the popover but missed a row — absorb so the
        // tab strip / message timeline don't react.
        true
    }
}
