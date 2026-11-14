//! The pixel backstop: `snap`, `flash` and `changes-twice` in the composited window, cell by
//! cell (see `raster`). It sees what no one element's own values show: a surface that drops out
//! one frame before what replaces it fades in, a layer's opacity, a clip, two elements trading
//! places. A region is reported at the top element painting it the frame before.
//!
//! Cells under what travels (an element moving on a frame and the next or last, the step in line
//! with its motion) at any frame of the event, a loop, a live binding or words that change (a
//! field's too, set without a key as dictation does) are skipped, as are Rune's own tooltip
//! (`El::help`) and cells an element marked with the matching `El::motion_intent` covers. A
//! region `snap` or `changes-twice` needs no element there to have changed on its frame (each
//! element's own rules report those) and a `snap` no `Tx::instant` commit; a region `flash` is
//! reported unless an element's `flash` or `flicker` there already is.

use std::collections::{HashMap, HashSet};

use super::painted::marked;
use super::raster::{CELL, Grid};
use super::series::{self, Tune, V};
use super::{Finding, LIVE, Recording, Snap, Track, short};

const BLOCKED: u8 = 1;
const PULSE: u8 = 2;
const CUT: u8 = 4;
const STEP: u8 = 8;
const CHANGED: u8 = 16;

fn moved(a: &Snap, b: &Snap) -> f32 {
    let d = (0..4).map(|i| (a.rect[i] - b.rect[i]).abs().max((a.vis[i] - b.vis[i]).abs())).fold(0.0, f32::max);
    d.max((a.scale - b.scale).abs() * 100.0)
}

fn painted_differs(t: &Track, k: usize) -> bool {
    match (t.paint_at(k - 1), t.paint_at(k)) {
        (Some(a), Some(b)) => a.colors != b.colors || a.sizes != b.sizes || a.gradient_end != b.gradient_end || a.effect != b.effect || a.image != b.image,
        _ => false,
    }
}

/// Per cell, on frame `k`: why a change there is no region's to report.
fn guard(rec: &Recording, loops: &HashSet<usize>, k: usize, (w, h): (usize, usize)) -> Vec<u8> {
    let mut m = vec![0u8; w * h];
    let mut mark = |r: [f32; 4], pad: f32, bit: u8| {
        if r[2] <= 0.0 && r[3] <= 0.0 { return; }
        let (x0, y0) = (((r[0] - pad) / CELL).floor().max(0.0) as usize, ((r[1] - pad) / CELL).floor().max(0.0) as usize);
        let (x1, y1) = ((((r[0] + r[2] + pad) / CELL).ceil().max(0.0) as usize).min(w), (((r[1] + r[3] + pad) / CELL).ceil().max(0.0) as usize).min(h));
        for y in y0..y1 { for x in x0..x1 { m[y * w + x] |= bit; } }
    };
    for (i, t) in rec.tracks.iter().enumerate() {
        let s = |j: usize| t.at(j);
        let (pp, p, c, nx) = (k.checked_sub(2).and_then(s), s(k - 1), s(k), s(k + 1));
        let pad = [k - 1, k].iter().filter_map(|&j| t.paint_at(j)).map(|p| p.sizes[super::paint::S_SHADOW] + 3.0 * p.sizes[super::paint::S_BLUR]).fold(0.0, f32::max);
        let span = |bit: u8, m: &mut dyn FnMut([f32; 4], f32, u8)| {
            for s in [p, c, nx].into_iter().flatten() { m(s.vis, pad + CELL, bit); }
        };
        let step = |a: Option<&Snap>, b: Option<&Snap>| a.zip(b).map_or(0.0, |(a, b)| moved(a, b));
        let (d0, d1, d2) = (step(pp, p), step(p, c), step(c, nx));
        let born_or_gone = p.is_some() != c.is_some();
        let faded = p.zip(c).is_some_and(|(a, b)| (a.op - b.op).abs() > 0.001);
        // Any motion counts: pixel snapping draws a glide's last fractions of a point as an
        // occasional whole-pixel step, which is the glide, not a region changing on its own.
        if born_or_gone || d1 > 0.01 || faded || painted_differs(t, k) {
            for s in [p, c].into_iter().flatten() { mark(s.vis, pad, CHANGED); }
        }
        let shown: Vec<&Snap> = [p, c].into_iter().flatten().filter(|s| s.op > 0.02).collect();
        if shown.is_empty() { continue; }
        let around = d0.max(d2);
        let travelling = d1 > 0.25 && around > 0.25 && d1 <= 3.0 * around + 2.0;
        let words = t.words.iter().chain(&t.edits).any(|w| w.0 + 1 >= k && w.0 <= k + 1);
        let live = shown.iter().any(|s| s.flags & LIVE != 0);
        if travelling || words || live || loops.contains(&i) { span(BLOCKED, &mut mark); }
        if marked(t, k, "flash") { span(PULSE, &mut mark); }
        if marked(t, k, "snap") { span(CUT, &mut mark); }
        if marked(t, k, "changes-twice") { span(STEP, &mut mark); }
    }
    for tip in [k - 1, k].iter().filter_map(|&j| rec.tips.get(j).copied().flatten()) { mark(tip, CELL, BLOCKED); }
    m
}

