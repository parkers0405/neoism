//! The file tree sidebar (`Alt+E`, `reference/17-file-tree.png`; `shared/src/panels/file_tree`):
//! the workspace listed lazily over the daemon's `Files` family, folders first, expanded in place.
//! Keys are the original's: j/k or arrows move, l/Right/Enter open or expand, h/Left collapse.

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
    /// Listings by workspace-relative directory ("" is the root).
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

impl App {
    fn list_dir(&mut self, dir: &str) {
        if !self.tree.requested.insert(dir.to_owned()) { return; }
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Files, serde_json::json!({"ListDir": {"path": dir}}), &self.cwd);
        }
    }

    /// `Alt+E` and the rail button: show the tree (focused) or hide it.
    pub fn toggle_file_tree(&mut self) {
        if self.sidebar == Some(Sidebar::Files) {
            self.sidebar = None;
            if self.focus == Focus::Sidebar { self.focus = Focus::Content; }
            return;
        }
        self.sidebar = Some(Sidebar::Files);
        self.focus = Focus::Sidebar;
        if !self.tree.loaded() { self.list_dir(""); }
    }

    pub(crate) fn apply_dir_listing(&mut self, path: &str, entries: &serde_json::Value) {
        let entries = entries.as_array().into_iter().flatten().filter_map(|e| {
            Some(Entry { name: e["name"].as_str()?.to_owned(), is_dir: e["is_dir"].as_bool().unwrap_or(false) })
        }).collect();
        let dir = path.trim_start_matches("./").trim_end_matches('/').to_owned();
        let dir = if dir == "." { String::new() } else { dir };
        self.tree.set_listing(dir, entries);
    }

    pub fn tree_move(&mut self, delta: isize) {
        let count = self.tree.rows().len();
        if count == 0 { return; }
        self.tree.selected = (self.tree.selected as isize + delta).clamp(0, count as isize - 1) as usize;
    }

    /// Open the selected file, or expand/collapse the selected folder.
    pub fn tree_activate(&mut self, index: Option<usize>) {
        if let Some(index) = index { self.tree.selected = index; }
        let Some(row) = self.tree.rows().get(self.tree.selected).cloned() else { return };
        if row.entry.is_dir {
            if !self.tree.open.remove(&row.path) {
                self.tree.open.insert(row.path.clone());
                self.list_dir(&row.path);
            }
        } else {
            self.open_file(&row.path, None);
        }
    }

    /// Collapse the selected folder, or step out to its parent.
    pub fn tree_collapse(&mut self) {
        let rows = self.tree.rows();
        let Some(row) = rows.get(self.tree.selected) else { return };
        if row.entry.is_dir && self.tree.open.remove(&row.path) { return; }
        let Some((parent, _)) = row.path.rsplit_once('/') else { return };
        if let Some(index) = rows.iter().position(|r| r.path == parent) {
            self.tree.selected = index;
            self.tree.open.remove(parent);
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
}
