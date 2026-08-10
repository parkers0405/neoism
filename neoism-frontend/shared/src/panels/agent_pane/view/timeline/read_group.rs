use super::*;

pub(crate) fn read_tool_group_at<M: AgentTimelineMessage>(
    messages: &[M],
    start_index: usize,
) -> Option<(usize, M)> {
    if !messages
        .get(start_index)
        .is_some_and(is_live_groupable_read_tool)
    {
        return None;
    }
    let mut end = start_index;
    while messages.get(end).is_some_and(is_live_groupable_read_tool) {
        end += 1;
    }
    if end.saturating_sub(start_index) < LIVE_READ_TOOL_GROUP_MIN {
        return None;
    }
    Some((
        end,
        live_read_tool_group_message(&messages[start_index..end]),
    ))
}

fn live_read_tool_group_message<M: AgentTimelineMessage>(tools: &[M]) -> M {
    let mut preview = String::new();
    let mut preview_count = 0;
    let mut detail = String::new();
    for tool in tools.iter().take(4) {
        let label = read_tool_activity_label(tool);
        if !preview.is_empty() {
            preview.push('\n');
        }
        preview.push_str(&label);
        preview_count += 1;

        let source = if tool.detail().trim().is_empty() {
            tool.text()
        } else {
            tool.detail()
        };
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(&label);
        detail.push('\t');
        detail.push_str(&bounded_tool_group_preview(source));
    }
    if tools.len() > preview_count {
        if !preview.is_empty() {
            preview.push('\n');
        }
        preview.push_str(&format!("+{} more", tools.len() - preview_count));
    }

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
    let last_id = tools.last().map(|tool| tool.id()).unwrap_or("tools");
    M::tool_group_message(
        format!("{first_id}..{last_id}"),
        format!("Reading/searching {} items", tools.len()),
        preview,
        status.to_string(),
        detail,
    )
}

fn bounded_tool_group_preview(text: &str) -> String {
    let mut out = String::new();
    for line in text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .take(4)
    {
        if !out.is_empty() {
            out.push_str("  /  ");
        }
        out.push_str(line);
    }
    if out.is_empty() {
        out = "No output".to_string();
    }
    truncate_chars(&out.replace('\t', " "), 420)
}

fn is_live_groupable_read_tool<M: AgentTimelineMessage>(message: &M) -> bool {
    message.kind() == AgentTimelineMessageKind::Tool
        && message.output_kind() == AgentTimelineOutputKind::Text
        && message.todos_empty()
        && is_groupable_tool_status(message.status())
        && is_read_like_tool(message.tool())
}

fn is_groupable_tool_status(status: &str) -> bool {
    !matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "error" | "failed" | "stopped" | "aborted"
    )
}

fn is_read_like_tool(tool: &str) -> bool {
    let normalized = tool
        .chars()
        .filter(|ch| !matches!(ch, '_' | '-' | '.'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "read"
            | "readfile"
            | "view"
            | "cat"
            | "grep"
            | "ripgrep"
            | "rg"
            | "glob"
            | "list"
            | "ls"
            | "find"
            | "search"
            | "multigrep"
            | "toolgroup"
    )
}

fn read_tool_label<M: AgentTimelineMessage>(message: &M) -> String {
    let title = message.title().trim();
    if title.is_empty() {
        message.tool().to_string()
    } else {
        title.to_string()
    }
}

fn read_tool_activity_label<M: AgentTimelineMessage>(message: &M) -> String {
    let status = match message.status().trim().to_ascii_lowercase().as_str() {
        "completed" | "success" | "done" => "done",
        "error" | "failed" | "stopped" | "aborted" => "error",
        "" => "running",
        _ => "running",
    };
    format!("{status}  {}", read_tool_label(message))
}
