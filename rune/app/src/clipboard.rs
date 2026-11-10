//! Copy and paste chords beyond the text fields' own Ctrl+C / Ctrl+V. Hyprland users bind
//! Super+C / Super+V system-wide (the original honoured them too): Super+V pastes into the
//! focused field, else the editor or the terminal; Super+C copies a field's selection, else the
//! transcript's.

use crate::app::App;
use rune::{TextEdit, Tx};

impl App {
    /// The text field that holds the keyboard, if any: an overlay's query first, then the
    /// active tab's composer.
    pub fn focused_field_mut(&mut self) -> Option<&mut TextEdit> {
        enum Field { Palette, Finder, Composer, Filter(u64), Draft(usize) }
        let tab = self.active_tab().id;
        let field = if self.picker.as_ref().is_some_and(|p| p.query.focused) { Field::Palette }
            else if self.finder.as_ref().is_some_and(|f| f.query.focused) { Field::Finder }
            else if self.composer.picker.as_ref().is_some_and(|p| p.query.focused) { Field::Composer }
            else if self.terminals.get(&tab).and_then(|t| t.filter.as_ref()).is_some_and(|(_, f)| f.focused) { Field::Filter(tab) }
            else if self.tabs.get(self.active).is_some_and(|t| t.draft.focused) { Field::Draft(self.active) }
            else { return None };
        match field {
            Field::Palette => self.picker.as_mut().map(|p| &mut p.query),
            Field::Finder => self.finder.as_mut().map(|f| &mut f.query),
            Field::Composer => self.composer.picker.as_mut().map(|p| &mut p.query),
            Field::Filter(tab) => self.terminals.get_mut(&tab).and_then(|t| t.filter.as_mut()).map(|(_, f)| f),
            Field::Draft(i) => self.tabs.get_mut(i).map(|t| &mut t.draft),
        }
    }

    /// Super+V: the clipboard's text into the focused field, else the editor or terminal.
    pub fn paste_chord(&mut self, tx: &mut Tx) {
        tx.read_clipboard(|a: &mut App, content, _tx| {
            let text = content.text().to_owned();
            if text.is_empty() { return; }
            match a.focused_field_mut() {
                Some(field) => { field.paste(&text); }
                None => a.paste_content(&text),
            }
        });
    }

    /// Super+C: the focused field's selection, else a Visual selection in the editor (yanked,
    /// as `y` would), else the transcript's.
    pub fn copy_chord(&mut self, tx: &mut Tx) {
        let from_field = self.focused_field_mut().map(|f| f.selected().to_owned()).filter(|t| !t.is_empty());
        let text = from_field.or_else(|| self.editor_yank_visual()).or_else(|| self.selection.copy.selected.clone());
        if let Some(text) = text { self.clipboard = Some(text.clone()); tx.copy(text); }
    }

    /// Shift+Delete: cut the focused field's selection.
    pub fn cut_chord(&mut self, tx: &mut Tx) {
        let Some(field) = self.focused_field_mut() else { return };
        let text = field.selected().to_owned();
        if text.is_empty() { return; }
        field.paste("");
        tx.copy(text);
    }
}
