use neoism_window::event::Ime;
use neoism_window::window::WindowId;

use crate::app::ime::Preedit;
use crate::app::Application;

impl Application<'_> {
    pub(in crate::app) fn handle_ime(&mut self, window_id: WindowId, ime: Ime) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        #[cfg(feature = "servo-artifacts")]
        if route.window.screen.html_artifact_ime(&ime) {
            route.request_redraw();
            return;
        }

        if route.window.screen.renderer.assistant.is_active() {
            return;
        }

        match ime {
            Ime::Commit(text) => {
                // Don't use bracketed paste for single char input.
                route.window.screen.paste(&text, text.chars().count() > 1);
            }
            Ime::Preedit(text, cursor_offset) => {
                let preedit = if text.is_empty() {
                    None
                } else {
                    Some(Preedit::new(text, cursor_offset.map(|offset| offset.0)))
                };

                if route.window.screen.context_manager.current().ime.preedit()
                    != preedit.as_ref()
                {
                    route
                        .window
                        .screen
                        .context_manager
                        .current_mut()
                        .ime
                        .set_preedit(preedit);
                    route.request_redraw();
                }
            }
            Ime::Enabled => {
                route
                    .window
                    .screen
                    .context_manager
                    .current_mut()
                    .ime
                    .set_enabled(true);
            }
            Ime::Disabled => {
                route
                    .window
                    .screen
                    .context_manager
                    .current_mut()
                    .ime
                    .set_enabled(false);
            }
        }
    }
}
