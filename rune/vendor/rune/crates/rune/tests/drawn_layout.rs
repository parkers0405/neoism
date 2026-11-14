//! Rects under live transforms: `El::on_drawn_layout` reports where an element on a zoomed,
//! panned canvas shows, hit tests and `ElementInfo::rect` agree with it, and `on_layout` keeps
//! the laid-out rect.
use rune::{Cx, Drag, El, Env, Live, Runtime, el};

struct S {
    pan: Live<[f32; 2]>,
    zoom: Live<f32>,
    drawn: Vec<[f32; 4]>,
    laid: Vec<[f32; 4]>,
    pressed: Vec<&'static str>,
    drags: Vec<(f32, f32)>,
    /// The node grows by 20 pt each time it reports its drawn width: layout feedback that must fall quiet.
    chase: bool,
    chase_scale: bool,
    width: f32,
    scale: f32,
}

fn new() -> S {
    S { pan: Live::new([0.0, 0.0]), zoom: Live::new(1.0), drawn: Vec::new(), laid: Vec::new(), pressed: Vec::new(), drags: Vec::new(), chase: false, chase_scale: false, width: 20.0, scale: 1.0 }
}

/// A 200 pt world under the camera, with a node at (100, 100) and one at (20, 20).
fn view(s: &S, _: &mut Cx<S>) -> El {
    let node = el().key("node").pin(Some(100.0), Some(100.0), None, None).size(s.width, 20.0).scale(s.scale)
        .on_drawn_layout(|s: &mut S, r| {
            s.drawn.push(r);
            if s.chase { s.width = 20.0 + r[2]; }
            if s.chase_scale { s.scale += 0.25; }
        })
        .on_layout(|s: &mut S, r| s.laid.push(r))
        .on_press(|s: &mut S| s.pressed.push("node"))
        .on_drag(|s: &mut S, d: &mut Drag, _| s.drags.push((d.x, d.y)));
    let other = el().key("other").pin(Some(20.0), Some(20.0), None, None).size(20.0, 20.0)
        .on_press(|s: &mut S| s.pressed.push("other"));
    let world = el().key("world").size(200.0, 200.0).live_offset(&s.pan).live_scale(&s.zoom).child(node).child(other);
    el().size(300.0, 300.0).child(world)
}

fn app() -> Runtime<S> {
    let mut rt = Runtime::new(new(), view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.0);
    rt
}

fn rect(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}")).rect
}

#[test]
fn a_camera_pan_and_zoom_report_the_drawn_rect_without_a_commit() {
    let mut rt = app();
    assert_eq!(rt.state.drawn, [[100.0, 100.0, 20.0, 20.0]], "reported once after it is first drawn");
    assert_eq!(rt.state.laid, [[100.0, 100.0, 20.0, 20.0]]);

    // Half size about the world's centre (100, 100), then 10 right.
    rt.state.zoom.set(0.5);
    rt.state.pan.set([10.0, 0.0]);
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.drawn.last(), Some(&[110.0, 100.0, 10.0, 10.0]));
    assert_eq!(rect(&rt, "node"), [110.0, 100.0, 10.0, 10.0], "ElementInfo::rect agrees");
    assert_eq!(rt.state.laid.len(), 1, "on_layout keeps the laid-out rect, which didn't move");

    // A long pan keeps reporting: live moves are outside layout feedback.
    for k in 1..=30 {
        rt.state.pan.set([10.0 + k as f32, 0.0]);
        rt.frame(1.0 / 60.0);
    }
    rt.frame(0.0);
    assert_eq!(rt.state.drawn.last(), Some(&[140.0, 100.0, 10.0, 10.0]));
    assert!(rt.state.drawn.len() >= 30, "every frame of the pan reported: {}", rt.state.drawn.len());
    let n = rt.state.drawn.len();
    for _ in 0..5 { rt.frame(1.0 / 60.0); }
    assert_eq!(rt.state.drawn.len(), n, "nothing moved, nothing reported");
    assert!(!rt.has_pending_input());
}

#[test]
fn hit_tests_and_drags_follow_the_live_zoom_and_pan() {
    let mut rt = app();
    rt.state.zoom.set(0.5);
    rt.state.pan.set([10.0, 0.0]);
    rt.frame(0.0);
    // At rest the node covers (100..120, 100..120); drawn it covers (110..120, 100..110).
    rt.pointer_down(112.0, 105.0);
    rt.pointer_up(112.0, 105.0);
    rt.frame(0.0);
    assert_eq!(rt.state.pressed, ["node"]);
    rt.pointer_down(105.0, 115.0);
    rt.pointer_up(105.0, 115.0);
    rt.frame(0.0);
    assert_eq!(rt.state.pressed, ["node"], "inside its layout box but outside where it is drawn");
    // `other` at rest (20..40, 20..40) is drawn at (70..80, 60..70).
    rt.pointer_down(72.0, 62.0);
    rt.pointer_up(72.0, 62.0);
    rt.frame(0.0);
    assert_eq!(rt.state.pressed, ["node", "other"]);

    rt.state.drags.clear();
    rt.pointer_down(112.0, 104.0);
    rt.pointer_move(118.0, 108.0);
    rt.pointer_up(118.0, 108.0);
    rt.frame(0.0);
    let drags = &rt.state.drags;
    assert_eq!((drags[0], drags[drags.len() - 1]), ((4.0, 8.0), (16.0, 16.0)), "drag points are in the node's own points at half size: {drags:?}");
}

#[test]
fn a_handler_that_moves_its_own_element_falls_quiet() {
    let mut rt = app();
    rt.update(|s, _| s.chase = true);
    rt.state.pan.set([5.0, 0.0]);
    let mut frames = 0;
    while (rt.frame(1.0 / 60.0) || rt.has_pending_input()) && frames < 2_000 { frames += 1; }
    assert!(frames < 2_000, "layout feedback stops");
    assert!(rt.state.drawn.len() <= 8, "a few reports, then quiet: {:?}", rt.state.drawn);

    let mut rt = app();
    rt.set_system_reduced_motion(true);
    rt.update(|s, _| s.chase_scale = true);
    rt.state.pan.set([5.0, 0.0]);
    let mut frames = 0;
    while (rt.frame(1.0 / 60.0) || rt.has_pending_input()) && frames < 2_000 { frames += 1; }
    assert!(frames < 2_000, "static scale feedback stops too");
    assert!(rt.state.drawn.len() <= 8, "a few reports, then quiet: {:?}", rt.state.drawn);
}

#[test]
fn drag_points_in_a_scrolled_list_are_from_where_the_row_is_drawn() {
    fn list(_: &Vec<(f32, f32)>, _: &mut Cx<Vec<(f32, f32)>>) -> El {
        let rows = (0..20).fold(rune::col().w_full(), |c, i| c.child(el().key(format!("row{i}")).w_full().h(40.0)
            .on_drag(|s: &mut Vec<(f32, f32)>, d: &mut Drag, _| s.push((d.x, d.y)))));
        rune::col().w_full().h_full().child(rune::col().key("list").w_full().h(200.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(rows))
    }
    let mut rt = Runtime::new(Vec::new(), list, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_y("list", 100.0));
    rt.frame(0.0);
    // Row 5 (laid out at 200) is drawn at 100.
    rt.pointer_down(10.0, 110.0);
    rt.pointer_up(10.0, 110.0);
    rt.frame(0.0);
    assert_eq!(rt.state.first(), Some(&(10.0, 10.0)), "{:?}", rt.state);
}
