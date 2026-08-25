use super::*;
use neoism_ui::panels::agent_pane::state::side_panel::STATUS_LABEL_GRACE;
use std::fs;

#[test]
fn file_mention_options_filter_workspace_trash_dirs() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
    for ignored in [".claude", ".cache", ".neoism", "node_modules", "target"] {
        fs::create_dir_all(root.path().join(ignored)).unwrap();
        fs::write(root.path().join(ignored).join("main.rs"), "trash").unwrap();
    }

    let search = neoism_agent_workspace_search_fff::FffWorkspaceSearchService::new();
    let pin = Mutex::new(None);
    let options = file_mention_options(&search, &pin, root.path(), "main", 20);
    let values = options
        .into_iter()
        .map(|option| option.value)
        .collect::<Vec<_>>();

    assert_eq!(values, vec!["src/main.rs"]);
}

#[test]
fn idle_clears_status_but_keeps_trace_until_session_reset() {
    let mut pane = NeoismAgentPane::default();
    pane.messages.push(NeoismAgentMessage::user("question"));

    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert!(pane.is_streaming());
    assert_eq!(pane.streaming_label(), "Crafting");

    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("working").with_id("reasoning-1"),
    );
    assert_eq!(pane.timeline_live_trace_start, Some(1));

    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert!(!pane.is_streaming());
    // A transient idle reading holds the displayed label; sustained
    // idle past the grace window clears it for real.
    assert_eq!(pane.streaming_label(), "Crafting");
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_label(), "");
    assert_eq!(pane.streaming_elapsed_seconds(), None);
    assert_eq!(pane.timeline_live_trace_start, Some(1));

    pane.reset_session_runtime_ui();
    assert_eq!(pane.timeline_live_trace_start, None);
}

#[test]
fn viewed_subagent_terminal_status_clears_and_latches_activity() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child-1".to_string());
    pane.parent_session_id = Some("parent".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);

    assert!(pane.reconcile_viewed_subagent_runtime("child-1", BranchStatus::Completed));
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.child_part_can_drive_streaming("child-1"));
}

fn child_permission(id: &str) -> NeoismAgentPendingPermission {
    NeoismAgentPendingPermission {
        id: id.to_string(),
        session_id: "child-1".to_string(),
        parent_session_id: Some("parent".to_string()),
        source_agent: None,
        source_title: None,
        title: "Run command".to_string(),
        permission: "bash".to_string(),
        patterns: Vec::new(),
        selected: 0,
        responding: false,
    }
}

#[test]
fn completed_child_ignores_late_permission_reply_event() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.enqueue_pending_permission(child_permission("perm-1"));
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::PermissionReplied {
            request_id: "perm-1".to_string(),
            session_id: Some("child-1".to_string()),
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Completed)
    );
    assert!(pane.side_panel.branch_terminal_locked("child-1"));
}

#[test]
fn completed_child_ignores_stale_permission_request_event() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::PermissionAsked(child_permission(
            "perm-late",
        ))],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Completed)
    );
    assert!(pane.side_panel.branch_terminal_locked("child-1"));
}

#[test]
fn authoritative_busy_child_event_reopens_completed_branch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::SubagentStatus {
            session_id: "child-1".to_string(),
            status: "busy".to_string(),
            started_at: Some(2),
            title: None,
            agent: None,
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.side_panel.branch_activity("child-1").map(|a| a.status),
        Some(BranchStatus::Active)
    );
    assert!(!pane.side_panel.branch_terminal_locked("child-1"));
}

#[test]
fn active_subagent_part_updates_do_not_restart_waiting_clock() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel
        .set_subagents(vec![NeoismAgentSessionEntry::new(
            "child-1", "child", "explore",
        )
        .with_runtime_status(Some("running".to_string()))]);
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();
    let original = pane.subagent_waiting_started_at;

    assert!(pane.note_subagent_part_activity(
        "child-1".to_string(),
        BranchStatus::Active,
        Some("read".to_string()),
        Some(1),
    ));
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.subagent_waiting_started_at, original);
}

#[test]
fn main_agent_verb_wins_while_it_streams_over_running_subagents() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();
    let original_clock = pane.subagent_waiting_started_at;
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );

    // The main agent starts talking (the user sent a prompt / the model
    // is responding): its own verb always wins over the aggregate
    // sub-agents label while it is actively streaming.
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::Generating
    );
    assert_eq!(pane.streaming_label(), "Crafting");

    // The main agent stops while the same child keeps running: only now
    // does "Sub-agents working" take over — and on the SAME waiting
    // clock (no restart, the child never stopped).
    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert_eq!(pane.streaming_label(), "Sub-agents working");
    assert_eq!(pane.subagent_waiting_started_at, original_clock);
}

#[test]
fn failed_subagent_refresh_preserves_sidebar_and_footer_activity() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    assert_eq!(pane.active_subagent_count(), 1);

    pane.side_panel.mark_subagent_tree_dirty();
    let generation = pane
        .side_panel
        .begin_subagent_refresh()
        .expect("refresh generation");
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SidePanelSubagentsRefreshed {
            session_id: "parent".to_string(),
            generation,
            result: Err("temporary transport failure".to_string()),
        })
        .unwrap();

    pane.drain_background_updates();

    assert_eq!(
        pane.side_panel
            .subagents()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["parent", "child"]
    );
    assert_eq!(pane.active_subagent_count(), 1);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert!(
        !pane.side_panel.should_refresh_subagents(),
        "a failed recovery snapshot must wait for the next lifecycle/reconnect edge"
    );
}

#[test]
fn event_stream_reconnect_requests_exactly_one_recovery_snapshot() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    assert!(!pane.side_panel.should_refresh_subagents());

    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::EventStreamReconnected],
    ));
    pane.drain_server_updates();

    assert!(pane.side_panel.should_refresh_subagents());
    let generation = pane
        .side_panel
        .begin_subagent_refresh()
        .expect("one reconnect snapshot");
    assert!(pane.side_panel.complete_subagent_refresh(generation));
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    assert!(!pane.side_panel.should_refresh_subagents());
}

#[test]
fn live_subagent_activity_invalidates_older_terminal_snapshot() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.messages = vec![task_tool_message("child", "running")];
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Active, Some(1));
    pane.side_panel.mark_subagent_tree_dirty();
    let stale_generation = pane
        .side_panel
        .begin_subagent_refresh()
        .expect("start stale refresh");

    // A newer live delta arrives while the old snapshot is still in flight.
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::SubagentActivity {
            session_id: "child".to_string(),
            status: "active".to_string(),
            current_tool: Some("read".to_string()),
            started_at: Some(2),
        }],
    ));
    pane.drain_server_updates();

    // The older split snapshot falsely says the same child completed. Its
    // invalidated generation must be ignored atomically, without one frame
    // of a completed task card or missing footer/sidebar row.
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SidePanelSubagentsRefreshed {
            session_id: "parent".to_string(),
            generation: stale_generation,
            result: Ok(vec![
                NeoismAgentSessionEntry::new("parent", "main session", "return"),
                NeoismAgentSessionEntry::new("child", "child", "explore")
                    .with_runtime_status(Some("completed".to_string())),
            ]),
        })
        .unwrap();
    pane.drain_background_updates();
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.active_subagent_count(), 1);
    assert_eq!(pane.messages[0].status, "running");
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    assert!(pane
        .side_panel
        .subagents()
        .iter()
        .any(|entry| entry.id == "child"));
}

#[test]
fn stale_running_snapshot_cannot_resurrect_task_or_footer_after_completion() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.messages = vec![task_tool_message("child", "running")];
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Active, Some(1));
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Completed, None);

    // A recovery request that began before completion returns stale running.
    // Side-panel reconciliation keeps the terminal lock, and task/runtime
    // reconciliation must consume that effective state rather than raw data.
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "child", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.reconcile_task_message_statuses();
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.active_subagent_count(), 0);
    assert_eq!(pane.messages[0].status, "completed");
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(pane.side_panel.branch_terminal_locked("child"));
}

#[test]
fn child_background_completion_stays_out_of_main_transcript() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [AgentSessionUpdate::BackgroundTaskCompleted {
            session_id: "child".to_string(),
            job_id: "job-child".to_string(),
            status: "completed".to_string(),
        }],
    ));

    pane.drain_server_updates();

    assert!(pane
        .messages
        .iter()
        .all(|message| message.id != "background-task-job-child"));
    assert!(pane.session_cache["child"]
        .messages
        .iter()
        .any(|message| message.id == "background-task-job-child"));
}

#[test]
fn background_completion_card_survives_messages_snapshot_replacement() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [
            // The live completion event injects the card…
            AgentSessionUpdate::BackgroundTaskCompleted {
                session_id: "parent".to_string(),
                job_id: "job-1".to_string(),
                status: "completed".to_string(),
            },
            // …then a full-transcript refresh lands BEFORE the server's
            // queued completion prompt drained — no trace of the card in
            // the snapshot. It must survive the replacement.
            AgentSessionUpdate::Messages {
                messages: vec![
                    NeoismAgentMessage::user("kick off the build").with_id("u-1"),
                    NeoismAgentMessage::assistant("Started.").with_id("a-1"),
                ],
                oldest_cursor: None,
            },
        ],
    ));

    pane.drain_server_updates();

    assert_eq!(pane.messages.len(), 3);
    assert_eq!(pane.messages[2].id, "background-task-job-1");
}

#[test]
fn server_background_completion_copy_replaces_client_card_without_duplicate() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    // Persisted runtime prompt as the shared mapping regenerates it —
    // the SAME durable id the live event used.
    let server_copy = neoism_ui::panels::agent_pane::api_mapping::message_blocks(&json!({
        "info": {
            "id": "msg_background_completion_job-1",
            "role": "user"
        },
        "parts": [{
            "id": "prt-background-done",
            "type": "text",
            "text": "Background shell task finished.\njob_id: job-1\nstatus: completed"
        }]
    }))
    .into_iter()
    .map(NeoismAgentMessage::from)
    .next()
    .expect("mapped completion card");
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "parent",
        [
            AgentSessionUpdate::BackgroundTaskCompleted {
                session_id: "parent".to_string(),
                job_id: "job-1".to_string(),
                status: "completed".to_string(),
            },
            AgentSessionUpdate::Messages {
                messages: vec![
                    NeoismAgentMessage::user("kick off the build").with_id("u-1"),
                    server_copy,
                ],
                oldest_cursor: None,
            },
        ],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| message.id == "background-task-job-1")
            .count(),
        1,
        "server copy replaces the client copy, no duplicate"
    );
}

#[test]
fn retry_status_includes_a_compact_provider_reason() {
    let mut pane = NeoismAgentPane::default();
    pane.note_streaming(
        NeoismAgentStreamingState::Retrying,
        Some("Our servers are currently overloaded. Please try again later.".to_string()),
    );

    assert_eq!(pane.streaming_label(), "Retrying · Provider overloaded");
}

