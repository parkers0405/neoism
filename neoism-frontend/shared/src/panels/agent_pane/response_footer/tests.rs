use super::*;
use crate::panels::agent_pane::text_reveal::{self, Projection};
use std::rc::Rc;

fn paint(state: &mut TextRevealState, id: &str, text: &str) -> text_reveal::PaintScope {
    let key = key(id);
    for update in state.pending(&key) {
        let mut p = Projection::default();
        p.push(&update.text, &update.text);
        state.apply_projection(&key, &p, update.at);
    }
    let mut p = Projection::default();
    p.push(text, text);
    state.paint(&key, 0, Rc::new(p))
}

#[test]
fn response_footer_birth_reveals_both_segments_without_body_replay() {
    let mut state = TextRevealState::default();
    let mut motion = ToolMotionState::default();
    let footer = "Büild · GPT-6.1 Sol · 16m 38s · 13.2 tok/s";
    record(&mut state, &mut motion, "answer", "", footer);
    assert!(!state.has_body("answer"));
    assert!(motion.sample_group([key("answer").as_str()]).opacity < 1.0);
    let _paint = paint(&mut state, "answer", footer);
    let (agent, rest) = segments(footer);
    let (rest, offset) = rest.unwrap();
    text_reveal::begin_line(footer);
    assert!(!text_reveal::ranges(agent).is_empty());
    text_reveal::begin_line_at(footer, offset);
    assert!(!text_reveal::ranges(rest).is_empty());
    assert!(!state.is_animating()); // ingestion/paint scopes alone own no redraw
}

#[test]
fn response_footer_metadata_reveals_only_changed_ink_and_does_not_reenter() {
    let mut state = TextRevealState::default();
    let mut motion = ToolMotionState::default();
    let old = "Büild · Sol · 13.2 tok/s";
    let new = "Büild · Sol · 14.2 tok/s";
    record(&mut state, &mut motion, "answer", old, new);
    assert!(motion
        .sample_group([key("answer").as_str()])
        .deadline
        .is_none());
    let _paint = paint(&mut state, "answer", new);
    text_reveal::begin_line(new);
    assert!(text_reveal::ranges("Büild").is_empty());
    let (_, rest) = segments(new);
    let (rest, offset) = rest.unwrap();
    text_reveal::begin_line_at(new, offset);
    let ranges = text_reveal::ranges(rest);
    assert_eq!(ranges.len(), 1);
    assert_eq!(&rest[ranges[0].range.clone()], "4");
}

#[test]
fn response_footer_history_baseline_and_identical_echo_do_not_replay() {
    let mut state = TextRevealState::default();
    let mut motion = ToolMotionState::default();
    let footer = "Build · Sol";
    record_refresh(
        &mut state,
        &mut motion,
        [].into_iter(),
        [("answer", footer)].into_iter(),
    );
    assert!(!state.has_body(&key("answer")));
    record(&mut state, &mut motion, "answer", "", footer);
    let before = state.pending(&key("answer"));
    let deadline = motion.sample_group([key("answer").as_str()]).deadline;
    record(&mut state, &mut motion, "answer", footer, footer);
    assert!(state.pending(&key("answer")).is_empty());
    assert!(before.last().unwrap().at.is_some());
    assert_eq!(
        motion.sample_group([key("answer").as_str()]).deadline,
        deadline
    );
    state.reconcile_history_with_footers([("answer", "prose", Some(footer))].into_iter());
    assert!(state.has_body(&key("answer")));
}

#[test]
fn response_footer_segments_preserve_whitespace_utf8_and_separator() {
    let footer = "Büild α · GPT-6.1 Sol · 16m 38s · 13.2 tok/s";
    let (agent, rest) = segments(footer);
    let (rest, offset) = rest.unwrap();
    assert_eq!(agent, "Büild α");
    assert_eq!(offset, "Büildα".len());
    assert_eq!(format!("{agent}{rest}"), footer);
    assert_eq!(segments("Build"), ("Build", None));
}

#[test]
fn response_footer_offscreen_ages_expire_without_paint_birth() {
    let mut state = TextRevealState::default();
    let mut motion = ToolMotionState::default();
    let footer = "Build · Sol";
    record(&mut state, &mut motion, "answer", "", footer);
    let key = key("answer");
    for update in state.pending(&key) {
        let mut p = Projection::default();
        p.push(&update.text, &update.text);
        state.apply_projection(
            &key,
            &p,
            update.at.map(|at| at - std::time::Duration::from_secs(1)),
        );
    }
    state.reconcile_history_with_footers([("answer", "prose", Some(footer))].into_iter());
    record(&mut state, &mut motion, "answer", footer, footer);
    let _paint = paint(&mut state, "answer", footer);
    text_reveal::begin_line(footer);
    assert!(text_reveal::ranges(footer).is_empty());
    assert!(!state.is_animating());
}

