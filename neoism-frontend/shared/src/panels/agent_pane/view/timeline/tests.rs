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
fn background_completion_card_stays_visible_after_turn_settles() {
    // The durable completion card (id `background-task-{job}`) is a
    // transcript event: settling the turn or reloading the session must
    // never declutter it. An ordinary background_task_result tool CALL
    // (part-id identity — the model rereading retained output) keeps
    // normal trace settling.
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

    // Fully settled (reload / turn over): the completion card survives,
    // the reread tool row hides like any other trace.
    assert_eq!(
        timeline_message_visibility(&messages, None),
        vec![true, true, false, true]
    );
    // Live window: everything visible as before.
    assert_eq!(
        timeline_message_visibility(&messages, Some(1)),
        vec![true; messages.len()]
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

    let sections = prepared_message_tool_diff_sections(&patch).expect("diff sections");
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
fn anchor_distinguishes_duplicate_optimistic_empty_ids() {
    let messages = vec![
        text_message("", NeoismAgentMessageKind::User, "first"),
        text_message("", NeoismAgentMessageKind::User, "second"),
    ];
    let key = TimelineViewAnchorKey::for_source(&messages, 1).expect("anchor key");

    assert_eq!(resolve_timeline_view_anchor(&messages, &key), Some(1));
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
