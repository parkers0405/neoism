use rune_motion::prelude::*;
use rune_motion::{Keyframes, MotionEvent, PropClass, Prop};

const X: Prop<f32> = Prop::custom(64, PropClass::Spatial);
const N: NodeId = NodeId(7);

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

fn state(s: &AnimationStore) -> (f32, f32) {
    (s.sample(N, X).unwrap(), s.sample_velocity(N, X).unwrap())
}

#[test]
fn randomized_retargets_are_c1_continuous() {
    for seed in 1..=8u64 {
        let mut rng = Rng(seed);
        let mut store = AnimationStore::new();
        store.set(N, X, 0.0);
        let mut t = 0.0f64;
        for step in 0..1500 {
            t += rng.range(0.001, 0.06) as f64;
            store.set_time(t);
            if step % 3 == 0 {
                store.tick();
                // The tick's presented value is the exact sample.
                let (x, v) = state(&store);
                assert!((store.get(N, X).unwrap() - x).abs() < 1e-3);
                assert!((store.velocity(N, X).unwrap() - v).abs() < 1e-2);
            }
            let (x0, v0) = state(&store);
            let target = rng.range(-500.0, 500.0);
            let kind = rng.next();
            let anim: Anim = if kind < 0.6 {
                Spring::new(rng.range(0.1, 0.8), rng.range(-0.6, 0.7)).into()
            } else if kind < 0.85 {
                Curve::tween(rng.range(0.05, 0.5), Easing::EASE_IN_OUT).into()
            } else if kind < 0.93 {
                Curve::Decay(Decay::IOS_FAST).into()
            } else {
                Spring::new(rng.range(0.1, 0.8), 0.0).additive()
            };
            store.animate(N, X, target, anim);
            let (x1, v1) = state(&store);
            let scale = 1.0 + x0.abs().max(v0.abs()) * 1e-5;
            assert!((x1 - x0).abs() < 2e-3 * scale, "seed {seed} step {step}: position jumped {x0} → {x1}");
            assert!(
                (v1 - v0).abs() < 2e-2 * scale.max(1.0 + v0.abs() * 1e-4),
                "seed {seed} step {step} ({anim:?}): velocity jumped {v0} → {v1}"
            );
        }
        // Everything settles exactly on the last target, and the track goes dormant.
        store.tick_at(t + 200.0);
        store.tick_at(t + 200.1);
        assert_eq!(store.active_len(), 0);
        assert_eq!(store.velocity(N, X), Some(0.0));
    }
}

#[test]
fn additive_springs_equal_merged_springs() {
    // Springs are linear ODEs: stacking additive layers on one spring is the same motion as
    // sample-and-restart (research 06 §2.6). Prove it numerically.
    let s = motion::REFLOW;
    let mut merged = AnimationStore::new();
    let mut added = AnimationStore::new();
    for st in [&mut merged, &mut added] {
        st.set(N, X, 0.0);
    }
    let mut rng = Rng(42);
    let mut t = 0.0;
    for _ in 0..6 {
        let target = rng.range(-300.0, 300.0);
        merged.set_time(t);
        added.set_time(t);
        merged.animate(N, X, target, s.merge());
        added.animate(N, X, target, s.additive());
        for k in 0..40 {
            let tt = t + k as f64 * 0.002;
            merged.set_time(tt);
            added.set_time(tt);
            let (a, b) = (state(&merged), state(&added));
            assert!((a.0 - b.0).abs() < 2e-3, "{a:?} vs {b:?}");
            assert!((a.1 - b.1).abs() < 5e-2, "{a:?} vs {b:?}");
        }
        t += rng.range(0.03, 0.2) as f64;
    }
}

#[test]
fn unchanged_target_keeps_the_trajectory() {
    let mut a = AnimationStore::new();
    let mut b = AnimationStore::new();
    for s in [&mut a, &mut b] {
        s.set(N, X, 0.0);
        s.animate(N, X, 100.0, motion::SETTLE);
    }
    b.set_time(0.1);
    b.animate(N, X, 100.0, motion::DRIFT); // same target: no restart, curve ignored
    for k in 1..60 {
        let t = k as f64 / 120.0;
        a.set_time(t);
        b.set_time(t);
        assert_eq!(state(&a), state(&b));
    }
}

