//! Layout direction: `El::dir` lays a subtree out right to left (or left to right inside one
//! that is), as HTML's `dir` attribute does, and `El::mirror_in_rtl` flips a directional icon.
//!
//! Layout runs left to right; a right-to-left element then mirrors where its children sit inside
//! it, so a row's first child is at its right, start and end alignment swap, and an absolute
//! child's left inset is its right. Its own padding, border sides, corner radii, soft clip
//! edges and linear gradients swap left and right with it, as CSS's logical `start` and `end`
//! resolve: in this model `left` means start (`El::unmirrored` keeps an element physical).
//! An `El::anchored` overlay's `Left` and `Right` sides and a `Top` or `Bottom` overlay's
//! `Start` and `End` alignment swap too. Shadows, `El::offset` and other visual transforms
//! stay physical, as CSS's do. Text lays out with a right-to-left paragraph
//! direction (bidi base level), so its `Start` alignment is its right edge.
//!
//! Each element's direction resolves as one step down the tree ([`step`]), from its parent's
//! [`Flow`] and its own [`DirEvent`]. Nothing is mirrored in a window that sets no
//! right-to-left direction: layout reads one small enum per element.
use super::*;

/// Which way an element and what it holds run, as HTML's `dir` attribute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Left to right. Inside a right-to-left subtree it's an island that keeps its own order,
    /// as `dir="ltr"` on a path, a command or code.
    #[default]
    Ltr,
    /// Right to left: Arabic, Hebrew, Persian, Urdu.
    Rtl,
    /// From the first strong character of the element's text (or, on a container, of the text
    /// inside it, skipping subtrees that set their own direction), as `dir="auto"` and `<bdi>`
    /// resolve it: right to left when that's Arabic or Hebrew, else left to right, and left to
    /// right when there's none. An empty text field takes its placeholder's. For user strings
    /// such as titles, names, messages and a prompt.
    Auto,
}

impl Direction {
    /// The direction of `text` as [`Direction::Auto`] resolves it: its first strong character's,
    /// outside isolates (UAX #9 rules P2 and P3), or `None` when it has none.
    pub fn of(text: &str) -> Option<Direction> {
        use icu_properties::{CodePointMapData, props::BidiClass};
        let classes = CodePointMapData::<BidiClass>::new();
        let mut isolates = 0u32;
        for c in text.chars() {
            match classes.get(c) {
                BidiClass::LeftToRightIsolate | BidiClass::RightToLeftIsolate | BidiClass::FirstStrongIsolate => isolates += 1,
                BidiClass::PopDirectionalIsolate => isolates = isolates.saturating_sub(1),
                BidiClass::ParagraphSeparator if isolates == 0 => return None,
                BidiClass::LeftToRight if isolates == 0 => return Some(Direction::Ltr),
                BidiClass::RightToLeft | BidiClass::ArabicLetter if isolates == 0 => return Some(Direction::Rtl),
                _ => {}
            }
        }
        None
    }

    /// Whether this is [`Direction::Rtl`].
    pub fn is_rtl(self) -> bool {
        self == Direction::Rtl
    }
}

impl El {
    /// Lays this element and its subtree out in `dir` (see [`Direction`]): `Rtl` mirrors rows,
    /// start and end alignment, padding, border sides, corner radii, anchored overlays and
    /// horizontal scrolling, and gives text a right-to-left paragraph direction; `Ltr` inside a
    /// right-to-left subtree keeps an island in its own order (a path, a command, code), and
    /// `Auto` takes the direction of the text it holds (a title, a name, a message). Elements
    /// that set none take their parent's; the window's root runs left to right.
    pub fn dir(mut self, dir: Direction) -> Self {
        self.flags.set_dir(dir);
        self
    }

    /// Keeps this element where left-to-right layout puts it inside a right-to-left parent,
    /// unmirrored, as a CSS physical `left` or `right` inset stays put under `dir="rtl"`: room
    /// kept clear of the window's own controls (Windows' caption buttons stay on the right,
    /// macOS's traffic lights on the left), or a pin that must not swap sides. Its own subtree
    /// still lays out in its direction.
    pub fn unmirrored(mut self) -> Self {
        self.flags.set(ElFlags::UNMIRRORED, true);
        self
    }

    /// Flips this element's icon left to right when it lays out right to left, for an icon
    /// that points along the line (a chevron, an arrow, a back button), as `@opencode/ui`'s
    /// `data-directional` icons do. Icons that don't point keep their drawing. A turned element
    /// or icon (`El::rotate`, `El::icon_rotation`) turns the other way, as a CSS `scaleX(-1)`
    /// outside its rotation: a collapsed disclosure's chevron turned to point right points left.
    pub fn mirror_in_rtl(mut self) -> Self {
        self.vis.cold_mut().mirror_rtl = true;
        self
    }
}

/// The direction an element resolved to, which its subtree lays out in: what `El::dir` and its
/// ancestors' give it, `Direction::Auto` settled by its text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Flow {
    #[default]
    Ltr,
    Rtl,
}

impl Flow {
    /// A laid-out paragraph's flow: its bidi base level.
    pub(crate) fn of_layout<B: parley::Brush>(layout: &parley::Layout<B>) -> Flow {
        if layout.is_rtl() { Flow::Rtl } else { Flow::Ltr }
    }

