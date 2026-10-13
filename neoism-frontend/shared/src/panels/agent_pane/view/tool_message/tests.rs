use super::diff::{cached_edit_diff_sections, snapshot_section_from_text};
use super::*;
use crate::panels::agent_pane::state::{NeoismAgentMessage, NeoismAgentOutputKind};

#[test]
fn timeline_diff_rows_do_not_wrap_when_the_pane_narrows() {
    let long = "very_long_identifier_".repeat(80);
    let section = snapshot_section_from_text(
        "src/lib.rs".to_string(),
        &format!("old_{long}\nkeep_{long}\n"),
        &format!("new_{long}\nkeep_{long}\n"),
    );
    for scale in [1.0, 2.0] {
        let narrow =
            super::diff::cached_diff_card_view(&section, 120.0 * scale, scale, false);
        let wide =
            super::diff::cached_diff_card_view(&section, 900.0 * scale, scale, false);
        assert_eq!(narrow.visual_rows, section.lines.len());
        assert_eq!(narrow.preview_visual_rows, wide.preview_visual_rows);
        assert_eq!(
            narrow.visual_row_offsets.as_slice(),
            &(0..=section.lines.len()).collect::<Vec<_>>()
        );
        assert_eq!(narrow.rows[0].text, section.lines[0].text);
    }
}

#[test]
fn timeline_diff_show_more_keeps_the_preview_height() {
    let after = (0..60)
        .map(|i| format!("line_{i}_{}\n", "x".repeat(400)))
        .collect::<String>();
    let section = snapshot_section_from_text("src/lib.rs".to_string(), "", &after);
    let preview = super::diff::cached_diff_card_view(&section, 120.0, 1.0, false);
    let expanded = super::diff::cached_diff_card_view(&section, 120.0, 1.0, true);
    assert_eq!(preview.visual_rows, COLLAPSED_DIFF_ROWS + 1);
    assert!(preview
        .rows
        .last()
        .unwrap()
        .text
        .contains("click to expand"));
    assert_eq!(expanded.visual_rows, section.lines.len());
    assert_eq!(expanded.preview_visual_rows, preview.preview_visual_rows);
}

#[test]
fn diff_horizontal_scroll_uses_fenced_controls_without_toggling_the_card() {
    let mut pane = NeoismAgentPane::default();
    pane.register_tool_hit_rect("edit:0".to_string(), [20.0, 100.0, 300.0, 120.0]);
    for (index, y) in [(0, 100.0), (1, 240.0)] {
        pane.code_scroll_pane()
            .unwrap()
            .register_markdown_horizontal_scroll_rect(
                format!("diff:edit:{index}"),
                [20.0, y, 300.0, 120.0],
                240.0,
            );
    }
    assert_eq!(
        pane.scroll_markdown_horizontal_at(40.0, 140.0, 75.0),
        Some(true)
    );
    let scroll_pane = pane.code_scroll_pane().unwrap();
    assert_eq!(
        scroll_pane.markdown_horizontal_scroll_offset("diff:edit:0", 240.0),
        75.0
    );
    assert_eq!(
        scroll_pane.markdown_horizontal_scroll_offset("diff:edit:1", 240.0),
        0.0
    );
    assert_eq!(
        scroll_pane.markdown_horizontal_scroll_offset("diff:edit:0", 50.0),
        50.0
    );
    scroll_pane.register_markdown_horizontal_scrollbar(
        "diff:edit:0".to_string(),
        [20.0, 200.0, 300.0, 16.0],
        [20.0, 200.0, 100.0, 16.0],
        240.0,
    );
    assert!(pane.begin_markdown_horizontal_scrollbar_drag(50.0, 208.0));
    assert!(pane.drag_markdown_horizontal_scrollbar_to(150.0));
    assert!(!pane.tool_expanded("edit:0"));
    assert!(pane.end_markdown_horizontal_scrollbar_drag());
    assert!(pane.toggle_tool_at(30.0, 160.0));
    assert!(pane.tool_expanded("edit:0"));
}

