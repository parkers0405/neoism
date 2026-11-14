//! The element tree that view functions return. Plain data plus type-erased handlers.

use std::rc::Rc;

use rune_motion::{Anim, Spring, Transition};
use rune_taffy::prelude::*;

use crate::color::Color;
pub(crate) use crate::el::drop::{DropHoverFn, DropSpec};
pub use crate::cursor::{Cursor, CursorImage};
pub(crate) use crate::el::sticky::{Sticky, StickyEdge};
use crate::edit::TextEdit;
use crate::growth::{GROWTH, StreamEdge, edge};
use crate::shape::Shape;
use crate::states::{downcast, edit_fn};
pub(crate) use crate::clipboard::tx::{ClipboardRead, ReadFn};
use crate::text::{Icon, Link, Span};
use crate::tok::TextStyle;

pub use crate::el::tx::{Shell, Tx};
pub(crate) use crate::el::tx::{SelectRequest, TraceCalls};

mod a11y;
mod anchor;
mod backdrop;
mod child_exit;
mod direction;
pub(crate) mod drop;
pub(crate) mod effect;

/// An effect slot (`El::effect`): size it like any box.
pub fn effect(mark: u32) -> El {
    base(FlexDirection::Row).effect(mark)
}
mod element_info;
mod enter_delay;
mod focus;
mod growth;
mod handler_set;
mod handlers;
mod layout;
pub(crate) mod layout_watch;
mod live_colors;
pub mod motion;
mod pills;
mod pixel_snap;
mod rare;
mod ride_trailing;
mod scroll;
mod selection_style;
mod sticky;
mod text;

pub fn text(content: impl Into<String>) -> El {
    let mut e = base(FlexDirection::Row);
    e.role = RoleTag::TEXT;
    e.text = Some(crate::cold::SharedBox::new(TextSpec {
        content: content.into(),
        style: crate::tok::type_::BODY,
        color: Color::WHITE,
        inked: false,
        align: Align::Start,
        spans: Vec::new(),
        wrap: false,
        reveal: -1.0,
        truncate: false,
        elide: Elide::End,
        max_lines: 0,
        truncate_fade: false,
        fade_em: TRUNCATE_FADE_EM,
        justify: false,
        live: None,
        links: Vec::new(),
        on_link: None,
        on_link_secondary: None,
        advance_in: None,
        case: None,
        source: None,
        selectable: false,
        selection: DEFAULT_SELECTION,
        uncopied: Vec::new(),
        copy_breaks: [1, 1],
        copy_prefix: None,
        rings: Vec::new(),
        span_radius: crate::text::SPAN_RADIUS,
        strikes: Vec::new(),
        highlights: Vec::new(),
        blocks: Vec::new(),
        soft_breaks: Vec::new(),
        pills: Vec::new(),
        pill_colors: None,
        rich_style: crate::edit::RichStyle::default(),
        redacted: false,
        secure_revealed: false,
        wrap_anywhere: false,
        exact_lines: true,
        flow: crate::el::Flow::Ltr,
        shape: None,
    }));
    e
}
mod text_spec;
mod tx;
pub(crate) mod virtual_list;
pub(crate) mod virtual_rows;
mod visual;
mod visuals;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
}

/// A pointer gesture on an element. `x`, `y`, `start_x`, `start_y`, `w` and `h` are in the
/// element's own layout points: from its top-left as drawn (after scrolling and live
/// offsets), divided by any scale it draws at, so they hold on a zoomed canvas.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub phase: Phase,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub start_x: f32,
    pub start_y: f32,
    /// Pointer travel since the press, in window points (independent of the element moving).
    pub dx: f32,
    pub dy: f32,
    /// Scratch kept across one gesture's phases (e.g. a grab offset).
    pub data: f32,
    /// Which click of a run the press is, the same on every phase: 1, then 2 and 3 (and on, as
    /// AppKit's `clickCount`) for presses on this element each within the double-click
    /// interval and 4 pt of the one before. A press that drags past 4 pt ends the run. A text
    /// view selects by words on 2 and lines on 3, and extends by them as the drag moves.
    pub clicks: u32,
    pub(crate) cancelled: bool,
    /// The pointer in window points.
    pub(crate) window: [f32; 2],
    /// The pointer in desktop pixels, when a native window hosts the view.
    pub(crate) screen: Option<(i32, i32)>,
}

impl Drag {
    /// On `Phase::Up`: the gesture ended without a drop (Escape, the window lost focus, or the
    /// pointer was taken away). Put back whatever the drag moved.
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// The pointer in window points from the window's top-left, whatever scale the element
    /// draws at.
    pub fn window_point(&self) -> [f32; 2] {
        self.window
    }

    /// The pointer in physical desktop pixels, the space `WindowOptions::position` and
    /// `WindowHandle::set_position` take: for opening a window under the pointer, as a tab
    /// dragged out of its strip does. `None` without a native window (headless runs, the web)
    /// and where the system hides window positions (Wayland).
    pub fn screen_point(&self) -> Option<(i32, i32)> {
        self.screen
    }
}

/// What a press on an element does to the window (`El::window_drag`, `El::no_window_drag`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowDrag {
    /// Nothing of its own: a press goes to what's under it.
    #[default]
    None,
    /// A press moves the window, and a double press zooms it.
    Drag,
    /// A press stays here, inside a `Drag` element, and does nothing to the window.
    NoDrag,
}

