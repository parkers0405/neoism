use serde_json::Value;

use crate::chat_diff::{patch_diff_sections, render_diff_rows, snapshot_diffs};
use crate::chat_ui::truncate_for_terminal;
use crate::{BOLD, DIM, GREEN, ORANGE, RED, RESET, WHITE};

const STRIKE: &str = "\x1b[9m";
const TOOL_BLUE: &str = "\x1b[38;2;125;179;255m";
const SHELL_COMMAND_BLUE: &str = "\x1b[38;2;69;126;232m";
const SHELL_NUMBER_GREEN: &str = "\x1b[38;2;152;195;121m";
const SHELL_STRING_YELLOW: &str = "\x1b[38;2;229;192;123m";

const BULLET: &str = "●";
const RUNNING_SQUARE: &str = "■";

#[derive(Clone, Debug)]
pub(crate) struct TruncatedOutput {
    pub(crate) header: String,
    pub(crate) lines: Vec<String>,
}

pub(crate) fn render_completed_tool(
    tool: &str,
    input: &Value,
    output: &str,
    metadata: &Value,
    title: &str,
) -> anyhow::Result<Option<TruncatedOutput>> {
    let truncated = match tool {
        "bash" => render_bash_cell(input, output),
        "task" => render_task_cell(input, output, metadata),
        "write" | "edit" | "apply_patch" | "patch" => {
            render_edit_cells(input, metadata, title)
        }
        "todowrite" => {
            render_todo_cell(metadata);
            None
        }
        _ => {
            render_generic_cell(tool, input, output);
            None
        }
    };
    Ok(truncated)
}

pub(crate) fn render_running_task(input: &Value, metadata: &Value) {
    let target = task_target(input);
    print_running_task_header(&target);
    let agent = string_field(input, &["subagent_type", "agent"])
        .unwrap_or_else(|| "subagent".to_string());
    if let Some(session_id) = metadata.get("sessionId").and_then(Value::as_str) {
        print_corner_line(&format!(
            "{DIM}@{agent} · {} · working{RESET}",
            terminal_link(&format!("neoism://session/{session_id}"), session_id)
        ));
    } else {
        print_corner_line(&format!("{DIM}@{agent} · starting subagent session{RESET}"));
    }
}

fn print_tool_header(name: &str, target: &str) {
    // Leading blank line gives consecutive tool cells breathing room.
    println!();
    if target.is_empty() {
        println!("{GREEN}{BOLD}{BULLET}{RESET} {BOLD}{TOOL_BLUE}{name}{RESET}");
    } else {
        println!(
            "{GREEN}{BOLD}{BULLET}{RESET} {BOLD}{TOOL_BLUE}{name}{RESET}({DIM}{target}{RESET})"
        );
    }
}

fn print_running_task_header(target: &str) {
    println!();
    if target.is_empty() {
        println!("{GREEN}{BOLD}{RUNNING_SQUARE}{RESET} {BOLD}{TOOL_BLUE}Task{RESET}");
    } else {
        println!(
            "{GREEN}{BOLD}{RUNNING_SQUARE}{RESET} {BOLD}{TOOL_BLUE}Task{RESET}({DIM}{target}{RESET})"
        );
    }
}

fn print_bash_header(command: &str) {
    println!();
    println!(
        "{GREEN}{BOLD}{BULLET}{RESET} {BOLD}{WHITE}Ran{RESET} {}",
        render_shell_command(command)
    );
}

fn print_corner_line(text: &str) {
    println!("  {DIM}└{RESET} {text}");
}

fn print_indent_line(text: &str) {
    println!("    {text}");
}

fn print_truncation(extra: usize) {
    println!("    {DIM}… +{extra} lines (ctrl+o to expand){RESET}");
}

fn print_task_inspect_hint(extra: usize) {
    println!("    {DIM}… +{extra} lines (/sub-agent to inspect subagent){RESET}");
}

fn tool_display_name(tool: &str) -> &str {
    match tool {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "apply_patch" | "patch" => "Patch",
        "grep" => "Grep",
        "glob" => "Glob",
        "webfetch" => "Fetch",
        "websearch" => "Search",
        "task" => "Task",
        "todowrite" => "TodoWrite",
        "question" => "Question",
        "plan_enter" => "Plan",
        "plan_exit" => "Build",
        other => other,
    }
}

