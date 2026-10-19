use super::*;

fn text_delta(text: &str) -> AgentSessionUpdate {
    AgentSessionUpdate::PartDelta {
        message_id: Some("msg-a".into()),
        part_id: Some("part-t".into()),
        kind: Some("text".into()),
        delta: text.into(),
    }
}

fn snapshot(text: &str) -> AgentSessionUpdate {
    AgentSessionUpdate::PartUpdated {
        message: NeoismAgentMessage::assistant(text).with_id("part-t"),
        parent_message_id: Some("msg-a".into()),
    }
}

fn drain(
    pane: &mut NeoismAgentPane,
    updates: impl IntoIterator<Item = AgentSessionUpdate>,
) {
    pane.event_stream = Some(AgentSessionEventStream::with_updates_for_test(
        "root", updates,
    ));
    pane.drain_live_session_updates();
}

#[test]
fn late_empty_stream_events_preserve_text_and_successor_deltas_without_retry() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    drain(&mut pane, [text_delta("streamed text"), snapshot("")]);
    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.messages[0].id, "part-t");
    assert_eq!(pane.messages[0].text, "streamed text");
    drain(
        &mut pane,
        [snapshot(""), text_delta(" continues"), snapshot("")],
    );
    assert_eq!(pane.messages[0].text, "streamed text continues");
    assert!(!pane.retry_reset_pending);
}

#[test]
fn late_empty_actual_retry_event_clears_once_then_preserves_successor_stream() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    drain(&mut pane, [text_delta("partial reply ")]);
    drain(
        &mut pane,
        [AgentSessionUpdate::Retrying {
            attempt: 1,
            message: Some("overloaded".into()),
        }],
    );
    assert!(pane.retry_reset_pending);
    assert_eq!(pane.messages[0].text, "partial reply ");
    drain(&mut pane, [snapshot("")]);
    assert_eq!(pane.messages[0].text, "");
    assert!(!pane.retry_reset_pending);
    drain(
        &mut pane,
        [
            text_delta("fresh reply"),
            snapshot(""),
            text_delta(" continues"),
        ],
    );
    assert_eq!(pane.messages.len(), 1);
    assert_eq!(pane.messages[0].text, "fresh reply continues");
    assert!(!pane.retry_reset_pending);
}

#[test]
fn late_empty_guard_keeps_nonempty_shrink_and_correction_authoritative() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    drain(
        &mut pane,
        [text_delta("long streamed answer"), snapshot("")],
    );
    drain(&mut pane, [snapshot("long")]);
    assert_eq!(pane.messages[0].text, "long");
    drain(
        &mut pane,
        [snapshot("corrected"), text_delta(" answer"), snapshot("")],
    );
    assert_eq!(pane.messages[0].text, "corrected answer");
}

#[test]
fn late_empty_unknown_part_does_not_consume_pending_retry_reset() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_part_delta(
        Some("msg-a".into()),
        Some("part-t".into()),
        Some("text".into()),
        "partial reply",
    );
    pane.retry_reset_pending = true;
    pane.upsert_part_message(NeoismAgentMessage::assistant("").with_id("other-part"));
    assert!(pane.retry_reset_pending);
    assert_eq!(pane.messages[0].text, "partial reply");
    pane.upsert_part_message(NeoismAgentMessage::assistant("").with_id("part-t"));
    assert!(!pane.retry_reset_pending);
    assert_eq!(pane.messages[0].text, "");
}
