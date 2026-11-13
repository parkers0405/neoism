//! Compact context-gathering runs, without losing the original calls or transcript boundaries.

use neoism_protocol::agent::{HistoryMessage, HistoryMessageKind, HistoryOutputKind};

/// Show the live turn's trace; settled turns retain messages and explicit user content.
pub fn visible(message: &HistoryMessage, index: usize, live_from: usize) -> bool {
    if message.kind == HistoryMessageKind::Reasoning && message.text.trim().is_empty()
        && matches!(message.title.trim(), "" | "Thinking") { return false; }
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
    let end = group_end(messages, start)?;
    let calls: Vec<_> = messages[start..end].iter().filter(|m| groupable_read(m)).cloned().collect();
    Some((end, read_group(&calls)))
}

/// Row-list discovery only checks metadata, never parses output or diff payloads.
pub fn group_end(messages: &[HistoryMessage], start: usize) -> Option<usize> {
    messages.get(start).filter(|m| groupable_read(m))?;
    let mut end = start + 1;
    for (index, message) in messages.iter().enumerate().skip(start + 1) {
        if groupable_read(message) {
            end = index + 1;
        } else if !visible(message, index, 0)
            || (message.kind == HistoryMessageKind::Assistant && message.text.trim().is_empty() && message.attachments.is_empty()) {
            continue;
        } else { break; }
    }
    Some(end)
}

