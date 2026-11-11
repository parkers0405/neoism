//! Pane tabs: what each tab shows, opening and closing them, and where a submitted line goes.

use crate::app::App;
use rune::TextEdit;

#[derive(Clone, Debug, PartialEq)]
pub enum TabKind {
    /// Welcome splash with the Home block terminal under it.
    Home,
    /// A plain terminal (`Alt+T`): the block terminal without the splash.
    Terminal,
    /// An agent conversation; `None` until the first prompt creates the thread.
    Agent { session: Option<String> },
    /// A file in the code editor (workspace-relative path).
    Editor { path: String },
    Database { path: String },
}

/// Exact close aliases precede fuzzy palette matches on every surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseCommand { pub workspace: bool, pub discard: bool }
impl CloseCommand {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().strip_prefix(':').unwrap_or(text.trim());
        let discard = text.ends_with('!');
        let name = text.strip_suffix('!').unwrap_or(text);
        let workspace = match name {
            "q" | "quit" | "quite" | "close" => false,
            "qa" | "qall" | "quitall" => true,
            _ => return None,
        };
        Some(Self { workspace, discard })
    }
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
        let terminal = matches!(kind, TabKind::Home | TabKind::Terminal);
        // The new tab's composer takes the keyboard from whichever field had it.
        self.tabs.iter_mut().for_each(|t| t.draft.focused = false);
        self.tabs.push(Tab { id, kind, draft: TextEdit::new(""), pending_prompt: None });
        self.active = self.tabs.len() - 1;
        self.tabs[self.active].draft.focused = true;
        self.layout.open(id);
        if terminal { self.open_terminal(id); }
        self.active
    }

    /// Close a tab; its pane closes with its last tab, and the last tab anywhere leaves a Home.
    pub fn close_tab(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index) else { return };
        let (dirty, can_save) = match &tab.kind {
            TabKind::Editor { path } => (self.buffers.get(path).is_some_and(|b| b.dirty()), true),
            TabKind::Database { path } => {
                if let Some(db) = self.databases.get_mut(path) {
                    if db.pending.is_some() { db.error = Some("Wait for the current database request before closing".into()); return; }
                    (db.cell.is_some() || !db.changes().is_empty(), false)
                } else { (false, false) }
            }
            _ => (false, false),
        };
        if dirty {
            self.modal = Some(crate::ui::modal::Modal::unsaved(tab.id, &self.tab_title(tab), can_save));
            return;
        }
        self.close_tab_now(index);
    }

    pub(crate) fn close_tab_now(&mut self, index: usize) {
        if index >= self.tabs.len() { return; }
        let removed = self.tabs.remove(index);
        self.close_terminal(removed.id);
        self.forget_table_state(removed.id);
        if let TabKind::Agent { session: Some(id) } = &removed.kind { self.release_closed_agent(id); }
        if let TabKind::Editor { path } = &removed.kind {
            if !self.tabs.iter().any(|t| t.kind == removed.kind) { self.lsp_close(path); }
        }
        if let TabKind::Database { path } = &removed.kind {
            if !self.tabs.iter().any(|t| t.kind == removed.kind) { self.databases.remove(path); }
        }
        if !self.layout.remove(removed.id) {
            self.open_tab(TabKind::Home);
            return;
        }
        self.show_focused_pane();
    }

    pub(crate) fn forget_table_state(&mut self, tab_id: u64) {
        let prefix = format!("table-x:{tab_id}:");
        self.table_offsets.borrow_mut().retain(|key, _| !key.starts_with(&prefix));
        if self.table_hover.as_ref().is_some_and(|key| key.starts_with(&prefix)) { self.table_hover = None; }
    }

    /// The tab being drawn: the focused pane's active tab, or an unfocused pane's while its
    /// content is built (`render_tab`).
    pub fn active_tab(&self) -> &Tab { &self.tabs[self.render_tab.get().unwrap_or(self.active)] }

    /// The agent TUI a Home/Terminal tab is running, if any.
    pub fn tab_program(&self, tab: u64) -> Option<&'static str> {
        self.terminals.get(&tab).and_then(|t| t.program)
    }

    pub fn tab_title(&self, tab: &Tab) -> String {
        match &tab.kind {
            TabKind::Home | TabKind::Terminal if let Some(program) = self.tab_program(tab.id) => crate::home::program_title(program).into(),
            TabKind::Home => "Home".into(),
            TabKind::Terminal => "Terminal".into(),
            TabKind::Database { path } if path.is_empty() => "Database".into(),
            TabKind::Database { path } | TabKind::Editor { path } => path.rsplit('/').next().unwrap_or(path).to_owned(),
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
        if matches!(self.tabs[index].kind, TabKind::Agent { .. }) {
            if let Some(command) = CloseCommand::parse(&text) {
                self.tabs[index].draft.clear();
                self.run_close_command(command);
                return;
            }
        }
        self.tabs[index].draft.clear();
        match self.tabs[index].kind.clone() {
            TabKind::Home | TabKind::Terminal => self.run_command(index, text),
            TabKind::Agent { session: Some(session_id) } => self.submit_prompt(&session_id, text),
            TabKind::Agent { session: None } => self.create_thread(index, text),
            TabKind::Editor { .. } | TabKind::Database { .. } => {}
        }
    }

    pub fn run_close_command(&mut self, command: CloseCommand) {
        if command.workspace {
            self.close_workspace_with_discard(self.workspace_active, command.discard);
        } else if command.discard {
            self.close_tab_now(self.active);
        } else {
            self.close_tab(self.active);
        }
    }

    /// Animate every visible pane's wordmark using the exact key its view draws.
    pub fn visible_wordmarks(&self) -> Vec<String> {
        self.layout.panes().iter().filter_map(|pane| {
            let id = pane.active_tab()?;
            let tab = self.tabs.iter().find(|tab| tab.id == id)?;
            match &tab.kind {
                TabKind::Home if !self.splash.hidden.contains(&id) && self.terminals.get(&id).is_none_or(|t| !t.raw()) => Some(crate::wordmark::host_key("splash", id)),
                TabKind::Agent { session: None } => Some(crate::wordmark::host_key("agent", id)),
                _ => None,
            }
        }).collect()
    }

    /// Whether the active tab shows user messages (and so presence orbs).
    pub fn orbs_on_screen(&self) -> bool {
        // In company, the rail (and any file a peer is in) shows presence orbs.
        if self.collab.anyone() { return true; }
        match &self.active_tab().kind {
            // A user row laid out within (or just above) the visible transcript: its orb sits
            // at the bubble's top, a tall bubble can hang down from above the view.
            TabKind::Agent { session: Some(id) } => self.sessions.get(id).is_some_and(|s| {
                let (offset, view) = (s.scroll.get().0, s.viewport_h.max(1.0));
                s.question_tops.values().any(|top| *top >= offset - view && *top <= offset + view)
            }),
            _ => false,
        }
    }
}
