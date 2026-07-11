use super::*;

impl Screen<'_> {
    pub fn process_mouse_bindings(
        &mut self,
        button: MouseButton,
        clipboard: &mut Clipboard,
    ) {
        let mode = self.get_mode();
        let binding_mode = BindingMode::new(&mode, self.search_active());
        let mouse_mode = self.mouse_mode();
        let mods = self.modifiers.state();

        for i in 0..self.mouse_bindings.len() {
            let mut binding = self.mouse_bindings[i].clone();

            // Shared policy: require shift for all modifiers when
            // mouse mode is active. Pure decision lives in
            // `neoism_ui::selection_input::mouse_binding_effective_modifiers`.
            let raw = neoism_ui::selection_input::ModBits::new(
                binding.mods.shift_key(),
                binding.mods.control_key(),
                binding.mods.alt_key(),
                binding.mods.super_key(),
            );
            let effective = neoism_ui::selection_input::mouse_binding_effective_modifiers(
                raw, mouse_mode,
            );
            binding.mods = ModifiersState::empty();
            if effective.shift {
                binding.mods |= ModifiersState::SHIFT;
            }
            if effective.control {
                binding.mods |= ModifiersState::CONTROL;
            }
            if effective.alt {
                binding.mods |= ModifiersState::ALT;
            }
            if effective.super_key {
                binding.mods |= ModifiersState::SUPER;
            }

            if binding.is_triggered_by(binding_mode.to_owned(), mods, &button)
                && binding.action == Act::PasteSelection
            {
                let content = clipboard.get(ClipboardType::Selection);
                self.paste(&content, true);
            }
        }
    }

    pub fn process_key_bindings(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        mode: &Mode,
        mods: ModifiersState,
        clipboard: &mut Clipboard,
    ) -> bool {
        let search_active = self.search_active();
        let binding_mode = BindingMode::new(mode, search_active);
        let mut ignore_chars = None;
        let route_id = self.context_manager.current_route();

        tracing::trace!(
            target: "neoism::input",
            route_id,
            bindings_len = self.bindings.len(),
            ?binding_mode,
            ?mode,
            ?mods,
            search_active,
            state = ?key.state,
            logical_key = ?key.logical_key,
            physical_key = ?key.physical_key,
            "key binding scan started"
        );

        if key.state == ElementState::Pressed {
            if mods.super_key()
                && !mods.control_key()
                && !mods.alt_key()
                && !mods.shift_key()
                && self.context_manager.current().editor.is_none()
            {
                let key_without_mods = key.key_without_modifiers();
                let is_copy = matches!(
                    key.physical_key,
                    PhysicalKey::Code(KeyCode::KeyC)
                ) || matches!(key_without_mods.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("c"));
                let is_paste = matches!(
                    key.physical_key,
                    PhysicalKey::Code(KeyCode::KeyV)
                ) || matches!(key_without_mods.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("v"));

                if is_copy {
                    self.copy_selection(ClipboardType::Clipboard, clipboard);
                    return true;
                }

                if is_paste {
                    if self.context_manager.current().neoism_agent.is_some() {
                        let attached = clipboard.get_image().is_some_and(|image| {
                            self.context_manager
                                .current_mut()
                                .neoism_agent
                                .as_mut()
                                .is_some_and(|agent| agent.attach_clipboard_image(image))
                        });
                        if attached {
                            self.mark_dirty();
                            return true;
                        }
                    }
                    let content = clipboard.get(ClipboardType::Clipboard);
                    if let Some(markdown) =
                        self.context_manager.current_mut().active_markdown_mut()
                    {
                        markdown.enter_insert();
                        markdown.insert_text(&content);
                        self.mark_dirty();
                    } else if let Some(agent) =
                        self.context_manager.current_mut().neoism_agent.as_mut()
                    {
                        agent.insert_paste(&content);
                        self.mark_dirty();
                    } else {
                        self.paste(&content, true);
                    }
                    return true;
                }
            }

            if !matches!(key.logical_key, Key::Named(NamedKey::Tab)) {
                // Pull the printable digit regardless of modifier
                // state; the shared `workspace_index_for_alt_digit`
                // enforces the "Alt only" gating + 0->9 / 1..9->0..8
                // index policy.
                let physical_digit = match &key.physical_key {
                    PhysicalKey::Code(KeyCode::Digit1)
                    | PhysicalKey::Code(KeyCode::Numpad1) => Some('1'),
                    PhysicalKey::Code(KeyCode::Digit2)
                    | PhysicalKey::Code(KeyCode::Numpad2) => Some('2'),
                    PhysicalKey::Code(KeyCode::Digit3)
                    | PhysicalKey::Code(KeyCode::Numpad3) => Some('3'),
                    PhysicalKey::Code(KeyCode::Digit4)
                    | PhysicalKey::Code(KeyCode::Numpad4) => Some('4'),
                    PhysicalKey::Code(KeyCode::Digit5)
                    | PhysicalKey::Code(KeyCode::Numpad5) => Some('5'),
                    PhysicalKey::Code(KeyCode::Digit6)
                    | PhysicalKey::Code(KeyCode::Numpad6) => Some('6'),
                    PhysicalKey::Code(KeyCode::Digit7)
                    | PhysicalKey::Code(KeyCode::Numpad7) => Some('7'),
                    PhysicalKey::Code(KeyCode::Digit8)
                    | PhysicalKey::Code(KeyCode::Numpad8) => Some('8'),
                    PhysicalKey::Code(KeyCode::Digit9)
                    | PhysicalKey::Code(KeyCode::Numpad9) => Some('9'),
                    PhysicalKey::Code(KeyCode::Digit0)
                    | PhysicalKey::Code(KeyCode::Numpad0) => Some('0'),
                    _ => match key.key_without_modifiers().as_ref() {
                        Key::Character(ch) => {
                            ch.chars().next().filter(|c| c.is_ascii_digit())
                        }
                        _ => None,
                    },
                };
                if let Some(digit) = physical_digit {
                    if let Some(index) = workspace_index_for_alt_digit(
                        digit,
                        mods.shift_key(),
                        mods.control_key(),
                        mods.alt_key(),
                        mods.super_key(),
                    ) {
                        self.select_top_level_workspace_at(index);
                        return true;
                    }
                }
            }

            let is_tab_key = matches!(key.logical_key, Key::Named(NamedKey::Tab))
                || matches!(key.physical_key, PhysicalKey::Code(KeyCode::Tab))
                || key.text.as_deref() == Some("\t");

            if mods.alt_key() && !mods.control_key() && !mods.super_key() && is_tab_key {
                if self.select_workspace_buffer_tab(mods.shift_key()) {
                    return true;
                }
            }

            if mods.control_key() && !mods.alt_key() && !mods.super_key() && is_tab_key {
                if self.select_active_buffer_tab(mods.shift_key()) {
                    return true;
                }
            }

            let key_without_mods = key.key_without_modifiers();
            let is_t = matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyT))
                || matches!(key_without_mods.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("t"));
            let is_w = matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyW))
                || matches!(key_without_mods.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("w"));
            if mods.control_key() && mods.shift_key() && is_t {
                self.create_workspace_terminal_tab();
                self.cancel_search(clipboard);
                return true;
            }
            if mods.control_key() && mods.shift_key() && is_w {
                self.create_tab(clipboard);
                return true;
            }
        }

        for i in 0..self.bindings.len() {
            let binding = &self.bindings[i];
            let trigger = &binding.trigger;
            let action = binding.action.clone();

            // We don't want the key without modifier, because it means something else most of
            // the time. However what we want is to manually lowercase the character to account
            // for both small and capital letters on regular characters at the same time.
            let logical_key = if let Key::Character(ch) = key.logical_key.as_ref() {
                // Match `Alt` bindings without `Alt` being applied, otherwise they use the
                // composed chars, which are not intuitive to bind.
                //
                // On Windows, the `Ctrl + Alt` mangles `logical_key` to unidentified values, thus
                // preventing them from being used in bindings
                //
                // For more see https://github.com/rust-windowing/winit/issues/2945.
                // if (cfg!(target_os = "macos") || (cfg!(windows) && mods.control_key()))
                // && mods.alt_key()
                if (mods.shift_key() || mods.alt_key())
                    || mods.alt_key() && (cfg!(windows) && mods.control_key())
                {
                    key.key_without_modifiers()
                } else {
                    Key::Character(ch.to_lowercase().into())
                }
            } else {
                key.logical_key.clone()
            };

            let key_match = match (&trigger, logical_key) {
                (BindingKey::Scancode(_), _) => BindingKey::Scancode(key.physical_key),
                (_, code) => BindingKey::Keycode {
                    key: code,
                    location: key.location,
                },
            };
            let physical_key_match = Self::physical_key_binding_match(key, mods);

            if binding.is_triggered_by(binding_mode.to_owned(), mods, &key_match)
                || physical_key_match.as_ref().is_some_and(|key_match| {
                    binding.is_triggered_by(binding_mode.to_owned(), mods, key_match)
                })
            {
                // Editor (nvim) panes have no PTY, so default
                // terminal-escape bindings (`Act::Esc("\x1b[A")` for
                // arrows, `\x7f` for Backspace, etc.) would consume
                // the key without producing useful output. Skip them
                // here so the editor early-dispatch in
                // `process_key_event` sees the raw KeyEvent and turns
                // it into nvim notation. Other binding actions (font
                // size, copy/paste, splits) still fire normally.
                if matches!(action, Act::Esc(_))
                    && (self.context_manager.current().editor.is_some()
                        || self.context_manager.current().markdown.is_some()
                        || self.context_manager.current().notebook.is_some())
                {
                    tracing::trace!(
                        target: "neoism::input",
                        route_id,
                        "skipping Act::Esc binding for editor pane"
                    );
                    continue;
                }
                *ignore_chars.get_or_insert(true) &= action != Act::ReceiveChar;
                tracing::trace!(
                    target: "neoism::input",
                    route_id,
                    binding_index = i,
                    trigger = ?trigger,
                    key_match = ?key_match,
                    action = ?&action,
                    binding_mods = ?binding.mods,
                    event_mods = ?mods,
                    binding_mode = ?binding.mode,
                    binding_notmode = ?binding.notmode,
                    active_mode = ?binding_mode,
                    ignore_chars = ?ignore_chars,
                    "key binding matched"
                );

                match &action {
                    Act::Run(program) => self.exec(program.program(), program.args()),
                    Act::Esc(s) => {
                        self.paste(s, false);
                    }
                    Act::Paste => {
                        let mut attached_image = false;
                        if self.context_manager.current().neoism_agent.is_some() {
                            attached_image = clipboard.get_image().is_some_and(|image| {
                                self.context_manager
                                    .current_mut()
                                    .neoism_agent
                                    .as_mut()
                                    .is_some_and(|agent| {
                                        agent.attach_clipboard_image(image)
                                    })
                            });
                        }
                        if attached_image {
                            self.mark_dirty();
                            return true;
                        }
                        let content = clipboard.get(ClipboardType::Clipboard);
                        if self.context_manager.current().editor.is_some() {
                            self.send_editor_command(
                                neoism_backend::performer::nvim::vim_paste_command(
                                    &content,
                                ),
                            );
                        } else if let Some(markdown) =
                            self.context_manager.current_mut().active_markdown_mut()
                        {
                            markdown.enter_insert();
                            markdown.insert_text(&content);
                            self.mark_dirty();
                        } else if let Some(agent) =
                            self.context_manager.current_mut().neoism_agent.as_mut()
                        {
                            agent.insert_paste(&content);
                            self.mark_dirty();
                        } else {
                            self.paste(&content, true);
                        }
                    }
                    Act::ClearSelection => {
                        self.clear_selection();
                    }
                    Act::PasteSelection => {
                        let content = clipboard.get(ClipboardType::Selection);
                        if let Some(markdown) =
                            self.context_manager.current_mut().active_markdown_mut()
                        {
                            markdown.enter_insert();
                            markdown.insert_text(&content);
                            self.mark_dirty();
                        } else if let Some(agent) =
                            self.context_manager.current_mut().neoism_agent.as_mut()
                        {
                            agent.insert_paste(&content);
                            self.mark_dirty();
                        } else {
                            self.paste(&content, true);
                        }
                    }
                    Act::Copy => {
                        if self.context_manager.current().editor.is_some() {
                            self.send_editor_command(
                                neoism_backend::performer::nvim::vim_copy_active_command(
                                ),
                            );
                        } else {
                            self.copy_selection(ClipboardType::Clipboard, clipboard);
                        }
                    }
                    Act::Hint(hint_config) => {
                        self.start_hint_mode(hint_config.clone());
                    }
                    Act::SearchForward => {
                        self.start_search(Direction::Right);
                        self.resize_top_or_bottom_line(self.ctx().len());
                        self.mark_dirty();
                    }
                    Act::SearchBackward => {
                        self.start_search(Direction::Left);
                        self.resize_top_or_bottom_line(self.ctx().len());
                        self.mark_dirty();
                    }
                    Act::Search(SearchAction::SearchConfirm) => {
                        self.confirm_search(clipboard);
                        self.resize_top_or_bottom_line(self.ctx().len());
                        self.mark_dirty();
                    }
                    Act::Search(SearchAction::SearchCancel) => {
                        self.cancel_search(clipboard);
                        self.resize_top_or_bottom_line(self.ctx().len());
                        self.mark_dirty();
                    }
                    Act::Search(SearchAction::SearchClear) => {
                        let direction = self.search_state.direction;
                        self.cancel_search(clipboard);
                        self.start_search(direction);
                        self.resize_top_or_bottom_line(self.ctx().len());
                        self.mark_dirty();
                    }
                    Act::Search(SearchAction::SearchFocusNext) => {
                        self.advance_search_origin(self.search_state.direction);
                        self.resize_top_or_bottom_line(self.ctx().len());
                        self.mark_dirty();
                    }
                    Act::Search(SearchAction::SearchFocusPrevious) => {
                        let direction = self.search_state.direction.opposite();
                        self.advance_search_origin(direction);
                        self.resize_top_or_bottom_line(self.ctx().len());
                        self.mark_dirty();
                    }
                    Act::Search(SearchAction::SearchDeleteWord) => {
                        self.search_pop_word();
                        self.mark_dirty();
                    }
                    Act::Search(SearchAction::SearchHistoryPrevious) => {
                        self.search_history_previous();
                        self.mark_dirty();
                    }
                    Act::Search(SearchAction::SearchHistoryNext) => {
                        self.search_history_next();
                        self.mark_dirty();
                    }
                    Act::ToggleViMode => {
                        let context = self.context_manager.current_mut();
                        let mut terminal = context.terminal.lock();
                        terminal.toggle_vi_mode();
                        let has_vi_mode_enabled = terminal.mode().contains(Mode::VI);
                        drop(terminal);
                        context
                            .renderable_content
                            .pending_update
                            .set_terminal_damage(
                                neoism_terminal_core::damage::TerminalDamage::Full,
                            );
                        self.renderer.set_vi_mode(has_vi_mode_enabled);
                        self.mark_dirty();
                    }
                    Act::ViMotion(motion) => {
                        let context = self.context_manager.current_mut();
                        let mut terminal = context.terminal.lock();
                        if terminal.mode().contains(Mode::VI) {
                            terminal.vi_motion(*motion);
                        }

                        if let Some(selection) = &terminal.selection {
                            context.renderable_content.selection_range =
                                selection.to_range(&terminal);
                        };
                        drop(terminal);
                        context
                            .renderable_content
                            .pending_update
                            .set_terminal_damage(
                                neoism_terminal_core::damage::TerminalDamage::Full,
                            );
                        self.mark_dirty();
                    }
                    Act::Vi(ViAction::CenterAroundViCursor) => {
                        let context = self.context_manager.current_mut();
                        let mut terminal = context.terminal.lock();
                        let display_offset = terminal.display_offset() as i32;
                        let target =
                            -display_offset + terminal.grid.screen_lines() as i32 / 2 - 1;
                        let line = terminal.vi_mode_cursor.pos.row;
                        let scroll_lines = target - line.0;

                        terminal.scroll_display(Scroll::Delta(scroll_lines));
                        drop(terminal);
                        context
                            .renderable_content
                            .pending_update
                            .set_terminal_damage(
                                neoism_terminal_core::damage::TerminalDamage::Full,
                            );
                        self.mark_dirty();
                    }
                    Act::Vi(ViAction::ToggleNormalSelection) => {
                        self.toggle_selection(
                            SelectionType::Simple,
                            Side::Left,
                            clipboard,
                        );
                        self.context_manager
                            .current_mut()
                            .renderable_content
                            .pending_update
                            .set_terminal_damage(
                                neoism_terminal_core::damage::TerminalDamage::Full,
                            );
                        self.mark_dirty();
                    }
                    Act::Vi(ViAction::ToggleLineSelection) => {
                        self.toggle_selection(
                            SelectionType::Lines,
                            Side::Left,
                            clipboard,
                        );
                        self.context_manager
                            .current_mut()
                            .renderable_content
                            .pending_update
                            .set_terminal_damage(
                                neoism_terminal_core::damage::TerminalDamage::Full,
                            );
                        self.mark_dirty();
                    }
                    Act::Vi(ViAction::ToggleBlockSelection) => {
                        self.toggle_selection(
                            SelectionType::Block,
                            Side::Left,
                            clipboard,
                        );
                        self.context_manager
                            .current_mut()
                            .renderable_content
                            .pending_update
                            .set_terminal_damage(
                                neoism_terminal_core::damage::TerminalDamage::Full,
                            );
                        self.mark_dirty();
                    }
                    Act::Vi(ViAction::ToggleSemanticSelection) => {
                        self.toggle_selection(
                            SelectionType::Semantic,
                            Side::Left,
                            clipboard,
                        );
                        self.context_manager
                            .current_mut()
                            .renderable_content
                            .pending_update
                            .set_terminal_damage(
                                neoism_terminal_core::damage::TerminalDamage::Full,
                            );
                        self.mark_dirty();
                    }
                    Act::SplitRight => {
                        self.split_right();
                    }
                    Act::SplitDown => {
                        self.split_down();
                    }
                    Act::MoveDividerUp => {
                        // User wants divider to move up visually, which means expanding the bottom split
                        self.move_divider_down();
                    }
                    Act::MoveDividerDown => {
                        // User wants divider to move down visually, which means expanding the top split
                        self.move_divider_up();
                    }
                    Act::MoveDividerLeft => {
                        self.move_divider_left();
                    }
                    Act::MoveDividerRight => {
                        self.move_divider_right();
                    }
                    Act::ConfigEditor => {
                        self.context_manager.switch_to_settings();
                    }
                    Act::WindowCreateNew => {
                        self.context_manager.create_new_window();
                    }
                    Act::CloseCurrentSplitOrTab => {
                        self.close_split_or_tab(clipboard);
                    }
                    Act::TabCreateNew => {
                        self.create_tab(clipboard);
                    }
                    Act::WorkspaceTerminalTabCreateNew => {
                        self.create_workspace_terminal_tab();
                        self.cancel_search(clipboard);
                    }
                    Act::TabCloseCurrent => {
                        self.close_tab(clipboard);
                    }
                    Act::TabCloseUnfocused => {
                        self.clear_selection();
                        self.cancel_search(clipboard);
                        if self.ctx().len() <= 1 {
                            return true;
                        }
                        self.context_manager.close_unfocused_tabs();
                        self.resize_top_or_bottom_line(1);
                        self.mark_dirty();
                    }
                    Act::Quit => {
                        tracing::info!(
                            target: "neoism::editor_tabs",
                            current_is_editor = self.context_manager.current().editor.is_some(),
                            active_tab_is_terminal = self.renderer.buffer_tabs.active_is_terminal(),
                            workspace_id = ?self.current_workspace_id(),
                            "Quit binding invoked"
                        );
                        self.context_manager.quit();
                    }
                    Act::IncreaseFontSize => {
                        self.change_font_size(FontSizeAction::Increase);
                    }
                    Act::DecreaseFontSize => {
                        self.change_font_size(FontSizeAction::Decrease);
                    }
                    Act::ResetFontSize => {
                        self.change_font_size(FontSizeAction::Reset);
                    }
                    Act::ScrollPageUp => {
                        if !self.scroll_markdown_page(-1.0, 0.86) {
                            // Move vi mode cursor.
                            let current = self.context_manager.current_mut();
                            let rtid = current.rich_text_id;
                            let mut terminal = current.terminal.lock();
                            let scroll_lines = terminal.grid.screen_lines() as i32;
                            terminal.vi_mode_cursor =
                                terminal.vi_mode_cursor.scroll(&terminal, scroll_lines);
                            terminal.scroll_display(Scroll::PageUp);
                            drop(terminal);
                            self.renderer.scrollbar.notify_scroll(rtid);
                            self.mark_dirty();
                        }
                    }
                    Act::ScrollPageDown => {
                        if !self.scroll_markdown_page(1.0, 0.86) {
                            // Move vi mode cursor.
                            let current = self.context_manager.current_mut();
                            let rtid = current.rich_text_id;
                            let mut terminal = current.terminal.lock();
                            let scroll_lines = -(terminal.grid.screen_lines() as i32);

                            terminal.vi_mode_cursor =
                                terminal.vi_mode_cursor.scroll(&terminal, scroll_lines);

                            terminal.scroll_display(Scroll::PageDown);
                            drop(terminal);
                            self.renderer.scrollbar.notify_scroll(rtid);
                            self.mark_dirty();
                        }
                    }
                    Act::ScrollHalfPageUp => {
                        if !self.scroll_markdown_page(-1.0, 0.5) {
                            // Move vi mode cursor.
                            let current = self.context_manager.current_mut();
                            let rtid = current.rich_text_id;
                            let mut terminal = current.terminal.lock();
                            let scroll_lines = terminal.grid.screen_lines() as i32 / 2;

                            terminal.vi_mode_cursor =
                                terminal.vi_mode_cursor.scroll(&terminal, scroll_lines);

                            terminal.scroll_display(Scroll::Delta(scroll_lines));
                            drop(terminal);
                            self.renderer.scrollbar.notify_scroll(rtid);
                            self.mark_dirty();
                        }
                    }
                    Act::ScrollHalfPageDown => {
                        if !self.scroll_markdown_page(1.0, 0.5) {
                            // Move vi mode cursor.
                            let current = self.context_manager.current_mut();
                            let rtid = current.rich_text_id;
                            let mut terminal = current.terminal.lock();
                            let scroll_lines = -(terminal.grid.screen_lines() as i32 / 2);

                            terminal.vi_mode_cursor =
                                terminal.vi_mode_cursor.scroll(&terminal, scroll_lines);

                            terminal.scroll_display(Scroll::Delta(scroll_lines));
                            drop(terminal);
                            self.renderer.scrollbar.notify_scroll(rtid);
                            self.mark_dirty();
                        }
                    }
                    Act::ScrollToTop => {
                        if !self.scroll_markdown_to_top() {
                            let current = self.context_manager.current_mut();
                            let rtid = current.rich_text_id;
                            let mut terminal = current.terminal.lock();
                            terminal.scroll_display(Scroll::Top);

                            let topmost_line = terminal.grid.topmost_line();
                            terminal.vi_mode_cursor.pos.row = topmost_line;
                            terminal.vi_motion(ViMotion::FirstOccupied);
                            drop(terminal);
                            self.renderer.scrollbar.notify_scroll(rtid);
                            self.mark_dirty();
                        }
                    }
                    Act::ScrollToBottom => {
                        if !self.scroll_markdown_to_bottom() {
                            let current = self.context_manager.current_mut();
                            let rtid = current.rich_text_id;
                            let mut terminal = current.terminal.lock();
                            terminal.scroll_display(Scroll::Bottom);

                            // Move vi mode cursor.
                            terminal.vi_mode_cursor.pos.row =
                                terminal.grid.bottommost_line();

                            // Move to beginning twice, to always jump across linewraps.
                            terminal.vi_motion(ViMotion::FirstOccupied);
                            terminal.vi_motion(ViMotion::FirstOccupied);
                            drop(terminal);
                            self.renderer.scrollbar.notify_scroll(rtid);
                            self.mark_dirty();
                        }
                    }
                    Act::Scroll(delta) => {
                        if !self.scroll_markdown_by(-(*delta as f32) * 42.0) {
                            let current = self.context_manager.current_mut();
                            let rtid = current.rich_text_id;
                            let mut terminal = current.terminal.lock();
                            terminal.scroll_display(Scroll::Delta(*delta));
                            drop(terminal);
                            self.renderer.scrollbar.notify_scroll(rtid);
                            self.mark_dirty();
                        }
                    }
                    Act::ClearHistory => {
                        let mut terminal =
                            self.context_manager.current_mut().terminal.lock();
                        terminal.clear_saved_history();
                        drop(terminal);
                        self.mark_dirty();
                    }
                    Act::ToggleFullscreen => self.context_manager.toggle_full_screen(),
                    Act::ToggleAppearanceTheme => {
                        self.context_manager.toggle_appearance_theme();
                    }
                    Act::OpenCommandPalette => {
                        // One-way "open": the action never closes an
                        // already-visible palette. Users close it via
                        // Esc (handled inside the palette's own key
                        // dispatcher in `router::mod`). Idempotent —
                        // re-firing while the palette is already open
                        // must NOT wipe the user's in-progress query.
                        if !self.renderer.command_palette.is_enabled() {
                            self.open_command_palette();
                        }
                    }
                    Act::ToggleFileTree => {
                        self.toggle_file_tree();
                    }
                    Act::OpenNeoismNotes => {
                        self.open_neoism_notes_sidebar();
                    }
                    Act::ToggleGitDiffPanel => {
                        self.toggle_git_diff_panel();
                    }
                    Act::Minimize => {
                        self.context_manager.minimize();
                    }
                    Act::Hide => {
                        self.context_manager.hide();
                    }
                    #[cfg(target_os = "macos")]
                    Act::HideOtherApplications => {
                        self.context_manager.hide_other_apps();
                    }
                    Act::SelectNextSplit => {
                        self.cancel_search(clipboard);
                        self.context_manager.select_next_split();
                        self.mark_dirty();
                    }
                    Act::SelectPrevSplit => {
                        self.cancel_search(clipboard);
                        self.context_manager.select_prev_split();
                        self.mark_dirty();
                    }
                    Act::SelectNextSplitOrTab => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        self.save_current_workspace_chrome();
                        let old_index = self.context_manager.current_index();
                        self.context_manager.switch_to_next_split_or_tab();
                        let new_index = self.context_manager.current_index();
                        self.context_manager.switch_context_visibility(
                            &mut self.sugarloaf,
                            old_index,
                            new_index,
                        );
                        if old_index != new_index {
                            self.load_current_workspace_chrome();
                            self.reapply_chrome_layout();
                        }
                        self.mark_dirty();
                    }
                    Act::SelectPrevSplitOrTab => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        self.save_current_workspace_chrome();
                        let old_index = self.context_manager.current_index();
                        self.context_manager.switch_to_prev_split_or_tab();
                        let new_index = self.context_manager.current_index();
                        self.context_manager.switch_context_visibility(
                            &mut self.sugarloaf,
                            old_index,
                            new_index,
                        );
                        if old_index != new_index {
                            self.load_current_workspace_chrome();
                            self.reapply_chrome_layout();
                        }
                        self.mark_dirty();
                    }
                    Act::SelectTab(tab_index) => {
                        self.save_current_workspace_chrome();
                        let old_index = self.context_manager.current_index();
                        self.context_manager.select_tab(*tab_index);
                        let new_index = self.context_manager.current_index();
                        self.context_manager.switch_context_visibility(
                            &mut self.sugarloaf,
                            old_index,
                            new_index,
                        );
                        if old_index != new_index {
                            self.load_current_workspace_chrome();
                            self.reapply_chrome_layout();
                        }
                        self.cancel_search(clipboard);
                        self.mark_dirty();
                    }
                    Act::SelectLastTab => {
                        self.cancel_search(clipboard);
                        self.save_current_workspace_chrome();
                        let old_index = self.context_manager.current_index();
                        self.context_manager.select_last_tab();
                        let new_index = self.context_manager.current_index();
                        self.context_manager.switch_context_visibility(
                            &mut self.sugarloaf,
                            old_index,
                            new_index,
                        );
                        if old_index != new_index {
                            self.load_current_workspace_chrome();
                            self.reapply_chrome_layout();
                        }
                        self.mark_dirty();
                    }
                    Act::SelectNextTab => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        self.select_top_level_workspace(false);
                        return true;
                    }
                    Act::MoveActiveBufferTabToPrev => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        self.move_active_buffer_tab(true);
                        return true;
                    }
                    Act::MoveActiveBufferTabToNext => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        self.move_active_buffer_tab(false);
                        return true;
                    }
                    Act::MoveCurrentTabToPrev => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        let old_index = self.context_manager.current_index();
                        self.context_manager.move_current_to_prev();
                        let new_index = self.context_manager.current_index();
                        self.context_manager.switch_context_visibility(
                            &mut self.sugarloaf,
                            old_index,
                            new_index,
                        );
                        self.mark_dirty();
                    }
                    Act::MoveCurrentTabToNext => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        let old_index = self.context_manager.current_index();
                        self.context_manager.move_current_to_next();
                        let new_index = self.context_manager.current_index();
                        self.context_manager.switch_context_visibility(
                            &mut self.sugarloaf,
                            old_index,
                            new_index,
                        );
                        self.mark_dirty();
                    }
                    Act::SelectPrevTab => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        self.select_top_level_workspace(true);
                        return true;
                    }
                    Act::SelectNextBufferTab => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        self.select_active_buffer_tab(false);
                        return true;
                    }
                    Act::SelectPrevBufferTab => {
                        self.cancel_search(clipboard);
                        self.clear_selection();
                        self.select_active_buffer_tab(true);
                        return true;
                    }
                    Act::ReceiveChar | Act::None => (),
                    _ => (),
                }
            }
        }

        tracing::trace!(
            target: "neoism::input",
            route_id,
            ignore_chars = ?ignore_chars,
            "key binding scan finished"
        );
        ignore_chars.unwrap_or(false)
    }
}
