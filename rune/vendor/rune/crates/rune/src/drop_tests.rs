//! `El::on_drop_files`: files dragged from another app reach the innermost element under the
//! drop that accepts them, in one call, with a drop hover while the drag is over it.
use super::*;
use crate::{Cx, El, Env, col, el};
use std::path::PathBuf;

#[derive(Default)]
struct Drops {
    /// Each `on_drop_files` call: the target and the files it got.
    dropped: Vec<(&'static str, Vec<String>)>,
    /// Each `on_drop_hover` call.
    hover: Vec<(&'static str, bool)>,
    root_takes: bool,
    /// The inner zone is gone, and a modal that takes drops is open.
    no_inner: bool,
    modal: bool,
}

fn zone(key: &'static str) -> El {
    el().key(key)
        .on_drop_files(move |s: &mut Drops, files, _| s.dropped.push((key, files.iter().map(|f| f.display().to_string()).collect())))
        .on_drop_hover(move |s: &mut Drops, over, _| s.hover.push((key, over)))
}

/// An outer zone that takes anything around an inner one that takes pictures, in a 400 by 300
/// window whose root takes drops when `root_takes`.
fn view(s: &Drops, _: &mut Cx<Drops>) -> El {
    let outer = zone("outer").size(200.0, 200.0);
    let outer = if s.no_inner { outer } else { outer.child(zone("inner").size(100.0, 100.0).accepts_extensions(&["png", ".JPG"])) };
    let mut root = col().w_full().h_full().child(outer);
    if s.modal {
        root = root.child(zone("modal").absolute(100.0, 50.0).size(200.0, 200.0).modal(false, |s: &mut Drops| s.modal = false));
    }
    if s.root_takes { root.on_drop_files(|s: &mut Drops, files, _| s.dropped.push(("root", files.iter().map(|f| f.display().to_string()).collect()))) } else { root }
}

fn run(root_takes: bool) -> Runtime<Drops> {
    let mut rt = Runtime::new(Drops { root_takes, ..Default::default() }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn files(names: &[&str]) -> Vec<PathBuf> { names.iter().map(PathBuf::from).collect() }

fn drop_at(rt: &mut Runtime<Drops>, names: &[&str], at: (f32, f32)) {
    rt.drag_files(files(names), Some(at));
    rt.drop_files(files(names), Some(at));
    rt.frame(1.0 / 60.0);
}

fn dropped(rt: &Runtime<Drops>) -> Vec<(&'static str, Vec<&str>)> {
    rt.state.dropped.iter().map(|(k, f)| (*k, f.iter().map(String::as_str).collect())).collect()
}

#[test]
fn the_innermost_accepting_target_takes_the_drop() {
    let mut rt = run(true);
    drop_at(&mut rt, &["a.png"], (50.0, 50.0));
    drop_at(&mut rt, &["b.png"], (150.0, 150.0));
    assert_eq!(dropped(&rt), [("inner", vec!["a.png"]), ("outer", vec!["b.png"])]);
}

#[test]
fn files_a_target_refuses_pass_to_the_next_that_accepts_them() {
    let mut rt = run(true);
    drop_at(&mut rt, &["notes.txt"], (50.0, 50.0));
    assert_eq!(dropped(&rt), [("outer", vec!["notes.txt"])], "the inner zone takes only pictures");
    drop_at(&mut rt, &["c.png"], (300.0, 250.0));
    assert_eq!(dropped(&rt)[1], ("root", vec!["c.png"]), "a drop over no zone falls back to the root");
    rt.drop_files(files(&["d.png"]), None);
    rt.frame(2.0 / 60.0);
    assert_eq!(dropped(&rt)[2], ("root", vec!["d.png"]), "as does a drop with no position");

    let mut rt = run(false);
    drop_at(&mut rt, &["c.png"], (300.0, 250.0));
    assert_eq!(dropped(&rt), [], "with no root handler a drop nothing takes is ignored");
    assert_eq!(rt.state.hover, [], "and shows no hover");
}

#[test]
fn the_drop_hover_follows_the_drag_and_ends_when_it_leaves_or_drops() {
    let mut rt = run(false);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    assert!(rt.dragging_files());
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hover, [("inner", true)]);
    assert_eq!(rt.drop_hovered().as_deref(), Some("inner"));
    rt.drag_files_to(Some((150.0, 150.0)));
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.hover[1..], [("inner", false), ("outer", true)], "only the innermost accepting target is hovered");
    rt.drag_files_leave();
    assert!(!rt.dragging_files());
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.hover[3..], [("outer", false)], "leaving the window ends the hover");
    assert_eq!(rt.drop_hovered(), None);

    rt.state.hover.clear();
    rt.drag_files(files(&["notes.txt"]), Some((50.0, 50.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.state.hover, [("outer", true)], "a target that refuses the files isn't hovered");
    rt.drop_files(files(&["notes.txt"]), Some((50.0, 50.0)));
    rt.frame(5.0 / 60.0);
    assert_eq!(rt.state.hover, [("outer", true), ("outer", false)], "dropping ends the hover");
    assert_eq!(dropped(&rt), [("outer", vec!["notes.txt"])]);
}

#[test]
fn several_files_arrive_in_one_call() {
    let mut rt = run(false);
    drop_at(&mut rt, &["a.png", "b.JPG", "c.txt"], (50.0, 50.0));
    assert_eq!(dropped(&rt), [("inner", vec!["a.png", "b.JPG"])], "the target gets the files it accepts, together");
    drop_at(&mut rt, &["a.png", "c.txt"], (150.0, 150.0));
    assert_eq!(dropped(&rt)[1], ("outer", vec!["a.png", "c.txt"]));
}

#[test]
fn a_covered_window_takes_a_drop_on_the_tree_it_has_now() {
    let mut rt = run(true);
    rt.set_live_visible(false);
    // While covered, the window builds nothing: the inner zone unmounts only in state.
    rt.update(|s, _| s.no_inner = true);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.drop_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.hidden_wake();
    assert_eq!(dropped(&rt), [("outer", vec!["a.png"])], "not the inner zone, which has gone");
    assert_eq!(rt.state.hover, [("outer", true), ("outer", false)]);

    // Nor the zone behind a modal opened while covered (which, still fading in, takes none).
    rt.update(|s, _| s.modal = true);
    rt.drop_files(files(&["b.png"]), Some((150.0, 150.0)));
    rt.hidden_wake();
    assert_eq!(dropped(&rt).len(), 1);
}

#[test]
fn a_drop_outside_the_window_or_an_open_modal_reaches_nothing() {
    let mut rt = run(true);
    drop_at(&mut rt, &["a.png"], (450.0, 50.0));
    assert_eq!(dropped(&rt), [], "the root takes drops only inside the window");
    rt.update(|s, _| s.modal = true);
    rt.frame(2.0 / 60.0);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.drop_hovered(), None, "the dimmed window behind a modal takes no drag");
    rt.drop_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(dropped(&rt), []);
    drop_at(&mut rt, &["b.png"], (150.0, 150.0));
    assert_eq!(dropped(&rt), [("modal", vec!["b.png"])]);
    assert_eq!(rt.state.hover, [("modal", true), ("modal", false)]);
}

#[test]
fn a_hovered_target_that_unmounts_hears_its_hover_end() {
    let mut rt = run(false);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.no_inner = true);
    rt.frame(2.0 / 60.0);
    // The host samples the pointer while the drag lasts; the target under it has changed.
    assert!(rt.drag_files_to(Some((50.0, 50.0))));
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.hover, [("inner", true), ("inner", false), ("outer", true)]);
    assert_eq!(rt.drop_hovered().as_deref(), Some("outer"));
}

