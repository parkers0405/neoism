//! Prose and inline-code colouring for agent markdown, matching the GPUiX transcript
//! (`neoism-frontend/shared/src/panels/agent_pane/view/markdown/inline_style.rs`; the word
//! classifiers below are copied from it unchanged).

use crate::ui::theme::{Palette, rgb};
use rune_kit::markdown::Highlight;
use std::ops::Range;

/// Colour ranges for `source`: inline code by what it names, and plain words by the same
/// word lists the original painter uses. Fenced blocks are left to the syntax highlighter.
pub fn highlights(source: &str, p: &Palette) -> Vec<(Range<usize>, Highlight)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence {
            scan_line(line, offset, p, &mut out);
        }
        offset += line.len();
    }
    out
}

fn scan_line(line: &str, base: usize, p: &Palette, out: &mut Vec<(Range<usize>, Highlight)>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            if let Some(end) = line[i + 1..].find('`') {
                let code = &line[i + 1..i + 1 + end];
                let color = if looks_like_path(code) { p.blue } else { p.syn_string };
                out.push((base + i + 1..base + i + 1 + end, Highlight::new().bg(rune::Color::TRANSPARENT).fg(rgb(color))));
                i += end + 2;
                continue;
            }
            // An unclosed backtick (streaming text before its pair arrives) is just a character.
            i += 1;
            continue;
        }
        if bytes[i].is_ascii_whitespace() || matches!(bytes[i], b'*' | b'_' | b'[' | b']' | b'(' | b')') {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'`' { i += 1; }
        let token = &line[start..i];
        let clean = token.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | ',' | '.' | ':' | ';' | '!' | '?' | '"' | '\'' | '*' | '_'));
        if clean.is_empty() || clean.starts_with("http") { continue; }
        if let Some(color) = word_color(clean, p) {
            let lead = token.find(clean).unwrap_or(0);
            out.push((base + start + lead..base + start + lead + clean.len(), Highlight::new().bg(rune::Color::TRANSPARENT).fg(rgb(color))));
        }
    }
}

fn looks_like_path(code: &str) -> bool {
    !code.contains(' ') && (code.contains('/') || code.rsplit_once('.').is_some_and(|(_, ext)| ext.len() <= 5 && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())))
}

fn word_color(clean: &str, p: &Palette) -> Option<u32> {
    if clean.starts_with('/') || clean.starts_with('@') || clean.starts_with('#') { return Some(p.cyan); }
    if is_template_reference(clean) { return Some(p.magenta); }
    if clean.starts_with('!') && clean.len() > 1 { return Some(p.yellow); }
    if is_keyboard_shortcut(clean) { return Some(p.accent); }
    if is_agent_mode_word(clean) { return Some(p.magenta); }
    if is_task_workflow_word(clean) { return Some(p.cyan); }
    if is_tool_config_word(clean) { return Some(p.blue); }
    if is_permission_policy_word(clean) { return Some(p.yellow); }
    if is_model_context_word(clean) || is_env_or_constant_word(clean) { return Some(p.syn_type); }
    if is_success_word(clean) { return Some(p.green); }
    if is_warning_word(clean) { return Some(p.yellow); }
    if is_error_word(clean) { return Some(p.red); }
    if looks_like_codeish_word(clean) { return Some(p.syn_string); }
    None
}

fn is_keyboard_shortcut(token: &str) -> bool {
    token.contains('+')
        && token.split('+').all(|part| {
            matches!(
                part,
                "Ctrl"
                    | "Cmd"
                    | "Shift"
                    | "Alt"
                    | "Meta"
                    | "Enter"
                    | "Esc"
                    | "Tab"
                    | "Space"
            ) || part.len() == 1
        })
}

fn is_template_reference(token: &str) -> bool {
    token.starts_with('$')
        || token.starts_with("env:")
        || token.starts_with("file:")
        || token.starts_with("{env:")
        || token.starts_with("{file:")
}

fn is_env_or_constant_word(token: &str) -> bool {
    let mut has_letter = false;
    let mut has_lower = false;
    let mut has_separator = false;
    for ch in token.chars() {
        if ch.is_ascii_alphabetic() {
            has_letter = true;
            has_lower |= ch.is_ascii_lowercase();
        }
        has_separator |= ch == '_' || ch == '-';
    }
    has_letter && !has_lower && (has_separator || token.len() > 1)
}

fn is_agent_mode_word(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "agent"
            | "agents"
            | "subagent"
            | "subagents"
            | "build"
            | "plan"
            | "planning"
            | "review"
            | "debug"
            | "assistant"
            | "persona"
            | "personas"
    )
}

