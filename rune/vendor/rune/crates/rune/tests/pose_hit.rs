//! `Runtime::presented_pose` and the moving flag cover shadow and rotation springs;
//! Picking shares press geometry, but also finds elements without handlers.
use rune::{Color, Cx, El, Env, Runtime, Shadow, Spring, col, el};
use std::f32::consts::FRAC_PI_4;

fn moving<T>(rt: &Runtime<T>, key: &str) -> bool {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().moving
}

fn settle<T>(rt: &mut Runtime<T>) {
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
}

fn sh(y: f32) -> Shadow {
    Shadow { x: 0.0, y, blur: 8.0, spread: 0.0, color: Color::BLACK.alpha(0.3), inset: false }
}

fn card(on: &bool, _: &mut Cx<bool>) -> El {
    let shadow = sh(if *on { 12.0 } else { 2.0 });
    col().w_full().h_full().items_start().padding(50.0).child(
        el().key("a").size(100.0, 40.0).fill(Color::WHITE).motion(Spring::new(0.3, 0.0)).box_shadow(shadow),
    )
}

#[test]
fn a_shadow_only_step_reports_moving_until_it_lands() {
    let mut rt = Runtime::new(false, card, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!moving(&rt, "a"));
    assert_eq!(rt.presented_pose("a").unwrap().shadows()[0].y, 2.0);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "a"), "the shadow's y is springing");
    let pose = rt.presented_pose("a").unwrap();
    assert!(pose.moving());
    let y = pose.shadows()[0].y;
    assert!(y > 2.0 && y < 12.0, "mid-spring y: {y}");
    settle(&mut rt);
    assert!(!moving(&rt, "a"));
    let pose = rt.presented_pose("a").unwrap();
    assert!(!pose.moving());
    assert_eq!(pose.shadows()[0].y, 12.0);
}

#[test]
fn a_rotation_only_step_reports_moving_until_it_lands() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        col().w_full().h_full().items_start().padding(50.0).child(
            el().key("a").size(100.0, 40.0).fill(Color::WHITE).motion(Spring::new(0.3, 0.0))
                .rotate(if *on { 1.0 } else { 0.0 }),
        )
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(!moving(&rt, "a"));
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "a"));
    let r = rt.presented_pose("a").unwrap().rotation();
    assert!(r > 0.0 && r < 1.0, "mid-spring rotation: {r}");
    settle(&mut rt);
    assert!(!moving(&rt, "a"));
    let pose = rt.presented_pose("a").unwrap();
    assert!((pose.rotation() - 1.0).abs() < 1e-3);
    assert!(!pose.moving());
}

