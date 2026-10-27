use super::*;
use serde_json::{json, Value};

mod review;

#[test]
fn esc_handoff_stays_neutral_and_next_dequeue_releases_abort_filter() {
    for windows in [false, true] {
        for state in [
            NeoismAgentStreamingState::Generating,
            NeoismAgentStreamingState::Thinking,
            NeoismAgentStreamingState::Working,
        ] {
            for cached in [false, true] {
                let mut pane = pane(windows);
                feed(&mut pane, vec![queue("dequeue", 0, true, None)]);
                pane.note_streaming(state, Some("old tool".into()));
                pane.queued_prompt_count = 1;
                pane.queued_prompt_preview = Some("next".into());
                let start = pane.streaming_started_at;
                pane.abort_session();
                assert!(pane.has_status_activity());
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
                assert!(pane.streaming_tool_label.is_none());
                assert_eq!(pane.streaming_started_at, start);
                assert!(pane.suppress_streaming_after_abort());
                if cached {
                    pane.cache_current_session(false);
                    pane.session_id = Some("other".into());
                }
                // Old-run idle must not create an empty activity frame.
                pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
                    "root",
                    [AgentSessionUpdate::SessionIdle],
                ));
                pane.drain_live_session_updates();
                if cached {
                    assert_eq!(
                        pane.session_cache["root"].runtime.streaming_state,
                        NeoismAgentStreamingState::Generating
                    );
                } else {
                    assert!(pane.has_status_activity());
                    assert!(pane.suppress_streaming_after_abort());
                }
                let mut dequeue = queue("dequeue", 0, true, None);
                dequeue["properties"]["messageID"] = json!("next");
                dequeue["properties"]["request"]["messageID"] = json!("next");
                dequeue["properties"]["request"]["parts"][0]["text"] = json!("next");
                feed(&mut pane, vec![dequeue]);
                if cached {
                    let runtime = std::mem::take(
                        &mut pane.session_cache.get_mut("root").unwrap().runtime,
                    );
                    pane.session_id = Some("root".into());
                    pane.restore_session_runtime_ui(runtime);
                }
                assert!(!pane.suppress_streaming_after_abort());
                assert!(pane.abort_requested_at.is_none());
                assert!(pane.has_status_activity());
                pane.note_streaming_from_part(
                    NeoismAgentMessageKind::Reasoning,
                    "new reasoning",
                );
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Thinking);
            }
        }
    }
}

#[test]
fn esc_compact_only_stops_but_queue_clear_settles_a_handoff() {
    for windows in [false, true] {
        let mut pane = pane(windows);
        pane.queued_prompt_count = 1;
        pane.queued_prompt_preview = Some("Compaction queued".into());
        pane.pending_compact = Some("compact-request".into());
        pane.abort_session();
        assert!(!pane.is_streaming());
        assert_eq!(pane.queued_prompt_count(), 0);
        assert!(pane.pending_compact.is_none());
        pane.note_streaming(NeoismAgentStreamingState::Working, Some("tool".into()));
        pane.queued_prompt_count = 1;
        pane.queued_prompt_preview = Some("next".into());
        pane.abort_session();
        assert!(pane.is_streaming());
        feed(&mut pane, vec![queue("clear", 0, false, None)]);
        assert!(!pane.is_streaming());
        assert!(!pane.status_timing.handoff.waiting());
    }
}