pub(crate) type PressFn = Rc<dyn Fn(crate::states::States<'_>, &mut Tx)>;
pub(crate) type InputKeyFn = Rc<dyn Fn(crate::states::States<'_>, &crate::KeyEvent, &mut Tx) -> bool>;
pub(crate) type StepFn = Rc<dyn Fn(crate::states::States<'_>, i32, &mut Tx)>;
pub(crate) type StepLimitFn = Rc<dyn Fn(crate::states::States<'_>, bool, &mut Tx)>;

/// A press position in window points, suitable for a context-menu anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPress {
    pub x: f32,
    pub y: f32,
}

/// Where the pointer is, for [`El::on_pointer_move_local`] and
/// [`El::on_secondary_press_local`]: `x` and `y` in the element's own layout points, as
/// [`Drag`] and [`crate::WheelEvent`] give them (from its top-left as drawn, divided by any
/// scale it draws at, so they hold on a zoomed canvas), and the same point in window points
/// for a context menu's anchor.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerEvent {
    pub x: f32,
    pub y: f32,
    pub window_x: f32,
    pub window_y: f32,
}

impl PointerEvent {
    /// The point in window points, as `on_pointer_move` and `on_secondary_press` give it: a
    /// popup or context menu's anchor.
    pub fn window(&self) -> PointerPress {
        PointerPress { x: self.window_x, y: self.window_y }
    }
}

pub(crate) type PointerFn = Rc<dyn Fn(crate::states::States<'_>, PointerEvent, &mut Tx)>;
pub(crate) type DragFn = Rc<dyn Fn(crate::states::States<'_>, &mut Drag, &mut Tx)>;
pub(crate) type DragOutFn = Rc<dyn Fn(crate::states::StatesRef<'_>) -> Option<crate::DragItem>>;
pub(crate) type DragEndFn = Rc<dyn Fn(crate::states::States<'_>, crate::DragOutcome, &mut Tx)>;
pub(crate) type EditFn = Rc<dyn for<'a> Fn(crate::states::States<'a>) -> &'a mut TextEdit>;

pub(crate) use crate::el::a11y::{A11y, Checked};
pub use crate::el::a11y::LiveRegion;

pub(crate) use crate::el::focus::FocusSpec;
pub(crate) use crate::el::handler_set::{Handlers, TouchPolicy};
pub(crate) use crate::el::rare::{ElCold, ElFlags, RoleTag};
pub use crate::el::visual::{Visual, VisualCold};
pub(crate) use crate::el::visual::VisualFlags;

#[derive(Clone)]
pub(crate) struct ModalSpec {
    pub dismiss: PressFn,
    pub outside: bool,
}

/// A vertical scroll viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollSpec {
    /// Follow the end: while scrolled to the bottom, growth keeps the bottom in view, and a new
    /// viewport opens at the bottom on its first frame. When the content is a growth stack, room
    /// the viewport gains at its bottom (a composer shrinking under it) is held under the last
    /// row and closes on the stack's spring once its rows stop growing, so a send only moves
    /// the rows up.
    pub stick_bottom: bool,
}

/// A growth stack (the session timeline): children are rows whose heights ease on `spring`
/// with per-second speed limits; positions derive from the presented heights above. Rows an
/// animated commit puts in a new order slide from where they were drawn into their new places.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrowthSpec {
    pub spring: Spring,
    pub edge: StreamEdge,
}

impl Default for GrowthSpec {
    fn default() -> Self {
        GrowthSpec { spring: GROWTH, edge: edge::FEATHER }
    }
}

/// The selection highlight until [`El::selection_color`] or [`El::caret`] sets one.
pub(crate) const DEFAULT_SELECTION: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.2 };

/// A text field bound to a [`TextEdit`] in app state.
#[derive(Clone)]
pub struct InputSpec {
    /// Text edits are immediate, but dependent layout (e.g. filtered results) may animate.
    pub animate_layout: bool,
    pub edit: TextEdit,
    pub placeholder: String,
    pub placeholder_color: Color,
    pub caret: Color,
    pub selection: Color,
    /// One unwrapped line that scrolls sideways to keep the caret visible, like `<input>`.
    pub single_line: bool,
    /// `placeholder_color`, and `caret` with `selection`, came from [`El::placeholder`] and
    /// [`El::caret`]; the others follow the window's foreground ([`Cx::foreground`](crate::Cx::foreground)).
    pub(crate) placeholder_inked: bool,
    pub(crate) caret_inked: bool,
    pub(crate) revealed_override: Option<bool>,
    pub(crate) decorations: Vec<Span>,
    pub(crate) rich_style: crate::edit::RichStyle,
    pub(crate) pill_icon: Option<PillIconFn>,
    pub(crate) pill_style: Option<PillStyleFn>,
    pub(crate) access: Option<EditFn>,
    pub(crate) submit: Option<PressFn>,
    pub(crate) key: Option<InputKeyFn>,
    pub(crate) change: Option<PressFn>,
    pub(crate) paste: Option<InputPasteFn>,
    /// `El::spellcheck`: the platform's checker marks misspelled words.
    pub(crate) spellcheck: bool,
    /// [`El::selection_style`]: the selection's highlight and ink, by window focus, over `selection`.
    pub(crate) selection_style: Option<SelectionStyle>,
}

impl std::fmt::Debug for InputSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputSpec")
            .field("animate_layout", &self.animate_layout)
            .field("edit", &self.edit)
            .field("placeholder", &self.placeholder)
            .field("placeholder_color", &self.placeholder_color)
            .field("caret", &self.caret)
            .field("selection", &self.selection)
            .field("single_line", &self.single_line)
            .finish_non_exhaustive()
    }
}

/// A CSS-style box shadow. Outer shadows draw behind the element; `inset` ones draw inside its
/// shape, over the fill. A zero-blur, zero-offset outer shadow with a spread is a ring outside
/// the border box (drawn as a ring, so translucent fills don't show it through).
///
/// `blur` is the CSS `box-shadow` blur radius (`blur = 2σ`, so Gaussian standard deviation
/// `σ = blur * 0.5`). `CALayer.shadowRadius` and Sketch / Figma shadow blur specify `r ≈ σ`,
/// so pass `blur = 2.0 * r` to match a `CALayer` shadow radius (whereas [`El::blur`] takes `σ` directly).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Shadow {
    pub x: f32,
    pub y: f32,
    /// Twice the Gaussian's standard deviation, in points (CSS blur radius).
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
    pub inset: bool,
}

/// Radius of [`El::radial_gradient`], in points: a circle (`f32`) or an `[rx, ry]` ellipse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadialRadius(pub [f32; 2]);

impl From<f32> for RadialRadius {
    fn from(r: f32) -> Self {
        Self([r, r])
    }
}

impl From<[f32; 2]> for RadialRadius {
    fn from(r: [f32; 2]) -> Self {
        Self(r)
    }
}

impl From<(f32, f32)> for RadialRadius {
    fn from((rx, ry): (f32, f32)) -> Self {
        Self([rx, ry])
    }
}

/// CSS writes an eased fade as a list of stops: 16 holds the eased gradients Desktop draws.
pub(crate) const MAX_GRADIENT_STOPS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GradientGeometry {
    Radial { center: [f32; 2], radius: [f32; 2] },
    Linear { angle: f32 },
}

/// A radial or angled linear gradient painted over an element's fill.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RadialGradient {
    pub(crate) geometry: GradientGeometry,
    /// Sorted `(t, color)` stops in `0.0..=1.0`, capped at [`MAX_GRADIENT_STOPS`].
    pub(crate) stops: Vec<(f32, Color)>,
}

fn normalize_gradient_stops(geometry: GradientGeometry, raw: &[(f32, Color)]) -> Option<RadialGradient> {
    let mut stops: Vec<(f32, Color)> = raw
        .iter()
        .copied()
        .filter(|(t, _)| t.is_finite())
        .map(|(t, c)| (t.clamp(0.0, 1.0), c))
        .collect();
    if stops.is_empty() {
        return None;
    }
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    stops.truncate(MAX_GRADIENT_STOPS);
    if stops.len() == 1 {
        let c = stops[0].1;
        stops = vec![(0.0, c), (1.0, c)];
    }
    Some(RadialGradient { geometry, stops })
}

impl Shadow {
    /// A CALayer shadow: `shadowRadius` `radius`, `shadowOffset` (`x`, `y`) with y down as in a
    /// flipped view, and `shadowColor` at `shadowOpacity` folded into `color`. Core Animation's
    /// blur has a standard deviation of about `shadowRadius` (0.97 to 1.02 times it, measured
    /// from 1.5 to 32 pt), so `blur` is twice the radius. Corners stay circular, as with a
    /// `CGPath(roundedRect:)` shadow path. With `srgb_blend(true)` it also composites as one.
    pub fn core_animation(x: f32, y: f32, radius: f32, color: Color) -> Shadow {
        Shadow { x, y, blur: 2.0 * radius, color, ..Shadow::default() }
    }
}

/// A ring drawn `offset` points outside the border box, `width` points wide (CSS `outline`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    pub width: f32,
    pub offset: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

pub use crate::el::anchor::{Anchor, Side, anchor_rect};
pub use crate::el::direction::Direction;
pub(crate) use crate::el::direction::{DirEffect, DirEvent, Flow, Pin, mirror_anchor, mirror_visual, step as step_direction};
pub(crate) use crate::el::scroll::FadeStyle;
pub use crate::el::scroll::ScrollFade;
pub use crate::el::selection_style::SelectionStyle;
pub use crate::el::text_spec::TextSpec;
pub(crate) use crate::el::text_spec::Elide;

