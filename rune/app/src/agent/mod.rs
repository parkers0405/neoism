//! The Neoism agent: conversation state fed by the daemon's `Agent` family, and its surfaces
//! (sidebar, transcript, composer). References: `rune/reference/04..13*.png`.

pub mod mcp;

mod activity;
pub mod attach;
mod composer;
pub(crate) mod family;
mod header;
mod projects;
mod prose;
mod sidebar;
mod table;
mod ticks;
pub mod slash;
mod user;
pub mod timeline;
pub mod tools;

pub use activity::Activity;
pub use composer::overlay;
pub use family::Families;
pub use header::Step as HeaderStep;
pub use projects::{ProjectMenu, view as project_menu};
pub use sidebar::{ConvHeader, catalog_scope, conversations};
pub use timeline::pane;

use crate::app::App;
use crate::daemon::Family;
use crate::tabs::TabKind;
use neoism_protocol::agent::{
    AgentClientMessage, AgentServerMessage, HistoryMessage, HistoryMessageKind, PromptDelivery,
    Role, StreamingState, ThreadSummary,
};
use serde_json::{Value, json};

/// Older pages are sized by bytes, as in the GPUiX fix (`rune/README.md`, data rules).
const OLDER_PAGE_TARGET_BYTES: usize = 160 * 1024;
const HEAD_PAGE: u32 = 80;

/// How often a streaming reply's new text re-runs the views.
///
/// A provider sends text as fast as it writes it, a delta every few milliseconds from a fast
/// model, and each one applied its own commit: a commit a frame for as long as the reply ran.
/// The text is applied as it arrives; the views show it at most `StreamPace::RATE` times a
/// second (and at once with anything else that re-runs them).
#[derive(Default)]
pub struct StreamPace {
    /// Text arrived that no view run has shown yet.
    pub pending: bool,
    shown: Option<std::time::Instant>,
}

impl StreamPace {
    /// Thirty view runs a second: a new batch of words every 33 ms.
    const INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

    /// Whether pending text is due a view run now, else how long until it is.
    pub fn due(&self, now: std::time::Instant) -> (bool, Option<std::time::Duration>) {
        if !self.pending { return (false, None); }
        match self.shown.map(|shown| now.saturating_duration_since(shown)) {
            Some(since) if since < Self::INTERVAL => (false, Some(Self::INTERVAL - since)),
            _ => (true, None),
        }
    }

    /// The views ran: whatever text was pending is on screen.
    pub fn ran(&mut self, now: std::time::Instant) {
        if std::mem::take(&mut self.pending) { self.shown = Some(now); }
    }
}

#[derive(Default)]
pub struct Session {
    /// Each message's revision: bumped whenever anything about it changes. Transcript rows
    /// are cached by it (`rune::memo`).
    pub message_versions: std::collections::HashMap<String, u64>,
    /// Bumped whenever the transcript's row list may have changed (a message arrived or
    /// changed; not a reply's text growing). With the message count it keys `rows`.
    pub structure: u64,
    /// The transcript's row list as last computed (`Session::row_items`).
    pub rows: std::cell::RefCell<Option<(timeline::RowsKey, std::rc::Rc<Vec<timeline::RowItem>>)>>,
    pub scroll: std::rc::Rc<std::cell::Cell<(f32, f32)>>,
    /// The wheel or trackpad glide's target and when it was set (`follow::wheel_base`).
    pub wheel_target: Option<(f32, std::time::Instant)>,
    /// The user scrolled up the transcript just now, and when the last older page arrived:
    /// older history loads while someone scrolls toward it, never on its own (GPUiX's cascade
    /// fix: a short page leaves the view at the top and would pull the whole history in).
    pub scrolled_up_at: Option<std::time::Instant>,
    pub older_at: Option<std::time::Instant>,
    /// A page just landed: the next waits for a new gesture (a pause in upward scrolling or a
    /// turn downward), so one fast flick or its coast loads one page, not a cascade.
    pub older_spent: bool,
    /// Questions whose rows are in view: only their `question_tops` are current.
    pub visible_questions: std::collections::HashSet<String>,
    /// Each question's row offset in the transcript, from its last layout (the question rail).
    pub question_tops: std::collections::HashMap<String, f32>,
    /// The question rail's lit tick, and the transcript viewport's height.
    pub rail_active: Option<usize>,
    pub viewport_h: f32,
    pub width: f32,
    pub messages: Vec<HistoryMessage>,
    pub older_cursor: Option<String>,
    pub loaded: bool,
    pub loading_older: bool,
    pub older_limit: u32,
    pub streaming: bool,
    pub status: Option<String>,
    /// First message of a turn that streamed while open here; its tool and thinking rows stay
    /// visible. Reloaded history shows finished turns as prompt, answer and footer only.
    pub live_from: Option<usize>,
    /// The session's context window from `ProviderState`, when the daemon knows it.
    pub context_limit: Option<u64>,
}

