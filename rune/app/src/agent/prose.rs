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

/// One independently drawn piece of a reply (`chunks`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece<'a> {
    pub text: &'a str,
    /// The space this piece is owed above it: what the Markdown kit would put between its
    /// first block and the block before, were they one document.
    pub space: f32,
    pub cut: Cut,
}

/// Why a piece starts where it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// The reply's start, or a top-level heading.
    Heading,
    /// A paragraph or code block that opens after a blank line, in a piece already long.
    Block,
    /// A long fenced code block, whole and alone: drawn in slabs (`timeline::code_slabs`).
    Code,
}

/// A piece is cut at a paragraph only once it holds this much text: short replies stay whole.
const PIECE_BYTES: usize = 1200;
/// A top-level fence with at least this many lines is a piece of its own, drawn in slabs.
pub const SLAB_FROM: usize = 48;

/// A reply's text cut into pieces that draw, stacked, exactly as the whole would: before each
/// top-level heading, before a paragraph or code block that follows a blank line once the
/// piece is long, and around each long code block. Each piece is cached by its own text
/// (`timeline.rs`): while a reply streams only the last one changes, so only that one is
/// parsed, highlighted, shaped and built again, however long the reply has grown. As one
/// document the whole of it was, thirty times a second (a 50 KB reply with a dozen code
/// blocks: 2.5 ms of every such commit; a 40 KB section with no heading in it: 7 ms).
///
/// A heading at the start of a line always opens a new top-level block (it ends a list, a
/// quote and a paragraph alike), and so does unindented text or a fence after a blank line,
/// so the pieces draw as the whole did. The space above each is the kit's collapsed margin
/// (`rune_kit::markdown`, `margins`): the larger of the new block's top margin (a heading's
/// 28, 24, 20 by level, a code block's 12, a paragraph's 0) and the bottom margin of what
/// precedes it (12 under a paragraph or a list, 16 under a quote, 24 under a code block or an
/// image, 32 under a rule). Under a heading it is the heading's own bottom margin.
/// Reference-style link definitions resolve across the whole text, so a reply that has any
/// stays whole.
///
/// Cuts depend only on the text before them and the line they open with, so a piece settles
/// once the next begins: text arriving later never changes it.
pub fn chunks(source: &str) -> Vec<Piece<'_>> {
    /// What the last non-blank line leaves below it.
    #[derive(Clone, Copy)]
    enum Below {
        /// A block's bottom margin, collapsing with the next block's top.
        Margin(f32),
        /// A heading's: the gap under it whatever follows.
        Heading(f32),
        /// Not somewhere to cut (a table row, HTML, a line that may yet be underlined).
        Unknown,
    }
    let whole = |source| vec![Piece { text: source, space: 0.0, cut: Cut::Heading }];
    let mut out: Vec<Piece> = Vec::new();
    let (mut start, mut space, mut cut, mut offset, mut in_fence) = (0, 0.0, Cut::Heading, 0, false);
    let mut below = Below::Unknown;
    let mut content = false;
    let mut after_blank = true;
    // The open top-level fence: where it starts, the space owed above it, its lines so far.
    let mut fence_at: Option<(usize, f32, usize)> = None;
    // A long code block just closed: the next block starts a piece.
    let mut after_code = false;
    for line in source.split_inclusive('\n') {
        let fence = { let t = line.trim_start(); t.starts_with("```") || t.starts_with("~~~") };
        let was_in_fence = in_fence;
        if fence { in_fence = !in_fence; }
        let shown = line.trim();
        let top_level = !line.starts_with([' ', '\t']);
        let opens_fence = fence && in_fence;
        if !was_in_fence && !shown.is_empty() {
            if !fence && line.starts_with('[') && line.contains("]: ") { return whole(source); }
            let level = if fence { 0 } else { line.bytes().take_while(|b| *b == b'#').count() };
            let heading = (1..=6).contains(&level) && line.as_bytes().get(level).is_some_and(|b| *b == b' ' || *b == b'\n');
            // Dashes right under a paragraph underline it (a heading); after a blank line they rule.
            let rule = after_blank && !fence && shown.len() >= 3 && [b'-', b'*', b'_'].iter().any(|c| shown.bytes().all(|b| b == *c || b == b' ') && shown.bytes().filter(|b| b == c).count() >= 3);
            // The gap the kit would leave above this line's block, where a piece may start.
            let gap = |top: f32| match below { Below::Margin(bottom) => Some(top.max(bottom)), Below::Heading(bottom) => Some(bottom), Below::Unknown => None };
            let begins = if !content {
                None
            } else if heading {
                let top = match level { 1 => 28.0_f32, 2 => 24.0, _ => 20.0 };
                Some((match below { Below::Margin(bottom) => top.max(bottom), _ => top }, Cut::Heading))
            } else if after_code && top_level {
                // Whatever follows a code block is at least its 24 below it.
                gap(if rule { 32.0 } else { 0.0 }).map(|space| (space, Cut::Block))
            } else if after_blank && top_level && offset - start >= PIECE_BYTES && (opens_fence || opens_paragraph(line)) {
                gap(if opens_fence { 12.0 } else { 0.0 }).map(|space| (space, Cut::Block))
            } else {
                None
            };
            if let Some((above, kind)) = begins && offset > start {
                // Blank lines alone are no piece: they stay at the head of the one they precede.
                if !source[start..offset].trim().is_empty() {
                    out.push(Piece { text: &source[start..offset], space, cut });
                    start = offset;
                }
                (space, cut) = (above, kind);
            }
            after_code = false;
            if opens_fence && top_level && !line_language(line).is_some_and(|lang| ["diff", "patch", "mermaid"].iter().any(|own| lang.eq_ignore_ascii_case(own))) {
                fence_at = if content { gap(12.0).map(|space| (offset, space, 0)) } else { Some((offset, 0.0, 0)) };
            }
            below = if heading {
                Below::Heading(match level { 1 => 12.0, 2 => 10.0, _ => 8.0 })
            } else if rule {
                Below::Margin(32.0)
            } else if opens_fence {
                Below::Unknown
            } else if shown.starts_with("![") {
                Below::Margin(24.0)
            } else if shown.starts_with('>') {
                Below::Margin(16.0)
            } else if shown.starts_with(['|', '<']) || shown.bytes().all(|b| b == b'=' || b == b'-' || b == b' ') {
                Below::Unknown
            } else {
                Below::Margin(12.0)
            };
        } else if was_in_fence {
            if let Some((_, _, lines)) = &mut fence_at && in_fence { *lines += 1; }
            if !in_fence {
                // The fence closed. A long top-level one becomes a piece of its own.
                below = Below::Margin(24.0);
                if let Some((at, above, lines)) = fence_at.take() && lines >= SLAB_FROM {
                    if at > start {
                        out.push(Piece { text: &source[start..at], space, cut });
                        (space, cut) = (above, Cut::Code);
                    } else {
                        cut = Cut::Code;
                    }
                    out.push(Piece { text: &source[at..offset + line.len()], space, cut });
                    (start, space, cut) = (offset + line.len(), 0.0, Cut::Block);
                    after_code = true;
                }
            }
        }
        if !shown.is_empty() { content = true; }
        after_blank = shown.is_empty() && !in_fence;
        offset += line.len();
    }
    // A long fence still arriving is already a piece of its own.
    if let Some((at, above, lines)) = fence_at && in_fence && lines >= SLAB_FROM {
        if at > start {
            out.push(Piece { text: &source[start..at], space, cut });
            space = above;
        }
        (start, cut) = (at, Cut::Code);
    }
    if start < source.len() || out.is_empty() { out.push(Piece { text: &source[start..], space, cut }); }
    out
}

