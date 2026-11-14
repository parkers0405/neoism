//! `snap`, `flash` and `changes-twice` on what an element paints: each colour, corner, blur,
//! shadow, effect payload and image it presents (see [`paint::CHANNELS`]), and its place in the
//! paint order against what it overlaps (`z-order`). A colour that springs or fades has no
//! one-frame step and passes. Changes an input asked for on its frame, a hover or press highlight
//! landing (it lands with the pointer, even on content sliding under a still one), a
//! `Tx::instant` commit, an `El::instant_changes` element, a matching `El::motion_intent` mark,
//! and a `motion(Spring::NONE)` written on the element itself (for what that spring governs) are
//! the design.

use std::collections::{HashMap, HashSet};

use super::paint::{self, CHANNELS, Paint};
use super::series::{self, Tune};
use super::{EXITING, Finding, GROWING, LIVE, Recording, Snap, Track, area, on_window, rule, seen, short};
use crate::el::motion::MotionIntent;

/// The `MotionIntent` marks that make a finding of `kind` the design.
pub fn intent_for(kind: &str) -> MotionIntent {
    match kind {
        "overshoot" => MotionIntent::OVERSHOOT,
        "reversal" => MotionIntent::RECOIL | MotionIntent::JIGGLE,
        "jiggle" => MotionIntent::JIGGLE,
        "jerk" | "stop-start" => MotionIntent::IMPULSE,
        "jump" | "appear-then-jump" | "offset-jump" | "changes-twice" => MotionIntent::STEP,
        "pop-in" | "pop-out" | "cut-exit" | "fade-snap" | "text-swap" => MotionIntent::CUT,
        "flicker" | "flash" => MotionIntent::PULSE,
        "snap" => MotionIntent::STEP | MotionIntent::CUT,
        _ => MotionIntent::NONE,
    }
}

/// Element tunes: a distance of 1 is a change seen at once (see `paint::CHANNELS`).
pub fn tune(hz: f64) -> Tune {
    Tune {
        snap: rule("snap"),
        flash: rule("flash"),
        still: 0.04,
        horizon: (0.4 * hz).round() as usize,
        rest: (2, (rule("changes-twice") as f64 * hz).round() as usize),
    }
}

/// Whether a change on frame `k` of `t` is marked as the design for `kind`.
pub fn marked(t: &Track, k: usize, kind: &str) -> bool {
    let bits = intent_for(kind);
    bits != MotionIntent::NONE && [k.saturating_sub(1), k].iter().any(|&j| t.paint_at(j).is_some_and(|p| p.intends(bits)))
}

/// Drops findings an element marked with the matching `El::motion_intent` (or sitting in one).
pub fn intended(rec: &Recording, out: &mut Vec<Finding>) {
    let mut by_name: HashMap<String, Vec<&Track>> = HashMap::new();
    for t in rec.tracks.iter().filter(|t| t.paint.iter().any(|p| p.as_ref().is_some_and(|p| p.intent != 0))) {
        by_name.entry(short(&t.path)).or_default().push(t);
    }
    if by_name.is_empty() { return; }
    out.retain(|f| !by_name.get(&f.element).is_some_and(|ts| ts.iter().any(|t| marked(t, f.frame, f.kind))));
}

fn shows(s: &Snap, size: (f32, f32)) -> bool {
    seen(s, size) && s.op >= rule("faint") && s.flags & (EXITING | LIVE) == 0
}

/// Every channel's discontinuities, for each track.
pub fn painted(rec: &Recording, loops: &HashSet<usize>) -> Vec<(usize, Finding)> {
    let tune = tune(rec.hz);
    let n = rec.frames;
    let mut out = Vec::new();
    let input_in = |a: usize, b: usize| rec.inputs.iter().filter(|x| x.0 >= a && x.0 <= b).count();
    for (i, t) in rec.tracks.iter().enumerate() {
        if loops.contains(&i) || t.paint.is_empty() { continue; }
        // Frame → (presented state, paint) where it shows and changes only by animation.
        let at: Vec<Option<(&Snap, &Paint)>> = (0..n).map(|k| {
            let s = t.at(k)?;
            let p = t.paint_at(k)?;
            (k >= rec.start && shows(s, rec.size) && !p.instant).then_some((s, p))
        }).collect();
        if at.iter().all(Option::is_none) { continue; }
        for (c, ch) in CHANNELS.iter().enumerate() {
            // An element's own `motion(Spring::NONE)` lands what it governs at once by design.
            let vals: Vec<Option<series::V>> = at.iter().map(|x| x.filter(|(_, p)| !p.at_once(c)).map(|(_, p)| (ch.embed)(p))).collect();
            let w = |k: usize| -> f32 {
                let one = |j: usize| at.get(j).copied().flatten().map_or(0.0, |(s, p)| (ch.weight)(p, &on_window(&s.vis, rec.size)) * s.op.min(1.0));
                one(k).max(one(k.wrapping_sub(1)))
            };
            // A hover or press highlight landing, even on content that slid under a still pointer.
            let pointer = |a: usize, b: usize| {
                ch.name == "fill" && (a.max(1)..=b).any(|j| at[j].zip(at[j - 1]).is_some_and(|((_, p), (_, q))| p.pointer != q.pointer))
            };
            for e in series::events(&vals, &tune, &w) {
                let k = e.k;
                let skip = match e.kind {
                    "snap" => input_in(k.saturating_sub(1), k) > 0 || rec.instant.get(k) == Some(&true) || pointer(k, k),
                    "flash" => input_in(e.span.0 + 1, e.span.1) >= 2 || pointer(e.span.0 + 1, e.span.1),
                    _ => input_in(e.span.0 + 1, e.span.1) > 0 || pointer(e.span.0 + 1, e.span.1),
                };
                if skip || marked(t, k, e.kind) || rec.loads.iter().any(|&l| k >= l && k <= l + 2) { continue; }
                let v = |j: usize| at.get(j).copied().flatten().map_or_else(|| "–".into(), |(_, p)| paint::value(p, ch.name));
                let detail = match e.kind {
                    "snap" => format!("{} {} → {} in one frame", ch.name, v(k - 1), v(k)),
                    "flash" => format!("{} {} → {} → {} over frames {}–{}", ch.name, v(e.span.0), v(k), v(e.span.1), e.span.0, e.span.1),
                    _ => format!("{} {} → {}, rests, then → {} over frames {}–{}", ch.name, v(e.span.0), v(k - 1), v(e.span.1), e.span.0, e.span.1),
                };
                out.push((i, Finding { kind: e.kind, element: short(&t.path), frame: k, detail }));
            }
        }
    }
    out.extend(z_order(rec));
    out
}

