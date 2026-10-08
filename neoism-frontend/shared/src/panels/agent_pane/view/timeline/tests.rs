use super::layout::{
    estimate_message_height, from_state_cache, into_state_cache,
    prepared_message_tool_diff_sections, timeline_message_visibility,
    timeline_row_range_for_source_range, timeline_row_range_intersects_viewport,
    visible_timeline_row_range,
};
use super::read_group::read_tool_group_at;

use super::*;

#[test]
fn streaming_status_reserves_every_visible_child_line() {
    assert_eq!(streaming_status_line_count(1, 0, 0), 1);
    assert_eq!(streaming_status_line_count(2, 0, 0), 2);
    assert_eq!(streaming_status_line_count(1, 1, 0), 2);
    assert_eq!(streaming_status_line_count(1, 0, 1), 2);
    assert_eq!(streaming_status_line_count(2, 1, 1), 4);
}

fn tool_message(id: &str, tool: &str, title: &str, status: &str) -> NeoismAgentMessage {
    NeoismAgentMessage {
        id: id.to_string(),
        kind: NeoismAgentMessageKind::Tool,
        title: title.to_string(),
        text: format!("{title} preview"),
        status: status.to_string(),
        tool: tool.to_string(),
        tool_batch_id: None,
        output_kind: NeoismAgentOutputKind::Text,
        lang: String::new(),
        line_offset: None,
        todos: Vec::new(),
        detail: format!("{title} detail"),
        usage: None,
        author: None,
        images: Vec::new(),
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
        tool_batch_id: None,
        output_kind: NeoismAgentOutputKind::Text,
        lang: String::new(),
        line_offset: None,
        todos: Vec::new(),
        detail: String::new(),
        usage: None,
        author: None,
        images: Vec::new(),
    }
}