/// Count calls, not matching files. Original messages remain available to child disclosures.
pub fn read_group(tools: &[HistoryMessage]) -> HistoryMessage {
    let (mut reads, mut searches, mut lists) = (0, 0, 0);
    for tool in tools {
        match read_family(&tool.tool) {
            Some("Read") => reads += 1,
            Some("List") => lists += 1,
            _ => searches += 1,
        }
    }
    let counts = [(reads, "read", "reads"), (searches, "search", "searches"), (lists, "list", "lists")]
        .into_iter().filter(|(n, ..)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
        .collect::<Vec<_>>().join(", ");
    let settled = tools.iter().all(|t| matches!(t.status.trim().to_ascii_lowercase().as_str(), "completed" | "success" | "done"));
    let title = format!("{} context · {counts}", if settled { "Gathered" } else { "Gathering" });
    let first = &tools[0];
    let mut result = synthetic(first, format!("{}..", first.id), title, String::new(), if settled { "completed" } else { "running" }, "tool_group", String::new());
    result.composition_children = tools.to_vec();
    result
}

fn groupable_read(m: &HistoryMessage) -> bool {
    m.kind == HistoryMessageKind::Tool
        && !is_todos(m)
        && m.todos.is_empty()
        && m.attachments.is_empty()
        && m.composition_children.is_empty()
        && visible(m, 0, 0)
        && matches!(m.status.trim().to_ascii_lowercase().as_str(), "pending" | "running" | "streaming" | "completed" | "success" | "done")
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

pub(super) fn read_label(m: &HistoryMessage) -> String {
    let title = m.title.trim();
    if title.is_empty() {
        return m.tool.clone();
    }
    let target = title.split_once('(').and_then(|(_, target)| target.strip_suffix(')')).unwrap_or(title).replace(['\r', '\n', '\t'], " ");
    format!("{} · {target}", read_family(&m.tool).unwrap_or("Tool"))
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
    fn empty_streaming_parts_do_not_split_context_calls() {
        let mut empty = tool("empty", "", "", "");
        empty.kind = HistoryMessageKind::Assistant;
        let mut thinking = empty.clone();
        thinking.kind = HistoryMessageKind::Reasoning;
        thinking.title = "Thinking".into();
        let calls = vec![tool("a", "read", "Read(a)", ""), empty,
            tool("b", "grep", "Grep(b)", ""), thinking, tool("c", "read", "Read(c)", "")];
        let (end, group) = group_at(&calls, 0).unwrap();
        assert_eq!(end, 5);
        assert_eq!(group.title, "Gathered context · 2 reads, 1 search");
        assert_eq!(crate::agent::timeline::row_items(&calls, 0, false), vec![crate::agent::timeline::RowItem::Group(0, 5)]);
    }

    #[test]
    fn mixed_context_calls_fold_across_batches_without_deduplicating_calls() {
        let messages = vec![
            tool("a", "read", "Read(src/a.rs)", "b1"),
            tool("b", "glob", "Glob(**/*.rs)", "b2"),
            tool("c", "grep", "Grep(symbol)", ""),
            tool("d", "list", "List(src)", "b3"),
            tool("e", "search", "Search(docs)", ""),
            tool("f", "read", "Read(src/a.rs)", "b4"),
        ];
        let (end, group) = group_at(&messages, 0).unwrap();
        assert_eq!(end, 6);
        assert_eq!(group.title, "Gathered context · 2 reads, 3 searches, 1 list");
        assert_eq!(group.composition_children.len(), messages.len());
        assert_eq!(group.id, "a..");
        assert_eq!(group_end(&messages, 0), Some(end));
        assert_eq!(read_label(&group.composition_children[1]), "Glob · **/*.rs");
        assert_eq!(crate::agent::timeline::row_items(&messages, 0, false), vec![
            crate::agent::timeline::RowItem::Group(0, 6),
        ]);
    }

    #[test]
    fn patches_stay_individual_and_preserve_payloads() {
        let mut messages = vec![tool("a", "apply_patch", "Edit a", ""), tool("b", "apply_patch", "Edit b", "")];
        for message in &mut messages {
            message.detail = format!("*** Begin Patch\n*** Update File: {}.rs\n@@\n-old\n+new\n*** End Patch", message.id);
        }
        assert_eq!(crate::agent::timeline::row_items(&messages, 0, false), vec![
            crate::agent::timeline::RowItem::Message(0), crate::agent::timeline::RowItem::Message(1),
        ]);
        assert_eq!(group_end(&messages, 0), None);
        assert!(group_at(&messages, 0).is_none());
        messages[1].status = "running".into();
        assert_eq!(group_end(&messages, 0), None);
        assert!(group_at(&messages, 0).is_none());
        messages[1].status = "completed".into();
        messages[1].detail.clear();
        assert_eq!(group_end(&messages, 0), None);
        assert!(group_at(&messages, 0).is_none());
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
        for kind in [HistoryMessageKind::Reasoning, HistoryMessageKind::Subtask, HistoryMessageKind::Tool] {
            let message = HistoryMessage { kind, ..read.clone() };
            assert!(!visible(&message, 0, usize::MAX));
            assert!(visible(&message, 0, 0));
        }
        todos.todos.clear();
        assert!(!visible(&todos, 0, 5));
    }

    #[test]
    fn internal_entries_remain_hidden_without_splitting_context_runs() {
        for (id, name, text) in [
            ("todos-snapshot", "read", ""),
            ("background-task-1", "background_task_result", ""),
            ("x", "compaction_reasoning", ""),
            ("x", "compaction_summary", ""),
            ("x", "read", "Subagent finished. Internal summary"),
        ] {
            let mut hidden = tool(id, name, "Internal", "");
            hidden.text = text.into();
            assert!(!visible(&hidden, 0, 0));
            let messages = vec![tool("a", "read", "Read(a)", ""), hidden, tool("b", "read", "Read(b)", "")];
            assert_eq!(group_end(&messages, 0), Some(3));
            assert_eq!(group_at(&messages, 0).unwrap().1.composition_children.len(), 2);
        }
    }

    #[test]
    fn errors_and_non_context_boundaries_are_never_absorbed() {
        let read = tool("a", "read", "Read(a)", "");
        let mut boundaries = vec![tool("shell", "bash", "Bash(pwd)", ""), tool("patch", "apply_patch", "Edit", "")];
        for status in ["error", "failed", "stopped", "aborted", "interrupted", "cancelled", "canceled"] {
            boundaries.push(HistoryMessage { status: status.into(), ..read.clone() });
        }
        for kind in [HistoryMessageKind::Assistant, HistoryMessageKind::User, HistoryMessageKind::Reasoning] {
            boundaries.push(HistoryMessage { kind, text: "A meaningful transcript boundary".into(), ..read.clone() });
        }
        boundaries.push(HistoryMessage { composition_children: vec![read.clone()], ..read.clone() });
        for boundary in boundaries {
            let messages = vec![read.clone(), boundary, read.clone()];
            assert_eq!(group_end(&messages, 0), Some(1));
            assert!(group_at(&messages, 1).is_none());
            assert_eq!(group_end(&messages, 2), Some(3));
        }
    }

    #[test]
    fn active_context_preserves_original_output_status_and_identity() {
        let mut read = tool("a", "read", "Read(a)", "one");
        read.text = "summary".into();
        read.detail = "first\nsecond\tfull output".into();
        let mut grep = tool("b", "grep", "Grep(needle)", "two");
        grep.status = "running".into();
        let (_, group) = group_at(&[read.clone(), grep.clone()], 0).unwrap();
        assert_eq!(group.status, "running");
        assert_eq!(group.title, "Gathering context · 1 read, 1 search");
        for (original, child) in [read, grep].iter().zip(&group.composition_children) {
            assert_eq!(serde_json::to_value(original).unwrap(), serde_json::to_value(child).unwrap());
        }
    }
}
