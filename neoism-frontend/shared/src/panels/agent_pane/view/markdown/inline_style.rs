use super::*;

pub(crate) fn parsed_markdown_inline_line(line: &str) -> Rc<Vec<MarkdownInlineSegment>> {
    if let Some(hit) = INLINE_LINE_CACHE.with(|cache| cache.borrow().get(line)) {
        return hit;
    }
    let segments = Rc::new(parse_markdown_inline_line(line));
    INLINE_LINE_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .insert(line.to_string(), segments.clone())
    });
    segments
}

fn parse_markdown_inline_line(line: &str) -> Vec<MarkdownInlineSegment> {
    let mut out = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                out.push(MarkdownInlineSegment::Bold(after[..end].to_string()));
                rest = &after[end + 2..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix("~~") {
            if let Some(end) = after.find("~~") {
                out.push(MarkdownInlineSegment::Strike(after[..end].to_string()));
                rest = &after[end + 2..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                let text = &after[..end];
                let target = md::looks_like_inline_code_ref(text)
                    .then(|| md::clean_link_target(text).to_string());
                out.push(MarkdownInlineSegment::Code {
                    text: text.to_string(),
                    target,
                });
                rest = &after[end + 1..];
                continue;
            }
        }
        if let Some(link) = md::parse_markdown_link(rest) {
            let target =
                md::looks_like_file_ref(link.target).then(|| link.target.to_string());
            out.push(MarkdownInlineSegment::MarkdownLink {
                label: link.label.to_string(),
                source_target: link.target.to_string(),
                target,
            });
            rest = &rest[link.consumed..];
            continue;
        }
        let next = md::next_inline_marker(rest).unwrap_or(rest.len());
        let text = &rest[..next.max(1).min(rest.len())];
        parse_plain_markdown_segment(text, &mut out);
        rest = &rest[text.len()..];
    }
    out
}

fn parse_plain_markdown_segment(text: &str, out: &mut Vec<MarkdownInlineSegment>) {
    let mut token = String::new();
    let mut whitespace = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !token.is_empty() {
                push_plain_markdown_token(&token, out);
                token.clear();
            }
            whitespace.push(ch);
        } else {
            if !whitespace.is_empty() {
                out.push(MarkdownInlineSegment::Text(std::mem::take(&mut whitespace)));
            }
            token.push(ch);
        }
    }
    if !whitespace.is_empty() {
        out.push(MarkdownInlineSegment::Text(whitespace));
    }
    if !token.is_empty() {
        push_plain_markdown_token(&token, out);
    }
}

fn push_plain_markdown_token(token: &str, out: &mut Vec<MarkdownInlineSegment>) {
    let target = md::clean_link_target(token);
    let clickable = md::looks_like_file_ref(target);
    out.push(MarkdownInlineSegment::PlainToken {
        text: token.to_string(),
        target: clickable.then(|| target.to_string()),
        style: (!clickable)
            .then(|| plain_markdown_token_style(token))
            .flatten(),
    });
}

fn plain_markdown_token_style(token: &str) -> Option<PlainTokenStyle> {
    let clean = token.trim_matches(|ch: char| {
        matches!(
            ch,
            '(' | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | '.'
                | ':'
                | ';'
                | '!'
                | '?'
                | '"'
                | '\''
        )
    });
    if clean.is_empty() {
        return None;
    }
    let bold = plain_markdown_clean_token_bold(clean);
    if clean.starts_with('/') || clean.starts_with('@') || clean.starts_with('#') {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Cyan,
            bold,
        });
    }
    if is_template_reference(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Magenta,
            bold,
        });
    }
    if clean.starts_with('!') && clean.len() > 1 {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Yellow,
            bold,
        });
    }
    if is_keyboard_shortcut(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Accent,
            bold,
        });
    }
    if is_agent_mode_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Magenta,
            bold,
        });
    }
    if is_task_workflow_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Cyan,
            bold,
        });
    }
    if is_tool_config_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Blue,
            bold,
        });
    }
    if is_permission_policy_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Yellow,
            bold,
        });
    }
    if is_model_context_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::SynType,
            bold,
        });
    }
    if is_env_or_constant_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::SynType,
            bold,
        });
    }
    if is_success_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Green,
            bold,
        });
    }
    if is_warning_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Yellow,
            bold,
        });
    }
    if is_error_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::Red,
            bold,
        });
    }
    if looks_like_codeish_word(clean) {
        return Some(PlainTokenStyle {
            color: PlainTokenColor::SynString,
            bold,
        });
    }
    None
}

fn plain_markdown_clean_token_bold(clean: &str) -> bool {
    clean.starts_with('/')
        || clean.starts_with('@')
        || clean.starts_with('#')
        || is_template_reference(clean)
        || is_keyboard_shortcut(clean)
        || is_agent_mode_word(clean)
        || is_task_workflow_word(clean)
        || is_tool_config_word(clean)
        || is_permission_policy_word(clean)
        || is_model_context_word(clean)
        || is_env_or_constant_word(clean)
        || is_success_word(clean)
        || is_warning_word(clean)
        || is_error_word(clean)
}

pub(crate) fn plain_token_color(style: PlainTokenStyle, theme: &IdeTheme) -> u32 {
    match style.color {
        PlainTokenColor::Accent => theme.accent,
        PlainTokenColor::Blue => theme.blue,
        PlainTokenColor::Cyan => theme.cyan,
        PlainTokenColor::Magenta => theme.magenta,
        PlainTokenColor::Yellow => theme.yellow,
        PlainTokenColor::SynType => theme.syn_type,
        PlainTokenColor::SynString => theme.syn_string,
        PlainTokenColor::Green => theme.green,
        PlainTokenColor::Red => theme.red,
    }
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

pub(crate) fn draw_hover_underline<P: AgentMarkdownPane>(
    sugarloaf: &mut Sugarloaf,
    pane: &P,
    target: &str,
    rect: [f32; 4],
    theme: &IdeTheme,
    viewport_clip: [f32; 4],
) {
    if pane.link_hovered(target) {
        draw_rect_clipped(
            sugarloaf,
            rect,
            theme.f32(theme.readable_accent(theme.blue)),
            ORDER_TEXT,
            viewport_clip,
        );
    }
}

pub(crate) fn rgba_from_u8(color: [u8; 4]) -> [f32; 4] {
    [
        color[0] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[2] as f32 / 255.0,
        color[3] as f32 / 255.0,
    ]
}
