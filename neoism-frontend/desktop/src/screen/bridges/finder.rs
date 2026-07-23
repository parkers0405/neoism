// Auto-split from screen/mod.rs. See sibling mod.rs for the Screen struct and
// the constructor/core methods. This file is part of the impl Screen<'_> block.

use super::super::*;
use neoism_backend::clipboard::{Clipboard, ClipboardType};
use neoism_terminal_core::crosswords::pos::{Boundary, Direction, Line};
use neoism_terminal_core::selection::SelectionType;
use neoism_ui::panels::finder::policy::{
    finder_cwd_decision, plan_finder_open, search_input_action, FinderCwdInputs,
    FinderOpenAction, SearchEdit, SearchInputAction,
};
use neoism_ui::panels::finder::FinderMode;

impl Screen<'_> {
    pub fn search_active(&self) -> bool {
        self.search_state.history_index.is_some()
    }

    pub(crate) fn search_pop_word(&mut self) {
        if let Some(regex) = self.search_state.regex_mut() {
            *regex = regex.trim_end().to_owned();
            regex.truncate(regex.rfind(' ').map_or(0, |i| i + 1));
            self.update_search();
        }
    }

    pub(crate) fn search_history_previous(&mut self) {
        let index = match &mut self.search_state.history_index {
            None => return,
            Some(index) if *index + 1 >= self.search_state.history.len() => return,
            Some(index) => index,
        };

        *index += 1;
        self.update_search();
    }

    pub(crate) fn search_history_next(&mut self) {
        let index = match &mut self.search_state.history_index {
            Some(0) | None => return,
            Some(index) => index,
        };

        *index -= 1;
        self.update_search();
    }

    pub(crate) fn advance_search_origin(&mut self, direction: Direction) {
        // Use focused match as new search origin if available.
        if let Some(focused_match) = &self.search_state.focused_match {
            let mut terminal = self.context_manager.current_mut().terminal.lock();
            let new_origin = match direction {
                Direction::Right => {
                    focused_match.end().add(&*terminal, Boundary::None, 1)
                }
                Direction::Left => {
                    focused_match.start().sub(&*terminal, Boundary::None, 1)
                }
            };

            terminal.scroll_to_pos(new_origin);
            drop(terminal);

            self.search_state.display_offset_delta = 0;
            self.search_state.origin = new_origin;
        }

        // Search for the next match using the supplied direction.
        let search_direction =
            std::mem::replace(&mut self.search_state.direction, direction);
        self.goto_match(None);
        self.search_state.direction = search_direction;

        // If we found a match, we set the search origin right in front of it to make sure that
        // after modifications to the regex the search is started without moving the focused match
        // around.
        let focused_match = match &self.search_state.focused_match {
            Some(focused_match) => focused_match,
            None => return,
        };

        // Set new origin to the left/right of the match, depending on search direction.
        let new_origin = match self.search_state.direction {
            Direction::Right => *focused_match.start(),
            Direction::Left => *focused_match.end(),
        };

        let mut terminal = self.context_manager.current_mut().terminal.lock();

        // Store the search origin with display offset by checking how far we need to scroll to it.
        let old_display_offset = terminal.display_offset() as i32;
        terminal.scroll_to_pos(new_origin);
        let new_display_offset = terminal.display_offset() as i32;
        self.search_state.display_offset_delta = new_display_offset - old_display_offset;

        // Store origin and scroll back to the match.
        terminal.scroll_display(Scroll::Delta(-self.search_state.display_offset_delta));
        drop(terminal);
        self.search_state.origin = new_origin;
    }

    pub fn handle_finder_click(&mut self) -> bool {
        if !self.renderer.finder.is_enabled() {
            return false;
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let size = self.sugarloaf.window_size();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        match self.renderer.finder.hit_test(
            mouse_x,
            mouse_y,
            (size.width as f32, size.height as f32, scale_factor),
        ) {
            Ok(Some(index)) => {
                self.renderer.finder.select_index(index);
                self.open_finder_selection();
                self.mark_dirty();
                true
            }
            Ok(None) => true,
            Err(()) => {
                // Click-outside dismisses; BufferLines mode restores
                // the pre-search cursor like Esc would.
                self.close_finder_overlay();
                self.finder_target_route = None;
                self.mark_dirty();
                true
            }
        }
    }

    /// Mode-aware finder dismissal: BufferLines cancels like Esc
    /// (restores the pre-search cursor, drops hlsearch); Symbols
    /// restores the pre-open cursor; every other mode just closes.
    /// Safe to call when the finder is already closed.
    pub(crate) fn close_finder_overlay(&mut self) {
        if !self.renderer.finder.is_enabled() {
            self.renderer.finder.close();
            return;
        }
        match self.renderer.finder.mode() {
            FinderMode::BufferLines | FinderMode::BufferReplace => {
                self.cancel_finder_buffer_search()
            }
            FinderMode::Symbols => self.cancel_finder_symbols(),
            _ => self.renderer.finder.close(),
        }
    }

    pub fn handle_search_click(&mut self, clipboard: &mut Clipboard) -> bool {
        if !self.renderer.search.is_active() {
            return false;
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let window_width = self.sugarloaf.window_size().width;
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();

        match self
            .renderer
            .search
            .hit_test(mouse_x, mouse_y, window_width, scale_factor)
        {
            Ok(Some(action)) => {
                use neoism_ui::panels::search::SearchOverlayAction;
                match action {
                    SearchOverlayAction::Next => {
                        self.advance_search_origin(self.search_state.direction);
                    }
                    SearchOverlayAction::Previous => {
                        let direction = self.search_state.direction.opposite();
                        self.advance_search_origin(direction);
                    }
                    SearchOverlayAction::Close => {
                        self.cancel_search(clipboard);
                        self.resize_top_or_bottom_line(self.ctx().len());
                    }
                }
                self.mark_dirty();
                true
            }
            Ok(None) => {
                // Clicked inside overlay but not on a button (input area)
                true
            }
            Err(()) => {
                // Clicked outside — don't close search, just pass through
                false
            }
        }
    }

    pub(crate) fn finder_target_route_for_current_focus(&self) -> Option<usize> {
        // No editor routes exist anymore — finder results open through
        // the native code/markdown panes, never a routed editor.
        None
    }

    pub(crate) fn finder_cwd(&self, _target_route: Option<usize>) -> std::path::PathBuf {
        finder_cwd_decision(FinderCwdInputs {
            active_pane_workspace_root: self.active_pane_workspace_root(),
            active_workspace_root: self.active_workspace_root.clone(),
            working_dir_config: self
                .context_manager
                .config
                .working_dir
                .clone()
                .map(std::path::PathBuf::from),
            fallback: std::env::current_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from(".")),
        })
    }

    pub fn open_finder_files(&mut self) {
        let _ = self.sync_workspace_root_from_active_pane();
        let target_route = self.finder_target_route_for_current_focus();
        let cwd = self.finder_cwd(target_route);
        self.finder_target_route = target_route;
        self.renderer.file_tree.set_focused(false);
        self.renderer.finder.open_files(cwd);
        self.mark_dirty();
    }

    pub fn open_finder_grep(&mut self) {
        let _ = self.sync_workspace_root_from_active_pane();
        let target_route = self.finder_target_route_for_current_focus();
        let cwd = self.finder_cwd(target_route);
        self.finder_target_route = target_route;
        self.renderer.file_tree.set_focused(false);
        self.renderer.finder.open_grep(cwd);
        self.mark_dirty();
    }

    #[allow(dead_code)]
    pub fn open_git_changes_finder(&mut self) {
        let _ = self.sync_workspace_root_from_active_pane();
        let target_route = self.finder_target_route_for_current_focus();
        let cwd = self.finder_cwd(target_route);
        self.finder_target_route = target_route;
        self.renderer.file_tree.set_focused(false);
        self.renderer
            .finder
            .open_git_changes(&self.renderer.finder_search, cwd);
        self.mark_dirty();
    }

    /// Search-plane reply from the daemon (finder searches for JOINED
    /// workspaces run on the host's disk). Routes into the shared
    /// finder's pending-request handler; returns true when the reply
    /// was consumed and the frame needs a redraw.
    pub(crate) fn apply_daemon_search_message(
        &mut self,
        request_id: u64,
        message: &neoism_protocol::search::SearchServerMessage,
    ) -> bool {
        if !self.renderer.finder.is_enabled() {
            return false;
        }
        let Ok(payload) = serde_json::to_value(message) else {
            return false;
        };
        let renderer = &mut self.renderer;
        let consumed = renderer.finder.handle_service_reply(
            request_id,
            &payload,
            &renderer.finder_search,
        );
        if consumed {
            self.mark_dirty();
        }
        consumed
    }

    pub fn open_finder_selection(&mut self) {
        // BufferLines rows have no path — Enter (and row clicks)
        // commit the in-buffer search instead of opening a file.
        if self.renderer.finder.mode() == FinderMode::BufferLines {
            self.confirm_finder_buffer_search();
            return;
        }
        // BufferReplace rows have no path either — Enter runs the
        // whole-file substitute.
        if self.renderer.finder.mode() == FinderMode::BufferReplace {
            self.confirm_finder_buffer_replace();
            return;
        }
        // Symbols rows have no path either — Enter jumps the active
        // code pane to the selected symbol.
        if self.renderer.finder.mode() == FinderMode::Symbols {
            self.confirm_finder_symbols();
            return;
        }
        // References rows carry an exact byte column — jump straight
        // through the code-pane open path (the gd cross-file pattern)
        // instead of the generic line-only open plan.
        if self.renderer.finder.mode() == FinderMode::References {
            let target = self.renderer.finder.selected_reference_target();
            self.renderer.finder.close();
            self.finder_target_route = None;
            self.renderer.file_tree.set_focused(false);
            if let Some((path, line, col)) = target {
                self.open_code_location(
                    path,
                    (line as usize).saturating_sub(1),
                    col as usize,
                );
            }
            self.mark_dirty();
            return;
        }
        let Some((path, line)) = self.renderer.finder.selected_open_target() else {
            return;
        };
        let mode = self.renderer.finder.mode();
        let query = self.renderer.finder.query.clone();
        self.renderer.finder.close();
        let target_route = self
            .finder_target_route
            .take()
            .or_else(|| self.finder_target_route_for_current_focus());
        // Finder is part of the editor chrome — make sure focus
        // returns to the editor pane (the tree may still hold focus
        // from a prior <leader>e). Without this, hjkl after Enter
        // would steer the tree, not the buffer.
        self.renderer.file_tree.set_focused(false);

        // POD decision: the open plan lives in
        // `neoism_ui::panels::finder::policy`. nvim removed — non-markdown
        // targets open in the native code pane; a grep/git hit jumps the
        // code cursor to the matched line.
        let request = neoism_ui::panels::finder::policy::FinderOpenRequest {
            path,
            line,
            mode,
            query,
        };
        match plan_finder_open(request, target_route) {
            FinderOpenAction::OpenMarkdown { path, line } => {
                self.open_path_in_markdown(path);
                if let Some(line) = line {
                    if let Some(markdown) =
                        self.context_manager.current_mut().markdown.as_mut()
                    {
                        markdown.jump_to_line(line as usize);
                        self.renderer.trail_cursor.reset();
                    }
                }
            }
            FinderOpenAction::EditAtLine {
                path,
                line,
                target_route,
                grep_query: _,
                is_git: _,
            } => {
                self.open_finder_target_tab(target_route, &path);
                self.open_path_in_editor(path.clone());
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    code.buffer.set_cursor_position(
                        (line as usize).saturating_sub(1),
                        0,
                        false,
                    );
                    self.renderer.trail_cursor.reset();
                }
            }
            FinderOpenAction::EditFile { path, target_route } => {
                self.open_finder_target_tab(target_route, &path);
                self.open_path_in_editor(path.clone());
            }
        }
        self.mark_dirty();
    }

    fn open_finder_target_tab(
        &mut self,
        target_route: Option<usize>,
        path: &std::path::Path,
    ) {
        self.clear_current_workspace_buf_enter_guard();
        self.renderer
            .file_tree
            .set_active_path(Some(path.to_path_buf()));
        if let Some(id) = self.current_workspace_id() {
            self.workspace_editor_active_paths
                .insert(id, path.to_path_buf());
        }

        if let Some(route) = target_route {
            let cwd = self.active_pane_workspace_root();
            if let Some(tabs) = self.renderer.pane_tabs.get_mut(&route) {
                tabs.open_path(path.to_path_buf());
                if let Some(crumbs) = self.renderer.pane_breadcrumbs.get_mut(&route) {
                    crumbs.set_from_path(path, cwd.as_deref());
                }
                return;
            }
        }

        self.renderer.buffer_tabs.ensure_terminal_tab();
        self.renderer.buffer_tabs.open_path(path.to_path_buf());
    }

    /// `/` (or `?` with `backward`) on the code pane: open the finder
    /// in BufferLines mode over a snapshot of the buffer (the same
    /// centered floating bar as Ctrl+P). Typing live-jumps from the
    /// origin (incsearch) and lists matching lines; Enter commits and
    /// arms `n`/`N` (reversed for `?`); Esc restores the origin. No-op
    /// when no code pane is active.
    pub(crate) fn open_finder_buffer_search(&mut self, backward: bool) {
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return;
        };
        code.search_origin = Some((code.buffer.cursor_line, code.buffer.cursor_col));
        code.search_backward = backward;
        let lines = code.buffer.lines.clone();
        self.finder_target_route = None;
        self.renderer.file_tree.set_focused(false);
        self.renderer.finder.open_buffer_lines(lines);
        self.mark_dirty();
    }

    /// Palette "Replace in File": open the finder in BufferReplace
    /// mode over the active code pane — the same centered bar, query
    /// typed as `pattern/replacement` (`:s` escaping), Enter replaces
    /// every occurrence in the file. No-op without a code pane.
    pub(crate) fn open_finder_buffer_replace(&mut self) {
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return;
        };
        code.search_origin = Some((code.buffer.cursor_line, code.buffer.cursor_col));
        code.search_backward = false;
        let lines = code.buffer.lines.clone();
        self.finder_target_route = None;
        self.renderer.file_tree.set_focused(false);
        self.renderer.finder.open_buffer_replace(lines);
        self.mark_dirty();
    }

    /// Enter in BufferReplace mode: parse `pattern/replacement`, run a
    /// whole-file global substitute through the `:s` engine (one undo
    /// step, count toast, hlsearch + `n` armed), close the finder.
    /// An empty pattern behaves like Esc.
    fn confirm_finder_buffer_replace(&mut self) {
        let query = self.renderer.finder.query.clone();
        let (pattern, replacement) =
            neoism_ui::editor::code::substitute::split_replace_query(&query);
        if pattern.is_empty() {
            self.cancel_finder_buffer_search();
            return;
        }
        self.renderer.finder.close();
        self.renderer.file_tree.set_focused(false);
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            code.search_origin = None;
        }
        let spec = neoism_ui::editor::code::substitute::SubstituteSpec {
            range: neoism_ui::editor::code::substitute::SubstituteRange::WholeFile,
            pattern,
            replacement: replacement.unwrap_or_default(),
            global: true,
            case_insensitive: false,
        };
        self.run_code_substitute(&spec);
    }

    /// BufferLines query changed: live-drive the pane — hlsearch bands
    /// for every occurrence, cursor on the first match at/after the
    /// origin (wrapping). Empty query restores the origin. No-op in
    /// other finder modes.
    pub(crate) fn finder_buffer_query_changed(&mut self) {
        use neoism_ui::editor::markdown::vim::vim_search_forward;
        use neoism_ui::editor::markdown::MarkdownPosition;
        let mode = self.renderer.finder.mode();
        if !matches!(mode, FinderMode::BufferLines | FinderMode::BufferReplace) {
            return;
        }
        let query = self.renderer.finder.query.clone();
        // Replace mode live-drives on the PATTERN half of the query.
        let query = if mode == FinderMode::BufferReplace {
            neoism_ui::editor::code::substitute::split_replace_query(&query).0
        } else {
            query
        };
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return;
        };
        let Some((origin_line, origin_col)) = code.search_origin else {
            return;
        };
        if query.is_empty() {
            code.buffer
                .set_cursor_position(origin_line, origin_col, false);
            code.buffer.follow_cursor = true;
            code.search_highlight = None;
            self.mark_dirty();
            return;
        }
        code.search_highlight = Some(query.clone());
        // Both helpers exclude the exact start position, so nudge the
        // origin one step the other way — a match AT the origin is
        // found either direction.
        let found = if code.search_backward {
            let start = MarkdownPosition {
                line: origin_line,
                col: origin_col + 1,
            };
            neoism_ui::editor::markdown::vim::vim_search_backward(
                &code.buffer.lines,
                start,
                &query,
                false,
            )
        } else {
            let start = if origin_col > 0 {
                MarkdownPosition {
                    line: origin_line,
                    col: origin_col - 1,
                }
            } else if origin_line > 0 {
                MarkdownPosition {
                    line: origin_line - 1,
                    col: usize::MAX,
                }
            } else {
                MarkdownPosition { line: 0, col: 0 }
            };
            vim_search_forward(&code.buffer.lines, start, &query, false)
        };
        if let Some(found) = found {
            code.buffer
                .set_cursor_position(found.line, found.col, false);
            code.buffer.follow_cursor = true;
        }
        self.mark_dirty();
    }

    /// BufferLines selection moved (arrows / wheel): live-preview by
    /// jumping the pane to the selected row's match. No-op in other
    /// finder modes.
    pub(crate) fn finder_buffer_preview_selected(&mut self) {
        let mode = self.renderer.finder.mode();
        if !matches!(mode, FinderMode::BufferLines | FinderMode::BufferReplace) {
            return;
        }
        let query = self.renderer.finder.query.clone();
        let query = if mode == FinderMode::BufferReplace {
            neoism_ui::editor::code::substitute::split_replace_query(&query).0
        } else {
            query
        };
        let Some(row_line) = self.renderer.finder.selected_line() else {
            return;
        };
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return;
        };
        let line_ix = (row_line as usize).saturating_sub(1);
        let col = code
            .buffer
            .lines
            .get(line_ix)
            .and_then(|line| line.find(&query))
            .unwrap_or(0);
        code.buffer.set_cursor_position(line_ix, col, false);
        code.buffer.follow_cursor = true;
        self.mark_dirty();
    }

    /// Enter in BufferLines mode: jump to the selected row's match,
    /// arm `n`/`N` with the committed pattern, keep hlsearch bands,
    /// forget the origin, close the finder. An empty query behaves
    /// like Esc (nothing to commit).
    fn confirm_finder_buffer_search(&mut self) {
        let query = self.renderer.finder.query.clone();
        let selected_line = self.renderer.finder.selected_line();
        self.renderer.finder.close();
        self.renderer.file_tree.set_focused(false);
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            self.mark_dirty();
            return;
        };
        if query.is_empty() {
            if let Some((line, col)) = code.search_origin.take() {
                code.buffer.set_cursor_position(line, col, false);
                code.buffer.follow_cursor = true;
            }
            code.search_highlight = None;
            self.mark_dirty();
            return;
        }
        if let Some(row_line) = selected_line {
            let line_ix = (row_line as usize).saturating_sub(1);
            let col = code
                .buffer
                .lines
                .get(line_ix)
                .and_then(|line| line.find(&query))
                .unwrap_or(0);
            code.buffer.set_cursor_position(line_ix, col, false);
            code.buffer.follow_cursor = true;
        }
        // No selected row (no matches): keep the cursor where the
        // live incsearch left it, but still commit the pattern.
        code.search_highlight = Some(query.clone());
        code.buffer.vim.search = Some(neoism_ui::editor::markdown::vim::VimSearch {
            pattern: query,
            // `?` commits with the direction reversed: `n` continues
            // up, `N` back down (nvim semantics).
            forward: !code.search_backward,
            whole_word: false,
        });
        code.search_origin = None;
        self.mark_dirty();
    }

    /// Esc in BufferLines mode: restore the pre-search cursor, drop
    /// the hlsearch bands, close the finder.
    pub(crate) fn cancel_finder_buffer_search(&mut self) {
        self.renderer.finder.close();
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            if let Some((line, col)) = code.search_origin.take() {
                code.buffer.set_cursor_position(line, col, false);
                code.buffer.follow_cursor = true;
            }
            code.search_highlight = None;
        }
        self.mark_dirty();
    }

    /// Palette "Go to Symbol…" / direct entry point: open the finder
    /// in Symbols mode over the active code pane's document symbols
    /// (VS Code Ctrl+P `@`). The rows are fetched on the code-LSP
    /// worker; the drain installs them into the finder when they
    /// land. Remembers the pre-open cursor so Esc restores it (the
    /// search_origin pattern).
    pub(crate) fn open_finder_symbols(&mut self) {
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            code.search_origin = Some((code.buffer.cursor_line, code.buffer.cursor_col));
        }
        self.finder_target_route = None;
        self.renderer.file_tree.set_focused(false);
        self.renderer.finder.open_symbols();
        let requested = self.request_code_document_symbols();
        self.renderer.finder.set_symbols_loading(requested);
        self.mark_dirty();
    }

    /// Files-mode `@` prefix (VS Code Ctrl+P `@`): when the typed
    /// query starts with `@`, flip the open finder into Symbols mode
    /// live, keeping whatever followed the `@` as the effective
    /// query. No-op in every other state.
    pub(crate) fn finder_symbols_switch_from_prefix(&mut self) {
        if !self.renderer.finder.is_enabled()
            || self.renderer.finder.mode() != FinderMode::Files
        {
            return;
        }
        let Some(rest) = self
            .renderer
            .finder
            .query
            .strip_prefix('@')
            .map(str::to_string)
        else {
            return;
        };
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            code.search_origin = Some((code.buffer.cursor_line, code.buffer.cursor_col));
        }
        self.renderer.finder.switch_to_symbols(rest);
        let requested = self.request_code_document_symbols();
        self.renderer.finder.set_symbols_loading(requested);
        self.mark_dirty();
    }

    /// Backspace on an EMPTY Symbols query: backspacing the `@` away
    /// returns to Files mode — the inverse of the prefix switch.
    /// Restores the pre-open cursor (arrow previews may have moved
    /// it). Returns true when the switch happened.
    pub(crate) fn finder_symbols_backspace_to_files(&mut self) -> bool {
        if !self.renderer.finder.is_enabled()
            || self.renderer.finder.mode() != FinderMode::Symbols
            || !self.renderer.finder.query.is_empty()
        {
            return false;
        }
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            if let Some((line, col)) = code.search_origin.take() {
                code.buffer.set_cursor_position(line, col, false);
                code.buffer.follow_cursor = true;
            }
        }
        let cwd = self.finder_cwd(self.finder_target_route);
        self.renderer.finder.switch_to_files(cwd);
        self.mark_dirty();
        true
    }

    /// Symbols selection moved (arrows): live-preview by jumping the
    /// pane to the selected symbol's line, like the BufferLines
    /// preview. No-op in other finder modes.
    pub(crate) fn finder_symbols_preview_selected(&mut self) {
        if self.renderer.finder.mode() != FinderMode::Symbols {
            return;
        }
        let Some((line, col)) = self.renderer.finder.selected_symbol_target() else {
            return;
        };
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return;
        };
        let line_ix = (line as usize).saturating_sub(1);
        code.buffer
            .set_cursor_position(line_ix, col as usize, false);
        code.buffer.follow_cursor = true;
        self.mark_dirty();
    }

    /// Enter in Symbols mode: jump to the selected symbol, forget the
    /// pre-open origin, close the finder. No selection behaves like
    /// Esc (restores the pre-open cursor).
    fn confirm_finder_symbols(&mut self) {
        let target = self.renderer.finder.selected_symbol_target();
        self.renderer.finder.close();
        self.renderer.file_tree.set_focused(false);
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            self.mark_dirty();
            return;
        };
        match target {
            Some((line, col)) => {
                code.search_origin = None;
                let line_ix = (line as usize).saturating_sub(1);
                code.buffer
                    .set_cursor_position(line_ix, col as usize, false);
                code.buffer.follow_cursor = true;
            }
            None => {
                if let Some((line, col)) = code.search_origin.take() {
                    code.buffer.set_cursor_position(line, col, false);
                    code.buffer.follow_cursor = true;
                }
            }
        }
        self.mark_dirty();
    }

    /// Esc in Symbols mode: restore the pre-open cursor, close the
    /// finder. hlsearch state is untouched — Symbols never sets it.
    pub(crate) fn cancel_finder_symbols(&mut self) {
        self.renderer.finder.close();
        if let Some(code) = self.context_manager.current_mut().code.as_mut() {
            if let Some((line, col)) = code.search_origin.take() {
                code.buffer.set_cursor_position(line, col, false);
                code.buffer.follow_cursor = true;
            }
        }
        self.mark_dirty();
    }

    pub(crate) fn start_search(&mut self, direction: Direction) {
        // Only create new history entry if the previous regex wasn't empty.
        if self
            .search_state
            .history
            .front()
            .is_none_or(|regex| !regex.is_empty())
        {
            self.search_state.history.push_front(String::new());
            self.search_state.history.truncate(MAX_SEARCH_HISTORY_SIZE);
        }

        self.search_state.history_index = Some(0);
        self.search_state.direction = direction;
        self.search_state.focused_match = None;

        // Store original search position as origin and reset location.
        if self.get_mode().contains(Mode::VI) {
            let terminal = self.context_manager.current().terminal.lock();
            self.search_state.origin = terminal.vi_mode_cursor.pos;
            self.search_state.display_offset_delta = 0;

            // Adjust origin for content moving upward on search start.
            if terminal.grid.cursor.pos.row + 1 == terminal.screen_lines() {
                self.search_state.origin.row -= 1;
            }
            drop(terminal);
        } else {
            let terminal = self.context_manager.current().terminal.lock();
            let viewport_top = Line(-(terminal.grid.display_offset() as i32)) - 1;
            let viewport_bottom = viewport_top + terminal.bottommost_line();
            let last_column = terminal.last_column();
            self.search_state.origin = match direction {
                Direction::Right => Pos::new(viewport_top, Column(0)),
                Direction::Left => Pos::new(viewport_bottom, last_column),
            };
            drop(terminal);
        }

        // Enable IME so we can input into the search bar with it if we were in Vi mode.
        // self.window().set_ime_allowed(true);

        self.mark_dirty();
    }

    pub(crate) fn confirm_search(&mut self, clipboard: &mut Clipboard) {
        // Just cancel search when not in vi mode.
        if !self.get_mode().contains(Mode::VI) {
            self.cancel_search(clipboard);
            return;
        }

        // Force unlimited search if the previous one was interrupted.
        // let timer_id = TimerId::new(Topic::DelayedSearch, self.display.window.id());
        // if self.scheduler.scheduled(timer_id) {
        // self.goto_match(None);
        // }

        self.exit_search();
    }

    pub(crate) fn cancel_search(&mut self, clipboard: &mut Clipboard) {
        if self.get_mode().contains(Mode::VI) {
            // Recover pre-search state in vi mode.
            self.search_reset_state();
        } else if let Some(focused_match) = &self.search_state.focused_match {
            // Create a selection for the focused match.
            let start = *focused_match.start();
            let end = *focused_match.end();
            self.start_selection(SelectionType::Simple, start, Side::Left, clipboard);
            self.update_selection(end, Side::Right);
            self.copy_selection(ClipboardType::Selection, clipboard);
        }

        self.search_state.dfas = None;
        self.exit_search();
        self.update_hint_state();
    }

    pub(crate) fn exit_search(&mut self) {
        // let vi_mode = self.get_mode().contains(Mode::VI);
        // self.window().set_ime_allowed(!vi_mode);

        self.search_state.history_index = None;

        // Clear focused match.
        self.search_state.focused_match = None;

        self.mark_dirty();
    }

    pub(crate) fn search_input(&mut self, c: char) {
        // POD decision: which slot to edit and whether the char is
        // printable lives in `policy::search_input_action`.
        let action = search_input_action(c, self.search_state.history_index);
        let edit = match action {
            SearchInputAction::Ignore | SearchInputAction::IgnoreNonPrintable => return,
            SearchInputAction::PromoteHistory { source_index, edit } => {
                self.search_state.history[0] =
                    self.search_state.history[source_index].clone();
                self.search_state.history_index = Some(0);
                edit
            }
            SearchInputAction::Apply { edit } => edit,
        };

        let regex = &mut self.search_state.history[0];
        match edit {
            SearchEdit::Backspace => {
                let _ = regex.pop();
            }
            SearchEdit::Push(c) => regex.push(c),
        }

        let mode = self.get_mode();
        if !mode.contains(Mode::VI) {
            // Clear selection so we do not obstruct any matches.
            self.context_manager.current_mut().set_selection(None);
        }

        self.update_search();
        self.mark_dirty();
    }

    pub(crate) fn update_search(&mut self) {
        let regex = match self.search_state.regex() {
            Some(regex) => regex,
            None => return,
        };

        if regex.is_empty() {
            // Stop search if there's nothing to search for.
            self.search_reset_state();
            self.search_state.dfas = None;
        } else {
            // Create search dfas for the new regex string.
            self.search_state.dfas = RegexSearch::new(regex).ok();

            // Update search highlighting.
            self.goto_match(MAX_SEARCH_WHILE_TYPING);
        }
    }

    pub(crate) fn search_reset_state(&mut self) {
        // Unschedule pending timers.
        // let timer_id = TimerId::new(Topic::DelayedSearch, self.display.window.id());
        // self.scheduler.unschedule(timer_id);

        // Clear focused match.
        self.search_state.focused_match = None;

        // The viewport reset logic is only needed for vi mode, since without it our origin is
        // always at the current display offset instead of at the vi cursor position which we need
        // to recover to.
        let mode = self.get_mode();
        if !mode.contains(Mode::VI) {
            return;
        }

        // Reset display offset and cursor position.
        {
            let mut terminal = self.context_manager.current_mut().terminal.lock();
            terminal.vi_mode_cursor.pos = self.search_state.origin;
            terminal
                .scroll_display(Scroll::Delta(self.search_state.display_offset_delta));
            drop(terminal);
        }
        self.search_state.display_offset_delta = 0;
    }

    pub(crate) fn goto_match(&mut self, mut limit: Option<usize>) {
        let dfas = match &mut self.search_state.dfas {
            Some(dfas) => dfas,
            None => return,
        };

        let mut should_reset_search_state = false;

        // Jump to the next match.
        {
            let mut terminal = self.context_manager.current_mut().terminal.lock();
            // Limit search only when enough lines are available to run into the limit.
            limit = limit.filter(|&limit| limit <= terminal.total_lines());

            let direction = self.search_state.direction;
            let clamped_origin = self
                .search_state
                .origin
                .grid_clamp(&*terminal, Boundary::Grid);
            match terminal.search_next(dfas, clamped_origin, direction, Side::Left, limit)
            {
                Some(regex_match) => {
                    let old_offset = terminal.display_offset() as i32;
                    if terminal.mode().contains(Mode::VI) {
                        // Move vi cursor to the start of the match.
                        terminal.vi_goto_pos(*regex_match.start());
                    } else {
                        // Select the match when vi mode is not active.
                        terminal.scroll_to_pos(*regex_match.start());
                    }

                    // Update the focused match.
                    self.search_state.focused_match = Some(regex_match);

                    // Store number of lines the viewport had to be moved.
                    let display_offset = terminal.display_offset();
                    self.search_state.display_offset_delta +=
                        old_offset - display_offset as i32;

                    // Since we found a result, we require no delayed re-search.
                    // let timer_id = TimerId::new(Topic::DelayedSearch, self.display.window.id());
                    // self.scheduler.unschedule(timer_id);
                }
                // Reset viewport only when we know there is no match, to prevent unnecessary jumping.
                None if limit.is_none() => {
                    should_reset_search_state = true;
                }
                None => {
                    // Schedule delayed search if we ran into our search limit.
                    // let timer_id = TimerId::new(Topic::DelayedSearch, self.display.window.id());
                    // if !self.scheduler.scheduled(timer_id) {
                    // let event = Event::new(EventType::SearchNext, self.display.window.id());
                    // self.scheduler.schedule(event, TYPING_SEARCH_DELAY, false, timer_id);
                    // }

                    // Clear focused match.
                    self.search_state.focused_match = None;
                }
            }
            drop(terminal);
        }

        if should_reset_search_state {
            self.search_reset_state();
        }
    }
}
