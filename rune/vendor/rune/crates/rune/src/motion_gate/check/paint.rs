//! What an element paints on one frame, beside its rect and opacity: its colours, corners,
//! blurs, shadow, effect payload, image and place in the paint order, and how far apart two
//! frames of it look.

use crate::color::{Color, over};
use crate::el::motion::MotionIntent;

/// Colours an element paints, in linear RGBA (`C_*` index them).
pub const COLORS: usize = 6;
pub const C_FILL: usize = 0;
pub const C_BORDER: usize = 1;
pub const C_TEXT: usize = 2;
pub const C_SHADOW: usize = 3;
pub const C_GRADIENT: usize = 4;
pub const C_OUTLINE: usize = 5;

/// Sizes an element paints, in points (`S_*` index them).
pub const SIZES: usize = 5;
pub const S_RADIUS: usize = 0;
pub const S_BORDER: usize = 1;
pub const S_BLUR: usize = 2;
pub const S_BACKDROP: usize = 3;
pub const S_SHADOW: usize = 4;

/// Every painted property an element presents on one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Paint {
    /// Fill (with its hover and press overlays and its paint alpha), border, text or icon,
    /// shadows together, gradient (its mean), outline.
    pub colors: [Color; COLORS],
    /// Corner radius, border width, blur, backdrop blur, shadow blur.
    pub sizes: [f32; SIZES],
    /// The second gradient colour (bottom, or a radial gradient's last stop).
    pub gradient_end: Color,
    /// `El::effect_data`, when it has an effect.
    pub effect: Option<[f32; 4]>,
    /// Which image content it shows (`El::pixels`), changing with each new frame of it.
    pub image: Option<(u64, u64)>,
    /// Its place in the paint order: higher paints over lower.
    pub rank: u32,
    /// [`MotionIntent`] bits marked on it or an element it sits in.
    pub intent: u16,
    /// It or its parent changes at once by design (`El::instant_changes`).
    pub instant: bool,
    /// Channels (a bit per index in [`CHANNELS`]) it lands at once by design: those its own
    /// `El::motion(Spring::NONE)` or `El::blur_motion(Spring::NONE)` governs (see [`at_once`]).
    pub at_once: u16,
    /// How hovered and how pressed it shows, 0 to 1. A highlight lands with the pointer by
    /// design, so a fill that steps with these isn't a snap.
    pub pointer: [f32; 2],
}

impl Default for Paint {
    fn default() -> Self {
        Paint { colors: [Color::TRANSPARENT; COLORS], sizes: [0.0; SIZES], gradient_end: Color::TRANSPARENT, effect: None, image: None, rank: 0, intent: 0, instant: false, at_once: 0, pointer: [0.0; 2] }
    }
}

impl Paint {
    /// Whether channel `i` of [`CHANNELS`] lands at once on it by design.
    pub fn at_once(&self, i: usize) -> bool {
        self.at_once & (1 << i) != 0
    }

    pub fn intends(&self, any: MotionIntent) -> bool {
        self.intent & any.bits() != 0
    }

    /// Whether it paints a box whose corners show.
    pub fn shows_corners(&self) -> bool {
        self.colors[C_FILL].a > 0.02 || self.colors[C_BORDER].a > 0.02 && self.sizes[S_BORDER] > 0.0 || self.colors[C_GRADIENT].a > 0.02 || self.sizes[S_BACKDROP] > 0.0
    }

    /// Whether its own fill hides what it covers.
    pub fn opaque(&self) -> bool {
        self.colors[C_FILL].a > 0.6 || self.colors[C_GRADIENT].a > 0.6 || self.image.is_some()
    }
}

/// OKLab with its lightness through Ottosson's toe (`L_r`), which follows CIELAB's near black:
/// plain OKLab's cube root makes the last percent of a fade to black look like a big step.
pub fn toe([l, a, b]: [f32; 3]) -> [f32; 3] {
    const K1: f32 = 0.206;
    const K2: f32 = 0.03;
    const K3: f32 = (1.0 + K1) / (1.0 + K2);
    let x = K3 * l - K1;
    let lr = 0.5 * (x + (x * x + 4.0 * K2 * K3 * l).max(0.0).sqrt());
    let s = if l > 1e-4 { lr / l } else { 0.0 };
    [lr, a * s, b * s]
}

