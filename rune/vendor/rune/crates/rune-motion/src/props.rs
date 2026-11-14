//! Typed property keys and the classes the runtime applies policy by.

use core::marker::PhantomData;

use crate::curve::Curve;
use crate::easing::Easing;
use crate::layer::Eps;
use crate::value::{Animatable, Color, Corners, Rect, Sides, Vec2};

/// A stable node key from the app side (a hash of the keyed path, a row id, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NodeId(pub u64);

/// An untyped property id, as it appears in events and traces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PropId(pub u16);

/// What kind of quantity a property is. The runtime picks settle thresholds and reduced-motion
/// behaviour from this, so authors can't forget either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropClass {
    /// Positions, edges, offsets, sizes (points).
    Spatial,
    Scale,
    /// Radians.
    Rotation,
    Opacity,
    /// Blur radius (points).
    Blur,
    /// Colours (premultiplied OKLab lanes).
    Color,
    /// Corner radii, borders, feathers (points).
    Shape,
    /// A 0…1 presence progress that fades content in and out.
    Presence,
    /// Values the user's own hand drives (scroll offset, drag position). Never reduced.
    Gesture,
    Other,
}

/// The fade that replaces travel under Reduce Motion (OpenCode `reducedFade` .20, 75 ms in a native app;
/// Kit: ≈ .15–.2 s).
pub const REDUCED_FADE: Curve = Curve::Tween { duration: 0.18, easing: Easing::EASE_IN_OUT };

impl PropClass {
    /// Settle thresholds, set below the 1/64 px snap quantum so the final snap is invisible
    /// (research 06 §2.3). `pixel_scale` is device pixels per point.
    pub fn eps(self, pixel_scale: f32) -> Eps {
        let px = 1.0 / (128.0 * pixel_scale.max(1.0));
        let pos = match self {
            PropClass::Spatial | PropClass::Shape | PropClass::Gesture | PropClass::Blur => px,
            PropClass::Scale | PropClass::Rotation | PropClass::Presence => 1e-5,
            PropClass::Opacity | PropClass::Color => 1.0 / 1024.0,
            PropClass::Other => 1e-4,
        };
        Eps { pos, vel: pos * 10.0 }
    }

    /// What Reduce Motion turns `curve` into for this class (research 06 §2.12): travel, scale,
    /// rotation, shape and blur jump; opacity, colour and presence take a short fade; the user's
    /// own gestures are untouched.
    pub fn reduce(self, curve: Curve) -> Curve {
        if matches!(curve, Curve::Instant | Curve::Decay(_)) {
            return curve;
        }
        match self {
            PropClass::Spatial
            | PropClass::Scale
            | PropClass::Rotation
            | PropClass::Shape
            | PropClass::Blur => Curve::Instant,
            PropClass::Opacity | PropClass::Color | PropClass::Presence => REDUCED_FADE,
            PropClass::Gesture | PropClass::Other => curve,
        }
    }
}

/// A typed property key: `prop::OPACITY` is a `Prop<f32>`, `prop::FRAME` a `Prop<Rect>`.
pub struct Prop<T> {
    pub id: PropId,
    pub class: PropClass,
    _t: PhantomData<fn() -> T>,
}

impl<T> Clone for Prop<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Prop<T> {}
impl<T> core::fmt::Debug for Prop<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Prop({}, {:?})", self.id.0, self.class)
    }
}

impl<T: Animatable> Prop<T> {
    /// A custom property. Ids below 64 are reserved for the built-ins.
    pub const fn custom(id: u16, class: PropClass) -> Prop<T> {
        assert!(id >= 64, "custom property ids start at 64");
        Prop { id: PropId(id), class, _t: PhantomData }
    }
    const fn builtin(id: u16, class: PropClass) -> Prop<T> {
        Prop { id: PropId(id), class, _t: PhantomData }
    }
}

/// Built-in properties the scene and renderer understand.
pub mod prop {
    use super::*;

    pub const OPACITY: Prop<f32> = Prop::builtin(0, PropClass::Opacity);
    /// The layout frame in parent space, animated as four edges.
    pub const FRAME: Prop<Rect> = Prop::builtin(1, PropClass::Spatial);
    pub const OFFSET: Prop<Vec2> = Prop::builtin(2, PropClass::Spatial);
    pub const SCALE: Prop<f32> = Prop::builtin(3, PropClass::Scale);
    pub const ROTATION: Prop<f32> = Prop::builtin(4, PropClass::Rotation);
    pub const BLUR: Prop<f32> = Prop::builtin(5, PropClass::Blur);
    pub const RADIUS: Prop<Corners> = Prop::builtin(6, PropClass::Shape);
    pub const FILL: Prop<Color> = Prop::builtin(7, PropClass::Color);
    /// Soft-mask insets (left, top, right, bottom) in points.
    pub const MASK: Prop<Sides> = Prop::builtin(8, PropClass::Shape);
    /// Scroll or drag offset owned by the render thread.
    pub const SCROLL: Prop<Vec2> = Prop::builtin(9, PropClass::Gesture);
    /// Presence progress: 0 absent, 1 present. Driven by the scene.
    pub const PRESENCE: Prop<f32> = Prop::builtin(10, PropClass::Presence);
    /// Screen-space travel progress (x, y) for matched geometry and `Travel::Screen`. A
    /// fraction of the path, so it settles like a scale: a point-sized threshold would end a
    /// long flight a few points short and snap.
    pub const TRAVEL: Prop<Vec2> = Prop::builtin(11, PropClass::Scale);

    /// Number of built-in property ids (the scene caches these per node).
    pub const BUILTIN_COUNT: usize = 12;
}
