use super::*;

type S = (bool, bool, bool);

/// A row whose "act" child rests at `s.0`'s level and shows on hover; `s.1` mounts a second
/// one; `s.2` turns on reduced motion.
fn view(s: &S, cx: &mut Cx<S>) -> El {
    cx.reduced_motion(s.2);
    let rest = if s.0 { 1.0 } else { 0.0 };
    let mut row = crate::row().key("row").size(200.0, 40.0).hover_group()
        .child(el().key("act").size(10.0, 10.0).fill(Color::WHITE).group_opacity(rest, 1.0));
    if s.1 {
        row = row.child(el().key("new").size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0));
    }
    col().size(200.0, 100.0).child(row)
}

fn alpha(rt: &Runtime<S>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map_or(-1.0, |e| e.opacity)
}

#[test]
fn a_rest_change_animates_with_the_element() {
    let mut rt = Runtime::new((false, false, false), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(alpha(&rt, "act"), 0.0);
    rt.update(|s, _| s.0 = true);
    let mut seen = Vec::new();
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
        seen.push(alpha(&rt, "act"));
    }
    assert!(seen[0] < 0.5, "the first frame doesn't cut to the new rest: {seen:?}");
    assert!(seen.windows(2).all(|w| w[1] >= w[0] - 1e-4), "it rises: {seen:?}");
    assert!((seen[119] - 1.0).abs() < 1e-3, "and lands: {seen:?}");
    assert!(!rt.animating);
}

#[test]
fn a_rest_change_snaps_when_instant_or_reduced() {
    let mut rt = Runtime::new((false, false, false), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, tx| { tx.instant(); s.0 = true });
    rt.frame(1.0 / 120.0);
    assert_eq!(alpha(&rt, "act"), 1.0, "an instant change lands in the frame");
    let mut rt = Runtime::new((true, false, true), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = false);
    rt.frame(1.0 / 120.0);
    assert_eq!(alpha(&rt, "act"), 0.0, "reduced motion lands in the frame");
}

#[test]
fn a_row_mounting_before_a_hovered_one_draws_its_actions_at_rest() {
    // The new row comes first in the tree but sits away from the still pointer, so every later
    // index shifts under it.
    fn rows(n: &bool, _: &mut Cx<bool>) -> El {
        let row = |k: &str| crate::row().key(k).size(200.0, 20.0).hover_group()
            .child(el().key(format!("{k}-act")).size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0));
        let mut list = col().size(200.0, 100.0);
        if *n { list = list.child(row("new").absolute(0.0, 60.0)); }
        list.child(row("old"))
    }
    let mut rt = Runtime::new(false, rows, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 10.0);
    rt.frame(1.0 / 120.0);
    let alpha = |rt: &Runtime<bool>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().opacity;
    assert_eq!(alpha(&rt, "old-act"), 1.0);
    rt.update(|n, _| *n = true);
    for f in 0..10 {
        rt.frame(1.0 / 120.0);
        assert_eq!(alpha(&rt, "new-act"), 0.0, "frame {f}");
        assert_eq!(alpha(&rt, "old-act"), 1.0, "frame {f}");
    }
}

#[test]
fn a_new_mount_draws_at_its_rest_on_its_first_frame() {
    let mut rt = Runtime::new((true, false, false), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.1 = true);
    for f in 0..30 {
        rt.frame(1.0 / 120.0);
        let a = alpha(&rt, "new");
        assert!((0.0..1.0 / 512.0).contains(&a), "frame {f}: a new child resting at 0 draws {a}");
    }
}

/// A row that drops `group_opacity` (its actions shown at rest) fades them to full on its
/// motion, and fades them back out when it takes the rest level again, rather than cutting.
#[test]
fn adding_or_dropping_group_opacity_fades() {
    fn view(on: &bool, _: &mut Cx<bool>) -> El {
        let act = el().key("act").size(10.0, 10.0).fill(Color::WHITE);
        let act = if *on { act.group_opacity(0.0, 1.0) } else { act };
        col().size(200.0, 100.0).child(crate::row().key("row").size(200.0, 40.0).hover_group().child(act))
    }
    let alpha = |rt: &Runtime<bool>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("act")).unwrap().opacity;
    let mut rt = Runtime::new(true, view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(alpha(&rt), 0.0);
    for (on, from, to) in [(false, 0.0, 1.0), (true, 1.0, 0.0)] {
        rt.update(|s, _| *s = on);
        let mut seen = Vec::new();
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
            seen.push(alpha(&rt));
        }
        assert!((seen[0] - from).abs() < 0.5, "the first frame doesn't cut to {to}: {seen:?}");
        assert!(seen.windows(2).all(|w| (w[1] - w[0]).abs() < 0.2), "it fades: {seen:?}");
        assert!((seen[119] - to).abs() < 1e-3, "and lands on {to}: {seen:?}");
    }
    assert!(!rt.animating);
}

/// A dropped group opacity's fade to full leaves nothing behind once it lands, with no further
/// commit: no rest track, nothing that paints the row in tree order. An instant opacity motion
/// drops it at once.
#[test]
fn a_dropped_group_opacity_leaves_nothing_once_it_lands() {
    fn view(s: &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        let mut act = el().key("act").size(10.0, 10.0).fill(Color::WHITE);
        if s.0 { act = act.group_opacity(0.0, 1.0); }
        if s.1 { act = act.opacity_motion(Anim::INSTANT); }
        col().size(200.0, 100.0).child(crate::row().key("row").size(200.0, 40.0).hover_group().child(act))
    }
    let left = |rt: &Runtime<(bool, bool)>| {
        let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("act")).unwrap();
        (n.vis.group_opacity, rt.scene.store().has(n.id, P_GROUP_REST), n.group_fade)
    };
    let mut rt = Runtime::new((true, false), view, (200.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.0 = false);
    rt.frame(1.0 / 120.0);
    assert_eq!(left(&rt), (None, true, true), "the fade to full runs");
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    assert!(!rt.animating);
    assert_eq!(left(&rt), (None, false, false), "and leaves nothing once it lands");

    rt.update(|s, _| *s = (true, true));
    rt.frame(1.0 / 120.0);
    rt.update(|s, _| s.0 = false);
    rt.frame(1.0 / 120.0);
    assert_eq!(left(&rt), (None, false, false), "an instant motion drops it at once");
}
