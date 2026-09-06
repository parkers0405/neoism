use std::path::{Path, PathBuf};

pub const HISTORY_LIMIT: usize = 256;
pub const PROMPT_BURST_MS: f32 = 320.0;
pub const TERMINAL_FAVORITES_FILE: &str = "terminal-favorites";
pub const TERMINAL_HISTORY_FILE: &str = "terminal-history";
pub const ZSH_HISTORY_FILE: &str = ".zsh_history";

#[cfg(not(target_arch = "wasm32"))]
pub fn default_terminal_history_path() -> Option<PathBuf> {
    dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .map(|base| base.join("neoism").join(TERMINAL_HISTORY_FILE))
}

#[cfg(target_arch = "wasm32")]
pub fn default_terminal_history_path() -> Option<PathBuf> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn legacy_config_terminal_history_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("neoism").join(TERMINAL_HISTORY_FILE))
}

#[cfg(target_arch = "wasm32")]
pub fn legacy_config_terminal_history_path() -> Option<PathBuf> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn default_zsh_history_path() -> Option<PathBuf> {
    std::env::var("HISTFILE")
        .ok()
        .and_then(|value| expand_shell_path(&value))
        .or_else(|| dirs::home_dir().map(|home| home.join(ZSH_HISTORY_FILE)))
}

#[cfg(target_arch = "wasm32")]
pub fn default_zsh_history_path() -> Option<PathBuf> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
pub fn expand_shell_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

pub fn sanitize_history_entry(command: &str) -> String {
    command
        .trim_matches(|ch| ch == '\r' || ch == '\n')
        .to_string()
}

pub fn sanitize_input_text(text: &str) -> String {
    strip_terminal_control_sequences(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || !ch.is_control())
        .collect()
}

fn strip_terminal_control_sequences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }

        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                consume_until_string_terminator(&mut chars, true);
            }
            Some('P' | '_' | '^') => {
                chars.next();
                consume_until_string_terminator(&mut chars, false);
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

fn consume_until_string_terminator<I>(chars: &mut std::iter::Peekable<I>, bel: bool)
where
    I: Iterator<Item = char>,
{
    let mut saw_escape = false;
    for c in chars.by_ref() {
        if bel && c == '\x07' {
            break;
        }
        if saw_escape {
            if c == '\\' {
                break;
            }
            saw_escape = false;
        }
        if c == '\x1b' {
            saw_escape = true;
        }
    }
}

pub fn parse_zsh_history_line(line: &str) -> Option<String> {
    let command = if line.starts_with(": ") {
        line.find(';')
            .map(|separator| &line[separator + 1..])
            .unwrap_or(line)
    } else {
        line
    };
    let command = sanitize_history_entry(command);
    (!command.trim().is_empty()).then_some(command)
}

pub fn command_prefers_hidden_cursor(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    let Some(program) = parts.next() else {
        return false;
    };
    let program = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    match program {
        "watch" => true,
        "docker" => matches!(parts.next(), Some("logs")),
        "kubectl" => matches!(parts.next(), Some("logs")),
        "journalctl" => command.split_whitespace().any(|part| part == "-f"),
        "tail" => command
            .split_whitespace()
            .any(|part| matches!(part, "-f" | "--follow")),
        _ => false,
    }
}

pub fn display_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

pub fn is_clear_command(command: &str) -> bool {
    crate::input::TerminalShellKind::Unknown.is_clear_command(command)
}
