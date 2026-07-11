use super::*;

use crate::input::CompletionFlashState;

pub(crate) fn history_suggestion_allowed(text: &str) -> bool {
    let command = text
        .trim_start()
        .split_whitespace()
        .next()
        .unwrap_or_default();
    !matches!(command, "cd" | "pushd")
}

/// Bridge `TerminalInputBuffer` to the shared `crate::input::InputBuffer`
/// surface so it can be passed to the shared `command_composer` panel as
/// `&dyn InputBuffer`. Each method delegates to the existing inherent
/// method.
impl crate::input::InputBuffer for TerminalInputBuffer {
    fn text(&self) -> &str {
        TerminalInputBuffer::text(self)
    }

    fn cursor_byte(&self) -> usize {
        TerminalInputBuffer::cursor_byte(self)
    }

    fn is_empty(&self) -> bool {
        TerminalInputBuffer::is_empty(self)
    }

    fn completion_items(&self) -> &[String] {
        TerminalInputBuffer::completion_items(self)
    }

    fn completion_detail(&self) -> Option<&str> {
        TerminalInputBuffer::completion_detail(self)
    }

    fn flash_state(&self) -> Option<CompletionFlashState> {
        TerminalInputBuffer::flash_state(self)
    }

    fn control_notice(&self) -> Option<&'static str> {
        TerminalInputBuffer::control_notice(self)
    }

    fn prompt_burst_elapsed_ms(&self) -> Option<f32> {
        TerminalInputBuffer::prompt_burst_elapsed_ms(self)
    }

    fn suggestion_after_cursor(&self) -> Option<&str> {
        TerminalInputBuffer::suggestion_after_cursor(self)
    }

    fn is_prompt_animating(&self) -> bool {
        TerminalInputBuffer::is_prompt_animating(self)
    }

    // `shell_kind` is fed to the shared composer through a separate
    // argument at the call site (`ctx.terminal_shell_kind`), so the
    // input buffer itself doesn't carry one. Default trait impl returns
    // `TerminalShellKind::Unknown`.
}
