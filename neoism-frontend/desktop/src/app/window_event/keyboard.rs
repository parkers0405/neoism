use neoism_window::event::{ElementState, KeyEvent, Modifiers};
use neoism_window::keyboard::{Key, NamedKey};
use neoism_window::platform::modifier_supplement::KeyEventExtModifierSupplement;
use neoism_window::window::WindowId;

use crate::app::scheduler::{TimerId, Topic};
use crate::app::Application;
use crate::router::routes::RoutePath;

/// Translate a winit `KeyEvent` into the shared crate's POD
/// [`IslandRenameKey`]. Returns `None` for keys the island's rename
/// field ignores (modifiers, function keys, arrow navigation, etc.) so
/// the caller can keep its original event-dispatch fall-through logic.
///
/// Only key-down events produce a rename key — repeat events count.
/// Key-up always returns `None`.
pub fn island_rename_key_from_winit(
    ev: &KeyEvent,
) -> Option<neoism_ui::widgets::island::IslandRenameKey> {
    use neoism_ui::widgets::island::IslandRenameKey;

    if ev.state != ElementState::Pressed {
        return None;
    }

    match &ev.logical_key {
        Key::Named(NamedKey::Escape) => Some(IslandRenameKey::Escape),
        Key::Named(NamedKey::Enter) => Some(IslandRenameKey::Enter),
        Key::Named(NamedKey::Backspace) => Some(IslandRenameKey::Backspace),
        _ => {
            // Prefer the modifier-aware text (handles shift, dead keys,
            // platform-specific composition) so the rename field reads
            // the same character the user sees on screen.
            let text = ev
                .text_with_all_modifiers()
                .or(ev.text.as_deref())
                .unwrap_or("");
            let ch = text.chars().next()?;
            if ch.is_control() {
                None
            } else {
                Some(IslandRenameKey::Character(ch))
            }
        }
    }
}

/// Translate a backend `ProgressReport` into the shared crate's POD
/// mirror so `Island::set_progress_report` can stay
/// frontend-agnostic. Mirrors `neoism_backend::event::ProgressReport` /
/// `ProgressState` variant-for-variant.
pub fn island_progress_report_from_backend(
    report: neoism_backend::event::ProgressReport,
) -> neoism_ui::widgets::island::ProgressReport {
    use neoism_backend::event::ProgressState as BackendState;
    use neoism_ui::widgets::island::{ProgressReport, ProgressState};
    let state = match report.state {
        BackendState::Remove => ProgressState::Remove,
        BackendState::Set => ProgressState::Set,
        BackendState::Error => ProgressState::Error,
        BackendState::Indeterminate => ProgressState::Indeterminate,
        BackendState::Pause => ProgressState::Pause,
    };
    ProgressReport {
        state,
        progress: report.progress,
    }
}

impl Application<'_> {
    pub(in crate::app) fn handle_modifiers_changed(
        &mut self,
        window_id: WindowId,
        modifiers: Modifiers,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        tracing::trace!(
            target: "neoism::input",
            ?window_id,
            modifiers = ?modifiers.state(),
            "modifiers changed"
        );
        route.window.screen.set_modifiers(modifiers);
    }

    pub(in crate::app) fn handle_keyboard_input(
        &mut self,
        window_id: WindowId,
        key_event: KeyEvent,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        let current_route = route.window.screen.ctx().current_route();
        let current_index = route.window.screen.ctx().current_index();
        let modifiers = route.window.screen.modifiers.state();
        tracing::trace!(
            target: "neoism::input",
            ?window_id,
            ?current_route,
            ?current_index,
            route_path = ?route.path,
            window_has_focus = route.window.winit_window.has_focus(),
            state = ?key_event.state,
            repeat = key_event.repeat,
            logical_key = ?key_event.logical_key,
            physical_key = ?key_event.physical_key,
            location = ?key_event.location,
            text = ?key_event.text,
            text_with_all_modifiers = ?key_event.text_with_all_modifiers(),
            ?modifiers,
            "keyboard input received"
        );

        let consumed_by_route =
            route.has_key_wait(&key_event, &mut self.router.clipboard);
        tracing::trace!(
            target: "neoism::input",
            ?window_id,
            ?current_route,
            consumed_by_route,
            route_path = ?route.path,
            "route key-wait check completed"
        );

        if consumed_by_route {
            if route.path != RoutePath::Terminal
                && key_event.state == ElementState::Released
            {
                // Scheduler must be cleaned after leave the terminal route
                self.scheduler.unschedule(TimerId::new(
                    Topic::Render,
                    route.window.screen.ctx().current_route(),
                ));
            }
            tracing::trace!(
                target: "neoism::input",
                ?window_id,
                ?current_route,
                route_path = ?route.path,
                "keyboard input consumed before screen processing"
            );
            return;
        }

        route.window.screen.context_manager.set_last_typing();
        tracing::trace!(
            target: "neoism::input",
            ?window_id,
            ?current_route,
            "dispatching keyboard input to screen"
        );
        route
            .window
            .screen
            .process_key_event(&key_event, &mut self.router.clipboard);
        // `process_key_event` used to call `self.render()` for
        // local-only keystrokes (VI mode, search input, hint
        // mode). Now it just marks `pending_update.set_dirty()`
        // through `mark_dirty`. Request a redraw so the next
        // vsync fires `RedrawRequested` — PTY-bound keystrokes
        // also flow through here but their render is idempotent
        // with the PTY-damage-driven redraw.
        route.request_redraw();
        tracing::trace!(
            target: "neoism::input",
            ?window_id,
            ?current_route,
            "requested redraw after keyboard input"
        );

        if key_event.state == ElementState::Released
            && self.config.terminal.hide_cursor_when_typing
        {
            route.window.set_cursor_visible(false);
            if route.window.screen.set_mouse_hidden_by_typing(true) {
                route.request_redraw();
            }
            tracing::trace!(
                target: "neoism::input",
                ?window_id,
                ?current_route,
                "hid cursor after key release"
            );
        }
    }
}
