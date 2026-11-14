//! What an element's handler is handed. Handlers are stored type-erased and take the state of
//! the type they were built for: the app's, or, while the playground shows a stage, the stage's
//! (a fresh scene state or a page's own), beside the playground's own controls.

use std::any::Any;

/// The states one handler call can reach; [`States::cast`] picks the handler's own.
pub(crate) struct States<'a> {
    app: &'a mut dyn Any,
    extra: Option<&'a mut dyn Any>,
}

impl<'a> States<'a> {
    pub(crate) fn one(app: &'a mut dyn Any) -> Self {
        States { app, extra: None }
    }

    pub(crate) fn two(app: &'a mut dyn Any, extra: &'a mut dyn Any) -> Self {
        States { app, extra: Some(extra) }
    }

    /// None when neither is an `S`: a deferred callback whose stage the playground has dropped.
    pub(crate) fn cast<S: 'static>(self) -> Option<&'a mut S> {
        if self.app.is::<S>() {
            return self.app.downcast_mut();
        }
        self.extra?.downcast_mut()
    }

    pub(crate) fn reborrow(&mut self) -> States<'_> {
        States { app: &mut *self.app, extra: self.extra.as_deref_mut() }
    }
}

/// [`States`] for reads (index-built rows, drag-out items).
#[derive(Clone, Copy)]
pub(crate) struct StatesRef<'a> {
    app: &'a dyn Any,
    extra: Option<&'a dyn Any>,
}

impl<'a> StatesRef<'a> {
    pub(crate) fn one(app: &'a dyn Any) -> Self {
        StatesRef { app, extra: None }
    }

    pub(crate) fn two(app: &'a dyn Any, extra: &'a dyn Any) -> Self {
        StatesRef { app, extra: Some(extra) }
    }

    pub(crate) fn cast<S: 'static>(self) -> Option<&'a S> {
        self.app.downcast_ref().or_else(|| self.extra?.downcast_ref())
    }
}

pub(crate) fn downcast<S: 'static>(s: States<'_>) -> &mut S {
    s.cast::<S>().expect("handler registered for a different state type than the app's")
}

/// Pins an edit-access closure's signature, so Rust ties the returned borrow to the states.
pub(crate) fn edit_fn<F: for<'a> Fn(States<'a>) -> &'a mut crate::edit::TextEdit>(f: F) -> F {
    f
}
