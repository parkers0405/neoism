use neoism_ui::user_event_policy::{
    focus_regained, should_unhide_cursor_on_mouse_activity,
};
use neoism_window::event::ElementState;
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

        if !focused {
            // Platforms are not required to deliver mouse-up after focus or
            // pointer-capture loss. Cancel gestures rather than allowing a
            // later unrelated release to commit a reorder/detach.
            route.window.screen.mouse.left_button_state = ElementState::Released;
            route.window.screen.mouse.middle_button_state = ElementState::Released;
            route.window.screen.mouse.right_button_state = ElementState::Released;
            if route.window.screen.cancel_island_drag() {
                route.window.set_cursor(neoism_window::window::CursorIcon::Default);
                route.request_redraw();
            }
            if route.window.screen.cancel_buffer_tab_drag() {
                route.window.set_cursor(neoism_window::window::CursorIcon::Default);
                route.request_redraw();
            }
        }

        if focus_regained(was_focused, focused) {
            route.request_redraw();
        }

        route.window.screen.on_focus_change(focused);
    }
}
