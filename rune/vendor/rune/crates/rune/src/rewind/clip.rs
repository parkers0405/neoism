//! A frozen copy of the recording, for scrubbing: any frame's elements, what changed in it, how
//! much moved, and where one element went over time.

use super::tape::{FIELDS, Q, Tape};

/// Checkpoints every this many frames, so rebuilding a frame replays at most this many.
const STRIDE: usize = 64;

/// One element as a frame presented it: `[x, y, w, h]` in points and opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Shot {
    pub rect: [f32; 4],
    pub opacity: f32,
}

impl Shot {
    pub(crate) fn of(q: &Q) -> Shot {
        Shot { rect: [q[0], q[1], q[2], q[3]].map(|v| v as f32 / 100.0), opacity: q[4] as f32 / 1000.0 }
    }

    /// The largest change of an edge or side from `other`, in points.
    pub fn distance(&self, other: &Shot) -> f32 {
        let (a, b) = (self.rect, other.rect);
        let edges = |r: [f32; 4]| [r[0], r[1], r[0] + r[2], r[1] + r[3]];
        edges(a).iter().zip(edges(b)).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
    }
}

/// An element whose rect or opacity changed in a frame.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Change {
    /// The element's id in this clip (stable for its whole run).
    pub id: u32,
    pub name: String,
    /// None: it appeared in this frame.
    pub before: Option<Shot>,
    /// None: it went away in this frame.
    pub after: Option<Shot>,
    /// It moved much further in this frame than in the frames either side: a likely glitch.
    pub jump: bool,
}

impl Change {
    /// Points it moved (its largest edge change); 0 when it appeared or went.
    pub fn distance(&self) -> f32 {
        match (self.before, self.after) {
            (Some(a), Some(b)) => a.distance(&b),
            _ => 0.0,
        }
    }

    /// A short label: `title ↓7.9 pt`, `card w +12 pt`, `toast 0.40→1.00`, `row appears`.
    pub fn label(&self) -> String {
        let (Some(a), Some(b)) = (self.before, self.after) else {
            return format!("{} {}", self.name, if self.after.is_some() { "appears" } else { "goes" });
        };
        let mut parts = Vec::new();
        let d = |x: f32| if x.abs() >= 10.0 { format!("{:.0}", x.abs()) } else { format!("{:.1}", x.abs()) };
        let (dx, dy) = (b.rect[0] - a.rect[0], b.rect[1] - a.rect[1]);
        let (dw, dh) = (b.rect[2] - a.rect[2], b.rect[3] - a.rect[3]);
        if dy.abs() >= 0.05 { parts.push(format!("{}{}", if dy > 0.0 { "↓" } else { "↑" }, d(dy))); }
        if dx.abs() >= 0.05 { parts.push(format!("{}{}", if dx > 0.0 { "→" } else { "←" }, d(dx))); }
        if dw.abs() >= 0.05 { parts.push(format!("w {}{}", if dw > 0.0 { "+" } else { "−" }, d(dw))); }
        if dh.abs() >= 0.05 { parts.push(format!("h {}{}", if dh > 0.0 { "+" } else { "−" }, d(dh))); }
        let mut label = self.name.clone();
        if !parts.is_empty() { label = format!("{label} {} pt", parts.join(" ")); }
        if (b.opacity - a.opacity).abs() >= 0.005 { label = format!("{label} {:.2}→{:.2}", a.opacity, b.opacity); }
        label
    }
}

/// The last stretch of a window's frames, frozen by [`Rewind::freeze`](super::Rewind::freeze).
pub struct Clip {
    pub(crate) names: Vec<String>,
    pub(crate) viewport: (f32, f32),
    times: Vec<f64>,
    starts: Vec<usize>,
    words: Vec<u32>,
    checkpoints: Vec<Vec<(u32, Q)>>,
    activity: Vec<f32>,
    jumps: Vec<Vec<u32>>,
}

