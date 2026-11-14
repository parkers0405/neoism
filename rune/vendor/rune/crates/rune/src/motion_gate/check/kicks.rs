//! `stop-start`: an edge that travels in kicks. Each time data arrives it sets off, slows, rests
//! a moment and sets off again the same way, like a reply's scroll that stops and kicks per
//! line, where one steady glide was meant. Each kick passes `jerk` and `jump` on its own.

use std::collections::HashSet;

use super::{Finding, Recording, edge, rule, seen, short};

/// A frame step under this many points is no motion.
const STILL: f32 = 0.005;
/// Points a kick travels before it rests: a creep in under a pixel a frame isn't one.
const KICK: f32 = 0.25;
/// Frames still before setting off again counts as a rest.
const REST: usize = 3;
/// Rests up to this many seconds are a stop within one travel; a longer one ends it.
const PAUSE: f64 = 0.5;
/// Seconds the restarts are counted over.
const WINDOW: f64 = 2.0;

pub(super) fn kicks(rec: &Recording, loops: &HashSet<usize>, out: &mut Vec<Finding>) {
    let tr = &rec.tracks;
    let (pause, window) = ((PAUSE * rec.hz) as usize, (WINDOW * rec.hz) as usize);
    let need = rule("stop-start") as usize;
    // Data arriving (`event…`) is what kicks it; a hand on the app may start a motion each time.
    let touched = |a: usize, b: usize| rec.inputs.iter().any(|(k, what)| *k >= a && *k <= b && !what.starts_with("event"));
    let mut hits: Vec<(usize, Finding)> = Vec::new();
    for (i, t) in tr.iter().enumerate().filter(|(i, _)| !loops.contains(i)) {
        for axis in 0..2 {
            let mut restarts: Vec<usize> = Vec::new();
            let (mut prev, mut dir, mut still, mut went) = (None::<(usize, f32)>, 0.0, 0, 0.0);
            for (k, s) in &t.frames {
                if *k <= rec.start || s.op < rule("faint") || !seen(s, rec.size) {
                    (prev, dir, still, went) = (None, 0.0, 0, 0.0);
                    continue;
                }
                let at = edge(s, axis);
                if let Some((_, was)) = prev.filter(|p| p.0 + 1 == *k) {
                    let d = at - was;
                    if d.abs() < STILL {
                        still += 1;
                    } else {
                        if still >= REST {
                            if still <= pause && d.signum() == dir && went >= KICK {
                                restarts.push(*k);
                            }
                            went = 0.0;
                        }
                        went += d.abs();
                        (dir, still) = (d.signum(), 0);
                    }
                }
                prev = Some((*k, at));
            }
            let burst = (0..restarts.len()).find(|&j| {
                restarts.get(j + need - 1).is_some_and(|&last| last - restarts[j] <= window && !touched(restarts[j], last))
            });
            if let Some(j) = burst {
                let k = restarts[j];
                let n = restarts[j..].iter().take_while(|&&r| r - k <= window).count();
                hits.push((i, Finding {
                    kind: "stop-start",
                    element: short(&t.path),
                    frame: k,
                    detail: format!("{} rests and sets off again {n} times in {WINDOW} s", ["x", "y"][axis]),
                }));
                break;
            }
        }
    }
    // A scroll's rows all kick together: report the outermost, once among its siblings and
    // everything inside them.
    let family = |i: usize| {
        let mut up = vec![i];
        let mut p = tr[i].parent;
        while let Some(j) = p {
            up.push(j);
            p = tr[j].parent;
        }
        up
    };
    let mut kept: Vec<(usize, usize)> = Vec::new();
    hits.sort_by_key(|(i, f)| (family(*i).len(), f.frame));
    for (i, f) in hits {
        let up = family(i);
        let near = |&(j, k): &(usize, usize)| k.abs_diff(f.frame) <= window && (up.contains(&j) || tr[j].parent.is_some_and(|p| up.contains(&p)));
        if kept.iter().any(near) {
            continue;
        }
        kept.push((i, f.frame));
        out.push(f);
    }
}