#[derive(Default)]
struct Hits(Vec<&'static str>);

fn bar(_: &Hits, _: &mut Cx<Hits>) -> El {
    col().w_full().h_full().items_start().padding(50.0)
        .on_press(|s: &mut Hits| s.0.push("bg"))
        .child(
            // 100x20 about (100, 60), turned 45°.
            el().key("bar").size(100.0, 20.0).fill(Color::WHITE).rotate(FRAC_PI_4)
                .on_press(|s: &mut Hits| s.0.push("bar"))
                .child(el().key("inner").size(10.0, 10.0)),
        )
}

fn key_at(rt: &Runtime<Hits>, x: f32, y: f32) -> Option<String> {
    rt.element_at(x, y).and_then(|p| p.key().map(str::to_owned))
}

#[test]
fn element_at_follows_the_turned_box_and_agrees_with_presses() {
    let mut rt = Runtime::new(Hits::default(), bar, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // In the upright layout box [50..150, 50..70]; off the 45° diagonal.
    assert_ne!(key_at(&rt, 60.0, 52.0).as_deref(), Some("bar"));
    assert!(!rt.contains("bar", 60.0, 52.0));
    // On the diagonal, outside the upright box.
    assert_eq!(key_at(&rt, 120.0, 80.0).as_deref(), Some("bar"));
    assert!(rt.contains("bar", 120.0, 80.0));
    // In the upright bounding box, outside the turned rect.
    assert!(!rt.contains("bar", 130.0, 30.0));
    assert!(rt.element_at(130.0, 30.0).unwrap().key().is_none(), "the background column");
    // A child with no handlers is found.
    assert_eq!(key_at(&rt, 71.7, 24.6).as_deref(), Some("inner"));
    assert_eq!(rt.element_at(500.0, 500.0), None);

    for (x, y) in [(60.0, 52.0), (120.0, 80.0), (130.0, 30.0), (80.0, 40.0), (140.0, 90.0), (71.7, 24.6)] {
        let picked_bar = matches!(key_at(&rt, x, y).as_deref(), Some("bar" | "inner"));
        rt.state.0.clear();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state.0 == ["bar"], picked_bar, "({x}, {y}): press {:?}", rt.state.0);
    }
}

#[test]
fn a_canvas_handler_picks_its_last_presented_children_before_rebuilding() {
    #[derive(Default)]
    struct Canvas { point: (f32, f32), picked: Option<String>, hide: bool }
    fn view(s: &Canvas, _: &mut Cx<Canvas>) -> El {
        el().w_full().h_full()
            .child(col().key("canvas").w_full().h_full().items_start().padding(50.0)
                .on_pointer_move_local(|s: &mut Canvas, p, _| s.point = (p.window_x, p.window_y))
                .on_press_tx(|s: &mut Canvas, tx| {
                    s.hide = true;
                    tx.element_at_in("canvas", s.point.0, s.point.1, |s: &mut Canvas, p, tx| {
                        s.picked = p.and_then(|p| p.key().map(str::to_owned));
                        tx.instant();
                    });
                })
                .children((!s.hide).then(|| el().key("bar").size(100.0, 20.0).rotate(FRAC_PI_4))))
            // This paints last, but has no input handlers. Scoped picking ignores it.
            .child(el().key("overlay").w_full().h_full().absolute(0.0, 0.0))
    }
    let mut rt = Runtime::new(Canvas::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.element_at(120.0, 80.0).unwrap().key(), Some("overlay"));
    assert!(rt.element_at_in("absent", 120.0, 80.0).is_none());
    rt.pointer_move(120.0, 80.0);
    rt.frame(0.0);
    rt.pointer_down(120.0, 80.0);
    rt.pointer_up(120.0, 80.0);
    rt.frame(0.0);
    assert_eq!(rt.state.picked.as_deref(), Some("bar"));
    assert!(!rt.contains("canvas/bar", 120.0, 80.0));
}

#[test]
fn picking_uses_raise_cache_scale_origin_offset_and_ancestor_clip() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().w_full().h_full()
            .child(col().key("clip").size(100.0, 100.0).items_start().clip_rounded(0.0)
                .child(el().key("raised").size(80.0, 40.0).scale(2.0).origin(0.0, 0.0)
                    .offset(20.0, 20.0).raise().cache()))
            .child(el().key("later").size(100.0, 100.0).absolute(0.0, 0.0))
            .child(el().key("hidden").w_full().h_full().opacity(0.0).absolute(0.0, 0.0))
    }
    let mut rt = Runtime::new((), view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.element_at(90.0, 50.0).unwrap().key(), Some("raised"));
    assert!(!rt.contains("raised", 110.0, 50.0), "ancestor clip cuts the scaled child");
    assert!(!rt.contains("hidden", 50.0, 50.0));
    assert!(rt.contains("raised", 30.0, 30.0), "no fill is still pickable geometry");
}

#[test]
fn a_shadow_colour_only_step_settles() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        el().key("a").size(40.0, 40.0).motion(Spring::new(0.3, 0.0))
            .box_shadow(Shadow { color: if *on { Color::WHITE } else { Color::BLACK }, ..sh(2.0) })
    }
    let mut rt = Runtime::new(false, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "a"));
    assert_ne!(rt.presented_pose("a").unwrap().shadows()[0].color, Color::WHITE);
    settle(&mut rt);
    assert!(!moving(&rt, "a"));
    assert!(!rt.presented_pose("a").unwrap().moving());
}

#[test]
fn an_icon_turn_only_step_settles() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        static DOT: rune::Icon = rune::Icon { name: "dot", view: 24.0, body: "<circle cx=\"12\" cy=\"12\" r=\"8\"/>" };
        rune::icon(&DOT, 40.0).key("a").motion(Spring::new(0.3, 0.0))
            .icon_rotation(if *on { 1.0 } else { 0.0 })
    }
    let mut rt = Runtime::new(false, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "a"));
    assert!(rt.presented_pose("a").unwrap().moving());
    settle(&mut rt);
    assert!(!moving(&rt, "a"));
    assert!(!rt.presented_pose("a").unwrap().moving());
}

#[test]
fn pose_and_element_moving_agree_for_loops_and_an_anchors_shadow() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        col().w_full().h_full()
            .child(el().key("anchor").size(40.0, 40.0).motion(Spring::new(0.3, 0.0))
                .box_shadow(sh(if *on { 12.0 } else { 2.0 })))
            .child(el().key("popup").size(20.0, 20.0).above("anchor", 0.0))
            .child(el().key("loop").size(20.0, 20.0).looping(rune::LoopSpec::rotation(1.0)))
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 60.0);
    assert!(moving(&rt, "popup"));
    assert!(rt.presented_pose("popup").unwrap().moving());
    settle(&mut rt);
    assert!(!moving(&rt, "popup"));
    assert!(!rt.presented_pose("popup").unwrap().moving());
    assert!(moving(&rt, "loop"));
    assert!(rt.presented_pose("loop").unwrap().moving());
}

#[test]
fn a_turned_ancestor_clips_its_child_in_layer_coordinates() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full().padding(50.0).items_start().child(
            el().key("clip").size(100.0, 100.0).rotate(FRAC_PI_4).clip_rounded(0.0)
                .child(el().key("child").size(200.0, 100.0).absolute(0.0, 0.0)),
        )
    }
    let mut rt = Runtime::new((), view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(rt.contains("child", 100.0, 100.0));
    assert!(!rt.contains("child", 149.5, 149.5), "inside child but outside the turned parent clip");
    assert_eq!(rt.element_at(100.0, 100.0).unwrap().key(), Some("child"));
    assert_ne!(rt.element_at(149.5, 149.5).unwrap().key(), Some("child"));
}

