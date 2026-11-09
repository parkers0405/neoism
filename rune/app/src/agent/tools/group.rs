//! Which tool rows the transcript shows, and how runs of them fold into one row: consecutive
//! reads/searches of one batch become a `Read: 3 files` group, two or more settled edits become a
//! `Patched files · N calls` stack. Ported from `timeline/{layout,read_group,patch_group}.rs`.

use super::diff;
use neoism_protocol::agent::{HistoryMessage, HistoryMessageKind, HistoryOutputKind};
use std::collections::HashSet;

/// Settled turns (before `live_from`) show prompts and answers only; the live turn shows its
/// whole trace. Checklists and direct shell commands are content and always stay.
pub fn visible(message: &HistoryMessage, index: usize, live_from: usize) -> bool {
    let tool = message.tool.as_str();
    if message.text.trim_start().starts_with("Subagent finished.")
        || (tool == "background_task_result" && message.id.starts_with("background-task-"))
        || message.id == "todos-snapshot"
        || matches!(tool, "compaction_reasoning" | "compaction_summary")
    {
        return false;
    }
    if message.kind == HistoryMessageKind::Tool && tool == "user_shell" {
        return true;
    }
    if is_todos(message) {
        return !message.todos.is_empty();
    }
    match message.kind {
        HistoryMessageKind::Compaction | HistoryMessageKind::User | HistoryMessageKind::Assistant => true,
        HistoryMessageKind::System => tool == "location_notice",
        HistoryMessageKind::Reasoning | HistoryMessageKind::Tool | HistoryMessageKind::Subtask => index >= live_from,
    }
}

pub fn is_todos(message: &HistoryMessage) -> bool {
    message.kind == HistoryMessageKind::Tool
        && (message.output_kind == Some(HistoryOutputKind::Todos) || (message.output_kind.is_none() && !message.todos.is_empty()))
}

/// The row starting at `start` when it folds a run: `(end exclusive, synthetic message)`.
pub fn group_at(messages: &[HistoryMessage], start: usize) -> Option<(usize, HistoryMessage)> {
    patch_stack_at(messages, start).or_else(|| read_group_at(messages, start))
}

fn patch_stack_at(messages: &[HistoryMessage], start: usize) -> Option<(usize, HistoryMessage)> {
    let mut end = start;
    while messages.get(end).is_some_and(|m| {
        m.kind == HistoryMessageKind::Tool
            && diff::is_edit_tool(&m.tool)
            && matches!(m.status.as_str(), "completed" | "success" | "done")
            && !is_todos(m)
            && !diff::sections(&m.tool, &m.status, &m.title, &m.text, &m.detail).is_empty()
    }) {
        end += 1;
    }
    if end < start + 2 {
        return None;
    }
    let calls: Vec<_> = messages[start..end].iter().map(|m| serde_json::json!({
        "id": m.id, "title": m.title, "text": m.text, "tool": m.tool, "detail": m.detail,
    })).collect();
    let first = &messages[start];
    let detail = serde_json::json!({"neoismToolDetail": "patchStack", "calls": calls}).to_string();
    Some((end, synthetic(first, first.id.clone(), format!("Patched files \u{b7} {} calls", end - start), String::new(), "completed", "apply_patch", detail)))
}

fn read_group_at(messages: &[HistoryMessage], start: usize) -> Option<(usize, HistoryMessage)> {
    let first = messages.get(start).filter(|m| groupable_read(m))?;
    let batch = first.tool_batch_id.as_deref().filter(|b| !b.trim().is_empty());
    let family = read_family(&first.tool);
    let mut end = start;
    while messages.get(end).is_some_and(|m| {
        groupable_read(m) && read_family(&m.tool) == family && m.tool_batch_id.as_deref().filter(|b| !b.trim().is_empty()) == batch
    }) {
        end += 1;
    }
    Some((end, read_group(&messages[start..end])))
}