impl Clip {
    pub(crate) fn from_tape(tape: &Tape) -> Clip {
        let words: Vec<u32> = tape.words.iter().copied().collect();
        let mut starts = Vec::with_capacity(tape.frames.len() + 1);
        let mut times = Vec::with_capacity(tape.frames.len());
        let mut at = 0;
        for m in &tape.frames {
            starts.push(at);
            times.push(m.t);
            at += m.len as usize;
        }
        starts.push(at);
        let mut clip = Clip {
            names: tape.names.clone(), viewport: tape.viewport, times, starts, words,
            checkpoints: Vec::new(), activity: Vec::new(), jumps: Vec::new(),
        };
        clip.index(&tape.base);
        clip
    }

    /// One pass over every frame: checkpoints, how far things moved, and jumps.
    fn index(&mut self, base: &[Option<Q>]) {
        let n = self.len();
        let mut state: Vec<Option<Q>> = base.to_vec();
        state.resize(self.names.len(), None);
        // Per frame: each element that moved, how far, and where it was before and after.
        let mut moved: Vec<Vec<(u32, f32, Shot, Shot)>> = Vec::with_capacity(n);
        self.activity = Vec::with_capacity(n);
        for k in 0..n {
            if k % STRIDE == 0 {
                self.checkpoints.push(state.iter().enumerate().filter_map(|(i, q)| q.map(|q| (i as u32, q))).collect());
            }
            let mut frame = Vec::new();
            let mut most = 0.0f32;
            let mut faded = false;
            Tape::decode(&self.words[self.starts[k]..self.starts[k + 1]], |id, mask, values| {
                let i = id as usize;
                if mask == 0 { state[i] = None; return; }
                let before = state[i];
                let mut q = before.unwrap_or([0; FIELDS]);
                for f in 0..FIELDS { if mask & (1 << f) != 0 { q[f] = values[f]; } }
                state[i] = Some(q);
                if let Some(b) = before {
                    let d = Shot::of(&b).distance(&Shot::of(&q));
                    if d > 0.0 { frame.push((id, d, Shot::of(&b), Shot::of(&q))); most = most.max(d); }
                    faded |= b[4] != q[4];
                }
            });
            // An opacity change alone still shows on the track, faintly.
            self.activity.push(if most == 0.0 && faded { 0.25 } else { most });
            moved.push(frame);
        }
        let find = |k: usize, id: u32| moved.get(k).and_then(|f| f.iter().find(|m| m.0 == id));
        self.jumps = (0..n).map(|k| {
            moved[k].iter().filter(|&&(id, d, was, _)| {
                let near = k.checked_sub(1).and_then(|p| find(p, id)).map_or(0.0, |m| m.1).max(find(k + 1, id).map_or(0.0, |m| m.1));
                // A blip: out in this frame and straight back in the next.
                let back = find(k + 1, id).is_some_and(|m| m.3.distance(&was) < 0.5);
                d >= 4.0 && (d > 2.5 * near || back)
            }).map(|m| m.0).collect()
        }).collect();
    }

    /// Frames in the clip.
    pub fn len(&self) -> usize { self.times.len() }

    pub fn is_empty(&self) -> bool { self.times.is_empty() }

    /// The window's size in points when the last frame was recorded.
    pub fn viewport(&self) -> (f32, f32) { self.viewport }

    /// Seconds from the last frame to frame `k` (0 for the last, negative before it).
    pub fn time(&self, k: usize) -> f64 {
        match (self.times.get(k), self.times.last()) {
            (Some(t), Some(last)) => t - last,
            _ => 0.0,
        }
    }

    /// Seconds the clip covers.
    pub fn duration(&self) -> f64 { -self.time(0) }

    /// The largest move of any element in frame `k`, in points (0.25 when only opacity changed).
    pub fn activity(&self, k: usize) -> f32 { self.activity.get(k).copied().unwrap_or(0.0) }

    /// Frame `k` has an element that jumped (see [`Change::jump`]).
    pub fn has_jump(&self, k: usize) -> bool { self.jumps.get(k).is_some_and(|j| !j.is_empty()) }

    /// An element's name: its trace name, key or path.
    pub fn name(&self, id: u32) -> &str { self.names.get(id as usize).map_or("", String::as_str) }