fn seen(c: Color) -> [f32; 3] {
    let [l, a, b, _] = c.to_oklab();
    toe([l, a, b])
}

/// Perceptual distance between two colours as they'd show on black and on white, whichever is
/// larger, in OKLab (0 is the same, 1 is black to white). An alpha change counts as far as it
/// shows.
pub fn oklab_de(a: Color, b: Color) -> f32 {
    let on = |c: Color, bg: Color| -> [f32; 3] {
        let [r, g, b, al] = over(c.premul(1.0), bg.premul(1.0));
        seen(Color { r, g, b, a: al })
    };
    [Color::BLACK, Color::WHITE].into_iter().map(|bg| {
        let (x, y) = (on(a, bg), on(b, bg));
        ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
    }).fold(0.0, f32::max)
}

/// How much a change over `area` pt² counts against a 40×40 control: a large surface's
/// change shows more, a speck's less, and one under 16 pt² not at all.
pub fn area_weight(area: f32) -> f32 {
    if area < 16.0 { 0.0 } else { (area / 1600.0).powf(0.25).clamp(0.5, 2.5) }
}

/// One painted property the checks follow: where a frame's value sits, so that how far apart
/// two frames' points are is how different they look, and how much of it shows.
pub struct Channel {
    pub name: &'static str,
    /// Its value as a point, scaled so a distance of 1 is a change a person sees at once on a
    /// 40×40 control.
    pub embed: fn(&Paint) -> V,
    /// How much a change counts for, given the visible part `rect` (see [`area_weight`]).
    pub weight: fn(&Paint, &[f32; 4]) -> f32,
}

pub type V = [f32; 6];

fn ring(r: &[f32; 4], width: f32) -> f32 {
    (2.0 * (r[2] + r[3]) * width.max(0.0)).min(r[2] * r[3])
}

/// A colour as it shows on black and on white, in OKLab, scaled so ΔE 0.08 is 1.
fn lab(c: Color) -> V {
    let on = |bg: Color| {
        let [r, g, b, al] = over(c.premul(1.0), bg.premul(1.0));
        seen(Color { r, g, b, a: al })
    };
    let (k, w) = (on(Color::BLACK), on(Color::WHITE));
    [k[0], k[1], k[2], w[0], w[1], w[2]].map(|v| v / (0.08 * std::f32::consts::SQRT_2))
}

fn size(v: f32, unit: f32) -> V {
    [v / unit, 0.0, 0.0, 0.0, 0.0, 0.0]
}

