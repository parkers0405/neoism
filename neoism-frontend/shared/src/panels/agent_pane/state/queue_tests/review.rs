use super::*;

fn restore(pane: &mut NeoismAgentPane) {
    let cached = pane.session_cache.remove("root").unwrap();
    pane.session_id = Some("root".into());
    pane.messages = cached.messages;
    pane.restore_session_runtime_ui(cached.runtime);
}

#[test]
fn old_start_empty_busy_is_ambiguous_for_active_and_cached_status_or_snapshot() {
    for cached in [false, true] {
        for canonical in [false, true] {
            let (mut pane, id) = submitted();
            pane.apply_queue_event("root", queue(&id, "dequeue", 0, true, None));
            pane.apply_queue(1, Some("B".into()), Some(100));
            pane.abort_session();
            if cached {
                pane.cache_current_session();
                pane.session_id = Some("other".into());
            }
            if canonical {
                pane.apply_queue_event("root", queue("next", "snapshot", 0, true, None));
            } else if cached {
                pane.cache_apply_queue("root", 0, None, Some(100));
            } else {
                pane.apply_queue(0, None, Some(100));
            }
            if cached {
                let runtime = &pane.session_cache["root"].runtime;
                assert!(runtime.queue_handoff.waiting());
                assert!(runtime.suppress_streaming_after_abort());
                assert_eq!(
                    runtime.streaming_state,
                    NeoismAgentStreamingState::Generating
                );
                pane.cache_apply_queue("root", 0, None, Some(200));
                restore(&mut pane);
            } else {
                assert!(pane.queue_handoff.waiting());
                assert!(pane.has_status_activity());
                assert!(pane.suppress_streaming_after_abort());
                pane.apply_queue(0, None, Some(200));
            }
            assert!(pane.is_streaming());
            assert!(!pane.suppress_streaming_after_abort());
            pane.note_session_idle();
            assert!(!pane.is_streaming());
        }
    }
}

#[test]
fn real_primary_head_preview_is_cleared_but_followup_count_and_post_dequeue_preview_survive(
) {
    for cached in [false, true] {
        for canonical in [false, true] {
            let (mut pane, id) = submitted();
            if cached {
                pane.cache_current_session();
                pane.session_id = Some("other".into());
            }
            if canonical {
                pane.apply_queue_event(
                    "root",
                    queue(&id, "enqueue", 2, false, Some("hello")),
                );
            } else if cached {
                pane.cache_apply_queue("root", 2, Some("hello".into()), None);
            } else {
                pane.apply_queue(2, Some("hello".into()), None);
            }
            if cached {
                assert_eq!(pane.session_cache["root"].runtime.queued_prompt_count, 1);
                assert_eq!(
                    pane.session_cache["root"].runtime.queued_prompt_preview,
                    None
                );
            } else {
                assert_eq!(pane.queued_prompt_count(), 1);
                assert_eq!(pane.queued_prompt_preview(), None);
            }
            pane.apply_queue_event("root", queue(&id, "dequeue", 1, true, Some("B")));
            if cached {
                restore(&mut pane);
            }
            assert_eq!(pane.queued_prompt_count(), 1);
            assert_eq!(pane.queued_prompt_preview(), Some("B"));
        }
    }
    let (mut pane, id) = submitted();
    pane.apply_queue_event(
        "root",
        queue(&id, "enqueue", 2, false, Some("Compaction queued")),
    );
    assert_eq!(pane.queued_prompt_count(), 1);
    assert_eq!(pane.queued_prompt_preview(), Some("Compaction queued"));
}

fn part(id: &str, tool: bool) -> NeoismAgentMessage {
    let mut message = if tool {
        NeoismAgentMessage::assistant("saved tool")
    } else {
        NeoismAgentMessage::reasoning("saved reasoning plus late")
    };
    message.id = id.into();
    if tool {
        message.kind = NeoismAgentMessageKind::Tool;
        message.title = "same tool title".into();
    }
    message
}

