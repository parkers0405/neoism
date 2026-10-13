use super::footer_tests::{answer, extent_with_lines};
use super::*;
use crate::panels::agent_pane::{
    state::NeoismAgentStreamingState, tool_motion::ToolMotionSample,
};

fn sample(progress: f32) -> ToolMotionSample {
    ToolMotionSample {
        opacity: progress,
        deadline: Some(web_time::Instant::now() + std::time::Duration::from_millis(180)),
        ..Default::default()
    }
}

fn fixture(
    lines: usize,
    active: bool,
    history: bool,
    scale: f32,
) -> (NeoismAgentPane, [f32; 4], f32, f32) {
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![answer("")]);
    // Actual input island top, not an invented footer/card boundary.
    let input = crate::panels::agent_pane::view::layout::chat_input_rect(
        &pane,
        [0.0, 0.0, 600.0 * scale, 800.0 * scale],
        scale,
    );
    let rect = [input[0], 0.0, input[2], input[1]];
    let old_body = extent_with_lines(&pane, scale, lines);
    if active {
        pane.note_streaming(NeoismAgentStreamingState::Working, None);
    }
    let old = super::render::activity_geometry(
        rect,
        old_body,
        if active { 26.0 * scale } else { 0.0 },
        18.0 * scale,
        scale,
        true,
        0.0,
    );
    pane.set_timeline_metrics_with_live_growth_scaled(
        rect,
        old.content_h,
        rect[3],
        Some(0.0),
        scale,
    );
    if history {
        pane.apply_history(vec![answer("Build · Sol · 1s · 13.2 tok/s")]);
    } else {
        pane.upsert_part_message(answer("Build · Sol · 1s · 13.2 tok/s"));
    }
    pane.note_streaming(NeoismAgentStreamingState::Idle, None);
    let body = extent_with_lines(&pane, scale, lines);
    let new =
        super::render::activity_geometry(rect, body, 0.0, 18.0 * scale, scale, true, 0.0);
    pane.set_timeline_metrics_with_live_growth_scaled(
        rect,
        new.content_h,
        rect[3],
        Some(body - old_body),
        scale,
    );
    let scroll_top = (new.content_h - rect[3]).max(0.0) - pane.timeline_scroll_offset();
    let natural = rect[1] + body - 28.0 * scale + 8.0 * scale - scroll_top;
    (pane, rect, natural, new.content_h - old.content_h)
}

#[test]
fn response_footer_edge_reader_resize_and_optout_cancel_without_redocking() {
    for cancel in 0..3 {
        let (mut pane, rect, natural, _) = fixture(40, true, false, 1.0);
        match cancel {
            0 => {
                pane.scroll_timeline_pixels(80.0);
                assert!(!pane.timeline_follow_bottom());
            }
            1 => {
                let smaller = [rect[0], rect[1], rect[2], rect[3] - 100.0];
                let body = extent_with_lines(&pane, 1.0, 40);
                pane.set_timeline_metrics_with_live_growth_scaled(
                    smaller,
                    body,
                    smaller[3],
                    Some(0.0),
                    1.0,
                );
            }
            _ => {
                pane.set_text_reveal_enabled(false);
                pane.set_text_reveal_enabled(true);
            }
        }
        assert_eq!(
            pane.response_footer_paint_y("answer", natural, sample(0.0)),
            natural
        );
        if cancel == 0 {
            pane.scroll_timeline_pixels(-10_000.0);
            assert!(pane.timeline_follow_bottom());
            assert_eq!(
                pane.response_footer_paint_y("answer", natural, sample(0.0)),
                natural
            );
        }
    }
}