#[test]
fn new_started_at_promotes_handoff_and_late_abort_failure_cannot_stop_successor() {
    let mut pane = pane(false);
    feed(
        &mut pane,
        vec![
            json!({"type": "session.status", "properties": {"sessionID": "root",
                "status": {"type": "busy", "startedAt": 100, "queue": {"count": 1, "preview": "next"}}
            }}),
        ],
    );
    pane.abort_session();
    feed(
        &mut pane,
        vec![
            json!({"type": "session.status", "properties": {"sessionID": "root",
                "status": {"type": "busy", "startedAt": 200, "queue": {"count": 0}}
            }}),
        ],
    );
    assert!(!pane.suppress_streaming_after_abort());
    assert!(pane.is_streaming());
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::AbortFailed {
            session_id: "root".into(),
            error: "late HTTP failure".into(),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(pane.is_streaming());
    pane.background_sender()
        .send(NeoismAgentBackgroundUpdate::AbortFailed {
            session_id: "other".into(),
            error: "other origin".into(),
        })
        .unwrap();
    pane.drain_background_updates();
    assert!(pane.is_streaming());
    assert!(pane.session_cache["other"]
        .messages
        .iter()
        .any(|message| message.text.contains("other origin")));
}

#[test]
fn esc_without_ordinary_queue_immediately_clears_every_activity_kind() {
    for windows in [false, true] {
        for state in [
            NeoismAgentStreamingState::Generating,
            NeoismAgentStreamingState::Thinking,
            NeoismAgentStreamingState::Working,
        ] {
            for compact_only in [false, true] {
                let mut pane = pane(windows);
                pane.note_streaming(state, Some("old tool or reason".into()));
                // Populate the display hold too: Stop must clear it immediately.
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
                assert_eq!(pane.queued_prompt_count(), 0);
                assert!(pane.pending_compact.is_none());
                assert!(pane.suppress_streaming_after_abort());
                pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
                    "root",
                    [AgentSessionUpdate::PartUpdated {
                        message: NeoismAgentMessage::reasoning("late old reasoning"),
                        parent_message_id: None,
                    }],
                ));
                pane.drain_live_session_updates();
                assert!(!pane.has_status_activity());
            }
        }
    }
}

#[test]
fn handoff_running_snapshot_recovers_missed_dequeue_but_clear_or_failure_never_starts_work(
) {
    for windows in [false, true] {
        for action in ["snapshot", "clear", "failed"] {
            let mut pane = pane(windows);
            feed(&mut pane, vec![queue("dequeue", 0, true, None)]);
            pane.queued_prompt_count = 1;
            pane.queued_prompt_preview = Some("next".into());
            pane.abort_session();
            feed(&mut pane, vec![queue(action, 0, true, None)]);
            assert_eq!(pane.is_streaming(), action == "snapshot");
            assert_eq!(pane.status_timing.handoff.waiting(), action == "snapshot");
            if action == "snapshot" {
                assert!(pane.suppress_streaming_after_abort());
                feed(
                    &mut pane,
                    vec![json!({"type": "session.status", "properties": {
                        "sessionID": "root", "status": {"type": "busy", "startedAt": 200, "queue": {"count": 0}}
                    }})],
                );
                assert!(pane.is_streaming());
                assert!(!pane.suppress_streaming_after_abort());
            }
        }
    }
}

#[test]
fn idle_after_an_ambiguous_empty_running_snapshot_settles_the_handoff() {
    let mut pane = pane(false);
    pane.queued_prompt_count = 1;
    pane.queued_prompt_preview = Some("next".into());
    pane.abort_session();
    feed(&mut pane, vec![queue("snapshot", 0, true, None)]);
    assert!(pane.is_streaming());
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root",
        [AgentSessionUpdate::SessionIdle],
    ));
    pane.drain_live_session_updates();
    assert!(!pane.is_streaming());
    assert!(!pane.status_timing.handoff.waiting());
}

fn pane(windows: bool) -> NeoismAgentPane {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.status_timing = status_timing::DesktopStatusTiming::for_test(windows);
    // Reopened, hydrated history is not an admission. Only Enter reserves one.
    pane.messages
        .push(NeoismAgentMessage::user("old turn").with_id("old"));
    pane.apply_runtime_status_for_session("root", &HashMap::new());
    pane.note_prompt_admission("first");
    pane.messages
        .push(NeoismAgentMessage::user("hello").with_id("first"));
    pane.note_streaming(NeoismAgentStreamingState::Generating, None);
    pane
}

fn queue(action: &str, count: usize, running: bool, preview: Option<&str>) -> Value {
    json!({"type": "session.queue.updated", "properties": {
        "sessionID": "root", "action": action, "messageID": "first",
        "request": {"type": "prompt", "messageID": "first", "parts": [{"type": "text", "text": "hello"}]},
        "queue": {"count": count, "running": running, "preview": preview}
    }})
}

