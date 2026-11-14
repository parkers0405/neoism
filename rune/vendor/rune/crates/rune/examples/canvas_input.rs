//! Canvas input: an infinite canvas driven headlessly, no window. The world sits under one
//! `live_offset` and `live_scale` (the camera), so a pan or zoom moves it without a relayout.
//!
//! - The wheel and a two-finger trackpad scroll pan (`on_wheel`).
//! - A pinch zooms about the pointer (`on_magnify`; Ctrl + wheel off macOS).
//! - Space held with a drag pans; the release ends it (`on_key_up`).
//! - The middle button drags to pan (`pointer_buttons`, `Tx::button`).
//! - "Focus" centres the selected node and glides its row in the layers list into view
//!   (`Tx::scroll_to`).
//! - A toolbar would float over the selection: `on_drawn_layout` reports where it shows.
//! - The handlers that only move the camera say `Tx::live_only`, so their events don't re-run
//!   the view.
//!
//! cargo run -p rune --example canvas_input
use rune::{Color, Cx, Drag, El, Env, Key, KeyEvent, Live, MagnifyEvent, Mods, Phase, PointerButton, Runtime, ScrollSpec, Tx, WheelEvent, col, el, row};

const CANVAS: f32 = 400.0;
const NODES: usize = 30;
const ROW: f32 = 40.0;

struct Canvas {
    /// The camera: drawn = centre + (world - centre) * zoom + pan, about the world's centre.
    pan: [f32; 2],
    zoom: f32,
    live_pan: Live<[f32; 2]>,
    live_zoom: Live<f32>,
    space: bool,
    pan_at_press: [f32; 2],
    selected: usize,
    /// Where the selected node shows, for a floating toolbar.
    toolbar: Option<[f32; 4]>,
}

impl Canvas {
    fn set_camera(&mut self, pan: [f32; 2], zoom: f32) {
        (self.pan, self.zoom) = (pan, zoom.clamp(0.1, 8.0));
        self.live_pan.set(self.pan);
        self.live_zoom.set(self.zoom);
    }
    /// The world point under canvas point `q`.
    fn world(&self, q: [f32; 2]) -> [f32; 2] {
        let c = CANVAS / 2.0;
        [c + (q[0] - c - self.pan[0]) / self.zoom, c + (q[1] - c - self.pan[1]) / self.zoom]
    }
    /// Where world point `p` shows in the canvas.
    fn screen(&self, p: [f32; 2]) -> [f32; 2] {
        let c = CANVAS / 2.0;
        [c + (p[0] - c) * self.zoom + self.pan[0], c + (p[1] - c) * self.zoom + self.pan[1]]
    }
    fn drag_pan(&mut self, d: &Drag) {
        if d.phase == Phase::Down { self.pan_at_press = self.pan; }
        let pan = if d.cancelled() { self.pan_at_press } else { [self.pan_at_press[0] + d.dx, self.pan_at_press[1] + d.dy] };
        self.set_camera(pan, self.zoom);
    }
}

fn node_at(i: usize) -> [f32; 2] {
    [20.0 + 150.0 * (i % 6) as f32, 20.0 + 120.0 * (i / 6) as f32]
}

fn view(s: &Canvas, _: &mut Cx<Canvas>) -> El {
    let nodes = (0..NODES).fold(el().key("world").size(CANVAS, CANVAS).live_offset(&s.live_pan).live_scale(&s.live_zoom), |w, i| {
        let [x, y] = node_at(i);
        let node = el().key(format!("node{i}")).pin(Some(x), Some(y), None, None).size(80.0, 60.0).radius(8.0)
            .fill(if i == s.selected { Color::WHITE } else { Color::WHITE.alpha(0.3) })
            .on_press(move |s: &mut Canvas| s.selected = i);
        w.child(if i == s.selected { node.on_drawn_layout(|s: &mut Canvas, r| s.toolbar = Some(r)) } else { node })
    });
    let canvas = el().key("canvas").size(CANVAS, CANVAS).clip_soft(0.0).child(nodes)
        // The camera is all these handlers change, and the view reads none of it: they say
        // `live_only`, and a pan or pinch draws without re-running the view.
        .on_wheel(|s: &mut Canvas, e: WheelEvent, tx| {
            s.set_camera([s.pan[0] + e.dx, s.pan[1] + e.dy], s.zoom);
            tx.live_only();
            true
        })
        .on_magnify(|s: &mut Canvas, e: MagnifyEvent, tx| {
            // Keep the world point under the pointer where it is.
            let (q, p, c) = ([e.x, e.y], s.world([e.x, e.y]), CANVAS / 2.0);
            let zoom = (s.zoom * (1.0 + e.delta)).clamp(0.1, 8.0);
            s.set_camera([q[0] - c - (p[0] - c) * zoom, q[1] - c - (p[1] - c) * zoom], zoom);
            tx.live_only();
            true
        })
        .pointer_buttons([PointerButton::Middle])
        .on_drag(|s: &mut Canvas, d: &mut Drag, tx: &mut Tx| {
            if tx.button() == PointerButton::Middle || s.space { s.drag_pan(d); tx.live_only(); }
        });
    let focus = el().key("focus").h(ROW).w_full().fill(Color::WHITE.alpha(0.1))
        .on_press_tx(|s: &mut Canvas, tx| {
            let [x, y] = node_at(s.selected);
            let c = CANVAS / 2.0;
            let centre = [x + 40.0, y + 30.0];
            s.set_camera([-(centre[0] - c) * s.zoom, -(centre[1] - c) * s.zoom], s.zoom);
            tx.scroll_to("layers", 0.0, s.selected as f32 * ROW);
        });
    let rows = (0..NODES).fold(col().w_full(), |c, i| c.child(el().key(format!("layer{i}")).w_full().h(ROW)
        .on_press(move |s: &mut Canvas| s.selected = i)));
    let side = col().w(200.0).h_full().child(focus)
        .child(col().key("layers").w_full().h(CANVAS - ROW).scroll(ScrollSpec { stick_bottom: false }).child(rows));
    row().w_full().h_full()
        .on_key_up(|s: &mut Canvas, e: &KeyEvent, _| {
            let space = e.key == Key::Text(" ".into());
            if space { s.space = false; }
            space
        })
        .child(canvas).child(side)
}

