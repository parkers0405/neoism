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
                // The directory dropdown's query is a PATH, not a fuzzy
                // filter — re-resolve + re-list on every keystroke.
                NeoismAgentPickerKind::Directory => {
                    let mut query = self
                        .picker
                        .as_ref()
                        .map(|picker| picker.query.clone())
                        .unwrap_or_default();
                    query.push_str(text);
                    self.sync_directory_picker_query(query);
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
                NeoismAgentPickerKind::Directory => {
                    let mut next = self
                        .picker
                        .as_ref()
                        .map(|picker| picker.query.clone())
                        .unwrap_or_default();
                    next.pop();
                    self.sync_directory_picker_query(next);
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
        let options = vec![
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

    /// Drop directory-keyed caches (the skill catalog) so they re-fetch for
    /// a newly-selected working directory. Lives here because the cache
    /// fields are private to the `pane` module; `apply_directory` (in the
    /// sibling `commands` module) drives the rest of the cwd-change flow.
    pub(crate) fn invalidate_directory_caches(&mut self) {
        self.skill_options.clear();
        self.skill_options_directory = None;
    }

    /// Open the working-directory dropdown from the side panel's
    /// "Directory" header. Lists the current directory's parent (`..`) and
    /// its immediate subdirectories from the local filesystem — the desktop
    /// fork is always local, so a plain `read_dir` is the right source.
    /// Selecting a row re-scopes the pane via [`apply_directory`].
    pub fn open_directory_picker(&mut self) {
        let base = self.directory_picker_base();
        let options = directory_picker_options(&base.to_string_lossy());
        self.picker = Some(NeoismAgentPicker::new(
            NeoismAgentPickerKind::Directory,
            "Working directory",
            options,
            0,
        ));
    }

    /// Base directory the working-directory picker resolves typed paths
    /// against: the pane's declared cwd, falling back to the process cwd.
    /// Stable while the picker is open (only [`apply_directory`] re-points
    /// `self.directory`), so relative queries like `../..` resolve
    /// consistently keystroke to keystroke.
    fn directory_picker_base(&self) -> std::path::PathBuf {
        self.directory
            .as_deref()
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }

    /// Re-list the working-directory picker for a typed PATH query (not a
    /// fuzzy filter). The query resolves against [`directory_picker_base`] —
    /// `~`/`~/` → `$HOME`, `.`/`..`/nested relative segments, absolute paths —
    /// and the resolved directory's subdirectories are listed live, filtered
    /// by the segment after the final `/`, with `..` always available. The
    /// options are pushed pre-filtered so the shared picker's fuzzy filter
    /// doesn't run over them; Enter (or a click) commits the selected row via
    /// [`apply_directory`], which no-ops on a non-existent target.
    pub(crate) fn sync_directory_picker_query(&mut self, query: String) {
        if self
            .picker
            .as_ref()
            .is_none_or(|picker| picker.kind != NeoismAgentPickerKind::Directory)
        {
            return;
        }
        let base = self.directory_picker_base();
        // `cd <path>` acts like a shell `cd`: the WHOLE arg is the
        // destination, so `cd ..` / `cd ~/Github/` change straight there on
        // Enter (the resolved dir is the first/default row). Anything else is
        // a browse/filter path (dir part lists, trailing segment filters).
        let options = if let Some(arg) = directory_cd_arg(&query) {
            directory_cd_options(&resolve_directory_query(&base, arg))
        } else {
            let (dir_part, filter) = split_directory_query(&query);
            directory_listing(&resolve_directory_query(&base, dir_part), filter)
        };
        if let Some(picker) = self
            .picker
            .as_mut()
            .filter(|picker| picker.kind == NeoismAgentPickerKind::Directory)
        {
            picker.set_pre_filtered_options(query, options);
        }
    }

    pub fn close_picker(&mut self) {
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

/// Build the working-directory dropdown rows for `base`: a `..` row for the
/// parent, then `base`'s immediate subdirectories (alphabetical, hidden
/// dot-directories skipped). Each row's `value` is the absolute target
/// path that [`NeoismAgentPane::apply_directory`] switches the cwd to.
fn directory_picker_options(base: &str) -> Vec<NeoismAgentPickerOption> {
    use std::path::{Path, PathBuf};
    let base_path: PathBuf = std::fs::canonicalize(base)
        .unwrap_or_else(|_| Path::new(base).to_path_buf());
    directory_listing(&base_path, "")
}

/// List `dir`'s immediate subdirectories (alphabetical, hidden dot-dirs
/// skipped) plus a `..` row for its parent, keeping only entries whose name
/// contains `filter` (case-insensitive; empty matches everything). Each row's
/// `value` is the absolute target path. `..` stays available while browsing
/// (empty filter); a typed filter narrows to matching subdirectories so the
/// first match is what Enter commits.
fn directory_listing(dir: &std::path::Path, filter: &str) -> Vec<NeoismAgentPickerOption> {
    let needle = filter.trim().to_lowercase();
    let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);
    let mut out = Vec::new();

    if let Some(parent) = dir.parent() {
        if matches("..") {
            let parent = parent.to_string_lossy().into_owned();
            out.push(NeoismAgentPickerOption::new(
                "..",
                &compact_directory_label(&parent),
                "parent",
                &parent,
            ));
        }
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut dirs: Vec<(String, String)> = entries
            .flatten()
            .filter_map(|entry| {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    return None;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || !matches(&name) {
                    return None;
                }
                Some((name, entry.path().to_string_lossy().into_owned()))
            })
            .collect();
        dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        for (name, path) in dirs {
            let footer = compact_directory_label(&path);
            out.push(NeoismAgentPickerOption::new(&name, &footer, "dir", &path));
        }
    }

    if out.is_empty() {
        let label = if needle.is_empty() {
            "no subdirectories"
        } else {
            "no match"
        };
        out.push(NeoismAgentPickerOption::new(
            label,
            &compact_directory_label(&dir.to_string_lossy()),
            "empty",
            "",
        ));
    }
    out
}

/// If `query` is a shell-style `cd <path>` command — the literal word `cd`
/// followed by whitespace, or a bare `cd` — return the path argument
/// (trimmed, `""` for bare `cd`). Returns `None` for a normal browse/filter
/// query (e.g. `code/`) so the existing path-navigation is unchanged.
fn directory_cd_arg(query: &str) -> Option<&str> {
    let rest = query.trim_start().strip_prefix("cd")?;
    if rest.is_empty() {
        Some("")
    } else if rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// Rows for a `cd <path>` command: the resolved destination as the first
/// (default) row so Enter changes straight to it, then that directory's
/// subdirectories for optional drilling. `apply_directory` rejects the row
/// when the destination isn't a real directory (e.g. a half-typed path).
fn directory_cd_options(target: &std::path::Path) -> Vec<NeoismAgentPickerOption> {
    let target_str = target.to_string_lossy().into_owned();
    let mut out = vec![NeoismAgentPickerOption::new(
        &compact_directory_label(&target_str),
        "↵ change to this directory",
        "cd",
        &target_str,
    )];
    out.extend(directory_listing(target, ""));
    out
}

/// Split a typed directory query into `(dir_part, filter)`: everything up to
/// and including the final `/` names the directory to list, and the trailing
/// segment filters its entries. A query with no `/` filters the base dir; one
/// ending in `/` drills into `dir_part` with no filter.
fn split_directory_query(query: &str) -> (&str, &str) {
    match query.rfind('/') {
        Some(index) => (&query[..=index], &query[index + 1..]),
        None => ("", query),
    }
}

/// Resolve a typed `dir_part` against `base`: a leading `~`/`~/` expands to
/// `$HOME`, absolute paths are taken as-is, and everything else (including
/// `.`/`..`/nested relative segments) resolves against `base`. The result is
/// lexically normalized, then canonicalized when it exists on disk.
fn resolve_directory_query(base: &std::path::Path, dir_part: &str) -> std::path::PathBuf {
    use std::path::{Path, PathBuf};
    let trimmed = dir_part.trim();
    let expanded: PathBuf = if trimmed == "~" || trimmed.starts_with("~/") {
        match home_directory() {
            Some(home) if trimmed == "~" => home,
            Some(home) => home.join(trimmed.trim_start_matches("~/")),
            None => PathBuf::from(trimmed),
        }
    } else if trimmed.is_empty() {
        base.to_path_buf()
    } else {
        let path = Path::new(trimmed);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    };
    let normalized = normalize_path(&expanded);
    std::fs::canonicalize(&normalized).unwrap_or(normalized)
}

/// Collapse `.` and `..` segments lexically so a not-yet-existing path (which
/// [`std::fs::canonicalize`] would reject) still resolves toward a real
/// ancestor we can list.
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// The user's home directory, for `~` expansion in the directory picker.
fn home_directory() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}
