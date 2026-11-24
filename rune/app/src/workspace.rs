//! The workspace root: where the finder, tree, git watch and new conversations look. An explicit
//! directory change goes through the main terminal when supported, then the workspace moves
//! to the directory the host confirms.
//!
//! One terminal per workspace owns the working directory (`mainTerminalSurface`): the one the
//! user starred, else the first Home or Terminal tab with a shell. While it is the active tab,
//! a `cd` inside it moves this window's view of the workspace (tree, notes, conversations, git,
//! paths) with it (`followMainTerminal`); the host's declared root is left alone.

use crate::app::App;
use crate::daemon::Family;
use crate::tabs::TabKind;
use neoism_protocol::pty::ClientMessage as PtyClient;

pub(crate) enum DirectoryChange {
    Starting { path: String },
    Shell { request_id: String, session_id: String },
    Workspace { request_id: u64 },
}

/// Keep an SSH tunnel alive until its close request has reached the server.
pub struct ClosingConnection {
    done: std::sync::mpsc::Receiver<()>,
    _link: crate::servers::flow::Link,
    deadline: std::time::Instant,
}

// Workers retain remote tunnels while the window is already gone. The main thread joins
// them only after Rune has destroyed the native windows and returned from its event loop.
static WINDOW_CLEANUP: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

fn defer_window_cleanup(pending: Vec<ClosingConnection>) {
    let worker = std::thread::spawn(move || {
        for closing in pending {
            let _ = closing.done.recv_timeout(closing.deadline.saturating_duration_since(std::time::Instant::now()));
        }
    });
    let mut workers = WINDOW_CLEANUP.lock().unwrap();
    workers.retain(|worker| !worker.is_finished());
    workers.push(worker);
}

