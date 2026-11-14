use super::*;


#[test]
fn overlay_hit_regions_follow_live_geometry_and_ancestor_clips_in_the_same_frame() {
    struct State { offset: crate::Live<[f32; 2]>, opacity: crate::Live<f32>, pressed: u32, hovered: u32, visible: bool }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        el().child(el().size(60.0, 60.0).clip_soft(0.0)
            .children(s.visible.then(|| el().key("chip").size(40.0, 20.0).pin(Some(30.0), Some(0.0), None, None)
                .live_offset(&s.offset).live_opacity(&s.opacity).hit()
                .on_hover(|s: &mut State| s.hovered += 1).on_press(|s: &mut State| s.pressed += 1))))
            .child(el().size(10.0, 10.0).on_hover(|_: &mut State| {}))
    }
    let offset = crate::Live::new([0.0, 0.0]);
    let opacity = crate::Live::new(1.0);
    let mut rt = Runtime::new(State { offset: offset.clone(), opacity: opacity.clone(), pressed: 0, hovered: 0, visible: true }, view, (100.0, 100.0), Env { system_dark: true, scale: 2.0 });
    rt.native_overlay = true;
    rt.frame(0.0);
    assert_eq!(rt.hit_regions(), &[[30.0, 0.0, 60.0, 20.0]]);
    rt.pointer_move(15.0, 10.0);
    offset.set([20.0, 0.0]);
    rt.frame(0.0);
    assert_eq!(rt.hit_regions(), &[[50.0, 0.0, 60.0, 20.0]], "logical regions must use this frame's live offset, not the previous layout");
    offset.set([-20.0, 0.0]);
    rt.frame(0.0);
    assert_eq!(rt.hit_regions(), &[[10.0, 0.0, 50.0, 20.0]]);
    rt.pointer_down(15.0, 10.0);
    rt.pointer_up(15.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.pressed, 1, "the native hit and Rune release must agree on the moved chip");
    assert_eq!(rt.state.hovered, 1, "a chip moving under a stationary pointer enters hover once");
    assert!(!rt.has_pending_input(), "stationary hover must not generate an endless frame loop");
    rt.pointer_down(15.0, 10.0);
    rt.pointer_up(55.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.pressed, 1, "release inside the old layout but outside the drawn chip must cancel");
    opacity.set(0.0);
    rt.frame(0.0);
    assert!(rt.hit_regions().is_empty(), "transparent nodes and hover-only elements do not block native input");
    opacity.set(1.0);
    rt.update(|s, _| s.visible = false);
    rt.frame(0.0);
    assert!(rt.hit_regions().is_empty(), "exiting chips must immediately release native input");
}

#[test]
fn overlay_pointer_observation_does_not_require_a_native_hit_region() {
    fn view(_: &Vec<PointerPress>, _: &mut Cx<Vec<PointerPress>>) -> El {
        el().on_pointer_move(|s: &mut Vec<PointerPress>, p| s.push(p))
    }
    let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.pointer_move(20.0, 40.0);
    rt.frame(0.0);
    assert_eq!(rt.state, [PointerPress { x: 20.0, y: 40.0 }]);
    assert!(rt.hit_regions().is_empty());
}

#[test]
fn overlay_idle_pointer_does_not_request_unchanged_frames() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        el().child(el().size(40.0, 20.0).hit().on_hover(|n: &mut u32| *n += 1))
    }
    let mut rt = Runtime::new(0, view, (100.0, 100.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    assert!(rt.observe_pointer(5.0, 5.0), "enter must run the hover callback");
    rt.frame(0.0);
    assert_eq!(rt.state, 1);
    assert!(!rt.observe_pointer(6.0, 5.0), "movement within an unchanged chip needs no frame");
    assert!(!rt.observe_pointer(6.0, 5.0), "duplicate monitor/winit observations need no frame");
    assert!(!rt.observe_pointer(90.0, 90.0));
    assert!(!rt.observe_pointer(200.0, 90.0), "movement outside the canvas needs no frame");
    assert!(!rt.has_pending_input());
}

#[test]
fn hit_tests_leave_out_the_right_and_bottom_edges() {
    fn tile(k: &'static str) -> El {
        el().key(k).size(40.0, 20.0)
            .on_press(move |s: &mut Vec<String>| s.push(format!("press {k}")))
            .on_hover(move |s: &mut Vec<String>| s.push(format!("hover {k}")))
            .on_hover_change(move |s: &mut Vec<String>, inside, _| if inside { s.push(format!("enter {k}")) })
            .on_pointer_move(move |s: &mut Vec<String>, _| s.push(format!("move {k}")))
    }
    fn view(_: &Vec<String>, _: &mut Cx<Vec<String>>) -> El {
        col().gap(20.0).child(crate::row().child(tile("a")).child(tile("b"))).child(tile("lone"))
    }
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 200.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    assert_eq!(rt.presented("lone").unwrap().0, [0.0, 40.0, 40.0, 20.0]);
    let mut click = |x, y| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        let mut events = std::mem::take(&mut rt.state);
        rt.pointer_leave();
        rt.frame(0.0);
        rt.state.clear();
        events.sort();
        events.dedup();
        events
    };
    assert_eq!(click(40.0, 10.0), ["enter b", "hover b", "move b", "press b"], "the shared edge is b's alone");
    assert_eq!(click(40.0, 50.0), Vec::<String>::new(), "a lone node's right edge is outside it");
    assert_eq!(click(20.0, 60.0), Vec::<String>::new(), "and so is its bottom edge");
    assert_eq!(click(0.0, 40.0), ["enter lone", "hover lone", "move lone", "press lone"], "its top-left corner is inside");
}

