//! Input method events for an editor the app draws itself: `App::on_ime` hears them while an
//! `El::ime_cursor` caret has the input method and no text field has focus.
use super::*;

/// `App::on_ime`, and whether the composition it was last sent is still open.
pub(crate) struct AppIme<S> {
    pub(crate) hook: Option<fn(&mut S, &Ime, &mut Tx) -> bool>,
    composing: bool,
}

impl<S> Default for AppIme<S> {
    fn default() -> Self { Self { hook: None, composing: false } }
}

impl<S> AppIme<S> {
    /// The app's editor has a composition open: Return and Escape belong to the input method.
    pub(super) fn composing(&self) -> bool { self.hook.is_some() && self.composing }
}

/// An `El::ime_cursor` element's drawn rect, device pixels, as the window's IME area in points.
pub(super) fn area(r: [f32; 4], s: f32) -> [f32; 4] {
    [r[0] / s, r[1] / s, (r[2] - r[0]) / s, (r[3] - r[1]) / s]
}

impl<S: 'static> Runtime<S> {
    /// An input method event goes to the focused text field, else to `App::on_ime`. A
    /// composition the app has open ends there even after a field took focus, and the input
    /// method turning on or off is only the app's to hear.
    pub(super) fn run_ime(&mut self, ev: Ime, one: &mut Tx) -> bool {
        let app = self.app_ime.composing() || matches!(ev, Ime::Enabled | Ime::Disabled) || self.focused_input().is_none();
        let active = self.composing() || matches!(&ev, Ime::Preedit(s, _) | Ime::Commit(s) if !s.is_empty());
        let ran = if app { self.app_ime_event(&ev, one) } else { self.with_focused(|e| e.ime(&ev), one) };
        // The key that confirmed or cancelled a composition comes back up unheard.
        if ran && active { self.ime_eat_key_up = true; }
        ran
    }

    fn app_ime_event(&mut self, ev: &Ime, one: &mut Tx) -> bool {
        let Some(f) = self.app_ime.hook else { return false };
        match ev {
            Ime::Preedit(s, _) => self.app_ime.composing = !s.is_empty(),
            Ime::Commit(_) => self.app_ime.composing = false,
            Ime::Enabled | Ime::Disabled => {}
        }
        let state = match &mut self.playground {
            Some(playground) if !playground.stage_hears(&self.state) => None,
            Some(playground) => playground.key_state(&mut self.state),
            None => Some(&mut self.state),
        };
        state.is_some_and(|state| Tx::took(app_call!(f, state, ev, one), one))
    }
}
