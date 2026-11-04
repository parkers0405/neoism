use super::diff::{cached_edit_diff_sections, snapshot_section_from_text};
use super::*;
use crate::panels::agent_pane::state::{NeoismAgentMessage, NeoismAgentOutputKind};

fn full_snapshot_state(text: Option<&str>) -> serde_json::Value {
    use base64::Engine as _;
    match text {
        Some(text) => serde_json::json!({"exists":true, "contentBase64":STANDARD.encode(text)}),
        None => serde_json::json!({"exists":false}),
    }
}

fn snapshot_patch_call(id: &str, before: Option<&str>, after: Option<&str>) -> serde_json::Value {
    let detail = serde_json::json!({"neoismToolDetail":"edit", "metadata":{"snapshots":[{
        "path":"src/lib.rs", "before":full_snapshot_state(before), "after":full_snapshot_state(after),
    }]}, "input":{"filePath":"src/lib.rs", "oldString":before.unwrap_or(""), "newString":after.unwrap_or("")}});
    serde_json::json!({"id":id, "tool":"apply_patch", "detail":detail.to_string()})
}

fn prepared_patch_stack(calls: Vec<serde_json::Value>) -> CachedToolDiffSections {
    let detail = serde_json::json!({"neoismToolDetail":"patchStack", "calls":calls}).to_string();
    let message = ToolMessageParts { id:"snapshot-chain-regression", title:"Patch", text:"",
        status:"completed", tool:"apply_patch", detail:&detail };
    cached_edit_diff_sections(&message).unwrap()
}

#[test]
fn same_file_full_snapshot_chains_merge_and_preserve_lifecycle() {
    for (before, after, expected_path) in [
        (Some("original\nkeep\n"), Some("final\nkeep\n"), "src/lib.rs"),
        (None, Some("final\nkeep\n"), "A src/lib.rs"),
        (Some("original\nkeep\n"), None, "D src/lib.rs"),
    ] {
        let sections = prepared_patch_stack(vec![
            snapshot_patch_call("first", before, Some("intermediate one\nkeep\n")),
            snapshot_patch_call("second", Some("intermediate one\nkeep\n"), Some("intermediate two\nkeep\n")),
            snapshot_patch_call("third", Some("intermediate two\nkeep\n"), after),
        ]);
        assert_eq!(sections.len(), 1);
        let section = &sections[0];
        assert_eq!(section.path, expected_path);
        assert!(!section.lines.iter().any(|line| line.text.contains("intermediate")));
        if before.is_some() {
            assert!(section.lines.iter().any(|line| line.text == "-original"));
        } else {
            assert_eq!(section.deletions, 0);
        }
        if after.is_some() {
            assert!(section.lines.iter().any(|line| line.text == "+final"));
        } else {
            assert_eq!(section.additions, 0);
        }
    }
}

#[test]
fn fragments_missing_or_conflicting_snapshots_never_prove_a_same_file_chain() {
    let fragment = |id: &str, old: &str, new: &str| serde_json::json!({
        "id":id, "tool":"edit", "detail":serde_json::json!({"neoismToolDetail":"edit", "input":{
            "filePath":"src/lib.rs", "oldString":old, "newString":new,
        }}).to_string(),
    });
    let first = snapshot_patch_call("first", Some("original"), Some("middle"));
    let mut missing = snapshot_patch_call("missing", Some("middle"), Some("final"));
    let mut detail: serde_json::Value = serde_json::from_str(missing["detail"].as_str().unwrap()).unwrap();
    detail["metadata"]["snapshots"][0]["before"] = serde_json::json!({"exists":true});
    missing["detail"] = serde_json::json!(detail.to_string());
    let cases = vec![
        vec![fragment("first", "original", "middle"), fragment("second", "middle", "final")],
        vec![first.clone(), fragment("second", "middle", "final")],
        vec![fragment("first", "original", "middle"), snapshot_patch_call("second", Some("middle"), Some("final"))],
        vec![first.clone(), snapshot_patch_call("mismatch", Some("different baseline"), Some("final"))],
        vec![first, missing],
        // Equal empty text is insufficient when existence/lifecycle differs.
        vec![snapshot_patch_call("first", Some("original"), Some("")), snapshot_patch_call("second", None, Some("final"))],
        vec![snapshot_patch_call("delete", Some("original"), None), snapshot_patch_call("recreate", None, Some("final"))],
        vec![snapshot_patch_call("create", None, Some("middle")), snapshot_patch_call("delete", Some("middle"), None)],
        // Never skip a fragment barrier to join an older matching snapshot.
        vec![snapshot_patch_call("first", Some("original"), Some("middle")), fragment("barrier", "middle", "middle"), snapshot_patch_call("third", Some("middle"), Some("final"))],
    ];
    for calls in cases {
        let expected = calls.len();
        let first = prepared_patch_stack(vec![calls[0].clone()]);
        let sections = prepared_patch_stack(calls);
        assert_eq!(sections.len(), expected);
        assert_eq!(sections[0].path, first[0].path);
        assert_eq!(sections[0].lines.iter().map(|line| &line.text).collect::<Vec<_>>(),
            first[0].lines.iter().map(|line| &line.text).collect::<Vec<_>>());
    }
}