#[test]
fn deleting_a_focused_row_does_not_call_its_stale_edit_accessor() {
    fn view(s: &Vec<crate::TextEdit>, _: &mut Cx<Vec<crate::TextEdit>>) -> El {
        col().children(s.iter().enumerate().map(|(i, edit)| {
            crate::text_input(edit, crate::TextStyle::new(14.0, 400.0))
                .key(format!("row-{i}"))
                .on_edit(crate::edit_access(move |s: &mut Vec<crate::TextEdit>| &mut s[i]))
        })).child(el().key("delete").size(40.0, 40.0).on_press(|s: &mut Vec<crate::TextEdit>| { s.clear(); }))
    }
    let mut edit = crate::TextEdit::default();
    edit.focused = true;
    let mut rt = Runtime::new(vec![edit], view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let path = rt.nodes.iter().find(|n| n.key.as_deref() == Some("delete")).unwrap().path.clone();
    rt.queue.push(Action::Press(path, crate::PointerButton::Primary));
    rt.frame(0.0);
    assert!(rt.state.is_empty());
    assert!(!rt.has_focus());
}

#[test]
fn secondary_only_children_preserve_primary_click_hover_and_background_blur() {
    type State = (u32, u32, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let parent = el().key("parent").size(80.0, 80.0);
        let parent = if s.2 { parent } else {
            parent.on_press(|s: &mut State| s.0 += 1)
                .on_hover(|s: &mut State| s.1 += 1)
        };
        parent.child(el().key("child").size(40.0, 40.0)
            .on_secondary_press(|_: &mut State, _, _| {}))
    }
    let mut rt = Runtime::new((0, 0, false), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state, (1, 1, false));
    rt.state.2 = true;
    rt.invalidate();
    rt.frame(0.0);
    rt.focus.focus = crate::runtime::focus_machine::Focus::On { path: "previous-input".into(), hovered: false, from: None };
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    assert!(rt.hovered.is_none());
    assert!(matches!(rt.queue.iter().last(), Some(Action::Blur)));
    assert!(rt.secondary_target(10.0, 10.0).is_some());
}

#[test]
fn secondary_press_routes_to_ancestor_without_primary_and_cancels_outside() {
    type State = (u32, Vec<PointerPress>, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().child(if s.2 {
            el()
        } else {
            el().key("parent").size(80.0, 80.0)
                .on_secondary_press(|s: &mut State, p, _| s.1.push(p))
                .child(el().key("child").size(40.0, 40.0).on_press(|s: &mut State| s.0 += 1))
        })
    }
    let mut rt = Runtime::new((0, Vec::new(), false), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.secondary_down(10.0, 10.0);
    rt.secondary_up(12.0, 12.0);
    rt.frame(0.0);
    assert_eq!(rt.state.0, 0);
    assert_eq!(rt.state.1, [PointerPress { x: 12.0, y: 12.0 }]);
    rt.secondary_down(10.0, 10.0);
    rt.secondary_up(90.0, 90.0);
    rt.frame(0.0);
    assert_eq!(rt.state.1.len(), 1);
    rt.secondary_down(10.0, 10.0);
    rt.state.2 = true;
    rt.invalidate();
    rt.frame(0.0);
    rt.secondary_up(12.0, 12.0);
    rt.frame(0.0);
    assert_eq!(rt.state.1.len(), 1);
}

#[test]
fn presses_land_inside_a_scrolled_view() {
    struct State { pressed: Vec<usize> }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        let mut list = col().key("list").w_full();
        for i in 0..10 {
            list = list.child(el().key(format!("b{i}")).size(200.0, 40.0).on_press(move |s: &mut State| s.pressed.push(i)));
        }
        col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
    }
    let mut rt = Runtime::new(State { pressed: Vec::new() }, view, (200.0, 200.0), Env { system_dark: true, scale: 1.0 });
    rt.frame(0.0);
    rt.wheel(100.0, 100.0, -120.0);
    rt.frame(1.0 / 120.0);
    let (x, y) = rt.find("b5").expect("b5 is drawn");
    assert!(y < 200.0, "b5 scrolled into view at {y}");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.pressed, [5]);
}

