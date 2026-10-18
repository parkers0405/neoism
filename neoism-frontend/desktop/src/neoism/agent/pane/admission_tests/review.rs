use super::*;

fn updates(pane: &mut NeoismAgentPane, updates: Vec<AgentSessionUpdate>) {
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root", updates,
    ));
    pane.drain_live_session_updates();
}

fn status(count: usize, started_at: u64) -> AgentSessionUpdate {
    AgentSessionUpdate::QueueStatus {
        count,
        preview: (count > 0).then(|| "B".into()),
        started_at: Some(started_at),
    }
}

fn restore(pane: &mut NeoismAgentPane) {
    let cached = pane.session_cache.remove("root").unwrap();
    pane.session_id = Some("root".into());
    pane.messages = cached.messages;
    pane.restore_session_runtime_ui(cached.runtime);
}

#[test]
fn same_old_start_empty_busy_does_not_settle_status_hydration_or_cached_handoff() {
    for windows in [false, true] {
        for cached in [false, true] {
            for source in ["canonical", "status", "hydration"] {
                let mut pane = pane(windows);
                feed(&mut pane, vec![queue("dequeue", 0, true, None)]);
                updates(&mut pane, vec![status(1, 100)]);
                pane.abort_session();
                if cached {
                    pane.cache_current_session(false);
                    pane.session_id = Some("other".into());
                }
                match source {
                    "canonical" => {
                        feed(&mut pane, vec![queue("snapshot", 0, true, None)])
                    }
                    "status" => updates(&mut pane, vec![status(0, 100)]),
                    _ if cached => pane
                        .session_cache
                        .get_mut("root")
                        .unwrap()
                        .runtime
                        .apply_queue_status(0, None, Some(100)),
                    _ => pane.apply_runtime_status_for_session(
                        "root",
                        &HashMap::from([(
                            "root".into(),
                            super::super::super::api::SessionStatusSnapshot {
                                kind: "busy".into(),
                                queue_count: 0,
                                running: Some(true),
                                started_at: Some(100),
                                ..Default::default()
                            },
                        )]),
                    ),
                }
                if cached {
                    let runtime = &pane.session_cache["root"].runtime;
                    assert!(runtime.status_timing.handoff.waiting(), "{source}");
                    assert!(runtime.suppress_streaming_after_abort());
                    assert_eq!(
                        runtime.streaming_state,
                        NeoismAgentStreamingState::Generating
                    );
                } else {
                    assert!(pane.status_timing.handoff.waiting(), "{source}");
                    assert!(pane.has_status_activity());
                    assert!(pane.suppress_streaming_after_abort());
                    assert_eq!(
                        pane.streaming_state,
                        NeoismAgentStreamingState::Generating
                    );
                }
                updates(&mut pane, vec![status(0, 200)]);
                if cached {
                    restore(&mut pane);
                }
                assert!(pane.is_streaming());
                assert!(!pane.suppress_streaming_after_abort());
                updates(&mut pane, vec![AgentSessionUpdate::SessionIdle]);
                assert!(!pane.is_streaming());
            }
        }
    }
}