fn busy(count: usize) -> Value {
    json!({"type": "session.status", "properties": {"sessionID": "root",
        "status": {"type": "busy", "queue": {"count": count, "preview": (count > 0).then_some("hello")}}
    }})
}

fn feed(pane: &mut NeoismAgentPane, events: Vec<Value>) {
    pane.event_stream = Some(AgentSessionEventStream::with_events_for_test(
        "root", events,
    ));
    pane.drain_live_session_updates();
}

#[test]
fn canonical_idle_primary_never_flashes_a_waiting_row_at_any_frame_boundary() {
    for windows in [false, true] {
        let events = vec![
            queue("enqueue", 1, false, Some("hello")),
            busy(1),
            queue("dequeue", 0, true, None),
            busy(0),
        ];
        // Every possible batching of the canonical decoder -> classifier ->
        // desktop stream sequence, including fully split and fully batched.
        for boundaries in 0..8 {
            let mut pane = pane(windows);
            let mut batch = Vec::new();
            for (index, event) in events.iter().enumerate() {
                batch.push(event.clone());
                if index == 3 || boundaries & (1 << index) != 0 {
                    feed(&mut pane, std::mem::take(&mut batch));
                    assert_eq!(pane.queued_prompt_count(), 0);
                    assert_eq!(pane.streaming_label(), "Crafting");
                }
            }
            assert_eq!(
                pane.messages
                    .iter()
                    .filter(|message| message.id == "first")
                    .count(),
                1
            );
        }
    }
}

#[test]
fn canonical_followup_survives_primary_dequeue_active_and_cached_roundtrip() {
    for windows in [false, true] {
        for cached in [false, true] {
            let mut pane = pane(windows);
            if cached {
                pane.cache_current_session(false);
                pane.session_id = Some("other".into());
            }
            feed(&mut pane, vec![queue("enqueue", 2, false, Some("hello"))]);
            if cached {
                assert_eq!(pane.session_cache["root"].runtime.queued_prompt_count, 1);
            } else {
                assert_eq!(pane.queued_prompt_count(), 1);
            }
            feed(&mut pane, vec![queue("dequeue", 1, true, Some("next"))]);
            if cached {
                let runtime = std::mem::take(
                    &mut pane.session_cache.get_mut("root").unwrap().runtime,
                );
                pane.session_id = Some("root".into());
                pane.restore_session_runtime_ui(runtime);
            }
            assert_eq!(pane.queued_prompt_count(), 1);
            assert_eq!(pane.queued_prompt_preview(), Some("next"));
        }
    }
}

#[test]
fn running_snapshot_closes_suppression_without_losing_late_primary_identity() {
    for windows in [false, true] {
        let mut pane = pane(windows);
        feed(&mut pane, vec![queue("enqueue", 1, false, Some("hello"))]);
        assert_eq!(pane.queued_prompt_count(), 0);
        feed(&mut pane, vec![queue("snapshot", 1, true, Some("next"))]);
        assert_eq!(pane.queued_prompt_count(), 1);
        feed(&mut pane, vec![queue("snapshot", 1, false, Some("next"))]);
        assert_eq!(pane.queued_prompt_count(), 1);
        feed(&mut pane, vec![queue("dequeue", 1, true, Some("next"))]);
        assert_eq!(pane.queued_prompt_count(), 1);
    }
}

#[test]
fn primary_admission_never_hides_compact_or_changes_countless_notification_idempotence() {
    for windows in [false, true] {
        let mut pane = pane(windows);
        pane.execute_slash_text("/compact");
        assert_eq!(pane.queued_prompt_count(), 1);
        feed(
            &mut pane,
            vec![queue("enqueue", 2, false, Some("Compaction queued"))],
        );
        assert_eq!(pane.queued_prompt_count(), 1);
        let notification = json!({"type": "session.compaction.queued", "properties": {"sessionID": "root"}});
        feed(&mut pane, vec![notification.clone(), notification]);
        assert_eq!(pane.queued_prompt_count(), 1);
        feed(
            &mut pane,
            vec![queue("snapshot", 1, false, Some("Compaction queued"))],
        );
        assert_eq!(pane.queued_prompt_count(), 1);
        assert_eq!(pane.queued_prompt_preview(), Some("Compaction queued"));
    }
}

