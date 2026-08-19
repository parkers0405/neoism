use super::*;

impl NeoismAgentPane {
    pub(in crate::panels::agent_pane::state) fn insert_input_text(&mut self, text: &str) {
        let mut buffer = self.input_buffer();
        buffer.insert_text(text);
        self.apply_input_buffer(buffer);
    }

    pub(in crate::panels::agent_pane::state) fn insert_input_token(
        &mut self,
        token: &str,
    ) {
        let mut buffer = self.input_buffer();
        buffer.insert_token(token);
        self.apply_input_buffer(buffer);
    }

    pub(in crate::panels::agent_pane::state) fn delete_input_char_before_cursor(
        &mut self,
    ) -> bool {
        let mut buffer = self.input_buffer();
        let deleted = buffer.delete_char_before_cursor();
        self.apply_input_buffer(buffer);
        deleted
    }

    /// Backspace that deletes composer tokens (`[pasted N lines]`,
    /// `@file` mentions, skill chips) as single units.
    pub(in crate::panels::agent_pane::state) fn delete_token_or_char_before_cursor(
        &mut self,
    ) {
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
                // Slash, FileMention, and SkillMention are typed into the input buffer so
                // the cursor follows the text and `sync_input_pickers` can
                // derive the picker query directly from the buffer.
                NeoismAgentPickerKind::FileMention
                | NeoismAgentPickerKind::SkillMention
                | NeoismAgentPickerKind::Slash => {
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

    pub fn replace_input(&mut self, text: &str) {
        if self.is_subagent_session() && self.picker.is_none() {
            return;
        }
        self.input.clear();
        self.input.push_str(text);
        self.cursor_byte = self.input.len();
        self.history_index = None;
        self.file_mention_anchor = None;
        self.picker = None;
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
        // Pickers whose input is their own query row (model / connect / secret
        // entry) receive the paste directly, so a long value (OAuth code, JWT)
        // fills the field instead of becoming a composer `[pasted …]`
        // attachment. Composer-driven pickers (slash / @file / skill) fall
        // through to the composer.
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

    /// Attach a file (or mention a directory) from the host
    /// filesystem — desktop parity (`pane/input.rs::attach_path`).
    /// Hosts without a local filesystem get `false` (the fs probes
    /// fail on wasm); they attach through
    /// [`Self::attach_file_bytes`] / [`Self::attach_clipboard_image`]
    /// instead.
    pub fn attach_path(&mut self, path: &std::path::Path) -> bool {
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

    /// Attach a clipboard image supplied as raw bytes — desktop parity
    /// (`pane/input.rs::attach_clipboard_image`), with the bytes
    /// handed over by the host (desktop reads the OS clipboard, web
    /// reads the browser `ClipboardEvent`). The image lands as an
    /// `[imageN]` composer token + chip and is sent with the next
    /// submit. Oversized payloads (> 20MB) are rejected.
    pub fn attach_clipboard_image(
        &mut self,
        filename: &str,
        mime: &str,
        bytes: &[u8],
    ) -> bool {
        if bytes.is_empty() || !mime.starts_with("image/") {
            return false;
        }
        let filename = if filename.trim().is_empty() {
            let next = self.file_attachment_count(|mime| mime.starts_with("image/")) + 1;
            format!(
                "clipboard-image-{next}.{}",
                input_controller::extension_for_mime(mime)
            )
        } else {
            filename.to_string()
        };
        self.attach_file_bytes(&filename, mime, bytes)
    }

    /// Attach a host-mediated file (drag-and-drop, file picker, pasted
    /// image) from raw bytes: `[imageN]` / `[pdfN]` / `[fileN: name]`
    /// token per `attachment_policy`, mime from the host (falling back
    /// to extension sniffing), 20MB cap, bytes inlined as a `data:`
    /// URL so they survive the wire to the daemon. The attachment
    /// shows as a composer chip and is sent on the next submit —
    /// matching desktop's attach-then-Enter flow.
    pub fn attach_file_bytes(&mut self, filename: &str, mime: &str, bytes: &[u8]) -> bool {
        use base64::Engine as _;

        if self.is_subagent_session() || bytes.is_empty() {
            return false;
        }
        if bytes.len() as u64 > MAX_INLINE_ATTACHMENT_BYTES {
            self.ui_events.push(NeoismAgentUiEvent::Notice {
                message: format!("Attachment too large (max 20MB): {filename}"),
                level: NeoismAgentNoticeLevel::Warn,
            });
            return false;
        }
        let filename = if filename.trim().is_empty() {
            "file".to_string()
        } else {
            filename.trim().to_string()
        };
        let mime = if mime.trim().is_empty() {
            input_controller::mime_for_path(std::path::Path::new(&filename)).to_string()
        } else {
            mime.trim().to_string()
        };
        let token =
            self.unique_attachment_token(&self.file_attachment_token_for(&filename, &mime));
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let url = format!("data:{mime};base64,{encoded}");
        self.close_picker();
        self.insert_input_token(&token);
        self.input_attachments
            .push(NeoismAgentInputAttachment::File {
                token,
                filename,
                url,
                mime,
            });
        self.sync_input_pickers();
        true
    }

    pub(in crate::panels::agent_pane::state) fn insert_pasted_text_attachment(
        &mut self,
        text: String,
    ) {
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
                NeoismAgentPickerKind::Slash => {
                    let mut next = self
                        .picker
                        .as_ref()
                        .map(|picker| picker.query.clone())
                        .unwrap_or_default();
                    if next.is_empty() {
                        self.input.clear();
                        self.picker = None;
                        return;
                    }
                    next.pop();
                    self.set_picker_query(next);
                }
                NeoismAgentPickerKind::FileMention => {
                    self.delete_input_char_before_cursor();
                    self.history_index = None;
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
            return;
        }
        let double = self.last_control_c_at.is_some_and(|last| {
            now.saturating_duration_since(last) <= Duration::from_millis(1400)
        });
        self.last_control_c_at = Some(now);
        if double {
            self.abort_session();
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
        match self.current_input_wrap_rows() {
            Some(rows) => buffer.move_up_with_history_visual(rows),
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
        match self.current_input_wrap_rows() {
            Some(rows) => buffer.move_down_with_history_visual(rows),
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
}