#[test]
fn a_double_press_follows_the_first_press_within_the_interval_and_tolerance() {
    #[derive(Default)]
    struct State { presses: u32, doubles: u32, other: u32 }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        crate::row().size(300.0, 100.0)
            .child(el().key("row").size(100.0, 100.0)
                .on_press(|s: &mut State| s.presses += 1)
                .on_double_press(|s: &mut State| s.doubles += 1))
            .child(el().key("other").size(100.0, 100.0).on_press(|s: &mut State| s.other += 1))
    }
    let tick = |rt: &mut Runtime<State>, seconds: f64| for _ in 0..(seconds * 120.0).round() as usize { rt.frame(1.0 / 120.0); };
    let click = |rt: &mut Runtime<State>, (x, y): (f32, f32)| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.frame(1.0 / 120.0);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 120.0);
    };
    let counts = |rt: &Runtime<State>| (rt.state.presses, rt.state.doubles);
    let mut rt = Runtime::new(State::default(), view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);

    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (1, 0), "the first press runs on_press right away");
    tick(&mut rt, 0.1);
    click(&mut rt, (53.0, 47.0));
    assert_eq!(counts(&rt), (2, 1), "the second press inside the interval and 4 pt is a double press");
    assert!(rt.events.iter().any(|e| e.starts_with("double press")));
    tick(&mut rt, 0.1);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (3, 1), "a third press starts a new pair");
    tick(&mut rt, 0.1);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (4, 2));

    tick(&mut rt, 0.1);
    click(&mut rt, (50.0, 50.0));
    let slow = rt.double_click_interval + 0.05;
    tick(&mut rt, slow);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (6, 2), "too slow");

    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    click(&mut rt, (55.0, 50.0));
    assert_eq!(counts(&rt), (8, 2), "moved more than 4 pt");

    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    click(&mut rt, (150.0, 50.0));
    click(&mut rt, (50.0, 50.0));
    assert_eq!((counts(&rt), rt.state.other), ((10, 2), 1), "a press elsewhere breaks the pair");

    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    rt.pointer_down(50.0, 50.0);
    rt.frame(1.0 / 120.0);
    rt.pointer_move(250.0, 50.0);
    rt.pointer_up(250.0, 50.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(counts(&rt), (11, 2), "a second press released outside is cancelled");

    // A second press that drags 40 pt (still inside) and lets go 5 s later is no click.
    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    rt.pointer_down(50.0, 50.0);
    rt.frame(1.0 / 120.0);
    rt.pointer_move(90.0, 50.0);
    rt.pointer_move(60.0, 50.0);
    tick(&mut rt, 5.0);
    rt.pointer_up(60.0, 50.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(counts(&rt), (13, 2), "a drag or a long press doesn't finish a double press");
    // Held without moving past the interval, it's no click either.
    tick(&mut rt, 0.6);
    click(&mut rt, (50.0, 50.0));
    rt.pointer_down(50.0, 50.0);
    let long = rt.double_click_interval + 0.05;
    tick(&mut rt, long);
    rt.pointer_up(50.0, 50.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(counts(&rt), (15, 2), "a long second press");
    // A first press that drags doesn't arm one.
    tick(&mut rt, 0.6);
    rt.pointer_down(50.0, 50.0);
    rt.pointer_move(50.0, 90.0);
    rt.pointer_move(50.0, 50.0);
    rt.pointer_up(50.0, 50.0);
    rt.frame(1.0 / 120.0);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (17, 2), "a dragged first press");

    // Headless, a slower system setting is honoured on the same virtual clock.
    rt.double_click_interval = 1.0;
    tick(&mut rt, 1.1);
    click(&mut rt, (50.0, 50.0));
    tick(&mut rt, 0.8);
    click(&mut rt, (50.0, 50.0));
    assert_eq!(counts(&rt), (19, 3));
}

#[test]
fn an_element_with_only_a_double_press_is_a_press_target() {
    struct State { doubles: u32, behind: u32 }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        el().size(100.0, 100.0).on_press(|s: &mut State| s.behind += 1)
            .child(el().size(100.0, 100.0).on_double_press(|s: &mut State| s.doubles += 1))
    }
    let mut rt = Runtime::new(State { doubles: 0, behind: 0 }, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for _ in 0..2 {
        rt.pointer_move(50.0, 50.0);
        rt.pointer_down(50.0, 50.0);
        rt.frame(1.0 / 120.0);
        rt.pointer_up(50.0, 50.0);
        rt.frame(1.0 / 120.0);
    }
    assert_eq!((rt.state.doubles, rt.state.behind), (1, 0));
}

#[test]
fn presses_on_a_window_drag_region_move_the_window_but_not_its_buttons() {
    struct State { pressed: u32 }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 200.0)
            .child(crate::el::row().key("titlebar").size(200.0, 40.0).window_drag()
                .child(el().key("button").size(40.0, 40.0).on_press(|s: &mut State| s.pressed += 1)))
            .child(el().key("body").size(200.0, 40.0))
    }
    let mut rt = Runtime::new(State { pressed: 0 }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    for (x, y, drags) in [(100.0, 20.0, true), (20.0, 20.0, false), (100.0, 60.0, false)] {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        assert_eq!(rt.take_titlebar_action() == Some(TitlebarAction::Drag), drags, "at {x}, {y}");
        assert!(rt.take_titlebar_action().is_none(), "a drag request is taken once");
        rt.pointer_up(x, y);
    }
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.pressed, 1, "the button inside the title bar still presses");
}

