//! The three discontinuities of one value over frames, shared by an element's painted
//! properties and a composited region of the window:
//!
//! - `snap`: it steps in one frame between frames that hold (nearly) still.
//! - `flash`: it steps out of a still state and within `horizon` frames comes back at least
//!   halfway, or drifts away and snaps back. A one-frame blink, or a surface that drops out and
//!   fades back in, is one.
//! - `changes-twice`: it changes, rests a moment, then carries on the same way, where one change
//!   was meant.
//!
//! A cross-fade or a spring transition moves a little each frame and has no isolated step, so
//! none of them fires on it.

/// A value at one frame, embedded so that Euclidean distance is how different it looks.
pub type V = [f32; 6];

pub fn dist(a: &V, b: &V) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

/// How far `p` is from the straight path `a` → `b`.
pub fn off_path(p: &V, a: &V, b: &V) -> f32 {
    let ab: Vec<f32> = (0..6).map(|i| b[i] - a[i]).collect();
    let len2: f32 = ab.iter().map(|x| x * x).sum();
    if len2 < 1e-12 { return dist(p, a); }
    let t = ((0..6).map(|i| (p[i] - a[i]) * ab[i]).sum::<f32>() / len2).clamp(0.0, 1.0);
    let q: V = std::array::from_fn(|i| a[i] + ab[i] * t);
    dist(p, &q)
}

/// Thresholds, in the units of the embedding, and windows in frames.
#[derive(Clone, Copy)]
pub struct Tune {
    /// A one-frame step at least this big is a `snap`.
    pub snap: f32,
    /// An excursion at least this big is a `flash`, and each half of a `changes-twice`.
    pub flash: f32,
    /// A change under this between frames is holding still.
    pub still: f32,
    /// Frames a flash comes back within.
    pub horizon: usize,
    /// Frames of rest between the two halves of a `changes-twice`.
    pub rest: (usize, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub kind: &'static str,
    /// The frame it shows on: the step, or where the second change starts.
    pub k: usize,
    /// The frames it spans, first to last.
    pub span: (usize, usize),
    /// Its size in the embedding, before weighting.
    pub size: f32,
}

/// Every discontinuity in `vals` (`None` where there's no value). `weight(k)` scales what a
/// change at frame `k` counts for: how much of it shows.
pub fn events(vals: &[Option<V>], tune: &Tune, weight: &dyn Fn(usize) -> f32) -> Vec<Event> {
    let n = vals.len();
    let d = |k: usize| -> Option<f32> {
        if k == 0 || k >= n { return None; }
        Some(dist(vals[k].as_ref()?, vals[k - 1].as_ref()?))
    };
    let still = |k: usize| d(k).is_none_or(|x| x < tune.still);
    let mut out = Vec::new();
    let mut taken = vec![false; n];
    for k in 1..n {
        let Some(step) = d(k) else { continue };
        let w = weight(k);
        if w <= 0.0 || step * w < tune.flash.min(tune.snap) { continue; }
        let (prev, next) = (d(k - 1).unwrap_or(0.0), d(k + 1).unwrap_or(0.0));
        let (a, b) = (vals[k - 1].unwrap(), vals[k].unwrap());
        // Out of a still state: where it rests within the horizon.
        if prev < step / 4.0 {
            let mut m = k + 1;
            while m < n.min(k + tune.horizon + 1) && vals[m].is_some() && !(still(m) && still(m + 1)) { m += 1; }
            let rest = vals[m.min(n - 1)].or(vals[m - 1]).unwrap();
            let e = off_path(&b, &a, &rest);
            if e >= 0.5 * step && e * w >= tune.flash {
                out.push(Event { kind: "flash", k, span: (k - 1, m.min(n - 1)), size: e });
                taken[k] = true;
                if let Some(back) = (k + 1..=m.min(n - 1)).find(|&j| d(j).is_some_and(|x| x >= step / 2.0)) { taken[back] = true; }
                continue;
            }
        }
        // Into a still state after drifting from one: a snap back.
        if next < step / 4.0 && !taken[k] {
            let from = k.saturating_sub(tune.horizon).max(2);
            if let Some(j) = (from..k - 1).rev().find(|&j| still(j) && still(j - 1) && vals[j].is_some())
                && (j + 1..k).all(|i| d(i).is_some_and(|x| x < step / 2.0))
            {
                let s = vals[j].unwrap();
                let e = off_path(&a, &s, &b);
                if e >= 0.5 * step && e * w >= tune.flash {
                    out.push(Event { kind: "flash", k, span: (j, k), size: e });
                    taken[k] = true;
                    continue;
                }
            }
        }
        if !taken[k] && step * w >= tune.snap && prev < step / 4.0 && next < step / 4.0 {
            out.push(Event { kind: "snap", k, span: (k - 1, k), size: step });
        }
    }
    // Motion segments: runs of frames that change, then the rest between them.
    let mut segs: Vec<(usize, usize)> = Vec::new();
    let mut k = 1;
    while k < n {
        if d(k).is_none_or(|x| x < tune.still) { k += 1; continue; }
        let a = k;
        while k < n && d(k).is_some_and(|x| x >= tune.still) { k += 1; }
        segs.push((a, k - 1));
    }
    for w in segs.windows(2) {
        let ((a1, b1), (a2, b2)) = (w[0], w[1]);
        let rest = a2 - b1 - 1;
        if rest < tune.rest.0 || rest > tune.rest.1 || (a1..=b2).any(|j| vals[j].is_none()) { continue; }
        // The second change counts from where the rest ends: a slow tail that creeps on below
        // `still` and then lands is one change.
        let (v0, v1, r, v2) = (vals[a1 - 1].unwrap(), vals[b1].unwrap(), vals[a2 - 1].unwrap(), vals[b2].unwrap());
        let (first, second) = (dist(&v0, &v1), dist(&r, &v2));
        let wk = weight(a2);
        if first * wk < tune.flash || second * wk < tune.flash { continue; }
        let dot: f32 = (0..6).map(|i| (v1[i] - v0[i]) * (v2[i] - r[i])).sum();
        // Each half one step, so neither is a transition: those are `snap`'s alone.
        if dot > 0.7 * first * second && !(taken[a2] || out.iter().any(|e| e.k == a1 || e.k == a2)) {
            out.push(Event { kind: "changes-twice", k: a2, span: (a1 - 1, b2), size: first + second });
        }
    }
    out.sort_by_key(|e| e.k);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TUNE: Tune = Tune { snap: 1.0, flash: 0.5, still: 0.01, horizon: 24, rest: (2, 15) };

    fn run(xs: &[f32]) -> Vec<&'static str> {
        let vals: Vec<Option<V>> = xs.iter().map(|&x| Some([x, 0.0, 0.0, 0.0, 0.0, 0.0])).collect();
        events(&vals, &TUNE, &|_| 1.0).into_iter().map(|e| e.kind).collect()
    }

