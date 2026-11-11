//! The workspace root: where the finder, tree, git watch and new conversations look. An explicit
//! directory change goes through the main terminal first (`ChangeTerminalDirectory`), and the
//! root follows the directory the host confirms (`model.changeWorkspaceDirectory`).
//!
//! One terminal per workspace owns the working directory (`mainTerminalSurface`): the one the
//! user starred, else the first Home or Terminal tab with a shell. While it is the active tab,
//! a `cd` inside it moves this window's view of the workspace (tree, notes, conversations, git,
//! paths) with it (`followMainTerminal`); the host's declared root is left alone.

use crate::app::App;
use crate::daemon::Family;
use crate::tabs::TabKind;
use neoism_protocol::pty::ClientMessage as PtyClient;

/// Keep an SSH tunnel alive until its close request has reached the server.
pub struct ClosingConnection {
    done: std::sync::mpsc::Receiver<()>,
    _link: crate::servers::flow::Link,
    deadline: std::time::Instant,
}

impl App {
    /// Terminal tabs that can own the directory: Home and Terminal tabs with a live shell.
    fn directory_terminals(&self) -> impl Iterator<Item = u64> + '_ {
        self.tabs.iter()
            .filter(|t| matches!(t.kind, TabKind::Home | TabKind::Terminal))
            .filter(|t| self.terminals.get(&t.id).is_some_and(|term| term.shell.session.is_some()))
            .map(|t| t.id)
    }

    /// The terminal that owns the working directory (`mainTerminalSurface`).
    pub fn main_terminal_tab(&self) -> Option<u64> {
        let mut eligible = self.directory_terminals();
        let first = eligible.next();
        let remembered = self.main_terminal.filter(|id| Some(*id) == first || self.directory_terminals().any(|t| t == *id));
        remembered.or(first)
    }

    /// The star: make this terminal own the working directory (`setMainTerminal`).
    pub fn set_main_terminal(&mut self, tab_id: u64) {
        if self.pending_cd.is_some() || !self.directory_terminals().any(|t| t == tab_id) { return; }
        self.main_terminal = Some(tab_id);
        self.follow_main_terminal();
    }

    /// While the owning terminal is the active tab, its shell's directory is the workspace
    /// view's root. Returns whether the root moved.
    pub fn follow_main_terminal(&mut self) -> bool {
        let active = self.active_tab().id;
        if self.pending_cd.is_some() || self.main_terminal_tab() != Some(active) { return false; }
        let Some(cwd) = self.terminals.get(&active).map(|t| t.shell.cwd.trim_end_matches('/').to_owned()) else { return false };
        if !crate::startup::absolute(&cwd) || cwd == self.cwd || cwd.chars().any(char::is_control) { return false; }
        self.view_workspace_root(&cwd);
        true
    }

    /// `cd` the workspace to `path` (absolute).
    pub fn change_directory(&mut self, path: &str) {
        let session = self.main_terminal_tab()
            .and_then(|id| self.terminals.get(&id))
            .and_then(|term| term.shell.session.clone());
        let (Some(session_id), Some(daemon)) = (session, &self.daemon) else {
            self.set_workspace_root(path);
            return;
        };
        let request_id = format!(
            "rune-cd-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let message = PtyClient::ChangeTerminalDirectory {
            session_id,
            directory: path.to_owned(),
            request_id: request_id.clone(),
        };
        if let Ok(value) = serde_json::to_value(&message) {
            daemon.send(Family::Pty, value);
        }
        self.pending_cd = Some(request_id);
    }

    /// The shell confirmed (or refused) a requested `cd`.
    pub(crate) fn directory_changed(
        &mut self,
        request_id: &str,
        directory: Option<&str>,
    ) {
        if self.pending_cd.as_deref() != Some(request_id) {
            return;
        }
        self.pending_cd = None;
        if let Some(directory) = directory {
            self.set_workspace_root(directory);
        }
    }

    /// Move everything workspace-scoped to `root` and declare it to the host.
    pub fn set_workspace_root(&mut self, root: &str) {
        let root = root.trim_end_matches('/').to_owned();
        if root == self.cwd {
            return;
        }
        self.view_workspace_root(&root);
        if self.workspace_ready {
            if let Some(d) = &self.daemon {
                d.send(Family::Workspace, serde_json::json!({"SetWorkspaceRoot":{"workspace_id":self.workspace_id,"root_dir":self.cwd}}));
            }
        }
    }

    /// This window's view of the workspace moves to `root`: name, tree, notes, conversations and
    /// git (`activateWorkspace` over `workspaceView`). Nothing is sent to the host.
    fn view_workspace_root(&mut self, root: &str) {
        let root = root.trim_end_matches('/').to_owned();
        let home = crate::startup::home();
        self.workspace_name = if root == home {
            "~".into()
        } else {
            std::path::Path::new(&root)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.clone())
        };
        self.cwd = root;
        self.tree = Default::default();
        self.notes = Default::default();
        // An open tree or notes panel lists the new root at once.
        if let Some(sidebar @ (crate::app::Sidebar::Files | crate::app::Sidebar::Notes)) = self.sidebar {
            self.sidebar = None;
            self.toggle_tree(sidebar);
        }
        self.refresh_threads();
        self.watch_git();
    }
}

