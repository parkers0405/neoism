use super::*;
use crate::{col, el, text};

struct State { dark: bool }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let (bg, fg) = if s.dark { (Color::BLACK, Color::WHITE) } else { (Color::WHITE, Color::BLACK) };
    col().w_full().h_full().fill(bg).children((0..12).map(|i| {
        el().key(format!("row-{i}")).w_full().h(20.0).fill(bg).radius(4.0).opacity(0.9)
            .border(1.0, fg).shadow(2.0, 6.0, fg).outline(1.0, 1.0, fg)
            .hover(fg.alpha(0.1)).press(fg.alpha(0.2)).press_scale(0.97)
            .child(text(format!("row {i}")).color(fg))
            .child(el().w(8.0).h(8.0).gradient(bg, fg))
    }))
}

fn settled(dark: bool) -> Runtime<State> {
    let mut rt = Runtime::new(State { dark }, view, (200.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let mut n = 0;
    while n < 240 && rt.frame(1.0 / 120.0) { n += 1; }
    rt.scene.frame();
    rt
}

fn row(rt: &Runtime<State>) -> &Node {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some("row-3")).unwrap()
}

fn label(rt: &Runtime<State>) -> &Node {
    let row = rt.nodes.iter().position(|n| n.key.as_deref() == Some("row-3")).unwrap();
    rt.nodes.iter().find(|n| n.parent == Some(row) && n.text.is_some()).unwrap()
}

#[test]
fn a_static_tree_keeps_no_motion_tracks() {
    let mut rt = settled(false);
    assert_eq!(rt.scene.store().track_len(), 0, "nothing here has moved");
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.scene.store().track_len(), 0, "an unchanged rebuild makes none either");
}

#[test]
fn an_unchanged_commit_leaves_the_scene_clean() {
    let mut rt = settled(false);
    let tx = rt.transaction();
    rt.commit(&tx);
    assert!(rt.scene.frame().changed.is_empty(), "no node presents again");
}

#[test]
fn a_first_colour_change_still_crossfades_from_the_drawn_colour() {
    let mut rt = settled(false);
    rt.update(|s, _| s.dark = true);
    rt.frame(1.0 / 120.0);
    rt.frame(1.0 / 120.0);
    let n = row(&rt);
    let fill = rt.color(n.id, P_FILL, n.vis.fill);
    assert!(fill.r < 1.0 && fill.r > 0.0, "the fill is between white and black: {fill:?}");
    let l = label(&rt);
    let text = rt.color(l.id, P_TEXT, l.text.as_ref().unwrap().color);
    assert!(text.r > 0.0 && text.r < 1.0, "the text is between black and white: {text:?}");
    let border = rt.color(n.id, P_BORDER, n.vis.border());
    assert!(border.r > 0.0 && border.r < 1.0, "the border is between black and white: {border:?}");
    let shadow = rt.color(n.id, P_SHADOWS[0], n.vis.shadows[0].color);
    assert!(shadow.r > 0.0 && shadow.r < 1.0, "the shadow is between black and white: {shadow:?}");
    let mut k = 0;
    while k < 240 && rt.frame(1.0 / 120.0) { k += 1; }
    let n = row(&rt);
    assert_eq!(rt.color(n.id, P_FILL, n.vis.fill), Color::BLACK);
    assert_eq!(rt.color(n.id, P_BORDER, n.vis.border()), Color::WHITE);
}

#[test]
fn an_instant_colour_change_lands_without_a_track() {
    let mut rt = settled(false);
    rt.update(|s, tx| { s.dark = true; tx.instant(); });
    rt.frame(1.0 / 120.0);
    let n = row(&rt);
    assert_eq!(rt.color(n.id, P_FILL, n.vis.fill), Color::BLACK);
    assert_eq!(rt.scene.store().track_len(), 0);
}

