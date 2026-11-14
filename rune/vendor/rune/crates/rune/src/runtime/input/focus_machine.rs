//! The focus machine: which element has keyboard focus, how focus shows, and where Tab starts
//! while nothing has it. `step` decides; `Runtime::set_focus` and the input routes feed it and
//! carry out what it returns (the IME, links and the inputs' own focus flags follow a move).
//!
//! The pieces it keeps apart, as a browser does:
//! - [`Focus`]: nothing focused, or one element. Only an element the pointer lit as a
//!   `focus_highlight` row is `hovered`, and only while nothing is focused is there a starting
//!   point for Tab (`from`) or a focus a press just took away (`blurred`).
//! - [`Modality`]: whether focus shows (CSS `:focus-visible`'s heuristic). It outlives a blur,
//!   so focus a closing modal hands back shows as it did.
//! - [`TypeSelect`]: a list's typed prefix, which a pause ends.
use crate::machine::Effects;

/// Whether the keyboard drove focus last: then a focused control shows its ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Modality {
    #[default]
    Pointer,
    Keyboard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    /// Nothing has focus. Tab starts from `from` (the element last focused, or pressed without
    /// taking focus, as a browser's sequential focus navigation starting point). `blurred` is
    /// the focus the gesture or commit underway took away: a scrim that blurs on pointer-down
    /// and closes its overlay on pointer-up still returns focus through `El::focus_return`.
    None { from: Option<String>, blurred: Option<String> },
    /// `path` has focus. `hovered`: the pointer lit it as a `focus_highlight` row, so its
    /// highlight shows without a key. `from`: a press on content that takes no focus queued a
    /// blur, and Tab starts there once it lands.
    On { path: String, hovered: bool, from: Option<String> },
}

impl Default for Focus {
    fn default() -> Self { Self::None { from: None, blurred: None } }
}

pub(crate) enum FocusEvent {
    /// Focus moves to `path`, or nowhere, and shows as `keyboard` says.
    Set { path: Option<String>, keyboard: bool },
    /// The pointer lit (`true`) or left (`false`) the focused `focus_highlight` row.
    Hover(bool),
    /// A key that navigates (Tab, an arrow): focus shows from now on.
    Navigated,
    /// A pointer press, or a key on a highlight the user couldn't see: focus stops showing.
    Pointed,
    /// A press on content that takes no focus: Tab goes on from there.
    StartAt(String),
    /// A commit settled with no press held, or `Tx::blur`: a blur it carried is final.
    Settled,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FocusEffect {
    /// Focus left the element it was on (or arrived from nowhere): the IME, links and the
    /// selection's keyboard follow.
    Moved,
}

/// The whole focus state the runtime keeps.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FocusState {
    pub(crate) focus: Focus,
    pub(crate) modality: Modality,
}

impl FocusState {
    pub(crate) fn path(&self) -> Option<&str> {
        match &self.focus {
            Focus::On { path, .. } => Some(path),
            Focus::None { .. } => None,
        }
    }

    pub(crate) fn owned(&self) -> Option<String> { self.path().map(str::to_owned) }

    pub(crate) fn is(&self, path: &str) -> bool { self.path() == Some(path) }

    pub(crate) fn keyboard(&self) -> bool { self.modality == Modality::Keyboard }

    /// The pointer lit the focused row.
    pub(crate) fn hovered(&self) -> bool { matches!(self.focus, Focus::On { hovered: true, .. }) }

    /// Focus shows, by the keyboard or the pointer's light on a row.
    pub(crate) fn shown(&self) -> bool { self.keyboard() || self.hovered() }

    /// Where Tab starts while nothing is focused.
    pub(crate) fn tab_start(&self) -> Option<&str> {
        match &self.focus {
            Focus::None { from, .. } => from.as_deref(),
            Focus::On { .. } => None,
        }
    }

    /// Focus now, else the focus the gesture underway took away.
    pub(crate) fn now_or_blurred(&self) -> Option<&str> {
        match &self.focus {
            Focus::On { path, .. } => Some(path),
            Focus::None { blurred, .. } => blurred.as_deref(),
        }
    }

    pub(crate) fn step(&mut self, event: FocusEvent) -> Effects<FocusEffect> {
        match event {
            FocusEvent::Set { path, keyboard } => {
                self.modality = if keyboard { Modality::Keyboard } else { Modality::Pointer };
                let moved = self.path() != path.as_deref();
                self.focus = match (std::mem::take(&mut self.focus), path) {
                    (Focus::On { path: old, hovered, .. }, Some(path)) if old == path => Focus::On { path, hovered, from: None },
                    (_, Some(path)) => Focus::On { path, hovered: false, from: None },
                    (Focus::On { path: old, from, .. }, None) => Focus::None { from: from.or_else(|| Some(old.clone())), blurred: Some(old) },
                    (none @ Focus::None { .. }, None) => none,
                };
                if moved { Effects::one(FocusEffect::Moved) } else { Effects::none() }
            }
            FocusEvent::Hover(lit) => {
                if let Focus::On { hovered, .. } = &mut self.focus { *hovered = lit; }
                Effects::none()
            }
            FocusEvent::Navigated => {
                self.modality = Modality::Keyboard;
                Effects::none()
            }
            FocusEvent::Pointed => {
                self.modality = Modality::Pointer;
                Effects::none()
            }
            FocusEvent::StartAt(path) => {
                match &mut self.focus {
                    Focus::None { from, .. } | Focus::On { from, .. } => *from = Some(path),
                }
                Effects::none()
            }
            FocusEvent::Settled => {
                if let Focus::None { blurred, .. } = &mut self.focus { *blurred = None; }
                Effects::none()
            }
        }
    }

    /// Sets focus aside for a key the playground's stage presses, without a transition: nothing
    /// moves, and [`Self::put_back`] restores it if nothing took focus meanwhile.
    pub(crate) fn set_aside(&mut self) -> Option<Focus> {
        let Focus::On { path, from, .. } = &self.focus else { return None };
        let rest = Focus::None { from: from.clone(), blurred: Some(path.clone()) };
        Some(std::mem::replace(&mut self.focus, rest))
    }

    pub(crate) fn put_back(&mut self, aside: Focus) {
        if matches!(self.focus, Focus::None { .. }) { self.focus = aside; }
    }
}

/// How long a list waits for the next typed letter before its prefix starts over (AppKit's).
pub(crate) const TYPE_SELECT_PAUSE: f64 = 1.0;

/// A list's type-select: nothing typed, or a prefix and when its last letter arrived.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum TypeSelect {
    #[default]
    Idle,
    Typing { prefix: String, at: f64 },
}

impl TypeSelect {
    /// The prefix after `typed` arrives at `now`: it starts over after a pause.
    pub(crate) fn typed(&mut self, typed: &str, now: f64) -> &str {
        let prefix = match std::mem::take(self) {
            Self::Typing { prefix, at } if now - at <= TYPE_SELECT_PAUSE => prefix,
            _ => String::new(),
        };
        *self = Self::Typing { prefix: prefix + &typed.to_lowercase(), at: now };
        match self {
            Self::Typing { prefix, .. } => prefix,
            Self::Idle => "",
        }
    }

    /// A key that moves by position (an arrow, Home, End) ends the prefix.
    pub(crate) fn reset(&mut self) { *self = Self::Idle; }
}
