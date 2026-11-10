//! The tree sidebars: Files (`Alt+E`, `reference/17-file-tree.png`) over the workspace and Notes
//! (`Alt+N`, `reference/19-notes.png`) over the notes vault (`shared/src/panels/file_tree`,
//! `notes_sidebar.rs`). Listed lazily over the daemon's `Files` family, folders first, expanded in
//! place. Keys: j/k or arrows move, l/Right/Enter open or expand, h/Left collapse.

pub mod view;

pub use view::view;

use crate::app::{App, Focus, Sidebar};
use crate::daemon::Family;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// One visible row: workspace-relative path, depth, entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: String,
    pub depth: usize,
    pub entry: Entry,
}

#[derive(Default)]
pub struct Tree {
    /// Absolute root for trees outside the workspace (the notes vault); its listings are
    /// requested by absolute path, which is also how their replies are told apart.
    pub root: Option<String>,
    /// Listings by directory relative to the root ("" is the root).
    listings: HashMap<String, Vec<Entry>>,
    pub open: HashSet<String>,
    requested: HashSet<String>,
    pub selected: usize,
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_owned() } else { format!("{dir}/{name}") }
}

impl Tree {
    /// Visible rows in display order: each open folder's children under it.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        self.push_rows("", 0, &mut rows);
        rows
    }

    fn push_rows(&self, dir: &str, depth: usize, rows: &mut Vec<Row>) {
        let Some(entries) = self.listings.get(dir) else { return };
        for entry in entries {
            let path = join(dir, &entry.name);
            let open = entry.is_dir && self.open.contains(&path);
            rows.push(Row { path: path.clone(), depth, entry: entry.clone() });
            if open { self.push_rows(&path, depth + 1, rows); }
        }
    }

    pub fn loaded(&self) -> bool { self.listings.contains_key("") }

    fn set_listing(&mut self, dir: String, mut entries: Vec<Entry>) {
        // Folders first, then names case-insensitively (dot entries sort first naturally).
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        self.requested.remove(&dir);
        self.listings.insert(dir, entries);
    }
}

impl Tree {
    /// The path a listing request names for `dir` (absolute under an absolute root).
    fn request_path(&self, dir: &str) -> String {
        match &self.root { Some(root) if dir.is_empty() => root.clone(), Some(root) => format!("{root}/{dir}"), None => dir.to_owned() }
    }

    /// `dir` relative to this tree's root, when a listing reply for `path` belongs here.
    fn own(&self, path: &str) -> Option<String> {
        let path = path.trim_end_matches('/');
        match &self.root {
            Some(root) => path.strip_prefix(root.as_str()).map(|rest| rest.trim_start_matches('/').to_owned()),
            None if path.starts_with('/') => None,
            None => Some(match path.trim_start_matches("./") { "." => String::new(), rest => rest.to_owned() }),
        }
    }

    /// Absolute or workspace-relative path of a row, as editors open it.
    pub fn open_path(&self, path: &str) -> String {
        match &self.root { Some(root) => format!("{root}/{path}"), None => path.to_owned() }
    }
}

impl App {
    fn tree_of(&mut self, sidebar: Sidebar) -> &mut Tree {
        if sidebar == Sidebar::Notes { &mut self.notes } else { &mut self.tree }
    }

    /// The tree the visible sidebar shows, if it is one.
    pub fn active_tree(&self) -> Option<&Tree> {
        match self.sidebar { Some(Sidebar::Files) => Some(&self.tree), Some(Sidebar::Notes) => Some(&self.notes), _ => None }
    }

