use std::path::PathBuf;
use web_time::Instant;

use super::command::{
    duration_ms, BlockStatusKind, CommandBlockSnapshot, TerminalCommandBlock,
    TerminalCommandBlockStatus,
};
use super::completion::{
    byte_at_char_column, common_prefix_case_insensitive, completion_candidates,
    completion_detail, completion_labels, history_fuzzy_match, history_prefix_match,
    CompletionCandidate, CompletionCycle, CompletionFlash, CompletionKind,
    NO_MATCH_FLASH_MS, NO_MATCH_SHAKE_AMP, SUCCESS_FLASH_MS,
};
use super::history::PersistentHistory;
use super::shell::{
    command_prefers_hidden_cursor, display_path, parse_zsh_history_line,
    sanitize_history_entry, sanitize_input_text, HISTORY_LIMIT, PROMPT_BURST_MS,
};

#[derive(Debug, Default)]
pub struct TerminalInputBuffer {
    shell_kind: Option<crate::input::TerminalShellKind>,
    text: String,
    cursor: usize,
    desired_visual_column: Option<usize>,
    history: Vec<String>,
    history_cursor: Option<usize>,
    history_draft: String,
    history_prefix: Option<String>,
    persistent_history: Option<PersistentHistory>,
    favorite_commands: Vec<String>,
    persistent_favorites: Option<PathBuf>,
    completion_items: Vec<String>,
    completion_detail: Option<String>,
    completion_state: Option<CompletionCycle>,
    completion_scope: Option<(String, Option<std::path::PathBuf>)>,
    command_blocks: Vec<TerminalCommandBlock>,
    passthrough_session_active: bool,
    /// True once the shell has emitted at least one OSC 133 prompt
    /// (`awaiting_command`) since this pane started. Before the first
    /// prompt arrives the terminal is still booting its shell
    /// integration, so `awaiting_command` reads `false` even though the
    /// composer should already own the empty command line. Gating the
    /// fresh-terminal window on this flag stops the first command's
    /// early keystrokes from leaking to the raw PTY (the shell's own
    /// line editor) while later keystrokes land in the composer —
    /// which submitted a different command than what was displayed.
    ever_awaited_command: bool,
    /// Joined-shell fallback for PTYs created before Neoism installed OSC
    /// prompt integration. Latched when the live cursor row looks like a
    /// returned shell prompt (or when remote `clear` explicitly resets the
    /// pane), and cleared on the next submission/command start.
    remote_prompt_fallback_open: bool,
    /// Stability anchor for the joined-shell BLANK-prompt completion. Neoism's
    /// daemon block-wrapper sets `PROMPT=''`, so a joined shell that emits no
    /// OSC 133 leaves no prompt glyph to key off. Instead we require a blank
    /// cursor row (sitting below the block's output) to hold the SAME position
    /// across two observations before finishing: while output is still
    /// streaming the cursor advances every frame and never stabilizes, so a
    /// long-running command is never marked done early.
    remote_blank_prompt_anchor: Option<usize>,
    /// Last OSC 133;D generation observed from the terminal parser. Keeping
    /// this separately from the final running/awaiting booleans preserves a
    /// fast command's completion when all lifecycle bytes arrive in one
    /// parser batch.
    last_command_finished_generation: u64,
    prompt_burst_started: Option<Instant>,
    /// Visual feedback for the most recent Tab press — drives the
    /// composer's success-flash + no-match-shake animations.
    completion_flash: Option<CompletionFlash>,
    control_notice: Option<(ControlNotice, Instant)>,
}

#[derive(Debug, Clone, Copy)]
enum ControlNotice {
    Interrupt,
}

mod buffer;
mod trait_impl;

pub(crate) use trait_impl::history_suggestion_allowed;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
