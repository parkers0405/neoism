//! macOS font smoothing as recovered from CoreGraphics (macOS 15.3, 24D60).
//!
//! CoreText's default grayscale antialiasing dilates each glyph outline before coverage. The
//! radius comes from the ink's linear-sRGB luminance bucket and the ppem ([`radius_for_bucket`]).
//! Every line and every x/y-monotonic cubic piece is translated by the signs of its direction:
//! sides move ±d, tops +2d, bottoms stay, with straight bevel connectors between pieces
//! ([`dilate`]). Integer sizes 7–18 then go through CoreGraphics' metric y autohinter
//! ([`hint`]), and curves are flattened with its tolerances ([`flatten`]) before zeno computes
//! coverage. Masks land within 3/255 of CoreGraphics; the rest is its fixed-point scan
//! conversion. Coordinates are device pixels, y up. The mixed f32/f64 arithmetic is
//! CoreGraphics' own and changes branch decisions, so keep it as written.

use std::collections::HashMap;

use swash::FontRef;
use swash::scale::ScaleContext;
use swash::zeno::{Command, Format, Mask, Origin, PathData, Placement};

use crate::text::{Atlas, Rasterized, Span};
use crate::color::Color;

#[path = "flatten.rs"]
mod flatten;
#[path = "hint.rs"]
mod hint;

pub(crate) use flatten::flatten;
pub(crate) use hint::{HintMetrics, autohint, truetype_metrics};

/// Which stages of CoreGraphics' glyph pipeline run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ApplePipeline {
    /// The colour-keyed outline dilation: CoreText's font smoothing.
    pub dilate: bool,
    /// CoreGraphics' curve flattening; off, zeno flattens.
    pub flatten: bool,
    /// The y autohinter at integer ppem 7–18 with zero vertical phase.
    pub hint: bool,
}

impl ApplePipeline {
    /// CoreText with font smoothing on, the macOS default.
    pub const SMOOTHED: ApplePipeline = ApplePipeline { dilate: true, flatten: true, hint: true };
    /// CoreText with smoothing off (`CGContextSetShouldSmoothFonts(false)`, a browser's
    /// `-webkit-font-smoothing: antialiased`): it still flattens and autohints.
    pub const UNSMOOTHED: ApplePipeline = ApplePipeline { dilate: false, flatten: true, hint: true };

    /// A glyph key's tag for this pipeline.
    fn bits(self) -> u8 {
        (self.dilate as u8) | (self.flatten as u8) << 1 | (self.hint as u8) << 2
    }
}

/// Hint metrics kept per face (font, index, variation coordinates) before the cache restarts.
const MAX_FACES: usize = 256;

pub(crate) type Point = [f64; 2];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Cmd {
    Move(Point),
    Line(Point),
    Quad(Point, Point),
    Cubic(Point, Point, Point),
    Close,
}

pub(crate) type Path = Vec<Cmd>;

impl Cmd {
    fn map(self, f: impl Fn(Point) -> Point) -> Cmd {
        match self {
            Cmd::Move(a) => Cmd::Move(f(a)),
            Cmd::Line(a) => Cmd::Line(f(a)),
            Cmd::Quad(a, b) => Cmd::Quad(f(a), f(b)),
            Cmd::Cubic(a, b, c) => Cmd::Cubic(f(a), f(b), f(c)),
            Cmd::Close => Cmd::Close,
        }
    }

    fn end(self) -> Option<Point> {
        match self {
            Cmd::Move(p) | Cmd::Line(p) | Cmd::Quad(_, p) | Cmd::Cubic(_, _, p) => Some(p),
            Cmd::Close => None,
        }
    }
}

/// The selected pipeline, per-ink dilation state and the autohinter's per-face metrics.
pub(crate) struct Smoothing {
    pipeline: ApplePipeline,
    /// Luminance bucket per `Ink`: 0 is the element's colour, n is `spans[n - 1]`'s.
    inks: Vec<u8>,
    metrics: HashMap<(u64, u32, u64), Option<HintMetrics>>,
}

impl Default for Smoothing {
    fn default() -> Self {
        Smoothing { pipeline: crate::text::FontSmoothing::default().pipeline(), inks: Vec::new(), metrics: HashMap::new() }
    }
}

