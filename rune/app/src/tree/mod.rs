//! The tree sidebars: Files (`Alt+E`, `reference/17-file-tree.png`) over the workspace and Notes
//! (`Alt+N`, `reference/19-notes.png`) over the notes vault (`shared/src/panels/file_tree`,
//! `notes_sidebar.rs`). Listed lazily over the daemon's `Files` family, folders first, expanded in
//! place. Keys: j/k or arrows move, l/Right/Enter open or expand, h/Left collapse.

pub mod drag;
pub(crate) mod git;
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
    git_statuses: HashMap<String, neoism_protocol::git::GitChangeStatus>,
    /// Deleted children absent from disk, indexed once per Git snapshot.
    git_deleted: HashMap<String, Vec<Entry>>,
    pub open: HashSet<String>,
    requested: HashSet<String>,
    pub selected: usize,
    /// The note or folder being created (its folder, name, and whether it is a folder), until
    /// the daemon says it exists.
    creating: Option<(String, String, bool)>,
    create_request: Option<u64>,
    renames: HashMap<u64, (String, String, String)>,
    /// The row to put the selection on once its folder's listing arrives.
    select_after: Option<String>,
}

pub(crate) fn join(dir: &str, name: &str) -> String {
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
        let entries = self.listings.get(dir).map(Vec::as_slice).unwrap_or_default();
        let mut merged;
        let entries = if let Some(deleted) = self.git_deleted.get(dir) {
            merged = entries.to_vec();
            let seen: HashSet<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
            merged.extend(deleted.iter().filter(|entry| !seen.contains(entry.name.as_str())).cloned());
            merged.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            merged.as_slice()
        } else { entries };
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
            Some(root) => {
                let windows = root.starts_with("\\\\") || root.as_bytes().get(1) == Some(&b':');
                let root = if windows { root.replace('\\', "/") } else { root.clone() };
                let path = if windows { path.replace('\\', "/") } else { path.to_owned() };
                let root = root.trim_end_matches('/');
                let rest = path.trim_end_matches('/').strip_prefix(root)?;
                if rest.is_empty() { Some(String::new()) } else { rest.strip_prefix('/').map(str::to_owned) }
            }
            None if crate::startup::absolute(path) => None,
            None => Some(match path.trim_start_matches("./") { "." => String::new(), rest => rest.to_owned() }),
        }
    }

    /// Absolute or workspace-relative path of a row, as editors open it.
    pub fn open_path(&self, path: &str) -> String {
        match &self.root { Some(root) => format!("{root}/{path}"), None => path.to_owned() }
    }
}

/// A vault file that is bookkeeping, not a note.
fn vault_bookkeeping(name: &str) -> bool { name.to_ascii_lowercase().ends_with(".json") }

/// `stem` + `ext`, numbered (`Untitled 2.md`) until no entry in `taken` (lower case) has it.
fn free_name(taken: &HashSet<String>, stem: &str, ext: &str) -> String {
    let mut name = format!("{stem}{ext}");
    let mut n = 2;
    while taken.contains(&name.to_lowercase()) {
        name = format!("{stem} {n}{ext}");
        n += 1;
    }
    name
}

/// The folder to ask the daemon about for the notes vault. Notes always open the default
/// vault: a vault linked to the project (`.neoism/workspace.json`) is not followed. The
/// daemon answers with the default vault for a folder that links none, and the filesystem
/// root of the workspace's drive never does, whatever daemon version is asked.
fn default_vault_probe(cwd: &str) -> String {
    let bytes = cwd.as_bytes();
    if cwd.starts_with("\\\\") || cwd.starts_with("//") {
        let normalized = cwd.replace('\\', "/");
        let mut parts = normalized.trim_start_matches('/').split('/');
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) { return format!("//{server}/{share}/"); }
    }
    if cwd.starts_with('/') { return "/".to_owned(); }
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' { return format!("{}\\", &cwd[..2]); }
    cwd.to_owned()
}

/// Marks the notes vault as asked for, among the directories asked for.
const RESOLVE: &str = "\0resolve-notes-vault";

impl App {
    pub(crate) fn tree_of(&mut self, sidebar: Sidebar) -> &mut Tree {
        if sidebar == Sidebar::Notes { &mut self.notes } else { &mut self.tree }
    }