fn is_task_workflow_word(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "task"
            | "tasks"
            | "todo"
            | "todos"
            | "step"
            | "steps"
            | "workflow"
            | "workflows"
            | "session"
            | "sessions"
            | "message"
            | "messages"
            | "conversation"
            | "history"
            | "timeline"
            | "prompt"
            | "prompts"
            | "command"
            | "commands"
            | "filename"
            | "filenames"
            | "attach"
            | "attached"
            | "fuzzy"
            | "search"
            | "copy"
            | "paste"
            | "clear"
            | "undo"
            | "redo"
            | "resume"
            | "restore"
            | "revert"
            | "continue"
            | "jump"
            | "navigate"
            | "switch"
            | "cycle"
            | "toggle"
            | "pin"
            | "pinned"
            | "slot"
            | "slots"
            | "sidebar"
            | "panel"
            | "parent"
            | "child"
            | "action"
            | "actions"
            | "change"
            | "changes"
            | "diff"
            | "patch"
            | "branch"
            | "commit"
            | "github"
            | "issue"
            | "issues"
            | "pr"
            | "prs"
    )
}

fn is_tool_config_word(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "tool"
            | "tools"
            | "bash"
            | "shell"
            | "webfetch"
            | "fetch"
            | "edit"
            | "write"
            | "formatter"
            | "formatters"
            | "prettier"
            | "gofmt"
            | "ruff"
            | "lsp"
            | "mcp"
            | "plugin"
            | "plugins"
            | "hook"
            | "hooks"
            | "config"
            | "configuration"
            | "setting"
            | "settings"
            | "theme"
            | "themes"
            | "json"
            | "tui"
            | "server"
            | "servers"
            | "api"
            | "headless"
            | "script"
            | "scripts"
            | "scripting"
            | "notification"
            | "notifications"
            | "keybind"
            | "keybinds"
            | "leader"
            | "palette"
            | "shortcut"
            | "shortcuts"
            | "schema"
            | "autocomplete"
            | "format"
            | "logs"
            | "stderr"
            | "stdout"
    )
}

fn is_permission_policy_word(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "permission"
            | "permissions"
            | "allow"
            | "deny"
            | "ask"
            | "approval"
            | "approve"
            | "blocked"
            | "disable"
            | "disabled"
            | "enable"
            | "enabled"
            | "none"
            | "auto"
            | "manual"
            | "public"
            | "private"
            | "share"
            | "shared"
            | "unshare"
            | "protected"
            | "external_directory"
            | "doom_loop"
            | "sensitive"
            | "destructive"
    )
}

fn is_model_context_word(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "model"
            | "models"
            | "provider"
            | "providers"
            | "llm"
            | "context"
            | "instructions"
            | "rules"
            | "temperature"
            | "tokens"
            | "input"
            | "output"
            | "clipboard"
            | "image"
            | "images"
            | "pdf"
            | "file"
            | "files"
            | "directory"
            | "workspace"
            | "codebase"
            | "project"
            | "zen"
            | "terminal"
    )
}

fn is_success_word(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "done"
            | "success"
            | "successful"
            | "passed"
            | "pass"
            | "enabled"
            | "ready"
            | "complete"
            | "completed"
    )
}

fn is_warning_word(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "warning" | "warn" | "pending" | "running" | "todo" | "note" | "important"
    )
}

fn is_error_word(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "error" | "failed" | "failure" | "fail" | "blocked" | "bug" | "fixme" | "panic"
    )
}

fn looks_like_codeish_word(token: &str) -> bool {
    (token.contains("::")
        || token.contains("()")
        || token.contains("--")
        || token.contains('='))
        || token.starts_with("--")
        || token.ends_with(".rs")
        || token.ends_with(".ts")
        || token.ends_with(".tsx")
        || token.ends_with(".js")
        || token.ends_with(".jsx")
        || token.ends_with(".json")
        || token.ends_with(".toml")
        || token.ends_with(".md")
        || token.ends_with(".py")
        || token.ends_with(".go")
        || token.ends_with(".yaml")
        || token.ends_with(".yml")
        || token.ends_with(".nix")
        || token.ends_with(".lua")
        || token.ends_with(".sh")
        || token.ends_with(".lock")
        || token.ends_with(".log")
        || matches!(
            token,
            "AGENTS.md"
                | "README.md"
                | "tui.json"
                | "package.json"
                | "Cargo.toml"
                | "Cargo.lock"
        )
        || token.contains(".opencode/")
        || token.contains(".neoism/")
        || token.contains("~/.config/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unclosed_backticks_terminate() {
        let p = crate::ui::theme::palette("pastel_dark");
        // Streaming text often ends mid-span; this used to spin forever.
        for text in ["Counting files in `rune/app/src", "a ` b", "`", "``", "x `y` `z"] {
            let _ = highlights(text, p);
        }
        let marks = highlights("see `src/main.rs` now", p);
        assert!(marks.iter().any(|(r, _)| &"see `src/main.rs` now"[r.clone()] == "src/main.rs"));
    }
}