/// One glyph to rasterize: `key` names the face for the metrics cache.
pub(crate) struct Job<'a> {
    pub font: FontRef<'a>,
    pub key: (u64, u32, u64),
    pub coords: &'a [i16],
    pub glyph: u16,
    pub ppem: f32,
    pub phase_x: f32,
    /// Device px the mask is moved down after hinting, so a gliding glyph keeps its shape.
    pub shift_y: f32,
    pub radius: f64,
    pub skew: u8,
}

impl Smoothing {
    pub(crate) fn pipeline(&self) -> ApplePipeline {
        self.pipeline
    }

    pub(crate) fn set_pipeline(&mut self, pipeline: ApplePipeline) {
        self.pipeline = pipeline;
    }

    /// The glyph key's pipeline tag: glyphs from different pipelines never share masks.
    pub(crate) fn key(&self) -> u8 {
        self.pipeline.bits()
    }

    /// The colours the next glyphs draw in, which choose their dilation.
    pub(crate) fn set_inks(&mut self, element: Color, spans: &[Span]) {
        let base = color_bucket(element);
        self.inks.clear();
        self.inks.push(base);
        self.inks.extend(spans.iter().map(|s| s.color.map_or(base, color_bucket)));
    }

    /// Each `Ink`'s luminance bucket, as [`Self::set_inks`] last chose them.
    pub(crate) fn inks(&self) -> &[u8] { &self.inks }

    /// The dilation radius of a glyph in `ink` at `ppem` device px (0 without the dilation
    /// stage). The radius, not the bucket, keys the atlas: buckets that reach the cap share
    /// masks.
    pub(crate) fn radius(&self, ink: u16, ppem: f32) -> f64 {
        if !self.pipeline.dilate { return 0.0; }
        let bucket = self.inks.get(ink as usize).or(self.inks.first()).copied().unwrap_or(0);
        radius_for_bucket(ppem as f64, bucket)
    }

    /// The y autohint `mask` applies to an upright glyph of face `key` at `ppem` dilated by
    /// `radius`, for a glyph drawn from its outline instead.
    pub(crate) fn hint(&mut self, cx: &mut ScaleContext, font: FontRef<'_>, key: (u64, u32, u64), coords: &[i16], ppem: f64, radius: f64) -> Option<hint::HintTransform> {
        if !self.pipeline.hint || !hint::eligible(ppem) {
            return None;
        }
        if self.metrics.len() >= MAX_FACES && !self.metrics.contains_key(&key) {
            self.metrics.clear();
        }
        let metrics = (*self.metrics.entry(key).or_insert_with(|| truetype_metrics(font, coords, cx)))?;
        autohint(metrics, ppem, if self.pipeline.dilate { radius } else { 0.0 })
    }

    pub(crate) fn rasterize(&mut self, cx: &mut ScaleContext, atlas: &mut Atlas, job: Job<'_>) -> Rasterized {
        let Some((data, at)) = self.mask(cx, &job) else { return Rasterized::Empty };
        if at.width == 0 || at.height == 0 || !data.iter().any(|&c| c != 0) {
            return Rasterized::Empty;
        }
        match atlas.put(at.width, at.height, &data, at.left, at.top) {
            Some(g) => Rasterized::Placed(g),
            None => Rasterized::Unplaced,
        }
    }

    /// The glyph's coverage and placement (swash's convention: `top` is rows above the baseline).
    pub(crate) fn mask(&mut self, cx: &mut ScaleContext, job: &Job<'_>) -> Option<(Vec<u8>, Placement)> {
        let pipeline = self.pipeline;
        let path = design_outline(cx, job.font, job.coords, job.glyph, job.ppem as f64)?;
        let path = if job.skew == 0 { path } else {
            let t = (job.skew as f64).to_radians().tan();
            path.into_iter().map(|c| c.map(|p| [p[1].mul_add(t, p[0]), p[1]])).collect()
        };
        // The autohinter needs an axis-aligned em matrix, which an oblique isn't.
        let metrics = if pipeline.hint && job.skew == 0 && hint::eligible(job.ppem as f64) {
            if self.metrics.len() >= MAX_FACES && !self.metrics.contains_key(&job.key) {
                self.metrics.clear();
            }
            *self.metrics.entry(job.key).or_insert_with(|| truetype_metrics(job.font, job.coords, cx))
        } else {
            None
        };
        let mut path = prepare_outline(&path, pipeline, job.ppem as f64, job.radius, [job.phase_x as f64, 0.0], metrics);
        if job.shift_y != 0.0 {
            let dy = -job.shift_y as f64;
            path = path.into_iter().map(|c| c.map(|v| [v[0], v[1] + dy])).collect();
        }
        (!path.is_empty()).then(|| mask(&path))
    }
}

