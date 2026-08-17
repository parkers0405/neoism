#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentMessageKindPolicy {
    User,
    Assistant,
    Reasoning,
    Tool,
    System,
    Subtask,
    Compaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentMessageShell {
    pub title: &'static str,
    pub status: &'static str,
    pub tool: &'static str,
    pub lang: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentNoticeLevelPolicy {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemMessageAction {
    Notice {
        message: String,
        level: AgentNoticeLevelPolicy,
    },
    Dialog {
        title: String,
        body: String,
    },
}

pub fn should_queue_notice(message: &str) -> bool {
    !message.trim().is_empty()
}

pub fn should_queue_dialog(body: &str) -> bool {
    !body.trim().is_empty()
}

pub fn copied_notice_message(char_count: usize) -> String {
    if char_count == 1 {
        "Copied 1 char to clipboard".to_string()
    } else {
        format!("Copied {char_count} chars to clipboard")
    }
}

pub fn message_shell(kind: AgentMessageKindPolicy) -> AgentMessageShell {
    AgentMessageShell {
        title: match kind {
            AgentMessageKindPolicy::Reasoning => "Thinking",
            AgentMessageKindPolicy::Compaction => "Compaction",
            _ => "",
        },
        status: "",
        tool: "",
        lang: "",
    }
}

pub fn plan_system_message(
    title: impl AsRef<str>,
    body: impl Into<String>,
) -> SystemMessageAction {
    let title = title.as_ref();
    let body = body.into();
    let title = if title.is_empty() { "System" } else { title };
    if body.contains('\n') || body.chars().count() > 140 {
        return SystemMessageAction::Dialog {
            title: title.to_string(),
            body,
        };
    }
    let level = if title.to_ascii_lowercase().contains("failed") {
        AgentNoticeLevelPolicy::Error
    } else if body.starts_with("no ") || body.starts_with("usage:") {
        AgentNoticeLevelPolicy::Warn
    } else {
        AgentNoticeLevelPolicy::Info
    };
    SystemMessageAction::Notice {
        message: format!("{title}: {body}"),
        level,
    }
}

pub fn same_message_identity(
    a_kind: AgentMessageKindPolicy,
    a_id: &str,
    a_title: &str,
    a_text: &str,
    b_kind: AgentMessageKindPolicy,
    b_id: &str,
    b_title: &str,
    b_text: &str,
) -> bool {
    // User messages are keyed by their text. The locally-pushed copy has an
    // empty id; the server's refresh assigns an id later. Without this special
    // case the user prompt loses its prior_index slot and sorts past the
    // assistant reply on idle.
    if a_kind == AgentMessageKindPolicy::User && b_kind == AgentMessageKindPolicy::User {
        return a_text.trim() == b_text.trim();
    }
    if !a_id.is_empty() || !b_id.is_empty() {
        return !a_id.is_empty() && a_id == b_id;
    }
    a_kind == b_kind && a_title == b_title && a_text == b_text
}

pub fn same_nonempty_id(a_id: &str, b_id: &str) -> bool {
    !a_id.is_empty() && a_id == b_id
}

pub fn is_streamed_live_part(kind: AgentMessageKindPolicy) -> bool {
    matches!(
        kind,
        AgentMessageKindPolicy::Assistant
            | AgentMessageKindPolicy::Reasoning
            | AgentMessageKindPolicy::Tool
            | AgentMessageKindPolicy::Subtask
    )
}

pub fn part_delta_message_kind(kind: Option<&str>) -> AgentMessageKindPolicy {
    match kind {
        Some("reasoning" | "thinking") => AgentMessageKindPolicy::Reasoning,
        _ => AgentMessageKindPolicy::Assistant,
    }
}

pub fn is_user_prompt(kind: AgentMessageKindPolicy, text: &str, prompt: &str) -> bool {
    kind == AgentMessageKindPolicy::User && text.trim() == prompt.trim()
}

pub fn preserve_streamed_text(
    existing_kind: AgentMessageKindPolicy,
    existing_text: &str,
    incoming_kind: AgentMessageKindPolicy,
    incoming_text: &str,
) -> bool {
    matches!(
        incoming_kind,
        AgentMessageKindPolicy::Assistant | AgentMessageKindPolicy::Reasoning
    ) && matches!(
        existing_kind,
        AgentMessageKindPolicy::Assistant | AgentMessageKindPolicy::Reasoning
    ) && (incoming_text.is_empty() || existing_text.starts_with(incoming_text))
}

pub fn should_merge_tool_parts(
    existing_kind: AgentMessageKindPolicy,
    incoming_kind: AgentMessageKindPolicy,
) -> bool {
    incoming_kind == AgentMessageKindPolicy::Tool
        && existing_kind == AgentMessageKindPolicy::Tool
}

pub fn task_id_from_text(detail: &str, text: &str) -> Option<String> {
    detail.lines().chain(text.lines()).find_map(|line| {
        line.trim()
            .strip_prefix("task_id:")
            .and_then(|rest| rest.split_whitespace().next())
            .map(str::to_string)
    })
}

pub fn is_task_message_for_id(
    kind: AgentMessageKindPolicy,
    tool: &str,
    text: &str,
    detail: &str,
    task_id: &str,
) -> bool {
    kind == AgentMessageKindPolicy::Tool
        && tool == "task"
        && (text.contains(task_id) || detail.contains(task_id))
}

pub fn normalize_task_message_status(status: &str) -> &'static str {
    match status {
        "completed" => "completed",
        "error" => "error",
        "running" => "running",
        "stopped" | "failed" => "error",
        _ => "running",
    }
}

pub fn apply_task_message_status(
    status_field: &mut String,
    text: &mut String,
    detail: &mut String,
    status: &str,
) -> bool {
    let normalized = normalize_task_message_status(status);
    let before_status = status_field.clone();
    let before_text = text.clone();
    let before_detail = detail.clone();

    *status_field = normalized.to_string();
    rewrite_task_status_marker(text, normalized);
    rewrite_task_status_marker(detail, normalized);
    rewrite_stale_task_running_explanation(text, normalized);
    rewrite_stale_task_running_explanation(detail, normalized);

    *status_field != before_status || *text != before_text || *detail != before_detail
}

fn rewrite_stale_task_running_explanation(field: &mut String, normalized: &str) {
    if !matches!(normalized, "completed" | "error") {
        return;
    }

    let Some(start) = field
        .lines()
        .scan(0usize, |offset, line| {
            let line_start = *offset;
            *offset += line.len() + 1;
            Some((line_start, line))
        })
        .find_map(|(line_start, line)| {
            let lower = line.to_ascii_lowercase();
            (lower.contains("subagent is running")
                || lower.contains("subagent is still running"))
            .then_some(line_start)
        })
    else {
        return;
    };

    field.truncate(start);
    *field = field.trim_end().to_string();
    field.push_str("\n\nThe subagent is no longer running.");
}

fn rewrite_task_status_marker(field: &mut String, normalized: &str) {
    for marker in [
        "status: running",
        "status: completed",
        "status: error",
        "status: stopped",
        "status: failed",
    ] {
        if field.contains(marker) {
            *field = field.replace(marker, &format!("status: {normalized}"));
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_identity_uses_prompt_text_until_server_id_arrives() {
        assert!(same_message_identity(
            AgentMessageKindPolicy::User,
            "",
            "",
            "hello",
            AgentMessageKindPolicy::User,
            "server-id",
            "",
            " hello ",
        ));
        assert!(!same_message_identity(
            AgentMessageKindPolicy::User,
            "",
            "",
            "hello",
            AgentMessageKindPolicy::User,
            "server-id",
            "",
            "other",
        ));
    }

    #[test]
    fn streamed_and_delta_kind_policy_matches_agent_timeline() {
        assert!(is_streamed_live_part(AgentMessageKindPolicy::Assistant));
        assert!(is_streamed_live_part(AgentMessageKindPolicy::Subtask));
        assert!(!is_streamed_live_part(AgentMessageKindPolicy::System));
        assert_eq!(
            part_delta_message_kind(Some("thinking")),
            AgentMessageKindPolicy::Reasoning
        );
        assert_eq!(
            part_delta_message_kind(Some("text")),
            AgentMessageKindPolicy::Assistant
        );
    }

    #[test]
    fn message_shell_carries_shared_constructor_defaults() {
        assert_eq!(
            message_shell(AgentMessageKindPolicy::Reasoning).title,
            "Thinking"
        );
        assert_eq!(
            message_shell(AgentMessageKindPolicy::Compaction).title,
            "Compaction"
        );
        assert_eq!(message_shell(AgentMessageKindPolicy::User).title, "");
        assert_eq!(message_shell(AgentMessageKindPolicy::Tool).status, "");
    }

    #[test]
    fn system_message_policy_routes_dialogs_and_notice_levels() {
        assert_eq!(
            plan_system_message("", "hello"),
            SystemMessageAction::Notice {
                message: "System: hello".to_string(),
                level: AgentNoticeLevelPolicy::Info,
            }
        );
        assert_eq!(
            plan_system_message("Command failed", "bad request"),
            SystemMessageAction::Notice {
                message: "Command failed: bad request".to_string(),
                level: AgentNoticeLevelPolicy::Error,
            }
        );
        assert_eq!(
            plan_system_message("Questions", "usage: /answer <text>"),
            SystemMessageAction::Notice {
                message: "Questions: usage: /answer <text>".to_string(),
                level: AgentNoticeLevelPolicy::Warn,
            }
        );
        assert_eq!(
            plan_system_message("Skills", "line one\nline two"),
            SystemMessageAction::Dialog {
                title: "Skills".to_string(),
                body: "line one\nline two".to_string(),
            }
        );
    }

    #[test]
    fn ui_event_queue_policy_filters_empty_payloads_and_labels_copy_notice() {
        assert!(!should_queue_notice(""));
        assert!(!should_queue_notice(" \n\t"));
        assert!(should_queue_notice("saved"));
        assert!(!should_queue_dialog("  "));
        assert!(should_queue_dialog("details"));

        assert_eq!(copied_notice_message(1), "Copied 1 char to clipboard");
        assert_eq!(copied_notice_message(2), "Copied 2 chars to clipboard");
    }

    #[test]
    fn merge_policy_preserves_rehydrated_text_and_tool_cards() {
        assert!(preserve_streamed_text(
            AgentMessageKindPolicy::Assistant,
            "hello world",
            AgentMessageKindPolicy::Assistant,
            "hello",
        ));
        assert!(!preserve_streamed_text(
            AgentMessageKindPolicy::Tool,
            "old",
            AgentMessageKindPolicy::Tool,
            "",
        ));
        assert!(should_merge_tool_parts(
            AgentMessageKindPolicy::Tool,
            AgentMessageKindPolicy::Tool,
        ));
    }

    #[test]
    fn task_id_reads_detail_before_message_text() {
        assert_eq!(
            task_id_from_text("task_id: child-1\nstatus: running", "task_id: wrong"),
            Some("child-1".to_string())
        );
        assert_eq!(task_id_from_text("", "no id"), None);
    }

    #[test]
    fn task_status_policy_finds_and_normalizes_task_messages() {
        assert!(is_task_message_for_id(
            AgentMessageKindPolicy::Tool,
            "task",
            "Task(child)",
            "task_id: child-1\nstatus: running",
            "child-1",
        ));
        assert!(!is_task_message_for_id(
            AgentMessageKindPolicy::Assistant,
            "task",
            "task_id: child-1",
            "",
            "child-1",
        ));
        assert_eq!(normalize_task_message_status("completed"), "completed");
        assert_eq!(normalize_task_message_status("stopped"), "error");
        assert_eq!(normalize_task_message_status("blocked"), "running");
    }

    #[test]
    fn task_status_policy_rewrites_known_status_markers() {
        let mut status = "running".to_string();
        let mut text = "task_id: child-1\nstatus: running\n\nThe subagent is running in the background and the user can still message the main session.".to_string();
        let mut detail = "status: stopped\nother".to_string();

        assert!(apply_task_message_status(
            &mut status,
            &mut text,
            &mut detail,
            "completed",
        ));

        assert_eq!(status, "completed");
        assert!(text.contains("status: completed"));
        assert!(!text.contains("running in the background"));
        assert!(text.contains("The subagent is no longer running."));
        assert!(detail.contains("status: completed"));
    }
}
