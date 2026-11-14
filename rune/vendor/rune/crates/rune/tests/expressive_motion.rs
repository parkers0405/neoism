//! Expressive presses: `El::scale` springs, `El::origin` pins the pivot, `El::rotate` turns a
//! subtree (and its hit tests), and shadow geometry springs with the element.
use rune::{Color, Cx, El, Env, Runtime, Spring, col, el};

#[derive(Default)]
struct S {
    k: f32,
    pressed: Vec<&'static str>,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let k = if s.k == 0.0 { 1.0 } else { s.k };
    col().w_full().h_full().items_start().padding(50.0).child(
        el().key("a")
            .size(100.0, 40.0)
            .fill(Color::WHITE)
            .motion(Spring::new(0.3, 0.0))
            .scale(k)
            .on_press(|s: &mut S| s.pressed.push("a")),
    )
}

fn app() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn rect<T>(rt: &Runtime<T>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
}

fn settle<T>(rt: &mut Runtime<T>) {
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

fn click(rt: &mut Runtime<S>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

#[test]
fn a_scale_change_springs_to_rest_and_hits_follow_it() {
    let mut rt = app();
    assert_eq!(rect(&rt, "a"), [50.0, 50.0, 100.0, 40.0]);
    rt.update(|s, _| s.k = 0.5);
    rt.frame(1.0 / 60.0);
    let w = rect(&rt, "a")[2];
    assert!(w > 50.0 && w < 100.0, "mid-spring: {w}");
    settle(&mut rt);
    assert_eq!(rect(&rt, "a"), [75.0, 60.0, 50.0, 20.0], "rests at half size about the centre");
    click(&mut rt, 60.0, 55.0);
    assert!(rt.state.pressed.is_empty(), "outside the drawn rect");
    click(&mut rt, 80.0, 65.0);
    assert_eq!(rt.state.pressed, ["a"]);
}

#[test]
fn a_scale_change_lands_at_once_under_reduce_motion_and_in_an_instant_transaction() {
    let mut rt = app();
    rt.set_system_reduced_motion(true);
    rt.update(|s, _| s.k = 0.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(rect(&rt, "a")[2], 50.0, "Reduce Motion");

    let mut rt = app();
    rt.update(|s, tx| {
        s.k = 2.0;
        tx.instant();
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rect(&rt, "a")[2], 200.0, "instant transaction");
}

#[test]
fn a_sprung_scale_multiplies_with_live_scale_and_press_scale() {
    struct M {
        k: f32,
        live: rune::Live<f32>,
    }
    fn view(m: &M, _: &mut Cx<M>) -> El {
        col().w_full().h_full().items_start().padding(50.0).child(
            el().key("a").size(100.0, 40.0).fill(Color::WHITE).scale(m.k).live_scale(&m.live).press_scale(0.5).on_press(|_: &mut M| {}),
        )
    }
    let live = rune::Live::new(0.5f32);
    let mut rt = Runtime::new(M { k: 1.0, live: live.clone() }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|m, _| m.k = 2.0);
    settle(&mut rt);
    assert_eq!(rect(&rt, "a")[2], 100.0, "2 x 0.5");
    rt.pointer_move(100.0, 70.0);
    rt.pointer_down(100.0, 70.0);
    settle(&mut rt);
    assert_eq!(rect(&rt, "a")[2], 50.0, "2 x 0.5 x 0.5 while pressed");
    rt.pointer_up(100.0, 70.0);
    live.set(1.0);
    settle(&mut rt);
    assert_eq!(rect(&rt, "a")[2], 200.0);
}

#[test]
fn an_origin_at_the_bottom_edge_keeps_it_fixed_while_the_scale_springs() {
    struct M {
        k: f32,
        live: rune::Live<f32>,
    }
    fn view(m: &M, _: &mut Cx<M>) -> El {
        col().w_full().h_full().items_start().padding(50.0).child(
            el().key("a").size(100.0, 40.0).fill(Color::WHITE).motion(Spring::new(0.3, 0.0))
                .origin(0.5, 1.0).scale(m.k).live_scale(&m.live).press_scale(0.5).on_press(|_: &mut M| {}),
        )
    }
    let live = rune::Live::new(1.0f32);
    let mut rt = Runtime::new(M { k: 1.0, live: live.clone() }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|m, _| m.k = 0.5);
    for f in 0..60 {
        rt.frame(1.0 / 60.0);
        let r = rect(&rt, "a");
        assert!((r[1] + r[3] - 90.0).abs() < 0.01, "frame {f}: bottom edge moved: {r:?}");
        assert!((r[0] + r[2] / 2.0 - 100.0).abs() < 0.01, "frame {f}: stays centred across: {r:?}");
    }
    assert_eq!(rect(&rt, "a"), [75.0, 70.0, 50.0, 20.0]);

    // `live_scale` and `press_scale` pivot there too.
    live.set(2.0);
    rt.frame(0.0);
    assert_eq!(rect(&rt, "a"), [50.0, 50.0, 100.0, 40.0]);
    rt.pointer_move(100.0, 70.0);
    rt.pointer_down(100.0, 70.0);
    settle(&mut rt);
    assert_eq!(rect(&rt, "a"), [75.0, 70.0, 50.0, 20.0]);
}

#[test]
fn a_transition_scales_about_the_origin() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        let mut root = col().w_full().h_full().items_start().padding(50.0);
        if *show {
            root = root.child(el().key("a").size(100.0, 40.0).fill(Color::WHITE).origin(0.0, 0.0)
                .transition(rune::Transition::scale(0.5)));
        }
        root
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    let r = rect(&rt, "a");
    assert!(r[2] < 100.0, "entering: {r:?}");
    assert_eq!([r[0], r[1]], [50.0, 50.0], "grows from its top-left corner");
}

#[test]
fn shadow_geometry_springs_with_the_element() {
    fn view(pressed: &bool, _: &mut Cx<bool>) -> El {
        let sh = if *pressed {
            rune::Shadow { x: 0.0, y: 2.0, blur: 4.0, spread: 0.0, color: Color::BLACK.alpha(0.3), inset: false }
        } else {
            rune::Shadow { x: 4.0, y: 10.0, blur: 20.0, spread: 2.0, color: Color::BLACK.alpha(0.3), inset: false }
        };
        col().w_full().h_full().items_start().padding(50.0).child(
            el().key("a").size(100.0, 40.0).fill(Color::WHITE).motion(Spring::new(0.3, 0.0)).box_shadow(sh),
        )
    }
    fn shadow(rt: &Runtime<bool>) -> ([f32; 4], f32) {
        let q = rt.instances.iter().find(|q| q.params[1] == rune::render::KIND_SHADOW).expect("a shadow");
        (q.rect, q.params[2])
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (rest, rest_sigma) = shadow(&rt);
    assert_eq!(rest, [52.0, 58.0, 104.0, 44.0], "x 4, y 10, spread 2");
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    let (mid, mid_sigma) = shadow(&rt);
    assert!(mid[1] > 52.0 && mid[1] < 58.0, "y between: {mid:?}");
    assert!(mid[0] > 50.0 && mid[0] < 52.0, "x between: {mid:?}");
    assert!(mid[2] > 100.0 && mid[2] < 104.0, "spread between: {mid:?}");
    assert!(mid_sigma < rest_sigma && mid_sigma > 2.0, "blur between: {mid_sigma}");
    settle(&mut rt);
    assert_eq!(shadow(&rt), ([50.0, 52.0, 100.0, 40.0], 2.0));

    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.set_system_reduced_motion(true);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert_eq!(shadow(&rt).0, [50.0, 52.0, 100.0, 40.0], "lands at once under Reduce Motion");
}

#[test]
fn rotate_springs_turns_the_subtree_in_a_layer_and_hits_where_it_shows() {
    fn view(turned: &bool, _: &mut Cx<bool>) -> El {
        col().w_full().h_full().items_start().padding(50.0).child(
            el().key("bar").size(100.0, 20.0).fill(Color::WHITE).motion(Spring::new(0.3, 0.0))
                .rotate(if *turned { std::f32::consts::FRAC_PI_2 } else { 0.0 })
                .on_press(|s: &mut bool| *s = !*s)
                .child(el().key("dot").size(10.0, 10.0).fill(Color::BLACK)),
        )
    }
    fn turn(rt: &Runtime<bool>) -> Option<f32> {
        rt.instances.iter().find(|q| q.params[1] == rune::render::KIND_LAYER).map(|q| q.border[1])
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(turn(&rt), None, "no layer while upright");
    // A press on its right end turns it a quarter, about its centre (100, 60).
    rt.pointer_move(145.0, 60.0);
    rt.pointer_down(145.0, 60.0);
    rt.pointer_up(145.0, 60.0);
    rt.frame(1.0 / 60.0);
    let mid = turn(&rt).expect("a turned layer");
    assert!(mid > 0.0 && mid < std::f32::consts::FRAC_PI_2, "mid-spring: {mid}");
    settle(&mut rt);
    assert!((turn(&rt).unwrap() - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    let r = rect(&rt, "bar");
    assert!(r.iter().zip([90.0, 10.0, 20.0, 100.0]).all(|(a, b)| (a - b).abs() < 0.01), "upright bounds of the turned bar: {r:?}");
    let d = rect(&rt, "dot");
    assert!(d.iter().zip([100.0, 10.0, 10.0, 10.0]).all(|(a, b)| (a - b).abs() < 0.01), "the child turns with it: {d:?}");
    // Its right end no longer shows there; its new top end does.
    rt.pointer_move(145.0, 60.0);
    rt.pointer_down(145.0, 60.0);
    rt.pointer_up(145.0, 60.0);
    rt.frame(0.0);
    assert!(rt.state, "missed the old right end");
    rt.pointer_move(100.0, 15.0);
    rt.pointer_down(100.0, 15.0);
    rt.pointer_up(100.0, 15.0);
    rt.frame(0.0);
    assert!(!rt.state, "hit the turned top end");

    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.set_system_reduced_motion(true);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!((turn(&rt).unwrap() - std::f32::consts::FRAC_PI_2).abs() < 1e-4, "lands at once under Reduce Motion");
}

#[test]
fn rotate_and_scale_share_the_origin() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full().items_start().padding(50.0).child(
            el().key("a").size(100.0, 20.0).fill(Color::WHITE).origin(0.0, 0.0)
                .rotate(std::f32::consts::FRAC_PI_2).scale(0.5),
        )
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let r = rect(&rt, "a");
    // Half size about its top-left (50, 50), then a quarter turn clockwise about the same point.
    assert!(r.iter().zip([40.0, 50.0, 10.0, 50.0]).all(|(a, b)| (a - b).abs() < 0.01), "{r:?}");
}

#[test]
fn rotated_hit_test_uses_turned_box_not_upright_bounds() {
    fn view(_: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
        col().w_full().h_full().items_start().padding(50.0)
            .on_press(|s: &mut Vec<&'static str>| s.push("bg"))
            .child(
                // Centre at (100, 60), 100x20 turned 45° clockwise.
                // Upright bbox is ~[57.6, 17.6]..[142.4, 102.4], but (130, 30) is in the empty top-right corner.
                el().key("bar").size(100.0, 20.0).fill(Color::WHITE)
                    .rotate(std::f32::consts::FRAC_PI_4)
                    .on_press(|s: &mut Vec<&'static str>| s.push("bar")),
            )
    }
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // (130, 30) is well inside the upright bbox [57.6..142.4, 17.6..102.4], but 42 pt off the 45° diagonal.
    rt.pointer_move(130.0, 30.0);
    rt.pointer_down(130.0, 30.0);
    rt.pointer_up(130.0, 30.0);
    rt.frame(0.0);
    assert_eq!(rt.state, ["bg"], "empty corner of upright bbox must not steal presses from behind");

    // (120, 80) lies right along the +45° diagonal from (100, 60).
    rt.pointer_move(120.0, 80.0);
    rt.pointer_down(120.0, 80.0);
    rt.pointer_up(120.0, 80.0);
    rt.frame(0.0);
    assert_eq!(rt.state, ["bg", "bar"]);
}

#[test]
fn clip_inside_rotated_element_and_rotated_element_inside_rounded_clip() {
    fn view(_: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
        col().w_full().h_full().items_start().padding(50.0).gap(20.0)
            .child(
                // Rotated 90° clockwise about its centre (80, 70): upright 60x40 becomes screen [60..100, 40..100].
                // Child is 120x40 (sticks out 60pt past the right edge of the 60x40 clip, which turns to y = 100..160).
                el().key("rot_clip").size(60.0, 40.0).clip_soft(0.0).rotate(std::f32::consts::FRAC_PI_2).child(
                    el().key("inner").size(120.0, 40.0).fill(Color::WHITE)
                        .on_press(|s: &mut Vec<&'static str>| s.push("inner")),
                ),
            )
            .child(
                el().key("outer_round").size(80.0, 40.0).radius(12.0).clip_rounded(12.0).child(
                    el().key("turned_in_round").size(80.0, 20.0).fill(Color::WHITE).rotate(0.4),
                ),
            )
    }
    let mut rt = Runtime::new(Vec::new(), view, (240.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Inside the rotated clip (80, 80): hits `inner`.
    rt.pointer_move(80.0, 80.0);
    rt.pointer_down(80.0, 80.0);
    rt.pointer_up(80.0, 80.0);
    rt.frame(0.0);
    assert_eq!(rt.state, ["inner"]);

    // In the protruding part of `inner` clipped away by `rot_clip` (80, 120): must not hit.
    rt.pointer_move(80.0, 120.0);
    rt.pointer_down(80.0, 120.0);
    rt.pointer_up(80.0, 120.0);
    rt.frame(0.0);
    assert_eq!(rt.state, ["inner"], "protruding part of child is clipped by rotated parent");

    // The turned layer inside `outer_round` carries both the turn (`border[1] != 0`) and the packed rounded clip (`fade[3] != 0`).
    let turned_layer = rt.instances.iter()
        .find(|q| q.params[1] == rune::render::KIND_LAYER && (q.border[1] - 0.4).abs() < 1e-4)
        .expect("turned layer inside rounded clip");
    assert_ne!(turned_layer.fade[3], 0, "turned layer composite preserves ancestor per-corner rounded clip");
}

#[test]
fn shadow_list_length_changes_and_instant_transactions_settle_cleanly() {
    fn view(mode: &u8, _: &mut Cx<u8>) -> El {
        let s1 = rune::Shadow { x: 0.0, y: if *mode == 1 { 2.0 } else { 10.0 }, blur: 12.0, spread: 0.0, color: Color::BLACK.alpha(0.3), inset: false };
        let s2 = rune::Shadow { x: 0.0, y: if *mode == 3 { 4.0 } else { 24.0 }, blur: 20.0, spread: 2.0, color: Color::BLACK.alpha(0.2), inset: false };
        let mut e = el().key("a").size(100.0, 40.0).fill(Color::WHITE).motion(Spring::new(0.3, 0.0)).box_shadow(s1);
        if *mode == 0 || *mode == 3 {
            e = e.box_shadow(s2);
        }
        col().w_full().h_full().items_start().padding(50.0).child(e)
    }
    let mut rt = Runtime::new(0u8, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Start springing shadow 1 and remove shadow 2 at the same time.
    rt.update(|m, _| *m = 1);
    rt.frame(1.0 / 60.0);
    // Land mid-spring with `tx.settle()` on the same target (`mode = 1`).
    rt.update(|_, tx| tx.settle());
    assert!(!rt.frame(1.0 / 60.0), "settling lands in-flight shadow geometry immediately");

    // Re-adding a second shadow (`mode = 3`) mounts at its new geometry (y = 4, spread = 2 -> rect y = 50 + 4 - 2 = 52)
    // instead of springing from the old removed second shadow (y = 24).
    rt.update(|m, _| *m = 3);
    rt.frame(1.0 / 60.0);
    let shadows: Vec<_> = rt.instances.iter().filter(|q| q.params[1] == rune::render::KIND_SHADOW).collect();
    assert_eq!(shadows.len(), 2);
    assert_eq!(shadows[1].rect, [48.0, 52.0, 104.0, 44.0], "re-added second shadow mounts at its own geometry");
}

#[test]
fn press_scale_and_exiting_child_inside_rotated_element_stay_turned() {
    fn view(show_child: &bool, _: &mut Cx<bool>) -> El {
        let mut bar = el().key("bar").size(100.0, 20.0).fill(Color::WHITE)
            .origin(0.0, 0.0)
            .rotate(std::f32::consts::FRAC_PI_2)
            .press_scale(0.5)
            .on_press(|_: &mut bool| {});
        if *show_child {
            bar = bar.child(el().key("chip").size(20.0, 10.0).fill(Color::BLACK).transition(rune::Transition::opacity()));
        }
        col().w_full().h_full().items_start().padding(50.0).child(bar)
    }
    let mut rt = Runtime::new(true, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rect(&rt, "bar").iter().zip([30.0, 50.0, 20.0, 100.0]).all(|(a, b)| (a - b).abs() < 0.01));

    // Press the rotated bar at (40, 80): `press_scale(0.5)` about origin (0, 0) = (50, 50)
    // shrinks the rotated bar to [40.0, 50.0, 10.0, 50.0] without its top-right pivot drifting.
    rt.pointer_move(40.0, 80.0);
    rt.pointer_down(40.0, 80.0);
    settle(&mut rt);
    let pressed_r = rect(&rt, "bar");
    assert!(pressed_r.iter().zip([40.0, 50.0, 10.0, 50.0]).all(|(a, b)| (a - b).abs() < 0.01), "{pressed_r:?}");
    rt.pointer_up(40.0, 80.0);
    settle(&mut rt);

    // Remove `chip`: while exiting, it must still draw inside `bar`'s turned layer.
    rt.update(|s, _| *s = false);
    rt.frame(1.0 / 60.0);
    let end_idx = rt.instances.iter().position(|q| q.params[1] == rune::render::KIND_LAYER_END).expect("turned layer end");
    assert_eq!(end_idx, rt.instances.len() - 1, "exiting child paints before the turned layer closes");
}