/// `glyph`'s unhinted outline at `ppem`, extracted at units-per-em and scaled in f64: swash's
/// final-size extraction rounds early enough to move CoreGraphics' branch decisions.
pub(crate) fn design_outline(cx: &mut ScaleContext, font: FontRef<'_>, coords: &[i16], glyph: u16, ppem: f64) -> Option<Path> {
    let upem = font.metrics(coords).units_per_em;
    let mut scaler = cx.builder(font).size(upem as f32).hint(false).normalized_coords(coords.iter().copied()).build();
    let outline = scaler.scale_outline(glyph)?;
    let scale = ppem / upem as f64;
    let p = |v: swash::zeno::Point| [v.x as f64 * scale, v.y as f64 * scale];
    Some(outline.path().commands().map(|c| match c {
        Command::MoveTo(a) => Cmd::Move(p(a)),
        Command::LineTo(a) => Cmd::Line(p(a)),
        Command::QuadTo(a, b) => Cmd::Quad(p(a), p(b)),
        Command::CurveTo(a, b, c) => Cmd::Cubic(p(a), p(b), p(c)),
        Command::Close => Cmd::Close,
    }).collect())
}

/// Steps 2–5 of the recipe, each stage as `pipeline` selects: fractional origin first, then
/// dilation by `radius`, the autohinter (with the same radius, 0 undilated) when `metrics` is
/// given and the vertical phase is zero, and CoreGraphics' flattening into lines.
pub(crate) fn prepare_outline(path: &[Cmd], pipeline: ApplePipeline, ppem: f64, radius: f64, phase: Point, metrics: Option<HintMetrics>) -> Path {
    let moved: Path = path.iter().map(|c| c.map(|v| [v[0] + phase[0], v[1] + phase[1]])).collect();
    let (mut out, radius) = if pipeline.dilate { (dilate(&moved, radius, radius), radius) } else { (moved, 0.0) };
    if pipeline.hint && phase[1] == 0.0 && let Some(t) = metrics.and_then(|m| autohint(m, ppem, radius)) {
        out = out.into_iter().map(|c| c.map(|v| [v[0], t.apply(v[1])])).collect();
    }
    if pipeline.flatten { flatten(&out) } else { out }
}

/// Zeno coverage of a prepared (y-up) outline, flattened by zeno if it still has curves.
pub(crate) fn mask(path: &[Cmd]) -> (Vec<u8>, Placement) {
    let p = |v: Point| swash::zeno::Point::new(v[0] as f32, v[1] as f32);
    let cmds: Vec<Command> = path.iter().map(|c| match *c {
        Cmd::Move(a) => Command::MoveTo(p(a)),
        Cmd::Line(a) => Command::LineTo(p(a)),
        Cmd::Quad(a, b) => Command::QuadTo(p(a), p(b)),
        Cmd::Cubic(a, b, c) => Command::CurveTo(p(a), p(b), p(c)),
        Cmd::Close => Command::Close,
    }).collect();
    // `inspect` sizes the mask first: zeno's `render` alone places a y-up mask's top at its
    // lowest point.
    Mask::new(&cmds[..]).format(Format::Alpha).origin(Origin::BottomLeft).inspect(|_, _, _| {}).render()
}

/// Encoded sRGB to linear; Rune's colours are already linear, so only the captures need it.
#[cfg(test)]
pub(crate) fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// The bucket (q × 4) of a linear-sRGB colour: Rec. 709 luminance rounded to quarters, half
/// away from zero. Alpha and the background play no part.
pub(crate) fn luminance_bucket([r, g, b]: [f64; 3]) -> u8 {
    let y = b.mul_add(0.0722, r.mul_add(0.2126, g * 0.7152));
    (y * 4.0).round().clamp(0.0, 4.0) as u8
}

pub(crate) fn color_bucket(c: Color) -> u8 {
    luminance_bucket([c.r, c.g, c.b].map(f64::from))
}

