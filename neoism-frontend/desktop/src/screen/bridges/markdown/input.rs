use super::*;
use neoism_backend::clipboard::{Clipboard, ClipboardType};
use neoism_window::event::{ElementState, MouseButton};
use neoism_window::keyboard::{Key, ModifiersState, NamedKey};
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn create_missing_markdown_note(&mut self, path: &Path) -> bool {
        use neoism_ui::panels::notifications::NotificationLevel;

        if let Some(root) = self
            .active_workspace_root
            .clone()
            .or_else(|| self.active_pane_workspace_root())
        {
            if path.is_absolute() && !path.starts_with(&root) {
                self.renderer.notifications.push(
                    format!(
                        "Refusing to create note outside workspace: {}",
                        path.display()
                    ),
                    NotificationLevel::Warn,
                );
                return false;
            }
        }

        let title = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| stem.trim())
            .filter(|stem| !stem.is_empty())
            .unwrap_or("Untitled");
        let source = format!("# {title}\n\n");
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .and_then(|mut file| {
                    use std::io::Write;
                    file.write_all(source.as_bytes())
                })
        })();
        match result {
            Ok(()) => {
                self.refresh_file_tree_entries();
                self.renderer.notifications.push(
                    format!("Created note {}", path.display()),
                    NotificationLevel::Info,
                );
                true
            }
            Err(err) if path.exists() => {
                tracing::debug!(
                    target: "neoism::markdown",
                    path = %path.display(),
                    error = %err,
                    "markdown note appeared while creating missing link target"
                );
                true
            }
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Could not create note {}: {err}", path.display()),
                    NotificationLevel::Error,
                );
                false
            }
        }
    }

    pub fn handle_markdown_hover(&mut self) -> bool {
        let [x, y] = self.markdown_mouse_logical();
        self.context_manager
            .current_mut()
            .active_markdown_mut()
            .is_some_and(|markdown| markdown.hover_at(x, y))
    }

    pub fn markdown_handle_hovered(&self) -> bool {
        self.context_manager
            .current()
            .active_markdown()
            .is_some_and(|markdown| markdown.handle_hovered())
    }

    pub fn markdown_link_hovered(&self) -> bool {
        let [x, y] = self.markdown_mouse_logical();
        self.context_manager
            .current()
            .active_markdown()
            .is_some_and(|markdown| markdown.link_at(x, y).is_some())
    }

    pub fn markdown_notebook_action_hovered(&self) -> bool {
        self.context_manager
            .current()
            .active_markdown()
            .is_some_and(|markdown| markdown.notebook_action_hovered())
    }

    pub fn markdown_drag_active(&self) -> bool {
        self.draw_over_note.is_some()
            || self
                .context_manager
                .current()
                .active_markdown()
                .is_some_and(|markdown| markdown.is_dragging())
    }

    pub fn markdown_grab_drag_active(&self) -> bool {
        self.context_manager
            .current()
            .active_markdown()
            .is_some_and(|markdown| markdown.is_grab_dragging())
    }

    fn yank_contact_link(&mut self, target: &str, clipboard: &mut Clipboard) -> bool {
        let value = neoism_ui::editor::markdown::markdown_contact_value(target);
        let Some(value) = value else {
            return false;
        };
        let value = value.trim().to_string();
        clipboard.set(ClipboardType::Clipboard, value.clone());
        self.renderer.notifications.push(
            format!("Yanked `{value}`"),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        self.mark_dirty();
        true
    }

    pub fn handle_markdown_mouse_press(
        &mut self,
        button: MouseButton,
        clipboard: &mut Clipboard,
    ) -> bool {
        if self.context_manager.current().active_markdown().is_none() {
            return false;
        }
        // Draw mode: route left-press to the ink pane (tools/toolbar).
        if button == MouseButton::Left && self.draw_over_note.is_some() {
            let [x, y] = self.markdown_mouse_logical();
            if self.draw_over_note_pointer(0, x, y) {
                return true;
            }
        }
        if button == MouseButton::Right {
            if self.context_manager.current().epub.is_some() {
                let [x, y] = self.markdown_mouse_logical();
                return self.open_epub_annotation_menu_at(x, y, false);
            }
            return self.open_markdown_spelling_menu();
        }
        if button != MouseButton::Left {
            return false;
        }
        let [x, y] = self.markdown_mouse_logical();
        if let Some((cell_index, action)) = self
            .context_manager
            .current()
            .notebook
            .as_ref()
            .and_then(|notebook| notebook.cell_action_at_point(x, y))
        {
            match action {
                neoism_ui::editor::notebook::NotebookCellAction::Run => {
                    self.run_notebook_cell(cell_index);
                }
                neoism_ui::editor::notebook::NotebookCellAction::RunAndBelow => {
                    self.run_notebook_cell_and_below_from(cell_index);
                }
                neoism_ui::editor::notebook::NotebookCellAction::ClearOutput => {
                    self.clear_notebook_cell_output(cell_index);
                }
            }
            return true;
        }
        // Books are immutable reading surfaces. Links and text selection are
        // interactive, but Markdown's block conversion/task/reorder affordances
        // must never mutate chapter source.
        if self.context_manager.current().epub.is_some() {
            if let Some(target) = self
                .context_manager
                .current()
                .active_markdown()
                .and_then(|markdown| markdown.link_at(x, y))
            {
                let contact = target.path.to_string_lossy().into_owned();
                if !self.yank_contact_link(&contact, clipboard) {
                    self.open_markdown_link_target(target);
                }
                return true;
            }
            if let Some(markdown) =
                self.context_manager.current_mut().active_markdown_mut()
            {
                let handled = markdown.click_at(x, y);
                // `click_at` seeds the mouse selection anchor in Insert mode;
                // returning to Normal keeps that anchor so a subsequent drag
                // still enters Visual mode without making the book editable.
                markdown.enter_normal();
                if handled {
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                    return true;
                }
            }
            // Edge turns are a fallback for empty canvas only. Text under the
            // pointer always wins so dragging from a line near either margin
            // starts a selection instead of unexpectedly changing pages.
            if let Some(direction) = self.epub_page_turn_direction_at(x, y) {
                self.turn_epub_page(direction);
                return true;
            }
            return true;
        }
        // Wave 7G: roster dots draw above everything in the pane's
        // top-right corner, so they win the hit-test. A hit queues a
        // centered reveal of that collaborator's cursor line.
        if self
            .context_manager
            .current_mut()
            .active_markdown_mut()
            .is_some_and(|markdown| markdown.roster_jump_at(x, y))
        {
            self.mark_dirty();
            return true;
        }
        if let Some(target) = self
            .context_manager
            .current()
            .active_markdown()
            .and_then(|markdown| markdown.link_at(x, y))
        {
            let contact = target.path.to_string_lossy().into_owned();
            if !self.yank_contact_link(&contact, clipboard) {
                self.open_markdown_link_target(target);
            }
            return true;
        }
        if let Some(rect) = self
            .context_manager
            .current_mut()
            .active_markdown_mut()
            .and_then(|markdown| markdown.block_conversion_at(x, y))
        {
            self.renderer.trail_cursor.reset();
            self.open_markdown_block_menu(Some(rect));
            return true;
        }
        let Some(markdown) = self.context_manager.current_mut().active_markdown_mut()
        else {
            return false;
        };
        if let Some(content) = markdown.copy_at(x, y) {
            clipboard.set(ClipboardType::Clipboard, content);
            self.renderer.notifications.push(
                "Copied Markdown block".to_string(),
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
            self.mark_dirty();
            return true;
        }
        if markdown.activate_table_action_at(x, y) {
            self.sync_active_markdown_modified();
            self.renderer.trail_cursor.reset();
            self.mark_dirty();
            return true;
        }
        if markdown.toggle_task_at(x, y) {
            self.sync_active_markdown_modified();
            self.mark_dirty();
            return true;
        }
        if markdown.begin_drag_at(x, y) || markdown.click_at(x, y) {
            self.renderer.trail_cursor.reset();
            self.mark_dirty();
        }
        true
    }

    pub fn handle_markdown_drag_move(&mut self) -> bool {
        if self.draw_over_note.is_some() {
            let [x, y] = self.markdown_mouse_logical();
            if self.draw_over_note_pointer(1, x, y) {
                return true;
            }
        }
        let [x, y] = self.markdown_mouse_logical();
        self.context_manager
            .current_mut()
            .active_markdown_mut()
            .is_some_and(|markdown| markdown.update_drag(x, y))
    }

    pub fn handle_markdown_mouse_release(&mut self) -> bool {
        if self.draw_over_note.is_some() {
            let [x, y] = self.markdown_mouse_logical();
            if self.draw_over_note_pointer(2, x, y) {
                return true;
            }
        }
        let epub_active = self.context_manager.current().epub.is_some();
        let [release_x, release_y] = self.markdown_mouse_logical();
        let clicked_noted_annotation = self
            .context_manager
            .current()
            .epub
            .as_ref()
            .filter(|epub| epub.markdown.visual_selection().is_none())
            .and_then(|epub| {
                epub.markdown
                    .text_position_at_point(release_x, release_y)
                    .and_then(|position| {
                        epub.annotation_at_source_position(position.line, position.col)
                    })
            })
            .is_some_and(|annotation| !annotation.note.trim().is_empty());
        let (handled, menu_rect) = if let Some(markdown) =
            self.context_manager.current_mut().active_markdown_mut()
        {
            let handled = markdown.end_drag();
            if epub_active
                && matches!(
                    markdown.mode,
                    neoism_ui::editor::markdown::MarkdownMode::Insert
                )
            {
                markdown.enter_normal();
            }
            let menu_rect = markdown.take_pending_block_menu_rect();
            (handled, menu_rect)
        } else {
            (false, None)
        };
        if let Some(rect) = menu_rect {
            self.renderer.trail_cursor.reset();
            self.open_markdown_block_menu(Some(rect));
            self.mark_dirty();
            return true;
        }
        if handled && !epub_active {
            self.sync_active_markdown_modified();
        }
        if epub_active {
            if let Some(epub) = self.context_manager.current_mut().epub.as_mut() {
                epub.capture_location();
            }
            if clicked_noted_annotation
                && self.open_epub_annotation_menu_at(release_x, release_y, true)
            {
                return true;
            }
        }
        handled
    }

    pub(crate) fn dispatch_markdown_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
        text: &str,
        clipboard: &mut Clipboard,
    ) {
        if key.state == ElementState::Released {
            return;
        }

        if self.context_manager.current().epub.is_some() {
            self.dispatch_epub_reader_key(key, mods, clipboard);
            return;
        }

        let plain = !mods.control_key() && !mods.alt_key() && !mods.super_key();
        let ctrl_only = mods.control_key() && !mods.alt_key() && !mods.super_key();

        // The page title is a virtual line, but global Normal-mode commands
        // must behave exactly as they do on a document line. Handle the
        // leader here before the title editor can swallow Space or interpret
        // the following `x` as a title deletion. Commands which move through
        // the document leave the virtual line, then continue through the
        // regular Markdown dispatcher below.
        let title_normal = self
            .context_manager
            .current()
            .active_markdown()
            .is_some_and(|markdown| {
                markdown.title_edit.is_some()
                    && matches!(
                        markdown.mode,
                        crate::editor::markdown::state::MarkdownMode::Normal
                    )
            });
        if title_normal {
            let now = std::time::Instant::now();
            if self.markdown_leader_pending.is_some_and(|started| {
                now.duration_since(started).as_millis() > LEADER_TIMEOUT_MS
            }) {
                self.markdown_leader_pending = None;
            }
            if self.markdown_leader_pending.is_some() {
                self.markdown_leader_pending = None;
                if plain
                    && matches!(key.logical_key.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("x"))
                {
                    if self.close_focused_buffer_tab() {
                        self.mark_dirty();
                    }
                    return;
                }
                if plain
                    && matches!(key.logical_key.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("h"))
                {
                    self.split_down();
                    return;
                }
            }

            let is_space = matches!(
                key.logical_key.as_ref(),
                Key::Named(NamedKey::Space)
            ) || matches!(key.logical_key.as_ref(), Key::Character(ch) if ch == " ");
            if plain && is_space {
                self.markdown_leader_pending = Some(now);
                return;
            }

            let exits_title = ctrl_only
                || matches!(
                    key.logical_key.as_ref(),
                    Key::Named(NamedKey::PageUp | NamedKey::PageDown)
                )
                || (plain
                    && matches!(
                        key.logical_key.as_ref(),
                        Key::Character(ch) if ch == ":" || ch == "/" || ch == "?"
                    ));
            if exits_title {
                if let Some(markdown) =
                    self.context_manager.current_mut().active_markdown_mut()
                {
                    markdown.cancel_title_edit();
                }
            }
        }

        // Virtual title line editing: ArrowUp (or `k` in Normal) from the
        // top of the buffer moves the cursor "up" into the big page title;
        // while active, every key drives the title edit. Enter commits and
        // renames the file; Esc/ArrowDown drop back into the document.
        {
            let mut title_handled = false;
            if let Some(markdown) =
                self.context_manager.current_mut().active_markdown_mut()
            {
                if markdown.title_edit.is_some() {
                    title_handled = true;
                    let insert_mode = matches!(
                        markdown.mode,
                        crate::editor::markdown::state::MarkdownMode::Insert
                    );
                    match key.key_without_modifiers().as_ref() {
                        Key::Named(NamedKey::Enter) => markdown.commit_title_edit(),
                        Key::Named(NamedKey::Escape) => {
                            if insert_mode {
                                // Esc on the title behaves like the
                                // body: drop to Normal (block cursor
                                // stays on the title), vim's step-left
                                // included.
                                markdown.enter_normal();
                                markdown.title_edit_move(-1);
                            } else {
                                markdown.cancel_title_edit()
                            }
                        }
                        Key::Named(NamedKey::ArrowDown) => markdown.cancel_title_edit(),
                        Key::Named(NamedKey::Backspace) => {
                            if insert_mode {
                                markdown.title_edit_backspace()
                            } else {
                                markdown.title_edit_move(-1)
                            }
                        }
                        Key::Named(NamedKey::Delete) => markdown.title_edit_delete(),
                        Key::Named(NamedKey::ArrowLeft) => markdown.title_edit_move(-1),
                        Key::Named(NamedKey::ArrowRight) => markdown.title_edit_move(1),
                        Key::Named(NamedKey::Home) => markdown.title_edit_home(),
                        Key::Named(NamedKey::End) => markdown.title_edit_end(),
                        _ if !insert_mode => {
                            // Normal mode on the title line — the usual
                            // vim keys, driving the title edit instead
                            // of the buffer.
                            if plain {
                                match key.text.as_deref() {
                                    Some("h") => markdown.title_edit_move(-1),
                                    Some("l") => markdown.title_edit_move(1),
                                    Some("0") | Some("^") => markdown.title_edit_home(),
                                    Some("$") => markdown.title_edit_end(),
                                    Some("i") => markdown.enter_insert(),
                                    Some("a") => {
                                        markdown.title_edit_move(1);
                                        markdown.enter_insert();
                                    }
                                    Some("I") => {
                                        markdown.title_edit_home();
                                        markdown.enter_insert();
                                    }
                                    Some("A") => {
                                        markdown.title_edit_end();
                                        markdown.enter_insert();
                                    }
                                    Some("x") => markdown.title_edit_delete(),
                                    Some("j") => markdown.cancel_title_edit(),
                                    _ => {}
                                }
                            }
                            // Swallow plain keys so they can't leak
                            // into the buffer beneath.
                            title_handled = plain;
                        }
                        _ => {
                            let mut inserted = false;
                            if !mods.control_key() && !mods.alt_key() && !mods.super_key()
                            {
                                if let Some(text) = key.text.as_deref() {
                                    if !text.is_empty()
                                        && text.chars().all(|c| !c.is_control())
                                    {
                                        markdown.title_edit_insert(text);
                                        inserted = true;
                                    }
                                }
                            }
                            // Swallow everything while editing except
                            // modified chords, so stray keys can't leak
                            // into the buffer beneath.
                            title_handled = inserted
                                || (!mods.control_key()
                                    && !mods.alt_key()
                                    && !mods.super_key());
                        }
                    }
                } else if plain
                    && markdown.cursor_line == 0
                    && markdown.title_edit.is_none()
                    && (matches!(
                        key.key_without_modifiers().as_ref(),
                        Key::Named(NamedKey::ArrowUp)
                    ) || (matches!(
                        markdown.mode,
                        crate::editor::markdown::state::MarkdownMode::Normal
                    ) && matches!(
                        key.key_without_modifiers().as_ref(),
                        Key::Character(ch) if ch == "k"
                    )))
                {
                    markdown.begin_title_edit();
                    title_handled = true;
                }
            }
            if title_handled {
                if let Some(new_title) = self
                    .context_manager
                    .current_mut()
                    .active_markdown_mut()
                    .and_then(|markdown| markdown.take_pending_title_rename())
                {
                    self.apply_markdown_title_rename(&new_title);
                }
                self.mark_dirty();
                return;
            }
        }
        // LSP-style value picker for `icon:`/`cover:` frontmatter lines:
        // while it is open, navigation/accept keys drive the popup instead
        // of the buffer. Esc falls through — leaving Insert closes the
        // picker naturally via `refresh_value_picker`.
        if plain {
            let mut picker_handled = false;
            let mut accepted_icon = None;
            if let Some(markdown) =
                self.context_manager.current_mut().active_markdown_mut()
            {
                if markdown.value_picker.is_some() {
                    match key.key_without_modifiers().as_ref() {
                        Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::Tab) => {
                            markdown.value_picker_move(1);
                            picker_handled = true;
                        }
                        Key::Named(NamedKey::Enter) => {
                            picker_handled = markdown.value_picker_accept();
                            if picker_handled {
                                accepted_icon = Some((
                                    markdown.path.clone(),
                                    markdown.frontmatter_property("icon"),
                                ));
                            }
                        }
                        Key::Named(NamedKey::ArrowUp) => {
                            markdown.value_picker_move(-1);
                            picker_handled = true;
                        }
                        _ => {}
                    }
                }
            }
            if picker_handled {
                // Mirror the fresh `icon:` straight onto the Alt+N row —
                // the sidebar entry was built from a disk walk and stays
                // stale until the daemon flushes the buffer.
                if let Some((path, icon)) = accepted_icon {
                    self.renderer
                        .notes_sidebar
                        .set_note_icon(&path, icon.clone());
                    self.sync_note_tab_icon(&path, icon);
                }
                self.mark_dirty();
                return;
            }
        }
        let notebook_mode = self
            .context_manager
            .current()
            .notebook
            .as_ref()
            .map(|notebook| notebook.markdown.mode);
        if let Some(mode) = notebook_mode {
            let unmodified = plain && !mods.shift_key();
            match key.key_without_modifiers().as_ref() {
                Key::Named(NamedKey::Enter) if plain && mods.shift_key() => {
                    self.run_current_notebook_cell_and_select_next();
                    return;
                }
                Key::Named(NamedKey::Escape)
                    if unmodified
                        && !matches!(
                            mode,
                            crate::editor::markdown::state::MarkdownMode::Normal
                        ) =>
                {
                    if let Some(notebook) =
                        self.context_manager.current_mut().notebook.as_mut()
                    {
                        notebook.enter_command_mode();
                    }
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                    return;
                }
                Key::Named(NamedKey::Enter)
                    if unmodified
                        && matches!(
                            mode,
                            crate::editor::markdown::state::MarkdownMode::Normal
                        ) =>
                {
                    if let Some(notebook) =
                        self.context_manager.current_mut().notebook.as_mut()
                    {
                        notebook.enter_current_cell_edit_mode();
                    }
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                    return;
                }
                Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown)
                    if unmodified
                        && matches!(
                            mode,
                            crate::editor::markdown::state::MarkdownMode::Normal
                        ) =>
                {
                    let delta = if matches!(
                        key.key_without_modifiers().as_ref(),
                        Key::Named(NamedKey::ArrowUp)
                    ) {
                        -1
                    } else {
                        1
                    };
                    if let Some(notebook) =
                        self.context_manager.current_mut().notebook.as_mut()
                    {
                        notebook.select_adjacent_cell(delta);
                    }
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                    return;
                }
                _ => {}
            }
        }
        if ctrl_only
            && matches!(
                key.key_without_modifiers().as_ref(),
                Key::Named(NamedKey::Enter)
            )
            && self.run_current_notebook_cell()
        {
            return;
        }
        let viewport = self.markdown_viewport_height();
        let mut handled = true;
        let mut snap_cursor = false;
        let mut open_block_menu = false;
        let mut open_block_menu_at = None;
        // `Some(reverse)` when `/`/`?` asked to open the shared command-
        // palette Search modal for this markdown pane (acted on after the
        // pane borrow ends, since opening the palette needs `self`).
        let mut open_markdown_search: Option<bool> = None;
        let mut arm_markdown_leader = false;
        let mut flushed_markdown_leader = false;
        let mut yank_message = None;
        let mut open_cursor_link = None;

        let markdown_mode = self
            .context_manager
            .current()
            .markdown
            .as_ref()
            .map(|markdown| markdown.mode)
            .or_else(|| {
                self.context_manager
                    .current()
                    .notebook
                    .as_ref()
                    .map(|notebook| notebook.markdown.mode)
            });
        if matches!(
            markdown_mode,
            Some(crate::editor::markdown::state::MarkdownMode::Normal)
        ) && plain
            && matches!(key.logical_key.as_ref(), Key::Character(ch) if ch == ":")
        {
            if let Some(markdown) =
                self.context_manager.current_mut().active_markdown_mut()
            {
                markdown.vim.clear_pending();
            }
            self.open_command_palette();
            return;
        }

        let now = std::time::Instant::now();
        let markdown_normal = matches!(
            markdown_mode,
            Some(crate::editor::markdown::state::MarkdownMode::Normal)
        );
        if markdown_normal {
            if let Some(started) = self.markdown_leader_pending {
                if now.duration_since(started).as_millis() > LEADER_TIMEOUT_MS {
                    self.markdown_leader_pending = None;
                    flushed_markdown_leader = true;
                }
            }
            if self.markdown_leader_pending.is_some() {
                self.markdown_leader_pending = None;
                if plain
                    && matches!(key.logical_key.as_ref(), Key::Character(ch) if ch == "x")
                {
                    let closed = self.close_focused_buffer_tab();
                    if closed {
                        self.mark_dirty();
                    }
                    return;
                }
                if plain
                    && matches!(key.logical_key.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("h"))
                {
                    self.split_down();
                    return;
                }
                flushed_markdown_leader = true;
            }
        } else {
            self.markdown_leader_pending = None;
        }

        let modifier_class =
            neoism_ui::editor::markdown::bridge_policy::MarkdownBridgeModifiers {
                shift: mods.shift_key(),
                control: mods.control_key(),
                alt: mods.alt_key(),
                super_key: mods.super_key(),
            }
            .classify();

        if let Some(markdown) = self.context_manager.current_mut().active_markdown_mut() {
            let is_z = matches!(
                key.key_without_modifiers().as_ref(),
                Key::Character(ch) if ch.eq_ignore_ascii_case("z")
            );
            if let Some(redo) =
                neoism_ui::editor::markdown::bridge_policy::markdown_super_z_intent(
                    modifier_class,
                    is_z,
                    mods.shift_key(),
                )
            {
                handled = if redo {
                    markdown.redo()
                } else {
                    markdown.undo()
                };
                if handled {
                    self.renderer.trail_cursor.reset();
                    self.sync_active_markdown_modified();
                    self.mark_dirty();
                }
                return;
            }

            if neoism_ui::editor::markdown::bridge_policy::markdown_flushed_leader_scrolls_normal_mode(
                Some(markdown.mode),
                flushed_markdown_leader,
            ) {
                markdown.scroll_by_content_pixels(viewport * 0.86, viewport);
            }

            let ctrl_key_kind = if ctrl_only {
                match key.logical_key.as_ref() {
                    Key::Character("d") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharD)
                    }
                    Key::Character("u") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharU)
                    }
                    Key::Character("e") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharE)
                    }
                    Key::Character("y") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharY)
                    }
                    Key::Character("r") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharR)
                    }
                    Key::Character("v") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharV)
                    }
                    Key::Character("o") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharO)
                    }
                    Key::Character("i") | Key::Named(NamedKey::Tab) => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharI)
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::ArrowUp)
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::ArrowDown)
                    }
                    Key::Named(NamedKey::ArrowLeft) => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::ArrowLeft)
                    }
                    Key::Named(NamedKey::ArrowRight) => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::ArrowRight)
                    }
                    _ => None,
                }
            } else {
                None
            };

            if ctrl_only {
                use neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlAction;
                let action = ctrl_key_kind.and_then(|kind| {
                    neoism_ui::editor::markdown::bridge_policy::markdown_ctrl_action(
                        modifier_class,
                        kind,
                    )
                });
                match action {
                    Some(MarkdownCtrlAction::ScrollCursorDownHalfPage) => {
                        markdown
                            .scroll_cursor_by_content_pixels(viewport * 0.5, viewport);
                    }
                    Some(MarkdownCtrlAction::ScrollCursorUpHalfPage) => {
                        markdown
                            .scroll_cursor_by_content_pixels(-(viewport * 0.5), viewport);
                    }
                    Some(MarkdownCtrlAction::ScrollCursorDownLine) => {
                        markdown.scroll_cursor_by_lines(1, viewport);
                    }
                    Some(MarkdownCtrlAction::ScrollCursorUpLine) => {
                        markdown.scroll_cursor_by_lines(-1, viewport);
                    }
                    Some(MarkdownCtrlAction::MoveTableRowUp) => {
                        handled = markdown.move_table_row_fast(false);
                        snap_cursor = handled;
                    }
                    Some(MarkdownCtrlAction::MoveTableRowDown) => {
                        handled = markdown.move_table_row_fast(true);
                        snap_cursor = handled;
                    }
                    Some(MarkdownCtrlAction::MoveTableCellPrev) => {
                        handled = markdown.move_table_cell(true);
                        snap_cursor = handled;
                    }
                    Some(MarkdownCtrlAction::MoveTableCellNext) => {
                        handled = markdown.move_table_cell(false);
                        snap_cursor = handled;
                    }
                    Some(MarkdownCtrlAction::Redo) => {
                        // Prefer the vim redo path so pending counts work.
                        let visual = matches!(
                            markdown.mode,
                            neoism_ui::editor::markdown::MarkdownMode::Visual
                        );
                        let feed = markdown.vim.feed_ctrl('r', visual);
                        let (h, snap, message) =
                            Self::apply_markdown_vim_feed(markdown, clipboard, feed);
                        handled = h;
                        snap_cursor = snap;
                        if let Some(message) = message {
                            self.renderer.notifications.push(
                                message,
                                neoism_ui::panels::notifications::NotificationLevel::Info,
                            );
                        }
                        if handled {
                            self.renderer.trail_cursor.reset();
                            self.sync_active_markdown_modified();
                        }
                    }
                    Some(MarkdownCtrlAction::VimBlockVisual) => {
                        let visual = matches!(
                            markdown.mode,
                            neoism_ui::editor::markdown::MarkdownMode::Visual
                        );
                        let feed = markdown.vim.feed_ctrl('v', visual);
                        let (h, snap, message) =
                            Self::apply_markdown_vim_feed(markdown, clipboard, feed);
                        handled = h;
                        snap_cursor = snap;
                        if let Some(message) = message {
                            self.renderer.notifications.push(
                                message,
                                neoism_ui::panels::notifications::NotificationLevel::Info,
                            );
                        }
                    }
                    Some(MarkdownCtrlAction::VimJumpBack) => {
                        let feed = markdown.vim.feed_ctrl('o', false);
                        let (h, snap, _) =
                            Self::apply_markdown_vim_feed(markdown, clipboard, feed);
                        handled = h;
                        snap_cursor = snap;
                    }
                    Some(MarkdownCtrlAction::VimJumpForward) => {
                        let feed = markdown.vim.feed_ctrl('i', false);
                        let (h, snap, _) =
                            Self::apply_markdown_vim_feed(markdown, clipboard, feed);
                        handled = h;
                        snap_cursor = snap;
                    }
                    None => handled = false,
                }
                if handled {
                    if snap_cursor {
                        self.renderer.trail_cursor.reset();
                    }
                    self.mark_dirty();
                }
                return;
            }

            match markdown.mode {
                crate::editor::markdown::state::MarkdownMode::Insert => {
                    match key.logical_key.as_ref() {
                        Key::Named(NamedKey::Escape) => {
                            markdown.enter_normal();
                            snap_cursor = true;
                        }
                        Key::Named(NamedKey::Enter) => {
                            if !(mods.shift_key() && markdown.insert_table_row(false)) {
                                markdown.insert_newline();
                            }
                            snap_cursor = true;
                        }
                        Key::Named(NamedKey::Backspace) => {
                            markdown.backspace();
                            snap_cursor = true;
                        }
                        Key::Named(NamedKey::Delete) => {
                            markdown.delete_forward();
                            snap_cursor = true;
                        }
                        Key::Named(NamedKey::Tab)
                            if !mods.control_key()
                                && !mods.alt_key()
                                && !mods.super_key() =>
                        {
                            if markdown.move_table_cell(mods.shift_key()) {
                                snap_cursor = true;
                            } else if markdown.indent_list_item(mods.shift_key()) {
                                snap_cursor = true;
                            } else if !mods.shift_key() {
                                markdown.insert_text("  ");
                                snap_cursor = true;
                            } else {
                                handled = false;
                            }
                        }
                        Key::Named(NamedKey::ArrowLeft) => markdown.move_left(),
                        Key::Named(NamedKey::ArrowRight) => markdown.move_right(),
                        Key::Named(NamedKey::ArrowUp) => markdown.move_up(),
                        Key::Named(NamedKey::ArrowDown) => markdown.move_down(),
                        Key::Named(NamedKey::Home) => markdown.move_line_start(),
                        Key::Named(NamedKey::End) => markdown.move_line_end(),
                        _ if plain && text == "\t" => {
                            if markdown.move_table_cell(mods.shift_key()) {
                                snap_cursor = true;
                            } else if markdown.indent_list_item(mods.shift_key()) {
                                snap_cursor = true;
                            } else if !mods.shift_key() {
                                markdown.insert_text("  ");
                                snap_cursor = true;
                            } else {
                                handled = false;
                            }
                        }
                        Key::Character(ch) if plain && ch == "/" => {
                            // Inside a wiki link (`[[…]]`) a slash is part of
                            // the path being typed — the link-completion menu
                            // owns the popup there, not the `/` block menu.
                            let in_wiki_link =
                                markdown.wiki_link_query_before_cursor().is_some();
                            markdown.insert_text("/");
                            snap_cursor = true;
                            if !in_wiki_link {
                                open_block_menu = true;
                                open_block_menu_at = markdown.cursor_rect;
                            }
                        }
                        _ if plain && !text.is_empty() => {
                            markdown.insert_text(text);
                            snap_cursor = true;
                        }
                        _ => handled = false,
                    }
                }
                crate::editor::markdown::state::MarkdownMode::Normal => {
                    match key.logical_key.as_ref() {
                        Key::Named(NamedKey::Escape) => {
                            handled = markdown.vim.clear_pending();
                        }
                        Key::Named(NamedKey::ArrowLeft) => markdown.move_left(),
                        Key::Named(NamedKey::ArrowRight) => markdown.move_right(),
                        Key::Named(NamedKey::ArrowUp) => markdown.move_up(),
                        Key::Named(NamedKey::ArrowDown) => markdown.move_down(),
                        Key::Named(NamedKey::Home) => markdown.move_line_start(),
                        Key::Named(NamedKey::End) => markdown.move_line_end(),
                        Key::Named(NamedKey::Tab)
                            if !mods.control_key()
                                && !mods.alt_key()
                                && !mods.super_key() =>
                        {
                            if markdown.move_table_cell(mods.shift_key())
                                || markdown.indent_list_item(mods.shift_key())
                            {
                                snap_cursor = true;
                            } else if !mods.shift_key() {
                                markdown.insert_text("  ");
                                snap_cursor = true;
                            } else {
                                handled = false;
                            }
                        }
                        Key::Named(NamedKey::Enter) if mods.shift_key() => {
                            handled = markdown.insert_table_row(false);
                            snap_cursor = handled;
                        }
                        // Links take precedence; task rows retain their existing
                        // keyboard checkbox toggle when no link is under the cursor.
                        Key::Named(NamedKey::Enter) if plain => {
                            open_cursor_link = markdown.link_at_cursor();
                            handled = open_cursor_link.is_some()
                                || markdown.toggle_task_at_cursor();
                        }
                        Key::Named(NamedKey::PageUp) => markdown
                            .scroll_by_content_pixels(-(viewport * 0.86), viewport),
                        Key::Named(NamedKey::PageDown) => {
                            markdown.scroll_by_content_pixels(viewport * 0.86, viewport)
                        }
                        Key::Named(NamedKey::Space) if mods.shift_key() => markdown
                            .scroll_by_content_pixels(-(viewport * 0.86), viewport),
                        Key::Named(NamedKey::Space) => {
                            arm_markdown_leader = true;
                        }
                        // `/` and `?` open the SAME command-palette Search modal
                        // the code editor uses; snapshot the origin here (so Esc
                        // restores it) and open the palette after the borrow ends.
                        Key::Character(ch) if plain && ch == "/" => {
                            markdown.search_begin(false);
                            open_markdown_search = Some(false);
                        }
                        Key::Character(ch) if plain && ch == "?" => {
                            markdown.search_begin(true);
                            open_markdown_search = Some(true);
                        }
                        Key::Character(ch) if plain && ch.chars().count() == 1 => {
                            let ch = ch.chars().next().unwrap_or_default();
                            let feed = markdown.vim.feed(ch, false);
                            let (vim_handled, vim_snap, vim_message) =
                                Self::apply_markdown_vim_feed(markdown, clipboard, feed);
                            handled = vim_handled;
                            snap_cursor |= vim_snap;
                            if vim_message.is_some() {
                                yank_message = vim_message;
                            }
                        }
                        _ => handled = false,
                    }
                }
                crate::editor::markdown::state::MarkdownMode::Visual => {
                    match key.logical_key.as_ref() {
                        Key::Named(NamedKey::Escape) => {
                            markdown.enter_normal();
                            snap_cursor = true;
                        }
                        Key::Named(NamedKey::ArrowLeft) => markdown.move_left(),
                        Key::Named(NamedKey::ArrowRight) => markdown.move_right(),
                        Key::Named(NamedKey::ArrowUp) => markdown.move_up(),
                        Key::Named(NamedKey::ArrowDown) => markdown.move_down(),
                        Key::Named(NamedKey::Home) => markdown.move_line_start(),
                        Key::Named(NamedKey::End) => markdown.move_line_end(),
                        Key::Named(NamedKey::Delete)
                        | Key::Named(NamedKey::Backspace) => {
                            let feed = neoism_ui::editor::markdown::vim::VimKeyFeed::Action(
                            neoism_ui::editor::markdown::vim::VimAction::Operate {
                                op: neoism_ui::editor::markdown::vim::VimOperator::Delete,
                                target:
                                    neoism_ui::editor::markdown::vim::VimTarget::Selection,
                                count: 1,
                            },
                        );
                            let (vim_handled, vim_snap, _) =
                                Self::apply_markdown_vim_feed(markdown, clipboard, feed);
                            handled = vim_handled;
                            snap_cursor |= vim_snap;
                        }
                        Key::Character(ch) if plain && ch.chars().count() == 1 => {
                            let ch = ch.chars().next().unwrap_or_default();
                            let feed = markdown.vim.feed(ch, true);
                            let (vim_handled, vim_snap, vim_message) =
                                Self::apply_markdown_vim_feed(markdown, clipboard, feed);
                            handled = vim_handled;
                            snap_cursor |= vim_snap;
                            if vim_message.is_some() {
                                yank_message = vim_message;
                            }
                        }
                        _ => handled = false,
                    }
                }
            }
        }

        if let Some(link) = open_cursor_link {
            match link {
                neoism_ui::editor::markdown::MarkdownCursorLink::Internal {
                    target,
                    code_ref: _,
                } => {
                    let resolved = self
                        .context_manager
                        .current()
                        .active_markdown()
                        .and_then(|markdown| markdown.resolve_markdown_link(&target));
                    if let Some(target) = resolved {
                        let contact = target.path.to_string_lossy().into_owned();
                        if !self.yank_contact_link(&contact, clipboard) {
                            self.open_markdown_link_target(target);
                        }
                    }
                }
                neoism_ui::editor::markdown::MarkdownCursorLink::External(target) => {
                    if !self.yank_contact_link(&target, clipboard) {
                        self.open_markdown_link_target(
                            neoism_ui::editor::markdown::MarkdownLinkTarget {
                                path: std::path::PathBuf::from(target),
                                line: None,
                                code_ref: false,
                            },
                        );
                    }
                }
            }
            return;
        }

        if let Some(reverse) = open_markdown_search {
            // Open the shared palette in Search mode; from here the flow is
            // identical to the code editor's `/`, except the host sources
            // matches from the markdown buffer (see dispatch_palette_search_query).
            if reverse {
                self.renderer.command_palette.enter_search_mode_backward();
            } else {
                self.renderer.command_palette.enter_search_mode();
            }
            self.mark_dirty();
            return;
        }

        if open_block_menu {
            self.open_markdown_block_menu(open_block_menu_at);
            if snap_cursor {
                self.renderer.trail_cursor.reset();
            }
            self.sync_active_markdown_modified();
            self.mark_dirty();
            return;
        }

        if arm_markdown_leader {
            self.markdown_leader_pending = Some(now);
        }

        if let Some(message) = yank_message {
            self.renderer.notifications.push(
                message,
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
        }

        if let Some(finalize) =
            neoism_ui::editor::markdown::bridge_policy::markdown_dispatch_finalize(
                handled,
                flushed_markdown_leader,
                snap_cursor,
            )
        {
            let (block_menu_changed, link_completion_changed) = if finalize.refresh_menus
            {
                (
                    self.refresh_markdown_block_menu(),
                    self.refresh_markdown_link_completion_menu(),
                )
            } else {
                (false, false)
            };
            if finalize.reset_trail_cursor {
                self.renderer.trail_cursor.reset();
            }
            if finalize.sync_active_modified {
                self.sync_active_markdown_modified();
            }
            if !(block_menu_changed || link_completion_changed) {
                self.mark_dirty();
            }
        }
    }

    pub(crate) fn run_current_notebook_cell(&mut self) -> bool {
        let cell_index = self
            .context_manager
            .current()
            .notebook
            .as_ref()
            .and_then(|notebook| notebook.current_cell_index())
            .unwrap_or(0);
        self.run_notebook_cell(cell_index)
    }

    fn run_current_notebook_cell_and_select_next(&mut self) -> bool {
        let Some((cell_index, cell_type)) = self
            .context_manager
            .current()
            .notebook
            .as_ref()
            .and_then(|notebook| {
                Some((
                    notebook.current_cell_index()?,
                    notebook.current_cell_type()?,
                ))
            })
        else {
            return false;
        };
        if cell_type == neoism_ui::editor::notebook::NotebookCellType::Code {
            self.run_notebook_cell(cell_index);
        }
        if let Some(notebook) = self.context_manager.current_mut().notebook.as_mut() {
            notebook.select_adjacent_cell(1);
            notebook.enter_command_mode();
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    /// Apply a resolved vim key feed to the pane, routing register
    /// traffic through the host clipboard (the unnamed register).
    /// Returns `(handled, snap_cursor, yank_message)`.
    fn apply_markdown_vim_feed(
        markdown: &mut neoism_ui::editor::markdown::MarkdownPane,
        clipboard: &mut Clipboard,
        feed: neoism_ui::editor::markdown::vim::VimKeyFeed,
    ) -> (bool, bool, Option<String>) {
        use neoism_ui::editor::markdown::vim::VimKeyFeed;
        match feed {
            VimKeyFeed::Pending | VimKeyFeed::Cancelled => (true, false, None),
            VimKeyFeed::Unhandled => (false, false, None),
            VimKeyFeed::Action(action) => {
                let paste = action
                    .wants_paste()
                    .then(|| clipboard.get(ClipboardType::Clipboard));
                let applied = markdown.apply_vim_action(&action, paste.as_deref());
                let mut message = None;
                if let Some(register) = applied.register {
                    if applied.yank_notification {
                        message = Some(Self::markdown_yank_message(&register));
                    }
                    if applied.sync_clipboard {
                        let clipboard_text = if applied.yank_notification {
                            neoism_ui::editor::markdown::rendered_inline_text(&register)
                        } else {
                            register
                        };
                        clipboard.set(ClipboardType::Clipboard, clipboard_text);
                    }
                }
                // Macro replay for markdown: feed chars while replaying.
                if let Some(keys) = applied.replay_keys {
                    markdown.vim.replaying_macro = true;
                    for ch in keys.chars() {
                        if !matches!(
                            markdown.mode,
                            neoism_ui::editor::markdown::MarkdownMode::Normal
                                | neoism_ui::editor::markdown::MarkdownMode::Visual
                        ) {
                            break;
                        }
                        let visual = matches!(
                            markdown.mode,
                            neoism_ui::editor::markdown::MarkdownMode::Visual
                        );
                        let feed = markdown.vim.feed(ch, visual);
                        let _ = Self::apply_markdown_vim_feed(markdown, clipboard, feed);
                    }
                    markdown.vim.replaying_macro = false;
                }
                (applied.handled, applied.snap_cursor, message)
            }
        }
    }

    fn dispatch_epub_reader_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
        clipboard: &mut Clipboard,
    ) {
        use neoism_ui::editor::markdown::vim::{VimAction, VimKeyFeed, VimOperator};

        let plain = !mods.control_key() && !mods.alt_key() && !mods.super_key();
        let ctrl_only = mods.control_key() && !mods.alt_key() && !mods.super_key();
        let viewport = self.markdown_viewport_height();

        let now = std::time::Instant::now();
        if let Some(started) = self.markdown_leader_pending {
            if now.duration_since(started).as_millis() > LEADER_TIMEOUT_MS {
                self.markdown_leader_pending = None;
            }
        }
        if self.markdown_leader_pending.is_some() {
            self.markdown_leader_pending = None;
            if plain
                && matches!(
                    key.key_without_modifiers().as_ref(),
                    Key::Character(value) if value.eq_ignore_ascii_case("x")
                )
            {
                if self.close_focused_buffer_tab() {
                    self.mark_dirty();
                }
                return;
            }
            if plain
                && matches!(
                    key.key_without_modifiers().as_ref(),
                    Key::Character(value) if value.eq_ignore_ascii_case("h")
                )
            {
                self.split_down();
                return;
            }
        }
        let reader_leader_key = matches!(
            key.key_without_modifiers().as_ref(),
            Key::Named(NamedKey::Space)
        ) || matches!(
            key.key_without_modifiers().as_ref(),
            Key::Character(value) if value == " "
        );
        if plain && reader_leader_key {
            self.markdown_leader_pending = Some(now);
            return;
        }

        if plain {
            match key.key_without_modifiers().as_ref() {
                Key::Character(value) if value == "/" || value == "?" => {
                    let reverse = value == "?";
                    if let Some(epub) = self.context_manager.current_mut().epub.as_mut() {
                        epub.markdown.search_begin(reverse);
                    }
                    if reverse {
                        self.renderer.command_palette.enter_search_mode_backward();
                    } else {
                        self.renderer.command_palette.enter_search_mode();
                    }
                    self.mark_dirty();
                    return;
                }
                Key::Character(value)
                    if value == "H"
                        && self.context_manager.current().epub.as_ref().is_some_and(
                            |epub| {
                                matches!(
                                    epub.markdown.mode,
                                    neoism_ui::editor::markdown::MarkdownMode::Visual
                                )
                            },
                        ) =>
                {
                    let result = self
                        .context_manager
                        .current_mut()
                        .epub
                        .as_mut()
                        .map(|epub| epub.add_highlight_from_selection(String::new()));
                    match result {
                        Some(Ok(Some(_))) => self.renderer.notifications.push(
                            "Highlight saved".to_string(),
                            neoism_ui::panels::notifications::NotificationLevel::Info,
                        ),
                        Some(Ok(None)) => self.renderer.notifications.push(
                            "Select text in Visual mode before highlighting".to_string(),
                            neoism_ui::panels::notifications::NotificationLevel::Warn,
                        ),
                        Some(Err(error)) => self.renderer.notifications.push(
                            format!("Could not save highlight: {error}"),
                            neoism_ui::panels::notifications::NotificationLevel::Error,
                        ),
                        None => {}
                    }
                    self.mark_dirty();
                    return;
                }
                Key::Character(value)
                    if value == "N"
                        && self.context_manager.current().epub.as_ref().is_some_and(
                            |epub| {
                                matches!(
                                    epub.markdown.mode,
                                    neoism_ui::editor::markdown::MarkdownMode::Visual
                                )
                            },
                        ) =>
                {
                    self.open_epub_note_prompt();
                    return;
                }
                Key::Character(value) if value == "]" => {
                    self.epub_next_chapter();
                    return;
                }
                Key::Character(value) if value == "[" => {
                    self.epub_previous_chapter();
                    return;
                }
                Key::Character(value) if value == "t" => {
                    self.open_epub_table_of_contents();
                    return;
                }
                Key::Character(value) if value == "a" => {
                    self.open_epub_annotations();
                    return;
                }
                Key::Character(value) if value == " " => {
                    self.turn_epub_page(1);
                    return;
                }
                Key::Named(NamedKey::PageDown) => {
                    self.turn_epub_page(1);
                    return;
                }
                Key::Named(NamedKey::PageUp) => {
                    self.turn_epub_page(-1);
                    return;
                }
                Key::Named(NamedKey::Escape) => {
                    if let Some(epub) = self.context_manager.current_mut().epub.as_mut() {
                        epub.markdown.enter_normal();
                        epub.capture_location();
                    }
                    self.mark_dirty();
                    return;
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    self.turn_epub_page(-1);
                    return;
                }
                Key::Named(NamedKey::ArrowRight) => {
                    self.turn_epub_page(1);
                    return;
                }
                Key::Named(NamedKey::ArrowUp) => {
                    let viewport = self.markdown_viewport_height();
                    if let Some(epub) = self.context_manager.current_mut().epub.as_mut() {
                        epub.markdown.scroll_pixels(58.0, viewport);
                        epub.capture_location();
                    }
                    self.mark_dirty();
                    return;
                }
                Key::Named(NamedKey::ArrowDown) => {
                    let viewport = self.markdown_viewport_height();
                    if let Some(epub) = self.context_manager.current_mut().epub.as_mut() {
                        epub.markdown.scroll_pixels(-58.0, viewport);
                        epub.capture_location();
                    }
                    self.mark_dirty();
                    return;
                }
                _ => {}
            }
        }

        if ctrl_only {
            match key.logical_key.as_ref() {
                Key::Character("d") => {
                    if let Some(epub) = self.context_manager.current_mut().epub.as_mut() {
                        epub.markdown
                            .scroll_cursor_by_content_pixels(viewport * 0.5, viewport);
                        epub.capture_location();
                    }
                    self.mark_dirty();
                    return;
                }
                Key::Character("u") => {
                    if let Some(epub) = self.context_manager.current_mut().epub.as_mut() {
                        epub.markdown
                            .scroll_cursor_by_content_pixels(-(viewport * 0.5), viewport);
                        epub.capture_location();
                    }
                    self.mark_dirty();
                    return;
                }
                _ => return,
            }
        }

        let Some(ch) = (plain && matches!(key.logical_key.as_ref(), Key::Character(_)))
            .then(|| match key.logical_key.as_ref() {
                Key::Character(value) => value.chars().next(),
                _ => None,
            })
            .flatten()
        else {
            return;
        };
        let mut yank_message = None;
        let mut handled = false;
        if let Some(epub) = self.context_manager.current_mut().epub.as_mut() {
            let visual = matches!(
                epub.markdown.mode,
                neoism_ui::editor::markdown::MarkdownMode::Visual
            );
            match epub.markdown.vim.feed(ch, visual) {
                VimKeyFeed::Pending | VimKeyFeed::Cancelled => handled = true,
                VimKeyFeed::Unhandled => {}
                VimKeyFeed::Action(action) => {
                    // Reader whitelist: motions, visual selection, yanks,
                    // search navigation, marks and jumplist are safe. Every
                    // editing action is consumed without touching book text.
                    let reader_safe = matches!(
                        action,
                        VimAction::Move { .. }
                            | VimAction::EnterVisual { .. }
                            | VimAction::VisualSwapEnds
                            | VimAction::VisualTextObject { .. }
                            | VimAction::Search { .. }
                            | VimAction::SearchWord { .. }
                            | VimAction::SetMark { .. }
                            | VimAction::GotoMark { .. }
                            | VimAction::JumpBack { .. }
                            | VimAction::JumpForward { .. }
                            | VimAction::Operate {
                                op: VimOperator::Yank,
                                ..
                            }
                    );
                    if reader_safe {
                        let applied = epub.markdown.apply_vim_action(&action, None);
                        handled = applied.handled;
                        if let Some(register) = applied.register {
                            if applied.sync_clipboard {
                                clipboard.set(
                                    ClipboardType::Clipboard,
                                    neoism_ui::editor::markdown::rendered_inline_text(
                                        &register,
                                    ),
                                );
                            }
                            if applied.yank_notification {
                                yank_message =
                                    Some(Self::markdown_yank_message(&register));
                            }
                        }
                    } else {
                        handled = true;
                        epub.markdown.vim.clear_pending();
                    }
                }
            }
            epub.capture_location();
        }
        if let Some(message) = yank_message {
            self.renderer.notifications.push(
                message,
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
        }
        if handled {
            self.renderer.trail_cursor.reset();
            self.mark_dirty();
        }
    }

    pub(crate) fn markdown_yank_message(text: &str) -> String {
        let count = if text.is_empty() {
            0
        } else {
            text.split('\n').count() - usize::from(text.ends_with('\n'))
        }
        .max(1);
        let unit = if count == 1 { "line" } else { "lines" };
        format!("Yanked {count} {unit}")
    }

    pub(crate) fn move_markdown_tab_between_strips(
        &mut self,
        source: crate::host::StripRef,
        dest: crate::host::StripRef,
        tab: neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        path: PathBuf,
    ) {
        let markdown_route = self.markdown_route_for_strip(source, &path);
        let mut moved_focus = None;
        self.activate_remaining_tab_in_strip(source);

        match dest {
            crate::host::StripRef::Workspace => {
                if let Some(route) = markdown_route {
                    let _ = self
                        .context_manager
                        .stack_existing_route_on_workspace(route, &mut self.sugarloaf);
                }
                let ix = self.renderer.buffer_tabs.open_markdown(path.clone());
                self.renderer
                    .buffer_tabs
                    .restore_presentation_from(ix, &tab);
                self.renderer.file_tree.set_active_path(Some(path.clone()));
                self.activate_rich_document_path(path.clone());
            }
            crate::host::StripRef::Pane(dest_route) => {
                let target_route = if let Some(route) = markdown_route {
                    if !self.context_manager.stack_existing_route_on_route(
                        route,
                        dest_route,
                        &mut self.sugarloaf,
                    ) {
                        self.reinsert_tab_into_strip(source, &tab, path);
                        self.renderer.notifications.push(
                            format!("Could not move `{}` into that split.", tab.title),
                            neoism_ui::panels::notifications::NotificationLevel::Warn,
                        );
                        return;
                    }
                    route
                } else {
                    let Some(route) =
                        self.ensure_pane_markdown_route_for_file(dest_route, &path)
                    else {
                        self.reinsert_tab_into_strip(source, &tab, path);
                        self.renderer.notifications.push(
                            format!("Could not move `{}` into that split.", tab.title),
                            neoism_ui::panels::notifications::NotificationLevel::Warn,
                        );
                        return;
                    };
                    route
                };
                let scale = self.renderer.chrome_scale();
                let tabs =
                    self.renderer
                        .pane_tabs
                        .entry(dest_route)
                        .or_insert_with(|| {
                            let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
                                crate::neoism::icon::AgentKind,
                            >::new();
                            tabs.set_scale(scale);
                            tabs
                        });
                let ix = tabs.open_markdown(path.clone());
                tabs.restore_presentation_from(ix, &tab);
                let cwd = self.active_pane_workspace_root();
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&dest_route)
                {
                    crumbs.set_from_path(&path, cwd.as_deref());
                }
                if let Some(node) = self
                    .context_manager
                    .current_grid()
                    .node_by_route_id(target_route)
                {
                    let _ = self
                        .context_manager
                        .current_grid_mut()
                        .set_current_node(node, &mut self.sugarloaf);
                    self.context_manager.select_route_from_current_grid();
                    moved_focus = Some((dest_route, target_route));
                }
            }
        }

        if let crate::host::StripRef::Pane(src_route) = source {
            let empty = self
                .renderer
                .pane_tabs
                .get(&src_route)
                .map(|t| t.tabs().is_empty())
                .unwrap_or(true);
            if empty {
                self.renderer.pane_tabs.remove(&src_route);
                self.renderer.pane_breadcrumbs.remove(&src_route);
                if self.context_manager.current_grid_len() > 1 {
                    if let Some(node) = self
                        .context_manager
                        .current_grid()
                        .node_by_route_id(src_route)
                    {
                        let _ = self
                            .context_manager
                            .current_grid_mut()
                            .set_current_node(node, &mut self.sugarloaf);
                        self.context_manager.select_route_from_current_grid();
                        self.context_manager
                            .remove_current_grid(&mut self.sugarloaf);
                        self.reapply_chrome_layout();
                    }
                }
            }
        }
        // Empty-source cleanup temporarily focuses the source node before
        // removing it. Restore the moved Markdown route so the destination
        // split's breadcrumbs are active on the first frame, not only after a
        // later focus round-trip.
        if let Some((dest_route, target_route)) = moved_focus {
            if let Some(node) = self
                .context_manager
                .current_grid()
                .node_by_route_id(target_route)
            {
                let _ = self
                    .context_manager
                    .current_grid_mut()
                    .set_current_node(node, &mut self.sugarloaf);
                self.context_manager.select_route_from_current_grid();
            }
            if let Some(active) = self
                .renderer
                .pane_tabs
                .get(&dest_route)
                .map(|tabs| tabs.active())
            {
                self.pane_tab_activate(dest_route, active);
            }
            self.reapply_chrome_layout();
        }
    }

    pub(crate) fn pane_markdown_route_for_strip(
        &self,
        strip_route: usize,
        path: &Path,
    ) -> Option<usize> {
        let grid = self.context_manager.current_grid();
        let node = grid.node_by_route_id(strip_route)?;
        if grid
            .contexts()
            .get(&node)
            .is_some_and(|item| context_has_rich_document_path(item.context(), path))
        {
            return Some(strip_route);
        }
        grid.stacked_children_of(node)
            .into_iter()
            .find_map(|child| {
                grid.contexts().get(&child).and_then(|item| {
                    context_has_rich_document_path(item.context(), path)
                        .then_some(item.context().route_id)
                })
            })
    }

    pub(crate) fn markdown_route_for_strip(
        &self,
        strip: crate::host::StripRef,
        path: &Path,
    ) -> Option<usize> {
        match strip {
            crate::host::StripRef::Workspace => self
                .context_manager
                .current_grid()
                .workspace_route_id()
                .and_then(|route| self.pane_markdown_route_for_strip(route, path))
                .or_else(|| {
                    if crate::editor::notebook::is_notebook_path(path) {
                        self.context_manager
                            .notebook_node_by_path(path)
                            .map(|(route, _)| route)
                    } else if crate::screen::bridges::epub::is_epub_path(path) {
                        self.context_manager
                            .epub_node_by_path(path)
                            .map(|(route, _)| route)
                    } else {
                        self.context_manager
                            .markdown_node_by_path(path)
                            .map(|(route, _)| route)
                    }
                }),
            crate::host::StripRef::Pane(route) => {
                self.pane_markdown_route_for_strip(route, path)
            }
        }
    }

    pub(crate) fn ensure_pane_markdown_route_for_file(
        &mut self,
        strip_route: usize,
        path: &std::path::Path,
    ) -> Option<usize> {
        if let Some(route) = self.pane_markdown_route_for_strip(strip_route, path) {
            return Some(route);
        }

        let current_grid = self.context_manager.current_grid();
        let (_context, margin) = current_grid.current_context_with_computed_dimension();
        let padding_x = margin.left;
        let padding_y_top = self.renderer.margin.top
            + self
                .renderer
                .island
                .as_ref()
                .map_or(0.0, |i| i.effective_height(self.context_manager.len()));
        let rich_text_id = next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        self.sugarloaf
            .set_position(rich_text_id, padding_x, padding_y_top);
        if crate::editor::notebook::is_notebook_path(path) {
            self.context_manager.add_stacked_notebook_on_route(
                path.to_path_buf(),
                strip_route,
                rich_text_id,
                &mut self.sugarloaf,
            )
        } else if crate::screen::bridges::epub::is_epub_path(path) {
            self.context_manager.add_stacked_epub_on_route(
                path.to_path_buf(),
                strip_route,
                rich_text_id,
                &mut self.sugarloaf,
            )
        } else {
            self.context_manager.add_stacked_markdown_on_route(
                path.to_path_buf(),
                strip_route,
                rich_text_id,
                &mut self.sugarloaf,
            )
        }
    }

    pub(crate) fn tear_out_markdown_tab_to_pane(
        &mut self,
        path: std::path::PathBuf,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        source: crate::host::StripRef,
        split_down: bool,
    ) {
        self.tear_out_markdown_tab_to_pane_at(path, tab, source, split_down, None);
    }

    pub(crate) fn tear_out_markdown_tab_to_pane_at(
        &mut self,
        path: std::path::PathBuf,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        source: crate::host::StripRef,
        split_down: bool,
        destination: Option<(usize, neoism_ui::session_layout::geometry::DropPlacement)>,
    ) {
        let mut markdown_route = self.markdown_route_for_strip(source, &path);
        if markdown_route.is_none() {
            markdown_route = match source {
                crate::host::StripRef::Workspace => {
                    self.activate_rich_document_path(path.clone());
                    self.markdown_route_for_strip(source, &path)
                }
                crate::host::StripRef::Pane(route) => {
                    self.ensure_pane_markdown_route_for_file(route, &path)
                }
            };
        }
        let Some(markdown_route) = markdown_route else {
            self.reinsert_tab_into_strip(source, tab, path);
            self.renderer.notifications.push(
                format!("Could not tear out `{}` to a split.", tab.title),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        };
        self.activate_remaining_tab_in_strip(source);
        let split_ok = if let Some((target_route, placement)) = destination {
            self.context_manager.split_existing_route_at(
                markdown_route,
                target_route,
                placement,
                &mut self.sugarloaf,
            )
        } else {
            self.context_manager.split_existing_route(
                markdown_route,
                split_down,
                &mut self.sugarloaf,
            )
        };
        if !split_ok {
            self.reinsert_tab_into_strip(source, tab, path);
            self.renderer.notifications.push(
                format!("Could not tear out `{}` to a split.", tab.title),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return;
        }

        let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<
            crate::neoism::icon::AgentKind,
        >::new();
        tabs.set_scale(self.renderer.chrome_scale());
        let ix = tabs.open_markdown(path.clone());
        tabs.restore_presentation_from(ix, tab);
        self.renderer.pane_tabs.insert(markdown_route, tabs);
        let mut crumbs = neoism_ui::panels::breadcrumbs::Breadcrumbs::new();
        crumbs.set_scale(self.renderer.chrome_scale());
        let cwd_for_crumbs = self.active_pane_workspace_root();
        crumbs.set_from_path(&path, cwd_for_crumbs.as_deref());
        self.renderer
            .pane_breadcrumbs
            .insert(markdown_route, crumbs);
        self.renderer.file_tree.set_focused(false);
        if let crate::host::StripRef::Pane(src_route) = source {
            let empty = self
                .renderer
                .pane_tabs
                .get(&src_route)
                .map(|t| t.tabs().is_empty())
                .unwrap_or(true);
            if empty {
                self.renderer.pane_tabs.remove(&src_route);
                self.renderer.pane_breadcrumbs.remove(&src_route);
            }
        }
        self.reapply_chrome_layout();
    }
}

fn context_has_rich_document_path<T: neoism_backend::event::EventListener>(
    context: &crate::context::Context<T>,
    path: &Path,
) -> bool {
    context
        .markdown
        .as_ref()
        .is_some_and(|pane| pane.path.as_path() == path)
        || context
            .notebook
            .as_ref()
            .is_some_and(|pane| pane.path.as_path() == path)
        || context
            .epub
            .as_ref()
            .is_some_and(|pane| pane.book.path.as_path() == path)
}
