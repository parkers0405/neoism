//! Motion gate for `El::anchored` at its anchor's zoom: an open popover follows the page's zoom
//! spring, and one entering or leaving a zoomed page plays its transition there.
use super::*;
use crate::el::{Align, Side};
use crate::motion_gate::{self, Allow, Job, Recording, Run};
use crate::{Cx, El, Env, TextStyle, Transition, col, el, text};

#[derive(Clone, Copy)]
struct S {
    zoom: f32,
    open: bool,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    // Small enough that the page's own scale spring stays under the `jerk` threshold.
    let page = col().key("page").size(300.0, 200.0).scale(s.zoom).origin(0.0, 0.0)
        .child(el().key("target").absolute(100.0, 100.0).size(80.0, 20.0).fill(crate::Color::WHITE));
    let root = col().size(800.0, 600.0).child(page);
    if !s.open {
        return root;
    }
    let pop = col().key("pop").w(120.0).padding(10.0).fill(crate::Color::WHITE)
        .child(text("Rename").key("label").font(TextStyle::new(13.0, 400.0)))
        .transition(Transition::scale(0.95).combined(Transition::opacity()))
        .anchored("target", Side::Bottom, Align::Start, 5.0);
    root.child(pop)
}

fn cell(start: S, what: &'static str, change: fn(&mut S)) -> Recording {
    let mut r = Run::new(Runtime::new(start, view, (800.0, 600.0), Env::new(true, 2.0)), 120.0);
    r.idle(1.0);
    r.step();
    r.act(what, |rt| rt.update(|s, _| change(s)));
    r.play(1.2);
    r.quiet();
    r.play(0.3);
    r.rec
}

const ALLOW: &[Allow] = &[];

#[test]
fn anchored_zoom_motion_gate() {
    let cells: Vec<(String, Job)> = vec![
        ("anchored-zoom/zoom-while-open".into(), Box::new(|| cell(S { zoom: 1.0, open: true }, "zoom to 1.6", |s| s.zoom = 1.6))),
        ("anchored-zoom/unzoom-while-open".into(), Box::new(|| cell(S { zoom: 1.6, open: true }, "zoom to 1.0", |s| s.zoom = 1.0))),
        ("anchored-zoom/enter-zoomed".into(), Box::new(|| cell(S { zoom: 1.6, open: false }, "open", |s| s.open = true))),
        ("anchored-zoom/exit-zoomed".into(), Box::new(|| cell(S { zoom: 1.6, open: true }, "close", |s| s.open = false))),
    ];
    motion_gate::gate(cells, ALLOW);
}