fn render_generic_cell(tool: &str, input: &Value, output: &str) {
    let label = tool_display_name(tool);
    let target = tool_target(tool, input);
    print_tool_header(label, &target);
    let summary = match tool {
        "read" => non_empty_lines(output)
            .map(|count| format!("read {count} lines"))
            .unwrap_or_else(|| "no output".to_string()),
        "grep" | "glob" => non_empty_lines(output)
            .map(|count| format!("{count} matches"))
            .unwrap_or_else(|| "no matches".to_string()),
        "webfetch" => format!("{} chars", output.chars().count()),
        "websearch" => non_empty_lines(output)
            .map(|count| format!("{count} results"))
            .unwrap_or_else(|| "no results".to_string()),
        "todowrite" => "todos updated".to_string(),
        _ => {
            if let Some(first) = output.lines().find(|line| !line.trim().is_empty()) {
                truncate_for_terminal(first, 120)
            } else {
                String::new()
            }
        }
    };
    if !summary.is_empty() {
        print_corner_line(&format!("{DIM}{summary}{RESET}"));
    }
}

fn render_task_cell(
    input: &Value,
    output: &str,
    metadata: &Value,
) -> Option<TruncatedOutput> {
    let target = task_target(input);
    print_tool_header("Task", &target);
    let agent = metadata
        .get("agent")
        .and_then(Value::as_str)
        .or_else(|| input.get("subagent_type").and_then(Value::as_str))
        .or_else(|| input.get("agent").and_then(Value::as_str));
    let session_id = metadata.get("sessionId").and_then(Value::as_str);
    match (agent, session_id) {
        (Some(agent), Some(session_id)) => println!(
            "  {DIM}├{RESET} @{agent} · {}",
            terminal_link(&format!("neoism://session/{session_id}"), session_id)
        ),
        (Some(agent), None) => println!("  {DIM}├{RESET} @{agent}"),
        (None, Some(session_id)) => println!(
            "  {DIM}├{RESET} {}",
            terminal_link(&format!("neoism://session/{session_id}"), session_id)
        ),
        (None, None) => {}
    }
    let lines = task_result_lines(output);
    println!("  {DIM}├{RESET} {DIM}completed · returned to main agent{RESET}");
    let summary = lines
        .first()
        .map(|line| truncate_for_terminal(line, 120))
        .unwrap_or_default();
    if !summary.is_empty() {
        print_corner_line(&format!("{DIM}{summary}{RESET}"));
    }
    if lines.len() > 1 {
        print_task_inspect_hint(lines.len() - 1);
    }
    None
}

fn task_target(input: &Value) -> String {
    string_field(input, &["description", "command"])
        .map(|value| truncate_for_terminal(&value, 96))
        .unwrap_or_default()
}

fn task_result_lines(output: &str) -> Vec<String> {
    let result = output
        .split_once("<task_result>")
        .and_then(|(_, rest)| rest.split_once("</task_result>").map(|(result, _)| result))
        .unwrap_or(output)
        .trim();
    result
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("task_id:"))
        .map(ToOwned::to_owned)
        .collect()
}

fn terminal_link(uri: &str, label: &str) -> String {
    format!("\x1b]8;;{uri}\x1b\\{label}\x1b]8;;\x1b\\")
}

fn render_todo_cell(metadata: &Value) {
    let todos = metadata.get("todos").and_then(Value::as_array);
    let Some(todos) = todos else {
        print_tool_header("TodoWrite", "");
        print_corner_line(&format!("{DIM}(empty){RESET}"));
        return;
    };
    let total = todos.len();
    let done = todos
        .iter()
        .filter(|t| t.get("status").and_then(Value::as_str) == Some("completed"))
        .count();
    let summary = format!("{done}/{total} done");
    print_tool_header("TodoWrite", &summary);
    if todos.is_empty() {
        print_corner_line(&format!("{DIM}(no items){RESET}"));
        return;
    }
    for (index, todo) in todos.iter().enumerate() {
        let content = todo
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let status = todo
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let prefix = if index == 0 { "  └ " } else { "    " };
        match status {
            "completed" => {
                println!("{prefix}{DIM}[{GREEN}✓{RESET}{DIM}] {STRIKE}{content}{RESET}");
            }
            "in_progress" | "in-progress" | "active" => {
                println!(
                    "{prefix}{ORANGE}[{BOLD}•{RESET}{ORANGE}]{RESET} {BOLD}{ORANGE}{content}{RESET}"
                );
            }
            _ => {
                println!("{prefix}{DIM}[ ] {content}{RESET}");
            }
        }
    }
}

