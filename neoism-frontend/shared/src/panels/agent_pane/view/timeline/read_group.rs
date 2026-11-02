use super::*;

pub(crate) fn read_tool_group_at<M: AgentTimelineMessage>(
    messages: &[M],
    start_index: usize,
) -> Option<(usize, M)> {
    let end = read_tool_group_end(messages, start_index)?;
    Some((
        end,
        live_read_tool_group_message(&messages[start_index..end]),
    ))
}

/// One operation family per explicit batch, or adjacent unbatched activity.
pub fn read_tool_group_end<M: AgentTimelineMessage>(
    messages: &[M],
    start_index: usize,
) -> Option<usize> {
    let first = messages.get(start_index)?;
    if !is_live_groupable_read_tool(first) {
        return None;
    }
    let batch_id = first.tool_batch_id().filter(|id| !id.trim().is_empty());
    let family = read_tool_family(first.tool());
    let mut end = start_index;
    while messages.get(end).is_some_and(|message| {
        is_live_groupable_read_tool(message)
            && read_tool_family(message.tool()) == family
            && message.tool_batch_id().filter(|id| !id.trim().is_empty()) == batch_id
    }) {
        end += 1;
    }
    Some(end)
}

fn live_read_tool_group_message<M: AgentTimelineMessage>(tools: &[M]) -> M {
    let family = read_tool_family(tools[0].tool()).unwrap_or("Search");
    let mut labels = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut file_results = family == "Read";
    for tool in tools {
        if family == "Grep" {
            // Native grep marks file headers separately from indented matches.
            // Retain paths only, never matched source or context lines.
            let mut native_output = false;
            for line in tool.detail().lines() {
                if line.starts_with("Grep:") {
                    native_output = true;
                    file_results = true;
                } else if native_output && !line.starts_with(char::is_whitespace) {
                    if let Some(path) = line.strip_suffix(':').filter(|path| !path.is_empty()) {
                        if seen.insert(path.to_string()) {
                            labels.push(path.replace(['\r', '\n', '\t'], " "));
                        }
                    }
                }
            }
        }
        if family == "Glob" {
            let mut lines = tool.detail().lines();
            if lines.next().is_some_and(|line| line.starts_with("Glob ")) {
                file_results = true;
                for path in lines.filter(|line| !line.is_empty() && *line != "No files found") {
                    if seen.insert(path.to_string()) {
                        labels.push(path.replace(['\r', '\n', '\t'], " "));
                    }
                }
            }
        }
        if !matches!(family, "Grep" | "Glob") {
            let label = read_tool_label(tool);
            if seen.insert(label.clone()) { labels.push(label); }
        }
    }
    if matches!(family, "Grep" | "Glob") && !file_results {
        labels = tools.iter().map(read_tool_label).collect();
    }
    let count = if file_results { labels.len() } else { tools.len() };
    let unit = if file_results { "file" } else { "search" };
    let title = format!("{family}: {count} {unit}{}", if count == 1 { "" } else { "s" });
    if labels.is_empty() { labels.push("No matching files".to_string()); }

    let status = if tools.iter().any(|tool| {
        !matches!(
            tool.status().trim().to_ascii_lowercase().as_str(),
            "completed" | "success" | "done"
        )
    }) {
        "running"
    } else {
        "completed"
    };
    let first_id = tools.first().map(|tool| tool.id()).unwrap_or("tools");
    // The first member owns the group identity even as the live run grows.
    M::tool_group_message(
        format!("{first_id}.."),
        title,
        labels.join("\n"),
        status.to_string(),
        String::new(),
    )
}

fn is_live_groupable_read_tool<M: AgentTimelineMessage>(message: &M) -> bool {
    message.kind() == AgentTimelineMessageKind::Tool
        && message.output_kind() != AgentTimelineOutputKind::Todos
        && message.todos_empty()
        && message.images().is_empty()
        && is_groupable_tool_status(message.status())
        && read_tool_family(message.tool()).is_some()
}

fn is_groupable_tool_status(status: &str) -> bool {
    !matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "error" | "failed" | "stopped" | "aborted"
    )
}

fn read_tool_family(tool: &str) -> Option<&'static str> {
    let normalized = tool
        .chars()
        .filter(|ch| !matches!(ch, '_' | '-' | '.'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    match normalized.as_str() {
        "read" | "readfile" | "view" | "cat" => Some("Read"),
        "grep" | "ripgrep" | "rg" | "multigrep" => Some("Grep"),
        "glob" | "find" | "ffffindfiles" => Some("Glob"),
        "list" | "ls" => Some("List"),
        "search" => Some("Search"),
        _ => None,
    }
}

fn read_tool_label<M: AgentTimelineMessage>(message: &M) -> String {
    let title = message.title().trim();
    if title.is_empty() {
        message.tool().to_string()
    } else {
        title.split_once('(')
            .and_then(|(_, target)| target.strip_suffix(')'))
            .unwrap_or(title)
            .replace(['\r', '\n', '\t'], " ")
    }
}