use crate::agent::Session;
use crate::app::{Connection, Focus, Sidebar};
use crate::daemon::{Daemon, Inbox};
use crate::home::Terminal;
use crate::tabs::Tab;
use neoism_protocol::agent::ThreadSummary;
use serde_json::json;
use std::collections::{HashMap, VecDeque};

/// Each workspace owns its socket and inbox: replies cannot cross workspace boundaries.
pub struct Entry {
    pub id: String,
    pub title: String,
    /// The server this workspace is on (`servers::Selection::id`); `None` is the local daemon.
    /// Its tab wears a cloud.
    pub server: Option<String>,
    pub state: Option<State>,
}

/// A workspace to open in a new tab (`App::open_workspace`).
pub struct Opening {
    pub id: String,
    pub title: String,
    pub cwd: String,
    /// The daemon it dials; `None` is the local one.
    pub target: Option<crate::daemon::Target>,
    pub link: crate::servers::flow::Link,
}
pub struct State {
    workspace_id: String,
    workspace_ready: bool,
    workspace_closing: Option<u64>,
    activity: Option<crate::agent::Activity>,
    pending_cd: Option<String>,
    main_terminal: Option<u64>,
    notes: crate::tree::Tree,
    tree: crate::tree::Tree,
    editor_follow: bool,
    tree_follow: crate::ui::follow::Follow,
    conv_selected: usize,
    conv_follow: crate::ui::follow::Follow,
    pending_g: bool,
    buffers: HashMap<String, crate::editor::Buffer>,
    finder: Option<crate::finder::Finder>,
    families: crate::agent::Families,
    header_hover: Option<crate::agent::HeaderStep>,
    git: Option<crate::ui::status::GitSummary>,
    picker: Option<crate::palette::Picker>,
    daemon: Option<Daemon>,
    inbox: Option<Inbox>,
    connection: Connection,
    workspace_name: String,
    cwd: String,
    threads: Vec<ThreadSummary>,
    sessions: HashMap<String, Session>,
    tabs: Vec<Tab>,
    active: usize,
    layout: crate::panes::Layout,
    field_tab: Option<u64>,
    sidebar: Option<Sidebar>,
    composer: crate::agent::slash::Composer,
    focus: Focus,
    terminals: HashMap<u64, Terminal>,
    pending_pty: VecDeque<u64>,
    databases: HashMap<String, crate::database::Database>,
    splash: crate::home::SplashState,
    /// The workspace's server (and the tunnel to it).
    link: crate::servers::flow::Link,
    collab: crate::collab::Collab,
}
impl State {
    /// This parked workspace as recovery data (`session.rs`).
    pub(crate) fn capture(&self) -> crate::session::SavedWorkspace {
        crate::session::capture_workspace(&self.workspace_name, &self.cwd, &self.tabs, self.active, &self.layout,
            self.sidebar, App::unsaved_buffers(&self.buffers))
    }

