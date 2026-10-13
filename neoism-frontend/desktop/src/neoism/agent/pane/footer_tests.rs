use super::*;

#[test]
fn response_footer_native_edge_follows_composer_and_cancels_on_reader_gesture() {
    use neoism_ui::panels::agent_pane::tool_motion::ToolMotionSample;
    let mut pane = NeoismAgentPane::default();
    let mut row = NeoismAgentMessage::assistant("already streamed answer");
    row.id = "answer".into();
    pane.messages.push(row.clone());
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics_with_live_growth_scaled(rect, 962.0, 300.0, Some(0.0), 1.0);
    row.status = "Build · Sol".into();
    pane.upsert_part_message(row);
    pane.set_timeline_metrics_with_live_growth_scaled(
        rect,
        928.0,
        300.0,
        Some(28.0),
        1.0,
    );
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    let sample = ToolMotionSample {
        opacity: 0.0,
        deadline: Some(web_time::Instant::now() + std::time::Duration::from_millis(180)),
        ..Default::default()
    };
    assert_eq!(pane.response_footer_paint_y("answer", 280.0, sample), 300.0);
    assert_eq!(
        pane.response_footer_paint_y(
            "answer",
            280.0,
            ToolMotionSample {
                opacity: 0.5,
                ..sample
            }
        ),
        290.0
    );
    pane.scroll_timeline_pixels(80.0);
    assert_eq!(pane.response_footer_paint_y("answer", 280.0, sample), 280.0);
    pane.scroll_timeline_pixels(-10_000.0);
    assert_eq!(pane.response_footer_paint_y("answer", 280.0, sample), 280.0);
}

#[test]
fn response_footer_native_follow_is_bounded_scaled_and_one_shot() {
    let mut pane = NeoismAgentPane::default();
    let mut row = NeoismAgentMessage::assistant("already streamed answer");
    row.id = "answer".into();
    pane.messages.push(row.clone());
    let rect = [0.0, 0.0, 800.0, 600.0];
    pane.set_timeline_metrics_with_live_growth_scaled(
        rect,
        1800.0,
        600.0,
        Some(0.0),
        2.0,
    );
    pane.timeline_live_growth = true; // older body revision remains latched
    row.status = "Build · Sol".into();
    pane.upsert_part_message(row.clone());
    pane.upsert_part_message(row.clone());
    pane.set_timeline_metrics_with_live_growth_scaled(
        rect,
        11_856.0,
        600.0,
        Some(10_056.0),
        2.0,
    );
    assert_eq!(pane.timeline_scroll_offset(), 56.0);
    pane.step_owned_timeline_spring(1.0 / 60.0);
    let lag = pane.timeline_scroll_offset();
    assert!(lag > 0.0 && lag < 56.0);
    row.status.push_str(" · 13.2 tok/s");
    pane.upsert_part_message(row);
    pane.set_timeline_metrics_with_live_growth_scaled(
        rect,
        21_856.0,
        600.0,
        Some(0.0),
        2.0,
    );
    assert_eq!(pane.timeline_scroll_offset(), lag);
    pane.apply_part_delta(Some("answer".into()), None, None, " next body");
    pane.set_timeline_metrics_with_live_growth_scaled(
        rect,
        21_880.0,
        600.0,
        Some(24.0),
        2.0,
    );
    assert_eq!(pane.timeline_scroll_offset(), lag + 24.0);
}

#[test]
fn response_footer_native_completion_does_not_replay_answer_or_arm_scroll() {
    let mut pane = NeoismAgentPane::default();
    let mut row = NeoismAgentMessage::assistant("already streamed answer");
    row.id = "answer".into();
    pane.messages.push(row.clone());
    pane.timeline_live_growth = false;
    row.status = "Build · GPT-6.1 Sol · 16m 38s · 13.2 tok/s".into();
    pane.upsert_part_message(row.clone());
    let key = neoism_ui::panels::agent_pane::response_footer::key("answer");
    assert!(!pane.timeline_live_growth);
    assert!(!pane.text_reveal.has_body("answer"));
    assert_eq!(pane.text_reveal.pending(&key).len(), 2);
    let deadline = pane.tool_motion.sample_group([key.as_str()]).deadline;
    assert!(deadline.is_some());
    pane.upsert_part_message(row);
    assert!(pane.text_reveal.pending(&key).is_empty());
    assert_eq!(
        pane.tool_motion.sample_group([key.as_str()]).deadline,
        deadline
    );
}
