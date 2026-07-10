use super::layout::{
    from_state_cache, into_state_cache, prepared_message_tool_diff_sections,
    timeline_message_visibility,
    timeline_row_range_for_source_range, timeline_row_range_intersects_viewport,
    visible_timeline_row_range,
};
use super::read_group::read_tool_group_at;

    use super::*;

    fn tool_message(
        id: &str,
        tool: &str,
        title: &str,
        status: &str,
    ) -> NeoismAgentMessage {
        NeoismAgentMessage {
            id: id.to_string(),
            kind: NeoismAgentMessageKind::Tool,
            title: title.to_string(),
            text: format!("{title} preview"),
            status: status.to_string(),
            tool: tool.to_string(),
            output_kind: NeoismAgentOutputKind::Text,
            lang: String::new(),
            line_offset: None,
            todos: Vec::new(),
            detail: format!("{title} detail"),
            usage: None,
        }
    }

    fn text_message(
        id: &str,
        kind: NeoismAgentMessageKind,
        text: &str,
    ) -> NeoismAgentMessage {
        NeoismAgentMessage {
            id: id.to_string(),
            kind,
            title: String::new(),
            text: text.to_string(),
            status: String::new(),
            tool: String::new(),
            output_kind: NeoismAgentOutputKind::Text,
            lang: String::new(),
            line_offset: None,
            todos: Vec::new(),
            detail: String::new(),
            usage: None,
        }
    }

    #[test]
    fn settled_timeline_keeps_user_and_final_answer_parts_only() {
        let messages = vec![
            text_message("u1", NeoismAgentMessageKind::User, "change it"),
            text_message("r1", NeoismAgentMessageKind::Reasoning, "planning"),
            text_message(
                "a-progress",
                NeoismAgentMessageKind::Assistant,
                "checking the build",
            ),
            tool_message("t1", "read", "Read(src/lib.rs)", "completed"),
            text_message("a1", NeoismAgentMessageKind::Assistant, "Done."),
            text_message(
                "a2",
                NeoismAgentMessageKind::Assistant,
                "Tests pass.",
            ),
            text_message("u2", NeoismAgentMessageKind::User, "explain"),
            text_message("a3", NeoismAgentMessageKind::Assistant, "Part one."),
            text_message("a4", NeoismAgentMessageKind::Assistant, "Part two."),
        ];

        assert_eq!(
            timeline_message_visibility(&messages, None),
            vec![true, false, false, false, true, true, true, true, true]
        );
        assert_eq!(
            timeline_message_visibility(&messages, Some(1)),
            vec![true, true, true, true, true, true, true, true, true]
        );
    }

    #[test]
    fn visit_trace_boundary_does_not_reveal_older_reasoning() {
        let messages = vec![
            text_message("u1", NeoismAgentMessageKind::User, "old question"),
            text_message("r1", NeoismAgentMessageKind::Reasoning, "old trace"),
            text_message("a1", NeoismAgentMessageKind::Assistant, "Old answer."),
            text_message("u2", NeoismAgentMessageKind::User, "new question"),
            text_message("r2", NeoismAgentMessageKind::Reasoning, "live trace"),
            tool_message("t2", "read", "Read(src/main.rs)", "running"),
            text_message(
                "a2",
                NeoismAgentMessageKind::Assistant,
                "live progress",
            ),
            text_message("s2", NeoismAgentMessageKind::System, "internal"),
        ];

        assert_eq!(
            timeline_message_visibility(&messages, Some(4)),
            vec![true, false, true, true, true, true, true, false]
        );
    }

    #[test]
    fn live_read_tools_group_into_one_display_message() {
        let messages = vec![
            tool_message("read-a", "read", "Read(src/a.rs)", "completed"),
            tool_message("grep-b", "grep", "Grep(Thing)", "completed"),
            tool_message("list-c", "list", "List(src)", "running"),
        ];

        let (end, group) = read_tool_group_at(&messages, 0).expect("group");

        assert_eq!(end, 3);
        assert_eq!(group.id, "read-a..list-c");
        assert_eq!(group.tool, "tool_group");
        assert_eq!(group.status, "running");
        assert!(group.text.contains("Read(src/a.rs)"));
        assert!(group.detail.contains("Read(src/a.rs)"));
        assert!(group.detail.contains("Read(src/a.rs) detail"));
    }

    #[test]
    fn live_grouping_keeps_short_or_failed_runs_separate() {
        let short = vec![
            tool_message("read-a", "read", "Read(src/a.rs)", "completed"),
            tool_message("grep-b", "grep", "Grep(Thing)", "completed"),
        ];
        assert!(read_tool_group_at(&short, 0).is_none());

        let failed = vec![
            tool_message("read-a", "read", "Read(src/a.rs)", "completed"),
            tool_message("grep-b", "grep", "Grep(Thing)", "error"),
            tool_message("list-c", "list", "List(src)", "completed"),
        ];
        assert!(read_tool_group_at(&failed, 0).is_none());
    }

    #[test]
    fn prepared_tool_diff_sections_survive_layout_cache_roundtrip() {
        let mut patch =
            tool_message("patch-1", "apply_patch", "Apply patch", "completed");
        patch.detail = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
"
        .to_string();

        let sections =
            prepared_message_tool_diff_sections(&patch).expect("diff sections");
        assert!(!sections.is_empty());

        let cache = TimelineLayoutCache {
            epoch: 1,
            source_len: 1,
            width_bucket: 100,
            scale_bucket: 4,
            gap_bucket: 72,
            content_height: 64.0,
            pages: Vec::new(),
            rows: vec![TimelineLayoutRow {
                source_index: 0,
                source_end_index: 0,
                top: 0.0,
                height: 64.0,
                display_text: None,
                display_message: Some(patch),
                markdown_blocks: None,
                tool_diff_sections: Some(sections.clone()),
                is_edit_tool: true,
            }],
            estimated_prefix_rows: 0,
        };

        let restored = from_state_cache(into_state_cache(cache));
        assert_eq!(
            restored.rows[0]
                .tool_diff_sections
                .as_ref()
                .map(|sections| sections.len()),
            Some(sections.len())
        );
    }

    #[test]
    fn visible_row_range_skips_rows_outside_registration_band() {
        let rows = vec![
            layout_row(0, 0.0, 20.0),
            layout_row(1, 30.0, 20.0),
            layout_row(2, 60.0, 20.0),
            layout_row(3, 90.0, 20.0),
        ];

        assert_eq!(visible_timeline_row_range(&rows, 35.0, 85.0), 1..3);
    }

    #[test]
    fn visible_row_range_includes_edge_intersections() {
        let rows = vec![layout_row(0, 0.0, 20.0), layout_row(1, 20.0, 20.0)];

        assert_eq!(visible_timeline_row_range(&rows, 20.0, 20.0), 0..2);
    }

    #[test]
    fn visible_row_range_handles_empty_or_inverted_band() {
        let rows = vec![layout_row(0, 0.0, 20.0)];

        assert_eq!(
            visible_timeline_row_range::<NeoismAgentMessage>(&[], 0.0, 100.0),
            0..0
        );
        assert_eq!(visible_timeline_row_range(&rows, 100.0, 0.0), 0..0);
        assert_eq!(visible_timeline_row_range(&rows, 25.0, 40.0), 1..1);
    }

    #[test]
    fn virtual_source_range_maps_to_grouped_timeline_rows() {
        let mut rows = vec![
            layout_row(0, 0.0, 20.0),
            layout_row(1, 30.0, 20.0),
            layout_row(4, 60.0, 20.0),
        ];
        rows[1].source_end_index = 3;

        assert_eq!(timeline_row_range_for_source_range(&rows, 2, 4), 1..3);
        assert_eq!(timeline_row_range_for_source_range(&rows, 5, 6), 3..3);
        assert_eq!(
            timeline_row_range_for_source_range::<NeoismAgentMessage>(&[], 0, 2),
            0..0
        );
    }

    #[test]
    fn stale_virtual_range_is_rejected_when_it_misses_registration_band() {
        let rows = vec![
            layout_row(0, 0.0, 20.0),
            layout_row(1, 30.0, 20.0),
            layout_row(2, 60.0, 20.0),
            layout_row(3, 90.0, 20.0),
        ];
        let stale_range = 0..1;
        let visible_range = visible_timeline_row_range(&rows, 55.0, 120.0);

        assert!(!timeline_row_range_intersects_viewport(
            &rows,
            stale_range,
            55.0,
            120.0
        ));
        assert_eq!(visible_range, 2..4);
    }

    fn layout_row(
        source_index: usize,
        top: f32,
        height: f32,
    ) -> TimelineLayoutRow<NeoismAgentMessage> {
        TimelineLayoutRow {
            source_index,
            source_end_index: source_index,
            top,
            height,
            display_text: None,
            display_message: None,
            markdown_blocks: None,
            tool_diff_sections: None,
            is_edit_tool: false,
        }
    }