/// A run of reads/searches as one message (also used for a lone read inside a composition).
pub fn read_group(tools: &[HistoryMessage]) -> HistoryMessage {
    let family = read_family(&tools[0].tool).unwrap_or("Search");
    let mut labels = Vec::new();
    let mut seen = HashSet::new();
    let mut file_results = family == "Read";
    let clean = |s: &str| s.replace(['\r', '\n', '\t'], " ");
    for tool in tools {
        match family {
            "Grep" => {
                let mut native = false;
                for line in tool.detail.lines() {
                    if line.starts_with("Grep:") {
                        native = true;
                        file_results = true;
                    } else if native && !line.starts_with(char::is_whitespace) {
                        if let Some(path) = line.strip_suffix(':').filter(|p| !p.is_empty()) {
                            if seen.insert(path.to_owned()) { labels.push(clean(path)); }
                        }
                    }
                }
            }
            "Glob" => {
                let mut lines = tool.detail.lines();
                if lines.next().is_some_and(|l| l.starts_with("Glob ")) {
                    file_results = true;
                    for path in lines.filter(|l| !l.is_empty() && *l != "No files found") {
                        if seen.insert(path.to_owned()) { labels.push(clean(path)); }
                    }
                }
            }
            _ => {
                let label = read_label(tool);
                if seen.insert(label.clone()) { labels.push(label); }
            }
        }
    }
    if matches!(family, "Grep" | "Glob") && !file_results {
        labels = tools.iter().map(read_label).collect();
    }
    let count = if file_results { labels.len() } else { tools.len() };
    let unit = if file_results { "file" } else { "search" };
    let title = format!("{family}: {count} {unit}{}", if count == 1 { "" } else { "s" });
    if labels.is_empty() { labels.push("No matching files".into()); }
    let settled = tools.iter().all(|t| matches!(t.status.trim().to_ascii_lowercase().as_str(), "completed" | "success" | "done"));
    let first = &tools[0];
    synthetic(first, format!("{}..", first.id), title, labels.join("\n"), if settled { "completed" } else { "running" }, "tool_group", String::new())
}

fn groupable_read(m: &HistoryMessage) -> bool {
    m.kind == HistoryMessageKind::Tool
        && !is_todos(m)
        && m.todos.is_empty()
        && m.attachments.is_empty()
        && !matches!(m.status.trim().to_ascii_lowercase().as_str(), "error" | "failed" | "stopped" | "aborted")
        && read_family(&m.tool).is_some()
}

pub fn read_family(tool: &str) -> Option<&'static str> {
    let normalized: String = tool.chars().filter(|c| !matches!(c, '_' | '-' | '.')).flat_map(char::to_lowercase).collect();
    match normalized.as_str() {
        "read" | "readfile" | "view" | "cat" => Some("Read"),
        "grep" | "ripgrep" | "rg" | "multigrep" => Some("Grep"),
        "glob" | "find" | "ffffindfiles" => Some("Glob"),
        "list" | "ls" => Some("List"),
        "search" => Some("Search"),
        _ => None,
    }
}

fn read_label(m: &HistoryMessage) -> String {
    let title = m.title.trim();
    if title.is_empty() {
        return m.tool.clone();
    }
    title.split_once('(').and_then(|(_, target)| target.strip_suffix(')')).unwrap_or(title).replace(['\r', '\n', '\t'], " ")
}

fn synthetic(base: &HistoryMessage, id: String, title: String, text: String, status: &str, tool: &str, detail: String) -> HistoryMessage {
    HistoryMessage {
        id, title, text, detail,
        status: status.into(),
        tool: tool.into(),
        kind: HistoryMessageKind::Tool,
        output_kind: Some(HistoryOutputKind::Text),
        composition_children: Vec::new(),
        todos: Vec::new(),
        attachments: Vec::new(),
        mcp: None,
        agent_kind: None,
        ..base.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, tool: &str, title: &str, batch: &str) -> HistoryMessage {
        serde_json::from_value(serde_json::json!({
            "id": id, "role": "Assistant", "kind": "Tool", "title": title, "text": "", "status": "completed",
            "tool": tool, "tool_batch_id": batch, "detail": "",
        })).unwrap()
    }

    #[test]
    fn reads_of_one_batch_fold_into_a_file_count() {
        let messages = vec![tool("a", "read", "Read(src/a.rs)", "b1"), tool("b", "read", "Read(src/b.rs)", "b1"), tool("c", "read", "Read(src/c.rs)", "b2")];
        let (end, group) = group_at(&messages, 0).unwrap();
        assert_eq!(end, 2);
        assert_eq!(group.title, "Read: 2 files");
        assert_eq!(group.text, "src/a.rs\nsrc/b.rs");
        assert_eq!(group.id, "a..");
    }

    #[test]
    fn settled_turns_hide_their_trace_but_keep_checklists() {
        let read = tool("a", "read", "Read(x)", "b");
        assert!(!visible(&read, 0, 5));
        assert!(visible(&read, 5, 5));
        let mut todos = tool("t", "todowrite", "TodoWrite", "b");
        todos.output_kind = Some(HistoryOutputKind::Todos);
        todos.todos = vec![serde_json::from_value(serde_json::json!({"status": "pending", "content": "x"})).unwrap()];
        assert!(visible(&todos, 0, 5));
    }
}