#[test]
fn projected_primary_never_labels_the_real_followup_on_any_desktop_queue_path() {
    for windows in [false, true] {
        for cached in [false, true] {
            for source in ["canonical", "status", "hydration"] {
                let mut pane = pane(windows);
                if cached {
                    pane.cache_current_session(false);
                    pane.session_id = Some("other".into());
                }
                match source {
                    "canonical" => {
                        feed(&mut pane, vec![queue("enqueue", 2, false, Some("hello"))])
                    }
                    "status" => updates(
                        &mut pane,
                        vec![AgentSessionUpdate::QueueStatus {
                            count: 2,
                            preview: Some("hello".into()),
                            started_at: None,
                        }],
                    ),
                    _ if cached => pane
                        .session_cache
                        .get_mut("root")
                        .unwrap()
                        .runtime
                        .apply_queue_status(2, Some("hello".into()), None),
                    _ => pane.apply_runtime_status_for_session(
                        "root",
                        &HashMap::from([(
                            "root".into(),
                            super::super::super::api::SessionStatusSnapshot {
                                kind: "busy".into(),
                                queue_count: 2,
                                preview: Some("hello".into()),
                                ..Default::default()
                            },
                        )]),
                    ),
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
                feed(&mut pane, vec![queue("dequeue", 1, true, Some("B"))]);
                if cached {
                    restore(&mut pane);
                }
                assert_eq!(pane.queued_prompt_count(), 1);
                assert_eq!(pane.queued_prompt_preview(), Some("B"));
            }
        }
    }
}

fn part(id: &str, parent: &str, tool: bool) -> AgentSessionUpdate {
    let mut message = if tool {
        NeoismAgentMessage::assistant("saved tool")
    } else {
        NeoismAgentMessage::reasoning("saved reasoning plus late")
    };
    message.id = id.into();
    if tool {
        message.kind = NeoismAgentMessageKind::Tool;
        message.title = "old tool".into();
    }
    AgentSessionUpdate::PartUpdated {
        message,
        parent_message_id: Some(parent.into()),
    }
}

fn late_old_parts() -> Vec<AgentSessionUpdate> {
    vec![
        part("old-reason", "old-assistant", false),
        part("old-new-tool", "old-assistant", true),
        AgentSessionUpdate::PartDelta {
            message_id: Some("old-assistant".into()),
            part_id: Some("old-new-reason".into()),
            kind: Some("reasoning".into()),
            delta: "late old delta".into(),
        },
    ]
}

#[test]
fn cancelled_assistant_identity_survives_dequeue_or_status_promotion_cache_and_history() {
    for windows in [false, true] {
        for cached in [false, true] {
            for by_status in [false, true] {
                let mut pane = pane(windows);
                feed(&mut pane, vec![queue("dequeue", 0, true, None)]);
                updates(
                    &mut pane,
                    vec![part("old-reason", "old-assistant", false), status(1, 100)],
                );
                pane.abort_session();
                if cached {
                    pane.cache_current_session(false);
                    pane.session_id = Some("other".into());
                }
                updates(&mut pane, late_old_parts());
                if cached {
                    assert_eq!(
                        pane.session_cache["root"].runtime.streaming_state,
                        NeoismAgentStreamingState::Generating
                    );
                } else {
                    assert_eq!(
                        pane.streaming_state,
                        NeoismAgentStreamingState::Generating
                    );
                }
                if by_status {
                    updates(&mut pane, vec![status(0, 200)]);
                } else {
                    updates(
                        &mut pane,
                        vec![
                            AgentSessionUpdate::DequeuedPrompt {
                                text: "B".into(),
                                message_id: Some("next".into()),
                                author: None,
                            },
                            AgentSessionUpdate::QueueSnapshot {
                                session_id: "root".into(),
                                count: Some(0),
                                preview: None,
                                running: Some(true),
                                action: Some("dequeue".into()),
                            },
                        ],
                    );
                }
                updates(&mut pane, late_old_parts());
                if cached {
                    assert_eq!(
                        pane.session_cache["root"].runtime.streaming_state,
                        NeoismAgentStreamingState::Generating
                    );
                    updates(&mut pane, vec![part("new-reason", "new-assistant", false)]);
                    assert_eq!(
                        pane.session_cache["root"].runtime.streaming_state,
                        NeoismAgentStreamingState::Thinking
                    );
                    updates(&mut pane, late_old_parts());
                    assert_eq!(
                        pane.session_cache["root"].runtime.streaming_state,
                        NeoismAgentStreamingState::Thinking
                    );
                    updates(
                        &mut pane,
                        vec![AgentSessionUpdate::PartDelta {
                            message_id: Some("new-assistant".into()),
                            part_id: Some("new-text".into()),
                            kind: Some("text".into()),
                            delta: "new output".into(),
                        }],
                    );
                    assert_eq!(
                        pane.session_cache["root"].runtime.streaming_state,
                        NeoismAgentStreamingState::Generating
                    );
                    restore(&mut pane);
                }
                assert!(!pane.suppress_streaming_after_abort());
                assert!(pane
                    .messages
                    .iter()
                    .all(|message| message.id != "old-new-tool"
                        && message.id != "old-new-reason"));
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
                assert!(pane.streaming_tool_label.is_none());
                updates(&mut pane, vec![part("new-reason", "new-assistant", false)]);
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Thinking);
                updates(&mut pane, late_old_parts());
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Thinking);
                updates(
                    &mut pane,
                    vec![AgentSessionUpdate::PartDelta {
                        message_id: Some("new-assistant".into()),
                        part_id: Some("new-text".into()),
                        kind: Some("text".into()),
                        delta: "new output".into(),
                    }],
                );
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
                updates(&mut pane, vec![part("new-tool", "new-assistant", true)]);
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Working);
                let saved = pane.messages.clone();
                updates(
                    &mut pane,
                    vec![AgentSessionUpdate::Messages {
                        messages: saved,
                        oldest_cursor: None,
                    }],
                );
                assert!(pane
                    .messages
                    .iter()
                    .any(|message| message.id == "old-reason"
                        && message.text.contains("saved reasoning")));
                assert!(!pane
                    .status_timing
                    .handoff
                    .stopped_parts
                    .allows("old-new-tool", None));
                assert!(pane
                    .status_timing
                    .handoff
                    .stopped_parts
                    .allows("new-reason", Some("new-assistant")));
                // An old saved row may be the tail after a history refresh;
                // refreshing it must not revive that assistant's activity.
                pane.messages.push(
                    NeoismAgentMessage::reasoning("saved old tail").with_id("old-reason"),
                );
                pane.note_streaming(NeoismAgentStreamingState::Generating, None);
                pane.refresh_streaming_from_tail();
                assert_eq!(pane.streaming_state, NeoismAgentStreamingState::Generating);
            }
        }
    }
}
