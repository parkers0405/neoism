//! `Tx::live_only`: a handler that moves only live values (a canvas camera) draws its frame,
//! hit tests and reports `on_drawn_layout` without re-running the view. And the element-local
//! points `on_pointer_move_local` and `on_secondary_press_local` give.
use rune::{Cx, Drag, El, Env, Key, KeyEvent, Live, MagnifyEvent, Mods, Phase, PointerButton, PointerEvent, PointerPress, RotateEvent, Runtime, Tx, WheelEvent, el};

struct S {
    /// Handlers say `Tx::live_only`.
    live: bool,
    pan: [f32; 2],
    zoom: f32,
    cam_pan: Live<[f32; 2]>,
    cam_zoom: Live<f32>,
    pan_at_press: [f32; 2],
    pressed: Vec<usize>,
    /// `on_drawn_layout_tx` reports for node 0, which a toolbar follows by its own live offset.
    drawn: Vec<[f32; 4]>,
    toolbar: Live<[f32; 2]>,
    moves: Vec<PointerEvent>,
    crosshair: Live<[f32; 2]>,
    menus: Vec<PointerEvent>,
    windows: Vec<PointerPress>,
    ups: usize,
}

impl S {
    fn new(live: bool) -> Self {
        S {
            live, pan: [0.0, 0.0], zoom: 1.0, cam_pan: Live::new([0.0, 0.0]), cam_zoom: Live::new(1.0),
            pan_at_press: [0.0, 0.0], pressed: Vec::new(), drawn: Vec::new(), toolbar: Live::new([0.0, 0.0]),
            moves: Vec::new(), crosshair: Live::new([0.0, 0.0]), menus: Vec::new(), windows: Vec::new(), ups: 0,
        }
    }
    /// The camera lives in fields the view never reads, and in the live values it binds.
    fn camera(&mut self, pan: [f32; 2], zoom: f32, tx: &mut Tx) {
        (self.pan, self.zoom) = (pan, zoom);
        self.cam_pan.set(pan);
        self.cam_zoom.set(zoom);
        if self.live { tx.live_only(); }
    }
}

/// A 400 pt canvas: a world of four 40 pt nodes under the camera, and a toolbar over node 0.
fn view(s: &S, _: &mut Cx<S>) -> El {
    let world = (0..4).fold(el().key("world").size(400.0, 400.0).live_offset(&s.cam_pan).live_scale(&s.cam_zoom), |w, i| {
        let node = el().key(format!("node{i}")).pin(Some(20.0 + 100.0 * i as f32), Some(100.0), None, None).size(40.0, 40.0)
            .on_press(move |s: &mut S| s.pressed.push(i));
        w.child(if i == 0 {
            node.on_drawn_layout_tx(|s: &mut S, r, tx| {
                s.drawn.push(r);
                s.toolbar.set([r[0], r[1] - 30.0]);
                if s.live { tx.live_only(); }
            })
        } else { node })
    });
    el().key("canvas").size(400.0, 400.0)
        .child(world)
        .child(el().key("toolbar").pin(Some(0.0), Some(0.0), None, None).size(60.0, 24.0).live_offset(&s.toolbar))
        .child(el().key("crosshair").pin(Some(0.0), Some(0.0), None, None).size(1.0, 1.0).live_offset(&s.crosshair))
        .on_wheel(|s: &mut S, e: WheelEvent, tx| { s.camera([s.pan[0] + e.dx, s.pan[1] + e.dy], s.zoom, tx); true })
        .on_magnify(|s: &mut S, e: MagnifyEvent, tx| { s.camera(s.pan, s.zoom * (1.0 + e.delta), tx); true })
        .on_rotate(|s: &mut S, e: RotateEvent, tx| { s.camera([s.pan[0] + e.degrees, s.pan[1]], s.zoom, tx); true })
        .pointer_buttons([PointerButton::Middle])
        .on_drag(|s: &mut S, d: &mut Drag, tx| {
            if d.phase == Phase::Down { s.pan_at_press = s.pan; }
            s.camera([s.pan_at_press[0] + d.dx, s.pan_at_press[1] + d.dy], s.zoom, tx);
        })
        .on_pointer_move_local(|s: &mut S, p, tx| {
            s.moves.push(p);
            s.crosshair.set([p.x, p.y]);
            if s.live { tx.live_only(); }
        })
        .on_key_up(|s: &mut S, _, tx| { s.ups += 1; s.camera([s.pan[0], s.pan[1] + 5.0], s.zoom, tx); true })
}