/// [`El::on_link`]'s handler.
#[derive(Clone)]
pub(crate) struct LinkFn(pub LinkHandler);
pub(crate) type LinkHandler = Rc<dyn Fn(crate::states::States<'_>, &str, &mut Tx)>;

impl std::fmt::Debug for LinkFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LinkFn")
    }
}
/// [`El::on_link_secondary`]'s handler.
#[derive(Clone)]
pub(crate) struct LinkSecondaryFn(pub Rc<dyn Fn(crate::states::States<'_>, &str, PointerPress, &mut Tx)>);
impl std::fmt::Debug for LinkSecondaryFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("LinkSecondaryFn") }
}

pub use rune_motion::Cycle;

/// Default `truncate_fade` edge, in ems.
const TRUNCATE_FADE_EM: f32 = 1.5;

/// A root paste handler; `rich` is true only for [`El::on_paste_content`], so a plain
/// [`El::on_paste`] never asks the OS clipboard for HTML or RTF.
#[derive(Clone)]
pub(crate) struct PasteFn {
    pub(crate) rich: bool,
    pub(crate) call: InputPasteFn,
}
/// A pill's leading icon by its id; see [`El::pill_icon`].
pub(crate) type PillIconFn = Rc<dyn Fn(&str) -> Option<&'static crate::Icon>>;
/// A pill's look by its id; see [`El::pill_style`].
pub(crate) type PillStyleFn = Rc<dyn Fn(&str) -> PillStyle>;
pub use pills::InlineIcon;

/// How one [`Mark::Pill`](crate::Mark::Pill) chip looks: any part left unset falls back to
/// [`El::pill_colors`] and [`El::pill_icon`], then the field's defaults. Build one with
/// `PillStyle::default().bg(..).icon(..)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PillStyle {
    pub(crate) bg: Option<Color>,
    pub(crate) fg: Option<Color>,
    pub(crate) icon: Option<&'static crate::Icon>,
}

impl PillStyle {
    pub const fn new() -> Self {
        Self { bg: None, fg: None, icon: None }
    }
    /// The chip's colour.
    pub const fn bg(mut self, bg: Color) -> Self {
        self.bg = Some(bg);
        self
    }
    /// The label and icon colour.
    pub const fn fg(mut self, fg: Color) -> Self {
        self.fg = Some(fg);
        self
    }
    /// The leading icon, in the label's colour.
    pub const fn icon(mut self, icon: &'static crate::Icon) -> Self {
        self.icon = Some(icon);
        self
    }
}
pub(crate) type InputPasteFn = Rc<dyn Fn(crate::states::States<'_>, &crate::ClipboardContent, &mut Tx) -> bool>;
pub(crate) type PickFn = Box<dyn FnOnce(crate::states::States<'_>, Option<crate::Picked>, &mut Tx)>;

#[derive(Clone)]
pub(crate) struct PickRead {
    pub within: Option<String>,
    pub x: f32,
    pub y: f32,
    pub pick: crate::Pick,
    pub(crate) f: Rc<std::cell::Cell<Option<PickFn>>>,
}
impl PickRead {
    pub fn take(&self) -> Option<PickFn> { self.f.take() }
}
impl std::fmt::Debug for PickRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("PickRead") }
}

/// What an `El::selection_copy` element copies for a selection inside it.
pub(crate) type SelectionCopy = Rc<dyn Fn(&crate::TextSelection) -> Option<String>>;
/// `El::on_selection_change`.
pub(crate) type SelectionChangeFn = Rc<dyn Fn(crate::states::States<'_>, &crate::SelectionChange, &mut Tx)>;

/// A scroll viewport's content-aware edges: `El::scroll_edge_fade`, `El::scroll_margin` and
/// `El::scroll_anchor`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScrollEdges {
    /// Fade width (points) for the top and bottom edges; 0 for none.
    pub(crate) fade: f32,
    /// How the fade follows the content hidden under each edge (`El::scroll_fade`).
    pub(crate) fade_style: scroll::FadeStyle,
    /// Space `Tx::scroll_into_view` leaves between a revealed row and the viewport's edge.
    pub(crate) margin: f32,
    /// The viewport keeps its top row where it is when content above it changes.
    pub(crate) anchor: bool,
}

/// The axes a scroll viewport moves on: `El::scroll` (vertical), `El::scroll_x`, `El::scroll_xy`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScrollAxes {
    #[default]
    Y,
    X,
    Both,
}

impl ScrollAxes {
    pub(crate) fn x(self) -> bool { self != ScrollAxes::Y }
    pub(crate) fn y(self) -> bool { self != ScrollAxes::X }
    /// Whether it moves on the horizontal axis (`true`) or the vertical one.
    pub(crate) fn has(self, horizontal: bool) -> bool { if horizontal { self.x() } else { self.y() } }
}