#[test]
fn hydration_projects_only_local_primary_and_running_evidence_closes_it() {
    for windows in [false, true] {
        let mut pane = pane(windows);
        let mut statuses = HashMap::from([(
            "root".into(),
            super::super::api::SessionStatusSnapshot {
                kind: "busy".into(),
                queue_count: 1,
                preview: Some("hello".into()),
                ..Default::default()
            },
        )]);
        pane.apply_runtime_status_for_session("root", &statuses);
        assert_eq!(pane.queued_prompt_count(), 0);
        statuses.get_mut("root").unwrap().running = Some(true);
        pane.apply_runtime_status_for_session("root", &statuses);
        assert_eq!(pane.queued_prompt_count(), 1);
        statuses.get_mut("root").unwrap().running = Some(false);
        pane.apply_runtime_status_for_session("root", &statuses);
        assert_eq!(pane.queued_prompt_count(), 1);
        let mut cold = NeoismAgentPane::default();
        cold.session_id = Some("root".into());
        cold.apply_runtime_status_for_session("root", &statuses);
        assert_eq!(cold.queued_prompt_count(), 1);
    }
}

#[test]
fn esc_shows_the_queued_prompt_bubble_before_the_server_dequeues_it() {
    for windows in [false, true] {
        let mut pane = pane(windows);
        feed(&mut pane, vec![queue("dequeue", 0, true, None)]);
        // What `submit()` records for a prompt sent behind a running turn.
        pane.queued_prompt_count = 1;
        pane.queued_prompt_preview = Some("next".into());
        pane.status_timing
            .note_local_queued(NeoismAgentMessage::user("next").with_id("next"));

        pane.abort_session();
        let bubbles = |pane: &NeoismAgentPane| {
            pane.messages.iter().filter(|message| message.id == "next").count()
        };
        assert_eq!(bubbles(&pane), 1, "bubble must appear on Esc, not on echo");
        assert_eq!(pane.messages.last().unwrap().id, "next");
        assert_eq!(pane.queued_prompt_count(), 0);
        assert!(pane.queued_prompt_preview().is_none());
        assert!(pane.has_status_activity());

        // The stopped run's busy snapshot still counts the prompt as queued.
        feed(
            &mut pane,
            vec![json!({"type": "session.status", "properties": {"sessionID": "root",
                "status": {"type": "busy", "startedAt": 100,
                    "queue": {"count": 1, "preview": "next"}}
            }})],
        );
        assert_eq!(pane.queued_prompt_count(), 0);
        assert!(pane.queued_prompt_preview().is_none());

        let mut dequeue = queue("dequeue", 0, true, None);
        dequeue["properties"]["messageID"] = json!("next");
        dequeue["properties"]["request"]["messageID"] = json!("next");
        dequeue["properties"]["request"]["parts"][0]["text"] = json!("next");
        feed(&mut pane, vec![dequeue]);
        assert_eq!(bubbles(&pane), 1, "echo reconciles onto the optimistic bubble");
        assert_eq!(pane.queued_prompt_count(), 0);
        assert!(pane.has_status_activity());
    }
}

#[test]
fn esc_without_a_local_queue_entry_waits_for_the_server_echo() {
    let mut pane = pane(false);
    feed(&mut pane, vec![queue("dequeue", 0, true, None)]);
    // Queued by another client: nothing local to promote.
    pane.queued_prompt_count = 1;
    pane.queued_prompt_preview = Some("theirs".into());
    let before = pane.messages.len();
    pane.abort_session();
    assert_eq!(pane.messages.len(), before);
    assert_eq!(pane.queued_prompt_count(), 1);
}