#[test]
fn expired_wordmark_click_does_not_drive_animation() {
    let mut pane = NeoismAgentPane::default();
    pane.wordmark.click_started = Some(
        Instant::now()
            .checked_sub(WORDMARK_CLICK_ANIMATION + Duration::from_millis(1))
            .unwrap(),
    );

    assert_ne!(pane.animation_reason(), Some("wordmark"));
}

#[test]
fn fresh_wordmark_click_drives_short_animation() {
    let mut pane = NeoismAgentPane::default();
    pane.wordmark.click_started = Some(Instant::now());

    assert_eq!(pane.animation_reason(), Some("wordmark"));
}

#[test]
fn connected_idle_event_stream_does_not_drive_animation() {
    let mut pane = NeoismAgentPane::default();
    pane.event_stream = Some(AgentSessionEventStream::connected_for_test("sess-1"));

    assert_eq!(pane.animation_reason(), Some("agent_home_wordmark"));
}

#[test]
fn subagent_session_suppresses_stale_main_composer_cursor() {
    let mut pane = NeoismAgentPane::default();
    let rect = [10.0, 20.0, 2.0, 18.0];
    pane.set_cursor_rect(Some(rect));
    assert_eq!(pane.cursor_rect(), Some(rect));

    pane.parent_session_id = Some("main".to_string());
    assert_eq!(pane.cursor_rect(), None);
}

#[test]
fn attached_session_counts_as_conversation_before_messages_load() {
    let mut pane = NeoismAgentPane::default();

    assert!(!pane.has_conversation());

    pane.session_id = Some("session-1".to_string());

    assert!(pane.has_conversation());
}

#[test]
fn running_background_task_count_tracks_started_and_collected_jobs() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1 (use this with background_task_result)\nstatus: running\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);
    pane.ensure_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 1);
    assert!(pane.has_status_activity());
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::BackgroundTasks
    );
    assert_eq!(pane.streaming_label(), "Background");
    assert!(pane.streaming_elapsed_seconds().is_some());
    assert!(!pane.background_task_details_expanded());
    assert_eq!(
        pane.active_background_task_summaries(),
        vec!["job-1 · running · cargo build".to_string()]
    );
    let mut running_result = NeoismAgentMessage::tool(
        "Background Task Result",
        "job_id: job-1\nstatus: running",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    running_result.detail = running_result.text.clone();
    pane.messages.push(running_result);
    pane.ensure_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 1);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::BackgroundTasks
    );

    let mut result = NeoismAgentMessage::tool(
        "Background Task Result",
        "job_id: job-1\nstatus: completed",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    result.detail = result.text.clone();
    pane.messages.push(result);
    pane.ensure_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    // The display grace hold bridges the collection edge; sustained idle
    // then clears it.
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::BackgroundTasks
    );
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.background_task_details_expanded());
}

#[test]
fn completed_background_task_tool_is_not_counted_as_running() {
    let mut pane = NeoismAgentPane::default();
    let mut task = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: completed\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    task.detail = task.text.clone();
    pane.messages.push(task);
    pane.ensure_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_elapsed_seconds(), None);
    assert!(pane.active_background_task_summaries().is_empty());
}

#[test]
fn historical_unmatched_background_task_is_not_live_activity() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: stale-job\nstatus: running\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.animation_reason(), None);
}

#[test]
fn runtime_background_finish_notice_clears_running_job() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);

    let mut notice = NeoismAgentMessage::assistant(
            "A background shell task has finished.\njob_id: job-1\nstatus: completed\ncommand: cargo build",
        );
    notice.detail = notice.text.clone();
    pane.messages.push(notice);
    pane.ensure_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert_eq!(pane.streaming_elapsed_seconds(), None);
    assert!(pane.active_background_task_summaries().is_empty());
}

#[test]
fn empty_background_task_snapshot_clears_stale_running_jobs() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);

    let mut snapshot = NeoismAgentMessage::tool(
        "Background tasks",
        "No background tasks are running.",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    snapshot.detail = snapshot.text.clone();
    pane.messages.push(snapshot);
    pane.ensure_background_task_activity_clock();

    assert_eq!(pane.running_background_task_count(), 0);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(!pane.has_status_activity());
    assert!(pane.active_background_task_summaries().is_empty());
}

#[test]
fn cancelled_background_task_clears_running_job() {
    let mut pane = NeoismAgentPane::default();
    let mut started = NeoismAgentMessage::tool(
        "Background Task",
        "job_id: job-1\nstatus: running\ncommand: cargo build",
        "completed",
        "background_task",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    started.detail = started.text.clone();
    pane.messages.push(started);
    let mut cancelled = NeoismAgentMessage::tool(
        "Background task",
        "job_id: job-1\nstatus: cancelled",
        "completed",
        "background_task_result",
        NeoismAgentOutputKind::Text,
        "text",
        Vec::new(),
    );
    cancelled.detail = cancelled.text.clone();
    pane.messages.push(cancelled);

    assert_eq!(pane.running_background_task_count(), 0);
    assert!(pane.active_background_task_summaries().is_empty());
}

#[test]
fn abort_session_queues_outbound_command_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    pane.abort_session();

    assert!(!pane.is_streaming());
    assert!(pane.abort_requested_at.is_some());
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::AbortSession]
    );
}

#[test]
fn abort_without_session_does_not_queue_outbound_command() {
    let mut pane = NeoismAgentPane::default();

    pane.abort_session();

    assert!(pane.drain_pending_outbound().is_empty());
}

#[test]
fn warm_switch_activates_cached_session_without_outbound_or_network_repair() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.messages = vec![NeoismAgentMessage::user("root")];
    let mut cached = CachedAgentSession::live_only();
    cached.hydrated = true;
    cached.messages = vec![NeoismAgentMessage::assistant("child")];
    cached.state.parent_id = Some("sess-1".to_string());
    cached.timeline_layout_epoch = 42;
    cached.runtime.queued_prompt_count = 2;
    cached
        .runtime
        .note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.session_cache.insert("sess-2".to_string(), cached);
    pane.runtime_hydrated_sessions.insert("sess-2".to_string());

    pane.switch_session("sess-2".to_string());

    assert_eq!(pane.session_id.as_deref(), Some("sess-2"));
    assert_eq!(pane.messages[0].text, "child");
    assert_eq!(pane.timeline_layout_epoch, 42);
    assert_eq!(pane.queued_prompt_count, 2);
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    assert_eq!(pane.session_cache["sess-1"].messages[0].text, "root");
    assert!(pane.drain_pending_outbound().is_empty());
    assert!(!pane.runtime_status_requests.contains_key("sess-2"));
}

#[test]
fn subagent_visit_preserves_parent_live_tool_trace() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("question").with_id("user-root"),
        task_tool_message("child", "completed"),
    ];
    pane.timeline_live_trace_start = Some(1);
    pane.timeline_live_trace_anchor = Some("user-root".to_string());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".to_string());
    child.messages = vec![NeoismAgentMessage::assistant("child answer")];
    pane.session_cache.insert("child".to_string(), child);
    pane.runtime_hydrated_sessions
        .extend(["root".to_string(), "child".to_string()]);

    pane.switch_session("child".to_string());
    pane.switch_session("root".to_string());

    assert_eq!(pane.timeline_live_trace_start, Some(1));
    assert_eq!(
        pane.timeline_live_trace_anchor.as_deref(),
        Some("user-root")
    );
}

#[test]
fn leaving_conversation_family_settles_live_tool_trace() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root-a".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("question").with_id("user-a"),
        task_tool_message("child-a", "completed"),
    ];
    pane.timeline_live_trace_start = Some(1);
    pane.timeline_live_trace_anchor = Some("user-a".to_string());
    let mut other = CachedAgentSession::live_only();
    other.hydrated = true;
    other.messages = vec![NeoismAgentMessage::assistant("other conversation")];
    pane.session_cache.insert("root-b".to_string(), other);
    pane.runtime_hydrated_sessions
        .extend(["root-a".to_string(), "root-b".to_string()]);

    pane.switch_session("root-b".to_string());
    pane.switch_session("root-a".to_string());

    assert_eq!(pane.timeline_live_trace_start, None);
    assert_eq!(pane.timeline_live_trace_anchor, None);
}

#[test]
fn opening_ongoing_child_reveals_already_emitted_tool_history() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".to_string());
    child.messages = vec![
        NeoismAgentMessage::user("first investigation"),
        NeoismAgentMessage::tool(
            "Read(src/lib.rs)",
            "src/lib.rs",
            "completed",
            "read",
            NeoismAgentOutputKind::Text,
            "rust",
            Vec::new(),
        ),
        NeoismAgentMessage::user("continue the investigation"),
        NeoismAgentMessage::reasoning("checking prior state"),
    ];
    child
        .runtime
        .note_streaming(NeoismAgentStreamingState::Working, Some("Read".to_string()));
    pane.session_cache.insert("child".to_string(), child);
    pane.runtime_hydrated_sessions.insert("child".to_string());

    pane.switch_session("child".to_string());

    assert_eq!(pane.timeline_live_trace_start, Some(0));
    assert_eq!(pane.messages.len(), 4);
}

#[test]
fn cold_runtime_hydration_reveals_ongoing_child_tool_history() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("investigate"),
        NeoismAgentMessage::tool(
            "Grep(query)",
            "query",
            "running",
            "grep",
            NeoismAgentOutputKind::Text,
            "text",
            Vec::new(),
        ),
    ];
    let statuses = HashMap::from([(
        "child".to_string(),
        super::super::api::SessionStatusSnapshot {
            kind: "busy".to_string(),
            ..Default::default()
        },
    )]);

    pane.apply_runtime_status_for_session("child", &statuses);

    assert_eq!(pane.timeline_live_trace_start, Some(0));
}

#[test]
fn older_pages_stay_visible_for_ongoing_child_inspector() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Working, None);
    pane.timeline_live_trace_start = Some(0);

    pane.note_timeline_prepend(40);

    assert_eq!(pane.timeline_live_trace_start, Some(0));
    assert_eq!(pane.take_timeline_prepend(), Some(40));
}

#[test]
fn offscreen_root_stream_is_live_before_switching_back() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.messages = vec![NeoismAgentMessage::assistant("child transcript")];
    let mut root = CachedAgentSession::live_only();
    root.hydrated = true;
    root.state.parent_id = None;
    root.messages = vec![NeoismAgentMessage::user("question")];
    pane.session_cache.insert("root".to_string(), root);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::QueueStatus {
                count: 1,
                preview: Some("next".to_string()),
                started_at: Some(1),
            },
            AgentSessionUpdate::GoalUpdated {
                goal: Some(SessionGoal {
                    text: "keep shipping".to_string(),
                    updated: 9,
                    ..SessionGoal::default()
                }),
                version: 9,
            },
            AgentSessionUpdate::PartDelta {
                message_id: Some("assistant-message".to_string()),
                part_id: Some("answer".to_string()),
                kind: Some("text".to_string()),
                delta: "streamed while viewing child".to_string(),
            },
        ],
    ));

    assert!(pane.drain_server_updates());
    pane.switch_session("root".to_string());

    assert_eq!(pane.session_id.as_deref(), Some("root"));
    assert!(pane
        .messages
        .iter()
        .any(|message| message.text == "streamed while viewing child"));
    assert_eq!(pane.queued_prompt_count, 1);
    assert_eq!(
        pane.side_panel
            .session_goal()
            .map(|goal| goal.text.as_str()),
        Some("keep shipping")
    );
    assert!(pane.runtime_status_requests.get("root").is_none());
}

