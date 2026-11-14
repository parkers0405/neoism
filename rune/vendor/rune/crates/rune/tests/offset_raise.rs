//! `El::offset` shifts an element without moving its siblings and springs between shifts;
//! `El::raise` paints and hits an element above later siblings; `on_hover_change` hears an
//! element that appears under a still pointer.
use rune::{Cx, El, Env, Runtime, Spring, col, el};

#[derive(Default)]
struct S {
    shift: f32,
    raise: bool,
    pressed: Vec<&'static str>,
    show: bool,
    hover: Vec<bool>,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let mut a = el()
        .key("a")
        .size(100.0, 40.0)
        .motion(Spring::new(0.3, 0.0))
        .offset(0.0, s.shift)
        .on_press(|s: &mut S| s.pressed.push("a"));
    if s.raise {
        a = a.raise();
    }
    let b = el().key("b").size(100.0, 40.0).on_press(|s: &mut S| s.pressed.push("b"));
    let mut root = col().w_full().h_full().child(a).child(b);
    if s.show {
        root = root.child(
            el().key("zone").size(200.0, 200.0).absolute(0.0, 0.0).on_hover_change(|s: &mut S, inside, _| s.hover.push(inside)),
        );
    }
    root
}

fn app() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn rect(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect
}

fn click(rt: &mut Runtime<S>, x: f32, y: f32) {
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
}

#[test]
fn an_offset_springs_without_moving_siblings_and_hits_follow_it() {
    let mut rt = app();
    rt.update(|s, _| s.shift = 40.0);
    rt.frame(1.0 / 60.0);
    let y = rect(&rt, "a")[1];
    assert!(y > 0.0 && y < 40.0, "mid-spring: {y}");
    assert_eq!(rect(&rt, "b")[1], 40.0, "the sibling stays in place");
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rect(&rt, "a")[1], 40.0);
    // `b` comes later in the tree, so it takes the press where they overlap.
    click(&mut rt, 50.0, 60.0);
    assert_eq!(rt.state.pressed, ["b"]);
    click(&mut rt, 50.0, 10.0);
    assert_eq!(rt.state.pressed, ["b"], "nothing is left where `a` was");

    rt.set_system_reduced_motion(true);
    rt.update(|s, _| s.shift = 0.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rect(&rt, "a")[1], 0.0, "Reduce Motion lands it at once");
}

#[test]
fn a_raised_element_paints_and_takes_presses_above_later_siblings() {
    let mut rt = app();
    rt.update(|s, _| {
        s.shift = 20.0;
        s.raise = true;
    });
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    click(&mut rt, 50.0, 50.0);
    assert_eq!(rt.state.pressed, ["a"], "the raised element is on top where they overlap");
    let order: Vec<_> = rt.elements().into_iter().filter_map(|e| e.key).collect();
    assert_eq!(&order[..2], ["a", "b"], "tree order is unchanged");

    rt.update(|s, _| s.raise = false);
    rt.frame(0.0);
    click(&mut rt, 50.0, 50.0);
    assert_eq!(rt.state.pressed, ["a", "b"]);
}

#[test]
fn hover_change_hears_an_element_that_appears_under_a_still_pointer() {
    let mut rt = app();
    rt.pointer_move(150.0, 150.0);
    rt.frame(0.0);
    rt.update(|s, _| s.show = true);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.hover, [true], "the pointer is inside without moving");
}

mod reorder {
    use std::borrow::Cow;

