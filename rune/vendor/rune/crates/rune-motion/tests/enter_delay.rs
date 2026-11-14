//! A delayed entrance (`Scene::insert_delayed`) holds the whole insertion effect, then plays
//! every part of it on one presence progress.
use rune_motion::prelude::*;
use rune_motion::Phase;

const HZ: f64 = 120.0;
const RISE: f32 = 12.0;

fn spec() -> NodeSpec {
    NodeSpec::new(Rect::new(0.0, 0.0, 100.0, 40.0)).transition(Transition::offset(0.0, RISE).combined(Transition::opacity()))
}

fn scene() -> Scene {
    let mut s = Scene::new();
    s.set_pixel_scale(8.0);
    s.frame_at(0.0);
    s
}

/// (time, offset progress, opacity) of `id` from `from` to `until`.
fn trace(s: &mut Scene, from: f64, until: f64, id: NodeId) -> Vec<(f64, f32, f32)> {
    let mut out = Vec::new();
    let mut t = from;
    while t <= until + 1e-9 {
        s.frame_at(t);
        let p = s.presented(id).unwrap();
        out.push((t, 1.0 - p.rect.y / RISE, p.opacity));
        t += 1.0 / HZ;
    }
    out
}

#[test]
fn holds_the_insertion_effect_then_moves_every_part_together() {
    let mut s = scene();
    let id = NodeId(1);
    s.insert_delayed(id, spec(), Mount::Animated, 0.3);
    for (t, offset, opacity) in trace(&mut s, 0.0, 0.29, id) {
        assert_eq!((offset, opacity), (0.0, 0.0), "t={t}: waits at the insertion effect");
    }
    let after = trace(&mut s, 0.3, 1.5, id);
    assert!(after.iter().any(|&(_, o, _)| o > 0.5), "it enters");
    for (t, offset, opacity) in after {
        assert!((offset - opacity).abs() < 2e-3, "t={t}: offset {offset} and opacity {opacity} share one progress");
    }
    assert_eq!(s.presented(id).unwrap().phase, Phase::Present);
}

#[test]
fn only_waiting_isnt_running() {
    let mut s = scene();
    s.insert_delayed(NodeId(1), spec(), Mount::Animated, 0.3);
    s.frame_at(0.1);
    assert!((s.store().waiting_until().unwrap() - 0.3).abs() < 1e-6);
    s.frame_at(0.31);
    assert_eq!(s.store().waiting_until(), None, "the entrance moves");
}

#[test]
fn a_node_clock_stretches_the_delay() {
    let mut s = scene();
    let id = NodeId(1);
    s.insert_delayed(id, spec(), Mount::Animated, 0.3);
    s.set_time_scale(id, 0.5);
    s.frame_at(0.1);
    assert!((s.store().waiting_until().unwrap() - 0.6).abs() < 1e-6, "0.3 s at half speed");
    s.frame_at(0.59);
    assert_eq!(s.presented(id).unwrap().opacity, 0.0);
    s.frame_at(0.7);
    assert!(s.presented(id).unwrap().opacity > 0.0);
}

#[test]
fn removed_while_waiting_unseen_goes_at_once() {
    let mut s = scene();
    let id = NodeId(1);
    s.insert_delayed(id, spec(), Mount::Animated, 0.3);
    s.frame_at(0.1);
    s.remove(id);
    assert!(s.frame_at(0.11).exited.contains(&id), "nothing showed, so there's no exit to play");
    assert!(!s.frame_at(0.5).animating, "the cancelled start never runs");
}

#[test]
fn a_visible_insertion_effect_fades_out_from_where_it_waits() {
    let mut s = scene();
    let id = NodeId(1);
    let slide = NodeSpec::new(Rect::new(0.0, 0.0, 100.0, 40.0)).transition(Transition::offset(0.0, RISE));
    s.insert_delayed(id, slide, Mount::Animated, 0.3);
    s.frame_at(0.1);
    assert_eq!((s.presented(id).unwrap().rect.y, s.presented(id).unwrap().opacity), (RISE, 1.0));
    s.remove(id);
    let mut last = 1.0;
    for (t, offset, opacity) in trace(&mut s, 0.1, 0.25, id) {
        assert_eq!(offset, 0.0, "t={t}: it stays where it waited");
        assert!(opacity <= last, "t={t}: fades out, {opacity} after {last}");
        last = opacity;
    }
    assert!(last < 1.0);
}

#[test]
fn entering_now_lands_a_waiting_entrance() {
    let mut s = scene();
    let id = NodeId(1);
    s.insert_delayed(id, spec(), Mount::Animated, 0.3);
    s.frame_at(0.1);
    s.land_waiting(id);
    let out = s.frame_at(0.11);
    assert!(!out.animating);
    let p = s.presented(id).unwrap();
    assert_eq!((p.rect.y, p.opacity, p.phase), (0.0, 1.0, Phase::Present));
}

#[test]
fn reduce_motion_keeps_the_delay_and_only_fades() {
    let mut s = scene();
    s.set_reduced_motion(true);
    let id = NodeId(1);
    s.insert_delayed(id, spec(), Mount::Animated, 0.3);
    let steps = trace(&mut s, 0.0, 1.0, id);
    for &(t, offset, opacity) in &steps {
        assert_eq!(offset, 1.0, "t={t}: nothing travels");
        if t < 0.3 - 1e-9 {
            assert_eq!(opacity, 0.0, "t={t}: the fade waits");
        }
    }
    let shown = steps.iter().find(|s| s.2 >= 1.0).unwrap().0;
    assert!(shown > 0.3 && shown < 0.6, "the short fade, after the delay: {shown}");
}

#[test]
fn a_staggered_list_of_five_enters_in_order() {
    let mut s = scene();
    let stagger = Stagger::new(0.05);
    for i in 0..5 {
        s.insert_delayed(NodeId(i + 1), spec(), Mount::Animated, stagger.delay(i as usize, 5));
    }
    let mut starts = [None; 5];
    let mut t = 0.0;
    while t < 1.5 {
        s.frame_at(t);
        for i in 0..5 {
            let p = s.presented(NodeId(i as u64 + 1)).unwrap();
            let (offset, opacity) = (1.0 - p.rect.y / RISE, p.opacity);
            assert!((offset - opacity).abs() < 2e-3, "row {i} t={t}: {offset} vs {opacity}");
            if opacity > 0.0 && starts[i].is_none() {
                starts[i] = Some(t);
            }
        }
        t += 1.0 / HZ;
    }
    let starts = starts.map(Option::unwrap);
    for (i, w) in starts.windows(2).enumerate() {
        assert!((w[1] - w[0] - 0.05).abs() < 1.5 / HZ, "row {} starts 50 ms after row {i}: {starts:?}", i + 1);
    }
}