#[test]
fn artifact_estimates_and_multi_message_prefixes_ignore_html_source_length() {
    let source = "<div>dashboard data</div>\n".repeat(500);
    let preview = format!("```neoism-html\n{source}```");
    let messages = [
        text_message("a", NeoismAgentMessageKind::Assistant, &preview),
        text_message("u", NeoismAgentMessageKind::User, "next prompt"),
        text_message(
            "b",
            NeoismAgentMessageKind::Assistant,
            &format!("{preview}\n{preview}"),
        ),
    ];
    let heights: Vec<_> = messages
        .iter()
        .map(|m| estimate_message_height(m, 900.0, 1.0))
        .collect();
    assert_eq!(heights, [354.0, 43.0, 674.0]);
    let mut top = 0.0;
    let rows = heights
        .iter()
        .enumerate()
        .map(|(index, height)| {
            let row = layout_row(index, top, *height);
            top += height + 18.0;
            row
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows.iter().map(|r| r.top).collect::<Vec<_>>(),
        [0.0, 372.0, 433.0]
    );
    let cache = lazy_cache(rows, 0, 3);
    assert_eq!(cache.content_height, 1107.0);
    let measurements =
        super::layout::timeline_virtual_row_measurements(&cache.rows, 18.0);
    assert_eq!(
        measurements.iter().map(|m| m.height).sum::<f32>(),
        cache.content_height
    );
}

#[test]
fn artifact_fence_completion_marks_owning_row_for_suffix_repatch() {
    let source = "<div>dashboard</div>\n".repeat(500);
    let incomplete = text_message(
        "a",
        NeoismAgentMessageKind::Assistant,
        &format!("```neoism-html\n{source}"),
    );
    let complete = text_message(
        "a",
        NeoismAgentMessageKind::Assistant,
        &format!("```neoism-html\n{source}```"),
    );
    let next = text_message("u", NeoismAgentMessageKind::User, "next");
    let mut row = layout_row(0, 0.0, 354.0);
    row.display_message = Some(incomplete);
    let cache = lazy_cache(vec![row, layout_row(1, 372.0, 43.0)], 0, 2);
    let mut dirty = TimelineDirtyMarks::default();
    super::layout::mark_changed_artifact_rows(
        &cache,
        &[complete.clone(), next.clone()],
        &mut dirty,
    );
    assert!(dirty.indices.contains(&0));
    assert_eq!(super::layout::patch_start_row(&cache, 0), Some(0));
    let mut current_cache = cache;
    current_cache.rows[0].display_message = Some(complete.clone());
    let mut unchanged = TimelineDirtyMarks::default();
    super::layout::mark_changed_artifact_rows(
        &current_cache,
        &[complete, next],
        &mut unchanged,
    );
    assert!(unchanged.indices.is_empty());
}

#[test]
fn lazy_tool_height_is_one_row_for_long_titles_and_outputs() {
    for status in ["pending", "running", "completed", "error"] {
        let mut message =
            tool_message("tool", "read", &"Read(long/path)".repeat(50), status);
        message.text = "output\n".repeat(1000);
        message.detail = "details\n".repeat(1000);
        for output_kind in [NeoismAgentOutputKind::Text, NeoismAgentOutputKind::Code] {
            message.output_kind = output_kind;
            for width in [160.0, 900.0] {
                assert_eq!(estimate_message_height(&message, width, 1.0), 30.0);
                assert_eq!(estimate_message_height(&message, width, 2.0), 60.0);
            }
        }
    }
}

#[test]
fn lazy_user_height_obeys_the_rendered_six_line_cap() {
    let long_paste = (0..200)
        .map(|index| format!("command line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let message = text_message("user", NeoismAgentMessageKind::User, &long_paste);

    assert_eq!(estimate_message_height(&message, 900.0, 1.0), 138.0);
}

#[test]
fn settled_turns_show_only_prompts_and_all_answer_text() {
    // Settled turns declutter exactly like the original design: reasoning,
    // tools, and edits are hidden. The one difference: assistant text is
    // NEVER masked — the old trailing-text-only rule wiped answers on
    // reload.
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
        text_message("a2", NeoismAgentMessageKind::Assistant, "Tests pass."),
        text_message("u2", NeoismAgentMessageKind::User, "explain"),
        text_message("a3", NeoismAgentMessageKind::Assistant, "Part one."),
        text_message("a4", NeoismAgentMessageKind::Assistant, "Part two."),
    ];

    // Fully settled (reload): reasoning + tool hidden; every text part and
    // both prompts survive.
    assert_eq!(
        timeline_message_visibility(&messages, None),
        vec![true, false, true, false, true, true, true, true, true]
    );
    // Live from index 1: everything visible.
    assert_eq!(
        timeline_message_visibility(&messages, Some(1)),
        vec![true; messages.len()]
    );
}

#[test]
fn live_window_reveals_trace_and_system_stays_hidden() {
    let messages = vec![
        text_message("u1", NeoismAgentMessageKind::User, "old question"),
        text_message("r1", NeoismAgentMessageKind::Reasoning, "old trace"),
        text_message("a1", NeoismAgentMessageKind::Assistant, "Old answer."),
        text_message("u2", NeoismAgentMessageKind::User, "new question"),
        text_message("r2", NeoismAgentMessageKind::Reasoning, "live trace"),
        tool_message("t2", "read", "Read(src/main.rs)", "running"),
        text_message("a2", NeoismAgentMessageKind::Assistant, "live progress"),
        text_message("s2", NeoismAgentMessageKind::System, "internal"),
    ];

    // Old turn's trace hidden; live turn's trace visible; System never
    // shows; text always shows.
    assert_eq!(
        timeline_message_visibility(&messages, Some(4)),
        vec![true, false, true, true, true, true, true, false]
    );
    // Fully settled: all trace hidden, prompts + every answer text still
    // visible.
    assert_eq!(
        timeline_message_visibility(&messages, None),
        vec![true, false, true, true, false, false, true, false]
    );
}

#[test]
fn location_notice_stays_visible_without_exposing_other_system_messages() {
    let mut location = text_message(
        "location",
        NeoismAgentMessageKind::System,
        "Switched location to /tmp/project",
    );
    location.tool = "location_notice".to_string();
    let messages = vec![
        text_message("internal", NeoismAgentMessageKind::System, "internal"),
        location,
    ];

    assert_eq!(
        timeline_message_visibility(&messages, None),
        vec![false, true]
    );
    assert!(super::render::display_timeline_message(&messages[0], false).is_none());
    assert!(super::render::display_timeline_message(&messages[1], false).is_some());
}

#[test]
fn live_boundary_reveals_only_the_current_turn_trace() {
    let messages = vec![
        text_message("u1", NeoismAgentMessageKind::User, "old question"),
        text_message("r1", NeoismAgentMessageKind::Reasoning, "old thought"),
        tool_message("t1", "read", "Read(old.rs)", "completed"),
        text_message("a1", NeoismAgentMessageKind::Assistant, "Old answer."),
        text_message("u2", NeoismAgentMessageKind::User, "new question"),
        text_message("r2", NeoismAgentMessageKind::Reasoning, "new thought"),
        tool_message("t2", "grep", "Grep(new)", "running"),
        text_message("a2", NeoismAgentMessageKind::Assistant, "New answer."),
    ];

    assert_eq!(
        timeline_message_visibility(&messages, Some(5)),
        vec![true, false, false, true, true, true, true, true]
    );
}

#[test]
fn final_answer_remains_visible_when_a_settled_turn_ends_on_a_tool() {
    let messages = vec![
        text_message("u1", NeoismAgentMessageKind::User, "fix it"),
        text_message("r1", NeoismAgentMessageKind::Reasoning, "planning"),
        text_message("a1", NeoismAgentMessageKind::Assistant, "Implemented."),
        tool_message("t1", "bash", "Bash(cargo test)", "completed"),
    ];

    assert_eq!(
        timeline_message_visibility(&messages, None),
        vec![true, false, true, false]
    );
}

#[test]
fn every_assistant_chunk_survives_settling_around_tools() {
    let messages = vec![
        text_message("u1", NeoismAgentMessageKind::User, "investigate"),
        text_message(
            "a-progress",
            NeoismAgentMessageKind::Assistant,
            "I am checking it.",
        ),
        tool_message("t1", "grep", "Grep(problem)", "completed"),
        text_message(
            "a-result",
            NeoismAgentMessageKind::Assistant,
            "The cause is fixed.",
        ),
        tool_message("t2", "bash", "Bash(cargo test)", "completed"),
        text_message("a-final", NeoismAgentMessageKind::Assistant, "Tests pass."),
    ];

    assert_eq!(
        timeline_message_visibility(&messages, None),
        vec![true, true, false, true, false, true]
    );
}

#[test]
fn background_completion_sentinel_never_renders_as_a_timeline_row() {
    // The synthetic completion sentinel (id `background-task-{job}`) exists
    // only to settle runtime activity. A genuine background_task_result tool
    // CALL has a normal part id and remains visible while its turn is live.
    let messages = vec![
        text_message("u1", NeoismAgentMessageKind::User, "start the build"),
        tool_message(
            "background-task-job-1",
            "background_task_result",
            "background_task_result",
            "completed",
        ),
        tool_message(
            "prt-reread-1",
            "background_task_result",
            "background_task_result",
            "completed",
        ),
        text_message("a1", NeoismAgentMessageKind::Assistant, "It finished."),
    ];

    // Fully settled: both tool rows are hidden like ordinary turn trace.
    assert_eq!(
        timeline_message_visibility(&messages, None),
        vec![true, false, false, true]
    );
    // Live: the genuine model tool call is shown; the synthetic sentinel is
    // still hidden and cannot disturb visible transcript order.
    assert_eq!(
        timeline_message_visibility(&messages, Some(1)),
        vec![true, false, true, true]
    );
}

#[test]
fn runtime_system_rows_stay_hidden_even_inside_the_live_window() {
    let messages = vec![
        text_message(
            "msg_background_completion_job_1",
            NeoismAgentMessageKind::System,
            "Background shell task finished.",
        ),
        text_message(
            "msg_subtask_completion_child_1",
            NeoismAgentMessageKind::System,
            "Subagent task finished.",
        ),
    ];

    assert_eq!(
        timeline_message_visibility(&messages, Some(0)),
        vec![false, false]
    );
}

#[test]
fn subagent_runtime_notification_never_paints_during_role_races() {
    let text = "Subagent finished.\ntask_id: ses_child\nstatus: completed";
    let messages = vec![
        text_message("live-user", NeoismAgentMessageKind::User, text),
        text_message("live-assistant", NeoismAgentMessageKind::Assistant, text),
    ];

    assert_eq!(
        timeline_message_visibility(&messages, Some(0)),
        vec![false, false]
    );
    assert!(super::render::display_timeline_message(&messages[0], false).is_none());
    assert!(super::render::display_timeline_message(&messages[1], false).is_none());
}

#[test]
fn subtasks_and_compaction_are_visible_live_and_hidden_after_reload() {
    let messages = vec![
        text_message("u1", NeoismAgentMessageKind::User, "work"),
        text_message("subtask", NeoismAgentMessageKind::Subtask, "exploring"),
        text_message("compact", NeoismAgentMessageKind::Compaction, "summary"),
        text_message("answer", NeoismAgentMessageKind::Assistant, "Done."),
    ];

    assert_eq!(
        timeline_message_visibility(&messages, Some(1)),
        vec![true, true, true, true]
    );
    assert_eq!(
        timeline_message_visibility(&messages, None),
        vec![true, false, false, true]
    );
}

#[test]
fn stale_live_boundary_past_the_transcript_is_safely_treated_as_settled() {
    let messages = vec![
        text_message("u1", NeoismAgentMessageKind::User, "question"),
        text_message("r1", NeoismAgentMessageKind::Reasoning, "thought"),
        text_message("a1", NeoismAgentMessageKind::Assistant, "answer"),
    ];

    assert_eq!(
        timeline_message_visibility(&messages, Some(usize::MAX)),
        timeline_message_visibility(&messages, None)
    );
}

#[test]
fn live_read_tools_group_only_with_a_shared_batch_identity() {
    let mut messages = vec![
        tool_message("read-a", "read", "Read(src/a.rs)", "completed"),
        tool_message("grep-b", "grep", "Grep(Thing)", "completed"),
        tool_message("list-c", "list", "List(src)", "running"),
    ];
    for message in &mut messages {
        message.tool_batch_id = Some("batch-a".to_string());
    }

    let (end, group) = read_tool_group_at(&messages, 0).expect("group");

    assert_eq!(end, 3);
    assert_eq!(group.id, "read-a..");
    assert_eq!(group.tool, "tool_group");
    assert_eq!(group.status, "running");
    assert!(group.text.contains("Read(src/a.rs)"));
    assert!(group.detail.contains("Read(src/a.rs)"));
    assert!(group.detail.contains("Read(src/a.rs) detail"));
}

#[test]
fn read_group_identity_survives_append_and_status_updates() {
    let mut messages = vec![
        tool_message("read-a", "read", "Read(src/same.rs)", "completed"),
        tool_message("read-b", "read", "Read(src/same.rs)", "running"),
        tool_message("read-c", "read", "Read(src/c.rs)", "completed"),
    ];
    for message in &mut messages {
        message.tool_batch_id = Some("batch-a".to_string());
    }
    let (_, before) = read_tool_group_at(&messages, 0).unwrap();
    messages[1].status = "completed".to_string();
    messages.push(tool_message(
        "read-d",
        "read",
        "Read(src/d.rs)",
        "completed",
    ));
    messages[3].tool_batch_id = Some("batch-a".to_string());
    let (_, after) = read_tool_group_at(&messages, 0).unwrap();
    assert_eq!(before.id, after.id);
    assert_eq!(after.status, "completed");
    for group in [&before, &after] {
        assert!(group.text.lines().any(|line| line.starts_with("read-a\t")));
        assert!(group.text.lines().any(|line| line.starts_with("read-b\t")));
        assert!(group
            .detail
            .lines()
            .any(|line| line.starts_with("read-a\t")));
        assert!(group
            .detail
            .lines()
            .any(|line| line.starts_with("read-b\t")));
    }
}

#[test]
fn read_group_animation_dirties_its_source_row_without_new_messages() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        tool_message("read-a", "read", "Read(src/a.rs)", "completed"),
        tool_message("read-b", "read", "Read(src/b.rs)", "completed"),
        tool_message("read-c", "read", "Read(src/c.rs)", "completed"),
    ];
    for message in &mut pane.messages {
        message.tool_batch_id = Some("batch-a".to_string());
    }
    let (_, group) = read_tool_group_at(&pane.messages, 0).unwrap();
    pane.register_tool_hit_rect(group.id.clone(), [0.0, 0.0, 300.0, 30.0]);
    assert!(pane.toggle_tool_at(10.0, 10.0));
    let _ = pane.take_timeline_dirty_marks();
    let mut dirty = TimelineDirtyMarks::default();
    super::layout::mark_animating_tool_rows_dirty(&pane, &mut dirty);
    assert!(dirty.indices.contains(&0));
    assert!(pane.tool_expand_animating(&group.id));
    assert!(pane.tool_expand_animating("read-a"));
    assert!(!pane.tool_expand_animating("read-b"));
}