#[test]
fn content_only_write_does_not_fabricate_an_empty_baseline() {
    let detail = serde_json::json!({"neoismToolDetail":"edit", "input": {
        "filePath":"src/lib.rs", "content":"new contents\n",
    }}).to_string();
    let message = ToolMessageParts { id: "write-without-baseline", title: "Write", text: "",
        status: "completed", tool: "write", detail: &detail };
    let sections = cached_edit_diff_sections(&message).unwrap();
    assert_eq!(sections[0].additions, 0);
    assert_eq!(sections[0].deletions, 0);
    assert!(sections[0].lines[0].text.contains("baseline unavailable"));
    assert_eq!(sections[0].lines[1].kind, DiffLineKind::Context);
}

#[test]
fn full_snapshots_win_over_patch_previews_without_dropping_other_files() {
    let full = "available line\n".repeat(600);
    let detail = serde_json::json!({"neoismToolDetail":"edit", "metadata": {
        "snapshots":[{"path":"src/full.rs", "before":full_snapshot_state(Some("")), "after":full_snapshot_state(Some(&full))}],
        "files":[
            {"relativePath":"src/full.rs", "type":"update", "patch":"@@ -0,0 +1 @@\n+preview"},
            {"relativePath":"src/other.rs", "type":"add", "patch":"@@ -0,0 +1 @@\n+other"}
        ]
    }}).to_string();
    let message = ToolMessageParts { id: "full-snapshot-partial-coverage", title: "Patch", text: "",
        status: "completed", tool: "apply_patch", detail: &detail };
    let sections = cached_edit_diff_sections(&message).unwrap();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].additions, 600);
    assert_eq!(sections[0].lines.len(), 600);
    assert_eq!(sections[1].path, "A src/other.rs");
}

#[test]
fn explicit_file_lifecycle_labels_survive_patch_formats_and_empty_creations() {
    for (id, patch, path) in [
        ("v4-create", "*** Begin Patch\n*** Add File: src/new.rs\n*** End Patch", "A src/new.rs"),
        ("v4-delete", "*** Begin Patch\n*** Delete File: src/old.rs\n*** End Patch", "D src/old.rs"),
        ("empty-unified-create", "diff --git a/empty.rs b/empty.rs\nnew file mode 100644\nindex 0000000..e69de29", "A empty.rs"),
        ("unified-create", "diff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+new", "A new.rs"),
        ("unified-delete", "diff --git a/old.rs b/old.rs\ndeleted file mode 100644\n--- a/old.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old", "D old.rs"),
    ] {
        let message = ToolMessageParts { id, title: "Patch", text: "", status: "completed", tool: "apply_patch", detail: patch };
        let sections = cached_edit_diff_sections(&message).unwrap();
        assert_eq!(sections[0].path, path);
    }
}