    fn from_app(app: App) -> Self {
        Self {
            workspace_id: app.workspace_id,
            workspace_ready: app.workspace_ready,
            workspace_closing: app.workspace_closing,
            activity: app.activity,
            pending_cd: app.pending_cd,
            main_terminal: app.main_terminal,
            notes: app.notes,
            tree: app.tree,
            editor_follow: app.editor_follow,
            tree_follow: app.tree_follow,
            conv_selected: app.conv_selected,
            conv_follow: app.conv_follow,
            pending_g: app.pending_g,
            buffers: app.buffers,
            finder: app.finder,
            families: app.families,
            header_hover: app.header_hover,
            git: app.git,
            picker: app.picker,
            daemon: app.daemon,
            inbox: app.inbox,
            connection: app.connection,
            workspace_name: app.workspace_name,
            cwd: app.cwd,
            threads: app.threads,
            sessions: app.sessions,
            tabs: app.tabs,
            active: app.active,
            layout: app.layout,
            field_tab: app.field_tab,
            sidebar: app.sidebar,
            composer: app.composer,
            focus: app.focus,
            terminals: app.terminals,
            pending_pty: app.pending_pty,
            databases: app.databases,
            splash: app.splash,
            link: app.server.link,
            collab: app.collab,
        }
    }
    fn swap(&mut self, app: &mut App) {
        std::mem::swap(&mut self.workspace_id, &mut app.workspace_id);
        std::mem::swap(&mut self.workspace_ready, &mut app.workspace_ready);
        std::mem::swap(&mut self.workspace_closing, &mut app.workspace_closing);
        std::mem::swap(&mut self.activity, &mut app.activity);
        std::mem::swap(&mut self.pending_cd, &mut app.pending_cd);
        std::mem::swap(&mut self.main_terminal, &mut app.main_terminal);
        std::mem::swap(&mut self.notes, &mut app.notes);
        std::mem::swap(&mut self.tree, &mut app.tree);
        std::mem::swap(&mut self.editor_follow, &mut app.editor_follow);
        std::mem::swap(&mut self.tree_follow, &mut app.tree_follow);
        std::mem::swap(&mut self.conv_selected, &mut app.conv_selected);
        std::mem::swap(&mut self.conv_follow, &mut app.conv_follow);
        std::mem::swap(&mut self.pending_g, &mut app.pending_g);
        std::mem::swap(&mut self.buffers, &mut app.buffers);
        std::mem::swap(&mut self.finder, &mut app.finder);
        std::mem::swap(&mut self.families, &mut app.families);
        std::mem::swap(&mut self.header_hover, &mut app.header_hover);
        std::mem::swap(&mut self.git, &mut app.git);
        std::mem::swap(&mut self.picker, &mut app.picker);
        std::mem::swap(&mut self.daemon, &mut app.daemon);
        std::mem::swap(&mut self.inbox, &mut app.inbox);
        std::mem::swap(&mut self.connection, &mut app.connection);
        std::mem::swap(&mut self.workspace_name, &mut app.workspace_name);
        std::mem::swap(&mut self.cwd, &mut app.cwd);
        std::mem::swap(&mut self.threads, &mut app.threads);
        std::mem::swap(&mut self.sessions, &mut app.sessions);
        std::mem::swap(&mut self.tabs, &mut app.tabs);
        std::mem::swap(&mut self.active, &mut app.active);
        std::mem::swap(&mut self.layout, &mut app.layout);
        std::mem::swap(&mut self.field_tab, &mut app.field_tab);
        std::mem::swap(&mut self.sidebar, &mut app.sidebar);
        std::mem::swap(&mut self.composer, &mut app.composer);
        std::mem::swap(&mut self.focus, &mut app.focus);
        std::mem::swap(&mut self.terminals, &mut app.terminals);
        std::mem::swap(&mut self.pending_pty, &mut app.pending_pty);
        std::mem::swap(&mut self.databases, &mut app.databases);
        std::mem::swap(&mut self.splash, &mut app.splash);
        std::mem::swap(&mut self.link, &mut app.server.link);
        std::mem::swap(&mut self.collab, &mut app.collab);
    }
}

pub fn new_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "rune-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

