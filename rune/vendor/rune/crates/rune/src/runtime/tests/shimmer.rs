use super::*;
use crate::render::KIND_GLYPH;
use crate::{Color, TextStyle, text};

/// A text shimmer: a clipped window slides across a dim label while the bright copy inside it
/// slides back by the same amount, so the copy stays on the label. Both loops wait out the
/// same delay, which the view works out afresh from the clock each time it builds.
#[derive(Clone, Copy)]
struct Shimmer {
    since: f64,
    now: f64,
    snap: bool,
    zoom: f32,
    on: bool,
    tail: bool,
    pad: f32,
    ring: bool,
    band: f32,
    /// When the bright copy mounts (app clock); the window mounts with `on`.
    lit_at: f64,
}

const DIM: Color = Color { r: 0.3, g: 0.3, b: 0.3, a: 1.0 };
const LIT: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
const LABEL_W: f32 = 52.0;
const SPEED: f32 = 90.0;

fn base() -> Shimmer {
    Shimmer { since: 0.0, now: 0.0, snap: true, zoom: 1.0, on: true, tail: false, pad: 0.0, ring: false, band: 30.0, lit_at: f64::NEG_INFINITY }
}

fn period(s: &Shimmer) -> f64 { f64::from((LABEL_W + s.band) / SPEED) }

fn shimmer_view(s: &Shimmer, _: &mut Cx<Shimmer>) -> El {
    let font = TextStyle::new(13.0, 400.0);
    let w = s.band;
    let (x0, x1) = (-w, LABEL_W);
    let p = period(s);
    let delay = { let d = (-(s.now - s.since) / p).rem_euclid(1.0) * p; if d > p - 1e-6 { 0.0 } else { d } };
    let spec = |a: f32, b: f32| rune_motion::LoopSpec { delay, ..rune_motion::LoopSpec::slide(p, [a, 0.0], [b, 0.0]).easing(rune_motion::Easing::LINEAR) };
    let lit = el().key("lit").absolute(0.0, 0.0).w(w).h_full().looping(spec(-x0 / w, -x1 / w))
        .child(text("Working").font(font).color(LIT).absolute(0.0, 0.0));
    let sheen = el().key("sheen").absolute(0.0, 0.0).w(w).h_full().clip_fade(w / 2.0, 0.0, w / 2.0, 0.0)
        .looping(spec(x0 / w, x1 / w)).children((s.now >= s.lit_at).then_some(lit)).pixel_snap(s.snap);
    let label = crate::row().child(text("Working").key("label").font(font).color(DIM)).children(s.on.then_some(sheen));
    let mut ring = el().key("ring").size(14.0, 14.0);
    if s.ring {
        for k in 0..3 {
            ring = ring.child(el().key(format!("d{k}")).absolute(k as f32 * 5.0, 5.0).size(3.0, 3.0).fill(DIM)
                .cycle(crate::Cycle { period: 1.2, phase: k as f32 / 3.0, stops: &[(0.0, 0.2), (0.5, 1.0), (1.0, 0.2)], still: 0.2, fps: 30.0 }));
        }
    }
    let row = crate::row().key("working").h(28.0).gap(8.0).center_y().pb(12.0).child(ring).child(label);
    let row = if s.tail { el().w_full().h(28.0).motion(rune_motion::Spring::new(0.3, 0.0)).child(row.absolute(0.0, 0.0).opacity(1.0)) } else { row };
    let content = col().size(400.0, 60.0).pl(s.pad).pt(s.pad / 2.0).child(row);
    // The whole UI zoomed, as an app's zoom setting does it.
    el().size(400.0, 60.0).center().child(el().size(400.0 / s.zoom, 60.0 / s.zoom).scale(s.zoom).child(content))
}

/// For each bright glyph quad, how far (device px) its centre sits from the dim glyph it
/// copies. A glyph drawn between pixels takes a slightly larger quad, and one sampled from its
/// rest-size raster under a zoom that is still settling a larger one again, so sizes match
/// loosely.
fn lit_offsets(rt: &Runtime<Shimmer>) -> Vec<f32> {
    let glyphs = |c: Color| rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH && (i.fill[0] - c.r).abs() < 0.02 && (i.fill[1] - c.g).abs() < 0.02)
        .map(|i| i.rect).collect::<Vec<_>>();
    let (dim, lit) = (glyphs(DIM), glyphs(LIT));
    lit.iter().map(|l| {
        dim.iter().filter(|d| (d[2] - l[2]).abs() < 3.0 && (d[3] - l[3]).abs() < 3.0)
            .map(|d| (l[0] + l[2] / 2.0) - (d[0] + d[2] / 2.0)).min_by(|a, b| a.abs().total_cmp(&b.abs())).unwrap_or(f32::NAN)
    }).collect()
}