fn non_empty_lines(output: &str) -> Option<usize> {
    let count = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    if count == 0 {
        None
    } else {
        Some(count)
    }
}

pub(crate) fn tool_target(tool: &str, input: &Value) -> String {
    let keys = match tool {
        "read" | "write" | "edit" => &["filePath"][..],
        "grep" | "glob" => &["pattern", "path"][..],
        "bash" => &["command", "cmd"][..],
        "webfetch" => &["url"][..],
        "websearch" => &["query"][..],
        _ => &[
            "path",
            "filePath",
            "file_path",
            "file",
            "query",
            "pattern",
            "command",
        ][..],
    };
    for key in keys {
        if let Some(value) = input.get(*key).and_then(Value::as_str) {
            return truncate_for_terminal(value, 96);
        }
    }
    if let Some(value) = input.as_str() {
        return truncate_for_terminal(value, 96);
    }
    String::new()
}

fn render_bash_cell(input: &Value, output: &str) -> Option<TruncatedOutput> {
    let command = tool_target("bash", input);
    let command = if command.is_empty() {
        "command".to_string()
    } else {
        command
    };
    print_bash_header(&command);
    let lines: Vec<String> = output.lines().map(ToOwned::to_owned).collect();
    if lines.is_empty() {
        print_corner_line(&format!("{DIM}(no output){RESET}"));
        return None;
    }
    let max_lines = 8;
    for (index, line) in lines.iter().take(max_lines).enumerate() {
        let rendered = truncate_for_terminal(line, 140);
        if index == 0 {
            print_corner_line(&rendered);
        } else {
            print_indent_line(&rendered);
        }
    }
    if lines.len() > max_lines {
        print_truncation(lines.len() - max_lines);
        Some(TruncatedOutput {
            header: format!("Bash({command})"),
            lines,
        })
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShellTokenKind {
    Word,
    Whitespace,
    Operator,
}

fn render_shell_command(command: &str) -> String {
    let mut rendered = String::new();
    let mut expect_command = true;
    for (kind, token) in shell_tokens(command) {
        match kind {
            ShellTokenKind::Whitespace => rendered.push_str(&token),
            ShellTokenKind::Operator => {
                rendered.push_str(DIM);
                rendered.push_str(&token);
                rendered.push_str(RESET);
                if matches!(token.as_str(), "|" | "||" | "&&" | ";") {
                    expect_command = true;
                }
            }
            ShellTokenKind::Word => {
                if expect_command && !is_shell_assignment(&token) {
                    rendered.push_str(SHELL_COMMAND_BLUE);
                    rendered.push_str(&token);
                    rendered.push_str(RESET);
                    expect_command = false;
                } else if token.starts_with('-') {
                    rendered.push_str(RED);
                    rendered.push_str(&token);
                    rendered.push_str(RESET);
                    expect_command = false;
                } else {
                    rendered.push_str(&render_shell_operand(&token));
                    if !is_shell_assignment(&token) {
                        expect_command = false;
                    }
                }
            }
        }
    }
    rendered
}

fn shell_tokens(command: &str) -> Vec<(ShellTokenKind, String)> {
    let mut tokens = Vec::new();
    let chars = command.chars().collect::<Vec<_>>();
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            let start = index;
            while index < chars.len() && chars[index].is_whitespace() {
                index += 1;
            }
            tokens.push((
                ShellTokenKind::Whitespace,
                chars[start..index].iter().collect(),
            ));
            continue;
        }
        if matches!(ch, '|' | '&' | ';') {
            let start = index;
            index += 1;
            if index < chars.len()
                && ((ch == '|' && chars[index] == '|')
                    || (ch == '&' && chars[index] == '&'))
            {
                index += 1;
            }
            tokens.push((
                ShellTokenKind::Operator,
                chars[start..index].iter().collect(),
            ));
            continue;
        }
        let start = index;
        while index < chars.len()
            && !chars[index].is_whitespace()
            && !matches!(chars[index], '|' | '&' | ';')
        {
            if matches!(chars[index], '\'' | '"') {
                let quote = chars[index];
                index += 1;
                while index < chars.len() {
                    let current = chars[index];
                    index += 1;
                    if current == '\\' && index < chars.len() {
                        index += 1;
                        continue;
                    }
                    if current == quote {
                        break;
                    }
                }
            } else {
                index += 1;
            }
        }
        tokens.push((ShellTokenKind::Word, chars[start..index].iter().collect()));
    }
    tokens
}

fn is_shell_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !name.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

