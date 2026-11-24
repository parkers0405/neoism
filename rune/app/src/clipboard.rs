//! Copy and paste chords beyond the text fields' own Ctrl+C / Ctrl+V. Hyprland users bind
//! Super+C / Super+V system-wide (the original honoured them too): Super+V pastes into the
//! focused field, else the editor or the terminal; Super+C copies a field's selection, else the
//! transcript's.

use crate::app::App;
use rune::{TextEdit, Tx};

impl App {
    pub(crate) fn accepts_chat_image_paste(&self) -> bool {
        self.focus == crate::app::Focus::Content
            && matches!(self.active_tab().kind, crate::tabs::TabKind::Agent { .. })
            && !self.has_prompt()
    }

    pub(crate) fn paste_window_content(&mut self, content: &rune::ClipboardContent) {
        // Clicking the chat transcript can leave the composer without field focus.
        if self.accepts_chat_image_paste() && self.paste_image(content) {
            self.composer_changed();
        } else if !content.text().is_empty() {
            self.paste_content(content.text());
        }
    }

    /// The text field that holds the keyboard, if any: an overlay's query first, then the
    /// active tab's composer.
    pub fn focused_field_mut(&mut self) -> Option<&mut TextEdit> {
        if self.workflows.is_some() { return self.workflows.as_mut().map(|panel| &mut panel.query); }
        if self.git_panel.open && self.focus == crate::app::Focus::Details && self.git_panel.message.focused {
            return Some(&mut self.git_panel.message);
        }
        if self.project_add.is_some() { return self.project_add.as_mut().map(|add| &mut add.query); }
        if self.focus == crate::app::Focus::Sidebar && self.conv_query.focused && self.project_menu.is_none() {
            return Some(&mut self.conv_query);
        }
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
            // An image into the agent composer attaches (`attach_clipboard_image`).
            let field = a.focused_field_mut().map(|f| f as *const TextEdit);
            let composing = a.tabs.get(a.active).is_some_and(|t| field == Some(&t.draft as *const TextEdit));
            if composing && a.paste_composer(&content) { return; }
            match a.focused_field_mut() {
                Some(field) => {
                    if content.text().is_empty() { return; }
                    field.paste(content.text());
                    if composing { a.composer_changed(); }
                }
                None => a.paste_window_content(content),
            }
        });
    }

    /// Super+C: the focused field's selection, else a Visual selection in the editor (yanked,
    /// as `y` would), else the transcript's.
    pub fn copy_chord(&mut self, tx: &mut Tx) -> bool {
        let from_field = self.focused_field_mut().map(|f| f.selected().to_owned()).filter(|t| !t.is_empty());
        let text = from_field.or_else(|| self.editor_yank_visual()).or_else(|| self.selection.copy.selected.clone());
        if let Some(text) = text { self.clipboard = Some(text.clone()); tx.copy(text); true } else { false }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::TabKind;

    #[test]
    fn pasting_a_chat_image_without_composer_focus_attaches_it_to_the_draft() {
        let mut app = App::new();
        app.open_tab(TabKind::Agent { session: None });
        app.tabs[app.active].draft.focused = false;
        let tab = app.active_tab().id;
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 0, 255]))
            .write_to(&mut png, image::ImageFormat::Png).unwrap();
        let image = rune::ClipboardImage::from_png(png.into_inner()).unwrap();
        let content = rune::ClipboardContent::default().with_image(image.clone());
        let mut rt = rune::Runtime::new(app, crate::ui::chrome::root, (900.0, 600.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(!rt.has_focus());
        rt.paste_content(content);
        rt.frame(0.0);
        let attachment = &rt.state.attach.pending[&tab][0];
        assert_eq!(attachment.mime, "image/png");
        assert_eq!(attachment.bytes.as_ref(), image.png());
    }
}