impl Session {
    fn note_message(&mut self, message: &HistoryMessage) {
        // Any difference counts: a tool row draws its title, detail, todos and children too.
        let changed = self.messages.iter().find(|m| m.id == message.id).is_none_or(|old| old != message);
        if changed {
            *self.message_versions.entry(message.id.clone()).or_default() += 1;
            self.structure += 1;
        }
    }
}

impl App {
    fn send_agent(&self, message: AgentClientMessage) {
        if let Some(daemon) = &self.daemon { daemon.agent(&message); }
    }

    /// The conversation catalog for this root: one project's chats, or every project's at
    /// the `~` / `/` dashboard (`workspace_catalog_scope`).
    pub fn refresh_threads(&self) {
        let directory = sidebar::catalog_scope(&self.cwd);
        self.send_agent(AgentClientMessage::ListThreads { directory, limit: Some(50), cursor: None });
    }

    pub(crate) fn agent_connected(&mut self) {
        self.refresh_threads();
        if let Some(daemon) = &self.daemon {
            daemon.send(Family::Agent, json!({"GetConfigDefaults": {"directory": self.cwd}}));
        }
        self.slash_connected();
    }

    pub fn open_conversation(&mut self, session_id: &str) {
        if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Agent { session: Some(session_id.into()) }) {
            self.select_tab(self.tabs[index].id);
            return;
        }
        self.open_tab(TabKind::Agent { session: Some(session_id.into()) });
        self.load_session(session_id);
    }

    /// A restored conversation tab (`session.rs`) loads like an opened one.
    pub(crate) fn load_session_history(&mut self, session_id: &str) { self.load_session(session_id); }

    /// Fetch a session's newest page and follow its stream, once.
    fn load_session(&mut self, session_id: &str) {
        let session = self.sessions.entry(session_id.into()).or_default();
        if session.loaded { return; }
        session.older_limit = 24;
        self.send_agent(AgentClientMessage::SwitchThread { session_id: session_id.into() });
        self.send_agent(AgentClientMessage::GetHistory { session_id: session_id.into(), cursor: None, limit: Some(HEAD_PAGE) });
        self.send_agent(AgentClientMessage::ResumeStream { session_id: session_id.into() });
    }

    /// Show `session_id` in the active agent tab: a sub-agent opened from a task card or the
    /// family header replaces the view in place, as GPUiX's `openThread(thread, false, root)`.
    pub fn view_session(&mut self, session_id: &str) {
        let index = self.active;
        if !matches!(self.tabs[index].kind, TabKind::Agent { .. }) { return; }
        self.tabs[index].kind = TabKind::Agent { session: Some(session_id.to_owned()) };
        if let Some(session) = self.sessions.get_mut(session_id) { session.wheel_target = None; }
        self.load_session(session_id);
    }

    /// A turn starts or ends. The live turn keeps its trace rows (`live_from`); when it ends the
    /// head page is fetched again for the server-composed footer and settled rows.
    fn set_streaming(&mut self, session_id: &str, streaming: bool) {
        let Some(session) = self.sessions.get_mut(session_id) else { return };
        if session.streaming == streaming { return; }
        session.streaming = streaming;
        if streaming {
            if session.live_from.is_none() {
                session.live_from = Some(session.messages.iter().rposition(|m| m.kind == HistoryMessageKind::User).unwrap_or(session.messages.len()));
            }
        } else {
            session.status = None;
            self.send_agent(AgentClientMessage::GetHistory { session_id: session_id.into(), cursor: None, limit: Some(HEAD_PAGE) });
        }
    }

    pub fn request_older(&mut self, session_id: &str) {
        let Some(session) = self.sessions.get_mut(session_id) else { return };
        let Some(cursor) = session.older_cursor.clone() else { return };
        if session.loading_older || session.older_spent { return; }
        // Only for someone scrolling up within a viewport of the top, and once per page: after
        // a page lands, the next needs a fresh upward wheel event (GPUiX's cascade fix).
        let now = std::time::Instant::now();
        let Some(scrolled) = session.scrolled_up_at.filter(|at| now.duration_since(*at) < std::time::Duration::from_millis(600)) else { return };
        if session.older_at.is_some_and(|landed| scrolled <= landed) { return; }
        if session.scroll.get().0 > session.viewport_h.max(1.0) { return; }
        session.loading_older = true;
        let limit = session.older_limit.max(8);
        self.send_agent(AgentClientMessage::GetHistory { session_id: session_id.into(), cursor: Some(cursor), limit: Some(limit) });
    }

    /// The first prompt of a new conversation waits for `ThreadCreated`, then submits.
    pub fn create_thread(&mut self, index: usize, text: String) {
        self.tabs[index].pending_prompt = Some(text);
        self.send_agent(AgentClientMessage::CreateThread {
            title: None,
            directory: Some(self.cwd.clone()),
            agent: Some(self.composer.selection.agent.clone()).filter(|a| !a.is_empty()),
            model: Some(self.composer.selection.model.clone()).filter(|m| !m.is_empty()),
            connection_id: None,
            external_provider: None,
            thinking: Some(self.composer.selection.thinking.clone()).filter(|t| !t.is_empty()),
        });
    }

    pub(crate) fn submit_prompt(&mut self, session_id: &str, text: String) {
        let (attachments, echo) = self.take_attachments(session_id);
        let message_id = format!("msg_rune_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let session = self.sessions.entry(session_id.into()).or_default();
        session.messages.push(HistoryMessage {
            id: message_id.clone(),
            role: Role::User,
            kind: HistoryMessageKind::User,
            author: Some(self.local_name.clone()),
            title: String::new(),
            text: text.clone(),
            status: String::new(),
            tool: String::new(),
            agent_kind: None,
            tool_batch_id: None,
            mcp: None,
            composition_children: Vec::new(),
            output_kind: None,
            lang: String::new(),
            line_offset: None,
            detail: String::new(),
            todos: Vec::new(),
            usage: None,
            attachments: echo,
            created_at: 0,
        });
        self.send_agent(AgentClientMessage::SubmitPrompt {
            session_id: session_id.into(),
            message_id,
            text,
            author: Some(self.local_name.clone()),
            attachments,
            mode: Some(self.composer.selection.agent.clone()).filter(|a| !a.is_empty()),
            model: Some(self.composer.selection.model.clone()).filter(|m| !m.is_empty()),
            connection_id: None,
            thinking: Some(self.composer.selection.thinking.clone()).filter(|t| !t.is_empty()),
            delivery: PromptDelivery::Steer,
        });
    }

    pub fn load_fixture(&mut self, session_id: &str, title: &str, json: &str) {
        let messages: Vec<HistoryMessage> = serde_json::from_str(json).unwrap_or_default();
        for message in &messages { self.mcp_icons.prepare_message(message); }
        let session = self.sessions.entry(session_id.into()).or_default();
        session.messages = messages;
        session.loaded = true;
        self.threads.push(ThreadSummary {
            session_id: session_id.into(), title: title.into(), directory: Some(self.cwd.clone()), model: None, agent: None,
            external_provider: None, updated_at: 0, message_count: 0, catalog_activity: None, busy: false, pinned: false,
            ..serde_json::from_value(json!({"session_id": session_id, "title": title})).unwrap()
        });
        self.open_tab(TabKind::Agent { session: Some(session_id.into()) });
    }

    pub fn stop(&self, session_id: &str) {
        self.send_agent(AgentClientMessage::CancelInflight { session_id: session_id.into() });
    }

    pub(crate) fn apply_agent(&mut self, message: AgentServerMessage) {
        match &message {
            AgentServerMessage::HistoryChunk { messages, .. } => for m in messages { self.mcp_icons.prepare_message(m); },
            AgentServerMessage::MessageUpdated { message, .. } => self.mcp_icons.prepare_message(message),
            _ => {}
        }
        if self.families.note(&message) { return; }
        if self.apply_slash_event(&message) { return; }
        match message {
            AgentServerMessage::ThreadList { threads, requested_cursor, .. } => {
                if requested_cursor.is_none() { self.threads = threads; } else { self.threads.extend(threads); }
            }
            AgentServerMessage::ThreadCreated { session_id, .. } => {
                if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Agent { session: None } && t.pending_prompt.is_some()) {
                    self.tabs[index].kind = TabKind::Agent { session: Some(session_id.clone()) };
                    let prompt = self.tabs[index].pending_prompt.take().unwrap_or_default();
                    let session = self.sessions.entry(session_id.clone()).or_default();
                    session.loaded = true;
                    // Follow the new conversation's stream before the first prompt goes out, so
                    // its streaming state, deltas and tool rows arrive live.
                    self.send_agent(AgentClientMessage::SwitchThread { session_id: session_id.clone() });
                    self.send_agent(AgentClientMessage::ResumeStream { session_id: session_id.clone() });
                    self.submit_prompt(&session_id, prompt);
                }
                self.refresh_threads();
            }
            AgentServerMessage::ThreadUpdated { session_id, title, pinned } => {
                if let Some(thread) = self.threads.iter_mut().find(|t| t.session_id == session_id) {
                    if let Some(title) = title { thread.title = title; }
                    if let Some(pinned) = pinned { thread.pinned = pinned; }
                }
            }
            AgentServerMessage::HistoryChunk { session_id, messages, next_cursor } => {
                let scroller_key = format!("conversation:{session_id}");
                // Task cards name their child sessions; seed the family so it can be walked.
                if !self.families.is_child(&session_id) { self.families.seed_from_history(&session_id, &messages); }
                let session = self.sessions.entry(session_id).or_default();
                for message in &messages { session.note_message(message); }
                if session.loading_older {
                    // Size the next page from this one (bytes per server message).
                    let bytes: usize = messages.iter().map(message_bytes).sum();
                    if bytes > 0 {
                        let next = (session.older_limit as usize * OLDER_PAGE_TARGET_BYTES / bytes).clamp(8, 64);
                        session.older_limit = next as u32;
                    }
                    session.loading_older = false;
                    let known: std::collections::HashSet<String> = session.messages.iter().map(|m| m.id.clone()).collect();
                    let mut older: Vec<HistoryMessage> = messages.into_iter().filter(|m| !known.contains(&m.id)).collect();
                    older.append(&mut session.messages);
                    session.messages = older;
                    session.older_cursor = next_cursor;
                    session.older_at = Some(std::time::Instant::now());
                    session.older_spent = true;
                    // The flick that asked for it stops here instead of coasting up into it.
                    self.scrollers.stop(&scroller_key);
                    // A glide's target predates the page above: the next delta starts from
                    // where the anchored view now is.
                    session.wheel_target = None;
                    // Row offsets measured before the page are all off by its height.
                    session.question_tops.clear();
                } else {
                    merge_head(&mut session.messages, messages);
                    if !session.loaded { session.older_cursor = next_cursor; self.settle_pending = true; }
                    session.loaded = true;
                }
            }
            AgentServerMessage::MessageUpdated { session_id, message, .. } => {
                let session = self.sessions.entry(session_id).or_default();
                session.note_message(&message);
                upsert(&mut session.messages, message);
            }
            AgentServerMessage::ContentDelta { session_id, message_id, text, kind, .. } => {
                let session = self.sessions.entry(session_id).or_default();
                *session.message_versions.entry(message_id.clone()).or_default() += 1;
                if let Some(message) = session.messages.iter_mut().find(|m| m.id == message_id) {
                    // A reply or a thought growing keeps the row list as it is; anything else
                    // (tool output, or text short enough to still decide its row) may not.
                    let prose = matches!(message.kind, HistoryMessageKind::Assistant | HistoryMessageKind::Reasoning) && message.text.len() >= 32;
                    if !prose { session.structure += 1; }
                    message.text.push_str(&text);
                } else {
                    session.structure += 1;
                    let kind = match serde_json::to_value(&kind).ok().as_ref().and_then(Value::as_str) {
                        Some("Thinking") | Some("Reasoning") => HistoryMessageKind::Reasoning,
                        _ => HistoryMessageKind::Assistant,
                    };
                    session.messages.push(HistoryMessage {
                        id: message_id, role: Role::Assistant, kind, author: None, title: String::new(), text,
                        status: String::new(), tool: String::new(), agent_kind: None, tool_batch_id: None, mcp: None,
                        composition_children: Vec::new(), output_kind: None, lang: String::new(), line_offset: None,
                        detail: String::new(), todos: Vec::new(), usage: None, attachments: Vec::new(), created_at: 0,
                    });
                }
            }
            AgentServerMessage::StreamingState { session_id, state, label } => {
                self.set_streaming(&session_id, !matches!(state, StreamingState::Idle));
                if let Some(session) = self.sessions.get_mut(&session_id) { session.status = label; }
            }
            // The daemon reports a conversation's turn as catalog activity (`running` / `idle`).
            AgentServerMessage::CatalogActivity { session_id, activity } => {
                if self.sessions.contains_key(&session_id) { self.set_streaming(&session_id, activity == "running"); }
            }
            AgentServerMessage::SessionIdle { session_id, .. } => self.set_streaming(&session_id, false),
            AgentServerMessage::ProviderState { session_id, model: Some(model), provider_id, context_limit, .. } => {
                if let Some(limit) = context_limit.filter(|l| *l > 0) { self.sessions.entry(session_id).or_default().context_limit = Some(limit); }
                let provider = provider_id.unwrap_or_default();
                self.model = title_case(&if provider.is_empty() || model.contains('/') { model } else { format!("{provider}/{model}") });
            }
            _ => {}
        }
    }
}

