//! Which tracks loop: motion that keeps going of its own accord, its wraps and turns its design.
//!
//! A loop is told from the trace. A clock loop (`El::looping`) reports a phase: a periodic one
//! wraps from near 1 to 0 and steps on until the capture ends, while a finite sequence (a
//! one-shot fade) climbs from 0 to 1 and holds 1 at rest, which is a transition that settled,
//! not a loop. Without a phase, an element that changes on most frames and keeps turning back
//! loops, unless its turns follow inputs: a list a person keeps toggling turns too.

use std::collections::{HashMap, HashSet};

use super::{Finding, Recording, Snap, Track, rule, short};

/// A finite sequence that played out: its phase rests at 1, which a periodic phase never reaches.
fn plays_out(t: &Track) -> bool {
    t.frames.windows(2).any(|w| w[0].1.phase == 1.0 && w[1].1.phase == 1.0)
}

/// Its phase wraps, or steps on to the capture's last frame, and it never played out.
fn periodic_phase(t: &Track, n: usize) -> bool {
    let ph: Vec<(usize, f32)> = t.frames.iter().filter(|f| !f.1.phase.is_nan()).map(|f| (f.0, f.1.phase)).collect();
    let steps = || ph.windows(2).filter(|w| w[1].0 == w[0].0 + 1);
    let wraps = steps().any(|w| w[1].1 < w[0].1 - 0.5);
    let to_end = ph.last().is_some_and(|l| l.0 + 1 == n) && steps().last().is_some_and(|w| w[1].1 != w[0].1);
    (wraps || to_end) && !plays_out(t)
}

/// Every track that loops, with everything inside one.
pub(super) fn loops(rec: &Recording) -> HashSet<usize> {
    let tr = &rec.tracks;
    // How long an input's answer may take: its spring, the scroll that follows it.
    let answer = (0.5 * rec.hz) as usize;
    let answering = |k: usize| rec.inputs.iter().any(|i| i.0 <= k && k <= i.0 + answer);
    let changed = |a: &Snap, b: &Snap| (0..4).any(|i| (a.rect[i] - b.rect[i]).abs() > 0.01) || (a.op - b.op).abs() > 0.002;
    let mut busy = vec![0usize; tr.len()];
    let mut group_busy: HashMap<usize, HashSet<usize>> = HashMap::new();
    for (i, t) in tr.iter().enumerate() {
        for w in t.frames.windows(2) {
            if w[1].0 == w[0].0 + 1 && changed(&w[0].1, &w[1].1) {
                busy[i] += 1;
                if let Some(p) = t.parent { group_busy.entry(p).or_default().insert(w[1].0); }
            }
        }
    }
    // Busy alone would take a long stream or a slow spring for a loop; a loop also comes back,
    // by itself: a turn soon after an input answers it.
    let periodic = |t: &Track| {
        let turns = |v: &dyn Fn(&Snap) -> f32, eps: f32| {
            let (mut n, mut last, mut dir) = (0, v(&t.frames[0].1), 0.0f32);
            for f in &t.frames[1..] {
                let x = v(&f.1);
                if (x - last).abs() > eps {
                    let s = (x - last).signum();
                    if dir != 0.0 && s != dir && !answering(f.0) { n += 1; }
                    dir = s;
                    last = x;
                }
            }
            n
        };
        turns(&|s| s.op, 0.01) >= 4 || (0..4).any(|a| turns(&|s| s.rect[a], 0.25) >= 4)
    };
    let busy_most = |i: usize, frames: usize| tr[i].frames.len() >= 60 && frames * 10 >= tr[i].frames.len() * 6;
    let mut loops: HashSet<usize> = (0..tr.len())
        .filter(|&i| periodic_phase(&tr[i], rec.frames) || busy_most(i, busy[i]) && periodic(&tr[i]))
        .collect();
    // Siblings taking turns (a typing indicator's dots) loop together.
    for (&p, ks) in &group_busy {
        if busy_most(p, ks.len()) && tr.iter().any(|c| c.parent == Some(p) && c.frames.len() >= 2 && periodic(c)) {
            loops.extend((0..tr.len()).filter(|&c| tr[c].parent == Some(p)));
        }
    }
    loop {
        let more: Vec<usize> = (0..tr.len()).filter(|i| !loops.contains(i) && tr[*i].parent.is_some_and(|p| loops.contains(&p))).collect();
        if more.is_empty() { break; }
        loops.extend(more);
    }
    loops
}

/// A periodic loop keeps its rhythm: each moving frame steps its phase as the last did, and one
/// back from a blink comes back where its rhythm has got to. One that restarted or skipped (a
/// remount, or a new loop where one was running) steps off it. A phase held still (a delay,
/// reduced motion) and a sequence that played out aren't checked.
pub(super) fn jumps(rec: &Recording, out: &mut Vec<Finding>) {
    let blink = (0.1 * rec.hz) as usize;
    for t in rec.tracks.iter().filter(|t| !plays_out(t)) {
        let ph: Vec<(usize, f32)> = t.frames.iter().filter(|f| !f.1.phase.is_nan()).map(|f| (f.0, f.1.phase)).collect();
        // (frame, frames since the last, phase step)
        let steps: Vec<(usize, usize, f32)> = ph.windows(2).filter(|w| w[1].0 - w[0].0 <= blink)
            .map(|w| (w[1].0, w[1].0 - w[0].0, (w[1].1 - w[0].1).rem_euclid(1.0)))
            .collect();
        // A step out of a hold starts partway through a frame.
        let moving: Vec<(usize, usize, f32)> = steps.windows(2).filter(|w| w[0].2 > 0.0 && w[1].2 > 0.0).map(|w| w[1]).collect();
        let mut sorted: Vec<f32> = moving.iter().filter(|s| s.1 == 1).map(|s| s.2).collect();
        if sorted.len() < 4 { continue; }
        sorted.sort_by(f32::total_cmp);
        let usual = sorted[sorted.len() / 2];
        let off = |&(_, frames, d): &(usize, usize, f32)| {
            let x = (d - frames as f32 * usual).rem_euclid(1.0);
            x.min(1.0 - x)
        };
        if let Some(&(k, frames, d)) = moving.iter().find(|s| off(s) > rule("loop-jump")) {
            let over = if frames > 1 { format!(" over {frames} frames") } else { String::new() };
            out.push(Finding { kind: "loop-jump", element: short(&t.path), frame: k, detail: format!("phase stepped {d:.3}{over} where it steps {usual:.3} a frame") });
        }
    }
}
