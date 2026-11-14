//! Finding a story by its path, name or slug, and opening the playground on it: one lookup for
//! `--playground NAME --variant V`, [`Tx::open_playground_at`] and `Runtime::open_playground`.

use super::*;

/// A story's path as a link takes it: lowercased, with each run of characters that aren't
/// letters or digits (the `/` between group and name, spaces, punctuation) made one `-`, and
/// none at either end. `"Timeline/Send"` is `"timeline-send"` and `"Lists/Many devices"` is
/// `"lists-many-devices"`. Letters outside ASCII are kept, lowercased. The rule won't change,
/// so a link stays good while the story keeps its path. Every lookup by name accepts a slug:
/// `--playground timeline-send`, [`Tx::open_playground_at`] and `Runtime::open_playground`.
pub fn slug(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    if out.ends_with('-') { out.pop(); }
    out
}

impl<S: 'static> Live<S> {
    /// The listed story called `name`: its path or just its name, matching case only if need
    /// be, or its path's [`slug`]. Where two match, the first in the list.
    pub(super) fn find(&self, name: &str) -> Option<usize> {
        self.find_in(name, &self.order)
    }

    /// [`Live::find`] among `ids`, in order.
    pub(super) fn find_in(&self, name: &str, ids: &[usize]) -> Option<usize> {
        let exact = |i: &&usize| self.path(**i) == name || self.name(**i) == name;
        let loose = |i: &&usize| self.path(**i).eq_ignore_ascii_case(name) || self.name(**i).eq_ignore_ascii_case(name);
        let slugged = |i: &&usize| slug(self.path(**i)) == name;
        ids.iter().find(exact).or_else(|| ids.iter().find(loose)).or_else(|| ids.iter().find(slugged)).copied()
    }

    /// Checks that story `name` (else the one selected) is there, and picks `variant` of it.
    pub(super) fn open_at(&mut self, name: Option<&str>, variant: Option<&str>) -> Result<(), String> {
        if let Some(name) = name && self.find(name).is_none() {
            let names: Vec<_> = self.order.iter().map(|&i| self.path(i)).collect();
            return Err(format!("no story {name:?} in the playground; it has: {}", names.join(", ")));
        }
        match variant {
            Some(variant) => self.pick(name, variant),
            None => Ok(()),
        }
    }

    /// [`Tx::open_playground_at`]'s story, once checked. An app can't stop over a bad link, so
    /// what's wrong is logged: an unknown story opens the list where it was, and a story's
    /// unknown variant opens the story on the variant it was on.
    pub(super) fn resolve(&mut self, name: &str, variant: Option<&str>) -> Option<String> {
        let Err(e) = self.open_at(Some(name), variant) else { return Some(name.to_owned()) };
        eprintln!("rune: Tx::open_playground_at: {e}");
        self.find(name).map(|_| name.to_owned())
    }

    /// Opens the playground, on story `name` if it's there; open already, it moves there.
    pub(super) fn open_story(&mut self, name: Option<&str>, real: &mut S) {
        let at = name.and_then(|n| self.find(n));
        if !self.open || at.is_some() {
            self.open = true;
            if let Some(i) = at.or(self.folds.start.then_some(self.selected)) { self.reveal(i); }
            self.select(at.unwrap_or(self.selected), real);
        }
    }
}

impl Tx {
    /// Shows [`App::playground`](crate::App::playground) on `story` (its `"Group/Name"` path,
    /// its name, or its path's [`slug`]), on `variant` (by number from 1, or by name) if given,
    /// as `--playground NAME --variant V` does. From `App::on_open_url` a link opens straight
    /// onto the story, at launch too: the hook runs before the first frame, which then shows
    /// the story with no frame of the app first. A story it doesn't have is logged and opens the
    /// list instead; an unknown variant is logged and the story opens as it would without one.
    /// Where the playground doesn't show, nothing happens, as with [`Tx::open_playground`].
    pub fn open_playground_at(&mut self, story: &str, variant: Option<&str>) {
        self.playground.push(Request::OpenAt(story.to_owned(), variant.map(str::to_owned)));
    }

    /// Makes the open story's stage again and runs its `'r'` trigger, as R does. After
    /// [`Tx::open_playground_at`] in the same handler it applies to the story that opens.
    /// With the playground closed, nothing happens.
    pub fn replay_playground(&mut self) {
        self.playground.push(Request::Replay);
    }

    /// Runs the open story's trigger on `key`, as its letter or its side-column button does;
    /// `'r'` is [`Tx::replay_playground`]. A key the story has no trigger on does nothing.
    pub fn playground_trigger(&mut self, key: char) {
        let key = key.to_ascii_lowercase();
        self.playground.push(if key == 'r' { Request::Replay } else { Request::Trigger(key) });
    }
}
