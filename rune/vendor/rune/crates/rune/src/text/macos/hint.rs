//! CoreGraphics' small-size y autohinter. It isn't TrueType bytecode: at integer ppem 7–18,
//! with an axis-aligned isotropic em matrix and zero vertical phase, it maps every control
//! point's y through a piecewise-linear transform that snaps the x-height ('o' top) and cap
//! height ('X' top) to whole pixels. It runs after dilation and before flattening.

use swash::FontRef;
use swash::scale::ScaleContext;

/// Unscaled font-unit metrics. `x_height` and `cap_height` are the bounding-box tops of 'o'
/// and 'X', not the OS/2 fields; `max_y` is the font bounding box's top; `stem_height` is the
/// descriptor's stemH (0 for TrueType).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HintMetrics {
    pub upem: f64,
    pub x_height: f64,
    pub cap_height: f64,
    pub max_y: f64,
    pub stem_height: f64,
}

/// `[-d, a, b, B, c, C, e, E, l, m, u]` in the spec's names.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HintTransform {
    pub values: [f64; 11],
}

impl HintTransform {
    pub(crate) fn apply(&self, y: f64) -> f64 {
        let v = self.values;
        if y < v[1] {
            y
        } else if y <= v[2] {
            (y - v[1]).mul_add(v[8], v[1])
        } else if y <= v[4] {
            (y - v[2]).mul_add(v[9], v[3])
        } else if y <= v[6] {
            y + (v[5] - v[4])
        } else {
            (y - v[6]).mul_add(v[10], v[7])
        }
    }
}

/// Integer ppem 7 through 18.
pub(crate) fn eligible(ppem: f64) -> bool {
    (7.0..=18.0).contains(&ppem) && ppem.fract() == 0.0
}

fn eval(m: HintMetrics, scale: f64, d: f64, x_target: f64, cap_target: f64) -> HintTransform {
    let stem = (if m.stem_height == 0.0 { m.cap_height } else { m.stem_height })
        .min(m.x_height * 0.333)
        .min((m.cap_height - m.x_height) * 0.5)
        * scale;
    let lo = stem + d;
    let x = m.x_height.mul_add(scale, d);
    let xt = x_target - d;
    let cap = m.cap_height.mul_add(scale, d);
    let mut ct = cap_target - d;
    let middle = m.cap_height * scale - stem - d;
    let mut mt = (-2.0f64).mul_add(d, ct - stem);
    let top = m.max_y.mul_add(scale, d);
    let slope = |a: f64, b: f64| if b == 0.0 { 0.0 } else { a / b };
    let s1 = slope(xt - lo, x - lo);
    let mut s2 = slope(mt - xt, middle - x);
    let mut s3 = slope(top - ct, top - cap);
    // A cap height that would squash the middle band isn't snapped.
    if s2 < 0.75 || s2 - s1 > 0.3 {
        ct = cap;
        mt = (-2.0f64).mul_add(d, cap - stem);
        s2 = slope(mt - xt, middle - x);
        s3 = 1.0;
    }
    HintTransform { values: [-d, lo, x, xt, middle, mt, cap, ct, s1, s2, s3] }
}

/// The transform for radius `d` at `ppem`, or None when the size or the metrics don't take it.
pub(crate) fn autohint(m: HintMetrics, ppem: f64, d: f64) -> Option<HintTransform> {
    if !eligible(ppem) || m.x_height == m.cap_height {
        return None;
    }
    let scale = ppem / m.upem;
    // Targets are chosen at a reference radius that doesn't depend on the ink.
    let reference = (ppem * 0.015125).min(0.3);
    let r = |v: f64| (v + 0.5).trunc();
    let mut xt = r(m.x_height.mul_add(scale, 2.0 * reference));
    let mut ct = r(m.cap_height.mul_add(scale, 2.0 * reference));
    let plain_x = r(m.x_height * scale);
    let plain_cap = r(m.cap_height * scale);
    if plain_x != xt {
        let cost = |x, c| {
            [reference, 0.0].iter().map(|&d| {
                let t = eval(m, scale, d, x, c);
                (t.values[8] - 1.0).abs() + (t.values[9] - 1.0).abs()
            }).sum::<f64>()
        };
        if cost(xt, ct) > cost(plain_x, plain_cap) {
            xt = plain_x;
            ct = plain_cap;
        }
    }
    let t = eval(m, scale, d, xt, ct);
    if m.x_height / m.cap_height > 0.85 && t.values[9] < 0.33 { None } else { Some(t) }
}

/// Metrics for a TrueType (`glyf`) face from its 'o' and 'X' outlines (at `coords`), `head`
/// and the known descriptor stemH. Other formats return None, so they aren't hinted.
pub(crate) fn truetype_metrics(font: FontRef<'_>, coords: &[i16], cx: &mut ScaleContext) -> Option<HintMetrics> {
    font.table(u32::from_be_bytes(*b"glyf"))?;
    let head = font.table(u32::from_be_bytes(*b"head"))?;
    let upem = font.metrics(coords).units_per_em as f64;
    let max_y = i16::from_be_bytes(head.get(42..44)?.try_into().ok()?) as f64;
    let mut top = |ch: char| {
        let id = font.charmap().map(ch);
        if id == 0 {
            return None;
        }
        let mut scaler = cx.builder(font).size(upem as f32).hint(false).normalized_coords(coords.iter().copied()).build();
        let outline = scaler.scale_outline(id)?;
        Some(outline.points().iter().map(|p| p.y as f64).fold(f64::NEG_INFINITY, f64::max))
    };
    let x_height = top('o')?;
    let cap_height = top('X')?;
    Some(HintMetrics { upem, x_height, cap_height, max_y, stem_height: descriptor_stem_height(font) })
}

/// The stemH CoreGraphics' font descriptor reports, which moves the hinter's lowest band. It
/// isn't in any table swash reads (not `post`, `OS/2` or `cvt `), so these are the values
/// macOS 15.3 reports for the faces measured; other TrueType faces, Inter among them, report 0.
fn descriptor_stem_height(font: FontRef<'_>) -> f64 {
    let name = font.localized_strings().find_by_id(swash::StringId::PostScript, None).map(|s| s.chars().collect::<String>()).unwrap_or_default();
    // SF Pro (SFNS.ttf) at every weight and optical size tried, and Helvetica Regular.
    if name.starts_with(".SFNS") { 79.0 } else if name == "Helvetica" { 85.0 } else { 0.0 }
}
