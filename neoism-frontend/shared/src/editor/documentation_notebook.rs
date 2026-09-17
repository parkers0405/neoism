//! Folder-backed documentation notebooks. Pages remain ordinary Markdown files.
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const MANIFEST_NAME: &str = "notebook.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotebookManifest {
    pub title: String,
    pub pages: Vec<NotebookPage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NotebookPage {
    Path(PathBuf),
    Named {
        path: PathBuf,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        section: Option<String>,
    },
}

impl NotebookPage {
    pub fn path(&self) -> &Path {
        match self {
            Self::Path(path) | Self::Named { path, .. } => path,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Self::Named {
                title: Some(title), ..
            } => title.clone(),
            _ => self
                .path()
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .replace(['-', '_'], " "),
        }
    }

    pub fn section(&self) -> Option<&str> {
        match self {
            Self::Named { section, .. } => section.as_deref(),
            _ => None,
        }
    }
}

/// Lexical normalization also works for not-yet-created files. No vault boundary
/// is imposed: a notebook may deliberately reference files outside its folder.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if result.file_name().is_some_and(|name| name != "..") {
                    result.pop();
                } else if !result.has_root() {
                    result.push("..");
                }
            }
            _ => result.push(component.as_os_str()),
        }
    }
    result
}

pub fn is_manifest(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == MANIFEST_NAME)
}

#[derive(Clone, Debug)]
pub struct NotebookBinding {
    pub path: PathBuf,
    pub session: Arc<Mutex<DocumentationNotebook>>,
}

