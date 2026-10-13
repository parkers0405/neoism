use super::*;
use crate::panels::agent_pane::response_footer;

fn answer(footer: &str) -> NeoismAgentMessage {
    let mut row = NeoismAgentMessage::assistant("already streamed answer");
    row.id = "answer".into();
    row.status = footer.into();
    row
}

#[test]
fn response_footer_completion_refresh_has_real_birth_and_keeps_body_and_scroll_baseline()
{
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![answer("")]);
    pane.timeline_history_position_hydrated = true;
    pane.timeline_live_growth = false;
    let epoch = pane.timeline_layout_epoch;
    pane.apply_history(vec![answer("Build · GPT-6.1 Sol · 16m 38s · 13.2 tok/s")]);
    assert_eq!(pane.timeline_layout_epoch, epoch);
    assert!(!pane.timeline_live_growth);
    assert!(!pane.text_reveal.has_body("answer"));
    let key = response_footer::key("answer");
    let updates = pane.text_reveal.pending(&key);
    assert_eq!(updates.len(), 2);
    assert!(updates[0].at.is_none());
    assert!(updates[1].at.is_some());
    assert!(pane.tool_motion.sample_group([key.as_str()]).opacity < 1.0);
    pane.apply_history(vec![answer("Build · GPT-6.1 Sol · 16m 38s · 13.2 tok/s")]);
    assert!(pane.text_reveal.pending(&key).is_empty());
}

#[test]
fn response_footer_live_part_and_loaded_history_use_distinct_baselines() {
    let mut pane = NeoismAgentPane::default();
    let footer = "Build · Sol · 1s";
    pane.apply_history(vec![answer(footer)]);
    assert!(!pane.text_reveal.has_body(&response_footer::key("answer")));
    pane.upsert_part_message(answer("Build · Sol · 2s"));
    assert!(!pane.text_reveal.has_body("answer"));
    assert_eq!(
        pane.text_reveal
            .pending(&response_footer::key("answer"))
            .len(),
        2
    );
    assert!(pane
        .tool_motion
        .sample_group([response_footer::key("answer").as_str()])
        .deadline
        .is_none());
}

#[test]
fn response_footer_edge_existing_follow_lag_is_not_hard_docked() {
    use crate::panels::agent_pane::tool_motion::ToolMotionSample;
    let rect = [0.0, 0.0, 400.0, 300.0];
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![answer("")]);
    pane.set_timeline_metrics(rect, 900.0, 300.0);
    pane.apply_part_delta(Some("answer".into()), None, None, " live body growth");
    pane.set_timeline_metrics_with_live_growth(rect, 1100.0, 300.0, Some(200.0));
    let old_top = 1100.0 - 300.0 - pane.timeline_scroll_offset();
    let mut completed = pane.messages[0].clone();
    completed.status = "Build · Sol".into();
    pane.upsert_part_message(completed);
    pane.set_timeline_metrics_with_live_growth(rect, 1128.0, 300.0, Some(28.0));
    assert_eq!(1128.0 - 300.0 - pane.timeline_scroll_offset(), old_top);
    let sample = ToolMotionSample {
        opacity: 0.0,
        deadline: Some(Instant::now() + std::time::Duration::from_millis(180)),
        ..Default::default()
    };
    let natural = 280.0 + pane.timeline_scroll_offset();
    assert!(natural > 300.0);
    assert_eq!(
        pane.response_footer_paint_y("answer", natural, sample),
        natural
    );
    for _ in 0..120 {
        if pane.timeline_scroll_offset() < 20.0 {
            break;
        }
        pane.step_owned_timeline_spring(1.0 / 60.0);
    }
    let natural = 280.0 + pane.timeline_scroll_offset();
    assert!(natural < 300.0);
    let middle = pane.response_footer_paint_y(
        "answer",
        natural,
        ToolMotionSample {
            opacity: 0.875,
            ..sample
        },
    );
    assert!(natural < middle && middle < 300.0);
    assert_eq!(
        pane.response_footer_paint_y("answer", natural, ToolMotionSample::default()),
        natural
    );
}

