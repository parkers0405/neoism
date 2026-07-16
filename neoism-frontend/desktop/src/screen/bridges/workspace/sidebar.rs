use super::*;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn notes_sidebar_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        if !self.renderer.notes_sidebar.is_visible() {
            return None;
        }
        // Notes dock right of the file tree, sharing the same middle
        // band (below the full-width top chrome, above the status bar).
        let (tree_top, tree_bottom) = self.side_panel_band();
        let tree_height = (tree_bottom - tree_top).max(0.0);
        let left = if self.renderer.file_tree.is_visible() {
            self.renderer.file_tree.width()
        } else {
            0.0
        };
        Some((
            left,
            tree_top,
            tree_height,
            self.renderer.notes_sidebar.width(),
        ))
    }

    pub(crate) fn is_hovering_notes_sidebar_resize_edge(&self) -> bool {
        let Some((left, top, height, width)) = self.notes_sidebar_bounds() else {
            return false;
        };
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let edge_x = left + width;
        mouse_y >= top && mouse_y <= top + height && (mouse_x - edge_x).abs() <= 5.0
    }

    pub(crate) fn begin_notes_sidebar_resize(&mut self) -> bool {
        if !self.is_hovering_notes_sidebar_resize_edge() {
            return false;
        }
        let scale_factor = self.sugarloaf.scale_factor();
        self.notes_sidebar_resize_state = Some(NotesSidebarResizeState {
            start_x: self.mouse.x as f32 / scale_factor,
            original_width: self.renderer.notes_sidebar.width(),
        });
        true
    }

    pub(crate) fn notes_sidebar_resize_active(&self) -> bool {
        self.notes_sidebar_resize_state.is_some()
    }

    pub(crate) fn drag_notes_sidebar_resize(&mut self) {
        let Some(state) = self.notes_sidebar_resize_state else {
            return;
        };
        let scale_factor = self.sugarloaf.scale_factor();
        let mouse_x = self.mouse.x as f32 / scale_factor;
        self.renderer.notes_sidebar.resize(
            mouse_x - state.start_x + state.original_width
                - self.renderer.notes_sidebar.width(),
        );
        self.reapply_chrome_layout();
        self.mark_dirty();
    }

    pub(crate) fn end_notes_sidebar_resize(&mut self) -> bool {
        let was_active = self.notes_sidebar_resize_state.take().is_some();
        if was_active {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
        was_active
    }

    /// Trackpad / wheel scrolling for the notes sidebar — mirrors
    /// `handle_file_tree_wheel`. Returns true when the pointer is over
    /// the panel so the gesture scrolls the note list instead of leaking
    /// into the editor/terminal pane behind it (scroll-on-hover).
    pub(crate) fn handle_notes_sidebar_wheel(
        &mut self,
        delta: &neoism_window::event::MouseScrollDelta,
    ) -> bool {
        let Some((left, top, height, width)) = self.notes_sidebar_bounds() else {
            return false;
        };
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        if mouse_x < left
            || mouse_x > left + width
            || mouse_y < top
            || mouse_y > top + height
        {
            return false;
        }
        let row_h = self.renderer.notes_sidebar.row_height().max(1.0);
        let rows_visible = self
            .renderer
            .notes_sidebar
            .visible_rows_for_panel_height(height);
        let pixels = match delta {
            // 3 rows per wheel "click" matches the file tree.
            neoism_window::event::MouseScrollDelta::LineDelta(_, y) => *y * row_h * 3.0,
            neoism_window::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
        };
        if pixels == 0.0 {
            return true;
        }
        self.renderer
            .notes_sidebar
            .scroll_pixels(pixels, rows_visible);
        self.mark_dirty();
        true
    }

    /// Re-list the open notes vault from disk — called from the shared
    /// file-tree fs-watcher path so an agent (or any external tool) that
    /// adds/deletes a page under the workspace refreshes the Alt+N panel
    /// live, without the user closing and reopening it. No-op while the
    /// panel is hidden. Returns true when a redraw is warranted.
    pub(crate) fn refresh_notes_sidebar_if_visible(&mut self) -> bool {
        if !self.renderer.notes_sidebar.is_visible() {
            return false;
        }
        // A joined workspace's notes live on the server — a local fs
        // re-walk would wipe the listing to empty; re-request instead.
        if self.context_manager.current_workspace_is_remote_joined() {
            self.request_remote_notes_listing();
            return true;
        }
        self.renderer.notes_sidebar.refresh_notes();
        true
    }

    pub(crate) fn handle_notes_sidebar_click(&mut self) -> bool {
        use neoism_ui::panels::notes_sidebar::NotesSidebarHit;

        if !self.renderer.notes_sidebar.is_visible() {
            return false;
        }
        let scale = self.sugarloaf.scale_factor();
        let x = self.mouse.x as f32 / scale;
        let y = self.mouse.y as f32 / scale;
        let Some(hit) = self.renderer.notes_sidebar.hit_test(x, y) else {
            return false;
        };
        self.renderer.notes_sidebar.set_focused(true);
        self.renderer.file_tree.set_focused(false);
        match hit {
            NotesSidebarHit::Settings => self.open_notes_settings_menu(),
            NotesSidebarHit::CreateFirstNote => {
                if let Some(dir) = self.notes_sidebar_create_target() {
                    self.open_notes_new_file_prompt(dir);
                }
            }
            NotesSidebarHit::WorkspacePicker => {
                self.open_notes_vault_menu(x, y);
            }
            NotesSidebarHit::NoteIcon(index) => {
                self.renderer.notes_sidebar.set_selected(index);
                if let Some(path) = self.renderer.notes_sidebar.note_path(index) {
                    self.open_notes_icon_menu(path, x, y);
                }
            }
            NotesSidebarHit::Note(index) => {
                self.renderer.notes_sidebar.set_selected(index);
                if self.renderer.notes_sidebar.note_is_dir(index) {
                    self.renderer.notes_sidebar.toggle_selected_dir();
                } else {
                    self.renderer.notes_sidebar.set_focused(false);
                    if let Some(path) = self.renderer.notes_sidebar.note_path(index) {
                        self.open_path_from_notes_sidebar(path);
                    }
                }
            }
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn handle_notes_sidebar_context_click(&mut self) -> bool {
        use neoism_ui::panels::notes_sidebar::NotesSidebarHit;

        if !self.renderer.notes_sidebar.is_visible() {
            return false;
        }
        let scale = self.sugarloaf.scale_factor();
        let x = self.mouse.x as f32 / scale;
        let y = self.mouse.y as f32 / scale;
        let Some(hit) = self.renderer.notes_sidebar.hit_test(x, y) else {
            return false;
        };
        self.renderer.notes_sidebar.set_focused(true);
        self.renderer.file_tree.set_focused(false);

        let target = match hit {
            NotesSidebarHit::Note(index) | NotesSidebarHit::NoteIcon(index) => {
                self.renderer.notes_sidebar.set_selected(index);
                self.renderer.notes_sidebar.note_path(index)
            }
            NotesSidebarHit::WorkspacePicker
            | NotesSidebarHit::Settings
            | NotesSidebarHit::CreateFirstNote => {
                self.renderer.notes_sidebar.workspace_path()
            }
        };
        let Some(target) = target else {
            return true;
        };
        self.open_notes_sidebar_context_menu_for_path(target, x, y);
        true
    }

    pub(crate) fn handle_notes_sidebar_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
    ) -> bool {
        use neoism_window::keyboard::{Key, NamedKey};

        let mods = self.modifiers.state();
        if mods.alt_key()
            && !mods.control_key()
            && !mods.super_key()
            && matches!(&key.logical_key, Key::Named(NamedKey::ArrowDown))
        {
            self.renderer.notes_sidebar.select_selector();
            return true;
        }
        // Ctrl+D / Ctrl+U — vim half-page jumps. Consuming them here is
        // what keeps Ctrl+D from falling through to the terminal behind
        // the panel as an EOF (`^D`), which closes the shell and takes
        // the window down with it. Mirrors the file tree's guard.
        if mods.control_key() && !mods.alt_key() && !mods.super_key() {
            if let Key::Character(s) = &key.logical_key {
                match s.as_str() {
                    "d" => {
                        self.renderer.notes_sidebar.select_half_page_down();
                        return true;
                    }
                    "u" => {
                        self.renderer.notes_sidebar.select_half_page_up();
                        return true;
                    }
                    _ => {}
                }
            }
        }
        if mods.alt_key() || mods.control_key() || mods.super_key() {
            return false;
        }
        match &key.logical_key {
            // Numeric count prefix (`5j`, `12G`). Accumulated into the
            // sidebar's pending count, consumed by the next motion.
            Key::Character(s)
                if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) =>
            {
                for c in s.chars() {
                    if let Some(d) = c.to_digit(10) {
                        self.renderer.notes_sidebar.push_count_digit(d);
                    }
                }
                true
            }
            // `gg` jumps to the top (a lone `g` arms the pair).
            Key::Character(s) if s == "g" => {
                if self.renderer.notes_sidebar.note_g() {
                    self.renderer.notes_sidebar.select_first();
                }
                true
            }
            // `G` / `$` jump to the bottom; `<count>G` jumps to that row.
            Key::Character(s) if s == "G" => {
                match self.renderer.notes_sidebar.pending_count() {
                    Some(n) => self.renderer.notes_sidebar.goto_row(n),
                    None => self.renderer.notes_sidebar.select_last(),
                }
                true
            }
            Key::Character(s) if s == "$" => {
                self.renderer.notes_sidebar.select_last();
                true
            }
            Key::Character(s) if s == "j" => {
                self.notes_sidebar_move(true);
                true
            }
            Key::Character(s) if s == "k" => {
                self.notes_sidebar_move(false);
                true
            }
            Key::Character(s) if s == "a" => {
                self.renderer.notes_sidebar.clear_pending();
                if let Some(dir) = self.notes_sidebar_create_target() {
                    self.open_notes_new_file_prompt(dir);
                }
                true
            }
            Key::Character(s) if s == "f" => {
                self.renderer.notes_sidebar.clear_pending();
                if let Some(dir) = self.notes_sidebar_target_dir() {
                    self.open_file_tree_new_folder_prompt(dir);
                }
                true
            }
            Key::Character(s) if s == "r" => {
                self.renderer.notes_sidebar.clear_pending();
                if let Some(path) = self.renderer.notes_sidebar.selected_note_path() {
                    self.open_file_tree_rename_prompt(path);
                }
                true
            }
            Key::Character(s) if s == "d" => {
                self.renderer.notes_sidebar.clear_pending();
                if let Some(path) = self.renderer.notes_sidebar.selected_note_path() {
                    self.confirm_delete_file_tree_path(path);
                }
                true
            }
            Key::Character(s) if s == "m" || s == " " => {
                self.renderer.notes_sidebar.clear_pending();
                if self.renderer.notes_sidebar.is_selector_selected() {
                    self.open_notes_vault_menu_for_selector();
                } else {
                    self.open_notes_sidebar_context_menu_for_selection();
                }
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.notes_sidebar_move(true);
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.notes_sidebar_move(false);
                true
            }
            // Vault selector → share icon → ⋮ menu caret walk. Consumed
            // either way so plain arrows never leak past the panel.
            Key::Named(NamedKey::ArrowRight) => {
                self.renderer.notes_sidebar.clear_pending();
                let _ = self.renderer.notes_sidebar.move_horizontal_focus(true);
                true
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.renderer.notes_sidebar.clear_pending();
                let _ = self.renderer.notes_sidebar.move_horizontal_focus(false);
                true
            }
            Key::Named(NamedKey::Enter) => {
                self.renderer.notes_sidebar.clear_pending();
                if self.renderer.notes_sidebar.is_settings_selected() {
                    self.open_notes_settings_menu();
                    return true;
                }
                if self.renderer.notes_sidebar.is_selector_selected() {
                    self.open_notes_vault_menu_for_selector();
                    return true;
                }
                if self
                    .renderer
                    .notes_sidebar
                    .note_is_dir(self.renderer.notes_sidebar.selected_index())
                {
                    self.renderer.notes_sidebar.toggle_selected_dir();
                    return true;
                }
                if let Some(path) = self.renderer.notes_sidebar.selected_note_path() {
                    self.renderer.notes_sidebar.set_focused(false);
                    self.open_path_from_notes_sidebar(path);
                }
                true
            }
            Key::Named(NamedKey::Escape) => {
                self.renderer.notes_sidebar.set_focused(false);
                true
            }
            _ => false,
        }
    }

    /// Move the notes selection one motion (`j`/`k`/arrows). Honours a
    /// pending vim count: `5j` steps five rows (clamped), while a plain
    /// `j` keeps the single-step behaviour that also walks onto the vault
    /// selector past the last row.
    fn notes_sidebar_move(&mut self, down: bool) {
        match self.renderer.notes_sidebar.pending_count() {
            Some(_) => {
                let n = self.renderer.notes_sidebar.take_count();
                if down {
                    self.renderer.notes_sidebar.select_next_by(n);
                } else {
                    self.renderer.notes_sidebar.select_prev_by(n);
                }
            }
            None => {
                self.renderer.notes_sidebar.take_count();
                if down {
                    self.renderer.notes_sidebar.select_next();
                } else {
                    self.renderer.notes_sidebar.select_prev();
                }
            }
        }
    }

    fn notes_sidebar_target_dir(&self) -> Option<PathBuf> {
        let path = self
            .renderer
            .notes_sidebar
            .selected_note_path()
            .or_else(|| self.renderer.notes_sidebar.workspace_path())?;
        if path.is_dir() {
            Some(path)
        } else {
            path.parent().map(Path::to_path_buf)
        }
    }

    /// Create-target that survives an EMPTY panel: a sidebar opened
    /// before its vault resolved (or whose vault dir was never created)
    /// has no workspace path, so `a` / "+ New note" silently did
    /// nothing. Resolve + create the vault, then retry. Remote-joined
    /// workspaces stay None — their notes live on the host.
    pub(crate) fn notes_sidebar_create_target(&mut self) -> Option<PathBuf> {
        if self.context_manager.current_workspace_is_remote_joined() {
            // Remote notes live under the workspace's `Notes/` on the
            // server; `is_dir` can't vouch for a path that isn't on
            // this disk, so hand back the panel's root as-is.
            return self.renderer.notes_sidebar.workspace_path();
        }
        if let Some(dir) = self.notes_sidebar_target_dir().filter(|dir| dir.is_dir()) {
            return Some(dir);
        }
        self.assign_local_vault_to_notes_sidebar();
        self.notes_sidebar_target_dir()
    }

    fn open_path_from_notes_sidebar(&mut self, path: PathBuf) {
        if crate::editor::markdown::state::is_markdown_path(&path) {
            self.open_path_in_markdown(path);
        } else {
            self.open_path_in_editor(path);
        }
    }

    pub(crate) fn open_notes_new_file_prompt(&mut self, dir: PathBuf) {
        use neoism_ui::widgets::modal::{
            ModalAction, ModalButton, ModalInputSpec, ModalSpec,
        };

        let label = self.file_tree_display_path(&dir);
        self.renderer.modal.open(ModalSpec {
            title: "New Note".to_string(),
            body: format!("Create a Markdown note under `{label}`."),
            meta: "Names without an extension are saved as .md and appear in the vault list.".to_string(),
            input: Some(ModalInputSpec {
                value: "".to_string(),
                placeholder: "Note name".to_string(),
            }),
            buttons: vec![
                ModalButton::new(
                    "Create",
                    "Enter",
                    ModalAction::NotesNewFile {
                        dir: dir.display().to_string(),
                        name: String::new(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        self.mark_dirty();
    }

    pub(crate) fn create_notes_file(&mut self, dir: PathBuf, name: String) {
        use neoism_ui::panels::notifications::NotificationLevel;

        let mut name = name.trim().to_string();
        if name.is_empty() {
            self.renderer.notifications.push(
                "Note name cannot be empty".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        if Path::new(&name).extension().is_none() {
            name.push_str(".md");
        }
        // Joined workspace: the note is created ON THE SERVER through
        // the files plane (same op the remote tree uses). The
        // `FileCreated` reply opens it in markdown and re-lists the
        // panel.
        if self.context_manager.current_workspace_is_remote_joined() {
            if let Some(remote_root) = self.renderer.file_tree.remote_root() {
                let rel_dir = dir
                    .strip_prefix(&remote_root)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| dir.to_string_lossy().into_owned());
                if self.send_remote_files_op(
                    neoism_protocol::files::FilesClientMessage::CreateFile {
                        dir: rel_dir,
                        name: name.clone(),
                    },
                ) {
                    self.renderer.notifications.push(
                        format!("Creating note {name} on the server…"),
                        NotificationLevel::Info,
                    );
                    self.mark_dirty();
                    return;
                }
            }
        }
        let path = dir.join(name);
        if path.exists() {
            self.renderer.notifications.push(
                "A file or folder already exists there.".to_string(),
                NotificationLevel::Warn,
            );
            self.mark_dirty();
            return;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.renderer.modal.close();
                self.invalidate_note_index_for_path(&path);
                self.rebuild_note_graph_for_path(&path);
                self.renderer.notes_sidebar.refresh_notes();
                self.renderer.notes_sidebar.set_focused(true);
                self.renderer.notifications.push(
                    format!("Created note {}", path.display()),
                    NotificationLevel::Info,
                );
            }
            Err(err) => self.renderer.notifications.push(
                format!("Create note failed: {err}"),
                NotificationLevel::Error,
            ),
        }
        self.mark_dirty();
    }

    /// The ⋮ header menu: create actions that always target the vault the
    /// sidebar is currently viewing.
    pub(crate) fn open_notes_create_menu(&mut self, x: f32, y: f32) {
        use neoism_ui::panels::context_menu::{ContextMenuAction, ContextMenuItem};
        use neoism_ui::widgets::modal::ModalAction;

        let dir = self.notes_creation_dir().display().to_string();
        let items = vec![
            ContextMenuItem::new(
                "New Note",
                "a",
                ContextMenuAction::Modal(
                    ModalAction::NotesPromptNewFile { dir: dir.clone() }.into(),
                ),
            ),
            ContextMenuItem::new(
                "New Drawing",
                "p",
                ContextMenuAction::Modal(
                    ModalAction::NotesNewDrawing { dir: dir.clone() }.into(),
                ),
            ),
            ContextMenuItem::new(
                "New Folder",
                "f",
                ContextMenuAction::Modal(
                    ModalAction::FileTreePromptNewFolder { dir }.into(),
                ),
            ),
        ];
        let scale = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        self.renderer.context_menu.open(
            "Create".to_string(),
            items,
            x,
            y,
            size.width as f32 / scale,
            self.context_menu_logical_height(),
        );
        self.mark_dirty();
    }
}
