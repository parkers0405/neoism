use super::*;
use crate::panels::agent_pane::view::markdown::{
    AgentMarkdownPane, AssistantMarkdownBlock,
};

// Headless exact-height fixture: the cached-artifact production path measures
// the same markdown blocks and fixed response slot without a GPU/font device.
pub(super) fn answer(footer: &str) -> NeoismAgentMessage {
    let mut row = NeoismAgentMessage::assistant(
        "Completed answer\n```neoism-html\n<div>answer</div>\n```",
    )
    .with_id("answer");
    row.status = footer.into();
    row
}

pub(super) fn extent(pane: &NeoismAgentPane, scale: f32) -> f32 {
    extent_with_lines(pane, scale, 40)
}

pub(super) fn extent_with_lines(pane: &NeoismAgentPane, scale: f32, lines: usize) -> f32 {
    let row = &pane.messages()[0];
    let blocks = std::rc::Rc::new(vec![
        AssistantMarkdownBlock::Paragraph(vec!["Answer line".into(); lines]),
        AssistantMarkdownBlock::HtmlArtifact {
            source: "<div>answer</div>".into(),
            copy_target: "fixture".into(),
        },
    ]);
    pane.store_markdown_blocks_for(
        &row.text,
        400.0 * scale - 48.0 * scale,
        scale,
        blocks,
    );
    let body =
        super::layout::estimate_message_height_with_pane(pane, row, 400.0 * scale, scale);
    super::render::activity_geometry(
        [0.0, 0.0, 400.0 * scale, 300.0 * scale],
        body,
        0.0,
        18.0 * scale,
        scale,
        false,
        0.0,
    )
    .content_h
}

fn rendered_top(pane: &NeoismAgentPane, height: f32, viewport: f32) -> f32 {
    // Production draw-loop transform before any spring tick (row.top = 0).
    -((height - viewport).max(0.0) - pane.timeline_scroll_offset())
}

#[test]
fn response_footer_follow_first_frame_completion_has_no_answer_yank() {
    for history in [false, true] {
        for scale in [1.0, 1.5, 2.0] {
            let rect = [0.0, 0.0, 400.0 * scale, 300.0 * scale];
            let mut pane = NeoismAgentPane::default();
            pane.apply_history(vec![answer("")]);
            let old = extent(&pane, scale);
            AgentTimelinePane::set_timeline_metrics(
                &mut pane, rect, old, rect[3], 0.0, scale,
            );
            let top = rendered_top(&pane, old, rect[3]);
            if history {
                pane.apply_history(vec![answer(
                    "Build · GPT-6.1 Sol · 16m 38s · 13.2 tok/s",
                )]);
            } else {
                pane.upsert_part_message(answer(
                    "Build · GPT-6.1 Sol · 16m 38s · 13.2 tok/s",
                ));
            }
            let new = extent(&pane, scale);
            assert_eq!(new - old, 28.0 * scale);
            AgentTimelinePane::set_timeline_metrics(
                &mut pane,
                rect,
                new,
                rect[3],
                new - old,
                scale,
            );
            assert_eq!(
                rendered_top(&pane, new, rect[3]),
                top,
                "footer shifted answer before the first spring tick (history={history})"
            );
            assert_eq!(pane.timeline_scroll_offset(), 28.0 * scale);
            assert!(pane.timeline_is_inertial());
        }
    }
}

#[test]
fn response_footer_follow_credit_never_charges_unrelated_refinement() {
    let rect = [0.0, 0.0, 400.0, 300.0];
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![answer("")]);
    let old = extent(&pane, 1.0);
    pane.set_timeline_metrics_with_live_growth(rect, old, 300.0, Some(0.0));
    pane.apply_history(vec![answer("Build · Sol")]);
    let new = extent(&pane, 1.0);
    // A cache correction can occur even in the measured tail during this pass.
    pane.set_timeline_metrics_with_live_growth(
        rect,
        new + 10_000.0,
        300.0,
        Some(10_028.0),
    );
    assert_eq!(pane.timeline_scroll_offset(), 28.0);
    pane.set_timeline_metrics_with_live_growth(rect, new + 20_000.0, 300.0, Some(0.0));
    assert_eq!(pane.timeline_scroll_offset(), 28.0);
    pane.apply_history(vec![answer("Build · Sol")]);
    pane.upsert_part_message(answer("Build · Sol · 13.2 tok/s"));
    pane.set_timeline_metrics_with_live_growth(
        rect,
        new + 30_000.0,
        300.0,
        Some(10_000.0),
    );
    assert_eq!(pane.timeline_scroll_offset(), 28.0);
    pane.apply_part_delta(Some("answer".into()), None, None, " next real body delta");
    pane.set_timeline_metrics_with_live_growth(rect, new + 30_024.0, 300.0, Some(24.0));
    assert_eq!(pane.timeline_scroll_offset(), 52.0);
}

#[test]
fn response_footer_follow_manual_reader_and_opt_out_keep_their_positions() {
    for disabled in [false, true] {
        let rect = [0.0, 0.0, 400.0, 300.0];
        let mut pane = NeoismAgentPane::default();
        pane.apply_history(vec![answer("")]);
        let old = extent(&pane, 1.0);
        pane.set_timeline_metrics_with_live_growth(rect, old, 300.0, Some(0.0));
        if disabled {
            pane.set_text_reveal_enabled(false);
        } else {
            pane.scroll_timeline_pixels(200.0);
        }
        let top = rendered_top(&pane, old, 300.0);
        let was_inertial = pane.timeline_is_inertial();
        pane.upsert_part_message(answer("Build · Sol"));
        let new = extent(&pane, 1.0);
        pane.set_timeline_metrics_with_live_growth(rect, new, 300.0, Some(new - old));
        assert_eq!(pane.timeline_is_inertial(), was_inertial);
        if disabled {
            assert_eq!(pane.timeline_scroll_offset(), 0.0);
            assert_eq!(rendered_top(&pane, new, 300.0), top - 28.0);
        } else {
            assert_eq!(rendered_top(&pane, new, 300.0), top);
            assert!(!pane.timeline_follow_bottom());
        }
        // Neither path leaves credit for a later geometry-only pass.
        pane.set_text_reveal_enabled(true);
        pane.set_timeline_metrics_with_live_growth(
            rect,
            new + 1000.0,
            300.0,
            Some(1000.0),
        );
        assert_eq!(pane.timeline_is_inertial(), was_inertial);
    }
}

#[test]
fn response_footer_follow_loaded_and_partial_newest_history_are_not_live() {
    let rect = [0.0, 0.0, 400.0, 300.0];
    let mut pane = NeoismAgentPane::default();
    pane.apply_history(vec![answer("Build · Sol")]);
    let height = extent(&pane, 1.0);
    pane.set_timeline_metrics_with_live_growth(rect, height, 300.0, Some(height));
    assert!(!pane.timeline_is_inertial());
    // A newest-page fetch replacing a larger transcript is history, even when
    // an existing row's footer is new in that same snapshot.
    pane.apply_history(vec![
        answer(""),
        NeoismAgentMessage::assistant("cached older row").with_id("cached"),
    ]);
    pane.set_timeline_metrics_with_live_growth(rect, height + 1000.0, 300.0, Some(0.0));
    pane.apply_history_page(vec![answer("Build · Sol")], Some("next-page".into()));
    pane.set_timeline_metrics_with_live_growth(
        rect,
        height + 2000.0,
        300.0,
        Some(1000.0),
    );
    assert_eq!(pane.timeline_scroll_offset(), 0.0);
    assert!(!pane.timeline_is_inertial());
}
