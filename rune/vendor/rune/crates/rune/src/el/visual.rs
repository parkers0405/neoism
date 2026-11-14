//! `Visual`: hot inline paint fields on `El` and `Node`, with rarely set extras behind `Cold`.
use std::ops::Deref;
use std::rc::Rc;

use rune_motion::{Anim, Transition};

use crate::cold::{Cold, Empty};
use crate::color::Color;
use crate::el::{Outline, RadialGradient, Shadow};
use crate::shape::Shape;
use crate::text::Icon;

/// Rarely set visual extras shared behind `Cold<VisualCold>` so common boxes and text pay 8 bytes.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct VisualCold {
    pub surface: Option<(crate::NativeSurface, crate::SurfaceFit)>,
    /// Where a surface or `pixels_fit` image sits in the rect (`El::surface_anchor`, `El::surface_unscaled`).
    pub(crate) surface_place: crate::native_surface::place::SurfacePlace,
    /// Bottom colour of a vertical border gradient whose top is `border` (`El::border_gradient`).
    pub border_bottom: Option<Color>,
    /// Per-side border widths `[top, right, bottom, left]` in points (`El::border_sides`).
    pub(crate) border_sides: Option<[f32; 4]>,
    /// The border takes room in layout, snapped to device pixels (`El::border_layout`).
    pub(crate) border_layout: bool,
    /// Dash and gap lengths of a dashed border, in points (`El::border_dash`).
    pub border_dash: Option<[f32; 2]>,
    /// Points into the dash pattern where it starts (`El::border_dash_phase`).
    pub border_dash_phase: f32,
    pub shadows: Vec<Shadow>,
    pub blur_motion: Option<Box<Anim>>,
    /// Soft clip of children to this rect: fade widths left, top, right, bottom. A negative
    /// width ramps linearly (`El::clip_fade_linear`).
    pub clip: Option<[f32; 4]>,
    /// Rounded clip of this element and its subtree, applied after subtree filters.
    pub clip_radius: Option<f32>,
    /// Per-corner clip radii override (`[tl, tr, br, bl]`, in points).
    pub(crate) clip_corners: Option<[f32; 4]>,
    /// A round hole through this element and its subtree (`El::cutout_circle`): centre x and y
    /// from the element's top-left, and the radii it's clear within and whole from, in points.
    pub(crate) cutout: Option<[f32; 4]>,
    pub icon: Option<(&'static Icon, Color)>,
    /// The icon's colour came from `El::color`; without it the icon takes the window's
    /// foreground (`Cx::foreground`) when the view sets one.
    pub(crate) icon_inked: bool,
    /// A colour icon draws as its silhouette in the icon's colour (`El::icon_one_color`).
    pub(crate) icon_one_color: bool,
    /// Vertical linear gradient (top, bottom) painted over the fill, under hover/press overlays.
    pub gradient: Option<Box<[Color; 2]>>,
    /// Radial or angled linear gradient painted over the fill, under hover/press overlays.
    pub(crate) radial: Option<Box<RadialGradient>>,
    pub outline: Option<Outline>,
    /// Vector strokes and fills in the element's own points, over the fill.
    pub shape: Option<Rc<Shape>>,
    /// An RGBA image scaled into the element's rect, over the fill and shape.
    pub pixels: Option<crate::pixels::Pixels>,
    /// How `pixels` keeps its aspect ratio; `None` stretches it to the rect.
    pub pixels_fit: Option<crate::SurfaceFit>,
    /// An effect slot (`El::effect`).
    pub(crate) effect: Option<crate::el::effect::EffectSlot>,
    /// Backdrop blur standard deviation in points (`El::backdrop_blur`); 0 is off.
    pub(crate) backdrop_blur: f32,
    /// The hover track's enter and exit curves (default: snappy spring both ways).
    pub hover_motion: Option<Rc<(Anim, Anim)>>,
    /// Hover curves for this element and the hover tracks below it without their own.
    pub(crate) hover_within: Option<Rc<(Anim, Anim)>>,
    /// Opacity factor (rest, hovered) following the nearest ancestor group's hover.
    pub group_opacity: Option<[f32; 2]>,
    /// Text or icon colour while the nearest ancestor group is hovered.
    pub group_color: Option<Color>,
    /// `El::hover_alone`: while the pointer is on this element, the hover tracks of the
    /// elements it sits inside stay off.
    pub hover_alone: bool,
    /// `El::hover_group_shared`: the key of the elements whose hover this group shares.
    pub(crate) hover_share: Option<Rc<str>>,
    /// `El::hover_group_focus`: keyboard focus on it or inside it hovers it too.
    pub(crate) hover_focus: bool,
    /// `El::icon_rotation`: the icon's turn in radians, clockwise.
    pub(crate) icon_turn: f32,
    /// Optional app-supplied key mixed into the cached subtree's signature (`El::cache_key`).
    pub(crate) cache_key: Option<u64>,
    pub(crate) offset_motion: Option<Box<Anim>>,
    /// The curve `El::scale` changes on.
    pub(crate) scale_motion: Option<Box<Anim>>,
    /// `El::origin`: the unit point scale and rotation pivot about; `None` is the centre.
    pub(crate) origin: Option<[f32; 2]>,
    /// The curve `El::rotate` changes on.
    pub(crate) rotate_motion: Option<Box<Anim>>,
    /// `El::child_exit`: how children removed in a commit leave, in place of their own.
    pub(crate) child_exit: Option<Box<Transition>>,
    /// `El::enter_delay`: seconds a new element holds its insertion effect before entering.
    pub(crate) enter_delay: f32,
    /// `El::motion_intent`: authored motion traits inherited by this subtree.
    pub(crate) motion_intent: crate::el::motion::MotionIntent,
    /// `El::motion(Spring::NONE)` written on this element itself, not inherited or from a
    /// token: what that spring governs here lands at once by design.
    pub(crate) instant_motion: bool,
    /// A scroll area's wrapper (`El::wrap_scroll_area`): rectangular clips and fades set on it
    /// go to its first child, the viewport, so what it draws beside that stays whole.
    pub(crate) clip_to_content: bool,
    /// `El::matched_crop`: a matched flight keeps the content at its own size, cropped.
    pub(crate) flight_crop: bool,
    /// `El::hold_landing_width`: an animated width change lands its contents and slides them.
    pub(crate) hold_landing: bool,
    /// `El::offset`: a visual shift in points.
    pub(crate) offset: [f32; 2],
    /// Static alpha of this element's own paint; never inherited or isolated.
    pub paint_alpha: f32,
    /// `El::layer_opacity`: static alpha its whole subtree composites at, as one layer.
    pub(crate) layer_opacity: f32,
    pub blur: f32,
    /// `El::rotate`, in radians clockwise.
    pub(crate) rotate: f32,
    /// App-defined tag a compositor reads per instance (`Runtime::marks`); 0 inherits the parent's.
    pub mark: u32,
    /// `El::mirror_in_rtl`: its icon flips left to right in a right-to-left element.
    pub(crate) mirror_rtl: bool,
    /// `El::paint_order`: where it paints among its siblings; 0 is tree order.
    pub(crate) paint_order: i16,
}

