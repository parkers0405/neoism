//! The Neoism agent: conversation state fed by the daemon's `Agent` family, and its surfaces
//! (sidebar, transcript, composer). References: `rune/reference/04..13*.png`.

pub mod mcp;

mod activity;
mod composer;
pub(crate) mod family;
mod header;
mod prose;
mod sidebar;
mod table;
pub mod slash;
mod user;
mod timeline;
pub mod tools;

pub use activity::Activity;
pub use composer::overlay;
pub use family::Families;
pub use header::Step as HeaderStep;
pub use sidebar::{catalog_scope, conversations};
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

#[derive(Default)]
pub struct Session {
    pub message_versions: std::collections::HashMap<String, u64>,
    pub scroll: std::rc::Rc<std::cell::Cell<(f32, f32)>>,
    pub wheel_target: Option<f32>,
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
        let changed = self.messages.iter().find(|m| m.id == message.id)
            .is_none_or(|old| old.text != message.text || old.status != message.status || old.kind != message.kind);
        if changed { *self.message_versions.entry(message.id.clone()).or_default() += 1; }
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
        if session.loading_older { return; }
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
            attachments: Vec::new(),
            created_at: 0,
        });
        self.send_agent(AgentClientMessage::SubmitPrompt {
            session_id: session_id.into(),
            message_id,
            text,
            author: Some(self.local_name.clone()),
            attachments: Vec::new(),
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
                } else {
                    merge_head(&mut session.messages, messages);
                    if !session.loaded { session.older_cursor = next_cursor; }
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
}
