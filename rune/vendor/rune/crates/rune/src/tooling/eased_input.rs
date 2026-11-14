//! Script inputs a hand eases over a duration, one step a frame.
use super::{Ev, Kind, Pos};
use crate::runtime::Runtime;
use serde_json::Value;

/// `{"type": "scroll"}`: a trackpad swipe, deltas eased over `dur`, then (with "fling") the
/// release velocity.
pub(super) fn swipe(out: &mut Vec<Ev>, e: &Value, t: f64, pos: Pos) {
    let dx = e["dx"].as_f64().unwrap_or(0.0) as f32;
    let dy = e["dy"].as_f64().unwrap_or(0.0) as f32;
    let dur = e["dur"].as_f64().unwrap_or(0.2).max(0.01);
    out.push(Ev { t, kind: Kind::Move, pos: pos.clone() });
    let steps = (dur * 120.0).ceil() as usize;
    let mut prev = (0.0, 0.0);
    for s in 1..=steps {
        let f = s as f32 / steps as f32;
        let f = f * (2.0 - f);
        out.push(Ev { t: t + dur * s as f64 / steps as f64, kind: Kind::Wheel(dx * f - prev.0, dy * f - prev.1), pos: pos.clone() });
        prev = (dx * f, dy * f);
    }
    if e["fling"].as_bool().unwrap_or(false) {
        let v = |d: f32| 2.0 * d / dur as f32 * 0.25;
        out.push(Ev { t: t + dur + 1e-4, kind: Kind::Fling(v(dx), v(dy)), pos });
    }
}

/// `{"type": "resize", "dw": …, "dh": …, "dur": …}`: a person dragging the window's corner, one
/// viewport step a frame on a smoothstep hand, as a live resize delivers it.
pub(super) fn resize(out: &mut Vec<Ev>, e: &Value, t: f64) {
    let d = |k: &str| e[k].as_f64().unwrap_or(0.0) as f32;
    let dur = e["dur"].as_f64().unwrap_or(0.3).max(0.01);
    let steps = (dur * 120.0).ceil() as usize;
    for s in 0..=steps {
        let f = s as f32 / steps as f32;
        let f = f * f * (3.0 - 2.0 * f);
        out.push(Ev { t: t + dur * s as f64 / steps as f64, kind: Kind::Resize(d("dw") * f, d("dh") * f), pos: Pos::Rel(0.0, 0.0) });
    }
}

/// One step of a scripted resize: (0, 0) marks where it starts, and content follows at once.
pub(super) fn step_resize<S: 'static>(rt: &mut Runtime<S>, from: &mut (f32, f32), (dw, dh): (f32, f32), scale: f32) {
    if (dw, dh) == (0.0, 0.0) { *from = rt.viewport; }
    rt.set_viewport(((from.0 + dw).max(1.0), (from.1 + dh).max(1.0)), scale);
}