    /// Every element on screen after frame `k`.
    pub fn frame(&self, k: usize) -> Vec<(u32, Shot)> {
        self.state(k).iter().enumerate().filter_map(|(i, q)| q.map(|q| (i as u32, Shot::of(&q)))).collect()
    }

    /// Every element on screen just before frame `k`.
    pub(crate) fn before(&self, k: usize) -> Vec<(u32, Shot)> {
        let state = if k == 0 { self.checkpoint_state(0) } else { self.state(k - 1) };
        state.iter().enumerate().filter_map(|(i, q)| q.map(|q| (i as u32, Shot::of(&q)))).collect()
    }

    /// The elements whose rect or opacity changed in frame `k`, furthest moved first.
    pub fn changes(&self, k: usize) -> Vec<Change> {
        if k >= self.len() { return Vec::new(); }
        let before = if k == 0 { self.checkpoint_state(0) } else { self.state(k - 1) };
        let mut out = Vec::new();
        Tape::decode(&self.words[self.starts[k]..self.starts[k + 1]], |id, mask, values| {
            let b = before.get(id as usize).copied().flatten();
            let after = (mask != 0).then(|| {
                let mut q = b.unwrap_or([0; FIELDS]);
                for f in 0..FIELDS { if mask & (1 << f) != 0 { q[f] = values[f]; } }
                Shot::of(&q)
            });
            let jump = self.jumps[k].contains(&id);
            out.push(Change { id, name: self.name(id).to_owned(), before: b.map(|q| Shot::of(&q)), after, jump });
        });
        out.sort_by(|a, b| b.jump.cmp(&a.jump).then(b.distance().total_cmp(&a.distance())));
        out
    }

    /// Where element `id` was in each frame of `range` it was on screen.
    pub fn path(&self, id: u32, range: std::ops::Range<usize>) -> Vec<(usize, Shot)> {
        let range = range.start.min(self.len())..range.end.min(self.len());
        let mut state = if range.start == 0 { self.checkpoint_state(0) } else { self.state(range.start - 1) };
        let mut out = Vec::new();
        for k in range {
            self.apply(k, &mut state);
            if let Some(q) = state.get(id as usize).copied().flatten() { out.push((k, Shot::of(&q))); }
        }
        out
    }

    /// The next frame from `k` in direction `step` (±1) where anything moved.
    pub fn next_active(&self, k: usize, step: isize) -> Option<usize> {
        let mut i = k as isize + step;
        while i >= 0 && (i as usize) < self.len() {
            if self.activity(i as usize) > 0.0 { return Some(i as usize); }
            i += step;
        }
        None
    }

    fn checkpoint_state(&self, c: usize) -> Vec<Option<Q>> {
        let mut state = vec![None; self.names.len()];
        for &(id, q) in self.checkpoints.get(c).map_or(&[][..], Vec::as_slice) { state[id as usize] = Some(q); }
        state
    }

    fn apply(&self, k: usize, state: &mut [Option<Q>]) {
        Tape::decode(&self.words[self.starts[k]..self.starts[k + 1]], |id, mask, values| {
            let i = id as usize;
            if mask == 0 { state[i] = None; return; }
            let mut q = state[i].unwrap_or([0; FIELDS]);
            for f in 0..FIELDS { if mask & (1 << f) != 0 { q[f] = values[f]; } }
            state[i] = Some(q);
        });
    }

    /// Every element's state after frame `k`.
    pub(crate) fn state(&self, k: usize) -> Vec<Option<Q>> {
        let k = k.min(self.len().saturating_sub(1));
        let c = k / STRIDE;
        let mut state = self.checkpoint_state(c);
        if self.is_empty() { return state; }
        for f in c * STRIDE..=k { self.apply(f, &mut state); }
        state
    }

    /// Seconds between frames, the median.
    pub(crate) fn frame_interval(&self) -> f64 {
        let mut dts: Vec<f64> = self.times.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0).collect();
        if dts.is_empty() { return 1.0 / 60.0; }
        dts.sort_by(f64::total_cmp);
        dts[dts.len() / 2]
    }
}