fn old_parts(pane: &mut NeoismAgentPane, cached: bool) {
    if cached {
        pane.cache_upsert_part_message_with_parent(
            "root",
            part("old-reason", false),
            Some("old-assistant"),
        );
        pane.cache_upsert_part_message_with_parent(
            "root",
            part("old-new-tool", true),
            Some("old-assistant"),
        );
        pane.cache_apply_part_delta_with_parent(
            "root",
            Some("old-assistant"),
            Some("old-new-reason"),
            Some("reasoning"),
            "late old delta",
        );
    } else {
        pane.ingest_live_part_message_with_parent(
            part("old-reason", false),
            Some("old-assistant"),
        );
        pane.ingest_live_part_message_with_parent(
            part("old-new-tool", true),
            Some("old-assistant"),
        );
        pane.ingest_live_part_delta(
            Some("old-assistant".into()),
            Some("old-new-reason".into()),
            Some("reasoning".into()),
            "late old delta",
        );
    }
}

#[test]
fn cancelled_assistant_parts_cannot_relabel_successor_after_status_or_dequeue_even_in_cache(
) {
    for cached in [false, true] {
        for by_status in [false, true] {
            let (mut pane, id) = submitted();
            pane.apply_queue_event("root", queue(&id, "dequeue", 0, true, None));
            pane.ingest_live_part_message_with_parent(
                part("old-reason", false),
                Some("old-assistant"),
            );
            pane.apply_queue(1, Some("B".into()), Some(100));
            pane.abort_session();
            if cached {
                pane.cache_current_session();
                pane.session_id = Some("other".into());
            }
            old_parts(&mut pane, cached);
            if cached {
                assert_eq!(
                    pane.session_cache["root"].runtime.streaming_state,
                    NeoismAgentStreamingState::Generating
                );
            } else {
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
            }
            if by_status {
                if cached {
                    pane.cache_apply_queue("root", 0, None, Some(200));
                } else {
                    pane.apply_queue(0, None, Some(200));
                }
            } else {
                pane.apply_queue_event("root", queue("next", "dequeue", 0, true, None));
            }
            old_parts(&mut pane, cached);
            if cached {
                assert_eq!(
                    pane.session_cache["root"].runtime.streaming_state,
                    NeoismAgentStreamingState::Generating
                );
                pane.cache_upsert_part_message_with_parent(
                    "root",
                    part("new-reason", false),
                    Some("new-assistant"),
                );
                assert_eq!(
                    pane.session_cache["root"].runtime.streaming_state,
                    NeoismAgentStreamingState::Thinking
                );
                old_parts(&mut pane, true);
                assert_eq!(
                    pane.session_cache["root"].runtime.streaming_state,
                    NeoismAgentStreamingState::Thinking
                );
                pane.cache_apply_part_delta_with_parent(
                    "root",
                    Some("new-assistant"),
                    Some("new-text"),
                    Some("text"),
                    "new output",
                );
                assert_eq!(
                    pane.session_cache["root"].runtime.streaming_state,
                    NeoismAgentStreamingState::Generating
                );
                restore(&mut pane);
            } else {
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
                pane.ingest_live_part_message_with_parent(
                    part("new-reason", false),
                    Some("new-assistant"),
                );
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Thinking);
                old_parts(&mut pane, false);
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Thinking);
            }
            assert!(!pane.suppress_streaming_after_abort());
            assert!(pane
                .messages
                .iter()
                .all(|message| message.id != "old-new-tool"
                    && message.id != "old-new-reason"));
            pane.ingest_live_part_delta(
                Some("new-assistant".into()),
                Some("new-text".into()),
                Some("text".into()),
                "new text",
            );
            assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
            pane.ingest_live_part_message_with_parent(
                part("new-tool", true),
                Some("new-assistant"),
            );
            assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Working);
            let snapshot = pane.messages.clone();
            pane.apply_history(snapshot);
            assert!(pane
                .messages
                .iter()
                .any(|message| message.id == "old-reason"
                    && message.text.contains("saved reasoning")));
            assert!(!pane
                .queue_handoff
                .stopped_parts
                .allows("old-new-tool", None));
            assert!(pane
                .queue_handoff
                .stopped_parts
                .allows("new-reason", Some("new-assistant")));
            pane.messages.push(
                NeoismAgentMessage::reasoning("saved old tail").with_id("old-reason"),
            );
            pane.note_streaming(NeoismAgentStreamingState::Generating, None);
            pane.refresh_streaming_from_tail();
            assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
        }
    }
}