#[test]
fn moving_over_one_target_queues_no_frames() {
    let mut rt = run(false);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.frame(1.0 / 60.0);
    for x in 51..90 {
        assert!(!rt.drag_files_to(Some((x as f32, 50.0))), "still over the inner zone");
    }
    assert!(!rt.has_pending_input(), "so the host draws nothing");
    assert!(rt.drag_files_to(Some((150.0, 150.0))), "onto the outer zone");
    assert!(rt.has_pending_input());
}

#[test]
fn a_drop_zone_over_other_elements_leaves_their_menus_and_help() {
    #[derive(Default)]
    struct S { menus: u32 }
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().w_full().h_full()
            .child(el().key("row").size(100.0, 40.0).help("Row").on_secondary_press(|s: &mut S, _, _| s.menus += 1))
            .child(el().key("zone").absolute(0.0, 0.0).w_full().h_full().on_drop_files(|_: &mut S, _, _| {}))
    }
    let mut rt = Runtime::new(S::default(), view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.secondary_down(10.0, 10.0);
    rt.secondary_up(10.0, 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.menus, 1, "the row's context menu");
    rt.observe_pointer(10.0, 10.0);
    rt.frame(2.0 / 60.0);
    rt.frame(rt.help_delay);
    assert_eq!(rt.help_tooltip().map(|(t, _)| t), Some("Row"));
}

