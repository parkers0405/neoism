//! Component history is read from the start of a commit, even when measurement retries.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

type Snapshots = HashMap<(TypeId, String), Rc<dyn Any>>;

thread_local! {
    static SCOPES: RefCell<Vec<Snapshots>> = const { RefCell::new(Vec::new()) };
}

/// A component's history before this commit first built it.
///
/// A view may run several times in one commit while layout reads, focus, caret or virtual
/// rows settle. Components that remember previous content or a first-seen flag must compare
/// each build against this snapshot, then write their new history, rather than comparing
/// against a discarded measurement's writes. `key` names the history within its type; give
/// independent components different keys. Outside a runtime commit this just clones `value`.
///
/// The snapshot lives only until the commit ends. `Clone` must copy the history itself, not
/// share mutable cells with it. This is for component history, not application state or I/O.
pub fn snapshot<T: Clone + 'static>(key: &str, value: &T) -> Rc<T> {
    SCOPES.with_borrow_mut(|scopes| {
        let Some(scope) = scopes.last_mut() else { return Rc::new(value.clone()) };
        scope.entry((TypeId::of::<T>(), key.to_owned()))
            .or_insert_with(|| Rc::new(value.clone()))
            .clone().downcast::<T>().ok().expect("snapshot type is part of its key")
    })
}

pub(crate) struct Guard;

/// One scope surrounds every measurement pass, not each invocation of the view.
pub(crate) fn enter() -> Guard {
    SCOPES.with_borrow_mut(|scopes| scopes.push(HashMap::new()));
    Guard
}

impl Drop for Guard {
    fn drop(&mut self) {
        SCOPES.with_borrow_mut(|scopes| { scopes.pop(); });
    }
}

#[cfg(test)]
mod tests;
