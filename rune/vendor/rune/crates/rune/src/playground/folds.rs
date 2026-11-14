//! Which groups the list folds, at any level: kept while the app runs, and through
//! [`Playground::folds`]' hooks from one run to the next.

use super::*;

/// [`Playground::folds`]' hooks, and [`Playground::start_folded`]. `load` runs once, when the
/// playground first applies a request.
#[derive(Default)]
pub(super) struct Folds {
    load: Option<Box<dyn FnOnce() -> Vec<String>>>,
    save: Option<Box<dyn Fn(&[String])>>,
    pub(super) start: bool,
}

impl<S: 'static> Playground<S> {
    /// Every group starts folded, at every level, but the ones around the open story: each
    /// time the playground opens, the open story's groups unfold, so a link through
    /// [`Tx::open_playground_at`] at launch shows its story. Off by default, where every group
    /// starts open and a folded one stays folded until it's unfolded. Worth it for a long,
    /// deep list.
    pub fn start_folded(mut self, on: bool) -> Self {
        self.folds.start = on;
        self
    }

    /// Keeps the folded groups from one run to the next. Without it, folds last while the app
    /// runs, closing and opening the playground included. A group is named by its path,
    /// `"Sidebar"` or `"Sidebar/Filter buttons"` for one inside it. `load` gives the names
    /// saved last time, once, as the playground first opens; names it no longer has are
    /// dropped. `save` gets the names each time a group folds or unfolds: the folded groups,
    /// or with [`Playground::start_folded`] the unfolded ones, so a group added since starts
    /// as the rest do. With [`Prefs`](crate::prefs::Prefs):
    ///
    /// ```ignore
    /// .folds(
    ///     || Prefs::open(APP_ID).get("playground_folds").unwrap_or_default(),
    ///     |groups| { Prefs::open(APP_ID).set("playground_folds", &groups).ok(); },
    /// )
    /// ```
    pub fn folds(mut self, load: impl Fn() -> Vec<String> + 'static, save: impl Fn(&[String]) + 'static) -> Self {
        self.folds = Folds { load: Some(Box::new(load)), save: Some(Box::new(save)), start: self.folds.start };
        self
    }
}

impl<S: 'static> Live<S> {
    /// Folds the groups [`Playground::folds`]' `load` names, the first time it's called.
    pub(super) fn load_folds(&mut self) {
        let Some(load) = self.folds.load.take() else { return };
        let names = load();
        let start = self.folds.start;
        self.folded = self.all_groups().into_iter().filter(|g| names.iter().any(|n| n == g) != start).collect();
    }

    /// Changes which groups are folded, and saves them if that changed them.
    pub(super) fn refold(&mut self, change: impl FnOnce(&mut Vec<&'static str>)) {
        let before = self.folded.clone();
        change(&mut self.folded);
        if self.folded == before { return; }
        let Some(save) = &self.folds.save else { return };
        let names: Vec<String> = match self.folds.start {
            true => self.all_groups().into_iter().filter(|g| !self.folded.contains(g)).map(str::to_owned).collect(),
            false => self.folded.iter().map(|g| (*g).to_owned()).collect(),
        };
        save(&names);
    }
}