#[test]
fn adjacent_reads_without_batch_provenance_are_never_bundled() {
    let mut messages = (0..12)
        .map(|i| {
            tool_message(
                &format!("read-{i}"),
                "read",
                "Read(src/lib.rs)",
                "completed",
            )
        })
        .collect::<Vec<_>>();
    for batch_id in [None, Some(String::new()), Some("  ".to_string())] {
        for message in &mut messages {
            message.tool_batch_id = batch_id.clone();
        }
        for index in 0..messages.len() {
            assert!(read_tool_group_at(&messages, index).is_none());
        }
    }
}

#[test]
fn two_member_batches_bundle_without_merging_neighboring_batches() {
    let mut messages = (0..4)
        .map(|i| {
            tool_message(
                &format!("read-{i}"),
                "read",
                "Read(src/lib.rs)",
                "completed",
            )
        })
        .collect::<Vec<_>>();
    for (index, message) in messages.iter_mut().enumerate() {
        message.tool_batch_id = Some(format!("batch-{}", index / 2));
    }
    let (end, first) = read_tool_group_at(&messages, 0).unwrap();
    assert_eq!(end, 2);
    assert_eq!(first.id, "read-0..");
    let (end, second) = read_tool_group_at(&messages, 2).unwrap();
    assert_eq!(end, 4);
    assert_eq!(second.id, "read-2..");
    messages[1].tool_batch_id = None;
    assert!(read_tool_group_at(&messages, 0).is_none());
}

