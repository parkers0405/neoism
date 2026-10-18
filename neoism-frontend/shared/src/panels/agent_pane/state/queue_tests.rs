use super::*;
use serde_json::{json, Value};

mod review;

fn queue(
    id: &str,
    action: &str,
    count: usize,
    running: bool,
    preview: Option<&str>,
) -> Value {
    json!({"sessionID": "root", "action": action, "messageID": id,
        "request": {"type": "prompt", "messageID": id, "parts": [{"type": "text", "text": "hello"}]},
        "queue": {"count": count, "running": running, "preview": preview}})
}

#[test]
fn shared_esc_without_ordinary_queue_clears_all_activity_immediately() {
    for state in [
        NeoismAgentStreamingState::Generating,
        NeoismAgentStreamingState::Thinking,
        NeoismAgentStreamingState::Working,
    ] {
        for compact_only in [false, true] {
            let (mut pane, _) = submitted();
            pane.note_streaming(state, Some("old tool or reason".into()));
            let _ = pane.streaming_state();
            if compact_only {
                pane.queued_prompt_count = 1;
                pane.queued_prompt_preview = Some("Compaction queued".into());
                pane.pending_compact = Some("compact".into());
            }
            pane.abort_session();
            assert!(!pane.is_streaming());
            assert!(!pane.has_status_activity());
            assert_eq!(pane.streaming_state(), NeoismAgentStreamingState::Idle);
            assert!(pane.streaming_tool_label.is_none());
            assert!(pane.pending_compact.is_none());
            assert_eq!(pane.queued_prompt_count(), 0);
            assert!(pane.suppress_streaming_after_abort());
            pane.note_streaming_from_part(
                NeoismAgentMessageKind::Reasoning,
                "late reasoning",
            );
            pane.note_remote_streaming(
                NeoismAgentStreamingState::Working,
                Some("late tool".into()),
            );
            assert!(!pane.has_status_activity());
        }
    }
}

#[test]
fn shared_primary_sequence_has_no_waiting_frame_and_cold_history_has_no_reservation() {
    let (mut pane, id) = submitted();
    pane.apply_queue_event("root", queue(&id, "enqueue", 1, false, Some("hello")));
    assert_eq!(pane.queued_prompt_count(), 0);
    pane.apply_queue(1, Some("hello".into()), None);
    assert_eq!(pane.queued_prompt_count(), 0);
    pane.apply_queue_event("root", queue(&id, "dequeue", 0, true, None));
    assert_eq!(pane.queued_prompt_count(), 0);
    pane.apply_queue(0, None, None);
    assert_eq!(pane.queued_prompt_count(), 0);
    assert_eq!(
        pane.streaming_state(),
        NeoismAgentStreamingState::Generating
    );
    let mut cold = NeoismAgentPane::default();
    cold.session_id = Some("root".into());
    cold.messages.push(NeoismAgentMessage::user("old history"));
    cold.apply_queue_event("root", queue("remote", "enqueue", 1, false, Some("remote")));
    assert_eq!(cold.queued_prompt_count(), 1);
}

#[test]
fn shared_real_steer_is_waiting_work_not_a_primary_reservation() {
    let (mut pane, id) = submitted();
    pane.apply_queue_event("root", queue(&id, "dequeue", 0, true, None));
    pane.send_prompt_with_echo("followup", "followup", false)
        .unwrap();
    pane.apply_queue_event("root", queue("next", "enqueue", 1, false, Some("followup")));
    assert_eq!(pane.queued_prompt_count(), 1);
}

#[test]
fn shared_handoff_snapshot_recovers_missed_dequeue_but_clear_or_failure_settles() {
    for action in ["snapshot", "clear", "failed"] {
        let (mut pane, id) = submitted();
        pane.apply_queue_event("root", queue(&id, "dequeue", 0, true, None));
        pane.apply_queue(1, Some("next".into()), Some(100));
        pane.abort_session();
        pane.apply_queue_event("root", queue("next", action, 0, true, None));
        assert_eq!(pane.is_streaming(), action == "snapshot");
        assert_eq!(pane.queue_handoff.waiting(), action == "snapshot");
        if action == "snapshot" {
            assert!(pane.suppress_streaming_after_abort());
            pane.apply_queue(0, None, Some(200));
            assert!(pane.is_streaming());
            assert!(!pane.suppress_streaming_after_abort());
        }
    }
}

#[test]
fn shared_idle_after_ambiguous_empty_snapshot_settles_the_handoff() {
    let (mut pane, id) = submitted();
    pane.apply_queue_event("root", queue(&id, "dequeue", 0, true, None));
    pane.apply_queue(1, Some("next".into()), Some(100));
    pane.abort_session();
    pane.apply_queue_event("root", queue("next", "snapshot", 0, true, None));
    assert!(pane.is_streaming());
    pane.note_session_idle();
    assert!(!pane.is_streaming());
    assert!(!pane.queue_handoff.waiting());
}

fn submitted() -> (NeoismAgentPane, String) {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    // No provider is called: this readiness bypass exercises only local echo
    // and outbound admission, which is identical for external/hosted threads.
    pane.new_chat_source = side_panel::ConversationSource::Codex;
    pane.messages.push(NeoismAgentMessage::user("old turn"));
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane.send_prompt_with_echo("hello", "hello", true).unwrap();
    let id = pane.messages.last().unwrap().id.clone();
    (pane, id)
}

