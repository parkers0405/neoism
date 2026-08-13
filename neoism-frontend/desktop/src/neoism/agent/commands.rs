use std::collections::HashSet;
use std::thread;
use std::time::{Duration, Instant};

use neoism_ui::panels::agent_pane::command_controller::{self, SlashCommandAction};
use neoism_ui::panels::agent_pane::outbound::OutboundAgentCommand;
use serde_json::{json, Value};

use super::api::{
    api_request_json, api_request_json_with_read_timeout, fetch_directory_options,
    fetch_session_messages, fetch_session_messages_page, fetch_session_state,
    fetch_skill_options, first_interaction_id, first_interaction_value,
    format_permissions, format_questions, format_queue, is_permission_reply,
    normalize_model_ref, normalize_thinking, percent_encode, permission_reply_alias,
    prompt_model_json, question_answers, question_count, session_model_json,
};
use super::pane::{
    merge_session_snapshot, CachedAgentSession, NeoismAgentBackgroundUpdate,
    NeoismAgentMessage, NeoismAgentMode, NeoismAgentNoticeLevel, NeoismAgentPane,
    NeoismAgentStreamingState,
};
use super::picker::{NeoismAgentPicker, NeoismAgentPickerKind, NeoismAgentPickerOption};
use super::side_panel::SessionGoal;

