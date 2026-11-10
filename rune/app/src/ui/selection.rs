//! Press, hold and drag over read-only text selects it; letting go copies it (the original's
//! `end_selection`, whose text the host writes to the clipboard). Rune's `selectable` texts
//! and `selection_group` do the selecting; this follows one surface's drag through
//! `on_selection_change` and copies once, on release. The agent transcript and the LSP hover
//! card both use it.

use rune::{Color, SelectionChange, Tx};

/// The read-only selection highlight (`view/markdown.rs`: the accent at 22%).
pub fn highlight(accent: Color) -> Color { accent.alpha(0.22) }

#[derive(Default)]
pub struct CopyOnRelease {
    dragging: bool,
    /// The surface's current selection, for an explicit copy (Super+C / Ctrl+C).
    pub selected: Option<String>,
}

impl CopyOnRelease {
    /// A press is dragging out a selection on this surface.
    pub fn dragging(&self) -> bool { self.dragging }

    /// Feed an `on_selection_change`. When the drag that made a selection lets go, its text
    /// goes to the clipboard and comes back here; a code-made selection (Cmd+A) copies nothing.
    pub fn change(&mut self, change: &SelectionChange, tx: &mut Tx) -> Option<String> {
        self.selected = change.selection().map(|s| s.text().to_owned()).filter(|t| !t.trim().is_empty());
        if change.dragging() {
            self.dragging = true;
            return None;
        }
        let released = std::mem::take(&mut self.dragging);
        let text = change.selection().map(|s| s.text().to_owned()).filter(|t| released && !t.trim().is_empty())?;
        tx.copy(text.clone());
        if std::env::var_os("NEOISM_RUNE_TRACE_COPY").is_some() { eprintln!("copied {text:?}"); }
        Some(text)
    }
}
