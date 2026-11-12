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

/// A reply's text cut before each top-level heading, with the space each piece is owed above
/// it. A long reply is drawn as these pieces, each cached by its own text (`timeline.rs`):
/// while it streams only the last one changes, so only that one is parsed, highlighted and
/// built again, however long the reply has grown. As one document the whole of it was, thirty
/// times a second (a 50 KB reply with a dozen code blocks: 2.5 ms of every such commit).
///
/// A heading at the start of a line always opens a new top-level block (it ends a list, a
/// quote and a paragraph alike), so the pieces draw as the whole did. The space above each is
/// what the Markdown kit puts above a heading inside a document: the larger of the heading's
/// own top margin (28, 24, 20 by level) and the bottom margin of what precedes it (24 under
/// a code block or an image, 32 under a rule; nothing else is taller than a heading's).
/// Reference-style link definitions resolve across the whole text, so a reply that has any
/// stays whole.
pub fn chunks(source: &str) -> Vec<(&str, f32)> {
    let mut out = Vec::new();
    let (mut start, mut space, mut offset, mut in_fence) = (0, 0.0, 0, false);
    // The bottom margin of the last non-blank line's block, as far as it can exceed a heading's top.
    let mut above = 0.0_f32;
    let mut content = false;
    let mut after_blank = true;
    fn continue_offset(offset: &mut usize, line: &str) { *offset += line.len(); }
    for line in source.split_inclusive('\n') {
        let fence = { let t = line.trim_start(); t.starts_with("```") || t.starts_with("~~~") };
        if fence { in_fence = !in_fence; }
        if !in_fence && !fence {
            if line.starts_with('[') && line.contains("]: ") { return vec![(source, 0.0)]; }
            let level = line.bytes().take_while(|b| *b == b'#').count();
            if content && (1..=6).contains(&level) && line.as_bytes().get(level).is_some_and(|b| *b == b' ' || *b == b'\n') {
                out.push((&source[start..offset], space));
                start = offset;
                space = match level { 1 => 28.0_f32, 2 => 24.0, _ => 20.0 }.max(above);
            }
        }
        let shown = line.trim();
        if shown.is_empty() { after_blank = true; continue_offset(&mut offset, line); continue; }
        {
            content = true;
            // Dashes right under a paragraph underline it (a heading); after a blank line they rule.
            let rule = after_blank && !in_fence && !fence && shown.len() >= 3 && [b'-', b'*', b'_'].iter().any(|c| shown.bytes().all(|b| b == *c || b == b' ') && shown.bytes().filter(|b| b == c).count() >= 3);
            above = if rule { 32.0 } else if (fence && !in_fence && !line.starts_with([' ', '\t'])) || (!in_fence && shown.starts_with("![")) { 24.0 } else { 0.0 };
            after_blank = false;
        }
        continue_offset(&mut offset, line);
    }
    out.push((&source[start..], space));
    out
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

    #[test]
    fn a_reply_is_cut_before_each_heading_outside_code() {
        let source = "Intro line.\n\n## One\ntext\n\n```sh\n# not a heading\n```\n\n### Two\n- item\n\n---\n\n# Three\nend";
        let pieces = chunks(source);
        assert_eq!(pieces.iter().map(|(text, _)| *text).collect::<String>(), source, "nothing is lost or repeated");
        let heads: Vec<(&str, f32)> = pieces.iter().map(|(text, space)| (text.lines().next().unwrap(), *space)).collect();
        // The space above is the heading's own, or more under a code block (24) or a rule (32).
        assert_eq!(heads, [("Intro line.", 0.0), ("## One", 24.0), ("### Two", 24.0), ("# Three", 32.0)]);
        // A reply that opens with a heading is not cut ahead of it; `#hashtag` is no heading.
        assert_eq!(chunks("# Title\nbody\n#tag\n").len(), 1);
        // Reference links are defined anywhere in the text: it stays whole.
        assert_eq!(chunks("See [a][1].\n\n## More\n\n[1]: https://example.com\n").len(), 1);
        // A piece settles once the next begins: text arriving later never changes it.
        let grown = format!("{source} more\n\n## Four\nlater");
        assert_eq!(chunks(&grown)[..3], pieces[..3]);
    }
}
