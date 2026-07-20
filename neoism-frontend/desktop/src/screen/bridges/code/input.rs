use super::*;
use super::lsp::CodeKeyEdit;
use neoism_backend::clipboard::{Clipboard, ClipboardType};
use neoism_ui::editor::code::{CodeInputMode, CodeMode, CodeMotion};
use neoism_ui::editor::markdown::vim::{VimAction, VimKeyFeed, VimStage};
use neoism_window::event::{ElementState, MouseButton};
use neoism_window::keyboard::{Key, ModifiersState, NamedKey};

impl Screen<'_> {
    pub(crate) fn dispatch_code_key(
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
        let primary = (mods.control_key() || mods.super_key()) && !mods.alt_key();
        let shift = mods.shift_key();

        // Code-action menu first refusal (any input mode): Up/Down
        // navigate, Enter applies, Esc dismisses; other keys dismiss
        // and fall through.
        if self.code_action_menu_key(key, mods) {
            return;
        }

        // Completion menu first refusal: while it is open (Insert-mode
        // only — mode changes dismiss it) Up/Down navigate, Tab/Enter
        // accept, Esc closes. All other keys fall through and settle
        // the menu via `code_lsp_after_key`.
        if self.code_completion_menu_key(key, mods) {
            return;
        }

        // Vim layer: modal interception when the pane's input mode is
        // Vim. Insert mode falls through to the standard path (typing
        // is typing) except Esc, which returns to Normal.
        let vim_state = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| (code.input_mode, code.buffer.mode));
        match vim_state {
            Some((CodeInputMode::Vim, CodeMode::Normal | CodeMode::Visual)) => {
                self.dispatch_code_vim_key(key, mods, text, clipboard);
                return;
            }
            Some((CodeInputMode::Vim, CodeMode::Insert))
                if matches!(key.logical_key.as_ref(), Key::Named(NamedKey::Escape)) =>
            {
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    code.buffer.mode = CodeMode::Normal;
                    code.buffer.clear_selection();
                    code.buffer.break_undo_group();
                    code.buffer.snap_normal_cursor();
                }
                self.dismiss_code_lsp_popups();
                self.mark_dirty();
                return;
            }
            _ => {}
        }

        if primary {
            match key.logical_key.as_ref() {
                Key::Character("s") => {
                    self.save_current_code();
                    return;
                }
                Key::Character("a") => {
                    if let Some(code) = self.context_manager.current_mut().code.as_mut()
                    {
                        code.buffer.select_all();
                    }
                    self.mark_dirty();
                    return;
                }
                Key::Character("c") => {
                    if let Some(code) = self.context_manager.current_mut().code.as_mut()
                    {
                        let (payload, _linewise) = code.buffer.copy_payload();
                        clipboard.set(ClipboardType::Clipboard, payload);
                    }
                    return;
                }
                Key::Character("x") => {
                    if let Some(code) = self.context_manager.current_mut().code.as_mut()
                    {
                        let (payload, _linewise) = code.buffer.cut_payload();
                        clipboard.set(ClipboardType::Clipboard, payload);
                    }
                    self.dismiss_code_lsp_popups();
                    self.sync_active_code_modified();
                    self.mark_dirty();
                    return;
                }
                Key::Character("v") => {
                    let content = clipboard.get(ClipboardType::Clipboard);
                    if let Some(code) = self.context_manager.current_mut().code.as_mut()
                    {
                        code.buffer.insert_text(&content);
                    }
                    self.dismiss_code_lsp_popups();
                    self.sync_active_code_modified();
                    self.mark_dirty();
                    return;
                }
                // Shift yields an uppercase logical key (Ctrl+Shift+Z).
                Key::Character("z") | Key::Character("Z") => {
                    if let Some(code) = self.context_manager.current_mut().code.as_mut()
                    {
                        if shift {
                            code.buffer.redo();
                        } else {
                            code.buffer.undo();
                        }
                    }
                    self.dismiss_code_lsp_popups();
                    self.sync_active_code_modified();
                    self.mark_dirty();
                    return;
                }
                Key::Character("y") => {
                    if let Some(code) = self.context_manager.current_mut().code.as_mut()
                    {
                        code.buffer.redo();
                    }
                    self.dismiss_code_lsp_popups();
                    self.sync_active_code_modified();
                    self.mark_dirty();
                    return;
                }
                // Ctrl+K: LSP hover docs at the cursor (standard mode).
                Key::Character("k") => {
                    self.request_code_hover();
                    self.mark_dirty();
                    return;
                }
                // Ctrl+.: LSP code actions / quick-fix at the cursor
                // (standard mode + vim Insert, which falls through
                // here; vim Normal uses `<Space>a`).
                Key::Character(".") => {
                    self.request_code_actions();
                    return;
                }
                // nvim i_CTRL-N / i_CTRL-P: open the completion menu
                // when it's closed (open-menu cycling is handled by the
                // menu intercept above). Control only — Super+P is the
                // palette.
                Key::Character("n") | Key::Character("p")
                    if mods.control_key() && !mods.super_key() =>
                {
                    self.request_code_completion(None);
                    self.mark_dirty();
                    return;
                }
                Key::Named(NamedKey::ArrowLeft) => {
                    self.code_motion(CodeMotion::WordLeft, shift);
                    return;
                }
                Key::Named(NamedKey::ArrowRight) => {
                    self.code_motion(CodeMotion::WordRight, shift);
                    return;
                }
                Key::Named(NamedKey::Home) => {
                    self.code_motion(CodeMotion::DocStart, shift);
                    return;
                }
                Key::Named(NamedKey::End) => {
                    self.code_motion(CodeMotion::DocEnd, shift);
                    return;
                }
                _ => {}
            }
        }

        let viewport_rows = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| code.geometry.viewport_rows())
            .unwrap_or(1);
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return;
        };
        let lsp_edit = match key.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => {
                code.buffer.clear_selection();
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::Enter) => {
                code.buffer.insert_newline();
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::Backspace) => {
                code.buffer.backspace();
                CodeKeyEdit::Backspace
            }
            Key::Named(NamedKey::Delete) => {
                code.buffer.delete_forward();
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::Tab) => {
                if shift {
                    code.buffer.outdent();
                } else {
                    code.buffer.insert_tab();
                }
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::ArrowLeft) => {
                code.buffer.apply_motion(CodeMotion::Left, shift);
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::ArrowRight) => {
                code.buffer.apply_motion(CodeMotion::Right, shift);
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::ArrowUp) => {
                code.buffer.apply_motion(CodeMotion::Up, shift);
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::ArrowDown) => {
                code.buffer.apply_motion(CodeMotion::Down, shift);
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::Home) => {
                code.buffer.apply_motion(CodeMotion::LineStartSmart, shift);
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::End) => {
                code.buffer.apply_motion(CodeMotion::LineEnd, shift);
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::PageUp) => {
                code.buffer
                    .apply_motion(CodeMotion::PageUp { rows: viewport_rows }, shift);
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::PageDown) => {
                code.buffer
                    .apply_motion(CodeMotion::PageDown { rows: viewport_rows }, shift);
                CodeKeyEdit::Other
            }
            Key::Named(NamedKey::Space) if plain => {
                code.buffer.insert_char(' ');
                CodeKeyEdit::Char(' ')
            }
            _ if plain && !text.is_empty() => {
                let mut chars = text.chars();
                match (chars.next(), chars.next()) {
                    // Single typed char goes through `insert_char` so
                    // consecutive keystrokes coalesce into one undo.
                    (Some(c), None) => {
                        code.buffer.insert_char(c);
                        CodeKeyEdit::Char(c)
                    }
                    _ => {
                        code.buffer.insert_text(text);
                        CodeKeyEdit::Other
                    }
                }
            }
            _ => return,
        };
        self.sync_active_code_modified();
        self.mark_dirty();
        self.code_lsp_after_key(lsp_edit);
    }

    /// Key interception while the code-action menu is open (any input
    /// mode — it opens from Normal `<Space>a` and Insert Ctrl+.).
    /// Arrows/Enter/Esc are consumed; any other key dismisses the menu
    /// and falls through so typing never gets eaten.
    fn code_action_menu_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        if !self.code_action_menu_open() {
            return false;
        }
        if mods.control_key() || mods.alt_key() || mods.super_key() {
            self.renderer.code_lsp.actions = None;
            self.mark_dirty();
            return false;
        }
        match key.logical_key.as_ref() {
            Key::Named(NamedKey::ArrowUp) => {
                self.move_code_action_selection(-1);
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.move_code_action_selection(1);
                true
            }
            Key::Named(NamedKey::Enter) => self.apply_selected_code_action(),
            Key::Named(NamedKey::Escape) => {
                self.renderer.code_lsp.actions = None;
                self.mark_dirty();
                true
            }
            _ => {
                self.renderer.code_lsp.actions = None;
                self.mark_dirty();
                false
            }
        }
    }

    /// Key interception while the completion menu is open. Returns true
    /// when the key was consumed by the menu.
    fn code_completion_menu_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        if !self.code_completion_menu_open() {
            return false;
        }
        // The menu only exists during Insert-mode typing; anything else
        // is stale state — let the key flow and the pump clean up.
        let insert_mode = self
            .context_manager
            .current()
            .code
            .as_ref()
            .is_some_and(|code| code.buffer.mode == CodeMode::Insert);
        if !insert_mode {
            return false;
        }
        // nvim i_CTRL-N / i_CTRL-P cycle the menu.
        if mods.control_key() && !mods.alt_key() && !mods.super_key() {
            match key.logical_key.as_ref() {
                Key::Character("n") => {
                    self.move_code_completion_selection(1);
                    return true;
                }
                Key::Character("p") => {
                    self.move_code_completion_selection(-1);
                    return true;
                }
                _ => {}
            }
        }
        if mods.control_key() || mods.alt_key() || mods.super_key() {
            return false;
        }
        match key.logical_key.as_ref() {
            Key::Named(NamedKey::ArrowUp) => {
                self.move_code_completion_selection(-1);
                true
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.move_code_completion_selection(1);
                true
            }
            Key::Named(NamedKey::Tab) if !mods.shift_key() => {
                self.accept_code_completion()
            }
            Key::Named(NamedKey::Enter) => self.accept_code_completion(),
            Key::Named(NamedKey::Escape) => {
                self.renderer.code_lsp.completion = None;
                self.mark_dirty();
                true
            }
            _ => false,
        }
    }

    fn code_motion(&mut self, motion: CodeMotion, extend: bool) {
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            // Wrapped continuations are lines: Up/Down walk VISUAL
            // rows when wrap is on (falls back to buffer lines).
            let handled = match motion {
                CodeMotion::Up => code.move_cursor_vertical_visual(false, extend),
                CodeMotion::Down => code.move_cursor_vertical_visual(true, extend),
                _ => false,
            };
            if !handled {
                code.buffer.apply_motion(motion, extend);
            }
        }
        self.mark_dirty();
    }

    /// Bare j/k (or arrows) in vim Normal/Visual walk VISUAL rows when
    /// wrap is on — the nvim `v:count == 0 ? 'gj' : 'j'` mapping.
    /// Counted or operator-pending motions keep buffer-line semantics
    /// (`3j`, `dj`). Returns false when the resolver should handle it.
    fn code_vim_vertical(&mut self, down: bool) -> bool {
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return false;
        };
        if !code.buffer.vim.pending.is_empty() {
            return false;
        }
        let extend = code.buffer.mode == CodeMode::Visual;
        if !code.move_cursor_vertical_visual(down, extend) {
            return false;
        }
        if code.buffer.mode == CodeMode::Normal {
            code.buffer.snap_normal_cursor();
        }
        self.mark_dirty();
        true
    }

    /// Vim Normal/Visual key handling: feed plain chars into the shared
    /// resolver, apply resolved actions with the clipboard as the
    /// unnamed register, and map the few host-level keys (Esc, arrows,
    /// Ctrl-R, `:`) the resolver doesn't own.
    fn dispatch_code_vim_key(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
        text: &str,
        clipboard: &mut Clipboard,
    ) {
        let plain = !mods.control_key() && !mods.alt_key() && !mods.super_key();
        let ctrl_only = mods.control_key() && !mods.alt_key() && !mods.super_key();

        if ctrl_only {
            match key.logical_key.as_ref() {
                // Ctrl+C: copy the visual selection (or current line in
                // Normal) to the clipboard, then drop back to Normal.
                Key::Character("c") => {
                    if let Some(code) = self.context_manager.current_mut().code.as_mut()
                    {
                        let (payload, _linewise) = code.buffer.copy_payload();
                        clipboard.set(ClipboardType::Clipboard, payload);
                        code.buffer.mode = CodeMode::Normal;
                        code.buffer.clear_selection();
                        code.buffer.snap_normal_cursor();
                    }
                    self.mark_dirty();
                }
                Key::Character("r") => {
                    if let Some(code) = self.context_manager.current_mut().code.as_mut()
                    {
                        code.buffer.redo();
                    }
                    self.sync_active_code_modified();
                    self.mark_dirty();
                }
                // Ctrl-P in Normal mode: the fuzzy file finder (the
                // ctrlp/telescope reflex; insert-mode Ctrl-P is
                // completion, matching nvim).
                Key::Character("p") => {
                    self.open_finder_files();
                }
                // Ctrl-D / Ctrl-U: half-page cursor sweep (vim).
                Key::Character("d") | Key::Character("u") => {
                    let down = matches!(key.logical_key.as_ref(), Key::Character("d"));
                    let rows = self
                        .context_manager
                        .current()
                        .code
                        .as_ref()
                        .map(|code| (code.geometry.viewport_rows() / 2).max(1))
                        .unwrap_or(1);
                    if let Some(code) = self.context_manager.current_mut().code.as_mut()
                    {
                        let extend = code.buffer.mode == CodeMode::Visual;
                        code.half_page_scroll(down, extend);
                        if code.buffer.mode == CodeMode::Normal {
                            code.buffer.snap_normal_cursor();
                        }
                    }
                    self.mark_dirty();
                }
                _ => {}
            }
            return;
        }

        match key.logical_key.as_ref() {
            Key::Named(NamedKey::Escape) => {
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    code.buffer.vim.clear_pending();
                    code.leader_pending = false;
                    code.buffer.mode = CodeMode::Normal;
                    code.buffer.clear_selection();
                    code.buffer.snap_normal_cursor();
                    // `:noh` convention — Esc drops the hlsearch bands.
                    code.search_highlight = None;
                }
                self.dismiss_code_lsp_popups();
                self.mark_dirty();
                return;
            }
            Key::Named(NamedKey::ArrowLeft) => return self.code_vim_char('h', clipboard),
            Key::Named(NamedKey::ArrowRight) => {
                return self.code_vim_char('l', clipboard)
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.code_vim_vertical(false) {
                    return;
                }
                return self.code_vim_char('k', clipboard);
            }
            Key::Named(NamedKey::ArrowDown) => {
                if self.code_vim_vertical(true) {
                    return;
                }
                return self.code_vim_char('j', clipboard);
            }
            Key::Named(NamedKey::Backspace) => {
                return self.code_vim_char('h', clipboard)
            }
            Key::Named(NamedKey::PageUp) | Key::Named(NamedKey::PageDown) => {
                let down = matches!(key.logical_key.as_ref(), Key::Named(NamedKey::PageDown));
                let rows = self
                    .context_manager
                    .current()
                    .code
                    .as_ref()
                    .map(|code| code.geometry.viewport_rows())
                    .unwrap_or(1);
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    let extend = code.buffer.mode == CodeMode::Visual;
                    if !code.move_cursor_vertical_visual_n(down, rows, extend) {
                        let motion = if down {
                            CodeMotion::PageDown { rows }
                        } else {
                            CodeMotion::PageUp { rows }
                        };
                        code.buffer.apply_motion(motion, extend);
                    }
                }
                self.mark_dirty();
                return;
            }
            Key::Named(NamedKey::Space) if plain => {
                // Space is the leader in Normal mode (`<Space>x` closes
                // the buffer); Visual keeps it as a motion char.
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    if code.buffer.mode == CodeMode::Normal
                        && code.buffer.vim.pending.is_empty()
                    {
                        code.leader_pending = true;
                        self.mark_dirty();
                        return;
                    }
                }
                return self.code_vim_char(' ', clipboard);
            }
            _ => {}
        }

        if !plain {
            return;
        }
        let mut chars = text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return;
        };

        // Leader chord: `<Space>` armed — the next key selects the
        // action; unknown keys just disarm.
        let leader_armed = self
            .context_manager
            .current()
            .code
            .as_ref()
            .is_some_and(|code| code.leader_pending);
        if leader_armed {
            if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                code.leader_pending = false;
            }
            match ch {
                'x' => {
                    let _ = self.close_focused_buffer_tab();
                }
                // <Space>a: LSP code actions / quick-fix at the cursor.
                'a' => self.request_code_actions(),
                // <Space>r: LSP rename symbol (modal prompt).
                'r' => self.open_code_rename_prompt(),
                _ => {}
            }
            self.mark_dirty();
            return;
        }

        // `/` opens in-buffer incremental search (nvim incsearch).
        let pending_empty_for_search = self
            .context_manager
            .current()
            .code
            .as_ref()
            .is_some_and(|code| code.buffer.vim.pending.is_empty());
        if ch == '/' && pending_empty_for_search {
            self.open_finder_buffer_search();
            return;
        }

        // `:` opens the command palette (the vim ex-command surface).
        let pending_empty = self
            .context_manager
            .current()
            .code
            .as_ref()
            .is_some_and(|code| code.buffer.vim.pending.is_empty());
        if ch == ':' && pending_empty {
            self.open_command_palette();
            return;
        }

        // LSP keys the resolver doesn't own: `K` hover, `g d`
        // go-to-definition, and `g r` find-references. `gd`/`gr`
        // piggyback on the resolver's pending `g` (Gee) stage without
        // touching the shared vim model.
        if let Some(code) = self.context_manager.current().code.as_ref() {
            let normal = code.buffer.mode == CodeMode::Normal;
            let pending = &code.buffer.vim.pending;
            if normal && ch == 'K' && pending.is_empty() {
                self.request_code_hover();
                self.mark_dirty();
                return;
            }
            if normal
                && ch == 'd'
                && pending.operator.is_none()
                && pending.stage == VimStage::Gee
            {
                let cursor = code.buffer.cursor();
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    code.buffer.vim.clear_pending();
                }
                self.request_code_definition_at(cursor.line, cursor.col);
                self.mark_dirty();
                return;
            }
            if normal
                && ch == 'r'
                && pending.operator.is_none()
                && pending.stage == VimStage::Gee
            {
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    code.buffer.vim.clear_pending();
                }
                self.request_code_references();
                self.mark_dirty();
                return;
            }
        }

        if (ch == 'j' || ch == 'k') && self.code_vim_vertical(ch == 'j') {
            return;
        }
        self.code_vim_char(ch, clipboard);
    }

    fn code_vim_char(&mut self, ch: char, clipboard: &mut Clipboard) {
        // Normal/Visual keys always settle the LSP popups (cursor is
        // about to move or the buffer to change).
        self.renderer.code_lsp.dismiss_popups();
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return;
        };
        let visual = code.buffer.mode == CodeMode::Visual;
        match code.buffer.vim.feed(ch, visual) {
            VimKeyFeed::Pending | VimKeyFeed::Cancelled => {}
            VimKeyFeed::Unhandled => return,
            VimKeyFeed::Action(action) => {
                // Only hit the OS clipboard when the action can paste.
                let paste = matches!(
                    action,
                    VimAction::Paste { .. } | VimAction::Repeat { .. }
                )
                .then(|| clipboard.get(ClipboardType::Clipboard));
                let Some(code) = self.context_manager.current_mut().code.as_mut()
                else {
                    return;
                };
                let applied = code.buffer.apply_vim_action(&action, paste.as_deref());
                if let Some(register) = applied.register {
                    if applied.yank_notification {
                        let lines = register.lines().count().max(1);
                        let message = if lines == 1 {
                            "Yanked 1 line".to_string()
                        } else {
                            format!("Yanked {lines} lines")
                        };
                        self.renderer.notifications.push(
                            message,
                            neoism_ui::panels::notifications::NotificationLevel::Info,
                        );
                    }
                    clipboard.set(ClipboardType::Clipboard, register);
                }
            }
        }
        self.sync_active_code_modified();
        self.mark_dirty();
    }

    pub(crate) fn code_scrollbar_drag_active(&self) -> bool {
        self.context_manager
            .current()
            .code
            .as_ref()
            .is_some_and(|code| code.scrollbar_drag.is_some())
    }

    /// 1:1 thumb drag: pointer position maps straight to scroll (no
    /// spring — every editor's scrollbar tracks the hand exactly).
    pub(crate) fn handle_code_scrollbar_drag_move(&mut self) -> bool {
        let [_, my] = self.markdown_mouse_logical();
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return false;
        };
        let (Some(track), Some(thumb), Some(grab)) = (
            code.scrollbar_track,
            code.scrollbar_thumb,
            code.scrollbar_drag,
        ) else {
            return false;
        };
        let span = (track[3] - thumb[3]).max(1.0);
        let progress = ((my - grab - track[1]) / span).clamp(0.0, 1.0);
        code.set_scroll_progress(progress);
        self.mark_dirty();
        true
    }

    pub(crate) fn end_code_scrollbar_drag(&mut self) {
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            code.scrollbar_drag = None;
        }
    }

    /// Palette `ToggleViMode`: flip the pane between standard and vim
    /// input. Entering vim lands in Normal; leaving returns to plain
    /// insert-style editing.
    pub(crate) fn toggle_code_vim_mode(&mut self) -> bool {
        self.renderer.code_lsp.dismiss_popups();
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return false;
        };
        let entering = code.input_mode == CodeInputMode::Standard;
        if entering {
            code.input_mode = CodeInputMode::Vim;
            code.buffer.mode = CodeMode::Normal;
            code.buffer.clear_selection();
            code.buffer.break_undo_group();
            code.buffer.snap_normal_cursor();
        } else {
            code.input_mode = CodeInputMode::Standard;
            code.buffer.mode = CodeMode::Insert;
            code.buffer.vim.clear_pending();
            code.buffer.clear_selection();
        }
        self.renderer.notifications.push(
            if entering {
                "Vim mode on"
            } else {
                "Vim mode off"
            },
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        self.mark_dirty();
        true
    }

    pub(crate) fn handle_code_mouse_press(&mut self, button: MouseButton) -> bool {
        if self.context_manager.current().code.is_none() {
            return false;
        }
        if button != MouseButton::Left {
            return false;
        }
        let [x, y] = self.markdown_mouse_logical();
        let shift = self.modifiers.state().shift_key();
        let ctrl = self.modifiers.state().control_key();
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return false;
        };
        // Scrollbar first refusal: thumb press starts a 1:1 drag,
        // track press jumps a viewport toward the click (house style).
        if let (Some(track), Some(thumb)) = (code.scrollbar_track, code.scrollbar_thumb)
        {
            let in_track = x >= track[0] - 4.0
                && x <= track[0] + track[2] + 4.0
                && y >= track[1]
                && y <= track[1] + track[3];
            if in_track {
                if y >= thumb[1] && y <= thumb[1] + thumb[3] {
                    code.scrollbar_drag = Some(y - thumb[1]);
                } else {
                    let page = code.scroll_viewport_height();
                    let delta = if y < thumb[1] { -page } else { page };
                    code.scroll_pixels(-delta, page);
                }
                self.mark_dirty();
                return true;
            }
        }
        let (line, col) = code.geometry.hit_position(&code.buffer.lines, x, y);
        // Ctrl+Click: go to definition of the clicked identifier.
        if ctrl && !shift {
            code.buffer.set_cursor_position(line, col, false);
            self.dismiss_code_lsp_popups();
            self.request_code_definition_at(line, col);
            self.mark_dirty();
            return true;
        }
        code.buffer.set_cursor_position(line, col, shift);
        code.mouse_selecting = true;
        self.show_code_diagnostic_card_at(line, col);
        self.dismiss_code_lsp_popups();
        self.mark_dirty();
        true
    }

    pub(crate) fn code_drag_active(&self) -> bool {
        self.context_manager
            .current()
            .code
            .as_ref()
            .is_some_and(|code| code.mouse_selecting)
    }

    pub(crate) fn handle_code_drag_move(&mut self) -> bool {
        let [x, y] = self.markdown_mouse_logical();
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return false;
        };
        if !code.mouse_selecting {
            return false;
        }
        let (line, col) = code.geometry.hit_position(&code.buffer.lines, x, y);
        code.buffer.set_cursor_position(line, col, true);
        self.mark_dirty();
        true
    }

    pub(crate) fn handle_code_mouse_release(&mut self) -> bool {
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return false;
        };
        if code.scrollbar_drag.take().is_some() {
            self.mark_dirty();
            return true;
        }
        if !code.mouse_selecting {
            return false;
        }
        code.mouse_selecting = false;
        true
    }
}
