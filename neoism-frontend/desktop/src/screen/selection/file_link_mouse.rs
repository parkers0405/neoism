use super::*;

impl Screen<'_> {
    pub(crate) fn draw_terminal_file_link_hover(&mut self) {
        let scale_factor = self.sugarloaf.scale_factor();
        if !self.contains_point(self.mouse.x, self.mouse.y) {
            return;
        }
        let display_offset = {
            let current = self.context_manager.current();
            if current.code.is_some()
                || current.markdown.is_some()
                || current.notebook.is_some()
                || current.neoism_agent.is_some()
                || current.neoism_tags.is_some()
            {
                return;
            }
            let Some(terminal) = current.terminal.try_lock_unfair() else {
                self.context_manager
                    .current_mut()
                    .renderable_content
                    .pending_update
                    .set_dirty();
                return;
            };
            terminal.display_offset()
        };
        let Some(point) = self.terminal_body_mouse_position(display_offset) else {
            return;
        };
        let Some(link) = self.terminal_file_link_at(point) else {
            return;
        };
        let current_grid = self.context_manager.current_grid();
        let scaled_margin = current_grid.get_scaled_margin();
        let Some(item) = current_grid.current_item() else {
            return;
        };
        let dim = item.val.dimension;
        let cell_w = dim.dimension.width.round().max(1.0);
        let cell_h = dim.dimension.height.round().max(1.0);

        let panel_left_logical =
            (item.layout_rect[0] + scaled_margin.left) / scale_factor;
        let panel_top_logical = (item.layout_rect[1] + scaled_margin.top) / scale_factor;
        let terminal_scroll_offset_logical = self
            .renderer
            .terminal_scroll
            .current_offset(item.val.rich_text_id)
            / scale_factor;
        let cell_w_logical = cell_w / scale_factor;
        let cell_h_logical = cell_h / scale_factor;

        let Some(rect) = terminal_file_link_hover_rect(TerminalFileLinkHoverInput {
            link_col_start: link.col_start,
            link_col_end: link.col_end,
            panel_left_logical,
            panel_top_logical,
            terminal_scroll_offset_logical,
            cell_width_logical: cell_w_logical,
            cell_height_logical: cell_h_logical,
            visible_lines: dim.lines,
            mouse_y_logical: self.mouse.y as f32 / scale_factor,
        }) else {
            return;
        };
        // `theme.blue` so the link reads as a hyperlink on every
        // theme even when `theme.accent` happens to match foreground.
        let color = self.renderer.theme.f32(self.renderer.theme.blue);
        self.sugarloaf.rect(
            None,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            color,
            0.0,
            TERMINAL_BLOCK_CHROME_ORDER,
        );
    }

    pub fn terminal_file_link_at_mouse(
        &self,
    ) -> Option<crate::terminal::file_link::FileLink> {
        if !self.contains_point(self.mouse.x, self.mouse.y) {
            return None;
        }
        let display_offset = {
            let current = self.context_manager.current();
            if current.code.is_some()
                || current.markdown.is_some()
                || current.notebook.is_some()
                || current.neoism_agent.is_some()
                || current.neoism_tags.is_some()
            {
                return None;
            }
            current
                .terminal
                .try_lock_unfair()
                .map(|terminal| terminal.display_offset())?
        };
        let point = self.terminal_body_mouse_position(display_offset)?;
        self.terminal_file_link_at(point)
    }

    pub(crate) fn terminal_file_link_at(
        &self,
        point: Pos,
    ) -> Option<crate::terminal::file_link::FileLink> {
        let current = self.context_manager.current();
        if current.code.is_some()
            || current.markdown.is_some()
            || current.notebook.is_some()
            || current.neoism_agent.is_some()
            || current.neoism_tags.is_some()
        {
            return None;
        }
        let cwd = self.current_terminal_completion_cwd();
        let probe = {
            let terminal = current.terminal.try_lock_unfair()?;
            terminal_file_link_probe(&terminal, point)?
        };
        crate::terminal::file_link::detect_at(
            &probe.row_text,
            probe.col,
            cwd.as_deref(),
            probe.abs_row,
        )
    }

    pub fn on_left_click(&mut self, point: Option<Pos>, clipboard: &mut Clipboard) {
        let side = self.mouse.square_side;

        // Block-hover icon click takes precedence — copy / filter
        // icons are anchored to the right edge of the header row,
        // away from typical text content, so a hit there is almost
        // certainly an icon click rather than a selection drag.
        if matches!(self.mouse.click_state, ClickState::Click) {
            let mouse_x_logical = self.mouse.x as f32 / self.sugarloaf.scale_factor();
            let mouse_y_logical = self.mouse.y as f32 / self.sugarloaf.scale_factor();
            let hit = self
                .block_hover_icons
                .iter()
                .find(|icon| {
                    let r = icon.rect;
                    mouse_x_logical >= r[0]
                        && mouse_x_logical < r[0] + r[2]
                        && mouse_y_logical >= r[1]
                        && mouse_y_logical < r[1] + r[3]
                })
                .copied();
            if let Some(icon) = hit {
                let now = Instant::now();
                let hover_started = self
                    .block_hover_icon_visual
                    .filter(|state| {
                        state.block_idx == icon.block_idx && state.action == icon.action
                    })
                    .map_or(now, |state| state.hover_started);
                self.block_hover_icon_visual = Some(BlockHoverIconVisualState {
                    block_idx: icon.block_idx,
                    action: icon.action,
                    hover_started,
                    clicked_at: Some(now),
                });
                self.execute_block_hover_action(icon, clipboard);
                self.mark_dirty();
                return;
            }
        }

        let Some(point) = point else {
            return;
        };

        // File-link click: a single-click without Shift/Ctrl/Alt over a
        // resolvable file/dir token in terminal output opens the path
        // in the editor instead of starting a selection. Drag still
        // works because we only intercept on the first click of a
        // ClickState::Click — drag continuations don't re-enter here.
        let click_kind = Self::selection_click_kind(&self.mouse.click_state);
        let selection_modifiers = self.selection_modifiers();

        if should_open_file_link_on_click(click_kind, selection_modifiers) {
            if let Some(link) = self.terminal_file_link_at(point) {
                let path = link.path;
                match file_link_open_target(
                    path.is_dir(),
                    crate::editor::markdown::state::is_markdown_path(&path),
                ) {
                    FileLinkOpenTarget::Directory => {
                        self.open_directory_link_in_file_tree(path);
                        return;
                    }
                    FileLinkOpenTarget::Markdown => self.open_path_in_markdown(path),
                    FileLinkOpenTarget::Editor => self.open_path_in_editor(path),
                }
                self.mark_dirty();
                return;
            }
        }

        match left_click_selection_action(
            click_kind,
            selection_modifiers,
            !self.selection_is_empty(),
            point,
            side,
        ) {
            LeftClickSelectionAction::None => {}
            LeftClickSelectionAction::Extend { point, side } => {
                self.update_selection(point, side);
            }
            LeftClickSelectionAction::Start {
                ty,
                point,
                side,
                clear_existing,
            } => {
                if clear_existing {
                    self.clear_selection();
                }
                self.start_selection(ty, point, side, clipboard);
            }
        };

        // Move vi mode cursor to mouse click position.
        let mut terminal = self.context_manager.current_mut().terminal.lock();
        if terminal.mode().contains(Mode::VI) {
            terminal.vi_mode_cursor.pos = point;
        }
        drop(terminal);
    }

    pub(crate) fn sgr_mouse_report(&mut self, pos: Pos, button: u8, state: ElementState) {
        let pressed = matches!(state, ElementState::Pressed);
        let msg = encode_sgr_mouse_report(pos, button, pressed);
        self.ctx_mut().current_mut().messenger.send_write(msg);
    }

    pub fn has_mouse_motion_and_drag(&mut self) -> bool {
        self.get_mode()
            .intersects(Mode::MOUSE_MOTION | Mode::MOUSE_DRAG)
    }

    pub fn has_mouse_motion(&mut self) -> bool {
        self.get_mode().intersects(Mode::MOUSE_MOTION)
    }

    pub fn mouse_report(&mut self, button: u8, state: ElementState) {
        let Some(terminal) = self.ctx().current().terminal.try_lock_unfair() else {
            self.ctx_mut()
                .current_mut()
                .renderable_content
                .pending_update
                .set_dirty();
            return;
        };
        let display_offset = terminal.display_offset();
        let mode = terminal.mode();
        drop(terminal);

        let pos = self.mouse_position(display_offset);

        // Assure the mouse pos is not in the scrollback.
        if pos.row < 0 {
            return;
        }

        // Calculate modifiers value via shared policy so web matches desktop.
        let mod_state = self.modifiers.state();
        let mods = mouse_report_modifier_bits(
            mod_state.shift_key(),
            mod_state.alt_key(),
            mod_state.control_key(),
        );
        let pressed = matches!(state, ElementState::Pressed);

        // Report mouse events.
        if mode.contains(Mode::SGR_MOUSE) {
            self.sgr_mouse_report(pos, button + mods, state);
        } else {
            let byte = mouse_report_legacy_button_byte(button, mods, pressed);
            self.normal_mouse_report(pos, byte);
        }
    }

    pub(crate) fn normal_mouse_report(&mut self, position: Pos, button: u8) {
        let utf8 = self.get_mode().contains(Mode::UTF8_MOUSE);
        let Some(msg) = encode_normal_mouse_report(position, button, utf8) else {
            return;
        };
        self.ctx_mut().current_mut().messenger.send_write(msg);
    }

    pub fn on_focus_change(&mut self, is_focused: bool) {
        if self.get_mode().contains(Mode::FOCUS_IN_OUT) {
            let chr = if is_focused { "I" } else { "O" };

            let msg = format!("\x1b[{chr}");
            self.ctx_mut()
                .current_mut()
                .messenger
                .send_write(msg.into_bytes());
        }
    }

    pub fn paste(&mut self, text: &str, bracketed: bool) {
        if self.search_active() {
            for c in text.chars() {
                self.search_input(c);
            }
            return;
        }
        if self.current_terminal_block_input_active() {
            self.context_manager
                .current_mut()
                .terminal_input
                .insert_paste(text);
            self.mark_dirty();
            return;
        }

        let bracketed_active = self.get_mode().contains(Mode::BRACKETED_PASTE);
        match paste_payload(text, bracketed, bracketed_active) {
            PastePayload::Bracketed { filtered } => {
                self.scroll_bottom_when_cursor_not_visible();
                self.clear_selection();
                let messenger = &mut self.ctx_mut().current_mut().messenger;
                messenger.send_write(BRACKETED_PASTE_START);
                messenger.send_write(filtered);
                messenger.send_write(BRACKETED_PASTE_END);
            }
            PastePayload::Raw(bytes) => {
                self.ctx_mut().current_mut().messenger.send_write(bytes);
            }
        }
    }

    pub fn update_ime_cursor_position_if_needed(
        &mut self,
        window: &neoism_window::window::Window,
    ) {
        // Check if IME cursor positioning is enabled in config
        if !self.context_manager.config.keyboard.ime_cursor_positioning {
            return;
        }

        let current_grid = self.context_manager.current_grid();
        let scaled_margin = current_grid.get_scaled_margin();

        let Some(current_item) = current_grid.current_item() else {
            return;
        };

        let layout = current_item.val.dimension;
        let terminal = current_item.val.terminal.lock();
        let cursor_pos = terminal.grid.cursor.pos;
        drop(terminal);

        let panel_rect = current_item.layout_rect;
        let cell_width = layout.dimension.width;
        let cell_height = layout.dimension.height;

        // Pure geometry / dimension guard lives in
        // neoism_ui::key_policy::ime_cursor_pixel_position; warn-log
        // and dedup decisions stay here so we don't have to plumb a
        // logging service into the shared crate.
        let Some(out) = ime_cursor_pixel_position(ImeCursorInput {
            panel_left_px: panel_rect[0],
            panel_top_px: panel_rect[1],
            scaled_margin_left_px: scaled_margin.left,
            scaled_margin_top_px: scaled_margin.top,
            cell_width_px: cell_width,
            cell_height_px: cell_height,
            cursor_col: cursor_pos.col.0,
            cursor_row: cursor_pos.row.0,
        }) else {
            if cell_width <= 0.0 || cell_height <= 0.0 {
                tracing::warn!(
                    "Invalid cell dimensions for IME cursor positioning: {}x{}",
                    cell_width,
                    cell_height
                );
            } else {
                tracing::warn!(
                    "Invalid IME cursor coordinates (panel=({},{}), cell=({}x{}), grid=({},{}))",
                    panel_rect[0],
                    panel_rect[1],
                    cell_width,
                    cell_height,
                    cursor_pos.col.0,
                    cursor_pos.row.0
                );
            }
            return;
        };

        if !ime_cursor_position_significantly_changed(
            self.last_ime_cursor_pos,
            out.pixel_x,
            out.pixel_y,
        ) {
            return;
        }

        self.last_ime_cursor_pos = Some((out.pixel_x, out.pixel_y));

        window.set_ime_cursor_area(
            neoism_window::dpi::PhysicalPosition::new(
                out.pixel_x as f64,
                out.pixel_y as f64,
            ),
            neoism_window::dpi::PhysicalSize::new(
                out.cell_width as f64,
                out.cell_height as f64,
            ),
        );
    }

    #[allow(dead_code)]
    pub fn hint_input(&mut self, c: char, clipboard: &mut Clipboard) {
        let terminal = self.context_manager.current().terminal.lock();
        if let Some(hint_match) = self.hint_state.keyboard_input(&*terminal, c) {
            drop(terminal);
            self.execute_hint_action(&hint_match, clipboard);
            // Stop hint mode and update state with proper damage tracking
            self.hint_state.stop();
            self.update_hint_state();
        } else {
            drop(terminal);
            self.update_hint_state();
        }
        self.mark_dirty();
    }

    pub fn start_hint_mode(
        &mut self,
        hint: std::rc::Rc<neoism_backend::config::hints::Hint>,
    ) {
        self.hint_state.start(hint);
        let terminal = self.context_manager.current().terminal.lock();
        self.hint_state.update_matches(&*terminal);
        drop(terminal);

        // Update hint state and trigger damage tracking
        self.update_hint_state();

        self.mark_dirty();
    }

    pub(crate) fn execute_hint_action(
        &mut self,
        hint_match: &crate::terminal::hints::HintMatch,
        clipboard: &mut Clipboard,
    ) {
        use neoism_backend::config::hints::{
            HintAction, HintCommand, HintInternalAction,
        };

        match &hint_match.hint.action {
            HintAction::Action { action } => match action {
                HintInternalAction::Copy => {
                    clipboard.set(ClipboardType::Clipboard, hint_match.text.clone());
                }
                HintInternalAction::Paste => {
                    self.paste(&hint_match.text, true);
                }
                HintInternalAction::Select => {
                    self.select_terminal_range(hint_match.start, hint_match.end);
                    self.mark_dirty();
                }
                HintInternalAction::MoveViModeCursor => {
                    // Move vi mode cursor to hint position
                    let mut terminal = self.context_manager.current().terminal.lock();
                    terminal.vi_mode_cursor.pos = hint_match.start;
                    drop(terminal);
                    self.mark_dirty();
                }
            },
            HintAction::Command { command } => {
                // If the match looks like a local path, resolve it against
                // the terminal's OSC 7 CWD and fall back to the raw text if
                // the path doesn't exist (or the text is a URL).
                let arg_text = {
                    let cwd = &self
                        .context_manager
                        .current()
                        .terminal
                        .lock()
                        .current_directory;
                    match crate::terminal::hints::resolve_path_for_opening(
                        &hint_match.text,
                        cwd.as_deref(),
                    ) {
                        Some(resolved) => resolved.to_string_lossy().into_owned(),
                        None => hint_match.text.clone(),
                    }
                };

                match command {
                    HintCommand::Simple(program) => {
                        self.exec(program, [&arg_text]);
                    }
                    HintCommand::WithArgs { program, args } => {
                        let mut all_args = args.clone();
                        all_args.push(arg_text);
                        self.exec(program, &all_args);
                    }
                }
            }
        }
    }

    pub fn update_hint_state(&mut self) {
        use neoism_terminal_core::damage::TerminalDamage;

        if self.hint_state.is_active() {
            // Update hint labels
            self.update_hint_labels();

            // Update hint matches in renderable content
            let matches: Vec<neoism_terminal_core::crosswords::search::Match> = self
                .hint_state
                .matches()
                .iter()
                .map(|hint_match| hint_match.start..=hint_match.end)
                .collect();
            self.context_manager
                .current_mut()
                .renderable_content
                .hint_matches = Some(matches);

            // Mark lines with hint labels as damaged
            let mut damaged_lines = std::collections::BTreeSet::new();
            {
                let current = &self.context_manager.current();
                let hint_labels = &current.renderable_content.hint_labels;
                let terminal = current.terminal.lock();
                let display_offset = terminal.display_offset();
                let screen_lines = terminal.screen_lines();
                drop(terminal);

                if !hint_labels.is_empty() {
                    // Collect all lines that have hint labels
                    for label in hint_labels {
                        let line = label.position.row.0 - display_offset as i32;
                        if line >= 0 && (line as usize) < screen_lines {
                            damaged_lines.insert(
                                neoism_terminal_core::crosswords::LineDamage::new(
                                    line as usize,
                                    true,
                                ),
                            );
                        }
                    }
                }

                // Also damage lines with hint matches
                if let Some(hint_matches) = &current.renderable_content.hint_matches {
                    for hint_match in hint_matches {
                        let start_line = hint_match.start().row.0 - display_offset as i32;
                        let end_line = hint_match.end().row.0 - display_offset as i32;

                        for line in start_line..=end_line {
                            if line >= 0 && (line as usize) < screen_lines {
                                damaged_lines.insert(
                                    neoism_terminal_core::crosswords::LineDamage::new(
                                        line as usize,
                                        true,
                                    ),
                                );
                            }
                        }
                    }
                }
            }

            let current = self.context_manager.current_mut();
            if !damaged_lines.is_empty() {
                current
                    .renderable_content
                    .pending_update
                    .set_terminal_damage(TerminalDamage::Partial(damaged_lines));
            } else {
                // Force full damage if no specific lines (for hint highlights)
                current
                    .renderable_content
                    .pending_update
                    .set_terminal_damage(TerminalDamage::Full);
            }
        } else if !self.search_active() {
            // Clear hint state only if search is not active,
            // since search also uses hint_matches for highlighting
            self.context_manager
                .current_mut()
                .renderable_content
                .hint_matches = None;
            self.context_manager
                .current_mut()
                .renderable_content
                .hint_labels
                .clear();
            // Force full damage to clear all hint highlights
            let current = self.context_manager.current_mut();
            current
                .renderable_content
                .pending_update
                .set_terminal_damage(TerminalDamage::Full);
        }
    }

    pub(crate) fn update_hint_labels(&mut self) {
        use crate::context::renderable::HintLabel;

        let match_starts: Vec<_> = self
            .hint_state
            .matches()
            .iter()
            .map(|hint_match| hint_match.start)
            .collect();
        let visible_labels = self.hint_state.visible_labels();
        let hint_labels = hint_label_placements(
            self.hint_state.is_active(),
            &match_starts,
            &visible_labels,
        )
        .into_iter()
        .map(|placement| HintLabel {
            position: placement.position,
            label: placement.label,
            is_first: placement.is_first,
        })
        .collect();

        self.context_manager
            .current_mut()
            .renderable_content
            .hint_labels = hint_labels;
    }

    pub(crate) fn hint_post_processing(
        &self,
        terminal: &neoism_terminal_core::crosswords::Crosswords,
        start_col: neoism_terminal_core::crosswords::pos::Column,
        end_col: neoism_terminal_core::crosswords::pos::Column,
        row: neoism_terminal_core::crosswords::pos::Line,
    ) -> Option<(
        neoism_terminal_core::crosswords::pos::Column,
        neoism_terminal_core::crosswords::pos::Column,
    )> {
        let grid = &terminal.grid;
        if start_col > end_col {
            return None;
        }

        let chars: Vec<char> = (start_col.0..=end_col.0)
            .map(|col| grid[row][neoism_terminal_core::crosswords::pos::Column(col)].c())
            .collect();
        post_process_hint_match_end(&chars).map(|end_offset| {
            (
                start_col,
                neoism_terminal_core::crosswords::pos::Column(start_col.0 + end_offset),
            )
        })
    }
}