fn render_shell_operand(token: &str) -> String {
    if token.starts_with('"') || token.starts_with('\'') {
        return render_quoted_shell_operand(token);
    }
    highlight_shell_numbers(token, WHITE)
}

fn render_quoted_shell_operand(token: &str) -> String {
    let mut rendered = String::new();
    let mut chars = token.chars();
    if let Some(quote) = chars.next() {
        rendered.push_str(SHELL_STRING_YELLOW);
        rendered.push(quote);
        rendered.push_str(RESET);
        let rest = chars.collect::<String>();
        if rest.ends_with(quote) && rest.len() > quote.len_utf8() {
            let body = &rest[..rest.len() - quote.len_utf8()];
            rendered.push_str(&highlight_shell_numbers(body, SHELL_STRING_YELLOW));
            rendered.push_str(SHELL_STRING_YELLOW);
            rendered.push(quote);
            rendered.push_str(RESET);
        } else {
            rendered.push_str(&highlight_shell_numbers(&rest, SHELL_STRING_YELLOW));
        }
    }
    rendered
}

fn highlight_shell_numbers(token: &str, base_color: &str) -> String {
    let mut rendered = String::new();
    let mut chars = token.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            rendered.push_str(SHELL_NUMBER_GREEN);
            rendered.push(ch);
            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() || matches!(next, '.' | '_' | ',') {
                    rendered.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            rendered.push_str(RESET);
            rendered.push_str(base_color);
        } else {
            if rendered.is_empty() {
                rendered.push_str(base_color);
            }
            rendered.push(ch);
        }
    }
    if !rendered.is_empty() {
        rendered.push_str(RESET);
    }
    rendered
}

fn render_edit_cells(
    input: &Value,
    metadata: &Value,
    title: &str,
) -> Option<TruncatedOutput> {
    let snapshots = snapshot_diffs(metadata);
    if !snapshots.is_empty() {
        for snapshot in snapshots {
            print_edit_header("Edited", &snapshot.path, snapshot.added, snapshot.removed);
            render_diff_rows(&snapshot.path, &snapshot.rows);
            if snapshot.omitted > 0 {
                print_truncation(snapshot.omitted);
            }
        }
        return None;
    }

    if let Some(patch) = string_field(input, &["patchText", "patch", "diff", "content"]) {
        let fallback_path = patch_path_from_metadata(metadata)
            .or_else(|| tool_target_from_title(title))
            .unwrap_or_else(|| "patch".to_string());
        for section in patch_diff_sections(&patch, &fallback_path) {
            print_edit_header("Edited", &section.path, section.added, section.removed);
            render_diff_rows(&section.path, &section.rows);
            if section.omitted > 0 {
                print_truncation(section.omitted);
            }
        }
        return None;
    }

    let path = string_field(metadata, &["path"])
        .or_else(|| tool_target_from_title(title))
        .unwrap_or_else(|| title.to_string());
    print_tool_header("Update", &path);
    None
}

fn print_edit_header(action: &str, path: &str, added: usize, removed: usize) {
    println!();
    println!(
        "{DIM}{BULLET}{RESET} {BOLD}{action}{RESET} {path} ({GREEN}+{added}{RESET} {RED}-{removed}{RESET})"
    );
}

fn patch_path_from_metadata(metadata: &Value) -> Option<String> {
    metadata
        .get("paths")
        .and_then(Value::as_array)
        .and_then(|paths| paths.first())
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn tool_target_from_title(title: &str) -> Option<String> {
    title
        .split_once(' ')
        .map(|(_, rest)| rest.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_command_renderer_colors_commands_flags_and_numbers() {
        let rendered =
            render_shell_command("git -C opencode show HEAD:path | sed -n '1,260p'");

        assert!(rendered.contains(&format!("{SHELL_COMMAND_BLUE}git{RESET}")));
        assert!(rendered.contains(&format!("{RED}-C{RESET}")));
        assert!(rendered.contains(&format!("{SHELL_COMMAND_BLUE}sed{RESET}")));
        assert!(rendered.contains(&format!("{RED}-n{RESET}")));
        assert!(rendered.contains(&format!("{SHELL_NUMBER_GREEN}1,260{RESET}")));
    }

    #[test]
    fn read_target_prefers_file_path_schema_key() {
        let input =
            serde_json::json!({ "filePath": "crates/neoism-agent-server/src/tool.rs" });

        assert_eq!(
            tool_target("read", &input),
            "crates/neoism-agent-server/src/tool.rs"
        );
    }
}