#[test]
fn a_double_press_on_nothing_in_the_native_titlebar_band_takes_the_titlebar_action() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(200.0, 200.0).child(el().key("button").size(40.0, 40.0).on_press(|_: &mut ()| {}))
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let double = |rt: &mut Runtime<()>, x: f32, y: f32| {
        let mut actions = Vec::new();
        for _ in 0..2 {
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            actions.push(rt.take_titlebar_action());
            rt.pointer_up(x, y);
        }
        actions
    };
    assert_eq!(double(&mut rt, 100.0, 10.0), [None, None], "no native band, nothing to do");
    rt.set_native_titlebar_band(28.0);
    assert_eq!(double(&mut rt, 100.0, 10.0), [None, Some(TitlebarAction::DoubleClick)], "the system drags; Rune adds the double click");
    assert_eq!(double(&mut rt, 100.0, 60.0), [None, None], "below the band");
    assert_eq!(double(&mut rt, 20.0, 10.0), [None, None], "a button in the band keeps its presses");
}

#[test]
fn hover_change_spans_descendants_and_holds_through_a_press() {
    type State = (Vec<bool>, u32, u32);
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 200.0)
            .child(el().key("pill").size(100.0, 40.0).on_hover_change(|s: &mut State, inside, _| s.0.push(inside))
                .child(el().key("button").size(40.0, 40.0)
                    .on_press(|s: &mut State| s.1 += 1).on_hover(|s: &mut State| s.2 += 1)))
    }
    let mut rt = Runtime::new((Vec::new(), 0, 0), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(80.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state.0, [true]);
    rt.pointer_move(20.0, 20.0);
    rt.pointer_down(20.0, 20.0);
    rt.pointer_up(20.0, 20.0);
    rt.frame(0.0);
    assert_eq!(rt.state, (vec![true], 1, 1), "the child keeps its hover and press; entering it is not a leave");
    rt.pointer_down(20.0, 20.0);
    rt.pointer_move(20.0, 120.0);
    rt.pointer_leave();
    rt.frame(0.0);
    assert_eq!(rt.state.0, [true], "a held press stays inside");
    rt.pointer_up(20.0, 120.0);
    rt.frame(0.0);
    assert_eq!(rt.state.0, [true, false]);
    rt.pointer_move(50.0, 20.0);
    rt.pointer_leave();
    rt.frame(0.0);
    assert_eq!(rt.state.0, [true, false, true, false]);
}
