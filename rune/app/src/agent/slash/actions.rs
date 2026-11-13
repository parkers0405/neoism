//! Running `/` commands and picker choices against the daemon, and folding its replies back
//! into the pickers (`gpuix/src/agent/SlashController.ts`, `state/connect.rs`,
//! `state/pickers_state.rs`).

use super::catalog::{self, ADD_ACCOUNT, ADD_PROVIDER, DISCONNECT_PROVIDER, DONE, RETRY};
use super::effects::{self, Effect, Running};
use super::picker::{Activity, Kind, Picker, Row};
use super::plan::{COMMANDS, SlashAction, plan};
use super::ConnectFlow;
use crate::app::App;
use crate::tabs::TabKind;
use neoism_protocol::agent::{AgentClientMessage as Client, AgentServerMessage as Server, HistoryMessage, HistoryMessageKind, Role};
use rune::{Key, KeyEvent, Tx};
use serde_json::Value;

impl App {
    /// The conversation the active agent tab shows.
    pub(super) fn active_session(&self) -> Option<String> {
        match &self.active_tab().kind {
            TabKind::Agent { session } => session.clone(),
            TabKind::Home | TabKind::Terminal | TabKind::Editor { .. } | TabKind::Database { .. } | TabKind::GitDiff => None,
        }
    }

    pub(super) fn agent_send(&self, message: Client) {
        #[cfg(test)]
        self.agent_sent.borrow_mut().push(serde_json::to_string(&message).unwrap_or_default());
        if let Some(daemon) = &self.daemon { daemon.agent(&message); }
    }

    /// A line in the transcript from the app itself (`notice` in the original).
    fn notice(&mut self, title: &str, body: impl Into<String>) {
        let Some(session) = self.active_session() else { return };
        let id = format!("notice-{}", self.composer.next_notice());
        self.sessions.entry(session).or_default().messages.push(HistoryMessage {
            id, role: Role::System, kind: HistoryMessageKind::System, author: None, title: title.into(), text: body.into(),
            status: String::new(), tool: String::new(), agent_kind: None, tool_batch_id: None, mcp: None,
            composition_children: Vec::new(), output_kind: None, lang: String::new(), line_offset: None,
            detail: String::new(), todos: Vec::new(), usage: None, attachments: Vec::new(), created_at: 0,
        });
    }

    /// Ask for the catalogs the pickers list (models, agents) once connected.
    pub(crate) fn slash_connected(&mut self) {
        let request_id = self.composer.request_id("models");
        self.agent_send(Client::ListModelCatalog { request_id, directory: Some(self.cwd.clone()) });
        self.agent_send(Client::ListAgents { directory: Some(self.cwd.clone()) });
    }

    // -- the composer --------------------------------------------------------------------

    /// The `/` menu follows the composer while its text is a bare command being typed.
    pub fn composer_changed(&mut self) {
        let mention = super::super::composer_tokens::skill_query(&self.tabs[self.active].draft).map(|(r, q)| (r, q.to_owned()));
        if let Some(picker) = self.composer.picker.as_mut().filter(|p| p.mention.is_some()) {
            if let Some((range, query)) = mention {
                picker.mention = Some(range);
                picker.set_query(&query);
            } else { self.composer.picker = None; }
            return;
        }
        if self.composer.picker.is_none() {
            if let Some((range, query)) = mention {
                self.open_skill_picker(Some(range));
                if let Some(picker) = &mut self.composer.picker { picker.set_query(&query); }
                return;
            }
        }
        let text = self.active_tab().draft.text.clone();
        let typing_command = text.starts_with('/') && !text.contains(char::is_whitespace);
        match &mut self.composer.picker {
            Some(picker) if picker.kind == Kind::Slash => {
                if typing_command { picker.set_query(text.trim_start_matches('/')); } else { self.composer.picker = None; }
            }
            None if typing_command && !self.composer.slash_dismissed => {
                let rows = COMMANDS.iter().map(|c| Row::new(c.title, c.description, c.footer, c.title)).collect();
                let mut picker = Picker::new(Kind::Slash, "Commands", rows);
                picker.set_query(text.trim_start_matches('/'));
                self.composer.picker = Some(picker);
            }
            _ => {}
        }
        if !text.starts_with('/') { self.composer.slash_dismissed = false; }
    }