#[test]
fn delays_hold_then_start_exactly_and_newer_animations_cancel_them() {
    let mut s = AnimationStore::new();
    s.set(N, X, 0.0);
    s.animate(N, X, 100.0, motion::SETTLE.delay(0.1));
    s.tick_at(0.05);
    assert_eq!(s.get(N, X), Some(0.0));
    s.tick_at(0.1 + 0.15);
    let expect = motion::SETTLE.sample(0.0, 0.0, 100.0, 0.15).0;
    assert!((s.get(N, X).unwrap() - expect).abs() < 1e-3);
    // A delayed retarget, then an immediate one: the delayed one never happens.
    s.animate(N, X, -100.0, motion::SETTLE.delay(0.2));
    s.set_time(0.3);
    s.animate(N, X, 50.0, motion::SETTLE);
    s.tick_at(10.0);
    assert_eq!(s.get(N, X), Some(50.0));
    assert_eq!(s.target(N, X), Some(50.0));
}

#[test]
fn set_is_immediate_and_first_animate_doesnt_animate() {
    let mut s = AnimationStore::new();
    s.animate(N, X, 40.0, motion::SETTLE); // first sight: set, no motion
    s.tick_at(0.0);
    assert_eq!(s.get(N, X), Some(40.0));
    assert!(!s.is_animating(N, X));
    s.animate(N, X, 80.0, motion::SETTLE);
    s.tick_at(0.05);
    assert!(s.is_animating(N, X));
    s.set(N, X, 10.0); // the user's hand: 1:1, cancels
    s.tick_at(0.06);
    assert_eq!(s.get(N, X), Some(10.0));
    assert!(!s.is_animating(N, X));
}

#[test]
fn settle_and_completion_events() {
    let mut s = AnimationStore::new();
    s.set(N, X, 0.0);
    s.animate(N, X, 1.0, motion::SNAP.completion(11));
    s.set_time(0.05);
    s.animate(N, X, 2.0, motion::SNAP.completion(12)); // replaces 11
    let mut events = Vec::new();
    for k in 0..400 {
        s.tick_at(0.05 + k as f64 / 120.0);
        events.extend_from_slice(s.events());
    }
    let completed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            MotionEvent::Completed { token, finished, .. } => Some((*token, *finished)),
            _ => None,
        })
        .collect();
    assert_eq!(completed, vec![(11, false), (12, true)]);
    let settles = events.iter().filter(|e| matches!(e, MotionEvent::Settled { .. })).count();
    assert_eq!(settles, 1);
    assert_eq!(s.get(N, X), Some(2.0));
    // Settle snaps below the 1/64 px quantum: the last moving frame was already that close.
    assert_eq!(s.active_len(), 0);
}

#[test]
fn removing_an_animating_track_completes_it_unfinished() {
    let completions = |s: &AnimationStore| -> Vec<(u64, bool)> {
        s.events().iter().filter_map(|e| match e {
            MotionEvent::Completed { token, finished, .. } => Some((*token, *finished)),
            _ => None,
        }).collect()
    };
    let mut s = AnimationStore::new();
    s.set(N, X, 0.0);
    s.animate(N, X, 1.0, motion::SNAP.completion(21));
    s.tick_at(0.02);
    assert!(s.is_animating(N, X));
    s.remove_node(N);
    s.tick_at(0.03);
    assert_eq!(completions(&s), [(21, false)], "the node unmounted mid-animation");

    s.set(N, X, 0.0);
    s.animate(N, X, 1.0, motion::SNAP.completion(22));
    s.tick_at(0.04);
    s.remove(N, X);
    s.tick_at(0.05);
    assert_eq!(completions(&s), [(22, false)], "the property's track went");

    s.set(N, X, 0.0);
    s.animate(N, X, 1.0, motion::SNAP.completion(23));
    let mut done = Vec::new();
    for k in 0..400 {
        s.tick_at(0.06 + k as f64 / 120.0);
        done.extend(completions(&s));
    }
    s.remove_node(N);
    s.tick_at(10.0);
    done.extend(completions(&s));
    assert_eq!(done, [(23, true)], "a finished animation completes once");
}