pub(crate) type AtBottomFn = Rc<dyn Fn(crate::states::States<'_>, bool)>;
pub(crate) type VisibleRowsFn = Rc<dyn Fn(crate::states::States<'_>, std::ops::Range<usize>) -> bool>;
pub(crate) type LayoutFn = Rc<dyn Fn(crate::states::States<'_>, [f32; 4])>;
pub(crate) type LayoutTxFn = Rc<dyn Fn(crate::states::States<'_>, [f32; 4], &mut Tx)>;
pub(crate) type ScrollOffsetFn = Rc<dyn Fn(crate::ScrollPos)>;
pub(crate) type WindowFocusFn = Rc<dyn Fn(crate::states::States<'_>, bool, &mut Tx)>;
pub(crate) type LanguagesFn = Rc<dyn Fn(crate::states::States<'_>, Vec<String>, &mut Tx)>;

/// An element: layout, paint, children and handlers. Hot fields sit inline in three cache lines;
/// the rest wait behind `cold`, allocated on the first write.
#[derive(Clone)]
pub struct El {
    pub(crate) style: CompactStyle,
    pub(crate) vis: Visual,
    pub(crate) children: Vec<El>,
    pub(crate) key: Option<String>,
    pub(crate) text: Option<crate::cold::SharedBox<TextSpec>>,
    pub(crate) handlers: crate::cold::Cold<Handlers>,
    /// The cached [`memo`](crate::memo) subtree this placeholder came from. Its children come
    /// before `children`; once the placeholder is expanded or given extra children,
    /// `ElFlags::MEMO_EXTENDED` says it is no longer that subtree itself.
    pub(crate) memo: Option<Rc<crate::memo::Built>>,
    pub(crate) cold: crate::cold::Cold<ElCold>,
    pub(crate) scale: f32,
    pub(crate) flags: ElFlags,
    pub(crate) role: RoleTag,
    pub(crate) vis_flags: VisualFlags,
    #[cfg(debug_assertions)]
    pub(crate) key_source: Option<&'static std::panic::Location<'static>>,
}

#[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
const _: () = assert!(std::mem::size_of::<El>() == 192);
#[cfg(all(target_pointer_width = "64", not(debug_assertions)))]
const _: () = assert!(std::mem::size_of::<Option<El>>() == 192);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Visual>() == 40);

#[inline]
fn base(dir: FlexDirection) -> El {
    El {
        // Relative makes every element the containing block for its absolute children, so they
        // size and pin against their parent's padding box. Taffy's default (static) hands them
        // to the window instead, as CSS does.
        style: CompactStyle::rune_base(dir),
        vis: Visual::default(),
        children: Vec::new(),
        key: None,
        text: None,
        handlers: crate::cold::Cold::NONE,
        memo: None,
        cold: crate::cold::Cold::NONE,
        scale: 1.0,
        flags: ElFlags::default(),
        role: RoleTag::GROUP,
        vis_flags: VisualFlags::default(),
        #[cfg(debug_assertions)]
        key_source: None,
    }
}

#[inline]
pub fn row() -> El {
    base(FlexDirection::Row)
}

#[inline]
pub fn col() -> El {
    base(FlexDirection::Column)
}

/// A plain box.
#[inline]
pub fn el() -> El {
    base(FlexDirection::Row)
}

/// Takes the free space along its parent's main axis.
#[inline]
pub fn spacer() -> El {
    base(FlexDirection::Row).grow(1.0)
}

/// Text with styled byte ranges; wraps at its width.
pub fn rich(content: impl Into<String>, spans: Vec<Span>) -> El {
    let mut e = text(content);
    if let Some(t) = &mut e.text {
        t.spans = if spans.len() <= 1 || spans.windows(2).all(|w| w[0].start < w[0].end && w[0].end <= w[1].start) {
            spans
        } else {
            crate::text::flatten_spans(&spans)
        };
        t.wrap = true;
    }
    e
}

/// Parts a help text from each of its [`El::help_keys`] keycaps.
pub(crate) const HELP_KEY: char = '\u{1f}';

/// A help text as words: its keycaps after it, a space apart.
pub(crate) fn help_words(help: &str) -> std::borrow::Cow<'_, str> {
    if help.contains(HELP_KEY) { help.replace(HELP_KEY, " ").into() } else { help.into() }
}

/// An SVG icon from a catalog, `size` points square (or what [`crate::text::set_icon_swap_size`]
/// gives a swapped icon), tinted with `.color()`.
pub fn icon(i: &'static Icon, size: f32) -> El {
    let size = crate::text::icon::swapped_size(i, size);
    let mut e = base(FlexDirection::Row).size(size, size);
    e.role = RoleTag::ICON;
    e.vis.cold_mut().icon = Some((i, Color::WHITE));
    e
}

/// An element that paints `shape`; size it like any element.
pub fn canvas(shape: Shape) -> El {
    let mut e = base(FlexDirection::Row).shape(shape);
    e.role = RoleTag::IMAGE;
    e
}

/// A text field showing `edit`; route edits back with `.on_edit(|s| &mut s.field)`.
///
/// Multi-line by default (see [`El::single_line`]). Plain Return in a paragraph submits (calling
/// [`El::on_submit`], or pressing the [`El::default_button`] when none is set) while Shift+Return
/// and Option+Return insert a line break; a document editor that wants plain Return to break
/// paragraphs attaches `.on_submit(|s, _| s.field.insert("\n"))`.
pub fn text_input(edit: &TextEdit, style: TextStyle) -> El {
    // An app may have assigned `edit.text` directly: clamp stale marks, blocks and caret to it.
    let mut edit = edit.clone();
    edit.set_single_line(false);
    edit.sync_invariants();
    let (content, _) = edit.display();
    let mut e = text(content).font(style);
    e.role = RoleTag::TEXTBOX;
    e.cold_mut().a11y.secure = edit.is_secure();
    if let Some(t) = &mut e.text {
        t.wrap = true;
        t.wrap_anywhere = edit.is_secure();
        t.redacted = edit.is_secure();
    }
    e.cold_mut().input = Some(Box::new(InputSpec {
        animate_layout: false,
        edit,
        placeholder: String::new(),
        placeholder_color: Color::WHITE.alpha(0.4),
        caret: Color::WHITE,
        selection: DEFAULT_SELECTION,
        single_line: false,
        placeholder_inked: false,
        caret_inked: false,
        revealed_override: None,
        decorations: Vec::new(),
        rich_style: crate::edit::RichStyle::default(),
        pill_icon: None,
        pill_style: None,
        access: None,
        submit: None,
        key: None,
        change: None,
        paste: None,
        spellcheck: false,
        selection_style: None,
    }));
    e.sync_input_pills();
    e.sync_input_rich();
    e
}

/// Fits a multi-line [`text_input`] to its text: `min_lines` tall at least, growing with each
/// wrapped line up to `max_lines`, then scrolling inside with the caret kept in view. The height
/// springs on the element's motion. Returns `field` inside a scroll viewport keyed `"scroll"`, so
/// key the field and attach its handlers first. A smaller `.max_h(..)` caps it in points.
#[cfg_attr(debug_assertions, track_caller)]
pub fn auto_grow(field: El, min_lines: usize, max_lines: usize) -> El {
    let line = field.text.as_ref().map_or(0.0, |t| t.style.line_px());
    let (min, max) = (min_lines.max(1), max_lines.max(min_lines.max(1)));
    col().key("scroll").w_full().shrink().min_h(min as f32 * line).max_h(max as f32 * line)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(field.animate_edit_layout())
}

impl El {
    /// On the root, receive native window activation/deactivation (not text-field focus).
    /// Also works on interactive overlays. Headless hosts may call Runtime::window_focus.
    pub fn on_window_focus<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.on_window_focus = Some(Rc::new(move |s, focused, tx| f(downcast::<S>(s), focused, tx)));
        self
    }
    /// On the root or around text fields: the app's spell checking setting, `on`, which the
    /// fields' menus show as a Spellcheck item with a check mark, as a browser's do. Picking it
    /// calls `f` with the setting flipped; the app keeps it and passes it to its fields'
    /// [`El::spellcheck`].
    pub fn on_spellcheck<S: 'static>(mut self, on: bool, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.on_spellcheck = Some((on, Rc::new(move |s, on, tx| f(downcast::<S>(s), on, tx))));
        self
    }
    /// Beside [`El::on_spellcheck`], where the platform has dictionaries to choose
    /// ([`crate::spell::available_languages`]): the fields' menus add a Spellcheck Languages
    /// submenu, as Chromium's do off macOS. Picking a dictionary checks with the new choice at
    /// once ([`crate::spell::set_languages`]) and calls `f` with it, for the app to keep and
    /// pass to `set_languages` at its next launch.
    pub fn on_spellcheck_languages<S: 'static>(mut self, f: impl Fn(&mut S, Vec<String>, &mut Tx) + 'static) -> Self {
        self.handlers.on_spellcheck_languages = Some(Rc::new(move |s, tags, tx| f(downcast::<S>(s), tags, tx)));
        self
    }
    /// Paint the latest text in this element's fixed layout box. `text("00:00")` reserves
    /// that placeholder's size; explicit `.size(w, h)` also works. Updates never resize the
    /// box or reflow siblings, and overflow is clipped. Plain single-line text only.
    pub fn live_text(mut self, value: &crate::LiveText) -> Self {
        assert!(self.cold.input.is_none(), "live_text cannot bind a text editor");
        let spec = self.text.as_mut().expect("live_text requires a text element");
        spec.live = Some(value.clone());
        self
    }
    /// On the root (or modal root), handle paste when no text field is focused.
    /// Native clipboard reads are asynchronous. Script `paste` uses the same handler.
    /// It hears the plain text; [`El::on_paste_content`] hears every form.
    pub fn on_paste<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.handlers.on_paste = Some(PasteFn {
            rich: false,
            call: Rc::new(move |s, content, tx| {
                content.has_plain() && { f(downcast::<S>(s), content.text(), tx); true }
            }),
        });
        self
    }
    /// [`El::on_paste`] with everything the clipboard held: plain text and, when the source
    /// offered them, sanitised HTML and RTF. Replaces an `on_paste` on the same element.
    pub fn on_paste_content<S: 'static>(mut self, f: impl Fn(&mut S, &crate::ClipboardContent, &mut Tx) + 'static) -> Self {
        self.handlers.on_paste = Some(PasteFn {
            rich: true,
            call: Rc::new(move |s, content, tx| { f(downcast::<S>(s), content, tx); true }),
        });
        self
    }
    /// Mask a text element or text input before layout; disable undo, copy, cut, selection and
    /// IME composition, omit accessibility text, redact [`Debug`], bypass layout/raster caches,
    /// and zeroize owned buffers on conceal and drop.
    ///
    /// Non-empty read-only [`text`] and [`rich`] labels show a fixed twelve-bullet mask on
    /// one line, hiding their length as well as their content. Explicit labels and help must
    /// describe the field, not contain its secret value. Links and rolling text are disabled.
    /// Pair with [`revealed`](Self::revealed) to toggle plaintext display.
    pub fn secure(mut self) -> Self {
        self.cold_mut().a11y.secure = true;
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.edit.set_secure(true);
            let single = input.single_line;
            if let Some(text) = &mut self.text {
                text.replace_content(input.edit.display().0);
                text.redacted = true;
                if !single {
                    text.wrap_anywhere = true;
                }
            }
            self.sync_input_pills();
            self.sync_input_rich();
        } else if let Some(text) = &mut self.text {
            text.redacted = true;
            if text.selectable {
                text.selectable = false;
                if self.handlers.cursor == Some(Cursor::Text) {
                    self.handlers.cursor = None;
                }
            }
        }
        self
    }

    /// Show the plain characters of a [`secure`](Self::secure) text element or text input while
    /// keeping all other secure protections (no undo, copy, cut, selection, IME or accessibility
    /// text, redacted [`Debug`], uncached shaping/raster, and zeroized buffers). Prefer
    /// [`TextEdit::revealed`] or [`TextEdit::set_revealed`] when an input model owns the state.
    pub fn revealed(mut self, revealed: bool) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.edit.set_revealed(revealed);
            input.revealed_override = Some(revealed);
            if let Some(text) = &mut self.text {
                text.replace_content(input.edit.display().0);
            }
            self.sync_input_pills();
            self.sync_input_rich();
        } else if let Some(text) = &mut self.text {
            text.secure_revealed = revealed;
        }
        self
    }

    /// A stable motion-lint name, independent of keys and generations. Defaults to the NodeId
    /// in MotionTrace; tooling retains its keyed path fallback. Stored without copying the name.
    /// Give simultaneous generations distinct names when comparing their motion separately.
    pub fn trace_name(mut self, name: &'static str) -> Self {
        self.cold_mut().trace_name = Some(name);
        self
    }

    /// Class and host for this subtree's trace metadata (descendants may override them).
    /// motion-lint pairs names by class too: use `("gpui", "pill")` to match a native Metal trace.
    /// The labels share the element's lazily allocated storage with its other rare fields.
    pub fn trace_namespace(mut self, class: &'static str, host: &'static str) -> Self {
        self.cold_mut().trace_namespace = Some((class, host));
        self
    }

    /// Bind opacity directly to a latest-wins value, without rebuilding or laying out the view.
    pub fn live_opacity(mut self, value: impl Into<crate::live::Binding<f32>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Opacity)); self
    }

    /// Bind a subtree's Gaussian blur (sigma in points), optionally smoothed with `Live::smooth`.
    pub fn live_blur(mut self, value: impl Into<crate::live::Binding<f32>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Blur));
        self
    }
    /// Bind an additive parent-space offset in points. Descendants and hit tests follow it.
    pub fn live_offset(mut self, value: impl Into<crate::live::Binding<[f32; 2]>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Offset)); self
    }
    /// Bind uniform scale around the node's centre; layout remains at its target size. Text,
    /// icons and images in the subtree scale with it, as under [`El::scale`]. Text rasterizes
    /// at the scale rounded to 1/32, so a dragged value doesn't fill the glyph atlas.
    pub fn live_scale(mut self, value: impl Into<crate::live::Binding<f32>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Scale)); self
    }
    /// Bind the [start, end] span of a shape's strokes that draws, as fractions (0 to 1) of
    /// their combined length, as [`Shape::trimmed`](crate::Shape::trimmed) cuts them. A
    /// [`Tween`](crate::Tween) runs a countdown ring or a travelling dash without rebuilding.
    pub fn live_trim(mut self, value: impl Into<crate::live::Binding<[f32; 2]>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Trim)); self
    }
    pub fn live_fill(mut self, value: impl Into<crate::live::Binding<Color>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Fill)); self
    }
    /// Bind presentation [x, y, width, height] in parent points. Does not reflow siblings or
    /// rewrap text; use ordinary layout for those changes. Width/height clamp at zero.
    pub fn live_rect(mut self, value: impl Into<crate::live::Binding<[f32; 4]>>) -> Self {
        self.bind_live(crate::live::Bound::new(value, crate::live::Property::Rect)); self
    }
    fn bind_live(&mut self, bound: crate::live::Bound) {
        if let Some(old) = self.cold_mut().live.iter_mut().find(|b| b.property == bound.property) { *old = bound; }
        else { self.cold_mut().live.push(bound); }
    }
    /// Called after a focused field's text changes, with the updated model available.
    pub fn on_input_change<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.change = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        }
        self
    }
    /// Handle a paste into this focused text field before the editor, with every form the
    /// clipboard held ([`ClipboardContent`](crate::ClipboardContent): plain text, and sanitised
    /// HTML and RTF when the source offered them). Return true when the app took it; false lets
    /// the editor paste the plain text as usual. Without it a field pastes plain text only.
    ///
    /// ```ignore
    /// text_input(&s.draft).on_input_paste(|s: &mut Chat, content, _| match content.html() {
    ///     Some(html) => { s.draft_rich.paste_html(html, content.text()); true }
    ///     None => false,
    /// })
    /// ```
    pub fn on_input_paste<S: 'static>(mut self, f: impl Fn(&mut S, &crate::ClipboardContent, &mut Tx) -> bool + 'static) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.paste = Some(Rc::new(move |s, content, tx| f(downcast::<S>(s), content, tx)));
        }
        self
    }
    /// Handle a focused text field's key before the editor. Return true to consume it.
    pub fn on_input_key<S: 'static>(mut self, f: impl Fn(&mut S, &crate::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        if let Some(input) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            input.key = Some(Rc::new(move |s, key, tx| f(downcast::<S>(s), key, tx)));
        }
        self
    }
    /// Keys while this element, or a control inside it, has focus: `f` sees each key before
    /// focus navigation (arrows in a group, Return and Space presses), the default button and
    /// the window's `on_key`, and returns true to consume it. Keys typed into a focused text
    /// field stay the field's. Nested handlers run innermost first.
    ///
    /// With `focusable(true)`, this is a control that owns its keyboard, like a virtualised
    /// list whose rows aren't all built: it keeps focus while its rows scroll in and out, and a
    /// click anywhere inside it focuses it (an NSTableView).
    pub fn on_key<S: 'static>(mut self, f: impl Fn(&mut S, &crate::KeyEvent, &mut Tx) -> bool + 'static) -> Self {
        self.cold_mut().focus.key = Some(Rc::new(move |s, key, tx| f(downcast::<S>(s), key, tx)));
        self
    }
    /// Secondary release inside the captured element (right-click or macOS control-click), at a
    /// point in window points: a context menu's anchor. [`El::on_secondary_press_local`] gives
    /// the element's own points as well. The Menu key and Shift+F10 call it too, while this
    /// element or one inside it is focused and no key handler took the key, at the focused
    /// element's bottom-left.
    pub fn on_secondary_press<S: 'static>(mut self, f: impl Fn(&mut S, PointerPress, &mut Tx) + 'static) -> Self {
        self.handlers.on_secondary_press = Some(Rc::new(move |s, p, tx| {
            f(downcast::<S>(s), p.window(), tx);
        }));
        self
    }
    /// [`El::on_secondary_press`] with the point in this element's own layout points too, as
    /// [`Drag`] gives them, so a canvas finds what was right-clicked at any zoom. Open the menu
    /// at [`PointerEvent::window`].
    ///
    /// ```ignore
    /// .on_secondary_press_local(|s: &mut Canvas, p, tx| {
    ///     s.menu = Some((s.shape_at(p.x, p.y), p.window()));
    /// })
    /// ```
    pub fn on_secondary_press_local<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.handlers.on_secondary_press = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        self
    }

    pub(crate) fn key_location(&self) -> String {
        #[cfg(debug_assertions)]
        if let Some(source) = self.key_source { return source.to_string(); }
        "unknown (build with debug assertions for source locations)".into()
    }

    /// Paint the newest video frame inside this node. Node dimensions are in points.
    pub fn native_surface(mut self, surface: &crate::NativeSurface, fit: crate::SurfaceFit) -> Self {
        self.vis.cold_mut().surface = Some((surface.clone(), fit));
        self
    }
    /// Which edges a surface or [`pixels_fit`](Self::pixels_fit) image sticks to inside this
    /// node, as (horizontal, vertical): `Start` is left and top, `End` is right and bottom. It
    /// positions the spare space `Fit` leaves and the overflow `Fill` crops; the default is
    /// `Center` on both axes. Applies to [`native_surface`](Self::native_surface),
    /// [`video`](Self::video) and `pixels_fit`.
    ///
    /// ```
    /// # use rune::{Align, SurfaceFit, el};
    /// # let stream = rune::NativeSurface::new();
    /// // A streamed page that keeps its top-left corner while the panel is resized.
    /// let view = el().native_surface(&stream, SurfaceFit::Fill).surface_anchor(Align::Start, Align::Start);
    /// ```
    pub fn surface_anchor(mut self, horizontal: Align, vertical: Align) -> Self {
        self.vis.cold_mut().surface_place.anchor = [horizontal, vertical];
        self
    }
    /// Draw a surface or `pixels_fit` image at one texel per device pixel instead of scaling it
    /// to the node, so `SurfaceFit` no longer applies. A node bigger than the image leaves the
    /// rest empty; a smaller one crops it. Combine with
    /// [`surface_anchor`](Self::surface_anchor) to choose which edges stay put. It suits a
    /// stream rendered at the window's scale, whose last frame should hold still, not stretch,
    /// until the next one arrives at the node's new size.
    ///
    /// ```
    /// # use rune::{Align, SurfaceFit, el};
    /// # let stream = rune::NativeSurface::new();
    /// let view = el()
    ///     .native_surface(&stream, SurfaceFit::Fill)
    ///     .surface_anchor(Align::Start, Align::Start)
    ///     .surface_unscaled(true);
    /// ```
    pub fn surface_unscaled(mut self, unscaled: bool) -> Self {
        self.vis.cold_mut().surface_place.unscaled = unscaled;
        self
    }
    /// Play `video` inside this node: `Fit` letterboxes, `Fill` crops. Frames come from the
    /// display link while it plays; a paused video draws no frames of its own. Audio players
    /// need no element.
    pub fn video(self, video: &crate::Player, fit: crate::SurfaceFit) -> Self {
        self.native_surface(video.surface(), fit)
    }
    /// Opt into keyboard and pointer focus. Disabled controls must set this false, which also
    /// keeps a pressable control out of the Tab order under [`Cx::web_focus`](crate::Cx::web_focus).
    pub fn focusable(mut self, enabled: bool) -> Self {
        // Recorded even on a bare element: under `Cx::web_focus` it keeps a pressable control
        // out of the Tab order.
        let focus = &mut self.cold_mut().focus;
        focus.enabled = enabled;
        focus.opt_out = !enabled;
        self
    }
    /// Positive overrides precede document order (0); negative values skip Tab only.
    pub fn tab_index(mut self, order: i32) -> Self {
        if order != 0 || !self.cold.focus.is_empty() {
            self.cold_mut().focus.order = order;
        }
        self
    }
    /// Arrow keys rove among focusable descendants; Tab visits only the current/selected item.
    /// Groups with role `menu`, `listbox`, `list` or `tree` move on Up and Down only; other
    /// groups (segments, tabs, radios, toolbars) move on Left and Right as well. Home and End
    /// jump to the ends. `list`, `tree` and `grid` stop at their ends, as NSTableView does;
    /// menus, popups and segments wrap.
    pub fn focus_group(mut self) -> Self {
        self.cold_mut().focus.group = true;
        self
    }
    pub fn focus_selected(mut self, selected: bool) -> Self {
        if selected || !self.cold.focus.is_empty() {
            self.cold_mut().focus.selected = selected;
        }
        self
    }
    /// On a virtualized stack, reveal its selected row when the selected key changes,
    /// even while another control owns keyboard focus. Frames dispatching `on_hover`
    /// do not scroll, so hover-driven selection does not move the row under the pointer.
    /// Off by default; useful for suggestions controlled by an outside text field.
    pub fn reveal_selected(mut self, enabled: bool) -> Self {
        if enabled || !self.cold.focus.is_empty() {
            self.cold_mut().focus.reveal_selected = enabled;
        }
        self
    }
    /// A press on this element or its unfocusable descendants leaves the current field
    /// focused instead of blurring it. Works on any element, not only popups: a list row, a
    /// button, a toolbar. The press still reaches `on_press` and the other handlers, and a
    /// scroll still cancels it as usual. Focusable descendants still take focus normally.
    pub fn preserve_focus_on_press(mut self, enabled: bool) -> Self {
        if enabled || !self.cold.focus.is_empty() {
            self.cold_mut().focus.preserve_on_press.focus = enabled;
        }
        self
    }
    /// On a focus group: arrow keys and type-select also press the item they focus, so
    /// selection follows focus (a source list). Radio and tab items always do.
    pub fn focus_follows(mut self) -> Self {
        self.cold_mut().focus.follows = true;
        self
    }
    /// A menu or list row's highlight: painted while focused from the keyboard or pointer,
    /// and moving the pointer over the row focuses it, so keys and mouse share one
    /// highlight (NSMenu). Use instead of `hover` and `focus_ring`.
    pub fn focus_highlight(mut self, color: Color) -> Self {
        self.cold_mut().focus.highlight = Some(color);
        self
    }
    /// A fill laid over this element while it shows keyboard focus, as CSS
    /// `:focus-visible { background }` paints one: a list row that Tab reaches. Unlike
    /// [`El::focus_highlight`] the pointer neither paints it nor moves focus. A row that shows
    /// focus by its fill alone also sets [`El::no_focus_ring`].
    pub fn focus_fill(mut self, color: Color) -> Self {
        self.cold_mut().focus.fill = Some(color);
        self
    }
    /// The window's default button, which Return presses (AppKit's `keyEquivalent = "\r"`).
    /// Return goes to it from anywhere in the window, or the open modal, unless the focused
    /// element takes Return itself: a text field with `on_submit`, a link, or a row of a
    /// `menu`, `listbox`, `list`, `tree` or `grid` group or a `focus_highlight` row, which
    /// Return presses. With several, the last one shown wins (a
    /// sheet's over its window's). It needs an `on_press`, and stays out of reach while hidden
    /// or behind a modal. A focused button presses on Space, and passes Return on to this.
    pub fn default_button(mut self) -> Self {
        self.cold_mut().focus.default = true;
        self
    }
    /// A slider's keyboard, as NSSlider's: while this element is focused, Left and Down call
    /// `f` with -1, Right and Up with +1, Page Down with -10 and Page Up with +10, repeating
    /// while held. `f` moves the value by that many of its steps and clamps it. The element
    /// becomes focusable, and its role `slider` unless it has another. Home and End go to the
    /// ends with [`El::on_step_limit`].
    pub fn on_step<S: 'static>(mut self, f: impl Fn(&mut S, i32, &mut Tx) + 'static) -> Self {
        self.cold_mut().focus.step = Some(Rc::new(move |s, delta, tx| f(downcast::<S>(s), delta, tx)));
        self.cold_mut().focus.enabled = true;
        if self.role == RoleTag::GROUP {
            self.role = RoleTag::SLIDER;
        }
        self
    }
    /// A slider's Home and End (see [`El::on_step`]): `f` gets `false` for Home, which goes to
    /// the minimum, and `true` for End, the maximum. Without it they scroll, as elsewhere.
    pub fn on_step_limit<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.cold_mut().focus.step_limit = Some(Rc::new(move |s, max, tx| f(downcast::<S>(s), max, tx)));
        self
    }
    /// Return focus to this keyed trigger (or its first focusable child) when removed.
    pub fn focus_return(mut self, trigger: impl Into<String>) -> Self {
        self.cold_mut().focus.return_to = Some(trigger.into());
        self
    }
    /// The control a [`El::modal`] focuses when it opens, as HTML's `autofocus` attribute in a
    /// dialog: a Cancel button, say. Without one the modal focuses its first text field, else
    /// its first Tab stop, or itself under [`Cx::web_focus`](crate::Cx::web_focus).
    pub fn autofocus(mut self) -> Self {
        self.cold_mut().focus.autofocus = true;
        self
    }
    /// Keyboard-only ring, on this element or a visual child of the focused control. A
    /// control with no ring of its own draws the default one (see [`El::no_focus_ring`]).
    pub fn focus_ring(mut self, width: f32, offset: f32, color: Color) -> Self {
        self.cold_mut().focus.ring = Some(Outline { width, offset, color });
        self.cold_mut().focus.within = false;
        self.cold_mut().focus.ring_off = false;
        self
    }
    /// A ring drawn while focus is on this element or anywhere inside it, whether the pointer or
    /// the keyboard put it there, as CSS `:focus-within` draws a text field's box: e.g.
    /// `focus_within_ring(1.0, -1.0, c)` for `outline: 1px solid; outline-offset: -1px`, a ring
    /// just inside the border box. It isn't a Tab stop by itself.
    pub fn focus_within_ring(mut self, width: f32, offset: f32, color: Color) -> Self {
        self.cold_mut().focus.ring = Some(Outline { width, offset, color });
        self.cold_mut().focus.within = true;
        self.cold_mut().focus.ring_off = false;
        self
    }
    /// While focus is on this element or anywhere inside it, from the pointer or the keyboard,
    /// it draws none of its [`El::box_shadow`]s and no [`El::hover`] fill, as a CSS field whose
    /// `:focus-within` rule sets `box-shadow: none` and whose hover rule is
    /// `:not(:focus-within)`: the [`El::focus_within_ring`] stands in for its raised edge.
    pub fn focus_within_flat(mut self) -> Self {
        self.cold_mut().focus.flat_within = true;
        self
    }
    /// No focus ring: not the default one, and not one set with [`El::focus_ring`]. A control
    /// that shows focus its own way (a fill, an outline it animates) sets it on itself or on
    /// the child that would draw the ring.
    ///
    /// Without it, a `focusable` element focused from the keyboard draws the default ring when
    /// neither it nor a child (short of another focusable) sets `focus_ring`: the view's
    /// [`Cx::focus_ring`](crate::Cx::focus_ring), else AppKit's, 3 points wide in
    /// [`Cx::focus_color`](crate::Cx::focus_color), just outside its border box, following
    /// its corner radii. Text fields, focused links, `focus_highlight` rows and the items of a `menu`,
    /// `listbox`, `list`, `tree` or `grid` group draw none, as in AppKit: their caret,
    /// highlight or selection shows focus.
    pub fn no_focus_ring(mut self) -> Self {
        self.cold_mut().focus.ring = None;
        self.cold_mut().focus.within = false;
        self.cold_mut().focus.ring_off = true;
        self
    }
    /// Trap focus and make the background inert while mounted. Escape requests dismissal.
    pub fn modal<S: 'static>(mut self, click_outside: bool, dismiss: impl Fn(&mut S) + 'static) -> Self {
        self.handlers.modal = Some(ModalSpec { outside: click_outside,
            dismiss: Rc::new(move |s, _| dismiss(downcast::<S>(s))) });
        self
    }
    /// [`El::modal`] whose dismissal gets the change's [`Tx`]: a dialog that closes at once
    /// (`tx.instant()`), say.
    pub fn modal_tx<S: 'static>(mut self, click_outside: bool, dismiss: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.handlers.modal = Some(ModalSpec { outside: click_outside,
            dismiss: Rc::new(move |s, tx| dismiss(downcast::<S>(s), tx)) });
        self
    }
    pub fn modal_click_outside(mut self, enabled: bool) -> Self {
        if let Some(h) = self.handlers.get_mut_if_some() && let Some(m) = &mut h.modal { m.outside = enabled; }
        self
    }
    /// A finite opacity/scale sequence on the shared render clock. The final pose holds;
    /// reduced motion and explicit settle land it immediately. A new key replays it.
    pub fn sequence(self, frames: rune_motion::Sequence) -> Self {
        self.looping(rune_motion::LoopSpec::sequence(frames))
    }

    /// A renderer-owned periodic or finite presentation track, on the shared clock.
    pub fn looping(mut self, spec: rune_motion::LoopSpec) -> Self {
        self.cold_mut().looping = Some(Box::new(spec));
        self
    }

    /// Identity among siblings. Siblings need unique keys: when two share one, the first keeps
    /// it, each later one is renamed `key#dup1`, `key#dup2`, ... and Rune prints a warning naming
    /// both call sites once per key.
    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.set_key(key);
        self
    }
    /// [`El::key`] in place.
    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    pub fn set_key(&mut self, key: impl Into<String>) -> &mut Self {
        self.key = Some(key.into());
        #[cfg(debug_assertions)]
        { self.key_source = Some(std::panic::Location::caller()); }
        self
    }
    /// The element's accessible role. `"textbox"` on a container also makes a press anywhere
    /// on it (padding, a leading icon, a trailing label) focus its one reachable text editor and
    /// place the caret at the nearer end, without blurring a focused field or dragging the
    /// window; buttons and other controls inside keep their own presses. With zero or more
    /// than one reachable editor inside, the container does nothing special.
    #[inline]
    pub fn role(mut self, role: &'static str) -> Self {
        self.set_role(role);
        self
    }
    /// [`El::role`] in place.
    #[inline]
    pub fn set_role(&mut self, role: &'static str) -> &mut Self {
        self.set_role_name(role);
        self
    }
    #[inline]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.set_label(label);
        self
    }
    /// [`El::label`] in place.
    #[inline]
    pub fn set_label(&mut self, label: impl Into<String>) -> &mut Self {
        self.cold_mut().label = Some(label.into());
        self
    }
    /// An outline row's depth for accessibility, 1 at the top (ARIA `aria-level`), as a
    /// `treeitem` carries it. The element tree reports it as `level`.
    pub fn level(mut self, level: u32) -> Self {
        self.cold_mut().a11y.level = Some(level);
        self
    }
    /// Whether a disclosure (a tree row with children, a collapsible header) is open, for
    /// accessibility (ARIA `aria-expanded`). Leave it unset on an element that can't open.
    /// The element tree reports it as `expanded`.
    pub fn expanded(mut self, open: bool) -> Self {
        self.cold_mut().a11y.expanded = Some(open);
        self
    }
    /// Turn this element's icon by `radians`, clockwise about its centre: a disclosure chevron
    /// pointing right when closed and down when open. A change springs on the element's
    /// [`motion`](Self::motion) and jumps under reduced motion. Only the glyph turns; layout and
    /// hit testing keep the upright box.
    pub fn icon_rotation(mut self, radians: f32) -> Self {
        let r = if radians.is_finite() { radians } else { 0.0 };
        if r != 0.0 || !self.vis.cold.is_empty() {
            self.vis.cold_mut().icon_turn = r;
        }
        self
    }
    /// A tooltip shown after the system's hover delay while the pointer rests on this element,
    /// as SwiftUI's `.help`. A press, scroll, key or the pointer leaving hides it; it takes no
    /// input itself. On macOS it is AppKit's own tooltip; elsewhere Rune draws it. The element
    /// tree (`Runtime::elements`, `--tree`) carries it as the element's help text.
    #[inline]
    pub fn help(mut self, text: impl Into<String>) -> Self {
        self.set_help(text);
        self
    }
    /// [`El::help`] in place.
    pub fn set_help(&mut self, text: impl Into<String>) -> &mut Self {
        let text = text.into();
        if !text.is_empty() || !self.cold.is_empty() {
            let cold = self.cold_mut();
            cold.help = (!text.is_empty()).then_some(text);
            cold.help_cut = false;
        }
        self
    }
    /// [`El::help`] that shows only while this text is cut short ([`truncate`](Self::truncate), `max_lines`
    /// or `truncate_fade`), as AppKit's expansion tooltips show a table cell's whole text.
    pub fn help_when_truncated(self, text: impl Into<String>) -> Self {
        let mut e = self.help(text);
        e.cold_mut().help_cut = true;
        e
    }
    /// Keycaps drawn after this element's [`El::help`] text, as a web tooltip shows its
    /// command's keybind: `.help("Toggle review").help_keys(["Ctrl", "Shift", "R"])`. Call it
    /// after `help`, which replaces them. The element tree and assistive technology read them
    /// after the text, a space apart.
    pub fn help_keys<K: Into<String>>(mut self, keys: impl IntoIterator<Item = K>) -> Self {
        if let Some(h) = self.cold.get_mut_if_some().and_then(|c| c.help.as_mut()) {
            for k in keys.into_iter().map(Into::into).filter(|k| !k.is_empty()) {
                h.push(HELP_KEY);
                h.push_str(&k);
            }
        }
        self
    }
    /// Shows this element's [`El::help`] tooltip above it, 4 points clear, and below only when
    /// there's no room above, as a web page's `placement="top"` tooltip sits over its button.
    pub fn help_above(mut self) -> Self {
        self.cold_mut().help_above = true;
        self
    }
    #[inline]
    pub fn child(mut self, child: El) -> Self {
        self.push_child(child);
        self
    }
    /// [`El::child`] in place.
    #[inline]
    pub fn push_child(&mut self, child: El) -> &mut Self {
        self.flags.set(ElFlags::MEMO_EXTENDED, self.memo.is_some());
        if child.flags.has(ElFlags::HAS_VIRTUAL) {
            self.flags.set(ElFlags::HAS_VIRTUAL, true);
        }
        self.children.push(child);
        self
    }
    #[inline]
    pub fn children(mut self, children: impl IntoIterator<Item = El>) -> Self {
        self.extend_children(children);
        self
    }
    /// [`El::children`] in place.
    #[inline]
    pub fn extend_children(&mut self, children: impl IntoIterator<Item = El>) -> &mut Self {
        self.flags.set(ElFlags::MEMO_EXTENDED, self.memo.is_some());
        let start = self.children.len();
        self.children.extend(children);
        if self.children[start..].iter().any(|c| c.flags.has(ElFlags::HAS_VIRTUAL)) {
            self.flags.set(ElFlags::HAS_VIRTUAL, true);
        }
        self
    }

    // Text
    #[inline]
    pub fn color(mut self, c: Color) -> Self {
        self.set_color(c);
        self
    }
    /// [`El::color`] in place.
    #[inline]
    pub fn set_color(&mut self, c: Color) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.color = c;
            t.inked = true;
        }
        if let Some(cold) = self.vis.cold.get_mut_if_some() && let Some((_, ic)) = &mut cold.icon {
            *ic = c;
            cold.icon_inked = true;
        }
        if self.cold.input.is_some() {
            self.sync_input_rich();
        }
        self
    }
    #[inline]
    pub fn align(mut self, a: Align) -> Self {
        self.set_align(a);
        self
    }
    /// [`El::align`] in place.
    #[inline]
    pub fn set_align(&mut self, a: Align) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.align = a;
        }
        self
    }
    /// Draw this text in capitals or lower case without changing its string, like SwiftUI's
    /// `textCase`: `text("Text channels").text_case(TextCase::Uppercase)` draws "TEXT CHANNELS".
    /// The dev tree, tools and link labels keep the authored text, and [`Span`] and [`Link`]
    /// ranges stay byte ranges of it. Casing follows `str::to_uppercase`, so "ß" draws "SS".
    /// Text fields draw what is typed and ignore it.
    pub fn text_case(mut self, case: crate::TextCase) -> Self {
        if let Some(t) = &mut self.text {
            t.case = Some(case);
        }
        self
    }
    /// Spread each wrapped line but the last to fill the width; the last line and unwrapped text
    /// keep [`El::align`].
    pub fn justify(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.justify = true;
        }
        self
    }
    /// Measure this text as the grapheme at byte `at` of `run`, shaped as one line in the same
    /// style, instead of on its own rounded up to a device pixel. Neighbouring cells of one run
    /// then sit at its fractional, kerned advances and together take its width, and each draws
    /// the run's own glyphs for its grapheme (contextual forms included).
    pub fn advance_in(mut self, run: &crate::text::Run, at: usize) -> Self {
        if let Some(t) = &mut self.text {
            t.advance_in = Some((run.clone(), at));
        }
        self
    }
}

/// Pins a closure to the signature [`El::on_edit`] needs, so Rust ties the returned borrow to
/// the state: `.on_edit(edit_access(move |s: &mut App| &mut s.rows[i].name))`.
pub fn edit_access<S, F: for<'a> Fn(&'a mut S) -> &'a mut TextEdit>(f: F) -> F {
    f
}
