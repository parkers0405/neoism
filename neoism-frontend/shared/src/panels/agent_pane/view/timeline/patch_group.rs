use super::*;

/// Adjacent successful patch calls become one file stack, not one synthetic
/// before/after. A non-edit, an in-flight/error result or missing diff ends it.
pub(crate) fn patch_tool_group_at<M: AgentTimelineMessage>(
    messages: &[M],
    start: usize,
) -> Option<(usize, M)> {
    let first = messages.get(start)?;
    let mut end = start;
    while let Some(message) = messages.get(end) {
        if message.kind() != AgentTimelineMessageKind::Tool
            || !crate::panels::agent_pane::view::tool_message::is_edit_tool_name(message.tool())
            || !matches!(message.status(), "completed" | "success" | "done")
            || message.output_kind() != AgentTimelineOutputKind::Text
            || !message.todos_empty()
            || cached_edit_diff_sections_for_parts(message.id(), message.title(), message.text(), message.status(), message.tool(), message.detail()).is_none()
        {
            break;
        }
        end += 1;
    }
    if end < start + 2 {
        return None;
    }
    let calls = messages[start..end].iter().map(|message| serde_json::json!({
        "id": message.id(), "title": message.title(), "text": message.text(),
        "tool": message.tool(), "detail": message.detail(),
    })).collect::<Vec<_>>();
    let detail = serde_json::json!({ "neoismToolDetail": "patchStack", "calls": calls }).to_string();
    Some((end, M::patch_stack_message(first.id().to_owned(), format!("Patched files · {} calls", end - start), detail)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::agent_pane::state::NeoismAgentMessage;

    fn edit(id: &str, path: &str, before: &str, after: &str) -> NeoismAgentMessage {
        let detail = serde_json::json!({"neoismToolDetail":"edit", "input": {
            "filePath":path, "oldString":before, "newString":after,
        }}).to_string();
        NeoismAgentMessage::patch_stack_message(id.into(), "Edit".into(), detail)
    }

    #[test]
    fn adjacent_edits_stack_without_inventing_a_cumulative_baseline() {
        let messages = vec![edit("a", "src/lib.rs", "old", "middle"), edit("b", "src/lib.rs", "middle", "new")];
        let (end, group) = patch_tool_group_at(&messages, 0).unwrap();
        assert_eq!(end, 2);
        assert_eq!(group.id, "a");
        let sections = cached_edit_diff_sections_for_parts(&group.id, &group.title, &group.text, &group.status, &group.tool, &group.detail).unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].lines()[0].text, "-old");
        assert_eq!(sections[1].lines()[0].text, "-middle");
    }

    #[test]
    fn unfinished_failed_and_non_edits_end_the_stack() {
        for status in ["running", "error", "stopped"] {
            let mut boundary = edit("b", "b.rs", "old", "new");
            boundary.status = status.into();
            assert!(patch_tool_group_at(&[edit("a", "a.rs", "old", "new"), boundary], 0).is_none());
        }
        let mut boundary = edit("b", "b.rs", "old", "new");
        boundary.tool = "read".into();
        assert!(patch_tool_group_at(&[edit("a", "a.rs", "old", "new"), boundary], 0).is_none());
    }
}