    /// The tree the visible sidebar shows, if it is one.
    pub fn active_tree(&self) -> Option<&Tree> {
        match self.sidebar { Some(Sidebar::Files) => Some(&self.tree), Some(Sidebar::Notes) => Some(&self.notes), _ => None }
    }

    pub(crate) fn list_dir(&mut self, sidebar: Sidebar, dir: &str) {
        let tree = self.tree_of(sidebar);
        if !tree.requested.insert(dir.to_owned()) { return; }
        let path = tree.request_path(dir);
        let scope = tree.root.clone().unwrap_or_else(|| self.cwd.clone());
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Files, serde_json::json!({"ListDir": {"path": path}}), &scope);
        }
    }

    /// Per frame: a tree that is showing with nothing listed asks for its listing. A sidebar
    /// restored with the window was never opened by a click, and a request made before the
    /// daemon answered was never heard.
    pub(crate) fn tree_ensure_listed(&mut self) {
        if self.connection != crate::app::Connection::Connected || !self.workspace_ready { return; }
        let Some(sidebar) = self.sidebar.filter(|s| matches!(s, Sidebar::Files | Sidebar::Notes)) else { return };
        if self.tree_of(sidebar).loaded() { return; }
        if sidebar == Sidebar::Notes && self.notes.root.is_none() {
            // The vault is resolved per workspace; its listing follows `NotesVaultResolved`.
            if self.notes.requested.insert(RESOLVE.to_owned()) && let Some(daemon) = &self.daemon {
                daemon.send(Family::Workspace, serde_json::json!({"ResolveNotesVault": {"root_dir": default_vault_probe(&self.cwd)}}));
            }
            return;
        }
        self.list_dir(sidebar, "");
    }

    /// The connection came back: whatever was asked on the old one is asked again on demand.
    pub(crate) fn tree_reconnected(&mut self) {
        for tree in [&mut self.tree, &mut self.notes] {
            tree.requested.clear();
            tree.creating = None;
            tree.create_request = None;
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
                    daemon.send(Family::Workspace, serde_json::json!({"ResolveNotesVault": {"root_dir": default_vault_probe(&self.cwd)}}));
                }
            }
            _ if !self.tree_of(sidebar).loaded() => self.list_dir(sidebar, ""),
            _ => {}
        }
    }

    pub fn toggle_file_tree(&mut self) { self.toggle_tree(Sidebar::Files); }

    /// The rail's tree button: open or close the panel, the keyboard staying where it was.
    pub fn click_tree(&mut self, sidebar: Sidebar) {
        let focus = self.focus;
        self.toggle_tree(sidebar);
        if self.sidebar.is_some() && focus != Focus::Sidebar { self.focus = focus; }
    }

    pub(crate) fn apply_notes_vault(&mut self, dir: &str) {
        let dir = if dir.starts_with("\\\\") || dir.as_bytes().get(1) == Some(&b':') { dir.replace('\\', "/") } else { dir.to_owned() };
        self.notes.root = Some(dir.trim_end_matches('/').to_owned());
        self.list_dir(Sidebar::Notes, "");
    }

    pub(crate) fn apply_dir_listing(&mut self, path: &str, entries: &serde_json::Value) {
        let mut entries: Vec<Entry> = entries.as_array().into_iter().flatten().filter_map(|e| {
            Some(Entry { name: e["name"].as_str()?.to_owned(), is_dir: e["is_dir"].as_bool().unwrap_or(false) })
        }).collect();
        // A notes tree claims only replies under its resolved vault; the rest are the workspace's.
        let notes = self.notes.root.is_some() && self.notes.own(path).is_some();
        // A vault's JSON files are its bookkeeping (`project.json`, notebook metadata), not notes.
        if notes { entries.retain(|e| e.is_dir || !vault_bookkeeping(&e.name)); }
        let tree = if notes { &mut self.notes } else { &mut self.tree };
        if let Some(dir) = tree.own(path) { tree.set_listing(dir, entries); }
        // A note or folder just made takes the selection once it is listed.
        if let Some(made) = tree.select_after.take() {
            match tree.rows().iter().position(|row| row.path == made) {
                Some(index) => { tree.selected = index; self.tree_follow.request(); }
                None => tree.select_after = Some(made),
            }
        }
    }

    /// The Notes header's `+` buttons: a new note (`Untitled.md`) or folder (`New Folder`)
    /// beside the selection: inside the selected folder when it is open, else next to the
    /// selected row, else at the vault's root. A name already taken gets a number. The note
    /// opens once the daemon has made it.
    pub fn notes_create(&mut self, folder: bool) {
        if self.notes.root.is_none() || self.notes.creating.is_some() { return; }
        let tree = &self.notes;
        let dir = match tree.rows().get(tree.selected) {
            Some(row) if row.entry.is_dir && tree.open.contains(&row.path) => row.path.clone(),
            Some(row) => row.path.rsplit_once('/').map(|(parent, _)| parent.to_owned()).unwrap_or_default(),
            None => String::new(),
        };
        let taken: HashSet<String> = tree.listings.get(&dir).into_iter().flatten().map(|e| e.name.to_lowercase()).collect();
        let name = free_name(&taken, if folder { "New Folder" } else { "Untitled" }, if folder { "" } else { ".md" });
        self.tree_create(true, &dir, &name, folder);
    }

    /// Make `name` in `dir` of a tree (the right-click menu's New File / New Folder).
    pub(crate) fn tree_create(&mut self, notes: bool, dir: &str, name: &str, folder: bool) {
        let sidebar = if notes { Sidebar::Notes } else { Sidebar::Files };
        let Some(root) = self.tree_root(notes) else { return };
        if self.tree_of(sidebar).creating.is_some() { return; }
        let (dir, name) = match name.rsplit_once('/') {
            Some((parent, name)) => (join(dir, parent), name),
            None => (dir.to_owned(), name),
        };
        let name = if notes && !folder && !name.to_ascii_lowercase().ends_with(".md") { format!("{name}.md") } else { name.to_owned() };
        let kind = if folder { "CreateDir" } else { "CreateFile" };
        let request = self.daemon.as_ref().map(|daemon| daemon.request(Family::Files, serde_json::json!({ kind: {"dir": dir, "name": name} }), Some(root)));
        let tree = self.tree_of(sidebar);
        tree.create_request = request;
        tree.creating = Some((dir, name, folder));
    }

    pub(crate) fn tree_creation_error(&mut self, request_id: u64, error: &str) -> bool {
        for sidebar in [Sidebar::Notes, Sidebar::Files] {
            let tree = self.tree_of(sidebar);
            if let Some((root, from, _)) = tree.renames.remove(&request_id) {
                self.cancel_editor_rename(&root, &from);
                self.modal = Some(crate::ui::modal::Modal::notice("Rename failed", error));
                return true;
            }
            if tree.create_request != Some(request_id) { continue; }
            tree.create_request = None;
            let Some((dir, name, folder)) = tree.creating.take() else { continue };
            let path = tree.open_path(&join(&dir, &name));
            self.modal = Some(crate::ui::modal::Modal::notice(if folder { "Could not create folder" } else { "Could not create file" }, &format!("{path}\n\n{error}")));
            return true;
        }
        false
    }

    /// The folder a tree's requests are scoped to: the vault, or the workspace.
    pub(crate) fn tree_root(&self, notes: bool) -> Option<String> {
        if notes { self.notes.root.clone() } else { Some(self.cwd.clone()) }
    }

    /// Rename or move `from` to `to` (both relative to the tree's root).
    pub(crate) fn tree_move_path(&mut self, notes: bool, from: &str, to: &str) {
        let Some(root) = self.tree_root(notes) else { return };
        if from == to || from.is_empty() || to.is_empty() { return; }
        if let Some(daemon) = self.daemon.clone() {
            if !self.prepare_editor_rename(&root, from, to) { return; }
            let request = daemon.request(Family::Files, serde_json::json!({"Rename": {"from": from, "to": to}}), Some(root.clone()));
            let sidebar = if notes { Sidebar::Notes } else { Sidebar::Files };
            self.tree_of(sidebar).renames.insert(request, (root, from.into(), to.into()));
        }
        let tree = if notes { &mut self.notes } else { &mut self.tree };
        // Where it lands is shown (its folder opened) and it keeps the selection.
        let folder = to.rsplit_once('/').map_or("", |(dir, _)| dir);
        if !folder.is_empty() { tree.open.insert(folder.to_owned()); }
        tree.select_after = Some(to.to_owned());
    }

    pub(crate) fn apply_tree_renamed(&mut self, request: u64, from: &str, to: &str) {
        for sidebar in [Sidebar::Files, Sidebar::Notes] {
            if let Some((root, old, new)) = self.tree_of(sidebar).renames.remove(&request) {
                self.apply_editor_renamed(&root, &old, &new);
                break;
            }
        }
        self.apply_tree_changed(&[from, to]);
    }

    /// Watch pushes cover external writers too (agent tools, shells and other windows).
    /// Refresh cached directories without treating an ordinary edit as a deleted folder.
    pub(crate) fn apply_tree_watch_changed(&mut self, root: &str, paths: &[String]) {
        for sidebar in [Sidebar::Files, Sidebar::Notes] {
            let scope = if sidebar == Sidebar::Notes {
                let Some(root) = self.notes.root.clone() else { continue };
                root
            } else { self.cwd.clone() };
            let owner = Tree { root: Some(scope), ..Default::default() };
            if owner.own(root).as_deref() != Some("") { continue; }
            let changed: Vec<String> = paths.iter().filter_map(|path| {
                owner.own(path).or_else(|| {
                    (!crate::startup::absolute(path)).then(|| path.trim_start_matches("./").to_owned())
                })
            }).collect();
            let tree = self.tree_of(sidebar);
            let dirs: Vec<String> = tree.listings.keys().filter(|dir| {
                changed.iter().any(|path| {
                    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
                    path.is_empty() || *dir == path || dir.as_str() == parent || dir.starts_with(&format!("{path}/"))
                })
            }).cloned().collect();
            for dir in dirs { self.list_dir(sidebar, &dir); }
        }
    }

    /// `Renamed` / `Deleted`: the folders that held the entry are listed again, in whichever
    /// tree shows them (a path does not say which tree asked; listing both costs a request).
    pub(crate) fn apply_tree_changed(&mut self, paths: &[&str]) {
        for sidebar in [Sidebar::Files, Sidebar::Notes] {
            for path in paths {
                let path = path.trim_start_matches("./");
                let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned();
                let tree = self.tree_of(sidebar);
                // A folder that is gone takes its listings and open state with it.
                tree.listings.retain(|listed, _| listed != path && !listed.starts_with(&format!("{path}/")));
                tree.open.retain(|open| open != path && !open.starts_with(&format!("{path}/")));
                if !tree.listings.contains_key(&dir) { continue; }
                tree.requested.remove(&dir);
                self.list_dir(sidebar, &dir);
            }
            let tree = self.tree_of(sidebar);
            let count = tree.rows().len();
            tree.selected = tree.selected.min(count.saturating_sub(1));
        }
    }

    /// `FileCreated`: the note or folder asked for exists. Its folder is listed again with it
    /// selected, and a note opens for writing.
    pub(crate) fn apply_file_created(&mut self, path: &str, is_dir: bool) {
        let path = path.trim_start_matches("./");
        for sidebar in [Sidebar::Notes, Sidebar::Files] {
            let tree = self.tree_of(sidebar);
            let Some((dir, name, folder)) = tree.creating.clone() else { continue };
            if join(&dir, &name) != path || folder != is_dir { continue; }
            tree.creating = None;
            tree.create_request = None;
            // Reserve the acknowledged name before a second click, even if listing is in flight.
            let mut entries = tree.listings.get(&dir).cloned().unwrap_or_default();
            if !entries.iter().any(|e| e.name == name) { entries.push(Entry { name, is_dir }); }
            tree.set_listing(dir.clone(), entries);
            tree.select_after = Some(path.to_owned());
            let file = tree.open_path(path);
            // Nested creation may have made every parent: expose and refresh the whole chain.
            let mut parent = dir.as_str();
            loop {
                let tree = self.tree_of(sidebar);
                tree.requested.remove(parent);
                if !parent.is_empty() { tree.open.insert(parent.to_owned()); }
                self.list_dir(sidebar, parent);
                if parent.is_empty() { break; }
                parent = parent.rsplit_once('/').map_or("", |(dir, _)| dir);
            }
            if !is_dir { self.open_file(&file, None); }
            return;
        }
    }

    fn sidebar_tree(&mut self) -> Option<(Sidebar, &mut Tree)> {
        let sidebar = self.sidebar.filter(|s| matches!(s, Sidebar::Files | Sidebar::Notes))?;
        Some((sidebar, self.tree_of(sidebar)))
    }

    /// Whole rows visible in the tree's scroll view (a page for PageUp/PageDown).
    pub fn tree_page_rows(&self) -> isize {
        let row_h = 26.0 * self.ty.scale();
        ((self.tree_follow.scroll.get().2 / row_h).floor() as isize - 1).max(1)
    }

    /// Select the tree's 1-based row `row` (vim `<count>G`, Alt+P `:N`), clamped to the last.
    pub fn tree_go_to_row(&mut self, row: usize) {
        let Some((_, tree)) = self.sidebar_tree() else { return };
        let count = tree.rows().len();
        if count == 0 { return; }
        tree.selected = row.saturating_sub(1).min(count - 1);
        self.tree_follow.request();
    }

    /// Rows in the focused file or notes tree, for the palette's `:N`.
    pub fn focused_tree_rows(&mut self) -> Option<usize> {
        if self.focus != crate::app::Focus::Sidebar { return None; }
        self.sidebar_tree().map(|(_, tree)| tree.rows().len()).filter(|n| *n > 0)
    }

    pub fn tree_move(&mut self, delta: isize) {
        let Some((_, tree)) = self.sidebar_tree() else { return };
        let count = tree.rows().len();
        if count == 0 { return; }
        tree.selected = (tree.selected as isize).saturating_add(delta).clamp(0, count as isize - 1) as usize;
        self.tree_follow.request();
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
            self.tree_follow.request();
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
    fn watch_push_refreshes_cached_tree_without_reopening_workspace() {
        for root in ["/project", "C:\\project"] {
            let mut app = App::new();
            app.cwd = root.into();
            app.notes.root = Some("/vault".into());
            app.apply_dir_listing("", &serde_json::json!([{"name":"src", "is_dir":true}]));
            app.apply_dir_listing("src", &serde_json::json!([{"name":"old.rs", "is_dir":false}]));
            app.apply_dir_listing("/vault", &serde_json::json!([]));
            app.tree.open.insert("src".into());
            app.apply(crate::daemon::Inbound::Reply {
                family: "Files".into(), request_id: 0,
                message: serde_json::json!({"Changed": {"root": root, "paths": [format!("{root}/src/old.rs"), format!("{root}/new-folder")]}}),
            });
            assert!(app.tree.requested.contains(""), "new root entries trigger a listing");
            assert!(app.tree.requested.contains("src"), "external child changes trigger a listing");
            assert!(app.tree.open.contains("src"), "watch bursts preserve expanded folders");
            assert!(app.notes.requested.is_empty(), "another root must not refresh notes");
            app.apply_dir_listing("", &serde_json::json!([{"name":"src", "is_dir":true}, {"name":"new-folder", "is_dir":true}]));
            app.apply_dir_listing("src", &serde_json::json!([{"name":"new.rs", "is_dir":false}]));
            let paths: Vec<String> = app.tree.rows().into_iter().map(|r| r.path).collect();
            assert_eq!(paths, ["new-folder", "src", "src/new.rs"]);
            assert!(app.tree.requested.is_empty());
            app.apply_tree_watch_changed("/other-project", &["/other-project/file.rs".into()]);
            assert!(app.tree.requested.is_empty());
            app.apply_tree_watch_changed("/vault", &["/vault/New Note.md".into()]);
            assert!(app.notes.requested.contains(""));
            assert!(app.tree.requested.is_empty());
        }
    }

    #[test]
    fn listings_route_to_their_tree() {
        let notes = Tree { root: Some("/vault".into()), ..Default::default() };
        let files = Tree::default();
        assert_eq!(notes.own("/vault"), Some(String::new()));
        assert_eq!(notes.own("/vault/a"), Some("a".into()));
        assert_eq!(notes.own("/vault-other/a"), None);
        let windows = Tree { root: Some("C:\\Users\\me\\Notes".into()), ..Default::default() };
        assert_eq!(windows.own("C:/Users/me/Notes/Ideas"), Some("Ideas".into()));
        assert_eq!(windows.own("C:\\Users\\me\\Notes\\Ideas"), Some("Ideas".into()));
        assert_eq!(windows.own("C:/Users/me/Notes-other/Ideas"), None);
        assert_eq!(notes.own("src"), None);
        assert_eq!(files.own("src"), Some("src".into()));
        assert_eq!(files.own("/vault/a"), None);
        assert_eq!(notes.request_path("a"), "/vault/a");
    }

    #[test]
    fn a_vault_hides_its_json_and_a_new_note_takes_a_free_name_and_the_selection() {
        let mut app = App::new();
        app.notes.root = Some("/vault".into());
        app.sidebar = Some(Sidebar::Notes);
        let entry = |name: &str, is_dir: bool| serde_json::json!({"name": name, "is_dir": is_dir});
        app.apply_dir_listing("/vault", &serde_json::json!([entry("project.json", false), entry("Untitled.md", false), entry("Ideas", true), entry("data.json", true)]));
        let names: Vec<String> = app.notes.rows().iter().map(|r| r.path.clone()).collect();
        assert_eq!(names, ["data.json", "Ideas", "Untitled.md"], "JSON files are hidden; a folder is a folder whatever its name");
        // The workspace's own tree still lists JSON.
        app.apply_dir_listing(".", &serde_json::json!([entry("package.json", false)]));
        assert_eq!(app.tree.rows().len(), 1);
        // A new note beside the selection: `Untitled.md` is taken.
        app.notes.selected = 2;
        app.notes_create(false);
        assert_eq!(app.notes.creating, Some((String::new(), "Untitled 2.md".into(), false)));
        // Someone else's file being created is not ours.
        app.apply_file_created("other.md", false);
        assert!(app.notes.creating.is_some());
        app.apply_file_created("Untitled 2.md", false);
        assert!(app.notes.creating.is_none());
        assert!(app.tabs.iter().any(|t| t.kind == crate::tabs::TabKind::Editor { path: "/vault/Untitled 2.md".into() }), "the note opens");
        app.apply_dir_listing("/vault", &serde_json::json!([entry("Untitled.md", false), entry("Untitled 2.md", false), entry("Ideas", true)]));
        assert_eq!(app.notes.rows()[app.notes.selected].path, "Untitled 2.md");
        // A folder made inside an open folder lands there.
        app.notes.selected = 0;
        app.notes.open.insert("Ideas".into());
        app.apply_dir_listing("/vault/Ideas", &serde_json::json!([entry("New Folder", true)]));
        app.notes_create(true);
        assert_eq!(app.notes.creating, Some(("Ideas".into(), "New Folder 2".into(), true)));
    }

    #[test]
    fn notes_creation_is_markdown_and_failures_allow_retry() {
        let mut app = App::new();
        app.apply_notes_vault("C:\\Users\\me\\Notes");
        assert_eq!(app.notes.root.as_deref(), Some("C:/Users/me/Notes"));
        app.tree_create(true, "Ideas", "Agenda", false);
        assert_eq!(app.notes.creating, Some(("Ideas".into(), "Agenda.md".into(), false)));
        app.notes.create_request = Some(42);
        assert!(!app.tree_creation_error(41, "unrelated"));
        assert!(app.notes.creating.is_some());
        assert!(app.tree_creation_error(42, "Permission denied"));
        assert!(app.notes.creating.is_none());
        assert!(app.modal.as_ref().unwrap().body.contains("C:/Users/me/Notes/Ideas/Agenda.md"));
        app.tree_create(true, "Ideas", "Existing.MD", false);
        assert_eq!(app.notes.creating.as_ref().unwrap().1, "Existing.MD");
        app.apply_file_created("Ideas/Existing.MD", false);
        app.tree_create(true, "", "New Folder", true);
        assert_eq!(app.notes.creating, Some((String::new(), "New Folder".into(), true)));
        app.apply_file_created("New Folder", true);
        app.tree_create(false, "", "plain-file", false);
        assert_eq!(app.tree.creating.as_ref().unwrap().1, "plain-file", "the workspace file tree stays general purpose");
    }

    #[test]
    fn nested_notes_and_folders_reveal_all_created_parents() {
        let mut app = App::new();
        app.notes.root = Some("/vault".into());
        app.sidebar = Some(Sidebar::Notes);
        app.apply_dir_listing("/vault", &serde_json::json!([]));
        app.tree_create(true, "", "Ideas/Projects/Meeting", false);
        assert_eq!(app.notes.creating, Some(("Ideas/Projects".into(), "Meeting.md".into(), false)));
        app.apply_file_created("Ideas/Projects/Meeting.md", false);
        assert!(app.notes.open.contains("Ideas"));
        assert!(app.notes.open.contains("Ideas/Projects"));
        assert_eq!(app.active_buffer().unwrap().path, "/vault/Ideas/Projects/Meeting.md");
        app.apply_dir_listing("/vault", &serde_json::json!([{"name":"Ideas", "is_dir":true}]));
        app.apply_dir_listing("/vault/Ideas", &serde_json::json!([{"name":"Projects", "is_dir":true}]));
        assert_eq!(app.notes.rows()[app.notes.selected].path, "Ideas/Projects/Meeting.md");
        let active = app.active;
        app.tree_create(true, "Ideas", "Archive/2026", true);
        assert_eq!(app.notes.creating, Some(("Ideas/Archive".into(), "2026".into(), true)));
        app.apply_file_created("Ideas/Archive/2026", true);
        assert!(app.notes.open.contains("Ideas/Archive"));
        assert_eq!(app.active, active, "creating a folder does not open an editor");
    }

    #[test]
    fn rename_ack_and_error_use_the_requests_vault_not_the_current_sidebar() {
        let mut app = App::new();
        app.open_file("/vault/Agenda.md", None);
        app.apply_opened_file("/vault/Agenda.md", b"keep this text\n");
        assert!(app.prepare_editor_rename("/vault", "Agenda.md", "Planning.md"));
        app.notes.renames.insert(42, ("/vault".into(), "Agenda.md".into(), "Planning.md".into()));
        app.notes.root = Some("/other-vault".into());
        app.sidebar = Some(Sidebar::Files);
        app.apply_tree_renamed(42, "Agenda.md", "Planning.md");
        assert_eq!(app.active_buffer().unwrap().path, "/vault/Planning.md");
        assert_eq!(app.active_buffer().unwrap().doc.lines, ["keep this text"]);
        assert!(app.prepare_editor_rename("/vault", "Planning.md", "Failed.md"));
        app.notes.renames.insert(43, ("/vault".into(), "Planning.md".into(), "Failed.md".into()));
        assert!(app.tree_creation_error(43, "Permission denied"));
        assert_eq!(app.active_buffer().unwrap().path, "/vault/Planning.md");
        assert!(!app.active_buffer().unwrap().renaming);
        assert!(!app.buffers.contains_key("/vault/Failed.md"));
    }

    #[test]
    fn notes_header_buttons_create_notes_and_folders_and_reserve_acknowledged_names() {
        let mut app = App::new();
        app.sidebar = Some(Sidebar::Notes);
        app.apply_notes_vault("/vault");
        app.apply_dir_listing("/vault", &serde_json::json!([]));
        let mut rt = rune::Runtime::new(app, |a, _| super::view::view(a), (300.0, 600.0), rune::Env::new(true, 1.0));
        rt.frame(1.0);
        let click = |rt: &mut rune::Runtime<App>, key: &str| {
            let [x, y, w, h] = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect;
            rt.pointer_down(x + w / 2.0, y + h / 2.0);
            rt.pointer_up(x + w / 2.0, y + h / 2.0);
            rt.frame(1.0 / 60.0);
        };
        click(&mut rt, "notes-new-note");
        assert_eq!(rt.state.notes.creating.as_ref().unwrap().1, "Untitled.md");
        click(&mut rt, "notes-new-note");
        assert_eq!(rt.state.notes.creating.as_ref().unwrap().1, "Untitled.md", "a pending click must not queue a duplicate");
        rt.update(|a, _| a.apply_file_created("Untitled.md", false));
        assert!(rt.state.tabs.iter().any(|t| t.kind == crate::tabs::TabKind::Editor { path: "/vault/Untitled.md".into() }));
        click(&mut rt, "notes-new-note");
        assert_eq!(rt.state.notes.creating.as_ref().unwrap().1, "Untitled 2.md", "acknowledged names are reserved before the next listing");
        rt.update(|a, _| a.apply_file_created("Untitled 2.md", false));
        click(&mut rt, "notes-new-folder");
        assert_eq!(rt.state.notes.creating, Some((String::new(), "New Folder".into(), true)));
    }

    #[test]
    fn notes_ask_for_the_default_vault_whatever_the_project_links() {
        assert_eq!(default_vault_probe("/home/me/projects/linked"), "/");
        assert_eq!(default_vault_probe("C:\\Users\\me\\linked"), "C:\\");
        assert_eq!(default_vault_probe("d:/work"), "d:\\");
        assert_eq!(default_vault_probe("\\\\server\\share\\project"), "//server/share/");
        // A remote home not resolved yet is asked as it stands.
        assert_eq!(default_vault_probe("~"), "~");
    }
}
