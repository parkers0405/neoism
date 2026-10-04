use super::*;

impl Screen<'_> {
    pub fn process_key_event(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        clipboard: &mut Clipboard,
    ) {
        let route_id = self.context_manager.current_route();
        let current_index = self.context_manager.current_index();
        let has_preedit = self.context_manager.current().ime.preedit().is_some();
        tracing::trace!(
            target: "neoism::input",
            route_id,
            current_index,
            has_preedit,
            file_tree_focused = self.renderer.file_tree.is_focused(),
            file_tree_visible = self.renderer.file_tree.is_visible(),
            search_active = self.search_active(),
            hint_active = self.hint_state.is_active(),
            state = ?key.state,
            repeat = key.repeat,
            logical_key = ?key.logical_key,
            physical_key = ?key.physical_key,
            location = ?key.location,
            text = ?key.text,
            text_with_all_modifiers = ?key.text_with_all_modifiers(),
            modifiers = ?self.modifiers.state(),
            "screen process_key_event entered"
        );

        if has_preedit {
            tracing::trace!(
                target: "neoism::input",
                route_id,
                "screen key event ignored: IME preedit is active"
            );
            return;
        }

        let mods = self.modifiers.state();

        if self.handle_plugin_keymap(key, mods) {
            return;
        }

        if self.handle_app_global_shortcut(key) {
            return;
        }

        // Alt+D is global but captures a deterministic terminal target.
        let alt_only = mods.alt_key()
            && !mods.control_key()
            && !mods.super_key()
            && !mods.shift_key();
        if alt_only {
            let new_terminal =
                matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyT));
            let change_directory =
                matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyD));
            if change_directory
                || (new_terminal
                    && !self.context_manager.current().has_non_terminal_surface())
            {
                if key.state == ElementState::Pressed {
                    if new_terminal {
                        self.create_focused_terminal_tab();
                    } else {
                        self.open_workspace_directory_palette();
                    }
                }
                return;
            }
        }

        if self.handle_context_menu_key(key, clipboard) {
            return;
        }

        if let Some(action) = Self::font_size_action_for_key(key, mods) {
            if key.state == ElementState::Pressed {
                self.change_font_size(action);
            }
            return;
        }

        if self.handle_buffer_tab_focus_key(key, mods) {
            return;
        }

        // Chrome focus + resize are pure predicate→action branches.
        // Build the POD, defer the decision to
        // `neoism_ui::selection_input::early_key_event_dispatch`, then
        // execute the returned action. The shared dispatcher resolves
        // the press/release short-circuit the same way for the web
        // frontend.
        {
            let mut dispatch_input =
                neoism_ui::selection_input::EarlyKeyDispatchInput::default();
            dispatch_input.is_pressed = key.state == ElementState::Pressed;
            dispatch_input.chrome_focus = Self::is_chrome_focus_key(key, mods);
            dispatch_input.chrome_resize = Self::is_chrome_resize_key(key, mods);
            match neoism_ui::selection_input::early_key_event_dispatch(dispatch_input) {
                neoism_ui::selection_input::EarlyKeyDispatchAction::PassThrough => {}
                neoism_ui::selection_input::EarlyKeyDispatchAction::ConsumeRelease => {
                    return;
                }
                neoism_ui::selection_input::EarlyKeyDispatchAction::FocusHorizontalChrome {
                    right,
                } => {
                    self.focus_horizontal_chrome(right);
                    return;
                }
                neoism_ui::selection_input::EarlyKeyDispatchAction::ResizeFocusedChromeOrSplit {
                    direction,
                } => {
                    self.resize_focused_chrome_or_split(direction);
                    return;
                }
                // Other variants cannot be reached here — only the
                // chrome predicates are populated in this dispatch wave.
                _ => unreachable!(
                    "chrome dispatch returned non-chrome action despite POD constraints"
                ),
            }
        }

        // Git diff panel owns keyboard while focused. Symmetric with
        // the file tree below; the panel's own handler returns false
        // for keys it doesn't care about so global shortcuts still
        // reach the rest of the dispatch chain.
        if self.renderer.git_diff_panel.is_focused()
            && key.state == ElementState::Pressed
            && self.handle_git_diff_panel_key(key)
        {
            self.mark_dirty();
            return;
        }

        // File-tree normally owns keyboard input, but keep plain `:`
        // available there too as the shared command palette. App-global
        // Cmd/Super shortcuts were already handled above before the tree
        // could swallow text input.
        if self.renderer.file_tree.is_focused()
            && Self::is_file_tree_command_palette_key(key, mods)
        {
            if key.state == ElementState::Pressed {
                self.open_command_palette();
            }
            return;
        }

        // File-tree owns key input while focused. Released events fall
        // through (terminal cares about up edges only when reporting
        // them, and the tree only acts on press). Hot keys: j/k move
        // selection, Enter activates, Esc returns focus to the pane.
        if self.renderer.file_tree.is_focused() && key.state == ElementState::Pressed {
            tracing::trace!(
                target: "neoism::input",
                route_id,
                logical_key = ?key.logical_key,
                "file tree handling key"
            );
            if self.handle_file_tree_key(key) {
                self.mark_dirty();
                tracing::trace!(
                    target: "neoism::input",
                    route_id,
                    "screen key event consumed by file tree"
                );
                return;
            }
            tracing::trace!(
                target: "neoism::input",
                route_id,
                "file tree did not consume key; continuing"
            );
        }

        if self.renderer.notes_sidebar.is_focused() && key.state == ElementState::Pressed
        {
            if self.handle_notes_sidebar_key(key) {
                self.mark_dirty();
                return;
            }
        }

        if self.handle_conversations_key(key) {
            self.mark_dirty();
            return;
        }

        if self.handle_neoism_agent_key(key, clipboard) {
            return;
        }

        if self.handle_extensions_key(key) {
            return;
        }

        let mode = self.get_mode();
        let search_active = self.search_active();
        let hint_active = self.hint_state.is_active();
        tracing::trace!(
            target: "neoism::input",
            route_id,
            ?mode,
            ?mods,
            search_active,
            hint_active,
            "screen key mode snapshot"
        );

        // Consume both press and release for Rust-owned buffer-tab
        // switching before the terminal byte path. The press changes
        // the active tab; the release arrives after that switch, so if
        // we let it fall through it can land in the newly-active PTY as
        // visible keyboard-protocol noise like `...t;`.
        if mods.control_key() && mods.shift_key() && !mods.alt_key() && !mods.super_key()
        {
            let key_without_mods = key.key_without_modifiers();
            let is_t = matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyT))
                || matches!(key_without_mods.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("t"));
            let is_w = matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyW))
                || matches!(key_without_mods.as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("w"));
            if is_t {
                if key.state == ElementState::Pressed {
                    self.create_workspace_terminal_tab();
                    self.cancel_search(clipboard);
                }
                return;
            }
            if is_w {
                if key.state == ElementState::Pressed {
                    self.create_tab(clipboard);
                }
                return;
            }
            if Self::is_arrow_left_key(key) {
                if key.state == ElementState::Pressed {
                    self.cancel_search(clipboard);
                    self.clear_selection();
                    self.select_active_buffer_tab(true);
                }
                return;
            }
            if Self::is_arrow_right_key(key) {
                if key.state == ElementState::Pressed {
                    self.cancel_search(clipboard);
                    self.clear_selection();
                    self.select_active_buffer_tab(false);
                }
                return;
            }
        }

        // Workspace tab + Insert + split-stack branches: build one POD
        // and let `early_key_event_dispatch` resolve the precedence so
        // this site shrinks to side-effect-only match arms. The shared
        // dispatcher mirrors the original `if` chain exactly — press
        // events run their side effect, releases short-circuit
        // (`ConsumeRelease`) once a predicate matches, and the
        // split-stack toggle is still gated on `current_grid_len() > 1`
        // via `split_stack_toggle_unlocked`.
        let workspace_dispatch_input =
            neoism_ui::selection_input::EarlyKeyDispatchInput {
                is_pressed: key.state == ElementState::Pressed,
                chrome_focus: None,
                chrome_resize: None,
                is_top_level_workspace_tab_switch:
                    Self::is_top_level_workspace_tab_switch_key(key, mods),
                is_workspace_buffer_tab_switch: Self::is_workspace_buffer_tab_switch_key(
                    key, mods,
                ),
                shift: mods.shift_key(),
                is_control_insert: Self::is_control_insert_key(key, mods),
                is_shift_insert: Self::is_shift_insert_key(key, mods),
                is_split_stack_toggle: Self::is_split_stack_toggle_key(key, mods),
                split_stack_toggle_unlocked: self.context_manager.current_grid_len() > 1,
                is_split_stack_auto_tab: Self::is_split_stack_auto_tab_key(key, mods),
            };
        match neoism_ui::selection_input::early_key_event_dispatch(
            workspace_dispatch_input,
        ) {
            neoism_ui::selection_input::EarlyKeyDispatchAction::PassThrough => {}
            neoism_ui::selection_input::EarlyKeyDispatchAction::ConsumeRelease => return,
            neoism_ui::selection_input::EarlyKeyDispatchAction::SelectTopLevelWorkspace {
                shift,
            } => {
                self.cancel_search(clipboard);
                self.clear_selection();
                self.select_top_level_workspace(shift);
                return;
            }
            neoism_ui::selection_input::EarlyKeyDispatchAction::SelectWorkspaceBufferTab {
                shift,
            } => {
                self.cancel_search(clipboard);
                self.clear_selection();
                self.select_workspace_buffer_tab(shift);
                return;
            }
            // Omarchy/Hyprland's global Super+C binding sends Ctrl+Insert
            // into the focused client. Rio's normal terminal binding for
            // Ctrl+Insert is an Insert escape sequence (`CSI 2;5~`), so
            // consume it as copy before it can leak visible `5~` text
            // into chat TUIs.
            neoism_ui::selection_input::EarlyKeyDispatchAction::ControlInsert => {
                self.copy_selection(ClipboardType::Clipboard, clipboard);
                return;
            }
            neoism_ui::selection_input::EarlyKeyDispatchAction::ShiftInsert => {
                let content = clipboard.get(ClipboardType::Clipboard);
                if let Some(code) = self.context_manager.current_mut().code.as_mut() {
                    code.buffer.insert_text(&content);
                    self.sync_active_code_modified();
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                } else if let Some(markdown) =
                    self.context_manager.current_mut().active_markdown_mut()
                {
                    markdown.enter_insert();
                    markdown.insert_text(&content);
                    self.sync_active_markdown_modified();
                    self.renderer.trail_cursor.reset();
                    self.mark_dirty();
                } else {
                    self.paste(&content, true);
                }
                return;
            }
            neoism_ui::selection_input::EarlyKeyDispatchAction::ToggleSplitStackFocus => {
                self.toggle_split_stack_focus();
                return;
            }
            neoism_ui::selection_input::EarlyKeyDispatchAction::MoveActiveTabToSplitStack => {
                self.move_active_tab_to_split_stack();
                return;
            }
            // Chrome focus/resize were handled in the earlier dispatch
            // wave above; this POD never sets those predicates.
            neoism_ui::selection_input::EarlyKeyDispatchAction::FocusHorizontalChrome { .. }
            | neoism_ui::selection_input::EarlyKeyDispatchAction::ResizeFocusedChromeOrSplit { .. } => {
                unreachable!(
                    "workspace dispatch returned a chrome action despite POD constraints"
                )
            }
        }

        if key.state == ElementState::Released {
            tracing::trace!(
                target: "neoism::input",
                route_id,
                report_event_types = mode.contains(Mode::REPORT_EVENT_TYPES),
                vi_mode = mode.contains(Mode::VI),
                search_active,
                hint_active,
                "handling key release"
            );
            // Whole-branch decision tree lifted to
            // `neoism_ui::selection_input::key_release_dispatch`. The
            // planner mirrors the original two `if` gates (suppression
            // first, then named-key reportability); the alt-mask
            // computation + `build_key_sequence` + PTY write are kept
            // here because they own the winit `KeyEvent` / messenger.
            let is_enter_tab_or_backspace = matches!(
                key.logical_key.as_ref(),
                Key::Named(NamedKey::Enter)
                    | Key::Named(NamedKey::Tab)
                    | Key::Named(NamedKey::Backspace)
            );
            let release_action = neoism_ui::selection_input::key_release_dispatch(
                neoism_ui::selection_input::KeyReleaseDispatchInput {
                    report_event_types: mode.contains(Mode::REPORT_EVENT_TYPES),
                    vi_mode: mode.contains(Mode::VI),
                    search_active,
                    hint_active,
                    is_enter_tab_or_backspace,
                    report_all_keys_as_esc: mode.contains(Mode::REPORT_ALL_KEYS_AS_ESC),
                },
            );
            match release_action {
                neoism_ui::selection_input::KeyReleaseDispatchAction::Suppress => {
                    tracing::trace!(
                        target: "neoism::input",
                        route_id,
                        "key release ignored by terminal mode/search/hint gates"
                    );
                    return;
                }
                neoism_ui::selection_input::KeyReleaseDispatchAction::DropUnreportableNamed => {
                    tracing::trace!(
                        target: "neoism::input",
                        route_id,
                        logical_key = ?key.logical_key,
                        "key release ignored: named key not reported without REPORT_ALL_KEYS_AS_ESC"
                    );
                    return;
                }
                neoism_ui::selection_input::KeyReleaseDispatchAction::EmitSequence => {}
            }

            // Mask `Alt` modifier from input when we won't send esc.
            let text = Self::text_for_key_event(key);
            let mods = if self.alt_send_esc(key, text) {
                mods
            } else {
                mods & !ModifiersState::ALT
            };
            tracing::trace!(
                target: "neoism::input",
                route_id,
                text = %text.escape_debug(),
                ?mods,
                "key release text/modifier snapshot"
            );

            let bytes = build_key_sequence(key, mods, mode);

            tracing::trace!(
                target: "neoism::input",
                route_id,
                byte_len = bytes.len(),
                bytes_hex = %Self::bytes_hex_for_log(&bytes),
                bytes_text = %Self::bytes_text_for_log(&bytes),
                "sending key release sequence to PTY"
            );
            self.ctx_mut().current_mut().messenger.send_write(bytes);

            return;
        }

        // All key bindings are disabled while a hint is being selected (like Alacritty)
        if self.hint_state.is_active() {
            tracing::trace!(
                target: "neoism::input",
                route_id,
                logical_key = ?key.logical_key,
                "hint state handling key"
            );
            // Shared classifier: Escape stops, Backspace pops, else
            // feed the event text into the label matcher one char at
            // a time. The pure decision lives in
            // `neoism_ui::selection_input::classify_hint_key`.
            let hint_logical = match key.logical_key {
                neoism_window::keyboard::Key::Named(
                    neoism_window::keyboard::NamedKey::Escape,
                ) => neoism_ui::selection_input::HintLogicalKey::Escape,
                neoism_window::keyboard::Key::Named(
                    neoism_window::keyboard::NamedKey::Backspace,
                ) => neoism_ui::selection_input::HintLogicalKey::Backspace,
                _ => neoism_ui::selection_input::HintLogicalKey::Other,
            };
            match neoism_ui::selection_input::classify_hint_key(hint_logical) {
                neoism_ui::selection_input::HintKeyAction::StopHintMode => {
                    tracing::trace!(target: "neoism::input", route_id, "hint mode stopped by Escape");
                    self.hint_state.stop();
                    self.update_hint_state();
                    self.mark_dirty();
                    return;
                }
                neoism_ui::selection_input::HintKeyAction::FeedBackspace => {
                    tracing::trace!(target: "neoism::input", route_id, "hint mode received Backspace");
                    let terminal = self.context_manager.current().terminal.lock();
                    self.hint_state.keyboard_input(&*terminal, '\x08');
                    drop(terminal);
                    self.update_hint_state();
                    self.mark_dirty();
                    return;
                }
                neoism_ui::selection_input::HintKeyAction::FeedText => {}
            }

            // Handle text input
            let text = Self::text_for_key_event(key);
            tracing::trace!(
                target: "neoism::input",
                route_id,
                text = %text.escape_debug(),
                "hint mode text input"
            );
            for character in text.chars() {
                // Acquire the terminal lock only for the duration of
                // the pure label-matcher call. The post-call branching
                // (execute vs continue) runs through the shared planner
                // `neoism_ui::selection_input::hint_keystroke_result_action`
                // *after* the lock is dropped, so the execute side
                // effects (which re-enter the context manager) can't
                // deadlock against the same Mutex.
                let hint_match = {
                    let terminal = self.context_manager.current().terminal.lock();
                    self.hint_state.keyboard_input(&*terminal, character)
                };
                match neoism_ui::selection_input::hint_keystroke_result_action(
                    hint_match.is_some(),
                ) {
                    neoism_ui::selection_input::HintKeystrokeResultAction::ExecuteAndStop => {
                        tracing::trace!(
                            target: "neoism::input",
                            route_id,
                            character = %character.escape_debug(),
                            "hint mode matched and executing action"
                        );
                        let hint_match = hint_match.expect("matched branch implies Some");
                        self.execute_hint_action(&hint_match, clipboard);
                        // Stop hint mode and update state with proper damage tracking
                        self.hint_state.stop();
                        self.update_hint_state();
                        self.mark_dirty();
                        return;
                    }
                    neoism_ui::selection_input::HintKeystrokeResultAction::Continue => {}
                }
            }
            self.update_hint_state();
            self.mark_dirty();
            return;
        }

        let early_text = Self::text_for_key_event(key);

        // Draw-over-note mode claims undo/redo (Ctrl+Z/Y, `u`), Esc (finish)
        // and Ctrl+S (save) before anything else swallows them.
        if self.draw_over_note.is_some() && self.handle_draw_over_note_key(key, mods) {
            return;
        }

        // Alt+Shift+D toggles in-place draw mode on a markdown note. Plain
        // Alt+D is app-global Change Directory and was consumed above.
        if mods.alt_key()
            && mods.shift_key()
            && !mods.control_key()
            && key.state == ElementState::Pressed
            && self.context_manager.current().markdown.is_some()
        {
            if let Key::Character(c) = &key.logical_key {
                if c.eq_ignore_ascii_case("d") {
                    self.draw_on_current_note();
                    return;
                }
            }
        }

        // `.neodraw` panes claim tool/edit keys (Delete, Esc, tool
        // shortcuts, Ctrl+Z/C/V/D) BEFORE the bindings table and the
        // terminal-block gate, which would otherwise swallow them.
        if self.context_manager.current().draw.is_some()
            && self.dispatch_draw_key(key, mods, early_text)
        {
            return;
        }

        // Block-input composer gate: when the active pane owns a
        // terminal-block command composer (Warp-style), keystrokes
        // route through it instead of the bindings table — unless the
        // search bar or vi mode is already active. The shared planner
        // `neoism_ui::selection_input::terminal_block_input_gate`
        // owns the precedence; the desktop fork keeps the actual
        // composer mutation here.
        let block_consumed = if !search_active && !mode.contains(Mode::VI) {
            self.handle_terminal_block_input_key(key, mods, early_text)
        } else {
            false
        };
        let block_gate = neoism_ui::selection_input::terminal_block_input_gate(
            neoism_ui::selection_input::TerminalBlockInputGateInput {
                search_active,
                vi_mode: mode.contains(Mode::VI),
                block_consumed,
            },
        );
        match block_gate {
            neoism_ui::selection_input::TerminalBlockInputGateAction::Consume => {
                tracing::trace!(
                    target: "neoism::input",
                    route_id,
                    "screen key event consumed by terminal block input before bindings"
                );
                return;
            }
            neoism_ui::selection_input::TerminalBlockInputGateAction::PassThrough => {}
        }

        let ignore_chars = self.process_key_bindings(key, &mode, mods, clipboard);
        tracing::trace!(
            target: "neoism::input",
            route_id,
            ignore_chars,
            "key binding processing completed"
        );
        if ignore_chars {
            tracing::trace!(
                target: "neoism::input",
                route_id,
                "screen key event consumed by key binding"
            );
            return;
        }

        let text = early_text;
        tracing::trace!(
            target: "neoism::input",
            route_id,
            text = %text.escape_debug(),
            text_len = text.len(),
            "screen text resolved for key"
        );

        // Mid-stage consumption gates: search bar → code surface →
        // markdown surface → vi-mode no-op. The shared dispatcher in
        // `neoism_ui::selection_input::mid_key_event_dispatch` mirrors
        // the original chained `if` precedence exactly; each match arm
        // below just runs the side effect the original branch ran.
        let mid_dispatch_input = neoism_ui::selection_input::MidKeyDispatchInput {
            search_active: self.search_active(),
            code_active: self.context_manager.current().code.is_some(),
            markdown_active: self.context_manager.current().markdown.is_some()
                || self.context_manager.current().notebook.is_some()
                || self.context_manager.current().epub.is_some(),
            vi_mode: mode.contains(Mode::VI),
        };
        match neoism_ui::selection_input::mid_key_event_dispatch(mid_dispatch_input) {
            neoism_ui::selection_input::MidKeyDispatchAction::PassThrough => {}
            neoism_ui::selection_input::MidKeyDispatchAction::RouteToSearch => {
                tracing::trace!(
                    target: "neoism::input",
                    route_id,
                    text = %text.escape_debug(),
                    "search input handling text"
                );
                for character in text.chars() {
                    self.search_input(character);
                }
                self.mark_dirty();
                return;
            }
            neoism_ui::selection_input::MidKeyDispatchAction::RouteToCode => {
                self.dispatch_code_key(key, mods, &text, clipboard);
                return;
            }
            neoism_ui::selection_input::MidKeyDispatchAction::RouteToMarkdown => {
                self.dispatch_markdown_key(key, mods, &text, clipboard);
                return;
            }
            // Vi mode on its own doesn't have any input — the search
            // input was handled by the `RouteToSearch` arm above.
            neoism_ui::selection_input::MidKeyDispatchAction::ConsumeViMode => {
                tracing::trace!(
                    target: "neoism::input",
                    route_id,
                    "screen key event ignored: vi mode active without search input"
                );
                return;
            }
        }

        // Mask `Alt` modifier from input when we won't send esc.
        let mods = if self.alt_send_esc(key, text) {
            mods
        } else {
            mods & !ModifiersState::ALT
        };
        tracing::trace!(
            target: "neoism::input",
            route_id,
            ?mods,
            alt_send_esc = self.alt_send_esc(key, text),
            "screen modifiers resolved for output"
        );

        let build_key_sequence = Self::should_build_sequence(key, text, mode, mods);
        tracing::trace!(
            target: "neoism::input",
            route_id,
            build_key_sequence,
            logical_key = ?key.logical_key,
            text = %text.escape_debug(),
            "screen output strategy resolved"
        );

        let bytes = if build_key_sequence {
            crate::input::kitty_keyboard::build_key_sequence(key, mods, mode)
        } else {
            // Shared raw-UTF-8 byte builder. Same shape as the original
            // inline `else` arm — kept in
            // `neoism_ui::selection_input::build_non_kitty_terminal_bytes`
            // so the web frontend produces byte-identical output.
            neoism_ui::selection_input::build_non_kitty_terminal_bytes(
                text,
                mods.alt_key(),
            )
        };
        tracing::trace!(
            target: "neoism::input",
            route_id,
            byte_len = bytes.len(),
            bytes_hex = %Self::bytes_hex_for_log(&bytes),
            bytes_text = %Self::bytes_text_for_log(&bytes),
            "screen output bytes built"
        );

        // Post-build output dispatch: empty → log no-op; else → write
        // bytes to PTY. Precedence lives in
        // `neoism_ui::selection_input::terminal_output_dispatch`.
        let output_action = neoism_ui::selection_input::terminal_output_dispatch(
            neoism_ui::selection_input::TerminalOutputDispatchInput {
                bytes_empty: bytes.is_empty(),
            },
        );
        match output_action {
            neoism_ui::selection_input::TerminalOutputDispatchAction::SkipEmpty => {
                tracing::trace!(
                    target: "neoism::input",
                    route_id,
                    logical_key = ?key.logical_key,
                    text = %text.escape_debug(),
                    "screen key event produced no output bytes"
                );
            }
            neoism_ui::selection_input::TerminalOutputDispatchAction::SendToPty => {
                self.scroll_bottom_when_cursor_not_visible();
                self.clear_selection();

                tracing::trace!(
                    target: "neoism::input",
                    route_id,
                    byte_len = bytes.len(),
                    bytes_hex = %Self::bytes_hex_for_log(&bytes),
                    bytes_text = %Self::bytes_text_for_log(&bytes),
                    "sending key bytes to PTY"
                );
                self.ctx_mut().current_mut().messenger.send_write(bytes);
            }
        }
    }

    pub fn set_mouse_hidden_by_typing(&mut self, hidden: bool) -> bool {
        if self.mouse_hidden_by_typing == hidden {
            return false;
        }
        self.mouse_hidden_by_typing = hidden;
        let cleared_hover = if hidden {
            self.context_manager
                .current_mut()
                .markdown
                .as_mut()
                .is_some_and(|markdown| markdown.clear_hover())
        } else {
            false
        };
        if hidden && cleared_hover {
            self.mark_dirty();
        }
        true
    }

    fn handle_plugin_keymap(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        mods: neoism_window::keyboard::ModifiersState,
    ) -> bool {
        let key_name = match key.key_without_modifiers().as_ref() {
            Key::Character(value) => value.to_lowercase(),
            Key::Named(named) => format!("{named:?}").to_lowercase(),
            _ => return false,
        };
        let mut chord = String::new();
        for (active, name) in [
            (mods.control_key(), "ctrl"),
            (mods.alt_key(), "alt"),
            (mods.shift_key(), "shift"),
            (mods.super_key(), "super"),
        ] {
            if active {
                if !chord.is_empty() {
                    chord.push('+');
                }
                chord.push_str(name);
            }
        }
        if !chord.is_empty() {
            chord.push('+');
        }
        chord.push_str(&key_name);

        let current = self.context_manager.current();
        let surface = if self.renderer.command_palette.is_enabled() {
            "palette"
        } else if self.renderer.finder.is_enabled() {
            "finder"
        } else if self.renderer.git_diff_panel.is_focused() {
            "git"
        } else if self.renderer.file_tree.is_focused() {
            "file-tree"
        } else if self.renderer.notes_sidebar.is_focused() {
            "notes"
        } else if current.neoism_agent.is_some() {
            "agent"
        } else if current.code.is_some() {
            "editor"
        } else if current.active_markdown().is_some() {
            "markdown"
        } else {
            "terminal"
        };
        let edit_mode = current.code.as_ref().map(|code| match code.buffer.mode {
            neoism_ui::editor::code::CodeMode::Normal => "normal",
            neoism_ui::editor::code::CodeMode::Insert => "insert",
            neoism_ui::editor::code::CodeMode::Visual => "visual",
        });

        if self
            .plugin_key_sequence_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.plugin_key_sequence.clear();
            self.plugin_key_sequence_deadline = None;
        }
        if key.state == ElementState::Released {
            return !self.plugin_key_sequence.is_empty()
                || self
                    .plugin_command_for_sequence(&chord, surface, edit_mode)
                    .is_some();
        }
        let prior = self.plugin_key_sequence.clone();
        let mut candidate = prior.clone();
        candidate.push(chord.clone());
        let candidate_text = candidate.join(" ");
        if let Some(command) =
            self.plugin_command_for_sequence(&candidate_text, surface, edit_mode)
        {
            self.plugin_key_sequence.clear();
            self.plugin_key_sequence_deadline = None;
            self.pending_plugin_commands.push(command);
            return true;
        }
        let has_prefix = self.renderer.plugins.keymaps.iter().any(|mapping| {
            plugin_mapping_matches(mapping, surface, edit_mode)
                && normalize_lua_sequence(&mapping.key)
                    .starts_with(&(candidate_text.clone() + " "))
        });
        if has_prefix {
            self.plugin_key_sequence = candidate;
            self.plugin_key_sequence_deadline =
                Some(Instant::now() + Duration::from_millis(750));
            return true;
        }
        if !prior.is_empty() {
            let prefix = prior.join(" ");
            let fallback = self
                .renderer
                .plugins
                .keymaps
                .iter()
                .filter(|mapping| plugin_mapping_matches(mapping, surface, edit_mode))
                .filter(|mapping| {
                    normalize_lua_sequence(&mapping.key)
                        .starts_with(&(prefix.clone() + " "))
                })
                .all(|mapping| mapping.fallback);
            self.plugin_key_sequence.clear();
            self.plugin_key_sequence_deadline = None;
            if !fallback {
                return true;
            }
        }
        let command = self.plugin_command_for_sequence(&chord, surface, edit_mode);
        let Some(command) = command else {
            let lazy = self.renderer.plugins.lazy_keys.iter().any(|trigger| {
                (matches!(trigger.mode.as_str(), "global" | "*")
                    || trigger.mode == surface
                    || edit_mode.is_some_and(|mode| trigger.mode == mode))
                    && normalize_lua_sequence(&trigger.key) == chord
                    && plugin_when_matches(trigger.when.as_deref(), surface, edit_mode)
            });
            if lazy && key.state == ElementState::Pressed {
                self.pending_plugin_keys.push(chord);
            }
            return lazy;
        };
        self.pending_plugin_commands.push(command);
        true
    }

    pub fn replay_plugin_key(&mut self, chord: &str) -> Option<String> {
        let current = self.context_manager.current();
        let surface = if self.renderer.command_palette.is_enabled() {
            "palette"
        } else if self.renderer.finder.is_enabled() {
            "finder"
        } else if self.renderer.git_diff_panel.is_focused() {
            "git"
        } else if self.renderer.file_tree.is_focused() {
            "file-tree"
        } else if self.renderer.notes_sidebar.is_focused() {
            "notes"
        } else if current.neoism_agent.is_some() {
            "agent"
        } else if current.code.is_some() {
            "editor"
        } else if current.active_markdown().is_some() {
            "markdown"
        } else {
            "terminal"
        };
        let edit_mode = current.code.as_ref().map(|code| match code.buffer.mode {
            neoism_ui::editor::code::CodeMode::Normal => "normal",
            neoism_ui::editor::code::CodeMode::Insert => "insert",
            neoism_ui::editor::code::CodeMode::Visual => "visual",
        });
        self.plugin_command_for_sequence(chord, surface, edit_mode)
    }

    fn plugin_command_for_sequence(
        &self,
        chord: &str,
        surface: &str,
        edit_mode: Option<&str>,
    ) -> Option<String> {
        self.renderer
            .plugins
            .keymaps
            .iter()
            .filter(|mapping| plugin_mapping_matches(mapping, surface, edit_mode))
            .filter(|mapping| normalize_lua_sequence(&mapping.key) == chord)
            .max_by_key(|mapping| (mapping.priority, mapping.order))
            .map(|mapping| mapping.command.clone())
    }
}