#[test]
fn child_completion_and_parent_continuation_stay_live_during_child_view() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("child".to_string());
    pane.parent_session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.messages = vec![NeoismAgentMessage::assistant("child transcript")];
    let mut root = CachedAgentSession::live_only();
    root.hydrated = true;
    root.messages = vec![task_tool_message("child", "running")];
    pane.session_cache.insert("root".to_string(), root);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::SubagentCompleted {
                task_id: "child".to_string(),
                status: "completed".to_string(),
                title: None,
                agent: None,
            },
            AgentSessionUpdate::PartDelta {
                message_id: Some("parent-answer".to_string()),
                part_id: Some("answer".to_string()),
                kind: Some("text".to_string()),
                delta: "parent resumed while child was open".to_string(),
            },
        ],
    ));

    pane.drain_server_updates();
    pane.switch_session("root".to_string());

    let task = pane
        .messages
        .iter()
        .find(|message| message.tool == "task")
        .expect("parent task card");
    assert_eq!(task.status, "completed");
    assert!(task.detail.contains("status: completed"));
    assert!(pane
        .messages
        .iter()
        .any(|message| message.text == "parent resumed while child was open"));
}

#[test]
fn late_completed_tool_part_cannot_resurrect_idle_parent_status() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Working, Some("Task".to_string()));
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [
            AgentSessionUpdate::SessionIdle,
            AgentSessionUpdate::PartUpdated {
                message: task_tool_message("child", "completed"),
                parent_message_id: Some("assistant-message".to_string()),
            },
        ],
    ));

    pane.drain_server_updates();

    assert!(!pane.is_streaming());
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert!(pane
        .messages
        .iter()
        .any(|message| message.tool == "task" && message.status == "completed"));
}

#[test]
fn switching_families_replaces_stale_event_stream_root() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root-a".to_string());
    pane.session_tree_root_id = Some("root-a".to_string());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root-b".to_string());
    pane.session_cache.insert("child-b".to_string(), child);
    pane.runtime_hydrated_sessions.insert("child-b".to_string());

    pane.switch_session("child-b".to_string());

    assert_eq!(pane.session_tree_root_id.as_deref(), Some("root-b"));
    assert_eq!(
        pane.event_stream
            .as_ref()
            .map(AgentSessionEventStream::session_id),
        Some("root-b")
    );
}

#[test]
fn stale_runtime_poll_cannot_overwrite_newer_live_state() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.note_session_runtime_event("root");
    pane.runtime_status_requests.insert("root".to_string(), 7);
    let statuses = HashMap::from([(
        "root".to_string(),
        super::super::api::SessionStatusSnapshot::default(),
    )]);
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::SessionRuntimeStatusRefreshed {
            session_id: "root".to_string(),
            request_generation: 7,
            runtime_revision: 0,
            result: Ok(statuses),
        })
        .unwrap();

    pane.drain_background_updates();

    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
    assert!(!pane.runtime_status_requests.contains_key("root"));
}

#[test]
fn omitted_child_from_runtime_status_snapshot_settles_working_latch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child", "Map frontend", "explore")
            .with_runtime_status(Some("running".to_string())),
    ]);
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Active, None);
    pane.sync_subagent_waiting_clock();
    assert_eq!(pane.active_subagent_count(), 1);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );

    pane.apply_runtime_status_for_session("parent", &HashMap::new());

    assert_eq!(pane.active_subagent_count(), 0);
    assert!(pane.subagent_waiting_started_at.is_none());
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
    assert_eq!(
        pane.side_panel
            .branch_activity("child")
            .map(|activity| activity.status),
        Some(BranchStatus::Completed)
    );
}

#[test]
fn terminal_child_stragglers_update_text_without_resurrecting_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.note_subagent_runtime("child".to_string(), BranchStatus::Completed, None);
    pane.session_cache
        .insert("child".to_string(), CachedAgentSession::live_only());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::ChildPartDelta {
            session_id: "child".to_string(),
            message_id: Some("message".to_string()),
            part_id: Some("answer".to_string()),
            kind: Some("text".to_string()),
            delta: "late suffix".to_string(),
        }],
    ));

    pane.drain_server_updates();

    let child = &pane.session_cache["child"];
    assert_eq!(child.messages[0].text, "late suffix");
    assert_eq!(
        child.runtime.streaming_state,
        NeoismAgentStreamingState::Idle
    );
}

#[test]
fn reconnect_child_snapshot_repairs_inactive_transcript() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.messages = vec![NeoismAgentMessage::assistant("old").with_id("answer")];
    pane.session_cache.insert("child".to_string(), child);
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::ChildMessages {
            session_id: "child".to_string(),
            messages: vec![NeoismAgentMessage::assistant("complete").with_id("answer")],
            oldest_cursor: Some("cursor".to_string()),
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(pane.session_cache["child"].messages[0].text, "complete");
    assert_eq!(
        pane.session_cache["child"]
            .timeline_history
            .oldest_loaded_cursor
            .as_deref(),
        Some("cursor")
    );
}

#[test]
fn child_compaction_runtime_is_routed_to_the_child_cache() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.session_cache
        .insert("child".to_string(), CachedAgentSession::live_only());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::CompactionStarted {
            session_id: "child".to_string(),
            id: "compact".to_string(),
            reason: "auto".to_string(),
        }],
    ));

    pane.drain_server_updates();

    assert_eq!(
        pane.session_cache["child"].runtime.streaming_state,
        NeoismAgentStreamingState::Compacting
    );
    assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Idle);
}

#[test]
fn inactive_prompt_failure_settles_the_origin_session() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.messages = vec![NeoismAgentMessage::user("ship it")];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".to_string());
    pane.session_cache.insert("child".to_string(), child);
    pane.runtime_hydrated_sessions.insert("child".to_string());
    pane.switch_session("child".to_string());
    pane.prompt_dispatch_in_flight = true;
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatchFailed {
            origin_session_id: Some("root".to_string()),
            origin_draft_id: 0,
            error: "offline".to_string(),
        })
        .unwrap();

    pane.drain_background_updates();

    let root = &pane.session_cache["root"];
    assert_eq!(
        root.runtime.streaming_state,
        NeoismAgentStreamingState::Idle
    );
    assert!(root.messages.iter().any(|message| {
        message.kind == NeoismAgentMessageKind::System && message.text == "offline"
    }));
}

#[test]
fn inactive_prompt_success_reconciles_server_echo_without_duplicate_user() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.session_tree_root_id = Some("root".to_string());
    pane.messages = vec![NeoismAgentMessage::user("ship it")];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.event_stream = Some(AgentSessionEventStream::connected_for_test("root"));
    let mut child = CachedAgentSession::live_only();
    child.hydrated = true;
    child.state.parent_id = Some("root".to_string());
    pane.session_cache.insert("child".to_string(), child);
    pane.runtime_hydrated_sessions.insert("child".to_string());
    pane.switch_session("child".to_string());
    pane.prompt_dispatch_in_flight = true;
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatched {
            origin_session_id: Some("root".to_string()),
            origin_draft_id: 0,
            session_id: "root".to_string(),
            transcript_echo: Some("ship it".to_string()),
            event_stream: None,
        })
        .unwrap();
    pane.drain_background_updates();
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::Messages {
            messages: vec![NeoismAgentMessage::user("ship it").with_id("server-user")],
            oldest_cursor: None,
        }],
    ));

    pane.drain_server_updates();

    let root = &pane.session_cache["root"];
    assert_eq!(
        root.messages
            .iter()
            .filter(|message| {
                message.kind == NeoismAgentMessageKind::User && message.text == "ship it"
            })
            .count(),
        1
    );
    assert!(root.pending_user_prompts.is_empty());
}

#[test]
fn selecting_current_session_cancels_pending_cold_switch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.pending_session_switch = Some("cold-child".to_string());

    pane.switch_session("root".to_string());

    assert!(pane.pending_session_switch.is_none());
}

#[test]
fn proactive_preload_queue_is_concurrency_and_memory_bounded() {
    let mut pane = NeoismAgentPane::default();
    pane.session_preloads_in_flight.insert("busy-1".to_string());
    pane.session_preloads_in_flight.insert("busy-2".to_string());
    for index in 0..20 {
        pane.ensure_session_preloaded(format!("child-{index}"), false);
    }

    assert_eq!(pane.session_preloads_in_flight.len(), 2);
    assert_eq!(pane.session_preload_queue.len(), 10);

    pane.pending_session_switch = Some("child-19".to_string());
    pane.ensure_session_preloaded("child-19".to_string(), false);
    assert_eq!(
        pane.session_preload_queue
            .front()
            .map(|(id, _)| id.as_str()),
        Some("child-19")
    );
}

#[test]
fn hidden_pane_live_drain_leaves_blocking_outbound_work_queued() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".to_string());
    pane.abort_session();

    pane.drain_live_session_updates();

    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::AbortSession]
    );
}

#[test]
fn compact_session_queues_outbound_command_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());

    pane.execute_slash_text("/compact");

    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::CompactSession]
    );
    assert!(pane.messages.is_empty());
    assert_ne!(pane.streaming_label(), "Compacting");
}

#[test]
fn submit_prompt_queues_send_prompt_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_text("ship it");

    assert!(pane.submit());

    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 1);
    match &drained[0] {
        OutboundAgentCommand::SendPrompt {
            message_id,
            text,
            agent,
            model,
            thinking,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "ship it");
            assert_eq!(pane.messages[0].id.as_str(), message_id);
            assert_eq!(agent.as_deref(), Some(DEFAULT_AGENT));
            assert_eq!(model, DEFAULT_MODEL);
            assert_eq!(thinking, &None);
            assert!(
                *transcript_echo,
                "idle submissions should be echoed into the transcript"
            );
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
    assert_eq!(pane.messages[0].text, "ship it");
    assert!(pane.is_streaming());
}

#[test]
fn prompt_dispatch_is_off_thread_and_serialized() {
    let mut pane = NeoismAgentPane::default();
    pane.server = "invalid://agent-server".to_string();
    pane.session_id = Some("sess-1".to_string());

    pane.insert_text("first");
    assert!(pane.submit());
    assert!(pane.drain_outbound_commands());
    assert!(pane.prompt_dispatch_in_flight);
    assert!(pane.pending_prompt_dispatches.is_empty());

    // A second send must wait behind the first worker instead of issuing a
    // concurrent POST that could overtake it at the backend.
    pane.insert_text("second");
    assert!(pane.submit());
    assert!(pane.drain_outbound_commands());
    assert!(pane.prompt_dispatch_in_flight);
    assert_eq!(pane.pending_prompt_dispatches.len(), 1);
}

