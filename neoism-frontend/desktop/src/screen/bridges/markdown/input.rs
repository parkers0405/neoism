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
                self.invalidate_note_index_for_path(path);
                self.rebuild_note_graph_for_path(path);
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
                .markdown
                .as_ref()
                .is_some_and(|markdown| markdown.is_dragging())
    }

    pub fn markdown_grab_drag_active(&self) -> bool {
        self.context_manager
            .current()
            .markdown
            .as_ref()
            .is_some_and(|markdown| markdown.is_grab_dragging())
    }

    pub fn handle_markdown_mouse_press(
        &mut self,
        button: MouseButton,
        clipboard: &mut Clipboard,
    ) -> bool {
        if self.context_manager.current().markdown.is_none()
            && self.context_manager.current().notebook.is_none()
        {
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
            self.open_markdown_link_target(target);
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
        let (handled, menu_rect) = if let Some(markdown) =
            self.context_manager.current_mut().active_markdown_mut()
        {
            let handled = markdown.end_drag();
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
        if handled {
            self.sync_active_markdown_modified();
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

        let plain = !mods.control_key() && !mods.alt_key() && !mods.super_key();
        let ctrl_only = mods.control_key() && !mods.alt_key() && !mods.super_key();
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
                match key.key_without_modifiers().as_ref() {
                    Key::Character(ch) if ch.eq_ignore_ascii_case("d") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharD)
                    }
                    Key::Character(ch) if ch.eq_ignore_ascii_case("u") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharU)
                    }
                    Key::Character(ch) if ch.eq_ignore_ascii_case("r") => {
                        Some(neoism_ui::editor::markdown::bridge_policy::MarkdownCtrlKeyKind::CharR)
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
                        handled = markdown.redo();
                        if handled {
                            self.renderer.trail_cursor.reset();
                            self.sync_active_markdown_modified();
                        }
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
                        // Plain Enter in Normal mode toggles the `- [ ]` / `- [x]`
                        // checkbox on the cursor line (keyboard equivalent of
                        // clicking the box). Falls through when the line isn't a
                        // task so Enter stays a no-op elsewhere in Normal mode.
                        Key::Named(NamedKey::Enter) if plain => {
                            handled = markdown.toggle_task_at_cursor();
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
                    clipboard.set(ClipboardType::Clipboard, register);
                }
                (applied.handled, applied.snap_cursor, message)
            }
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
        self.activate_remaining_tab_in_strip(source);

        match dest {
            crate::host::StripRef::Workspace => {
                if let Some(route) = markdown_route {
                    let _ = self
                        .context_manager
                        .stack_existing_route_on_workspace(route, &mut self.sugarloaf);
                }
                self.renderer.buffer_tabs.open_markdown(path.clone());
                self.renderer.file_tree.set_active_path(Some(path.clone()));
                self.activate_markdown_path(path.clone());
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
                tabs.open_markdown(path.clone());
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
    }

    pub(crate) fn pane_markdown_route_for_strip(
        &self,
        strip_route: usize,
        path: &Path,
    ) -> Option<usize> {
        let grid = self.context_manager.current_grid();
        let node = grid.node_by_route_id(strip_route)?;
        if grid.contexts().get(&node).is_some_and(|item| {
            item.context()
                .markdown
                .as_ref()
                .is_some_and(|pane| pane.path.as_path() == path)
        }) {
            return Some(strip_route);
        }
        grid.stacked_children_of(node)
            .into_iter()
            .find_map(|child| {
                grid.contexts().get(&child).and_then(|item| {
                    item.context()
                        .markdown
                        .as_ref()
                        .filter(|pane| pane.path.as_path() == path)
                        .map(|_| item.context().route_id)
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
                    self.context_manager
                        .markdown_node_by_path(path)
                        .map(|(route, _)| route)
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
        self.context_manager.add_stacked_markdown_on_route(
            path.to_path_buf(),
            strip_route,
            rich_text_id,
            &mut self.sugarloaf,
        )
    }

    pub(crate) fn tear_out_markdown_tab_to_pane(
        &mut self,
        path: std::path::PathBuf,
        tab: &neoism_ui::panels::buffer_tabs::BufferTab<crate::neoism::icon::AgentKind>,
        source: crate::host::StripRef,
        split_down: bool,
    ) {
        let mut markdown_route = self.markdown_route_for_strip(source, &path);
        if markdown_route.is_none() {
            markdown_route = match source {
                crate::host::StripRef::Workspace => {
                    self.activate_markdown_path(path.clone());
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
        if !self.context_manager.split_existing_route(
            markdown_route,
            split_down,
            &mut self.sugarloaf,
        ) {
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
        tabs.open_markdown(path.clone());
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
