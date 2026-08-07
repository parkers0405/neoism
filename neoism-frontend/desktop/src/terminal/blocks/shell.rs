//! Desktop shell helpers.
//!
//! The pure parts (sanitisation, parsing, `display_path`,
//! `is_clear_command`, the `HISTORY_LIMIT`/`PROMPT_BURST_MS` constants,
//! and the `TerminalShellKind` enum itself) live in `neoism-ui`. This
//! module re-exports them and adds the host-only path-discovery helpers
//! that need `dirs` and the foreground-process detection that needs
//! `libc::tcgetpgrp` + `/proc/<pid>/comm`.

use std::path::PathBuf;

pub use neoism_ui::terminal_blocks::shell::*;
pub use neoism_ui::TerminalShellKind;

#[cfg(not(target_os = "macos"))]
pub use super::shell_detect::detect_foreground_shell;

pub fn default_terminal_history_path() -> Option<PathBuf> {
    dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .map(|base| base.join("neoism").join(TERMINAL_HISTORY_FILE))
}

pub fn default_terminal_favorites_path() -> Option<PathBuf> {
    dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .map(|base| base.join("neoism").join(TERMINAL_FAVORITES_FILE))
}

pub fn legacy_config_terminal_history_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("neoism").join(TERMINAL_HISTORY_FILE))
}

pub fn default_zsh_history_path() -> Option<PathBuf> {
    std::env::var("HISTFILE")
        .ok()
        .and_then(|value| expand_shell_path(&value))
        .or_else(|| dirs::home_dir().map(|home| home.join(ZSH_HISTORY_FILE)))
}

pub fn expand_shell_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value == "~" {
        return dirs::home_dir();
    }
    value
        .strip_prefix("~/")
        .and_then(|rest| dirs::home_dir().map(|home| home.join(rest)))
        .or_else(|| Some(PathBuf::from(value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_payload_sends_plain_command_without_control_prefix() {
        let payload = TerminalShellKind::Bash.command_payload("echo ok", true);
        assert_eq!(payload, b"echo ok\n");
    }

    #[test]
    fn command_payload_strips_c0_controls_except_newline_and_tab() {
        let payload =
            TerminalShellKind::Bash.command_payload("echo\x04 ok\tthere\nnext", true);
        assert_eq!(payload, b"echo ok\tthere\nnext\n");
    }

    #[test]
    fn command_payload_normalizes_multiline_paste_before_submit() {
        let payload = TerminalShellKind::Bash
            .command_payload("echo one\r\necho \x1b[31mtwo\x03", true);
        assert_eq!(payload, b"echo one\necho two\n");
        assert!(!payload.windows(2).any(|window| window == b"\\n"));
        assert!(!payload.contains(&b'\x1b'));
        assert!(!payload.contains(&b'\r'));
    }
}
