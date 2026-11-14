//! One glyph's outline as the GPU reads it: quadratic Béziers in font units (y up), split into
//! xy-monotone pieces, CoreText's dilation pieces and bevel connectors, filed into row bands.
//!
//! The monotone split and the bands port windfoil-algorithm's `geometry.js` and `bands.js`
//! (Apache-2.0). Dilation is CoreGraphics' (see `text/macos/smoothing.rs`): every monotone piece
//! moves by the signs of its direction, sides ±r, tops +2r, bottoms not at all, and straight
//! bevels join the moved pieces. The moves are a code per piece, so one record serves every
//! radius and the shader applies `r` per pixel. A connector is a line between two moved copies of
//! one point, owned by a single band.

use swash::zeno::Command;

/// A flat quadratic: x0, y0, cx, cy, x1, y1.
type Quad = [f64; 6];

/// Pieces per band the bands aim for.
const TARGET_PER_BAND: usize = 10;
const MAX_BANDS: usize = 64;
/// Bands with more elements than this sort them by right edge, so the shader stops early.
pub(crate) const SORT_MIN: usize = 4;
/// Marks a connector in an element's code word.
pub(crate) const CONNECTOR: u32 = 1 << 8;

/// A built record, positions relative to its own first texel.
pub(crate) struct Built {
    pub texels: Vec<[u32; 4]>,
    /// Ink box in font units before dilation: x0, y0, x1, y1 (y up).
    pub bbox: [f32; 4],
    /// Font units per em.
    pub upem: f32,
}

/// Builds `commands` (y up, `unit` font units each) leaning `skew` (tan of the oblique angle),
/// or None for an empty outline.
pub(crate) fn build(commands: impl Iterator<Item = Command>, unit: f64, skew: f64) -> Option<Built> {
    let contours = contours(commands, unit, skew);
    if contours.iter().all(|c| c.is_empty()) {
        return None;
    }
    let contours = if signed_area(&contours) < 0.0 { reverse(contours) } else { contours };
    let mut elements = Vec::new();
    for contour in &contours {
        dilation_elements(contour, &mut elements);
    }
    let bbox = contours.iter().flatten().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, q| {
        [b[0].min(q[0]).min(q[2]).min(q[4]), b[1].min(q[1]).min(q[3]).min(q[5]), b[2].max(q[0]).max(q[2]).max(q[4]), b[3].max(q[1]).max(q[3]).max(q[5])]
    });
    (bbox[2] >= bbox[0] && bbox[3] >= bbox[1]).then(|| Built { texels: bands(&elements, bbox[1], bbox[3]), bbox: bbox.map(|v| v as f32), upem: 0.0 })
}

/// One drawn element: a moved monotone piece, or a connector at `q[0..2]`.
struct Element {
    q: Quad,
    code: u32,
}

impl Element {
    fn connector(&self) -> bool { self.code & CONNECTOR != 0 }
    fn y_range(&self) -> (f64, f64) { (self.q[1].min(self.q[5]), self.q[1].max(self.q[5])) }
    fn x_max(&self) -> f64 { self.q[0].max(self.q[2]).max(self.q[4]) }
}

/// The contours of `commands` as quads, cubics split into quads within a tenth of a font unit,
/// each closed by a line back to its start. That line has zero length where the contour already
/// ends on its start, and stays: CoreGraphics' dilator emits it too.
fn contours(commands: impl Iterator<Item = Command>, unit: f64, skew: f64) -> Vec<Vec<Quad>> {
    let p = |v: swash::zeno::Point| [(v.x as f64 + v.y as f64 * skew) * unit, v.y as f64 * unit];
    let mut out: Vec<Vec<Quad>> = Vec::new();
    let (mut cur, mut start, mut open) = ([0.0; 2], [0.0; 2], false);
    let close = |out: &mut Vec<Vec<Quad>>, cur: [f64; 2], start: [f64; 2]| {
        if let Some(c) = out.last_mut() && !c.is_empty() {
            c.push([cur[0], cur[1], (cur[0] + start[0]) / 2.0, (cur[1] + start[1]) / 2.0, start[0], start[1]]);
        }
    };
    for command in commands {
        match command {
            Command::MoveTo(a) => {
                if open { close(&mut out, cur, start); }
                (cur, start, open) = (p(a), p(a), true);
                out.push(Vec::new());
            }
            Command::LineTo(a) => {
                let a = p(a);
                if let Some(c) = out.last_mut() { c.push([cur[0], cur[1], (cur[0] + a[0]) / 2.0, (cur[1] + a[1]) / 2.0, a[0], a[1]]); }
                cur = a;
            }
            Command::QuadTo(c1, a) => {
                let (c1, a) = (p(c1), p(a));
                if let Some(c) = out.last_mut() { c.push([cur[0], cur[1], c1[0], c1[1], a[0], a[1]]); }
                cur = a;
            }
            Command::CurveTo(c1, c2, a) => {
                let (c1, c2, a) = (p(c1), p(c2), p(a));
                if let Some(c) = out.last_mut() { cubic_quads([cur, c1, c2, a], c); }
                cur = a;
            }
            Command::Close => {
                if open { close(&mut out, cur, start); }
                (cur, open) = (start, false);
            }
        }
    }
    if open { close(&mut out, cur, start); }
    out
}