    /// Keys the composer routes here first. True when the `/` menu used it.
    pub fn agent_composer_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if self.has_prompt() { return self.prompt_key(event); }
        let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Slash || p.mention.is_some()) else { return false };
        if picker.mention.is_some() {
            match event.key {
                Key::Up => { tx.animated(); picker.step(-1); }
                Key::Down => { tx.animated(); picker.step(1); }
                Key::Enter | Key::Tab if !event.mods.shift => self.accept_picker_row(None, tx),
                Key::Escape => self.close_prompt_picker(),
                _ => return false,
            }
            return true;
        }
        match event.key {
            // Keys arrive through a text field (an instant commit): the slide must animate.
            Key::Up => { tx.animated(); picker.step(-1) }
            Key::Down => { tx.animated(); picker.step(1) }
            Key::Escape => {
                self.composer.picker = None;
                self.composer.slash_dismissed = true;
            }
            Key::Tab => {
                // Complete the command and keep typing its arguments.
                if let Some(row) = picker.selected_row() {
                    let text = format!("{} ", row.value);
                    let i = self.active;
                    self.tabs[i].draft.replace_text(&text);
                    self.composer.picker = None;
                }
            }
            Key::Enter if !event.mods.shift => self.accept_picker_row(None, tx),
            _ => return false,
        }
        true
    }

    /// A submitted composer line that is a command runs here instead of being sent.
    pub fn submit_command(&mut self, tx: &mut Tx) -> bool {
        let i = self.active;
        if !self.tabs[i].draft.text.trim().starts_with('/') { return false; }
        let text = self.expanded_composer_text(i).trim().to_owned();
        self.tabs[i].draft.clear();
        self.tabs[i].pastes.clear();
        self.composer.picker = None;
        self.run_slash(&text, tx);
        true
    }

    // -- commands ------------------------------------------------------------------------

    pub fn run_slash(&mut self, text: &str, tx: &mut Tx) {
        let session = self.active_session();
        let need_session = |app: &mut App| {
            if session.is_none() { app.notice("Commands", "Open a conversation first"); }
            session.clone()
        };
        match plan(text) {
            SlashAction::Noop => {}
            SlashAction::ShowHelp => {
                let help = COMMANDS.iter().map(|c| format!("{} \u{2014} {}", c.title, c.description)).collect::<Vec<_>>().join("\n");
                self.notice("Commands", help);
            }
            SlashAction::OpenModelPicker => self.open_selection(Kind::Model),
            SlashAction::OpenAgentPicker => self.open_selection(Kind::Agent),
            SlashAction::OpenThinkingPicker => self.open_selection(Kind::Thinking),
            SlashAction::ApplyModel(model) => {
                let found = self.composer.providers.iter().flat_map(|p| p.models.iter().map(move |m| format!("{}/{}", p.id, m.id)))
                    .find(|id| *id == model || id.ends_with(&format!("/{model}")));
                match found {
                    Some(id) => self.set_model(id),
                    None => self.notice("Model", "Model not found. Use /model to choose a server model"),
                }
            }
            SlashAction::ApplyAgent(agent) => {
                if self.composer.agents.iter().any(|a| a.name == agent) { self.set_agent(agent); } else { self.notice("Agent", "Unknown agent. Use /agent to choose"); }
            }
            SlashAction::ApplyThinking(thinking) => {
                let thinking = match thinking.as_str() { "none" | "default" | "off" => String::new(), other => other.to_owned() };
                self.set_thinking(thinking);
            }
            SlashAction::OpenConnectPicker => self.open_connect(),
            SlashAction::ShowMcp => self.open_mcp(),
            SlashAction::OpenSessionsPicker => {
                let rows = self.threads.iter().map(|t| Row::new(t.title.clone(), t.directory.clone().unwrap_or_default(), "", t.session_id.clone())
                    .current(Some(&t.session_id) == session.as_ref())).collect();
                self.composer.picker = Some(Picker::new(Kind::Session, "Sessions", rows).select_value(session.as_deref().unwrap_or_default()));
                self.refresh_threads();
            }
            SlashAction::SwitchSession(id) => self.open_conversation(&id),
            SlashAction::OpenSubagentPicker => {
                let Some(id) = need_session(self) else { return };
                let root = self.families.root_of(&id).to_owned();
                let rows = self.families.roster(&root).iter().map(|m| Row::new(m.title.clone(), m.status.clone(), m.agent.clone().unwrap_or_default(), m.id.clone())).collect();
                self.composer.picker = Some(Picker::new(Kind::Subagent, "Subagent sessions", rows));
            }
            SlashAction::OpenSkillPicker | SlashAction::ShowSkills => {
                self.open_skill_picker(None);
            }
            SlashAction::ShowSkill(name) => self.notice("Skill", format!("Skill {name}")),
            SlashAction::ShowSkillUsage => self.notice("Skill", "Usage: /skill info <name>"),
            SlashAction::InsertSkillMention(name) => self.insert_skill_pill(&name, None),
            SlashAction::CompactSession => {
                let Some(session_id) = need_session(self) else { return };
                let request_id = Some(self.composer.request_id("compact"));
                self.agent_send(Client::Compact { session_id, request_id });
            }
            SlashAction::UndoSession => if let Some(session_id) = need_session(self) { self.edit_agent_history(&session_id, false); },
            SlashAction::RedoSession => if let Some(session_id) = need_session(self) { self.edit_agent_history(&session_id, true); },
            SlashAction::AbortSession => if let Some(session_id) = need_session(self) { self.agent_send(Client::CancelInflight { session_id }); },
            SlashAction::HandleQueue(action) => if let Some(session_id) = need_session(self) { self.agent_send(Client::HandleQueue { session_id, action }); },
            SlashAction::ShowPermissions => if let Some(session_id) = need_session(self) { self.agent_send(Client::ShowPermissions { session_id }); },
            SlashAction::ShowQuestions => if let Some(session_id) = need_session(self) { self.agent_send(Client::ShowQuestions { session_id }); },
            SlashAction::HandlePermit(args) => if let Some(session_id) = need_session(self) {
                self.agent_send(Client::HandlePermit { session_id, reply: args.first().cloned().unwrap_or_else(|| "once".into()), request_id: args.get(1).cloned() });
            },
            SlashAction::HandleAnswer(answer) => if let Some(session_id) = need_session(self) {
                if answer.trim().is_empty() { self.notice("Question", "Usage: /answer <text>"); } else { self.agent_send(Client::HandleAnswer { session_id, answer }); }
            },
            SlashAction::HandleReject(request_id) => if let Some(session_id) = need_session(self) { self.agent_send(Client::HandleReject { session_id, request_id }); },
            SlashAction::ToggleSkipPermissions | SlashAction::ShowGoal | SlashAction::SetGoal(_) | SlashAction::ClearGoal
            | SlashAction::PauseGoal | SlashAction::ResumeGoal | SlashAction::ChangeDirectory(_) => {
                // Server-side plugins; the daemon forwards the command text.
                match session.clone() {
                    Some(session_id) => self.agent_send(Client::SlashCommand { session_id, text: text.to_owned() }),
                    None => self.notice("Commands", "Open a conversation first"),
                }
            }
            SlashAction::OpenUsagePicker => self.open_usage(),
            SlashAction::OpenTasksPicker => self.open_tasks(),
            SlashAction::OpenGoalsPicker => self.open_goals(),
            SlashAction::OpenDirectoryPicker => self.open_directory_picker(),
            SlashAction::ToggleInputHelp => {
                self.composer.hints_hidden = !self.composer.hints_hidden;
                self.agent_send(Client::SetInputHelpVisible { visible: !self.composer.hints_hidden });
            }
            SlashAction::ToggleSidebar => {
                self.toggle_conversations();
                self.agent_send(Client::SetSidebarVisible { visible: self.sidebar.is_some() });
            }
            SlashAction::Effect(effect) => self.play_effect(effect),
            SlashAction::CreateNewSession => {
                let i = self.active;
                self.tabs[i].kind = TabKind::Agent { session: None };
            }
            SlashAction::RequestCloseTab => self.close_tab(self.active),
            SlashAction::RunServerCommand { .. } => match session {
                Some(session_id) => self.agent_send(Client::SlashCommand { session_id, text: text.to_owned() }),
                None => {
                    // A server command starts the conversation it runs in.
                    let i = self.active;
                    self.create_thread(i, text.to_owned());
                }
            },
        }
        let _ = tx;
    }

    fn open_skill_picker(&mut self, mention: Option<std::ops::Range<usize>>) {
        let mut picker = Picker::loading(Kind::Skill, "Skills");
        picker.query.focused = mention.is_none();
        picker.mention = mention;
        self.focus_picker(picker);
        self.agent_send(Client::ListSkills { directory: Some(self.cwd.clone()) });
    }

    // -- selections ----------------------------------------------------------------------

    /// Model, agent and reasoning pickers (also opened from the composer chips).
    pub fn open_selection(&mut self, kind: Kind) {
        let selection = self.composer.selection.clone();
        let picker = match kind {
            Kind::Model => {
                let catalog = catalog::models(&self.composer.providers);
                let rows = catalog::model_root(&catalog, &selection.model, &self.composer.recent_models);
                let current = selection.model.split_once('/').map(|(p, _)| p).unwrap_or_default().to_owned();
                let mut picker = Picker::new(Kind::Model, "Select model", rows).placeholder("Search recent models or providers").select_value(&current);
                picker.loading = self.composer.providers.is_empty();
                let request_id = self.composer.request_id("models");
                self.agent_send(Client::ListModelCatalog { request_id, directory: Some(self.cwd.clone()) });
                picker
            }
            Kind::Agent => {
                self.agent_send(Client::ListAgents { directory: Some(self.cwd.clone()) });
                let mut picker = Picker::new(Kind::Agent, "Agents", catalog::agents(&self.composer.agents, &selection.agent)).select_value(&selection.agent);
                picker.loading = self.composer.agents.is_empty();
                picker
            }
            _ => Picker::new(Kind::Thinking, "Reasoning effort", catalog::thinking(&selection.model, &selection.thinking)).select_value(&selection.thinking),
        };
        self.focus_picker(picker);
    }

    pub(super) fn focus_picker(&mut self, picker: Picker) {
        let i = self.active;
        self.tabs[i].draft.focused = picker.kind == Kind::Slash || picker.mention.is_some();
        self.composer.picker = Some(picker);
    }

    /// Close any picker and give the composer its focus back.
    pub fn close_prompt_picker(&mut self) {
        self.composer.picker = None;
        self.composer.connect = None;
        self.composer.mcp_server = None;
        let i = self.active;
        self.tabs[i].draft.focused = true;
    }

    fn set_model(&mut self, model: String) {
        self.composer.recent_models.retain(|m| *m != model);
        self.composer.recent_models.insert(0, model.clone());
        self.composer.recent_models.truncate(8);
        self.composer.selection.model = model.clone();
        if let Some(session_id) = self.active_session() {
            let thinking = Some(self.composer.selection.thinking.clone()).filter(|t| !t.is_empty());
            self.agent_send(Client::SetModel { session_id, model, connection_id: None, thinking });
        }
        self.remember_composer_selection();
        self.sync_chips();
    }

    fn set_agent(&mut self, agent: String) {
        self.composer.selection.agent = agent.clone();
        if let Some(session_id) = self.active_session() { self.agent_send(Client::SetAgent { session_id, agent }); }
        self.remember_composer_selection();
        self.sync_chips();
    }

    fn set_thinking(&mut self, thinking: String) {
        self.composer.selection.thinking = thinking.clone();
        if let Some(session_id) = self.active_session() { self.agent_send(Client::SetThinkingMode { session_id, thinking }); }
        self.remember_composer_selection();
        self.sync_chips();
    }

    /// Selection belongs to a conversation, or to an unsent tab until it gets one.
    fn remember_composer_selection(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if let TabKind::Agent { session: Some(id) } = &tab.kind {
            self.sessions.entry(id.clone()).or_default().selection = Some(self.composer.selection.clone());
        } else if matches!(tab.kind, TabKind::Agent { session: None }) {
            tab.selection = Some(self.composer.selection.clone());
        }
    }

    pub(crate) fn restore_composer_selection(&mut self) -> bool {
        let Some(tab) = self.tabs.get(self.active) else { return false };
        let TabKind::Agent { session } = &tab.kind else { self.composer.selection_owner = None; return false };
        let owner = (tab.id, session.clone());
        if self.composer.selection_owner.as_ref() == Some(&owner) { return false; }
        let selection = match session {
            Some(id) => self.sessions.get(id).and_then(|s| s.selection.clone()).unwrap_or_default(),
            None => tab.selection.clone().unwrap_or_else(|| self.composer.defaults.clone()),
        };
        let session = session.clone();
        self.composer.selection = selection;
        self.composer.selection_owner = Some(owner);
        self.composer.picker = None;
        self.sync_chips();
        // The conversation's own settings come back with the switch. A sub-agent has no input
        // bar, and switching the daemon to it binds a second stream to it beside its
        // conversation's: every word of its reply then streamed twice.
        if let Some(session_id) = session.filter(|id| !self.families.is_child(id)) { self.switch_conversation(&session_id); }
        true
    }

    /// The chip labels: title-cased ids (`Openai/Gpt-6.1-Sol`, `Build`, `Medium`).
    pub(crate) fn sync_chips(&mut self) {
        let s = &self.composer.selection;
        self.model = if s.model.is_empty() { "Select model".into() } else { crate::agent::title_case(&s.model) };
        self.agent = crate::agent::title_case(if s.agent.is_empty() { "build" } else { &s.agent });
        self.effort = crate::agent::title_case(if s.thinking.is_empty() { "default" } else { &s.thinking });
    }

    // -- picker choices ------------------------------------------------------------------

    /// Run the selected row (or row `index`, when clicked).
    pub fn accept_picker_row(&mut self, index: Option<usize>, tx: &mut Tx) {
        let Some(picker) = self.composer.picker.as_mut() else { return };
        if let Some(index) = index { picker.selected = index; }
        let kind = picker.kind;
        if kind == Kind::Usage { self.open_usage(); return; }
        if kind.is_entry() {
            let text = picker.query.text.trim().to_owned();
            return self.submit_entry(kind, text);
        }
        let Some(row) = picker.selected_row().cloned() else { return };
        match kind {
            Kind::Slash => {
                let i = self.active;
                self.tabs[i].draft.clear();
                self.composer.picker = None;
                self.run_slash(&row.value, tx);
            }
            Kind::Model => {
                if row.value == ADD_PROVIDER { return self.open_connect(); }
                let on_providers = self.composer.picker.as_ref().is_some_and(|p| p.provider.is_none());
                if on_providers && !row.value.is_empty() && !row.value.contains('/') {
                    return self.open_provider_models(&row.value, &row.title);
                }
                self.set_model(row.value);
                self.close_prompt_picker();
            }
            Kind::Agent => { self.set_agent(row.value); self.close_prompt_picker(); }
            Kind::Thinking => { self.set_thinking(row.value); self.close_prompt_picker(); }
            Kind::Session => { self.close_prompt_picker(); self.open_conversation(&row.value); }
            Kind::Usage => { if row.value == "refresh" { self.open_usage(); } },
            Kind::Tasks | Kind::Goals => {},
            Kind::Subagent => { self.close_prompt_picker(); self.view_session(&row.value); }
            Kind::Skill => {
                let inline = picker.mention.is_some();
                let range = if inline { super::super::composer_tokens::skill_query(&self.tabs[self.active].draft).map(|(r, _)| r) } else { None };
                self.close_prompt_picker();
                if !inline || range.is_some() { self.insert_skill_pill(&row.value, range); }
            }
            Kind::Mcp => self.open_mcp_actions(&row.value),
            Kind::McpActions => self.run_mcp_action(&row.value),
            Kind::Connect => self.choose_connect_provider(&row.value),
            Kind::ConnectAccount => self.choose_account(&row.value),
            Kind::ConnectAccountActions => self.run_account_action(&row.value),
            Kind::ConnectAuth => self.choose_auth_method(&row.value),
            Kind::ConnectConfirm => self.confirm_disconnect(row.value == "confirm_disconnect"),
            Kind::ConnectSecret | Kind::ConnectLabel => {}
            Kind::Directory => {
                self.close_prompt_picker();
                self.change_session_directory(&row.value);
            }
        }
    }

    // -- /cd -----------------------------------------------------------------------------

    /// `/cd` with no path (`open_directory_picker`): the session directory, home, then the
    /// daemon's fuzzy directory search as the query changes.
    fn open_directory_picker(&mut self) {
        let picker = Picker::new(Kind::Directory, "Change directory", self.directory_rows(Vec::new()))
            .placeholder("Path or fuzzy directory");
        self.focus_picker(picker);
    }

    fn directory_rows(&self, hits: Vec<String>) -> Vec<Row> {
        let query = self.composer.picker.as_ref().filter(|p| p.kind == Kind::Directory)
            .map(|p| p.query.text.trim().to_owned()).unwrap_or_default();
        let mut rows = Vec::new();
        if query.is_empty() {
            rows.push(Row::new(self.display_cwd(), "Current session directory", "current", self.cwd.clone()).current(true));
            rows.push(Row::new("~/", "Home directory", "directory", "~"));
        } else {
            // A typed path is always offered as-is first, then the matches.
            rows.push(Row::new(query.clone(), "Go to this path", "path", query.clone()));
        }
        rows.extend(hits.into_iter().filter(|h| *h != query).map(|h| Row::new(h.clone(), "", "directory", h)));
        rows
    }

    /// The `/cd` query changed: ask the daemon for directories matching it.
    pub(crate) fn directory_query_changed(&mut self) {
        let Some(picker) = self.composer.picker.as_ref().filter(|p| p.kind == Kind::Directory) else { return };
        let query = picker.query.text.trim().to_owned();
        let rows = self.directory_rows(Vec::new());
        if let Some(picker) = self.composer.picker.as_mut() { picker.replace(rows); }
        if query.is_empty() { return; }
        // Composer searches use their own id range so the palette's replies never cross.
        self.composer.directory_request += 1;
        let req_id = (1u64 << 40) + self.composer.directory_request;
        let message = serde_json::json!({"SearchDirectories": {"req_id": req_id, "query": query, "cwd": ""}});
        if let Some(daemon) = &self.daemon { daemon.send_scoped(crate::daemon::Family::Search, message, &self.cwd); }
    }

    /// Directory hits for the `/cd` picker; false when they belong to someone else.
    pub(crate) fn apply_composer_directory_hits(&mut self, req_id: u64, hits: &[String]) -> bool {
        if req_id != (1u64 << 40) + self.composer.directory_request { return false; }
        if !self.composer.picker.as_ref().is_some_and(|p| p.kind == Kind::Directory) { return true; }
        let rows = self.directory_rows(hits.to_vec());
        if let Some(picker) = self.composer.picker.as_mut() { picker.replace(rows); }
        true
    }

    /// Point the conversation at `directory` (the daemon PATCHes the session and reports back).
    fn change_session_directory(&mut self, directory: &str) {
        let session = match &self.active_tab().kind { TabKind::Agent { session } => session.clone(), _ => None };
        match session {
            Some(session_id) => self.agent_send(Client::SlashCommand { session_id, text: format!("/cd {directory}") }),
            None => self.notice("Directory", "Open a conversation first"),
        }
    }

    /// Back out one screen (Esc on a nested screen, Backspace on an empty query).
    pub fn picker_back(&mut self) -> bool {
        let Some(picker) = self.composer.picker.as_mut() else { return false };
        match picker.kind {
            Kind::Model if picker.provider.is_some() => { self.open_selection(Kind::Model); true }
            Kind::McpActions => { self.open_mcp(); true }
            _ => false,
        }
    }

    fn open_provider_models(&mut self, provider: &str, name: &str) {
        let catalog = catalog::models(&self.composer.providers);
        let rows = catalog::provider_models(&catalog, provider, &self.composer.selection.model);
        let mut picker = Picker::new(Kind::Model, name, rows).placeholder("Search models").select_value(&self.composer.selection.model);
        picker.provider = Some(provider.to_owned());
        picker.last_provider = Some(provider.to_owned());
        self.focus_picker(picker);
    }

    /// A picker key from its own query field.
    pub fn picker_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(picker) = self.composer.picker.as_mut() else { return false };
        if matches!(picker.kind, Kind::Usage | Kind::Tasks | Kind::Goals) {
            match event.key {
                Key::Enter if picker.kind == Kind::Usage => self.open_usage(),
                Key::Enter if picker.kind == Kind::Goals => self.open_goals(),
                Key::Escape => self.close_prompt_picker(),
                Key::Up | Key::Down => {
                    let delta = if event.key == Key::Down { 60.0 } else { -60.0 } * self.ty.scale();
                    picker.usage_scroll = (picker.usage_scroll + delta).clamp(0.0, picker.usage_max_scroll.get());
                    tx.animated();
                }
                _ => {},
            }
            return true;
        }
        match event.key {
            // Keys arrive through a text field (an instant commit): the slide must animate.
            Key::Up => { tx.animated(); picker.step(-1) }
            Key::Down => { tx.animated(); picker.step(1) }
            Key::Escape => { if !self.picker_back() { self.close_prompt_picker(); } }
            Key::Backspace if picker.query.text.is_empty() => { if !self.picker_back() { return false; } }
            Key::Enter => self.accept_picker_row(None, tx),
            _ => return crate::keys::shortcut(self, event, tx),
        }
        true
    }

    // -- MCP -----------------------------------------------------------------------------

    pub(crate) fn open_mcp(&mut self) {
        let rows = catalog::mcp_servers(&self.composer.mcp_status);
        let mut picker = Picker::new(Kind::Mcp, "MCP servers", rows);
        picker.loading = true;
        self.composer.mcp_server = None;
        self.focus_picker(picker);
        self.agent_send(Client::ShowMcp { directory: Some(self.cwd.clone()) });
    }

    fn open_mcp_actions(&mut self, value: &str) {
        let Ok(entry) = serde_json::from_str::<Value>(value) else { return };
        let name = entry["name"].as_str().unwrap_or("MCP").to_owned();
        self.composer.mcp_server = Some(name.clone());
        self.focus_picker(Picker::new(Kind::McpActions, format!("{name} actions"), catalog::mcp_actions(&entry)));
    }

    fn run_mcp_action(&mut self, value: &str) {
        let Ok(action) = serde_json::from_str::<Value>(value) else { return };
        let name = action["name"].as_str().unwrap_or_default().to_owned();
        let directory = Some(self.cwd.clone());
        let (message, body) = match action["action"].as_str().unwrap_or_default() {
            "enable" => (Client::McpSetEnabled { name: name.clone(), enabled: true, directory }, "Enabling"),
            "disable" => (Client::McpSetEnabled { name: name.clone(), enabled: false, directory }, "Disabling"),
            "connect" => (Client::McpConnect { name: name.clone(), directory, redirect_uri: None }, "Connecting"),
            "disconnect" => (Client::McpDisconnect { name: name.clone(), directory }, "Disconnecting"),
            "authenticate" => (Client::McpOauthAuthorize { name: name.clone(), directory, redirect_uri: None }, "Preparing authorization for"),
            "logout" => (Client::McpRemoveAuth { name: name.clone(), directory }, "Logging out of"),
            _ => return,
        };
        self.agent_send(message);
        if let Some(picker) = self.composer.picker.as_mut() {
            picker.activity = Some(Activity { body: format!("{body} {name}"), status: "Working".into(), busy: true, show_input: false, url: None });
        }
    }

    // -- /connect ------------------------------------------------------------------------

    fn open_connect(&mut self) {
        self.composer.connect = Some(ConnectFlow { waiting: true, ..ConnectFlow::default() });
        self.focus_picker(Picker::loading(Kind::Connect, "Connect a provider"));
        let request_id = Some(self.composer.request_id("connect"));
        self.agent_send(Client::ConnectListProviders { directory: Some(self.cwd.clone()), request_id });
    }

    fn connect_activity(&mut self, body: impl Into<String>, status: &str, busy: bool) {
        if let Some(picker) = self.composer.picker.as_mut() {
            picker.activity = Some(Activity { body: body.into(), status: status.into(), busy, show_input: false, url: None });
        }
    }

    fn choose_connect_provider(&mut self, value: &str) {
        match value {
            DONE => return self.close_prompt_picker(),
            RETRY => {
                let method = self.composer.connect.as_ref().and_then(|f| f.method.clone());
                if let Some(method) = method { return self.choose_auth_method(&method.index.to_string()); }
                return self.open_connect();
            }
            _ => {}
        }
        let Some(flow) = self.composer.connect.as_mut() else { return };
        let Some(provider) = flow.providers.iter().find(|p| p.id == value).cloned() else { return };
        flow.provider = Some(provider.clone());
        flow.waiting = true;
        self.focus_picker(Picker::loading(Kind::ConnectAccount, "Choose an account"));
        self.agent_send(Client::ConnectListConnections { provider_id: provider.id });
    }

    fn choose_account(&mut self, value: &str) {
        let Some(flow) = self.composer.connect.as_mut() else { return };
        if value == ADD_ACCOUNT {
            flow.adding = true;
            flow.connection = None;
            return self.focus_picker(Picker::new(Kind::ConnectLabel, "Label new account", Vec::new()).placeholder("Account label"));
        }
        let Some(account) = flow.connections.iter().find(|c| c.connection_id == value).cloned() else { return };
        flow.connection = Some(account.clone());
        let mut rows = vec![Row::new("Select account", account.label.clone(), "use", "select"), Row::new("Rename", account.label.clone(), "edit label", "rename")];
        if !account.is_default { rows.push(Row::new("Make default", account.label.clone(), "default", "default")); }
        rows.push(Row::new("Disconnect", account.label.clone(), "remove", "disconnect"));
        self.focus_picker(Picker::new(Kind::ConnectAccountActions, "Account actions", rows));
    }

    fn run_account_action(&mut self, action: &str) {
        let Some(flow) = self.composer.connect.as_mut() else { return };
        let (Some(provider), Some(account)) = (flow.provider.clone(), flow.connection.clone()) else { return };
        match action {
            "select" => {
                self.close_prompt_picker();
                self.notice("Account", format!("Using {}.", account.label));
            }
            "rename" => {
                flow.adding = false;
                let mut picker = Picker::new(Kind::ConnectLabel, "Rename account", Vec::new()).placeholder(account.label.clone());
                picker.query.replace_text(&account.label);
                self.focus_picker(picker);
            }
            "default" => {
                flow.waiting = true;
                self.agent_send(Client::ConnectSetDefault { provider_id: provider.id, connection_id: account.connection_id });
                self.connect_activity("Updating the default account", "Updating account", true);
            }
            "disconnect" => {
                let rows = vec![Row::new("Disconnect", account.label.clone(), "cannot be undone", "confirm_disconnect"), Row::new("Cancel", "", "keep account", "cancel")];
                let mut picker = Picker::new(Kind::ConnectConfirm, "Disconnect account?", rows);
                picker.step(1);
                self.focus_picker(picker);
            }
            _ => {}
        }
    }

    fn confirm_disconnect(&mut self, confirm: bool) {
        let Some(flow) = self.composer.connect.as_mut() else { return };
        let (Some(provider), Some(account)) = (flow.provider.clone(), flow.connection.clone()) else { return };
        if !confirm { return self.close_prompt_picker(); }
        flow.waiting = true;
        self.agent_send(Client::ConnectDisconnect { provider_id: provider.id, connection_id: Some(account.connection_id) });
        self.connect_activity("Disconnecting the account", "Updating account", true);
    }

    fn open_auth_methods(&mut self) {
        let Some(flow) = self.composer.connect.as_ref() else { return };
        let Some(provider) = flow.provider.clone() else { return };
        let methods = flow.methods.get(&provider.id).cloned().unwrap_or_default();
        if methods.is_empty() {
            self.close_prompt_picker();
            return self.notice("Connect", format!("{} exposes no auth methods", provider.name));
        }
        let mut provider = provider;
        provider.connected &= !flow.adding;
        let title = format!("{} \u{2014} select auth method", provider.name);
        self.focus_picker(Picker::new(Kind::ConnectAuth, title, catalog::connect_methods(&provider, &methods)));
    }

    fn choose_auth_method(&mut self, value: &str) {
        let Some(flow) = self.composer.connect.as_mut() else { return };
        let Some(provider) = flow.provider.clone() else { return };
        if value == DISCONNECT_PROVIDER {
            flow.waiting = true;
            let connection_id = flow.connection.as_ref().map(|c| c.connection_id.clone());
            self.agent_send(Client::ConnectDisconnect { provider_id: provider.id, connection_id });
            return self.connect_activity("Disconnecting the provider", "Updating account", true);
        }
        let Ok(index) = value.parse::<usize>() else { return };
        let methods = flow.methods.get(&provider.id).cloned().unwrap_or_default();
        let Some(mut method) = methods.iter().find(|m| m.index == index).cloned() else { return };
        // Desktop parity: browser OAuth for OpenAI (and xAI) runs the headless variant.
        let wants_headless = (provider.id == "openai" && method.label.to_lowercase().contains("browser"))
            || (provider.id == "xai" && !method.api && !method.label.contains("Headless"));
        if wants_headless {
            if let Some(headless) = methods.iter().find(|m| m.label.to_lowercase().contains("headless")) { method = headless.clone(); }
        }
        flow.method = Some(method.clone());
        let (label, connection_id) = (flow.label.clone(), flow.connection.as_ref().map(|c| c.connection_id.clone()));
        if method.api { return self.open_secret_entry(true); }
        flow.waiting = true;
        self.agent_send(Client::ConnectOauthAuthorize { provider_id: provider.id, method_index: method.index, label, connection_id });
        self.connect_activity(format!("Preparing sign-in to {}", provider.name), "Preparing authorization", true);
    }

    fn open_secret_entry(&mut self, api: bool) {
        let (title, placeholder) = if api { ("Manually enter API Key", "API key") } else { ("Paste OAuth token", "OAuth token") };
        self.focus_picker(Picker::new(Kind::ConnectSecret, title, Vec::new()).placeholder(placeholder));
    }

    fn submit_entry(&mut self, kind: Kind, text: String) {
        let Some(flow) = self.composer.connect.as_mut() else { return };
        let Some(provider) = flow.provider.clone() else { return };
        if kind == Kind::ConnectLabel {
            if text.is_empty() { return; }
            if flow.adding {
                flow.label = Some(text);
                return self.open_auth_methods();
            }
            let Some(account) = flow.connection.clone() else { return };
            flow.waiting = true;
            self.agent_send(Client::ConnectRename { provider_id: provider.id, connection_id: account.connection_id, label: text });
            return self.connect_activity("Renaming the account", "Updating account", true);
        }
        if text.is_empty() { return; }
        let Some(method) = flow.method.clone() else { return };
        flow.waiting = true;
        let (label, connection_id, attempt_id) = (flow.label.clone(), flow.connection.as_ref().map(|c| c.connection_id.clone()), flow.attempt_id.clone());
        if method.api {
            self.agent_send(Client::ConnectStoreApiKey { provider_id: provider.id, key: text, label, connection_id });
            self.connect_activity(format!("Saving the connection to {}", provider.name), "Connecting", true);
        } else {
            self.agent_send(Client::ConnectOauthCallback { provider_id: provider.id, method_index: method.index, code: Some(text), attempt_id });
            self.connect_activity(format!("Finishing sign-in to {}", provider.name), "Connecting", true);
        }
    }

    fn connect_finished(&mut self, provider_id: &str, ok: Result<(), String>) {
        let Some(flow) = self.composer.connect.as_mut() else { return };
        let name = flow.provider.as_ref().filter(|p| p.id == provider_id).map(|p| p.name.clone()).unwrap_or_else(|| provider_id.to_owned());
        flow.waiting = false;
        match ok {
            Ok(()) => {
                let authenticating = flow.method.is_some();
                let rows = vec![Row::new("Done", "", "", DONE)];
                self.focus_picker(Picker::new(Kind::Connect, "Provider connection", rows));
                let (body, status) = if authenticating { (format!("{name} connected. Open /model to pick a model."), "Connected") } else { (format!("{name} account updated."), "Updated") };
                self.connect_activity(body.clone(), status, false);
                self.notice(status, body);
                let request_id = self.composer.request_id("models");
                self.agent_send(Client::ListModelCatalog { request_id, directory: Some(self.cwd.clone()) });
            }
            Err(error) => {
                let api = flow.method.as_ref().map(|m| m.api);
                if api == Some(true) {
                    self.open_secret_entry(true);
                } else {
                    let first = if api.is_some() { Row::new("Retry authorization", "", "", RETRY) } else { Row::new("Back to accounts", "", "", RETRY) };
                    self.focus_picker(Picker::new(Kind::Connect, "Provider connection", vec![first, Row::new("Cancel", "", "", DONE)]));
                }
                self.connect_activity(error.clone(), "Connection failed", false);
                self.notice(&name, error);
            }
        }
    }

    // -- daemon replies ------------------------------------------------------------------

    /// Fold catalogs, selection state and connect/MCP replies in. True when the message was
    /// only for the slash system.
    pub(crate) fn apply_slash_event(&mut self, message: &Server) -> bool {
        match message {
            Server::ProviderCatalog { providers, .. } => {
                self.composer.providers = providers.clone();
                if let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Model) {
                    let catalog = catalog::models(providers);
                    let selection = &self.composer.selection;
                    let rows = match picker.provider.clone() {
                        Some(provider) => catalog::provider_models(&catalog, &provider, &selection.model),
                        None => catalog::model_root(&catalog, &selection.model, &self.composer.recent_models),
                    };
                    picker.replace(rows);
                }
                true
            }
            Server::AgentCatalog { agents } => {
                self.composer.agents = agents.clone();
                if let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Agent) {
                    picker.replace(catalog::agents(agents, &self.composer.selection.agent));
                }
                true
            }
            Server::SkillCatalog { skills } => {
                if let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Skill) {
                    picker.replace(skills.iter().map(|s| Row::new(s.name.clone(), s.description.clone(), "skill", s.name.clone())).collect());
                }
                true
            }
            Server::ConfigDefaults { agent, model, thinking, input_help_visible, .. } => {
                let s = &mut self.composer.defaults;
                if let Some(agent) = agent { s.agent = agent.clone(); }
                if let Some(model) = model.as_ref().filter(|_| s.model.is_empty()) { s.model = model.clone(); }
                if let Some(thinking) = thinking { s.thinking = thinking.clone(); }
                if let Some(visible) = input_help_visible { self.composer.hints_hidden = !visible; }
                if self.active_session().is_none() && self.tabs.get(self.active).is_some_and(|t| t.selection.is_none()) {
                    self.composer.selection = self.composer.defaults.clone();
                    self.sync_chips();
                }
                true
            }
            Server::ProviderState { session_id, model, provider_id, agent, thinking, authoritative, .. } => {
                let s = self.sessions.entry(session_id.clone()).or_default().selection.get_or_insert_with(Default::default);
                if let Some(model) = model {
                    let provider = provider_id.clone().unwrap_or_default();
                    s.model = if provider.is_empty() || model.starts_with(&format!("{provider}/")) { model.clone() } else { format!("{provider}/{model}") };
                }
                if let Some(agent) = agent { s.agent = agent.clone(); }
                if let Some(thinking) = thinking { s.thinking = thinking.clone(); } else if *authoritative { s.thinking.clear(); }
                let selection = s.clone();
                if self.active_session().as_deref() == Some(session_id.as_str()) {
                    self.composer.selection = selection;
                    self.sync_chips();
                }
                false
            }
            Server::McpCatalog { status, .. } => {
                if let Some(servers) = status.as_object() { for entry in servers.values() { self.mcp_icons.prepare(entry); } }
                self.composer.mcp_status = status.clone();
                if let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Mcp) {
                    picker.replace(catalog::mcp_servers(status));
                    picker.activity = None;
                }
                true
            }
            Server::McpChanged { .. } => {
                if self.composer.picker.as_ref().is_some_and(|p| matches!(p.kind, Kind::Mcp | Kind::McpActions)) { self.open_mcp(); }
                true
            }
            Server::McpOauthUrl { name, url } => {
                open_in_browser(url);
                if let Some(picker) = self.composer.picker.as_mut() {
                    picker.activity = Some(Activity { body: format!("Authorize {name} in your browser."), status: "Waiting for authorization".into(), busy: true, show_input: false, url: Some(url.clone()) });
                }
                true
            }
            Server::McpFailed { name, error } => {
                if let Some(picker) = self.composer.picker.as_mut() {
                    picker.activity = Some(Activity { body: error.clone(), status: format!("{} failed", name.as_deref().unwrap_or("MCP")), busy: false, show_input: false, url: None });
                    picker.loading = false;
                }
                true
            }
            Server::ConnectProviderCatalog { providers, auth, .. } => {
                let Some(flow) = self.composer.connect.as_mut() else { return true };
                let (list, methods) = catalog::parse_connect(providers, auth);
                flow.providers = list;
                flow.methods = methods;
                flow.waiting = false;
                if let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Connect) {
                    picker.replace(catalog::connect_providers(&flow.providers));
                }
                true
            }
            Server::ConnectProviderCatalogFailed { error, .. } => {
                if let Some(picker) = self.composer.picker.as_mut().filter(|p| p.kind == Kind::Connect) {
                    picker.replace(vec![Row::new("Retry", "", "", RETRY), Row::new("Cancel", "", "", DONE)]);
                }
                self.connect_activity(error.clone(), "Catalog unavailable", false);
                true
            }
            Server::ConnectConnections { provider, connections } => {
                let Some(flow) = self.composer.connect.as_mut() else { return true };
                if flow.provider.as_ref().map(|p| p.id.as_str()) != Some(provider.as_str()) { return true; }
                flow.connections = connections.clone();
                flow.waiting = false;
                let name = flow.provider.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                self.focus_picker(Picker::new(Kind::ConnectAccount, name, catalog::connect_accounts(connections)));
                true
            }
            Server::ConnectOauthUrl { url, auto, instructions, attempt_id } => {
                let Some(flow) = self.composer.connect.as_mut() else { return true };
                flow.attempt_id = attempt_id.clone();
                let (Some(provider), Some(method)) = (flow.provider.clone(), flow.method.clone()) else { return true };
                open_in_browser(url);
                if *auto {
                    // The daemon waits for the browser redirect; Neoism connects on its own.
                    self.agent_send(Client::ConnectOauthCallback { provider_id: provider.id, method_index: method.index, code: None, attempt_id: attempt_id.clone() });
                    let body = if instructions.trim().is_empty() { "Finish signing in in your browser. Neoism connects automatically.".to_owned() } else { instructions.clone() };
                    if let Some(picker) = self.composer.picker.as_mut() {
                        picker.activity = Some(Activity { body, status: "Waiting for authorization".into(), busy: true, show_input: false, url: Some(url.clone()) });
                    }
                } else {
                    self.open_secret_entry(false);
                    let body = if instructions.trim().is_empty() { "Authorize in your browser, then paste the token below.".to_owned() } else { instructions.clone() };
                    if let Some(picker) = self.composer.picker.as_mut() {
                        picker.activity = Some(Activity { body, status: "Awaiting token".into(), busy: false, show_input: true, url: Some(url.clone()) });
                    }
                }
                true
            }
            Server::ConnectFinished { provider, .. } => {
                // A rename / default change returns to the account list; sign-ins finish.
                let back_to_accounts = self.composer.connect.as_ref().is_some_and(|f| f.method.is_none() && !f.adding && f.connection.is_some())
                    && self.composer.picker.as_ref().is_some_and(|p| p.activity.as_ref().is_some_and(|a| a.status == "Updating account"));
                if back_to_accounts {
                    if let Some(flow) = self.composer.connect.as_mut() { flow.waiting = true; }
                    self.agent_send(Client::ConnectListConnections { provider_id: provider.clone() });
                } else {
                    self.connect_finished(provider, Ok(()));
                }
                true
            }
            Server::ConnectFailed { provider, error } => { self.connect_finished(provider, Err(error.clone())); true }
            Server::Notice { session_id, title, body, .. } => {
                if self.active_session().as_deref() == Some(session_id.as_str()) { self.notice(title, body.clone()); }
                true
            }
            Server::CommandOutput { title, body, .. } => { self.notice(title, body.clone()); true }
            _ => false,
        }
    }

    // -- skits ---------------------------------------------------------------------------

    fn play_effect(&mut self, effect: Effect) {
        self.composer.effect = Some(Running::new(effect, self.active_session()));
    }

    /// Advance a playing skit: send its follow-up prompt when due, end it when done.
    /// Returns whether it still needs frames.
    pub fn tick_effect(&mut self) -> bool {
        let Some(running) = self.composer.effect.as_mut() else { return false };
        let elapsed = running.elapsed();
        if !running.prompted && elapsed >= effects::prompt_at(running.effect) {
            running.prompted = true;
            let (session, text) = (running.session.clone(), effects::followup_prompt(running.effect).to_owned());
            match session {
                Some(session_id) => self.submit_prompt(&session_id, text),
                None => { let i = self.active; self.create_thread(i, text); }
            }
        }
        let done = self.composer.effect.as_ref().is_some_and(|r| r.elapsed() > effects::total_seconds(r.effect));
        if done { self.composer.effect = None; }
        !done
    }
}