impl NotebookBinding {
    pub fn load(path: &Path) -> Result<Self, String> {
        let path = if path.is_dir() {
            path.join(MANIFEST_NAME)
        } else {
            path.to_path_buf()
        };
        let path = std::fs::canonicalize(&path)
            .map_err(|error| format!("Cannot open notebook: {error}"))?;
        let source = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let manifest: NotebookManifest = serde_json::from_str(&source)
            .map_err(|error| format!("Invalid notebook manifest: {error}"))?;
        let mut session = DocumentationNotebook::new(path.clone(), manifest)?;
        session.saved_source = Some(source);
        Ok(Self {
            path,
            session: Arc::new(Mutex::new(session)),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotebookInput {
    Open,
    Create,
    AddPage,
    AddExisting,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NotebookAction {
    Page(usize),
    Back,
    Forward,
    ToggleContents,
    ToggleSection(String),
    AddPage,
    AddExisting,
    MoveUp,
    MoveDown,
}

#[derive(Debug)]
pub struct DocumentationNotebook {
    pub path: PathBuf,
    pub manifest: NotebookManifest,
    pub current: usize,
    pub history: Vec<usize>,
    pub history_index: usize,
    pub contents_open: bool,
    pub collapsed_sections: std::collections::HashSet<String>,
    pub modified_pages: std::collections::HashSet<PathBuf>,
    pub scroll_rows: usize,
    pub hit_regions: Vec<([f32; 4], NotebookAction)>,
    pub rail_rect: Option<[f32; 4]>,
    saved_source: Option<String>,
}

impl DocumentationNotebook {
    pub fn new(path: PathBuf, manifest: NotebookManifest) -> Result<Self, String> {
        if manifest.pages.len() > 10_000 {
            return Err("Notebook exceeds 10,000 pages".into());
        }
        if manifest.title.trim().is_empty() || manifest.pages.is_empty() {
            return Err("A notebook needs a title and at least one page".into());
        }
        let root = path.parent().ok_or("Notebook has no parent folder")?;
        let mut unique = std::collections::HashSet::new();
        for page in &manifest.pages {
            let path = normalize_path(&root.join(page.path()));
            if page.path().as_os_str().is_empty()
                || !super::markdown::is_markdown_path(&path)
            {
                return Err(format!("Not a Markdown page: {}", page.path().display()));
            }
            if !unique.insert(path) {
                return Err(format!(
                    "Duplicate notebook page: {}",
                    page.path().display()
                ));
            }
        }
        Ok(Self {
            path,
            manifest,
            current: 0,
            history: vec![0],
            history_index: 0,
            contents_open: false,
            collapsed_sections: std::collections::HashSet::new(),
            modified_pages: std::collections::HashSet::new(),
            scroll_rows: 0,
            hit_regions: Vec::new(),
            rail_rect: None,
            saved_source: None,
        })
    }

    pub fn page_path(&self, index: usize) -> Option<PathBuf> {
        Some(normalize_path(
            &self
                .path
                .parent()?
                .join(self.manifest.pages.get(index)?.path()),
        ))
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        let path = normalize_path(path);
        (0..self.manifest.pages.len())
            .find(|&index| self.page_path(index).as_ref() == Some(&path))
    }

    pub fn navigate(&mut self, index: usize) -> bool {
        if index >= self.manifest.pages.len() || index == self.current {
            return false;
        }
        self.history.truncate(self.history_index + 1);
        self.history.push(index);
        self.history_index = self.history.len() - 1;
        self.current = index;
        true
    }

    pub fn back(&mut self) -> bool {
        if self.history_index == 0 {
            return false;
        }
        self.history_index -= 1;
        self.current = self.history[self.history_index];
        true
    }

    pub fn forward(&mut self) -> bool {
        if self.history_index + 1 >= self.history.len() {
            return false;
        }
        self.history_index += 1;
        self.current = self.history[self.history_index];
        true
    }

    pub fn action_at(&self, x: f32, y: f32) -> Option<NotebookAction> {
        self.hit_regions
            .iter()
            .rev()
            .find(|(rect, _)| contains(*rect, x, y))
            .map(|(_, action)| action.clone())
    }

    pub fn wheel_at(&mut self, x: f32, y: f32, delta: f32) -> bool {
        if !self.rail_rect.is_some_and(|rect| contains(rect, x, y)) {
            return false;
        }
        let steps = (delta.abs() / 30.0).ceil() as usize;
        self.scroll_rows = if delta > 0.0 {
            self.scroll_rows.saturating_sub(steps)
        } else {
            self.scroll_rows
                .saturating_add(steps)
                .min(self.manifest.pages.len().saturating_sub(1))
        };
        true
    }

    pub fn persist(&mut self) -> Result<(), String> {
        if let Some(expected) = &self.saved_source {
            let current =
                std::fs::read_to_string(&self.path).map_err(|error| error.to_string())?;
            if &current != expected {
                return Err("The notebook manifest changed on disk. Close and reopen the notebook before changing its pages".into());
            }
        }
        // Write alongside the manifest, then replace it so an interrupted write
        // cannot leave a half-written collection definition.
        let bytes = serde_json::to_vec_pretty(&self.manifest)
            .map_err(|error| error.to_string())?;
        let temp = self.path.with_extension("json.tmp");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("Cannot stage notebook manifest: {error}"))?;
        use std::io::Write;
        let result = (|| -> std::io::Result<()> {
            if let Some(expected) = &self.saved_source {
                if std::fs::read_to_string(&self.path)? != *expected {
                    return Err(std::io::Error::other("Notebook changed on disk; reopen it before editing the page list"));
                }
            }
            file.write_all(&bytes)?;
            file.sync_all()
        })();
        drop(file);
        if let Err(error) = result {
            let _ = std::fs::remove_file(&temp);
            return Err(error.to_string());
        }
        if let Err(error) = std::fs::rename(&temp, &self.path) {
            let _ = std::fs::remove_file(&temp);
            return Err(error.to_string());
        }
        self.saved_source =
            Some(String::from_utf8(bytes).map_err(|error| error.to_string())?);
        Ok(())
    }

    pub fn add_existing(&mut self, path: PathBuf) -> Result<usize, String> {
        let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
        if !path.is_file() || !super::markdown::is_markdown_path(&path) {
            return Err("Choose an existing Markdown file".into());
        }
        if let Some(index) = self.index_of(&path) {
            return Ok(index);
        }
        let reference = path
            .strip_prefix(self.path.parent().unwrap())
            .unwrap_or(&path)
            .to_path_buf();
        self.manifest.pages.push(NotebookPage::Path(reference));
        if let Err(error) = self.persist() {
            self.manifest.pages.pop();
            return Err(error);
        }
        Ok(self.manifest.pages.len() - 1)
    }

    /// Repoint references after the host successfully renames a file or folder.
    pub fn rebase_paths(&mut self, old: &Path, new: &Path) -> bool {
        let old_manifest = self.path.clone();
        let old_paths: Vec<_> = (0..self.manifest.pages.len())
            .filter_map(|index| self.page_path(index))
            .collect();
        if let Ok(suffix) = self.path.strip_prefix(old) {
            self.path = new.join(suffix);
        }
        let root = self.path.parent().unwrap();
        let mut changed = self.path != old_manifest;
        for (page, path) in self.manifest.pages.iter_mut().zip(old_paths) {
            let moved = path.strip_prefix(old).ok().map(|suffix| new.join(suffix));
            changed |= moved.is_some();
            if moved.is_some() || self.path != old_manifest {
                let destination = moved.unwrap_or(path);
                let reference = destination
                    .strip_prefix(root)
                    .unwrap_or(&destination)
                    .to_path_buf();
                match page {
                    NotebookPage::Path(path) | NotebookPage::Named { path, .. } => {
                        *path = reference
                    }
                }
            }
        }
        changed
    }

    pub fn move_current(&mut self, down: bool) -> Result<(), String> {
        let next = if down {
            self.current + 1
        } else {
            self.current.saturating_sub(1)
        };
        if next >= self.manifest.pages.len() || next == self.current {
            return Ok(());
        }
        self.manifest.pages.swap(self.current, next);
        if let Err(error) = self.persist() {
            self.manifest.pages.swap(self.current, next);
            return Err(error);
        }
        for index in &mut self.history {
            if *index == self.current {
                *index = next;
            } else if *index == next {
                *index = self.current;
            }
        }
        self.current = next;
        Ok(())
    }
}

fn contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && x < rect[0] + rect[2] && y >= rect[1] && y < rect[1] + rect[3]
}

mod storage;
mod view;
pub use view::render_navigation;

#[cfg(test)]
mod tests {
    use super::*;
    fn book() -> DocumentationNotebook {
        DocumentationNotebook::new(
            PathBuf::from("/notes/book/notebook.json"),
            NotebookManifest {
                title: "Architecture".into(),
                pages: vec![
                    NotebookPage::Path("intro.md".into()),
                    NotebookPage::Path("api.md".into()),
                    NotebookPage::Path("../reference.md".into()),
                ],
            },
        )
        .unwrap()
    }
    #[test]
    fn history_is_distinct_from_page_order() {
        let mut book = book();
        assert!(book.navigate(2));
        assert!(book.back());
        assert_eq!(book.current, 0);
        assert!(book.forward());
        assert_eq!(book.current, 2);
        book.back();
        book.navigate(1);
        assert!(!book.forward());
        assert_eq!(book.history, vec![0, 1]);
    }
    #[test]
    fn references_can_leave_the_notebook_folder() {
        let book = book();
        assert_eq!(
            book.page_path(2),
            Some(PathBuf::from("/notes/reference.md"))
        );
        assert_eq!(
            book.index_of(Path::new("/notes/book/../reference.md")),
            Some(2)
        );
    }
    #[test]
    fn duplicate_paths_are_rejected() {
        let mut manifest = book().manifest;
        manifest.pages.push(NotebookPage::Path("./intro.md".into()));
        assert!(
            DocumentationNotebook::new("/book/notebook.json".into(), manifest).is_err()
        );
    }
    #[test]
    fn moving_a_notebook_keeps_external_references_pointing_to_the_same_file() {
        let mut book = book();
        assert!(book.rebase_paths(Path::new("/notes/book"), Path::new("/archive/manual")));
        assert_eq!(book.path, PathBuf::from("/archive/manual/notebook.json"));
        assert_eq!(
            book.page_path(0),
            Some(PathBuf::from("/archive/manual/intro.md"))
        );
        assert_eq!(
            book.page_path(2),
            Some(PathBuf::from("/notes/reference.md"))
        );
        assert!(book.rebase_paths(
            Path::new("/notes/reference.md"),
            Path::new("/notes/renamed.md")
        ));
        assert_eq!(book.page_path(2), Some(PathBuf::from("/notes/renamed.md")));
    }

    #[test]
    fn invalid_navigation_does_not_mutate_history() {
        let mut book = book();
        assert!(!book.navigate(99));
        assert!(!book.back());
        assert!(!book.forward());
        assert_eq!(book.history, vec![0]);
    }

    #[test]
    fn page_management_persists_without_overwriting_external_manifest_edits() {
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let manifest = dir.path().join(MANIFEST_NAME);
        std::fs::write(dir.path().join("intro.md"), "# Intro\n").unwrap();
        std::fs::write(external.path().join("reference.md"), "# Reference\n").unwrap();
        std::fs::write(&manifest, r#"{"title":"Notes","pages":["intro.md"]}"#).unwrap();
        let binding = NotebookBinding::load(&manifest).unwrap();
        let mut book = binding.session.lock().unwrap();
        let index = book
            .add_existing(external.path().join("reference.md"))
            .unwrap();
        assert_eq!(index, 1);
        book.navigate(index);
        book.move_current(false).unwrap();
        assert_eq!(book.current, 0);
        assert_eq!(book.history, vec![1, 0]);
        let saved: NotebookManifest =
            serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
        assert!(saved.pages[0].path().is_absolute());
        assert_eq!(saved.pages[1].path(), Path::new("intro.md"));
        let edited = r#"{"title":"Edited elsewhere","pages":["intro.md"]}"#;
        std::fs::write(&manifest, edited).unwrap();
        assert!(book.move_current(true).is_err());
        assert_eq!(book.current, 0);
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), edited);
    }

    #[test]
    fn accepts_simple_and_named_pages() {
        let manifest: NotebookManifest = serde_json::from_str(r#"{"title":"Notes","pages":["intro.md",{"path":"api.md","title":"API","section":"Internals"}]}"#).unwrap();
        assert_eq!(manifest.pages[1].title(), "API");
        assert_eq!(manifest.pages[1].section(), Some("Internals"));
    }
}
