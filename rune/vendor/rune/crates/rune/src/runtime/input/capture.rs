//! The pointer's hold on an element: the primary (or an extra) button's press from down to up,
//! and the secondary button's beside it. A held press keeps what only it has (where and when it
//! went down, whether it's still inside its element or a scroll cancelled it, whether it
//! travelled past the click slop, whether it's the second of a double press, an armed OS drag),
//! so none of it outlives the release. Between presses only the first click of a possible
//! double press is kept. The input routes in `pointer.rs`, `gesture.rs`, `secondary.rs` and
//! `selection/click.rs` drive it.
use crate::PointerButton;
use crate::el::Drag;

/// Where a held press stands against its element: a browser's `:active` while it's inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hit {
    Inside,
    /// The pointer moved off the element; back on, it's inside again.
    Outside,
    /// A scroll under the press let go of it ([`Runtime::scroll_cancels_press`]): the release
    /// presses nothing, wherever the pointer is.
    Cancelled,
}

/// The first click of a possible double press: the element, its down time and point.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FirstClick {
    pub(crate) path: String,
    pub(crate) at: f64,
    pub(crate) point: (f32, f32),
}

/// A press the pointer holds on element `path`.
#[derive(Clone, Debug)]
pub(crate) struct Held {
    pub(crate) path: String,
    /// The `Phase::Down` its drags carry on from.
    pub(crate) drag: Drag,
    /// A scroll/hold-policy touch has not yet delivered its custom drag Down.
    pub(crate) touch_drag_pending: bool,
    pub(crate) button: PointerButton,
    /// Where (window points) and when (`now_clock`) it went down.
    pub(crate) at: (f32, f32),
    pub(crate) down_at: f64,
    pub(crate) hit: Hit,
    /// It travelled past the click slop: it isn't a click of a double press.
    pub(crate) moved: bool,
    /// It's the second press of a double press.
    pub(crate) second: bool,
    /// It's on a `drag_out` element and hasn't crossed this threshold (points) yet.
    pub(crate) drag_out: Option<f32>,
    /// The pointer left the window while it held; the leave lands on release.
    pub(crate) left: bool,
}

impl Held {
    /// A press that just went down at `at`, inside its element and still.
    pub(crate) fn new(path: String, drag: Drag, button: PointerButton, at: (f32, f32), down_at: f64, second: bool, drag_out: Option<f32>) -> Self {
        Self { path, drag, touch_drag_pending: false, button, at, down_at, hit: Hit::Inside, moved: false, second, drag_out, left: false }
    }

    pub(crate) fn inside(&self) -> bool { self.hit == Hit::Inside }
}

#[derive(Clone, Debug)]
pub(crate) enum Press {
    Up { first: Option<FirstClick> },
    Held(Box<Held>),
}

impl Default for Press {
    fn default() -> Self { Self::Up { first: None } }
}

impl Press {
    pub(crate) fn held(&self) -> Option<&Held> {
        match self {
            Self::Held(h) => Some(h),
            Self::Up { .. } => None,
        }
    }

    pub(crate) fn held_mut(&mut self) -> Option<&mut Held> {
        match self {
            Self::Held(h) => Some(h),
            Self::Up { .. } => None,
        }
    }

    pub(crate) fn path(&self) -> Option<&str> { self.held().map(|h| h.path.as_str()) }

    pub(crate) fn is_held(&self) -> bool { matches!(self, Self::Held(_)) }

    /// Held, and inside its element.
    pub(crate) fn inside(&self) -> bool { self.held().is_some_and(Held::inside) }

    /// Lets go of a held press without a click to pair: a cancel, a drag-out taking the pointer,
    /// a modal opening. Returns it.
    pub(crate) fn drop_held(&mut self) -> Option<Box<Held>> {
        match std::mem::take(self) {
            Self::Held(h) => Some(h),
            up => {
                *self = up;
                None
            }
        }
    }

    /// The release: back up, with `first` the click a next press may pair with. Returns the
    /// press that was held.
    pub(crate) fn release(&mut self, first: Option<FirstClick>) -> Option<Box<Held>> {
        let held = self.drop_held()?;
        *self = Self::Up { first };
        Some(held)
    }

    /// A new press takes the place of whatever came before: the first click it may pair with.
    pub(crate) fn take_first(&mut self) -> Option<FirstClick> {
        match self {
            Self::Up { first } => first.take(),
            Self::Held(_) => None,
        }
    }

    pub(crate) fn hold(&mut self, held: Held) { *self = Self::Held(Box::new(held)); }
}

/// The secondary button's hold: the element whose menu it opens on release.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Secondary {
    #[default]
    Up,
    Held { path: String, left: bool },
}

impl Secondary {
    pub(crate) fn path(&self) -> Option<&str> {
        match self {
            Self::Held { path, .. } => Some(path),
            Self::Up => None,
        }
    }

    pub(crate) fn is_held(&self) -> bool { matches!(self, Self::Held { .. }) }
}