/// `min(ppem · (0.0058 + 0.01 q), 0.3f)`: a radius, not total growth. The cap is the f32
/// literal 0.3 widened to f64, and there is no ppem cutoff.
pub(crate) fn radius_for_bucket(ppem: f64, bucket: u8) -> f64 {
    ((bucket.min(4) as f64 * 0.25).mul_add(0.010000000000000002, 0.0058) * ppem).min(0.3f32 as f64)
}

fn f32p(p: Point) -> Point {
    p.map(|v| v as f32 as f64)
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    [(b[0] - a[0]).mul_add(t, a[0]), (b[1] - a[1]).mul_add(t, a[1])]
}

fn split(p: [Point; 4], t: f64) -> ([Point; 4], [Point; 4]) {
    let a = lerp(p[0], p[1], t);
    let b = lerp(p[1], p[2], t);
    let c = lerp(p[2], p[3], t);
    let d = lerp(a, b, t);
    let e = lerp(b, c, t);
    let f = lerp(d, e, t);
    ([p[0], a, d, f], [f, e, c, p[3]])
}

/// Derivative roots of a cubic along `axis` inside 1e-10..=0.9999999999.
fn roots(p: [Point; 4], axis: usize) -> Vec<f64> {
    let [p0, p1, p2, p3] = p.map(|v| v[axis]);
    let a = (p1 - p2).mul_add(3.0, p3 - p0);
    let b = p1.mul_add(-2.0, p0 + p2);
    let disc = (p0 - p1).mul_add(p3, p2.mul_add(p2 - p0, (p1 - p2) * p1));
    if disc < 0.0 {
        return vec![];
    }
    let mut ts = if a == 0.0 {
        if b == 0.0 { vec![] } else { vec![(p0 - p1) * 0.5 / b] }
    } else if b == 0.0 && disc == 0.0 {
        vec![]
    } else {
        let q = -(if b < 0.0 { -1.0f64 } else { 1.0f64 }).mul_add(disc.sqrt(), b);
        // CoreGraphics' second root divides the y delta p1.y - p0.y for both axes; the
        // x-split stage inherits that quirk, which the reference reproduces.
        vec![q / a, (p[1][1] - p[0][1]) / q]
    };
    ts.retain(|t| t.is_finite() && *t >= 1e-10 && *t <= 0.9999999999);
    ts.sort_by(|a, b| a.total_cmp(b));
    ts.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    ts
}

fn monotonic_parts(p: [Point; 4], axis: usize) -> Vec<[Point; 4]> {
    let mut prev = 0.0;
    let mut rem = p;
    let mut out = vec![];
    for t in roots(p, axis) {
        let (a, b) = split(rem, (t - prev) / (1.0 - prev));
        out.push(a);
        rem = b;
        prev = t;
    }
    out.push(rem);
    out
}

/// The whole glyph turned to positive signed area: all contours reverse together, so holes
/// keep winding against the outer contours.
pub(crate) fn normalize(path: &[Cmd]) -> Path {
    let mut area = 0.0;
    let mut prev = [0.0; 2];
    let mut first = prev;
    for c in path {
        match *c {
            Cmd::Move(p) => {
                prev = p;
                first = p;
            }
            Cmd::Line(p) => {
                area += prev[0] * p[1] - p[0] * prev[1];
                prev = p;
            }
            Cmd::Quad(q, p) => {
                area += curve_area([prev, lerp(prev, q, 2.0 / 3.0), lerp(p, q, 2.0 / 3.0), p]);
                prev = p;
            }
            Cmd::Cubic(a, b, p) => {
                area += curve_area([prev, a, b, p]);
                prev = p;
            }
            Cmd::Close => {
                area += prev[0] * first[1] - first[0] * prev[1];
                prev = first;
            }
        }
    }
    if area >= 0.0 {
        return path.to_vec();
    }
    fn flush(segs: &mut Vec<(Point, Cmd)>, out: &mut Path, cur: Point) {
        if segs.is_empty() {
            return;
        }
        out.push(Cmd::Move(cur));
        for (p, c) in segs.drain(..).rev() {
            out.push(match c {
                Cmd::Quad(a, _) => Cmd::Quad(a, p),
                Cmd::Cubic(a, b, _) => Cmd::Cubic(b, a, p),
                _ => Cmd::Line(p),
            });
        }
        out.push(Cmd::Close);
    }
    let mut out = vec![];
    let mut segs = vec![];
    let mut start = [0.0; 2];
    let mut cur = start;
    for c in path {
        match *c {
            Cmd::Move(p) => {
                flush(&mut segs, &mut out, cur);
                start = p;
                cur = p;
            }
            Cmd::Close => {
                flush(&mut segs, &mut out, cur);
                cur = start;
            }
            _ => {
                segs.push((cur, *c));
                cur = c.end().unwrap_or(cur);
            }
        }
    }
    flush(&mut segs, &mut out, cur);
    out
}

