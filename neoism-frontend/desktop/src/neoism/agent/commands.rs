use std::collections::HashSet;
use std::thread;
use std::time::{Duration, Instant};

use neoism_ui::panels::agent_pane::command_controller::{self, SlashCommandAction};
use neoism_ui::panels::agent_pane::outbound::OutboundAgentCommand;
use serde_json::{json, Value};

use super::api::{
    api_request_json, api_request_json_with_read_timeout, fetch_session_messages,
    fetch_session_messages_page, fetch_session_state, fetch_skill_options,
    first_interaction_id, first_interaction_value, format_mcp_status, format_permissions,
    format_questions, format_queue, is_permission_reply, normalize_model_ref,
    normalize_thinking, percent_encode, permission_reply_alias, prompt_model_json,
    question_answers, question_count, session_model_json,
};
use super::pane::{
    NeoismAgentBackgroundUpdate, NeoismAgentMessage, NeoismAgentMessageKind,
    NeoismAgentMode, NeoismAgentNoticeLevel, NeoismAgentPane, NeoismAgentStreamingState,
};
use super::picker::NeoismAgentPickerOption;
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
        let Some(session_id) = self.session_id.clone() else {
            self.system_message("Goal", "no session has started yet");
            return;
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
        self.input.clear();
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
        self.refresh_model_context_limit();
        self.input.clear();
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
        self.input.clear();
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
        let started = super::perf::now();
        let previous_session_id = self.session_id.clone();
        let previous_message_count = self.messages.len();
        if session_id.is_empty() {
            return;
        }
        let state_started = super::perf::now();
        let state = fetch_session_state(&self.server, &session_id).ok();
        let state_ok = state.is_some();
        let state_us = super::perf::elapsed_us(state_started);
        let messages_started = super::perf::now();
        match fetch_session_messages(&self.server, &session_id) {
            Ok(messages) => {
                self.clear_pending_user_prompts();
                self.session_id = Some(session_id.clone());
                self.input.clear();
                self.close_picker();
                self.reset_session_runtime_ui();
                // Pagination state is per-session. Leaking the previous
                // session's cursor/has_older here either disabled "load
                // older" entirely or fed the server a foreign message id,
                // which resolves to the newest page and dedupes to nothing.
                self.timeline_history = Default::default();
                self.side_panel.invalidate_subagent_refresh();
                // Reset the previous session's goal AND its version, then
                // force a refetch so the Goal section reflects the session we
                // just switched to (a fresh version lets the new goal apply).
                self.side_panel.reset_session_goal();
                self.side_panel.invalidate_goal_refresh();
                // Pull the session's stored agent / model / thinking so the
                // bottom-input chips reflect the resumed turn instead of the
                // pane's default config.
                self.parent_session_id =
                    state.as_ref().and_then(|state| state.parent_id.clone());
                if let Some(state) = state {
                    if let Some(agent) = state.agent {
                        self.agent = Some(agent);
                    }
                    if let Some(model) = state.model {
                        self.model = model;
                    }
                    self.thinking = state.thinking;
                    self.execute_refresh_model_context_limit_command();
                }
                if self.is_subagent_session() {
                    self.clear_composer();
                    self.close_picker();
                }
                self.messages = messages;
                self.invalidate_timeline_layout();
                let hydrate_started = super::perf::now();
                self.hydrate_runtime_status_for_session(&session_id);
                let hydrate_us = super::perf::elapsed_us(hydrate_started);
                self.start_session_updates(&session_id);
                if self.messages.is_empty() {
                    self.system_message("Session", format!("session {session_id}"));
                }
                if super::perf::enabled() {
                    tracing::info!(
                        target: "neoism::agent_ui_perf",
                        previous_session_id = previous_session_id.as_deref(),
                        session_id,
                        previous_message_count,
                        message_count = self.messages.len(),
                        tool_messages = self.messages.iter().filter(|message| matches!(message.kind, NeoismAgentMessageKind::Tool | NeoismAgentMessageKind::Subtask)).count(),
                        text_bytes = self.messages.iter().map(|message| message.text.len()).sum::<usize>(),
                        state_ok,
                        state_us,
                        messages_ok = true,
                        messages_us = super::perf::elapsed_us(messages_started),
                        hydrate_us,
                        total_us = super::perf::elapsed_us(started),
                        "agent switch session"
                    );
                }
            }
            Err(error) => {
                if super::perf::enabled() {
                    tracing::warn!(
                        target: "neoism::agent_ui_perf",
                        previous_session_id = previous_session_id.as_deref(),
                        session_id,
                        previous_message_count,
                        state_ok,
                        state_us,
                        messages_ok = false,
                        messages_us = super::perf::elapsed_us(messages_started),
                        total_us = super::perf::elapsed_us(started),
                        error = %error,
                        "agent switch session failed"
                    );
                }
                self.side_panel.invalidate_subagent_refresh();
                self.system_message("Session", error)
            }
        }
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
            text: prompt,
            parts,
            system,
            agent: self.agent.clone(),
            model: self.model.clone(),
            thinking: self.thinking.clone(),
            transcript_echo,
        });
        Ok(())
    }

    pub(super) fn execute_send_prompt_command(
        &mut self,
        text: String,
        parts: Vec<Value>,
        system: Option<String>,
        agent: Option<String>,
        model: String,
        thinking: Option<String>,
        transcript_echo: bool,
    ) -> Result<(), String> {
        if self.is_subagent_session() {
            return Err("subagent sessions are view-only".to_string());
        }
        let session_id = self.ensure_session()?;
        let body = json!({
            "messageId": null,
            "model": prompt_model_json(model.as_str(), thinking.as_deref()),
            "agent": agent,
            "noReply": false,
            "system": system,
            "tools": null,
            "parts": parts,
        });
        self.start_session_updates(&session_id);
        api_request_json(
            &self.server,
            "POST",
            &format!("/session/{session_id}/prompt_async"),
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

    fn show_mcp(&mut self) {
        self.push_outbound(OutboundAgentCommand::ShowMcp {
            directory: self.directory.clone(),
        });
    }

    pub(super) fn execute_show_mcp_command(&mut self, directory: Option<String>) {
        let path = directory
            .as_deref()
            .map(|dir| format!("/mcp?directory={}", percent_encode(dir)))
            .unwrap_or_else(|| "/mcp".to_string());
        match api_request_json(&self.server, "GET", &path, None) {
            Ok(value) => self.system_message("MCP", format_mcp_status(value.as_ref())),
            Err(error) => self.system_message("MCP", error),
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
            Ok(_) => {
                self.system_message("Abort", "session abort requested");
            }
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
            self.timeline_history.loading_older = false;
            self.timeline_history.last_requested_session_id = None;
            return;
        }
        let cursor =
            before.or_else(|| self.messages.first().map(|message| message.id.clone()));
        let server = self.server.clone();
        let background_tx = self.background_sender();
        if let Err(error) = thread::Builder::new()
            .name(format!("neoism-agent-history-{session_id}"))
            .spawn(move || {
                let update = match fetch_session_messages_page(
                    &server,
                    &session_id,
                    cursor.as_deref(),
                    limit,
                ) {
                    Ok(page) => {
                        let raw_count = page.raw_count;
                        let mut older = page.blocks;
                        // Server returns newest-first; flip to oldest-first
                        // so it prepends in reading order.
                        older.reverse();
                        NeoismAgentBackgroundUpdate::OlderTimelineLoaded {
                            session_id,
                            messages: older,
                            raw_count,
                            requested_limit: limit,
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
    ) {
        self.timeline_history.loading_older = false;
        self.timeline_history.last_requested_session_id = None;
        if self.session_id.as_deref() != Some(session_id.as_str()) {
            return;
        }
        // "Is there more history?" is a property of *stored messages*, not the
        // expanded render blocks. A full page (raw_count == limit) means more
        // may remain; a short page means we hit the start. Comparing block
        // count here would falsely cap pagination, because one message yields
        // several blocks (and some yield none).
        let reached_start = raw_count < requested_limit;
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
            return;
        }
        self.mark_timeline_prepend_pending_at_current_height();
        self.timeline_history.oldest_loaded_cursor =
            older.first().map(|message| message.id.clone());
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
        Ok(id)
    }
}

pub(super) fn slash_options() -> Vec<NeoismAgentPickerOption> {
    command_controller::slash_options()
}