#[test]
fn response_footer_edge_history_echo_metadata_and_refinement_never_rearm() {
    let (mut pane, rect, natural, _) = fixture(40, true, true, 1.0);
    assert_eq!(
        pane.response_footer_paint_y("answer", natural, ToolMotionSample::default()),
        natural
    );
    pane.apply_history(pane.messages().to_vec());
    let mut metadata = pane.messages()[0].clone();
    metadata.status.push_str(" · updated metadata");
    pane.upsert_part_message(metadata);
    pane.set_timeline_metrics_with_live_growth_scaled(
        rect,
        extent_with_lines(&pane, 1.0, 40) + 10_000.0,
        rect[3],
        Some(0.0),
        1.0,
    );
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert_eq!(
        pane.response_footer_paint_y("answer", natural, sample(0.0)),
        natural
    );
    let mut loaded = NeoismAgentPane::default();
    loaded.apply_history(vec![answer("Build · Sol · 1s")]);
    loaded.set_timeline_metrics_with_live_growth_scaled(
        rect,
        extent_with_lines(&loaded, 1.0, 40),
        rect[3],
        Some(0.0),
        1.0,
    );
    assert_eq!(
        loaded.response_footer_paint_y("answer", natural, sample(0.0)),
        natural
    );
}

#[test]
fn response_footer_edge_hidden_first_frame_only_destination_ink_owns_redraw() {
    let (mut pane, rect, natural, _) = fixture(40, true, false, 1.0);
    let motion = sample(0.0);
    let painted = pane.response_footer_paint_y("answer", natural, motion);
    let destination = [rect[0] + 18.0, natural, 200.0, 12.0];
    let hidden = [destination[0], painted, destination[2], destination[3]];
    use super::super::tool_message::motion::active_visible;
    assert!(!active_visible(motion, hidden, rect, &[]));
    pane.tool_motion_state().begin_frame();
    assert!(active_visible(motion, destination, rect, &[]));
    pane.tool_motion_state().mark_visible(motion);
    assert!(pane.tool_motion_state().is_animating_for(None));
    pane.tool_motion_state().begin_frame();
    assert!(!active_visible(motion, destination, rect, &[rect]));
    assert!(!pane.tool_motion_state().is_animating_for(None));
    let expired = ToolMotionSample {
        deadline: Some(web_time::Instant::now() - std::time::Duration::from_millis(1)),
        ..motion
    };
    assert!(!active_visible(expired, destination, rect, &[]));
    assert_eq!(
        pane.response_footer_paint_y("answer", natural, expired),
        natural
    );
}

#[test]
fn response_footer_edge_activity_removal_emerges_from_real_composer_border() {
    for history in [false, true] {
        let (mut pane, rect, natural, delta) = fixture(40, true, history, 1.0);
        assert_eq!(delta, -34.0); // 62px activity reclaimed; fixed footer adds 28.
        assert_eq!(pane.timeline_scroll_offset(), 0.0); // no false growth credit
        let edge = rect[1] + rect[3];
        assert_eq!(natural, edge - 20.0);
        let first = pane.response_footer_paint_y("answer", natural, sample(0.0));
        assert_eq!(
            first, edge,
            "completed footer was already fully above composer on arrival"
        );
        let middle = pane.response_footer_paint_y("answer", natural, sample(0.875));
        assert!(natural < middle && middle < first);
        assert_eq!(
            pane.response_footer_paint_y("answer", natural, ToolMotionSample::default()),
            natural
        );
    }
}

#[test]
fn response_footer_edge_short_answer_rises_to_unchanged_footer_slot() {
    for scale in [1.0, 2.0] {
        let (mut pane, rect, natural, _) = fixture(2, false, false, scale);
        let edge = rect[1] + rect[3];
        assert!(natural < edge - 28.0 * scale);
        assert!(!pane.timeline_is_inertial()); // fits, so no follow spring is needed
        assert_eq!(
            pane.response_footer_paint_y("answer", natural, sample(0.0)),
            edge
        );
        assert_eq!(
            pane.response_footer_paint_y("answer", natural, sample(0.5)),
            (edge + natural) * 0.5
        );
        assert_eq!(
            pane.response_footer_paint_y("answer", natural, ToolMotionSample::default()),
            natural
        );
    }
}