impl App {
    /// The roots of this window's other workspaces (the project dropdown lists them).
    pub fn window_roots(&self) -> Vec<String> {
        self.workspaces.iter().filter_map(|e| e.state.as_ref()).map(|s| s.cwd.trim_end_matches('/').to_owned()).collect()
    }

    fn swap_workspace(&mut self, index: usize) {
        if index == self.workspace_active || index >= self.workspaces.len() {
            return;
        }
        let old = self.workspace_active;
        self.workspaces[old].title = self.workspace_name.clone();
        let mut incoming = self.workspaces[index]
            .state
            .take()
            .expect("inactive workspace state");
        incoming.swap(self);
        self.workspaces[old].state = Some(incoming);
        self.workspace_active = index;
        self.render_tab.set(None);
        // Labels are window fields; the actual prompt selection belongs to the workspace.
        // Refresh them with the incoming composer before the next frame draws its chips.
        self.sync_chips();
    }

    pub fn select_workspace(&mut self, index: usize) {
        if self.workspace_closing.is_some() || index >= self.workspaces.len() || index == self.workspace_active {
            return;
        }
        // This user's cursor leaves the workspace they are switching away from.
        self.presence_leave();
        self.editor_hidden();
        self.conversations_hidden();
        self.swap_workspace(index);
        // This strip is window-scoped and may not relayout when selection changes.
        // Keep its measured bounds, including an in-flight workspace drag.
        self.rects.retain(|key, _| key.starts_with("workspace-") || key.starts_with("reorder:workspace:"));
        self.tab_hover = None;
        self.field_tab = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if index != self.active { tab.draft.focused = false; }
        }
    }

    /// A new local workspace tab, at the active one's directory (on a remote tab: home).
    pub fn new_workspace(&mut self) {
        let (title, cwd) = if self.server.remote() {
            let home = crate::startup::home();
            ("~".to_owned(), home)
        } else { (self.workspace_name.clone(), self.cwd.clone()) };
        self.open_workspace(Opening { id: new_id(), title, cwd, target: None, link: Default::default() });
    }

    /// Open a workspace in a new tab and go to it: its own daemon socket (to `target`), tabs
    /// and state; the other tabs keep theirs.
    pub fn open_workspace(&mut self, opening: Opening) {
        if self.workspace_closing.is_some() {
            return;
        }
        let Opening { id, title, cwd, target, link } = opening;
        let server = link.current.as_ref().map(|s| s.id.clone());
        let mut fresh = App::new();
        fresh.workspace_id = id;
        fresh.cwd = cwd;
        fresh.workspace_name = title;
        fresh.server.link = link;
        fresh.next_tab = self.next_tab;
        // Tab ids stay unique across the window (Rune element keys and scroll handles).
        fresh.tabs.clear();
        fresh.layout = Default::default();
        fresh.terminals.clear();
        fresh.open_tab(TabKind::Home);
        self.next_tab = fresh.next_tab;
        if let Some(wake) = self
            .waker
            .clone()
            .filter(|_| std::env::var_os("NEOISM_RUNE_OFFLINE").is_none())
        {
            let (daemon, inbox) = Daemon::start_to(target, move || wake());
            fresh.daemon = Some(daemon);
            fresh.inbox = Some(inbox);
        }
        self.workspaces.push(Entry {
            id: fresh.workspace_id.clone(),
            title: fresh.workspace_name.clone(),
            server,
            state: Some(State::from_app(fresh)),
        });
        self.select_workspace(self.workspaces.len() - 1);
    }

    /// A workspace on another server opens before its folder is known (`~` stands in): the
    /// daemon has now said where it is. The terminals opened meanwhile start there too, since
    /// no shell can start in a folder named `~`.
    pub(crate) fn resolve_root(&mut self, root: &str) {
        self.cwd = root.to_owned();
        for term in self.terminals.values_mut() {
            if term.shell.cwd == "~" && term.shell.session.is_none() { term.shell.cwd = root.to_owned(); }
        }
    }

    pub(crate) fn register_workspace(&self) {
        // On another server the daemon picks the workspace first (`adopt_workspace`).
        if self.server.link.adopt.is_some() {
            if let Some(d) = &self.daemon { d.send(Family::Workspace, json!({"ResolveInitialWorkspace":{"preferred_host_id":null}})); }
            return;
        }
        if let Some(d) = &self.daemon {
            d.send(Family::Workspace, json!({"CreateWorkspace":{"workspace_id":self.workspace_id,"title":self.workspace_name,"root_dir":self.cwd}}));
            d.send(Family::Workspace, json!({"SetWindowWorkspaces":{"workspace_ids":[self.workspace_id],"prune_empty":null}}));
            d.send(
                Family::Workspace,
                json!({"SwitchHostWorkspace":{"workspace_id":self.workspace_id}}),
            );
        }
    }

    pub(crate) fn workspace_reply(
        &mut self,
        request_id: u64,
        message: &serde_json::Value,
    ) {
        if let Some(resolved) = message.get("InitialWorkspaceResolved") { self.adopt_workspace(&resolved["workspace"]); }
        // The daemon's record says whether the workspace is shared (`workspace.visibility`).
        for record in ["InitialWorkspaceResolved", "HostWorkspaceUpserted"].iter().filter_map(|key| message.get(*key)).map(|m| &m["workspace"]) {
            if record["id"].as_str() == Some(&self.workspace_id) && let Some(visibility) = record["visibility"].as_str() {
                self.collab.shared = matches!(visibility, "shared" | "team");
            }
        }
        // A remote home workspace (`~`) reports where that is.
        if self.server.remote() && self.cwd == "~"
            && let Some(workspace) = message.get("HostWorkspaceUpserted").map(|m| &m["workspace"]).filter(|w| w["id"].as_str() == Some(&self.workspace_id))
            && let Some(root) = workspace["root_dir"].as_str() { let root = root.to_owned(); self.resolve_root(&root); }
        if message["HostWorkspaceChanged"]["workspace_id"].as_str()
            == Some(&self.workspace_id)
            && !self.workspace_ready
        {
            self.workspace_ready = true;
            self.agent_connected();
            self.terminals_connected();
            self.watch_git();
            self.lsp_reconnect();
        }
        if self.workspace_closing == Some(request_id) {
            self.workspace_closing = None;
            if message.get("HostWorkspaceTree").is_some() {
                self.finish_workspace_close();
            } else if let Some(error) = message.get("Error") {
                self.modal = Some(crate::ui::modal::Modal {
                    title: "Could not close workspace".into(),
                    body: error["message"]
                        .as_str()
                        .unwrap_or("Daemon rejected close")
                        .into(),
                    buttons: vec![],
                    selected: 0,
                });
            }
        }
    }

    pub fn close_workspace(&mut self, index: usize) {
        self.close_workspace_with_discard(index, false);
    }

    pub fn close_workspace_with_discard(&mut self, index: usize, discard: bool) {
        if index >= self.workspaces.len() {
            return;
        }
        self.select_workspace(index);
        if self.workspace_closing.is_some() {
            return;
        }
        // No unsaved-changes blocker: closing a workspace just closes it.
        let _ = discard;
        let pending = self.release_workspace();
        if let Some(done) = pending {
            self.closing_connections.push(ClosingConnection { done, _link: std::mem::take(&mut self.server.link),
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(2) });
        }
        self.finish_workspace_close();
    }

    /// Close only resources owned by this view. The daemon decides whether the workspace
    /// itself is private and can be retired, or is shared and must survive our departure.
    fn release_workspace(&mut self) -> Option<std::sync::mpsc::Receiver<()>> {
        let tabs: Vec<_> = self.terminals.keys().copied().collect();
        for tab in tabs { self.close_terminal(tab); }
        let tabs: Vec<_> = self.tabs.iter().map(|tab| tab.id).collect();
        for tab in tabs { self.forget_table_state(tab); }
        self.daemon.take().map(|daemon| daemon.close(&self.workspace_id))
    }

    pub fn poll_closing_connections(&mut self, tx: &mut rune::Tx) {
        self.closing_connections.retain(|closing| {
            closing.deadline > std::time::Instant::now()
                && matches!(closing.done.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty))
        });
        if !self.closing_connections.is_empty() { tx.wake_after(std::time::Duration::from_millis(50)); }
    }

    pub fn close_window_resources(&mut self) {
        if std::mem::replace(&mut self.resources_closed, true) { return; }
        if !self.session_closed { self.save_session(true); }
        let active = self.workspace_active;
        let mut pending = Vec::new();
        for index in 0..self.workspaces.len() {
            self.swap_workspace(index);
            if let Some(done) = self.release_workspace() { pending.push(done); }
        }
        self.swap_workspace(active);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        for done in pending { let _ = done.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())); }
        for closing in self.closing_connections.drain(..) {
            let _ = closing.done.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()));
        }
        crate::servers::shutdown();
    }

    fn finish_workspace_close(&mut self) {
        if self.workspaces.len() == 1 {
            // Closing the last workspace on purpose leaves nothing to reopen next time.
            // (Never from a unit test: `qall` there would delete the user's own session file.)
            if crate::session::enabled() && !cfg!(test) { let _ = std::fs::remove_file(crate::session::path()); }
            self.session_writer = Default::default();
            self.session_closed = true;
            self.quit_requested = true;
            return;
        }
        let closing = self.workspace_active;
        let next = if closing + 1 < self.workspaces.len() {
            closing + 1
        } else {
            closing - 1
        };
        self.swap_workspace(next);
        // Dropping only this socket releases its editor leases. Shared daemon sessions survive.
        self.workspaces.remove(closing);
        if next > closing {
            self.workspace_active -= 1;
        }
        self.rects.clear();
        self.tab_hover = None;
        self.workspace_hover = None;
    }

    pub fn drain(&mut self) -> bool {
        let ids: Vec<String> = self
            .workspaces
            .iter()
            .enumerate()
            .map(|(i, e)| {
                if i == self.workspace_active {
                    self.workspace_id.clone()
                } else {
                    e.state.as_ref().unwrap().workspace_id.clone()
                }
            })
            .collect();
        let mut changed = false;
        for id in ids {
            let original = self.workspace_id.clone();
            let Some(i) = self.workspace_index(&id) else {
                continue;
            };
            let inbox = if i == self.workspace_active {
                self.inbox.as_ref()
            } else {
                self.workspaces[i]
                    .state
                    .as_ref()
                    .and_then(|s| s.inbox.as_ref())
            };
            if inbox.is_none_or(|inbox| inbox.lock().unwrap().is_empty()) {
                continue;
            }
            self.swap_workspace(i);
            changed |= self.drain_current();
            if let Some(i) = self.workspace_index(&original) {
                self.swap_workspace(i);
            }
        }
        changed
    }

    fn workspace_index(&self, id: &str) -> Option<usize> {
        self.workspaces.iter().enumerate().find_map(|(i, e)| {
            (if i == self.workspace_active {
                &self.workspace_id
            } else {
                &e.state.as_ref()?.workspace_id
            } == id)
                .then_some(i)
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_chips_follow_the_restored_prompt_selection() {
        let mut app = crate::app::App::new();
        app.open_tab(crate::tabs::TabKind::Agent { session: None });
        app.composer.selection = crate::agent::slash::Selection {
            model: "openai/gpt-6.1-sol".into(), agent: "build".into(), thinking: "high".into(),
        };
        app.sync_chips();
        let first = (app.model.clone(), app.agent.clone(), app.effort.clone());
        app.new_workspace();
        app.open_tab(crate::tabs::TabKind::Agent { session: None });
        app.composer.selection = crate::agent::slash::Selection {
            model: "openrouter/anthropic/claude".into(), agent: "plan".into(), thinking: "medium".into(),
        };
        app.sync_chips();
        let second = (app.model.clone(), app.agent.clone(), app.effort.clone());
        app.select_workspace(0);
        assert_eq!((app.model.clone(), app.agent.clone(), app.effort.clone()), first);
        assert_eq!(app.composer.selection.model, "openai/gpt-6.1-sol");
        app.select_workspace(1);
        assert_eq!((app.model.clone(), app.agent.clone(), app.effort.clone()), second);
        app.composer.selection.model.clear();
        app.select_workspace(0);
        app.select_workspace(1);
        assert_eq!(app.model, "Select model");
    }

    use super::*;
    #[test]
    fn switching_preserves_drafts_panes_focus_and_unique_ids() {
        let mut app = App::new();
        let first = app.workspace_id.clone();
        app.open_tab(TabKind::Agent { session: None });
        app.tabs[app.active].draft = rune::TextEdit::new("unfinished prompt");
        let tab = app.active_tab().id;
        app.focus = Focus::Buffers(app.active);
        app.new_workspace();
        assert_ne!(app.workspace_id, first);
        assert!(app.active_tab().id > tab);
        assert_eq!(app.tabs.len(), 1);
        app.select_workspace(0);
        assert_eq!(app.workspace_id, first);
        assert_eq!(app.active_tab().id, tab);
        assert_eq!(app.active_tab().draft.text, "unfinished prompt");
        assert!(matches!(app.focus, Focus::Buffers(_)));
    }
    #[test]
    fn workspace_selection_keeps_window_strip_geometry() {
        let mut app = App::new();
        app.new_workspace();
        let rect = [55.5, 0.0, 180.0, 30.0];
        let stable = format!("reorder:workspace:{}", app.workspaces[0].id);
        app.rects.insert(stable.clone(), rect);
        app.rects.insert("workspace-tabs".into(), rect);
        app.rects.insert("old-editor".into(), rect);
        app.select_workspace(0);
        assert_eq!(app.rects[&stable], rect);
        assert_eq!(app.rects["workspace-tabs"], rect);
        assert!(!app.rects.contains_key("old-editor"));
        app.rects.insert("current-editor".into(), rect);
        app.select_workspace(0);
        assert_eq!(app.rects["current-editor"], rect);
    }
    #[test]
    fn close_selects_next_then_previous_and_final_window() {
        let mut app = App::new();
        app.new_workspace();
        app.new_workspace();
        let last = app.workspace_id.clone();
        app.close_workspace(1);
        assert_eq!(app.workspaces.len(), 2);
        assert_eq!(app.workspace_id, last);
        app.close_workspace(1);
        assert_eq!(app.workspace_active, 0);
        assert!(!app.quit_requested);
        app.close_workspace(0);
        assert!(app.quit_requested);
    }
    #[test]
    fn agent_q_and_qall_have_distinct_scopes() {
        let mut app = App::new();
        app.open_tab(TabKind::Agent { session: None });
        app.tabs[app.active].draft = rune::TextEdit::new("q");
        app.submit();
        assert_eq!(app.tabs.len(), 1);
        assert!(!app.quit_requested);
        app.open_tab(TabKind::Agent { session: None });
        app.tabs[app.active].draft = rune::TextEdit::new("qall");
        app.submit();
        assert!(app.quit_requested);
    }
    #[test]
    fn workspace_close_never_blocks_on_drafts() {
        let mut app = App::new();
        app.new_workspace();
        app.tabs[0].draft = rune::TextEdit::new("unsent");
        app.close_workspace(1);
        assert!(app.modal.is_none());
    }
    #[test]
    fn replies_after_close_cannot_change_the_next_workspaces_buffer() {
        use crate::daemon::Inbound;
        let mut app = App::new();
        app.new_workspace();
        let next = app.workspace_id.clone();
        app.open_file("shared.txt", None);
        app.select_workspace(0);
        app.workspace_closing = Some(7);
        app.inbox = Some(std::sync::Arc::new(std::sync::Mutex::new(VecDeque::from(
            [
                Inbound::Reply {
                    family: "Workspace".into(),
                    request_id: 7,
                    message: json!({"HostWorkspaceTree":{}}),
                },
                Inbound::Reply {
                    family: "Files".into(),
                    request_id: 8,
                    message: json!({"FileContent":{"path":"shared.txt","bytes":b"wrong workspace"}}),
                },
            ],
        ))));
        assert!(app.drain_current());
        assert_eq!(app.workspace_id, next);
        assert!(!app.buffers["shared.txt"].loaded);
    }
}