fn rect(rt: &Runtime<Canvas>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}")).rect
}

fn near(a: f32, b: f32) -> bool { (a - b).abs() <= 1.0 }

fn settle(rt: &mut Runtime<Canvas>) {
    for _ in 0..600 { if !rt.frame(1.0 / 60.0) && !rt.has_pending_input() { return; } }
    panic!("never settled");
}

fn main() {
    let state = Canvas {
        pan: [0.0, 0.0], zoom: 1.0, live_pan: Live::new([0.0, 0.0]), live_zoom: Live::new(1.0),
        space: false, pan_at_press: [0.0, 0.0], selected: 7, toolbar: None,
    };
    let mut rt = Runtime::new(state, view, (600.0, CANVAS), Env::new(true, 1.0));
    rt.on_key = Some(|s, e, _| {
        let space = e.key == Key::Text(" ".into());
        if space { s.space = true; }
        space
    });
    settle(&mut rt);
    let start = rect(&rt, "node7");
    println!("node7 at {start:?}, toolbar {:?}", rt.state.toolbar);

    // Wheel pan: a two-finger scroll moves the camera, never the page.
    rt.wheel_event(WheelEvent::new(-30.0, -20.0).at(200.0, 200.0).precise(true));
    settle(&mut rt);
    let panned = rect(&rt, "node7");
    assert!(near(panned[0], start[0] - 30.0) && near(panned[1], start[1] - 20.0), "wheel pan: {panned:?}");
    assert_eq!(rt.state.toolbar, Some(panned), "the toolbar follows without a relayout");
    println!("wheel pan    -> node7 at {panned:?}");

    // Pinch about the node's corner: the corner stays under the pointer.
    let q = [panned[0], panned[1]];
    rt.magnify_event(MagnifyEvent::new(1.0).at(q[0], q[1]));
    settle(&mut rt);
    let zoomed = rect(&rt, "node7");
    assert!(near(zoomed[0], q[0]) && near(zoomed[1], q[1]) && near(zoomed[2], 160.0), "pinch: {zoomed:?}");
    println!("pinch x2     -> node7 at {zoomed:?}, zoom {}", rt.state.zoom);

    // Space + drag pans; after Space's release the same drag doesn't.
    let space = KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() };
    rt.key(space.clone());
    rt.frame(1.0 / 60.0);
    assert!(rt.state.space);
    // From the gap between the first four nodes, so the press lands on the canvas itself.
    let drag = |rt: &mut Runtime<Canvas>, button: PointerButton| {
        let [x, y] = rt.state.screen([135.0, 110.0]);
        rt.pointer_move(x, y);
        rt.button_down(x, y, button);
        rt.pointer_move(x - 20.0, y - 10.0);
        rt.pointer_move(x - 40.0, y - 20.0);
        rt.button_up(x - 40.0, y - 20.0, button);
        settle(rt);
    };
    drag(&mut rt, PointerButton::Primary);
    let spaced = rect(&rt, "node7");
    assert!(near(spaced[0], zoomed[0] - 40.0) && near(spaced[1], zoomed[1] - 20.0), "space drag: {spaced:?}");
    println!("space drag   -> node7 at {spaced:?}");
    assert!(rt.key_up(space));
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.space, "the release ended the pan mode");
    drag(&mut rt, PointerButton::Primary);
    assert_eq!(rect(&rt, "node7"), spaced, "a plain drag after the release doesn't pan");

    // The middle button pans with no key held.
    drag(&mut rt, PointerButton::Middle);
    let middled = rect(&rt, "node7");
    assert!(near(middled[0], spaced[0] - 40.0) && near(middled[1], spaced[1] - 20.0), "middle drag: {middled:?}");
    println!("middle drag  -> node7 at {middled:?}");

    // Select node 13 on the canvas, then Focus: the camera centres it and its layer row glides
    // to the top of the list.
    let n13 = rect(&rt, "node13");
    let (x, y) = (n13[0] + n13[2] / 2.0, n13[1] + n13[3] / 2.0);
    assert!(x > 0.0 && x < CANVAS && y > 0.0 && y < CANVAS, "node13 shows: {n13:?}");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.selected, 13, "a hit test at 2x lands on the drawn node");
    let (fx, fy) = rt.find("focus").expect("focus");
    rt.pointer_down(fx, fy);
    rt.pointer_up(fx, fy);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let gliding = rect(&rt, "layer13")[1];
    settle(&mut rt);
    let focused = rect(&rt, "node13");
    let row = rect(&rt, "layer13");
    assert!(near(focused[0] + focused[2] / 2.0, CANVAS / 2.0) && near(focused[1] + focused[3] / 2.0, CANVAS / 2.0), "centred: {focused:?}");
    assert!(gliding > row[1] && near(row[1], ROW), "the row glided to the top of the list: {gliding} then {row:?}");
    assert_eq!(rt.state.toolbar, Some(focused));
    println!("focus        -> node13 at {focused:?}, layer13 row at y {}", row[1]);
    println!("ok");
}