#[test]
fn response_footer_follow_next_unpainted_body_arrival_keeps_ordinary_growth() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![answer("")]);
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics(rect, 900.0, 300.0);
    pane.upsert_part_message(answer("Build · Sol"));
    // Both arrivals precede paint: the following genuine body delta must not
    // be capped to the earlier footer's 28px budget.
    pane.apply_part_delta(Some("answer".into()), None, None, " more real text");
    pane.set_timeline_metrics_with_live_growth(rect, 976.0, 300.0, Some(76.0));
    assert_eq!(pane.timeline_scroll_offset(), 76.0);
    assert_eq!(pane.footer_growth.take(1.0, None), 0.0);
}

#[test]
fn response_footer_follow_existing_spring_settles_and_credit_is_one_shot() {
    let rect = [0.0, 0.0, 400.0, 300.0];
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![answer("")]);
    pane.set_timeline_metrics(rect, 900.0, 300.0);
    // The preceding body's live flag can remain latched after its measurement.
    pane.timeline_live_growth = true;
    pane.upsert_part_message(answer("Build · Sol"));
    pane.upsert_part_message(answer("Build · Sol"));
    pane.set_timeline_metrics_with_live_growth(rect, 10_928.0, 300.0, Some(10_028.0));
    assert_eq!(pane.timeline_scroll_offset(), 28.0);
    assert_eq!(
        pane.timeline_scroll_owner,
        TimelineScrollOwner::FollowBottom
    );
    pane.step_owned_timeline_spring(1.0 / 60.0);
    let lag = pane.timeline_scroll_offset();
    let velocity = pane.timeline_velocity_px_s;
    let clock = pane.timeline_last_tick_at;
    assert!(lag > 0.0 && lag < 28.0);
    assert!(velocity < 0.0);
    pane.set_timeline_metrics_with_live_growth(rect, 20_928.0, 300.0, Some(0.0));
    assert_eq!(pane.timeline_scroll_offset(), lag);
    assert_eq!(pane.timeline_velocity_px_s, velocity);
    assert_eq!(pane.timeline_last_tick_at, clock);
    for _ in 0..120 {
        pane.step_owned_timeline_spring(1.0 / 60.0);
    }
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert!(!pane.timeline_is_inertial());
    pane.apply_part_delta(Some("answer".into()), None, None, " next body");
    pane.set_timeline_metrics_with_live_growth(rect, 20_952.0, 300.0, Some(24.0));
    assert_eq!(pane.timeline_scroll_offset(), 24.0);
}

#[test]
fn response_footer_follow_pagination_cancels_pending_credit() {
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![answer("")]);
    let rect = [0.0, 0.0, 400.0, 300.0];
    pane.set_timeline_metrics(rect, 900.0, 300.0);
    pane.upsert_part_message(answer("Build · Sol"));
    pane.timeline_history.loading_older = true;
    pane.apply_history_page(
        vec![NeoismAgentMessage::assistant("older page").with_id("older")],
        None,
    );
    pane.set_timeline_metrics_with_live_growth(rect, 1928.0, 300.0, Some(1028.0));
    assert!(!pane.timeline_is_inertial());
    assert_eq!(pane.footer_growth.take(1.0, None), 0.0);
}

#[test]
fn response_footer_disabled_completion_refresh_is_sharp() {
    let mut pane = NeoismAgentPane::default();
    pane.set_text_reveal_enabled(false);
    pane.apply_history(vec![answer("")]);
    pane.apply_history(vec![answer("Build · Sol")]);
    assert!(!pane.text_reveal.has_body(&response_footer::key("answer")));
    assert!(pane
        .tool_motion
        .sample_group([response_footer::key("answer").as_str()])
        .deadline
        .is_none());
}