fn message_bytes(message: &HistoryMessage) -> usize {
    message.text.len() + message.detail.len() + message.title.len()
        + message.composition_children.iter().map(message_bytes).sum::<usize>()
}

fn upsert(messages: &mut Vec<HistoryMessage>, message: HistoryMessage) {
    if let Some(existing) = messages.iter_mut().find(|m| m.id == message.id) {
        *existing = message;
    } else {
        messages.push(message);
    }
}

/// Head refresh: rows the page knows replace their old copies; rows it doesn't know stay where
/// they were; new rows land after the last known row (the GPUiX `mergeHistoryHead` contract).
fn merge_head(messages: &mut Vec<HistoryMessage>, page: Vec<HistoryMessage>) {
    if messages.is_empty() {
        *messages = page;
        return;
    }
    for message in page { upsert(messages, message); }
}

pub(crate) fn title_case(model: &str) -> String {
    model.split('/').map(|part| part.split('-').map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }).collect::<Vec<_>>().join("-")).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn unchanged_history_keeps_other_message_versions() {
        let message: HistoryMessage = serde_json::from_value(json!({"id":"a", "role":"Assistant", "kind":"Assistant", "text":"first"})).unwrap();
        let mut session = Session::default();
        session.note_message(&message);
        session.messages.push(message.clone());
        let version = session.message_versions["a"];
        session.note_message(&message);
        assert_eq!(session.message_versions["a"], version);
        let mut changed = message.clone();
        changed.status = "Build · model".into();
        session.note_message(&changed);
        assert_eq!(session.message_versions["a"], version + 1);
        let mut other = message; other.id = "b".into();
        session.note_message(&other);
        assert_eq!(session.message_versions["a"], version + 1);
    }

    fn tool(id: &str, status: &str, detail: &str) -> HistoryMessage {
        serde_json::from_value(json!({"id": id, "role": "Assistant", "kind": "Tool", "tool": "bash", "title": "Bash", "status": status, "detail": detail})).unwrap()
    }

    #[test]
    fn any_change_to_a_message_bumps_its_revision() {
        let mut app = App::new();
        let update = |app: &mut App, message: HistoryMessage| app.apply_agent(AgentServerMessage::MessageUpdated { session_id: "s".into(), message, parent_message_id: None });
        update(&mut app, tool("t", "running", ""));
        let first = app.sessions["s"].message_versions["t"];
        // A tool row draws its detail too: a change there alone is a new revision.
        update(&mut app, tool("t", "running", "line one"));
        assert_eq!(app.sessions["s"].message_versions["t"], first + 1);
        update(&mut app, tool("t", "running", "line one"));
        assert_eq!(app.sessions["s"].message_versions["t"], first + 1);
    }

    #[test]
    fn a_growing_reply_keeps_the_row_list_and_a_new_row_recomputes_it() {
        let mut app = App::new();
        let delta = |app: &mut App, text: &str| app.apply_agent(AgentServerMessage::ContentDelta {
            session_id: "s".into(), message_id: "a".into(), parent_message_id: None, kind: neoism_protocol::agent::ContentKind::Text, text: text.into(),
        });
        delta(&mut app, "The scheduler presents on the display clock. ");
        let key = |app: &App| { let s = &app.sessions["s"]; (s.structure, s.messages.len(), usize::MAX, false) };
        let rows = app.sessions["s"].row_items(key(&app));
        assert_eq!(*rows, vec![timeline::RowItem::Message(0)]);
        // More text for the same reply: same key, the very same list.
        delta(&mut app, "It never busy-waits.");
        assert!(std::rc::Rc::ptr_eq(&rows, &app.sessions["s"].row_items(key(&app))));
        // A tool call arrives: a new key and a new list.
        app.apply_agent(AgentServerMessage::MessageUpdated { session_id: "s".into(), message: tool("t", "running", ""), parent_message_id: None });
        let next = app.sessions["s"].row_items((key(&app).0, 2, 0, false));
        assert_eq!(*next, vec![timeline::RowItem::Message(0), timeline::RowItem::Message(1)]);
        // Its status changing may fold or unfold rows: the key moves again.
        let structure = app.sessions["s"].structure;
        app.apply_agent(AgentServerMessage::MessageUpdated { session_id: "s".into(), message: tool("t", "completed", ""), parent_message_id: None });
        assert!(app.sessions["s"].structure > structure);
    }

    #[test]
    fn streamed_text_re_runs_the_views_at_most_thirty_times_a_second() {
        let mut pace = StreamPace::default();
        let start = std::time::Instant::now();
        assert_eq!(pace.due(start), (false, None), "nothing arrived");
        pace.pending = true;
        assert_eq!(pace.due(start), (true, None), "the first text shows at once");
        pace.ran(start);
        // More text right behind it waits out the interval, then shows.
        pace.pending = true;
        let soon = start + std::time::Duration::from_millis(10);
        assert_eq!(pace.due(soon), (false, Some(std::time::Duration::from_millis(23))));
        assert_eq!(pace.due(start + StreamPace::INTERVAL), (true, None));
        // Anything else that runs the views shows the text with it.
        pace.ran(soon);
        assert_eq!(pace.due(soon), (false, None));
    }

    #[test]
    fn deltas_wait_for_the_stream_pace_and_other_events_do_not() {
        let mut app = App::new();
        let inbox: crate::daemon::Inbox = Default::default();
        app.inbox = Some(inbox.clone());
        let delta = || crate::daemon::Inbound::Agent(Box::new(AgentServerMessage::ContentDelta {
            session_id: "s".into(), message_id: "a".into(), parent_message_id: None, kind: neoism_protocol::agent::ContentKind::Text, text: "word ".into(),
        }));
        inbox.lock().unwrap().push_back(delta());
        assert!(!app.drain_current(), "text alone does not re-run the views by itself");
        assert!(app.stream_pace.pending);
        assert_eq!(app.sessions["s"].messages[0].text, "word ", "but it is applied at once");
        inbox.lock().unwrap().extend([delta(), crate::daemon::Inbound::Agent(Box::new(AgentServerMessage::SessionIdle { session_id: "s".into() }))]);
        assert!(app.drain_current(), "any other event re-runs them now");
    }
}
