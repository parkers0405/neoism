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

        #[cfg(feature = "cef-artifacts")]
        if route.window.screen.html_artifact_ime(&ime) {
            route.request_redraw();
            return;
        }

        if route.window.screen.renderer.assistant.is_active() {
            return;
        }

        #[cfg(feature = "neoui")]
        if route.window.screen.renderer.settings.is_active()
            && !route.window.screen.renderer.modal.is_active()
        {
            use neoism_ui::neoui::input::InputEvent;
            let input = match &ime {
                Ime::Commit(text) => Some(InputEvent::ImeCommit(text.clone())),
                Ime::Preedit(text, selection) => Some(InputEvent::ImePreedit {
                    text: text.clone(),
                    selection: selection.map(|(start, end)| start..end),
                }),
                Ime::Disabled => Some(InputEvent::ImeCancel),
                Ime::Enabled => None,
            };
            if let Some(input) = input {
                if route.window.screen.renderer.settings.handle_edit_event(&input).is_none() {
                    if let InputEvent::ImeCommit(text) = input {
                        for ch in text.chars().filter(|ch| !ch.is_control()) {
                            route.window.screen.renderer.settings.input_char(ch);
                        }
                    }
                }
            }
            route.request_overlay_redraw();
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
