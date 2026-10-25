/// Linear or sRGB RGBA, as agreed with the host renderer. The core does not convert
/// color spaces or premultiply alpha. Components must be finite and in `0..=1`.
pub type Color = [f32; 4];

/// A logical-pixel rectangle. Containment uses half-open edges: `[x, right) × [y, bottom)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(self) -> f32 {
        self.x + self.width
    }
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    /// Invalid and non-positive rectangles are empty, never hit targets.
    pub fn is_empty(self) -> bool {
        !self.is_valid() || self.width == 0.0 || self.height == 0.0
    }

    pub fn is_valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 0.0
            && self.height >= 0.0
            && self.right().is_finite()
            && self.bottom().is_finite()
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        !self.is_empty()
            && x >= self.x
            && x < self.right()
            && y >= self.y
            && y < self.bottom()
    }

    /// Empty intersections have zero size at the maximum of the two origins.
    pub fn intersection(self, other: Self) -> Self {
        if !self.is_valid() || !other.is_valid() {
            return Self::ZERO;
        }
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Self::new(
            x,
            y,
            (self.right().min(other.right()) - x).max(0.0),
            (self.bottom().min(other.bottom()) - y).max(0.0),
        )
    }

    pub fn translated(self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.width, self.height)
    }
}
