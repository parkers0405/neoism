//! Values that can animate: anything that maps to a fixed number of independent `f32` lanes.
//!
//! Each lane springs independently on the same timing, so a `Rect` is four edge springs and a
//! `Color` is four premultiplied-OKLab springs. Lanes are what the store keeps in its flat arrays.

/// Most lanes a single animated value may have.
pub const MAX_LANES: usize = 16;

/// A value made of `LANES` independent scalar lanes (a vector space).
pub trait Animatable: Copy {
    const LANES: usize;
    fn write_lanes(&self, out: &mut [f32]);
    fn read_lanes(lanes: &[f32]) -> Self;

    fn add(self, other: Self) -> Self {
        zip(self, other, |a, b| a + b)
    }
    fn sub(self, other: Self) -> Self {
        zip(self, other, |a, b| a - b)
    }
    fn scale(self, k: f32) -> Self {
        zip(self, self, |a, _| a * k)
    }
    /// Euclidean distance in lane space.
    fn distance(self, other: Self) -> f32 {
        let (a, b) = (lanes(&self), lanes(&other));
        (0..Self::LANES).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum::<f32>().sqrt()
    }
    /// `self + (other − self) · t` in lane space.
    fn lerp(self, other: Self, t: f32) -> Self {
        zip(self, other, |a, b| a + (b - a) * t)
    }
}

pub(crate) fn lanes<T: Animatable>(v: &T) -> [f32; MAX_LANES] {
    assert!(T::LANES <= MAX_LANES, "at most {MAX_LANES} lanes");
    let mut out = [0.0; MAX_LANES];
    v.write_lanes(&mut out[..T::LANES]);
    out
}

fn zip<T: Animatable>(a: T, b: T, f: impl Fn(f32, f32) -> f32) -> T {
    let (x, y) = (lanes(&a), lanes(&b));
    let mut out = [0.0; MAX_LANES];
    for i in 0..T::LANES {
        out[i] = f(x[i], y[i]);
    }
    T::read_lanes(&out[..T::LANES])
}

impl Animatable for f32 {
    const LANES: usize = 1;
    fn write_lanes(&self, out: &mut [f32]) {
        out[0] = *self;
    }
    fn read_lanes(l: &[f32]) -> Self {
        l[0]
    }
}

/// A 2D point, size or offset in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
    pub const fn new(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }
}

impl Animatable for Vec2 {
    const LANES: usize = 2;
    fn write_lanes(&self, out: &mut [f32]) {
        out[0] = self.x;
        out[1] = self.y;
    }
    fn read_lanes(l: &[f32]) -> Self {
        Vec2 { x: l[0], y: l[1] }
    }
}