#[test]
fn completed_prompt_does_not_attach_to_a_replaced_draft() {
    let mut pane = NeoismAgentPane::default();
    let original_draft_id = pane.prompt_draft_id;
    pane.prompt_dispatch_in_flight = true;

    pane.create_new_session();
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::PromptDispatched {
            origin_session_id: None,
            origin_draft_id: original_draft_id,
            session_id: "old-draft-session".to_string(),
            transcript_echo: Some("old prompt".to_string()),
            event_stream: None,
        })
        .unwrap();

    assert!(pane.drain_background_updates());
    assert_eq!(pane.session_id, None);
    assert!(pane.pending_user_prompts.is_empty());
}

#[test]
fn submit_prompt_with_session_queues_send_prompt_only() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.insert_text("continue");

    assert!(pane.submit());

    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 1);
    match &drained[0] {
        OutboundAgentCommand::SendPrompt {
            text,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "continue");
            assert!(
                *transcript_echo,
                "idle submissions should be echoed into the transcript"
            );
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
}

#[test]
fn setting_goal_starts_an_agent_turn_with_the_goal_text() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());

    pane.start_goal_prompt("ship the durable goal".to_string());

    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.messages[0].text, "ship the durable goal");
    assert!(pane.is_streaming());
    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 1);
    assert!(matches!(
        &drained[0],
        OutboundAgentCommand::SendPrompt {
            text,
            transcript_echo: true,
            ..
        } if text == "ship the durable goal"
    ));
}

#[test]
fn setting_goal_during_a_run_queues_its_agent_turn() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    pane.start_goal_prompt("next durable goal".to_string());

    assert!(pane.messages.is_empty());
    assert_eq!(pane.queued_prompt_count, 1);
    assert_eq!(
        pane.queued_prompt_preview.as_deref(),
        Some("next durable goal")
    );
    assert!(matches!(
        pane.drain_pending_outbound().as_slice(),
        [OutboundAgentCommand::SendPrompt {
            text,
            transcript_echo: false,
            ..
        }] if text == "next durable goal"
    ));
}

#[test]
fn submit_pasted_text_expands_outbound_but_keeps_transcript_token() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.insert_paste("first line\nsecond line");

    assert!(pane.submit());

    let drained = pane.drain_pending_outbound();
    match &drained[0] {
        OutboundAgentCommand::SendPrompt {
            text,
            parts,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "first line\nsecond line");
            assert_eq!(parts[0]["text"], "first line\nsecond line");
            assert!(*transcript_echo);
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
    assert_eq!(pane.messages[0].text, "[pasted 2 lines]");
}

#[test]
fn submit_prompt_while_streaming_queues_bottom_preview_without_transcript_echo() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());
    pane.messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("still running"),
    ];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.insert_text("queued turn");

    assert!(pane.submit());

    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| is_user_prompt(message, "queued turn"))
            .count(),
        0,
        "queued submissions should stay out of the transcript until dequeue"
    );
    assert_eq!(pane.queued_prompt_count, 1);
    assert_eq!(pane.queued_prompt_preview.as_deref(), Some("queued turn"));
    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 1);
    match &drained[0] {
        OutboundAgentCommand::SendPrompt {
            text,
            transcript_echo,
            ..
        } => {
            assert_eq!(text, "queued turn");
            assert!(
                !*transcript_echo,
                "streaming submissions should remain in the queue preview"
            );
        }
        other => panic!("expected SendPrompt, got {other:?}"),
    }
}

#[test]
fn server_expanded_user_part_merges_into_pasted_token_echo() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_paste("first line\nsecond line");
    assert!(pane.submit());
    assert_eq!(pane.messages[0].text, "[pasted 2 lines]");

    // The server streams the user part back EXPANDED, with its own id.
    let mut server_part = NeoismAgentMessage::user("first line\nsecond line");
    server_part.id = "srv-user-1".to_string();
    pane.upsert_part_message(server_part);

    let users: Vec<_> = pane
        .messages
        .iter()
        .filter(|message| message.kind == NeoismAgentMessageKind::User)
        .collect();
    assert_eq!(
        users.len(),
        1,
        "expanded server echo must not add a second user bubble"
    );
    assert_eq!(users[0].text, "[pasted 2 lines]");
    assert_eq!(users[0].id, "srv-user-1");
}

#[test]
fn streamed_first_prompt_image_and_text_merge_in_either_order() {
    for image_first in [false, true] {
        let mut pane = NeoismAgentPane::default();
        let image = neoism_ui::panels::agent_pane::state::NeoismAgentImage {
            filename: "clipboard.png".to_string(),
            url: "data:image/png;base64,AA==".to_string(),
            mime: "image/png".to_string(),
        };
        let mut optimistic = NeoismAgentMessage::user("[image1] inspect this");
        optimistic.images.push(image.clone());
        pane.messages.push(optimistic);

        let mut text_part = NeoismAgentMessage::user("[image1] inspect this");
        text_part.id = "msg-user-1".to_string();
        let mut image_part = NeoismAgentMessage::user("");
        image_part.id = "msg-user-1".to_string();
        image_part.images.push(image.clone());

        if image_first {
            pane.upsert_part_message(image_part);
            pane.upsert_part_message(text_part);
        } else {
            pane.upsert_part_message(text_part);
            pane.upsert_part_message(image_part);
        }

        assert_eq!(pane.messages.len(), 1, "image_first={image_first}");
        assert_eq!(pane.messages[0].id, "msg-user-1");
        assert_eq!(pane.messages[0].text, "[image1] inspect this");
        assert_eq!(pane.messages[0].images, vec![image]);
    }
}

#[test]
fn history_refresh_compacts_expanded_pasted_user_text() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_paste("first line\nsecond line");
    assert!(pane.submit());

    let mut server_user = NeoismAgentMessage::user("first line\nsecond line");
    server_user.id = "srv-user-1".to_string();
    let compacted = pane.compact_inbound_user_texts(vec![server_user]);

    assert_eq!(compacted[0].text, "[pasted 2 lines]");
}

#[test]
fn backspace_removes_pasted_token_and_attachment_atomically() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_text("see ");
    pane.insert_paste("first line\nsecond line");
    assert!(pane.input.contains("[pasted 2 lines]"));
    assert_eq!(pane.input_attachments.len(), 1);

    // One backspace removes the whole token plus the trailing space
    // `insert_token` added.
    pane.backspace();

    assert_eq!(pane.input, "see ");
    assert!(
        pane.input_attachments.is_empty(),
        "deleting the token must drop its attachment"
    );
}

#[test]
fn dequeued_prompt_consumes_preview_and_appends_once_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("done"),
    ];
    pane.queued_prompt_count = 1;
    pane.queued_prompt_preview = Some("queued turn".to_string());

    assert!(pane.insert_dequeued_user_prompt("queued turn".to_string()));
    assert!(!pane.insert_dequeued_user_prompt("queued turn".to_string()));

    assert_eq!(pane.queued_prompt_count, 0);
    assert_eq!(pane.queued_prompt_preview, None);
    assert_eq!(
        pane.messages
            .iter()
            .filter(|message| is_user_prompt(message, "queued turn"))
            .count(),
        1
    );
    assert_eq!(pane.messages.last().unwrap().text, "queued turn");
}

#[test]
fn unknown_slash_command_queues_session_and_slash_for_runtime() {
    let mut pane = NeoismAgentPane::default();

    pane.execute_slash_text("/login token=abc");

    let drained = pane.drain_pending_outbound();
    assert_eq!(drained.len(), 2);
    assert!(matches!(drained[0], OutboundAgentCommand::EnsureSession));
    match &drained[1] {
        OutboundAgentCommand::SlashCommand { name, args } => {
            assert_eq!(name, "login");
            assert_eq!(args, "token=abc");
        }
        other => panic!("expected SlashCommand, got {other:?}"),
    }
}

#[test]
fn typing_space_after_slash_command_closes_picker_and_focuses_input() {
    let mut pane = NeoismAgentPane::default();

    for ch in ["/", "g", "o", "a", "l"] {
        pane.insert_text(ch);
    }
    // Picker is open while typing the bare command name.
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Slash));

    // Pressing space commits the command name and starts the argument:
    // the picker must dismiss and the caret must move to the composer.
    pane.insert_text(" ");
    assert!(pane.picker().is_none());
    assert_eq!(pane.input(), "/goal ");
    assert_eq!(pane.cursor_byte(), "/goal ".len());

    // Subsequent argument text lands in the input bar, not the picker.
    for ch in ["s", "h", "i", "p"] {
        pane.insert_text(ch);
    }
    assert!(pane.picker().is_none());
    assert_eq!(pane.input(), "/goal ship");
    assert_eq!(pane.cursor_byte(), "/goal ship".len());
}

#[test]
fn typing_skill_mention_keeps_query_visible_in_input() {
    let mut pane = NeoismAgentPane::default();

    pane.insert_text("$");
    pane.insert_text("neo");

    assert_eq!(pane.input(), "$neo");
    assert_eq!(pane.cursor_byte(), "$neo".len());
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention));
}

#[test]
fn backspace_updates_skill_mention_input_and_dismisses_when_trigger_removed() {
    let mut pane = NeoismAgentPane::default();

    pane.insert_text("$");
    pane.insert_text("neo");
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention));

    pane.backspace();
    assert_eq!(pane.input(), "$ne");
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention));

    pane.backspace();
    pane.backspace();
    assert_eq!(pane.input(), "$");
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::SkillMention));

    pane.backspace();
    assert_eq!(pane.input(), "");
    assert!(pane.picker().is_none());
}

#[test]
fn enter_still_runs_argumentless_slash_command_from_picker() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("sess-1".to_string());

    for ch in ["/", "c", "o", "m", "p", "a", "c", "t"] {
        pane.insert_text(ch);
    }
    assert!(pane
        .picker()
        .is_some_and(|picker| picker.kind == NeoismAgentPickerKind::Slash));

    // No trailing space: Enter commits the highlighted command.
    assert!(pane.submit());
    assert!(pane.picker().is_none());
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::CompactSession]
    );
}

#[test]
fn model_change_queues_context_limit_refresh_for_runtime() {
    let mut pane = NeoismAgentPane::default();

    pane.apply_model("claude-test".to_string());

    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::RefreshModelContextLimit]
    );
}

#[test]
fn zero_token_compaction_usage_does_not_reset_context_meter() {
    let mut pane = NeoismAgentPane::default();
    let mut completed = NeoismAgentMessage::assistant("done");
    completed.usage = Some(NeoismAgentUsage {
        input: 18_000,
        output: 500,
        reasoning: 250,
        cache_read: 31_250,
        cache_write: 0,
        total: 50_000,
        cost_micros: 0,
        context_limit: Some(400_000),
    });
    let mut compacted = NeoismAgentMessage::compaction("summary", "auto");
    compacted.usage = Some(NeoismAgentUsage {
        input: 0,
        output: 0,
        reasoning: 0,
        cache_read: 0,
        cache_write: 0,
        total: 0,
        cost_micros: 0,
        context_limit: Some(400_000),
    });
    pane.messages.extend([completed, compacted]);

    assert_eq!(pane.latest_usage().map(|usage| usage.total), Some(50_000));
}