pub fn finish_window_cleanup() {
    let workers = std::mem::take(&mut *WINDOW_CLEANUP.lock().unwrap());
    for worker in workers { let _ = worker.join(); }
    crate::servers::shutdown();
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
        let Some(term) = self.terminals.get_mut(&active) else { return false };
        let cwd = shell_directory(&term.shell.cwd, &self.cwd);
        if term.cwd_before_workspace_change.as_ref().is_some_and(|old| same_directory(old, &cwd)) { return false; }
        if crate::startup::absolute(&cwd) { term.cwd_before_workspace_change = None; }
        if !crate::startup::absolute(&cwd) || same_directory(&cwd, &self.cwd) || cwd.chars().any(char::is_control) { return false; }
        if self.root_refused.as_deref() == Some(cwd.as_str()) { return false; }
        // What a shell says is a claim, not a folder: an `ssh` or a container inside the
        // terminal reports a directory of another machine, and a shell may spell its own in
        // a way the daemon cannot open. The workspace only moves to a folder the daemon
        // confirms it has (`root_probe_reply`); until then, and if it refuses, it stays.
        let Some(daemon) = &self.daemon else {
            self.view_workspace_root(&cwd);
            return true;
        };
        if self.root_probe.as_ref().is_none_or(|(_, asked)| asked != &cwd) {
            let id = daemon.request(Family::Files, serde_json::json!({"Stat": {"path": "."}}), Some(cwd.clone()));
            self.root_probe = Some((id, cwd));
        }
        false
    }

    /// The daemon's answer about a folder the main terminal reported. Returns whether the
    /// reply was that answer.
    pub(crate) fn root_probe_reply(&mut self, request_id: u64, message: &serde_json::Value) -> bool {
        let Some((_, root)) = self.root_probe.take_if(|(asked, _)| *asked == request_id) else { return false };
        if message.get("Stat").is_some() {
            self.root_refused = None;
            // Only if that terminal still owns the directory and is still there.
            let active = self.active_tab().id;
            let still = self.main_terminal_tab() == Some(active)
                && self.terminals.get(&active).is_some_and(|t| same_directory(&shell_directory(&t.shell.cwd, &self.cwd), &root) || shell_directory(&t.shell.cwd, &self.cwd) == root);
            if still && self.pending_cd.is_none() { self.view_workspace_root(&root); }
        } else {
            self.root_refused = Some(root);
        }
        true
    }

    /// `cd` the workspace to `path` (absolute).
    pub fn change_directory(&mut self, path: &str) {
        if !crate::startup::absolute(path) || path.chars().any(char::is_control) {
            return self.directory_error("Choose an absolute directory without control characters.");
        }
        if self.pending_cd.is_some() {
            return self.directory_error("Still waiting for the previous directory change.");
        }
        if self.daemon.is_some() && !self.workspace_ready {
            // A fast pick on the splash can arrive before the host has registered this workspace.
            self.pending_cd = Some(DirectoryChange::Starting { path: path.to_owned() });
            return;
        }
        let session = self.main_terminal_tab()
            .and_then(|id| self.terminals.get(&id))
            .and_then(|term| term.shell.session.clone());
        let (Some(session_id), Some(daemon)) = (session, &self.daemon) else {
            self.request_workspace_root(path);
            return;
        };
        let request_id = format!("rune-cd-{}", uuid::Uuid::new_v4());
        let message = PtyClient::ChangeTerminalDirectory {
            session_id: session_id.clone(),
            directory: path.to_owned(),
            request_id: request_id.clone(),
        };
        if let Ok(value) = serde_json::to_value(&message) {
            daemon.send(Family::Pty, value);
        }
        self.pending_cd = Some(DirectoryChange::Shell { request_id, session_id });
    }

    /// The shell confirmed (or refused) a requested `cd`.
    pub(crate) fn directory_changed(
        &mut self,
        session: &str,
        request_id: &str,
        result: Result<&str, &str>,
    ) {
        let Some(DirectoryChange::Shell { .. }) = self.pending_cd.take_if(|pending| matches!(pending,
            DirectoryChange::Shell { request_id: id, session_id, .. } if id == request_id && session_id == session)) else { return };
        match result {
            Ok(directory) => {
                if let Some(term) = self.terminals.values_mut().find(|term| term.shell.session.as_deref() == Some(session)) {
                    term.shell.cwd = directory.to_owned();
                }
                self.request_workspace_root(directory);
            }
            Err(message) => self.directory_error(message),
        }
    }

    pub(crate) fn directory_terminal_closed(&mut self, session: &str) {
        if matches!(&self.pending_cd, Some(DirectoryChange::Shell { session_id, .. }) if session_id == session) {
            self.pending_cd = None;
            self.directory_error("The main terminal closed before confirming the directory change.");
        }
    }

    pub(crate) fn directory_disconnected(&mut self) {
        if self.pending_cd.take().is_some() {
            self.directory_error("The connection closed before confirming the directory change. Try again after reconnecting.");
        }
    }

    fn directory_error(&mut self, message: &str) {
        tracing::warn!("Directory change failed: {message}");
    }

    fn request_workspace_root(&mut self, root: &str) {
        if let Some(daemon) = &self.daemon {
            let request_id = daemon.request(Family::Workspace, json!({"SetWorkspaceRoot":{"workspace_id":self.workspace_id,"root_dir":root}}), None);
            self.pending_cd = Some(DirectoryChange::Workspace { request_id });
        } else {
            self.apply_explicit_root(root);
        }
    }

    fn apply_explicit_root(&mut self, root: &str) {
        self.root_probe = None;
        self.root_refused = None;
        for (id, term) in &mut self.terminals {
            if term.shell.session.is_none() && !self.pending_pty.contains(id) {
                term.shell.cwd = root.to_owned();
                term.cwd_before_workspace_change = None;
            } else {
                // A terminal already at its prompt (or still starting) must report a new
                // directory before it can pull the workspace away from the explicit choice.
                term.cwd_before_workspace_change = Some(shell_directory(&term.shell.cwd, root));
            }
        }
        let filter_changed = std::mem::take(&mut self.conv_all);
        let normalized = normalize_root(root);
        if !same_directory(&normalized, &self.cwd) {
            self.view_workspace_root(&normalized);
        } else if filter_changed {
            self.refresh_threads();
        }
    }

    /// This window's view of the workspace moves to `root`: name, tree, notes, conversations and
    /// git (`activateWorkspace` over `workspaceView`). Nothing is sent to the host.
    fn view_workspace_root(&mut self, root: &str) {
        // `/` is a folder too: trimmed to nothing it left the workspace with no root at all.
        let root = normalize_root(root);
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
        self.reset_git_panel();
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

fn normalize_root(root: &str) -> String {
    let trimmed = root.trim_end_matches('/');
    if trimmed.is_empty() && root.starts_with('/') {
        "/".to_owned()
    } else if trimmed.len() == 2 && trimmed.len() < root.len() && drive_path(trimmed) {
        format!("{trimmed}/")
    } else {
        trimmed.to_owned()
    }
}

/// Whether `path` is a Windows drive path (`C:\Users`, `c:/work`).
fn drive_path(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && b.get(2).is_none_or(|c| matches!(c, b'\\' | b'/'))
}

/// A shell's reported directory as the workspace names directories.
///
/// A shell says where it is with a `file://` URL (OSC 7). On Windows that is
/// `file://host/C:/Users/me`, whose path reads `/C:/Users/me`, and a POSIX-flavoured shell
/// (Git Bash, MSYS) says `/c/Users/me`. Both are absolute, neither is a folder the Windows
/// daemon can open: taken as the workspace's root they emptied the file tree, the notes and
/// the conversation list, and left every new shell and prompt with nowhere to run, a moment
/// after the window had shown them. On a workspace whose root is a drive path they are read
/// back as that drive path (`C:\Users\me`).
fn shell_directory(reported: &str, workspace: &str) -> String {
    let reported = reported.trim();
    if !drive_path(workspace) {
        let trimmed = reported.trim_end_matches('/');
        return if trimmed.is_empty() && reported.starts_with('/') { "/".to_owned() } else { trimmed.to_owned() };
    }
    let b = reported.as_bytes();
    let drive = if b.first() == Some(&b'/') && drive_path(&reported[1..]) {
        // `/C:/Users/me`
        Some((b[1] as char, &reported[3..]))
    } else if drive_path(reported) {
        Some((b[0] as char, &reported[2..]))
    } else if b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b.get(2).is_none_or(|c| *c == b'/') {
        // `/c/Users/me`
        Some((b[1] as char, &reported[2..]))
    } else { None };
    let Some((letter, rest)) = drive else { return reported.trim_end_matches(['/', '\\']).to_owned() };
    let rest = rest.replace('/', "\\");
    let rest = rest.trim_matches('\\');
    format!("{}:\\{rest}", letter.to_ascii_uppercase())
}

/// Whether two spellings name one directory: exactly, or on Windows without regard to case,
/// separator or a trailing separator.
fn same_directory(a: &str, b: &str) -> bool {
    if a == b { return true; }
    if !drive_path(a) || !drive_path(b) { return false; }
    let fold = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    fold(a) == fold(b)
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
    agent_resync_due: bool,
    workspace_closing: Option<u64>,
    activity: Option<crate::agent::Activity>,
    pending_cd: Option<DirectoryChange>,
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
    git_panel: crate::git::State,
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
    /// Positive availability evidence without swapping a parked workspace into App.
    pub(crate) fn server_connected(&self) -> bool { matches!(self.connection, Connection::Connected) }

    /// This parked workspace as recovery data (`session.rs`).
    pub(crate) fn capture(&self) -> crate::session::SavedWorkspace {
        crate::session::capture_workspace(&self.workspace_name, &self.cwd, &self.tabs, self.active, &self.layout,
            self.sidebar, App::unsaved_buffers(&self.buffers))
    }

    fn from_app(app: App) -> Self {
        Self {
            workspace_id: app.workspace_id,
            workspace_ready: app.workspace_ready,
            agent_resync_due: app.agent_resync_due,
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
            git_panel: app.git_panel,
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
        std::mem::swap(&mut self.agent_resync_due, &mut app.agent_resync_due);
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
        std::mem::swap(&mut self.git_panel, &mut app.git_panel);
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
        self.workflows = None;
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
        if self.pending_cd.take_if(|pending| matches!(pending,
            DirectoryChange::Workspace { request_id: id } if *id == request_id)).is_some() {
            let workspace = &message["HostWorkspaceUpserted"]["workspace"];
            if workspace["id"].as_str() == Some(&self.workspace_id)
                && let Some(root) = workspace["root_dir"].as_str().filter(|root| crate::startup::absolute(root))
            {
                self.apply_explicit_root(root);
            } else {
                self.directory_error(message["Error"]["message"].as_str().unwrap_or("The host did not confirm the selected directory."));
            }
        }
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
            if let Some(DirectoryChange::Starting { path }) = self.pending_cd.take_if(|pending| matches!(pending, DirectoryChange::Starting { .. })) {
                self.change_directory(&path);
            }
            self.agent_connected();
            if std::mem::take(&mut self.agent_resync_due) { self.agent_resync(true); }
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut pending = std::mem::take(&mut self.closing_connections);
        for index in 0..self.workspaces.len() {
            self.swap_workspace(index);
            if let Some(done) = self.release_workspace() {
                pending.push(ClosingConnection { done, _link: std::mem::take(&mut self.server.link), deadline });
            }
        }
        self.swap_workspace(active);
        defer_window_cleanup(pending);
    }

    fn finish_workspace_close(&mut self) {
        if self.workspaces.len() == 1 {
            // Closing the last workspace on purpose leaves nothing to reopen next time.
            self.forget_session();
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

    /// Reserve the rest of the frame for input, animation and rendering. One event may
    /// exceed this soft deadline; queued events are never dropped to meet it.
    pub fn drain(&mut self) -> (bool, bool) {
        self.drain_budgeted(std::time::Instant::now() + std::time::Duration::from_millis(1), 256)
    }

    fn drain_budgeted(&mut self, deadline: std::time::Instant, limit: usize) -> (bool, bool) {
        static TRACE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("NEOISM_RUNE_TRACE_POLL").is_some());
        let started = std::time::Instant::now();
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
        if ids.is_empty() { return (false, false); }
        let mut next = self.inbox_next.as_ref().and_then(|id| ids.iter().position(|candidate| candidate == id))
            .unwrap_or(self.workspace_active.min(ids.len() - 1));
        let mut changed = false;
        let mut applied = 0;
        let mut empty = 0;
        // Keep at least one event progressing even if scheduling used up this slice already.
        while applied < limit && empty < ids.len() && (applied == 0 || std::time::Instant::now() < deadline) {
            let id = &ids[next];
            next = (next + 1) % ids.len();
            self.inbox_next = Some(ids[next].clone());
            let original = self.workspace_id.clone();
            let Some(i) = self.workspace_index(id) else {
                empty += 1;
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
            let Some(inbox) = inbox.cloned() else { empty += 1; continue };
            let Some(first) = inbox.lock().unwrap().pop_front() else {
                empty += 1;
                continue;
            };
            empty = 0;
            self.swap_workspace(i);
            let mut event = first;
            for turn in 0..16 {
                changed |= self.apply_inbound(event);
                applied += 1;
                // A close acknowledgement can select a different workspace. Never apply
                // the closed socket's remaining events to that workspace.
                if self.workspace_id != *id || applied >= limit || std::time::Instant::now() >= deadline || turn == 15 { break; }
                let Some(queued) = inbox.lock().unwrap().pop_front() else { break };
                event = queued;
            }
            if let Some(i) = self.workspace_index(&original) {
                self.swap_workspace(i);
            }
        }
        let remaining: usize = self.workspaces.iter().enumerate().map(|(i, entry)| {
            let inbox = if i == self.workspace_active { self.inbox.as_ref() } else { entry.state.as_ref().and_then(|state| state.inbox.as_ref()) };
            inbox.map_or(0, |inbox| inbox.lock().unwrap().len())
        }).sum();
        if *TRACE && (applied > 0 || remaining > 0) {
            eprintln!("[inbox] events={applied} remaining={remaining} elapsed_us={}", started.elapsed().as_micros());
        }
        (changed, remaining > 0)
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
    fn parked_socket_is_positive_server_availability_evidence() {
        let mut app = crate::app::App::new();
        app.connection = crate::app::Connection::Connected;
        let mut state = super::State::from_app(app);
        assert!(state.server_connected());
        state.connection = crate::app::Connection::Disconnected("client disconnected".into());
        assert!(!state.server_connected());
    }

    #[test]
    fn git_panel_belongs_to_its_workspace() {
        let mut app = crate::app::App::new();
        crate::git::fixture(&mut app, false);
        assert!(app.git_panel.open);
        app.new_workspace();
        assert!(!app.git_panel.open);
        app.select_workspace(0);
        assert!(app.git_panel.open);
        app.select_workspace(1);
        assert!(!app.git_panel.open);
    }

    #[test]
    fn window_close_does_not_wait_for_a_stalled_cleanup_ack() {
        let mut app = crate::app::App::new();
        app.session_closed = true;
        let (ack, done) = std::sync::mpsc::channel();
        app.closing_connections.push(super::ClosingConnection {
            done, _link: Default::default(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
        });
        let started = std::time::Instant::now();
        app.close_window_resources();
        assert!(started.elapsed() < std::time::Duration::from_millis(200));
        assert!(app.resources_closed);
        assert!(app.closing_connections.is_empty());
        // The cleanup receiver still exists after returning to the window manager.
        ack.send(()).unwrap();
        app.close_window_resources(); // Rune may also invoke the quit hook.
    }

    #[test]
    fn workspace_chips_follow_the_restored_prompt_selection() {
        let mut app = crate::app::App::new();
        app.open_tab(crate::tabs::TabKind::Agent { session: None });
        app.composer.selection = crate::agent::slash::Selection {
            model: "openai/gpt-6.1-sol".into(), agent: "build".into(), thinking: "high".into(),
            connection_id: None,
        };
        app.sync_chips();
        let first = (app.model.clone(), app.agent.clone(), app.effort.clone());
        app.new_workspace();
        app.open_tab(crate::tabs::TabKind::Agent { session: None });
        app.composer.selection = crate::agent::slash::Selection {
            model: "openrouter/anthropic/claude".into(), agent: "plan".into(), thinking: "medium".into(),
            connection_id: None,
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

    fn delta(text: &str) -> crate::daemon::Inbound {
        crate::daemon::Inbound::Agent(Box::new(neoism_protocol::agent::AgentServerMessage::ContentDelta {
            session_id: "s".into(), message_id: "reply".into(), text: text.into(),
            kind: neoism_protocol::agent::ContentKind::Text, parent_message_id: Some("turn".into()),
        }))
    }

    #[test]
    fn event_slices_rotate_workspaces_without_reordering_or_switching_the_view() {
        use neoism_protocol::agent::AgentServerMessage as M;
        use crate::daemon::Inbound;
        let inbox = |events| std::sync::Arc::new(std::sync::Mutex::new(VecDeque::from(events)));
        let idle = || Inbound::Agent(Box::new(M::SessionIdle { session_id: "s".into() }));
        let mut app = App::new();
        let canonical = serde_json::from_value(json!({"id":"reply", "role":"Assistant", "kind":"Assistant", "text":"ab!"})).unwrap();
        let first = inbox(vec![delta("a"), delta("b"), Inbound::Agent(Box::new(M::MessageUpdated {
            session_id: "s".into(), message: canonical, parent_message_id: Some("turn".into()),
        })), delta("c"), idle()]);
        app.inbox = Some(first.clone());
        app.sessions.entry("s".into()).or_default().streaming = true;
        app.new_workspace();
        let selected = app.workspace_id.clone();
        let second = inbox(vec![delta("x"), delta("y"), idle()]);
        app.inbox = Some(second.clone());
        app.sessions.entry("s".into()).or_default().streaming = true;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        assert_eq!(app.drain_budgeted(deadline, 1), (false, true));
        assert_eq!(app.sessions["s"].messages[0].text, "x");
        assert_eq!(app.drain_budgeted(deadline, 1), (false, true));
        assert_eq!(app.workspaces[0].state.as_ref().unwrap().sessions["s"].messages[0].text, "a");
        for _ in 0..6 {
            app.drain_budgeted(deadline, 1);
            assert_eq!(app.workspace_id, selected);
        }
        let background = &app.workspaces[0].state.as_ref().unwrap().sessions["s"];
        assert_eq!(background.messages[0].text, "ab!c");
        assert!(!background.streaming);
        assert_eq!(app.sessions["s"].messages[0].text, "xy");
        assert!(!app.sessions["s"].streaming);
        assert!(first.lock().unwrap().is_empty());
        assert!(second.lock().unwrap().is_empty());
        assert_eq!(app.drain_budgeted(deadline, 1), (false, false));
    }

    #[test]
    fn elapsed_slice_yields_and_preserves_new_arrivals_behind_the_backlog() {
        let mut app = App::new();
        let inbox = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::from([delta("a"), delta("b")])));
        app.inbox = Some(inbox.clone());
        // An expired deadline deterministically permits one event, then yields.
        assert_eq!(app.drain_budgeted(std::time::Instant::now(), 256), (false, true));
        assert_eq!(app.sessions["s"].messages[0].text, "a");
        inbox.lock().unwrap().push_back(delta("c"));
        assert_eq!(app.drain_budgeted(std::time::Instant::now(), 256), (false, true));
        assert_eq!(app.sessions["s"].messages[0].text, "ab");
        assert_eq!(app.drain_budgeted(std::time::Instant::now(), 256), (false, false));
        assert_eq!(app.sessions["s"].messages[0].text, "abc");
    }

    #[test]
    fn ignored_event_backlogs_still_request_a_follow_up_poll() {
        let mut app = App::new();
        let event = || crate::daemon::Inbound::Agent(Box::new(neoism_protocol::agent::AgentServerMessage::SessionEvent {
            session_id: "s".into(), kind: "ignored.test".into(), properties: json!({}),
        }));
        app.inbox = Some(std::sync::Arc::new(std::sync::Mutex::new(VecDeque::from([event(), event()]))));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        assert_eq!(app.drain_budgeted(deadline, 1), (false, true));
        assert_eq!(app.drain_budgeted(deadline, 1), (false, false));
    }

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
        assert!(app.drain_budgeted(std::time::Instant::now() + std::time::Duration::from_secs(10), 64).0);
        assert_eq!(app.workspace_id, next);
        assert!(!app.buffers["shared.txt"].loaded);
    }

    #[test]
    fn a_windows_shells_directory_is_read_as_a_drive_path() {
        let workspace = "C:\\Users\\me";
        // OSC 7 on Windows, PowerShell or cmd: `file://host/C:/Users/me/proj`.
        assert_eq!(shell_directory("/C:/Users/me/proj", workspace), "C:\\Users\\me\\proj");
        // Git Bash / MSYS.
        assert_eq!(shell_directory("/c/Users/me/proj/", workspace), "C:\\Users\\me\\proj");
        assert_eq!(shell_directory("c:/Users/me", workspace), "C:\\Users\\me");
        assert_eq!(shell_directory("/d", workspace), "D:\\");
        // The same folder spelled another way is not a move.
        assert!(same_directory(&shell_directory("/c:/users/ME/", workspace), workspace));
        assert!(!same_directory("C:\\Users\\me\\proj", workspace));
        // A POSIX workspace is left exactly as it was: `/c/data` is a real folder there.
        assert_eq!(shell_directory("/c/data/", "/home/me"), "/c/data");
        assert_eq!(shell_directory("/", "/home/me"), "/");
        assert!(!same_directory("/home/Me", "/home/me"));
    }

    #[test]
    fn a_windows_shell_reporting_its_folder_does_not_empty_the_workspace() {
        let mut app = App::new();
        app.cwd = "C:\\Users\\me".into();
        let home = app.active_tab().id;
        app.terminals.get_mut(&home).unwrap().shell.session = Some("shell".into());
        app.terminals.get_mut(&home).unwrap().shell.cwd = "/C:/Users/me".into();
        assert!(!app.follow_main_terminal(), "the same folder: nothing is reset");
        assert_eq!(app.cwd, "C:\\Users\\me");
        app.terminals.get_mut(&home).unwrap().shell.cwd = "/C:/Users/me/proj".into();
        assert!(app.follow_main_terminal());
        assert_eq!(app.cwd, "C:\\Users\\me\\proj", "a real cd moves to a path the daemon can open");
    }

    #[test]
    fn the_workspace_moves_only_to_a_folder_the_daemon_has() {
        let mut app = App::new();
        app.cwd = "/home/me".into();
        let (daemon, _inbox) = Daemon::start_to(Some(crate::daemon::Target { url: "ws://127.0.0.1:9/session".into(), ..Default::default() }), || {});
        app.daemon = Some(daemon);
        let home = app.active_tab().id;
        app.terminals.get_mut(&home).unwrap().shell.session = Some("shell".into());
        // `ssh box` in the terminal: its shell reports a folder of that machine.
        app.terminals.get_mut(&home).unwrap().shell.cwd = "/srv/app".into();
        assert!(!app.follow_main_terminal(), "asked first, not taken on trust");
        assert_eq!(app.cwd, "/home/me");
        let (id, asked) = app.root_probe.clone().expect("the daemon is asked");
        assert_eq!(asked, "/srv/app");
        assert!(!app.follow_main_terminal());
        assert_eq!(app.root_probe.as_ref().map(|p| p.0), Some(id), "asked once");
        // The daemon has no such folder: the workspace stays, and is not asked again.
        assert!(app.root_probe_reply(id, &json!({"Error": {"message": "workspace_root is not a directory: /srv/app"}})));
        assert_eq!(app.cwd, "/home/me");
        assert!(!app.follow_main_terminal());
        assert!(app.root_probe.is_none());
        // A real `cd`: confirmed, then followed.
        app.terminals.get_mut(&home).unwrap().shell.cwd = "/home/me/proj".into();
        assert!(!app.follow_main_terminal());
        let (id, _) = app.root_probe.clone().unwrap();
        assert!(app.root_probe_reply(id, &json!({"Stat": {"entry": {}}})));
        assert_eq!(app.cwd, "/home/me/proj");
        // Another reply is not ours.
        assert!(!app.root_probe_reply(id + 100, &json!({"Stat": {}})));
    }

    #[test]
    fn the_filesystem_root_stays_a_root() {
        let mut app = App::new();
        let home = app.active_tab().id;
        app.terminals.get_mut(&home).unwrap().shell.session = Some("shell".into());
        app.terminals.get_mut(&home).unwrap().shell.cwd = "/".into();
        assert!(app.follow_main_terminal());
        assert_eq!(app.cwd, "/");
    }

    #[test]
    fn directory_choices_move_the_first_terminal_before_the_workspace_without_popups() {
        let path = "/new";
        for from_project_menu in [false, true] {
            let mut app = App::new();
            app.cwd = "/old".into();
            let home = app.active_tab().id;
            let term = app.terminals.get_mut(&home).unwrap();
            term.shell.session = Some("shell".into());
            term.shell.cwd = app.cwd.clone();
            let (daemon, _inbox) = Daemon::start_to(Some(crate::daemon::Target { url: "ws://127.0.0.1:9/session".into(), ..Default::default() }), || {});
            app.daemon = Some(daemon);
            app.workspace_ready = true;

            let all_projects = app.project_rows()[0].value.clone();
            app.activate_project(&all_projects);
            assert_eq!(app.cwd, "/old");
            assert!(app.pending_cd.is_none());
            if from_project_menu {
                app.activate_project(&format!("project:{path}"));
            } else {
                app.change_directory(path);
            }
            let Some(DirectoryChange::Shell { request_id, .. }) = &app.pending_cd else { panic!("both folder choices must try the live shell") };
            let request_id = request_id.clone();
            app.directory_changed("shell", &request_id, Ok(path));
            let Some(DirectoryChange::Workspace { request_id, .. }) = app.pending_cd else { panic!("the host should confirm the workspace") };
            assert_eq!(app.cwd, "/old");
            app.workspace_reply(request_id, &json!({"HostWorkspaceUpserted":{"workspace":{"id":app.workspace_id,"root_dir":path}}}));

            assert_eq!(app.cwd, path);
            assert!(!app.conv_all);
            assert_eq!(app.terminals[&home].shell.cwd, path);
            assert!(!app.follow_main_terminal());
            assert!(app.root_probe.is_none(), "the old terminal prompt cannot undo the selection");
            assert!(app.modal.is_none(), "a successful directory change should not interrupt the user");

            app.terminals.get_mut(&home).unwrap().shell.cwd = "/a-real-shell-cd".into();
            app.follow_main_terminal();
            let (id, _) = app.root_probe.clone().expect("a later shell cd is followed");
            app.root_probe_reply(id, &json!({"Stat":{}}));
            assert_eq!(app.cwd, "/a-real-shell-cd");
        }
    }

    #[test]
    fn a_refused_directory_change_keeps_the_current_root_without_a_popup() {
        let mut app = App::new();
        app.cwd = "/old".into();
        app.pending_cd = Some(DirectoryChange::Shell { request_id: "cd".into(), session_id: "shell".into() });
        app.directory_changed("shell", "cd", Err("Shell has unsubmitted input"));
        assert!(app.pending_cd.is_none());
        assert_eq!(app.cwd, "/old");
        assert!(app.modal.is_none(), "directory changes never open a popup, including failures");
    }
}