    fn list_dir(&mut self, sidebar: Sidebar, dir: &str) {
        let tree = self.tree_of(sidebar);
        if !tree.requested.insert(dir.to_owned()) { return; }
        let path = tree.request_path(dir);
        let scope = tree.root.clone().unwrap_or_else(|| self.cwd.clone());
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Files, serde_json::json!({"ListDir": {"path": path}}), &scope);
        }
    }

    /// `Alt+E` / `Alt+N` and the rail buttons: show a tree sidebar (focused) or hide it.
    pub fn toggle_tree(&mut self, sidebar: Sidebar) {
        if self.sidebar == Some(sidebar) {
            self.sidebar = None;
            if self.focus == Focus::Sidebar { self.focus = Focus::Content; }
            return;
        }
        self.sidebar = Some(sidebar);
        self.focus = Focus::Sidebar;
        match sidebar {
            Sidebar::Notes if self.notes.root.is_none() => {
                // The vault is resolved per workspace; its listing follows `NotesVaultResolved`.
                if let Some(daemon) = &self.daemon {
                    daemon.send(Family::Workspace, serde_json::json!({"ResolveNotesVault": {"root_dir": self.cwd}}));
                }
            }
            _ if !self.tree_of(sidebar).loaded() => self.list_dir(sidebar, ""),
            _ => {}
        }
    }

    pub fn toggle_file_tree(&mut self) { self.toggle_tree(Sidebar::Files); }

    pub(crate) fn apply_notes_vault(&mut self, dir: &str) {
        self.notes.root = Some(dir.trim_end_matches('/').to_owned());
        self.list_dir(Sidebar::Notes, "");
    }

    pub(crate) fn apply_dir_listing(&mut self, path: &str, entries: &serde_json::Value) {
        let entries: Vec<Entry> = entries.as_array().into_iter().flatten().filter_map(|e| {
            Some(Entry { name: e["name"].as_str()?.to_owned(), is_dir: e["is_dir"].as_bool().unwrap_or(false) })
        }).collect();
        // A notes tree claims only replies under its resolved vault; the rest are the workspace's.
        let tree = if self.notes.root.is_some() && self.notes.own(path).is_some() { &mut self.notes } else { &mut self.tree };
        if let Some(dir) = tree.own(path) { tree.set_listing(dir, entries); }
    }

    fn sidebar_tree(&mut self) -> Option<(Sidebar, &mut Tree)> {
        let sidebar = self.sidebar.filter(|s| matches!(s, Sidebar::Files | Sidebar::Notes))?;
        Some((sidebar, self.tree_of(sidebar)))
    }

    pub fn tree_move(&mut self, delta: isize) {
        let Some((_, tree)) = self.sidebar_tree() else { return };
        let count = tree.rows().len();
        if count == 0 { return; }
        tree.selected = (tree.selected as isize + delta).clamp(0, count as isize - 1) as usize;
        self.tree_follow = true;
    }

    /// Open the selected file, or expand/collapse the selected folder.
    pub fn tree_activate(&mut self, index: Option<usize>) {
        let Some((sidebar, tree)) = self.sidebar_tree() else { return };
        if let Some(index) = index { tree.selected = index; }
        let Some(row) = tree.rows().get(tree.selected).cloned() else { return };
        if row.entry.is_dir {
            if !tree.open.remove(&row.path) {
                tree.open.insert(row.path.clone());
                self.list_dir(sidebar, &row.path);
            }
        } else {
            let path = tree.open_path(&row.path);
            self.open_file(&path, None);
        }
    }

    /// Collapse the selected folder, or step out to its parent.
    pub fn tree_collapse(&mut self) {
        let Some((_, tree)) = self.sidebar_tree() else { return };
        let rows = tree.rows();
        let Some(row) = rows.get(tree.selected) else { return };
        if row.entry.is_dir && tree.open.remove(&row.path) { return; }
        let Some((parent, _)) = row.path.rsplit_once('/') else { return };
        if let Some(index) = rows.iter().position(|r| r.path == parent) {
            tree.selected = index;
            tree.open.remove(parent);
            self.tree_follow = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> Entry { Entry { name: name.into(), is_dir } }

    #[test]
    fn folders_first_and_open_folders_nest() {
        let mut tree = Tree::default();
        tree.set_listing(String::new(), vec![entry("b.rs", false), entry("src", true), entry(".cargo", true), entry("A.md", false)]);
        let names: Vec<String> = tree.rows().iter().map(|r| r.path.clone()).collect();
        assert_eq!(names, [".cargo", "src", "A.md", "b.rs"]);
        tree.open.insert("src".into());
        tree.set_listing("src".into(), vec![entry("main.rs", false)]);
        let rows = tree.rows();
        assert_eq!(rows[2].path, "src/main.rs");
        assert_eq!(rows[2].depth, 1);
    }

    #[test]
    fn listings_route_to_their_tree() {
        let notes = Tree { root: Some("/vault".into()), ..Default::default() };
        let files = Tree::default();
        assert_eq!(notes.own("/vault"), Some(String::new()));
        assert_eq!(notes.own("/vault/a"), Some("a".into()));
        assert_eq!(notes.own("src"), None);
        assert_eq!(files.own("src"), Some("src".into()));
        assert_eq!(files.own("/vault/a"), None);
        assert_eq!(notes.request_path("a"), "/vault/a");
    }
}