#[test]
fn composer_edge_footer_first_paint_uses_original_birth_progress() {
    use crate::panels::agent_pane::tool_motion::ToolMotionSample;
    let rect = Some([0.0, 0.0, 400.0, 300.0]);
    let mut growth = Growth::default();
    growth.record_with_edge(["live".into()], true, rect);
    let sample = ToolMotionSample {
        opacity: 0.4,
        deadline: Some(web_time::Instant::now() + std::time::Duration::from_millis(100)),
        ..Default::default()
    };
    assert_eq!(growth.paint_y("live", 200.0, rect, true, sample), 260.0);
    growth.record_with_edge(["live".into()], true, rect); // identical eligibility does not reset anchor
    assert!(
        (growth.paint_y(
            "live",
            200.0,
            rect,
            true,
            ToolMotionSample {
                opacity: 0.7,
                ..sample
            }
        ) - 230.0)
            .abs()
            < 0.001
    );
    assert_eq!(
        growth.paint_y("live", 200.0, rect, true, ToolMotionSample::default()),
        200.0
    );
}

#[test]
fn composer_edge_footer_late_first_paint_and_expired_next_frame_are_continuous() {
    use crate::panels::agent_pane::tool_motion::ToolMotionSample;
    // 170–179ms first paint, then a 16ms later frame: the original 180ms
    // deadline has passed. Both a near-bar footer and the short-answer
    // fixture's 454px gap must be almost landed before that expired frame.
    for age_ms in [170.0_f32, 179.0] {
        for distance in [20.0_f32, 454.0] {
            let rect = Some([0.0, 0.0, 400.0, 680.0]);
            let natural = 680.0 - distance;
            let mut growth = Growth::default();
            growth.record_with_edge(["late".into()], true, rect);
            let sample = ToolMotionSample {
                opacity: 1.0 - (1.0 - age_ms / 180.0).powi(3),
                deadline: Some(
                    web_time::Instant::now()
                        + std::time::Duration::from_millis((180.0 - age_ms) as u64),
                ),
                ..Default::default()
            };
            let first = growth.paint_y("late", natural, rect, true, sample);
            assert!(
                first >= natural && first - natural < 0.1,
                "age={age_ms}, distance={distance}, first={first}, natural={natural}"
            );
            let expired = ToolMotionSample {
                opacity: 1.0,
                deadline: Some(
                    web_time::Instant::now() - std::time::Duration::from_millis(1),
                ),
                ..sample
            };
            let next = growth.paint_y("late", natural, rect, true, expired);
            assert_eq!(next, natural);
            assert!((next - first).abs() < 0.1);
            // Even a subsequent active-looking sample cannot revive eligibility.
            assert_eq!(growth.paint_y("late", natural, rect, true, sample), natural);
        }
    }
}

#[test]
fn response_footer_edge_birth_eligibility_and_offscreen_expiry_never_replay() {
    use crate::panels::agent_pane::tool_motion::ToolMotionSample;
    let rect = Some([0.0, 0.0, 400.0, 300.0]);
    let mut growth = Growth::default();
    let sample = ToolMotionSample {
        opacity: 0.0,
        deadline: Some(web_time::Instant::now() + std::time::Duration::from_millis(180)),
        ..Default::default()
    };
    growth.record_with_edge(["reader".into()], false, rect);
    growth.take(1.0, Some(28.0));
    assert_eq!(growth.paint_y("reader", 280.0, rect, true, sample), 280.0);
    growth.record_with_edge(["live".into()], true, rect);
    growth.take(1.0, Some(28.0));
    assert_eq!(growth.paint_y("live", 280.0, rect, true, sample), 300.0);
    let expired = ToolMotionSample {
        deadline: Some(web_time::Instant::now() - std::time::Duration::from_millis(1)),
        ..sample
    };
    growth.record_with_edge(["offscreen".into()], true, rect);
    assert_eq!(
        growth.paint_y("offscreen", 280.0, rect, true, expired),
        280.0
    );
    assert_eq!(
        growth.paint_y("offscreen", 280.0, rect, true, sample),
        280.0
    );
    for i in 0..300 {
        growth.record_with_edge([format!("unpainted-{i}")], true, rect);
    }
    assert!(growth.edges.len() <= 256);
    growth.clear();
    assert!(growth.edges.is_empty());
}

#[test]
fn response_footer_opt_out_is_immediate() {
    let mut state = TextRevealState::default();
    let mut motion = ToolMotionState::default();
    state.set_enabled(false);
    motion.set_enabled(false);
    record(&mut state, &mut motion, "answer", "", "Build · Sol");
    assert!(!state.has_body(&key("answer")));
    assert!(motion
        .sample_group([key("answer").as_str()])
        .deadline
        .is_none());
}