impl Empty for VisualCold {
    const EMPTY: &'static Self = &VisualCold {
        surface: None,
        surface_place: crate::native_surface::place::SurfacePlace::CENTRED,
        border_bottom: None,
        border_sides: None,
        border_layout: false,
        border_dash: None,
        border_dash_phase: 0.0,
        shadows: Vec::new(),
        blur_motion: None,
        clip: None,
        clip_radius: None,
        clip_corners: None,
        cutout: None,
        icon: None,
        icon_inked: false,
        icon_one_color: false,
        gradient: None,
        radial: None,
        outline: None,
        shape: None,
        pixels: None,
        pixels_fit: None,
        effect: None,
        backdrop_blur: 0.0,
        hover_motion: None,
        hover_within: None,
        group_opacity: None,
        group_color: None,
        hover_alone: false,
        hover_focus: false,
        hover_share: None,
        icon_turn: 0.0,
        cache_key: None,
        offset_motion: None,
        scale_motion: None,
        origin: None,
        rotate_motion: None,
        child_exit: None,
        enter_delay: 0.0,
        motion_intent: crate::el::motion::MotionIntent::NONE,
        instant_motion: false,
        clip_to_content: false,
        flight_crop: false,
        hold_landing: false,
        offset: [0.0; 2],
        paint_alpha: 1.0,
        layer_opacity: 1.0,
        blur: 0.0,
        rotate: 0.0,
        mark: 0,
        mirror_rtl: false,
        paint_order: 0,
    };
}