/// A cubic as quadratics, as many as keep the error under `TOLERANCE` font units.
fn cubic_quads(p: [[f64; 2]; 4], out: &mut Vec<Quad>) {
    const TOLERANCE: f64 = 0.1;
    // The error of one quadratic is √3/36 of the third difference.
    let d = [p[3][0] - 3.0 * p[2][0] + 3.0 * p[1][0] - p[0][0], p[3][1] - 3.0 * p[2][1] + 3.0 * p[1][1] - p[0][1]];
    let err = (d[0] * d[0] + d[1] * d[1]).sqrt() * 3f64.sqrt() / 36.0;
    let n = ((err / TOLERANCE).cbrt().ceil() as usize).clamp(1, 16);
    let at = |t: f64| {
        let u = 1.0 - t;
        let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
        [0, 1].map(|k| w[0] * p[0][k] + w[1] * p[1][k] + w[2] * p[2][k] + w[3] * p[3][k])
    };
    let tangent = |t: f64| {
        let u = 1.0 - t;
        [0, 1].map(|k| 3.0 * (u * u * (p[1][k] - p[0][k]) + 2.0 * u * t * (p[2][k] - p[1][k]) + t * t * (p[3][k] - p[2][k])))
    };
    for i in 0..n {
        let (t0, t1) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
        let (a, b) = (at(t0), at(t1));
        // The control point where the end tangents of the segment meet; their midpoint estimate
        // when they're parallel.
        let (ta, tb) = (tangent(t0), tangent(t1));
        let den = ta[0] * tb[1] - ta[1] * tb[0];
        let c = if den.abs() > 1e-12 {
            let s = ((b[0] - a[0]) * tb[1] - (b[1] - a[1]) * tb[0]) / den;
            [a[0] + ta[0] * s, a[1] + ta[1] * s]
        } else {
            let m = at((t0 + t1) / 2.0);
            [2.0 * m[0] - (a[0] + b[0]) / 2.0, 2.0 * m[1] - (a[1] + b[1]) / 2.0]
        };
        // A control point far off the chord means an inflection: the midpoint fit is safer.
        let chord = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let off = ((c[0] - (a[0] + b[0]) / 2.0).powi(2) + (c[1] - (a[1] + b[1]) / 2.0).powi(2)).sqrt();
        let c = if den.abs() > 1e-12 && off <= chord * 2.0 { c } else {
            let m = at((t0 + t1) / 2.0);
            [2.0 * m[0] - (a[0] + b[0]) / 2.0, 2.0 * m[1] - (a[1] + b[1]) / 2.0]
        };
        out.push([a[0], a[1], c[0], c[1], b[0], b[1]]);
    }
}

/// The glyph's signed area: positive when its outer contours run anticlockwise (y up).
fn signed_area(contours: &[Vec<Quad>]) -> f64 {
    // Green's theorem on each quadratic, exactly: ∫ p × p' dt = (2 p0×p1 + p0×p2 + 2 p1×p2) / 3.
    let cross = |ax: f64, ay: f64, bx: f64, by: f64| ax * by - ay * bx;
    contours.iter().flatten().map(|q| {
        2.0 * cross(q[0], q[1], q[2], q[3]) + cross(q[0], q[1], q[4], q[5]) + 2.0 * cross(q[2], q[3], q[4], q[5])
    }).sum::<f64>() / 6.0
}

/// Every contour run backwards, so holes keep winding against the outer contours.
fn reverse(contours: Vec<Vec<Quad>>) -> Vec<Vec<Quad>> {
    contours.into_iter().map(|c| c.into_iter().rev().map(|q| [q[4], q[5], q[2], q[3], q[0], q[1]]).collect()).collect()
}

/// A contour's moved pieces and the bevels between them. The last piece is the closing line, or
/// a point where the contour already ended on its start: CoreGraphics moves that point up by r
/// alone, which shapes the bevel at the start.
fn dilation_elements(contour: &[Quad], out: &mut Vec<Element>) {
    if contour.is_empty() {
        return;
    }
    let mut pieces: Vec<(Quad, u32)> = Vec::new();
    for q in contour {
        let degenerate = q[0] == q[4] && q[1] == q[5];
        if degenerate {
            pieces.push((*q, code(0, 1)));
            continue;
        }
        let mut split = Vec::with_capacity(3);
        monotone(q, &mut split);
        for s in split {
            pieces.push((s, direction_code(&s)));
        }
    }
    let n = pieces.len();
    for i in 0..n {
        let (q, c) = pieces[i];
        if q[1] != q[3] || q[1] != q[5] {
            out.push(Element { q, code: c | c << 4 });
        }
        let next = pieces[(i + 1) % n].1;
        if next != c {
            out.push(Element { q: [q[4], q[5], q[4], q[5], q[4], q[5]], code: c | next << 4 | CONNECTOR });
        }
    }
}

