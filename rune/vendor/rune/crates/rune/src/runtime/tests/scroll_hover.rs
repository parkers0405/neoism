use super::*;

/// Forty 20-point rows in a 200-point viewport, each revealing an action on hover.
fn view(_: &(), _: &mut Cx<()>) -> El {
    let rows = col().key("rows").w_full().children((0..40).map(|k| crate::row().key(format!("r{k}")).w_full().h(20.0).hover_group()
        .child(el().key(format!("a{k}")).size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0))));
    col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

/// The rows whose action shows at all this frame.
fn shown(rt: &Runtime<()>) -> Vec<String> {
    rt.elements().into_iter().filter(|e| e.key.as_ref().is_some_and(|k| k.starts_with('a')) && e.opacity > 0.0)
        .filter_map(|e| e.key).collect()
}

fn hovering() -> Runtime<()> {
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 110.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(shown(&rt), ["a5"]);
    rt
}

#[test]
fn a_glide_holds_a_still_pointers_hover_until_it_stops() {
    let mut rt = hovering();
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 400.0));
    let mut frames = 0;
    for f in 0..600 {
        rt.frame(1.0 / 120.0);
        if rt.scrolls.values().all(|s| s.glide().is_none()) { break; }
        frames += 1;
        assert_eq!(shown(&rt), ["a5"], "frame {f}: no row passing under the pointer takes its hover");
    }
    assert!(frames > 5, "it glided");
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    assert_eq!(shown(&rt), ["a25"], "once it stops, the row under the pointer has it");
}

#[test]
fn moving_the_pointer_during_a_glide_hovers_again() {
    let mut rt = hovering();
    rt.update(|_, tx| tx.scroll_to_bottom("scroll"));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    assert_eq!(shown(&rt), ["a5"]);
    rt.pointer_move(100.0, 111.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    // Hover paints a frame behind the moving rows, as with a wheel scroll.
    let offset = rt.scrolls.values().next().unwrap().offset as f32;
    let under = ((offset + 111.0) / 20.0) as usize;
    let rows: Vec<usize> = shown(&rt).iter().map(|k| k[1..].parse().unwrap()).collect();
    assert!(rows.iter().any(|&k| k != 5 && k + 2 >= under && k <= under), "the moved pointer hovers near row {under}: {rows:?}");
}

#[test]
fn a_wheel_scroll_still_hovers_each_row_it_brings_under_the_pointer() {
    let mut rt = hovering();
    rt.wheel(100.0, 110.0, -40.0);
    // Hover retargets from the frame's drawn rows and paints on the next.
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    let offset = rt.scrolls.values().next().unwrap().offset as f32;
    assert!(offset > 0.0);
    let under = format!("a{}", ((offset + 110.0) / 20.0) as usize);
    assert!(shown(&rt).contains(&under), "{under}: {:?}", shown(&rt));
}

#[test]
fn a_hover_a_glide_carried_out_of_view_lets_go_at_once() {
    let mut rt = hovering();
    let id = rt.nodes.iter().find(|n| n.named("r5")).unwrap().id;
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 400.0));
    for f in 0..600 {
        rt.frame(1.0 / 120.0);
        if rt.scene.store().target(id, P_HOVER) == Some(0.0) {
            assert!(rt.scrolls.values().all(|s| s.glide().is_none()), "frame {f}: the hold kept r5's hover through the glide");
            assert!(!rt.scene.store().is_animating(id, P_HOVER), "r5, out of view, lands its leave instead of fading it unseen");
            return;
        }
    }
    panic!("r5 never let go of its hover");
}