impl NeoismAgentPane {
    pub(super) fn execute_slash_text(&mut self, text: &str) {
        match command_controller::plan_slash_command(text) {
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
                self.change_directory(directory)
            }
            SlashCommandAction::OpenDirectoryPicker => self.open_directory_picker(),
            SlashCommandAction::OpenSubagentPicker => self.open_subagent_picker(),
            SlashCommandAction::ShowSkills => self.show_skills(),
            SlashCommandAction::ShowSkill(name) => self.show_skill(name),
            SlashCommandAction::ShowSkillUsage => {
                self.system_message("Skill", "usage: /skill info <name>");
            }
            SlashCommandAction::InsertSkillMentionByName(skill) => {
                self.insert_skill_mention_by_name(skill);
            }
            SlashCommandAction::OpenSkillPicker => self.open_skill_picker(),
            SlashCommandAction::HandleQueue(action) => {
                self.handle_queue(action.as_deref())
            }
            SlashCommandAction::ShowMcp => self.show_mcp(),
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
                self.side_panel_mut().toggle_visibility();
                let visible = !self.side_panel().user_hidden();
                self.push_outbound(OutboundAgentCommand::SetSidebarVisible { visible });
            }
            SlashCommandAction::PissOnScreen => self.start_fx_easter_egg(
                neoism_ui::panels::agent_pane::view::fx::AgentFxKind::Piss,
            ),
            SlashCommandAction::CussOnScreen => self.start_fx_easter_egg(
                neoism_ui::panels::agent_pane::view::fx::AgentFxKind::Cuss,
            ),
            SlashCommandAction::GlitchOnScreen => self.start_fx_easter_egg(
                neoism_ui::panels::agent_pane::view::fx::AgentFxKind::Glitch,
            ),
            SlashCommandAction::DiscoOnScreen => self.start_fx_easter_egg(
                neoism_ui::panels::agent_pane::view::fx::AgentFxKind::Disco,
            ),
            SlashCommandAction::GangFightOnScreen => self.start_fx_easter_egg(
                neoism_ui::panels::agent_pane::view::fx::AgentFxKind::GangFight,
            ),
            SlashCommandAction::PraiseOnScreen => self.start_fx_easter_egg(
                neoism_ui::panels::agent_pane::view::fx::AgentFxKind::Praise,
            ),
            SlashCommandAction::AbortSession => self.abort_session(),
            SlashCommandAction::CreateNewSession => self.create_new_session(),
            SlashCommandAction::RequestCloseTab => self.request_close_tab(),
            SlashCommandAction::RunServerCommand { command, args } => {
                self.run_server_command(&command, &args);
            }
            SlashCommandAction::ShowGoal => self.show_goal(),
            SlashCommandAction::SetGoal(text) => self.set_goal(text),
            SlashCommandAction::ClearGoal => self.clear_goal(),
            SlashCommandAction::PauseGoal => self.set_goal_paused(true),
            SlashCommandAction::ResumeGoal => self.set_goal_paused(false),
        }
    }

    fn open_directory_picker(&mut self) {
        let session_id = match self.ensure_session() {
            Ok(session_id) => session_id,
            Err(error) => {
                self.system_message("Directory", error);
                return;
            }
        };
        match fetch_directory_options(
            &self.server,
            &session_id,
            self.directory.as_deref(),
        ) {
            Ok(options) if !options.is_empty() => {
                let selected = options
                    .iter()
                    .position(|option| option.is_current)
                    .unwrap_or(0);
                let mut picker = NeoismAgentPicker::new(
                    NeoismAgentPickerKind::Directory,
                    "Change directory",
                    options,
                    selected,
                );
                picker.search_placeholder = Some("Path or fuzzy directory".to_string());
                self.picker = Some(picker);
            }
            Ok(_) => self.system_message("Directory", "no directories found"),
            Err(error) => self.system_message("Directory", error),
        }
    }

    pub(super) fn change_directory(&mut self, requested: String) {
        let session_id = match self.ensure_session() {
            Ok(session_id) => session_id,
            Err(error) => {
                self.system_message("Directory", error);
                return;
            }
        };
        let body = json!({ "directory": requested });
        match api_request_json(
            &self.server,
            "PATCH",
            &format!("/session/{session_id}"),
            Some(&body),
        ) {
            Ok(Some(value)) => {
                let directory = value
                    .get("directory")
                    .and_then(Value::as_str)
                    .unwrap_or(requested.as_str())
                    .to_string();
                self.directory = Some(directory.clone());
                self.invalidate_skill_options();
                self.close_picker();
                self.side_panel.invalidate_goal_refresh();
                let mut message = NeoismAgentMessage::system(
                    "Directory",
                    format!("Switched location to {directory}"),
                );
                message.tool = "location_notice".to_string();
                self.upsert_part_message(message);
            }
            Ok(None) => self.system_message("Directory", "server returned no session"),
            Err(error) => self.system_message("Directory failed", error),
        }
    }

    fn show_goal(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Goal", "no session has started yet");
            return;
        };
        match api_request_json(
            &self.server,
            "GET",
            &format!("/session/{session_id}/goal"),
            None,
        ) {
            Ok(value) => {
                let value = value.unwrap_or(Value::Null);
                let goal = value.get("goal").filter(|goal| !goal.is_null());
                let text = goal
                    .and_then(|goal| goal.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if text.is_empty() {
                    self.system_message("Goal", "no goal set — use /goal <text>");
                } else {
                    let paused = goal
                        .and_then(|goal| goal.get("paused"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let research = value
                        .get("researchEnabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let pause_suffix = if paused { " (paused)" } else { "" };
                    let suffix = if research {
                        ""
                    } else {
                        " (research disabled: set FIRECRAWL_API_KEY)"
                    };
                    self.system_message("Goal", format!("{text}{pause_suffix}{suffix}"));
                }
            }
            Err(error) => self.system_message("Goal", error),
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

    fn set_goal(&mut self, text: String) {
        let session_id = match self.ensure_session() {
            Ok(session_id) => session_id,
            Err(error) => {
                self.system_message("Goal failed", error);
                return;
            }
        };
        let body = json!({ "text": text });
        match api_request_json(
            &self.server,
            "POST",
            &format!("/session/{session_id}/goal"),
            Some(&body),
        ) {
            Ok(value) => {
                self.apply_goal_response(value.as_ref());
                self.system_message("Goal", format!("goal set: {text}"));
                self.start_goal_prompt(text);
            }
            Err(error) => self.system_message("Goal", error),
        }
    }

    fn clear_goal(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Goal", "no session has started yet");
            return;
        };
        match api_request_json(
            &self.server,
            "DELETE",
            &format!("/session/{session_id}/goal"),
            None,
        ) {
            Ok(value) => {
                self.apply_goal_response(value.as_ref());
                self.system_message("Goal", "goal cleared");
            }
            Err(error) => self.system_message("Goal", error),
        }
    }

    /// Reflect a `/goal` mutation (set / clear / pause / resume) in the side
    /// panel immediately, instead of waiting for the next incidental
    /// `SESSION_UPDATED`. The POST/DELETE `/goal` response is authoritative —
    /// `{ "goal": <goal|null> }` — so a present goal applies with its own
    /// monotonic `updated` version, and a null goal force-clears the section.
    /// A refetch is invalidated afterward so any backend-canonical detail (a
    /// research summary, a normalized status) still lands correctly.
    fn apply_goal_response(&mut self, value: Option<&Value>) {
        let goal = value
            .and_then(|value| value.get("goal"))
            .and_then(SessionGoal::from_json);
        match goal {
            Some(goal) => {
                let version = goal.updated;
                self.side_panel.set_session_goal(Some(goal), version);
            }
            None => self.side_panel.clear_session_goal_local(),
        }
        self.side_panel.invalidate_goal_refresh();
    }

    fn set_goal_paused(&mut self, paused: bool) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Goal", "no session has started yet");
            return;
        };
        let current = match api_request_json(
            &self.server,
            "GET",
            &format!("/session/{session_id}/goal"),
            None,
        ) {
            Ok(Some(value)) => value,
            Ok(None) => Value::Null,
            Err(error) => {
                self.system_message("Goal", error);
                return;
            }
        };
        let Some(goal) = current.get("goal").filter(|goal| !goal.is_null()) else {
            self.system_message("Goal", "no goal set");
            return;
        };
        let text = goal.get("text").and_then(Value::as_str).unwrap_or_default();
        if text.trim().is_empty() {
            self.system_message("Goal", "no goal set");
            return;
        }
        let mut body = json!({ "text": text, "paused": paused });
        if let Some(research) = goal.get("research") {
            body["research"] = research.clone();
        }
        match api_request_json(
            &self.server,
            "POST",
            &format!("/session/{session_id}/goal"),
            Some(&body),
        ) {
            Ok(value) => {
                self.apply_goal_response(value.as_ref());
                self.system_message(
                    "Goal",
                    if paused {
                        "goal paused"
                    } else {
                        "goal resumed"
                    },
                );
            }
            Err(error) => self.system_message("Goal", error),
        }
    }

    pub(super) fn apply_agent(&mut self, value: String) {
        self.agent = (!value.is_empty()).then_some(value.clone());
        match value.as_str() {
            "build" => self.mode = NeoismAgentMode::Build,
            "plan" => self.mode = NeoismAgentMode::Plan,
            _ => {}
        }
        self.close_picker();
        if let Some(session_id) = self.session_id.clone() {
            if !value.is_empty() {
                self.push_outbound(OutboundAgentCommand::ApplyAgent {
                    session_id,
                    agent: value,
                });
            }
        }
        self.system_message("Agent", format!("agent {}", self.agent_label()));
    }

    pub(super) fn execute_apply_agent_command(
        &mut self,
        session_id: String,
        agent: String,
    ) {
        let body = json!({ "agent": agent });
        if let Err(error) = api_request_json(
            &self.server,
            "PATCH",
            &format!("/session/{session_id}"),
            Some(&body),
        ) {
            self.system_message("Agent", error);
        }
    }

    /// Persist a renamed session title at the daemon level (right-click →
    /// Rename on an agent tab). Mirrors `execute_apply_agent_command`'s
    /// `PATCH /session/{id}` shape with a `{ "title": ... }` body.
    pub(super) fn execute_set_title_command(
        &mut self,
        session_id: String,
        title: String,
    ) {
        let body = json!({ "title": title });
        if let Err(error) = api_request_json(
            &self.server,
            "PATCH",
            &format!("/session/{session_id}"),
            Some(&body),
        ) {
            self.system_message("Rename", error);
        }
    }

    pub(super) fn apply_model(&mut self, value: String) {
        self.remember_model_value(&value);
        self.model = value;
        if !self.model.trim().is_empty() {
            self.push_outbound(OutboundAgentCommand::PersistConfigChoice {
                model: Some(self.model.clone()),
                thinking: None,
            });
        }
        self.refresh_model_context_limit();
        self.close_picker();
        if let Some(session_id) = self.session_id.clone() {
            if let Some(model) =
                session_model_json(self.model.as_str(), self.thinking.as_deref())
            {
                self.push_outbound(OutboundAgentCommand::ApplyModel {
                    session_id,
                    model,
                });
            }
        }
        self.system_message("Model", format!("model {}", self.model()));
    }

    pub(super) fn execute_apply_model_command(
        &mut self,
        session_id: String,
        model: Value,
    ) {
        let body = json!({ "model": model });
        if let Err(error) = api_request_json(
            &self.server,
            "PATCH",
            &format!("/session/{session_id}"),
            Some(&body),
        ) {
            self.system_message("Model", error);
        }
    }

    pub(super) fn apply_thinking(&mut self, value: String) {
        self.thinking = (!value.is_empty()).then_some(value);
        if self.thinking.is_some() {
            self.push_outbound(OutboundAgentCommand::PersistConfigChoice {
                model: None,
                thinking: self.thinking.clone(),
            });
        }
        self.close_picker();
        if let Some(session_id) = self.session_id.clone() {
            self.push_outbound(OutboundAgentCommand::ApplyThinking {
                session_id,
                model: self.model.clone(),
                thinking: self.thinking.clone(),
            });
        }
        self.system_message("Think", format!("think {}", self.thinking_label()));
    }

    pub(super) fn execute_apply_thinking_command(
        &mut self,
        session_id: String,
        model: String,
        thinking: Option<String>,
    ) {
        let Some(model_json) = session_model_json(model.as_str(), thinking.as_deref())
        else {
            return;
        };
        let body = json!({ "model": model_json });
        if let Err(error) = api_request_json(
            &self.server,
            "PATCH",
            &format!("/session/{session_id}"),
            Some(&body),
        ) {
            self.system_message("Think", error);
        }
    }

    pub(super) fn switch_session(&mut self, session_id: String) {
        if session_id.is_empty() {
            return;
        }
        self.push_outbound(OutboundAgentCommand::SwitchSession { session_id });
    }

    pub(super) fn execute_switch_session_command(&mut self, session_id: String) {
        if session_id.is_empty() {
            return;
        }
        if self
            .session_cache
            .get(&session_id)
            .is_some_and(|cached| cached.hydrated)
        {
            self.activate_cached_session(&session_id);
            return;
        }
        // Keep the current transcript painted while the target hydrates off
        // the UI thread. A live child cache may already contain streamed
        // deltas; the preload result merges with them before activation.
        self.pending_session_switch = Some(session_id.clone());
        self.ensure_session_preloaded(session_id, false);
    }

    pub(crate) fn ensure_session_preloaded(&mut self, session_id: String, force: bool) {
        if session_id.is_empty() {
            return;
        }
        if !force
            && self
                .session_cache
                .get(&session_id)
                .is_some_and(|cached| cached.hydrated)
        {
            return;
        }
        if !self.session_preloads_in_flight.insert(session_id.clone()) {
            return;
        }
        self.session_cache
            .entry(session_id.clone())
            .or_insert_with(CachedAgentSession::live_only);
        let server = self.server.clone();
        let tx = self.background_sender();
        let thread_session_id = session_id.clone();
        let spawn = thread::Builder::new()
            .name(format!("neoism-agent-preload-{thread_session_id}"))
            .spawn(move || {
                let update = fetch_session_state(&server, &thread_session_id)
                    .and_then(|state| {
                        fetch_session_messages_page(
                            &server,
                            &thread_session_id,
                            None,
                            100,
                        )
                        .map(|page| {
                            NeoismAgentBackgroundUpdate::SessionPreloaded {
                                session_id: thread_session_id.clone(),
                                state,
                                messages: page.blocks,
                                oldest_cursor: page.oldest_cursor,
                            }
                        })
                    })
                    .unwrap_or_else(|error| {
                        NeoismAgentBackgroundUpdate::SessionPreloadFailed {
                            session_id: thread_session_id,
                            error,
                        }
                    });
                let _ = tx.send(update);
            });
        if let Err(error) = spawn {
            self.session_preloads_in_flight.remove(&session_id);
            if self.pending_session_switch.as_deref() == Some(session_id.as_str()) {
                self.pending_session_switch = None;
                self.system_message(
                    "Session",
                    format!("failed to preload session: {error}"),
                );
            }
        }
    }

    pub(crate) fn cache_current_session(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        let state = neoism_ui::panels::agent_pane::api_mapping::SessionState {
            agent: self.agent.clone(),
            model: (!self.model.is_empty()).then(|| self.model.clone()),
            thinking: self.thinking.clone(),
            parent_id: self.parent_session_id.clone(),
            directory: self.directory.clone(),
        };
        let live = self
            .session_cache
            .remove(&session_id)
            .map(|cached| cached.messages)
            .unwrap_or_default();
        self.session_cache.insert(
            session_id,
            CachedAgentSession {
                state,
                messages: merge_session_snapshot(self.messages.clone(), live),
                timeline_history: self.timeline_history.clone(),
                timeline_scroll_px: self.timeline_scroll_px,
                timeline_follow_bottom: self.timeline_follow_bottom,
                hydrated: true,
            },
        );
    }

    pub(crate) fn activate_cached_session(&mut self, session_id: &str) {
        if self.session_id.as_deref() == Some(session_id) {
            self.pending_session_switch = None;
            return;
        }
        let Some(cached) = self.session_cache.get(session_id).cloned() else {
            return;
        };
        self.cache_current_session();
        let state = cached.state;
        if state.parent_id.is_none() {
            self.session_tree_root_id = Some(session_id.to_string());
        } else if self.session_tree_root_id.is_none() {
            self.session_tree_root_id = state.parent_id.clone();
        }
        self.clear_pending_user_prompts();
        self.session_id = Some(session_id.to_string());
        self.parent_session_id = state.parent_id.clone();
        self.side_panel
            .set_viewed_session_id(Some(session_id.to_string()));
        self.input.clear();
        self.close_picker();
        self.reset_session_runtime_ui();
        self.timeline_history = cached.timeline_history;
        self.timeline_scroll_px = cached.timeline_scroll_px;
        self.timeline_follow_bottom = cached.timeline_follow_bottom;
        self.side_panel.set_show_home_override(false);
        self.side_panel.invalidate_subagent_refresh();
        self.side_panel.reset_session_goal();
        self.side_panel.invalidate_goal_refresh();
        if let Some(directory) = state.directory {
            self.directory = Some(directory);
            self.invalidate_skill_options();
        }
        if let Some(agent) = state.agent {
            self.agent = Some(agent);
        }
        if let Some(model) = state.model {
            self.model = model;
        }
        self.thinking = state.thinking;
        self.execute_refresh_model_context_limit_command();
        if self.is_subagent_session() {
            self.clear_composer();
            self.set_cursor_rect(None);
            self.close_picker();
        }
        self.messages = cached.messages;
        self.invalidate_timeline_layout();
        self.hydrate_runtime_status_for_session(session_id);
        let stream_session_id = self
            .session_tree_root_id
            .clone()
            .unwrap_or_else(|| session_id.to_string());
        self.start_session_updates(&stream_session_id);
        self.pending_session_switch = None;
    }

    pub(super) fn send_prompt(
        &mut self,
        text: &str,
        transcript_echo: bool,
    ) -> Result<(), String> {
        if self.is_subagent_session() {
            return Err("subagent sessions are view-only".to_string());
        }
        let prompt = self.expand_text_attachments(text);
        let parts = self.prompt_parts_for(&prompt);
        let system = self.prompt_system_for(&prompt);
        if self.session_id.is_none() {
            self.push_outbound(OutboundAgentCommand::EnsureSession);
        }
        self.push_outbound(OutboundAgentCommand::SendPrompt {
            message_id: neoism_ui::panels::agent_pane::outbound::next_prompt_message_id(),
            text: prompt,
            parts,
            system,
            agent: self.agent.clone(),
            model: self.model.clone(),
            thinking: self.thinking.clone(),
            delivery: neoism_protocol::agent::PromptDelivery::Steer,
            transcript_echo,
        });
        Ok(())
    }

    pub(super) fn execute_send_prompt_command(
        &mut self,
        message_id: String,
        text: String,
        parts: Vec<Value>,
        system: Option<String>,
        agent: Option<String>,
        model: String,
        thinking: Option<String>,
        delivery: neoism_protocol::agent::PromptDelivery,
        transcript_echo: bool,
    ) -> Result<(), String> {
        if self.is_subagent_session() {
            return Err("subagent sessions are view-only".to_string());
        }
        let session_id = self.ensure_session()?;
        // Stamp who is sending this prompt so a shared/joined session
        // attributes the turn to the true sender: the host's agent-server
        // persists this on the user message and re-broadcasts it to every
        // attached client, and a remote peer renders THIS name + its
        // deterministic presence orb (instead of a generic "You"). The local
        // sender's own bubble still reads "You" — its optimistic echo carries
        // no author and the server echo is deduped onto it by text.
        let author = self.local_presence_name().map(str::to_string);
        let body = json!({
            "messageId": message_id,
            "model": prompt_model_json(model.as_str(), thinking.as_deref()),
            "agent": agent,
            "noReply": false,
            "system": system,
            "tools": null,
            "author": author,
            "parts": parts,
            "delivery": match delivery {
                neoism_protocol::agent::PromptDelivery::Steer => "steer",
                neoism_protocol::agent::PromptDelivery::Queue => "queue",
            },
        });
        self.start_session_updates(&session_id);
        api_request_json(
            &self.server,
            "POST",
            &format!("/api/session/{session_id}/prompt"),
            Some(&body),
        )?;
        if transcript_echo {
            // Pending prompts must match the transcript echo, which uses
            // the compact composer form for pasted attachments.
            let echo = self
                .compact_user_prompt_text(&text)
                .unwrap_or_else(|| text.clone());
            self.remember_pending_user_prompt(&echo);
        }
        Ok(())
    }

    pub(super) fn system_message(
        &mut self,
        title: impl AsRef<str>,
        body: impl Into<String>,
    ) {
        let title = title.as_ref();
        let body = body.into();
        let title = if title.is_empty() { "System" } else { title };
        if body.contains('\n') || body.chars().count() > 140 {
            self.push_dialog(title.to_string(), body);
            return;
        }
        let level = if title.to_ascii_lowercase().contains("failed") {
            NeoismAgentNoticeLevel::Error
        } else if body.starts_with("no ") || body.starts_with("usage:") {
            NeoismAgentNoticeLevel::Warn
        } else {
            NeoismAgentNoticeLevel::Info
        };
        self.push_notice(format!("{title}: {body}"), level);
    }

    fn show_help(&mut self) {
        let body = slash_options()
            .into_iter()
            .map(|option| format!("{}  {}", option.title, option.description))
            .collect::<Vec<_>>()
            .join("\n");
        self.system_message("Commands", body);
    }

    fn show_skills(&mut self) {
        self.push_outbound(OutboundAgentCommand::ShowSkills {
            directory: self.directory.clone(),
        });
    }

    pub(super) fn execute_show_skills_command(&mut self, directory: Option<String>) {
        match fetch_skill_options(&self.server, directory.as_deref()) {
            Ok(options) if !options.is_empty() => {
                let body = options
                    .into_iter()
                    .map(|option| {
                        if option.description.is_empty() {
                            format!("{}  {}", option.title, option.footer)
                        } else {
                            format!(
                                "{}  {}  {}",
                                option.title, option.description, option.footer
                            )
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.system_message("Skills", body);
            }
            Ok(_) => self.system_message("Skills", "no skills discovered"),
            Err(error) => self.system_message("Skills", error),
        }
    }

    pub(super) fn show_skill(&mut self, name: String) {
        match fetch_skill_options(&self.server, self.directory.as_deref()) {
            Ok(options) => {
                let needle = name.to_ascii_lowercase();
                if let Some(option) = options.into_iter().find(|option| {
                    option.value.eq_ignore_ascii_case(&name)
                        || option.title.eq_ignore_ascii_case(&name)
                        || option.title.to_ascii_lowercase().contains(&needle)
                }) {
                    self.system_message(
                        "Skill",
                        format!(
                            "{}\n{}\n{}\n\nModel usage: call the skill tool with name \"{}\".",
                            option.title,
                            option.description,
                            option.footer,
                            option.value
                        ),
                    );
                } else {
                    self.system_message("Skill", format!("skill {name} not found"));
                }
            }
            Err(error) => self.system_message("Skill", error),
        }
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

    pub(super) fn execute_stop_background_task_command(
        &mut self,
        session_id: String,
        job_id: String,
    ) {
        match api_request_json(
            &self.server,
            "DELETE",
            &format!("/session/{session_id}/background-task/{job_id}"),
            None,
        ) {
            Ok(_) => self.system_message("Background task", format!("Stopping {job_id}")),
            Err(error) => self.system_message("Background task", error),
        }
    }

    pub(super) fn execute_handle_queue_command(
        &mut self,
        session_id: String,
        action: Option<String>,
    ) {
        let result = match action.as_deref() {
            Some("clear") => api_request_json(
                &self.server,
                "DELETE",
                &format!("/session/{session_id}/queue"),
                None,
            ),
            Some("pop") => api_request_json(
                &self.server,
                "POST",
                &format!("/session/{session_id}/queue/pop"),
                None,
            ),
            _ => api_request_json(
                &self.server,
                "GET",
                &format!("/session/{session_id}/queue"),
                None,
            ),
        };
        match result {
            Ok(value) => self.system_message("Queue", format_queue(value.as_ref())),
            Err(error) => self.system_message("Queue", error),
        }
    }

    pub(super) fn show_mcp(&mut self) {
        let path = self
            .directory
            .as_deref()
            .map(|dir| format!("/mcp/catalog?directory={}", percent_encode(dir)))
            .unwrap_or_else(|| "/mcp/catalog".to_string());
        match api_request_json(&self.server, "GET", &path, None) {
            Ok(value) => {
                let options =
                    neoism_ui::panels::agent_pane::state::mcp_options_from_status(
                        value.as_ref().unwrap_or(&Value::Null),
                    );
                self.picker = Some(NeoismAgentPicker::new(
                    NeoismAgentPickerKind::Mcp,
                    "MCP servers",
                    options,
                    0,
                ));
            }
            Err(error) => self.system_message("MCP", error),
        }
    }

    pub(super) fn begin_mcp_oauth(&mut self, name: String) {
        let directory = self
            .directory
            .as_deref()
            .map(|dir| format!("?directory={}", percent_encode(dir)))
            .unwrap_or_default();
        let path = format!("/mcp/{}/auth{directory}", percent_encode(&name));
        let value = match api_request_json(&self.server, "POST", &path, Some(&json!({})))
        {
            Ok(value) => value.unwrap_or(Value::Null),
            Err(error) => {
                self.system_message(&name, error);
                return;
            }
        };
        let Some(url) = value.get("authorizationUrl").and_then(Value::as_str) else {
            self.system_message(&name, "MCP server returned no authorization URL");
            return;
        };
        let browser_error = crate::background_process::open_url(url).err();
        let lead = browser_error.map_or_else(
            || "Opened your browser to authorize this MCP server.".to_string(),
            |error| format!("Could not open your browser ({error})."),
        );
        self.system_message(
            name,
            format!("{lead}\n\nAuthorization URL:\n[{url}]({url})"),
        );
    }

    pub(super) fn execute_mcp_action(&mut self, value: &Value) {
        let Some(name) = value.get("name").and_then(Value::as_str) else {
            return;
        };
        let Some(action) = value.get("action").and_then(Value::as_str) else {
            return;
        };
        if action == "authenticate" {
            self.begin_mcp_oauth(name.to_string());
            return;
        }
        let directory = self
            .directory
            .as_deref()
            .map(|dir| format!("?directory={}", percent_encode(dir)))
            .unwrap_or_default();
        let (method, path, body) = match action {
            "enable" => (
                "PATCH",
                format!("/mcp/{}/config{directory}", percent_encode(name)),
                Some(json!({ "enabled": true })),
            ),
            "disable" => (
                "PATCH",
                format!("/mcp/{}/config{directory}", percent_encode(name)),
                Some(json!({ "enabled": false })),
            ),
            "connect" => (
                "POST",
                format!("/mcp/{}/connect{directory}", percent_encode(name)),
                Some(json!({})),
            ),
            "disconnect" => (
                "POST",
                format!("/mcp/{}/disconnect{directory}", percent_encode(name)),
                Some(json!({})),
            ),
            "logout" => (
                "DELETE",
                format!("/mcp/{}/auth{directory}", percent_encode(name)),
                None,
            ),
            _ => return,
        };
        let result = if action == "connect" {
            api_request_json_with_read_timeout(
                &self.server,
                method,
                &path,
                body.as_ref(),
                Duration::from_secs(35),
            )
        } else {
            api_request_json(&self.server, method, &path, body.as_ref())
        };
        match result {
            Ok(Some(Value::Bool(false))) if action == "connect" => {
                self.begin_mcp_oauth(name.to_string())
            }
            Ok(_) => self.show_mcp(),
            Err(error) => self.system_message(name, error),
        }
    }

    fn show_permissions(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Permissions", "no session has started yet");
            return;
        };
        self.push_outbound(OutboundAgentCommand::ShowPermissions { session_id });
    }

    pub(super) fn execute_show_permissions_command(&mut self, session_id: String) {
        match api_request_json(&self.server, "GET", "/permission", None) {
            Ok(value) => self.system_message(
                "Permissions",
                format_permissions(value.as_ref(), Some(&session_id)),
            ),
            Err(error) => self.system_message("Permissions", error),
        }
    }

    fn show_questions(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Questions", "no session has started yet");
            return;
        };
        self.push_outbound(OutboundAgentCommand::ShowQuestions { session_id });
    }

    pub(super) fn execute_show_questions_command(&mut self, session_id: String) {
        match api_request_json(&self.server, "GET", "/question", None) {
            Ok(value) => self.system_message(
                "Questions",
                format_questions(value.as_ref(), Some(&session_id)),
            ),
            Err(error) => self.system_message("Questions", error),
        }
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

    pub(super) fn execute_handle_permit_command(
        &mut self,
        session_id: String,
        reply: String,
        id: Option<String>,
    ) {
        let id = id.or_else(|| {
            first_interaction_id(&self.server, "/permission", Some(&session_id))
                .ok()
                .flatten()
        });
        let Some(id) = id else {
            self.system_message("Permissions", "no pending permissions");
            return;
        };
        let body = json!({ "reply": reply });
        match api_request_json(
            &self.server,
            "POST",
            &format!("/permission/{id}/reply"),
            Some(&body),
        ) {
            Ok(_) => self.system_message("Permission", format!("{id}: {reply}")),
            Err(error) => self.system_message("Permission", error),
        }
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

    pub(super) fn execute_handle_answer_command(
        &mut self,
        session_id: String,
        answer: String,
    ) {
        let item = first_interaction_value(&self.server, "/question", Some(&session_id))
            .ok()
            .flatten();
        let Some(item) = item else {
            self.system_message("Questions", "no pending questions");
            return;
        };
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            self.system_message("Questions", "pending question has no id");
            return;
        };
        let body = json!({ "answers": question_answers(&answer, question_count(&item)) });
        match api_request_json(
            &self.server,
            "POST",
            &format!("/question/{id}/reply"),
            Some(&body),
        ) {
            Ok(_) => self.system_message("Question", format!("answered {id}")),
            Err(error) => self.system_message("Question", error),
        }
    }

    fn handle_reject(&mut self, id_arg: Option<&str>) {
        if self.session_id.is_none() {
            self.system_message("Interaction", "no session has started yet");
            return;
        }
        let session_id = self.session_id.clone().unwrap();
        self.push_outbound(OutboundAgentCommand::HandleReject {
            session_id,
            id: id_arg.map(str::to_string),
        });
    }

    pub(super) fn execute_handle_reject_command(
        &mut self,
        session_id: String,
        id_arg: Option<String>,
    ) {
        if let Some(id) = id_arg.or_else(|| {
            first_interaction_id(&self.server, "/question", Some(&session_id))
                .ok()
                .flatten()
        }) {
            match api_request_json(
                &self.server,
                "POST",
                &format!("/question/{id}/reject"),
                None,
            ) {
                Ok(_) => self.system_message("Question", format!("rejected {id}")),
                Err(error) => self.system_message("Question", error),
            }
            return;
        }
        if let Some(id) =
            first_interaction_id(&self.server, "/permission", Some(&session_id))
                .ok()
                .flatten()
        {
            let body = json!({ "reply": "reject" });
            match api_request_json(
                &self.server,
                "POST",
                &format!("/permission/{id}/reply"),
                Some(&body),
            ) {
                Ok(_) => self.system_message("Permission", format!("rejected {id}")),
                Err(error) => self.system_message("Permission", error),
            }
            return;
        }
        self.system_message("Interaction", "no pending permissions or questions");
    }

    fn compact_session(&mut self) {
        if self.session_id.is_none() {
            self.system_message("Context", "no session has started yet");
            return;
        }
        self.push_outbound(OutboundAgentCommand::CompactSession);
    }

    pub(super) fn execute_compact_session_command(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Context", "no session has started yet");
            return;
        };
        self.start_session_updates(&session_id);
        let server = self.server.clone();
        let background_tx = self.background_sender();
        if let Err(error) = thread::Builder::new()
            .name(format!("neoism-agent-compact-{session_id}"))
            .spawn(move || {
                let mut last_error = None;
                for attempt in 0..300 {
                    match api_request_json_with_read_timeout(
                        &server,
                        "POST",
                        &format!("/api/session/{session_id}/compact"),
                        None,
                        Duration::from_secs(600),
                    ) {
                        Ok(_) => {
                            let _ = background_tx
                                .send(NeoismAgentBackgroundUpdate::CompactFinished);
                            return;
                        }
                        Err(error)
                            if error.contains("already running") && attempt < 299 =>
                        {
                            last_error = Some(error);
                            thread::sleep(Duration::from_secs(1));
                        }
                        Err(error) => {
                            last_error = Some(error);
                            break;
                        }
                    }
                }
                let _ = background_tx.send(NeoismAgentBackgroundUpdate::CompactFailed(
                    last_error.unwrap_or_else(|| {
                        "compact request did not complete".to_string()
                    }),
                ));
            })
        {
            self.fail_compaction_message(format!(
                "failed to start compact thread: {error}"
            ));
        }
    }

    pub(super) fn execute_undo_session_command(&mut self) {
        self.execute_session_history_command("Undo", "undo");
    }

    pub(super) fn execute_redo_session_command(&mut self) {
        self.execute_session_history_command("Redo", "redo");
    }

    fn execute_session_history_command(&mut self, title: &str, action: &str) {
        let Some(session_id) = self.session_id.clone() else {
            self.system_message(title, "no session has started yet");
            return;
        };
        // Run the revert POST and the (potentially large) message re-fetch off
        // the UI thread. Doing them inline blocked the drain loop, freezing the
        // whole pane — so ESC and any other keystroke were ignored until the
        // revert finished. The result is applied back via a background update.
        let server = self.server.clone();
        let title = title.to_string();
        let action = action.to_string();
        let background_tx = self.background_sender();
        let thread_session = session_id.clone();
        let thread_title = title.clone();
        let thread_action = action.clone();
        if let Err(error) = thread::Builder::new()
            .name(format!("neoism-agent-{action}-{session_id}"))
            .spawn(move || {
                let update = match api_request_json(
                    &server,
                    "POST",
                    &format!("/api/session/{thread_session}/{thread_action}"),
                    None,
                )
                .and_then(|_| fetch_session_messages(&server, &thread_session))
                {
                    Ok(messages) => NeoismAgentBackgroundUpdate::SessionHistoryApplied {
                        session_id: thread_session,
                        title: thread_title,
                        messages,
                    },
                    Err(error) => NeoismAgentBackgroundUpdate::SessionHistoryFailed {
                        session_id: thread_session,
                        title: thread_title,
                        error,
                    },
                };
                let _ = background_tx.send(update);
            })
        {
            self.system_message(
                &title,
                format!("failed to start {action} thread: {error}"),
            );
        }
    }

    pub(super) fn abort_session(&mut self) {
        if self.session_id.is_none() {
            self.note_streaming(NeoismAgentStreamingState::Idle, None);
            self.system_message("Abort", "no session has started yet");
            return;
        }
        self.abort_requested_at = Some(Instant::now());
        self.note_streaming(NeoismAgentStreamingState::Idle, None);
        self.push_outbound(OutboundAgentCommand::AbortSession);
    }

    pub(super) fn execute_abort_session_command(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.note_streaming(NeoismAgentStreamingState::Idle, None);
            self.system_message("Abort", "no session has started yet");
            return;
        };
        match api_request_json(
            &self.server,
            "POST",
            &format!("/session/{session_id}/abort"),
            None,
        ) {
            // Escape already updates the local running state immediately.
            // A successful abort is intentionally silent: surfacing it as a
            // system notice leaves an unnecessary "interrupted run" pill in
            // the chat every time the user presses Escape.
            Ok(_) => {}
            Err(error) => self.system_message("Abort", error),
        }
    }

    /// Kick off an older-history fetch on a background thread. The blocking
    /// HTTP GET must never run on the UI thread — doing so froze scrolling
    /// for the duration of the request every time the reader neared the top.
    /// The fetched page is delivered through the background channel and
    /// applied by [`apply_older_timeline_page`].
    pub(super) fn execute_load_older_timeline_command(
        &mut self,
        session_id: String,
        before: Option<String>,
        limit: usize,
    ) {
        if self.session_id.as_deref() != Some(session_id.as_str()) {
            return;
        }
        let cursor =
            before.or_else(|| self.messages.first().map(|message| message.id.clone()));
        let server = self.server.clone();
        let background_tx = self.background_sender();
        if let Err(error) = thread::Builder::new()
            .name(format!("neoism-agent-history-{session_id}"))
            .spawn(move || {
                let update = match (|| {
                    // Exactly one bounded server page per UI request. Do not
                    // chase a user-role boundary: a tool-heavy agent turn may
                    // be hundreds of messages long, and accumulating all of
                    // it off-thread still creates one enormous synchronous
                    // prepend/layout operation on the render thread.
                    let page = fetch_session_messages_page(
                        &server,
                        &session_id,
                        cursor.as_deref(),
                        limit,
                    )?;
                    let raw_count = page.raw_count;
                    let reached_start = raw_count < limit;
                    let oldest_cursor = page.oldest_cursor;
                    let older = page.blocks;
                    Ok::<_, String>((older, raw_count, oldest_cursor, reached_start))
                })() {
                    Ok((older, raw_count, oldest_cursor, reached_start)) => {
                        NeoismAgentBackgroundUpdate::OlderTimelineLoaded {
                            session_id,
                            messages: older,
                            raw_count,
                            requested_limit: limit,
                            oldest_cursor,
                            reached_start,
                        }
                    }
                    Err(error) => NeoismAgentBackgroundUpdate::OlderTimelineFailed {
                        session_id,
                        error,
                    },
                };
                let _ = background_tx.send(update);
            })
        {
            self.timeline_history.loading_older = false;
            self.timeline_history.last_requested_session_id = None;
            self.system_message(
                "History",
                format!("failed to start history thread: {error}"),
            );
        }
    }

    /// Apply an older-history page fetched off-thread: dedupe against what is
    /// already loaded, prepend in reading order, and pin the reader's scroll
    /// position so the viewport doesn't jump. A page shorter than what we
    /// asked for means we've reached the start of the transcript.
    pub(super) fn apply_older_timeline_page(
        &mut self,
        session_id: String,
        mut older: Vec<NeoismAgentMessage>,
        raw_count: usize,
        requested_limit: usize,
        oldest_cursor: Option<String>,
        reached_start: bool,
    ) {
        if self.session_id.as_deref() != Some(session_id.as_str()) {
            return;
        }
        self.timeline_history.loading_older = false;
        // "Is there more history?" is a property of *stored messages*, not the
        // expanded render blocks. A full page (raw_count == limit) means more
        // may remain; a short page means we hit the start. Comparing block
        // count here would falsely cap pagination, because one message yields
        // several blocks (and some yield none).
        let reached_start = reached_start || raw_count < requested_limit;
        let existing = self
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<HashSet<_>>();
        older.retain(|message| {
            message.id.is_empty() || !existing.contains(message.id.as_str())
        });
        if older.is_empty() {
            self.timeline_history.has_older = !reached_start;
            self.timeline_history.oldest_loaded_cursor = oldest_cursor;
            return;
        }
        self.mark_timeline_prepend_pending_at_current_height();
        self.timeline_history.oldest_loaded_cursor = oldest_cursor;
        let prepended = older.len();
        self.messages.splice(0..0, older);
        self.timeline_history.has_older = !reached_start;
        // Incremental fold instead of a full relayout: keep the existing cache
        // and tell the renderer how many messages landed at the front. Without
        // this every page rerendered all prior rows, so pagination slowed down
        // with each page loaded.
        self.note_timeline_prepend(prepended);
    }

    pub(super) fn create_new_session(&mut self) {
        self.session_id = None;
        self.parent_session_id = None;
        self.side_panel.set_viewed_session_id(None);
        self.clear_pending_user_prompts();
        self.messages.clear();
        self.invalidate_timeline_layout();
        self.reset_session_runtime_ui();
        self.clear_composer();
        self.system_message("Session", "new draft");
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

    pub(super) fn execute_server_command(
        &mut self,
        command: String,
        command_args: String,
    ) {
        let session_id = match self.ensure_session() {
            Ok(session_id) => session_id,
            Err(error) => {
                self.system_message("Command failed", error);
                return;
            }
        };
        let body = json!({
            "messageId": null,
            "model": prompt_model_json(self.model.as_str(), self.thinking.as_deref()),
            "agent": self.agent.clone(),
            "command": command,
            "arguments": command_args,
        });
        self.start_session_updates(&session_id);
        match api_request_json(
            &self.server,
            "POST",
            &format!("/session/{session_id}/command"),
            Some(&body),
        ) {
            Ok(_) => {
                self.system_message("Command", format!("/{command}"));
            }
            Err(error) => self.system_message("Command failed", error),
        }
    }

    pub(super) fn execute_ensure_session_command(&mut self) -> Result<String, String> {
        self.ensure_session()
    }

    fn ensure_session(&mut self) -> Result<String, String> {
        if let Some(session_id) = self.session_id.clone() {
            return Ok(session_id);
        }
        let path = self
            .directory
            .as_deref()
            .map(|dir| format!("/session?directory={}", percent_encode(dir)))
            .unwrap_or_else(|| "/session".to_string());
        let body = json!({
            "parentId": null,
            "title": null,
            "agent": self.agent.clone(),
            "model": session_model_json(self.model.as_str(), self.thinking.as_deref()),
            "permission": null,
            "workspaceId": null,
        });
        let response = api_request_json(&self.server, "POST", &path, Some(&body))?
            .ok_or_else(|| "server did not return session".to_string())?;
        let id = response
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "server did not return session id".to_string())?
            .to_string();
        self.session_id = Some(id.clone());
        self.parent_session_id = None;
        self.session_tree_root_id = Some(id.clone());
        self.side_panel.set_viewed_session_id(Some(id.clone()));
        Ok(id)
    }
}

pub(super) fn slash_options() -> Vec<NeoismAgentPickerOption> {
    command_controller::slash_options()
}