/// Border, hover and press colours and per-corner radii: interactive boxes set them, most
/// boxes don't, so they wait behind `Cold<VisualBox>`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VisualBox {
    pub(crate) border: Color,
    pub(crate) hover: Color,
    pub(crate) press: Color,
    /// Per-corner radii `[tl, tr, br, bl]`, read when `Visual::radius` is NaN.
    pub(crate) corners: [f32; 4],
    pub(crate) border_width: f32,
}

impl Empty for VisualBox {
    const EMPTY: &'static Self = &VisualBox {
        border: Color::TRANSPARENT,
        hover: Color::TRANSPARENT,
        press: Color::TRANSPARENT,
        corners: [0.0; 4],
        border_width: 0.0,
    };
}

/// The paint fields every box may set; the rest wait behind `box_paint` and `cold`.
#[derive(Clone, Debug)]
pub struct Visual {
    pub fill: Color,
    /// The corner radius; NaN when the corners differ, which `box_paint.corners` then holds.
    pub(crate) radius: f32,
    pub opacity: f32,
    pub(crate) box_paint: Cold<VisualBox>,
    pub(crate) cold: Cold<VisualCold>,
}

impl Visual {
    #[inline]
    pub(crate) fn cold_mut(&mut self) -> &mut VisualCold {
        &mut self.cold
    }

    #[inline]
    pub(crate) fn box_paint_mut(&mut self) -> &mut VisualBox {
        &mut self.box_paint
    }

    /// The corner radii `[tl, tr, br, bl]`.
    #[inline]
    pub(crate) fn radii(&self) -> [f32; 4] {
        if self.radius.is_nan() { self.box_paint.corners } else { [self.radius; 4] }
    }

    /// Sets the corner radii, inline when they're all the same.
    #[inline]
    pub(crate) fn set_radii(&mut self, r: [f32; 4]) {
        if r == [r[0]; 4] {
            self.radius = r[0];
        } else {
            self.radius = f32::NAN;
            self.box_paint_mut().corners = r;
        }
    }

    #[inline]
    pub(crate) fn border(&self) -> Color {
        self.box_paint.border
    }

    #[inline]
    pub(crate) fn border_width(&self) -> f32 {
        self.box_paint.border_width
    }

    #[inline]
    pub(crate) fn hover(&self) -> Color {
        self.box_paint.hover
    }

    #[inline]
    pub(crate) fn press(&self) -> Color {
        self.box_paint.press
    }

    /// Sets the hover colour, allocating `box_paint` only for one that isn't the default.
    #[inline]
    pub(crate) fn set_hover(&mut self, c: Color) {
        if c != Color::TRANSPARENT || !self.box_paint.is_empty() {
            self.box_paint_mut().hover = c;
        }
    }

    /// Sets the press colour, allocating `box_paint` only for one that isn't the default.
    #[inline]
    pub(crate) fn set_press(&mut self, c: Color) {
        if c != Color::TRANSPARENT || !self.box_paint.is_empty() {
            self.box_paint_mut().press = c;
        }
    }

