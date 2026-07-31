use neoism_ui::user_event_policy::{
    focus_regained, should_unhide_cursor_on_mouse_activity,
};
use neoism_window::window::WindowId;

use crate::app::Application;

impl Application<'_> {
    pub(in crate::app) fn handle_focused(&mut self, window_id: WindowId, focused: bool) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        if should_unhide_cursor_on_mouse_activity(
            self.config.terminal.hide_cursor_when_typing,
        ) {
            route.window.set_cursor_visible(true);
            if route.window.screen.set_mouse_hidden_by_typing(false) {
                route.request_redraw();
            }
        }

        let was_focused = route.window.is_focused;
        route.window.is_focused = focused;

        if focus_regained(was_focused, focused) {
            route.request_redraw();
        }

        route.window.screen.on_focus_change(focused);
    }
}
