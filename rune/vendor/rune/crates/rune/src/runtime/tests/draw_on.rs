use super::*;
use crate::{Shape, canvas};

/// A 100-point line in 10 segments, drawn on when `.0` mounts it.
fn line_view(s: &(bool, Transition), _: &mut Cx<(bool, Transition)>) -> El {
    let line: Vec<[f32; 2]> = (0..=10).map(|i| [i as f32 * 10.0, 20.0]).collect();
    col().size(200.0, 100.0).children(s.0.then(|| canvas(Shape::new().stroke(line, 2.0, crate::Color::WHITE)).key("line").size(120.0, 40.0).transition(s.1)))
}

fn segments(rt: &Runtime<(bool, Transition)>) -> usize {
    rt.instances.iter().filter(|q| q.params[1] == KIND_SEGMENT).count()
}

#[test]
fn a_draw_transition_traces_a_shape_on_and_reduce_motion_draws_it_whole() {
    let mut rt = Runtime::new((false, Transition::draw()), line_view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = true);
    rt.frame(0.0);
    let mut seen = vec![segments(&rt)];
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
        seen.push(segments(&rt));
    }
    assert!(seen[0] < 3, "starts undrawn: {seen:?}");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "draws on, never back: {seen:?}");
    assert!(seen.iter().any(|&n| n > 3 && n < 10), "passes through partly drawn: {seen:?}");
    assert_eq!(*seen.last().unwrap(), 10, "ends whole: {seen:?}");

    let mut rt = Runtime::new((false, Transition::draw()), line_view, (200.0, 100.0), Env::new(true, 1.0));
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    rt.update(|s, _| s.0 = true);
    rt.frame(0.0);
    assert_eq!(segments(&rt), 10, "drawn whole the frame it mounts under Reduce Motion");
}

#[test]
fn a_live_trim_cuts_a_shape_every_frame_without_rebuilding() {
    type S = crate::Tween<[f32; 2]>;
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let line: Vec<[f32; 2]> = (0..=10).map(|i| [i as f32 * 10.0, 20.0]).collect();
        col().size(200.0, 100.0).child(canvas(Shape::new().stroke(line, 2.0, crate::Color::WHITE)).key("line").size(120.0, 40.0).live_trim(s))
    }
    let count = |rt: &Runtime<S>| rt.instances.iter().filter(|q| q.params[1] == KIND_SEGMENT).count();
    let tween = crate::Tween::linear([0.0, 1.0], [1.0, 1.0], std::time::Duration::from_secs(1));
    let mut rt = Runtime::new(tween, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(count(&rt), 10, "starts whole");
    let commits = rt.stats.commits;
    let mut seen = Vec::new();
    for _ in 0..10 {
        assert!(rt.frame(0.1 - 1e-4), "keeps drawing while it runs");
        seen.push(count(&rt));
    }
    assert!(seen.windows(2).all(|w| w[0] >= w[1]), "only shortens: {seen:?}");
    assert!(seen.contains(&5), "half gone at half time: {seen:?}");
    rt.frame(0.01);
    assert_eq!(count(&rt), 0, "gone at the end");
    assert!(!rt.frame(0.1), "settles");
    assert_eq!(rt.stats.commits, commits, "no view rebuild");

    rt.update(|s, _| *s = crate::Tween::linear([0.25, 0.5], [0.25, 0.5], std::time::Duration::ZERO));
    rt.frame(0.0);
    assert_eq!(count(&rt), 3, "a fixed span keeps its stretch, cut mid-segment at both ends");
}