#[test]
fn sticky_and_anchored_elements_pick_in_paint_order_after_scrolling() {
    fn view(_: &Hits, _: &mut Cx<Hits>) -> El {
        col().w_full().h_full().child(
            col().key("scroll").size(100.0, 100.0).scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().w_full().h(300.0)
                    .child(el().key("header").w_full().h(30.0).sticky_top(0.0)
                        .on_press(|s: &mut Hits| s.0.push("header")))
                    .child(el().key("row").w_full().h(270.0)
                        .on_press(|s: &mut Hits| s.0.push("row")))
                    .child(el().key("popup").size(60.0, 20.0)
                        .anchored("header", rune::Side::Bottom, rune::Align::Start, 0.0)
                        .on_press(|s: &mut Hits| s.0.push("popup")))),
        )
    }
    let mut rt = Runtime::new(Hits::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_y("scroll", 20.0));
    rt.frame(0.0);
    for (y, expected) in [(15.0, "header"), (35.0, "popup"), (65.0, "row")] {
        assert_eq!(rt.element_at(20.0, y).unwrap().key(), Some(expected));
        rt.state.0.clear();
        rt.pointer_down(20.0, y);
        rt.pointer_up(20.0, y);
        rt.frame(0.0);
        assert_eq!(rt.state.0, [expected]);
    }
}

#[test]
fn pick_transparent_finds_opacity_zero_elements_but_not_exiting_ones() {
    #[derive(Default)]
    struct S { plain: Option<String>, with: Option<String>, gone: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        el().w_full().h_full().child(
            col().key("canvas").w_full().h_full().items_start()
                .child(el().key("clear").size(50.0, 50.0).opacity(0.0))
                .child(el().key("parent").size(50.0, 50.0).opacity(0.0)
                    .child(el().key("kid").size(20.0, 20.0)))
                .children((!s.gone).then(|| el().key("leaver").size(50.0, 50.0).fill(Color::WHITE)
                    .transition(rune::Transition::opacity()))),
        )
    }
    let mut rt = Runtime::new(S::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let on = rune::Pick::new().transparent(true);
    let key = |p: Option<rune::Picked>| p.and_then(|p| p.key().map(str::to_owned));
    // `clear` is at y 0..50, `parent` (and `kid`) at 50..100.
    assert_ne!(key(rt.element_at(10.0, 10.0)).as_deref(), Some("clear"));
    assert_eq!(key(rt.element_at_with(10.0, 10.0, rune::Pick::default())).as_deref(), key(rt.element_at(10.0, 10.0)).as_deref());
    assert_eq!(key(rt.element_at_with(10.0, 10.0, on)).as_deref(), Some("clear"));
    assert_eq!(key(rt.element_at_in_with("canvas", 10.0, 10.0, on)).as_deref(), Some("clear"));
    assert_ne!(key(rt.element_at_in("canvas", 10.0, 10.0)).as_deref(), Some("clear"));
    assert!(!rt.contains("clear", 10.0, 10.0));
    assert!(rt.contains_with("clear", 10.0, 10.0, on));
    // A child of an opacity-0 parent inherits 0.
    assert_ne!(key(rt.element_at(10.0, 60.0)).as_deref(), Some("kid"));
    assert_eq!(key(rt.element_at_with(10.0, 60.0, on)).as_deref(), Some("kid"));
    assert!(rt.contains_with("kid", 10.0, 60.0, on) && !rt.contains("kid", 10.0, 60.0));

    // The Tx path.
    fn read(s: &mut S, with: bool, tx: &mut rune::Tx) {
        let on = rune::Pick::new().transparent(with);
        let f = move |s: &mut S, p: Option<rune::Picked>, _: &mut rune::Tx| {
            let k = p.and_then(|p| p.key().map(str::to_owned));
            if with { s.with = k } else { s.plain = k }
        };
        let _ = s;
        tx.element_at_in_with("canvas", 10.0, 10.0, on, f);
    }
    rt.update(|s, tx| { read(s, false, tx); read(s, true, tx); });
    rt.frame(0.0);
    assert_ne!(rt.state.plain.as_deref(), Some("clear"));
    assert_eq!(rt.state.with.as_deref(), Some("clear"));

    // An exiting element is skipped even with the option.
    let (x, y) = (10.0, 110.0);
    assert_eq!(key(rt.element_at_with(x, y, on)).as_deref(), Some("leaver"));
    rt.update(|s, _| s.gone = true);
    rt.frame(1.0 / 60.0);
    assert_ne!(key(rt.element_at_with(x, y, on)).as_deref(), Some("leaver"));
    assert!(!rt.contains_with("leaver", x, y, on));
}