#[test]
fn composed_reads_keep_independent_calls_with_compact_file_details() {
    let children: Vec<_> = ["src/a.rs", "src/a.rs", "src/b.rs"].iter().enumerate().map(|(i, path)| {
        let mut child = NeoismAgentMessage::tool(format!("Read({path})"), "hidden source", "completed", "read", NeoismAgentOutputKind::Code, "", Vec::new()).with_id(format!("child-{i}"));
        child.detail = "hidden detail".into();
        child
    }).collect();
    let rows = super::render::compact_composition_children(&children);
    assert_eq!(rows.len(), 3);
    for (index, path) in ["src/a.rs", "src/a.rs", "src/b.rs"].iter().enumerate() {
        assert_eq!(rows[index].id, children[index].id);
        assert_eq!(rows[index].title, format!("Read({path})"));
        assert_eq!(rows[index].text, *path);
        assert!(rows[index].detail.is_empty());
        assert!(rows[index].composition_child);
    }
    assert_eq!(children[0].detail, "hidden detail");
}

#[test]
fn tool_preview_cache_ignores_hidden_output_but_tracks_visible_edits() {
    use crate::primitives::paint_record::RecordingPainter;
    let mut paint = RecordingPainter::new("monospace", 400, vec![]);
    let opts = DrawOpts { font_size: 13.0, ..DrawOpts::default() };
    let prefix = "first\r\n  nested\n\nlast\n";
    let before = tool_wrapped_rows(&mut paint, prefix, 600.0, &opts, 4);
    let output = format!("{prefix}{}", "hidden output\n".repeat(100_000));
    let after = tool_wrapped_rows(&mut paint, &output, 600.0, &opts, 4);
    assert!(Rc::ptr_eq(&before, &after));
    assert_eq!(after.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(), ["first", "nested", "", "last"]);
    assert!(after[1].nested);
    let changed = tool_wrapped_rows(&mut paint, &output.replacen("first", "edited", 1), 600.0, &opts, 4);
    assert!(!Rc::ptr_eq(&after, &changed));
    assert_eq!(changed[0].text, "edited");
}

#[test]
fn timeline_diff_rows_wrap_when_the_pane_narrows() {
    let long = "very_long_identifier_".repeat(80);
    let section = snapshot_section_from_text(
        "src/lib.rs".to_string(),
        &format!("old_{long}\nkeep_{long}\n"),
        &format!("new_{long}\nkeep_{long}\n"),
    );
    for scale in [1.0, 2.0] {
        let narrow =
            super::diff::cached_diff_card_view(&section, 120.0 * scale, scale, true);
        let wide =
            super::diff::cached_diff_card_view(&section, 900.0 * scale, scale, true);
        assert!(!Rc::ptr_eq(&narrow, &wide));
        assert!(narrow.visual_rows > wide.visual_rows);
        assert!(wide.visual_rows > section.lines.len());
        assert_eq!(
            narrow.visual_row_offsets.as_slice(),
            &diff_card::visual_row_offsets(&section.lines, 120.0 * scale, scale)
        );
        assert_eq!(narrow.rows[0].text, section.lines[0].text);
    }
}

#[test]
fn timeline_diff_gutter_reserves_hidden_numbers_from_both_sides() {
    let after = "line\n".repeat(60);
    for (display, old, new, expected) in [
        (Some(9_999), None, None, 9_999),
        (Some(53), Some(100_000), Some(53), 100_000),
        (None, None, Some(u32::MAX), u32::MAX),
    ] {
        let mut section = snapshot_section_from_text(
            format!("gutter-{expected}.rs"),
            "",
            &after,
        );
        let last = section.lines.last_mut().unwrap();
        last.line_number = display;
        last.old_line_number = old;
        last.new_line_number = new;
        for scale in [0.7, 1.0, 1.5, 2.0] {
            let preview = super::diff::cached_diff_card_view(&section, 120.0 * scale, scale, false);
            let expanded = super::diff::cached_diff_card_view(&section, 900.0 * scale, scale, true);
            assert!(preview.rows.len() < section.lines.len());
            assert_eq!(preview.max_line_number, expected);
            assert_eq!(expanded.max_line_number, preview.max_line_number);
        }
    }
}