#[test]
fn agent_model_and_thinking_changes_preserve_composer_draft() {
    let mut pane = NeoismAgentPane::default();
    pane.insert_text("keep this question");
    let draft = pane.input().to_string();

    pane.apply_model("openai/gpt-test".to_string());
    assert_eq!(pane.input(), draft);

    pane.apply_thinking("high".to_string());
    assert_eq!(pane.input(), draft);

    pane.apply_agent("plan".to_string());
    assert_eq!(pane.input(), draft);
}

#[test]
fn with_directory_queues_config_defaults_for_runtime() {
    let mut pane = NeoismAgentPane::with_directory(Some("/tmp/project".to_string()));

    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::ApplyConfigDefaults]
    );
}

#[test]
fn permission_reply_queues_outbound_command_for_runtime() {
    let mut pane = NeoismAgentPane::default();
    pane.pending_permission = Some(test_permission(0));

    assert!(pane.respond_pending_permission(NeoismAgentPermissionChoice::Reject));

    assert!(pane.pending_permission.as_ref().unwrap().responding);
    assert_eq!(
        pane.drain_pending_outbound(),
        vec![OutboundAgentCommand::ReplyPermission {
            id: "perm-1".to_string(),
            reply: "reject".to_string(),
        }]
    );
}

#[test]
fn stale_idle_snapshot_keeps_streamed_assistant_text_by_id() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("what"),
        NeoismAgentMessage::assistant("streamed final answer").with_id("part-1"),
    ];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    let refreshed = pane.preserve_streamed_response_text(vec![
        NeoismAgentMessage::user("what"),
        NeoismAgentMessage::assistant("").with_id("part-1"),
    ]);

    assert_eq!(refreshed.len(), 2);
    assert_eq!(refreshed[1].text, "streamed final answer");
}

#[test]
fn child_hydration_merges_snapshot_without_losing_streamed_prefix() {
    let snapshot = vec![
        NeoismAgentMessage::user("inspect it").with_id("msg-user"),
        NeoismAgentMessage::assistant("").with_id("part-answer"),
    ];
    let live =
        vec![NeoismAgentMessage::assistant("the streamed prefix").with_id("part-answer")];

    let merged = merge_session_snapshot(snapshot, live);

    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1].text, "the streamed prefix");
}

#[test]
fn partial_snapshot_keeps_cached_history_and_subagent_notice_in_order() {
    let notice = NeoismAgentMessage::system(
        "Subagent",
        "Subagent finished.\ntask_id: ses-child\nstatus: completed",
    )
    .with_id("msg_subtask_completion_ses-child");
    let live = vec![
        NeoismAgentMessage::user("oldest").with_id("u-1"),
        NeoismAgentMessage::assistant("before task").with_id("a-1"),
        notice,
        NeoismAgentMessage::user("after task").with_id("u-2"),
        NeoismAgentMessage::assistant("newest").with_id("a-2"),
    ];
    let snapshot = vec![
        NeoismAgentMessage::assistant("before task").with_id("a-1"),
        NeoismAgentMessage::user("after task").with_id("u-2"),
        NeoismAgentMessage::assistant("newest").with_id("a-2"),
    ];

    let merged = merge_session_snapshot(snapshot, live);
    let ids = merged
        .iter()
        .map(|message| message.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        vec![
            "u-1",
            "a-1",
            "msg_subtask_completion_ses-child",
            "u-2",
            "a-2"
        ]
    );
}

#[test]
fn runtime_completion_rehydrate_does_not_append_cached_part_at_bottom() {
    let live = crate::neoism::agent::api::part_block(&json!({
        "id": "prt-background-done",
        "messageID": "msg_background_completion_job_123",
        "type": "text",
        "role": "user",
        "text": "Background shell task finished.\njob_id: job_123\nstatus: completed"
    }))
    .expect("live runtime completion");
    // The persisted runtime prompt maps to the SAME durable card id as the
    // live broadcast (`background-task-{job}`), so rehydration merges them
    // into one row instead of appending a duplicate at the bottom.
    let snapshot = neoism_ui::panels::agent_pane::api_mapping::message_blocks(&json!({
        "info": {
            "id": "msg_background_completion_job_123",
            "role": "user"
        },
        "parts": [{
            "id": "prt-background-done",
            "type": "text",
            "text": "Background shell task finished.\njob_id: job_123\nstatus: completed"
        }]
    }))
    .into_iter()
    .map(NeoismAgentMessage::from)
    .collect::<Vec<_>>();

    let merged = merge_session_snapshot(snapshot, vec![live]);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, "background-task-job_123");
    assert_eq!(merged[0].kind, NeoismAgentMessageKind::Tool);
    assert_eq!(merged[0].tool, "background_task_result");
}

#[test]
fn child_live_cache_accumulates_deltas_before_navigation() {
    let mut messages = Vec::new();

    apply_cached_part_delta(
        &mut messages,
        Some("part-answer"),
        Some("text"),
        "the prefix",
    );
    apply_cached_part_delta(
        &mut messages,
        Some("part-answer"),
        Some("text"),
        " and suffix",
    );

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "part-answer");
    assert_eq!(messages[0].text, "the prefix and suffix");
}

#[test]
fn session_cache_preserves_each_transcripts_scroll_state() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("ses-root".to_string());
    pane.messages = vec![NeoismAgentMessage::user("root")];
    pane.timeline_scroll_px = 240.0;
    pane.timeline_follow_bottom = false;

    pane.cache_current_session(false);

    let cached = pane.session_cache.get("ses-root").expect("root cache");
    assert_eq!(cached.timeline_scroll_px, 240.0);
    assert!(!cached.timeline_follow_bottom);
}

#[test]
fn inactive_session_cache_is_bounded() {
    let mut pane = NeoismAgentPane::default();
    for index in 0..45 {
        pane.session_cache
            .insert(format!("session-{index}"), CachedAgentSession::live_only());
    }

    pane.trim_session_cache();

    assert_eq!(pane.session_cache.len(), 40);
}

#[test]
fn transcript_refresh_keeps_live_trace_anchored_to_its_turn() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("old"),
        NeoismAgentMessage::user("latest").with_id("latest"),
        NeoismAgentMessage::reasoning("thinking").with_id("reasoning"),
        NeoismAgentMessage::assistant("tool").with_id("tool"),
    ];
    pane.timeline_live_trace_start = Some(2);
    pane.timeline_live_trace_anchor = Some("latest".to_string());

    // A refresh inserts an older answer above the anchored turn; the marker
    // must follow its turn (id anchor), not jump rows or drift to a newer
    // boundary. Turns revealed during a visit stay revealed until the
    // session is left.
    pane.messages = vec![
        NeoismAgentMessage::user("old"),
        NeoismAgentMessage::assistant("old answer").with_id("old-answer"),
        NeoismAgentMessage::user("latest").with_id("latest"),
        NeoismAgentMessage::assistant("durable answer").with_id("answer"),
    ];
    pane.rebase_current_turn_trace();

    assert_eq!(pane.timeline_live_trace_start, Some(3));
    assert_eq!(pane.messages[3].text, "durable answer");

    // A newer prompt arriving must NOT move the boundary forward: the
    // anchored turn's trace stays visible for the rest of the visit.
    pane.messages
        .push(NeoismAgentMessage::user("newer").with_id("newer"));
    pane.messages
        .push(NeoismAgentMessage::assistant("newer answer").with_id("newer-answer"));
    pane.rebase_current_turn_trace();
    assert_eq!(pane.timeline_live_trace_start, Some(3));

    // An unfindable (optimistic, empty-id) anchor falls back to the latest
    // turn and re-anchors on its durable id.
    pane.timeline_live_trace_anchor = Some(String::new());
    pane.rebase_current_turn_trace();
    assert_eq!(pane.timeline_live_trace_start, Some(5));
    assert_eq!(pane.timeline_live_trace_anchor.as_deref(), Some("newer"));
}

#[test]
fn stale_idle_snapshot_does_not_append_orphan_streamed_assistant_tail() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("what"),
        NeoismAgentMessage::assistant("streamed final answer").with_id("part-1"),
    ];
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);

    let refreshed =
        pane.preserve_streamed_response_text(vec![NeoismAgentMessage::user("what")]);

    assert_eq!(refreshed, vec![NeoismAgentMessage::user("what")]);
}

#[test]
fn stale_idle_snapshot_does_not_append_orphan_streamed_tool_card() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("edit"),
        NeoismAgentMessage::tool(
            "ApplyPatch(src/lib.rs)",
            "applying patch",
            "running",
            "apply_patch",
            NeoismAgentOutputKind::Text,
            "rust",
            Vec::new(),
        )
        .with_id("tool-1"),
    ];
    pane.note_streaming(
        NeoismAgentStreamingState::Working,
        Some("ApplyPatch".to_string()),
    );

    let refreshed =
        pane.preserve_streamed_response_text(vec![NeoismAgentMessage::user("edit")]);

    assert_eq!(refreshed, vec![NeoismAgentMessage::user("edit")]);
}

#[test]
fn history_refresh_keeps_server_order_for_late_user_and_reasoning() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::reasoning("old local thought").with_id("reasoning-1"),
        NeoismAgentMessage::assistant("old local final").with_id("answer-1"),
    ];

    let server_messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::reasoning("server thought").with_id("reasoning-1"),
        NeoismAgentMessage::assistant("server final").with_id("answer-1"),
        NeoismAgentMessage::user("second"),
        NeoismAgentMessage::reasoning("server thought 2").with_id("reasoning-2"),
        NeoismAgentMessage::assistant("server final 2").with_id("answer-2"),
    ];

    let refreshed = pane.preserve_streamed_response_text(server_messages);

    assert_eq!(refreshed[3].kind, NeoismAgentMessageKind::User);
    assert_eq!(refreshed[3].text, "second");
    assert_eq!(refreshed[4].kind, NeoismAgentMessageKind::Reasoning);
    assert_eq!(refreshed[5].kind, NeoismAgentMessageKind::Assistant);
}

#[test]
fn history_refresh_does_not_duplicate_compaction_summary() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("compact this"),
        NeoismAgentMessage::compaction("summary body", "auto")
            .with_id("local-compaction"),
    ];

    let refreshed = pane.preserve_streamed_response_text(vec![
        NeoismAgentMessage::user("compact this"),
        NeoismAgentMessage::assistant("summary body").with_id("server-summary"),
    ]);

    assert_eq!(refreshed.len(), 2);
    assert_eq!(
        refreshed
            .iter()
            .filter(|message| message.text == "summary body")
            .count(),
        1
    );
}

