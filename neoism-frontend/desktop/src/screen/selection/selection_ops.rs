use super::*;

impl Screen<'_> {
    pub fn copy_selection(&mut self, ty: ClipboardType, clipboard: &mut Clipboard) {
        let text = {
            let current = self.context_manager.current();
            let terminal = current.terminal.lock();
            selected_text(&terminal, current.renderable_content.selection_range)
        };

        if let Some(text) = text {
            clipboard.set(ty, text);
        }
    }

    pub(crate) fn select_terminal_range(&mut self, start: Pos, end: Pos) {
        let current = self.context_manager.current_mut();
        let mut terminal = current.terminal.lock();
        let (selection, selection_range) = selection_with_range(
            &terminal,
            SelectionType::Simple,
            SelectionEndpoint::new(start, Side::Left),
            Some(SelectionEndpoint::new(end, Side::Right)),
        );
        terminal.selection = Some(selection);
        drop(terminal);

        current.set_selection(selection_range);
    }

    pub fn clear_selection(&mut self) {
        // Clear the selection on the terminal.
        let mut terminal = self.context_manager.current_mut().terminal.lock();
        terminal.selection.take();
        drop(terminal);
        self.context_manager.current_mut().set_selection(None);
    }

    pub(crate) fn start_selection(
        &mut self,
        ty: SelectionType,
        point: Pos,
        side: Side,
        clipboard: &mut Clipboard,
    ) {
        self.copy_selection(ClipboardType::Selection, clipboard);
        let current = self.context_manager.current_mut();
        let mut terminal = current.terminal.lock();
        let (selection, selection_range) = selection_with_range(
            &terminal,
            ty,
            SelectionEndpoint::new(point, side),
            None,
        );
        terminal.selection = Some(selection);
        drop(terminal);

        // Use set_selection to trigger render
        current.set_selection(selection_range);

        // Request render to ensure it shows immediately
        self.context_manager.request_render();
    }

    pub(crate) fn toggle_selection(
        &mut self,
        ty: SelectionType,
        side: Side,
        clipboard: &mut Clipboard,
    ) {
        let mut terminal = self.context_manager.current().terminal.lock();
        let toggle_action = toggle_selection_action(
            terminal
                .selection
                .as_ref()
                .map(|selection| SelectionSnapshot {
                    ty: selection.ty,
                    is_empty: selection.is_empty(),
                }),
            ty,
        );
        match toggle_action {
            ToggleSelectionAction::Clear => {
                drop(terminal);
                self.clear_selection();
            }
            ToggleSelectionAction::RetypeExisting => {
                if let Some(selection) = &mut terminal.selection {
                    selection.ty = ty;
                }
                drop(terminal);
                self.copy_selection(ClipboardType::Selection, clipboard);
            }
            ToggleSelectionAction::StartAtCursor => {
                let pos = terminal.vi_mode_cursor.pos;
                drop(terminal);
                self.start_selection(ty, pos, side, clipboard)
            }
        }

        if !toggle_action_needs_include_all(toggle_action) {
            return;
        }

        let current = self.context_manager.current_mut();
        let mut terminal = current.terminal.lock();
        current.renderable_content.selection_range =
            include_all_current_selection(&mut terminal);
        drop(terminal);
    }

    pub fn update_selection(&mut self, pos: Pos, side: Side) {
        let is_search_active = self.search_active();
        let current = self.context_manager.current_mut();
        let Some(mut terminal) = current.terminal.try_lock_unfair() else {
            current.renderable_content.pending_update.set_dirty();
            return;
        };
        let vi_mode = terminal.mode().contains(Mode::VI);
        let selection_range = match apply_selection_update(
            &mut terminal,
            pos,
            side,
            vi_mode,
            is_search_active,
        ) {
            Some(selection_range) => selection_range,
            None => return,
        };
        drop(terminal);

        // Use set_selection to trigger render
        current.set_selection(Some(selection_range));

        // Request render to ensure it shows immediately
        self.context_manager.request_render();
    }

    pub fn update_highlighted_hints(&mut self) -> bool {
        use neoism_ui::selection_input::{
            hint_highlight_transition, HintHighlightInput, HintHighlightTransition,
        };

        let should_highlight = hint_highlight_eligible(
            self.hint_mouse_activations(),
            self.hint_modifier_state(),
        );

        let had_highlight = self
            .context_manager
            .current()
            .renderable_content
            .highlighted_hint
            .is_some();

        // Branch 1: `!should_highlight`. Decide via shared planner.
        if !should_highlight {
            let plan = hint_highlight_transition(HintHighlightInput {
                should_highlight: false,
                had_highlight,
                mouse_in_grid: false,
                hint_match_found: false,
            });
            debug_assert!(matches!(
                plan,
                HintHighlightTransition::NoChange
                    | HintHighlightTransition::ClearHighlight { .. }
            ));

            let current = self.context_manager.current_mut();

            // Clear any previous hint damage
            if current.renderable_content.highlighted_hint.is_some() {
                let Some(mut terminal) = current.terminal.try_lock_unfair() else {
                    current.renderable_content.pending_update.set_dirty();
                    return false;
                };
                let display_offset = terminal.display_offset();
                terminal.update_selection_damage(None, display_offset);
            }

            current.renderable_content.highlighted_hint = None;
            return had_highlight;
        }

        // Snapshot the display offset under a SHORT-LIVED lock,
        // then drop the guard before calling
        // `terminal_body_mouse_position` — that helper takes its
        // own terminal lock internally (via
        // `terminal_block_source_row_at_visual_row` →
        // `current.terminal.lock()`), so holding our guard
        // across the call would re-enter the same Mutex on the
        // same thread → freeze on `futex_do_wait`. Same
        // re-entrant-Mutex bug pattern called out in
        // `feedback_no_reentrant_mutex.md` — caught live in a
        // gdb backtrace where this exact chain (update_high...
        // → terminal_body_mouse_position →
        // terminal_block_source_row_at_visual_row →
        // terminal.lock()) was the deadlock site.
        let display_offset = {
            let Some(terminal) =
                self.context_manager.current().terminal.try_lock_unfair()
            else {
                self.context_manager
                    .current_mut()
                    .renderable_content
                    .pending_update
                    .set_dirty();
                return false;
            };
            terminal.display_offset()
        };
        let Some(mouse_point) = self.terminal_body_mouse_position(display_offset) else {
            let current = self.context_manager.current_mut();
            if current.renderable_content.highlighted_hint.is_some() {
                let Some(mut terminal) = current.terminal.try_lock_unfair() else {
                    current.renderable_content.pending_update.set_dirty();
                    return false;
                };
                let display_offset = terminal.display_offset();
                terminal.update_selection_damage(None, display_offset);
                drop(terminal);
                current
                    .renderable_content
                    .pending_update
                    .set_terminal_damage(
                        neoism_terminal_core::damage::TerminalDamage::Full,
                    );
            }
            current.renderable_content.highlighted_hint = None;
            return had_highlight;
        };

        // Re-take the lock for the hint search — by now
        // `terminal_body_mouse_position` has finished its inner
        // lock, so this acquire is safe.
        let Some(terminal) = self.context_manager.current().terminal.try_lock_unfair()
        else {
            self.context_manager
                .current_mut()
                .renderable_content
                .pending_update
                .set_dirty();
            return false;
        };
        let highlighted_hint =
            self.find_hint_at_point(&terminal, mouse_point, self.modifiers.state());
        drop(terminal);

        // Shared planner decides which terminal-side damage branch
        // to take. The lock acquisitions are still desktop-owned.
        let plan = hint_highlight_transition(HintHighlightInput {
            should_highlight: true,
            had_highlight,
            mouse_in_grid: true,
            hint_match_found: highlighted_hint.is_some(),
        });
        debug_assert!(matches!(
            plan,
            HintHighlightTransition::SetHighlight
                | HintHighlightTransition::NoMatchClearHighlight { .. }
        ));

        let current = self.context_manager.current_mut();

        if let Some(hint_match) = highlighted_hint {
            // Mark the hint range as damaged so it gets re-rendered.
            //
            // Two damage signals are required:
            // * Terminal-side: `update_selection_damage` marks the affected
            // lines so the partial render path knows what to redraw.
            // * Renderer-side: `pending_update.set_terminal_damage(Full)`
            // ensures the render loop doesn't early-exit on
            // `!pending_update.is_dirty()`
            {
                let Some(mut terminal) = current.terminal.try_lock_unfair() else {
                    current.renderable_content.pending_update.set_dirty();
                    return false;
                };
                let display_offset = terminal.display_offset();

                let hint_range = neoism_terminal_core::selection::SelectionRange::new(
                    hint_match.start,
                    hint_match.end,
                    false,
                );
                terminal.update_selection_damage(Some(hint_range), display_offset);
            }

            current
                .renderable_content
                .pending_update
                .set_terminal_damage(neoism_terminal_core::damage::TerminalDamage::Full);
            current.renderable_content.highlighted_hint = Some(hint_match);
            true
        } else {
            if current.renderable_content.highlighted_hint.is_some() {
                let Some(mut terminal) = current.terminal.try_lock_unfair() else {
                    current.renderable_content.pending_update.set_dirty();
                    return false;
                };
                let display_offset = terminal.display_offset();
                terminal.update_selection_damage(None, display_offset);
            }

            // Force a render so the previously-highlighted line clears.
            if had_highlight {
                current
                    .renderable_content
                    .pending_update
                    .set_terminal_damage(
                        neoism_terminal_core::damage::TerminalDamage::Full,
                    );
            }
            current.renderable_content.highlighted_hint = None;
            had_highlight
        }
    }
}
