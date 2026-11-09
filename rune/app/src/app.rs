//! The application state every view reads, and the daemon event loop that changes it. Feature
//! modules (`agent`, `home`, `tabs`) add their own `impl App` blocks next to their views.

use crate::agent::Session;
use crate::daemon::{Daemon, Family, Inbound, Inbox};
use crate::ui::status::GitSummary;
use neoism_protocol::git::GitRepoStatus;
use serde_json::json;
use crate::home::Terminal;
use crate::tabs::{Tab, TabKind};
use crate::ui::theme::{Palette, Type, palette};
use crate::wordmark::Wordmark;
use neoism_protocol::agent::ThreadSummary;
use neoism_protocol::pty::ServerMessage as PtyServer;
use rune::ImageCache;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub enum Connection {
    Connecting,
    Connected,
    Disconnected(String),
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
}

/// Window keyboard focus zones (`rune/README.md`, "Keyboard focus and the cursor").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Content,
    Sidebar,
    Buffers(usize),
    Workspaces(usize),
}

const GIT_WATCH_TOKEN: &str = "neoism-rune-git-status";

pub struct App {
    /// This person's presence name (their orb seed and message author).
    pub local_name: String,
    /// Presence orbs (profile pictures) shown so far.
    pub orbs: crate::ui::orb::Orbs,
    /// The transcript column's width, for user bubble geometry.
    pub timeline_width: f32,
    /// Wakes the UI thread (poll) from views and background work.
    pub waker: Option<rune::Waker>,
    /// The window cursor: focus bar on chrome, block caret in fields, one trail between them.
    pub cursor: crate::ui::cursor::Cursor,
    /// Laid-out rects of focusable chrome (`pane-tab:N`, `workspace-tab:N`), for the cursor.
    pub rects: HashMap<String, [f32; 4]>,
    /// Sub-agent families: child sessions, their roots and roster order.
    pub families: crate::agent::Families,
    pub header_hover: Option<crate::agent::HeaderStep>,
    /// Branch and line counts from the daemon's Git `RepoStatus` watch.
    pub git: Option<crate::ui::status::GitSummary>,
    pub splash: crate::home::SplashState,
    /// The open centered picker (`Alt+P`, themes), above everything else.
    pub picker: Option<crate::palette::Picker>,
    pub daemon: Option<Daemon>,
    pub inbox: Option<Inbox>,
    pub images: Option<ImageCache>,
    pub connection: Connection,
    pub pal: &'static Palette,
    pub ty: Type,
    pub workspace_name: String,
    pub cwd: String,
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
    /// Home block terminals by tab id, and the tabs whose `CreatePty` is still in flight.
    pub terminals: HashMap<u64, Terminal>,
    pub pending_pty: VecDeque<u64>,
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
            threads: Vec::new(),
            sessions: HashMap::new(),
            tabs: Vec::new(),
            active: 0,
            next_tab: 1,
            sidebar: None,
            overlay: None,
            focus: Focus::Content,
            agent: "Build".into(),
            model: crate::agent::title_case(config.model.as_deref().unwrap_or("openai/gpt-6.1-sol")),
            effort: crate::agent::title_case(config.variant.as_deref().unwrap_or("medium")),
            wordmark: Wordmark::default(),
            fps: None,
            scroll: Default::default(),
            wheel_target: None,
            picker: None,
            local_name: crate::ui::orb::local_display_name(config.display_name.as_deref()),
            orbs: Default::default(),
            timeline_width: 830.0,
            waker: None,
            cursor: Default::default(),
            rects: HashMap::new(),
            families: Default::default(),
            header_hover: None,
            // The local branch until the daemon's Git watch reports counts.
            git: crate::config::git_branch().map(|branch| crate::ui::status::GitSummary { branch, ..Default::default() }),
            splash: Default::default(),
            terminals: HashMap::new(),
            pending_pty: Default::default(),
        };
        app.open_tab(TabKind::Home);
        app
    }

    pub fn display_cwd(&self) -> String {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() && self.cwd.starts_with(&home) { format!("~{}", &self.cwd[home.len()..]) } else { self.cwd.clone() }
    }

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
                self.agent_connected();
                self.terminals_connected();
                self.watch_git();
            }
            Inbound::Disconnected(reason) => self.connection = Connection::Disconnected(reason),
            Inbound::Agent(message) => self.apply_agent(*message),
            Inbound::Reply { family, message, .. } if family == "Pty" => {
                if let Ok(typed) = serde_json::from_value::<PtyServer>(message) { self.apply_pty(typed); }
            }
            Inbound::Reply { family, message, .. } if family == "Git" => {
                if let Some(status) = message.get("RepoStatus") { self.apply_git_status(status); }
            }
            Inbound::Reply { .. } => {}
        }
    }

    /// Subscribe to the workspace's Git status (branch and changed line counts).
    fn watch_git(&self) {
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Git, json!({"WatchStatus": {"token": GIT_WATCH_TOKEN}}), &self.cwd);
        }
    }

    fn apply_git_status(&mut self, status: &serde_json::Value) {
        if status["token"] != GIT_WATCH_TOKEN { return; }
        let Ok(snapshot) = serde_json::from_value::<GitRepoStatus>(status["snapshot"].clone()) else { return };
        if snapshot.error.is_some() { return; }
        self.git = snapshot.branch.filter(|_| snapshot.repo_root.is_some()).map(|branch| GitSummary {
            branch,
            added: snapshot.files.iter().map(|f| u64::from(f.additions)).sum(),
            deleted: snapshot.files.iter().map(|f| u64::from(f.deletions)).sum(),
        });
    }
}