#[test]
fn history_refresh_keeps_compaction_marker_in_original_slot() {
    let mut pane = NeoismAgentPane::default();
    // A compaction happened mid-conversation, then another turn streamed in.
    pane.messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("first answer").with_id("answer-1"),
        NeoismAgentMessage::compaction("summary", "model").with_id("compaction-1"),
        NeoismAgentMessage::user("second"),
        NeoismAgentMessage::assistant("second answer").with_id("answer-2"),
    ];

    // Compactions are now server-persisted (see
    // `persisted_compaction_is_only_compaction_message_source`), so the idle
    // history snapshot re-sends the marker in its original slot rather than
    // dropping it — the refresh is server-authoritative and preserves it.
    let server_messages = vec![
        NeoismAgentMessage::user("first"),
        NeoismAgentMessage::assistant("first answer").with_id("answer-1"),
        NeoismAgentMessage::compaction("summary", "model").with_id("compaction-1"),
        NeoismAgentMessage::user("second"),
        NeoismAgentMessage::assistant("second answer").with_id("answer-2"),
    ];

    let refreshed = pane.preserve_streamed_response_text(server_messages);

    assert_eq!(refreshed.len(), 5);
    assert_eq!(refreshed[2].kind, NeoismAgentMessageKind::Compaction);
    assert_eq!(refreshed[2].text, "summary");
    // The marker must not be appended past the latest assistant reply.
    assert_eq!(refreshed[4].kind, NeoismAgentMessageKind::Assistant);
    assert_eq!(refreshed[4].text, "second answer");
}

#[test]
fn subagent_rehydrate_does_not_complete_task_without_explicit_status() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "running")];

    pane.reconcile_task_message_statuses();

    assert_eq!(pane.messages[0].status, "running");
    assert!(pane.messages[0].detail.contains("status: running"));
}

#[test]
fn subagent_rehydrate_completes_task_from_explicit_child_status() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "running")];
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child-1", "child", "codex")
            .with_runtime_status(Some("completed".to_string())),
    ]);

    pane.reconcile_task_message_statuses();

    assert_eq!(pane.messages[0].status, "completed");
    assert!(pane.messages[0].detail.contains("status: completed"));
}

#[test]
fn stale_active_subagent_id_does_not_revert_completed_task_to_running() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "running")];
    pane.active_subagent_ids.insert("child-1".to_string());
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child-1", "child", "codex")
            .with_runtime_status(Some("completed".to_string())),
    ]);

    pane.reconcile_task_message_statuses();

    assert_eq!(pane.messages[0].status, "completed");
    assert!(pane.messages[0].detail.contains("status: completed"));
    assert!(!pane.active_subagent_ids.contains("child-1"));
}

#[test]
fn runtime_child_keeps_waiting_status_before_sidebar_hydrates() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("parent".to_string());

    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Active, Some(1));
    pane.sync_subagent_waiting_clock();

    assert_eq!(pane.active_subagent_count(), 1);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );

    pane.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    pane.sync_subagent_waiting_clock();
    assert_eq!(pane.active_subagent_count(), 0);
    // The completion edge is bridged by the display grace hold, then
    // sustained idle clears the label.
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::WaitingSubagents
    );
    pane.side_panel
        .rewind_status_display_hold(STATUS_LABEL_GRACE);
    assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
}

#[test]
fn part_activity_hydrates_missing_row_but_terminal_straggler_does_not() {
    let mut live = NeoismAgentPane::default();
    live.session_id = Some("parent".to_string());
    assert!(live.note_subagent_part_activity(
        "child-1".to_string(),
        BranchStatus::Active,
        Some("responding".to_string()),
        Some(1),
    ));
    assert!(live
        .side_panel
        .subagents()
        .iter()
        .any(|entry| entry.id == "child-1"));

    let mut completed = NeoismAgentPane::default();
    completed.session_id = Some("parent".to_string());
    completed.note_subagent_runtime("child-1".to_string(), BranchStatus::Completed, None);
    assert!(!completed.note_subagent_part_activity(
        "child-1".to_string(),
        BranchStatus::Active,
        Some("responding".to_string()),
        Some(2),
    ));
    assert!(!completed
        .side_panel
        .subagents()
        .iter()
        .any(|entry| entry.id == "child-1"));
}

#[test]
fn completed_task_card_drops_stale_running_explanation() {
    let mut pane = NeoismAgentPane::default();
    let mut message = task_tool_message("child-1", "running");
    message.detail.push_str(
        "\n\nThe subagent is running in the background and the user can still message the main session.",
    );
    pane.messages = vec![message];

    pane.set_task_message_status("child-1", "completed");

    assert!(pane.messages[0].detail.contains("status: completed"));
    assert!(!pane.messages[0]
        .detail
        .contains("running in the background"));
    assert!(pane.messages[0]
        .detail
        .contains("The subagent is no longer running."));
}

#[test]
fn later_stale_running_task_snapshot_does_not_replace_completed_card() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "completed")];

    let refreshed = pane
        .preserve_streamed_response_text(vec![task_tool_message("child-1", "running")]);

    assert_eq!(refreshed[0].status, "completed");
    assert!(refreshed[0].detail.contains("status: completed"));
}

#[test]
fn subagent_rehydrate_resets_reused_child_task_to_running() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![task_tool_message("child-1", "completed")];
    pane.side_panel.set_subagents(vec![
        NeoismAgentSessionEntry::new("parent", "main session", "return"),
        NeoismAgentSessionEntry::new("child-1", "child", "codex")
            .with_runtime_status(Some("running".to_string())),
    ]);

    pane.reconcile_task_message_statuses();

    assert_eq!(pane.messages[0].status, "running");
    assert!(pane.messages[0].detail.contains("status: running"));
}

#[test]
fn permission_selection_moves_in_visual_order() {
    let mut pane = NeoismAgentPane::default();
    pane.pending_permission = Some(test_permission(0));

    pane.move_permission_selection(-1);
    assert_eq!(pane.pending_permission.as_ref().unwrap().selected, 1);

    pane.move_permission_selection(1);
    assert_eq!(pane.pending_permission.as_ref().unwrap().selected, 0);

    pane.move_permission_selection(1);
    assert_eq!(pane.pending_permission.as_ref().unwrap().selected, 2);

    pane.move_permission_selection(1);
    assert_eq!(pane.pending_permission.as_ref().unwrap().selected, 1);
}

#[test]
fn stale_idle_snapshot_keeps_streamed_tool_detail_by_id() {
    let mut pane = NeoismAgentPane::default();
    let mut streamed = NeoismAgentMessage::tool(
        "Edit(src/lib.rs)",
        "src/lib.rs",
        "running",
        "edit",
        NeoismAgentOutputKind::Text,
        "rust",
        Vec::new(),
    )
    .with_id("tool-1");
    streamed.detail =
        r#"{"neoismToolDetail":"edit","metadata":{"snapshots":[]}}"#.to_string();
    pane.messages = vec![NeoismAgentMessage::user("edit"), streamed];
    pane.note_streaming(NeoismAgentStreamingState::Working, Some("Edit".to_string()));

    let refreshed = pane.preserve_streamed_response_text(vec![
        NeoismAgentMessage::user("edit"),
        NeoismAgentMessage::tool(
            "Edit(src/lib.rs)",
            "completed",
            "completed",
            "edit",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("tool-1"),
    ]);

    let tool = refreshed
        .iter()
        .find(|message| message.id == "tool-1")
        .expect("tool is preserved");
    assert_eq!(tool.status, "completed");
    assert!(tool.detail.contains("neoismToolDetail"));
    assert_eq!(tool.lang, "rust");
}

#[test]
fn compaction_lifecycle_events_do_not_create_messages() {
    let mut pane = NeoismAgentPane::default();

    pane.start_compaction_message("event-1".to_string(), "auto".to_string());
    assert!(pane.messages.is_empty());
    assert_ne!(pane.streaming_label(), "Compacting");

    pane.apply_compaction_delta("summary");
    pane.finish_compaction_message("summary", "model");
    assert!(pane.messages.is_empty());
}

#[test]
fn persisted_compaction_is_only_compaction_message_source() {
    let mut pane = NeoismAgentPane::default();

    pane.start_compaction_message("event-1".to_string(), "auto".to_string());
    pane.apply_compaction_delta("event delta");
    assert!(pane.messages.is_empty());

    pane.upsert_part_message(
        NeoismAgentMessage::compaction("", "summary").with_id("assistant-compaction"),
    );
    pane.apply_part_delta(
        Some("assistant-compaction".to_string()),
        Some("text-part".to_string()),
        Some("text".to_string()),
        "real summary",
    );
    pane.apply_compaction_delta("event delta tail");
    pane.finish_compaction_message(
        "compaction done\ncompaction summary\ncompaction model",
        "model",
    );

    let compactions: Vec<_> = pane
        .messages
        .iter()
        .filter(|message| message.kind == NeoismAgentMessageKind::Compaction)
        .collect();
    assert_eq!(compactions.len(), 1);
    assert_eq!(compactions[0].id, "assistant-compaction");
    assert_eq!(compactions[0].text, "real summary");
}

#[test]
fn completed_answer_stays_above_later_streamed_reasoning() {
    // The model answers (non-empty text), then opens a fresh thinking
    // block. The finished answer must keep its slot above the later
    // reasoning — it must not drop below it mid-stream.
    let mut pane = NeoismAgentPane::default();

    pane.apply_part_delta(
        None,
        Some("text-1".to_string()),
        Some("text".to_string()),
        "final",
    );
    pane.apply_part_delta(
        None,
        Some("reason-1".to_string()),
        Some("reasoning".to_string()),
        "thought",
    );

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::Assistant);
    assert_eq!(pane.messages[0].text, "final");
    assert_eq!(pane.messages[1].kind, NeoismAgentMessageKind::Reasoning);
    assert_eq!(pane.messages[1].text, "thought");
}

#[test]
fn empty_assistant_placeholder_drops_below_reasoning() {
    // A provider that opens the turn with a blank text part before it
    // streams reasoning: the empty placeholder is pulled below so the
    // thinking renders first, then fills in.
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(NeoismAgentMessage::assistant("").with_id("text-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("thought").with_id("reason-1"),
    );

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].id, "reason-1");
    assert_eq!(pane.messages[1].id, "text-1");
}

#[test]
fn streamed_final_part_inserts_after_existing_reasoning() {
    let mut pane = NeoismAgentPane::default();

    pane.apply_part_delta(
        None,
        Some("reason-1".to_string()),
        Some("reasoning".to_string()),
        "thought",
    );
    pane.apply_part_delta(
        None,
        Some("text-1".to_string()),
        Some("text".to_string()),
        "final",
    );

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::Reasoning);
    assert_eq!(pane.messages[1].kind, NeoismAgentMessageKind::Assistant);
    assert_eq!(pane.messages[1].text, "final");
}

