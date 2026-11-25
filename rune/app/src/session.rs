//! Window recovery (GPUiX's `session-store.ts`): every workspace's tabs, split tree, composer
//! drafts, sidebar and unsaved editor text survive a restart. The snapshot is written
//! atomically next to the other Neoism state, at most once a second and only when it changed.
//! Each live window exclusively claims its own snapshot; a normal launch restores the newest
//! unclaimed window, while --new-window starts fresh. Old clean snapshots are bounded on disk.
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
use std::path::Path;
use std::fs::{File, OpenOptions};
use std::time::{Duration, Instant};

const VERSION: u32 = 1;
const INTERVAL: Duration = Duration::from_secs(1);
const RECENT_WINDOWS: usize = 20;

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
    Agent { session: Option<String>, #[serde(default)] draft: String, #[serde(default, skip_serializing_if = "Vec::is_empty")] pills: Vec<SavedPill> },
    Editor { path: String },
    Database { path: String },
    GitDiff,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct SavedPill {
    start: usize,
    end: usize,
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    payload: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedNode {
    Pane { tabs: Vec<usize>, active: usize, #[serde(default)] focused: bool },
    Split { horizontal: bool, children: Vec<SavedNode>, ratios: Vec<f32> },
}

/// Legacy snapshot and recovery namespace: additional windows live in `session.windows/`.
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

fn read(path: &Path) -> Option<Saved> {
    let saved: Saved = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (saved.version == VERSION && !saved.workspaces.is_empty()).then_some(saved)
}

fn write(path: &Path, raw: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let next = path.with_extension(format!("json.{}.next", std::process::id()));
    std::fs::write(&next, raw)?;
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o600))?; }
    std::fs::rename(next, path)
}

/// Disk recovery is not a live daemon workspace. Locks are held for the window lifetime,
/// and released by the OS even after a crash. The index lock serializes claiming and pruning.
struct Recovery {
    path: PathBuf,
    _lock: File,
}

fn lock_file(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; file.set_permissions(std::fs::Permissions::from_mode(0o600))?; }
    Ok(file)
}

impl Saved {
    fn has_unsaved_work(&self) -> bool {
        self.workspaces.iter().any(|w| !w.unsaved.is_empty() || w.tabs.iter().any(|tab| match tab {
            SavedTab::Home { draft } | SavedTab::Terminal { draft } | SavedTab::Agent { draft, .. } => !draft.is_empty(),
            _ => false,
        }))
    }
}

impl Recovery {
    fn open(base: &Path, fresh: bool) -> std::io::Result<(Self, Option<Saved>)> {
        let dir = base.with_extension("windows");
        std::fs::create_dir_all(&dir)?;
        let index = lock_file(&dir.join("index.lock"))?;
        index.lock()?;
        let mut paths = vec![base.to_path_buf()]; // Migrate the shipped single-window snapshot in place.
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json") { paths.push(path.clone()); }
            // Explicitly closed windows leave only a lock file. Reap it under the index
            // lock, but never unlink a lock that a live window still owns.
            if path.extension().is_some_and(|ext| ext == "lock") && path.file_name().is_some_and(|n| n != "index.lock")
                && !path.with_extension("json").exists() {
                let lock = lock_file(&path)?;
                if lock.try_lock().is_ok() {
                    drop(lock);
                    std::fs::remove_file(path)?;
                }
            }
        }
        paths.sort_by_key(|path| std::cmp::Reverse(std::fs::metadata(path).and_then(|m| m.modified()).ok()));
        let mut selected = None;
        let mut clean = 0;
        for path in paths {
            let lock_path = path.with_extension("lock");
            let lock = lock_file(&lock_path)?;
            match lock.try_lock() {
                Ok(()) => {},
                Err(std::fs::TryLockError::WouldBlock) => continue,
                Err(std::fs::TryLockError::Error(error)) => return Err(error),
            }
            let Some(saved) = read(&path) else { continue };
            if !fresh && selected.is_none() {
                selected = Some((Self { path, _lock: lock }, Some(saved)));
                continue;
            }
            // Never age out drafts or unsaved files. Active windows were skipped above.
            if !saved.has_unsaved_work() {
                clean += 1;
                if clean >= RECENT_WINDOWS {
                    std::fs::remove_file(&path)?;
                    drop(lock);
                    std::fs::remove_file(lock_path)?;
                }
            }
        }
        if let Some(selected) = selected { return Ok(selected); }
        let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
        let lock = lock_file(&path.with_extension("lock"))?;
        lock.lock()?;
        Ok((Self { path, _lock: lock }, None))
    }
}

