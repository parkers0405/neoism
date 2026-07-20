use super::*;

impl Screen<'_> {
    /// Feed logical pointer coordinates into the same-frame inline lens hit
    /// regions. Diagnostic hover owns the pointer ahead of symbol hover, so a
    /// full diagnostic card and an unrelated documentation popup never stack.
    pub(crate) fn update_inline_diagnostic_hover(
        &mut self,
    ) -> neoism_ui::panels::inline_diagnostics::InlineDiagnosticHoverOutcome {
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let outcome = self.renderer.inline_diagnostics.hover(mouse_x, mouse_y);
        if outcome.changed {
            self.mark_dirty();
        }
        outcome
    }

    /// Pin/unpin an inline detail card or dismiss it from an outside click.
    pub(crate) fn handle_inline_diagnostic_click(&mut self) -> bool {
        use neoism_ui::panels::inline_diagnostics::InlineDiagnosticClickAction;

        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        match self.renderer.inline_diagnostics.click(mouse_x, mouse_y) {
            InlineDiagnosticClickAction::None => false,
            InlineDiagnosticClickAction::Dismissed => {
                self.mark_dirty();
                false
            }
            InlineDiagnosticClickAction::Consumed => {
                self.mark_dirty();
                true
            }
            InlineDiagnosticClickAction::QuickFix { .. } => {
                // nvim removed; native editor quick-fix TBD.
                self.renderer.inline_diagnostics.dismiss_detail();
                self.mark_dirty();
                true
            }
        }
    }

    pub(crate) fn dismiss_inline_diagnostic_detail(&mut self) -> bool {
        if self.renderer.inline_diagnostics.dismiss_detail() {
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    pub(crate) fn clear_inline_diagnostic_hover(&mut self) -> bool {
        if self.renderer.inline_diagnostics.clear_hover() {
            self.mark_dirty();
            true
        } else {
            false
        }
    }

    pub fn scroll_bottom_when_cursor_not_visible(&mut self) {
        let mut terminal = self.ctx_mut().current_mut().terminal.lock();
        if terminal.display_offset() != 0 {
            terminal.scroll_display(Scroll::Bottom);
        }
        drop(terminal);
    }
}