fn app(live: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S::new(live), view, (400.0, 400.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, e, tx| {
        let dx = match e.key { Key::Left => -10.0, Key::Right => 10.0, _ => return false };
        s.camera([s.pan[0] + dx, s.pan[1]], s.zoom, tx);
        true
    });
    // These tests count commits; debug builds' spot check would add one.
    rt.check_live_only(false);
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<S>) {
    for _ in 0..600 { if !rt.frame(1.0 / 120.0) && !rt.has_pending_input() { return; } }
    panic!("never settled");
}

fn rect(rt: &Runtime<S>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}")).rect
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool { a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01) }

/// Runs `event` and the frames after it; returns the commits they took.
fn commits(rt: &mut Runtime<S>, event: impl FnOnce(&mut Runtime<S>)) -> u64 {
    let before = rt.stats.commits;
    event(rt);
    settle(rt);
    rt.stats.commits - before
}

/// Every kind of canvas handler moves the camera: a wheel, a pinch, a rotation, a middle-button
/// drag, a key and a key release.
fn every_event(rt: &mut Runtime<S>) -> Vec<(&'static str, u64)> {
    let mut out = Vec::new();
    out.push(("wheel", commits(rt, |rt| rt.wheel_event(WheelEvent::new(-30.0, -20.0).at(200.0, 300.0).precise(true)))));
    out.push(("magnify", commits(rt, |rt| rt.magnify_event(MagnifyEvent::new(0.5).at(200.0, 300.0)))));
    out.push(("rotate", commits(rt, |rt| rt.rotate_event(RotateEvent::new(15.0).at(200.0, 300.0)))));
    rt.button_down(200.0, 300.0, PointerButton::Middle);
    settle(rt);
    out.push(("drag", commits(rt, |rt| {
        for i in 1..=4 { rt.pointer_move(200.0 + 5.0 * i as f32, 300.0); rt.frame(1.0 / 120.0); }
    })));
    rt.button_up(220.0, 300.0, PointerButton::Middle);
    settle(rt);
    out.push(("key", commits(rt, |rt| rt.key(KeyEvent { key: Key::Right, mods: Mods::default() }))));
    out.push(("key up", commits(rt, |rt| { rt.key_up(KeyEvent { key: Key::Right, mods: Mods::default() }); })));
    out
}

#[test]
fn live_only_handlers_move_the_canvas_without_re_running_the_view() {
    let mut rt = app(true);
    let start = rect(&rt, "node0");
    let ran = every_event(&mut rt);
    assert!(ran.iter().all(|&(_, n)| n == 0), "no event re-ran the view: {ran:?}");
    assert!(rt.live_only_events() >= 6, "{}", rt.live_only_events());

    // The camera moved what's drawn: node 0 shows where the camera puts it.
    let [px, py] = rt.state.pan;
    let z = rt.state.zoom;
    let at = |x: f32, y: f32| [200.0 + (x - 200.0) * z + px, 200.0 + (y - 200.0) * z + py];
    let [x, y] = at(20.0, 100.0);
    let node = rect(&rt, "node0");
    assert!(near(node, [x, y, 40.0 * z, 40.0 * z]), "node0 {node:?}, from {start:?}");

    // `on_drawn_layout_tx` reported where it shows, and its live-only toolbar followed it.
    assert!(near(*rt.state.drawn.last().unwrap(), node), "{:?}", rt.state.drawn.last());
    let toolbar = rect(&rt, "toolbar");
    assert!(near(toolbar, [node[0], node[1] - 30.0, 60.0, 24.0]), "toolbar {toolbar:?}");

    // Hit tests follow the camera: a press where node 1 now shows presses it, and where it was
    // before presses nothing.
    let [x1, y1] = at(140.0, 120.0);
    rt.pointer_down(x1, y1);
    rt.pointer_up(x1, y1);
    rt.pointer_down(140.0, 120.0);
    rt.pointer_up(140.0, 120.0);
    settle(&mut rt);
    assert_eq!(rt.state.pressed, [1]);
}

#[test]
fn without_live_only_each_event_re_runs_the_view() {
    let mut rt = app(false);
    let ran = every_event(&mut rt);
    assert!(ran.iter().all(|&(_, n)| n >= 1), "each event committed: {ran:?}");
    assert_eq!(rt.live_only_events(), 0);
}

#[test]
fn a_declined_event_takes_back_its_live_only() {
    // The inner handler says live_only but declines the pinch; the outer one takes it without
    // saying so, which commits.
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        el().size(100.0, 100.0)
            .child(el().key("inner").size(100.0, 100.0).on_magnify(|_: &mut u32, _, tx| { tx.live_only(); false }))
            .on_magnify(|s: &mut u32, _, _| { *s += 1; true })
    }
    let mut rt = Runtime::new(0u32, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.check_live_only(false);
    rt.frame(0.0);
    let before = rt.stats.commits;
    rt.magnify_event(MagnifyEvent::new(0.5).at(50.0, 50.0));
    rt.frame(1.0 / 120.0);
    assert_eq!((rt.state, rt.stats.commits - before, rt.live_only_events()), (1, 1, 0), "the outer handler took it and committed");
}