/// A point for each distinct value of a hash, all √2 apart: a swap is a step of 1.4 and a swap
/// back returns where it was.
fn token(h: u64) -> V {
    let mut x = h ^ 0x9E37_79B9_7F4A_7C15;
    std::array::from_fn(|_| {
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 29;
        (x >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    }).map(|v: f32| v / 1.41)
}

fn area(r: &[f32; 4]) -> f32 {
    r[2].max(0.0) * r[3].max(0.0)
}

/// Every channel the element checks follow.
pub const CHANNELS: &[Channel] = &[
    Channel { name: "fill", embed: |p| lab(p.colors[C_FILL]), weight: |_, r| area_weight(area(r)) },
    Channel { name: "border", embed: |p| lab(p.colors[C_BORDER]), weight: |p, r| area_weight(ring(r, p.sizes[S_BORDER].max(1.0)) * 6.0) },
    Channel { name: "text-color", embed: |p| lab(p.colors[C_TEXT]), weight: |_, r| area_weight(0.4 * area(r)) },
    Channel { name: "shadow", embed: |p| lab(p.colors[C_SHADOW]), weight: |_, r| area_weight(0.5 * area(r)) },
    Channel { name: "gradient", embed: |p| {
        let (a, b) = (lab(p.colors[C_GRADIENT]), lab(p.gradient_end));
        std::array::from_fn(|i| (a[i] + b[i]) / 2.0)
    }, weight: |_, r| area_weight(area(r)) },
    Channel { name: "outline", embed: |p| lab(p.colors[C_OUTLINE]), weight: |_, r| area_weight(ring(r, 2.0) * 6.0) },
    Channel { name: "radius", embed: |p| size(p.sizes[S_RADIUS], 3.0), weight: |p, _| if p.shows_corners() { 1.0 } else { 0.0 } },
    Channel { name: "border-width", embed: |p| size(p.sizes[S_BORDER], 1.0), weight: |p, _| if p.colors[C_BORDER].a > 0.05 { 1.0 } else { 0.0 } },
    Channel { name: "blur", embed: |p| size(p.sizes[S_BLUR], 2.0), weight: |_, _| 1.0 },
    Channel { name: "backdrop-blur", embed: |p| size(p.sizes[S_BACKDROP], 4.0), weight: |_, _| 1.0 },
    Channel { name: "shadow-blur", embed: |p| size(p.sizes[S_SHADOW], 4.0), weight: |p, _| if p.colors[C_SHADOW].a > 0.05 { 1.0 } else { 0.0 } },
    Channel { name: "effect", embed: |p| match p.effect {
        Some(d) => [d[0], d[1], d[2], d[3], 0.0, 1.0].map(|v| v / 0.5),
        None => [0.0; 6],
    }, weight: |_, r| if area(r) >= 16.0 { 1.0 } else { 0.0 } },
    Channel { name: "image", embed: |p| p.image.map_or([0.0; 6], |(id, g)| token(id.wrapping_mul(31).wrapping_add(g))), weight: |_, r| if area(r) >= 16.0 { 1.0 } else { 0.0 } },
];

/// The [`Paint::at_once`] bits for an element whose own `El::motion` is `Spring::NONE`
/// (`motion`) and whose own blur curve is instant (`blur`; `Some(false)` when it has a curve of
/// its own that isn't). Its colours, corners and shadow land on that spring, and its blur too
/// unless it has its own curve; text colour keeps a short fade, and border width, backdrop
/// blur, effect and image don't spring at all.
pub fn at_once(motion: bool, blur: Option<bool>) -> u16 {
    let sprung: &[&str] = &["fill", "border", "shadow", "gradient", "outline", "radius", "shadow-blur"];
    let blurs = blur.unwrap_or(motion);
    CHANNELS.iter().enumerate()
        .filter(|(_, c)| motion && sprung.contains(&c.name) || blurs && c.name == "blur")
        .fold(0, |bits, (i, _)| bits | 1 << i)
}

/// The colour a channel shows, for evidence: its swatch.
pub fn swatch(p: &Paint, channel: &str) -> Option<Color> {
    Some(match channel {
        "fill" => p.colors[C_FILL],
        "border" => p.colors[C_BORDER],
        "text-color" => p.colors[C_TEXT],
        "shadow" => p.colors[C_SHADOW],
        "gradient" => p.colors[C_GRADIENT],
        "outline" => p.colors[C_OUTLINE],
        _ => return None,
    })
}

/// A channel's value as text, for evidence.
pub fn value(p: &Paint, channel: &str) -> String {
    if let Some(c) = swatch(p, channel) { return c.hex(); }
    match channel {
        "radius" => format!("{:.1}", p.sizes[S_RADIUS]),
        "border-width" => format!("{:.1}", p.sizes[S_BORDER]),
        "blur" => format!("{:.1}", p.sizes[S_BLUR]),
        "backdrop-blur" => format!("{:.1}", p.sizes[S_BACKDROP]),
        "shadow-blur" => format!("{:.1}", p.sizes[S_SHADOW]),
        "effect" => p.effect.map_or("none".into(), |d| format!("[{:.2} {:.2} {:.2} {:.2}]", d[0], d[1], d[2], d[3])),
        "image" => p.image.map_or("none".into(), |(id, g)| format!("#{id}.{g}")),
        "z-order" => p.rank.to_string(),
        _ => String::new(),
    }
}
