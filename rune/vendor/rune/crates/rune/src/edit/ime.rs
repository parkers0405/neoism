//! Input method events: what the platform's input method (dead keys, CJK, the emoji picker,
//! dictation) composes, for the focused `text_input` or an app's own editor (`App::on_ime`);
//! and a field's [`Composition`], stepped by those events.
use zeroize::Zeroize;

use crate::machine::Effects;

/// One input method event. Plain typing arrives as keys (`Key::Text`); these carry what an
/// input method composed.
#[derive(Clone, Debug, PartialEq)]
pub enum Ime {
    /// The composition so far (marked text), drawn at the caret and not yet part of the text,
    /// with the byte range of its cursor or selection in it. Empty ends the composition.
    Preedit(String, Option<(usize, usize)>),
    /// Text to insert at the caret, replacing the preedit.
    Commit(String),
    /// The window's input method turned on for the caret that takes text (a focused
    /// `text_input` or an `El::ime_cursor` surface), or off again. Informational: Rune turns it
    /// on and off.
    Enabled,
    Disabled,
}

impl Drop for Ime {
    fn drop(&mut self) {
        match self {
            Self::Preedit(text, _) | Self::Commit(text) => text.zeroize(),
            Self::Enabled | Self::Disabled => {}
        }
    }
}

/// Whether a field's input method is composing: the marked text it draws at the caret, not
/// yet part of the field's text, and the byte range of its cursor or selection in it.
#[derive(Clone, Default, PartialEq, Hash)]
pub(super) enum Composition {
    #[default]
    Idle,
    Composing { text: String, cursor: Option<(usize, usize)> },
}

/// What a composition step asks of its field.
#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// Insert the committed text at the caret.
    Commit(String),
}

impl Composition {
    pub(super) fn preedit(&self) -> Option<(&str, Option<(usize, usize)>)> {
        match self {
            Composition::Idle => None,
            Composition::Composing { text, cursor } => Some((text.as_str(), *cursor)),
        }
    }

    /// Ends any composition, wiping its text: the field blurred, an edit or undo replaced the
    /// text under it, or the app cancelled it.
    pub(super) fn cancel(&mut self) {
        if let Composition::Composing { text, .. } = self { text.zeroize(); }
        *self = Composition::Idle;
    }
}

/// Steps `composition` with an input method event.
pub(super) fn step(composition: &mut Composition, event: &Ime) -> Effects<Effect> {
    match event {
        Ime::Preedit(text, _) if text.is_empty() => {
            composition.cancel();
            Effects::none()
        }
        Ime::Preedit(text, cursor) => {
            composition.cancel();
            *composition = Composition::Composing { text: text.clone(), cursor: *cursor };
            Effects::none()
        }
        Ime::Commit(text) => {
            composition.cancel();
            Effects::one(Effect::Commit(text.clone()))
        }
        Ime::Enabled | Ime::Disabled => Effects::none(),
    }
}