/// Signed area integral of a cubic Bezier, exact in its polynomial form.
fn curve_area(p: [Point; 4]) -> f64 {
    let mut a = [[0.0; 2]; 4];
    for k in 0..2 {
        a[0][k] = p[0][k];
        a[1][k] = 3.0 * (p[1][k] - p[0][k]);
        a[2][k] = 3.0 * (p[0][k] - 2.0 * p[1][k] + p[2][k]);
        a[3][k] = -p[0][k] + 3.0 * p[1][k] - 3.0 * p[2][k] + p[3][k];
    }
    let mut area = 0.0;
    for (i, ai) in a.iter().enumerate() {
        for (j, aj) in a.iter().enumerate().skip(1) {
            area += (ai[0] * aj[1] - ai[1] * aj[0]) * j as f64 / (i + j) as f64;
        }
    }
    area
}

struct Dilator {
    out: Path,
    cur: Point,
    first: Point,
    started: bool,
    dx: f32,
    dy: f32,
}

impl Dilator {
    /// One translated piece from `cur` to `to`, joined to the previous one by a line.
    fn emit(&mut self, to: Point, controls: Option<(Point, Point)>) {
        let sign = |v: f32| if v > 0.0 { 1.0 } else if v < 0.0 { -1.0 } else { 0.0 };
        let delta = [(to[0] - self.cur[0]) as f32, (to[1] - self.cur[1]) as f32];
        let (sx, sy, dy) = (sign(delta[1]) * self.dx, -sign(delta[0]) * self.dy, self.dy);
        let start = [((self.cur[0] as f32 + sx) + 0.0) as f64, ((self.cur[1] as f32 + sy) + dy) as f64];
        self.out.push(if self.started { Cmd::Line(start) } else { Cmd::Move(start) });
        self.started = true;
        let shift = |p: Point| [(sx as f64 + p[0]) + 0.0, (sy as f64 + p[1]) + dy as f64];
        self.out.push(match controls {
            Some((a, b)) => Cmd::Cubic(shift(a), shift(b), shift(to)),
            None => Cmd::Line(shift(to)),
        });
        self.cur = f32p(to);
    }

    fn close(&mut self) {
        if self.started {
            self.emit(self.first, None);
            self.out.push(Cmd::Close);
            self.started = false;
        }
    }

    fn cubic(&mut self, p: [Point; 4]) {
        for xpart in monotonic_parts(p, 0) {
            for part in monotonic_parts(xpart, 1) {
                self.emit(part[3], Some((part[1], part[2])));
            }
        }
    }
}

/// CoreGraphics' direction-sign dilation by radii `dx`, `dy` (y also gains +dy). Quadratics
/// become cubics; cubics split at x then y extrema; pieces join with bevel lines.
pub(crate) fn dilate(path: &[Cmd], dx: f64, dy: f64) -> Path {
    let mut d = Dilator { out: vec![], cur: [0.0; 2], first: [0.0; 2], started: false, dx: dx as f32, dy: dy as f32 };
    for c in normalize(path) {
        match c {
            Cmd::Move(p) => {
                d.close();
                d.cur = f32p(p);
                d.first = d.cur;
            }
            Cmd::Line(p) => d.emit(p, None),
            Cmd::Quad(q, p) => {
                let qc = q.map(|x| (2.0 * x / 3.0) as f32);
                let a = [(d.cur[0] as f32 / 3.0 + qc[0]) as f64, (d.cur[1] as f32 / 3.0 + qc[1]) as f64];
                let b = [p[0] / 3.0 + qc[0] as f64, p[1] / 3.0 + qc[1] as f64];
                d.cubic([d.cur, a, b, p]);
            }
            Cmd::Cubic(a, b, p) => d.cubic([d.cur, a, b, p]),
            Cmd::Close => d.close(),
        }
    }
    d.close();
    d.out
}

#[cfg(test)]
#[path = "smoothing_tests.rs"]
mod tests;