#[test]
fn rebuilding_reuses_node_paths_and_restores_on_layout_retry() {
    #[derive(Default)]
    struct RetryState { open: bool, tick: u32 }
    fn retry_view(s: &RetryState, cx: &mut Cx<RetryState>) -> El {
        let measured = cx.layout_size("box").map_or(10.0, |[w, _]| w * 0.5);
        let mut sheet = crate::row().key("sheet").w(60.0).h(40.0);
        if s.open {
            sheet = sheet
                .child(el().key("btn").w(30.0).h(20.0).focusable(true))
                .child(el().size(0.0, 0.0).focus_return("anchor"));
        }
        col().w_full().h_full()
            .child(el().key("box").w(if s.tick == 0 { 80.0 } else { 120.0 }).h(measured).child(text(format!("{}", s.tick))))
            .child(el().key("anchor").w(40.0).h(20.0).focusable(true))
            .child(sheet)
    }
    let mut rt = Runtime::new(RetryState { open: true, tick: 0 }, retry_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let ptr0 = rt.nodes.iter().find(|n| n.key.as_deref() == Some("anchor")).unwrap().path.as_ptr();
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    let ptr1 = rt.nodes.iter().find(|n| n.key.as_deref() == Some("anchor")).unwrap().path.as_ptr();
    assert_eq!(ptr0, ptr1, "steady-state rebuild moves the previous commit's path String in place");

    // Trigger both a layout-read settle retry (`tick = 1` changes `box` width) and `return_focus_to`
    // under `sheet` (whose own path was moved on the previous commit).
    let btn_path = rt.nodes.iter().find(|n| n.key.as_deref() == Some("btn")).unwrap().path.clone();
    rt.focus.focus = crate::runtime::focus_machine::Focus::On { path: btn_path, hovered: false, from: None };
    rt.update(|s, _| { s.open = false; s.tick = 1; });
    rt.frame(1.0 / 120.0);
    let anchor_path = rt.nodes.iter().find(|n| n.key.as_deref() == Some("anchor")).unwrap().path.clone();
    assert_eq!(rt.focus.path(), Some(anchor_path.as_str()));
}

#[test]
fn changing_one_sibling_skips_resting_siblings_without_drift() {
    struct SibState { active: usize }
    fn sib_view(s: &SibState, _: &mut Cx<SibState>) -> El {
        col().w_full().h_full().children((0..8).map(|i| {
            let fg = if i == s.active { Color::WHITE } else { Color::BLACK };
            el().key(format!("r-{i}")).w_full().h(20.0).fill(fg).hover(fg.alpha(0.2)).focusable(true)
                .child(text(format!("item {i}")).color(fg))
        }))
    }
    let mut rt = Runtime::new(SibState { active: 0 }, sib_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    rt.pointer_move(20.0, 70.0); // hover row 3
    rt.frame(1.0 / 120.0);
    while rt.frame(1.0 / 120.0) {}

    rt.update(|s, _| s.active = 5);
    rt.frame(1.0 / 120.0);
    let r5 = rt.nodes.iter().find(|n| n.key.as_deref() == Some("r-5")).unwrap().id;
    let r2 = rt.nodes.iter().find(|n| n.key.as_deref() == Some("r-2")).unwrap().id;
    assert!(rt.scene.store().is_node_animating(r5), "changed sibling animates its colour");
    assert!(!rt.scene.store().is_node_animating(r2), "unchanged sibling stays at rest with no track");
    while rt.frame(1.0 / 120.0) {}

    let mut fresh = Runtime::new(SibState { active: 5 }, sib_view, (200.0, 200.0), Env::new(true, 1.0));
    fresh.frame(0.0);
    while fresh.frame(1.0 / 120.0) {}
    for k in 0..8 {
        let key = format!("r-{k}");
        let na = rt.nodes.iter().find(|n| n.key.as_deref() == Some(key.as_str())).unwrap();
        let nb = fresh.nodes.iter().find(|n| n.key.as_deref() == Some(key.as_str())).unwrap();
        assert_eq!(rt.color(na.id, P_FILL, na.vis.fill), fresh.color(nb.id, P_FILL, nb.vis.fill));
    }
}