/// Forty rows, each darkening while pressed and gliding twenty rows on when pressed.
fn pressing(_: &(), _: &mut Cx<()>) -> El {
    let rows = col().key("rows").w_full().children((0..40).map(|k| crate::row().key(format!("r{k}")).w_full().h(20.0)
        .press(Color::WHITE).press_scale(0.9).on_press_tx(|_: &mut (), tx: &mut Tx| tx.scroll_to("scroll", 0.0, 400.0))));
    col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

#[test]
fn a_press_that_starts_a_glide_still_lets_go() {
    let mut rt = Runtime::new((), pressing, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 110.0);
    rt.frame(1.0 / 120.0);
    rt.pointer_down(100.0, 110.0);
    rt.frame(1.0 / 120.0);
    let id = rt.nodes.iter().find(|n| n.named("r5")).unwrap().id;
    assert_eq!(rt.scene.store().target(id, P_PRESS), Some(1.0));
    assert_eq!(rt.scene.store().target(id, P_PRESS_SCALE), Some(0.9));
    rt.pointer_up(100.0, 110.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.hover_hold.held, "the press's glide holds hover");
    assert_eq!(rt.scene.store().target(id, P_PRESS), Some(0.0), "the press lets go mid-glide");
    assert_eq!(rt.scene.store().target(id, P_PRESS_SCALE), Some(1.0), "the press scale springs back mid-glide");
}

#[test]
fn the_hold_ends_with_its_glide() {
    let mut rt = hovering();
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 400.0));
    let mut held = false;
    for _ in 0..600 {
        rt.frame(1.0 / 120.0);
        held |= rt.hover_hold.held;
        if rt.scrolls.values().all(|s| s.glide().is_none()) { break; }
    }
    assert!(held, "the glide held hover");
    assert!(!rt.hover_hold.held && !rt.hover_hold.released, "no stale hold is left for a later glide");
}

/// Two viewports side by side, and a modal over both while `open`.
fn pair(open: &bool, _: &mut Cx<bool>) -> El {
    let side = |key: &str| col().key(key).size(100.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false })
        .child(col().w_full().children((0..40).map(|k| crate::row().key(format!("{key}{k}")).w_full().h(20.0).hover_group()
            .child(el().key(format!("a{key}{k}")).size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0)))));
    let root = crate::row().size(200.0, 200.0).child(side("l")).child(side("r"));
    if *open { root.child(col().key("sheet").absolute(150.0, 150.0).size(40.0, 40.0).modal(true, |s: &mut bool| *s = false)) } else { root }
}

fn gliding_pair() -> Runtime<bool> {
    let mut rt = Runtime::new(false, pair, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 110.0);
    rt.frame(1.0 / 120.0);
    rt.update(|_, tx| { tx.scroll_to("l", 0.0, 400.0); tx.scroll_to("r", 0.0, 400.0); });
    // Until a row other than the hovered one glides under the pointer.
    for _ in 0..30 {
        rt.frame(1.0 / 120.0);
        if rt.hover_hold.held { return rt; }
    }
    panic!("the glides held hover");
}

#[test]
fn a_hand_scroll_anywhere_lets_go_of_the_hold() {
    let mut rt = gliding_pair();
    rt.wheel(150.0, 110.0, -10.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.scrolls.values().any(|s| s.glide().is_some()), "the viewport under the pointer still glides");
    assert!(rt.hover_hold.released && !rt.hover_hold.held, "a wheel over the other viewport lets hover follow the pointer again");
}

#[test]
fn a_viewport_under_a_modal_holds_no_hover() {
    let mut rt = gliding_pair();
    rt.update(|s, _| *s = true);
    rt.frame(1.0 / 120.0);
    assert!(rt.scrolls.values().any(|s| s.glide().is_some()));
    assert!(!rt.hover_hold.held, "a modal makes the gliding viewport inert");
}

/// Two hundred virtual rows, each recording its `on_hover_change`.
fn hover_rows(_: &Vec<(usize, bool)>, _: &mut Cx<Vec<(usize, bool)>>) -> El {
    let rows = col().key("rows").w_full().growth(crate::el::GrowthSpec::default()).virtualize(20.0).children((0..200).map(|k| crate::row().key(format!("r{k}")).w_full().h(20.0)
        .on_hover_change(move |s: &mut Vec<(usize, bool)>, inside, _| s.push((k, inside)))));
    col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

#[test]
fn a_row_leaving_the_tree_under_a_held_glide_hears_the_pointer_leave() {
    let mut rt = Runtime::new(Vec::new(), hover_rows, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 110.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state, [(5, true)]);
    rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 3000.0));
    rt.frame(1.0 / 120.0);
    assert!(rt.hover_hold.held);
    assert!(!rt.nodes.iter().any(|n| n.named("r5") && !rt.is_exiting(n.id)), "the glide's cut unmounts the row");
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        if rt.state.contains(&(5, false)) { break; }
    }
    assert!(rt.state.contains(&(5, false)), "the row heard the leave before it was pruned: {:?}", rt.state);
    assert!(rt.hover_hold.held, "while the glide still holds hover");
}