#[test]
fn fling_into_a_bound_hands_off_velocity_smoothly() {
    let mut s = AnimationStore::new();
    s.set(N, X, 0.0);
    s.release(N, X, 3000.0, Release::DecayClamped { decay: Decay::IOS_NORMAL, min: 0.0, max: 500.0, spring: motion::GLIDE });
    // Find the handoff by walking finely; velocity must stay continuous across it.
    let mut prev = state(&s);
    let mut max_dv: f32 = 0.0;
    let dt = 1.0 / 2000.0;
    for k in 1..8000 {
        s.tick_at(k as f64 * dt);
        let cur = (s.get(N, X).unwrap(), s.velocity(N, X).unwrap());
        max_dv = max_dv.max((cur.1 - prev.1).abs());
        prev = cur;
    }
    // Decay deceleration is k·v ≈ 6000/s² → 3 per step; the spring's is larger but bounded.
    assert!(max_dv < 60.0, "velocity jumped by {max_dv}");
    assert_eq!(s.get(N, X), Some(500.0));
}

#[test]
fn rubber_band_then_spring_back() {
    let mut s = AnimationStore::new();
    let over = rune_motion::rubber_band_clamp(-120.0, 0.0, 500.0, 800.0);
    assert!(over < 0.0 && over > -120.0 * 0.55);
    s.set(N, X, over);
    s.release(N, X, 0.0, Release::DecayClamped { decay: Decay::IOS_NORMAL, min: 0.0, max: 500.0, spring: motion::GLIDE });
    s.tick_at(5.0);
    s.tick_at(5.1);
    assert_eq!(s.get(N, X), Some(0.0));
}

#[test]
fn keyframes_hit_values_and_keep_velocity_through_cubics() {
    let mut s = AnimationStore::new();
    s.set(N, X, 0.0);
    let kf = Keyframes::new()
        .linear(10.0, 0.1)
        .cubic(30.0, 0.2)
        .cubic(20.0, 0.2)
        .cubic(40.0, 0.2)
        .spring(0.0, motion::SETTLE);
    s.play_keyframes(N, X, &kf);
    let at = |s: &mut AnimationStore, t: f64| {
        s.tick_at(t);
        (s.get(N, X).unwrap(), s.velocity(N, X).unwrap())
    };
    assert!((at(&mut s, 0.05).0 - 5.0).abs() < 1e-3, "linear halfway");
    assert!((at(&mut s, 0.1).0 - 10.0).abs() < 1e-3);
    assert!((at(&mut s, 0.3).0 - 30.0).abs() < 1e-3);
    // C¹ at the cubic joins: velocity just before and after 0.5 s agree.
    let before = at(&mut s, 0.5 - 1e-4).1;
    let after = at(&mut s, 0.5 + 1e-4).1;
    assert!((before - after).abs() < 0.5, "{before} vs {after}");
    assert!((at(&mut s, 0.7).0 - 40.0).abs() < 1e-3);
    at(&mut s, 3.0);
    assert_eq!(at(&mut s, 3.1).0, 0.0);
}

#[test]
fn reduced_motion_maps_by_property_class() {
    let mut s = AnimationStore::new();
    s.set_reduced_motion(true);
    s.set(N, prop::OFFSET, Vec2::ZERO);
    s.set(N, prop::OPACITY, 0.0);
    s.set(N, prop::BLUR, 4.0);
    s.animate(N, prop::OFFSET, Vec2::new(100.0, 0.0), motion::GLIDE);
    s.animate(N, prop::OPACITY, 1.0, motion::GLIDE);
    s.animate(N, prop::BLUR, 0.0, motion::GLIDE);
    s.tick_at(0.0);
    assert_eq!(s.get(N, prop::OFFSET), Some(Vec2::new(100.0, 0.0)), "travel jumps");
    assert_eq!(s.get(N, prop::BLUR), Some(0.0), "blur jumps");
    s.tick_at(0.09);
    let o = s.get(N, prop::OPACITY).unwrap();
    assert!(o > 0.3 && o < 0.7, "opacity takes the short fade: {o}");
    s.tick_at(0.18);
    assert_eq!(s.get(N, prop::OPACITY), Some(1.0));
    // Gestures are the user's own motion and are untouched.
    s.set(N, prop::SCROLL, Vec2::ZERO);
    s.release(N, prop::SCROLL, Vec2::new(0.0, 1000.0), Release::Decay(Decay::IOS_NORMAL));
    s.tick_at(0.3);
    assert!(s.get(N, prop::SCROLL).unwrap().y > 50.0);
}

