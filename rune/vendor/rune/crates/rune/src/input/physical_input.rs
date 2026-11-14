//! Physical input is queued with its logical fallback, so a preceding click can claim it.
use crate::{Mods, PhysicalKeyCode};

#[derive(Clone, Copy, Debug)]
pub enum PhysicalInput {
    Key { key: PhysicalKeyCode, mods: Mods, pressed: bool, repeat: bool },
    Modifiers(Mods),
    FocusLost,
}

pub(crate) type Handler = std::rc::Rc<dyn Fn(crate::states::States<'_>, &PhysicalInput, &mut crate::Tx) -> bool>;

impl crate::El {
    /// Receives physical keys before logical editing, app shortcuts and focus navigation.
    /// Return true to consume the event. FocusLost also arrives on window deactivation.
    pub fn on_physical_input<S: 'static>(mut self,
        f: impl Fn(&mut S, &PhysicalInput, &mut crate::Tx) -> bool + 'static) -> Self {
        self.cold_mut().focus.physical = Some(std::rc::Rc::new(move |state, event, tx| {
            state.cast::<S>().is_some_and(|s| f(s, event, tx))
        }));
        self
    }
}
