use neoism_window::event_loop::ActiveEventLoop;
use neoism_window::window::WindowId;

use crate::app::Application;

impl Application<'_> {
    pub(in crate::app) fn handle_destroyed(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
    ) {
        self.router.unbind_native_window(window_id);
        self.router.routes.remove(&window_id);
        self.window_sessions.remove(&window_id);
        crate::app::freeze_watchdog::unregister_window(window_id);

        if self.router.routes.is_empty() {
            event_loop.exit();
        }
    }

    pub(in crate::app) fn handle_close_requested(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
    ) {
        if !self.router.routes.contains_key(&window_id) {
            return;
        }

        // macOS: Cmd+Q quit confirmation is handled by
        // `applicationShouldTerminate` in neoism-window.
        // Windows: per-window close confirmation is handled
        // by `MessageBoxW` in neoism-window's WM_CLOSE handler
        // (see `set_confirm_before_quit` plumbing).
        // Either way, by the time we see `CloseRequested`
        // the user has already confirmed — just close.
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            if let Some(daemon_window_id) =
                self.router.daemon_window_for_native(window_id)
            {
                self.send_window_message(
                    window_id,
                    neoism_protocol::workspace::WorkspaceClientMessage::RequestCloseWindow {
                        window_id: daemon_window_id.to_string(),
                    },
                );
            }
            if self.router.routes.len() <= 1 {
                event_loop.exit();
                return;
            }
            self.router.unbind_native_window(window_id);
            self.router.routes.remove(&window_id);
            self.window_sessions.remove(&window_id);
            crate::app::freeze_watchdog::unregister_window(window_id);
            if self.router.routes.is_empty() {
                event_loop.exit();
            }
            return;
        }

        if self.config.confirm_before_quit {
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.confirm_quit();
                route.request_redraw();
            }
            return;
        } else {
            if let Some(daemon_window_id) =
                self.router.daemon_window_for_native(window_id)
            {
                self.send_window_message(
                    window_id,
                    neoism_protocol::workspace::WorkspaceClientMessage::RequestCloseWindow {
                        window_id: daemon_window_id.to_string(),
                    },
                );
            }
            if self.router.routes.len() <= 1 {
                event_loop.exit();
                return;
            }
            self.router.unbind_native_window(window_id);
            self.router.routes.remove(&window_id);
            self.window_sessions.remove(&window_id);
            crate::app::freeze_watchdog::unregister_window(window_id);
        }

        if self.router.routes.is_empty() {
            event_loop.exit();
        }
    }
}