#[test]
fn pointer_move_and_secondary_press_give_local_points_on_a_zoomed_canvas() {
    // A 200 pt canvas at (50, 50), drawn at twice its size about its centre: its top-left shows
    // at (-50, -50) and a window point maps to (p + 50) / 2 in its own points.
    fn view(_: &S, _: &mut Cx<S>) -> El {
        el().size(300.0, 300.0)
            .child(el().key("canvas").pin(Some(50.0), Some(50.0), None, None).size(200.0, 200.0).scale(2.0)
                .on_pointer_move_local(|s: &mut S, p, _| s.moves.push(p))
                .on_secondary_press_local(|s: &mut S, p, _| s.menus.push(p)))
            .child(el().key("old").pin(Some(100.0), Some(250.0), None, None).size(100.0, 50.0)
                .on_pointer_move(|s: &mut S, p| s.windows.push(p))
                .on_secondary_press(|s: &mut S, p, _| s.windows.push(p)))
    }
    let mut rt = Runtime::new(S::new(false), view, (300.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let canvas = rect(&rt, "canvas");
    assert!(near(canvas, [-50.0, -50.0, 400.0, 400.0]), "drawn at twice its size: {canvas:?}");
    rt.pointer_move(110.0, 30.0);
    rt.frame(1.0 / 120.0);
    rt.secondary_down(110.0, 30.0);
    rt.secondary_up(110.0, 30.0);
    rt.frame(1.0 / 120.0);
    let p = rt.state.moves[0];
    assert_eq!((p.x, p.y, p.window_x, p.window_y), (80.0, 40.0, 110.0, 30.0));
    assert_eq!(p.window(), PointerPress { x: 110.0, y: 30.0 });
    let m = rt.state.menus[0];
    assert_eq!((m.x, m.y, m.window()), (80.0, 40.0, PointerPress { x: 110.0, y: 30.0 }));

    // The original builders still give window points.
    rt.state.windows.clear();
    rt.pointer_move(150.0, 270.0);
    rt.frame(1.0 / 120.0);
    rt.secondary_down(150.0, 270.0);
    rt.secondary_up(150.0, 270.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.windows, [PointerPress { x: 150.0, y: 270.0 }, PointerPress { x: 150.0, y: 270.0 }]);
}

#[test]
fn pointer_move_local_can_be_live_only() {
    let mut rt = app(true);
    let ran = commits(&mut rt, |rt| { rt.pointer_move(120.0, 80.0); rt.frame(1.0 / 120.0); rt.pointer_move(130.0, 90.0); });
    assert_eq!(ran, 0);
    let last = *rt.state.moves.last().unwrap();
    assert_eq!((last.x, last.y), (130.0, 90.0), "the canvas sits at the window's origin");
    let c = rect(&rt, "crosshair");
    assert!(near(c, [130.0, 90.0, 1.0, 1.0]), "the crosshair drew where the pointer is: {c:?}");
}

#[test]
fn live_transform_refreshes_hover_under_a_stationary_pointer() {
    struct Hover { pan: Live<[f32; 2]>, changes: Vec<bool>, enters: usize }
    fn view(s: &Hover, _: &mut Cx<Hover>) -> El {
        el().child(el().key("moving").size(40.0, 40.0).live_offset(&s.pan)
            .on_hover(|s: &mut Hover| s.enters += 1)
            .on_hover_change(|s: &mut Hover, inside, _| s.changes.push(inside)))
    }
    let mut rt = Runtime::new(Hover { pan: Live::new([100.0, 0.0]), changes: Vec::new(), enters: 0 }, view,
        (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(20.0, 20.0); rt.frame(0.0);
    rt.state.pan.set([0.0, 0.0]);
    assert!(rt.frame(0.0), "hover reports need another frame even for an immediate live transform");
    rt.frame(0.0);
    assert_eq!(rt.state.changes, [true]);
    assert_eq!(rt.state.enters, 1);
    rt.state.pan.set([100.0, 0.0]);
    assert!(rt.frame(0.0)); rt.frame(0.0);
    assert_eq!(rt.state.changes, [true, false]);
}

#[test]
fn nested_live_and_static_scales_keep_local_points_per_runtime() {
    fn view(s: &S, _: &mut Cx<S>) -> El {
        el().child(el().size(300.0, 300.0).scale(2.0).live_offset(&s.cam_pan)
            .child(el().key("inner").pin(Some(60.0), Some(60.0), None, None).size(100.0, 100.0)
                .live_scale(&s.cam_zoom).on_pointer_move_local(|s: &mut S, p, _| s.moves.push(p))
                .on_secondary_press_local(|s: &mut S, p, _| s.menus.push(p))))
    }
    for scale in [1.0, 2.0] {
        let state = S::new(false);
        state.cam_pan.set([30.0, 20.0]); state.cam_zoom.set(0.5);
        let mut rt = Runtime::new(state, view, (500.0, 500.0), Env::new(true, scale));
        rt.frame(0.0);
        let r = rect(&rt, "inner");
        // Nested scales multiply to one, but both anchors still move the origin.
        let (x, y) = (r[0] + 25.0, r[1] + 35.0);
        rt.pointer_move(x, y); rt.secondary_down(x, y); rt.secondary_up(x, y); rt.frame(0.0);
        for p in [rt.state.moves[0], rt.state.menus[0]] {
            assert!((p.x - 25.0).abs() < 0.01 && (p.y - 35.0).abs() < 0.01, "{p:?}");
            assert_eq!((p.window_x, p.window_y), (x, y));
        }
    }
}

#[test]
fn live_only_drag_keeps_capture_outside_the_moving_element() {
    struct Held { pan: Live<[f32; 2]>, phases: Vec<Phase> }
    fn view(s: &Held, _: &mut Cx<Held>) -> El {
        el().child(el().size(40.0, 40.0).live_offset(&s.pan).on_drag(|s: &mut Held, d, tx| {
            s.phases.push(d.phase);
            if d.phase == Phase::Move { s.pan.set([d.dx, d.dy]); }
            tx.live_only();
        }))
    }
    let mut rt = Runtime::new(Held { pan: Live::new([0.0, 0.0]), phases: Vec::new() }, view,
        (200.0, 200.0), Env::new(true, 1.0));
    rt.check_live_only(false); rt.frame(0.0);
    let before = rt.stats.commits;
    rt.pointer_down(20.0, 20.0); rt.frame(0.0);
    rt.pointer_move(300.0, 300.0); rt.frame(0.0);
    assert!(rt.pressing());
    rt.pointer_up(350.0, 350.0); rt.frame(0.0);
    assert!(!rt.pressing());
    assert_eq!(rt.state.phases, [Phase::Down, Phase::Move, Phase::Move, Phase::Up]);
    assert_eq!(rt.stats.commits, before);
}

/// A marker beside a scrolling list follows row 10 by its own live offset, set from
/// `on_layout_tx`.
struct Follow {
    live: bool,
    marker: Live<[f32; 2]>,
    heard: usize,
}

fn follow_view(s: &Follow, _: &mut Cx<Follow>) -> El {
    let rows = (0..40).fold(rune::col().w_full(), |c, i| {
        let row = el().key(format!("r{i}")).w_full().h(30.0);
        c.child(if i == 10 {
            row.on_layout_tx(|s: &mut Follow, r, tx| {
                s.heard += 1;
                s.marker.set([0.0, r[1]]);
                if s.live { tx.live_only(); }
            })
        } else { row })
    });
    rune::col().w_full().h_full()
        .child(rune::col().key("list").w_full().h(300.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(rows))
        .child(el().key("marker").pin(Some(0.0), Some(0.0), None, None).size(10.0, 10.0).live_offset(&s.marker))
}

fn follow(live: bool) -> (u64, usize, Runtime<Follow>) {
    let mut rt = Runtime::new(Follow { live, marker: Live::new([0.0, 0.0]), heard: 0 }, follow_view, (300.0, 400.0), Env::new(true, 1.0));
    rt.check_live_only(false);
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    let (commits, heard) = (rt.stats.commits, rt.state.heard);
    for _ in 0..30 {
        rt.wheel(100.0, 100.0, -3.0);
        rt.frame(1.0 / 120.0);
    }
    rt.frame(1.0 / 120.0);
    (rt.stats.commits - commits, rt.state.heard - heard, rt)
}

#[test]
fn a_live_only_on_layout_tx_follows_a_scroll_without_re_running_the_view() {
    let (commits, heard, rt) = follow(true);
    assert!(heard >= 30, "heard every frame of the scroll: {heard}");
    assert_eq!(commits, 0);
    let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r10")).unwrap().rect;
    let marker = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("marker")).unwrap().rect;
    assert_eq!(row[1], 210.0, "scrolled 90");
    assert_eq!(marker[1], row[1], "the marker follows the row");
    let (commits, heard, _) = follow(false);
    assert!(commits >= 30 && heard >= 30, "without live_only each report re-runs the view: {commits}");
}