    fn ease(from: f32, to: f32, frames: usize) -> Vec<f32> {
        (1..=frames).map(|i| { let t = i as f32 / frames as f32; from + (to - from) * t * t * (3.0 - 2.0 * t) }).collect()
    }

    #[test]
    fn a_step_between_still_frames_snaps() {
        assert_eq!(run(&[0.0, 0.0, 0.0, 2.0, 2.0, 2.0]), ["snap"]);
    }

    #[test]
    fn a_transition_neither_snaps_nor_flashes() {
        let mut xs = vec![0.0; 4];
        xs.extend(ease(0.0, 2.0, 12));
        xs.extend([2.0; 6]);
        assert!(run(&xs).is_empty(), "{:?}", run(&xs));
    }

    #[test]
    fn a_cross_fade_through_a_dip_is_no_flash() {
        // Two layers trading places: the mix dips a little and comes back, smoothly.
        let xs: Vec<f32> = (0..30).map(|k| if k < 4 { 1.0 } else { 1.0 - 0.3 * ((k - 4) as f32 / 20.0 * std::f32::consts::PI).sin().max(0.0) }).collect();
        assert!(run(&xs).is_empty(), "{:?}", run(&xs));
    }

    #[test]
    fn dropping_out_and_fading_back_flashes() {
        let mut xs = vec![2.0; 4];
        xs.push(0.0);
        xs.extend(ease(0.0, 2.0, 12));
        xs.extend([2.0; 6]);
        assert_eq!(run(&xs), ["flash"]);
    }

    #[test]
    fn a_one_frame_blink_flashes_once() {
        assert_eq!(run(&[1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0]), ["flash"]);
    }

    #[test]
    fn drifting_away_and_snapping_back_flashes() {
        let mut xs = vec![0.0; 4];
        xs.extend(ease(0.0, 1.5, 10));
        xs.extend([0.0; 6]);
        assert_eq!(run(&xs), ["flash"]);
    }

    #[test]
    fn two_steps_the_same_way_snap_each_and_no_more() {
        assert_eq!(run(&[0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0]), ["snap", "snap"]);
    }

    #[test]
    fn a_transition_that_rests_and_carries_on_changes_twice() {
        let mut xs = vec![0.0; 3];
        xs.extend(ease(0.0, 1.0, 8));
        xs.extend([1.0; 4]);
        xs.extend(ease(1.0, 2.0, 8));
        xs.extend([2.0; 4]);
        assert_eq!(run(&xs), ["changes-twice"]);
    }

    #[test]
    fn a_tail_that_creeps_then_lands_changes_once() {
        let mut xs = vec![0.0; 3];
        xs.extend(ease(0.0, 1.0, 8));
        xs.extend((1..=14).map(|i| 1.0 + 0.009 * i as f32));
        let last = *xs.last().unwrap();
        xs.extend([last + 0.4; 6]);
        assert!(run(&xs).is_empty(), "{:?}", run(&xs));
    }

    #[test]
    fn going_there_and_back_later_changes_once_each_way() {
        let mut xs = vec![0.0; 3];
        xs.extend(ease(0.0, 1.0, 8));
        xs.extend([1.0; 4]);
        xs.extend(ease(1.0, 0.0, 8));
        xs.extend([0.0; 4]);
        assert!(run(&xs).is_empty(), "{:?}", run(&xs));
    }
}