#[test]
fn delayed_reasoning_end_uses_its_assistant_message_group_order() {
    let mut pane = NeoismAgentPane::default();
    pane.remember_live_part_parent("text-1", Some("assistant-message-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("final answer").with_id("text-1"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::tool(
            "Bash(echo ok)",
            "",
            "completed",
            "bash",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("tool-1"),
    );
    pane.remember_live_part_parent("tool-1", Some("assistant-message-1"));
    pane.remember_live_part_parent("reason-1", Some("assistant-message-1"));

    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Clarifying task ID reuse and results")
            .with_id("reason-1"),
    );

    assert_eq!(pane.messages[0].id, "tool-1");
    assert_eq!(pane.messages[1].id, "reason-1");
    assert_eq!(pane.messages[2].id, "text-1");
}

#[test]
fn delayed_subagent_completion_stays_before_its_live_assistant_response() {
    let mut pane = NeoismAgentPane::default();
    pane.remember_live_part_parent("answer", Some("msg_03a800b6c001assistant"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("## Second-pass verdict").with_id("answer"),
    );

    pane.upsert_part_message(
        NeoismAgentMessage::system("Subagent", "Subagent finished.")
            .with_id("msg_03a800995001completion"),
    );
    pane.remember_live_part_parent("thinking", Some("msg_03a800b6c001assistant"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("Summarizing confirmed findings").with_id("thinking"),
    );

    assert_eq!(
        pane.messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["msg_03a800995001completion", "thinking", "answer"]
    );
}

#[test]
fn updated_reasoning_part_does_not_pull_finished_answer_below_it() {
    // A non-empty answer that already streamed keeps its slot even
    // when its reasoning part updates afterwards.
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(NeoismAgentMessage::assistant("final").with_id("text-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("thought").with_id("reason-1"),
    );
    assert_eq!(pane.messages[0].id, "text-1");
    assert_eq!(pane.messages[1].id, "reason-1");

    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("thought done").with_id("reason-1"),
    );

    assert_eq!(pane.messages[0].id, "text-1");
    assert_eq!(pane.messages[1].id, "reason-1");
    assert_eq!(pane.messages[1].text, "thought done");
}

#[test]
fn new_reasoning_does_not_drop_completed_answers_below_it() {
    // Two finished answers then a fresh thinking block: every
    // completed answer keeps its chronological slot.
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(
        NeoismAgentMessage::assistant("old final").with_id("text-old"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("new final").with_id("text-new"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("new thought").with_id("reason-new"),
    );

    assert_eq!(pane.messages[0].id, "text-old");
    assert_eq!(pane.messages[1].id, "text-new");
    assert_eq!(pane.messages[2].id, "reason-new");
}

#[test]
fn reasoning_part_does_not_move_previous_turn_final_below_new_user_prompt() {
    let mut pane = NeoismAgentPane::default();

    pane.messages.push(NeoismAgentMessage::user("first"));
    pane.upsert_part_message(
        NeoismAgentMessage::assistant("old final").with_id("text-old"),
    );
    pane.messages.push(NeoismAgentMessage::user("second"));
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("new thought").with_id("reason-new"),
    );

    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::User);
    assert_eq!(pane.messages[1].id, "text-old");
    assert_eq!(pane.messages[2].kind, NeoismAgentMessageKind::User);
    assert_eq!(pane.messages[3].id, "reason-new");
}

#[test]
fn reasoning_after_finished_answer_and_tool_keeps_chronological_order() {
    // answer → tool → reasoning, all chronological. The finished
    // answer is non-empty so nothing reorders; reasoning appends last.
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(NeoismAgentMessage::assistant("final").with_id("text-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::tool(
            "Bash(echo ok)",
            "",
            "completed",
            "bash",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("tool-1"),
    );
    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("post tool thought").with_id("reason-1"),
    );

    assert_eq!(pane.messages[0].id, "text-1");
    assert_eq!(pane.messages[1].id, "tool-1");
    assert_eq!(pane.messages[2].id, "reason-1");
}

#[test]
fn untagged_reasoning_delta_does_not_append_to_final_text() {
    let mut pane = NeoismAgentPane::default();

    pane.apply_part_delta(None, None, Some("text".to_string()), "final");
    pane.apply_part_delta(None, None, Some("reasoning".to_string()), "thought");

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].kind, NeoismAgentMessageKind::Assistant);
    assert_eq!(pane.messages[0].text, "final");
    assert_eq!(pane.messages[1].kind, NeoismAgentMessageKind::Reasoning);
    assert_eq!(pane.messages[1].text, "thought");
}

#[test]
fn updated_final_part_does_not_reorder_past_later_tool() {
    let mut pane = NeoismAgentPane::default();

    pane.upsert_part_message(
        NeoismAgentMessage::reasoning("thought").with_id("reason-1"),
    );
    pane.upsert_part_message(NeoismAgentMessage::assistant("final").with_id("text-1"));
    pane.upsert_part_message(
        NeoismAgentMessage::tool(
            "Bash(echo ok)",
            "",
            "running",
            "bash",
            NeoismAgentOutputKind::Text,
            "",
            Vec::new(),
        )
        .with_id("tool-1"),
    );

    assert_eq!(pane.messages[0].id, "reason-1");
    assert_eq!(pane.messages[1].id, "text-1");
    assert_eq!(pane.messages[2].id, "tool-1");
}

#[test]
fn diff_file_toggle_does_not_move_or_reanchor_the_timeline() {
    let mut pane = NeoismAgentPane::default();
    pane.messages.push(
        NeoismAgentMessage::tool(
            "ApplyPatch",
            "patch",
            "completed",
            "apply_patch",
            NeoismAgentOutputKind::Text,
            "diff",
            Vec::new(),
        )
        .with_id("tool-1"),
    );
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.timeline_velocity_px_s = 75.0;
    pane.register_tool_hit_rect("tool-1:0".to_string(), [20.0, 150.0, 300.0, 60.0]);
    let scroll_before = pane.timeline_scroll_px;
    let velocity_before = pane.timeline_velocity_px_s;

    assert!(pane.toggle_tool_at(30.0, 160.0));

    assert!(pane.tool_expanded("tool-1:0"));
    assert_eq!(pane.timeline_scroll_px, scroll_before);
    assert_eq!(pane.timeline_velocity_px_s, velocity_before);
    assert!(pane.pending_timeline_anchor.is_none());
    assert!(pane.timeline_view_anchor.is_none());
    assert!(!pane.tool_expand_animating("tool-1"));
}

#[test]
fn markdown_horizontal_scroll_is_block_local_and_geometry_is_frame_local() {
    let mut pane = NeoismAgentPane::default();
    pane.register_markdown_horizontal_scroll_rect(
        "markdown:message-1:code:0".to_string(),
        [20.0, 100.0, 300.0, 120.0],
        240.0,
    );
    pane.register_markdown_horizontal_scroll_rect(
        "markdown:message-1:table:1".to_string(),
        [20.0, 240.0, 300.0, 120.0],
        400.0,
    );

    assert_eq!(
        pane.scroll_markdown_horizontal_at(40.0, 140.0, 75.0),
        Some(true)
    );
    assert_eq!(
        pane.markdown_horizontal_scroll_offset("markdown:message-1:code:0", 240.0),
        75.0
    );
    assert_eq!(
        pane.markdown_horizontal_scroll_offset("markdown:message-1:table:1", 400.0),
        0.0
    );

    pane.clear_tool_hit_rects();
    assert_eq!(pane.scroll_markdown_horizontal_at(40.0, 140.0, 20.0), None);
    assert_eq!(
        pane.markdown_horizontal_scroll_offset("markdown:message-1:code:0", 240.0),
        75.0
    );

    pane.register_markdown_horizontal_scrollbar(
        "markdown:message-1:code:0".to_string(),
        [20.0, 200.0, 300.0, 16.0],
        [20.0, 200.0, 100.0, 16.0],
        240.0,
    );
    assert!(pane.begin_markdown_horizontal_scrollbar_drag(50.0, 208.0));
    pane.clear_tool_hit_rects();
    assert!(pane.markdown_horizontal_scrollbar_dragging());
    assert!(pane.drag_markdown_horizontal_scrollbar_to(150.0));
    assert!(
        pane.markdown_horizontal_scroll_offset("markdown:message-1:code:0", 240.0) > 75.0
    );
    assert!(pane.end_markdown_horizontal_scrollbar_drag());
}

#[test]
fn timeline_growth_preserves_reader_position_when_scrolled_up() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);

    let max_scroll = pane.max_timeline_scroll();
    let scroll_top = max_scroll - pane.timeline_scroll_offset();
    assert_eq!(scroll_top, 400.0);
}

#[test]
fn anchor_restore_shifts_active_wheel_target_by_actual_scroll_delta() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_follow_bottom = false;
    pane.timeline_scroll_px = 200.0;
    pane.timeline_wheel_target_px = Some(250.0);

    pane.restore_timeline_view_anchor(300.0, 0.0);

    assert_eq!(pane.timeline_scroll_px, 300.0);
    assert_eq!(pane.timeline_wheel_target_px, Some(350.0));
}

#[test]
fn identical_pending_prompts_consume_server_occurrences_one_to_one() {
    let mut pane = NeoismAgentPane::default();
    pane.messages = vec![
        NeoismAgentMessage::user("same").with_id("local-1"),
        NeoismAgentMessage::user("same").with_id("local-2"),
    ];
    pane.pending_user_prompts = vec!["same".to_string(), "same".to_string()];

    let merged = pane.merge_pending_user_prompts(vec![
        NeoismAgentMessage::user("same").with_id("server-1")
    ]);

    assert_eq!(merged.len(), 2);
    assert_eq!(pane.pending_user_prompts, vec!["same"]);
    assert!(merged.iter().any(|message| message.id == "local-2"));
}

#[test]
fn duplicate_id_structural_snapshot_is_not_a_stable_source_prefix() {
    let previous = vec![
        NeoismAgentMessage::user("first").with_id("duplicate"),
        NeoismAgentMessage::user("second").with_id("duplicate"),
    ];
    let incoming = vec![
        NeoismAgentMessage::user("second").with_id("duplicate"),
        NeoismAgentMessage::user("replacement").with_id("duplicate"),
        NeoismAgentMessage::assistant("tail").with_id("tail"),
    ];

    assert!(!super::ingest::stable_timeline_source_prefix(
        &previous, &incoming
    ));
}

#[test]
fn mouse_wheel_notch_uses_a_fixed_spring_target() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_wheel_pixels(24.0));
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert_eq!(pane.timeline_wheel_target_px, Some(24.0));
    assert_eq!(pane.timeline_velocity_px_s, 0.0);
    assert!(pane.timeline_is_inertial());
    pane.timeline_last_tick_at = Instant::now().checked_sub(Duration::from_millis(16));

    assert!(pane.tick_timeline_scroll());
    assert!(pane.timeline_scroll_offset() > 0.0);
    assert!(pane.timeline_scroll_offset() < 24.0);
}