/// The region tune: cells' colours in OKLab, ΔE 0.08 a step of 1, as the element checks.
fn tune(hz: f64) -> Tune {
    let t = super::painted::tune(hz);
    Tune { still: 0.06, ..t }
}

pub fn regions(rec: &Recording, loops: &HashSet<usize>, found: &[Finding]) -> Vec<Finding> {
    let n = rec.frames;
    let Some(g0) = rec.grids.iter().flatten().next() else { return Vec::new() };
    if rec.grids.len() < n || rec.grids.iter().any(Option::is_none) { return Vec::new(); }
    let (w, h) = (g0.w, g0.h);
    let grid = |k: usize| -> &Grid { rec.grids[k].as_deref().unwrap() };
    let tune = tune(rec.hz);
    let v = |k: usize, i: usize| -> V {
        let [l, a, b] = super::paint::toe(grid(k).lab(i));
        [l / 0.08, a / 0.08, b / 0.08, 0.0, 0.0, 0.0]
    };
    // (kind, frame) → the cells that showed it, with each one's event.
    let mut hits: HashMap<(&'static str, usize), Vec<(usize, series::Event)>> = HashMap::new();
    let floor = tune.flash.min(tune.snap);
    for i in 0..w * h {
        let changes = (1..n).any(|k| !std::ptr::eq(grid(k), grid(k - 1)) && series::dist(&v(k, i), &v(k - 1, i)) >= floor);
        if !changes { continue; }
        let vals: Vec<Option<V>> = (0..n).map(|k| (k >= rec.start).then(|| v(k, i))).collect();
        for e in series::events(&vals, &tune, &|_| 1.0) {
            hits.entry((e.kind, e.k)).or_default().push((i, e));
        }
    }
    let input_in = |a: usize, b: usize| rec.inputs.iter().any(|x| x.0 >= a && x.0 <= b);
    let mut guards: HashMap<usize, Vec<u8>> = HashMap::new();
    let mut keys: Vec<(&'static str, usize)> = hits.keys().copied().collect();
    keys.sort_by_key(|k| (k.1, k.0));
    let mut out = Vec::new();
    for key in keys {
        let (kind, k) = key;
        if k <= rec.start || input_in(k.saturating_sub(1), k) || rec.loads.iter().any(|&l| k >= l && k <= l + 2) { continue; }
        if kind == "snap" && rec.instant.get(k) == Some(&true) { continue; }
        // A cell is vetoed by what holds anywhere in its event's frames (something travelling
        // across it shows out and back), and by an element changing there on the frame itself.
        let frames = |e: &series::Event| e.span.0.max(1).min(k)..=e.span.1.max(k);
        for (_, e) in &hits[&key] {
            for j in frames(e) { guards.entry(j).or_insert_with(|| guard(rec, loops, j, (w, h))); }
        }
        let veto = BLOCKED | match kind { "flash" => PULSE, "snap" => CUT, _ => STEP };
        let vetoed = |i: usize, e: &series::Event| {
            kind != "flash" && guards[&k][i] & CHANGED != 0 || frames(e).any(|j| guards[&j][i] & veto != 0)
        };
        let cells: HashMap<usize, &series::Event> = hits[&key].iter().filter(|(i, e)| !vetoed(*i, e) && !(kind == "changes-twice" && input_in(e.span.0, e.span.1))).map(|(i, e)| (*i, e)).collect();
        // Connected regions of two cells or more.
        let mut seen: HashSet<usize> = HashSet::new();
        let mut starts: Vec<usize> = cells.keys().copied().collect();
        starts.sort();
        for s in starts {
            if !seen.insert(s) { continue; }
            let mut region = vec![s];
            let mut q = vec![s];
            while let Some(c) = q.pop() {
                let (x, y) = (c % w, c / w);
                let near = [(x > 0).then(|| c - 1), (x + 1 < w).then(|| c + 1), (y > 0).then(|| c - w), (y + 1 < h).then(|| c + w)];
                for nb in near.into_iter().flatten() {
                    if cells.contains_key(&nb) && seen.insert(nb) { region.push(nb); q.push(nb); }
                }
            }
            if region.len() < 2 { continue; }
            let (x0, y0) = (region.iter().map(|c| c % w).min().unwrap(), region.iter().map(|c| c / w).min().unwrap());
            let (x1, y1) = (region.iter().map(|c| c % w).max().unwrap() + 1, region.iter().map(|c| c / w).max().unwrap() + 1);
            let rect = [x0 as f32 * CELL, y0 as f32 * CELL, (x1 - x0) as f32 * CELL, (y1 - y0) as f32 * CELL];
            if explained(rec, found, kind, k, &rect) { continue; }
            let worst = *region.iter().max_by(|a, b| cells[a].size.total_cmp(&cells[b].size)).unwrap();
            let e = cells[&worst];
            let hex = |j: usize| grid(j).color(worst).hex();
            let colors = match kind {
                "snap" => format!("{} → {}", hex(k - 1), hex(k)),
                _ => format!("{} → {} → {}", hex(e.span.0), hex(k), hex(e.span.1)),
            };
            // The top surface there before or after, whichever changed on the frame.
            let (was, now) = (top(rec, k - 1, &rect), top(rec, k, &rect));
            let changed = |t: &&Track| t.at(k - 1).map(|s| (s.rect, s.op)) != t.at(k).map(|s| (s.rect, s.op)) || painted_differs(t, k);
            let who = was.filter(changed).or(now.filter(changed)).or(was).or(now).map_or_else(|| "window".into(), |t| short(&t.path));
            out.push(Finding { kind, element: format!("{who} (pixels)"), frame: k,
                detail: format!("{} cells ({:.0}×{:.0} pt at {:.0},{:.0}) {colors} over frames {}–{}", region.len(), rect[2], rect[3], rect[0], rect[1], e.span.0, e.span.1) });
        }
    }
    out
}

fn overlaps(a: &[f32; 4], b: &[f32; 4]) -> bool {
    super::area(&super::painted::meet(a, b)) > 0.0
}

/// An element's own finding of the same nature covers this region around frame `k`.
fn explained(rec: &Recording, found: &[Finding], kind: &str, k: usize, rect: &[f32; 4]) -> bool {
    let kinds: &[&str] = match kind {
        "flash" => &["flash", "flicker"],
        "snap" => &["snap", "fade-snap", "pop-in", "pop-out", "cut-exit", "jump", "offset-jump", "appear-then-jump", "text-swap"],
        _ => &["changes-twice", "stop-start"],
    };
    found.iter().filter(|f| kinds.contains(&f.kind) && f.frame + 1 >= k && f.frame <= k + 1).any(|f| {
        rec.tracks.iter().filter(|t| short(&t.path) == f.element).any(|t| (k.saturating_sub(1)..=k).filter_map(|j| t.at(j)).any(|s| overlaps(&s.vis, rect)))
    })
}

/// The element painting highest over the middle of `rect` on frame `k`.
fn top<'a>(rec: &'a Recording, k: usize, rect: &[f32; 4]) -> Option<&'a Track> {
    let (cx, cy) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    rec.tracks.iter().filter_map(|t| {
        let (s, p) = (t.at(k)?, t.paint_at(k)?);
        let paints = p.colors.iter().any(|c| c.a > 0.05) || p.image.is_some();
        let over = s.vis[0] <= cx && cx <= s.vis[0] + s.vis[2] && s.vis[1] <= cy && cy <= s.vis[1] + s.vis[3];
        (paints && over && s.op > 0.02).then_some((p.rank, t))
    }).max_by_key(|x| x.0).map(|x| x.1)
}
