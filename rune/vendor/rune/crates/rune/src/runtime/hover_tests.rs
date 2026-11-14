use super::*;
use crate::{col, el};

struct State { custom: bool, growth: bool, rows: usize }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let rows = (0..s.rows).map(|i| {
        let row = el().key(format!("row-{i}")).w_full().h(30.0)
            .hover(Color::WHITE).on_press(|_: &mut State| {});
        let group = col().key(format!("group-{i}")).w_full().hover_group()
            .child(row)
            .child(el().key("action").size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0));
        if s.custom {
            // Explicit animations take the original, unconditional update path.
            group.hover_motion_within(Anim::INSTANT, rune_motion::Curve::tween(0.1, rune_motion::Easing::EASE_OUT))
        } else { group }
    });
    let stack = col().key("rows").w_full().children(rows);
    let stack = if s.growth { stack.growth(GrowthSpec::default()) } else { stack };
    col().w_full().h_full().child(
        col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false })
            .child(stack),
    )
}

#[test]
fn default_hover_matches_explicit_trajectories_through_scroll_and_reversal() {
    for (reduced, growth) in [(false, false), (true, false), (false, true), (true, true)] {
        let make = |custom| Runtime::new(State { custom, growth, rows: 30 }, view, (240.0, 150.0), Env::new(true, 2.0));
        let mut fast = make(false);
        let mut reference = make(true);
        for rt in [&mut fast, &mut reference] {
            rt.set_system_reduced_motion(reduced);
            rt.frame(0.0);
        }
        for frame in 0..120 {
            for rt in [&mut fast, &mut reference] {
                if growth && frame % 12 == 0 { rt.update(|s, _| s.rows += 1); }
                if frame == 0 { rt.pointer_move(60.0, 65.0); }
                if frame < 60 { rt.wheel(60.0, 65.0, if frame < 30 { -7.0 } else { 7.0 }); }
                if frame == 15 { rt.pointer_down(60.0, 65.0); }
                if frame == 18 { rt.pointer_up(60.0, 65.0); }
                if frame == 65 { rt.pointer_leave(); }
                if frame == 67 { rt.pointer_move(60.0, 65.0); }
                if frame == 90 { rt.pointer_leave(); }
                rt.frame(1.0 / 120.0);
            }
            assert_eq!(bytemuck::cast_slice::<_, u8>(&fast.instances), bytemuck::cast_slice::<_, u8>(&reference.instances),
                "same painted hover at frame {frame}, reduced={reduced}, growth={growth}");
        }
    }
}

#[test]
fn unchanged_default_hover_does_not_dirty_scene_poses() {
    for custom in [false, true] {
        let mut rt = Runtime::new(State { custom, growth: false, rows: 30 }, view, (240.0, 150.0), Env::new(true, 1.0));
        rt.frame(0.0);
        // Drain any requests made by the initial paint's interaction update.
        rt.scene.frame();
        rt.update_interaction(Some(false));
        assert_eq!(!rt.scene.frame().changed.is_empty(), custom,
            "only explicit animations may dirty poses for unchanged targets");
    }
}

#[test]
fn inherited_hover_curves_leave_descendants_visuals_shared() {
    let mut rt = Runtime::new(State { custom: true, growth: false, rows: 3 }, view, (240.0, 150.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, _| s.rows += 1);
    rt.frame(1.0 / 120.0);
    let rows: Vec<_> = rt.nodes.iter().filter(|n| n.path.rsplit('/').next().is_some_and(|k| k.starts_with("row-"))).collect();
    assert_eq!(rows.len(), 4);
    for n in rows {
        assert!(n.hover_motion().is_some(), "{} goes by its group's curves", n.path);
        assert!(n.vis.cold.is_empty(), "{} keeps no cold visuals of its own", n.path);
    }
}

/// A face in a hoverable row: on the face only the face shows
/// hover, and the row's wash and its group styles rest; back on the row they light again.
/// The pointer moving between them only paints.
#[test]
fn a_hover_alone_child_keeps_the_row_around_it_unlit() {
    struct S;
    fn view(_: &S, _: &mut Cx<S>) -> El {
        let face = el().key("face").size(20.0, 20.0).hover(Color::WHITE).hover_alone();
        let row = crate::row().key("row").size(200.0, 30.0).hover(Color::WHITE)
            .child(el().key("label").size(100.0, 30.0))
            .child(el().key("dot").size(4.0, 4.0).fill(Color::WHITE).group_opacity(0.0, 1.0))
            .child(face);
        col().w_full().h_full().child(row)
    }
    let mut rt = Runtime::new(S, view, (240.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let id = |rt: &Runtime<S>, key: &str| rt.nodes.iter().find(|n| n.path.rsplit('/').next() == Some(key)).map(|n| n.id).unwrap();
    let lit = |rt: &Runtime<S>, key: &str| rt.scene.store().target(id(rt, key), P_HOVER).unwrap_or(0.0);
    let commits = rt.stats.commits;

    rt.pointer_move(50.0, 15.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((lit(&rt, "row"), lit(&rt, "face")), (1.0, 0.0), "on the label the row lights");

    rt.pointer_move(114.0, 10.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((lit(&rt, "row"), lit(&rt, "face")), (0.0, 1.0), "on the face only the face lights");

    rt.pointer_move(50.0, 15.0);
    rt.frame(1.0 / 120.0);
    assert_eq!((lit(&rt, "row"), lit(&rt, "face")), (1.0, 0.0), "back on the label the row lights again");
    assert_eq!(rt.stats.commits, commits, "hover between them never commits");
}
