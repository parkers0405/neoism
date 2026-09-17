use super::*;
use neoism_ui::editor::documentation_notebook::{
    NotebookAction, NotebookBinding, NotebookInput, MANIFEST_NAME,
};
use std::path::{Path, PathBuf};

impl Screen<'_> {
    fn documentation_notebook_binding(&self, path: &Path) -> Option<NotebookBinding> {
        self.context_manager
            .current_grid()
            .contexts()
            .values()
            .find_map(|item| {
                item.context()
                    .markdown
                    .as_ref()?
                    .documentation_notebook
                    .as_ref()
                    .filter(|binding| binding.path == path)
                    .cloned()
            })
    }

    pub(crate) fn rebind_documentation_notebook_paths(&mut self, old: &Path, new: &Path) {
        let mut bindings = std::collections::HashMap::new();
        for grid in self.context_manager.contexts_mut() {
            for item in grid.contexts_mut().values_mut() {
                if let Some(pane) = item.context_mut().markdown.as_mut() {
                    if let Some(binding) = &pane.documentation_notebook {
                        bindings
                            .entry(binding.path.clone())
                            .or_insert_with(|| binding.clone());
                        if let Ok(suffix) = pane.path.strip_prefix(old) {
                            pane.path = new.join(suffix);
                        }
                    }
                }
            }
        }
        let mut renamed = std::collections::HashMap::new();
        for (previous, binding) in bindings {
            let result = binding
                .session
                .lock()
                .map_err(|_| "Notebook lock failed".to_string())
                .and_then(|mut book| {
                    if book.rebase_paths(old, new) {
                        book.persist()?;
                    }
                    Ok(book.path.clone())
                });
            match result {
                Ok(path) => {
                    renamed.insert(previous, path);
                }
                Err(error) => self.file_tree_notify(
                    format!("Notebook references need attention: {error}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                ),
            }
        }
        for grid in self.context_manager.contexts_mut() {
            for item in grid.contexts_mut().values_mut() {
                if let Some(binding) = item
                    .context_mut()
                    .markdown
                    .as_mut()
                    .and_then(|pane| pane.documentation_notebook.as_mut())
                {
                    if let Some(path) = renamed.get(&binding.path) {
                        binding.path = path.clone();
                    }
                }
            }
        }
        for (previous, path) in renamed {
            if previous != path {
                self.renderer
                    .buffer_tabs
                    .rename_path(&previous, path.clone());
                for tabs in self.renderer.pane_tabs.values_mut() {
                    tabs.rename_path(&previous, path.clone());
                }
                for tabs in self.workspace_buffer_tabs.values_mut() {
                    tabs.rename_path(&previous, path.clone());
                }
            }
        }
    }

    pub(crate) fn open_documentation_notebook(&mut self, path: PathBuf) {
        if self.context_manager.current_workspace_is_remote_joined() {
            self.file_tree_notify(
                "Folder notebooks currently require a local workspace",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }
        let path = if path.is_dir() {
            path.join(MANIFEST_NAME)
        } else {
            path
        };
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let binding = self
            .documentation_notebook_binding(&path)
            .map(Ok)
            .unwrap_or_else(|| {
                let binding = NotebookBinding::load(&path)?;
                if let Ok(source) = std::fs::read(resume_path(&binding.path)) {
                    if let Ok(page) = serde_json::from_slice::<PathBuf>(&source) {
                        if let Ok(mut book) = binding.session.lock() {
                            if let Some(index) = book.index_of(&page).filter(|&index| {
                                book.page_path(index).is_some_and(|path| path.is_file())
                            }) {
                                book.current = index;
                                book.scroll_rows = index.saturating_sub(2);
                                book.history = vec![index];
                            }
                        }
                    }
                }
                Ok::<_, String>(binding)
            });
        match binding {
            Ok(binding) => {
                self.activate_documentation_notebook_page(binding);
            }
            Err(error) => self.file_tree_notify(
                error,
                neoism_ui::panels::notifications::NotificationLevel::Error,
            ),
        }
    }

    fn activate_documentation_notebook_page(&mut self, binding: NotebookBinding) -> bool {
        let (path, title) = {
            let Ok(book) = binding.session.lock() else {
                return false;
            };
            let Some(path) = book.page_path(book.current) else {
                return false;
            };
            (path, book.manifest.title.clone())
        };
        if !path.is_file() {
            self.file_tree_notify(
                format!("Notebook page not found: {}", path.display()),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return false;
        }
        if self
            .context_manager
            .markdown_pane_mut_by_path(&path)
            .is_some_and(|pane| {
                pane.documentation_notebook
                    .as_ref()
                    .is_some_and(|other| other.path != binding.path)
            })
        {
            self.file_tree_notify("This page is already open in another notebook; close that notebook before opening it here", neoism_ui::panels::notifications::NotificationLevel::Warn);
            return false;
        }
        // The old pane remains alive with its document, undo, caret and scroll
        // state. Only its tab presentation is grouped under the manifest.
        self.sync_active_markdown_modified();
        self.activate_markdown_path(path.clone());
        let vim = self.renderer.vim_mode;
        let spellcheck = self.renderer.markdown_spellcheck;
        let Some(pane) = self
            .context_manager
            .current_mut()
            .markdown
            .as_mut()
            .filter(|pane| pane.path == path)
        else {
            return false;
        };
        pane.documentation_notebook = Some(binding.clone());
        pane.vim_enabled = vim;
        pane.spellcheck_enabled = spellcheck;
        if !vim {
            pane.enter_insert();
        }
        self.clear_current_workspace_buf_enter_guard();
        self.renderer.buffer_tabs.ensure_terminal_tab();
        let ix = self
            .renderer
            .buffer_tabs
            .open_markdown(binding.path.clone());
        self.renderer.buffer_tabs.set_title(ix, title);
        self.renderer
            .buffer_tabs
            .set_path_icon(&binding.path, Some("\u{f02d}".into()));
        let state_path = resume_path(&binding.path);
        if let Some(parent) = state_path.parent() {
            let saved = std::fs::create_dir_all(parent).and_then(|_| {
                let bytes = serde_json::to_vec(&path).map_err(std::io::Error::other)?;
                std::fs::write(&state_path, bytes)
            });
            if let Err(error) = saved {
                tracing::warn!(%error, "Could not remember notebook page");
            }
        }
        self.renderer.file_tree.set_active_path(Some(path));
        if let Some(id) = self.current_workspace_id() {
            self.workspace_editor_active_paths
                .insert(id, binding.path.clone());
        }
        self.sync_documentation_notebook_modified(&binding.path);
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn sync_documentation_notebook_modified(&mut self, manifest: &Path) {
        let modified_pages: std::collections::HashSet<_> = self
            .context_manager
            .current_grid()
            .contexts()
            .values()
            .filter_map(|item| item.context().markdown.as_ref())
            .filter(|pane| pane.tab_path() == manifest && pane.is_dirty())
            .map(|pane| pane.path.clone())
            .collect();
        let modified = !modified_pages.is_empty();
        if let Some(binding) = self.documentation_notebook_binding(manifest) {
            if let Ok(mut book) = binding.session.lock() {
                book.modified_pages = modified_pages;
            }
        }
        self.renderer.buffer_tabs.set_modified(manifest, modified);
        for tabs in self.renderer.pane_tabs.values_mut() {
            tabs.set_modified(manifest, modified);
        }
    }

    pub(crate) fn handle_documentation_notebook_click(&mut self, x: f32, y: f32) -> bool {
        let Some(binding) = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .and_then(|pane| pane.documentation_notebook.clone())
        else {
            return false;
        };
        let action = binding
            .session
            .lock()
            .ok()
            .and_then(|book| book.action_at(x, y));
        let Some(action) = action else {
            return binding.session.lock().ok().is_some_and(|book| {
                book.rail_rect.is_some_and(|r| {
                    x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3]
                })
            });
        };
        match action {
            NotebookAction::AddPage => {
                self.open_documentation_notebook_prompt(NotebookInput::AddPage)
            }
            NotebookAction::AddExisting => {
                self.open_documentation_notebook_prompt(NotebookInput::AddExisting)
            }
            NotebookAction::MoveUp | NotebookAction::MoveDown => {
                let result = binding
                    .session
                    .lock()
                    .map_err(|_| "Notebook lock failed".to_string())
                    .and_then(|mut book| {
                        book.move_current(action == NotebookAction::MoveDown)
                    });
                if let Err(error) = result {
                    self.file_tree_notify(
                        error,
                        neoism_ui::panels::notifications::NotificationLevel::Error,
                    );
                }
            }
            NotebookAction::ToggleSection(section) => {
                if let Ok(mut book) = binding.session.lock() {
                    if !book.collapsed_sections.remove(&section) {
                        book.collapsed_sections.insert(section);
                    }
                }
            }
            NotebookAction::ToggleContents => {
                if let Ok(mut book) = binding.session.lock() {
                    book.contents_open = !book.contents_open;
                }
            }
            _ => {
                self.navigate_documentation_notebook(binding, action);
            }
        }
        self.mark_dirty();
        true
    }

    fn navigate_documentation_notebook(
        &mut self,
        binding: NotebookBinding,
        action: NotebookAction,
    ) {
        let before = {
            let Ok(mut book) = binding.session.lock() else {
                return;
            };
            let before = (book.current, book.history.clone(), book.history_index);
            match action {
                NotebookAction::Page(index) => {
                    book.navigate(index);
                }
                NotebookAction::Back => {
                    book.back();
                }
                NotebookAction::Forward => {
                    book.forward();
                }
                _ => return,
            }
            if let Some(section) = book.manifest.pages[book.current]
                .section()
                .map(str::to_string)
            {
                book.collapsed_sections.remove(&section);
            }
            if !book.hit_regions.iter().any(|(rect, action)| {
                *action == NotebookAction::Page(book.current)
                    && book.rail_rect.is_some_and(|rail| {
                        rect[0] < rail[0] + rail[2] && rect[1] < rail[1] + rail[3]
                    })
            }) {
                book.scroll_rows = book.current.saturating_sub(2);
            }
            before
        };
        if !self.activate_documentation_notebook_page(binding.clone()) {
            if let Ok(mut book) = binding.session.lock() {
                (book.current, book.history, book.history_index) = before;
            }
        }
    }

    pub(crate) fn follow_documentation_notebook_link(
        &mut self,
        path: &Path,
        line: Option<usize>,
    ) -> bool {
        let Some(binding) = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .and_then(|pane| pane.documentation_notebook.clone())
        else {
            return false;
        };
        let index = binding
            .session
            .lock()
            .ok()
            .and_then(|book| book.index_of(path));
        let Some(index) = index else {
            return false;
        };
        self.navigate_documentation_notebook(binding, NotebookAction::Page(index));
        if let Some(line) = line {
            if let Some(pane) = self
                .context_manager
                .current_mut()
                .markdown
                .as_mut()
                .filter(|pane| pane.path == path)
            {
                pane.jump_to_line(line.max(1));
                pane.flash_line(line.max(1));
            }
        }
        true
    }

    pub(crate) fn create_documentation_notebook_in(&mut self, dir: PathBuf) {
        if self.context_manager.current_workspace_is_remote_joined() {
            self.file_tree_notify("Folder notebooks currently require a local workspace", neoism_ui::panels::notifications::NotificationLevel::Warn);
            return;
        }
        let result = NotebookBinding::create_untitled_in(&dir);
        match result {
            Ok(binding) => {
                self.renderer.notes_sidebar.reveal_dir(&dir);
                self.renderer.notes_sidebar.refresh_notes();
                if let Some(folder) = binding.path.parent() { self.renderer.notes_sidebar.select_path(folder); }
                self.refresh_file_tree_entries();
                self.open_documentation_notebook(binding.path);
            }
            Err(error) => self.file_tree_notify(error, neoism_ui::panels::notifications::NotificationLevel::Error),
        }
        self.mark_dirty();
    }

    pub(crate) fn open_documentation_notebook_prompt(&mut self, kind: NotebookInput) {
        let notebook = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .and_then(|pane| pane.documentation_notebook.as_ref())
            .map(|binding| binding.path.clone());
        self.open_documentation_notebook_prompt_at(kind, notebook);
    }

    pub(crate) fn open_documentation_notebook_prompt_at(
        &mut self,
        kind: NotebookInput,
        notebook: Option<PathBuf>,
    ) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };
        let (title, body, placeholder) = match kind {
            NotebookInput::Create => ("Create Documentation Notebook", "Enter a folder path. Existing Markdown files become pages; files are never moved or replaced.", "Folder path"),
            NotebookInput::Open => ("Open Documentation Notebook", "Enter the notebook folder or its notebook.json path.", "Folder or notebook.json"),
            NotebookInput::AddPage => ("Add Notebook Page", "Enter a page title. A new Markdown file will be created in this notebook.", "Page title"),
            NotebookInput::AddExisting => ("Link Existing Page", "Enter a Markdown file path. Relative paths start at the notebook folder; outside files are referenced, not copied.", "Path to Markdown file"),
        };
        self.renderer.modal.open(ModalSpec {
            title: title.into(),
            body: body.into(),
            meta: notebook
                .as_ref()
                .and_then(|path| path.parent())
                .map(|folder| format!("Folder: {}", folder.display()))
                .unwrap_or_else(|| {
                    "Documentation notebook / ordinary Markdown files".into()
                }),
            input: Some(ModalInputSpec {
                value: String::new(),
                placeholder: placeholder.into(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Continue",
                    "Enter",
                    ModalAction::DocumentationNotebook {
                        kind,
                        notebook,
                        value: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    pub(crate) fn submit_documentation_notebook_input(
        &mut self,
        kind: NotebookInput,
        notebook: Option<PathBuf>,
        value: String,
    ) {
        let result =
            self.apply_documentation_notebook_input(kind, notebook, value.trim());
        if let Err(error) = result {
            self.file_tree_notify(
                error,
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
        self.mark_dirty();
    }

    fn apply_documentation_notebook_input(
        &mut self,
        kind: NotebookInput,
        notebook: Option<PathBuf>,
        value: &str,
    ) -> Result<(), String> {
        if value.is_empty() {
            return Err("Enter a title or path".into());
        }
        if self.context_manager.current_workspace_is_remote_joined() {
            return Err("Folder notebooks currently require a local workspace".into());
        }
        let root = notebook
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(|| self.active_pane_workspace_root())
            .or_else(|| self.active_workspace_root.clone())
            .or_else(|| std::env::current_dir().ok())
            .ok_or("No workspace directory")?;
        let input_path = if let Some(suffix) = value.strip_prefix("~/") {
            dirs::home_dir()
                .ok_or("Home directory is unavailable")?
                .join(suffix)
        } else {
            root.join(value)
        };
        match kind {
            NotebookInput::Open => {
                self.open_documentation_notebook(input_path);
            }
            NotebookInput::Create => {
                let binding = NotebookBinding::create(&input_path, None)?;
                self.open_documentation_notebook(binding.path);
                self.renderer.file_tree.refresh();
                self.renderer.notes_sidebar.refresh_notes();
            }
            NotebookInput::AddPage | NotebookInput::AddExisting => {
                let binding = notebook
                    .as_ref()
                    .and_then(|path| self.documentation_notebook_binding(path))
                    .ok_or("Notebook is no longer open")?;
                let path = if kind == NotebookInput::AddPage {
                    if value.contains(['/', '\\']) || value == "." || value == ".." {
                        return Err("Use a page title without path separators".into());
                    }
                    let path = root.join(format!("{}.md", value.trim_end_matches(".md")));
                    use std::io::Write;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map_err(|error| error.to_string())?;
                    writeln!(file, "# {value}\n").map_err(|error| error.to_string())?;
                    path
                } else {
                    input_path
                };
                let index = binding
                    .session
                    .lock()
                    .map_err(|_| "Notebook lock failed")?
                    .add_existing(path)?;
                self.navigate_documentation_notebook(
                    binding,
                    NotebookAction::Page(index),
                );
                self.renderer.notes_sidebar.refresh_notes();
            }
        }
        Ok(())
    }
}

fn resume_path(manifest: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let key = format!(
        "{:x}",
        Sha256::digest(manifest.to_string_lossy().as_bytes())
    );
    neoism_backend::config::config_dir_path()
        .join("notebooks")
        .join(format!("{key}.json"))
}