/// Forty 20-point rows, each revealing an action on hover and recording its `on_hover_change`.
fn logged_rows(_: &Vec<(usize, bool)>, _: &mut Cx<Vec<(usize, bool)>>) -> El {
    let rows = col().key("rows").w_full().children((0..40).map(|k| crate::row().key(format!("r{k}")).w_full().h(20.0).hover_group()
        .on_hover_change(move |s: &mut Vec<(usize, bool)>, inside, _| s.push((k, inside)))
        .child(el().key(format!("a{k}")).size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0))));
    col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

#[test]
fn an_app_stepping_its_own_glide_holds_a_still_pointers_hover_until_it_stops() {
    let mut rt = Runtime::new(Vec::new(), logged_rows, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 110.0);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state, [(5, true)]);
    // The app glides 400 points itself, an instant scroll each frame.
    let mut flashes = 0;
    for f in 1..=100 {
        rt.update(move |_, tx| tx.scroll_to_instant("scroll", 0.0, 4.0 * f as f32));
        rt.frame(1.0 / 120.0);
        let shown: Vec<String> = rt.elements().into_iter().filter(|e| e.key.as_ref().is_some_and(|k| k.starts_with('a')) && e.opacity > 0.0)
            .filter_map(|e| e.key).collect();
        flashes += usize::from(shown != ["a5"]);
    }
    let commits = rt.state.len() - 1;
    assert_eq!((commits, flashes), (0, 0), "no hover commits or chrome under the still pointer mid-scroll: {:?}", rt.state);
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.state, [(5, true), (5, false), (25, true)], "once it stops, hover resolves once at the pointer");
    assert!(!rt.hover_hold.held, "and lets go");
}

/// What forty rows' hover handlers heard: `on_hover_change` and `on_hover`.
#[derive(Default)]
struct Heard {
    changes: Vec<(usize, bool)>,
    enters: Vec<usize>,
}

fn heard_rows(_: &Heard, _: &mut Cx<Heard>) -> El {
    let rows = col().key("rows").w_full().children((0..40).map(|k| crate::row().key(format!("r{k}")).w_full().h(20.0)
        .on_hover_change(move |s: &mut Heard, inside, _| s.changes.push((k, inside)))
        .on_hover(move |s: &mut Heard| s.enters.push(k))));
    col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

#[test]
fn a_glide_to_an_offset_runs_no_hover_handler_under_a_still_pointer_until_it_lands() {
    for spring in [None, Some(crate::Spring::new(0.18, 0.0))] {
        let mut rt = Runtime::new(Heard::default(), heard_rows, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.pointer_move(100.0, 110.0);
        rt.frame(1.0 / 120.0);
        rt.frame(1.0 / 120.0);
        assert_eq!((&rt.state.changes[..], &rt.state.enters[..]), (&[(5, true)][..], &[5][..]));
        rt.update_if(move |_, tx| {
            match spring {
                Some(spring) => tx.scroll_to_with("scroll", 0.0, 400.0, spring),
                None => tx.scroll_to("scroll", 0.0, 400.0),
            }
            false
        });
        let commits = rt.stats.commits;
        let mut frames = 0;
        for _ in 0..600 {
            rt.frame(1.0 / 120.0);
            if rt.scrolls.values().all(|s| s.glide().is_none()) { break; }
            frames += 1;
        }
        assert!(frames > 5, "{spring:?}: it glided");
        assert_eq!(rt.stats.commits, commits, "{spring:?}: no hover handler ran mid-glide: {:?}", rt.state.changes);
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        assert_eq!(rt.state.changes, [(5, true), (5, false), (25, true)], "{spring:?}: once it lands, hover resolves once at the pointer");
        assert_eq!(rt.state.enters, [5, 25], "{spring:?}");
        assert!(!rt.hover_hold.held);
    }
}