#[test]
fn composition_sections_update_without_losing_source_or_becoming_edit_diffs() {
    use crate::panels::agent_pane::api_mapping::part_block;
    let mut part = serde_json::json!({"id":"compose", "type":"tool", "tool":"compose",
        "state":{"status":"running", "input":{"code":"return 42;"}},
        "metadata":{"composition":{"toolCalls":[{"path":"docs.write", "status":"running", "input":{}}]}}});
    let live = part_block(&part).unwrap();
    let sections = cached_edit_diff_sections(&live).unwrap();
    assert_eq!(sections.len(), 3);
    assert!(sections[0].lines[0].text.contains("[running]"));
    assert_eq!(sections[2].lines[0].text.trim_start(), "return 42;");
    assert!(sections
        .iter()
        .all(|s| s.additions == 0 && s.deletions == 0 && s.link_target.is_empty()));
    assert!(show_tool_diff_cards("compose", "running", true, false));
    assert!(!show_tool_diff_cards("compose", "running", false, false));
    part["state"]["status"] = serde_json::json!("error");
    part["metadata"]["composition"]["toolCalls"][0]["status"] =
        serde_json::json!("completed");
    let failed = part_block(&part).unwrap();
    let updated = cached_edit_diff_sections(&failed).unwrap();
    assert!(updated[0].lines[0].text.contains("[completed]"));
    assert_eq!(updated[2].fingerprint, sections[2].fingerprint);
    assert!(!is_edit_tool_name("compose"));
}

#[test]
fn mcp_header_identity_is_stable_across_status_and_reveal_details() {
    let mut message = apply_patch_message("pending", "");
    message.tool = "execute".to_owned();
    message.title = "Docs Search".to_owned();
    for status in ["pending", "running", "completed", "error"] {
        message.status = status.to_owned();
        for detail in ["", "SECRET_ARGUMENTS", "SECRET_OUTPUT", "SECRET_ERROR"] {
            message.detail = detail.to_owned();
            assert_eq!(message.title_text(), "Docs Search");
        }
    }
}

#[test]
fn compact_tool_title_omits_completed_and_parentheses() {
    let mut message = apply_patch_message("completed", "");
    message.title = "Read(markdown/html_artifact.rs)".to_string();
    assert_eq!(message.title_text(), "Read  markdown/html_artifact.rs");
    message.status = "pending".to_string();
    assert_eq!(message.title_text(), "Read  markdown/html_artifact.rs");
}

#[test]
fn compact_task_followup_title_uses_the_same_header_spacing() {
    let mut message = apply_patch_message("pending", "");
    message.tool = "task".to_owned();
    for status in ["pending", "queued", "running", "completed"] {
        message.status = status.to_owned();
        for name in ["Task", "Follow-up"] {
            message.title = format!("{name}(@explore · Check parser (edge cases))");
            assert_eq!(
                message.title_text(),
                format!("{name}  @explore · Check parser (edge cases)")
            );
        }
    }
}

#[test]
fn compact_tool_title_keeps_shell_arguments_and_one_line() {
    let mut message = apply_patch_message("running", "");
    message.title = "Bash(printf '(two  words)'\nnext command)".to_string();
    assert_eq!(
        message.title_text(),
        "Bash  printf '(two  words)' next command"
    );
    message.title.clear();
    assert_eq!(message.title_text(), "Tool");
}

#[test]
fn expanded_tool_details_reserve_nested_indent_and_right_padding() {
    for scale in [1.0, 2.0] {
        for width in [160.0, 900.0] {
            let width = width * scale;
            assert_eq!(tool_body_wrap_width(width, scale) + 100.0 * scale, width);
        }
    }
}

