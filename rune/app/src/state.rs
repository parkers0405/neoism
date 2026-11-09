//! Application state and how daemon events change it. Views only read this.

use crate::daemon::{Daemon, Family, Inbound, Inbox};
use crate::theme::{Palette, Type, palette};
use crate::wordmark::Wordmark;
use neoism_protocol::agent::{
    AgentClientMessage, AgentServerMessage, HistoryMessage, HistoryMessageKind, PromptDelivery,
    Role, StreamingState, ThreadSummary,
};
use rune::{ImageCache, TextEdit};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Older pages are sized by bytes, as in the GPUiX fix (`rune/README.md`, data rules).
const OLDER_PAGE_TARGET_BYTES: usize = 160 * 1024;
const HEAD_PAGE: u32 = 80;

#[derive(Clone, Debug, PartialEq)]
pub enum Connection {
    Connecting,
    Connected,
    Disconnected(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TabKind {
    /// Welcome splash with the Home block terminal under it.
    Home,
    /// An agent conversation; `None` until the first prompt creates the thread.
    Agent { session: Option<String> },
}

pub struct Tab {
    pub id: u64,
    pub kind: TabKind,
    pub draft: TextEdit,
    /// Prompt waiting for `ThreadCreated` on a new conversation.
    pub pending_prompt: Option<String>,
}

#[derive(Default)]
pub struct Session {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sidebar {
    Conversations,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Slash,
    Models,
    Agents,
    Palette,
}

/// Window keyboard focus zones (`rune/README.md`, "Keyboard focus and the cursor").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Content,
    Sidebar,
    Buffers(usize),
    Workspaces(usize),
}

pub struct App {
    pub daemon: Option<Daemon>,
    pub inbox: Option<Inbox>,
    pub images: Option<ImageCache>,
    pub connection: Connection,
    pub pal: &'static Palette,
    pub ty: Type,
    pub workspace_name: String,
    pub cwd: String,
    pub branch: Option<String>,
    pub threads: Vec<ThreadSummary>,
    pub sessions: HashMap<String, Session>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub next_tab: u64,
    pub sidebar: Option<Sidebar>,
    pub overlay: Option<Overlay>,
    pub focus: Focus,
    pub agent: String,
    pub model: String,
    pub effort: String,
    pub wordmark: Wordmark,
    pub fps: Option<u32>,
    /// The transcript viewport's `(offset, max)`, mirrored each frame from `on_scroll_offset`.
    pub scroll: std::rc::Rc<std::cell::Cell<(f32, f32)>>,
    /// Where notched wheel input is gliding the transcript to.
    pub wheel_target: Option<f32>,
}

impl App {
    pub fn new() -> App {
        let config = crate::config::load();
        let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
        let home = std::env::var("HOME").unwrap_or_default();
        let workspace_name = std::path::Path::new(&cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or("~".into());
        let mut app = App {
            daemon: None,
            inbox: None,
            images: None,
            connection: Connection::Connecting,
            pal: palette(&config.theme),
            ty: Type { size: config.font_size, weight: config.font_weight },
            workspace_name: if cwd == home { "~".into() } else { workspace_name },
            cwd,
            branch: crate::config::git_branch(),
            threads: Vec::new(),
            sessions: HashMap::new(),
            tabs: Vec::new(),
            active: 0,
            next_tab: 1,
            sidebar: None,
            overlay: None,
            focus: Focus::Content,
            agent: "Build".into(),
            model: title_case(config.model.as_deref().unwrap_or("openai/gpt-6.1-sol")),
            effort: title_case(config.variant.as_deref().unwrap_or("medium")),
            wordmark: Wordmark::default(),
            fps: None,
            scroll: Default::default(),
            wheel_target: None,
        };
        app.open_tab(TabKind::Home);
        app
    }

    pub fn display_cwd(&self) -> String {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() && self.cwd.starts_with(&home) { format!("~{}", &self.cwd[home.len()..]) } else { self.cwd.clone() }
    }

    pub fn open_tab(&mut self, kind: TabKind) -> usize {
        let id = self.next_tab;
        self.next_tab += 1;
        self.tabs.push(Tab { id, kind, draft: TextEdit::new(""), pending_prompt: None });
        self.active = self.tabs.len() - 1;
        self.tabs[self.active].draft.focused = true;
        self.active
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() { return; }
        self.tabs.remove(index);
        if self.tabs.is_empty() { self.open_tab(TabKind::Home); }
        self.active = self.active.min(self.tabs.len() - 1);
    }

    pub fn active_tab(&self) -> &Tab { &self.tabs[self.active] }

    pub fn tab_title(&self, tab: &Tab) -> String {
        match &tab.kind {
            TabKind::Home => "Home".into(),
            TabKind::Agent { session: None } => "Neoism".into(),
            TabKind::Agent { session: Some(id) } => self.threads.iter().find(|t| &t.session_id == id)
                .map(|t| t.title.clone()).unwrap_or_else(|| "Neoism".into()),
        }
    }

    fn send_agent(&self, message: AgentClientMessage) {
        if let Some(daemon) = &self.daemon { daemon.agent(&message); }
    }

    pub fn refresh_threads(&self) {
        self.send_agent(AgentClientMessage::ListThreads { directory: Some(self.cwd.clone()), limit: Some(50), cursor: None });
    }

    /// Open (or focus) a conversation tab and load its newest page.
    pub fn open_conversation(&mut self, session_id: &str) {
        if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Agent { session: Some(session_id.into()) }) {
            self.active = index;
            return;
        }
        self.open_tab(TabKind::Agent { session: Some(session_id.into()) });
        let session = self.sessions.entry(session_id.into()).or_default();
        if !session.loaded {
            session.older_limit = 24;
            self.send_agent(AgentClientMessage::SwitchThread { session_id: session_id.into() });
            self.send_agent(AgentClientMessage::GetHistory { session_id: session_id.into(), cursor: None, limit: Some(HEAD_PAGE) });
            self.send_agent(AgentClientMessage::ResumeStream { session_id: session_id.into() });
        }
    }

    pub fn request_older(&mut self, session_id: &str) {
        let Some(session) = self.sessions.get_mut(session_id) else { return };
        let Some(cursor) = session.older_cursor.clone() else { return };
        if session.loading_older { return; }
        session.loading_older = true;
        let limit = session.older_limit.max(8);
        self.send_agent(AgentClientMessage::GetHistory { session_id: session_id.into(), cursor: Some(cursor), limit: Some(limit) });
    }

    pub fn submit(&mut self) {
        let index = self.active;
        let text = self.tabs[index].draft.text.trim().to_owned();
        if text.is_empty() { return; }
        self.tabs[index].draft.clear();
        match self.tabs[index].kind.clone() {
            TabKind::Home => {
                // The Home composer runs a shell command (block terminal, step 6).
                self.tabs[index].draft.replace_text("");
            }
            TabKind::Agent { session: Some(session_id) } => self.submit_prompt(&session_id, text),
            TabKind::Agent { session: None } => {
                self.tabs[index].pending_prompt = Some(text);
                self.send_agent(AgentClientMessage::CreateThread {
                    title: None,
                    directory: Some(self.cwd.clone()),
                    agent: Some(self.agent.to_lowercase()),
                    model: None,
                    connection_id: None,
                    external_provider: None,
                    thinking: Some(self.effort.to_lowercase()),
                });
            }
        }
    }

    fn submit_prompt(&mut self, session_id: &str, text: String) {
        let message_id = format!("msg_rune_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let session = self.sessions.entry(session_id.into()).or_default();
        session.messages.push(HistoryMessage {
            id: message_id.clone(),
            role: Role::User,
            kind: HistoryMessageKind::User,
            author: None,
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
            attachments: Vec::new(),
            created_at: 0,
        });
        self.send_agent(AgentClientMessage::SubmitPrompt {
            session_id: session_id.into(),
            message_id,
            text,
            author: None,
            attachments: Vec::new(),
            mode: Some(self.agent.to_lowercase()),
            model: None,
            connection_id: None,
            thinking: Some(self.effort.to_lowercase()),
            delivery: PromptDelivery::Steer,
        });
    }

    /// Headless scenes: show a recorded transcript as if the daemon had sent it.
    pub fn load_fixture(&mut self, session_id: &str, title: &str, json: &str) {
        let messages: Vec<HistoryMessage> = serde_json::from_str(json).unwrap_or_default();
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

    /// Drain the daemon inbox on the UI thread. Returns whether anything visible changed.
    pub fn drain(&mut self) -> bool {
        let Some(inbox) = &self.inbox else { return false };
        let events: Vec<Inbound> = inbox.lock().unwrap().drain(..).collect();
        let changed = !events.is_empty();
        for event in events { self.apply(event); }
        changed
    }

    fn apply(&mut self, event: Inbound) {
        match event {
            Inbound::Connecting => self.connection = Connection::Connecting,
            Inbound::Connected => {
                self.connection = Connection::Connected;
                self.refresh_threads();
                if let Some(daemon) = &self.daemon {
                    daemon.send(Family::Agent, json!({"GetConfigDefaults": {"directory": self.cwd}}));
                }
            }
            Inbound::Disconnected(reason) => self.connection = Connection::Disconnected(reason),
            Inbound::Agent(message) => self.apply_agent(*message),
            Inbound::Reply { .. } => {}
        }
    }

    fn apply_agent(&mut self, message: AgentServerMessage) {
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
                let session = self.sessions.entry(session_id).or_default();
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
                } else {
                    merge_head(&mut session.messages, messages);
                    if !session.loaded { session.older_cursor = next_cursor; }
                    session.loaded = true;
                }
            }
            AgentServerMessage::MessageUpdated { session_id, message, .. } => {
                let session = self.sessions.entry(session_id).or_default();
                upsert(&mut session.messages, message);
            }
            AgentServerMessage::ContentDelta { session_id, message_id, text, kind, .. } => {
                let session = self.sessions.entry(session_id).or_default();
                if let Some(message) = session.messages.iter_mut().find(|m| m.id == message_id) {
                    message.text.push_str(&text);
                } else {
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
                let session = self.sessions.entry(session_id).or_default();
                session.streaming = !matches!(state, StreamingState::Idle);
                if session.streaming && session.live_from.is_none() {
                    session.live_from = Some(session.messages.iter().rposition(|m| m.kind == HistoryMessageKind::User).unwrap_or(session.messages.len()));
                }
                session.status = label;
            }
            AgentServerMessage::SessionIdle { session_id, .. } => {
                if let Some(session) = self.sessions.get_mut(&session_id) { session.streaming = false; session.status = None; }
            }
            AgentServerMessage::ProviderState { model: Some(model), provider_id, .. } => {
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

fn title_case(model: &str) -> String {
    model.split('/').map(|part| part.split('-').map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }).collect::<Vec<_>>().join("-")).collect::<Vec<_>>().join("/")
}