/// A shimmer at 2× in the default font smoothing.
fn shimmer_rt(s: Shimmer) -> Runtime<Shimmer> {
    Runtime::new(s, shimmer_view, (400.0, 60.0), Env::new(true, 2.0))
}

/// Pixel snapping moves a glyph by up to one device px. Above 33 ppem (13 pt zoomed 1.6×)
/// CoreGraphics' pens go to whole pixels, so a copy whose origin is a fraction off can land one
/// more pixel away, as CoreText's would. More is a misplaced copy: these tests check motion,
/// not placement.
const GLYPH_PX: f32 = 2.0;

fn worst_of(rt: &Runtime<Shimmer>, worst: f32) -> f32 {
    let w = lit_offsets(rt).into_iter().fold(0.0f32, |a, b| if b.is_nan() || b.abs() > a.abs() { b } else { a });
    if w.is_nan() || w.abs() > worst.abs() { w } else { worst }
}

fn run(rt: &mut Runtime<Shimmer>, frames: usize) -> f32 {
    let mut worst = 0.0f32;
    let dt = 1.0 / 120.0;
    for _ in 0..frames {
        rt.state.now += dt;
        rt.invalidate();
        rt.frame(dt);
        worst = worst_of(rt, worst);
    }
    worst
}

fn worst_offset(snap: bool, zoom: f32, mount_at: f64) -> f32 {
    let s = Shimmer { now: mount_at, snap, zoom, ..base() };
    let mut rt = shimmer_rt(s);
    rt.frame(0.0);
    run(&mut rt, (period(&s) * 2.5 * 120.0) as usize)
}

#[test]
fn a_counter_sliding_shimmer_keeps_its_bright_copy_on_the_label() {
    let mut bad = Vec::new();
    for zoom in [1.0, 1.1, 0.9, 1.25] {
        for snap in [true, false] {
            for mount_at in [0.0, 0.5] {
                let w = worst_offset(snap, zoom, mount_at);
                // An opted-out shimmer over a snapped label rounds its glyphs on its own, up to
                // a whole pixel from the label's.
                let bound = if snap { GLYPH_PX } else { GLYPH_PX + 1.0 / 64.0 };
                if !(w.abs() < bound) { bad.push((zoom, snap, mount_at, w)); }
            }
        }
    }
    assert!(bad.is_empty(), "the bright copy drifts off the label (zoom, snap, mounted at, px): {bad:?}");
}

#[test]
fn a_shimmer_that_stops_and_starts_again_keeps_its_bright_copy_on_the_label() {
    let mut bad = Vec::new();
    for tail in [false, true] {
        for off_frames in [1usize, 6, 60] {
            for stop_at in [0.3f64, 0.9, 1.5] {
                let s = Shimmer { tail, ..base() };
                let mut rt = shimmer_rt(s);
                rt.frame(0.0);
                run(&mut rt, (stop_at * 120.0) as usize);
                rt.state.on = false;
                run(&mut rt, off_frames);
                rt.state.on = true;
                rt.state.since = rt.state.now;
                let w = run(&mut rt, (period(&s) * 2.5 * 120.0) as usize);
                if !(w.abs() < GLYPH_PX) { bad.push((tail, off_frames, stop_at, w)); }
            }
        }
    }
    assert!(bad.is_empty(), "the bright copy drifts off the label after a restart (tail, frames off, stopped at, px): {bad:?}");
}

fn loop_offsets(rt: &Runtime<Shimmer>) -> Option<(f32, f32)> {
    let find = |key: &str| rt.nodes.iter().find(|n| n.path.ends_with(key)).map(|n| n.id);
    let (s, l) = (find("/sheen")?, find("/lit")?);
    let off = |id| rt.scene.get(id, prop::OFFSET).map_or(0.0, |v| v.x);
    Some((off(s), off(l)))
}

#[test]
fn a_shimmer_stopped_while_waiting_restarts_in_step() {
    let mut bad = Vec::new();
    for stop_at in [0.2f64, 0.7, 1.2] {
        for gap in [0.05f64, 0.5] {
            // Mounted 0.8 s into a 2.13 s pass: both loops wait 1.33 s for the next.
            let s = Shimmer { since: -0.8, band: 140.0, ..base() };
            let mut rt = shimmer_rt(s);
            rt.frame(0.0);
            run(&mut rt, (stop_at * 120.0) as usize);
            rt.state.on = false;
            run(&mut rt, (gap * 120.0) as usize);
            rt.state.on = true;
            rt.state.since = rt.state.now;
            let mut worst = 0.0f32;
            for _ in 0..(period(&s) * 1.5 * 120.0) as usize {
                rt.state.now += 1.0 / 120.0;
                rt.invalidate();
                rt.frame(1.0 / 120.0);
                if let Some((a, b)) = loop_offsets(&rt) {
                    let d = a + b;
                    if d.abs() > worst.abs() { worst = d; }
                }
            }
            if worst.abs() > 0.01 { bad.push((stop_at, gap, worst)); }
        }
    }
    assert!(bad.is_empty(), "the bright copy's slide falls out of step with its window (stopped at, gap, pt): {bad:?}");
}