#[test]
fn live_grouping_keeps_short_or_failed_runs_separate() {
    let short = vec![
        tool_message("read-a", "read", "Read(src/a.rs)", "completed"),
        tool_message("grep-b", "grep", "Grep(Thing)", "completed"),
    ];
    assert!(read_tool_group_at(&short, 0).is_none());

    let mut failed = vec![
        tool_message("read-a", "read", "Read(src/a.rs)", "completed"),
        tool_message("grep-b", "grep", "Grep(Thing)", "error"),
        tool_message("list-c", "list", "List(src)", "completed"),
    ];
    for message in &mut failed {
        message.tool_batch_id = Some("batch-a".to_string());
    }
    assert!(read_tool_group_at(&failed, 0).is_none());
    failed[1].tool = "tool_group".to_string();
    failed[1].status = "completed".to_string();
    assert!(read_tool_group_at(&failed, 0).is_none());
}

#[test]
fn prepared_tool_diff_sections_survive_layout_cache_roundtrip() {
    let mut patch = tool_message("patch-1", "apply_patch", "Apply patch", "completed");
    patch.detail = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new
"
    .to_string();

    assert!(prepared_message_tool_diff_sections(&patch, false).is_some());
    let mut pending = patch.clone();
    pending.status = "running".to_string();
    assert!(prepared_message_tool_diff_sections(&pending, false).is_none());
    assert!(prepared_message_tool_diff_sections(&pending, true).is_none());
    let read = tool_message("read", "read", "Read(src/lib.rs)", "completed");
    assert!(prepared_message_tool_diff_sections(&read, false).is_none());
    let sections =
        prepared_message_tool_diff_sections(&patch, true).expect("diff sections");
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
        estimated_suffix_start: 1,
    };

    let restored = from_state_cache(into_state_cache(cache));
    assert_eq!(
        restored.estimated_prefix_rows, 0,
        "web/state round-trip stays fully exact"
    );
    assert_eq!(restored.estimated_suffix_start, restored.rows.len());
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
fn anchor_distinguishes_duplicate_optimistic_empty_ids() {
    let messages = vec![
        text_message("", NeoismAgentMessageKind::User, "first"),
        text_message("", NeoismAgentMessageKind::User, "second"),
    ];
    let key = TimelineViewAnchorKey::for_source(&messages, 1).expect("anchor key");

    assert_eq!(resolve_timeline_view_anchor(&messages, &key), Some(1));
}

