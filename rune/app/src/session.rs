//! Window recovery (GPUiX's `session-store.ts`): every workspace's tabs, split tree, composer
//! drafts, sidebar and unsaved editor text survive a restart. The snapshot is written
//! atomically next to the other Neoism state, at most once a second and only when it changed;
//! startup rebuilds the workspaces from it before the daemon connects, so each one registers
//! with its restored root.
//!
//! Terminals come back as fresh shells in the same tabs (their programs do not survive the
//! app); conversations reload their history; files reopen from disk with any unsaved text
//! laid back over them.

use crate::app::{App, Sidebar};
use crate::panes::{Axis, Layout, Node, Pane};
use crate::tabs::{Tab, TabKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const VERSION: u32 = 1;
const INTERVAL: Duration = Duration::from_secs(1);

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Saved {
    pub version: u32,
    pub active: usize,
    pub workspaces: Vec<SavedWorkspace>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SavedWorkspace {
    pub name: String,
    pub cwd: String,
    pub tabs: Vec<SavedTab>,
    /// The split tree; panes name tabs by their index in `tabs`.
    pub layout: SavedNode,
    /// Index into `tabs` of the focused pane's tab.
    pub active: usize,
    #[serde(default)]
    pub sidebar: Option<String>,
    /// Unsaved editor text by workspace-relative path.
    #[serde(default)]
    pub unsaved: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedTab {
    Home { #[serde(default)] draft: String },
    Terminal { #[serde(default)] draft: String },
    Agent { session: Option<String>, #[serde(default)] draft: String },
    Editor { path: String },
    Database { path: String },
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedNode {
    Pane { tabs: Vec<usize>, active: usize, #[serde(default)] focused: bool },
    Split { horizontal: bool, children: Vec<SavedNode>, ratios: Vec<f32> },
}

/// Where the snapshot lives: `NEOISM_RUNE_STATE_PATH`, else `$XDG_STATE_HOME/neoism/rune/session.json`.
pub fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("NEOISM_RUNE_STATE_PATH") { return path.into(); }
    let base = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state"));
    base.join("neoism/rune/session.json")
}

/// Recovery runs with a daemon, or headless when a test names its own state file.
pub fn enabled() -> bool {
    std::env::var_os("NEOISM_RUNE_STATE_PATH").is_some()
        || (std::env::var_os("NEOISM_RUNE_OFFLINE").is_none() && std::env::var_os("NEOISM_RUNE_NO_RESTORE").is_none())
}

pub fn read() -> Option<Saved> {
    let saved: Saved = serde_json::from_slice(&std::fs::read(path()).ok()?).ok()?;
    (saved.version == VERSION && !saved.workspaces.is_empty()).then_some(saved)
}

fn write(raw: &str) -> std::io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let next = path.with_extension(format!("json.{}.next", std::process::id()));
    std::fs::write(&next, raw)?;
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o600))?; }
    std::fs::rename(next, path)
}

/// Throttles writes and skips unchanged snapshots.
#[derive(Default)]
pub struct Writer {
    last: String,
    at: Option<Instant>,
}

/// Capture one workspace from its parts (the active one lives on `App`, the rest in parked state).
pub fn capture_workspace(name: &str, cwd: &str, tabs: &[Tab], active: usize, layout: &Layout,
    sidebar: Option<Sidebar>, unsaved: BTreeMap<String, String>) -> SavedWorkspace {
    let index_of = |id: u64| tabs.iter().position(|t| t.id == id);
    fn node(n: &Node, focus: u64, index_of: &dyn Fn(u64) -> Option<usize>) -> SavedNode {
        match n {
            Node::Pane(p) => {
                let kept: Vec<(usize, usize)> = p.tabs.iter().enumerate().filter_map(|(slot, id)| index_of(*id).map(|i| (slot, i))).collect();
                let active = kept.iter().position(|(slot, _)| *slot == p.active).unwrap_or(0);
                SavedNode::Pane { tabs: kept.into_iter().map(|(_, i)| i).collect(), active, focused: p.id == focus }
            }
            Node::Split { axis, children, ratios } => SavedNode::Split {
                horizontal: *axis == Axis::Horizontal,
                children: children.iter().map(|c| node(c, focus, index_of)).collect(),
                ratios: ratios.clone(),
            },
        }
    }
    let tabs_saved = tabs.iter().map(|t| {
        let draft = t.draft.text.clone();
        match &t.kind {
            TabKind::Home => SavedTab::Home { draft },
            TabKind::Terminal => SavedTab::Terminal { draft },
            TabKind::Agent { session } => SavedTab::Agent { session: session.clone(), draft },
            TabKind::Editor { path } => SavedTab::Editor { path: path.clone() },
            TabKind::Database { path } => SavedTab::Database { path: path.clone() },
        }
    }).collect();
    let sidebar = sidebar.map(|s| match s { Sidebar::Conversations => "conversations", Sidebar::Files => "files", Sidebar::Notes => "notes" }.to_owned());
    SavedWorkspace { name: name.to_owned(), cwd: cwd.to_owned(), tabs: tabs_saved, layout: node(&layout.root, layout.focus, &index_of),
        active: active.min(tabs.len().saturating_sub(1)), sidebar, unsaved }
}

impl App {
    /// Unsaved text of the active workspace's loaded, dirty buffers.
    pub(crate) fn unsaved_buffers(buffers: &std::collections::HashMap<String, crate::editor::Buffer>) -> BTreeMap<String, String> {
        buffers.iter().filter(|(_, b)| b.loaded && b.dirty()).map(|(path, b)| (path.clone(), b.doc.lines.join("\n"))).collect()
    }

    pub fn capture_session(&self) -> Saved {
        let active = capture_workspace(&self.workspace_name, &self.cwd, &self.tabs, self.active, &self.layout,
            self.sidebar, App::unsaved_buffers(&self.buffers));
        // Only local workspaces come back next launch: one on another server needs its
        // connection (a tunnel, a password) made again.
        let local = |i: usize| self.workspaces.get(i).is_some_and(|entry| entry.server.is_none());
        let workspaces: Vec<SavedWorkspace> = if self.workspaces.is_empty() { vec![active] } else {
            let mut active = Some(active);
            self.workspaces.iter().enumerate().filter(|(i, _)| local(*i)).filter_map(|(i, entry)| {
                if i == self.workspace_active { active.take() } else { entry.state.as_ref().map(|s| s.saved()) }
            }).collect()
        };
        // The active tab's place among the saved ones (a remote tab: the local one before it).
        let before = (0..self.workspace_active.min(self.workspaces.len())).filter(|i| local(*i)).count();
        Saved { version: VERSION, active: before.min(workspaces.len().saturating_sub(1)), workspaces }
    }

    /// Write the snapshot when it changed, at most once a second (`force` on quit). A throttled
    /// call returns how long until the window opens again, so the caller can wake then and the
    /// last change before going idle is still written.
    pub fn save_session(&mut self, force: bool) -> Option<Duration> {
        if !enabled() { return None; }
        if let Some(wait) = self.session_writer.at.map(|at| INTERVAL.saturating_sub(at.elapsed())).filter(|w| !force && !w.is_zero()) {
            return Some(wait);
        }
        self.session_writer.at = Some(Instant::now());
        let saved = self.capture_session();
        // Nothing local is open (every tab is on a server): the last local window stays saved.
        if saved.workspaces.is_empty() { return None; }
        let Ok(raw) = serde_json::to_string(&saved) else { return None };
        if raw == self.session_writer.last { return None; }
        match write(&raw) {
            Ok(()) => self.session_writer.last = raw,
            Err(error) => eprintln!("neoism-rune: could not save the session: {error}"),
        }
        None
    }

    /// Rebuild the window from a snapshot: the first workspace replaces the starting one, the
    /// rest open after it, then the saved one is selected.
    pub fn restore_session(&mut self, saved: Saved) {
        for (index, workspace) in saved.workspaces.into_iter().enumerate() {
            if index > 0 { self.new_workspace(); }
            self.restore_workspace(workspace);
        }
        if self.workspaces.len() > 1 { self.select_workspace(saved.active.min(self.workspaces.len() - 1)); }
        self.session_writer.last = serde_json::to_string(&self.capture_session()).unwrap_or_default();
    }

    fn restore_workspace(&mut self, saved: SavedWorkspace) {
        // A saved window may remember a launcher's folder (`/`, the app bundle, System32) from
        // before the app mapped those to home: it is not a project either (`recoveredStartupDirectory`).
        if !saved.cwd.is_empty() && std::path::Path::new(&saved.cwd).is_dir() && !crate::startup::launcher_directory(std::path::Path::new(&saved.cwd)) {
            self.cwd = saved.cwd;
            self.workspace_name = saved.name;
        }
        // Start from nothing: the opening Home tab and its shell are replaced.
        for id in self.tabs.iter().map(|t| t.id).collect::<Vec<_>>() { self.terminals.remove(&id); }
        self.tabs.clear();
        self.layout = Layout::default();
        let mut ids = Vec::with_capacity(saved.tabs.len());
        for tab in saved.tabs {
            let (kind, draft) = match tab {
                SavedTab::Home { draft } => (TabKind::Home, draft),
                SavedTab::Terminal { draft } => (TabKind::Terminal, draft),
                SavedTab::Agent { session, draft } => (TabKind::Agent { session }, draft),
                SavedTab::Editor { path } => {
                    self.open_file(&path, None);
                    if let Some(text) = saved.unsaved.get(&path) {
                        if let Some(buffer) = self.buffers.get_mut(&path) { buffer.recovered = Some(text.clone()); }
                    }
                    ids.push(self.tabs.last().map(|t| t.id));
                    continue;
                }
                SavedTab::Database { path } => {
                    self.open_database(&path);
                    ids.push(self.tabs.last().map(|t| t.id));
                    continue;
                }
            };
            let session = match &kind { TabKind::Agent { session: Some(id) } => Some(id.clone()), _ => None };
            self.open_tab(kind);
            if let Some(session) = session { self.load_session_history(&session); }
            if let Some(tab) = self.tabs.last_mut() { tab.draft.replace_text(&draft); }
            ids.push(self.tabs.last().map(|t| t.id));
        }
        if self.tabs.is_empty() { self.open_tab(TabKind::Home); }
        if let Some(layout) = rebuild(&saved.layout, &ids) { self.layout = layout; }
        // Every tab sits in some pane even if the saved tree lost it.
        for tab in self.tabs.iter().map(|t| t.id).collect::<Vec<_>>() {
            if !self.layout.panes().iter().any(|p| p.tabs.contains(&tab)) { self.layout.open(tab); }
        }
        let focused = self.layout.focused().active_tab();
        self.active = focused.and_then(|id| self.tabs.iter().position(|t| t.id == id))
            .unwrap_or(saved.active.min(self.tabs.len() - 1));
        self.sidebar = match saved.sidebar.as_deref() {
            Some("conversations") => Some(Sidebar::Conversations),
            Some("files") => Some(Sidebar::Files),
            Some("notes") => Some(Sidebar::Notes),
            _ => None,
        };
        self.tabs.iter_mut().enumerate().for_each(|(i, t)| t.draft.focused = i == self.active);
    }
}

/// The saved tree with tab indices mapped to the new tab ids; `None` when nothing survives.
fn rebuild(saved: &SavedNode, ids: &[Option<u64>]) -> Option<Layout> {
    let mut next = 1;
    let mut focus = None;
    fn walk(n: &SavedNode, ids: &[Option<u64>], next: &mut u64, focus: &mut Option<u64>) -> Option<Node> {
        match n {
            SavedNode::Pane { tabs, active, focused } => {
                let kept: Vec<(usize, u64)> = tabs.iter().enumerate().filter_map(|(slot, i)| ids.get(*i).copied().flatten().map(|id| (slot, id))).collect();
                if kept.is_empty() { return None; }
                let active = kept.iter().position(|(slot, _)| slot == active).unwrap_or(0);
                let id = *next;
                *next += 1;
                if *focused || focus.is_none() { *focus = Some(id); }
                Some(Node::Pane(Pane { id, tabs: kept.into_iter().map(|(_, id)| id).collect(), active }))
            }
            SavedNode::Split { horizontal, children, ratios } => {
                let kept: Vec<Node> = children.iter().filter_map(|c| walk(c, ids, next, focus)).collect();
                match kept.len() {
                    0 => None,
                    1 => kept.into_iter().next(),
                    n => {
                        // A lost child takes its ratios with it: share the space evenly then.
                        let ratios = if n == children.len() { ratios.clone() } else { (1..n).map(|i| i as f32 / n as f32).collect() };
                        Some(Node::Split { axis: if *horizontal { Axis::Horizontal } else { Axis::Vertical }, children: kept, ratios })
                    }
                }
            }
        }
    }
    let root = walk(saved, ids, &mut next, &mut focus)?;
    Some(Layout::from_parts(root, focus?, next))
}

impl crate::workspace::State {
    pub(crate) fn saved(&self) -> SavedWorkspace { self.capture() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_tree_round_trips_through_tab_indices() {
        let saved = SavedNode::Split { horizontal: true, ratios: vec![0.3], children: vec![
            SavedNode::Pane { tabs: vec![0, 1], active: 1, focused: false },
            SavedNode::Pane { tabs: vec![2], active: 0, focused: true },
        ] };
        let layout = rebuild(&saved, &[Some(10), Some(11), Some(12)]).expect("layout");
        let panes = layout.panes();
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].tabs, vec![10, 11]);
        assert_eq!(panes[0].active, 1);
        assert_eq!(layout.focused().tabs, vec![12]);
        let tabs: Vec<Tab> = [10, 11, 12].iter().map(|id| Tab { id: *id, kind: TabKind::Home, draft: rune::TextEdit::new(""), pending_prompt: None }).collect();
        let back = capture_workspace("w", "/", &tabs, 2, &layout, None, BTreeMap::new());
        assert_eq!(back.layout, saved);
    }

    #[test]
    fn unsaved_text_is_laid_over_the_reopened_file() {
        let mut app = App::new();
        app.open_file("a.rs", None);
        app.buffers.get_mut("a.rs").unwrap().recovered = Some("fn edited() {}\n".into());
        app.apply_opened_file("a.rs", b"fn original() {}\n");
        let buffer = &app.buffers["a.rs"];
        assert_eq!(buffer.doc.lines[0], "fn edited() {}");
        assert!(buffer.dirty(), "recovered edits stay unsaved against the disk text");
    }

    #[test]
    fn workspaces_and_their_tabs_round_trip() {
        let mut app = App::new();
        app.open_tab(TabKind::Agent { session: Some("s1".into()) });
        app.tabs.last_mut().unwrap().draft.replace_text("half a prompt");
        app.new_workspace();
        app.open_tab(TabKind::Terminal);
        let saved = app.capture_session();
        assert_eq!(saved.workspaces.len(), 2);
        assert_eq!(saved.active, 1);
        let raw = serde_json::to_string(&saved).unwrap();
        let mut fresh = App::new();
        fresh.restore_session(serde_json::from_str(&raw).unwrap());
        assert_eq!(fresh.workspaces.len(), 2);
        assert_eq!(fresh.workspace_active, 1);
        assert_eq!(fresh.capture_session(), saved);
    }

    #[test]
    fn a_lost_tab_collapses_its_split() {
        let saved = SavedNode::Split { horizontal: false, ratios: vec![0.5], children: vec![
            SavedNode::Pane { tabs: vec![0], active: 0, focused: true },
            SavedNode::Pane { tabs: vec![1], active: 0, focused: false },
        ] };
        let layout = rebuild(&saved, &[Some(5), None]).expect("layout");
        assert_eq!(layout.panes().len(), 1);
        assert_eq!(layout.focused().tabs, vec![5]);
    }
}