#[test]
fn a_shimmer_under_a_hovering_pointer_and_late_frames_stays_in_step() {
    let mut bad = Vec::new();
    for seed in 0..6u64 {
        let s = Shimmer { since: -0.8, band: 140.0, ring: true, tail: true, zoom: 1.6, pad: 40.0, ..base() };
        let mut rt = shimmer_rt(s);
        rt.frame(0.0);
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut rnd = move || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x % 1000) as f64 / 1000.0 };
        let (mut worst, mut glyph) = (0.0f32, 0.0f32);
        for k in 0..900 {
            let r = rnd();
            let (catch_up, dt) = if r < 0.05 { (0.15 + rnd() * 0.3, 1.0 / 120.0) } else if r < 0.15 { (1.0 / 60.0, 1.0 / 120.0) } else { (0.0, 1.0 / 120.0) };
            rt.state.now = rt.clock + catch_up + dt;
            if rnd() < 0.2 { rt.invalidate(); }
            if rnd() < 0.3 { rt.pointer_move(150.0 + rnd() as f32 * 120.0, 20.0 + rnd() as f32 * 20.0); }
            if k == 300 { rt.state.on = false; rt.invalidate(); }
            if k == 300 + (seed as usize % 3) * 4 + 1 { rt.state.on = true; rt.state.since = rt.state.now - 0.3; rt.invalidate(); }
            rt.frame_split(catch_up, dt);
            if let Some((a, b)) = loop_offsets(&rt) { if (a + b).abs() > worst.abs() { worst = a + b; } }
            glyph = worst_of(&rt, glyph);
        }
        if worst.abs() > 0.01 || !(glyph.abs() < GLYPH_PX) { bad.push((seed, worst, glyph)); }
    }
    assert!(bad.is_empty(), "out of step (seed, loop sum pt, glyph px): {bad:?}");
}

#[derive(Clone, Copy, Debug)]
enum Ev {
    /// No frame for this long, then one late frame, as `FrameClock` steps it.
    Stall(f64),
    /// Covered for this long: hidden wakes only, then uncovered (settles, as the window does).
    Covered(f64),
    /// Covered for this long, stopping at once and starting again halfway through.
    RestartCovered(f64),
    Stop,
    Start,
}

/// Steps `rt` like the window host for `secs` of 120 Hz frames, applying each due event and
/// recording the worst loop sum (pt) and, away from events, the worst glyph offset (px).
fn window(rt: &mut Runtime<Shimmer>, secs: f64, events: &mut Vec<(f64, Ev)>, worst: &mut (f32, f32)) {
    let start = rt.clock;
    let mut quiet_from = start + 0.6;
    while rt.clock < start + secs {
        let mut covered = false;
        let gap = match events.first().filter(|e| rt.clock - start >= e.0).map(|e| e.1) {
            None => 1.0 / 120.0,
            Some(ev) => {
                events.remove(0);
                quiet_from = quiet_from.max(rt.clock + 1.5);
                match ev {
                    Ev::Stall(g) => g,
                    Ev::Stop => { rt.state.on = false; 1.0 / 120.0 }
                    Ev::Start => { rt.state.on = true; rt.state.since = rt.clock; 1.0 / 120.0 }
                    Ev::Covered(g) | Ev::RestartCovered(g) => {
                        rt.set_live_visible(false);
                        let end = rt.clock + g;
                        let restart = matches!(ev, Ev::RestartCovered(_));
                        if restart { rt.state.on = false; }
                        while rt.clock < end {
                            rt.clock += 0.05;
                            rt.state.now = rt.clock;
                            if restart && !rt.state.on && rt.clock >= end - g / 2.0 { rt.state.on = true; rt.state.since = rt.clock; }
                            rt.invalidate();
                            rt.hidden_wake();
                        }
                        rt.set_live_visible(true);
                        rt.settle_all();
                        covered = true;
                        g
                    }
                }
            }
        };
        let step = crate::frame::clock::split(gap, rt.animating);
        if !covered { rt.clock += (gap - step.total()).max(0.0); }
        rt.state.now = rt.clock + step.total();
        rt.invalidate();
        rt.frame_split(step.catch_up, step.dt);
        // A copy mounted mid-pass waits at its start for the next pass, while the window runs.
        let waiting = |key: &str| rt.nodes.iter().find(|n| n.path.ends_with(key)).and_then(|n| rt.loops.get(&n.id))
            .is_some_and(|l| l.pending_start(rt.t).is_some());
        if waiting("/sheen") != waiting("/lit") { continue; }
        if let Some((a, b)) = loop_offsets(rt) && (a + b).abs() > worst.0.abs() { worst.0 = a + b; }
        if rt.clock >= quiet_from && rt.state.on { worst.1 = worst_of(rt, worst.1); }
    }
}