#[test]
fn anchor_source_match_requires_the_same_transcript_shape() {
    let messages = vec![text_message(
        "anchor",
        NeoismAgentMessageKind::Assistant,
        "held",
    )];
    let key = TimelineViewAnchorKey::for_source(&messages, 0).expect("anchor key");

    assert!(key.is_for_source(0, 1));
    assert!(!key.is_for_source(0, 2));
    assert!(!key.is_for_source(1, 1));
}

#[test]
fn optimistic_anchor_survives_durable_id_transition() {
    let before = vec![
        text_message("", NeoismAgentMessageKind::User, "optimistic"),
        text_message("", NeoismAgentMessageKind::User, "other"),
    ];
    let key = TimelineViewAnchorKey::for_source(&before, 0).expect("anchor key");
    let after = vec![
        text_message("message-1", NeoismAgentMessageKind::User, "optimistic"),
        text_message("", NeoismAgentMessageKind::User, "other"),
    ];

    assert_eq!(resolve_timeline_view_anchor(&after, &key), Some(0));
}

#[test]
fn duplicate_durable_anchor_does_not_jump_after_its_row_is_removed() {
    let before = vec![
        text_message("duplicate", NeoismAgentMessageKind::User, "first"),
        text_message("duplicate", NeoismAgentMessageKind::User, "second"),
    ];
    let key = TimelineViewAnchorKey::for_source(&before, 1).expect("anchor key");
    let after = vec![text_message(
        "duplicate",
        NeoismAgentMessageKind::User,
        "first",
    )];

    assert_eq!(resolve_timeline_view_anchor(&after, &key), None);
}