/// Two overlapping opaque surfaces trading places in the paint order in one frame: one shows
/// through where the other was. Back within the flash window, it's a `flash`.
fn z_order(rec: &Recording) -> Vec<(usize, Finding)> {
    let n = rec.frames;
    let horizon = (0.4 * rec.hz).round() as usize;
    let tr = &rec.tracks;
    let inside = |a: &str, b: &str| a.len() > b.len() && a.starts_with(b) && a.as_bytes()[b.len()] == b'/';
    // Frame → the solid surfaces showing, with their rank and visible part.
    let solid = |k: usize| -> Vec<(usize, u32, [f32; 4])> {
        tr.iter().enumerate().filter_map(|(i, t)| {
            let (s, p) = (t.at(k)?, t.paint_at(k)?);
            (shows(s, rec.size) && s.flags & GROWING == 0 && s.op >= 0.5 && p.opaque() && area(&s.vis) >= 64.0).then_some((i, p.rank, s.vis))
        }).collect()
    };
    let meets = |a: &[f32; 4], b: &[f32; 4]| area(&meet(a, b)) >= 64.0;
    let mut out = Vec::new();
    let mut prev = solid(rec.start);
    let mut flipped: HashSet<(usize, usize)> = HashSet::new();
    for k in rec.start + 1..n {
        let cur = solid(k);
        let before: HashMap<usize, (u32, [f32; 4])> = prev.iter().map(|&(i, r, v)| (i, (r, v))).collect();
        for (x, &(a, ra, va)) in cur.iter().enumerate() {
            let Some(&(pa, pva)) = before.get(&a) else { continue };
            for &(b, rb, vb) in &cur[x + 1..] {
                let Some(&(pb, pvb)) = before.get(&b) else { continue };
                if (ra > rb) == (pa > pb) || !meets(&va, &vb) || !meets(&pva, &pvb) || inside(&tr[a].path, &tr[b].path) || inside(&tr[b].path, &tr[a].path) { continue; }
                if rec.inputs.iter().any(|x| x.0 + 1 >= k && x.0 <= k) || flipped.contains(&(a.min(b), a.max(b))) { continue; }
                // Back within the window: the order it had returns.
                let back = (k + 1..n.min(k + horizon + 1)).find(|&j| {
                    let (Some(p), Some(q)) = (tr[a].paint_at(j), tr[b].paint_at(j)) else { return false };
                    (p.rank > q.rank) == (pa > pb)
                });
                let kind = if back.is_some() { "flash" } else { "snap" };
                if kind == "snap" && rec.instant.get(k) == Some(&true) { continue; }
                // Reported at the one whose place changed most.
                let (who, other) = if (ra as i64 - pa as i64).abs() >= (rb as i64 - pb as i64).abs() { (a, b) } else { (b, a) };
                if marked(&tr[who], k, kind) { continue; }
                let over = if who == a { ra > rb } else { rb > ra };
                flipped.insert((a.min(b), a.max(b)));
                out.push((who, Finding { kind, element: short(&tr[who].path), frame: k,
                    detail: format!("z-order: paints {} {} in one frame{}", if over { "over" } else { "under" }, short(&tr[other].path),
                        back.map_or(String::new(), |j| format!(", and back at frame {j}"))) }));
            }
        }
        prev = cur;
    }
    out
}

/// Where two rects `[x, y, w, h]` overlap.
pub fn meet(a: &[f32; 4], b: &[f32; 4]) -> [f32; 4] {
    let (x0, y0) = (a[0].max(b[0]), a[1].max(b[1]));
    [x0, y0, ((a[0] + a[2]).min(b[0] + b[2]) - x0).max(0.0), ((a[1] + a[3]).min(b[1] + b[3]) - y0).max(0.0)]
}
