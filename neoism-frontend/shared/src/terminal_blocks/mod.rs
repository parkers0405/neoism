pub mod command;
pub mod completion;
pub mod history;
pub mod shell;

pub use command::{
    duration_ms, BlockStatusKind, CommandBlockSnapshot, TerminalCommandBlock,
    TerminalCommandBlockStatus,
};
pub use completion::{
    CompletionCandidate, CompletionCycle, CompletionFlash, CompletionKind,
    COMPLETION_LIMIT, NO_MATCH_FLASH_MS, NO_MATCH_SHAKE_AMP, SUCCESS_FLASH_MS,
};
pub use history::PersistentHistory;
pub use shell::{
    command_prefers_hidden_cursor, display_path, is_clear_command,
    parse_zsh_history_line, sanitize_history_entry, sanitize_input_text, HISTORY_LIMIT,
    PROMPT_BURST_MS, TERMINAL_FAVORITES_FILE, TERMINAL_HISTORY_FILE, ZSH_HISTORY_FILE,
};

pub const COMMAND_BLOCK_CHROME_ROWS: usize = 2;