#[test]
fn legacy_optimistic_anchor_can_move_and_gain_a_durable_id() {
    let before = vec![
        text_message("", NeoismAgentMessageKind::User, "optimistic"),
        text_message("other", NeoismAgentMessageKind::Assistant, "answer"),
    ];
    let key = TimelineViewAnchorKey::for_source(&before, 0).expect("anchor key");
    let after = vec![
        text_message("older", NeoismAgentMessageKind::User, "older"),
        text_message("message-1", NeoismAgentMessageKind::User, "optimistic"),
        text_message("other", NeoismAgentMessageKind::Assistant, "answer"),
    ];

    assert_eq!(resolve_timeline_view_anchor(&after, &key), Some(1));
}

#[test]
fn anchor_resolution_distinguishes_tail_append_from_history_prepend_by_identity() {
    let before = vec![
        text_message("a", NeoismAgentMessageKind::User, "a"),
        text_message("anchor", NeoismAgentMessageKind::Assistant, "held"),
    ];
    let key = TimelineViewAnchorKey::for_source(&before, 1).expect("anchor key");
    let appended = vec![
        before[0].clone(),
        before[1].clone(),
        text_message("tail", NeoismAgentMessageKind::Assistant, "tail"),
    ];
    let prepended = vec![
        text_message("older", NeoismAgentMessageKind::User, "older"),
        before[0].clone(),
        before[1].clone(),
    ];

    assert_eq!(resolve_timeline_view_anchor(&appended, &key), Some(1));
    assert_eq!(resolve_timeline_view_anchor(&prepended, &key), Some(2));
}

