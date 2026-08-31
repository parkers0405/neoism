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
    NeoismAgentStreamingState, PendingPromptDispatch,
};
use super::picker::{NeoismAgentPicker, NeoismAgentPickerKind, NeoismAgentPickerOption};
use super::side_panel::{BranchStatus, SessionGoal};
use super::updates::{start_session_event_stream, AgentSessionEventStream};

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
            &format!("/v2/sessions/{session_id}"),
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
            &format!("/v2/plugins/dev.neoism.goals/{session_id}"),
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
            &format!("/v2/plugins/dev.neoism.goals/{session_id}"),
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
            &format!("/v2/plugins/dev.neoism.goals/{session_id}"),
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
        if let Some(session_id) = self.session_id.clone() {
            let version = goal.as_ref().map(|goal| goal.updated).unwrap_or(0);
            self.session_goal_cache
                .insert(session_id, (goal.clone(), version));
        }
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
            &format!("/v2/plugins/dev.neoism.goals/{session_id}"),
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
            &format!("/v2/plugins/dev.neoism.goals/{session_id}"),
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
            &format!("/v2/sessions/{session_id}"),
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
            &format!("/v2/sessions/{session_id}"),
            Some(&body),
        ) {
            self.system_message("Rename", error);
        }
    }

    pub(super) fn apply_model(&mut self, value: String) {
        if self.reconcile_model_account(value) {
            return;
        }
    }

    pub(in crate::neoism::agent) fn apply_model_with_connection(&mut self, value: String, connection_id: Option<String>) {
        self.remember_model_value(&value);
        self.model = value;
        self.connection_id = connection_id;
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
                session_model_json(self.model.as_str(), self.thinking.as_deref(), self.connection_id.as_deref())
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
            &format!("/v2/sessions/{session_id}"),
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
        let Some(model_json) = session_model_json(model.as_str(), thinking.as_deref(), self.connection_id.as_deref())
        else {
            return;
        };
        let body = json!({ "model": model_json });
        if let Err(error) = api_request_json(
            &self.server,
            "PATCH",
            &format!("/v2/sessions/{session_id}"),
            Some(&body),
        ) {
            self.system_message("Think", error);
        }
    }

    pub(super) fn switch_session(&mut self, session_id: String) {
        if session_id.is_empty() {
            return;
        }
        // Session selection is local state. Execute it in the input event so
        // the very next paint sees the target instead of waiting for the
        // outbound-command drain on that paint.
        self.execute_switch_session_command(session_id);
    }

    pub(super) fn execute_switch_session_command(&mut self, session_id: String) {
        if session_id.is_empty() {
            return;
        }
        if self.session_id.as_deref() == Some(session_id.as_str()) {
            self.pending_session_switch = None;
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
        const MAX_CONCURRENT_PRELOADS: usize = 2;
        const MAX_QUEUED_PRELOADS: usize = 10;
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
        if self.session_preloads_in_flight.contains(&session_id) {
            if force {
                self.session_preloads_force_pending.insert(session_id);
            }
            return;
        }
        if let Some(index) = self
            .session_preload_queue
            .iter()
            .position(|(queued, _)| queued == &session_id)
        {
            let (_, was_force) = self
                .session_preload_queue
                .remove(index)
                .expect("queued preload index");
            let request = (session_id.clone(), force || was_force);
            if self.pending_session_switch.as_deref() == Some(session_id.as_str())
                || force
            {
                self.session_preload_queue.push_front(request);
            } else {
                self.session_preload_queue.push_back(request);
            }
            return;
        }
        if self.session_preloads_in_flight.len() >= MAX_CONCURRENT_PRELOADS {
            let request = (session_id.clone(), force);
            if self.pending_session_switch.as_deref() == Some(session_id.as_str())
                || force
            {
                if self.session_preload_queue.len() >= MAX_QUEUED_PRELOADS {
                    self.session_preload_queue.pop_back();
                }
                self.session_preload_queue.push_front(request);
            } else if self.session_preload_queue.len() < MAX_QUEUED_PRELOADS {
                self.session_preload_queue.push_back(request);
            }
            return;
        }
        self.start_session_preload(session_id);
    }

    fn start_session_preload(&mut self, session_id: String) {
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
            self.start_queued_session_preloads();
        }
    }

    pub(crate) fn start_queued_session_preloads(&mut self) {
        const MAX_CONCURRENT_PRELOADS: usize = 2;
        while self.session_preloads_in_flight.len() < MAX_CONCURRENT_PRELOADS {
            let Some((session_id, force)) = self.session_preload_queue.pop_front() else {
                break;
            };
            if !force
                && self
                    .session_cache
                    .get(&session_id)
                    .is_some_and(|cached| cached.hydrated)
            {
                continue;
            }
            self.start_session_preload(session_id);
        }
    }

    pub(crate) fn cache_current_session(&mut self, preserve_live_trace: bool) {
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        let state = neoism_ui::panels::agent_pane::api_mapping::SessionState {
            agent: self.agent.clone(),
            model: (!self.model.is_empty()).then(|| self.model.clone()),
            connection_id: self.connection_id.clone(),
            thinking: self.thinking.clone(),
            parent_id: self.parent_session_id.clone(),
            directory: self.directory.clone(),
        };
        let cached_live = self
            .session_cache
            .remove(&session_id)
            .map(|cached| cached.messages)
            .unwrap_or_default();
        let messages = std::mem::take(&mut self.messages);
        let messages = if cached_live.is_empty() {
            messages
        } else {
            merge_session_snapshot(messages, cached_live)
        };
        let timeline_history = std::mem::take(&mut self.timeline_history);
        let timeline_layout_cache = self.timeline_layout_cache.replace(None);
        let runtime = self.take_session_runtime_ui();
        let (timeline_live_trace_start, timeline_live_trace_anchor) =
            self.live_trace_for_cache(preserve_live_trace);
        self.session_cache.insert(
            session_id,
            CachedAgentSession {
                state,
                messages,
                pending_user_prompts: std::mem::take(&mut self.pending_user_prompts),
                prompt_echo_aliases: std::mem::take(&mut self.prompt_echo_aliases),
                timeline_history,
                timeline_scroll_px: self.timeline_scroll_px,
                timeline_follow_bottom: self.timeline_follow_bottom,
                timeline_content_height_px: self.timeline_content_height_px,
                timeline_live_trace_start,
                timeline_live_trace_anchor,
                timeline_layout_epoch: self.timeline_layout_epoch,
                timeline_layout_cache,
                timeline_dirty_message_ids: std::mem::take(
                    &mut self.timeline_dirty_message_ids,
                ),
                timeline_dirty_message_indices: std::mem::take(
                    &mut self.timeline_dirty_message_indices,
                ),
                runtime,
                model_context_limit: self.model_context_limit,
                hydrated: true,
                last_access: Instant::now(),
            },
        );
        self.trim_session_cache();
    }

    pub(crate) fn activate_cached_session(&mut self, session_id: &str) {
        if self.session_id.as_deref() == Some(session_id) {
            self.pending_session_switch = None;
            return;
        }
        let Some(cached) = self.session_cache.remove(session_id) else {
            return;
        };
        // Decide BEFORE the ids change hands: a switch within the same
        // conversation family (parent ↔ child ↔ sibling) keeps the
        // parent-keyed subagent roster alive — the sidebar continues to
        // show the family's sub-agent names/statuses while a child
        // transcript is open, and sibling lifecycle updates keep
        // landing in it. Only leaving the family rebuilds the roster.
        let stays_in_family = self.session_family_contains(session_id)
            || cached
                .state
                .parent_id
                .as_deref()
                .is_some_and(|parent| self.session_family_contains(parent));
        // A live-only cache slot carries no session metadata. When the
        // roster tracks the target as a child row, carry the family root
        // as its parent so the restored view opens as a view-only
        // subagent transcript instead of a detached main session.
        let roster_parent = self
            .side_panel
            .subagents()
            .first()
            .map(|entry| entry.id.clone())
            .filter(|root| {
                root != session_id
                    && self
                        .side_panel
                        .subagents()
                        .iter()
                        .skip(1)
                        .any(|entry| entry.id == session_id)
            });
        self.cache_current_session(stays_in_family);
        let state = cached.state;
        let parent_id = state.parent_id.clone().or(roster_parent.clone());
        // Never carry a root from a previously viewed family. A nested child
        // may initially use its direct parent; the tree refresh promotes this
        // to the true root without leaving the family stream snapshot-only.
        self.session_tree_root_id = Some(
            roster_parent
                .or_else(|| parent_id.clone())
                .unwrap_or_else(|| session_id.to_string()),
        );
        self.session_id = Some(session_id.to_string());
        self.parent_session_id = parent_id;
        self.side_panel
            .set_viewed_session_id(Some(session_id.to_string()));
        self.input.clear();
        self.close_picker();
        self.reset_timeline_navigation_for_session_switch();
        if stays_in_family {
            self.restore_cached_live_trace(
                cached.timeline_live_trace_start,
                cached.timeline_live_trace_anchor,
            );
        }
        // The restored timeline must render fresh — no raised/expanded
        // card artifacts from the click that navigated away. Leave-and-return
        // also collapses the live-trace window, so a parked layout that still
        // contains tool rows would paint leftover titles until the next click.
        self.reset_transient_timeline_interactions();
        self.timeline_history = cached.timeline_history;
        self.timeline_scroll_px = cached.timeline_scroll_px;
        self.timeline_follow_bottom = cached.timeline_follow_bottom;
        self.timeline_content_height_px = cached.timeline_content_height_px;
        self.side_panel.set_show_home_override(false);
        if !stays_in_family {
            self.side_panel.invalidate_subagent_refresh();
            self.clear_pending_interactions();
            self.execution_activity = None;
            self.execution_timer_anchor = None;
            self.runtime_snapshot_root = None;
            self.runtime_snapshot_revision = 0;
            self.terminal_subagent_revisions.clear();
        }
        self.side_panel.reset_session_goal();
        if let Some((goal, version)) = self.session_goal_cache.get(session_id).cloned() {
            if goal.is_some() {
                self.side_panel.set_session_goal(goal, version);
            }
        } else {
            self.side_panel.invalidate_goal_refresh();
        }
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
        self.connection_id = state.connection_id;
        self.thinking = state.thinking;
        self.model_context_limit = cached.model_context_limit;
        if self.model_context_limit.is_none() && !self.model.is_empty() {
            self.execute_refresh_model_context_limit_command();
        }
        if self.is_subagent_session() {
            self.clear_composer();
            self.set_cursor_rect(None);
            self.close_picker();
        }
        self.messages = cached.messages;
        self.pending_user_prompts = cached.pending_user_prompts;
        self.prompt_echo_aliases = cached.prompt_echo_aliases;
        self.restore_session_runtime_ui(cached.runtime);
        if !stays_in_family {
            self.clear_family_activity();
        }
        // A cold or previously settled ongoing child must immediately reveal
        // the activity it already emitted
        // (reasoning, tools, edits, and subtasks), not wait for one more SSE
        // part before its existing history becomes visible.
        if self.is_streaming() {
            self.reveal_ongoing_session_trace();
        }
        // Warm switches adopt the cached layout epoch wholesale: the cached
        // session's live updates already bump its own epoch, so an unchanged
        // transcript renders from the parked layout with no relayout flash.
        self.timeline_layout_epoch = cached.timeline_layout_epoch;
        if !stays_in_family {
            // Leaving the family collapses the live-trace window; drop the
            // parked layout so settled tool/reasoning rows are re-masked
            // instead of flashing as leftover titles from the previous visit.
            self.invalidate_timeline_layout();
        }
        if !stays_in_family || !self.runtime_hydrated_sessions.contains(session_id) {
            self.hydrate_runtime_status_for_session(session_id);
        }
        let stream_session_id = self
            .session_tree_root_id
            .clone()
            .unwrap_or_else(|| session_id.to_string());
        self.start_session_updates(&stream_session_id);
        self.pending_session_switch = None;
    }

    pub(super) fn send_prepared_prompt(
        &mut self,
        prompt: String,
        transcript_echo: bool,
        message_id: String,
    ) -> Result<(), String> {
        if self.is_subagent_session() {
            return Err("subagent sessions are view-only".to_string());
        }
        let parts = self.prompt_parts_for(&prompt);
        let system = self.prompt_system_for(&prompt);
        self.push_outbound(OutboundAgentCommand::SendPrompt {
            message_id,
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

    pub(super) fn queue_send_prompt_command(
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
    ) {
        if self.is_subagent_session() {
            self.system_message("Prompt failed", "subagent sessions are view-only");
            return;
        }
        let origin_session_id = self.session_id.clone();
        let origin_draft_id = self.prompt_draft_id;
        if let Some(session_id) = origin_session_id.as_deref() {
            self.start_session_updates(session_id);
        }
        // Stamp who is sending this prompt so a shared/joined session
        // attributes the turn to the true sender: the host's agent-server
        // persists this on the user message and re-broadcasts it to every
        // attached client, and a remote peer renders THIS name + its
        // deterministic presence orb (instead of a generic "You"). The local
        // sender's own bubble still reads "You" because its author matches the
        // pane's local presence name. The server echo is reconciled by durable
        // message identity rather than prompt text.
        // Never emit `author: null` from a native desktop. Pane identity is
        // normally installed when the tab opens; the system-derived fallback
        // keeps attribution intact if submission races pane initialization.
        let author = Some(native_prompt_author(self.local_presence_name()));
        let transcript_echo = if transcript_echo {
            // Pending prompts must match the transcript echo, which uses
            // the compact composer form for pasted attachments.
            Some(
                self.compact_user_prompt_text(&text)
                    .unwrap_or_else(|| text.clone()),
            )
        } else {
            None
        };
        self.pending_prompt_dispatches
            .push_back(PendingPromptDispatch {
                origin_session_id,
                origin_draft_id,
                server: self.server.clone(),
                directory: self.directory.clone(),
                message_id,
                parts,
                system,
                agent,
                model,
                connection_id: self.connection_id.clone(),
                thinking,
                delivery,
                author,
                transcript_echo,
                event_wake: self.event_wake(),
            });
        self.start_next_prompt_dispatch();
    }

    pub(crate) fn start_next_prompt_dispatch(&mut self) {
        if self.prompt_dispatch_in_flight {
            return;
        }
        let Some(request) = self.pending_prompt_dispatches.pop_front() else {
            return;
        };
        self.prompt_dispatch_in_flight = true;
        let tx = self.background_sender();
        if let Err(error) = thread::Builder::new()
            .name("neoism-agent-prompt".into())
            .spawn(move || {
                let origin_session_id = request.origin_session_id.clone();
                let origin_draft_id = request.origin_draft_id;
                let transcript_echo = request.transcript_echo.clone();
                let update = match dispatch_prompt_request(request) {
                    Ok((session_id, event_stream)) => {
                        NeoismAgentBackgroundUpdate::PromptDispatched {
                            origin_session_id,
                            origin_draft_id,
                            session_id,
                            transcript_echo,
                            event_stream,
                        }
                    }
                    Err(error) => NeoismAgentBackgroundUpdate::PromptDispatchFailed {
                        origin_session_id,
                        origin_draft_id,
                        error,
                    },
                };
                let _ = tx.send(update);
            })
        {
            self.prompt_dispatch_in_flight = false;
            self.system_message(
                "Prompt failed",
                format!("failed to start prompt request: {error}"),
            );
            self.start_next_prompt_dispatch();
            if !self.prompt_dispatch_in_flight {
                self.note_streaming(NeoismAgentStreamingState::Idle, None);
            }
        }
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
            &format!("/v2/sessions/{session_id}/jobs/{job_id}"),
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
                &format!("/v2/sessions/{session_id}/queue"),
                None,
            ),
            Some("pop") => api_request_json(
                &self.server,
                "POST",
                &format!("/v2/sessions/{session_id}/queue/pop"),
                None,
            ),
            _ => api_request_json(
                &self.server,
                "GET",
                &format!("/v2/sessions/{session_id}/queue"),
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
            .map(|dir| format!("/v2/plugins/dev.neoism.mcp/catalog?directory={}", percent_encode(dir)))
            .unwrap_or_else(|| "/v2/plugins/dev.neoism.mcp/catalog".to_string());
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
        let path = format!("/v2/plugins/dev.neoism.mcp/{}/auth{directory}", percent_encode(&name));
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
                format!("/v2/plugins/dev.neoism.mcp/{}/config{directory}", percent_encode(name)),
                Some(json!({ "enabled": true })),
            ),
            "disable" => (
                "PATCH",
                format!("/v2/plugins/dev.neoism.mcp/{}/config{directory}", percent_encode(name)),
                Some(json!({ "enabled": false })),
            ),
            "connect" => (
                "POST",
                format!("/v2/plugins/dev.neoism.mcp/{}/connect{directory}", percent_encode(name)),
                Some(json!({})),
            ),
            "disconnect" => (
                "POST",
                format!("/v2/plugins/dev.neoism.mcp/{}/disconnect{directory}", percent_encode(name)),
                Some(json!({})),
            ),
            "logout" => (
                "DELETE",
                format!("/v2/plugins/dev.neoism.mcp/{}/auth{directory}", percent_encode(name)),
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
        match api_request_json(&self.server, "GET", "/v2/interactions/permissions", None) {
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
        match api_request_json(&self.server, "GET", "/v2/interactions/questions", None) {
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
            first_interaction_id(&self.server, "/v2/interactions/permissions", Some(&session_id))
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
            &format!("/v2/interactions/permissions/{id}/reply"),
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
        let item = first_interaction_value(&self.server, "/v2/interactions/questions", Some(&session_id))
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
            &format!("/v2/interactions/questions/{id}/reply"),
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
            first_interaction_id(&self.server, "/v2/interactions/questions", Some(&session_id))
                .ok()
                .flatten()
        }) {
            match api_request_json(
                &self.server,
                "POST",
                &format!("/v2/interactions/questions/{id}/reject"),
                None,
            ) {
                Ok(_) => self.system_message("Question", format!("rejected {id}")),
                Err(error) => self.system_message("Question", error),
            }
            return;
        }
        if let Some(id) =
            first_interaction_id(&self.server, "/v2/interactions/permissions", Some(&session_id))
                .ok()
                .flatten()
        {
            let body = json!({ "reply": "reject" });
            match api_request_json(
                &self.server,
                "POST",
                &format!("/v2/interactions/permissions/{id}/reply"),
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
                        &format!("/v2/sessions/{session_id}/compact"),
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
                    &format!("/v2/sessions/{thread_session}/{thread_action}"),
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
        // A user-driven stop is a hard clear: the status label must not
        // linger through the display grace hold, or Stop reads as lag.
        self.side_panel.clear_status_display_hold();
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

    pub(super) fn execute_abort_session_command(&mut self) {
        let Some(session_id) = self.session_id.clone() else {
            self.note_streaming(NeoismAgentStreamingState::Idle, None);
            self.system_message("Abort", "no session has started yet");
            return;
        };
        match api_request_json(
            &self.server,
            "POST",
            &format!("/v2/sessions/{session_id}/abort"),
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
        // Release the latch even for a stale response from a previous
        // session: a switch mid-request otherwise leaves `loading_older`
        // wedged and the new session can never page back.
        self.timeline_history.loading_older = false;
        if self.session_id.as_deref() != Some(session_id.as_str()) {
            return;
        }
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
        self.prompt_draft_id = self.prompt_draft_id.wrapping_add(1);
        self.session_id = None;
        self.parent_session_id = None;
        self.side_panel.set_viewed_session_id(None);
        // A fresh chat must not inherit the previous conversation's
        // grace-held status label.
        self.side_panel.clear_status_display_hold();
        self.clear_pending_user_prompts();
        self.messages.clear();
        self.reset_transient_timeline_interactions();
        self.invalidate_timeline_layout();
        self.reset_session_runtime_ui();
        self.clear_family_activity();
        self.execution_activity = None;
        self.execution_timer_anchor = None;
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
            "model": prompt_model_json(self.model.as_str(), self.thinking.as_deref(), self.connection_id.as_deref()),
            "agent": self.agent.clone(),
            "command": command,
            "arguments": command_args,
        });
        self.start_session_updates(&session_id);
        match api_request_json(
            &self.server,
            "POST",
            &format!("/v2/sessions/{session_id}/commands"),
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
            .map(|dir| format!("/v2/sessions?directory={}", percent_encode(dir)))
            .unwrap_or_else(|| "/v2/sessions".to_string());
        let body = json!({
            "parentId": null,
            "title": null,
            "agent": self.agent.clone(),
            "model": session_model_json(self.model.as_str(), self.thinking.as_deref(), self.connection_id.as_deref()),
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

pub(super) fn native_prompt_author(local_presence_name: Option<&str>) -> String {
    local_presence_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::screen::presence::local_presence_identity(None).1)
}

#[cfg(test)]
mod native_prompt_author_tests {
    use super::native_prompt_author;

    #[test]
    fn uses_published_presence_name() {
        assert_eq!(native_prompt_author(Some("  Parker  ")), "Parker");
    }

    #[test]
    fn system_identity_prevents_missing_native_author() {
        assert!(!native_prompt_author(None).trim().is_empty());
    }
}

fn dispatch_prompt_request(
    request: PendingPromptDispatch,
) -> Result<(String, Option<AgentSessionEventStream>), String> {
    let (session_id, event_stream) = match request.origin_session_id.as_deref() {
        Some(session_id) => (session_id.to_string(), None),
        None => {
            let session_id = create_prompt_session(&request)?;
            // Subscribe before admitting the first prompt. The receiver can
            // queue updates while this worker waits for the POST response, so
            // a fast provider cannot emit the beginning of the turn before
            // the pane knows which fresh session it belongs to.
            let mut event_stream =
                start_session_event_stream(request.server.clone(), session_id.clone());
            if let Some(wake) = request.event_wake.clone() {
                event_stream.set_wake(wake);
            }
            (session_id, Some(event_stream))
        }
    };
    let body = json!({
        "messageId": request.message_id,
        "model": prompt_model_json(request.model.as_str(), request.thinking.as_deref(), request.connection_id.as_deref()),
        "agent": request.agent,
        "noReply": false,
        "system": request.system,
        "tools": null,
        "author": request.author,
        "parts": request.parts,
        "delivery": match request.delivery {
            neoism_protocol::agent::PromptDelivery::Steer => "steer",
            neoism_protocol::agent::PromptDelivery::Queue => "queue",
        },
    });
    api_request_json(
        &request.server,
        "POST",
        &format!("/v2/sessions/{session_id}/prompt"),
        Some(&body),
    )?;
    Ok((session_id, event_stream))
}

fn create_prompt_session(request: &PendingPromptDispatch) -> Result<String, String> {
    let path = request
        .directory
        .as_deref()
        .map(|directory| format!("/v2/sessions?directory={}", percent_encode(directory)))
        .unwrap_or_else(|| "/v2/sessions".to_string());
    let body = json!({
        "parentId": null,
        "title": null,
        "agent": request.agent.clone(),
        "model": session_model_json(request.model.as_str(), request.thinking.as_deref(), request.connection_id.as_deref()),
        "permission": null,
        "workspaceId": null,
    });
    let response = api_request_json(&request.server, "POST", &path, Some(&body))?
        .ok_or_else(|| "server did not return session".to_string())?;
    response
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "server did not return session id".to_string())
}

pub(super) fn slash_options() -> Vec<NeoismAgentPickerOption> {
    command_controller::slash_options()
}