/// An axis-aligned rectangle. It animates as its four **edges** (left, top, right, bottom), so an
/// edge that doesn't move has no motion at all and can't jiggle (research 06 §2.7).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn from_edges(left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect { x: left, y: top, w: right - left, h: bottom - top }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn edges(&self) -> [f32; 4] {
        [self.x, self.y, self.right(), self.bottom()]
    }
    pub fn origin(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.w, self.h)
    }
    pub fn center(&self) -> Vec2 {
        Vec2::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
    pub fn translated(&self, d: Vec2) -> Rect {
        Rect { x: self.x + d.x, y: self.y + d.y, ..*self }
    }
    pub fn inflate(self, m: f32) -> Rect {
        Rect { x: self.x - m, y: self.y - m, w: self.w + 2.0 * m, h: self.h + 2.0 * m }
    }
    pub fn union(self, o: Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
    pub fn from_points(a: [f32; 2], b: [f32; 2]) -> Rect {
        let x0 = a[0].min(b[0]);
        let y0 = a[1].min(b[1]);
        Rect { x: x0, y: y0, w: (a[0] - b[0]).abs(), h: (a[1] - b[1]).abs() }
    }
    pub fn xywh(self) -> [f32; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

impl Animatable for Rect {
    const LANES: usize = 4;
    fn write_lanes(&self, out: &mut [f32]) {
        out[..4].copy_from_slice(&self.edges());
    }
    fn read_lanes(l: &[f32]) -> Self {
        Rect::from_edges(l[0], l[1], l[2], l[3])
    }
}

/// Per-corner radii: top-left, top-right, bottom-right, bottom-left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Corners(pub [f32; 4]);

impl Corners {
    pub const ZERO: Corners = Corners([0.0; 4]);
    pub const fn all(r: f32) -> Corners {
        Corners([r; 4])
    }
}

impl Animatable for Corners {
    const LANES: usize = 4;
    fn write_lanes(&self, out: &mut [f32]) {
        out[..4].copy_from_slice(&self.0);
    }
    fn read_lanes(l: &[f32]) -> Self {
        Corners([l[0], l[1], l[2], l[3]])
    }
}

/// Insets per side (left, top, right, bottom): mask insets and feathers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sides(pub [f32; 4]);

impl Sides {
    pub const ZERO: Sides = Sides([0.0; 4]);
    pub const fn all(v: f32) -> Sides {
        Sides([v; 4])
    }
}

impl Animatable for Sides {
    const LANES: usize = 4;
    fn write_lanes(&self, out: &mut [f32]) {
        out[..4].copy_from_slice(&self.0);
    }
    fn read_lanes(l: &[f32]) -> Self {
        Sides([l[0], l[1], l[2], l[3]])
    }
}

/// A Gaussian blur sigma (standard deviation) in points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Blur(pub f32);

impl Animatable for Blur {
    const LANES: usize = 1;
    fn write_lanes(&self, out: &mut [f32]) {
        out[0] = self.0;
    }
    fn read_lanes(l: &[f32]) -> Self {
        Blur(l[0])
    }
}

/// A decomposed 2D transform: translate, then scale and rotate about the node's anchor.
/// Decomposed parts animate independently, so a spinning, growing card never shears.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Transform {
    pub translate: Vec2,
    pub scale: Vec2,
    /// Radians, unwrapped: animating 0 → 4π spins twice, always the same way.
    pub rotate: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform =
        Transform { translate: Vec2::ZERO, scale: Vec2 { x: 1.0, y: 1.0 }, rotate: 0.0 };

    /// The 2×3 affine matrix `[a, b, c, d, tx, ty]` (column-major like CGAffineTransform) that
    /// applies this transform about `anchor` (in the same space as the translate).
    pub fn to_affine(&self, anchor: Vec2) -> [f32; 6] {
        let (s, c) = crate::math::sin_cos(self.rotate);
        let (a, b) = (c * self.scale.x, s * self.scale.x);
        let (cc, d) = (-s * self.scale.y, c * self.scale.y);
        // p' = M (p − anchor) + anchor + translate
        let tx = anchor.x + self.translate.x - (a * anchor.x + cc * anchor.y);
        let ty = anchor.y + self.translate.y - (b * anchor.x + d * anchor.y);
        [a, b, cc, d, tx, ty]
    }
}

impl Animatable for Transform {
    const LANES: usize = 5;
    fn write_lanes(&self, out: &mut [f32]) {
        out[..5].copy_from_slice(&[self.translate.x, self.translate.y, self.scale.x, self.scale.y, self.rotate]);
    }
    fn read_lanes(l: &[f32]) -> Self {
        Transform { translate: Vec2::new(l[0], l[1]), scale: Vec2::new(l[2], l[3]), rotate: l[4] }
    }
}

/// An sRGB colour with straight (unpremultiplied) alpha, components in 0…1.
///
/// It animates in **premultiplied OKLab** (`[L·α, a·α, b·α, α]`): hue changes don't pass through
/// grey, and fading to transparent doesn't drag the colour toward black.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    /// `0xRRGGBB` with alpha 1.
    pub const fn hex(rgb: u32) -> Color {
        Color {
            r: ((rgb >> 16) & 0xff) as f32 / 255.0,
            g: ((rgb >> 8) & 0xff) as f32 / 255.0,
            b: (rgb & 0xff) as f32 / 255.0,
            a: 1.0,
        }
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// `[L, a, b]` in OKLab (Björn Ottosson's matrices).
    pub fn to_oklab(self) -> [f32; 3] {
        linear_to_oklab(srgb_decode(self.r), srgb_decode(self.g), srgb_decode(self.b))
    }

    pub fn from_oklab(lab: [f32; 3], alpha: f32) -> Color {
        let [r, g, b] = oklab_to_linear(lab).map(|c| srgb_encode(c.clamp(0.0, 1.0)));
        Color { r, g, b, a: alpha }
    }
}

/// Linear sRGB `[r, g, b]` to OKLab `[L, a, b]` (Björn Ottosson's matrices).
pub fn linear_to_oklab(r: f32, g: f32, b: f32) -> [f32; 3] {
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab `[L, a, b]` to unclamped linear sRGB `[r, g, b]`.
pub fn oklab_to_linear([ll, a, b]: [f32; 3]) -> [f32; 3] {
    let l = ll + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = ll - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = ll - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

pub fn srgb_decode(c: f32) -> f32 {
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn srgb_encode(c: f32) -> f32 {
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

impl Animatable for Color {
    const LANES: usize = 4;
    fn write_lanes(&self, out: &mut [f32]) {
        let [l, a, b] = self.to_oklab();
        let al = self.a;
        out[..4].copy_from_slice(&[l * al, a * al, b * al, al]);
    }
    fn read_lanes(x: &[f32]) -> Self {
        let alpha = x[3].clamp(0.0, 1.0);
        if x[3] <= 1e-6 {
            return Color::TRANSPARENT;
        }
        Color::from_oklab([x[0] / x[3], x[1] / x[3], x[2] / x[3]], alpha)
    }
}

impl<A: Animatable, B: Animatable> Animatable for (A, B) {
    const LANES: usize = A::LANES + B::LANES;
    fn write_lanes(&self, out: &mut [f32]) {
        self.0.write_lanes(&mut out[..A::LANES]);
        self.1.write_lanes(&mut out[A::LANES..A::LANES + B::LANES]);
    }
    fn read_lanes(l: &[f32]) -> Self {
        (A::read_lanes(&l[..A::LANES]), B::read_lanes(&l[A::LANES..A::LANES + B::LANES]))
    }
}

impl<A: Animatable, B: Animatable, C: Animatable> Animatable for (A, B, C) {
    const LANES: usize = A::LANES + B::LANES + C::LANES;
    fn write_lanes(&self, out: &mut [f32]) {
        let (a, b) = (A::LANES, A::LANES + B::LANES);
        self.0.write_lanes(&mut out[..a]);
        self.1.write_lanes(&mut out[a..b]);
        self.2.write_lanes(&mut out[b..b + C::LANES]);
    }
    fn read_lanes(l: &[f32]) -> Self {
        let (a, b) = (A::LANES, A::LANES + B::LANES);
        (A::read_lanes(&l[..a]), B::read_lanes(&l[a..b]), C::read_lanes(&l[b..b + C::LANES]))
    }
}

impl<T: Animatable, const N: usize> Animatable for [T; N] {
    const LANES: usize = T::LANES * N;
    fn write_lanes(&self, out: &mut [f32]) {
        for (i, v) in self.iter().enumerate() {
            v.write_lanes(&mut out[i * T::LANES..(i + 1) * T::LANES]);
        }
    }
    fn read_lanes(l: &[f32]) -> Self {
        core::array::from_fn(|i| T::read_lanes(&l[i * T::LANES..(i + 1) * T::LANES]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_round_trips() {
        for c in [Color::hex(0xff3b30), Color::hex(0x34c759), Color::hex(0x007aff), Color::WHITE] {
            let back = Color::read_lanes(&lanes(&c)[..4]);
            for (x, y) in [(c.r, back.r), (c.g, back.g), (c.b, back.b), (c.a, back.a)] {
                assert!((x - y).abs() < 1e-3, "{c:?} → {back:?}");
            }
        }
    }

    #[test]
    fn rect_lanes_are_edges() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(&lanes(&r)[..4], &[10.0, 20.0, 110.0, 70.0]);
        assert_eq!(Rect::read_lanes(&[10.0, 20.0, 110.0, 70.0]), r);
    }

    #[test]
    fn tuples_and_arrays() {
        let v = (1.0f32, Vec2::new(2.0, 3.0));
        assert_eq!(<(f32, Vec2)>::LANES, 3);
        assert_eq!(v.add(v), (2.0, Vec2::new(4.0, 6.0)));
        let a = [Vec2::new(1.0, 1.0); 3];
        assert_eq!(<[Vec2; 3]>::LANES, 6);
        assert!((a.distance([Vec2::ZERO; 3]) - 6f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn affine_rotates_about_anchor() {
        let t = Transform { rotate: core::f32::consts::FRAC_PI_2, ..Transform::IDENTITY };
        let [a, b, c, d, tx, ty] = t.to_affine(Vec2::new(10.0, 10.0));
        // The anchor stays put.
        let (x, y) = (a * 10.0 + c * 10.0 + tx, b * 10.0 + d * 10.0 + ty);
        assert!((x - 10.0).abs() < 1e-4 && (y - 10.0).abs() < 1e-4);
    }
}