/// Open a sign-in page in the default browser, as GPUiX's `openExternal` does.
pub(crate) fn open_in_browser(url: &str) {
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
        let _ = crate::cli::open_external(url);
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    fn provider(id: &str, provider: &str, model: &str, thinking: Option<&str>) -> Server {
        Server::ProviderState { session_id: id.into(), authoritative: true, provider_id: Some(provider.into()),
            model: Some(model.into()), connection_id: None, agent: Some("build".into()), thinking: thinking.map(str::to_owned), context_limit: Some(200000) }
    }
    #[test]
    fn tab_selection_and_late_provider_replies_remain_session_scoped() {
        let mut app = App::new();
        let a = app.open_tab(TabKind::Agent { session: Some("a".into()) });
        let a = app.tabs[a].id;
        app.apply_agent(provider("a", "openai", "gpt-6.1-sol", Some("high")));
        let b = app.open_tab(TabKind::Agent { session: Some("b".into()) });
        let b = app.tabs[b].id;
        app.apply_agent(provider("b", "openrouter", "anthropic/claude-sonnet-4.5", Some("low")));
        assert_eq!(app.composer.selection.model, "openrouter/anthropic/claude-sonnet-4.5");
        app.select_tab(a);
        assert_eq!(app.composer.selection.model, "openai/gpt-6.1-sol");
        assert_eq!(app.effort, "High");
        app.apply_agent(provider("b", "openrouter", "anthropic/claude-sonnet-4.5", None));
        assert_eq!(app.model, "Openai/Gpt-6.1-Sol");
        assert_eq!(app.effort, "High");
        app.set_thinking("medium".into());
        app.select_tab(b);
        assert_eq!(app.effort, "Default");
        app.select_tab(a);
        assert_eq!(app.effort, "Medium");
        app.new_workspace();
        app.open_tab(TabKind::Agent { session: Some("c".into()) });
        app.apply_agent(provider("c", "openrouter", "anthropic/claude-sonnet-4.5", Some("low")));
        app.select_workspace(0);
        app.sync_field_focus();
        app.sync_panes();
        assert_eq!(app.active_tab().id, a);
        assert_eq!(app.composer.selection.model, "openai/gpt-6.1-sol");
        assert_eq!(app.effort, "Medium");
        app.open_selection(Kind::Thinking);
        assert_eq!(app.composer.picker.as_ref().unwrap().selected_row().unwrap().value, "medium");
    }
    #[test]
    fn unsent_tabs_keep_their_own_prompt_settings() {
        let mut app = App::new();
        let a = app.open_tab(TabKind::Agent { session: None });
        let a = app.tabs[a].id;
        app.set_model("openai/gpt-6.1-sol".into());
        app.set_thinking("high".into());
        let b = app.open_tab(TabKind::Agent { session: None });
        let b = app.tabs[b].id;
        app.set_model("openrouter/anthropic/claude-sonnet-4.5".into());
        app.set_agent("plan".into());
        app.select_tab(a);
        assert_eq!(app.composer.selection.model, "openai/gpt-6.1-sol");
        assert_eq!(app.effort, "High");
        app.select_tab(b);
        assert_eq!(app.agent, "Plan");
        assert_eq!(app.composer.selection.model, "openrouter/anthropic/claude-sonnet-4.5");
    }
}
