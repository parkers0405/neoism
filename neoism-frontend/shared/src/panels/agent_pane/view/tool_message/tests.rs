use super::diff::{cached_edit_diff_sections, snapshot_section_from_text};
use super::*;
use crate::panels::agent_pane::state::{NeoismAgentMessage, NeoismAgentOutputKind};

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
fn completed_apply_patch_parses_diff_card() {
    let detail = r#"{"neoismToolDetail":"edit","tool":"apply_patch","input":{"patchText":"*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n"},"metadata":null}"#;
    let sections = cached_edit_diff_sections(&apply_patch_message("completed", detail))
        .expect("settled apply_patch should parse");
    assert!(!sections.is_empty());
}
