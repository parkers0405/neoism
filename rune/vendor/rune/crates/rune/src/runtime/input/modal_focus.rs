//! The modal focus machine: the open modals, innermost last, each holding where focus returns
//! when it closes, as a web dialog stack does (`dialog-machine.ts` in OpenCode's UI: an open
//! entry's `focus` is where focus was before it opened, and only the top layer traps focus and
//! hands it back). `step` decides; `Runtime::sync_modals` feeds it what the commit shows and
//! carries out its effects. A menu or a select's listbox the pointer opened hands focus back
//! without a ring, as Desktop's menu machine does (`ui/src/navigation/menu/machine.ts`,
//! `selectMenuRestoreTriggerWithoutRing`): keys used inside it don't make its trigger ring.
use super::*;

/// Modal roles that close as Desktop's menus do: a menu, and a select's listbox.
const MENU_ROLES: [&str; 2] = ["menu", "listbox"];

/// One open modal: its element, where focus was before it opened, and how it opened.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModalLayer {
    path: String,
    restore: Option<String>,
    opened: Opened,
}

/// What opened a modal, for how focus shows as it hands focus back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opened {
    /// A dialog, or a menu the keyboard opened: focus returns showing as the keys left it.
    Dialog,
    /// A menu or listbox the pointer opened: focus returns without a ring.
    PointerMenu,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum ModalFocus {
    #[default]
    Closed,
    /// Never empty: the last layer is the top one, which holds the trap.
    Open(Vec<ModalLayer>),
}

pub(crate) enum ModalEvent {
    /// A modal mounted; `restore` is the focus it hands back on closing.
    Opened { path: String, restore: Option<String>, opened: Opened },
    /// The top modal's element is gone. `kept` is focus the closing change already put outside
    /// it (a field the handler focused), which stays.
    TopRemoved { kept: bool },
    /// Focus is outside the top modal (nothing focused, or behind it).
    FocusOutside,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ModalEffect {
    /// Drop gestures captured before the modal opened.
    CancelGestures,
    /// Focus what the top modal opens on (`FocusOutside`, which a new modal is in until a field
    /// inside it asks for focus): its `autofocus` control, else its field, else itself.
    FocusInitial,
    /// Focus returns where it was before the closed modal opened; `ring` false keeps it from
    /// showing whatever the keys did.
    Return { restore: Option<String>, ring: bool },
}

impl ModalFocus {
    pub(crate) fn top(&self) -> Option<&str> {
        match self {
            Self::Closed => None,
            Self::Open(layers) => layers.last().map(|l| l.path.as_str()),
        }
    }

    fn holds(&self, path: &str) -> bool {
        matches!(self, Self::Open(layers) if layers.iter().any(|l| l.path == path))
    }

    pub(crate) fn step(self, event: ModalEvent) -> (Self, Vec<ModalEffect>) {
        match (self, event) {
            (state, ModalEvent::Opened { path, .. }) if state.holds(&path) => (state, vec![]),
            (Self::Closed, ModalEvent::Opened { path, restore, opened }) => {
                (Self::Open(vec![ModalLayer { path, restore, opened }]), vec![ModalEffect::CancelGestures])
            }
            (Self::Open(mut layers), ModalEvent::Opened { path, restore, opened }) => {
                layers.push(ModalLayer { path, restore, opened });
                (Self::Open(layers), vec![ModalEffect::CancelGestures])
            }
            (Self::Open(mut layers), ModalEvent::TopRemoved { kept }) => {
                let closed = layers.pop();
                let state = if layers.is_empty() { Self::Closed } else { Self::Open(layers) };
                let effects = closed.filter(|_| !kept)
                    .map(|l| ModalEffect::Return { restore: l.restore, ring: l.opened == Opened::Dialog })
                    .into_iter().collect();
                (state, effects)
            }
            (state @ Self::Open(_), ModalEvent::FocusOutside) => (state, vec![ModalEffect::FocusInitial]),
            (state, _) => (state, vec![]),
        }
    }
}

impl<S: 'static> Runtime<S> {
    /// Feeds the modal focus machine what this commit shows: modals that went (innermost first),
    /// a modal that opened, and focus that left the top modal.
    pub(super) fn sync_modals(&mut self) {
        // Exiting visuals are already inert, so a closing modal counts as gone.
        while let Some(top) = self.modal_focus.top().map(str::to_owned).filter(|p| self.node(p).is_none()) {
            let inside = |f: &str| f.strip_prefix(top.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
            let kept = self.focus.path().is_some_and(|f| !inside(f) && self.node(f).is_some());
            self.modal_step(ModalEvent::TopRemoved { kept });
        }
        let Some(i) = self.active_modal() else { return };
        let path = self.nodes[i].path.clone();
        // A field in the modal that asked for focus took it earlier in this commit; closing
        // returns to what had focus before.
        let inside = self.focused_index().is_some_and(|j| self.allowed(j));
        let restore = if inside { self.focus_before_commit.clone() } else { self.focus.owned() };
        let menu = |j: usize| MENU_ROLES.contains(&self.nodes[j].role);
        let is_menu = menu(i) || (i + 1..self.nodes.len()).take_while(|&j| self.nodes[j].parent.is_some_and(|p| p >= i)).any(|j| self.nodes[j].parent == Some(i) && menu(j));
        let opened = if is_menu && !self.focus.keyboard() { Opened::PointerMenu } else { Opened::Dialog };
        self.modal_step(ModalEvent::Opened { path, restore, opened });
        if !self.focused_index().is_some_and(|j| self.allowed(j)) {
            self.modal_step(ModalEvent::FocusOutside);
        }
    }

    fn modal_step(&mut self, event: ModalEvent) {
        let (state, effects) = std::mem::take(&mut self.modal_focus).step(event);
        self.modal_focus = state;
        for effect in effects {
            match effect {
                ModalEffect::CancelGestures => {
                    self.press.drop_held();
                    self.secondary = capture::Secondary::Up;
                    self.hovered = None;
                }
                ModalEffect::FocusInitial => {
                    if let Some(target) = self.modal_initial_focus() {
                        self.set_focus(Some(target), self.focus.keyboard());
                    }
                }
                ModalEffect::Return { restore, ring } => {
                    let restore = restore.filter(|p| self.node(p).is_some());
                    self.set_focus(restore, ring && self.focus.keyboard());
                }
            }
        }
    }

    /// What the top modal focuses: its `El::autofocus` control, else under `Cx::web_focus` its
    /// first text field and otherwise its first Tab stop, else the modal itself, as a web dialog
    /// focuses its content (Kobalte's `onOpenAutoFocus` in OpenCode's `Dialog`).
    fn modal_initial_focus(&self) -> Option<String> {
        let i = self.active_modal()?;
        let inside = |j: usize| self.modal_member(j) && j > i;
        let autofocus = (i..self.nodes.len()).find(|&j| inside(j) && self.nodes[j].focus.autofocus && self.can_focus(j));
        let first = if self.defaults.web_focus {
            (i..self.nodes.len()).find(|&j| inside(j) && self.nodes[j].input.is_some() && self.can_focus(j))
        } else {
            self.tab_order().first().copied()
        };
        Some(autofocus.or(first).map_or_else(|| self.nodes[i].path.clone(), |j| self.nodes[j].path.clone()))
    }
}