    use rune::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, Spring, col, el};

    struct S {
        order: Vec<usize>,
        shift: Vec<f32>,
        list: u8,
    }

    fn row(s: &S, slot: usize) -> El {
        el().key(format!("r{}", s.order[slot])).w_full().h(40.0).motion(Spring::new(0.3, 0.0)).offset(0.0, s.shift[slot])
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let list = col().key("list").w_full();
        let list = match s.list {
            0 => list.children((0..s.order.len()).map(|i| row(s, i))),
            1 => list.growth(GrowthSpec::default()).children((0..s.order.len()).map(|i| row(s, i))),
            _ => list.growth(GrowthSpec::default()).virtual_rows(
                s.order.len(),
                40.0,
                |s: &S, i| Cow::Owned(format!("r{}", s.order[i])),
                |s: &S, i| row(s, i),
            ),
        };
        col().w_full().h_full().child(col().w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(list))
    }

    fn y(rt: &Runtime<S>, k: &str) -> f32 {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).unwrap().rect[1]
    }

    /// Drag `r0` 50 points down past `r1`, which steps up, then drop it in slot 1.
    fn drop_reordered(list: u8) {
        let mut rt = Runtime::new(S { order: vec![0, 1, 2, 3], shift: vec![0.0; 4], list }, view, (200.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|s, tx| {
            tx.instant();
            s.shift = vec![50.0, -40.0, 0.0, 0.0];
        });
        rt.frame(1.0 / 60.0);
        assert_eq!((y(&rt, "r0"), y(&rt, "r1")), (50.0, 0.0));
        rt.update(|s, _| {
            s.order = vec![1, 0, 2, 3];
            s.shift = vec![0.0; 4];
        });
        let mut last = 50.0;
        for _ in 0..90 {
            rt.frame(1.0 / 60.0);
            let (r0, r1) = (y(&rt, "r0"), y(&rt, "r1"));
            assert!(r0 <= last + 0.01 && r0 >= 39.99, "list {list}: r0 slides from 50 to 40, at {r0}");
            assert!(r1.abs() < 0.01, "list {list}: r1 stays where it was drawn, at {r1}");
            last = r0;
        }
        assert!((last - 40.0).abs() < 0.01);
    }

    #[test]
    fn a_reordered_row_that_drops_its_offset_slides_from_where_it_was_drawn() {
        drop_reordered(0);
        drop_reordered(1);
        drop_reordered(2);
    }

    #[test]
    fn a_drop_mid_spring_in_five_thousand_virtual_rows_preserves_the_drawn_position() {
        let mut rt = Runtime::new(S { order: (0..5000).collect(), shift: vec![0.0; 5000], list: 2 }, view, (200.0, 400.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|s, _| { s.shift[0] = 85.0; s.shift[1] = -40.0; s.shift[2] = -40.0; });
        rt.frame(1.0 / 120.0);
        rt.frame(1.0 / 120.0);
        let before: Vec<_> = ["r0", "r1", "r2"].map(|k| y(&rt, k)).into();
        rt.update(|s, _| { s.order.remove(0); s.order.insert(2, 0); s.shift.fill(0.0); });
        rt.frame(0.0);
        for (i, key) in ["r0", "r1", "r2"].iter().enumerate() {
            assert!((y(&rt, key) - before[i]).abs() < 0.1, "{key} jumped on drop: {} -> {}", before[i], y(&rt, key));
        }
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        assert_eq!([y(&rt, "r0"), y(&rt, "r1"), y(&rt, "r2")], [80.0, 0.0, 40.0]);
    }
}

mod drag_animated {
    use rune::{Cx, Drag, El, Env, Phase, Runtime, Spring, Tx, col, el};

    struct S {
        animated: bool,
        shift: f32,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let handle = el().key("handle").size(100.0, 40.0).on_drag(|s: &mut S, d: &mut Drag, tx: &mut Tx| {
            if d.phase == Phase::Move {
                if s.animated {
                    tx.animated();
                }
                s.shift = 40.0;
            }
        });
        let other = el().key("other").size(100.0, 40.0).motion(Spring::new(0.3, 0.0)).offset(0.0, s.shift);
        col().w_full().h_full().child(handle).child(other)
    }

    fn moved_after_a_drag(animated: bool) -> f32 {
        let mut rt = Runtime::new(S { animated, shift: 0.0 }, view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.pointer_move(50.0, 20.0);
        rt.pointer_down(50.0, 20.0);
        rt.pointer_move(50.0, 60.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        rt.pointer_up(50.0, 60.0);
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("other")).unwrap().rect[1] - 40.0
    }

    #[test]
    fn a_drag_move_lands_at_once_unless_the_handler_animates_it() {
        assert_eq!(moved_after_a_drag(false), 40.0, "direct manipulation");
        let y = moved_after_a_drag(true);
        assert!(y > 0.0 && y < 40.0, "on its spring: {y}");
    }

    #[test]
    fn a_live_only_drag_cannot_make_another_handlers_commit_instant() {
        fn live_view(s: &S, _: &mut Cx<S>) -> El {
            col().w_full().h_full()
                .child(el().size(100.0, 40.0).on_drag(|_: &mut S, d, tx| {
                    if d.phase == Phase::Move { tx.live_only(); }
                }))
                .child(el().key("other").size(100.0, 40.0).offset(0.0, s.shift).motion(Spring::new(0.3, 0.0)))
        }
        let mut rt = Runtime::new(S { animated: false, shift: 0.0 }, live_view, (200.0, 200.0), Env::new(true, 1.0));
        rt.check_live_only(false);
        rt.on_key = Some(|s, _, tx| { s.shift = 40.0; tx.motion(Spring::new(0.6, 0.0)); true });
        rt.frame(0.0);
        rt.pointer_down(20.0, 20.0);
        rt.frame(0.0);
        rt.pointer_move(20.0, 30.0);
        rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        let shift = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("other")).unwrap().rect[1] - 40.0;
        assert!(shift > 0.0 && shift < 10.0, "the key's spring must survive a live-only Move: {shift}");
        assert_eq!(rt.live_only_events(), 1);
    }
}

mod double_press_on_a_drag {
    use rune::{Cx, Drag, El, Env, Phase, Runtime, Spring, Tx, col, el};

    #[derive(Default)]
    struct S {
        wide: bool,
    }

    fn view(s: &S, _: &mut Cx<S>) -> El {
        let grip = el()
            .key("grip")
            .size(100.0, 40.0)
            .on_drag(|_: &mut S, d: &mut Drag, tx: &mut Tx| {
                if d.phase == Phase::Move {
                    tx.instant();
                }
            })
            .on_double_press(|s: &mut S| s.wide = true);
        let pane = el().key("pane").h(40.0).w(if s.wide { 200.0 } else { 100.0 }).motion(Spring::new(0.3, 0.0));
        col().w_full().h_full().child(grip).child(pane)
    }

    #[test]
    fn a_double_click_on_a_draggable_element_animates_what_it_changes() {
        let mut rt = Runtime::new(S::default(), view, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.pointer_move(50.0, 20.0);
        for _ in 0..2 {
            rt.pointer_down(50.0, 20.0);
            rt.pointer_up(50.0, 20.0);
        }
        rt.frame(1.0 / 60.0);
        assert!(rt.state.wide, "double pressed");
        let w = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("pane")).unwrap().rect[2];
        assert!(w > 100.0 && w < 200.0, "on its spring, not at once: {w}");
    }
}
