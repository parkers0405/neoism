use super::*;

impl NeoismAgentPane {
    pub fn commit_picker(&mut self) -> bool {
        let Some(picker) = self.picker.take() else {
            return false;
        };
        // The `/connect` secret stage has no selectable rows — its query row is
        // the input field, so commit reads the typed query directly.
        if picker.kind == NeoismAgentPickerKind::ConnectSecret {
            self.submit_connect_secret(picker.query.clone());
            return true;
        }
        if picker.kind == NeoismAgentPickerKind::Directory {
            let query = picker.query.trim();
            let selected = picker.selected_option().map(|option| option.value.clone());
            let path = if query.is_empty() {
                selected
            } else if query.starts_with(['~', '/', '.', '\\'])
                || query.contains(['/', '\\'])
                || query.as_bytes().get(1) == Some(&b':')
                || selected.is_none()
            {
                Some(query.to_string())
            } else {
                selected
            };
            if let (Some(session_id), Some(path)) = (self.session_id.clone(), path) {
                self.push_outbound(OutboundAgentCommand::SlashCommand {
                    name: "cd".to_string(),
                    args: path,
                });
                self.side_panel.set_viewed_session_id(Some(session_id));
            }
            return true;
        }
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
            NeoismAgentPickerKind::Mcp => {
                self.open_mcp_actions(&option.value);
            }
            NeoismAgentPickerKind::McpActions => {
                let value =
                    serde_json::from_str::<Value>(&option.value).unwrap_or_default();
                let name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let directory = self.directory.clone();
                match value.get("action").and_then(Value::as_str) {
                    Some("enable") => {
                        self.push_outbound(OutboundAgentCommand::McpSetEnabled {
                            name,
                            enabled: true,
                            directory,
                        })
                    }
                    Some("disable") => {
                        self.push_outbound(OutboundAgentCommand::McpSetEnabled {
                            name,
                            enabled: false,
                            directory,
                        })
                    }
                    Some("connect") => {
                        self.push_outbound(OutboundAgentCommand::McpConnect {
                            name,
                            directory,
                        })
                    }
                    Some("disconnect") => {
                        self.push_outbound(OutboundAgentCommand::McpDisconnect {
                            name,
                            directory,
                        })
                    }
                    Some("authenticate") => {
                        self.push_outbound(OutboundAgentCommand::McpOauthAuthorize {
                            name,
                            directory,
                        })
                    }
                    Some("logout") => {
                        self.push_outbound(OutboundAgentCommand::McpRemoveAuth {
                            name,
                            directory,
                        })
                    }
                    _ => {}
                }
            }
            NeoismAgentPickerKind::Thinking => self.apply_thinking(option.value),
            NeoismAgentPickerKind::Session | NeoismAgentPickerKind::Subagent => {
                self.switch_session(option.value);
            }
            NeoismAgentPickerKind::Directory => unreachable!("handled above"),
            NeoismAgentPickerKind::Skill => self.apply_skill_mention(option),
            NeoismAgentPickerKind::SkillMention => {
                self.apply_inline_skill_mention(option)
            }
            NeoismAgentPickerKind::FileMention => self.apply_file_mention(option.value),
            NeoismAgentPickerKind::Connect => self.enter_connect_auth(&option.value),
            NeoismAgentPickerKind::ConnectAuth => {
                if option.value == super::connect::DISCONNECT_VALUE {
                    self.disconnect_connect_provider();
                } else if let Ok(index) = option.value.parse::<usize>() {
                    self.start_connect_method(index);
                }
            }
            // Handled above (no selectable row).
            NeoismAgentPickerKind::ConnectSecret => {}
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
        // The transcript echo keeps the compact composer form while the
        // server receives (and later re-echoes) the expanded prompt.
        // Remember the pairing so inbound snapshots canonicalize back to
        // ONE bubble (see `compact_inbound_user_texts`).
        if prompt.trim() != text.trim() {
            self.prompt_echo_aliases
                .push((prompt.trim().to_string(), text.clone()));
            if self.prompt_echo_aliases.len() > 16 {
                self.prompt_echo_aliases.remove(0);
            }
        }
        let was_streaming = self.is_streaming();
        self.abort_requested_at = None;
        // For a fresh run, show activity immediately. During an active run,
        // keep the current state and show the queued-message line instead.
        if !was_streaming {
            self.note_streaming(NeoismAgentStreamingState::Generating, None);
        }
        let send_result = self.send_prompt_with_echo(&prompt, &text, !was_streaming);
        self.input_attachments.clear();
        match send_result {
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

    pub(in crate::panels::agent_pane::state) fn active_file_mention(
        &self,
    ) -> Option<(usize, String)> {
        self.active_prefixed_token('@')
    }

    pub(in crate::panels::agent_pane::state) fn active_prefixed_token(
        &self,
        trigger_char: char,
    ) -> Option<(usize, String)> {
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

    /// Seed the composer's Up-arrow recall with a persisted history
    /// (oldest first). Hosts with durable storage (web localStorage,
    /// desktop's prompt-history file) restore through this on
    /// startup; live entries submitted afterwards append on top.
    pub fn seed_sent_history(&mut self, oldest_first: Vec<String>) {
        let mut seeded: Vec<String> = oldest_first
            .into_iter()
            .filter(|entry| !entry.trim().is_empty())
            .collect();
        seeded.dedup();
        const MAX_HISTORY: usize = 100;
        if seeded.len() > MAX_HISTORY {
            let extra = seeded.len() - MAX_HISTORY;
            seeded.drain(0..extra);
        }
        self.sent_history = seeded;
    }

    pub(in crate::panels::agent_pane::state) fn remember_sent_prompt(
        &mut self,
        text: &str,
    ) {
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

    pub(in crate::panels::agent_pane::state) fn update_picker_query(
        &mut self,
        text: &str,
    ) {
        let mut query = self
            .picker
            .as_ref()
            .map(|picker| picker.query.clone())
            .unwrap_or_default();
        query.push_str(text);
        self.set_picker_query(query);
    }

    pub(in crate::panels::agent_pane::state) fn set_picker_query(
        &mut self,
        query: String,
    ) {
        if let Some(picker) = self.picker.as_mut() {
            if picker.kind == NeoismAgentPickerKind::Slash {
                self.input = format!("/{query}");
            }
            picker.set_query(query);
        }
    }

    /// Options for the `@` file-mention picker, ranked exactly like the
    /// desktop pane: the host-fed candidate list is scored with the
    /// shared `fuzzy_score` policy, sorted best-first, and capped to
    /// [`FILE_MENTION_LIMIT`] rows. Row shape mirrors desktop's
    /// `file_mention_options` (`@path` title, "file in parent"
    /// description, `file` footer, relative path as the value).
    pub(in crate::panels::agent_pane::state) fn file_mention_options(
        &self,
        query: &str,
    ) -> Vec<NeoismAgentPickerOption> {
        use crate::panels::agent_pane::attachment_policy::{
            file_mention_description, fuzzy_score,
        };

        let mut scored: Vec<(i64, &str)> = self
            .file_mention_candidates
            .iter()
            .filter_map(|path| fuzzy_score(path, query).map(|score| (score, path.as_str())))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored
            .into_iter()
            .take(FILE_MENTION_LIMIT)
            .map(|(_, relative)| {
                let description = file_mention_description(relative, "file");
                NeoismAgentPickerOption::new(
                    &format!("@{relative}"),
                    &description,
                    "file",
                    relative,
                )
            })
            .collect()
    }

    /// Host hook: install the `@`-mention candidate paths. Paths are
    /// mention-root-relative and `/`-separated (backslashes are
    /// normalized, leading `./` stripped). Hosts with a local
    /// filesystem can walk it themselves; the web host feeds the
    /// daemon's workspace file list. Re-syncs an open FileMention
    /// picker so candidates arriving mid-typing appear immediately.
    pub fn set_file_mention_candidates(&mut self, candidates: Vec<String>) {
        self.file_mention_candidates = candidates
            .into_iter()
            .map(|path| {
                let path = path.replace('\\', "/");
                path.strip_prefix("./").map(str::to_string).unwrap_or(path)
            })
            .filter(|path| !path.is_empty())
            .collect();
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::FileMention)
        {
            self.sync_file_mention_picker();
        }
    }

    /// The active `@`-mention query (text between the trigger `@` and
    /// the cursor), or `None` when no mention is being typed. Hosts
    /// poll this after input mutations to decide when to (re)feed
    /// [`Self::set_file_mention_candidates`].
    pub fn file_mention_query(&self) -> Option<String> {
        self.active_file_mention().map(|(_, query)| query)
    }

    pub(in crate::panels::agent_pane::state) fn file_mention_root(
        &self,
    ) -> std::path::PathBuf {
        self.directory
            .as_deref()
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }

    pub(in crate::panels::agent_pane::state) fn apply_file_mention(
        &mut self,
        value: String,
    ) {
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

    /// Add a quiet, unboxed transcript line for a successful `/cd`.
    pub fn location_message(&mut self, text: impl Into<String>) {
        let mut message = NeoismAgentMessage::system("Directory", text);
        message.tool = "location_notice".to_string();
        self.messages.push(message);
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
            let mut message = NeoismAgentMessage::user(echo_prompt.clone());
            message.images = self.input_images();
            self.messages.push(message);
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
            message_id: crate::panels::agent_pane::outbound::next_prompt_message_id(),
            text: prompt.to_string(),
            parts,
            system,
            agent: self.agent.clone(),
            model: self.model.clone(),
            thinking: self.thinking.clone(),
            delivery: neoism_protocol::agent::PromptDelivery::Steer,
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

    pub(in crate::panels::agent_pane::state) fn set_agent_local(
        &mut self,
        value: String,
    ) -> String {
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
    /// "Neoism" while a conversation is already showing.
    pub fn start_new_conversation(&mut self) {
        self.session_id = None;
        self.parent_session_id = None;
        self.side_panel.set_viewed_session_id(None);
        // A fresh chat must not inherit the previous conversation's
        // grace-held status label.
        self.side_panel.clear_status_display_hold();
        self.clear_pending_user_prompts();
        self.prompt_echo_aliases.clear();
        self.messages.clear();
        self.reset_transient_timeline_interactions();
        self.reset_session_runtime_ui();
        self.invalidate_timeline_layout();
    }

    pub(in crate::panels::agent_pane::state) fn switch_session(
        &mut self,
        session_id: String,
    ) {
        let trimmed = session_id.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        // Already showing — nothing to do (desktop
        // `execute_switch_session_command` parity).
        if self.session_id.as_deref() == Some(trimmed.as_str()) {
            return;
        }
        if self.activate_cached_session(&trimmed) {
            // Instant restore from the session cache. The outbound
            // SwitchSession below still runs: the host re-binds the
            // daemon stream (ResumeStream) and issues a background
            // GetHistory whose snapshot reconciles through
            // `apply_history` — the web analogue of desktop's
            // resume-time event-stream snapshot refresh.
            self.push_outbound(OutboundAgentCommand::SwitchSession {
                session_id: trimmed,
            });
            return;
        }
        // Family bookkeeping, decided BEFORE ids change hands: a switch
        // within the tracked conversation family (parent ↔ child ↔
        // sibling) keeps the parent-keyed subagent roster alive so the
        // sidebar keeps showing sub-agent names/statuses while a child
        // transcript is open. Leaving the family clears it instead of
        // letting a stale roster shadow the new conversation.
        let stays_in_family = self.session_family_contains(&trimmed);
        let family_root = self
            .side_panel
            .subagents()
            .first()
            .map(|entry| entry.id.clone())
            .or_else(|| self.parent_session_id.clone());
        // Cold switch: park the current conversation first so returning
        // to it later is instant, then seed from whatever streamed into
        // the target's live-only cache slot while it was backgrounded.
        self.cache_current_session();
        let live_only = self.take_live_only_cache(&trimmed);
        self.session_id = Some(trimmed.clone());
        // Opening a roster child makes this a view-only subagent
        // transcript keyed to the family root (desktop restores the
        // same linkage from its cached SessionState); opening the root
        // — or leaving the family — clears it.
        self.parent_session_id = if stays_in_family {
            family_root.filter(|root| root != &trimmed)
        } else {
            None
        };
        if !stays_in_family {
            self.side_panel.invalidate_subagent_refresh();
        }
        self.side_panel.set_viewed_session_id(Some(trimmed.clone()));
        // Optimistic echoes belong to the session they were sent in.
        // Leaving them armed would let `apply_history`'s reconciliation
        // resurrect them inside the newly opened session's transcript.
        self.clear_pending_user_prompts();
        self.prompt_echo_aliases.clear();
        self.messages.clear();
        // The new timeline must render fresh — no raised/expanded card
        // artifacts from the click that navigated away.
        self.reset_transient_timeline_interactions();
        // Any session switch returns the panel to chat view — the "← Back"
        // home-override peek shouldn't linger onto the newly opened session.
        self.side_panel.set_show_home_override(false);
        self.reset_session_runtime_ui();
        self.reset_timeline_navigation_for_session_switch();
        if let Some(live_only) = live_only {
            // Background-streamed parts show immediately; the incoming
            // HistoryChunk reconciles them via `apply_history`'s
            // preserve/merge pipeline (desktop's SessionPreloaded
            // active-merge path).
            self.messages = live_only.messages;
            self.pending_user_prompts = live_only.pending_user_prompts;
            self.prompt_echo_aliases = live_only.prompt_echo_aliases;
            self.restore_session_runtime_ui(live_only.runtime);
        }
        self.invalidate_timeline_layout();
        self.push_outbound(OutboundAgentCommand::SwitchSession {
            session_id: trimmed,
        });
    }

    pub(in crate::panels::agent_pane::state) fn execute_slash_text(
        &mut self,
        text: &str,
    ) {
        use crate::panels::agent_pane::api_mapping::{
            normalize_model_ref, normalize_thinking,
        };
        use crate::panels::agent_pane::command_controller::{
            plan_slash_command, SlashCommandAction,
        };
        use crate::panels::agent_pane::view::fx::AgentFxKind;

        // Consume the SAME planned action table the desktop dispatcher
        // does (`desktop/src/neoism/agent/commands.rs`), minus the
        // direct HTTP calls: pure state mutations run inline; anything
        // that needs IO records the request on the outbound queue for
        // the host to drain.
        match plan_slash_command(text) {
            SlashCommandAction::Noop => {}
            SlashCommandAction::ShowHelp => self.show_help(),
            SlashCommandAction::ApplyModel(model) => {
                self.apply_model(normalize_model_ref(&model));
            }
            SlashCommandAction::OpenModelPicker => self.open_model_picker(),
            SlashCommandAction::OpenConnectPicker => self.open_connect_picker(),
            SlashCommandAction::ApplyThinking(value) => {
                self.apply_thinking(normalize_thinking(&value));
            }
            SlashCommandAction::OpenThinkingPicker => self.open_thinking_picker(),
            SlashCommandAction::ApplyAgent(agent) => self.apply_agent(agent),
            SlashCommandAction::OpenAgentPicker => self.open_agent_picker(),
            SlashCommandAction::SwitchSession(session_id) => {
                self.switch_session(session_id);
            }
            SlashCommandAction::OpenSessionsPicker => self.open_sessions_picker(),
            SlashCommandAction::ChangeDirectory(directory) => {
                // Desktop PATCHes the session synchronously; IO-free
                // here, the host executes the daemon-side `cd`.
                self.push_outbound(OutboundAgentCommand::SlashCommand {
                    name: "cd".to_string(),
                    args: directory,
                });
            }
            SlashCommandAction::OpenDirectoryPicker => self.open_directory_picker(),
            SlashCommandAction::OpenSubagentPicker => self.open_subagent_picker(),
            SlashCommandAction::ShowSkills => {
                self.push_outbound(OutboundAgentCommand::ShowSkills {
                    directory: self.directory.clone(),
                });
            }
            SlashCommandAction::ShowSkill(name) => self.show_skill(name),
            SlashCommandAction::ShowSkillUsage => {
                self.system_message("Skill", "usage: /skill info <name>");
            }
            SlashCommandAction::InsertSkillMentionByName(skill) => {
                self.insert_skill_mention_by_name(skill);
            }
            SlashCommandAction::OpenSkillPicker => self.open_skill_picker(),
            SlashCommandAction::HandleQueue(action) => {
                self.handle_queue(action.as_deref());
            }
            SlashCommandAction::ShowMcp => self.open_mcp_picker(),
            SlashCommandAction::ShowPermissions => self.show_permissions(),
            SlashCommandAction::ShowQuestions => self.show_questions(),
            SlashCommandAction::ToggleSkipPermissions => self.toggle_skip_permissions(),
            SlashCommandAction::HandlePermit(args) => self.handle_permit(&args),
            SlashCommandAction::HandleAnswer(answer) => self.handle_answer(&answer),
            SlashCommandAction::HandleReject(id) => self.handle_reject(id.as_deref()),
            SlashCommandAction::CompactSession => self.compact_session(),
            SlashCommandAction::UndoSession => self.undo_session(),
            SlashCommandAction::RedoSession => self.redo_session(),
            SlashCommandAction::ToggleInputHelp => {
                self.toggle_input_help();
            }
            SlashCommandAction::ToggleSidebar => {
                self.side_panel.toggle_visibility();
                let visible = !self.side_panel.user_hidden();
                self.push_outbound(OutboundAgentCommand::SetSidebarVisible { visible });
            }
            SlashCommandAction::PissOnScreen => {
                self.start_fx_easter_egg(AgentFxKind::Piss);
            }
            SlashCommandAction::CussOnScreen => {
                self.start_fx_easter_egg(AgentFxKind::Cuss);
            }
            SlashCommandAction::GlitchOnScreen => {
                self.start_fx_easter_egg(AgentFxKind::Glitch);
            }
            SlashCommandAction::DiscoOnScreen => {
                self.start_fx_easter_egg(AgentFxKind::Disco);
            }
            SlashCommandAction::GangFightOnScreen => {
                self.start_fx_easter_egg(AgentFxKind::GangFight);
            }
            SlashCommandAction::PraiseOnScreen => {
                self.start_fx_easter_egg(AgentFxKind::Praise);
            }
            SlashCommandAction::AbortSession => self.abort_session(),
            SlashCommandAction::CreateNewSession => self.start_new_conversation(),
            SlashCommandAction::RequestCloseTab => self.request_close_tab(),
            SlashCommandAction::RunServerCommand { command, args } => {
                self.run_server_command(&command, &args);
            }
            // The daemon websocket protocol has no goal envelope yet —
            // desktop drives `/goal` over its local HTTP surface. Mirror
            // desktop's session gate, then say so instead of inventing
            // wire messages (same posture as the question-reply gap in
            // `protocol_mapping.rs`).
            SlashCommandAction::ShowGoal
            | SlashCommandAction::SetGoal(_)
            | SlashCommandAction::ClearGoal
            | SlashCommandAction::PauseGoal
            | SlashCommandAction::ResumeGoal => {
                if self.session_id.is_none() {
                    self.system_message("Goal", "no session has started yet");
                } else {
                    self.system_message(
                        "Goal",
                        "goals aren't available over this connection yet",
                    );
                }
            }
        }
    }

    fn show_help(&mut self) {
        let body = slash_options()
            .into_iter()
            .map(|option| format!("{}  {}", option.title, option.description))
            .collect::<Vec<_>>()
            .join("\n");
        self.system_message("Commands", body);
    }

    /// `/skill info <name>` — desktop fetches the catalogue and prints
    /// the matching skill card. IO-free here: search the host-supplied
    /// cache and ask the host to refresh it for next time.
    fn show_skill(&mut self, name: String) {
        self.push_outbound(OutboundAgentCommand::RefreshSkills {
            directory: self.directory.clone(),
        });
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
            self.system_message(
                "Skill",
                format!(
                    "{}\n{}\n{}\n\nModel usage: call the skill tool with name \"{}\".",
                    option.title, option.description, option.footer, option.value
                ),
            );
        } else {
            self.system_message("Skill", format!("skill {name} not found"));
        }
    }

    fn undo_session(&mut self) {
        if self.session_id.is_none() {
            self.system_message("Undo", "no session has started yet");
            return;
        }
        self.push_outbound(OutboundAgentCommand::UndoSession);
    }

    fn redo_session(&mut self) {
        if self.session_id.is_none() {
            self.system_message("Redo", "no session has started yet");
            return;
        }
        self.push_outbound(OutboundAgentCommand::RedoSession);
    }

    fn handle_queue(&mut self, action: Option<&str>) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Queue", "no session has started yet");
            return;
        };
        self.push_outbound(OutboundAgentCommand::HandleQueue {
            session_id,
            action: action.map(str::to_string),
        });
    }

    fn show_permissions(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Permissions", "no session has started yet");
            return;
        };
        self.push_outbound(OutboundAgentCommand::ShowPermissions { session_id });
    }

    fn show_questions(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Questions", "no session has started yet");
            return;
        };
        self.push_outbound(OutboundAgentCommand::ShowQuestions { session_id });
    }

    fn handle_permit(&mut self, args: &[String]) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Permissions", "no session has started yet");
            return;
        };
        let reply =
            permission_reply_alias(args.first().map(String::as_str).unwrap_or("once"));
        let id = args
            .get(1)
            .map(String::as_str)
            .or_else(|| {
                args.first()
                    .map(String::as_str)
                    .filter(|value| !is_permission_reply(value))
            })
            .map(str::to_string);
        self.push_outbound(OutboundAgentCommand::HandlePermit {
            session_id,
            reply: reply.to_string(),
            id,
        });
    }

    fn handle_answer(&mut self, answer: &str) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Questions", "no session has started yet");
            return;
        };
        if answer.trim().is_empty() {
            self.system_message("Questions", "usage: /answer <text>");
            return;
        }
        self.push_outbound(OutboundAgentCommand::HandleAnswer {
            session_id,
            answer: answer.to_string(),
        });
    }

    fn handle_reject(&mut self, id_arg: Option<&str>) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Interaction", "no session has started yet");
            return;
        };
        self.push_outbound(OutboundAgentCommand::HandleReject {
            session_id,
            id: id_arg.map(str::to_string),
        });
    }

    fn run_server_command(&mut self, command: &str, command_args: &str) {
        if self.session_id.is_none() {
            self.push_outbound(OutboundAgentCommand::EnsureSession);
        }
        self.push_outbound(OutboundAgentCommand::SlashCommand {
            name: command.to_string(),
            args: command_args.to_string(),
        });
    }

    pub fn apply_model(&mut self, value: String) {
        self.remember_model_value(&value);
        self.set_model_local(value.clone());
        if !value.trim().is_empty() {
            self.push_outbound(OutboundAgentCommand::PersistConfigChoice {
                model: Some(value.clone()),
                thinking: None,
            });
        }
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

    pub(in crate::panels::agent_pane::state) fn set_model_local(
        &mut self,
        value: String,
    ) {
        let changed = self.model != value;
        self.model = value;
        if changed {
            self.model_context_limit =
                self.model_context_limits.get(&self.model).copied();
        }
    }

    pub(in crate::panels::agent_pane::state) fn remember_model_value(
        &mut self,
        value: &str,
    ) {
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

    pub(in crate::panels::agent_pane::state) fn remember_model_option(
        &mut self,
        option: &NeoismAgentPickerOption,
    ) {
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
        if thinking.is_some() {
            self.push_outbound(OutboundAgentCommand::PersistConfigChoice {
                model: None,
                thinking: thinking.clone(),
            });
        }
        if let Some(session_id) = self.session_id.clone() {
            self.push_outbound(OutboundAgentCommand::ApplyThinking {
                session_id,
                model: self.model.clone(),
                thinking,
            });
        }
    }

    pub(in crate::panels::agent_pane::state) fn set_thinking_local(
        &mut self,
        value: String,
    ) -> Option<String> {
        let thinking = (!value.trim().is_empty()).then_some(value);
        self.thinking = thinking.clone();
        thinking
    }

    pub(in crate::panels::agent_pane::state) fn abort_session(&mut self) {
        // A user-driven stop is a hard clear: the status label must not
        // linger through the display grace hold, or Stop reads as lag.
        self.side_panel.clear_status_display_hold();
        // Mirrors desktop `commands::abort_session`: without a session
        // there's nothing for the host to abort.
        if self.session_id.is_none() {
            self.note_streaming(NeoismAgentStreamingState::Idle, None);
            self.system_message("Abort", "no session has started yet");
            return;
        }
        self.abort_requested_at = Some(Instant::now());
        self.note_streaming(NeoismAgentStreamingState::Idle, None);
        self.settle_tracked_subagents(BranchStatus::Stopped);
        self.push_outbound(OutboundAgentCommand::AbortSession);
    }

    pub(in crate::panels::agent_pane::state) fn compact_session(&mut self) {
        // No session yet → nothing to compact (matches desktop).
        if self.session_id.is_none() {
            self.system_message("Context", "no session has started yet");
            return;
        }
        // The host turns the `CompactSession` outbound command into the
        // actual `/api/session/.../compact` POST. The pane enters Compacting
        // only after the backend emits the same compaction.started event used
        // by auto compaction.
        self.push_outbound(OutboundAgentCommand::CompactSession);
    }

    pub(in crate::panels::agent_pane) fn insert_skill_mention_by_name(
        &mut self,
        name: String,
    ) {
        self.apply_skill_mention(NeoismAgentPickerOption::new(&name, "", "skill", &name));
    }

    pub(in crate::panels::agent_pane::state) fn apply_skill_mention(
        &mut self,
        option: NeoismAgentPickerOption,
    ) {
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

    pub(in crate::panels::agent_pane::state) fn apply_inline_skill_mention(
        &mut self,
        option: NeoismAgentPickerOption,
    ) {
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

    pub(in crate::panels::agent_pane::state) fn remember_file_mention(
        &mut self,
        token: &str,
        value: &str,
    ) {
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

    /// A pasted single-line file path / `file://` URI that resolves to
    /// an attachable file on the host filesystem (desktop parity —
    /// `pane/submit.rs`). Hosts without a local filesystem (wasm)
    /// always fall through: `is_file()` is `false` there.
    pub(in crate::panels::agent_pane::state) fn pasted_attachment_path(
        &self,
        text: &str,
    ) -> Option<std::path::PathBuf> {
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

    pub(in crate::panels::agent_pane::state) fn file_attachment_token(
        &self,
        path: &std::path::Path,
        mime: &str,
    ) -> String {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file");
        self.file_attachment_token_for(filename, mime)
    }

    /// `[imageN]` / `[pdfN]` / `[fileN: name]` token for the next
    /// attachment of `mime` — shared by the path- and byte-based
    /// attach flows (`attachment_policy::file_attachment_token`
    /// numbering, counted per attachment class like desktop).
    pub(in crate::panels::agent_pane::state) fn file_attachment_token_for(
        &self,
        filename: &str,
        mime: &str,
    ) -> String {
        let next = if mime.starts_with("image/") {
            self.file_attachment_count(|mime| mime.starts_with("image/")) + 1
        } else if mime == "application/pdf" {
            self.file_attachment_count(|mime| mime == "application/pdf") + 1
        } else {
            self.file_attachment_count(|mime| {
                !mime.starts_with("image/") && mime != "application/pdf"
            }) + 1
        };
        crate::panels::agent_pane::attachment_policy::file_attachment_token(
            filename, mime, next,
        )
    }

    pub(in crate::panels::agent_pane::state) fn file_attachment_count<F>(
        &self,
        mut predicate: F,
    ) -> usize
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

    pub(in crate::panels::agent_pane::state) fn unique_attachment_token(
        &self,
        base: &str,
    ) -> String {
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

    pub(in crate::panels::agent_pane) fn prompt_parts_for(
        &self,
        text: &str,
    ) -> Vec<Value> {
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

    pub(in crate::panels::agent_pane) fn prompt_system_for(
        &self,
        text: &str,
    ) -> Option<String> {
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

    pub(in crate::panels::agent_pane::state) fn expand_text_attachments(
        &self,
        text: &str,
    ) -> String {
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

/// `/permit` reply aliasing — mirrors `permission_reply_alias` in the
/// desktop's `agent/api.rs`.
fn permission_reply_alias(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "a" | "always" => "always",
        "n" | "no" | "deny" | "reject" => "reject",
        _ => "once",
    }
}

/// Whether a `/permit` argument names a reply (vs a permission id) —
/// mirrors `is_permission_reply` in the desktop's `agent/api.rs`.
fn is_permission_reply(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "once" | "always" | "reject" | "y" | "a" | "n" | "yes" | "no" | "deny"
    )
}