    /// The paragraph direction text in this flow lays out in.
    pub(crate) fn base(self) -> parley::BaseDirection {
        match self {
            Flow::Ltr => parley::BaseDirection::Ltr,
            Flow::Rtl => parley::BaseDirection::Rtl,
        }
    }
}

/// Where an element sits in its parent: where its parent's flow puts it, or where left-to-right
/// layout does whatever the flow (`El::unmirrored`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Pin {
    #[default]
    Flowing,
    Physical,
}

impl Pin {
    pub(crate) fn of(el: &El) -> Pin {
        if el.flags.has(ElFlags::UNMIRRORED) { Pin::Physical } else { Pin::Flowing }
    }
}

/// What an element says about its own direction, the event that steps its parent's flow to its.
pub(crate) enum DirEvent<'a> {
    /// No `El::dir`: it takes its parent's.
    Inherit,
    /// `El::dir(Ltr)` or `El::dir(Rtl)`.
    Set(Flow),
    /// `El::dir(Auto)`: its first strong character decides, left to right when none does.
    Auto(&'a El),
}

impl<'a> DirEvent<'a> {
    pub(crate) fn of(el: &'a El) -> DirEvent<'a> {
        match el.flags.dir() {
            None => DirEvent::Inherit,
            Some(Direction::Ltr) => DirEvent::Set(Flow::Ltr),
            Some(Direction::Rtl) => DirEvent::Set(Flow::Rtl),
            Some(Direction::Auto) => DirEvent::Auto(el),
        }
    }
}

/// What flattening does to an element once its flow is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirEffect {
    /// Left to right: as authored.
    Keep,
    /// Right to left: its own left and right sides (`mirror_visual`) and its anchored
    /// overlay's side (`mirror_anchor`) swap.
    Mirror,
}

/// One step down the tree: the parent's flow and the element's own direction give its flow,
/// and what it has to mirror.
pub(crate) fn step(parent: Flow, event: DirEvent<'_>) -> (Flow, DirEffect) {
    let flow = match event {
        DirEvent::Inherit => parent,
        DirEvent::Set(flow) => flow,
        DirEvent::Auto(el) => if first_strong(el) == Some(Direction::Rtl) { Flow::Rtl } else { Flow::Ltr },
    };
    let effect = match flow {
        Flow::Ltr => DirEffect::Keep,
        Flow::Rtl => DirEffect::Mirror,
    };
    (flow, effect)
}

/// The first strong direction in `el`'s text, then its children's in order, skipping children
/// that set their own direction.
fn first_strong(el: &El) -> Option<Direction> {
    if let Some(t) = &el.text
        && let Some(d) = Direction::of(&t.content)
    {
        return Some(d);
    }
    // An empty field reads its placeholder, as the placeholder shows in its place.
    if let Some(i) = el.cold.input.as_ref().filter(|i| !i.edit.is_secure())
        && let Some(d) = Direction::of(if i.edit.text.is_empty() { &i.placeholder } else { &i.edit.text })
    {
        return Some(d);
    }
    el.all_children().filter(|c| c.flags.dir().is_none()).find_map(first_strong)
}

/// Mirrors what a right-to-left element draws on its own left and right: corner radii, border
/// sides, soft clip edges and an angled linear gradient's run.
pub(crate) fn mirror_visual(vis: &mut Visual) {
    // Uniform corners (most boxes) and no extras: nothing to mirror, nothing touched.
    if vis.radius.is_nan() {
        let [tl, tr, br, bl] = vis.radii();
        vis.set_radii([tr, tl, bl, br]);
    }
    let Some(cold) = vis.cold.get() else { return };
    let turns = cold.mirror_rtl && (cold.rotate != 0.0 || cold.icon_turn != 0.0);
    let flips = turns || cold.border_sides.is_some() || cold.clip.is_some() || cold.clip_corners.is_some()
        || cold.radial.as_ref().is_some_and(|r| matches!(r.geometry, GradientGeometry::Linear { .. }));
    if !flips {
        return;
    }
    let cold = vis.cold_mut();
    if cold.mirror_rtl {
        // A mirrored drawing turns the other way.
        cold.rotate = -cold.rotate;
        cold.icon_turn = -cold.icon_turn;
    }
    if let Some(r) = &mut cold.radial
        && let GradientGeometry::Linear { angle } = &mut r.geometry
    {
        // CSS angles run clockwise from up: a left-to-right ramp runs right to left.
        *angle = -*angle;
    }
    if let Some([t, r, b, l]) = cold.border_sides {
        cold.border_sides = Some([t, l, b, r]);
    }
    if let Some([l, t, r, b]) = cold.clip {
        cold.clip = Some([r, t, l, b]);
    }
    if let Some([tl, tr, br, bl]) = cold.clip_corners {
        cold.clip_corners = Some([tr, tl, bl, br]);
    }
}
/// An anchored overlay's placement as a right-to-left element means it: `Left` and `Right`
/// swap, and along a `Top` or `Bottom` side so do `Start` and `End`.
pub(crate) fn mirror_anchor(a: &mut Anchor) {
    a.side = match a.side {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
        s => s,
    };
    if matches!(a.side, Side::Top | Side::Bottom) {
        a.align = match a.align {
            Align::Start => Align::End,
            Align::End => Align::Start,
            Align::Center => Align::Center,
        };
    }
}