#[test]
fn grouped_row_anchor_includes_final_child() {
    let mut rows = vec![layout_row(2, 40.0, 20.0)];
    rows[0].source_end_index = 4;

    assert_eq!(
        timeline_row_for_anchor_source(&rows, 4).map(|row| row.source_index),
        Some(2)
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

fn lazy_cache(
    rows: Vec<TimelineLayoutRow<NeoismAgentMessage>>,
    estimated_prefix_rows: usize,
    estimated_suffix_start: usize,
) -> TimelineLayoutCache<NeoismAgentMessage> {
    let content_height = rows.last().map(|row| row.top + row.height).unwrap_or(0.0);
    TimelineLayoutCache {
        epoch: 1,
        source_len: rows.len(),
        width_bucket: 100,
        scale_bucket: 4,
        gap_bucket: 72,
        content_height,
        pages: Vec::new(),
        rows,
        estimated_prefix_rows,
        estimated_suffix_start,
    }
}

#[test]
fn prepend_rejects_estimated_edit_rows_in_reused_cache() {
    let mut rows = vec![
        layout_row(0, 0.0, 48.0),
        layout_row(1, 60.0, 48.0),
        layout_row(2, 120.0, 48.0),
    ];
    rows[2].is_edit_tool = true;
    assert!(!super::layout::prepend_cache_is_exact(&lazy_cache(
        rows.clone(),
        0,
        2
    )));
    assert!(!super::layout::prepend_cache_is_exact(&lazy_cache(
        rows.clone(),
        1,
        3
    )));
    assert!(super::layout::prepend_cache_is_exact(&lazy_cache(
        rows, 0, 3
    )));
}

#[test]
fn streaming_tail_patch_cannot_strand_estimated_edit_rows() {
    let mut rows = (0..5)
        .map(|index| layout_row(index, index as f32 * 60.0, 48.0))
        .collect::<Vec<_>>();
    rows[2].is_edit_tool = true;
    rows[3].is_edit_tool = true;
    let cache = lazy_cache(rows, 0, 2);

    // Only the first two rows were measured. Streaming at the tail must not
    // patch row 4 exactly while leaving short Edit rows 2 and 3 in the middle.
    assert_eq!(super::layout::patch_start_row(&cache, 4), None);
    assert!(!super::layout::prepend_cache_is_exact(&cache));
    // Starting at the first estimated row removes the entire estimated suffix.
    assert_eq!(super::layout::patch_start_row(&cache, 2), Some(2));
    assert_eq!(super::layout::patch_start_row(&cache, 1), Some(1));
}

#[test]
fn lazy_cache_covers_a_mid_history_exact_window() {
    let rows = vec![
        layout_row(0, 0.0, 100.0),
        layout_row(1, 120.0, 100.0),
        layout_row(2, 240.0, 100.0),
        layout_row(3, 360.0, 100.0),
        layout_row(4, 480.0, 100.0),
        layout_row(5, 600.0, 100.0),
    ];
    let cache = lazy_cache(rows, 1, 5);
    // content_h=700, viewport=100, offset=350 -> scroll_top=250.
    // Exact rows [1..5) span 120..600, so both edges keep a viewport of lead.
    assert!(super::layout::lazy_cache_covers_viewport_for_test(
        &cache, 350.0, 100.0
    ));
}

#[test]
fn lazy_cache_rebuilds_when_scroll_nears_estimated_suffix() {
    let rows = vec![
        layout_row(0, 0.0, 100.0),
        layout_row(1, 120.0, 100.0),
        layout_row(2, 240.0, 100.0),
        layout_row(3, 360.0, 100.0),
        layout_row(4, 480.0, 100.0),
        layout_row(5, 600.0, 100.0),
    ];
    let cache = lazy_cache(rows, 2, 4);
    // Scrolling toward later messages (smaller offset) pushes the viewport
    // into the estimated suffix, so the cache must rebuild.
    assert!(!super::layout::lazy_cache_covers_viewport_for_test(
        &cache, 20.0, 100.0
    ));
}