/// Throttles writes and skips unchanged snapshots.
#[derive(Default)]
pub struct Writer {
    last: String,
    at: Option<Instant>,
    recovery: Option<Recovery>,
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
            TabKind::Agent { session } => SavedTab::Agent {
                session: session.clone(), draft,
                pills: t.draft.pills().filter(|(_, id)| id.starts_with("paste:") || id.starts_with("skill:"))
                    .map(|(r, id)| SavedPill { start: r.start, end: r.end, id: id.into(), payload: t.pastes.get(id).cloned() }).collect(),
            },
            TabKind::Editor { path } | TabKind::Image { path } => SavedTab::Editor { path: path.clone() },
            TabKind::Database { path } => SavedTab::Database { path: path.clone() },
            TabKind::GitDiff => SavedTab::GitDiff,
        }
    }).collect();
    let sidebar = sidebar.map(|s| match s { Sidebar::Conversations => "conversations", Sidebar::Files => "files", Sidebar::Notes => "notes" }.to_owned());
    SavedWorkspace { name: name.to_owned(), cwd: cwd.to_owned(), tabs: tabs_saved, layout: node(&layout.root, layout.focus, &index_of),
        active: active.min(tabs.len().saturating_sub(1)), sidebar, unsaved }
}

impl App {
    pub fn recover_session(&mut self, fresh: bool) {
        if !enabled() { return; }
        match Recovery::open(&path(), fresh) {
            Ok((recovery, saved)) => {
                self.session_writer.recovery = Some(recovery);
                if let Some(saved) = saved { self.restore_session(saved); }
            }
            Err(error) => eprintln!("neoism-rune: could not claim window recovery: {error}"),
        }
    }

    pub(crate) fn forget_session(&mut self) {
        if let Some(recovery) = &self.session_writer.recovery {
            let _ = std::fs::remove_file(&recovery.path);
        }
    }

    /// Unsaved text of the active workspace's loaded, dirty buffers.
    pub(crate) fn unsaved_buffers(buffers: &std::collections::HashMap<String, crate::editor::Buffer>) -> BTreeMap<String, String> {
        buffers.iter().filter_map(|(path, b)| {
            let text = b.recovered.clone().or_else(|| b.dirty().then(|| b.doc.lines.join("\n")))?;
            Some((path.clone(), text))
        }).collect()
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
        if !enabled() || self.session_closed || self.session_writer.recovery.is_none() { return None; }
        if let Some(wait) = self.session_writer.at.map(|at| INTERVAL.saturating_sub(at.elapsed())).filter(|w| !force && !w.is_zero()) {
            return Some(wait);
        }
        self.session_writer.at = Some(Instant::now());
        let saved = self.capture_session();
        // Nothing local is open (every tab is on a server): the last local window stays saved.
        if saved.workspaces.is_empty() { return None; }
        let Ok(raw) = serde_json::to_string(&saved) else { return None };
        if raw == self.session_writer.last { return None; }
        match write(&self.session_writer.recovery.as_ref().unwrap().path, &raw) {
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
            let pills = match &tab { SavedTab::Agent { pills, .. } => pills.clone(), _ => Vec::new() };
            let (kind, draft) = match tab {
                SavedTab::Home { draft } => (TabKind::Home, draft),
                SavedTab::Terminal { draft } => (TabKind::Terminal, draft),
                SavedTab::Agent { session, draft, .. } => (TabKind::Agent { session }, draft),
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
                SavedTab::GitDiff => {
                    self.open_tab(TabKind::GitDiff);
                    self.git_panel.open = true;
                    ids.push(self.tabs.last().map(|t| t.id));
                    continue;
                }
            };
            let session = match &kind { TabKind::Agent { session: Some(id) } => Some(id.clone()), _ => None };
            self.open_tab(kind);
            if let Some(session) = session { self.load_session_history(&session); }
            if let Some(tab) = self.tabs.last_mut() {
                tab.draft.replace_text(&draft);
                tab.draft.set_marks(pills.iter().map(|p| rune::MarkRange { start: p.start, end: p.end, mark: rune::Mark::Pill(p.id.clone()) }).collect());
                tab.pastes = pills.into_iter().filter_map(|p| p.payload.map(|payload| (p.id, payload))).collect();
            }
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
        self.tabs.iter_mut().enumerate().for_each(|(i, t)| t.draft.focused = i == self.active && !cfg!(target_os = "ios"));
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
    fn live_windows_are_never_restored_or_overwritten_and_closed_windows_recover() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("session.json");
        let mut app = App::new();
        app.open_tab(TabKind::Agent { session: Some("remember-me".into()) });
        app.tabs.last_mut().unwrap().draft.replace_text("unfinished prompt");
        let raw = serde_json::to_string(&app.capture_session()).unwrap();
        write(&base, &raw).unwrap();
        let (first, saved) = Recovery::open(&base, false).unwrap();
        assert_eq!(saved.unwrap(), app.capture_session(), "legacy recovery is retained");
        let (second, saved) = Recovery::open(&base, false).unwrap();
        assert!(saved.is_none(), "a live window must not be cloned");
        assert_ne!(first.path, second.path);
        write(&second.path, &serde_json::to_string(&App::new().capture_session()).unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&first.path).unwrap(), raw);
        let first_path = first.path.clone();
        drop(first); // Also models the OS releasing a crashed process's lock.
        let (reopened, saved) = Recovery::open(&base, false).unwrap();
        assert_eq!(reopened.path, first_path);
        assert_eq!(saved.unwrap(), app.capture_session());
        drop(reopened);
        let (fresh, saved) = Recovery::open(&base, true).unwrap();
        assert!(saved.is_none(), "--new-window ignores closed recovery too");
        assert_ne!(fresh.path, first_path);
    }