#[test]
fn a_covered_window_still_takes_a_drop() {
    let mut rt = run(false);
    // As the host does for `WindowEvent::Occluded(true)`, then for a drop (`covered_step`).
    rt.set_live_visible(false);
    rt.drag_files(files(&["a.png"]), Some((50.0, 50.0)));
    rt.drop_files(files(&["a.png"]), Some((50.0, 50.0)));
    assert!(rt.has_pending_input());
    rt.hidden_wake();
    assert_eq!(dropped(&rt), [("inner", vec!["a.png"])]);
    assert_eq!(rt.state.hover, [("inner", true), ("inner", false)]);
}

#[derive(Default)]
struct Picky {
    dropped: Vec<(&'static str, Vec<String>)>,
    rejected: Vec<Vec<String>>,
    hover: Vec<(&'static str, bool)>,
}

fn names(files: &[PathBuf]) -> Vec<String> { files.iter().map(|f| f.display().to_string()).collect() }

/// A zone that takes only exports, and says why it refuses anything else, inside one that takes
/// anything.
fn picky(_: &Picky, _: &mut Cx<Picky>) -> El {
    let exports = el().key("exports").size(100.0, 100.0)
        .on_drop_files(|s: &mut Picky, files, _| s.dropped.push(("exports", names(files))))
        .accepts_extensions(&["json"])
        .on_drop_hover(|s: &mut Picky, over, _| s.hover.push(("exports", over)))
        .on_drop_rejected(|s: &mut Picky, files, _| s.rejected.push(names(files)))
        .on_drop_rejected_hover(|s: &mut Picky, over, _| s.hover.push(("exports-rejected", over)));
    col().w_full().h_full().child(
        el().key("outer").size(200.0, 200.0)
            .on_drop_files(|s: &mut Picky, files, _| s.dropped.push(("outer", names(files))))
            .on_drop_hover(|s: &mut Picky, over, _| s.hover.push(("outer", over)))
            .child(exports),
    )
}

#[test]
fn a_target_that_hears_rejected_drops_stops_them_and_shows_its_own_hover() {
    let mut rt = Runtime::new(Picky::default(), picky, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.drag_files(files(&["notes.txt"]), Some((50.0, 50.0)));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hover, [("exports-rejected", true)], "not the outer zone's hover");
    assert_eq!((rt.drop_rejected_hovered().as_deref(), rt.drop_hovered()), (Some("exports"), None));
    rt.drag_files_to(Some((150.0, 150.0)));
    rt.frame(2.0 / 60.0);
    assert_eq!(rt.state.hover[1..], [("exports-rejected", false), ("outer", true)]);
    assert_eq!((rt.drop_rejected_hovered(), rt.drop_hovered().as_deref()), (None, Some("outer")));
    rt.drag_files_to(Some((50.0, 50.0)));
    rt.drop_files(files(&["notes.txt"]), Some((50.0, 50.0)));
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.state.hover[3..], [("outer", false), ("exports-rejected", true), ("exports-rejected", false)], "dropping ends it");
    assert_eq!((rt.state.rejected.as_slice(), rt.state.dropped.as_slice()), (&[vec!["notes.txt".to_owned()]][..], &[][..]));

    rt.state.hover.clear();
    rt.drag_files(files(&["a.json", "b.txt"]), Some((50.0, 50.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(rt.state.hover, [("exports", true)], "a drag carrying an export is accepted");
    rt.drop_files(files(&["a.json", "b.txt"]), Some((50.0, 50.0)));
    rt.frame(5.0 / 60.0);
    assert_eq!(rt.state.dropped, [("exports", vec!["a.json".to_owned()])]);
    assert_eq!(rt.state.rejected[1..], [vec!["b.txt".to_owned()]], "and the rest are rejected");

    rt.drag_files(files(&["a.json"]), Some((50.0, 50.0)));
    rt.frame(6.0 / 60.0);
    rt.drag_files_leave();
    rt.drag_files(files(&["c.txt"]), Some((50.0, 50.0)));
    rt.frame(7.0 / 60.0);
    assert_eq!(rt.state.hover[2..], [("exports", true), ("exports", false), ("exports-rejected", true)], "the same target swaps hovers");

    rt.drag_files_leave();
    rt.frame(8.0 / 60.0);
    rt.state.hover.clear();
    rt.drag_files(Vec::new(), Some((50.0, 50.0)));
    rt.drop_files(Vec::new(), Some((50.0, 50.0)));
    rt.frame(9.0 / 60.0);
    assert!(rt.state.hover.is_empty() && rt.drop_rejected_hovered().is_none(), "an empty drag claims no rejected hover");
}

#[test]
fn a_target_that_follows_the_drag_hears_its_point_on_the_element_and_where_it_dropped() {
    #[derive(Default)]
    struct Caret { seen: Vec<(&'static str, Option<(f32, f32)>)> }
    fn view(_: &Caret, _: &mut Cx<Caret>) -> El {
        let local = |tx: &crate::Tx| tx.drop_point().map(|p| (p.x, p.y));
        col().w_full().h_full().children([
            el().size(200.0, 20.0),
            el().key("doc").size(200.0, 100.0)
                .on_drop_files(move |s: &mut Caret, _, tx| s.seen.push(("drop", local(tx))))
                .on_drop_hover(move |s: &mut Caret, over, tx| s.seen.push((if over { "over" } else { "off" }, local(tx))))
                .on_drop_move(|s: &mut Caret, p, tx| { assert_eq!(tx.drop_point(), Some(p)); s.seen.push(("move", Some((p.x, p.y)))) }),
            el().key("plain").size(200.0, 100.0).on_drop_files(|_: &mut Caret, _, _| {}),
        ])
    }
    let mut rt = Runtime::new(Caret::default(), view, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let take = |rt: &mut Runtime<Caret>| std::mem::take(&mut rt.state.seen);
    rt.drag_files(files(&["a.txt"]), Some((30.0, 40.0)));
    rt.frame(1.0 / 60.0);
    assert_eq!(take(&mut rt), [("over", Some((30.0, 20.0))), ("move", Some((30.0, 20.0)))], "points on the element, below its 20 pt neighbour");

    assert!(!rt.drag_files_to(Some((30.0, 40.0))), "a drag that didn't move asks for nothing");
    assert!(rt.drag_files_to(Some((50.0, 60.0))), "a move over a target that follows it asks for a frame");
    assert!(!rt.drag_files_to(Some((60.0, 70.0))), "once a frame");
    rt.frame(2.0 / 60.0);
    assert_eq!(take(&mut rt), [("move", Some((60.0, 50.0)))], "at the latest point");

    assert!(rt.drag_files_to(Some((60.0, 150.0))), "onto another target");
    rt.frame(3.0 / 60.0);
    assert_eq!(take(&mut rt), [("off", None)]);
    assert!(!rt.drag_files_to(Some((70.0, 160.0))), "a target that doesn't follow the drag asks for no frames as it moves");

    rt.drag_files(files(&["a.txt"]), Some((70.0, 80.0)));
    rt.drop_files(files(&["a.txt"]), Some((70.0, 80.0)));
    rt.frame(4.0 / 60.0);
    assert_eq!(take(&mut rt), [("over", Some((70.0, 60.0))), ("move", Some((70.0, 60.0))), ("off", None), ("drop", Some((70.0, 60.0)))]);
}
