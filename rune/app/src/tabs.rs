//! Pane tabs: what each tab shows, opening and closing them, and where a submitted line goes.

use crate::app::App;
use rune::TextEdit;

#[derive(Clone, Debug, PartialEq)]
pub enum TabKind {
    /// Welcome splash with the Home block terminal under it.
    Home,
    /// An agent conversation; `None` until the first prompt creates the thread.
    Agent { session: Option<String> },
    /// A file in the code editor (workspace-relative path).
    Editor { path: String },
}

pub struct Tab {
    pub id: u64,
    pub kind: TabKind,
    pub draft: TextEdit,
    /// Prompt waiting for `ThreadCreated` on a new conversation.
    pub pending_prompt: Option<String>,
}

impl App {
    pub fn open_tab(&mut self, kind: TabKind) -> usize {
        let id = self.next_tab;
        self.next_tab += 1;
        self.tabs.push(Tab { id, kind, draft: TextEdit::new(""), pending_prompt: None });
        self.active = self.tabs.len() - 1;
        self.tabs[self.active].draft.focused = true;
        if matches!(self.tabs[self.active].kind, TabKind::Home) { self.open_terminal(id); }
        self.active
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() { return; }
        let removed = self.tabs.remove(index);
        self.close_terminal(removed.id);
        if self.tabs.is_empty() { self.open_tab(TabKind::Home); }
        self.active = self.active.min(self.tabs.len() - 1);
    }

    pub fn active_tab(&self) -> &Tab { &self.tabs[self.active] }

    pub fn tab_title(&self, tab: &Tab) -> String {
        match &tab.kind {
            TabKind::Home => "Home".into(),
            TabKind::Editor { path } => path.rsplit('/').next().unwrap_or(path).to_owned(),
            TabKind::Agent { session: None } => "Neoism".into(),
            TabKind::Agent { session: Some(id) } => self.threads.iter().find(|t| &t.session_id == id)
                .map(|t| t.title.clone())
                .or_else(|| self.families.member(id).map(|m| m.title.clone()))
                .unwrap_or_else(|| "Neoism".into()),
        }
    }

    pub fn submit(&mut self) {
        let index = self.active;
        let text = self.tabs[index].draft.text.trim().to_owned();
        if text.is_empty() { return; }
        self.tabs[index].draft.clear();
        match self.tabs[index].kind.clone() {
            TabKind::Home => self.run_command(index, text),
            TabKind::Agent { session: Some(session_id) } => self.submit_prompt(&session_id, text),
            TabKind::Agent { session: None } => self.create_thread(index, text),
            TabKind::Editor { .. } => {}
        }
    }

    /// Which wordmark (if any) the active tab shows: the Home splash before the first command,
    /// or a new agent chat. Keys match the views (`wordmark::view` reads the kind from them).
    pub fn visible_wordmark(&self) -> Option<&'static str> {
        let tab = self.active_tab();
        match &tab.kind {
            TabKind::Home => {
                let busy = self.terminals.get(&tab.id).is_some_and(|t| !t.blocks.is_empty() || t.live.is_some() || t.error.is_some());
                (!busy).then_some("splash-wordmark")
            }
            TabKind::Agent { session: None } => Some("agent-wordmark"),
            TabKind::Agent { .. } | TabKind::Editor { .. } => None,
        }
    }

    /// Whether the active tab shows user messages (and so presence orbs).
    pub fn orbs_on_screen(&self) -> bool {
        match &self.active_tab().kind {
            TabKind::Agent { session: Some(id) } => self.sessions.get(id)
                .is_some_and(|s| s.messages.iter().any(|m| m.kind == neoism_protocol::agent::HistoryMessageKind::User)),
            _ => false,
        }
    }
}