#[test]
fn consecutive_mouse_wheel_notches_accumulate_a_deterministic_target() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_wheel_pixels(24.0));
    assert!(pane.scroll_timeline_wheel_pixels(24.0));
    assert_eq!(pane.timeline_wheel_target_px, Some(48.0));
}

#[test]
fn active_scroll_advances_stream_layout_anchor_with_reader() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.set_timeline_view_anchor(
        Some(TimelineViewAnchorKey::at_source(0, 1, "visible-message")),
        24.0,
    );

    assert!(pane.scroll_timeline_wheel_pixels(24.0));
    let (_, immediate_offset) = pane.timeline_view_anchor().expect("anchor");
    assert_eq!(immediate_offset, 24.0);
    pane.timeline_last_tick_at = Instant::now().checked_sub(Duration::from_millis(16));
    assert!(pane.tick_timeline_scroll());
    let (_, spring_offset) = pane.timeline_view_anchor().expect("anchor");
    assert!(spring_offset > immediate_offset);

    // A streamed block above the anchor grows by 200 px in the same frame.
    // Restoring the logical row must keep its post-wheel screen position.
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);
    let shifted_row_top = 624.0;
    pane.restore_timeline_view_anchor(shifted_row_top, spring_offset);
    let restored_scroll_top = pane.max_timeline_scroll() - pane.timeline_scroll_offset();
    assert!((shifted_row_top - restored_scroll_top - spring_offset).abs() < 0.01);

    // The spring moves the same logical anchor along with the reader. If streamed
    // code or a completed apply-patch card remeasures in this frame, restoring
    // this anchor preserves the newly scrolled-to position.
    pane.timeline_last_tick_at = Instant::now().checked_sub(Duration::from_millis(16));
    assert!(pane.tick_timeline_scroll());
    let (_, inertial_offset) = pane.timeline_view_anchor().expect("anchor");
    assert!(inertial_offset > spring_offset);
}

#[test]
fn trackpad_pixels_keep_the_direct_response_path() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_pixels(17.0));
    assert_eq!(pane.timeline_scroll_offset(), 17.0);
}

#[test]
fn timeline_prepend_preserves_reader_position_when_scrolled_up() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.pending_timeline_prepend_height_px = Some(900.0);

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);

    let max_scroll = pane.max_timeline_scroll();
    let scroll_top = max_scroll - pane.timeline_scroll_offset();
    assert_eq!(scroll_top, 600.0);
}

#[test]
fn measured_prepend_ignores_simultaneous_live_tail_growth() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.pending_timeline_prepend_height_px = Some(900.0);
    pane.pending_timeline_prepend_delta_px = Some(200.0);

    // 200px was inserted above; the other 100px grew below in the live turn.
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1200.0, 300.0);

    let max_scroll = pane.max_timeline_scroll();
    let scroll_top = max_scroll - pane.timeline_scroll_offset();
    assert_eq!(scroll_top, 600.0);
}

#[test]
fn timeline_prepend_anchor_survives_until_content_height_grows() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 200.0;
    pane.pending_timeline_prepend_height_px = Some(900.0);

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    assert_eq!(pane.pending_timeline_prepend_height_px, Some(900.0));

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);
    let max_scroll = pane.max_timeline_scroll();
    let scroll_top = max_scroll - pane.timeline_scroll_offset();
    assert_eq!(scroll_top, 600.0);
    assert_eq!(pane.pending_timeline_prepend_height_px, None);
}

#[test]
fn older_timeline_request_skipped_while_following_bottom() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());

    pane.maybe_request_older_timeline_page(0.0, 500.0);

    assert!(!pane.timeline_history.loading_older);
    assert!(pane.drain_pending_outbound().is_empty());
}

#[test]
fn older_timeline_request_gate_reopens_after_success() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    pane.timeline_follow_bottom = false;
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert!(pane.timeline_history.loading_older);
    let commands = pane.drain_pending_outbound();
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        &commands[0],
        OutboundAgentCommand::LoadOlderTimeline { limit: 64, .. }
    ));

    // The in-flight gate blocks duplicates.
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert_eq!(pane.drain_pending_outbound().len(), 0);

    // Completing a page does not cascade into another request just because
    // hidden tool rows left the viewport near the boundary.
    pane.timeline_history.loading_older = false;
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert_eq!(pane.drain_pending_outbound().len(), 0);

    // Explicit movement farther into history re-arms exactly one page.
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 300.0], 900.0, 300.0);
    assert!(pane.scroll_timeline_pixels(10.0));
    pane.maybe_request_older_timeline_page(0.0, 500.0);
    assert_eq!(pane.drain_pending_outbound().len(), 1);
}

#[test]
fn older_timeline_request_skipped_below_boundary() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    pane.timeline_follow_bottom = false;

    pane.maybe_request_older_timeline_page(800.0, 500.0);
    assert!(!pane.timeline_history.loading_older);
    assert_eq!(pane.drain_pending_outbound().len(), 0);
}

#[test]
fn apply_older_page_prepends_and_keeps_loading_when_full() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    let mut current = NeoismAgentMessage::user("current");
    current.id = "m-current".to_string();
    pane.messages = vec![current];
    pane.timeline_history.loading_older = true;
    pane.timeline_history.last_requested_session_id = Some("session-1".to_string());

    let mut older = NeoismAgentMessage::user("older");
    older.id = "m-older".to_string();
    // A full page (raw_count == requested limit) means more may remain.
    pane.apply_older_timeline_page(
        "session-1".to_string(),
        vec![older],
        1,
        1,
        Some("raw-oldest".to_string()),
        false,
    );

    assert_eq!(pane.messages.len(), 2);
    assert_eq!(pane.messages[0].id, "m-older");
    assert!(pane.timeline_history.has_older);
    assert!(!pane.timeline_history.loading_older);
    assert_eq!(
        pane.timeline_history.last_requested_session_id.as_deref(),
        Some("session-1")
    );
    assert_eq!(
        pane.timeline_history.oldest_loaded_cursor.as_deref(),
        Some("raw-oldest")
    );
    // The prepend is folded incrementally, not via a full relayout.
    assert_eq!(pane.pending_timeline_prepend_count, Some(1));
}

#[test]
fn timeline_prepend_count_accumulates_and_invalidation_clears_it() {
    let mut pane = NeoismAgentPane::default();
    pane.timeline_live_trace_start = Some(10);
    pane.note_timeline_prepend(3);
    pane.note_timeline_prepend(2);
    assert_eq!(pane.pending_timeline_prepend_count, Some(5));
    assert_eq!(pane.timeline_live_trace_start, Some(15));

    // A full invalidation makes the incremental fold moot.
    pane.invalidate_timeline_layout();
    assert_eq!(pane.pending_timeline_prepend_count, None);

    // take consumes the pending fold exactly once.
    pane.note_timeline_prepend(4);
    assert_eq!(pane.take_timeline_prepend(), Some(4));
    assert_eq!(pane.take_timeline_prepend(), None);
}

#[test]
fn apply_older_page_stops_at_start_on_short_page() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-1".to_string());
    let mut current = NeoismAgentMessage::user("current");
    current.id = "m-current".to_string();
    pane.messages = vec![current];
    pane.timeline_history.loading_older = true;

    let mut older = NeoismAgentMessage::user("older");
    older.id = "m-older".to_string();
    // Server returned fewer messages than requested → reached the start.
    pane.apply_older_timeline_page(
        "session-1".to_string(),
        vec![older],
        1,
        128,
        Some("raw-oldest".to_string()),
        true,
    );

    assert_eq!(pane.messages.len(), 2);
    assert!(!pane.timeline_history.has_older);
}

#[test]
fn apply_older_page_ignored_after_session_switch() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("session-2".to_string());
    pane.timeline_history.loading_older = true;

    let mut older = NeoismAgentMessage::user("older");
    older.id = "m-older".to_string();
    pane.apply_older_timeline_page(
        "session-1".to_string(),
        vec![older],
        1,
        1,
        Some("raw-oldest".to_string()),
        false,
    );

    assert!(pane.messages.is_empty());
    assert!(!pane.timeline_history.loading_older);
}

#[test]
fn timeline_growth_keeps_following_stream_at_bottom() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);
    pane.timeline_scroll_px = 0.0;

    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1100.0, 300.0);

    assert_eq!(pane.timeline_scroll_offset(), 0.0);
}

#[test]
fn timeline_growth_respects_upward_scroll_intent_near_bottom() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_pixels(1.0));
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1000.0, 300.0);

    assert!((pane.timeline_scroll_offset() - 101.0).abs() < 0.01);
}

#[test]
fn returning_to_timeline_bottom_restores_following() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_pixels(100.0));
    assert!(pane.scroll_timeline_pixels(-100.0));
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 1000.0, 300.0);

    assert_eq!(pane.timeline_scroll_offset(), 0.0);
}

#[test]
fn ctrl_u_d_half_page_scroll_moves_timeline_by_half_viewport() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([10.0, 100.0, 400.0, 300.0], 900.0, 300.0);

    assert!(pane.scroll_timeline_half_page(true));
    assert_eq!(pane.timeline_scroll_offset(), 150.0);

    assert!(pane.scroll_timeline_half_page(false));
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
}

#[test]
fn transcript_selection_can_start_in_whitespace_beside_text() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 200.0, 200.0);
    pane.register_selectable_line("select me", [20.0, 30.0, 70.0, 20.0]);

    assert!(pane.begin_selection_at(300.0, 40.0));
    assert!(pane.has_active_selection());
}

#[test]
fn transcript_selection_preserves_registered_blank_lines() {
    let mut pane = NeoismAgentPane::default();
    pane.set_timeline_metrics([0.0, 0.0, 400.0, 200.0], 200.0, 200.0);
    pane.register_selectable_line("first", [0.0, 20.0, 50.0, 18.0]);
    pane.register_selectable_line("", [0.0, 40.0, 12.0, 18.0]);
    pane.register_selectable_line("second", [0.0, 60.0, 60.0, 18.0]);

    assert!(pane.begin_selection_at(0.0, 20.0));
    assert!(pane.drag_selection_to(60.0, 60.0));
    assert_eq!(pane.end_selection().as_deref(), Some("first\n\nsecond"));
}

fn task_tool_message(task_id: &str, status: &str) -> NeoismAgentMessage {
    let mut message = NeoismAgentMessage::tool(
        "Task(child)",
        format!("task_id: {task_id}\nstatus: {status}"),
        status,
        "task",
        NeoismAgentOutputKind::Text,
        "",
        Vec::new(),
    );
    message.detail = format!("task_id: {task_id}\nstatus: {status}");
    message
}

fn test_permission(selected: usize) -> NeoismAgentPendingPermission {
    NeoismAgentPendingPermission {
        id: "perm-1".to_string(),
        session_id: "session-1".to_string(),
        parent_session_id: None,
        source_agent: None,
        source_title: None,
        title: "Run command".to_string(),
        permission: "shell".to_string(),
        patterns: Vec::new(),
        selected,
        responding: false,
    }
}