#[test]
fn timeline_diff_defaults_to_header_only_and_expands_every_available_line() {
    let after = (0..600)
        .map(|i| format!("line_{i}_{}\n", "x".repeat(400)))
        .collect::<String>();
    let section = snapshot_section_from_text("src/lib.rs".to_string(), "", &after);
    let preview = super::diff::cached_diff_card_view(&section, 120.0, 1.0, false);
    let expanded = super::diff::cached_diff_card_view(&section, 120.0, 1.0, true);
    assert_eq!(preview.visual_rows, 0);
    assert_eq!(preview.preview_visual_rows, 0);
    assert!(preview.rows.is_empty());
    assert_eq!(section.omitted, 0);
    assert_eq!(expanded.rows.len(), 600);
    assert!(expanded.visual_rows > 600);
    assert_eq!(expanded.rows.last().unwrap().text, format!("+line_599_{}", "x".repeat(400)));
    assert_eq!(expanded.preview_visual_rows, expanded.visual_rows);
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
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].lines[0].text.trim_start(), "return 42;");
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
    assert_eq!(failed.composition_children[0].status, "completed");
    assert_eq!(updated[0].fingerprint, sections[0].fingerprint);
    assert!(!is_edit_tool_name("compose"));
}

#[test]
fn composition_program_retains_full_source_and_child_edits_show_file_accordions() {
    use crate::panels::agent_pane::api_mapping::part_block;
    let code = (0..190).map(|i| format!("console.log({i});\n")).collect::<String>();
    let part = serde_json::json!({"id":"p", "type":"tool", "tool":"compose", "state":{"status":"completed", "input":{"code":"old"}, "metadata":{"composition":{"code":code, "toolCalls":[
        {"id":"edit", "path":"neoism.edit", "status":"completed", "input":{"filePath":"src/lib.rs", "oldString":"a", "newString":"b"}, "presentation":{"tool":"edit", "output":"edited"}}
    ]}}}});
    let m = part_block(&part).unwrap();
    let sections = cached_edit_diff_sections(&m).unwrap();
    assert_eq!(sections[0].path, "Program source.js");
    assert_eq!(sections[0].lines.len(), 190);
    assert_eq!(sections[0].omitted, 0);
    assert_eq!(Lang::from_path(&sections[0].path), Lang::Javascript);
    let mut pane = NeoismAgentPane::default(); pane.upsert_part_message(m.clone());
    assert!(!show_tool_diff_cards("compose", "completed", pane.tool_expanded("p"), pane.tool_archived("p")));
    let child = &m.composition_children[0];
    assert!(cached_edit_diff_sections(child).is_some());
    assert!(show_tool_diff_cards(&child.tool, &child.status, false, pane.tool_archived(&child.id) || child.composition_child()));
    assert!(show_tool_diff_cards(&child.tool, &child.status, true, pane.tool_archived(&child.id) || child.composition_child()));
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
fn edit_file_rows_are_visible_without_parent_disclosure() {
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
            show_tool_diff_cards(tool, "completed", false, true),
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

#[test]
fn patch_files_render_inline_and_expand_independently_without_parent_click() {
    use crate::primitives::paint_record::{PaintCommand, RecordingPainter};
    let message = apply_patch_message("completed", "*** Begin Patch\n*** Update File: src/first.rs\n@@\n-old_first\n+new_first\n*** Add File: src/second.rs\n+new_second\n*** End Patch");
    for (width, scale) in [(260.0, 1.0), (900.0, 1.0), (900.0, 2.0)] {
        let mut pane = NeoismAgentPane::default();
        let theme = IdeTheme::by_name("dark");
        let clip = [0.0, 0.0, width * scale, 1200.0 * scale];
        let mut heights = Vec::new();
        for expanded in [false, true, false] {
            let mut paint = RecordingPainter::new("monospace", 400, vec![]);
            pane.clear_tool_hit_rects();
            let height = measure_tool_message_height(
                &mut paint, &pane, &message, width * scale, scale, false, None,
            ).unwrap();
            heights.push(height);
            render_tool_message(
                &mut paint, &mut pane, 0.0, 0.0, width * scale, height,
                &message, &theme, scale, 0.0, clip, &[], None, None,
            );
            let commands = paint.commands();
            let arrows: Vec<_> = commands.iter().filter_map(|command| match command {
                PaintCommand::Text { x, text, .. } if text == "›" || text == "⌄" => Some((*x, text.as_str())),
                _ => None,
            }).collect();
            assert_eq!(arrows, [
                ((width - 16.0) * scale, if expanded { "⌄" } else { "›" }),
                ((width - 16.0) * scale, "›"),
            ]);
            for command in commands.iter() {
                if let PaintCommand::Text { x, width: text_width, text, .. } = command {
                    if text == "+1" || text == "-1" {
                        assert!(x + text_width < arrows[0].0);
                    }
                }
            }
            let stems: Vec<_> = commands.iter().filter_map(|command| match command {
                PaintCommand::Rect { rect, .. }
                    if (rect[0] - 30.0 * scale).abs() < 0.01 && rect[2] == 2.0 * scale => Some(*rect),
                _ => None,
            }).collect();
            assert_eq!(stems.len(), 2);
            assert_eq!(stems[0][1], TOOL_HEADER_HEIGHT * scale);
            assert!(stems[0][1] + stems[0][3] >= stems[1][1]);
            if expanded {
                assert!(stems[1][3] > stems[0][3]);
            }
            for filename in ["first.rs", "second.rs"] {
                assert!(commands.iter().any(|command| matches!(command,
                    PaintCommand::Text { text, .. } if text == filename)));
            }
            if !expanded {
                let rows: Vec<_> = commands.iter().filter_map(|command| match command {
                    PaintCommand::Text { y, text, .. } if text == "first.rs" || text == "second.rs" => Some(*y),
                    _ => None,
                }).collect();
                assert_eq!(rows[1] - rows[0], TOOL_HEADER_HEIGHT * scale);
            }
            assert_eq!(commands.iter().any(|command| matches!(command,
                PaintCommand::Text { text, .. } if text.contains("new_first"))), expanded);
            assert!(!commands.iter().any(|command| matches!(command,
                PaintCommand::Text { text, .. } if text.contains("new_second"))));
            if expanded {
                let mut clipped_paint = RecordingPainter::new("monospace", 400, vec![]);
                let short_clip = [0.0, 0.0, width * scale, 80.0 * scale];
                let cut = [30.0 * scale, 50.0 * scale, 2.0 * scale, 10.0 * scale];
                render_tool_message(
                    &mut clipped_paint, &mut pane, 0.0, 0.0, width * scale, height,
                    &message, &theme, scale, 0.0, short_clip, &[cut], None, None,
                );
                let clipped_commands = clipped_paint.commands();
                let stems: Vec<_> = clipped_commands.iter().filter_map(|command| match command {
                    PaintCommand::Rect { rect, .. }
                        if (rect[0] - 30.0 * scale).abs() < 0.01 && rect[2] == 2.0 * scale => Some(*rect),
                    _ => None,
                }).collect();
                // The second file is below the viewport, but its stem must
                // still reach the bottom alongside the first file's code.
                assert!(stems.iter().any(|rect| rect[1] + rect[3] == short_clip[3]));
                for rect in stems {
                    assert!(intersect_rect(rect, cut).is_none());
                    assert_eq!(intersect_rect(rect, short_clip), Some(rect));
                }
            }
            assert!(!pane.tool_hit(80.0 * scale, 10.0 * scale));
            assert!(pane.toggle_tool_at((width - 12.0) * scale, (TOOL_HEADER_HEIGHT + 10.0) * scale));
            assert_eq!(pane.tool_expanded("tool-1:0"), !expanded);
            assert!(!pane.tool_expanded("tool-1:1"));
        }
        assert!(heights[1] > heights[0]);
        assert_eq!(heights[0], heights[2]);
        // Match the composition parent + two compact child tool rows, without
        // the old diff-card gap between files or after the last file.
        assert_eq!(heights[0], 3.0 * TOOL_HEADER_HEIGHT * scale);
    }
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
