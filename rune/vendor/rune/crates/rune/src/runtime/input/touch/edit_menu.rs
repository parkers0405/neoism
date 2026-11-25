//! Handoff between the Rune input model and UIKit's edit menu. The path is a lease on the
//! original field; a menu action must not edit whichever tab happens to be focused later.
use super::*;
use winit::platform::ios::{EditAction, WindowExtIOS};

impl<S: 'static> Runtime<S> {
    pub(crate) fn dismiss_touch_edit_menu(&mut self, window: &winit::window::Window) {
        if self.touch.edit_menu.take().is_some() { window.dismiss_edit_menu(); }
    }

    pub(crate) fn sync_touch_edit_menu(&mut self, window: &winit::window::Window) {
        let Some((path, _)) = &self.touch.edit_menu else { return };
        if !window.has_focus() || !self.valid_touch_edit_field(path) {
            self.dismiss_touch_edit_menu(window);
            return;
        }
        let Some(point) = self.touch.edit_menu.as_mut().and_then(|(_, point)| point.take()) else { return };
        let Some(edit) = self.focused_input().map(|input| &input.edit) else { return };
        let (start, end) = edit.range();
        let selected = start != end;
        let select_all = !edit.text.is_empty() && (start != 0 || end != edit.text.len());
        window.present_edit_menu(point, selected, select_all);
    }

    fn valid_touch_edit_field(&self, path: &str) -> bool {
        self.focus.path() == Some(path) && self.live_index(path).is_some_and(|i| {
            self.allowed(i) && self.touch_edit_field(i)
                && self.nodes[i].input.as_ref().is_some_and(|input| input.edit.preedit().is_none())
        })
    }

    pub(crate) fn touch_edit_action(&mut self, action: EditAction) {
        let Some((path, None)) = &self.touch.edit_menu else { return };
        if !self.valid_touch_edit_field(path) { return; }
        let path = path.clone();
        let command = match action {
            EditAction::Paste(text) => {
                if !text.is_empty() { self.paste_result(Some(path), crate::ClipboardContent::new(text)); }
                return;
            }
            EditAction::Cut => 'x',
            EditAction::Copy => 'c',
            EditAction::SelectAll => 'a',
        };
        self.queue.push(Action::TouchEditCommand(path, command));
    }

    pub(in crate::runtime) fn run_touch_edit_command(&mut self, path: &str, command: char, tx: &mut Tx) -> bool {
        if !self.valid_touch_edit_field(path) { return false; }
        let ev = KeyEvent::from(crate::Key::Command(command));
        let layout = self.focused_layout();
        self.with_focused(|edit| edit.key(&ev, layout.as_deref()), tx)
    }
}