    /// Sets the border's colour alone, allocating `box_paint` only for a change.
    #[inline]
    pub(crate) fn set_border_color(&mut self, c: Color) {
        if c != self.box_paint.border {
            self.box_paint_mut().border = c;
        }
    }

    /// Sets the border, allocating `box_paint` only for one that isn't the default.
    #[inline]
    pub(crate) fn set_border(&mut self, width: f32, c: Color) {
        if width != 0.0 || c != Color::TRANSPARENT || !self.box_paint.is_empty() {
            let b = self.box_paint_mut();
            b.border_width = width;
            b.border = c;
        }
    }
}

impl Deref for Visual {
    type Target = VisualCold;
    #[inline(always)]
    fn deref(&self) -> &VisualCold {
        &self.cold
    }
}

impl Default for Visual {
    #[inline]
    fn default() -> Self {
        Visual { fill: Color::TRANSPARENT, radius: 0.0, opacity: 1.0, box_paint: Cold::NONE, cold: Cold::NONE }
    }
}

/// `Visual`'s flags, one bit each, kept beside it on `El` and `Node`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VisualFlags(u8);

impl VisualFlags {
    /// Rectangular native input region for overlays using `OverlayInput::Regions`.
    pub(crate) const HIT: u8 = 1;
    /// Corners follow Apple's continuous curve instead of circular arcs (`El::continuous_corners`).
    pub(crate) const CONTINUOUS_CORNERS: u8 = 1 << 1;
    /// Composites with `Blend::PlusLighter` (`El::plus_lighter`).
    pub(crate) const PLUS_LIGHTER: u8 = 1 << 2;
    /// Tracks hover for its descendants' `group_*` styles even without a hover colour. Any
    /// element with a hover colour is a group too.
    pub(crate) const HOVER_GROUP: u8 = 1 << 3;
    /// Rasterize this subtree once into an offscreen texture when settled (`El::cache`).
    pub(crate) const CACHE: u8 = 1 << 4;
    /// `El::raise`: paints and takes input above the rest of the window.
    pub(crate) const RAISE: u8 = 1 << 5;
    /// `El::pixel_snap(false)`: this subtree paints at its exact fractional rects.
    pub(crate) const NO_PIXEL_SNAP: u8 = 1 << 6;
    /// `El::ride_trailing`: x holds against the parent's trailing edge while its width moves.
    pub(crate) const RIDE_TRAILING: u8 = 1 << 7;

    #[inline(always)]
    pub(crate) fn set(&mut self, bit: u8, on: bool) {
        if on { self.0 |= bit } else { self.0 &= !bit }
    }

    #[inline(always)]
    pub(crate) fn hit(self) -> bool { self.0 & Self::HIT != 0 }
    #[inline(always)]
    pub(crate) fn continuous_corners(self) -> bool { self.0 & Self::CONTINUOUS_CORNERS != 0 }
    #[inline(always)]
    pub(crate) fn blend(self) -> crate::render::Blend {
        if self.0 & Self::PLUS_LIGHTER != 0 { crate::render::Blend::PlusLighter } else { crate::render::Blend::SourceOver }
    }
    #[inline(always)]
    pub(crate) fn hover_group(self) -> bool { self.0 & Self::HOVER_GROUP != 0 }
    #[inline(always)]
    pub(crate) fn cache(self) -> bool { self.0 & Self::CACHE != 0 }
    #[inline(always)]
    pub(crate) fn raise(self) -> bool { self.0 & Self::RAISE != 0 }
    #[inline(always)]
    pub(crate) fn no_pixel_snap(self) -> bool { self.0 & Self::NO_PIXEL_SNAP != 0 }
    #[inline(always)]
    pub(crate) fn ride_trailing(self) -> bool { self.0 & Self::RIDE_TRAILING != 0 }
}