#[test]
fn live_edits_keep_diff_cards_without_expanding_the_parent_tool_row() {
    for tool in [
        "edit",
        "Edit",
        "write",
        "multiedit",
        "apply_patch",
        "ApplyPatch",
        "patch",
    ] {
        assert!(
            show_tool_diff_cards(tool, "completed", false, false),
            "{tool}"
        );
        assert!(
            !show_tool_diff_cards(tool, "completed", false, true),
            "{tool}"
        );
        assert!(
            show_tool_diff_cards(tool, "completed", true, true),
            "{tool}"
        );
    }
    for tool in ["read", "grep", "bash", "tool_group", "fff_find_files"] {
        assert!(
            !show_tool_diff_cards(tool, "completed", false, false),
            "{tool}"
        );
    }
    for status in ["pending", "running", "streaming"] {
        assert!(!show_tool_diff_cards("apply_patch", status, false, false));
    }
}

#[test]
fn snapshot_diff_pairs_replacement_rows_by_line_number() {
    let before = "keep\nold one\nold two\ntail\n";
    let after = "keep\nnew one\nnew two\ntail\n";

    let section = snapshot_section_from_text("src/lib.rs".to_string(), before, after);
    let rows = section.lines;

    let remove_one = rows
        .iter()
        .position(|row| row.kind == DiffLineKind::Remove && row.text == "-old one")
        .expect("old one remove row");
    assert_eq!(rows[remove_one].line_number, Some(2));
    assert_eq!(rows[remove_one + 1].kind, DiffLineKind::Add);
    assert_eq!(rows[remove_one + 1].text, "+new one");
    assert_eq!(rows[remove_one + 1].line_number, Some(2));
    assert_eq!(rows[remove_one + 2].kind, DiffLineKind::Remove);
    assert_eq!(rows[remove_one + 2].line_number, Some(3));
    assert_eq!(rows[remove_one + 3].kind, DiffLineKind::Add);
    assert_eq!(rows[remove_one + 3].line_number, Some(3));
}

fn apply_patch_message(status: &str, detail: &str) -> NeoismAgentMessage {
    let mut message = NeoismAgentMessage::tool(
        "ApplyPatch(src/lib.rs)",
        "applying patch",
        status,
        "apply_patch",
        NeoismAgentOutputKind::Text,
        "rust",
        Vec::new(),
    );
    message.id = "tool-1".to_string();
    message.detail = detail.to_string();
    message
}

#[test]
fn running_apply_patch_skips_live_diff_parse() {
    let detail = r#"{"neoismToolDetail":"edit","tool":"apply_patch","input":{"patchText":"*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n"},"metadata":null}"#;
    assert!(cached_edit_diff_sections(&apply_patch_message("running", detail)).is_none());
    assert!(cached_edit_diff_sections(&apply_patch_message("pending", detail)).is_none());
}

#[test]
fn completed_acp_edit_uses_explicit_diff_content() {
    let detail = serde_json::json!({
        "neoismToolDetail": "edit", "tool": "edit", "input": {},
        "metadata": {"acpDiffs": [{"path": "src/lib.rs", "oldText": "old\n", "newText": "new\n"}]}
    }).to_string();
    let mut message = apply_patch_message("completed", &detail);
    message.tool = "edit".to_string();
    let sections = cached_edit_diff_sections(&message).expect("ACP diff should render");
    assert_eq!(
        sections[0]
            .lines
            .iter()
            .filter(|row| row.kind == DiffLineKind::Add)
            .count(),
        1
    );
    assert_eq!(
        sections[0]
            .lines
            .iter()
            .filter(|row| row.kind == DiffLineKind::Remove)
            .count(),
        1
    );
}

#[test]
fn completed_apply_patch_parses_diff_card() {
    let detail = r#"{"neoismToolDetail":"edit","tool":"apply_patch","input":{"patchText":"*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n"},"metadata":null}"#;
    let sections = cached_edit_diff_sections(&apply_patch_message("completed", detail))
        .expect("settled apply_patch should parse");
    assert!(!sections.is_empty());
}