#[test]
fn multi_lane_values_share_timing() {
    let mut s = AnimationStore::new();
    s.set(N, prop::FILL, Color::hex(0xff0000));
    s.animate(N, prop::FILL, Color::hex(0x0000ff), motion::SETTLE);
    s.tick_at(0.15);
    let mid = s.get(N, prop::FILL).unwrap();
    // All four lanes ride one spring: the colour is the OKLab interpolation at that progress.
    let p = motion::SETTLE.sample(0.0, 0.0, 1.0, 0.15).0;
    let (a, b) = (Color::hex(0xff0000).to_oklab(), Color::hex(0x0000ff).to_oklab());
    let want = Color::from_oklab(core::array::from_fn(|k| a[k] + (b[k] - a[k]) * p), 1.0);
    for (x, y) in [(mid.r, want.r), (mid.g, want.g), (mid.b, want.b), (mid.a, want.a)] {
        assert!((x - y).abs() < 2e-3, "{mid:?} vs {want:?}");
    }
    s.tick_at(5.0);
    let end = s.get(N, prop::FILL).unwrap();
    assert!((end.b - 1.0).abs() < 1e-3 && end.r < 1e-3);
}

#[test]
fn retune_respring_keeps_position_and_velocity() {
    let mut s = AnimationStore::new();
    s.set(N, X, 0.0);
    s.animate(N, X, 100.0, motion::DRIFT);
    s.set_time(0.2);
    let before = state(&s);
    s.tick();
    s.retune(motion::DRIFT, motion::SNAP);
    let after = state(&s);
    assert!((before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-2);
    s.tick_at(0.2 + motion::SNAP.settling_duration(1e-4) as f64 + 0.1);
    assert_eq!(s.get(N, X), Some(100.0));
}

#[test]
fn removing_and_reusing_tracks_is_generational() {
    let mut s = AnimationStore::new();
    let a = s.set(NodeId(1), X, 1.0);
    s.remove_node(NodeId(1));
    assert!(!s.contains(a));
    let b = s.set(NodeId(2), X, 2.0);
    assert!(s.contains(b) && !s.contains(a));
    assert_eq!(s.get(NodeId(2), X), Some(2.0));
    assert_eq!(s.get(NodeId(1), X), None);
}

#[test]
fn a_removed_track_leaves_touched_even_when_its_slot_is_reused() {
    let mut s = AnimationStore::new();
    s.set(NodeId(1), X, 1.0);
    s.set(NodeId(3), X, 3.0);
    s.tick();
    let touched = |s: &AnimationStore| s.touched().map(|(n, _)| n.0).collect::<Vec<_>>();
    assert_eq!(touched(&s), [1, 3]);
    s.remove_node(NodeId(1));
    assert_eq!(touched(&s), [3]);
    s.set(NodeId(1), X, 1.0);
    s.remove_node(NodeId(1));
    // Node 2's track takes the freed slot; the old track's entry doesn't count for it.
    s.set(NodeId(2), X, 2.0);
    s.tick();
    assert_eq!(touched(&s), [2]);
}

#[test]
fn node_time_scale_holds_slows_and_resumes_in_place() {
    const M: NodeId = NodeId(8);
    let mut s = AnimationStore::new();
    s.set(N, X, 0.0);
    s.set(M, X, 0.0);
    s.set_node_time_scale(M, 0.5);
    s.animate(N, X, 100.0, motion::SETTLE);
    s.animate(M, X, 100.0, motion::SETTLE.delay(0.1));
    s.tick_at(0.1);
    let reference = s.get(N, X).unwrap();
    assert_eq!(s.get(M, X), Some(0.0), "the delay runs at half speed too");
    s.tick_at(0.4);
    assert!((s.get(M, X).unwrap() - reference).abs() < 1e-3);
    // Stopped: nothing moves and nothing counts as running, pending or not.
    s.set_node_time_scale(N, 0.0);
    s.set_node_time_scale(M, 0.0);
    let held = (s.get(N, X).unwrap(), s.get(M, X).unwrap());
    s.tick_at(5.0);
    assert_eq!((s.get(N, X).unwrap(), s.get(M, X).unwrap()), held);
    assert!(!s.running() && s.active_len() == 2);
    // Resumed: carries on from the same place on the same curve.
    s.set_node_time_scale(M, 1.0);
    s.tick();
    assert_eq!(s.get(M, X).unwrap(), held.1);
    assert!(s.running());
    s.tick_at(5.15);
    assert!((s.get(M, X).unwrap() - motion::SETTLE.sample(0.0, 0.0, 100.0, 0.25).0).abs() < 1e-3);
    assert_eq!(s.node_time_scale(N), 0.0);
    assert_eq!(s.node_time_scale(M), 1.0);
}