/// The move code of x step `x` (-1..=1) and y step `y` (0..=2), in r: `x + 1` in bits 0-1, `y`
/// in bits 2-3.
fn code(x: i32, y: i32) -> u32 {
    ((x + 1) | y << 2) as u32
}

/// CoreGraphics' move for a piece: its x by the sign of its y travel, its y up by r less r times
/// the sign of its x travel, signs taken in f32 as it takes them.
fn direction_code(q: &Quad) -> u32 {
    let sign = |v: f32| if v > 0.0 { 1 } else if v < 0.0 { -1 } else { 0 };
    let (dx, dy) = ((q[4] as f32 - q[0] as f32), (q[5] as f32 - q[1] as f32));
    code(sign(dy), 1 - sign(dx))
}

fn extremum_t(p0: f64, p1: f64, p2: f64) -> Option<f64> {
    let a = p0 - 2.0 * p1 + p2;
    if a == 0.0 {
        return None;
    }
    let t = (p0 - p1) / a;
    (t > 0.0 && t < 1.0).then_some(t)
}

fn subdivide(q: &Quad, t: f64) -> (Quad, Quad) {
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    let (x01, y01, x12, y12) = (lerp(q[0], q[2]), lerp(q[1], q[3]), lerp(q[2], q[4]), lerp(q[3], q[5]));
    let (xm, ym) = (lerp(x01, x12), lerp(y01, y12));
    ([q[0], q[1], x01, y01, xm, ym], [xm, ym, x12, y12, q[4], q[5]])
}

/// `geometry.js` `pushMonotonePieces`: `q` split at its x and y extrema.
fn monotone(q: &Quad, out: &mut Vec<Quad>) {
    let (tx, ty) = (extremum_t(q[0], q[2], q[4]), extremum_t(q[1], q[3], q[5]));
    let (first, second) = match (tx, ty) {
        (Some(a), Some(b)) => (Some(a.min(b)), Some(a.max(b))),
        (a, b) => (a.or(b), None),
    };
    let mut rest = *q;
    let mut consumed = 0.0;
    for t in [first, second].into_iter().flatten() {
        let local = (t - consumed) / (1.0 - consumed);
        if !(local > 0.0 && local < 1.0) {
            continue;
        }
        let (l, r) = subdivide(&rest, local);
        out.push(l);
        rest = r;
        consumed = t;
    }
    out.push(rest);
}

/// The record's texels: a header `[band base, band count, y0, 1 / band height]`, one texel per
/// band `[first element, count]`, then each band's elements, two texels each:
/// `[p0, p1]`, `[p2, code, right edge]`. Element and band offsets are relative to the header.
fn bands(elements: &[Element], y0: f64, y1: f64) -> Vec<[u32; 4]> {
    let n = elements.len();
    let r = if n <= TARGET_PER_BAND { 1 } else { n.div_ceil(TARGET_PER_BAND).min(MAX_BANDS) };
    let inv_h = if r > 1 && y1 > y0 { r as f64 / (y1 - y0) } else { 0.0 };
    let r = if inv_h == 0.0 { 1 } else { r };
    let band = |y: f64| if inv_h == 0.0 { 0 } else { (((y - y0) * inv_h).floor().max(0.0) as usize).min(r - 1) };
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); r];
    for (k, e) in elements.iter().enumerate() {
        let (lo, hi) = e.y_range();
        if e.connector() {
            buckets[band(lo)].push(k);
            continue;
        }
        for bucket in &mut buckets[band(lo)..=band(hi)] {
            bucket.push(k);
        }
    }
    let f = |v: f64| (v as f32).to_bits();
    let mut texels = vec![[1, r as u32, f(y0), (inv_h as f32).to_bits()]];
    texels.resize(1 + r, [0; 4]);
    for (b, bucket) in buckets.iter_mut().enumerate() {
        if bucket.len() > SORT_MIN {
            bucket.sort_by(|&a, &c| elements[c].x_max().total_cmp(&elements[a].x_max()));
        }
        texels[1 + b] = [texels.len() as u32, bucket.len() as u32, 0, 0];
        for &k in bucket.iter() {
            let e = &elements[k];
            texels.push([f(e.q[0]), f(e.q[1]), f(e.q[2]), f(e.q[3])]);
            texels.push([f(e.q[4]), f(e.q[5]), e.code, f(e.x_max())]);
        }
    }
    texels
}