    #[test]
    fn repeated_fresh_windows_bound_clean_recovery_but_preserve_unsaved_work() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("session.json");
        let mut app = App::new();
        app.open_file("edited.rs", None);
        app.buffers.get_mut("edited.rs").unwrap().recovered = Some("unsaved edit".into());
        let (protected, _) = Recovery::open(&base, true).unwrap();
        let protected_path = protected.path.clone();
        write(&protected.path, &serde_json::to_string(&app.capture_session()).unwrap()).unwrap();
        drop(protected);
        let clean = serde_json::to_string(&App::new().capture_session()).unwrap();
        for _ in 0..(RECENT_WINDOWS * 3) {
            let (window, _) = Recovery::open(&base, true).unwrap();
            write(&window.path, &clean).unwrap();
        }
        let entries: Vec<_> = std::fs::read_dir(base.with_extension("windows")).unwrap().map(|e| e.unwrap().path()).collect();
        assert!(entries.iter().filter(|p| p.extension().is_some_and(|e| e == "json")).count() <= RECENT_WINDOWS + 1);
        assert!(entries.len() <= (RECENT_WINDOWS + 1) * 2 + 1, "lock files must be bounded too");
        let saved = read(&protected_path).expect("unsaved recovery is never pruned");
        assert_eq!(saved.workspaces[0].unsaved["edited.rs"], "unsaved edit");
        assert!(saved.has_unsaved_work());
        app.open_tab(TabKind::Agent { session: None });
        app.tabs.last_mut().unwrap().draft.replace_text("draft");
        app.buffers.clear();
        assert!(app.capture_session().has_unsaved_work(), "drafts alone prevent eviction");
    }

    #[test]
    fn explicit_close_removes_only_its_snapshot_and_orphan_locks_are_reaped() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("session.json");
        let (first, _) = Recovery::open(&base, true).unwrap();
        let (second, _) = Recovery::open(&base, true).unwrap();
        let raw = serde_json::to_string(&App::new().capture_session()).unwrap();
        write(&first.path, &raw).unwrap();
        write(&second.path, &raw).unwrap();
        let lock_path = first.path.with_extension("lock");
        let mut app = App::new();
        app.session_writer.recovery = Some(first);
        app.forget_session();
        assert!(read(&second.path).is_some());
        drop(app);
        let (_next, _) = Recovery::open(&base, true).unwrap();
        assert!(!lock_path.exists());
        assert!(read(&second.path).is_some());
    }

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
        let tabs: Vec<Tab> = [10, 11, 12].iter().map(|id| Tab { id: *id, kind: TabKind::Home, draft: rune::TextEdit::new(""), pastes: Default::default(), pending_prompt: None, selection: None }).collect();
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
    fn compact_paste_payload_and_skill_pills_survive_session_restore() {
        let mut app = App::new();
        app.open_tab(TabKind::Agent { session: None });
        app.insert_skill_pill("review", None);
        let payload = "restored pasted text\n".repeat(100);
        assert!(app.paste_composer_text(&payload));
        let visible = app.active_tab().draft.text.clone();
        let expanded = app.expanded_composer_text(app.active);
        let raw = serde_json::to_string(&app.capture_session()).unwrap();
        let mut fresh = App::new();
        fresh.restore_session(serde_json::from_str(&raw).unwrap());
        assert_eq!(fresh.active_tab().draft.text, visible);
        assert_eq!(fresh.active_tab().draft.pills().count(), 2);
        assert_eq!(fresh.expanded_composer_text(fresh.active), expanded);
    }

    #[test]
    fn restored_home_has_a_block_terminal_before_the_daemon_connects() {
        let mut app = App::new();
        app.open_tab(TabKind::Agent { session: None });
        app.open_tab(TabKind::Agent { session: None });
        let home = app.tabs[0].id;
        app.select_tab(home);
        let saved = app.capture_session();
        let mut fresh = App::new();
        fresh.restore_session(saved);
        assert!(fresh.daemon.is_none());
        assert_eq!(fresh.active_tab().kind, TabKind::Home);
        assert_eq!(fresh.layout.focused().active_tab(), Some(fresh.active_tab().id));
        let term = fresh.terminals.get(&fresh.active_tab().id).expect("restored Home shell");
        assert!(!term.raw(), "Home starts as blocks, not an empty PTY grid");
        assert!(!term.grid);
        assert!(term.shell.session.is_none());
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