#[test]
fn a_shimmer_through_what_a_real_window_does_stays_in_step() {
    let s = Shimmer { since: -0.8, band: 140.0, ring: true, tail: true, zoom: 1.6, pad: 40.0, ..base() };
    let mut cases: Vec<(String, Shimmer, Vec<(f64, Ev)>)> = Vec::new();
    for lit_at in [0.3, 1.6, 2.9] {
        cases.push((format!("lit_at={lit_at}"), Shimmer { lit_at, ..s }, vec![]));
        cases.push((format!("lit_at={lit_at} covered"), Shimmer { lit_at, ..s }, vec![(lit_at - 0.2, Ev::Covered(0.5))]));
    }
    for at in [0.5, 1.7, 2.0] {
        for g in [0.2, 0.6, 3.0] {
            cases.push((format!("stall at={at} g={g}"), s, vec![(at, Ev::Stall(g))]));
        }
        for g in [0.3, 2.0, 6.0] {
            cases.push((format!("covered at={at} g={g}"), s, vec![(at, Ev::Covered(g))]));
            cases.push((format!("restart covered at={at} g={g}"), s, vec![(at, Ev::RestartCovered(g))]));
        }
        for off in [0.02, 0.2] {
            cases.push((format!("restart at={at} off={off} stall"), s, vec![(at, Ev::Stop), (at + off, Ev::Start), (at + off + 0.1, Ev::Stall(0.4))]));
        }
    }
    let mut bad = Vec::new();
    for (tag, s, mut events) in cases {
        let mut rt = shimmer_rt(s);
        rt.frame(0.0);
        let mut worst = (0.0f32, 0.0f32);
        window(&mut rt, 7.0, &mut events, &mut worst);
        if worst.0.abs() > 0.01 || !(worst.1.abs() < GLYPH_PX) { bad.push((tag, worst)); }
    }
    assert!(bad.is_empty(), "out of step (case, (loop sum pt, glyph px)): {bad:?}");
}

/// Drives `rt` like a window: a frame every 1/120 s while it animates, otherwise asleep until
/// its wake deadline. The app clock follows the host clock; the view commits only at `events`
/// (host time, change to the state).
fn host(rt: &mut Runtime<Shimmer>, until: f64, events: &mut Vec<(f64, fn(&mut Shimmer))>) -> f32 {
    let mut worst = 0.0f32;
    let mut dt: f64 = 1.0 / 120.0;
    while rt.clock < until {
        let next = events.first().map_or(f64::INFINITY, |e| e.0);
        let dt_now = dt.min((next - rt.clock).max(0.0));
        rt.state.now = rt.clock + dt_now;
        if events.first().is_some_and(|e| e.0 <= rt.clock + dt_now + 1e-9) {
            let (_, f) = events.remove(0);
            f(&mut rt.state);
            rt.state.since = if rt.state.on && rt.state.since < 0.0 { rt.state.now } else { rt.state.since };
            rt.invalidate();
        }
        let animating = rt.frame(dt_now);
        worst = worst_of(rt, worst);
        dt = if animating { 1.0 / 120.0 } else { rt.wake_deadline().map_or(0.25, |d| (d - rt.clock).max(1.0 / 120.0)) };
    }
    worst
}

#[test]
fn a_shimmer_on_a_sleeping_host_keeps_its_bright_copy_on_the_label() {
    let mut bad = Vec::new();
    for zoom in [1.0f32, 1.6] {
        for gap in [0.05f64, 0.4, 1.3] {
            for stop_at in [0.7f64, 1.6, 2.05] {
                let s = Shimmer { since: -0.8, ring: true, band: 140.0, tail: true, zoom, ..base() };
                let mut rt = shimmer_rt(s);
                rt.frame(0.0);
                let mut events: Vec<(f64, fn(&mut Shimmer))> = vec![
                    (stop_at, |s| s.on = false),
                    (stop_at + gap, |s| { s.on = true; s.since = -1.0; }),
                    (stop_at + gap + 1.1, |s| s.zoom += 0.0),
                ];
                let w = host(&mut rt, stop_at + gap + 3.0 * period(&s), &mut events);
                if !(w.abs() < GLYPH_PX) { bad.push((zoom, gap, stop_at, w)); }
            }
        }
    }
    assert!(bad.is_empty(), "the bright copy drifts off the label on a sleeping host (zoom, gap, stopped at, px): {bad:?}");
}