/// A fence line's language: the first word of its info string.
pub fn line_language(line: &str) -> Option<&str> {
    line.trim_start().trim_start_matches(['`', '~']).split(|c: char| c.is_whitespace() || c == ',').find(|s| !s.is_empty())
}

/// Whether `line`, unindented and after a blank line, can only be the start of a paragraph: it
/// opens with a letter, bold or inline code. Not a list item, a quote, a table, HTML, a rule or
/// a number that may turn out to be a list marker once the rest of the line arrives.
fn opens_paragraph(line: &str) -> bool {
    let mut chars = line.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() => true,
        Some('*') => chars.next() == Some('*') && chars.next().is_some_and(|c| !c.is_whitespace() && c != '*'),
        Some('`') => chars.next().is_some_and(|c| c != '`'),
        _ => false,
    }
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
        assert_eq!(pieces.iter().map(|piece| piece.text).collect::<String>(), source, "nothing is lost or repeated");
        let heads: Vec<(&str, f32)> = pieces.iter().map(|piece| (piece.text.lines().next().unwrap(), piece.space)).collect();
        // The space above is the heading's own, or more under a code block (24) or a rule (32).
        assert_eq!(heads, [("Intro line.", 0.0), ("## One", 24.0), ("### Two", 24.0), ("# Three", 32.0)]);
        assert!(pieces.iter().all(|piece| piece.cut == Cut::Heading));
        // A reply that opens with a heading is not cut ahead of it; `#hashtag` is no heading.
        assert_eq!(chunks("# Title\nbody\n#tag\n").len(), 1);
        // Reference links are defined anywhere in the text: it stays whole.
        assert_eq!(chunks("See [a][1].\n\n## More\n\n[1]: https://example.com\n").len(), 1);
        // A piece settles once the next begins: text arriving later never changes it.
        let grown = format!("{source} more\n\n## Four\nlater");
        assert_eq!(chunks(&grown)[..3], pieces[..3]);
    }

    #[test]
    fn a_long_section_is_cut_at_paragraphs_with_the_kits_margins() {
        let para = "word ".repeat(300);
        // Short text is never cut, however many paragraphs.
        assert_eq!(chunks("One.\n\nTwo.\n\nThree.\n").len(), 1);
        // A paragraph after a long one: 12 between paragraphs.
        let source = format!("{para}\n\nNext paragraph.\n");
        let pieces = chunks(&source);
        assert_eq!(pieces.iter().map(|piece| piece.text).collect::<String>(), source);
        assert_eq!(pieces.len(), 2);
        assert_eq!((pieces[1].text, pieces[1].space, pieces[1].cut), ("Next paragraph.\n", 12.0, Cut::Block));
        // What precedes sets the space: a list 12, a quote 16, a code block 24, a rule 32.
        // (The code block is itself cut from the paragraph before it, so it is a long one.)
        let block = format!("```\n{}```", "a line of code, forty characters long...\n".repeat(40));
        for (before, space) in [("- item\n- item", 12.0), ("> quoted", 16.0), (block.as_str(), 24.0), ("---", 32.0), ("![alt](a.png)", 24.0)] {
            let source = format!("{para}\n\n{before}\n\nNext paragraph.\n");
            let last = *chunks(&source).last().unwrap();
            assert_eq!((last.text, last.space), ("Next paragraph.\n", space), "after {:?}", before.lines().next());
        }
        // A code block is owed 12 above it, and more under what asks for more.
        let source = format!("{para}\n\n```rust\nfn main() {{}}\n```\n");
        assert_eq!(chunks(&source)[1].space, 12.0);
        // Never inside a list, a quote, a table or indented text, before a list or a number
        // that may become one, under a heading's text, or without a blank line.
        for rest in ["- one\n\n- two", "- one\n\n  continued", "1. one\n\n2. two", "text\n\n1", "> a\n\n> b", "| a |\n\nText", "text\nMore", "    code\n\n    more", "Title\n===\n\nText"] {
            let source = format!("{para}\n\n{rest}\n");
            let pieces = chunks(&source);
            assert!(pieces.len() <= 2 && pieces.last().unwrap().text.starts_with(rest.lines().next().unwrap()) || pieces.len() == 1, "cut inside {rest:?}: {:?}",
                pieces.iter().map(|piece| piece.text.chars().take(12).collect::<String>()).collect::<Vec<_>>());
        }
        // Streaming: every prefix cuts its settled pieces where the whole text does.
        let source = format!("{para}\n\nSecond {para}\n\n**Third** {para}\n\n`fourth` paragraph.\n");
        let whole = chunks(&source);
        assert_eq!(whole.len(), 4);
        for end in (0..source.len()).step_by(97) {
            let partial = chunks(&source[..end]);
            for (piece, settled) in partial.iter().zip(&whole).take(partial.len().saturating_sub(1)) {
                assert_eq!(piece, settled, "a settled piece changed at {end}");
            }
        }
    }

    #[test]
    fn a_long_code_block_is_a_piece_of_its_own() {
        let code: String = (0..SLAB_FROM + 10).map(|n| format!("let x{n} = {n};\n")).collect();
        let source = format!("Here is the file:\n\n```rust\n{code}```\n\nDone.\n");
        let pieces = chunks(&source);
        assert_eq!(pieces.iter().map(|piece| piece.text).collect::<String>(), source);
        assert_eq!(pieces.iter().map(|piece| (piece.cut, piece.space)).collect::<Vec<_>>(), [(Cut::Heading, 0.0), (Cut::Code, 12.0), (Cut::Block, 24.0)]);
        assert!(pieces[1].text.starts_with("```rust\n") && pieces[1].text.ends_with("```\n"));
        // Still arriving, it is already its own piece, and the text before it has settled.
        let open = &source[..source.len() - 30];
        let arriving = chunks(open);
        assert_eq!(arriving.len(), 2);
        assert_eq!(arriving[0], pieces[0]);
        assert_eq!(arriving[1].cut, Cut::Code);
        // A short block, a diff, a diagram and a fence inside a list stay with the prose.
        for block in [format!("```rust\n{}```", "x\n".repeat(SLAB_FROM - 1)), format!("```diff\n{code}```"), format!("```mermaid\n{code}```"), format!("- item\n\n  ```rust\n{}  ```", "  x\n".repeat(SLAB_FROM + 5))] {
            assert!(chunks(&format!("Text.\n\n{block}\n\nDone.\n")).iter().all(|piece| piece.cut != Cut::Code));
        }
        for lang in ["mermaid", "MERMAID", "mermaid,title=chart", "diff", "patch"] {
            for closer in ["", "```\n"] {
                let source = format!("```{lang}\n{code}{closer}");
                assert!(chunks(&source).iter().all(|piece| piece.cut != Cut::Code), "specialized {lang} frame must not become slabs, even without preceding prose");
            }
        }
        // A reply that is one long block; and one under a heading (the heading's own 12).
        assert_eq!(chunks(&format!("```rust\n{code}```\n"))[0].cut, Cut::Code);
        let titled = format!("# Title\n\n```rust\n{code}```\n");
        let under = chunks(&titled);
        assert_eq!((under[1].cut, under[1].space), (Cut::Code, 12.0));
    }
}
