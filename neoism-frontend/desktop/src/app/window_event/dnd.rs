use std::path::PathBuf;
use std::time::Instant;

use neoism_window::event_loop::{ActiveEventLoop, ControlFlow};
use neoism_window::window::WindowId;

use crate::app::Application;
use crate::router::routes::RoutePath;

impl Application<'_> {
    pub(in crate::app) fn handle_dropped_file(
        &mut self,
        window_id: WindowId,
        path: PathBuf,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        if route.window.screen.renderer.assistant.is_active() {
            return;
        }

        if let Some(agent) = route
            .window
            .screen
            .context_manager
            .current_mut()
            .neoism_agent
            .as_mut()
        {
            if agent.attach_path(&path) {
                route.window.screen.mark_dirty();
                return;
            }
        }

        let path: String = path.to_string_lossy().into();
        route.window.screen.paste(&(path + " "), true);
    }

    pub(in crate::app) fn handle_redraw_requested(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
    ) {
        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        route.mark_redraw_delivered();
        crate::app::freeze_watchdog::mark_redraw_delivered(window_id);
        crate::app::freeze_watchdog::mark_render_stage(
            window_id,
            "redraw_requested.enter",
        );
        tracing::trace!(
            target: "neoism::render_event",
            ?window_id,
            route_path = ?route.path,
            current_route = route.window.screen.ctx().current_route(),
            pending_dirty = route
                .window
                .screen
                .ctx()
                .current()
                .renderable_content
                .pending_update
                .is_dirty(),
            "RedrawRequested received"
        );
        route.begin_render();
        crate::app::freeze_watchdog::mark_render_stage(window_id, "route.begin_render");

        match route.path {
            RoutePath::Welcome => {
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "render_welcome.begin",
                );
                let winit_window = &route.window.winit_window;
                route
                    .window
                    .screen
                    .render_welcome(|| winit_window.pre_present_notify());
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "render_welcome.end",
                );
            }
            RoutePath::Terminal | RoutePath::ConfirmQuit => {
                if route.path == RoutePath::ConfirmQuit {
                    let dim = route.window.screen.ctx().current().dimension;
                    crate::router::routes::dialog::screen(
                        &mut route.window.screen.sugarloaf,
                        &dim,
                        "want to quit?",
                        "yes (y)",
                        "no (n)",
                    );
                }

                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "screen.render.begin",
                );
                let animation_dt = route.window.animation_frame_delta();
                let winit_window = &route.window.winit_window;
                let is_fullscreen = winit_window.fullscreen().is_some();
                let window_update =
                    route.window.screen.render(animation_dt, is_fullscreen, || {
                        winit_window.pre_present_notify()
                    });
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "screen.render.end",
                );
                tracing::trace!(
                    target: "neoism::render_event",
                    ?window_id,
                    current_route = route.window.screen.ctx().current_route(),
                    has_window_update = window_update.is_some(),
                    pending_dirty = route
                        .window
                        .screen
                        .ctx()
                        .current()
                        .renderable_content
                        .pending_update
                        .is_dirty(),
                    "screen render completed"
                );
                if let Some(window_update) = window_update {
                    use crate::context::renderable::{BackgroundState, WindowUpdate};
                    match window_update {
                        WindowUpdate::Background(bg_state) => {
                            // for now setting this as allowed because it fails on linux builds
                            #[allow(unused_variables)]
                            let bg_color = match bg_state {
                                BackgroundState::Set(color) => color,
                                BackgroundState::Reset => self.config.colors.background.1,
                            };

                            #[cfg(target_os = "macos")]
                            {
                                use neoism_window::platform::macos::WindowExtMacOS;

                                route.window.winit_window.set_background_color(
                                    bg_color.r, bg_color.g, bg_color.b, bg_color.a,
                                );
                            }

                            #[cfg(target_os = "windows")]
                            {
                                use neoism_window::platform::windows::WindowExtWindows;
                                route.window.winit_window.set_title_bar_background_color(
                                    bg_color.r, bg_color.g, bg_color.b, bg_color.a,
                                );
                            }
                        }
                    }
                }

                // Update IME cursor position after rendering to ensure it's current
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "ime_cursor_update.begin",
                );
                route
                    .window
                    .screen
                    .update_ime_cursor_position_if_needed(&route.window.winit_window);
                crate::app::freeze_watchdog::mark_render_stage(
                    window_id,
                    "ime_cursor_update.end",
                );
            }
        }

        route.window.finish_frame_cadence(Instant::now());

        // let duration = start.elapsed();
        // println!("Time elapsed in render() is: {:?}", duration);
        // }

        // Game mode = unlocked framerate, so keep the event loop
        // spinning. Every other case is vsync-paced: a
        // `request_redraw` tells winit to deliver
        // `RedrawRequested` at the next platform vsync, and the
        // OS parks the thread until that event arrives. Busy-
        // polling between vsyncs here would burn CPU without
        // delivering more frames.
        if self.config.renderer.strategy.is_game() {
            route.request_redraw();
            event_loop.set_control_flow(ControlFlow::Poll);
        } else {
            // Continuous animation redraws are requested from
            // `about_to_wait`, matching Neovide's event-loop
            // phase ordering. Requesting the next Wayland
            // frame callback from inside `RedrawRequested`
            // can land too early and shows up as every-other-
            // vblank cadence on high-refresh displays.
            event_loop.set_control_flow(ControlFlow::Wait);
        }
        crate::app::freeze_watchdog::mark_render_done(window_id);
    }
}