#[test]
fn shared_local_primary_projects_canonical_queue_and_preserves_followups_in_cache() {
    for cached in [false, true] {
        let (mut pane, id) = submitted();
        pane.apply_queue_event("root", queue(&id, "enqueue", 1, false, Some("hello")));
        pane.apply_queue(1, Some("hello".into()), None);
        assert_eq!(pane.queued_prompt_count(), 0);
        if cached {
            pane.cache_current_session();
            pane.session_id = Some("other".into());
        }
        pane.apply_queue_event("root", queue(&id, "enqueue", 2, false, Some("hello")));
        pane.apply_queue_event("root", queue(&id, "dequeue", 1, true, Some("next")));
        if cached {
            let runtime =
                std::mem::take(&mut pane.session_cache.get_mut("root").unwrap().runtime);
            pane.session_id = Some("root".into());
            pane.restore_session_runtime_ui(runtime);
        }
        assert_eq!(pane.queued_prompt_count(), 1);
        assert_eq!(pane.queued_prompt_preview(), Some("next"));
    }
}

#[test]
fn shared_running_snapshot_closes_reservation_and_compact_stays_visible() {
    let (mut pane, id) = submitted();
    pane.pending_compact = Some("compact".into());
    assert_eq!(pane.queued_prompt_count(), 1);
    pane.apply_queue_event(
        "root",
        queue(&id, "enqueue", 2, false, Some("Compaction queued")),
    );
    assert_eq!(pane.queued_prompt_count(), 1);
    pane.note_compaction_queued();
    pane.note_compaction_queued();
    assert_eq!(pane.queued_prompt_count(), 1);
    pane.apply_queue_event(
        "root",
        queue(&id, "snapshot", 1, false, Some("Compaction queued")),
    );
    assert_eq!(pane.queued_prompt_count(), 1);
    pane.apply_queue_event("root", queue(&id, "snapshot", 1, true, Some("next")));
    assert_eq!(pane.queued_prompt_count(), 1);
    pane.apply_queue_event("root", queue(&id, "snapshot", 1, false, Some("next")));
    assert_eq!(pane.queued_prompt_count(), 1);
    pane.apply_queue_event("root", queue(&id, "dequeue", 1, true, Some("next")));
    assert_eq!(pane.queued_prompt_count(), 1);
}

#[test]
fn shared_esc_handoff_stays_mounted_and_next_dequeue_unblocks_streaming() {
    for state in [
        NeoismAgentStreamingState::Generating,
        NeoismAgentStreamingState::Thinking,
        NeoismAgentStreamingState::Working,
    ] {
        for cached in [false, true] {
            let (mut pane, id) = submitted();
            pane.apply_queue_event("root", queue(&id, "dequeue", 0, true, None));
            pane.note_streaming(state, Some("old tool".into()));
            pane.apply_queue(1, Some("next".into()), Some(100));
            let start = pane.streaming_started_at;
            pane.abort_session();
            assert!(pane.has_status_activity());
            assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
            assert!(pane.streaming_tool_label.is_none());
            assert_eq!(pane.streaming_started_at, start);
            assert!(pane.suppress_streaming_after_abort());
            pane.note_streaming_from_part(
                NeoismAgentMessageKind::Reasoning,
                "old reasoning",
            );
            assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
            if cached {
                pane.cache_current_session();
                pane.session_id = Some("other".into());
                pane.cache_note_session_idle("root");
                assert_eq!(
                    pane.session_cache["root"].runtime.streaming_state,
                    NeoismAgentStreamingState::Generating
                );
            } else {
                pane.note_session_idle();
                assert!(pane.has_status_activity());
                assert!(pane.suppress_streaming_after_abort());
            }
            pane.apply_queue_event("root", queue("next", "dequeue", 0, true, None));
            if cached {
                let runtime = std::mem::take(
                    &mut pane.session_cache.get_mut("root").unwrap().runtime,
                );
                pane.session_id = Some("root".into());
                pane.restore_session_runtime_ui(runtime);
            }
            assert!(pane.abort_requested_at.is_none());
            assert!(!pane.suppress_streaming_after_abort());
            assert!(pane.has_status_activity());
            pane.note_streaming_from_part(
                NeoismAgentMessageKind::Reasoning,
                "new reasoning",
            );
            assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Thinking);
        }
    }
}

#[test]
fn shared_esc_compact_only_stops_and_cleared_queue_settles_handoff() {
    let (mut pane, _) = submitted();
    pane.apply_queue(1, Some("Compaction queued".into()), None);
    pane.pending_compact = Some("compact".into());
    pane.abort_session();
    assert!(!pane.is_streaming());
    assert_eq!(pane.queued_prompt_count(), 0);
    assert!(pane.pending_compact.is_none());
    pane.note_streaming(NeoismAgentStreamingState::Working, Some("tool".into()));
    pane.apply_queue(1, Some("next".into()), Some(100));
    pane.abort_session();
    assert!(pane.is_streaming());
    pane.apply_queue_event("root", queue("next", "clear", 0, false, None));
    assert!(!pane.is_streaming());
    assert!(!pane.queue_handoff.waiting());
}

#[test]
fn shared_new_status_start_promotes_even_if_dequeue_was_missed() {
    let (mut pane, id) = submitted();
    pane.apply_queue_event("root", queue(&id, "dequeue", 0, true, None));
    pane.apply_queue(1, Some("next".into()), Some(100));
    pane.abort_session();
    pane.apply_queue(1, Some("next".into()), Some(100));
    assert!(pane.suppress_streaming_after_abort());
    pane.apply_queue(0, None, Some(200));
    assert!(!pane.suppress_streaming_after_abort());
    assert!(pane.is_streaming());
}