fn plugin_mapping_matches(
    mapping: &neoism_lua::KeymapContribution,
    surface: &str,
    edit_mode: Option<&str>,
) -> bool {
    (matches!(mapping.mode.as_str(), "global" | "*")
        || mapping.mode == surface
        || edit_mode.is_some_and(|mode| mapping.mode == mode))
        && plugin_when_matches(mapping.when.as_deref(), surface, edit_mode)
}

fn plugin_when_matches(
    when: Option<&str>,
    surface: &str,
    edit_mode: Option<&str>,
) -> bool {
    let Some(when) = when else { return true };
    when.split("&&").all(|term| {
        let term = term.trim();
        let (negated, term) = term
            .strip_prefix('!')
            .map_or((false, term), |term| (true, term.trim()));
        let matches = match term {
            "editorFocus" | "editor" => surface == "editor",
            "terminalFocus" | "terminal" => surface == "terminal",
            "agentFocus" | "agent" => surface == "agent",
            "markdownFocus" | "markdown" => surface == "markdown",
            "fileTreeFocus" | "file-tree" => surface == "file-tree",
            "notesFocus" | "notes" => surface == "notes",
            "gitFocus" | "git" => surface == "git",
            "normalMode" | "normal" => edit_mode == Some("normal"),
            "insertMode" | "insert" => edit_mode == Some("insert"),
            "visualMode" | "visual" => edit_mode == Some("visual"),
            _ => false,
        };
        matches != negated
    })
}

fn normalize_lua_chord(value: &str) -> String {
    let value = value.trim().trim_start_matches('<').trim_end_matches('>');
    let mut modifiers = Vec::new();
    let mut key = String::new();
    for part in value.split(['+', '-']) {
        let part = part.trim().to_ascii_lowercase();
        match part.as_str() {
            "c" | "ctrl" | "control" => modifiers.push("ctrl"),
            "a" | "alt" | "option" => modifiers.push("alt"),
            "s" | "shift" => modifiers.push("shift"),
            "m" | "cmd" | "meta" | "super" => modifiers.push("super"),
            other => key = other.to_owned(),
        }
    }
    ["ctrl", "alt", "shift", "super"]
        .into_iter()
        .filter(|modifier| modifiers.contains(modifier))
        .chain(std::iter::once(key.as_str()))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("+")
}

fn normalize_lua_sequence(value: &str) -> String {
    value
        .split_whitespace()
        .map(normalize_lua_chord)
        .collect::<Vec<_>>()
        .join(" ")
}
